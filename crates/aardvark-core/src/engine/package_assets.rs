use std::env;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use url::Url;
use walkdir::WalkDir;

pub(in crate::engine) const MAX_LOCAL_PACKAGE_ASSET_BYTES: u64 = 512 * 1024 * 1024;

pub(in crate::engine) fn normalize_package_root(path: PathBuf) -> PathBuf {
    if path.is_relative() {
        if let Ok(cwd) = env::current_dir() {
            return cwd.join(path);
        }
    }
    path
}

/// Map a fetch URL onto a file of the staged distribution under `root`.
///
/// Guest code can call the native fetch hook with any URL, so this is a trust
/// boundary: the path comes from the parsed URL, segments that are empty,
/// `.`, `..` or contain a separator are refused, and a candidate is served
/// only if it canonicalizes to a regular file inside the canonical `root`
/// (no traversal, no symlink escape, no `/proc` or device files).
pub(in crate::engine) fn resolve_local_package_path(root: &Path, url: &str) -> Option<PathBuf> {
    let parsed = Url::parse(url).ok()?;
    let segments: Vec<&str> = parsed.path_segments()?.collect();
    // Distribution file names never need escapes, so a `%` (an encoded dot or
    // separator, say) is refused rather than decoded.
    let safe = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && !segment.contains(['/', '\\', '%', '\0'])
    };
    // A trailing empty segment (`.../dir/`) names no file.
    if segments.is_empty() || !segments.iter().all(|segment| safe(segment)) {
        return None;
    }
    let as_path: PathBuf = segments.iter().collect();
    let canonical_root = root.canonicalize().ok()?;
    let mut attempts: Vec<PathBuf> = Vec::new();

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join(file_name));
    }

    if let Some(variant_relative) = strip_variant_prefix(&as_path) {
        push_unique(&mut attempts, root.join(&variant_relative));
        if let Some(last) = variant_relative.file_name() {
            push_unique(&mut attempts, root.join(last));
        }
    }

    push_unique(&mut attempts, root.join(&as_path));
    push_unique(&mut attempts, root.join("pyodide").join(&as_path));

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join("full").join(file_name));
    }

    for candidate in attempts {
        let confined = confine(&canonical_root, &candidate);
        tracing::debug!(
            target: "aardvark::packages",
            path = %candidate.display(),
            served = confined.is_some(),
            "checking local package candidate"
        );
        if let Some(path) = confined {
            tracing::debug!(
                target: "aardvark::packages",
                path = %path.display(),
                "resolved local package path"
            );
            return Some(path);
        }
    }
    if let Some(file_name) = as_path.file_name() {
        if let Some(found) = walk_for_file(root, file_name) {
            if let Some(path) = confine(&canonical_root, &found) {
                tracing::debug!(
                    target: "aardvark::packages",
                    path = %path.display(),
                    "resolved local package path via search"
                );
                return Some(path);
            }
        }
    }
    None
}

/// `candidate`, canonicalized, if it is a regular file inside
/// `canonical_root`.
fn confine(canonical_root: &Path, candidate: &Path) -> Option<PathBuf> {
    let resolved = candidate.canonicalize().ok()?;
    let inside = resolved.starts_with(canonical_root);
    let regular = resolved.metadata().is_ok_and(|meta| meta.is_file());
    (inside && regular).then_some(resolved)
}

pub(in crate::engine) fn is_pyodide_package_asset_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    let path = parsed.path().to_ascii_lowercase();
    let has_package_extension = matches!(
        Path::new(&path).extension().and_then(|ext| ext.to_str()),
        Some("whl" | "zip" | "tar")
    ) || path.ends_with(".tar.gz")
        || path.ends_with(".tar.bz2");
    if !has_package_extension {
        return false;
    }
    host.contains("pyodide")
        || path.contains("/pyodide/")
        || path.starts_with("/pyodide/")
        || path.starts_with("/pyodide@")
        || path.contains("/pyodide@")
}

fn strip_variant_prefix(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    match (components.next()?, components.next(), components.next()) {
        (
            Component::Normal(first),
            Some(Component::Normal(_version)),
            Some(Component::Normal(_variant)),
        ) if first == OsStr::new("pyodide") => {
            let remaining = components.as_path();
            if remaining.as_os_str().is_empty() {
                None
            } else {
                Some(remaining.to_path_buf())
            }
        }
        _ => None,
    }
}

fn push_unique(list: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !list.iter().any(|existing| existing == &candidate) {
        list.push(candidate);
    }
}

fn walk_for_file(root: &Path, needle: &OsStr) -> Option<PathBuf> {
    let walker = WalkDir::new(root).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.file_name() == Some(needle) {
            return Some(path.to_path_buf());
        }
    }
    None
}

pub(in crate::engine) fn guess_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("json") => "application/json",
        Some("js") => "application/javascript",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("py") => "text/x-python",
        Some("data") => "application/octet-stream",
        Some("whl") => "application/octet-stream",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("bz2") => "application/x-bzip2",
        Some("tar") => "application/x-tar",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod confinement_tests {
    use super::*;
    use std::fs;

    fn staged() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("dist");
        fs::create_dir_all(root.join("full")).unwrap();
        fs::write(root.join("pyodide.asm.wasm"), b"wasm").unwrap();
        fs::write(root.join("full/numpy-2.4.whl"), b"wheel").unwrap();
        fs::write(temp.path().join("secret.txt"), b"secret").unwrap();
        (temp, root)
    }

    #[test]
    fn serves_distribution_files() {
        let (_temp, root) = staged();
        let resolved = resolve_local_package_path(
            &root,
            "https://cdn.jsdelivr.net/pyodide/v314.0.7/full/pyodide.asm.wasm",
        )
        .expect("distribution asset");
        assert!(resolved.ends_with("pyodide.asm.wasm"));
        assert!(resolve_local_package_path(&root, "http://x/numpy-2.4.whl")
            .expect("found by search")
            .ends_with("full/numpy-2.4.whl"));
    }

    #[test]
    fn refuses_traversal_in_any_spelling() {
        let (_temp, root) = staged();
        for url in [
            "http://x/../secret.txt",
            "http://x/../../../../../etc/passwd",
            "http://x/%2e%2e/secret.txt",
            "http://x/full/%2e%2e%2f..%2fsecret.txt",
            "http://x/..%5csecret.txt",
            "http://x/proc/self/environ",
            "http://x//etc/passwd",
        ] {
            assert_eq!(resolve_local_package_path(&root, url), None, "{url}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_out_of_the_root_and_non_regular_files() {
        let (temp, root) = staged();
        std::os::unix::fs::symlink(temp.path().join("secret.txt"), root.join("escape.whl"))
            .unwrap();
        assert_eq!(
            resolve_local_package_path(&root, "http://x/escape.whl"),
            None
        );
        std::os::unix::fs::symlink("/proc/self/environ", root.join("environ")).unwrap();
        assert_eq!(resolve_local_package_path(&root, "http://x/environ"), None);
        fs::create_dir(root.join("subdir")).unwrap();
        assert_eq!(resolve_local_package_path(&root, "http://x/subdir"), None);
    }
}
