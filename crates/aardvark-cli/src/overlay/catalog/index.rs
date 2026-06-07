use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use super::{canonicalize_package_name, parse_dynlibs_from_metadata, OverlayDynlibEntry};
use crate::overlay::blob::{normalize_sha256_digest, overlay_blob_filename, OverlayBlobInfo};
use crate::overlay::files::{read_limited_json, MAX_OVERLAY_METADATA_BYTES};

#[derive(Debug, Deserialize, Serialize, Default)]
pub(in crate::overlay) struct OverlayIndex {
    #[serde(default, rename = "compatibilityFingerprint")]
    pub(super) compatibility_fingerprint: Option<String>,
    #[serde(default)]
    pub(super) packages: HashMap<String, OverlayIndexEntry>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
pub(in crate::overlay) struct OverlayIndexEntry {
    #[serde(default)]
    pub(in crate::overlay) digest: String,
    #[serde(default)]
    pub(in crate::overlay) blob: String,
    #[serde(default)]
    pub(in crate::overlay) dynlibs: Option<Vec<OverlayDynlibEntry>>,
}

fn overlay_index_from_metadata(cache_root: &Path) -> Result<OverlayIndex> {
    let mut sidecars = Vec::new();
    let mut fingerprints = BTreeSet::new();
    let Some(parent) = cache_root.parent() else {
        return Ok(OverlayIndex::default());
    };
    if !parent.exists() {
        return Ok(OverlayIndex::default());
    }
    for entry in fs::read_dir(parent)? {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = match path.file_name().and_then(|name| name.to_str()) {
            Some(value) => value,
            None => continue,
        };
        if !file_name.ends_with(".overlay.json") {
            continue;
        }
        let value: Value =
            match read_limited_json(&path, MAX_OVERLAY_METADATA_BYTES, "overlay metadata") {
                Ok(value) => value,
                Err(_) => continue,
            };
        let fingerprint = value
            .get("compatibilityFingerprint")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        let dynlibs = parse_dynlibs_from_metadata(&value);
        let dynlibs_opt = if dynlibs.is_empty() {
            None
        } else {
            Some(dynlibs)
        };
        let mut entries = Vec::new();
        if let Some(packages) = value.get("packages").and_then(Value::as_array) {
            for pkg in packages {
                let canonical = pkg
                    .get("canonical")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let digest = pkg
                    .get("digest")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Ok(digest) = normalize_sha256_digest(digest) else {
                    continue;
                };
                if canonical.is_empty() {
                    continue;
                }
                let Ok(blob) = overlay_blob_filename(&digest) else {
                    continue;
                };
                entries.push((
                    canonicalize_package_name(canonical),
                    OverlayIndexEntry {
                        digest,
                        blob,
                        dynlibs: dynlibs_opt.clone(),
                    },
                ));
            }
        }
        if entries.is_empty() {
            continue;
        }
        if let Some(fingerprint) = fingerprint.as_ref() {
            fingerprints.insert(fingerprint.clone());
        }
        sidecars.push((fingerprint, entries));
    }
    let compatibility_fingerprint = if fingerprints.len() == 1 {
        fingerprints.iter().next().cloned()
    } else {
        None
    };
    if fingerprints.len() > 1 {
        debug!(
            target = "aardvark::overlay",
            overlay.cache_root = %cache_root.display(),
            "skipping overlay sidecar backfill with mixed compatibility fingerprints"
        );
        return Ok(OverlayIndex::default());
    }

    let mut index = OverlayIndex {
        compatibility_fingerprint,
        packages: HashMap::new(),
    };
    for (sidecar_fingerprint, entries) in sidecars {
        if index.compatibility_fingerprint.is_some()
            && sidecar_fingerprint.as_ref() != index.compatibility_fingerprint.as_ref()
        {
            continue;
        }
        for (canonical, entry) in entries {
            index.packages.insert(canonical, entry);
        }
    }
    Ok(index)
}

pub(super) fn load_overlay_index(cache_root: &Path) -> Result<OverlayIndex> {
    let index_path = cache_root.join("index.json");
    if !index_path.exists() {
        return overlay_index_from_metadata(cache_root);
    }
    read_limited_json(&index_path, MAX_OVERLAY_METADATA_BYTES, "overlay index")
}

fn save_overlay_index(cache_root: &Path, index: &OverlayIndex) -> Result<()> {
    let index_path = cache_root.join("index.json");
    let data = serde_json::to_vec_pretty(index)?;
    fs::write(&index_path, data)
        .with_context(|| format!("failed to write overlay index {}", index_path.display()))?;
    Ok(())
}

pub(in crate::overlay) fn update_overlay_index(
    cache_root: &Path,
    updates: &HashMap<String, OverlayIndexEntry>,
    compatibility_fingerprint: Option<&str>,
) -> Result<()> {
    let mut index = load_overlay_index(cache_root)?;
    let mut changed = false;
    if index.compatibility_fingerprint.as_deref() != compatibility_fingerprint {
        if !index.packages.is_empty() {
            tracing::info!(
                target = "aardvark::overlay",
                overlay.cache_root = %cache_root.display(),
                overlay.previous_fingerprint = index.compatibility_fingerprint.as_deref().unwrap_or("<missing>"),
                overlay.current_fingerprint = compatibility_fingerprint.unwrap_or("<missing>"),
                "clearing stale overlay catalog index"
            );
        }
        index.packages.clear();
        index.compatibility_fingerprint = compatibility_fingerprint.map(ToOwned::to_owned);
        changed = true;
    }
    for (canonical, update_entry) in updates {
        let Ok(digest) = normalize_sha256_digest(&update_entry.digest) else {
            continue;
        };
        let Ok(canonical_blob) = overlay_blob_filename(&digest) else {
            continue;
        };
        let canonical_key = canonicalize_package_name(canonical);
        let mut candidate = update_entry.clone();
        candidate.digest = digest;
        if !is_safe_blob_filename(&candidate.blob) {
            candidate.blob = canonical_blob.clone();
        }
        if candidate.blob != canonical_blob {
            candidate.blob = canonical_blob;
        }
        let entry_changed = match index.packages.get(&canonical_key) {
            Some(existing) => existing != &candidate,
            None => true,
        };
        if entry_changed {
            index.packages.insert(canonical_key.clone(), candidate);
            changed = true;
        }
    }
    index.packages.retain(|canonical, entry| {
        let exists = safe_cache_blob_path(cache_root, &entry.blob)
            .map(|path| path.exists())
            .unwrap_or(false);
        if !exists {
            debug!(
                target = "aardvark::overlay",
                overlay.cache_root = %cache_root.display(),
                overlay.canonical = %canonical,
                overlay.digest = %entry.digest,
                "removing overlay index entry without blob"
            );
            changed = true;
        }
        exists
    });
    let index_path = cache_root.join("index.json");
    if !changed && index_path.exists() {
        return Ok(());
    }
    save_overlay_index(cache_root, &index)?;
    tracing::info!(
        target = "aardvark::overlay",
        overlay.index_entries = index.packages.len(),
        overlay.index_updates = updates.len(),
        overlay.cache_root = %cache_root.display(),
        "overlay catalog index updated"
    );
    Ok(())
}

pub(in crate::overlay) fn collect_index_updates(
    metadata: &Value,
    digest_entries: &HashMap<String, OverlayBlobInfo>,
) -> HashMap<String, OverlayIndexEntry> {
    let mut updates = HashMap::new();
    let dynlib_entries = parse_dynlibs_from_metadata(metadata);
    let Some(packages) = metadata.get("packages").and_then(Value::as_array) else {
        return updates;
    };
    for package in packages {
        let Some(obj) = package.as_object() else {
            continue;
        };
        let canonical = match obj.get("canonical").and_then(Value::as_str) {
            Some(value) if !value.is_empty() => value,
            _ => continue,
        };
        let digest = match obj
            .get("digest")
            .and_then(Value::as_str)
            .or_else(|| obj.get("blob").and_then(Value::as_str))
        {
            Some(value) if !value.is_empty() => match normalize_sha256_digest(value) {
                Ok(digest) => digest,
                Err(_) => continue,
            },
            _ => continue,
        };
        let Some(info) = digest_entries.get(&digest) else {
            continue;
        };
        if !info.path.exists() {
            continue;
        }
        let dynlibs = if dynlib_entries.is_empty() {
            None
        } else {
            Some(dynlib_entries.clone())
        };
        updates.insert(
            canonicalize_package_name(canonical),
            OverlayIndexEntry {
                digest: digest.to_string(),
                blob: info.file_name.clone(),
                dynlibs,
            },
        );
    }
    updates
}

fn is_safe_blob_filename(value: &str) -> bool {
    let path = Path::new(value);
    !value.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

pub(super) fn safe_cache_blob_path(cache_root: &Path, filename: &str) -> Option<PathBuf> {
    if is_safe_blob_filename(filename) {
        Some(cache_root.join(filename))
    } else {
        None
    }
}
