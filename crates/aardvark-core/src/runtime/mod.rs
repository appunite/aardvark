//! Runtime coordination between the host and language-specific engines.

mod javascript;
mod python;

use crate::bundle::Bundle;
use crate::bundle_manifest::{BundleManifest, ManifestFilesystemMode, ManifestFilesystemResources};
use crate::config::{PyRuntimeConfig, ResetPolicy};
use crate::engine::{ExecutionOutput, FilesystemModeConfig, JsRuntime};
use crate::error::{PyRunnerError, Result};
use crate::invocation::{InvocationDescriptor, InvocationLimits};
use crate::outcome::{
    Diagnostics, ExecutionOutcome, FailureKind, FilesystemViolation, NetworkDeniedHost,
    NetworkHostContact, ResultPayload,
};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use crate::strategy::{
    DefaultInvocationStrategy, InvocationContext, JavaScriptInvocationStrategy,
    PyInvocationStrategy, StrategyResult,
};
use std::collections::HashSet;
use std::env;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;
use tracing::{info, info_span, warn};
use v8::{self, PinScope};

pub use javascript::JavaScriptEngine;
pub use python::PythonEngine;

pub type PyRuntime = AardvarkRuntime;

/// Primary host-facing runtime capable of executing bundles for multiple guest languages.
///
/// Instances are cheap to create but expensive to warm; consider pooling via
/// [`PyRuntimePool`](crate::PyRuntimePool) for throughput-sensitive workloads.
pub struct AardvarkRuntime {
    config: PyRuntimeConfig,
    engine: Option<Box<dyn LanguageEngine>>,
    runtime_id: Option<String>,
}

trait LanguageEngine {
    fn language(&self) -> RuntimeLanguage;
    fn js_mut(&mut self) -> &mut JsRuntime;
    fn prepare_environment(&mut self, config: &PyRuntimeConfig) -> Result<()>;
    fn load_manifest_packages(&mut self, manifest: &BundleManifest) -> Result<()>;
    fn mount_bundle(&mut self, bundle: &Bundle) -> Result<()>;
}

#[derive(Debug, Clone)]
struct FilesystemPolicy {
    mode: FilesystemMode,
    quota_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesystemMode {
    Read,
    ReadWrite,
}

impl Default for FilesystemPolicy {
    fn default() -> Self {
        Self {
            mode: FilesystemMode::Read,
            quota_bytes: None,
        }
    }
}

impl FilesystemPolicy {
    fn from_manifest(value: &ManifestFilesystemResources) -> Self {
        let mode = match value.mode {
            Some(ManifestFilesystemMode::ReadWrite) => FilesystemMode::ReadWrite,
            _ => FilesystemMode::Read,
        };
        Self {
            mode,
            quota_bytes: value.quota_bytes,
        }
    }

    fn mode_config(&self) -> FilesystemModeConfig {
        match self.mode {
            FilesystemMode::Read => FilesystemModeConfig::Read,
            FilesystemMode::ReadWrite => FilesystemModeConfig::ReadWrite,
        }
    }
}

#[derive(Clone, Default)]
struct CollectedDiagnostics {
    cpu_ms_used: Option<u64>,
    filesystem_bytes_written: Option<u64>,
    network_hosts_contacted: Vec<NetworkHostContact>,
    network_hosts_blocked: Vec<NetworkDeniedHost>,
    filesystem_violations: Vec<FilesystemViolation>,
}

impl AardvarkRuntime {
    /// Creates a new runtime instance based on the provided configuration.
    pub fn new(config: PyRuntimeConfig) -> Result<Self> {
        let engine = create_engine(config.default_language, &config)?;
        Ok(Self {
            config,
            engine: Some(engine),
            runtime_id: None,
        })
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
        let mut descriptor = descriptor;
        let language = descriptor.language.unwrap_or(self.config.default_language);
        descriptor.language = Some(language);
        self.ensure_engine(language)?;

        {
            let js = self.engine_mut().js_mut();
            js.set_network_policy(&[], true);
        }
        {
            let config = self.config.clone();
            self.engine_mut().prepare_environment(&config)?;
        }
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
        let session = PySession::new(bundle, descriptor);
        self.publish_descriptor(session.descriptor())?;
        self.engine_mut().mount_bundle(session.bundle())?;
        Ok(session)
    }

    pub fn prepare_session_with_manifest(
        &mut self,
        bundle: Bundle,
    ) -> Result<(PySession, Option<BundleManifest>)> {
        let manifest = bundle.manifest()?;
        let entrypoint = manifest
            .as_ref()
            .map(|m| m.entrypoint().to_owned())
            .unwrap_or_else(|| "main:handler".to_string());

        let mut descriptor = InvocationDescriptor::new(entrypoint);
        if let Some(runtime) = manifest
            .as_ref()
            .and_then(|m| m.runtime.as_ref())
            .and_then(|rt| rt.language)
        {
            descriptor.language = Some(runtime);
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

        let session = self.prepare_session_with_descriptor(bundle, descriptor)?;

        {
            let js = self.engine_mut().js_mut();
            js.set_filesystem_policy(
                filesystem_policy.mode_config(),
                filesystem_policy.quota_bytes,
            )?;
        }
        self.apply_host_capabilities(manifest_host_capabilities.as_deref())?;

        if let Some(manifest) = &manifest {
            if let Some(resources) = manifest.resources() {
                if let Some(network) = &resources.network {
                    self.engine_mut()
                        .js_mut()
                        .set_network_policy(network.allow.as_slice(), network.https_only);
                }
            }
            if !manifest.packages().is_empty() {
                self.engine_mut().load_manifest_packages(manifest)?;
            }
        }

        Ok((session, manifest))
    }

    /// Runs a prepared session using the default invocation strategy for the selected language.
    pub fn run_session(&mut self, session: &PySession) -> Result<ExecutionOutcome> {
        let language = session
            .descriptor()
            .language
            .unwrap_or(self.config.default_language);
        match language {
            RuntimeLanguage::Python => {
                let mut strategy = DefaultInvocationStrategy;
                self.run_session_with_strategy(session, &mut strategy)
            }
            RuntimeLanguage::JavaScript => {
                let mut strategy = JavaScriptInvocationStrategy;
                self.run_session_with_strategy(session, &mut strategy)
            }
        }
    }

    /// Runs a prepared session with a caller-provided invocation strategy.
    pub fn run_session_with_strategy<S: PyInvocationStrategy>(
        &mut self,
        session: &PySession,
        strategy: &mut S,
    ) -> Result<ExecutionOutcome> {
        let descriptor = session.descriptor();
        let language = descriptor.language.unwrap_or(self.config.default_language);
        let limits = self.effective_limits(descriptor);
        info!(
            target: "aardvark::budget",
            runtime_id = self.runtime_id_str(),
            entrypoint = descriptor.entrypoint(),
            limits.wall_ms = limits.wall_ms.unwrap_or(0),
            limits.heap_mb = limits.heap_mb.unwrap_or(0),
            limits.cpu_ms = limits.cpu_ms.unwrap_or(0),
            strategy = strategy.name(),
            "applying descriptor limits"
        );

        let heap_limit_bytes = limits.heap_mb.map(bytes_from_mb);
        if let (Some(limit_bytes), Some(limit_mb)) = (heap_limit_bytes, limits.heap_mb) {
            let used_before = self.engine_mut().js_mut().heap_used_bytes();
            if used_before > limit_bytes {
                warn!(
                    target: "aardvark::budget",
                    runtime_id = self.runtime_id_str(),
                    heap.used_bytes = used_before,
                    heap.limit_bytes = limit_bytes,
                    "heap usage already exceeds descriptor limit before execution"
                );
                self.cleanup_filesystem();
                return Err(PyRunnerError::HeapLimitExceeded {
                    requested_mb: limit_mb,
                });
            }
        }

        {
            let js = self.engine_mut().js_mut();
            js.clear_network_contacts();
            js.clear_network_denied();
            js.clear_filesystem_events();
        }

        let mut watchdog = self.arm_watchdog(limits.wall_ms);
        let cpu_start_ns = thread_cpu_time_ns();

        let runtime_id_owned = self.runtime_id_str().to_owned();
        let strategy_result = {
            let js = self.engine_mut().js_mut();
            let mut ctx = InvocationContext::new(session, js, language);
            Self::execute_strategy(strategy, &mut ctx, &runtime_id_owned)?
        };

        let timeout_triggered = if let Some(guard) = watchdog.take() {
            guard.complete(self.engine_mut().js_mut())
        } else {
            false
        };

        let cpu_end_ns = thread_cpu_time_ns();
        let cpu_ms_used = match (cpu_start_ns, cpu_end_ns) {
            (Some(start), Some(end)) => Some(ns_to_ms(end.saturating_sub(start))),
            _ => None,
        };
        let (
            filesystem_bytes_written,
            network_contacts_raw,
            network_denied_raw,
            filesystem_violations_raw,
        ) = {
            let js = self.engine_mut().js_mut();
            let usage = js.filesystem_usage_bytes().ok();
            let contacts = js.drain_network_contacts();
            let denied = js.drain_network_denied();
            let fs_violations = js.drain_filesystem_violations();
            (usage, contacts, denied, fs_violations)
        };
        let network_hosts_contacted: Vec<NetworkHostContact> = network_contacts_raw
            .into_iter()
            .map(|record| NetworkHostContact {
                host: record.host,
                port: record.port,
                https: record.https,
            })
            .collect();
        let network_hosts_blocked: Vec<NetworkDeniedHost> = network_denied_raw
            .into_iter()
            .map(|record| NetworkDeniedHost {
                host: record.host,
                port: record.port,
                https_required: record.https_required,
                reason: record.reason,
            })
            .collect();
        let filesystem_violations: Vec<FilesystemViolation> = filesystem_violations_raw
            .into_iter()
            .map(|record| FilesystemViolation {
                path: record.path,
                message: record.message,
            })
            .collect();
        let collected = CollectedDiagnostics {
            cpu_ms_used,
            filesystem_bytes_written,
            network_hosts_contacted,
            network_hosts_blocked,
            filesystem_violations,
        };

        Self::emit_diagnostics_events(&collected, self.runtime_id_str(), descriptor.entrypoint());

        if timeout_triggered {
            warn!(
                target: "aardvark::budget",
                runtime_id = self.runtime_id_str(),
                entrypoint = descriptor.entrypoint(),
                limits.wall_ms = limits.wall_ms.unwrap_or(0),
                "wall-clock limit exceeded"
            );
            let diagnostics = Self::make_diagnostics(
                strategy_result.as_ref().ok().map(|res| &res.execution),
                &collected,
            );
            return self.finish_with_cleanup(ExecutionOutcome::failure(
                FailureKind::TimeoutExceeded {
                    requested_ms: limits.wall_ms.unwrap_or_default(),
                },
                diagnostics,
            ));
        }
        if let Some(limit_ms) = limits.cpu_ms {
            if let Some(used_ms) = collected.cpu_ms_used {
                if used_ms > limit_ms {
                    warn!(
                        target: "aardvark::budget",
                        runtime_id = self.runtime_id_str(),
                        entrypoint = descriptor.entrypoint(),
                        limits.cpu_ms = limit_ms,
                        cpu.used_ms = used_ms,
                        "cpu limit exceeded"
                    );
                    let diagnostics = Self::make_diagnostics(
                        strategy_result.as_ref().ok().map(|res| &res.execution),
                        &collected,
                    );
                    return self.finish_with_cleanup(ExecutionOutcome::failure(
                        FailureKind::CpuLimitExceeded {
                            requested_ms: limit_ms,
                            used_ms,
                        },
                        diagnostics,
                    ));
                } else {
                    info!(
                        target: "aardvark::budget",
                        runtime_id = self.runtime_id_str(),
                        entrypoint = descriptor.entrypoint(),
                        limits.cpu_ms = limit_ms,
                        cpu.used_ms = used_ms,
                        "cpu usage recorded"
                    );
                }
            }
        }
        if let (Some(limit_bytes), Some(limit_mb)) = (heap_limit_bytes, limits.heap_mb) {
            let used_after = self.engine_mut().js_mut().heap_used_bytes();
            if used_after > limit_bytes {
                warn!(
                    target: "aardvark::budget",
                    runtime_id = self.runtime_id_str(),
                    entrypoint = descriptor.entrypoint(),
                    heap.used_bytes = used_after,
                    heap.limit_bytes = limit_bytes,
                    "heap usage exceeded"
                );
                let diagnostics = Self::make_diagnostics(
                    strategy_result.as_ref().ok().map(|res| &res.execution),
                    &collected,
                );
                return self.finish_with_cleanup(ExecutionOutcome::failure(
                    FailureKind::HeapLimitExceeded {
                        requested_mb: limit_mb,
                    },
                    diagnostics,
                ));
            }
        }

        let mut outcome = match strategy_result {
            Ok(result) => Self::finalize_success(result, descriptor, &collected),
            Err(err) => {
                let diagnostics = Self::make_diagnostics(None, &collected);
                ExecutionOutcome::failure(
                    FailureKind::AdapterError {
                        message: err.to_string(),
                    },
                    diagnostics,
                )
            }
        };

        if matches!(self.config.reset_policy, ResetPolicy::AfterInvocation) {
            let span = info_span!(
                target: "aardvark::runtime",
                "runtime.reset",
                runtime_id = self.runtime_id_str()
            );
            let _guard = span.enter();
            match self.reset_to_snapshot() {
                Ok(_) => {
                    info!(
                        target: "aardvark::runtime",
                        runtime_id = self.runtime_id_str(),
                        "reset complete"
                    );
                }
                Err(err) => {
                    warn!(
                        target: "aardvark::runtime",
                        runtime_id = self.runtime_id_str(),
                        error = %err,
                        "reset failed"
                    );
                    outcome = ExecutionOutcome::failure(
                        FailureKind::Other {
                            message: format!("runtime reset failed: {err}"),
                        },
                        outcome.diagnostics.clone(),
                    );
                }
            }
        }

        self.finish_with_cleanup(outcome)
    }

    pub fn cleanup_filesystem(&mut self) {
        if let Err(err) = self.engine_mut().js_mut().reset_filesystem() {
            warn!(
                target: "aardvark::sandbox",
                runtime_id = self.runtime_id_str(),
                error = %err,
                "filesystem cleanup failed"
            );
        }
    }

    /// Exposes the underlying JS runtime for advanced operations.
    pub fn js_runtime(&mut self) -> &mut JsRuntime {
        self.engine_mut().js_mut()
    }

    pub fn set_runtime_id(&mut self, id: impl Into<String>) {
        self.runtime_id = Some(id.into());
    }

    pub fn runtime_id(&self) -> Option<&str> {
        self.runtime_id.as_deref()
    }

    pub fn reset_to_snapshot(&mut self) -> Result<()> {
        let span = info_span!(
            target: "aardvark::runtime",
            "runtime.reset_to_snapshot",
            runtime_id = self.runtime_id_str()
        );
        let _guard = span.enter();
        if let Some(token) = env::var_os("AARDVARK_TEST_FORCE_RESET_FAILURE") {
            env::remove_var("AARDVARK_TEST_FORCE_RESET_FAILURE");
            let label = token
                .to_str()
                .filter(|value| !value.is_empty())
                .map(|value| format!(" forced by {value}"))
                .unwrap_or_default();
            return Err(PyRunnerError::Internal(format!(
                "forced reset failure{label}"
            )));
        }
        let language = self
            .engine
            .as_ref()
            .map(|engine| engine.language())
            .unwrap_or(self.config.default_language);
        if let Some(old) = self.engine.take() {
            drop(old);
        }
        self.engine = Some(create_engine(language, &self.config)?);
        Ok(())
    }

    fn ensure_engine(&mut self, language: RuntimeLanguage) -> Result<()> {
        if self.engine.as_ref().map(|engine| engine.language()) == Some(language) {
            return Ok(());
        }
        if let Some(old) = self.engine.take() {
            drop(old);
        }
        self.engine = Some(create_engine(language, &self.config)?);
        Ok(())
    }

    fn engine_mut(&mut self) -> &mut dyn LanguageEngine {
        self.engine
            .as_deref_mut()
            .expect("language engine must be initialized")
    }

    fn runtime_id_str(&self) -> &str {
        self.runtime_id.as_deref().unwrap_or("<unassigned>")
    }

    fn publish_manifest(&mut self, bundle: &Bundle) -> Result<()> {
        let engine = self.engine_mut();
        engine.js_mut().with_context(|scope, context| {
            let manifest = serialize_manifest(scope, bundle)?;
            let global = context.global(scope);
            let key = v8::String::new(scope, "__pyRunnerManifest")
                .ok_or_else(|| PyRunnerError::Internal("failed to allocate manifest key".into()))?;
            let _ = global.set(scope, key.into(), manifest);
            Ok(())
        })
    }

    fn publish_descriptor(&mut self, descriptor: &InvocationDescriptor) -> Result<()> {
        let json = serde_json::to_string(descriptor).map_err(|err| {
            PyRunnerError::Descriptor(format!("failed to serialize descriptor: {err}"))
        })?;
        let engine = self.engine_mut();
        engine.js_mut().with_context(|scope, context| {
            let global = context.global(scope);
            let key =
                v8::String::new(scope, "__pyRunnerInvocationDescriptor").ok_or_else(|| {
                    PyRunnerError::Internal("failed to allocate descriptor key".into())
                })?;
            let value = v8::String::new(scope, &json).ok_or_else(|| {
                PyRunnerError::Internal("failed to allocate descriptor payload".into())
            })?;
            let _ = global.set(scope, key.into(), value.into());
            Ok(())
        })
    }

    fn effective_limits(&self, descriptor: &InvocationDescriptor) -> InvocationLimits {
        descriptor
            .limits
            .merged_with(self.config.budget_override.as_ref())
    }

    fn arm_watchdog(&mut self, wall_ms: Option<u64>) -> Option<WallClockGuard> {
        let requested_ms = wall_ms?;
        if requested_ms == 0 {
            return None;
        }
        let handle = self.engine_mut().js_mut().isolate_handle();
        Some(WallClockGuard::new(handle, requested_ms))
    }

    fn execute_strategy<S: PyInvocationStrategy>(
        strategy: &mut S,
        ctx: &mut InvocationContext<'_>,
        runtime_id: &str,
    ) -> Result<Result<StrategyResult>> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        info!(
            target: "aardvark::strategy",
            runtime_id,
            entrypoint = entrypoint.as_str(),
            strategy = strategy.name(),
            phase = "pre_execute_js"
        );
        strategy.pre_execute_js(ctx)?;

        info!(
            target: "aardvark::strategy",
            runtime_id,
            entrypoint = entrypoint.as_str(),
            strategy = strategy.name(),
            phase = "pre_execute_py"
        );
        strategy.pre_execute_py(ctx)?;

        info!(
            target: "aardvark::strategy",
            runtime_id,
            entrypoint = entrypoint.as_str(),
            strategy = strategy.name(),
            phase = "invoke"
        );
        let result = strategy.invoke(ctx);

        if let Ok(ref outcome) = result {
            info!(
                target: "aardvark::strategy",
                runtime_id,
                entrypoint = entrypoint.as_str(),
                strategy = strategy.name(),
                phase = "post_execute_py"
            );
            strategy.post_execute_py(ctx, outcome)?;

            info!(
                target: "aardvark::strategy",
                runtime_id,
                entrypoint = entrypoint.as_str(),
                strategy = strategy.name(),
                phase = "post_execute_js"
            );
            strategy.post_execute_js(ctx, outcome)?;
        }

        Ok(result)
    }

    fn finalize_success(
        result: StrategyResult,
        descriptor: &InvocationDescriptor,
        collected: &CollectedDiagnostics,
    ) -> ExecutionOutcome {
        let diagnostics = Self::make_diagnostics(Some(&result.execution), collected);

        if let Some(exception) = diagnostics.exception.clone() {
            return ExecutionOutcome::failure(FailureKind::PythonException(exception), diagnostics);
        }

        if !descriptor.outputs.is_empty() && matches!(result.payload, ResultPayload::None) {
            let expected = &descriptor.outputs[0].name;
            return ExecutionOutcome::failure(
                FailureKind::AdapterError {
                    message: format!(
                        "descriptor expects output '{expected}' but entrypoint returned no result"
                    ),
                },
                diagnostics,
            );
        }

        ExecutionOutcome::success(result.payload, diagnostics)
    }

    fn apply_host_capabilities(&mut self, manifest_caps: Option<&[String]>) -> Result<()> {
        let allowed =
            normalize_capabilities(self.config.host_capabilities.iter().map(String::as_str));
        let allowed_set: HashSet<String> = allowed.iter().cloned().collect();
        let requested =
            manifest_caps.map(|caps| normalize_capabilities(caps.iter().map(String::as_str)));

        let mut effective = Vec::new();
        match requested {
            Some(requested_caps) => {
                if requested_caps.is_empty() {
                    effective = allowed;
                } else {
                    for capability in requested_caps {
                        if allowed_set.contains(&capability) {
                            effective.push(capability);
                        } else {
                            warn!(
                                target = "aardvark::sandbox",
                                runtime_id = self.runtime_id_str(),
                                capability = capability.as_str(),
                                "manifest requested capability not permitted by host configuration"
                            );
                        }
                    }
                }
            }
            None => {
                effective = allowed;
            }
        }

        let engine = self.engine_mut();
        let js = engine.js_mut();
        js.set_host_capabilities(&effective)?;
        Ok(())
    }

    fn finish_with_cleanup(&mut self, outcome: ExecutionOutcome) -> Result<ExecutionOutcome> {
        self.cleanup_filesystem();
        Ok(outcome)
    }

    fn make_diagnostics(
        output: Option<&ExecutionOutput>,
        collected: &CollectedDiagnostics,
    ) -> Diagnostics {
        let mut diagnostics = if let Some(out) = output {
            Diagnostics {
                stdout: out.stdout.clone(),
                stderr: out.stderr.clone(),
                exception: if out.exception_type.is_some()
                    || out.exception_value.is_some()
                    || out.traceback.is_some()
                {
                    Some(crate::outcome::ExceptionInfo {
                        typ: out.exception_type.clone(),
                        value: out.exception_value.clone(),
                        traceback: out.traceback.clone(),
                    })
                } else {
                    None
                },
                ..Diagnostics::default()
            }
        } else {
            Diagnostics::default()
        };
        diagnostics.cpu_ms_used = collected.cpu_ms_used;
        diagnostics.filesystem_bytes_written = collected.filesystem_bytes_written;
        diagnostics.network_hosts_contacted = collected.network_hosts_contacted.clone();
        diagnostics.network_hosts_blocked = collected.network_hosts_blocked.clone();
        diagnostics.filesystem_violations = collected.filesystem_violations.clone();
        diagnostics
    }

    fn emit_diagnostics_events(
        collected: &CollectedDiagnostics,
        runtime_id: &str,
        entrypoint: &str,
    ) {
        if let Some(cpu_ms) = collected.cpu_ms_used {
            info!(
                target: "aardvark::diagnostics",
                runtime_id,
                entrypoint,
                cpu_ms_used = cpu_ms,
                "recorded cpu usage"
            );
        }
        if let Some(bytes) = collected.filesystem_bytes_written {
            info!(
                target: "aardvark::diagnostics",
                runtime_id,
                entrypoint,
                filesystem_bytes_written = bytes,
                "recorded filesystem usage"
            );
        }
        for contact in &collected.network_hosts_contacted {
            info!(
                target: "aardvark::diagnostics",
                runtime_id,
                entrypoint,
                network_host = contact.host.as_str(),
                network_port = contact.port,
                network_https = contact.https,
                "network contact allowed"
            );
        }
        for blocked in &collected.network_hosts_blocked {
            warn!(
                target: "aardvark::diagnostics",
                runtime_id,
                entrypoint,
                network_host = blocked.host.as_str(),
                network_port = blocked.port,
                network_reason = blocked.reason.as_str(),
                https_required = blocked.https_required,
                "network request blocked by policy"
            );
        }
        for violation in &collected.filesystem_violations {
            warn!(
                target: "aardvark::diagnostics",
                runtime_id,
                entrypoint,
                filesystem_path = violation.path.as_deref().unwrap_or("<unknown>"),
                filesystem_message = violation.message.as_str(),
                "filesystem policy violation"
            );
        }
    }
}

fn create_engine(
    language: RuntimeLanguage,
    config: &PyRuntimeConfig,
) -> Result<Box<dyn LanguageEngine>> {
    match language {
        RuntimeLanguage::Python => Ok(Box::new(PythonEngine::new(config)?)),
        RuntimeLanguage::JavaScript => Ok(Box::new(JavaScriptEngine::new(config)?)),
    }
}

fn normalize_capabilities<'a, I>(caps: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut normalized: Vec<String> = caps
        .into_iter()
        .map(|cap| cap.trim().to_ascii_lowercase())
        .filter(|cap| !cap.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

struct WallClockGuard {
    fired: Arc<AtomicBool>,
    cancel_tx: Option<mpsc::Sender<()>>,
}

impl WallClockGuard {
    fn new(handle: v8::IsolateHandle, requested_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let fired = Arc::new(AtomicBool::new(false));
        let thread_handle = handle.clone();
        let fired_clone = fired.clone();
        std::thread::spawn(
            move || match rx.recv_timeout(Duration::from_millis(requested_ms)) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    fired_clone.store(true, Ordering::SeqCst);
                    thread_handle.terminate_execution();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {}
            },
        );
        Self {
            fired,
            cancel_tx: Some(tx),
        }
    }

    fn complete(mut self, runtime: &mut JsRuntime) -> bool {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        let fired = self.fired.load(Ordering::SeqCst);
        if !fired {
            runtime.cancel_terminate_execution();
        }
        fired
    }
}

fn bytes_from_mb(value: u64) -> usize {
    const MIB: usize = 1024 * 1024;
    (value as usize).saturating_mul(MIB)
}

fn thread_cpu_time_ns() -> Option<u64> {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;

        unsafe {
            let which = thread_rusage_scope();
            let mut usage = MaybeUninit::<libc::rusage>::uninit();
            if libc::getrusage(which, usage.as_mut_ptr()) != 0 {
                return None;
            }
            let usage = usage.assume_init();
            let user = timeval_to_ns(usage.ru_utime);
            let sys = timeval_to_ns(usage.ru_stime);
            Some(user.saturating_add(sys))
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn timeval_to_ns(tv: libc::timeval) -> u64 {
    let secs = tv.tv_sec as i128;
    let micros = tv.tv_usec as i128;
    let total = secs
        .saturating_mul(1_000_000_000)
        .saturating_add(micros.saturating_mul(1_000));
    if total < 0 {
        0
    } else {
        total as u64
    }
}

#[cfg(unix)]
fn thread_rusage_scope() -> libc::c_int {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        libc::RUSAGE_THREAD
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        libc::RUSAGE_SELF
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        libc::RUSAGE_SELF
    }
}

fn ns_to_ms(value: u64) -> u64 {
    value.div_ceil(1_000_000)
}

fn serialize_manifest<'a>(
    scope: &mut PinScope<'a, '_>,
    bundle: &Bundle,
) -> Result<v8::Local<'a, v8::Value>> {
    let manifest = v8::Array::new(scope, bundle.entries().len() as i32);
    for (index, entry) in bundle.entries().iter().enumerate() {
        let obj = v8::Object::new(scope);
        let path_key = v8::String::new(scope, "path")
            .ok_or_else(|| PyRunnerError::Internal("failed to allocate path key".into()))?;
        let size_key = v8::String::new(scope, "size")
            .ok_or_else(|| PyRunnerError::Internal("failed to allocate size key".into()))?;
        let path_value = v8::String::new(scope, entry.path())
            .ok_or_else(|| PyRunnerError::Internal("failed to allocate path value".into()))?;
        let size_value = v8::Number::new(scope, entry.contents().len() as f64);
        let _ = obj.set(scope, path_key.into(), path_value.into());
        let _ = obj.set(scope, size_key.into(), size_value.into());
        let _ = manifest.set_index(scope, index as u32, obj.into());
    }
    Ok(manifest.into())
}
