//! Runtime configuration options.

use crate::engine::OverlayExport;
use crate::error::{PyRunnerError, Result};
use crate::invocation::InvocationLimits;
use crate::pyodide::PYODIDE_VERSION;
use crate::runtime::PyRuntime;
use crate::runtime_language::RuntimeLanguage;
use crate::BundleManifest;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_PYODIDE_DISTRIBUTION_PROFILE: &str = "default";

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
        let mut guard = self.cache.bytes.lock();
        *guard = None;
    }

    pub(crate) fn cached_bytes(&self) -> Option<CachedSnapshot> {
        self.cache.bytes.lock().clone()
    }

    pub(crate) fn store_cached_bytes(
        &self,
        bytes: Arc<[u8]>,
        compatibility_fingerprint: Option<&str>,
    ) {
        let mut guard = self.cache.bytes.lock();
        *guard = Some(CachedSnapshot {
            bytes,
            compatibility_fingerprint: compatibility_fingerprint.map(ToOwned::to_owned),
        });
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
    bytes: Mutex<Option<CachedSnapshot>>,
}

#[derive(Clone)]
pub(crate) struct CachedSnapshot {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) compatibility_fingerprint: Option<String>,
}

impl fmt::Debug for SnapshotCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cached = self.bytes.lock().as_ref().map(|snapshot| {
            (
                snapshot.bytes.len(),
                snapshot.compatibility_fingerprint.clone(),
            )
        });
        f.debug_struct("SnapshotCache")
            .field("bytes", &cached)
            .finish()
    }
}

/// Type alias for host-provided warm snapshot hooks.
pub type WarmHook = dyn Fn(&mut PyRuntime) -> Result<()> + Send + Sync;

/// Host-configurable hooks for the warm snapshot lifecycle.
#[derive(Clone, Default)]
pub struct HostHooks {
    /// Invoked immediately before a warm snapshot is captured.
    pub before_warm_snapshot: Option<Arc<WarmHook>>,
    /// Invoked after a warm snapshot has been applied to a runtime.
    pub after_warm_restore: Option<Arc<WarmHook>>,
}

impl fmt::Debug for HostHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostHooks")
            .field(
                "before_warm_snapshot",
                &self.before_warm_snapshot.as_ref().map(|_| "Some"),
            )
            .field(
                "after_warm_restore",
                &self.after_warm_restore.as_ref().map(|_| "Some"),
            )
            .finish()
    }
}

/// Captured warm state containing a Pyodide snapshot and its overlay assets.
#[derive(Clone)]
pub struct WarmState {
    snapshot: Arc<[u8]>,
    overlay: Arc<OverlayExport>,
    compatibility_fingerprint: Option<String>,
    overlay_preloaded: bool,
}

impl WarmState {
    /// Constructs a warm state from raw components and its Pyodide distribution fingerprint.
    pub fn new(
        snapshot: Arc<[u8]>,
        overlay: OverlayExport,
        compatibility_fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            snapshot,
            overlay: Arc::new(overlay),
            compatibility_fingerprint: Some(compatibility_fingerprint.into()),
            overlay_preloaded: false,
        }
    }

    /// Constructs a warm state that already includes the overlay in the snapshot image.
    ///
    /// Use this only when the overlay contents were hydrated before snapshot capture.
    pub fn with_overlay_preloaded(
        snapshot: Arc<[u8]>,
        overlay: OverlayExport,
        compatibility_fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            snapshot,
            overlay: Arc::new(overlay),
            compatibility_fingerprint: Some(compatibility_fingerprint.into()),
            overlay_preloaded: true,
        }
    }

    pub(crate) fn without_compatibility_fingerprint(
        snapshot: Arc<[u8]>,
        overlay: OverlayExport,
    ) -> Self {
        Self {
            snapshot,
            overlay: Arc::new(overlay),
            compatibility_fingerprint: None,
            overlay_preloaded: false,
        }
    }

    /// Flags the warm state as already containing the overlay contents inside the snapshot.
    ///
    /// Hosts that assemble a warm state manually can call this to skip the overlay import
    /// step during `prepare_environment`.
    pub fn into_overlay_preloaded(mut self) -> Self {
        self.overlay_preloaded = true;
        self
    }

    /// Returns the snapshot bytes.
    pub fn snapshot(&self) -> Arc<[u8]> {
        self.snapshot.clone()
    }

    /// Returns the overlay export.
    pub fn overlay(&self) -> Arc<OverlayExport> {
        self.overlay.clone()
    }

    /// Returns the Pyodide distribution fingerprint this warm state was captured with.
    pub fn compatibility_fingerprint(&self) -> Option<&str> {
        self.compatibility_fingerprint.as_deref()
    }

    /// Indicates whether the overlay contents were baked into the snapshot, allowing
    /// the runtime to skip `import_overlay` when restoring the warm state.
    pub fn overlay_preloaded(&self) -> bool {
        self.overlay_preloaded
    }
}

impl fmt::Debug for WarmState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WarmState")
            .field("snapshot_len", &self.snapshot.len())
            .field("overlay_blobs", &self.overlay.blobs.len())
            .field(
                "compatibility_fingerprint",
                &self.compatibility_fingerprint.as_deref(),
            )
            .field("overlay_preloaded", &self.overlay_preloaded)
            .finish()
    }
}

/// Configuration applied when constructing [`PyRuntime`] or pool members.
#[derive(Debug, Clone)]
pub struct PyRuntimeConfig {
    /// Bundled Pyodide version string (usually derived from build-time assets).
    pub pyodide_version: String,
    /// Optional Aardvark Pyodide distribution directory.
    ///
    /// When set, the runtime verifies `aardvark-pyodide-dist.json` and loads both
    /// core Pyodide assets and package wheels from this distribution. This is the
    /// preferred production contract.
    pub pyodide_dist_dir: Option<PathBuf>,
    /// Active distribution profile selected for this runtime.
    ///
    /// Profiles are host-defined labels such as `default`, `blas`, or
    /// `ndarray-fast`. The selected profile is applied before the Pyodide
    /// isolate is created.
    pub pyodide_distribution_profile: Option<String>,
    /// Host-provided registry of profile labels to staged distribution dirs.
    pub pyodide_distribution_profiles: BTreeMap<String, PathBuf>,
    /// Default guest language selected when manifests/descriptors omit one.
    pub default_language: RuntimeLanguage,
    /// Snapshot-related configuration.
    pub snapshot: SnapshotConfig,
    /// Host lifecycle hooks executed around warm snapshot capture/restore.
    pub hooks: HostHooks,
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
        let default_dist_dir = std::env::var_os("AARDVARK_PYODIDE_DIST_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let mut pyodide_distribution_profiles = pyodide_distribution_profiles_from_env();
        if let Some(path) = default_dist_dir.as_ref() {
            pyodide_distribution_profiles
                .entry(DEFAULT_PYODIDE_DISTRIBUTION_PROFILE.to_string())
                .or_insert_with(|| path.clone());
        }
        Self {
            pyodide_version: PYODIDE_VERSION.to_owned(),
            pyodide_dist_dir: default_dist_dir,
            pyodide_distribution_profile: None,
            pyodide_distribution_profiles,
            default_language: RuntimeLanguage::Python,
            snapshot: SnapshotConfig::default(),
            hooks: HostHooks::default(),
            budget_override: None,
            reset_policy: ResetPolicy::Manual,
            host_capabilities: vec!["rawctx_buffers".to_string()],
            warm_state: None,
        }
    }
}

impl PyRuntimeConfig {
    /// Returns the configured Aardvark Pyodide distribution directory, if any.
    pub fn pyodide_dist_dir(&self) -> Option<&PathBuf> {
        self.pyodide_dist_dir.as_ref()
    }

    /// Sets the Aardvark Pyodide distribution directory override.
    pub fn set_pyodide_dist_dir<P: Into<PathBuf>>(&mut self, path: P) {
        self.pyodide_dist_dir = Some(path.into());
    }

    /// Clears any explicit Aardvark Pyodide distribution directory override.
    pub fn clear_pyodide_dist_dir(&mut self) {
        self.pyodide_dist_dir = None;
    }

    /// Returns a new configuration with the provided Aardvark Pyodide distribution directory.
    pub fn with_pyodide_dist_dir<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.set_pyodide_dist_dir(path);
        self
    }

    /// Registers a staged Pyodide distribution for a host-defined profile.
    pub fn set_pyodide_distribution_profile_dir(
        &mut self,
        profile: impl AsRef<str>,
        path: impl Into<PathBuf>,
    ) -> Result<()> {
        let profile = normalize_pyodide_distribution_profile(profile.as_ref())?;
        self.pyodide_distribution_profiles
            .insert(profile, path.into());
        Ok(())
    }

    /// Returns a new configuration with a profile-to-distribution mapping.
    pub fn with_pyodide_distribution_profile_dir(
        mut self,
        profile: impl AsRef<str>,
        path: impl Into<PathBuf>,
    ) -> Result<Self> {
        self.set_pyodide_distribution_profile_dir(profile, path)?;
        Ok(self)
    }

    /// Selects a registered Pyodide distribution profile for this runtime.
    ///
    /// This must be called before the runtime is constructed. Once a V8 isolate
    /// exists, the Pyodide distribution cannot be swapped in-place.
    pub fn set_pyodide_distribution_profile(&mut self, profile: impl AsRef<str>) -> Result<()> {
        let profile = normalize_pyodide_distribution_profile(profile.as_ref())?;
        if let Some(path) = self.pyodide_distribution_profiles.get(&profile).cloned() {
            self.pyodide_dist_dir = Some(path);
        } else if profile != DEFAULT_PYODIDE_DISTRIBUTION_PROFILE {
            return Err(PyRunnerError::Validation(format!(
                "Pyodide distribution profile '{profile}' is not registered"
            )));
        }
        self.pyodide_distribution_profile = Some(profile);
        Ok(())
    }

    /// Returns a new configuration with the active Pyodide distribution profile.
    pub fn with_pyodide_distribution_profile(mut self, profile: impl AsRef<str>) -> Result<Self> {
        self.set_pyodide_distribution_profile(profile)?;
        Ok(self)
    }

    /// Applies a bundle-requested distribution profile to the configuration.
    pub(crate) fn apply_manifest_pyodide_distribution_profile(
        &mut self,
        profile: Option<&str>,
    ) -> Result<()> {
        let Some(profile) = profile else {
            return Ok(());
        };
        self.set_pyodide_distribution_profile(profile)
    }

    /// Applies bundle manifest runtime requirements that must be known before
    /// constructing the isolate.
    ///
    /// Today this selects `runtime.pyodide.profile`; hosts should call this
    /// before `PyRuntime::new` when they construct runtimes directly from ZIP
    /// bundles rather than going through `BundlePool`.
    pub fn apply_bundle_manifest(&mut self, manifest: Option<&BundleManifest>) -> Result<()> {
        self.apply_manifest_pyodide_distribution_profile(
            manifest.and_then(BundleManifest::pyodide_distribution_profile),
        )
    }
}

pub(crate) fn normalize_pyodide_distribution_profile(profile: &str) -> Result<String> {
    let trimmed = profile.trim();
    if trimmed.is_empty() {
        return Err(PyRunnerError::Manifest(
            "runtime.pyodide.profile cannot be empty".into(),
        ));
    }
    let normalized = trimmed.to_ascii_lowercase();
    if normalized.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    }) {
        Ok(normalized)
    } else {
        Err(PyRunnerError::Manifest(format!(
            "runtime.pyodide.profile '{trimmed}' must contain only ASCII letters, digits, '-' or '_'"
        )))
    }
}

fn pyodide_distribution_profiles_from_env() -> BTreeMap<String, PathBuf> {
    let Some(raw) = std::env::var_os("AARDVARK_PYODIDE_DIST_PROFILES") else {
        return BTreeMap::new();
    };
    let text = raw.to_string_lossy();
    let mut profiles = BTreeMap::new();
    for entry in text.split([';', ',']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((profile, path)) = entry.split_once('=') else {
            tracing::warn!(
                target: "aardvark::config",
                entry,
                "ignoring malformed AARDVARK_PYODIDE_DIST_PROFILES entry"
            );
            continue;
        };
        match normalize_pyodide_distribution_profile(profile) {
            Ok(profile) => {
                let path = path.trim();
                if !path.is_empty() {
                    profiles.insert(profile, PathBuf::from(path));
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "aardvark::config",
                    error = %error,
                    "ignoring invalid Pyodide distribution profile from environment"
                );
            }
        }
    }
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_applies_bundle_manifest_pyodide_distribution_profile() -> Result<()> {
        let json = format!(
            r#"{{
                "schemaVersion": "1.0",
                "entrypoint": "main:run",
                "runtime": {{
                    "language": "python",
                    "pyodide": {{"version": "{}", "profile": "blas"}}
                }}
            }}"#,
            PYODIDE_VERSION
        );
        let manifest = BundleManifest::from_bytes(json.as_bytes())?;
        let mut config = PyRuntimeConfig::default();
        config.set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
        config.apply_bundle_manifest(Some(&manifest))?;

        assert_eq!(config.pyodide_distribution_profile.as_deref(), Some("blas"));
        assert_eq!(
            config.pyodide_dist_dir.as_deref(),
            Some(std::path::Path::new("/tmp/aardvark-blas-dist"))
        );
        Ok(())
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
