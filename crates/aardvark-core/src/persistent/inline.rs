use std::collections::HashSet;
use std::io::{Cursor, Write};

use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::bundle::Bundle;
use crate::bundle_manifest::{
    BundleManifest, ManifestResources, ManifestRuntime, MANIFEST_BASENAME, MANIFEST_SCHEMA_VERSION,
};
use crate::error::{PyRunnerError, Result};
use crate::runtime_language::RuntimeLanguage;

/// Options configuring inline Python execution without a prebuilt bundle.
#[derive(Debug, Clone, Default)]
pub struct InlinePythonOptions {
    /// Optional entrypoint override (defaults to `"main:handler"`).
    pub entrypoint: Option<String>,
    /// Optional package hints passed through the manifest.
    pub packages: Vec<String>,
    /// Optional runtime overrides copied into the manifest.
    pub runtime: Option<ManifestRuntime>,
    /// Optional sandbox resource policies copied into the manifest.
    pub resources: Option<ManifestResources>,
}

impl InlinePythonOptions {
    /// Returns the entrypoint, falling back to `main:handler`.
    pub fn entrypoint(&self) -> &str {
        self.entrypoint.as_deref().unwrap_or("main:handler")
    }

    /// Builds a bundle from the inline code and returns it with the normalised entrypoint.
    pub(crate) fn build_bundle(&self, code: &str) -> Result<(Bundle, String)> {
        let (manifest, manifest_bytes) = self.build_manifest()?;
        let entrypoint = manifest.entrypoint().to_owned();
        let (module_path, init_modules) = module_path_for_entrypoint(&entrypoint)?;
        let bundle = assemble_inline_bundle(code, &module_path, &init_modules, &manifest_bytes)?;
        Ok((bundle, entrypoint))
    }

    fn build_manifest(&self) -> Result<(BundleManifest, Vec<u8>)> {
        if let Some(runtime) = &self.runtime {
            if matches!(runtime.language, Some(RuntimeLanguage::JavaScript)) {
                return Err(PyRunnerError::Manifest(
                    "inline python options must target the python runtime".into(),
                ));
            }
        }

        let mut map = serde_json::Map::new();
        map.insert(
            "schemaVersion".to_string(),
            Value::String(MANIFEST_SCHEMA_VERSION.to_string()),
        );
        map.insert(
            "entrypoint".to_string(),
            Value::String(self.entrypoint().to_string()),
        );
        if !self.packages.is_empty() {
            map.insert(
                "packages".to_string(),
                serde_json::to_value(&self.packages).map_err(|err| {
                    PyRunnerError::Manifest(format!("failed to encode inline package list: {err}"))
                })?,
            );
        }
        if let Some(runtime) = &self.runtime {
            map.insert(
                "runtime".to_string(),
                serde_json::to_value(runtime).map_err(|err| {
                    PyRunnerError::Manifest(format!("failed to encode inline runtime block: {err}"))
                })?,
            );
        }
        if let Some(resources) = &self.resources {
            map.insert(
                "resources".to_string(),
                serde_json::to_value(resources).map_err(|err| {
                    PyRunnerError::Manifest(format!(
                        "failed to encode inline resources block: {err}"
                    ))
                })?,
            );
        }

        let manifest_value = Value::Object(map);
        let manifest_bytes = serde_json::to_vec(&manifest_value).map_err(|err| {
            PyRunnerError::Manifest(format!("failed to serialise inline manifest: {err}"))
        })?;
        let manifest = BundleManifest::from_bytes(&manifest_bytes)?;
        let manifest_bytes = serde_json::to_vec(&manifest).map_err(|err| {
            PyRunnerError::Manifest(format!(
                "failed to serialise normalised inline manifest: {err}"
            ))
        })?;
        Ok((manifest, manifest_bytes))
    }
}

fn module_path_for_entrypoint(entrypoint: &str) -> Result<(String, Vec<String>)> {
    let (module, _) = entrypoint
        .split_once(':')
        .ok_or_else(|| PyRunnerError::Manifest("entrypoint must include module:function".into()))?;
    let module = module.trim();
    if module.is_empty() {
        return Err(PyRunnerError::Manifest(
            "entrypoint must specify a module".into(),
        ));
    }

    let mut components = Vec::new();
    for token in module.split('.') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(PyRunnerError::Manifest(
                "entrypoint module cannot contain empty components".into(),
            ));
        }
        components.push(trimmed);
    }
    let mut init_modules = Vec::new();
    if components.len() > 1 {
        let mut prefix = String::new();
        for component in &components[..components.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            init_modules.push(format!("{prefix}/__init__.py"));
        }
    }
    let module_path = format!(
        "{}{}.py",
        if components.len() > 1 {
            components[..components.len() - 1].join("/") + "/"
        } else {
            String::new()
        },
        components.last().unwrap()
    );
    Ok((module_path, init_modules))
}

fn assemble_inline_bundle(
    code: &str,
    module_path: &str,
    init_modules: &[String],
    manifest_bytes: &[u8],
) -> Result<Bundle> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut seen = HashSet::new();
    for init in init_modules {
        if seen.insert(init.clone()) {
            writer
                .start_file(init, options)
                .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;
            writer
                .write_all(b"")
                .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;
        }
    }

    writer
        .start_file(module_path, options)
        .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;
    writer
        .write_all(code.as_bytes())
        .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;

    writer
        .start_file(MANIFEST_BASENAME, options)
        .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;
    writer
        .write_all(manifest_bytes)
        .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;

    let cursor = writer
        .finish()
        .map_err(|err| PyRunnerError::Bundle(err.to_string()))?;
    Bundle::from_zip_bytes(cursor.into_inner())
}
