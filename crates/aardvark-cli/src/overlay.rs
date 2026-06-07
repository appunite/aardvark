mod blob;
mod cache;
mod catalog;
mod files;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use aardvark_core::{OverlayExport, PyRuntime};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::{info, warn};

use self::blob::{
    digest_for_bytes, normalize_sha256_digest, overlay_blob_filename, validate_blob_bytes,
    validate_blob_file, OverlayBlobInfo,
};
use self::cache::{enforce_overlay_cache_policy, overlay_cache_config, OverlayEvictionStats};
pub(crate) use self::catalog::{canonicalize_package_name, hydrate_overlay_from_catalog};
use self::catalog::{
    collect_index_updates, collect_overlay_blobs, parse_dynlibs_from_metadata,
    update_overlay_index, OverlayIndexEntry,
};
use self::files::MAX_OVERLAY_METADATA_BYTES;
use crate::read_limited::read_file_limited;

pub(crate) fn restore_snapshot_overlay(
    runtime: &mut PyRuntime,
    snapshot_path: &Path,
    compatibility_fingerprint: Option<&str>,
) -> Result<bool> {
    let overlay_path = snapshot_overlay_path(snapshot_path);
    if !overlay_path.exists() {
        tracing::info!(
            target = "aardvark::overlay",
            overlay.restored = false,
            "overlay import skipped"
        );
        return Ok(false);
    }

    let meta_bytes = match read_file_limited(
        &overlay_path,
        MAX_OVERLAY_METADATA_BYTES,
        "overlay metadata",
    ) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("failed to read overlay {}: {error}", overlay_path.display());
            return Ok(false);
        }
    };

    let metadata_value = match serde_json::from_slice::<Value>(&meta_bytes) {
        Ok(value) => value,
        Err(error) => {
            warn!(
                "failed to parse overlay metadata {}: {error}",
                overlay_path.display()
            );
            return Ok(false);
        }
    };

    if !metadata_fingerprint_matches(&metadata_value, compatibility_fingerprint) {
        warn!(
            "overlay metadata {} has stale or missing compatibility fingerprint; skipping",
            overlay_path.display()
        );
        return Ok(false);
    }

    let blob_list = match collect_overlay_blobs(snapshot_path, &overlay_path, &metadata_value) {
        Ok(blobs) => blobs,
        Err(error) => {
            warn!(
                "overlay blobs unavailable for {}: {error}",
                overlay_path.display()
            );
            return Ok(false);
        }
    };

    if let Err(error) = runtime.js_runtime().import_overlay(&meta_bytes, &blob_list) {
        warn!(
            "failed to import overlay {}: {error}",
            overlay_path.display()
        );
        return Ok(false);
    }

    let package_count = metadata_package_count(&metadata_value);
    let restored = package_count > 0;
    let blob_bytes: usize = blob_list.iter().map(|blob| blob.bytes.len()).sum();
    tracing::info!(
        target = "aardvark::overlay",
        overlay.restored = true,
        overlay.packages = package_count,
        overlay.meta_bytes = meta_bytes.len(),
        overlay.blob_bytes = blob_bytes,
        overlay.blob_count = blob_list.len(),
        "overlay import completed"
    );
    info!(
        "restored overlay from {} (meta {} bytes, {} blobs, {} bytes total, {} packages)",
        overlay_path.display(),
        meta_bytes.len(),
        blob_list.len(),
        blob_bytes,
        package_count
    );
    Ok(restored)
}

pub(crate) fn export_snapshot_overlay(
    runtime: &mut PyRuntime,
    snapshot_path: &Path,
    compatibility_fingerprint: Option<&str>,
) {
    match runtime.js_runtime().export_overlay() {
        Ok(OverlayExport { metadata, blobs }) => {
            let overlay_path = snapshot_overlay_path(snapshot_path);
            if metadata.is_empty() || blobs.is_empty() {
                if overlay_path.exists() {
                    if let Err(error) = fs::remove_file(&overlay_path) {
                        warn!(
                            "failed to remove old overlay {}: {error}",
                            overlay_path.display()
                        );
                    }
                }
                return;
            }

            let mut metadata_value =
                serde_json::from_slice::<Value>(&metadata).unwrap_or_else(|_| json!({}));
            if !metadata_value.is_object() {
                metadata_value = json!({});
            }
            let cache_config = overlay_cache_config(Some(snapshot_path));
            if let Err(error) = fs::create_dir_all(&cache_config.root) {
                warn!(
                    "failed to create overlay blob dir {}: {error}",
                    cache_config.root.display()
                );
            }

            let mut canonical_to_digest: HashMap<String, String> = HashMap::new();
            let mut digest_entries: HashMap<String, OverlayBlobInfo> = HashMap::new();
            for blob in &blobs {
                let digest = match blob.digest.as_deref().filter(|value| !value.is_empty()) {
                    Some(value) => match normalize_sha256_digest(value) {
                        Ok(digest) => digest,
                        Err(error) => {
                            warn!("overlay blob has invalid digest {value:?}: {error}; skipping");
                            continue;
                        }
                    },
                    None => digest_for_bytes(&blob.bytes),
                };
                let file_name = match overlay_blob_filename(&digest) {
                    Ok(file_name) => file_name,
                    Err(error) => {
                        warn!("failed to derive overlay blob filename for {digest}: {error}");
                        continue;
                    }
                };
                let target_path = cache_config.root.join(&file_name);
                let mut stored = false;
                if target_path.exists() {
                    match validate_blob_file(&target_path, &digest) {
                        Ok(()) => {
                            stored = true;
                        }
                        Err(error) => {
                            warn!(
                                "existing overlay blob {} failed validation: {error}; rewriting",
                                target_path.display()
                            );
                            let _ = fs::remove_file(&target_path);
                        }
                    }
                }
                if !stored {
                    if let Err(error) = fs::write(&target_path, &blob.bytes) {
                        warn!(
                            "failed to write overlay tar {}: {error}",
                            target_path.display()
                        );
                        continue;
                    }
                    if let Err(error) = validate_blob_bytes(&blob.bytes, &digest) {
                        warn!(
                            "overlay blob {} failed digest check: {error}",
                            target_path.display()
                        );
                        let _ = fs::remove_file(&target_path);
                        continue;
                    }
                }
                let size = fs::metadata(&target_path)
                    .map(|meta| meta.len() as usize)
                    .unwrap_or(blob.bytes.len());
                canonical_to_digest.insert(blob.key.clone(), digest.clone());
                digest_entries.insert(
                    digest.clone(),
                    OverlayBlobInfo {
                        path: target_path.clone(),
                        file_name: file_name.clone(),
                        size,
                    },
                );
            }

            let eviction_stats = match enforce_overlay_cache_policy(&cache_config) {
                Ok(stats) => stats,
                Err(error) => {
                    warn!(
                        "overlay cache policy enforcement failed for {}: {error}",
                        cache_config.root.display()
                    );
                    OverlayEvictionStats::default()
                }
            };
            let overlay_evicted = eviction_stats.evicted_files;
            let overlay_evicted_bytes = eviction_stats.evicted_bytes.min(u64::MAX as u128) as u64;

            {
                let Some(metadata_object) = metadata_value.as_object_mut() else {
                    warn!("overlay metadata was not a JSON object; skipping overlay export");
                    return;
                };
                metadata_object.insert("version".to_string(), json!(3));
                metadata_object.insert("format".to_string(), json!("catalog"));
                if let Some(fingerprint) = compatibility_fingerprint {
                    metadata_object
                        .insert("compatibilityFingerprint".to_string(), json!(fingerprint));
                }
            }

            let mut available_digests: HashSet<String> = HashSet::new();
            for (digest, info) in &digest_entries {
                if info.path.exists() {
                    available_digests.insert(digest.clone());
                }
            }

            if let Some(packages) = metadata_value
                .get_mut("packages")
                .and_then(|value| value.as_array_mut())
            {
                for package in packages {
                    if let Some(obj) = package.as_object_mut() {
                        let canonical = obj
                            .get("canonical")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default();
                        if let Some(digest) = canonical_to_digest.get(canonical) {
                            if !available_digests.contains(digest) {
                                warn!(
                                    "overlay digest {} missing on disk; skipping metadata update for {}",
                                    digest, canonical
                                );
                                continue;
                            }
                            obj.insert("digest".to_string(), json!(digest));
                            obj.insert("blob".to_string(), json!(digest));
                        }
                    }
                }
            }

            let mut blob_map = serde_json::Map::new();
            let mut total_blob_bytes = 0usize;
            let mut inserted_count = 0usize;
            for (digest, info) in &digest_entries {
                if !info.path.exists() {
                    warn!(
                        "overlay tar {} missing after cache enforcement; metadata may be incomplete",
                        info.path.display()
                    );
                    continue;
                }
                let size = fs::metadata(&info.path)
                    .map(|meta| meta.len() as usize)
                    .unwrap_or(info.size);
                total_blob_bytes += size;
                inserted_count += 1;
                blob_map.insert(
                    digest.to_string(),
                    json!({
                        "digest": digest,
                        "blob": info.file_name,
                        "size": size
                    }),
                );
            }
            let Some(metadata_object) = metadata_value.as_object_mut() else {
                warn!("overlay metadata was not a JSON object; skipping overlay export");
                return;
            };
            metadata_object.insert("blobs".to_string(), Value::Object(blob_map));

            let mut index_updates = collect_index_updates(&metadata_value, &digest_entries);
            if index_updates.is_empty() {
                let dynlib_entries = parse_dynlibs_from_metadata(&metadata_value);
                for (canonical, digest) in &canonical_to_digest {
                    if digest.is_empty() {
                        continue;
                    }
                    if let Some(info) = digest_entries.get(digest) {
                        if !info.path.exists() {
                            continue;
                        }
                    } else {
                        continue;
                    }
                    let dynlibs = if dynlib_entries.is_empty() {
                        None
                    } else {
                        Some(dynlib_entries.clone())
                    };
                    index_updates.insert(
                        canonicalize_package_name(canonical),
                        OverlayIndexEntry {
                            digest: digest.clone(),
                            blob: match overlay_blob_filename(digest) {
                                Ok(file_name) => file_name,
                                Err(error) => {
                                    warn!(
                                        "failed to derive overlay index blob for {digest}: {error}"
                                    );
                                    continue;
                                }
                            },
                            dynlibs,
                        },
                    );
                }
            }
            let index_update_count = index_updates.len();

            tracing::info!(
                target = "aardvark::overlay",
                overlay.meta_bytes = metadata.len(),
                overlay.blob_bytes = total_blob_bytes,
                overlay.blob_count = inserted_count,
                overlay.packages = canonical_to_digest.len(),
                overlay.index_updates = index_update_count,
                overlay.evicted = overlay_evicted,
                overlay.evicted_bytes = overlay_evicted_bytes,
                overlay.cache_root = %cache_config.root.display(),
                "overlay export completed"
            );

            if let Err(error) = update_overlay_index(
                &cache_config.root,
                &index_updates,
                compatibility_fingerprint,
            ) {
                warn!(
                    "failed to update overlay index at {}: {error}",
                    cache_config.root.display()
                );
            }

            match serde_json::to_vec_pretty(&metadata_value) {
                Ok(serialized) => {
                    if let Err(error) = fs::write(&overlay_path, &serialized) {
                        warn!(
                            "failed to write overlay {}: {error}",
                            overlay_path.display()
                        );
                    } else {
                        info!(
                            "wrote overlay metadata to {} ({} blobs, {} bytes total) [cache root: {}]",
                            overlay_path.display(),
                            inserted_count,
                            total_blob_bytes,
                            cache_config.root.display()
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        "failed to serialize overlay metadata {}: {error}",
                        overlay_path.display()
                    );
                }
            }
        }
        Err(error) => {
            warn!("failed to export overlay: {error}");
        }
    }
}

pub(crate) fn write_snapshot_metadata(path: &Path, compatibility_fingerprint: &str) -> Result<()> {
    let metadata_path = snapshot_metadata_path(path);
    let payload = json!({
        "version": 1,
        "compatibilityFingerprint": compatibility_fingerprint,
    });
    fs::write(&metadata_path, serde_json::to_vec_pretty(&payload)?)
        .with_context(|| format!("write {}", metadata_path.display()))?;
    Ok(())
}

fn snapshot_overlay_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".overlay.json");
    PathBuf::from(os)
}

fn snapshot_metadata_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".aardvark.json");
    PathBuf::from(os)
}

fn metadata_fingerprint_matches(metadata: &Value, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    metadata
        .get("compatibilityFingerprint")
        .and_then(Value::as_str)
        == Some(expected)
}

fn metadata_package_count(metadata: &Value) -> usize {
    metadata
        .get("packages")
        .and_then(|value| value.as_array())
        .map(|array| array.len())
        .unwrap_or(0)
}
