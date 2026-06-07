use std::fs::File;
use std::io::Read;
use std::path::Path;

use aardvark_core::pyodide_distribution::{
    DistributionFeatures, PackageFeatures, PyodideDistributionManifest,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use wasmparser::{Parser as WasmParser, Payload as WasmPayload, VisitOperator, VisitSimdOperator};

use crate::overlay::canonicalize_package_name;
use crate::read_limited::{read_file_limited, read_text_file_limited};

const MAX_FEATURE_SCAN_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PYODIDE_LOCKFILE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn distribution_features_for_reporting(
    root: &Path,
    manifest: &PyodideDistributionManifest,
) -> Result<DistributionFeatures> {
    if !manifest.features.is_empty() {
        return Ok(manifest.features.clone());
    }
    let lockfile_path = root.join(&manifest.lockfile.path);
    let lockfile_raw = read_text_file_limited(
        &lockfile_path,
        MAX_PYODIDE_LOCKFILE_BYTES,
        "Pyodide lockfile",
    )?;
    let lockfile_value: Value = serde_json::from_str(&lockfile_raw)
        .with_context(|| format!("parse {}", lockfile_path.display()))?;
    let package_root = manifest
        .package_root
        .as_deref()
        .map(|value| root.join(value))
        .unwrap_or_else(|| root.to_path_buf());
    scan_distribution_features(&package_root, &lockfile_value)
}

pub(super) fn print_distribution_features(features: &DistributionFeatures) {
    let mut enabled = Vec::new();
    if features.wasm_simd {
        enabled.push("wasm-simd");
    }
    if features.openblas {
        enabled.push("openblas");
    }
    if enabled.is_empty() {
        println!("features: none detected");
        return;
    }
    println!("features: {}", enabled.join(", "));

    let package_count = features.packages.len();
    let wasm_module_count: u32 = features
        .packages
        .values()
        .map(|package_features| package_features.wasm_modules)
        .sum();
    let simd_package_count = features
        .packages
        .values()
        .filter(|package_features| package_features.wasm_simd)
        .count();
    let openblas_package_count = features
        .packages
        .values()
        .filter(|package_features| package_features.openblas)
        .count();
    println!(
        "feature package summary: {package_count} packages, {wasm_module_count} wasm modules, {simd_package_count} wasm-simd packages, {openblas_package_count} openblas-linked packages"
    );

    let highlights = features
        .packages
        .iter()
        .filter(|(name, package_features)| {
            package_features.openblas
                || matches!(
                    name.as_str(),
                    "numpy" | "pandas" | "scipy" | "scikit-learn" | "matplotlib" | "libopenblas"
                )
        })
        .map(|(name, package_features)| {
            format!("{name}({})", format_package_features(package_features))
        })
        .collect::<Vec<_>>();
    if !highlights.is_empty() {
        println!("feature package highlights: {}", highlights.join(", "));
    }
}

fn format_package_features(features: &PackageFeatures) -> String {
    let mut flags = Vec::new();
    if features.wasm_simd {
        flags.push("wasm-simd");
    }
    if features.openblas {
        flags.push("openblas");
    }
    if features.wasm_modules > 0 {
        flags.push("wasm-modules");
    }
    flags.join("+")
}

pub(super) fn scan_distribution_features(
    root: &Path,
    lockfile: &Value,
) -> Result<DistributionFeatures> {
    let mut summary = DistributionFeatures::default();
    let packages = lockfile
        .get("packages")
        .and_then(Value::as_object)
        .context("pyodide-lock.json missing packages object")?;

    for (canonical, package) in packages {
        let Some(file_name) = package.get("file_name").and_then(Value::as_str) else {
            continue;
        };
        let path = root.join(file_name);
        if !path.exists() {
            continue;
        }
        let mut features = scan_package_features(&path)
            .with_context(|| format!("scan package features for {}", path.display()))?;
        if package_depends_on_openblas(package) {
            features.openblas = true;
        }
        if features.is_empty() {
            continue;
        }
        summary.wasm_simd |= features.wasm_simd;
        summary.openblas |= features.openblas;
        summary.packages.insert(canonical.clone(), features);
    }

    Ok(summary)
}

fn package_depends_on_openblas(package: &Value) -> bool {
    package
        .get("depends")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|dep| canonicalize_package_name(dep) == "libopenblas")
}

fn scan_package_features(path: &Path) -> Result<PackageFeatures> {
    let extension = path.extension().and_then(|value| value.to_str());
    if matches!(extension, Some("whl" | "zip")) {
        return scan_zip_package_features(path);
    }

    let mut features = PackageFeatures::default();
    let bytes = read_feature_scan_bytes(path)?;
    scan_binary_feature_bytes(
        path.file_name().and_then(|name| name.to_str()),
        &bytes,
        &mut features,
    )?;
    Ok(features)
}

fn scan_zip_package_features(path: &Path) -> Result<PackageFeatures> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("open zip {}", path.display()))?;
    let mut features = PackageFeatures::default();

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("read zip entry {index} from {}", path.display()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        if !is_feature_scan_candidate(&name) {
            if name.to_ascii_lowercase().contains("openblas") {
                features.openblas = true;
            }
            continue;
        }
        if entry.size() > MAX_FEATURE_SCAN_BYTES {
            bail!(
                "refusing to scan zip entry {name} from {}: {} bytes exceeds the {} byte feature-scan limit",
                path.display(),
                entry.size(),
                MAX_FEATURE_SCAN_BYTES
            );
        }
        let mut bytes = Vec::with_capacity(entry.size().min(8 * 1024 * 1024) as usize);
        let mut limited = entry.take(MAX_FEATURE_SCAN_BYTES.saturating_add(1));
        limited
            .read_to_end(&mut bytes)
            .with_context(|| format!("read zip entry {name} from {}", path.display()))?;
        if bytes.len() as u64 > MAX_FEATURE_SCAN_BYTES {
            bail!(
                "zip entry {name} from {} exceeded the {} byte feature-scan limit while reading",
                path.display(),
                MAX_FEATURE_SCAN_BYTES
            );
        }
        scan_binary_feature_bytes(Some(name.as_str()), &bytes, &mut features)?;
    }

    Ok(features)
}

fn read_feature_scan_bytes(path: &Path) -> Result<Vec<u8>> {
    read_file_limited(path, MAX_FEATURE_SCAN_BYTES, "feature scan target")
}

fn is_feature_scan_candidate(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with(".so")
        || lowered.ends_with(".wasm")
        || lowered.ends_with(".data")
        || lowered.contains("openblas")
}

pub(crate) fn scan_binary_feature_bytes(
    name: Option<&str>,
    bytes: &[u8],
    features: &mut PackageFeatures,
) -> Result<()> {
    if name
        .map(|value| value.to_ascii_lowercase().contains("openblas"))
        .unwrap_or(false)
        || contains_ascii(bytes, b"libopenblas")
        || contains_ascii(bytes, b"openblas")
    {
        features.openblas = true;
    }

    if bytes.starts_with(b"\0asm") {
        features.wasm_modules = features.wasm_modules.saturating_add(1);
        if wasm_module_uses_simd(bytes)? {
            features.wasm_simd = true;
        }
    }
    Ok(())
}

pub(crate) fn wasm_module_uses_simd(bytes: &[u8]) -> Result<bool> {
    let mut visitor = SimdDetector;
    for payload in WasmParser::new(0).parse_all(bytes) {
        if let WasmPayload::CodeSectionEntry(body) = payload? {
            let reader = body.get_operators_reader()?;
            for op in reader {
                if visitor.visit_operator(&op?) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

struct SimdDetector;

macro_rules! non_simd_operator {
    ($(@$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        $(
            fn $visit(&mut self $($(,$arg: $argty)*)?) -> Self::Output {
                $( $( let _ = $arg; )* )?
                false
            }
        )*
    };
}

impl<'a> VisitOperator<'a> for SimdDetector {
    type Output = bool;

    fn simd_visitor(&mut self) -> Option<&mut dyn VisitSimdOperator<'a, Output = Self::Output>> {
        Some(self)
    }

    wasmparser::for_each_visit_operator!(non_simd_operator);
}

macro_rules! simd_operator {
    ($(@$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        $(
            fn $visit(&mut self $($(,$arg: $argty)*)?) -> Self::Output {
                $( $( let _ = $arg; )* )?
                true
            }
        )*
    };
}

impl<'a> VisitSimdOperator<'a> for SimdDetector {
    wasmparser::for_each_visit_simd_operator!(simd_operator);
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
