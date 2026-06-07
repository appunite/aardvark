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

pub(in crate::engine) fn resolve_local_package_path(root: &Path, url: &str) -> Option<PathBuf> {
    let scheme_split = url.find("://").map(|idx| idx + 3).unwrap_or(0);
    let remainder = &url[scheme_split..];
    let path_part = remainder.split_once('/').map_or("", |(_, rest)| rest);
    if path_part.is_empty() {
        return None;
    }
    let trimmed = path_part
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let as_path = Path::new(trimmed);
    let mut attempts: Vec<PathBuf> = Vec::new();

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join(file_name));
    }

    if let Some(variant_relative) = strip_variant_prefix(as_path) {
        push_unique(&mut attempts, root.join(&variant_relative));
        if let Some(last) = variant_relative.file_name() {
            push_unique(&mut attempts, root.join(last));
        }
    }

    push_unique(&mut attempts, root.join(as_path));
    push_unique(&mut attempts, root.join("pyodide").join(as_path));

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join("full").join(file_name));
    }

    for candidate in attempts {
        tracing::debug!(
            target = "aardvark::packages",
            path = %candidate.display(),
            exists = candidate.exists(),
            "checking local package candidate"
        );
        if candidate.exists() {
            tracing::debug!(
                target = "aardvark::packages",
                path = %candidate.display(),
                "resolved local package path"
            );
            return Some(candidate);
        }
    }
    if let Some(file_name) = as_path.file_name() {
        if let Some(found) = walk_for_file(root, file_name) {
            tracing::debug!(
                target = "aardvark::packages",
                path = %found.display(),
                "resolved local package path via search"
            );
            return Some(found);
        }
    }
    None
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
