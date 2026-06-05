use std::sync::Arc;

use super::InlinePythonOptions;
use crate::bundle::{Bundle, BundleFingerprint};
use crate::bundle_manifest::BundleManifest;
use crate::error::Result;
use crate::invocation::InvocationDescriptor;
use crate::runtime_language::RuntimeLanguage;

/// Immutable metadata describing a normalised bundle.
#[derive(Clone)]
pub struct BundleArtifact {
    bundle: Bundle,
    manifest: Option<BundleManifest>,
    fingerprint: BundleFingerprint,
    entrypoint: String,
    language: RuntimeLanguage,
    pyodide_distribution_profile: Option<String>,
}

impl BundleArtifact {
    /// Parses bundle bytes and normalises manifest metadata.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Arc<Self>> {
        let bundle = Bundle::from_zip_bytes(bytes)?;
        Self::from_bundle(bundle)
    }

    /// Normalises an in-memory bundle and returns a shared artifact.
    pub fn from_bundle(bundle: Bundle) -> Result<Arc<Self>> {
        Self::from_bundle_inner(bundle, None)
    }

    /// Normalises an in-memory bundle while overriding the default entrypoint.
    ///
    /// This is useful for host surfaces such as CLIs that accept an explicit
    /// entrypoint flag while still preserving the bundle manifest's language and
    /// package metadata.
    pub fn from_bundle_with_entrypoint(
        bundle: Bundle,
        entrypoint: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let descriptor = InvocationDescriptor::new(entrypoint);
        Self::from_bundle_inner(bundle, Some(descriptor.entrypoint().to_owned()))
    }

    fn from_bundle_inner(bundle: Bundle, entrypoint_override: Option<String>) -> Result<Arc<Self>> {
        let manifest = bundle.manifest()?;
        let fingerprint = bundle.fingerprint();
        let (language, entrypoint, pyodide_distribution_profile) = match &manifest {
            Some(manifest) => {
                let language = manifest
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.language)
                    .unwrap_or(RuntimeLanguage::Python);
                let entrypoint = manifest.entrypoint().to_owned();
                let pyodide_distribution_profile =
                    manifest.pyodide_distribution_profile().map(str::to_owned);
                (language, entrypoint, pyodide_distribution_profile)
            }
            None => (RuntimeLanguage::Python, "main:handler".to_string(), None),
        };
        let entrypoint = entrypoint_override.unwrap_or(entrypoint);
        Ok(Arc::new(Self {
            bundle,
            manifest,
            fingerprint,
            entrypoint,
            language,
            pyodide_distribution_profile,
        }))
    }

    /// Builds an artifact from inline Python code and manifest-style options.
    pub fn from_inline_python(code: &str, options: InlinePythonOptions) -> Result<Arc<Self>> {
        let (bundle, _) = options.build_bundle(code)?;
        Self::from_bundle(bundle)
    }

    /// Returns a clone of the underlying bundle.
    pub fn bundle(&self) -> Bundle {
        self.bundle.clone()
    }

    /// Returns the parsed manifest, if present.
    pub fn manifest(&self) -> Option<&BundleManifest> {
        self.manifest.as_ref()
    }

    /// Returns the normalised default entrypoint.
    pub fn entrypoint(&self) -> &str {
        self.entrypoint.as_str()
    }

    /// Returns the derived runtime language for this bundle.
    pub fn language(&self) -> RuntimeLanguage {
        self.language
    }

    /// Returns the bundle-requested Pyodide distribution profile, if any.
    pub fn pyodide_distribution_profile(&self) -> Option<&str> {
        self.pyodide_distribution_profile.as_deref()
    }

    /// Returns the stable bundle fingerprint.
    pub fn fingerprint(&self) -> BundleFingerprint {
        self.fingerprint
    }

    /// Creates an invocation descriptor seeded with the default entrypoint and language.
    pub fn default_descriptor(&self) -> InvocationDescriptor {
        let mut descriptor = InvocationDescriptor::new(self.entrypoint.clone());
        self.apply_manifest_descriptor_defaults(&mut descriptor);
        descriptor
    }

    pub(crate) fn apply_manifest_descriptor_defaults(&self, descriptor: &mut InvocationDescriptor) {
        descriptor.language = descriptor.language.or(Some(self.language));
        if let Some(cpu_limit) = self
            .manifest
            .as_ref()
            .and_then(BundleManifest::resources)
            .and_then(|resources| resources.cpu.as_ref())
            .and_then(|cpu| cpu.default_limit_ms)
        {
            descriptor.limits.cpu_ms = Some(cpu_limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_manifest::{
        ManifestCpuResources, ManifestFilesystemMode, ManifestFilesystemResources,
        ManifestNetworkResources, ManifestResources,
    };
    use crate::BUNDLE_MANIFEST_BASENAME;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    #[test]
    fn inline_artifact_embeds_manifest_and_code() -> Result<()> {
        let options = InlinePythonOptions {
            entrypoint: Some("analytics.echo:handler".to_string()),
            packages: vec![
                "NumPy".to_string(),
                "numpy".to_string(),
                "pandas".to_string(),
            ],
            resources: Some(ManifestResources {
                cpu: Some(ManifestCpuResources {
                    default_limit_ms: Some(750),
                }),
                network: Some(ManifestNetworkResources {
                    allow: vec!["Api.Example.com".to_string()],
                    https_only: false,
                }),
                filesystem: Some(ManifestFilesystemResources {
                    mode: Some(ManifestFilesystemMode::ReadWrite),
                    quota_bytes: Some(2048),
                }),
                host_capabilities: vec!["rawctx_buffers".to_string(), "RAWCTX_BUFFERS".to_string()],
            }),
            ..InlinePythonOptions::default()
        };

        let code = r#"import numpy as np

def handler():
    return f"numpy:{np.__name__}"
"#;

        let artifact = BundleArtifact::from_inline_python(code, options)?;
        assert_eq!(artifact.entrypoint(), "analytics.echo:handler");
        assert_eq!(artifact.language(), RuntimeLanguage::Python);

        let manifest = artifact
            .manifest()
            .expect("inline artifact should embed manifest");
        assert_eq!(
            manifest.packages(),
            &["NumPy".to_string(), "pandas".to_string()]
        );
        let resources = manifest
            .resources()
            .expect("resources block should be preserved");
        assert_eq!(
            resources.cpu.as_ref().and_then(|cpu| cpu.default_limit_ms),
            Some(750)
        );
        let network = resources.network.as_ref().expect("network block present");
        assert_eq!(network.allow, vec!["Api.Example.com".to_string()]);
        assert!(!network.https_only);
        let filesystem = resources
            .filesystem
            .as_ref()
            .expect("filesystem block present");
        assert_eq!(filesystem.mode, Some(ManifestFilesystemMode::ReadWrite));
        assert_eq!(filesystem.quota_bytes, Some(2048));
        assert_eq!(
            resources.host_capabilities,
            vec!["rawctx_buffers".to_string()]
        );

        let entries: Vec<_> = artifact
            .bundle()
            .entries()
            .iter()
            .map(|entry| entry.path().to_string())
            .collect();
        assert!(entries.contains(&"analytics/__init__.py".to_string()));
        assert!(entries.contains(&"analytics/echo.py".to_string()));
        assert!(entries.contains(&BUNDLE_MANIFEST_BASENAME.to_string()));

        Ok(())
    }

    #[test]
    fn artifact_entrypoint_override_preserves_manifest_language() -> Result<()> {
        let options = InlinePythonOptions {
            entrypoint: Some("main:handler".to_string()),
            ..InlinePythonOptions::default()
        };
        let (bundle, _) = options.build_bundle(
            r#"
def handler():
    return "default"

def custom():
    return "override"
"#,
        )?;

        let artifact = BundleArtifact::from_bundle_with_entrypoint(bundle, "  main:custom  ")?;
        assert_eq!(artifact.entrypoint(), "main:custom");
        assert_eq!(artifact.language(), RuntimeLanguage::Python);
        assert!(artifact.manifest().is_some());
        Ok(())
    }

    #[test]
    fn artifact_preserves_pyodide_distribution_profile() -> Result<()> {
        let mut bytes = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut bytes);
            let mut writer = zip::ZipWriter::new(cursor);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            writer.start_file("main.py", options).unwrap();
            writer.write_all(b"def handler():\n    return 1\n").unwrap();
            writer
                .start_file(BUNDLE_MANIFEST_BASENAME, options)
                .unwrap();
            writer
                .write_all(
                    br#"{
                    "schemaVersion": "1.0",
                    "entrypoint": "main:handler",
                    "runtime": {
                        "language": "python",
                        "pyodide": {"profile": "BLAS"}
                    }
                }"#,
                )
                .unwrap();
            writer.finish().unwrap();
        }

        let artifact = BundleArtifact::from_bytes(bytes)?;
        assert_eq!(artifact.pyodide_distribution_profile(), Some("blas"));
        Ok(())
    }
}
