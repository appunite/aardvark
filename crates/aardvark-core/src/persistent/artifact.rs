use std::sync::Arc;

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
