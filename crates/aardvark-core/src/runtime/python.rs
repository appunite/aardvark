//! Python-specific engine backed by the embedded Pyodide runtime.

use crate::bundle::Bundle;
use crate::bundle_manifest::BundleManifest;
use crate::config::{PyRuntimeConfig, WarmState};
use crate::engine::{JsRuntime, PyodideLoadOptions};
use crate::error::{PyRunnerError, Result};
use crate::package_metadata;
use crate::pyodide_distribution::PyodideDistribution;
use crate::runtime_language::RuntimeLanguage;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use v8;

use super::LanguageEngine;

pub struct PythonEngine {
    js: JsRuntime,
    distribution: PyodideDistribution,
    snapshot_bytes: Option<Arc<[u8]>>,
    warm_state: Option<WarmState>,
    installed_packages: HashSet<String>,
}

impl PythonEngine {
    pub fn new(config: &PyRuntimeConfig) -> Result<Self> {
        let js = JsRuntime::new()?;
        let distribution = PyodideDistribution::resolve(config)?;
        crate::engine::set_package_root_override(distribution.package_root());
        let warm_state = config.warm_state.clone();
        let snapshot_bytes = load_snapshot_bytes(config, distribution.compatibility_fingerprint())?;
        let mut engine = Self {
            js,
            distribution,
            snapshot_bytes,
            warm_state,
            installed_packages: HashSet::new(),
        };
        engine.register_core_assets()?;
        engine.inject_version_globals(config)?;
        Ok(engine)
    }

    fn ensure_pyodide_module(&mut self) -> Result<()> {
        self.js.ensure_module("pyodide.mjs")
    }

    fn inject_version_globals(&mut self, config: &PyRuntimeConfig) -> Result<()> {
        let version = config.pyodide_version.clone();
        self.js.with_context(|scope, context| {
            let global = context.global(scope);
            let key = v8::String::new(scope, "__pyRunnerPyodideVersion")
                .ok_or_else(|| PyRunnerError::Init("failed to allocate version key".into()))?;
            let value = v8::String::new(scope, &version)
                .ok_or_else(|| PyRunnerError::Init("failed to allocate version string".into()))?;
            let _ = global.set(scope, key.into(), value.into());
            Ok(())
        })
    }

    fn register_core_assets(&self) -> Result<()> {
        let js = &self.js;
        js.insert_binary_asset_owned(
            "pyodide.asm.wasm",
            self.distribution
                .read_binary_asset("pyodide.asm.wasm")?
                .as_ref()
                .to_vec(),
        );
        js.insert_text_asset(
            "pyodide.asm.js",
            self.distribution.read_text_asset("pyodide.asm.js")?,
        );
        js.insert_text_asset(
            "pyodide.asm.patched.js",
            self.distribution
                .read_text_asset("pyodide.asm.patched.js")?,
        );
        js.insert_text_asset(
            "pyodide_builtin_wrappers.js",
            self.distribution
                .read_text_asset("pyodide_builtin_wrappers.js")?,
        );
        js.insert_text_asset(
            "pyodide_bootstrap.js",
            self.distribution.read_text_asset("pyodide_bootstrap.js")?,
        );
        js.insert_text_asset(
            "pyodide_emscripten_setup.js",
            self.distribution
                .read_text_asset("pyodide_emscripten_setup.js")?,
        );
        js.insert_text_asset(
            "pyodide_packages.js",
            self.distribution.read_text_asset("pyodide_packages.js")?,
        );
        js.insert_text_asset(
            "pyodide.mjs",
            self.distribution.read_text_asset("pyodide.mjs")?,
        );
        js.insert_text_asset(
            "pyodide.js",
            self.distribution.read_text_asset("pyodide.js")?,
        );
        js.insert_binary_asset_owned(
            "python_stdlib.zip",
            self.distribution
                .read_binary_asset("python_stdlib.zip")?
                .as_ref()
                .to_vec(),
        );
        let lockfile = self.distribution.read_text_asset("pyodide-lock.json")?;
        let metadata = package_metadata::package_metadata_from_lockfile(&lockfile);
        js.insert_text_asset("pyodide-lock.json", metadata.json_text);
        js.insert_binary_asset_owned("pyodide-lock.capnp", metadata.capnp_bytes);
        js.insert_text_asset(
            "entropy/allow_entropy.py",
            crate::assets::entropy_allow_py(),
        );
        js.insert_text_asset(
            "entropy/entropy_import_context.py",
            crate::assets::entropy_import_context_py(),
        );
        js.insert_text_asset(
            "entropy/entropy_patches.py",
            crate::assets::entropy_patches_py(),
        );
        js.insert_text_asset(
            "entropy/import_patch_manager.py",
            crate::assets::entropy_import_patch_manager_py(),
        );
        Ok(())
    }

    pub(crate) fn compatibility_fingerprint(&self) -> &str {
        self.distribution.compatibility_fingerprint()
    }
}

impl LanguageEngine for PythonEngine {
    fn language(&self) -> RuntimeLanguage {
        RuntimeLanguage::Python
    }

    fn compatibility_fingerprint(&self) -> Option<&str> {
        Some(self.compatibility_fingerprint())
    }

    fn js_mut(&mut self) -> &mut JsRuntime {
        &mut self.js
    }

    fn prepare_environment(&mut self, config: &PyRuntimeConfig) -> Result<()> {
        self.ensure_pyodide_module()?;
        let make_snapshot = config.snapshot.save_to.is_some();
        let snapshot_owned = self.snapshot_bytes.clone();
        let load_opts = PyodideLoadOptions {
            snapshot: snapshot_owned.as_ref().map(|arc| arc.as_ref()),
            make_snapshot,
        };
        self.js.load_pyodide(load_opts)?;
        self.snapshot_bytes = None;
        self.installed_packages.clear();
        if let Some(state) = self.warm_state.as_ref() {
            if state.overlay_preloaded() {
                // Overlay already baked into the snapshot; refresh dynlibs to keep loaders in sync.
                self.js.prepare_dynlibs()?;
            } else {
                if let Some(token) = env::var_os("AARDVARK_TEST_FORCE_OVERLAY_IMPORT_FAILURE") {
                    env::remove_var("AARDVARK_TEST_FORCE_OVERLAY_IMPORT_FAILURE");
                    let label = token
                        .to_str()
                        .filter(|value| !value.is_empty())
                        .map(|value| format!(" forced by {value}"))
                        .unwrap_or_default();
                    return Err(PyRunnerError::Init(format!(
                        "forced overlay import failure{label}"
                    )));
                }
                let overlay = state.overlay();
                self.js.import_overlay(&overlay.metadata, &overlay.blobs)?;
                self.js.prepare_dynlibs()?;
            }
        }
        Ok(())
    }

    fn load_manifest_packages(&mut self, manifest: &BundleManifest) -> Result<()> {
        if manifest.packages().is_empty() {
            return Ok(());
        }
        if self.warm_state.is_some() {
            // Packages already included in the warm snapshot.
            for package in manifest.packages() {
                self.installed_packages.insert(package.clone());
            }
            return Ok(());
        }
        let requested: Vec<String> = manifest.packages().to_vec();
        let mut missing: Vec<String> = Vec::new();
        for package in requested {
            if self.installed_packages.contains(&package) {
                continue;
            }
            missing.push(package.clone());
        }
        if missing.is_empty() {
            return Ok(());
        }
        if self.distribution.package_root().is_none() {
            return Err(PyRunnerError::Init(
                "Pyodide package loading requires AARDVARK_PYODIDE_DIST_DIR or PyRuntimeConfig::with_pyodide_dist_dir".into(),
            ));
        }
        tracing::info!(target: "aardvark::packages", packages = ?missing, "loading packages from manifest");
        self.js.load_packages(&missing)?;
        for package in missing {
            self.installed_packages.insert(package);
        }
        self.js.prepare_dynlibs()?;
        Ok(())
    }

    fn mount_bundle(&mut self, bundle: &Bundle) -> Result<()> {
        self.js.mount_bundle(bundle, "/app")
    }

    fn reset_in_place(&mut self, config: &PyRuntimeConfig) -> Result<()> {
        self.js.reset()?;
        self.snapshot_bytes =
            load_snapshot_bytes(config, self.distribution.compatibility_fingerprint())?;
        self.warm_state = config.warm_state.clone();
        self.installed_packages.clear();
        self.register_core_assets()?;
        self.inject_version_globals(config)?;
        Ok(())
    }

    fn set_warm_state(&mut self, state: Option<WarmState>) -> Result<()> {
        if let Some(state) = state.as_ref() {
            validate_warm_state(state, self.distribution.compatibility_fingerprint())?;
        }
        self.warm_state = state;
        self.snapshot_bytes = self.warm_state.as_ref().map(|s| s.snapshot());
        if self.warm_state.is_some() {
            self.installed_packages.clear();
        }
        Ok(())
    }
}

fn load_snapshot_bytes(
    config: &PyRuntimeConfig,
    expected_fingerprint: &str,
) -> Result<Option<Arc<[u8]>>> {
    if let Some(state) = config.warm_state.as_ref() {
        validate_warm_state(state, expected_fingerprint)?;
        return Ok(Some(state.snapshot()));
    }
    if let Some(cached) = config.snapshot.cached_bytes() {
        validate_cached_snapshot(
            cached.compatibility_fingerprint.as_deref(),
            expected_fingerprint,
        )?;
        return Ok(Some(cached.bytes));
    }
    let Some(path) = config.snapshot.load_from.as_ref() else {
        return Ok(None);
    };
    validate_snapshot_metadata(path, expected_fingerprint)?;
    let bytes = read_snapshot_bytes(path)?;
    config
        .snapshot
        .store_cached_bytes(bytes.clone(), Some(expected_fingerprint));
    Ok(Some(bytes))
}

fn validate_warm_state(state: &WarmState, expected_fingerprint: &str) -> Result<()> {
    validate_compatibility_fingerprint(
        "warm state",
        state.compatibility_fingerprint(),
        expected_fingerprint,
    )
}

fn validate_cached_snapshot(found: Option<&str>, expected_fingerprint: &str) -> Result<()> {
    validate_compatibility_fingerprint("cached snapshot", found, expected_fingerprint)
}

fn validate_compatibility_fingerprint(
    label: &str,
    found: Option<&str>,
    expected_fingerprint: &str,
) -> Result<()> {
    let found = found.unwrap_or_default();
    if found != expected_fingerprint {
        return Err(PyRunnerError::Init(format!(
            "{label} compatibility fingerprint mismatch: expected {}, found {}",
            expected_fingerprint,
            if found.is_empty() { "<missing>" } else { found }
        )));
    }
    Ok(())
}

fn validate_snapshot_metadata(path: &Path, expected_fingerprint: &str) -> Result<()> {
    let metadata_path = snapshot_metadata_path(path);
    let raw = fs::read_to_string(&metadata_path).map_err(|err| {
        PyRunnerError::Init(format!(
            "snapshot {} is missing compatibility metadata {}: {err}",
            path.display(),
            metadata_path.display()
        ))
    })?;
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to parse snapshot metadata {}: {err}",
            metadata_path.display()
        ))
    })?;
    let found = value
        .get("compatibilityFingerprint")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if found != expected_fingerprint {
        return Err(PyRunnerError::Init(format!(
            "snapshot {} compatibility fingerprint mismatch: expected {}, found {}",
            path.display(),
            expected_fingerprint,
            if found.is_empty() { "<missing>" } else { found }
        )));
    }
    Ok(())
}

fn snapshot_metadata_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".aardvark.json");
    PathBuf::from(os)
}

fn read_snapshot_bytes(path: &Path) -> Result<Arc<[u8]>> {
    let data = fs::read(path).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to read snapshot from {}: {err}",
            path.display()
        ))
    })?;
    Ok(Arc::<[u8]>::from(data.into_boxed_slice()))
}
