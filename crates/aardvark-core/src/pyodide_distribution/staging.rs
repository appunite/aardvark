use super::{
    compute_compatibility_fingerprint, DistributionFeatures, LockfileManifest, PackageFeatures,
    PyodideDistribution, PyodideDistributionManifest, PyodideDistributionVariant,
    PythonCompatibility, UpstreamArchive, UpstreamArchives, DISTRIBUTION_MANIFEST,
};
use crate::error::{PyRunnerError, Result};
use crate::pyodide::{
    PYODIDE_ADAPTER_VERSION, PYODIDE_CORE_ARCHIVE_NAME, PYODIDE_CORE_ARCHIVE_SHA256,
    PYODIDE_FULL_ARCHIVE_NAME, PYODIDE_FULL_ARCHIVE_SHA256, PYODIDE_RELEASE_BASE_URL,
    PYODIDE_VERSION,
};
use bzip2::read::BzDecoder;
use hex::ToHex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tar::Archive;
use ureq::Agent;
use wasmparser::{Parser as WasmParser, Payload as WasmPayload, VisitOperator, VisitSimdOperator};

const MAX_PYODIDE_LOCKFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PYODIDE_PATCH_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PYODIDE_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FEATURE_SCAN_BYTES: u64 = 128 * 1024 * 1024;

/// Options for staging the pinned Aardvark-compatible Pyodide distribution.
#[derive(Clone, Debug)]
pub struct PyodideDistributionStageOptions {
    /// Pyodide archive variant to stage.
    pub variant: PyodideDistributionVariant,
    /// Destination directory for the staged distribution.
    pub output_dir: PathBuf,
    /// Optional local upstream Pyodide `.tar.bz2` archive.
    ///
    /// When `None`, the pinned archive for `variant` is downloaded from the
    /// Pyodide release URL compiled into this Aardvark version.
    pub archive: Option<PathBuf>,
    /// Replace an existing Aardvark staged distribution directory.
    pub force: bool,
}

/// Summary of a successfully staged and verified Pyodide distribution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PyodideDistributionStageReport {
    /// Directory containing the verified staged distribution.
    pub output_dir: PathBuf,
    /// Staged Pyodide distribution variant.
    pub variant: PyodideDistributionVariant,
    /// Compatibility fingerprint computed from the generated distribution manifest.
    pub compatibility_fingerprint: String,
}

/// Returns Aardvark's conventional staging path for a Pyodide variant.
pub fn default_pyodide_distribution_stage_output_dir(
    variant: PyodideDistributionVariant,
) -> PathBuf {
    PathBuf::from(".aardvark/pyodide-distributions").join(format!(
        "aardvark-{}-pyodide-v{}-{}",
        env!("CARGO_PKG_VERSION"),
        PYODIDE_VERSION,
        variant.as_str()
    ))
}

/// Stages and verifies an Aardvark-compatible Pyodide distribution.
///
/// The staging flow downloads the pinned upstream Pyodide archive when no
/// archive is supplied, verifies the upstream SHA-256 digest, unpacks the
/// requested `core` or `full` distribution, copies Aardvark adapter assets,
/// generates `pyodide.asm.patched.js`, writes
/// `aardvark-pyodide-dist.json`, verifies the result with
/// [`PyodideDistribution::external`], and returns a small report.
pub fn stage_pyodide_distribution(
    options: PyodideDistributionStageOptions,
) -> Result<PyodideDistributionStageReport> {
    let PyodideDistributionStageOptions {
        variant,
        output_dir,
        archive,
        force,
    } = options;

    let workspace = tempfile::tempdir().map_err(|err| {
        PyRunnerError::Init(format!("failed to create Pyodide staging workspace: {err}"))
    })?;
    let user_supplied = archive.is_some();
    let archive_path = match archive {
        Some(path) => path,
        None => download_variant_archive(variant, workspace.path())?,
    };

    if user_supplied {
        verify_sha256(&archive_path, variant.expected_sha()).map_err(|err| {
            PyRunnerError::Init(format!(
                "supplied Pyodide archive failed checksum for {}: {err}",
                archive_path.display()
            ))
        })?;
    }

    tracing::info!(
        target = "aardvark::assets",
        variant = variant.as_str(),
        archive = %archive_path.display(),
        "unpacking Pyodide archive"
    );

    unpack_archive(&archive_path, workspace.path())?;
    let pyodide_root = find_pyodide_dir(workspace.path())
        .ok_or_else(|| PyRunnerError::Init("expected 'pyodide' directory inside archive".into()))?;
    let source_dir = pyodide_variant_source_dir(&pyodide_root, variant)?;

    prepare_output_dir(&output_dir, force)?;
    copy_dir_recursive(&source_dir, &output_dir)?;
    copy_adapter_assets(&output_dir)?;
    generate_patched_pyodide(&output_dir)?;
    write_distribution_manifest(&output_dir, variant)?;

    let verified = PyodideDistribution::external(&output_dir).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to verify staged distribution {}: {err}",
            output_dir.display()
        ))
    })?;

    let compatibility_fingerprint = verified.compatibility_fingerprint().to_owned();
    tracing::info!(
        target = "aardvark::assets",
        variant = variant.as_str(),
        output = %output_dir.display(),
        fingerprint = %compatibility_fingerprint,
        "staged Aardvark Pyodide distribution"
    );

    Ok(PyodideDistributionStageReport {
        output_dir,
        variant,
        compatibility_fingerprint,
    })
}

impl PyodideDistributionVariant {
    fn archive_name(self) -> &'static str {
        match self {
            Self::Core => PYODIDE_CORE_ARCHIVE_NAME,
            Self::Full => PYODIDE_FULL_ARCHIVE_NAME,
        }
    }

    fn expected_sha(self) -> &'static str {
        match self {
            Self::Core => PYODIDE_CORE_ARCHIVE_SHA256,
            Self::Full => PYODIDE_FULL_ARCHIVE_SHA256,
        }
    }

    fn archive_url(self) -> String {
        format!(
            "{base}/{version}/{name}",
            base = PYODIDE_RELEASE_BASE_URL,
            version = PYODIDE_VERSION,
            name = self.archive_name()
        )
    }
}

fn pyodide_variant_source_dir(
    pyodide_root: &Path,
    variant: PyodideDistributionVariant,
) -> Result<PathBuf> {
    let nested = pyodide_root
        .join("pyodide")
        .join(format!("v{PYODIDE_VERSION}"))
        .join(variant.as_str());
    if nested.exists() {
        return Ok(nested);
    }
    if pyodide_root.join("pyodide-lock.json").exists() {
        return Ok(pyodide_root.to_path_buf());
    }
    Err(PyRunnerError::Init(format!(
        "archive missing Pyodide distribution files under {}",
        pyodide_root.display()
    )))
}

fn copy_adapter_assets(output_dir: &Path) -> Result<()> {
    let embedded = PyodideDistribution::embedded().map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to load embedded Aardvark Pyodide adapter assets: {err}"
        ))
    })?;
    for file in [
        "pyodide_builtin_wrappers.js",
        "pyodide_bootstrap.js",
        "pyodide_emscripten_setup.js",
        "pyodide_packages.js",
    ] {
        let dst = output_dir.join(file);
        let text = embedded.read_text_asset(file).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to read embedded adapter asset {file}: {err}"
            ))
        })?;
        fs::write(&dst, text.as_ref()).map_err(|err| {
            PyRunnerError::Init(format!("failed to write {}: {err}", dst.display()))
        })?;
    }
    Ok(())
}

fn generate_patched_pyodide(output_dir: &Path) -> Result<()> {
    let source_path = output_dir.join("pyodide.asm.js");
    let target_path = output_dir.join("pyodide.asm.patched.js");
    let source = read_text_file_limited(
        &source_path,
        MAX_PYODIDE_PATCH_SOURCE_BYTES,
        "Pyodide JS source",
    )?;
    let patched = apply_pyodide_replacements(&source)?;
    fs::write(&target_path, patched).map_err(|err| {
        PyRunnerError::Init(format!("failed to write {}: {err}", target_path.display()))
    })?;
    Ok(())
}

fn apply_pyodide_replacements(source: &str) -> Result<String> {
    const PRELUDE: &str = r#"import {
    addEventListener,
    getRandomValues,
    location,
    monotonicDateNow,
    newWasmModule,
    patchedApplyFunc,
    patchDynlibLookup,
    reportUndefinedSymbolsPatched,
    wasmInstantiate,
    patched_PyEM_CountFuncParams,
} from "./pyodide_builtin_wrappers.js";
"#;

    let required_replacements: [(&str, String); 8] = [
        (
            "var _createPyodideModule",
            format!("{PRELUDE}export const _createPyodideModule"),
        ),
        (
            "globalThis._createPyodideModule = _createPyodideModule;",
            String::new(),
        ),
        ("new WebAssembly.Module", "newWasmModule".into()),
        ("WebAssembly.instantiate", "wasmInstantiate".into()),
        ("Date.now", "monotonicDateNow".into()),
        (
            "reportUndefinedSymbols()",
            "reportUndefinedSymbolsPatched(Module)".into(),
        ),
        ("crypto.getRandomValues(", "getRandomValues(Module, ".into()),
        (
            "const API=Module.API;",
            "const API=Module.API||(Module.API={});if(!API.runtimeEnv){API.runtimeEnv={IN_BUN:false,IN_DENO:false,IN_NODE:false,IN_SAFARI:false,IN_SHELL:false,IN_BROWSER:true,IN_BROWSER_MAIN_THREAD:true,IN_BROWSER_WEB_WORKER:false,IN_NODE_COMMONJS:false,IN_NODE_ESM:false};}"
                .into(),
        ),
    ];

    let mut result = source.to_owned();
    for (needle, replacement) in required_replacements {
        if !result.contains(needle) {
            return Err(PyRunnerError::Init(format!(
                "required Pyodide patch pattern missing: {needle}"
            )));
        }
        result = result.replace(needle, &replacement);
    }

    let table_needle = "var tableBase=metadata.tableSize?wasmTable.length:0;";
    if result.contains(table_needle) {
        result = result.replace(
            table_needle,
            &format!(
                "{table_needle}\nModule.snapshotDebug && console.log('loadWebAssemblyModule', libName, memoryBase, tableBase);"
            ),
        );
    } else {
        tracing::warn!(
            target = "aardvark::assets",
            "optional Pyodide table-base debug patch pattern missing"
        );
    }

    Ok(result)
}

fn write_distribution_manifest(
    output_dir: &Path,
    variant: PyodideDistributionVariant,
) -> Result<()> {
    let lockfile_path = output_dir.join("pyodide-lock.json");
    let lockfile_raw = read_text_file_limited(
        &lockfile_path,
        MAX_PYODIDE_LOCKFILE_BYTES,
        "Pyodide lockfile",
    )?;
    let lockfile_value: Value = serde_json::from_str(&lockfile_raw).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to parse {}: {err}",
            lockfile_path.display()
        ))
    })?;
    let info = lockfile_value
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| PyRunnerError::Init("pyodide-lock.json missing info object".into()))?;

    let mut files = BTreeMap::new();
    collect_distribution_file_hashes(output_dir, output_dir, &mut files)?;

    let mut manifest = PyodideDistributionManifest {
        schema_version: 1,
        aardvark_version: env!("CARGO_PKG_VERSION").to_string(),
        pyodide_version: PYODIDE_VERSION.to_string(),
        adapter_version: PYODIDE_ADAPTER_VERSION.to_string(),
        variant,
        upstream: UpstreamArchives {
            base_url: PYODIDE_RELEASE_BASE_URL.to_string(),
            core: UpstreamArchive {
                name: PYODIDE_CORE_ARCHIVE_NAME.to_string(),
                sha256: PYODIDE_CORE_ARCHIVE_SHA256.to_string(),
            },
            full: UpstreamArchive {
                name: PYODIDE_FULL_ARCHIVE_NAME.to_string(),
                sha256: PYODIDE_FULL_ARCHIVE_SHA256.to_string(),
            },
        },
        python: PythonCompatibility {
            version: info
                .get("python")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            abi: info
                .get("abi_version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            platform: info
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arch: info
                .get("arch")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },
        lockfile: LockfileManifest {
            path: "pyodide-lock.json".to_string(),
            sha256: format!("sha256:{}", sha256_file_hex(&lockfile_path)?),
        },
        package_root: Some(".".to_string()),
        features: scan_distribution_features(output_dir, &lockfile_value)?,
        files,
        compatibility_fingerprint: String::new(),
    };
    manifest.compatibility_fingerprint = compute_compatibility_fingerprint(&manifest);
    let serialized = serde_json::to_vec_pretty(&manifest).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to serialize Pyodide distribution manifest: {err}"
        ))
    })?;
    fs::write(output_dir.join(DISTRIBUTION_MANIFEST), serialized).map_err(|err| {
        PyRunnerError::Init(format!("failed to write distribution manifest: {err}"))
    })?;
    Ok(())
}

fn collect_distribution_file_hashes(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let entries = fs::read_dir(dir)
        .map_err(|err| PyRunnerError::Init(format!("failed to read {}: {err}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            PyRunnerError::Init(format!("failed to read entry in {}: {err}", dir.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| {
            PyRunnerError::Init(format!("failed to stat {}: {err}", path.display()))
        })?;
        if file_type.is_dir() {
            collect_distribution_file_hashes(root, &path, files)?;
        } else if file_type.is_file() {
            if path.file_name().and_then(|name| name.to_str()) == Some(DISTRIBUTION_MANIFEST) {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(rel, format!("sha256:{}", sha256_file_hex(&path)?));
        }
    }
    Ok(())
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() {
        let metadata = output.metadata().map_err(|err| {
            PyRunnerError::Init(format!("failed to stat {}: {err}", output.display()))
        })?;
        if !metadata.is_dir() {
            return Err(PyRunnerError::Init(format!(
                "output path {} exists but is not a directory",
                output.display()
            )));
        }
        if force {
            if path_has_entries(output)? {
                validate_force_removal_target(output)?;
                fs::remove_dir_all(output).map_err(|err| {
                    PyRunnerError::Init(format!("failed to remove {}: {err}", output.display()))
                })?;
            }
        } else if path_has_entries(output)? {
            return Err(PyRunnerError::Init(format!(
                "output directory {} is not empty; re-run with --force to overwrite",
                output.display()
            )));
        }
    }
    fs::create_dir_all(output).map_err(|err| {
        PyRunnerError::Init(format!("failed to create {}: {err}", output.display()))
    })?;
    Ok(())
}

fn path_has_entries(path: &Path) -> Result<bool> {
    let mut entries = path
        .read_dir()
        .map_err(|err| PyRunnerError::Init(format!("failed to read {}: {err}", path.display())))?;
    Ok(entries.next().is_some())
}

fn validate_force_removal_target(output: &Path) -> Result<()> {
    let canonical = output.canonicalize().map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to canonicalize {}: {err}",
            output.display()
        ))
    })?;
    let cwd = env::current_dir()
        .map_err(|err| PyRunnerError::Init(format!("failed to read current directory: {err}")))?
        .canonicalize()
        .map_err(|err| {
            PyRunnerError::Init(format!("failed to canonicalize current directory: {err}"))
        })?;
    if canonical.parent().is_none() || cwd.starts_with(&canonical) {
        return Err(PyRunnerError::Init(format!(
            "refusing to force-remove dangerous output directory {}",
            output.display()
        )));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        if let Ok(home) = home.canonicalize() {
            if canonical == home {
                return Err(PyRunnerError::Init(format!(
                    "refusing to force-remove home directory {}",
                    output.display()
                )));
            }
        }
    }
    if canonical.join(DISTRIBUTION_MANIFEST).is_file()
        || path_is_under_default_stage_root(&canonical, &cwd)
    {
        return Ok(());
    }
    Err(PyRunnerError::Init(format!(
        "refusing to force-remove {}; only Aardvark staged distribution directories may be replaced with --force",
        output.display()
    )))
}

fn path_is_under_default_stage_root(canonical_output: &Path, canonical_cwd: &Path) -> bool {
    let default_root = canonical_cwd
        .join(".aardvark")
        .join("pyodide-distributions");
    canonical_output.starts_with(default_root)
}

fn download_variant_archive(
    variant: PyodideDistributionVariant,
    workspace: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(workspace).map_err(|err| {
        PyRunnerError::Init(format!("failed to create {}: {err}", workspace.display()))
    })?;
    let archive_path = workspace.join(variant.archive_name());
    let timeout = Some(Duration::from_secs(120));
    let agent: Agent = Agent::config_builder()
        .timeout_global(None)
        .timeout_send_request(timeout)
        .timeout_send_body(timeout)
        .timeout_recv_response(timeout)
        .timeout_recv_body(timeout)
        .build()
        .into();
    let url = variant.archive_url();
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|err| PyRunnerError::Init(format!("failed downloading {url}: {err}")))?;
    let reader = response.body_mut().as_reader();
    copy_archive_with_limit(reader, &archive_path, MAX_PYODIDE_ARCHIVE_BYTES, &url)?;
    verify_sha256(&archive_path, variant.expected_sha())?;
    Ok(archive_path)
}

fn copy_archive_with_limit<R: Read>(
    reader: R,
    archive_path: &Path,
    limit: u64,
    context: &str,
) -> Result<u64> {
    let mut limited = reader.take(limit.saturating_add(1));
    let mut file = File::create(archive_path).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to create {}: {err}",
            archive_path.display()
        ))
    })?;
    let written = std::io::copy(&mut limited, &mut file).map_err(|err| {
        PyRunnerError::Init(format!("failed to write {}: {err}", archive_path.display()))
    })?;
    if written > limit {
        let _ = fs::remove_file(archive_path);
        return Err(PyRunnerError::Init(format!(
            "refusing to download {context}: archive exceeded the {limit} byte limit"
        )));
    }
    Ok(written)
}

fn unpack_archive(archive_path: &Path, workspace: &Path) -> Result<()> {
    let file = File::open(archive_path).map_err(|err| {
        PyRunnerError::Init(format!("failed to open {}: {err}", archive_path.display()))
    })?;
    let decoder = BzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(workspace).map_err(|err| {
        PyRunnerError::Init(format!(
            "failed to unpack {} into {}: {err}",
            archive_path.display(),
            workspace.display()
        ))
    })?;
    Ok(())
}

fn find_pyodide_dir(base: &Path) -> Option<PathBuf> {
    let mut stack = vec![base.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path
                .file_name()
                .map(|name| name == "pyodide")
                .unwrap_or(false)
            {
                return Some(path);
            }
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)
        .map_err(|err| PyRunnerError::Init(format!("failed to create {}: {err}", dst.display())))?;
    let entries = fs::read_dir(src)
        .map_err(|err| PyRunnerError::Init(format!("failed to read {}: {err}", src.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            PyRunnerError::Init(format!("failed to read entry in {}: {err}", src.display()))
        })?;
        let file_type = entry.file_type().map_err(|err| {
            PyRunnerError::Init(format!("failed to stat {}: {err}", entry.path().display()))
        })?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).map_err(|err| {
                PyRunnerError::Init(format!(
                    "failed to copy {} -> {}: {err}",
                    src_path.display(),
                    dst_path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file_hex(path)?;
    if actual != expected {
        return Err(PyRunnerError::Init(format!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        )));
    }
    Ok(())
}

fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .map_err(|err| PyRunnerError::Init(format!("failed to open {}: {err}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 131_072];
    loop {
        let read = file.read(&mut buffer).map_err(|err| {
            PyRunnerError::Init(format!("failed to read {}: {err}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().encode_hex::<String>())
}

fn scan_distribution_features(root: &Path, lockfile: &Value) -> Result<DistributionFeatures> {
    let mut summary = DistributionFeatures::default();
    let packages = lockfile
        .get("packages")
        .and_then(Value::as_object)
        .ok_or_else(|| PyRunnerError::Init("pyodide-lock.json missing packages object".into()))?;

    for (canonical, package) in packages {
        let Some(file_name) = package.get("file_name").and_then(Value::as_str) else {
            continue;
        };
        let path = root.join(file_name);
        if !path.exists() {
            continue;
        }
        let mut features = scan_package_features(&path).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to scan package features for {}: {err}",
                path.display()
            ))
        })?;
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

fn canonicalize_package_name(name: &str) -> String {
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

fn scan_package_features(path: &Path) -> Result<PackageFeatures> {
    let extension = path.extension().and_then(|value| value.to_str());
    if matches!(extension, Some("whl" | "zip")) {
        return scan_zip_package_features(path);
    }

    let mut features = PackageFeatures::default();
    let bytes = read_file_limited(path, MAX_FEATURE_SCAN_BYTES, "feature scan target")?;
    scan_binary_feature_bytes(
        path.file_name().and_then(|name| name.to_str()),
        &bytes,
        &mut features,
    )?;
    Ok(features)
}

fn scan_zip_package_features(path: &Path) -> Result<PackageFeatures> {
    let file = File::open(path)
        .map_err(|err| PyRunnerError::Init(format!("failed to open {}: {err}", path.display())))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| {
        PyRunnerError::Init(format!("failed to open zip {}: {err}", path.display()))
    })?;
    let mut features = PackageFeatures::default();

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to read zip entry {index} from {}: {err}",
                path.display()
            ))
        })?;
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
            return Err(PyRunnerError::Init(format!(
                "refusing to scan zip entry {name} from {}: {} bytes exceeds the {} byte feature-scan limit",
                path.display(),
                entry.size(),
                MAX_FEATURE_SCAN_BYTES
            )));
        }
        let mut bytes = Vec::with_capacity(entry.size().min(8 * 1024 * 1024) as usize);
        let mut limited = entry.take(MAX_FEATURE_SCAN_BYTES.saturating_add(1));
        limited.read_to_end(&mut bytes).map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to read zip entry {name} from {}: {err}",
                path.display()
            ))
        })?;
        if bytes.len() as u64 > MAX_FEATURE_SCAN_BYTES {
            return Err(PyRunnerError::Init(format!(
                "zip entry {name} from {} exceeded the {} byte feature-scan limit while reading",
                path.display(),
                MAX_FEATURE_SCAN_BYTES
            )));
        }
        scan_binary_feature_bytes(Some(name.as_str()), &bytes, &mut features)?;
    }

    Ok(features)
}

fn is_feature_scan_candidate(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with(".so")
        || lowered.ends_with(".wasm")
        || lowered.ends_with(".data")
        || lowered.contains("openblas")
}

fn scan_binary_feature_bytes(
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

fn wasm_module_uses_simd(bytes: &[u8]) -> Result<bool> {
    let mut visitor = SimdDetector;
    for payload in WasmParser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|err| {
            PyRunnerError::Init(format!(
                "failed to parse wasm module for feature scan: {err}"
            ))
        })?;
        if let WasmPayload::CodeSectionEntry(body) = payload {
            let reader = body.get_operators_reader().map_err(|err| {
                PyRunnerError::Init(format!(
                    "failed to read wasm operators for feature scan: {err}"
                ))
            })?;
            for op in reader {
                let op = op.map_err(|err| {
                    PyRunnerError::Init(format!(
                        "failed to read wasm operator for feature scan: {err}"
                    ))
                })?;
                if visitor.visit_operator(&op) {
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn copy_archive_with_limit_rejects_oversized_download() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("pyodide.tar.bz2");

        let err = copy_archive_with_limit(
            Cursor::new(vec![0_u8; 4]),
            &archive_path,
            3,
            "https://example.invalid/pyodide.tar.bz2",
        )
        .expect_err("download over limit should fail");

        assert!(
            err.to_string().contains("exceeded the 3 byte limit"),
            "unexpected error: {err}"
        );
        assert!(
            !archive_path.exists(),
            "oversized partial archive should be removed"
        );
    }

    #[test]
    fn binary_feature_scan_detects_openblas_without_wasm() {
        let mut features = PackageFeatures::default();
        scan_binary_feature_bytes(Some("libopenblas.so"), b"native payload", &mut features)
            .unwrap();
        assert!(features.openblas);
        assert!(!features.wasm_simd);
        assert_eq!(features.wasm_modules, 0);
    }

    #[test]
    fn force_output_guard_rejects_unmarked_custom_directory() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("custom");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("not-a-distribution.txt"), b"data").unwrap();

        assert!(validate_force_removal_target(&output).is_err());
    }

    #[test]
    fn force_output_guard_allows_existing_distribution_directory() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("existing-dist");
        fs::create_dir(&output).unwrap();
        fs::write(output.join(DISTRIBUTION_MANIFEST), b"{}").unwrap();

        validate_force_removal_target(&output).unwrap();
    }

    #[test]
    fn force_output_guard_rejects_current_directory() {
        let cwd = env::current_dir().unwrap();

        assert!(validate_force_removal_target(&cwd).is_err());
    }

    #[test]
    fn wasm_simd_detector_requires_parsed_simd_operator() {
        let mut scalar_features = PackageFeatures::default();
        scan_binary_feature_bytes(
            Some("scalar.wasm"),
            &minimal_scalar_wasm(),
            &mut scalar_features,
        )
        .unwrap();
        assert_eq!(scalar_features.wasm_modules, 1);
        assert!(!scalar_features.wasm_simd);

        let mut simd_features = PackageFeatures::default();
        scan_binary_feature_bytes(Some("simd.wasm"), &minimal_simd_wasm(), &mut simd_features)
            .unwrap();
        assert_eq!(simd_features.wasm_modules, 1);
        assert!(simd_features.wasm_simd);
    }

    fn minimal_scalar_wasm() -> Vec<u8> {
        use wasm_encoder::{
            CodeSection, Function, FunctionSection, Instruction, Module, TypeSection, ValType,
        };

        let mut types = TypeSection::new();
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
        let mut functions = FunctionSection::new();
        functions.function(0);
        let mut func = Function::new([]);
        func.instruction(&Instruction::I32Const(1))
            .instruction(&Instruction::Drop)
            .instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&func);
        let mut module = Module::new();
        module.section(&types).section(&functions).section(&code);
        module.finish()
    }

    fn minimal_simd_wasm() -> Vec<u8> {
        use wasm_encoder::{
            CodeSection, Function, FunctionSection, Instruction, Module, TypeSection, ValType,
        };

        let mut types = TypeSection::new();
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
        let mut functions = FunctionSection::new();
        functions.function(0);
        let mut func = Function::new([]);
        func.instruction(&Instruction::V128Const(0))
            .instruction(&Instruction::Drop)
            .instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&func);
        let mut module = Module::new();
        module.section(&types).section(&functions).section(&code);
        module.finish()
    }
}
