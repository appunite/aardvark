use crate::assets;
use crate::config::PyRuntimeConfig;
use crate::error::{PyRunnerError, Result};
use crate::pyodide::{PYODIDE_ADAPTER_VERSION, PYODIDE_VERSION};
use hex::ToHex;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const DISTRIBUTION_MANIFEST: &str = "aardvark-pyodide-dist.json";
const MAX_EXTERNAL_BINARY_ASSET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXTERNAL_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXTERNAL_TEXT_ASSET_BYTES: u64 = 256 * 1024 * 1024;

static VERIFIED_EXTERNAL_DISTRIBUTIONS: Lazy<Mutex<BTreeSet<String>>> =
    Lazy::new(|| Mutex::new(BTreeSet::new()));
static EXTERNAL_TEXT_ASSET_CACHE: Lazy<Mutex<BTreeMap<String, Arc<str>>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));
static EXTERNAL_BINARY_ASSET_CACHE: Lazy<Mutex<BTreeMap<String, Arc<[u8]>>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

#[derive(Clone, Debug)]
pub struct PyodideDistribution {
    source: DistributionSource,
    manifest: PyodideDistributionManifest,
    package_root: Option<PathBuf>,
    external_cache_key: Option<String>,
}

#[derive(Clone, Debug)]
enum DistributionSource {
    Embedded,
    External { root: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyodideDistributionManifest {
    pub schema_version: u32,
    pub aardvark_version: String,
    pub pyodide_version: String,
    pub adapter_version: String,
    pub variant: PyodideDistributionVariant,
    pub upstream: UpstreamArchives,
    pub python: PythonCompatibility,
    pub lockfile: LockfileManifest,
    pub package_root: Option<String>,
    #[serde(default, skip_serializing_if = "DistributionFeatures::is_empty")]
    pub features: DistributionFeatures,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    pub compatibility_fingerprint: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionFeatures {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub packages: BTreeMap<String, PackageFeatures>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wasm_simd: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub openblas: bool,
}

impl DistributionFeatures {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && !self.wasm_simd && !self.openblas
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFeatures {
    #[serde(default, skip_serializing_if = "is_false")]
    pub wasm_simd: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub openblas: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wasm_modules: u32,
}

impl PackageFeatures {
    pub fn is_empty(&self) -> bool {
        !self.wasm_simd && !self.openblas && self.wasm_modules == 0
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PyodideDistributionVariant {
    Core,
    Full,
}

impl PyodideDistributionVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamArchives {
    pub base_url: String,
    pub core: UpstreamArchive,
    pub full: UpstreamArchive,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamArchive {
    pub name: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PythonCompatibility {
    pub version: String,
    pub abi: String,
    pub platform: String,
    pub arch: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockfileManifest {
    pub path: String,
    pub sha256: String,
}

impl PyodideDistribution {
    pub fn resolve(config: &PyRuntimeConfig) -> Result<Self> {
        if let Some(root) = config.pyodide_dist_dir.as_ref() {
            return Self::external(root);
        }
        Self::embedded()
    }

    pub fn external(root: impl AsRef<Path>) -> Result<Self> {
        let root = normalize_path(root.as_ref());
        let manifest_path = root.join(DISTRIBUTION_MANIFEST);
        let raw = read_text_file_limited(
            &manifest_path,
            MAX_EXTERNAL_MANIFEST_BYTES,
            "Pyodide distribution manifest",
        )?;
        let manifest: PyodideDistributionManifest = serde_json::from_str(&raw).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to parse Pyodide distribution manifest {}: {err}",
                manifest_path.display()
            ))
        })?;
        verify_manifest_identity(&manifest)?;
        verify_manifest_fingerprint(&manifest)?;
        let external_cache_key = verify_external_files(&root, &manifest)?;
        let package_root = manifest
            .package_root
            .as_deref()
            .map(|value| root.join(value))
            .or_else(|| Some(root.clone()));
        Ok(Self {
            source: DistributionSource::External { root },
            manifest,
            package_root,
            external_cache_key: Some(external_cache_key),
        })
    }

    pub fn embedded() -> Result<Self> {
        let manifest: PyodideDistributionManifest =
            serde_json::from_str(assets::distribution_manifest_json()).map_err(|err| {
                PyRunnerError::Init(format!(
                    "embedded Pyodide distribution manifest is invalid: {err}"
                ))
            })?;
        verify_manifest_identity(&manifest)?;
        verify_manifest_fingerprint(&manifest)?;
        Ok(Self {
            source: DistributionSource::Embedded,
            manifest,
            package_root: None,
            external_cache_key: None,
        })
    }

    pub fn manifest(&self) -> &PyodideDistributionManifest {
        &self.manifest
    }

    pub fn compatibility_fingerprint(&self) -> &str {
        &self.manifest.compatibility_fingerprint
    }

    pub fn package_root(&self) -> Option<PathBuf> {
        self.package_root.clone()
    }

    pub fn read_text_asset(&self, name: &str) -> Result<Arc<str>> {
        match &self.source {
            DistributionSource::Embedded => embedded_text_asset(name).map(Arc::<str>::from),
            DistributionSource::External { root } => {
                let cache_key = self.external_cache_key.as_deref().ok_or_else(|| {
                    PyRunnerError::Init("external Pyodide distribution missing cache key".into())
                })?;
                read_external_text_asset(root, cache_key, name)
            }
        }
    }

    pub fn read_binary_asset(&self, name: &str) -> Result<Arc<[u8]>> {
        match &self.source {
            DistributionSource::Embedded => embedded_binary_asset(name).map(Arc::<[u8]>::from),
            DistributionSource::External { root } => {
                let cache_key = self.external_cache_key.as_deref().ok_or_else(|| {
                    PyRunnerError::Init("external Pyodide distribution missing cache key".into())
                })?;
                read_external_binary_asset(root, cache_key, name)
            }
        }
    }
}

pub fn compute_compatibility_fingerprint(manifest: &PyodideDistributionManifest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aardvark-pyodide-distribution-v1\n");
    hasher.update(format!("pyodide={}\n", manifest.pyodide_version).as_bytes());
    hasher.update(format!("adapter={}\n", manifest.adapter_version).as_bytes());
    hasher.update(format!("variant={}\n", manifest.variant.as_str()).as_bytes());
    hasher.update(format!("python={}\n", manifest.python.version).as_bytes());
    hasher.update(format!("abi={}\n", manifest.python.abi).as_bytes());
    hasher.update(format!("platform={}\n", manifest.python.platform).as_bytes());
    hasher.update(format!("arch={}\n", manifest.python.arch).as_bytes());
    hasher.update(format!("lockfile={}\n", manifest.lockfile.sha256).as_bytes());
    format!("sha256:{}", hasher.finalize().encode_hex::<String>())
}

fn verify_manifest_identity(manifest: &PyodideDistributionManifest) -> Result<()> {
    if manifest.schema_version != 1 {
        return Err(PyRunnerError::Init(format!(
            "unsupported Pyodide distribution schema version {}",
            manifest.schema_version
        )));
    }
    if manifest.pyodide_version != PYODIDE_VERSION {
        return Err(PyRunnerError::Init(format!(
            "Pyodide distribution version mismatch: build expects {}, distribution has {}",
            PYODIDE_VERSION, manifest.pyodide_version
        )));
    }
    if manifest.adapter_version != PYODIDE_ADAPTER_VERSION {
        return Err(PyRunnerError::Init(format!(
            "Pyodide adapter version mismatch: build expects {}, distribution has {}",
            PYODIDE_ADAPTER_VERSION, manifest.adapter_version
        )));
    }
    Ok(())
}

fn verify_manifest_fingerprint(manifest: &PyodideDistributionManifest) -> Result<()> {
    let actual = compute_compatibility_fingerprint(manifest);
    if actual != manifest.compatibility_fingerprint {
        return Err(PyRunnerError::Init(format!(
            "Pyodide distribution fingerprint mismatch: expected {}, computed {}",
            manifest.compatibility_fingerprint, actual
        )));
    }
    Ok(())
}

fn verify_external_files(root: &Path, manifest: &PyodideDistributionManifest) -> Result<String> {
    let cache_key = external_verification_cache_key(root, manifest);
    if VERIFIED_EXTERNAL_DISTRIBUTIONS.lock().contains(&cache_key) {
        tracing::debug!(
            target: "aardvark::packages",
            root = %root.display(),
            fingerprint = %manifest.compatibility_fingerprint,
            "using cached Pyodide distribution verification"
        );
        return Ok(cache_key);
    }
    for (rel, expected) in &manifest.files {
        let path = root.join(rel);
        let actual = sha256_file(&path)?;
        if actual != normalize_sha256(expected) {
            return Err(PyRunnerError::Init(format!(
                "Pyodide distribution file checksum mismatch for {}: expected {}, got sha256:{}",
                path.display(),
                expected,
                actual
            )));
        }
    }
    VERIFIED_EXTERNAL_DISTRIBUTIONS
        .lock()
        .insert(cache_key.clone());
    Ok(cache_key)
}

fn external_verification_cache_key(root: &Path, manifest: &PyodideDistributionManifest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(manifest.compatibility_fingerprint.as_bytes());
    hasher.update(b"\0");
    for (rel, expected) in &manifest.files {
        hasher.update(rel.as_bytes());
        hasher.update(b"=");
        hasher.update(expected.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().encode_hex::<String>()
}

fn external_asset_cache_key(distribution_key: &str, name: &str) -> String {
    let mut key = String::with_capacity(distribution_key.len() + name.len() + 1);
    key.push_str(distribution_key);
    key.push('\0');
    key.push_str(name);
    key
}

fn read_external_text_asset(root: &Path, distribution_key: &str, name: &str) -> Result<Arc<str>> {
    let cache_key = external_asset_cache_key(distribution_key, name);
    if let Some(cached) = EXTERNAL_TEXT_ASSET_CACHE.lock().get(&cache_key).cloned() {
        return Ok(cached);
    }

    let path = root.join(name);
    let text = read_text_file_limited(&path, MAX_EXTERNAL_TEXT_ASSET_BYTES, "Pyodide text asset")?;
    let text = Arc::<str>::from(text);
    let mut cache = EXTERNAL_TEXT_ASSET_CACHE.lock();
    let cached = cache.entry(cache_key).or_insert_with(|| text.clone());
    Ok(cached.clone())
}

fn read_external_binary_asset(
    root: &Path,
    distribution_key: &str,
    name: &str,
) -> Result<Arc<[u8]>> {
    let cache_key = external_asset_cache_key(distribution_key, name);
    if let Some(cached) = EXTERNAL_BINARY_ASSET_CACHE.lock().get(&cache_key).cloned() {
        return Ok(cached);
    }

    let path = root.join(name);
    let bytes = read_file_limited(
        &path,
        MAX_EXTERNAL_BINARY_ASSET_BYTES,
        "Pyodide binary asset",
    )?;
    let bytes = Arc::<[u8]>::from(bytes.into_boxed_slice());
    let mut cache = EXTERNAL_BINARY_ASSET_CACHE.lock();
    let cached = cache.entry(cache_key).or_insert_with(|| bytes.clone());
    Ok(cached.clone())
}

fn read_text_file_limited(path: &Path, limit: u64, kind: &str) -> Result<String> {
    let bytes = read_file_limited(path, limit, kind)?;
    String::from_utf8(bytes).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to read {kind} {} as UTF-8: {err}",
            path.display()
        ))
    })
}

fn read_file_limited(path: &Path, limit: u64, kind: &str) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|err| {
        PyRunnerError::Init(format!("failed to open {kind} {}: {err}", path.display()))
    })?;
    let metadata = file.metadata().map_err(|err| {
        PyRunnerError::Init(format!("failed to stat {kind} {}: {err}", path.display()))
    })?;
    let len = metadata.len();
    if len > limit {
        return Err(PyRunnerError::Init(format!(
            "refusing to read {kind} {}: {} bytes exceeds the {} byte limit",
            path.display(),
            len,
            limit
        )));
    }

    let mut bytes = Vec::with_capacity(len.min(8 * 1024 * 1024) as usize);
    let mut limited = file.take(limit.saturating_add(1));
    limited.read_to_end(&mut bytes).map_err(|err| {
        PyRunnerError::Init(format!("failed to read {kind} {}: {err}", path.display()))
    })?;
    if bytes.len() as u64 > limit {
        return Err(PyRunnerError::Init(format!(
            "{kind} {} exceeded the {} byte limit while reading",
            path.display(),
            limit
        )));
    }
    Ok(bytes)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to open Pyodide distribution file {}: {err}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 131_072];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to read Pyodide distribution file {}: {err}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().encode_hex::<String>())
}

fn normalize_sha256(value: &str) -> String {
    value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .to_ascii_lowercase()
}

fn normalize_path(path: &Path) -> PathBuf {
    if path.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join(path);
        }
    }
    path.to_path_buf()
}

fn embedded_text_asset(name: &str) -> Result<&'static str> {
    match name {
        DISTRIBUTION_MANIFEST => Ok(assets::distribution_manifest_json()),
        "pyodide.asm.js" => Ok(assets::pyodide_asm_js()),
        "pyodide.asm.patched.js" => Ok(assets::pyodide_asm_patched_js()),
        "pyodide_builtin_wrappers.js" => Ok(assets::builtin_wrappers_js()),
        "pyodide_bootstrap.js" => Ok(assets::bootstrap_js()),
        "pyodide_emscripten_setup.js" => Ok(assets::emscripten_setup_js()),
        "pyodide_packages.js" => Ok(assets::packages_js()),
        "pyodide.mjs" => Ok(assets::loader_mjs()),
        "pyodide.js" => Ok(assets::loader_js()),
        "pyodide-lock.json" => Ok(assets::lockfile_json_raw()),
        _ => Err(PyRunnerError::Init(format!(
            "embedded Pyodide text asset not found: {name}"
        ))),
    }
}

fn embedded_binary_asset(name: &str) -> Result<&'static [u8]> {
    match name {
        "pyodide.asm.wasm" => Ok(assets::wasm()),
        "python_stdlib.zip" => Ok(assets::python_stdlib_zip()),
        _ => Err(PyRunnerError::Init(format!(
            "embedded Pyodide binary asset not found: {name}"
        ))),
    }
}
