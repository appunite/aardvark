use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use hex::ToHex;
use sha2::{Digest, Sha256};

use super::cache::overlay_blob_dir;

pub(super) struct OverlayBlobInfo {
    pub(super) path: PathBuf,
    pub(super) file_name: String,
    pub(super) size: usize,
}

pub(super) fn overlay_blob_filename(digest: &str) -> Result<String> {
    let digest = normalize_sha256_digest(digest)?;
    Ok(format!("sha256-{}.tar", digest_hex(&digest)?))
}

pub(super) fn overlay_blob_path(snapshot_path: &Path, digest: &str) -> Result<PathBuf> {
    Ok(overlay_blob_dir(snapshot_path).join(overlay_blob_filename(digest)?))
}

pub(super) fn normalize_sha256_digest(digest: &str) -> Result<String> {
    let trimmed = digest.trim();
    if trimmed.is_empty() {
        bail!("overlay blob digest is empty");
    }
    let without_prefix = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    if without_prefix.len() != 64 {
        bail!("overlay blob digest must be a sha256 digest");
    }
    if !without_prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("overlay blob digest contains non-hex characters");
    }
    Ok(format!("sha256:{}", without_prefix.to_ascii_lowercase()))
}

pub(super) fn digest_for_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn digest_hex(digest: &str) -> Result<&str> {
    digest
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow::anyhow!("overlay blob digest must use sha256 prefix"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.encode_hex::<String>()
}

pub(super) fn validate_blob_bytes(bytes: &[u8], digest: &str) -> Result<()> {
    let expected = digest_hex(&normalize_sha256_digest(digest)?)?.to_string();
    let actual = sha256_hex(bytes);
    if actual != expected {
        bail!("overlay blob digest mismatch (expected sha256:{expected}, found sha256:{actual})");
    }
    Ok(())
}

pub(super) fn validate_blob_file(path: &Path, digest: &str) -> Result<()> {
    let expected = digest_hex(&normalize_sha256_digest(digest)?)?.to_string();
    let file = File::open(path)
        .with_context(|| format!("failed to open overlay blob {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 131_072];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to read overlay blob {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher.finalize().encode_hex::<String>();
    if actual != expected {
        bail!("overlay blob digest mismatch (expected sha256:{expected}, found sha256:{actual})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_blob_bytes_checks_digest() {
        let data = b"hello world";
        let digest = format!("sha256:{}", Sha256::digest(data).encode_hex::<String>());
        assert!(validate_blob_bytes(data, &digest).is_ok());
        let bad_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(validate_blob_bytes(data, bad_digest).is_err());
    }
}
