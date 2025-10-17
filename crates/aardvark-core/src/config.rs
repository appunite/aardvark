//! Runtime configuration options.

use crate::engine::OverlayExport;
use crate::invocation::InvocationLimits;
use crate::runtime_language::RuntimeLanguage;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Controls how the runtime loads and captures Pyodide snapshots.
#[derive(Clone)]
pub struct SnapshotConfig {
    /// Optional path to load a prebuilt snapshot from.
    pub load_from: Option<std::path::PathBuf>,
    /// Optional path to write a freshly captured snapshot to.
    pub save_to: Option<std::path::PathBuf>,
    cache: Arc<SnapshotCache>,
}

impl SnapshotConfig {
    /// Clears any cached snapshot bytes.
    pub fn clear_cache(&self) {
        let mut guard = self.cache.bytes.lock().unwrap();
        *guard = None;
    }

    pub(crate) fn cached_bytes(&self) -> Option<Arc<[u8]>> {
        self.cache.bytes.lock().unwrap().clone()
    }

    pub(crate) fn store_cached_bytes(&self, bytes: Arc<[u8]>) {
        let mut guard = self.cache.bytes.lock().unwrap();
        *guard = Some(bytes);
    }
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            load_from: None,
            save_to: None,
            cache: Arc::new(SnapshotCache::default()),
        }
    }
}

impl fmt::Debug for SnapshotConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotConfig")
            .field("load_from", &self.load_from)
            .field("save_to", &self.save_to)
            .field("cached", &self.cache)
            .finish()
    }
}

#[derive(Default)]
struct SnapshotCache {
    bytes: Mutex<Option<Arc<[u8]>>>,
}

impl fmt::Debug for SnapshotCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cached = self.bytes.lock().unwrap().as_ref().map(|arc| arc.len());
        f.debug_struct("SnapshotCache")
            .field("bytes", &cached)
            .finish()
    }
}

/// Captured warm state containing a Pyodide snapshot and its overlay assets.
#[derive(Clone)]
pub struct WarmState {
    snapshot: Arc<[u8]>,
    overlay: Arc<OverlayExport>,
}

impl WarmState {
    /// Constructs a warm state from raw components.
    pub fn new(snapshot: Arc<[u8]>, overlay: OverlayExport) -> Self {
        Self {
            snapshot,
            overlay: Arc::new(overlay),
        }
    }

    /// Returns the snapshot bytes.
    pub fn snapshot(&self) -> Arc<[u8]> {
        self.snapshot.clone()
    }

    /// Returns the overlay export.
    pub fn overlay(&self) -> Arc<OverlayExport> {
        self.overlay.clone()
    }
}

impl fmt::Debug for WarmState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WarmState")
            .field("snapshot_len", &self.snapshot.len())
            .field("overlay_blobs", &self.overlay.blobs.len())
            .finish()
    }
}

/// Configuration applied when constructing [`PyRuntime`][crate::PyRuntime] or pool members.
#[derive(Debug, Clone)]
pub struct PyRuntimeConfig {
    /// Bundled Pyodide version string (usually derived from build-time assets).
    pub pyodide_version: String,
    /// Default guest language selected when manifests/descriptors omit one.
    pub default_language: RuntimeLanguage,
    /// Snapshot-related configuration.
    pub snapshot: SnapshotConfig,
    /// Optional global budget override applied to every session.
    pub budget_override: Option<InvocationLimits>,
    /// Runtime reset behaviour after each invocation.
    pub reset_policy: ResetPolicy,
    /// Host capabilities enabled for exposed native APIs.
    pub host_capabilities: Vec<String>,
    /// Optional prebuilt warm state (snapshot + overlay).
    pub warm_state: Option<WarmState>,
}

impl Default for PyRuntimeConfig {
    fn default() -> Self {
        Self {
            pyodide_version: "0.28.2".to_owned(),
            default_language: RuntimeLanguage::Python,
            snapshot: SnapshotConfig::default(),
            budget_override: None,
            reset_policy: ResetPolicy::Manual,
            host_capabilities: vec!["rawctx_buffers".to_string()],
            warm_state: None,
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
