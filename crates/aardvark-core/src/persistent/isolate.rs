use std::io::{Cursor, Write};
use std::sync::Arc;

use crate::bundle::BundleFingerprint;
use crate::config::PyRuntimeConfig;
use crate::error::Result;
use crate::invocation::InvocationDescriptor;
use crate::persistent::BundleArtifact;
use crate::runtime::PyRuntime;
use crate::runtime_language::RuntimeLanguage;
use crate::strategy::{
    DefaultInvocationStrategy, JsonInvocationStrategy, PyInvocationStrategy, RawCtxInput,
    RawCtxInvocationStrategy,
};
use crate::Bundle;
use serde_json::Value as JsonValue;
use zip::write::FileOptions;
use zip::CompressionMethod;

/// Cleanup behaviour applied after each invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupMode {
    Full,
    SharedBuffersOnly,
    None,
}

impl Default for CleanupMode {
    fn default() -> Self {
        Self::Full
    }
}

/// Configuration applied when constructing a [`PythonIsolate`].
#[derive(Clone)]
pub struct IsolateConfig {
    pub runtime: PyRuntimeConfig,
    pub cleanup: CleanupMode,
}

impl Default for IsolateConfig {
    fn default() -> Self {
        Self {
            runtime: PyRuntimeConfig::default(),
            cleanup: CleanupMode::Full,
        }
    }
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
        descriptor.language = descriptor.language.or(Some(self.artifact.language()));
        HandlerSession {
            artifact: self.artifact.clone(),
            descriptor,
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
}

impl HandlerSession {
    /// Returns the invocation descriptor for inspection.
    pub fn descriptor(&self) -> &InvocationDescriptor {
        &self.descriptor
    }

    /// Provides mutable access to the underlying descriptor for fine-grained changes.
    pub fn descriptor_mut(&mut self) -> &mut InvocationDescriptor {
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
        self.loaded_fingerprint = None;
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
        let bundle = handler.artifact().bundle();
        let descriptor = handler.descriptor_cloned();
        if handler.artifact().manifest().is_some() {
            let (session, _) = self
                .runtime
                .prepare_session_with_manifest_and_descriptor(bundle, descriptor)?;
            self.runtime.run_session_with_strategy(&session, strategy)
        } else {
            let session = self
                .runtime
                .prepare_session_with_descriptor(bundle, descriptor)?;
            self.runtime.run_session_with_strategy(&session, strategy)
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

    /// Executes inline Python code by wrapping it into a temporary bundle.
    pub fn run_inline_python(
        &mut self,
        code: &str,
        entrypoint: &str,
    ) -> Result<crate::ExecutionOutcome> {
        let bundle = bundle_from_inline(code)?;
        let descriptor = InvocationDescriptor::new(entrypoint.to_owned());
        let session = self
            .runtime
            .prepare_session_with_descriptor(bundle, descriptor)?;
        self.runtime.run_session(&session)
    }
}

fn bundle_from_inline(code: &str) -> Result<Bundle> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.py",
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .map_err(|err| crate::error::PyRunnerError::Bundle(err.to_string()))?;
    writer
        .write_all(code.as_bytes())
        .map_err(|err| crate::error::PyRunnerError::Bundle(err.to_string()))?;
    let cursor = writer
        .finish()
        .map_err(|err| crate::error::PyRunnerError::Bundle(err.to_string()))?;
    Bundle::from_zip_bytes(cursor.into_inner())
}
