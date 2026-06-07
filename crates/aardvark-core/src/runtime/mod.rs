//! Runtime coordination between the host and language-specific engines.

mod execution;
mod javascript;
mod metrics;
mod python;
mod sessions;
mod watchdog;

use crate::bundle::{Bundle, BundleFingerprint};
use crate::bundle_manifest::{BundleManifest, ManifestFilesystemMode, ManifestFilesystemResources};
use crate::config::{
    PyRuntimeConfig, ResetPolicy, WarmState, DEFAULT_PYODIDE_DISTRIBUTION_PROFILE,
};
use crate::engine::{ExecutionOutput, FilesystemModeConfig, JsRuntime};
use crate::error::{PyRunnerError, Result};
use crate::invocation::{InvocationDescriptor, InvocationLimits};
use crate::outcome::{
    Diagnostics, ExecutionOutcome, FailureKind, FilesystemViolation, NetworkDeniedHost,
    NetworkHostContact, ResetMode, ResetSummary, ResultPayload,
};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use crate::strategy::{
    DefaultInvocationStrategy, InvocationContext, JavaScriptInvocationStrategy,
    PyInvocationStrategy, StrategyResult,
};
use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, info_span, warn};
use v8::{self, PinScope};

pub use javascript::JavaScriptEngine;
pub use python::PythonEngine;

use metrics::{bytes_from_mb, ns_to_ms, thread_cpu_time_ns};
use watchdog::WallClockGuard;

pub type PyRuntime = AardvarkRuntime;

/// Primary host-facing runtime capable of executing bundles for multiple guest languages.
///
/// Instances are cheap to create but expensive to warm; consider pooling via
/// [`PyRuntimePool`](crate::PyRuntimePool) for throughput-sensitive workloads.
pub struct AardvarkRuntime {
    config: PyRuntimeConfig,
    engine: Option<Box<dyn LanguageEngine>>,
    runtime_id: Option<String>,
    warm_restored: bool,
    engine_generation: u64,
    pending_reset_summary: Option<ResetSummary>,
    environment_ready: bool,
    current_bundle: Option<CurrentBundleState>,
}

trait LanguageEngine {
    fn language(&self) -> RuntimeLanguage;
    fn js_mut(&mut self) -> &mut JsRuntime;
    fn prepare_environment(&mut self, config: &PyRuntimeConfig) -> Result<()>;
    fn load_manifest_packages(&mut self, manifest: &BundleManifest) -> Result<()>;
    fn mount_bundle(&mut self, bundle: &Bundle) -> Result<()>;
    fn reset_in_place(&mut self, config: &PyRuntimeConfig) -> Result<()>;
    fn set_warm_state(&mut self, _state: Option<WarmState>) -> Result<()> {
        Ok(())
    }
    fn compatibility_fingerprint(&self) -> Option<&str> {
        None
    }
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
    reset_summary: Option<crate::outcome::ResetSummary>,
    queue_wait_ms: Option<u64>,
    prepare_ms: Option<u64>,
    cleanup_ms: Option<u64>,
    py_heap_kib: Option<u64>,
    rss_kib_before: Option<u64>,
    rss_kib_after: Option<u64>,
}

#[derive(Clone)]
struct CurrentBundleState {
    fingerprint: BundleFingerprint,
    language: RuntimeLanguage,
    pyodide_preload_imports: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeCleanupMode {
    Full,
    SharedBuffersOnly,
    None,
}

impl AardvarkRuntime {
    fn ensure_environment_ready(&mut self, language: RuntimeLanguage) -> Result<()> {
        self.ensure_engine(language)?;

        if self
            .current_bundle
            .as_ref()
            .map(|state| state.language != language)
            .unwrap_or(false)
        {
            self.current_bundle = None;
            self.environment_ready = false;
        }

        let mut prepared_now = false;
        if !self.environment_ready {
            let config = self.config.clone();
            self.engine_mut().prepare_environment(&config)?;
            self.environment_ready = true;
            prepared_now = true;
        }

        {
            let js = self.engine_mut().js_mut();
            js.set_network_policy(&[], true);
        }

        if prepared_now && self.config.warm_state.is_some() && !self.warm_restored {
            if let Some(hook) = self.config.hooks.after_warm_restore.clone() {
                hook(self)?;
            }
            self.warm_restored = true;
        }

        Ok(())
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

    /// Returns the compatibility fingerprint for the active Pyodide distribution.
    pub fn pyodide_compatibility_fingerprint(&self) -> Option<&str> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.compatibility_fingerprint())
    }

    pub fn reset_to_snapshot(&mut self) -> Result<()> {
        let span = info_span!(
            target: "aardvark::runtime",
            "runtime.reset_to_snapshot",
            runtime_id = self.runtime_id_str()
        );
        let _guard = span.enter();
        let start = Instant::now();
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
        self.engine_generation = self.engine_generation.saturating_add(1);
        let summary = ResetSummary {
            mode: ResetMode::RecreateEngine,
            duration_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            engine_generation: self.engine_generation,
        };
        info!(
            target: "aardvark::runtime",
            runtime_id = self.runtime_id_str(),
            reset.mode = ?summary.mode,
            reset.duration_ms = summary.duration_ms,
            reset.engine_generation = summary.engine_generation,
            "reset recorded"
        );
        self.pending_reset_summary = Some(summary);
        self.warm_restored = false;
        self.environment_ready = false;
        self.current_bundle = None;
        Ok(())
    }

    /// Resets the runtime by rebuilding the language engine in place without dropping the isolate.
    pub fn reset_in_place(&mut self) -> Result<()> {
        let span = info_span!(
            target: "aardvark::runtime",
            "runtime.reset_in_place",
            runtime_id = self.runtime_id_str()
        );
        let _guard = span.enter();
        let start = Instant::now();
        let language = self
            .engine
            .as_ref()
            .map(|engine| engine.language())
            .unwrap_or(self.config.default_language);
        if let Some(engine) = self.engine.as_mut() {
            engine.reset_in_place(&self.config)?;
            let summary = ResetSummary {
                mode: ResetMode::InPlace,
                duration_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                engine_generation: self.engine_generation,
            };
            info!(
                target: "aardvark::runtime",
                runtime_id = self.runtime_id_str(),
                reset.mode = ?summary.mode,
                reset.duration_ms = summary.duration_ms,
                reset.engine_generation = summary.engine_generation,
                "reset recorded"
            );
            self.pending_reset_summary = Some(summary);
        } else {
            self.engine = Some(create_engine(language, &self.config)?);
            self.engine_generation = self.engine_generation.saturating_add(1);
            let summary = ResetSummary {
                mode: ResetMode::RecreateEngine,
                duration_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                engine_generation: self.engine_generation,
            };
            info!(
                target: "aardvark::runtime",
                runtime_id = self.runtime_id_str(),
                reset.mode = ?summary.mode,
                reset.duration_ms = summary.duration_ms,
                reset.engine_generation = summary.engine_generation,
                "reset recorded"
            );
            self.pending_reset_summary = Some(summary);
            self.warm_restored = false;
            return Ok(());
        }
        self.warm_restored = false;
        self.environment_ready = false;
        self.current_bundle = None;
        Ok(())
    }

    fn ensure_engine(&mut self, language: RuntimeLanguage) -> Result<()> {
        if self.engine.as_ref().map(|engine| engine.language()) == Some(language) {
            return Ok(());
        }
        let start = Instant::now();
        if let Some(old) = self.engine.take() {
            drop(old);
        }
        self.engine = Some(create_engine(language, &self.config)?);
        self.engine_generation = self.engine_generation.saturating_add(1);
        let summary = ResetSummary {
            mode: ResetMode::RecreateEngine,
            duration_ms: start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            engine_generation: self.engine_generation,
        };
        info!(
            target: "aardvark::runtime",
            runtime_id = self.runtime_id_str(),
            reset.mode = ?summary.mode,
            reset.duration_ms = summary.duration_ms,
            reset.engine_generation = summary.engine_generation,
            "reset recorded"
        );
        self.pending_reset_summary = Some(summary);
        self.warm_restored = false;
        self.environment_ready = false;
        self.current_bundle = None;
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

    fn finish_with_cleanup(
        &mut self,
        outcome: ExecutionOutcome,
        cleanup_entrypoint: Option<&str>,
        cleanup_mode: RuntimeCleanupMode,
    ) -> Result<ExecutionOutcome> {
        if matches!(cleanup_mode, RuntimeCleanupMode::Full) {
            if let Some(module) = cleanup_entrypoint.and_then(module_from_entrypoint) {
                self.cleanup_python_module(module);
            }
            self.engine_mut().js_mut().clear_rawctx_auto_wrapper_cache();
        }

        let shared_buffers_already_drained = matches!(
            (cleanup_mode, outcome.payload()),
            (
                RuntimeCleanupMode::SharedBuffersOnly,
                Some(ResultPayload::SharedBuffers(_))
            )
        );

        if matches!(cleanup_mode, RuntimeCleanupMode::Full)
            || (matches!(cleanup_mode, RuntimeCleanupMode::SharedBuffersOnly)
                && !shared_buffers_already_drained)
        {
            if let Err(err) = self.engine_mut().js_mut().reset_shared_buffers() {
                warn!(
                    target: "aardvark::sandbox",
                    runtime_id = self.runtime_id_str(),
                    error = %err,
                    "reset shared buffers failed"
                );
            }
        }

        if matches!(cleanup_mode, RuntimeCleanupMode::Full) {
            self.cleanup_filesystem();
        }

        Ok(outcome)
    }

    fn cleanup_python_module(&mut self, module: &str) {
        let script = format!(
            "import sys\nglobals().pop('__aardvark_rawctx_installed_spec_key', None)\n_cache = globals().get('__aardvark_entrypoint_cache')\nif isinstance(_cache, dict):\n    for _key in list(_cache):\n        if _key.partition(':')[0] == {module:?}:\n            _cache.pop(_key, None)\nsys.modules.pop({module:?}, None)\n"
        );
        if let Err(err) = self.engine_mut().js_mut().run_python_snippet(&script) {
            warn!(
                target: "aardvark::runtime",
                runtime_id = self.runtime_id_str(),
                module,
                error = %err,
                "module cleanup failed"
            );
        }
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
        diagnostics.reset = collected.reset_summary.clone();
        diagnostics.queue_wait_ms = collected.queue_wait_ms;
        diagnostics.prepare_ms = collected.prepare_ms;
        diagnostics.cleanup_ms = collected.cleanup_ms;
        diagnostics.py_heap_kib = collected.py_heap_kib;
        diagnostics.rss_kib_before = collected.rss_kib_before;
        diagnostics.rss_kib_after = collected.rss_kib_after;
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

fn module_from_entrypoint(entrypoint: &str) -> Option<&str> {
    let trimmed = entrypoint.trim();
    if trimmed.is_empty() {
        return None;
    }
    let module_part = trimmed
        .split_once(':')
        .map(|(module, _)| module)
        .unwrap_or(trimmed);
    let module_trimmed = module_part.trim();
    if module_trimmed.is_empty() {
        None
    } else {
        Some(module_trimmed)
    }
}

fn create_engine(
    language: RuntimeLanguage,
    config: &PyRuntimeConfig,
) -> Result<Box<dyn LanguageEngine>> {
    let mut engine: Box<dyn LanguageEngine> = match language {
        RuntimeLanguage::Python => Box::new(PythonEngine::new(config)?),
        RuntimeLanguage::JavaScript => Box::new(JavaScriptEngine::new(config)?),
    };
    engine.set_warm_state(config.warm_state.clone())?;
    Ok(engine)
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
