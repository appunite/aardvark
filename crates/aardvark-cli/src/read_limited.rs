use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};

const MAX_PREALLOC_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn read_file_limited(path: &Path, limit: u64, kind: &str) -> Result<Vec<u8>> {
    let file =
        File::open(path).with_context(|| format!("failed to open {kind} {}", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat {kind} {}", path.display()))?
        .len();
    if len > limit {
        bail!(
            "refusing to read {kind} {}: {} bytes exceeds the {} byte limit",
            path.display(),
            len,
            limit
        );
    }

    let mut bytes = Vec::with_capacity(len.min(MAX_PREALLOC_BYTES) as usize);
    let mut limited = file.take(limit.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {kind} {}", path.display()))?;
    if bytes.len() as u64 > limit {
        bail!(
            "{kind} {} exceeded the {} byte limit while reading",
            path.display(),
            limit
        );
    }
    Ok(bytes)
}

pub(crate) fn read_utf8_limited<R: Read>(reader: R, limit: u64, kind: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(limit.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {kind}"))?;
    if bytes.len() as u64 > limit {
        bail!("{kind} exceeded the configured limit of {limit} bytes");
    }
    String::from_utf8(bytes).with_context(|| format!("{kind} is not UTF-8"))
}
