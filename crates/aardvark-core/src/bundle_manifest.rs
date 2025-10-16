//! Bundle manifest structures exposed to host integrations and bundle authors.
//!
//! The manifest travels inside the ZIP archive shipped by users. It augments the
//! runtime descriptor with package hints, resource policies, and (eventually)
//! additional metadata for other host features. All parsing goes through
//! [`BundleManifest::from_bytes`], which normalises whitespace, deduplicates
//! lists, and validates schema rules.

use std::collections::HashSet;

use serde::Deserialize;

use crate::assets::PYODIDE_VERSION;
use crate::error::{PyRunnerError, Result};
use crate::runtime_language::RuntimeLanguage;

/// Canonical filename for the manifest within the bundle archive.
pub const MANIFEST_BASENAME: &str = "aardvark.manifest.json";
/// Current schema version supported by the runtime.
pub const MANIFEST_SCHEMA_VERSION: &str = "1.0";
/// Embedded JSON schema used by tooling for validation.
pub const MANIFEST_SCHEMA: &str = include_str!("../schemas/aardvark.bundle-manifest.schema.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Normalised view of `aardvark.manifest.json`.
pub struct BundleManifest {
    /// Schema version string (must equal [`MANIFEST_SCHEMA_VERSION`]).
    pub schema_version: String,
    /// Entrypoint formatted as `module:function`.
    pub entrypoint: String,
    /// Optional packages that the runtime should preload inside Pyodide.
    /// Ignored when the selected language is JavaScript.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Optional runtime selection and language-specific constraints.
    #[serde(default)]
    pub runtime: Option<ManifestRuntime>,
    /// Optional sandbox resource policies.
    #[serde(default)]
    pub resources: Option<ManifestResources>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Runtime-specific manifest configuration selected per bundle.
pub struct ManifestRuntime {
    /// Desired guest language runtime.
    #[serde(default)]
    pub language: Option<RuntimeLanguage>,
    /// Optional Pyodide configuration block. Only respected when `language`
    /// resolves to [`RuntimeLanguage::Python`].
    #[serde(default)]
    pub pyodide: Option<ManifestPyodide>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Pyodide-specific overrides applied when Python is selected.
pub struct ManifestPyodide {
    /// Optional Pyodide version requirement.
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Resource policy hints the runtime should enforce per invocation.
pub struct ManifestResources {
    /// CPU budget applied when the descriptor omits one.
    #[serde(default)]
    pub cpu: Option<ManifestCpuResources>,
    /// Network allowlist and HTTPS setting.
    #[serde(default)]
    pub network: Option<ManifestNetworkResources>,
    /// Filesystem access mode and quota.
    #[serde(default)]
    pub filesystem: Option<ManifestFilesystemResources>,
    /// Host capability names required by the bundle.
    #[serde(default)]
    pub host_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
/// CPU-related defaults.
pub struct ManifestCpuResources {
    /// Optional per-invocation CPU budget in milliseconds.
    #[serde(default)]
    pub default_limit_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Network sandbox configuration.
pub struct ManifestNetworkResources {
    /// Allowed hosts (exact or wildcard suffix, with optional port).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Whether HTTPS is required for outbound requests (defaults to `true`).
    #[serde(default = "ManifestNetworkResources::default_https_only")]
    pub https_only: bool,
}

impl Default for ManifestNetworkResources {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            https_only: true,
        }
    }
}

impl ManifestNetworkResources {
    const fn default_https_only() -> bool {
        true
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
/// Filesystem sandbox configuration.
pub struct ManifestFilesystemResources {
    /// Writable mode (defaults to read-only).
    #[serde(default)]
    pub mode: Option<ManifestFilesystemMode>,
    /// Optional byte quota enforced when write mode is enabled.
    #[serde(default)]
    pub quota_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Filesystem access level requested by the bundle.
pub enum ManifestFilesystemMode {
    /// Mount the session directory read-only.
    Read,
    /// Allow writes under `/session` (subject to quota enforcement).
    ReadWrite,
}

impl BundleManifest {
    /// Parse the manifest from raw bytes, performing normalisation and validation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut manifest: BundleManifest = serde_json::from_slice(bytes).map_err(|err| {
            PyRunnerError::Manifest(format!("failed to parse manifest JSON: {err}"))
        })?;
        manifest.normalize()?;
        Ok(manifest)
    }

    /// Returns the canonical entrypoint (`module:function`).
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    /// Returns the list of packages requested by the manifest.
    pub fn packages(&self) -> &[String] {
        &self.packages
    }

    /// Returns optional resource policies.
    pub fn resources(&self) -> Option<&ManifestResources> {
        self.resources.as_ref()
    }

    fn normalize(&mut self) -> Result<()> {
        if self.schema_version.trim() != MANIFEST_SCHEMA_VERSION {
            return Err(PyRunnerError::Manifest(format!(
                "unsupported manifest schema version '{}'; expected {}",
                self.schema_version, MANIFEST_SCHEMA_VERSION
            )));
        }

        let trimmed_entrypoint = self.entrypoint.trim();
        let (module, function) = trimmed_entrypoint.split_once(':').ok_or_else(|| {
            PyRunnerError::Manifest(format!(
                "entrypoint '{}' must be formatted as module:function",
                trimmed_entrypoint
            ))
        })?;
        if module.trim().is_empty() || function.trim().is_empty() {
            return Err(PyRunnerError::Manifest(
                "entrypoint must include both module and function names".into(),
            ));
        }
        self.entrypoint = format!("{}:{}", module.trim(), function.trim());

        if let Some(runtime) = &self.runtime {
            if matches!(runtime.language, Some(RuntimeLanguage::JavaScript)) {
                if !self.packages.is_empty() {
                    return Err(PyRunnerError::Manifest(
                        "javascript runtime bundles must inline dependencies; 'packages' is not supported".into(),
                    ));
                }
                if runtime.pyodide.is_some() {
                    return Err(PyRunnerError::Manifest(
                        "pyodide configuration is unsupported when runtime.language is 'javascript'".into(),
                    ));
                }
            }
        }

        let mut seen = HashSet::new();
        let mut normalized = Vec::with_capacity(self.packages.len());
        for pkg in self.packages.iter() {
            let trimmed = pkg.trim();
            if trimmed.is_empty() {
                return Err(PyRunnerError::Manifest(
                    "package names cannot be empty strings".into(),
                ));
            }
            let lowered = trimmed.to_ascii_lowercase();
            if seen.insert(lowered.clone()) {
                normalized.push(trimmed.to_string());
            }
        }
        self.packages = normalized;

        if let Some(runtime) = &self.runtime {
            if let Some(pyodide) = &runtime.pyodide {
                if let Some(version) = pyodide.version.as_ref() {
                    if version.trim() != PYODIDE_VERSION {
                        return Err(PyRunnerError::Manifest(format!(
                            "manifest targets Pyodide {}, but runtime is bundled with {}",
                            version.trim(),
                            PYODIDE_VERSION
                        )));
                    }
                }
            }
        }

        if let Some(resources) = &mut self.resources {
            resources.normalize()?;
        }

        Ok(())
    }
}

impl ManifestResources {
    fn normalize(&mut self) -> Result<()> {
        if let Some(cpu) = &self.cpu {
            if matches!(cpu.default_limit_ms, Some(0)) {
                return Err(PyRunnerError::Manifest(
                    "resources.cpu.defaultLimitMs must be greater than zero".into(),
                ));
            }
        }

        if let Some(network) = &mut self.network {
            let mut dedup = HashSet::new();
            let mut normalized = Vec::with_capacity(network.allow.len());
            for host in network.allow.iter() {
                let trimmed = host.trim();
                if trimmed.is_empty() {
                    return Err(PyRunnerError::Manifest(
                        "resources.network.allow entries cannot be empty".into(),
                    ));
                }
                let lowered = trimmed.to_ascii_lowercase();
                if dedup.insert(lowered) {
                    normalized.push(trimmed.to_string());
                }
            }
            network.allow = normalized;
        }

        if let Some(filesystem) = &self.filesystem {
            if matches!(filesystem.quota_bytes, Some(0)) {
                return Err(PyRunnerError::Manifest(
                    "resources.filesystem.quotaBytes must be positive when specified".into(),
                ));
            }
        }

        if !self.host_capabilities.is_empty() {
            let mut dedup = HashSet::new();
            let mut normalized = Vec::with_capacity(self.host_capabilities.len());
            for capability in self.host_capabilities.iter() {
                let trimmed = capability.trim();
                if trimmed.is_empty() {
                    return Err(PyRunnerError::Manifest(
                        "resources.hostCapabilities entries cannot be empty".into(),
                    ));
                }
                let lowered = trimmed.to_ascii_lowercase();
                if dedup.insert(lowered) {
                    normalized.push(trimmed.to_string());
                }
            }
            self.host_capabilities = normalized;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let json = format!(
            "{{\n            \"schemaVersion\": \"1.0\",\n            \"entrypoint\": \"main:run\",\n            \"packages\": [\"Pandas\", \"numpy\"],\n            \"runtime\": {{\"language\": \"python\", \"pyodide\": {{\"version\": \"{}\"}}}},\n            \"resources\": {{\n                \"cpu\": {{\"defaultLimitMs\": 5000}},\n                \"network\": {{\"allow\": [\"Example.com\", \"api.example.com\"], \"httpsOnly\": true}},\n                \"filesystem\": {{\"mode\": \"readWrite\", \"quotaBytes\": 1048576}},\n                \"hostCapabilities\": [\"rawctx_buffers\", \"rawctx_buffers\"]\n            }}\n        }}",
            PYODIDE_VERSION
        );

        let manifest = BundleManifest::from_bytes(json.as_bytes()).expect("manifest parses");
        assert_eq!(manifest.entrypoint(), "main:run");
        assert_eq!(
            manifest.packages(),
            &["Pandas".to_string(), "numpy".to_string()]
        );
        let resources = manifest.resources().expect("resources present");
        assert_eq!(
            resources.cpu.as_ref().and_then(|cpu| cpu.default_limit_ms),
            Some(5_000)
        );
        let network = resources.network.as_ref().expect("network present");
        assert_eq!(
            network.allow,
            vec!["Example.com".to_string(), "api.example.com".to_string()]
        );
        assert!(network.https_only);
        let filesystem = resources.filesystem.as_ref().expect("filesystem present");
        assert_eq!(filesystem.mode, Some(ManifestFilesystemMode::ReadWrite));
        assert_eq!(filesystem.quota_bytes, Some(1_048_576));
        assert_eq!(
            resources.host_capabilities,
            vec!["rawctx_buffers".to_string()]
        );
        let runtime = manifest.runtime.as_ref().expect("runtime present");
        assert_eq!(runtime.language, Some(RuntimeLanguage::Python));
    }

    #[test]
    fn manifest_rejects_bad_entrypoint() {
        let json = r#"{"schemaVersion":"1.0","entrypoint":"invalid","packages":[]}"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, PyRunnerError::Manifest(_)));
    }

    #[test]
    fn manifest_rejects_wrong_version() {
        let json = r#"{"schemaVersion":"9.9","entrypoint":"main:run"}"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, PyRunnerError::Manifest(_)));
    }

    #[test]
    fn manifest_rejects_empty_resource_entries() {
        let json = r#"{
            "schemaVersion": "1.0",
            "entrypoint": "main:run",
            "resources": {
                "network": {
                    "allow": [""]
                }
            }
        }"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, PyRunnerError::Manifest(_)));
    }

    #[test]
    fn manifest_rejects_zero_cpu_limit() {
        let json = r#"{
            "schemaVersion": "1.0",
            "entrypoint": "main:run",
            "resources": {
                "cpu": {
                    "defaultLimitMs": 0
                }
            }
        }"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, PyRunnerError::Manifest(_)));
    }

    #[test]
    fn manifest_rejects_packages_for_js_runtime() {
        let json = r#"{
            "schemaVersion": "1.0",
            "entrypoint": "app:main",
            "packages": ["leftpad"],
            "runtime": { "language": "javascript" }
        }"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(
            matches!(err, PyRunnerError::Manifest(_)),
            "expected manifest error, got {err:?}"
        );
    }

    #[test]
    fn manifest_rejects_pyodide_block_for_js_runtime() {
        let json = r#"{
            "schemaVersion": "1.0",
            "entrypoint": "app:main",
            "runtime": { "language": "javascript", "pyodide": { "version": "0.23.0" } }
        }"#;
        let err = BundleManifest::from_bytes(json.as_bytes()).unwrap_err();
        assert!(
            matches!(err, PyRunnerError::Manifest(_)),
            "expected manifest error, got {err:?}"
        );
    }

    #[test]
    fn manifest_allows_minimal_js_bundle() {
        let json = r#"{
            "schemaVersion": "1.0",
            "entrypoint": "main:default",
            "runtime": { "language": "javascript" }
        }"#;
        let manifest = BundleManifest::from_bytes(json.as_bytes()).expect("manifest parses");
        assert!(manifest.packages().is_empty());
        assert_eq!(manifest.entrypoint(), "main:default");
        assert_eq!(
            manifest.runtime.as_ref().and_then(|rt| rt.language),
            Some(RuntimeLanguage::JavaScript)
        );
    }
}
