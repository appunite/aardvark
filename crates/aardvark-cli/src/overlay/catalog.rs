use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use aardvark_core::{OverlayBlob, PyRuntime};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value};
use tracing::{debug, info, info_span, warn};

use super::blob::{
    normalize_sha256_digest, overlay_blob_filename, overlay_blob_path, validate_blob_bytes,
};
use super::cache::{
    enforce_overlay_cache_policy, env_var_os, overlay_cache_config, OverlayEvictionStats,
};
use super::files::{read_limited_json, read_overlay_blob_bytes, MAX_OVERLAY_LOCKFILE_BYTES};

mod index;

pub(super) use index::{collect_index_updates, update_overlay_index, OverlayIndexEntry};
use index::{load_overlay_index, safe_cache_blob_path};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub(super) struct OverlayDynlibEntry {
    pub(super) location: String,
    #[serde(rename = "relPath")]
    pub(super) rel_path: String,
    #[serde(default)]
    pub(super) path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PyodideLockfile {
    #[serde(default)]
    packages: HashMap<String, LockPackage>,
}

#[derive(Debug, Deserialize)]
struct LockPackage {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    depends: Vec<String>,
    #[serde(default)]
    package_type: Option<String>,
}

pub(crate) fn canonicalize_package_name(name: &str) -> String {
    let mut canonical = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            canonical.push(lower);
            last_dash = false;
        } else if matches!(lower, '-' | '_' | '.') && !last_dash {
            canonical.push('-');
            last_dash = true;
        }
    }
    if canonical.ends_with('-') {
        canonical.pop();
    }
    canonical
}

fn load_lockfile() -> Result<Option<PyodideLockfile>> {
    let package_dir = match env_var_os("AARDVARK_PYODIDE_DIST_DIR") {
        Some(value) => PathBuf::from(value),
        None => return Ok(None),
    };
    let lock_path = package_dir.join("pyodide-lock.json");
    if !lock_path.exists() {
        return Ok(None);
    }
    let lock: PyodideLockfile =
        read_limited_json(&lock_path, MAX_OVERLAY_LOCKFILE_BYTES, "Pyodide lockfile")?;
    Ok(Some(lock))
}

fn resolve_lockfile_requirements(requested: &[String], lockfile: &PyodideLockfile) -> Vec<String> {
    fn visit(
        name: &str,
        lockfile: &PyodideLockfile,
        seen: &mut HashSet<String>,
        ordered: &mut Vec<String>,
    ) {
        let canonical = canonicalize_package_name(name);
        if seen.contains(&canonical) {
            return;
        }
        let Some(meta) = lockfile.packages.get(&canonical) else {
            warn!("lockfile is missing metadata for {name}");
            return;
        };
        for dep in &meta.depends {
            visit(dep, lockfile, seen, ordered);
        }
        if seen.insert(canonical.clone()) {
            ordered.push(canonical);
        }
    }

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for name in requested {
        visit(name, lockfile, &mut seen, &mut ordered);
    }
    ordered
}

fn normalize_dynlib_location(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "usr" | "dynlib" | "/usr/lib" | "lib") {
        return Some("usr".to_string());
    }
    if matches!(
        lower.as_str(),
        "site" | "session" | "/session" | "/session/site-packages" | "site-packages"
    ) {
        return Some("site".to_string());
    }
    None
}

pub(super) fn parse_dynlibs_from_metadata(metadata: &Value) -> Vec<OverlayDynlibEntry> {
    let mut result = Vec::new();
    let Some(array) = metadata.get("dynlibs").and_then(Value::as_array) else {
        return result;
    };
    let mut seen = HashSet::new();
    for entry in array {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(location_raw) = obj.get("location").and_then(Value::as_str) else {
            continue;
        };
        let Some(location) = normalize_dynlib_location(location_raw) else {
            continue;
        };
        let rel_path_value = obj
            .get("relPath")
            .or_else(|| obj.get("rel_path"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if rel_path_value.is_empty() {
            continue;
        }
        let rel_path = rel_path_value.replace('\\', "/");
        if rel_path.is_empty() {
            continue;
        }
        let key = format!("{}:{}", location, rel_path);
        if !seen.insert(key) {
            continue;
        }
        let path = obj
            .get("path")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        result.push(OverlayDynlibEntry {
            location,
            rel_path,
            path,
        });
    }
    result
}

pub(crate) fn hydrate_overlay_from_catalog(
    runtime: &mut PyRuntime,
    packages: &[String],
    compatibility_fingerprint: Option<&str>,
) -> Result<bool> {
    let span = info_span!(
        target: "aardvark::overlay",
        "overlay.catalog.hydrate",
        overlay.packages_requested = packages.len() as u64,
        overlay.hydrate_packages = tracing::field::Empty,
        overlay.hydrate_blob_count = tracing::field::Empty,
        overlay.hydrate_blob_bytes = tracing::field::Empty,
        overlay.hydrate_duration_ms = tracing::field::Empty,
        overlay.hydrate_hit = tracing::field::Empty
    );
    let _guard = span.enter();
    let started = Instant::now();
    let mut metrics_blob_bytes: usize = 0;
    let mut metrics_blob_count: usize = 0;
    let mut metrics_packages: usize = 0;
    let mut metrics_cache_root: Option<PathBuf> = None;
    let mut eviction_stats: Option<OverlayEvictionStats> = None;
    let mut dynlib_accumulator: Vec<OverlayDynlibEntry> = Vec::new();
    let mut dynlib_seen: HashSet<String> = HashSet::new();

    let result = (|| -> Result<bool> {
        let Some(lockfile) = load_lockfile()? else {
            return Ok(false);
        };
        let requirements = resolve_lockfile_requirements(packages, &lockfile);
        if requirements.is_empty() {
            return Ok(false);
        }
        let cache_config = overlay_cache_config(None);
        metrics_cache_root = Some(cache_config.root.clone());
        if !cache_config.root.exists() {
            return Ok(false);
        }

        match enforce_overlay_cache_policy(&cache_config) {
            Ok(stats) => {
                if stats.evicted_files > 0 {
                    eviction_stats = Some(stats);
                }
            }
            Err(error) => {
                tracing::warn!(
                    target = "aardvark::overlay",
                    overlay.event = "evict",
                    overlay.error = %error,
                    overlay.cache_root = %cache_config.root.display(),
                    "overlay cache eviction during hydrate failed"
                );
            }
        }

        let prune_updates: HashMap<String, OverlayIndexEntry> = HashMap::new();
        if let Err(error) = update_overlay_index(
            &cache_config.root,
            &prune_updates,
            compatibility_fingerprint,
        ) {
            tracing::warn!(
                target = "aardvark::overlay",
                overlay.event = "index-prune",
                overlay.error = %error,
                overlay.cache_root = %cache_config.root.display(),
                "overlay index prune failed during hydrate"
            );
        }

        let cache_root = cache_config.root.clone();
        let index = load_overlay_index(&cache_root)?;
        if index.compatibility_fingerprint.as_deref() != compatibility_fingerprint {
            let empty_updates: HashMap<String, OverlayIndexEntry> = HashMap::new();
            update_overlay_index(&cache_root, &empty_updates, compatibility_fingerprint)?;
            return Ok(false);
        }
        debug!(
            target = "aardvark::overlay",
            overlay.index_entries = index.packages.len(),
            overlay.cache_root = %cache_root.display(),
            "overlay catalog loaded"
        );
        let mut blobs = Vec::new();
        let mut package_entries = Vec::new();
        let mut blob_map = JsonMap::new();
        let mut total_bytes: usize = 0;
        let mut hydrated = Vec::new();
        for canonical in &requirements {
            let Some(meta) = lockfile.packages.get(canonical) else {
                continue;
            };
            let package_type = meta.package_type.as_deref().unwrap_or("package");
            if !matches!(package_type, "package" | "shared_library") {
                continue;
            }
            let Some(index_entry) = index.packages.get(canonical) else {
                continue;
            };
            let digest = match normalize_sha256_digest(&index_entry.digest) {
                Ok(digest) => digest,
                Err(error) => {
                    tracing::warn!(
                        target = "aardvark::overlay",
                        overlay.canonical = %canonical,
                        overlay.error = %error,
                        "catalog entry has invalid digest"
                    );
                    continue;
                }
            };
            let Some(blob_path) = safe_cache_blob_path(&cache_root, &index_entry.blob) else {
                tracing::warn!(
                    target = "aardvark::overlay",
                    overlay.canonical = %canonical,
                    overlay.blob = %index_entry.blob,
                    "catalog entry has unsafe blob path"
                );
                continue;
            };
            let blob_filename = index_entry.blob.clone();
            if !blob_path.exists() {
                tracing::debug!(
                    target = "aardvark::overlay",
                    overlay.digest_miss = %digest,
                    overlay.cache_root = %cache_root.display(),
                    "catalog blob not present in cache"
                );
                continue;
            }
            let bytes = read_overlay_blob_bytes(&blob_path)?;
            validate_blob_bytes(&bytes, &digest).with_context(|| {
                format!(
                    "overlay catalog blob {} failed digest verification",
                    blob_path.display()
                )
            })?;
            let size = bytes.len();
            total_bytes += size;
            blobs.push(OverlayBlob {
                key: canonical.clone(),
                digest: Some(digest.clone()),
                bytes,
            });
            if let Some(entry_dynlibs) = &index_entry.dynlibs {
                for dynlib in entry_dynlibs {
                    let key = format!("{}:{}", dynlib.location, dynlib.rel_path);
                    if dynlib_seen.insert(key.clone()) {
                        dynlib_accumulator.push(dynlib.clone());
                    }
                }
            }
            package_entries.push(json!({
                "canonical": canonical,
                "name": meta.name.as_deref().unwrap_or(canonical),
                "digest": digest,
                "blob": digest,
                "mounts": [],
                "size": size,
            }));
            blob_map.insert(
                digest.clone(),
                json!({
                    "digest": digest,
                    "blob": blob_filename,
                    "size": size,
                }),
            );
            hydrated.push(canonical.clone());
        }

        if blobs.is_empty() {
            return Ok(false);
        }

        metrics_blob_bytes = total_bytes;
        metrics_blob_count = blobs.len();
        metrics_packages = hydrated.len();

        let mut metadata_obj = JsonMap::new();
        metadata_obj.insert("version".into(), Value::Number(3.into()));
        metadata_obj.insert("format".into(), Value::String("catalog".into()));
        if let Some(fingerprint) = compatibility_fingerprint {
            metadata_obj.insert(
                "compatibilityFingerprint".into(),
                Value::String(fingerprint.to_string()),
            );
        }
        metadata_obj.insert("packages".into(), Value::Array(package_entries));
        metadata_obj.insert("blobs".into(), Value::Object(blob_map));
        if !dynlib_accumulator.is_empty() {
            let dynlib_value = serde_json::to_value(&dynlib_accumulator)
                .context("failed to serialize overlay dynlibs")?;
            metadata_obj.insert("dynlibs".into(), dynlib_value);
        }
        let metadata = Value::Object(metadata_obj);
        let metadata_bytes = serde_json::to_vec(&metadata)?;
        runtime
            .js_runtime()
            .import_overlay(&metadata_bytes, &blobs)
            .context("failed to import overlay catalog")?;
        Ok(true)
    })();

    let elapsed = started.elapsed();
    span.record("overlay.hydrate_duration_ms", elapsed.as_millis() as u64);
    span.record("overlay.hydrate_blob_bytes", metrics_blob_bytes as u64);
    span.record("overlay.hydrate_packages", metrics_packages as u64);
    span.record("overlay.hydrate_blob_count", metrics_blob_count as u64);
    span.record("overlay.hydrate_hit", matches!(result, Ok(true)));

    if let (Some(stats), Some(root)) = (&eviction_stats, metrics_cache_root.as_ref()) {
        info!(
            target = "aardvark::overlay",
            overlay.event = "evict",
            overlay.evicted = stats.evicted_files,
            overlay.evicted_bytes = stats.evicted_bytes.min(u64::MAX as u128) as u64,
            overlay.cache_root = %root.display(),
            "overlay cache eviction applied during hydrate"
        );
    }

    if let (Ok(true), Some(root)) = (&result, metrics_cache_root.as_ref()) {
        info!(
            target = "aardvark::overlay",
            overlay.hydrated = true,
            overlay.packages = metrics_packages,
            overlay.blob_count = metrics_blob_count,
            overlay.blob_bytes = metrics_blob_bytes,
            overlay.duration_ms = elapsed.as_millis() as u64,
            overlay.cache_root = %root.display(),
            "overlay hydrated from catalog"
        );
    } else if let (Ok(false), Some(root)) = (&result, metrics_cache_root.as_ref()) {
        debug!(
            target = "aardvark::overlay",
            overlay.event = "hydrate",
            overlay.hydrated = false,
            overlay.cache_root = %root.display(),
            overlay.duration_ms = elapsed.as_millis() as u64,
            "overlay catalog miss"
        );
    }

    result
}

fn resolve_catalog_blob_path(
    snapshot_path: &Path,
    metadata: &Value,
    canonical: &str,
    digest: &str,
) -> Result<PathBuf> {
    let cache_root = overlay_cache_config(Some(snapshot_path)).root;
    let digest = normalize_sha256_digest(digest)?;
    let blobs_obj = metadata.get("blobs").and_then(Value::as_object);
    if let Some(map) = blobs_obj {
        if let Some(entry) = map.get(&digest).or_else(|| {
            if canonical.is_empty() {
                None
            } else {
                map.get(canonical)
            }
        }) {
            if let Some(rel) = entry.get("blob").and_then(Value::as_str) {
                let rel_trimmed = rel.trim();
                if !rel_trimmed.is_empty() {
                    let Some(cache_candidate) = safe_cache_blob_path(&cache_root, rel_trimmed)
                    else {
                        bail!("overlay catalog blob path must be a cache-local filename");
                    };
                    return Ok(cache_candidate);
                }
            }
            if let Some(digest_path) = entry
                .get("digest")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                let digest_path = normalize_sha256_digest(digest_path)?;
                return Ok(cache_root.join(overlay_blob_filename(&digest_path)?));
            }
        }
    }
    Ok(cache_root.join(overlay_blob_filename(&digest)?))
}

pub(super) fn collect_overlay_blobs(
    snapshot_path: &Path,
    _overlay_path: &Path,
    metadata: &Value,
) -> Result<Vec<OverlayBlob>> {
    let version = metadata.get("version").and_then(Value::as_u64).unwrap_or(2);
    let format = metadata
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("tar");

    if version >= 3 && format.eq_ignore_ascii_case("catalog") {
        let mut seen = HashSet::new();
        let mut blobs = Vec::new();
        if let Some(packages) = metadata.get("packages").and_then(Value::as_array) {
            for package in packages {
                let Some(obj) = package.as_object() else {
                    continue;
                };
                let canonical = obj
                    .get("canonical")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let digest = obj
                    .get("digest")
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("blob").and_then(Value::as_str))
                    .unwrap_or(canonical);
                let digest = normalize_sha256_digest(digest)
                    .with_context(|| format!("invalid overlay digest for package {canonical}"))?;
                if !seen.insert(digest.clone()) {
                    continue;
                }
                let blob_path =
                    resolve_catalog_blob_path(snapshot_path, metadata, canonical, &digest)?;
                if !blob_path.exists() {
                    tracing::warn!(
                        target = "aardvark::overlay",
                        overlay.digest_miss = %digest,
                        overlay.canonical = %canonical,
                        overlay.snapshot = %snapshot_path.display(),
                        "overlay blob missing"
                    );
                    bail!(
                        "overlay blob {} not found (expected for digest {})",
                        blob_path.display(),
                        digest
                    );
                }
                let bytes = read_overlay_blob_bytes(&blob_path)?;
                validate_blob_bytes(&bytes, &digest).with_context(|| {
                    format!(
                        "overlay blob {} failed digest verification",
                        blob_path.display()
                    )
                })?;
                blobs.push(OverlayBlob {
                    key: canonical.to_string(),
                    digest: Some(digest),
                    bytes,
                });
            }
        }
        Ok(blobs)
    } else {
        let tar_value = metadata
            .get("tar")
            .ok_or_else(|| anyhow!("overlay metadata missing 'tar' section"))?;
        let digest = tar_value
            .get("digest")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("overlay tar metadata missing digest"))?;
        let digest = normalize_sha256_digest(digest)?;
        let blob_path = overlay_blob_path(snapshot_path, &digest)?;
        if !blob_path.exists() {
            tracing::warn!(
                target = "aardvark::overlay",
                overlay.digest_miss = %digest,
                overlay.snapshot = %snapshot_path.display(),
                "overlay tar blob missing"
            );
            bail!("overlay tar {} not found", blob_path.display());
        }
        let bytes = read_overlay_blob_bytes(&blob_path)?;
        validate_blob_bytes(&bytes, &digest).with_context(|| {
            format!(
                "overlay blob {} failed digest verification",
                blob_path.display()
            )
        })?;
        Ok(vec![OverlayBlob {
            key: digest.clone(),
            digest: Some(digest),
            bytes,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::blob::OverlayBlobInfo;
    use crate::overlay::catalog::index::OverlayIndex;
    use crate::overlay::files::MAX_OVERLAY_METADATA_BYTES;
    use crate::read_limited::read_file_limited;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn canonicalize_package_name_normalizes() {
        assert_eq!(canonicalize_package_name("NumPy"), "numpy");
        assert_eq!(canonicalize_package_name("scikit-learn"), "scikit-learn");
        assert_eq!(canonicalize_package_name("Pandas_core"), "pandas-core");
        assert_eq!(canonicalize_package_name("foo.bar_baz"), "foo-bar-baz");
    }

    #[test]
    fn update_overlay_index_writes_manifest() {
        let dir = tempdir().unwrap();
        let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let blob_name = overlay_blob_filename(digest).unwrap();
        fs::write(dir.path().join(&blob_name), vec![0u8; 16]).unwrap();
        let mut updates = HashMap::new();
        updates.insert(
            "dummy".to_string(),
            OverlayIndexEntry {
                digest: digest.to_string(),
                blob: blob_name,
                dynlibs: None,
            },
        );
        update_overlay_index(dir.path(), &updates, Some("sha256:test")).unwrap();
        let index_path = dir.path().join("index.json");
        assert!(index_path.exists(), "expected index.json to be written");
        let index_bytes =
            read_file_limited(&index_path, MAX_OVERLAY_METADATA_BYTES, "overlay index").unwrap();
        let index: OverlayIndex =
            serde_json::from_slice(&index_bytes).expect("parse overlay index");
        assert_eq!(
            index.compatibility_fingerprint.as_deref(),
            Some("sha256:test")
        );
    }

    #[test]
    fn load_overlay_index_backfills_sidecar_fingerprint() {
        let dir = tempdir().unwrap();
        let cache_root = dir.path().join("catalog");
        fs::create_dir(&cache_root).unwrap();
        let digest = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let blob_name = overlay_blob_filename(digest).unwrap();
        fs::write(cache_root.join(&blob_name), b"blob").unwrap();
        let metadata = json!({
            "version": 3,
            "format": "catalog",
            "compatibilityFingerprint": "sha256:pyodide-profile",
            "packages": [
                {"canonical": "NumPy", "digest": digest}
            ]
        });
        fs::write(
            dir.path().join("snapshot.bin.overlay.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();

        let index = load_overlay_index(&cache_root).unwrap();
        assert_eq!(
            index.compatibility_fingerprint.as_deref(),
            Some("sha256:pyodide-profile")
        );
        assert!(
            index.packages.contains_key("numpy"),
            "expected normalized numpy entry from sidecar backfill"
        );
    }

    #[test]
    fn collect_index_updates_preserves_dynlibs() {
        let dir = tempdir().unwrap();
        let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let blob_name = overlay_blob_filename(digest).unwrap();
        let blob_path = dir.path().join(&blob_name);
        fs::write(&blob_path, b"blob").unwrap();
        let mut digest_entries = HashMap::new();
        digest_entries.insert(
            digest.to_string(),
            OverlayBlobInfo {
                path: blob_path,
                file_name: blob_name.clone(),
                size: 4,
            },
        );
        let metadata = json!({
            "packages": [
                {"canonical": "numpy", "digest": digest}
            ],
            "dynlibs": [
                {"location": "usr", "relPath": "lib/libfoo.so", "path": "/usr/lib/libfoo.so"},
                {"location": "site", "relPath": "pkg.libs/libbar.so"}
            ]
        });
        let updates = collect_index_updates(&metadata, &digest_entries);
        let entry = updates
            .get("numpy")
            .expect("expected numpy entry in overlay updates");
        let dynlibs = entry
            .dynlibs
            .as_ref()
            .expect("expected dynlibs to be captured");
        assert_eq!(dynlibs.len(), 2, "expected two dynlib entries");
        assert_eq!(dynlibs[0].location, "usr");
        assert_eq!(dynlibs[0].rel_path, "lib/libfoo.so");
        assert_eq!(dynlibs[1].location, "site");
        assert_eq!(dynlibs[1].rel_path, "pkg.libs/libbar.so");
    }

    #[test]
    fn collect_overlay_blobs_rejects_absolute_blob_path() {
        let dir = tempdir().unwrap();
        let snapshot_path = dir.path().join("snapshot.bin");
        let overlay_path = dir.path().join("snapshot.bin.overlay.json");
        let digest = "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let metadata = json!({
            "version": 3,
            "format": "catalog",
            "packages": [{"canonical": "numpy", "digest": digest}],
            "blobs": {
                digest: {"digest": digest, "blob": "/tmp/overlay.tar"}
            }
        });

        let err = match collect_overlay_blobs(&snapshot_path, &overlay_path, &metadata) {
            Ok(_) => panic!("absolute blob path should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("cache-local filename"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_overlay_blobs_rejects_parent_relative_blob_path() {
        let dir = tempdir().unwrap();
        let snapshot_path = dir.path().join("snapshot.bin");
        let overlay_path = dir.path().join("snapshot.bin.overlay.json");
        let digest = "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let metadata = json!({
            "version": 3,
            "format": "catalog",
            "packages": [{"canonical": "numpy", "digest": digest}],
            "blobs": {
                digest: {"digest": digest, "blob": "../overlay.tar"}
            }
        });

        let err = match collect_overlay_blobs(&snapshot_path, &overlay_path, &metadata) {
            Ok(_) => panic!("parent-relative blob path should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("cache-local filename"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn collect_overlay_blobs_rejects_malformed_digest() {
        let dir = tempdir().unwrap();
        let snapshot_path = dir.path().join("snapshot.bin");
        let overlay_path = dir.path().join("snapshot.bin.overlay.json");
        let metadata = json!({
            "version": 3,
            "format": "catalog",
            "packages": [{"canonical": "numpy", "digest": "sha256:not-a-real-digest"}],
            "blobs": {}
        });

        let err = match collect_overlay_blobs(&snapshot_path, &overlay_path, &metadata) {
            Ok(_) => panic!("malformed digest should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("invalid overlay digest"),
            "unexpected error: {err}"
        );
    }
}
