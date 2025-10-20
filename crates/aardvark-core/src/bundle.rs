//! Utilities for handling user-provided Python bundles.

use std::fmt;
use std::io::{Cursor, Read, Seek};
use std::path::{Component, Path};
use std::sync::Arc;

use crate::bundle_manifest::{BundleManifest, MANIFEST_BASENAME};
use crate::error::{PyRunnerError, Result};
use blake3::Hasher;
use zip::read::ZipFile;
use zip::ZipArchive;

/// Representation of a file contained in a bundle.
#[derive(Clone)]
pub struct BundleEntry {
    path: Arc<str>,
    data: Arc<[u8]>,
}

impl BundleEntry {
    /// Returns the normalized relative path for this entry.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the raw contents for this entry.
    pub fn contents(&self) -> &[u8] {
        &self.data
    }
}

impl fmt::Debug for BundleEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BundleEntry")
            .field("path", &self.path)
            .field("len", &self.data.len())
            .finish()
    }
}

/// An in-memory bundle extracted from a ZIP archive.
#[derive(Clone, Default)]
pub struct Bundle {
    inner: Arc<BundleInner>,
}

#[derive(Default)]
struct BundleInner {
    entries: Vec<BundleEntry>,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("entries", &self.inner.entries)
            .finish()
    }
}

impl Bundle {
    /// Constructs a bundle from a ZIP archive held entirely in memory.
    ///
    /// Cloning the resulting `Bundle` is inexpensive; the file data is reference
    /// counted internally so you can parse once and reuse it across invocations.
    pub fn from_zip_bytes(bytes: impl AsRef<[u8]>) -> Result<Self> {
        let cursor = Cursor::new(bytes.as_ref().to_vec());
        Self::from_reader(cursor)
    }

    /// Constructs a bundle from any `Read + Seek` ZIP archive.
    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        let mut archive = ZipArchive::new(reader)
            .map_err(|err| PyRunnerError::Bundle(format!("invalid zip archive: {err}")))?;
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let file = archive
                .by_index(i)
                .map_err(|err| PyRunnerError::Bundle(format!("zip access error: {err}")))?;
            if file.is_dir() {
                continue;
            }
            let normalized = normalize_entry_path(file.name()).map_err(|err| {
                PyRunnerError::Bundle(format!("invalid entry '{}': {err}", file.name()))
            })?;
            let data = read_zip_file(file)?;
            entries.push(BundleEntry {
                path: Arc::<str>::from(normalized),
                data: Arc::<[u8]>::from(data),
            });
        }
        if entries.is_empty() {
            return Err(PyRunnerError::Bundle(
                "bundle did not contain any files".to_owned(),
            ));
        }
        Ok(Self {
            inner: Arc::new(BundleInner { entries }),
        })
    }

    /// Returns all entries in this bundle.
    pub fn entries(&self) -> &[BundleEntry] {
        &self.inner.entries
    }

    /// Returns a stable fingerprint for the bundle contents.
    pub fn fingerprint(&self) -> BundleFingerprint {
        let mut hasher = Hasher::new();
        let mut entries: Vec<_> = self
            .inner
            .entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.data.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, data) in entries {
            hasher.update(path.as_bytes());
            let len_bytes = (data.len() as u64).to_le_bytes();
            hasher.update(&len_bytes);
            hasher.update(&data);
        }
        BundleFingerprint(hasher.finalize().as_bytes()[..8].try_into().unwrap())
    }

    /// Parses and returns the embedded bundle manifest, if present.
    pub fn manifest(&self) -> Result<Option<BundleManifest>> {
        let entry = self
            .inner
            .entries
            .iter()
            .find(|entry| entry.path.as_ref() == MANIFEST_BASENAME);
        match entry {
            Some(manifest_entry) => {
                let manifest = BundleManifest::from_bytes(manifest_entry.contents())?;
                Ok(Some(manifest))
            }
            None => Ok(None),
        }
    }

    /// Consumes the bundle and returns its entries.
    pub fn into_entries(self) -> Vec<BundleEntry> {
        Arc::try_unwrap(self.inner)
            .map(|inner| inner.entries)
            .unwrap_or_else(|inner| inner.entries.clone())
    }
}

/// Eight-byte bundle fingerprint derived from a BLAKE3 hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BundleFingerprint([u8; 8]);

impl BundleFingerprint {
    /// Returns the fingerprint as a u64 integer.
    pub fn as_u64(&self) -> u64 {
        u64::from_le_bytes(self.0)
    }
}

fn read_zip_file(mut file: ZipFile<'_>) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| PyRunnerError::Bundle(format!("failed to read '{}': {err}", file.name())))?;
    Ok(buf)
}

fn normalize_entry_path(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(PyRunnerError::Bundle("entry has empty name".into()));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(PyRunnerError::Bundle(
            "absolute paths are not allowed".into(),
        ));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(PyRunnerError::Bundle("unsupported path prefix".into()))
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(PyRunnerError::Bundle(
                        "path traversal outside bundle root is not allowed".into(),
                    ));
                }
            }
            Component::Normal(token) => {
                let segment = token.to_str().ok_or_else(|| {
                    PyRunnerError::Bundle("non-utf8 path segments not supported".into())
                })?;
                if segment.is_empty() {
                    return Err(PyRunnerError::Bundle(
                        "empty path segment encountered".into(),
                    ));
                }
                parts.push(segment.to_owned());
            }
        }
    }
    if parts.is_empty() {
        return Err(PyRunnerError::Bundle("entry resolves to empty path".into()));
    }
    Ok(parts.join("/"))
}
