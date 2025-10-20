//! Aardvark is an embeddable multi-language runtime for executing sandboxed bundles inside V8.
//!
//! The crate targets host services that need a predictable, resource-constrained way to run
//! guest code without shipping a full browser. It currently supports Python (via Pyodide)
//! and an experimental JavaScript engine. It exposes:
//!
//! * [`PyRuntime`] – a single-tenant runtime that prepares bundles, enforces
//!   resource limits, and surfaces structured outcomes.
//! * [`PyRuntimePool`] – a reset-aware pool for amortising Pyodide startup cost.
//! * [`Bundle`] and [`BundleManifest`] helpers for normalising user-provided ZIP
//!   archives and their manifest metadata.
//! * [`InvocationDescriptor`] – a host-controlled contract describing inputs,
//!   outputs, and budgets for individual invocations.
//! * [`ExecutionOutcome`] and [`SandboxTelemetry`] – diagnostics tailored for
//!   observability pipelines.
//!
//! ### Quick Example
//!
//! ```no_run
//! use aardvark_core::{Bundle, PyRuntime, PyRuntimeConfig};
//!
//! fn invoke(bytes: &[u8]) -> anyhow::Result<()> {
//!     let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
//!     let bundle = Bundle::from_zip_bytes(bytes)?;
//!     let (session, _manifest) = runtime.prepare_session_with_manifest(bundle)?;
//!     let outcome = runtime.run_session(&session)?;
//!     if let Some(payload) = outcome.payload() {
//!         println!("payload kind: {}", payload.kind());
//!     }
//!     if outcome.sandbox_telemetry().has_policy_violations() {
//!         eprintln!("invocation tripped sandbox policy");
//!     }
//!     Ok(())
//! }
//! ```
//!
//! See the `docs/architecture` and `docs/api` directories in the repository for
//! a deeper discussion of the runtime design, manifest schema, and integration
//! patterns.

mod asset_store;
pub mod assets;
pub mod bundle;
mod bundle_manifest;
pub mod config;
pub mod error;
pub mod host;
pub mod invocation;
pub mod outcome;
mod package_metadata;
pub mod persistent;
pub mod pool;
pub mod runtime;
mod runtime_language;
pub mod strategy;

mod engine;
mod session;

pub use bundle::{Bundle, BundleFingerprint};
pub use bundle_manifest::{
    BundleManifest, ManifestPyodide, ManifestRuntime,
    MANIFEST_BASENAME as BUNDLE_MANIFEST_BASENAME, MANIFEST_SCHEMA as BUNDLE_MANIFEST_SCHEMA,
    MANIFEST_SCHEMA_VERSION as BUNDLE_MANIFEST_SCHEMA_VERSION,
};
pub use config::{HostHooks, PyRuntimeConfig, WarmHook, WarmState};
pub use engine::{ExecutionOutput, OverlayBlob, OverlayExport};
pub use error::{PyRunnerError, Result};
pub use host::{FilesystemTelemetry, NetworkTelemetry, SandboxTelemetry};
pub use invocation::{FieldDescriptor, InvocationDescriptor, InvocationLimits, WindowConfig};
pub use outcome::{
    Diagnostics, ExecutionOutcome, FailureKind, OutcomeStatus, ResultPayload, SharedBufferHandle,
};
pub use persistent::{
    BundleArtifact, BundleHandle, BundlePool, CleanupMode, HandlerSession, IsolateConfig,
    PoolOptions, PoolStats, PythonIsolate, QueueMode,
};
pub use pool::{PoolConfig, PyRuntimePool};
pub use runtime::PyRuntime;
pub use runtime_language::RuntimeLanguage;
pub use session::PySession;
pub use strategy::{
    DefaultInvocationStrategy, JavaScriptInvocationStrategy, JsonInvocationStrategy,
    PyInvocationStrategy, RawCtxBindingBuilder, RawCtxInput, RawCtxInvocationStrategy,
    RawCtxMetadata, RawCtxPublishBuilder, RawCtxTableColumnBuilder, RawCtxTableSpec,
    RawCtxTableSpecBuilder,
};
