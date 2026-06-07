//! Utilities for handling user-provided Python bundles.

use std::fmt;
use std::io::{Cursor, Read, Seek};
use std::path::{Component, Path};
use std::sync::Arc;

use crate::bundle_manifest::{BundleManifest, MANIFEST_BASENAME};
use crate::error::{BundleLimitKind, PyRunnerError, Result};
use blake3::Hasher;
use zip::read::ZipFile;
use zip::ZipArchive;

const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_MAX_ENTRIES: usize = 10_000;
const DEFAULT_MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;

/// Read limits applied while normalising a user-provided bundle ZIP archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BundleLimits {
    /// Maximum compressed archive size accepted by `from_zip_bytes`.
    pub max_archive_bytes: u64,
    /// Maximum number of ZIP records in the archive, including directories.
    pub max_entries: usize,
    /// Maximum uncompressed size of a single bundle file.
    pub max_entry_bytes: u64,
    /// Maximum uncompressed size across all bundle files.
    pub max_total_uncompressed_bytes: u64,
}

impl Default for BundleLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_total_uncompressed_bytes: DEFAULT_MAX_TOTAL_UNCOMPRESSED_BYTES,
        }
    }
}

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
        Self::from_zip_bytes_with_limits(bytes, BundleLimits::default())
    }

    /// Constructs a bundle from ZIP bytes using explicit read limits.
    pub fn from_zip_bytes_with_limits(
        bytes: impl AsRef<[u8]>,
        limits: BundleLimits,
    ) -> Result<Self> {
        let bytes = bytes.as_ref();
        ensure_limit(
            BundleLimitKind::ArchiveBytes,
            None,
            bytes.len() as u64,
            limits.max_archive_bytes,
        )?;
        let cursor = Cursor::new(bytes);
        Self::from_reader_with_limits(cursor, limits)
    }

    /// Constructs a bundle from any `Read + Seek` ZIP archive.
    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        Self::from_reader_with_limits(reader, BundleLimits::default())
    }

    /// Constructs a bundle from any `Read + Seek` ZIP archive using explicit read limits.
    pub fn from_reader_with_limits<R: Read + Seek>(
        reader: R,
        limits: BundleLimits,
    ) -> Result<Self> {
        let mut archive = ZipArchive::new(reader)
            .map_err(|err| PyRunnerError::Bundle(format!("invalid zip archive: {err}")))?;
        ensure_limit(
            BundleLimitKind::EntryCount,
            None,
            archive.len() as u64,
            limits.max_entries as u64,
        )?;
        let mut entries = Vec::with_capacity(archive.len());
        let mut total_uncompressed = 0_u64;
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
            let declared_size = file.size();
            ensure_limit(
                BundleLimitKind::EntryBytes,
                Some(&normalized),
                declared_size,
                limits.max_entry_bytes,
            )?;
            let prospective_total =
                total_uncompressed.saturating_add(declared_size.min(limits.max_entry_bytes));
            ensure_limit(
                BundleLimitKind::TotalUncompressedBytes,
                None,
                prospective_total,
                limits.max_total_uncompressed_bytes,
            )?;
            let remaining_total = limits
                .max_total_uncompressed_bytes
                .saturating_sub(total_uncompressed);
            let data = read_zip_file(
                file,
                &normalized,
                limits.max_entry_bytes.min(remaining_total),
            )?;
            total_uncompressed = total_uncompressed.saturating_add(data.len() as u64);
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
        let digest = hasher.finalize();
        let mut fingerprint = [0_u8; 8];
        fingerprint.copy_from_slice(&digest.as_bytes()[..8]);
        BundleFingerprint(fingerprint)
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

fn read_zip_file<R: Read>(file: ZipFile<'_, R>, path: &str, limit: u64) -> Result<Vec<u8>> {
    let read_limit = limit.saturating_add(1);
    let capacity = usize::try_from(file.size().min(limit)).unwrap_or(usize::MAX);
    let mut buf = Vec::with_capacity(capacity);
    let mut limited = file.take(read_limit);
    limited
        .read_to_end(&mut buf)
        .map_err(|err| PyRunnerError::Bundle(format!("failed to read '{path}': {err}")))?;
    if buf.len() as u64 > limit {
        return Err(PyRunnerError::BundleLimitExceeded {
            kind: BundleLimitKind::EntryBytes,
            path: Some(path.to_owned()),
            actual: buf.len() as u64,
            limit,
        });
    }
    Ok(buf)
}

fn ensure_limit(kind: BundleLimitKind, path: Option<&str>, actual: u64, limit: u64) -> Result<()> {
    if actual > limit {
        return Err(PyRunnerError::BundleLimitExceeded {
            kind,
            path: path.map(str::to_owned),
            actual,
            limit,
        });
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    #[test]
    fn from_zip_bytes_rejects_archive_over_limit() {
        let err = Bundle::from_zip_bytes_with_limits(
            vec![0_u8; 16],
            BundleLimits {
                max_archive_bytes: 8,
                ..BundleLimits::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PyRunnerError::BundleLimitExceeded {
                kind: BundleLimitKind::ArchiveBytes,
                ..
            }
        ));
    }

    #[test]
    fn from_reader_rejects_too_many_entries() {
        let bytes = zip_bytes(&[("a.py", b"1".as_slice()), ("b.py", b"2".as_slice())]);
        let err = Bundle::from_reader_with_limits(
            Cursor::new(bytes),
            BundleLimits {
                max_entries: 1,
                ..BundleLimits::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PyRunnerError::BundleLimitExceeded {
                kind: BundleLimitKind::EntryCount,
                ..
            }
        ));
    }

    #[test]
    fn from_reader_rejects_total_uncompressed_bytes_over_limit() {
        let bytes = zip_bytes(&[("a.py", b"abc".as_slice()), ("b.py", b"def".as_slice())]);
        let err = Bundle::from_reader_with_limits(
            Cursor::new(bytes),
            BundleLimits {
                max_total_uncompressed_bytes: 5,
                ..BundleLimits::default()
            },
        )
        .unwrap_err();

        assert!(matches!(
            err,
            PyRunnerError::BundleLimitExceeded {
                kind: BundleLimitKind::TotalUncompressedBytes,
                ..
            }
        ));
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (path, data) in entries {
            writer
                .start_file(
                    *path,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
                )
                .expect("start zip entry");
            writer.write_all(data).expect("write zip entry");
        }
        writer.finish().expect("finish zip").into_inner()
    }
}
