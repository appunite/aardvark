//! Python-specific engine backed by the embedded Pyodide runtime.

use crate::assets;
use crate::bundle_manifest::BundleManifest;
use crate::config::PyRuntimeConfig;
use crate::engine::{JsRuntime, PyodideLoadOptions};
use crate::error::{PyRunnerError, Result};
use crate::package_metadata;
use crate::runtime_language::RuntimeLanguage;
use v8;

use super::LanguageEngine;
use std::fs;
use std::path::Path;

pub struct PythonEngine {
    js: JsRuntime,
    snapshot_bytes: Option<Vec<u8>>,
}

impl PythonEngine {
    pub fn new(config: &PyRuntimeConfig) -> Result<Self> {
        let js = JsRuntime::new()?;
        let mut engine = Self {
            js,
            snapshot_bytes: load_snapshot_bytes(config)?,
        };
        engine.register_core_assets();
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

    fn register_core_assets(&self) {
        let js = &self.js;
        js.insert_binary_asset("pyodide.asm.wasm", assets::wasm());
        js.insert_text_asset("pyodide.asm.js", assets::pyodide_asm_js());
        js.insert_text_asset("pyodide.asm.patched.js", assets::pyodide_asm_patched_js());
        js.insert_text_asset("pyodide_builtin_wrappers.js", assets::builtin_wrappers_js());
        js.insert_text_asset("pyodide_bootstrap.js", assets::bootstrap_js());
        js.insert_text_asset("pyodide_emscripten_setup.js", assets::emscripten_setup_js());
        js.insert_text_asset("pyodide_packages.js", assets::packages_js());
        js.insert_text_asset("pyodide.mjs", assets::loader_mjs());
        js.insert_text_asset("pyodide.js", assets::loader_js());
        js.insert_binary_asset("python_stdlib.zip", assets::python_stdlib_zip());
        js.insert_text_asset(
            "pyodide-lock.json",
            package_metadata::package_metadata_json(),
        );
        let capnp_lock = package_metadata::package_metadata_capnp();
        js.insert_binary_asset_owned("pyodide-lock.capnp", capnp_lock.as_ref().to_vec());
        js.insert_text_asset("entropy/allow_entropy.py", assets::entropy_allow_py());
        js.insert_text_asset(
            "entropy/entropy_import_context.py",
            assets::entropy_import_context_py(),
        );
        js.insert_text_asset("entropy/entropy_patches.py", assets::entropy_patches_py());
        js.insert_text_asset(
            "entropy/import_patch_manager.py",
            assets::entropy_import_patch_manager_py(),
        );
    }
}

impl LanguageEngine for PythonEngine {
    fn language(&self) -> RuntimeLanguage {
        RuntimeLanguage::Python
    }

    fn js_mut(&mut self) -> &mut JsRuntime {
        &mut self.js
    }

    fn prepare_environment(&mut self, config: &PyRuntimeConfig) -> Result<()> {
        self.ensure_pyodide_module()?;
        let make_snapshot = config.snapshot.save_to.is_some();
        let snapshot_owned = self.snapshot_bytes.clone();
        let load_opts = PyodideLoadOptions {
            snapshot: snapshot_owned.as_deref(),
            make_snapshot,
        };
        self.js.load_pyodide(load_opts)?;
        self.snapshot_bytes = None;
        Ok(())
    }

    fn load_manifest_packages(&mut self, manifest: &BundleManifest) -> Result<()> {
        if manifest.packages().is_empty() {
            return Ok(());
        }
        self.js.load_packages(manifest.packages())?;
        self.js.prepare_dynlibs()?;
        Ok(())
    }
}

fn load_snapshot_bytes(config: &PyRuntimeConfig) -> Result<Option<Vec<u8>>> {
    if let Some(path) = config.snapshot.load_from.as_ref() {
        read_snapshot_bytes(path).map(Some)
    } else {
        Ok(None)
    }
}

fn read_snapshot_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to read snapshot from {}: {err}",
            path.display()
        ))
    })
}
