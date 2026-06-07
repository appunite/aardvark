use std::path::Path;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use crate::read_limited::read_file_limited;

pub(super) const MAX_OVERLAY_BLOB_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MAX_OVERLAY_LOCKFILE_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_OVERLAY_METADATA_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn read_overlay_blob_bytes(path: &Path) -> Result<Vec<u8>> {
    read_file_limited(path, MAX_OVERLAY_BLOB_BYTES, "overlay blob")
}

pub(super) fn read_limited_json<T>(path: &Path, limit: u64, kind: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let bytes = read_file_limited(path, limit, kind)?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {kind} {}", path.display()))
}
