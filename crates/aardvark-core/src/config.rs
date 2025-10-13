//! Runtime configuration options.

use crate::invocation::InvocationLimits;

/// Controls how the runtime loads and captures Pyodide snapshots.
#[derive(Debug, Clone, Default)]
pub struct SnapshotConfig {
    /// Optional path to load a prebuilt snapshot from.
    pub load_from: Option<std::path::PathBuf>,
    /// Optional path to write a freshly captured snapshot to.
    pub save_to: Option<std::path::PathBuf>,
}

/// Configuration applied when constructing [`PyRuntime`][crate::PyRuntime] or pool members.
#[derive(Debug, Clone)]
pub struct PyRuntimeConfig {
    /// Bundled Pyodide version string (usually derived from build-time assets).
    pub pyodide_version: String,
    /// Snapshot-related configuration.
    pub snapshot: SnapshotConfig,
    /// Optional global budget override applied to every session.
    pub budget_override: Option<InvocationLimits>,
    /// Runtime reset behaviour after each invocation.
    pub reset_policy: ResetPolicy,
    /// Host capabilities enabled for exposed native APIs.
    pub host_capabilities: Vec<String>,
}

impl Default for PyRuntimeConfig {
    fn default() -> Self {
        Self {
            pyodide_version: "0.28.2".to_owned(),
            snapshot: SnapshotConfig::default(),
            budget_override: None,
            reset_policy: ResetPolicy::Manual,
            host_capabilities: vec!["rawctx_buffers".to_string()],
        }
    }
}

/// Determines how the runtime resets between invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResetPolicy {
    /// Host is responsible for calling [`PyRuntime::reset_to_snapshot`](crate::PyRuntime::reset_to_snapshot).
    #[default]
    Manual,
    /// Runtime automatically resets to its baseline snapshot after each invocation.
    AfterInvocation,
}
