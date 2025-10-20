use crate::assets;
use crate::bundle::Bundle;
use crate::bundle_manifest::BundleManifest;
use crate::config::PyRuntimeConfig;
use crate::engine::JsRuntime;
use crate::error::{PyRunnerError, Result};
use crate::runtime_language::RuntimeLanguage;

use super::LanguageEngine;

pub struct JavaScriptEngine {
    js: JsRuntime,
}

impl JavaScriptEngine {
    pub fn new(_config: &PyRuntimeConfig) -> Result<Self> {
        let js = JsRuntime::new()?;
        js.insert_text_asset("js_runtime_bootstrap.js", assets::js_runtime_bootstrap_js());
        let mut engine = Self { js };
        engine.js.ensure_module("js_runtime_bootstrap.js")?;
        Ok(engine)
    }
}

impl LanguageEngine for JavaScriptEngine {
    fn language(&self) -> RuntimeLanguage {
        RuntimeLanguage::JavaScript
    }

    fn js_mut(&mut self) -> &mut JsRuntime {
        &mut self.js
    }

    fn prepare_environment(&mut self, _config: &PyRuntimeConfig) -> Result<()> {
        self.js.ensure_module("js_runtime_bootstrap.js")?;
        Ok(())
    }

    fn load_manifest_packages(&mut self, manifest: &BundleManifest) -> Result<()> {
        if !manifest.packages().is_empty() {
            return Err(PyRunnerError::Manifest(
                "javascript runtime does not support manifest packages".into(),
            ));
        }
        Ok(())
    }

    fn mount_bundle(&mut self, bundle: &Bundle) -> Result<()> {
        for entry in bundle.entries() {
            if let Ok(text) = std::str::from_utf8(entry.contents()) {
                self.js.insert_text_asset(entry.path(), text.to_owned());
            } else {
                self.js
                    .insert_binary_asset_owned(entry.path(), entry.contents().to_vec());
            }
        }
        Ok(())
    }

    fn reset_in_place(&mut self, _config: &PyRuntimeConfig) -> Result<()> {
        self.js.reset()?;
        self.js
            .insert_text_asset("js_runtime_bootstrap.js", assets::js_runtime_bootstrap_js());
        self.js.ensure_module("js_runtime_bootstrap.js")
    }
}
