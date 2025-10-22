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
}

impl BundleArtifact {
    /// Parses bundle bytes and normalises manifest metadata.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Arc<Self>> {
        let bundle = Bundle::from_zip_bytes(bytes)?;
        Self::from_bundle(bundle)
    }

    /// Normalises an in-memory bundle and returns a shared artifact.
    pub fn from_bundle(bundle: Bundle) -> Result<Arc<Self>> {
        let manifest = bundle.manifest()?;
        let fingerprint = bundle.fingerprint();
        let (language, entrypoint) = match &manifest {
            Some(manifest) => {
                let language = manifest
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.language)
                    .unwrap_or(RuntimeLanguage::Python);
                let entrypoint = manifest.entrypoint().to_owned();
                (language, entrypoint)
            }
            None => (RuntimeLanguage::Python, "main:handler".to_string()),
        };

        Ok(Arc::new(Self {
            bundle,
            manifest,
            fingerprint,
            entrypoint,
            language,
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

    /// Returns the stable bundle fingerprint.
    pub fn fingerprint(&self) -> BundleFingerprint {
        self.fingerprint
    }

    /// Creates an invocation descriptor seeded with the default entrypoint and language.
    pub fn default_descriptor(&self) -> InvocationDescriptor {
        let mut descriptor = InvocationDescriptor::new(self.entrypoint.clone());
        descriptor.language = Some(self.language);
        descriptor
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

    #[test]
    fn inline_artifact_embeds_manifest_and_code() -> Result<()> {
        let mut options = InlinePythonOptions::default();
        options.entrypoint = Some("analytics.echo:handler".to_string());
        options.packages = vec![
            "NumPy".to_string(),
            "numpy".to_string(),
            "pandas".to_string(),
        ];
        options.resources = Some(ManifestResources {
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
        });

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
}
