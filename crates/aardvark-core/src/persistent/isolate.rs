use std::sync::Arc;

use crate::bundle::BundleFingerprint;
use crate::config::PyRuntimeConfig;
use crate::error::{PyRunnerError, Result};
use crate::invocation::InvocationDescriptor;
use crate::persistent::{BundleArtifact, InlinePythonOptions};
use crate::runtime::{PyRuntime, RuntimeCleanupMode};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use crate::strategy::{
    rawctx_spec_json_for_descriptor, DefaultInvocationStrategy, JsonInput, JsonInvocationStrategy,
    PyInvocationStrategy, RawCtxInput, RawCtxInvocationStrategy,
};
use once_cell::sync::OnceCell;
use serde_json::Value as JsonValue;

/// Cleanup behaviour applied after each invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CleanupMode {
    /// Remove loaded Python modules, clear shared buffers, and reset scratch files.
    #[default]
    Full,
    /// Keep Python modules hot and clear only transient shared buffers.
    SharedBuffersOnly,
    /// Keep all isolate state after each invocation.
    None,
}

/// Configuration applied when constructing a [`PythonIsolate`].
#[derive(Clone, Default)]
pub struct IsolateConfig {
    pub runtime: PyRuntimeConfig,
    pub cleanup: CleanupMode,
}

/// Handle referencing a loaded bundle artifact.
#[derive(Clone)]
pub struct BundleHandle {
    artifact: Arc<BundleArtifact>,
}

impl BundleHandle {
    /// Creates a bundle handle from a shared artifact.
    pub fn from_artifact(artifact: Arc<BundleArtifact>) -> Self {
        Self { artifact }
    }

    /// Returns a session prepared from the bundle's descriptor template.
    pub fn prepare_default_handler(&self) -> HandlerSession {
        self.prepare_handler(None)
    }

    /// Builds a handler session using an optional descriptor override.
    pub fn prepare_handler(&self, descriptor: Option<InvocationDescriptor>) -> HandlerSession {
        let mut descriptor = descriptor.unwrap_or_else(|| self.artifact.default_descriptor());
        self.artifact
            .apply_manifest_descriptor_defaults(&mut descriptor);
        HandlerSession {
            artifact: self.artifact.clone(),
            descriptor,
            rawctx_spec_json: OnceCell::new(),
            prepared_session: OnceCell::new(),
        }
    }

    pub(crate) fn artifact(&self) -> &Arc<BundleArtifact> {
        &self.artifact
    }
}

/// Prepared handler ready for repeated invocation.
pub struct HandlerSession {
    artifact: Arc<BundleArtifact>,
    descriptor: InvocationDescriptor,
    rawctx_spec_json: OnceCell<Option<Arc<String>>>,
    prepared_session: OnceCell<Arc<PySession>>,
}

impl HandlerSession {
    /// Returns the invocation descriptor for inspection.
    pub fn descriptor(&self) -> &InvocationDescriptor {
        &self.descriptor
    }

    /// Provides mutable access to the underlying descriptor for fine-grained changes.
    pub fn descriptor_mut(&mut self) -> &mut InvocationDescriptor {
        let _ = self.rawctx_spec_json.take();
        let _ = self.prepared_session.take();
        &mut self.descriptor
    }

    /// Executes the handler using the default invocation strategy.
    pub fn invoke(&self, isolate: &mut PythonIsolate) -> Result<crate::ExecutionOutcome> {
        let mut strategy = DefaultInvocationStrategy;
        isolate.invoke_with_strategy(self, &mut strategy)
    }

    /// Executes the handler using JSON adapters.
    pub fn invoke_json(
        &self,
        isolate: &mut PythonIsolate,
        input: Option<JsonValue>,
    ) -> Result<crate::ExecutionOutcome> {
        let mut strategy = JsonInvocationStrategy::new(input);
        isolate.invoke_with_strategy(self, &mut strategy)
    }

    /// Executes the handler using a prepared JSON adapter input.
    pub fn invoke_json_input(
        &self,
        isolate: &mut PythonIsolate,
        input: Option<JsonInput>,
    ) -> Result<crate::ExecutionOutcome> {
        let mut strategy = JsonInvocationStrategy::with_input(input);
        isolate.invoke_with_strategy(self, &mut strategy)
    }

    /// Executes the handler using RawCtx adapters.
    pub fn invoke_rawctx(
        &self,
        isolate: &mut PythonIsolate,
        inputs: Vec<RawCtxInput>,
    ) -> Result<crate::ExecutionOutcome> {
        let mut strategy = RawCtxInvocationStrategy::new(inputs);
        isolate.invoke_with_strategy(self, &mut strategy)
    }

    /// Executes the handler asynchronously using the default strategy.
    pub async fn invoke_async(
        &self,
        isolate: &mut PythonIsolate,
    ) -> Result<crate::ExecutionOutcome> {
        self.invoke(isolate)
    }

    pub(crate) fn artifact(&self) -> &Arc<BundleArtifact> {
        &self.artifact
    }

    pub(crate) fn descriptor_cloned(&self) -> InvocationDescriptor {
        self.descriptor.clone()
    }

    pub(crate) fn rawctx_spec_json(&self) -> Result<Option<Arc<String>>> {
        Ok(self
            .rawctx_spec_json
            .get_or_try_init(|| rawctx_spec_json_for_descriptor(&self.descriptor))?
            .clone())
    }
}

/// Persistent Pyodide isolate keeping the interpreter hot between invocations.
pub struct PythonIsolate {
    runtime: PyRuntime,
    cleanup: CleanupMode,
    loaded_fingerprint: Option<BundleFingerprint>,
    current_artifact: Option<Arc<BundleArtifact>>,
}

impl PythonIsolate {
    pub fn new(config: IsolateConfig) -> Result<Self> {
        let runtime = PyRuntime::new(config.runtime.clone())?;
        Ok(Self {
            runtime,
            cleanup: config.cleanup,
            loaded_fingerprint: None,
            current_artifact: None,
        })
    }

    /// Ensures the isolate has materialised the bundle requested by `handle`.
    pub fn load_bundle(&mut self, handle: &BundleHandle) -> Result<()> {
        let fingerprint = handle.artifact().fingerprint();
        if self.loaded_fingerprint == Some(fingerprint) {
            self.current_artifact = Some(handle.artifact().clone());
            return Ok(());
        }
        self.materialise_bundle(handle.artifact())?;
        self.loaded_fingerprint = Some(fingerprint);
        self.current_artifact = Some(handle.artifact().clone());
        Ok(())
    }

    /// Invokes the handler with a caller-supplied strategy.
    pub fn invoke_with_strategy<S: PyInvocationStrategy>(
        &mut self,
        handler: &HandlerSession,
        strategy: &mut S,
    ) -> Result<crate::ExecutionOutcome> {
        self.ensure_artifact(handler.artifact())?;
        let session = self.prepare_handler_session(handler)?;
        self.runtime.run_session_with_strategy_and_cleanup(
            session.as_ref(),
            strategy,
            self.cleanup.into(),
        )
    }

    /// Prepares handler-specific caches without executing the user handler.
    pub fn prewarm_handler(&mut self, handler: &HandlerSession) -> Result<()> {
        self.ensure_artifact(handler.artifact())?;
        let session = self.prepare_handler_session(handler)?;
        let language = session
            .descriptor()
            .language
            .unwrap_or_else(|| self.runtime_language().unwrap_or(RuntimeLanguage::Python));
        if language == RuntimeLanguage::Python {
            let entrypoint = session.entrypoint().to_owned();
            self.runtime
                .js_runtime()
                .prewarm_python_entrypoint(&entrypoint)?;
            RawCtxInvocationStrategy::prewarm_python_handler(&session, self.runtime.js_runtime())?;
        }
        Ok(())
    }

    fn prepare_handler_session(
        &mut self,
        handler: &HandlerSession,
    ) -> Result<Arc<crate::session::PySession>> {
        let bundle = handler.artifact().bundle();
        let mut descriptor = handler.descriptor_cloned();
        let language = descriptor
            .language
            .unwrap_or_else(|| handler.artifact().language());
        descriptor.language = Some(language);

        if language == RuntimeLanguage::Python
            && self.loaded_fingerprint == Some(handler.artifact().fingerprint())
        {
            // BundlePool has already materialised this artifact. For Python hot calls,
            // keep package/policy/descriptor publication in load/prewarm and build the
            // handler session once from the prepared descriptor.
            return Ok(handler
                .prepared_session
                .get_or_try_init(|| {
                    descriptor
                        .validate()
                        .map_err(|err| PyRunnerError::Descriptor(err.to_string()))?;
                    let rawctx_spec_json = handler.rawctx_spec_json()?;
                    Ok(Arc::new(PySession::new_with_rawctx_spec_json(
                        bundle,
                        descriptor,
                        rawctx_spec_json,
                    )))
                })?
                .clone());
        }

        if handler.artifact().manifest().is_some() {
            let (session, _) = self
                .runtime
                .prepare_session_with_manifest_and_descriptor(bundle, descriptor)?;
            Ok(Arc::new(session))
        } else {
            let session = self
                .runtime
                .prepare_session_with_descriptor(bundle, descriptor)?;
            Ok(Arc::new(session))
        }
    }

    fn ensure_artifact(&mut self, artifact: &Arc<BundleArtifact>) -> Result<()> {
        let fingerprint = artifact.fingerprint();
        if self.loaded_fingerprint == Some(fingerprint) {
            return Ok(());
        }
        self.materialise_bundle(artifact)?;
        self.loaded_fingerprint = Some(fingerprint);
        self.current_artifact = Some(artifact.clone());
        Ok(())
    }

    fn materialise_bundle(&mut self, artifact: &Arc<BundleArtifact>) -> Result<()> {
        let descriptor = artifact.default_descriptor();
        if artifact.manifest().is_some() {
            let bundle = artifact.bundle();
            let (_session, _) = self
                .runtime
                .prepare_session_with_manifest_and_descriptor(bundle, descriptor)?;
        } else {
            let bundle = artifact.bundle();
            let _ = self
                .runtime
                .prepare_session_with_descriptor(bundle, descriptor)?;
        }
        Ok(())
    }

    pub fn cleanup_mode(&self) -> CleanupMode {
        self.cleanup
    }

    pub fn runtime_language(&self) -> Option<RuntimeLanguage> {
        self.current_artifact
            .as_ref()
            .map(|artifact| artifact.language())
    }

    pub fn runtime(&mut self) -> &mut PyRuntime {
        &mut self.runtime
    }

    /// Executes inline Python code using default manifest options and entrypoint.
    pub fn run_inline_python(
        &mut self,
        code: &str,
        entrypoint: &str,
    ) -> Result<crate::ExecutionOutcome> {
        let options = InlinePythonOptions {
            entrypoint: Some(entrypoint.to_owned()),
            ..InlinePythonOptions::default()
        };
        self.run_inline_python_with_options(code, options)
    }

    /// Executes inline Python code with manifest-style configuration options.
    pub fn run_inline_python_with_options(
        &mut self,
        code: &str,
        options: InlinePythonOptions,
    ) -> Result<crate::ExecutionOutcome> {
        let (bundle, entrypoint) = options.build_bundle(code)?;
        let descriptor = InvocationDescriptor::new(entrypoint);
        let (session, _) = self
            .runtime
            .prepare_session_with_manifest_and_descriptor(bundle, descriptor)?;
        self.runtime.run_session(&session)
    }
}

impl From<CleanupMode> for RuntimeCleanupMode {
    fn from(value: CleanupMode) -> Self {
        match value {
            CleanupMode::Full => RuntimeCleanupMode::Full,
            CleanupMode::SharedBuffersOnly => RuntimeCleanupMode::SharedBuffersOnly,
            CleanupMode::None => RuntimeCleanupMode::None,
        }
    }
}
