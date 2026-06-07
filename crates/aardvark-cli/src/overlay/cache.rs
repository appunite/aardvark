use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Result};
use tracing::warn;

#[derive(Clone, Debug)]
pub(super) struct OverlayCacheConfig {
    pub(super) root: PathBuf,
    pub(super) max_bytes: Option<u64>,
    pub(super) max_age: Option<Duration>,
}

struct CacheEntry {
    path: PathBuf,
    size: u64,
    modified_ns: u128,
}

#[derive(Default, Debug, Clone, Copy)]
pub(super) struct OverlayEvictionStats {
    pub(super) evicted_files: usize,
    pub(super) evicted_bytes: u128,
}

fn env_var(key: &str) -> Option<String> {
    let value = env::var(key).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn env_var_os(key: &str) -> Option<OsString> {
    let value = env::var_os(key)?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(super) fn overlay_cache_config(snapshot_path: Option<&Path>) -> OverlayCacheConfig {
    let env_dir = env_var("AARDVARK_OVERLAY_CACHE_DIR");
    let root = env_dir
        .map(PathBuf::from)
        .or_else(|| {
            snapshot_path.and_then(|path| path.parent().map(|parent| parent.join("_overlay_cache")))
        })
        .unwrap_or_else(|| PathBuf::from("_overlay_cache"));

    let max_bytes =
        env_var("AARDVARK_OVERLAY_CACHE_MAX_BYTES").and_then(|value| parse_cache_size(&value));
    let max_age =
        env_var("AARDVARK_OVERLAY_CACHE_MAX_AGE").and_then(|value| parse_cache_duration(&value));

    OverlayCacheConfig {
        root,
        max_bytes,
        max_age,
    }
}

pub(super) fn overlay_blob_dir(snapshot_path: &Path) -> PathBuf {
    overlay_cache_config(Some(snapshot_path)).root
}

fn parse_cache_size(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut idx = trimmed.len();
    for (i, ch) in trimmed.char_indices() {
        if !(ch.is_ascii_digit() || ch == '_' || ch == '.') {
            idx = i;
            break;
        }
    }
    let (number_part, unit_part) = trimmed.split_at(idx);
    let normalized = number_part.replace('_', "");
    if normalized.is_empty() {
        return None;
    }
    let number = normalized.parse::<f64>().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let unit = unit_part.trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let value = number * multiplier;
    if value > (u64::MAX as f64) {
        return None;
    }
    Some(value.round() as u64)
}

fn parse_cache_duration(input: &str) -> Option<Duration> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut idx = trimmed.len();
    for (i, ch) in trimmed.char_indices() {
        if !(ch.is_ascii_digit() || ch == '_' || ch == '.') {
            idx = i;
            break;
        }
    }
    let (number_part, unit_part) = trimmed.split_at(idx);
    let normalized = number_part.replace('_', "");
    if normalized.is_empty() {
        return None;
    }
    let number = normalized.parse::<f64>().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let unit = unit_part.trim().to_ascii_lowercase();
    let seconds = match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => number,
        "m" | "min" | "mins" | "minute" | "minutes" => number * 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => number * 3600.0,
        "d" | "day" | "days" => number * 86400.0,
        "w" | "week" | "weeks" => number * 604800.0,
        _ => return None,
    };
    if seconds > (u64::MAX as f64) {
        return None;
    }
    Some(Duration::from_secs(seconds.round() as u64))
}

pub(super) fn enforce_overlay_cache_policy(
    config: &OverlayCacheConfig,
) -> Result<OverlayEvictionStats> {
    if config.max_bytes.is_none() && config.max_age.is_none() {
        return Ok(OverlayEvictionStats::default());
    }
    let root = &config.root;
    if !root.exists() {
        return Ok(OverlayEvictionStats::default());
    }
    let now = SystemTime::now();
    let mut total_size: u128 = 0;
    let mut entries: Vec<CacheEntry> = Vec::new();
    let mut stats = OverlayEvictionStats::default();
    let iter = match fs::read_dir(root) {
        Ok(iter) => iter,
        Err(error) => {
            return Err(anyhow!(
                "failed to read overlay cache directory {}: {error}",
                root.display()
            ))
        }
    };
    for entry in iter {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!("failed to iterate overlay cache entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("tar"))
            != Some(true)
        {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            Err(error) => {
                warn!(
                    "failed to read metadata for overlay cache entry {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        let size = metadata.len();
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(max_age) = config.max_age {
            if let Ok(elapsed) = now.duration_since(modified) {
                if elapsed > max_age {
                    match fs::remove_file(&path) {
                        Ok(()) => {
                            stats.evicted_files += 1;
                            stats.evicted_bytes += size as u128;
                        }
                        Err(error) => {
                            warn!(
                                "failed to remove expired overlay blob {}: {error}",
                                path.display()
                            );
                        }
                    }
                    continue;
                }
            }
        }
        let modified_ns = modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        total_size += size as u128;
        entries.push(CacheEntry {
            path,
            size,
            modified_ns,
        });
    }

    if let Some(limit) = config.max_bytes {
        let limit = limit as u128;
        if total_size > limit {
            entries.sort_by_key(|entry| entry.modified_ns);
            for entry in &entries {
                if total_size <= limit {
                    break;
                }
                match fs::remove_file(&entry.path) {
                    Ok(()) => {
                        stats.evicted_files += 1;
                        stats.evicted_bytes += entry.size as u128;
                        total_size = total_size.saturating_sub(entry.size as u128);
                    }
                    Err(error) => {
                        warn!(
                            "failed to remove overlay blob during eviction {}: {error}",
                            entry.path.display()
                        );
                        continue;
                    }
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tempfile::tempdir;

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let original = env::var(key).ok();
            env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                env::set_var(self.key, value);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_cache_size_supports_common_units() {
        assert_eq!(
            parse_cache_size("64M"),
            Some(64 * 1024 * 1024),
            "megabyte parsing failed"
        );
        assert_eq!(
            parse_cache_size("1.5G"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64),
            "gigabyte parsing failed"
        );
        assert_eq!(parse_cache_size(""), None);
        assert_eq!(parse_cache_size("abc"), None);
    }

    #[test]
    fn parse_cache_duration_supports_units() {
        assert_eq!(parse_cache_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_cache_duration("90m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_cache_duration(""), None);
        assert_eq!(parse_cache_duration("foo"), None);
    }

    #[test]
    fn overlay_cache_config_respects_env_override() {
        let dir = tempdir().unwrap();
        let _guard = EnvGuard::set("AARDVARK_OVERLAY_CACHE_DIR", dir.path());
        let config = overlay_cache_config(None);
        assert_eq!(config.root, dir.path());
    }

    #[test]
    fn enforce_overlay_cache_policy_evicts_oldest_when_over_quota() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("sha256-first.tar");
        let second = dir.path().join("sha256-second.tar");
        fs::write(&first, vec![0u8; 2048]).unwrap();
        thread::sleep(Duration::from_millis(10));
        fs::write(&second, vec![0u8; 2048]).unwrap();
        let config = OverlayCacheConfig {
            root: dir.path().to_path_buf(),
            max_bytes: Some(2048),
            max_age: None,
        };
        let stats = enforce_overlay_cache_policy(&config).unwrap();
        assert!(
            !first.exists() && second.exists(),
            "expected oldest entry to be evicted when over quota"
        );
        assert!(stats.evicted_files >= 1, "expected at least one eviction");
        assert!(
            stats.evicted_bytes >= 2048,
            "expected eviction byte accounting"
        );
    }
}
