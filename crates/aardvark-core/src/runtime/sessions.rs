use super::*;

impl AardvarkRuntime {
    /// Creates a new runtime instance based on the provided configuration.
    pub fn new(config: PyRuntimeConfig) -> Result<Self> {
        let engine = create_engine(config.default_language, &config)?;
        Ok(Self {
            config,
            engine: Some(engine),
            runtime_id: None,
            warm_restored: false,
            engine_generation: 1,
            pending_reset_summary: None,
            environment_ready: false,
            current_bundle: None,
        })
    }

    /// Creates a runtime configured for a specific bundle.
    ///
    /// This applies manifest requirements that must be known before isolate
    /// creation, such as `runtime.pyodide.profile`, then delegates to
    /// [`AardvarkRuntime::new`].
    pub fn new_for_bundle(mut config: PyRuntimeConfig, bundle: &Bundle) -> Result<Self> {
        let manifest = bundle.manifest()?;
        config.apply_bundle_manifest(manifest.as_ref())?;
        Self::new(config)
    }

    /// Prepares a session from a bundle and entrypoint string using default limits.
    pub fn prepare_session(&mut self, bundle: Bundle, entrypoint: &str) -> Result<PySession> {
        let descriptor = InvocationDescriptor::trivial(entrypoint);
        self.prepare_session_with_descriptor(bundle, descriptor)
    }

    /// Prepares a session using a host-supplied descriptor, allowing fine-grained
    /// control over limits, language selection, and expected inputs/outputs.
    pub fn prepare_session_with_descriptor(
        &mut self,
        bundle: Bundle,
        descriptor: InvocationDescriptor,
    ) -> Result<PySession> {
        let _fingerprint = bundle.fingerprint();
        let (session, _, _) = self.prepare_session_core(bundle, descriptor, None, _fingerprint)?;
        Ok(session)
    }

    pub fn prepare_session_with_manifest(
        &mut self,
        bundle: Bundle,
    ) -> Result<(PySession, Option<BundleManifest>)> {
        self.prepare_session_with_manifest_internal(bundle, None)
    }

    /// Prepares a session by combining manifest metadata with a custom descriptor.
    pub fn prepare_session_with_manifest_and_descriptor(
        &mut self,
        bundle: Bundle,
        descriptor: InvocationDescriptor,
    ) -> Result<(PySession, Option<BundleManifest>)> {
        self.prepare_session_with_manifest_internal(bundle, Some(descriptor))
    }

    fn prepare_session_with_manifest_internal(
        &mut self,
        bundle: Bundle,
        descriptor_override: Option<InvocationDescriptor>,
    ) -> Result<(PySession, Option<BundleManifest>)> {
        let manifest = bundle.manifest()?;
        self.validate_manifest_pyodide_profile(manifest.as_ref())?;
        let entrypoint = manifest
            .as_ref()
            .map(|m| m.entrypoint().to_owned())
            .unwrap_or_else(|| "main:handler".to_string());

        let mut descriptor = match descriptor_override {
            Some(desc) if desc.entrypoint().trim().is_empty() => {
                InvocationDescriptor::new(entrypoint.clone())
            }
            Some(desc) => desc,
            None => InvocationDescriptor::new(entrypoint.clone()),
        };

        if descriptor.language.is_none() {
            if let Some(runtime) = manifest
                .as_ref()
                .and_then(|m| m.runtime.as_ref())
                .and_then(|rt| rt.language)
            {
                descriptor.language = Some(runtime);
            }
        }

        let mut filesystem_policy = FilesystemPolicy::default();
        let mut manifest_host_capabilities: Option<Vec<String>> = None;
        if let Some(manifest) = &manifest {
            if let Some(resources) = manifest.resources() {
                if let Some(cpu_limit) = resources.cpu.as_ref().and_then(|cpu| cpu.default_limit_ms)
                {
                    descriptor.limits.cpu_ms = Some(cpu_limit);
                }
                if let Some(filesystem) = &resources.filesystem {
                    filesystem_policy = FilesystemPolicy::from_manifest(filesystem);
                }
                if resources.host_capabilities.is_empty() {
                    manifest_host_capabilities = None;
                } else {
                    manifest_host_capabilities = Some(resources.host_capabilities.clone());
                }
            }
        }

        let _fingerprint = bundle.fingerprint();
        let (session, manifest, _bundle_changed) =
            self.prepare_session_core(bundle, descriptor, manifest, _fingerprint)?;

        {
            let js = self.engine_mut().js_mut();
            js.set_filesystem_policy(
                filesystem_policy.mode_config(),
                filesystem_policy.quota_bytes,
            )?;
        }
        self.apply_host_capabilities(manifest_host_capabilities.as_deref())?;

        if let Some(manifest) = manifest.as_ref() {
            if let Some(resources) = manifest.resources() {
                if let Some(network) = &resources.network {
                    self.engine_mut()
                        .js_mut()
                        .set_manifest_network_policy(network);
                }
            }
            if !manifest.packages().is_empty() {
                self.engine_mut().load_manifest_packages(manifest)?;
            }
        }

        Ok((session, manifest))
    }

    fn validate_manifest_pyodide_profile(&self, manifest: Option<&BundleManifest>) -> Result<()> {
        let Some(requested_profile) =
            manifest.and_then(BundleManifest::pyodide_distribution_profile)
        else {
            return Ok(());
        };
        let active_profile = self
            .config
            .pyodide_distribution_profile
            .as_deref()
            .unwrap_or(DEFAULT_PYODIDE_DISTRIBUTION_PROFILE);
        if requested_profile == active_profile {
            return Ok(());
        }

        Err(PyRunnerError::Validation(format!(
            "bundle manifest requests Pyodide distribution profile '{requested_profile}', but runtime was created with profile '{active_profile}'; construct the runtime with PyRuntime::new_for_bundle or select the profile before PyRuntime::new"
        )))
    }

    fn prepare_session_core(
        &mut self,
        bundle: Bundle,
        mut descriptor: InvocationDescriptor,
        manifest: Option<BundleManifest>,
        fingerprint: BundleFingerprint,
    ) -> Result<(PySession, Option<BundleManifest>, bool)> {
        let language = descriptor.language.unwrap_or(self.config.default_language);
        descriptor.language = Some(language);

        self.ensure_environment_ready(language)?;

        {
            let js = self.engine_mut().js_mut();
            js.set_filesystem_policy(FilesystemModeConfig::Read, None)?;
        }
        self.apply_host_capabilities(None)?;

        descriptor
            .validate()
            .map_err(|err| PyRunnerError::Descriptor(err.to_string()))?;

        info!(
            target: "aardvark::session",
            runtime_id = self.runtime_id_str(),
            entrypoint = descriptor.entrypoint(),
            inputs = descriptor.inputs.len(),
            outputs = descriptor.outputs.len(),
            limits.wall_ms = descriptor.limits.wall_ms.unwrap_or(0),
            limits.heap_mb = descriptor.limits.heap_mb.unwrap_or(0),
            "descriptor accepted"
        );

        self.publish_manifest(&bundle)?;

        let bundle_changed = self
            .current_bundle
            .as_ref()
            .map(|state| state.fingerprint != fingerprint)
            .unwrap_or(true);
        if bundle_changed {
            let bundle_clone = bundle.clone();
            self.engine_mut().mount_bundle(&bundle_clone)?;
        }

        let session = PySession::new(bundle, descriptor);
        self.publish_descriptor(session.descriptor())?;
        self.current_bundle = Some(CurrentBundleState {
            fingerprint,
            language,
            pyodide_preload_imports: manifest
                .as_ref()
                .map(|manifest| manifest.pyodide_preload_imports().to_vec())
                .unwrap_or_default(),
        });

        Ok((session, manifest, bundle_changed))
    }

    /// Captures a warm state (snapshot + overlay) after packages are loaded.
    pub fn capture_warm_state(&mut self) -> Result<WarmState> {
        self.preload_current_bundle_imports()?;
        if let Some(hook) = self.config.hooks.before_warm_snapshot.clone() {
            hook(self)?;
        }
        let snapshot_bytes = {
            let bytes = self.engine_mut().js_mut().collect_snapshot()?;
            Arc::<[u8]>::from(bytes.into_boxed_slice())
        };
        let overlay = self.engine_mut().js_mut().export_overlay()?;
        let compatibility_fingerprint = self.pyodide_compatibility_fingerprint().map(str::to_owned);
        let state = if let Some(fingerprint) = compatibility_fingerprint.as_deref() {
            WarmState::new(snapshot_bytes, overlay, fingerprint)
        } else {
            WarmState::without_compatibility_fingerprint(snapshot_bytes, overlay)
        };
        self.config.warm_state = Some(state.clone());
        self.engine_mut().set_warm_state(Some(state.clone()))?;
        self.config
            .snapshot
            .store_cached_bytes(state.snapshot(), compatibility_fingerprint.as_deref());
        self.warm_restored = true;
        if let Some(hook) = self.config.hooks.after_warm_restore.clone() {
            hook(self)?;
        }
        Ok(state)
    }

    fn preload_current_bundle_imports(&mut self) -> Result<()> {
        let Some(state) = self.current_bundle.as_ref() else {
            return Ok(());
        };
        if state.language != RuntimeLanguage::Python || state.pyodide_preload_imports.is_empty() {
            return Ok(());
        }
        let imports = state.pyodide_preload_imports.clone();
        for module in imports {
            let literal = serde_json::to_string(module.as_str()).map_err(|err| {
                PyRunnerError::Execution(format!(
                    "failed to encode Pyodide preload import '{module}': {err}"
                ))
            })?;
            self.engine_mut()
                .js_mut()
                .run_python_snippet(&format!("__import__({literal})"))?;
        }
        Ok(())
    }
}
