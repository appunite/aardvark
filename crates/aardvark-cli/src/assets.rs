use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aardvark_core::pyodide::{
    PYODIDE_ADAPTER_VERSION, PYODIDE_CORE_ARCHIVE_NAME, PYODIDE_CORE_ARCHIVE_SHA256,
    PYODIDE_FULL_ARCHIVE_NAME, PYODIDE_FULL_ARCHIVE_SHA256, PYODIDE_RELEASE_BASE_URL,
    PYODIDE_VERSION,
};
use aardvark_core::pyodide_distribution::{
    compute_compatibility_fingerprint, LockfileManifest, PyodideDistribution,
    PyodideDistributionManifest, PyodideDistributionVariant, PythonCompatibility, UpstreamArchive,
    UpstreamArchives, DISTRIBUTION_MANIFEST,
};
use anyhow::{bail, Context, Result};
use bzip2::read::BzDecoder;
use hex::ToHex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tar::Archive;
use tracing::{info, warn};
use ureq::Agent;

use crate::args::{AssetsCommand, AssetsStageArgs, AssetsVerifyArgs, StageVariant};
use crate::read_limited::read_text_file_limited;

mod feature_scan;

#[cfg(test)]
pub(crate) use feature_scan::scan_binary_feature_bytes;
use feature_scan::{
    distribution_features_for_reporting, print_distribution_features, scan_distribution_features,
};

const MAX_PYODIDE_LOCKFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PYODIDE_PATCH_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PYODIDE_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

impl StageVariant {
    fn archive_name(self) -> &'static str {
        match self {
            StageVariant::Core => PYODIDE_CORE_ARCHIVE_NAME,
            StageVariant::Full => PYODIDE_FULL_ARCHIVE_NAME,
        }
    }

    fn expected_sha(self) -> &'static str {
        match self {
            StageVariant::Core => PYODIDE_CORE_ARCHIVE_SHA256,
            StageVariant::Full => PYODIDE_FULL_ARCHIVE_SHA256,
        }
    }

    fn subdir(self) -> &'static str {
        match self {
            StageVariant::Core => "core",
            StageVariant::Full => "full",
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

impl fmt::Display for StageVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StageVariant::Core => "core",
            StageVariant::Full => "full",
        })
    }
}

pub(crate) fn handle_assets_command(command: AssetsCommand) -> Result<()> {
    match command {
        AssetsCommand::Stage(args) => stage_assets(args),
        AssetsCommand::Verify(args) => verify_assets(args),
    }
}

fn stage_assets(args: AssetsStageArgs) -> Result<()> {
    let AssetsStageArgs {
        variant,
        output,
        archive,
        force,
    } = args;

    let output_dir = output.unwrap_or_else(|| default_stage_output_dir(variant));
    let workspace = tempfile::tempdir().context("create staging workspace")?;
    let user_supplied = archive.is_some();
    let archive_path = match archive {
        Some(path) => path,
        None => download_variant_archive(variant, workspace.path())?,
    };

    if user_supplied {
        verify_sha256(&archive_path, variant.expected_sha()).with_context(|| {
            format!(
                "supplied Pyodide archive failed checksum for {}",
                archive_path.display()
            )
        })?;
    }

    info!(
        target = "aardvark::assets",
        variant = %variant,
        archive = %archive_path.display(),
        "unpacking Pyodide archive"
    );

    unpack_archive(&archive_path, workspace.path())?;
    let pyodide_root = find_pyodide_dir(workspace.path())
        .context("expected 'pyodide' directory inside archive")?;
    let source_dir = pyodide_variant_source_dir(&pyodide_root, variant)?;

    prepare_output_dir(&output_dir, force)?;
    copy_dir_recursive(&source_dir, &output_dir)?;
    copy_adapter_assets(&output_dir)?;
    generate_patched_pyodide(&output_dir)?;
    write_distribution_manifest(&output_dir, variant)?;
    let verified = PyodideDistribution::external(&output_dir)
        .with_context(|| format!("verify staged distribution {}", output_dir.display()))?;

    info!(
        target = "aardvark::assets",
        variant = %variant,
        output = %output_dir.display(),
        fingerprint = verified.compatibility_fingerprint(),
        "staged Aardvark Pyodide distribution"
    );
    Ok(())
}

fn default_stage_output_dir(variant: StageVariant) -> PathBuf {
    PathBuf::from(".aardvark/pyodide-distributions").join(format!(
        "aardvark-{}-pyodide-v{}-{}",
        env!("CARGO_PKG_VERSION"),
        PYODIDE_VERSION,
        variant
    ))
}

fn verify_assets(args: AssetsVerifyArgs) -> Result<()> {
    let dist = PyodideDistribution::external(&args.path)
        .with_context(|| format!("verify distribution {}", args.path.display()))?;
    let features = distribution_features_for_reporting(&args.path, dist.manifest())?;
    println!(
        "verified {} ({}, fingerprint {})",
        args.path.display(),
        dist.manifest().variant.as_str(),
        dist.compatibility_fingerprint()
    );
    print_distribution_features(&features);
    Ok(())
}

fn pyodide_variant_source_dir(pyodide_root: &Path, variant: StageVariant) -> Result<PathBuf> {
    let nested = pyodide_root
        .join("pyodide")
        .join(format!("v{PYODIDE_VERSION}"))
        .join(variant.subdir());
    if nested.exists() {
        return Ok(nested);
    }
    if pyodide_root.join("pyodide-lock.json").exists() {
        return Ok(pyodide_root.to_path_buf());
    }
    bail!(
        "archive missing Pyodide distribution files under {}",
        pyodide_root.display()
    )
}

fn copy_adapter_assets(output_dir: &Path) -> Result<()> {
    let embedded =
        PyodideDistribution::embedded().context("load embedded Aardvark Pyodide adapter assets")?;
    for file in [
        "pyodide_builtin_wrappers.js",
        "pyodide_bootstrap.js",
        "pyodide_emscripten_setup.js",
        "pyodide_packages.js",
    ] {
        let dst = output_dir.join(file);
        let text = embedded
            .read_text_asset(file)
            .with_context(|| format!("read embedded adapter asset {file}"))?;
        fs::write(&dst, text.as_ref()).with_context(|| format!("write {}", dst.display()))?;
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
    fs::write(&target_path, patched).with_context(|| format!("write {}", target_path.display()))?;
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
            bail!("required Pyodide patch pattern missing: {needle}");
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
        warn!(
            target = "aardvark::assets",
            "optional Pyodide table-base debug patch pattern missing"
        );
    }

    Ok(result)
}

fn write_distribution_manifest(output_dir: &Path, variant: StageVariant) -> Result<()> {
    let lockfile_path = output_dir.join("pyodide-lock.json");
    let lockfile_raw = read_text_file_limited(
        &lockfile_path,
        MAX_PYODIDE_LOCKFILE_BYTES,
        "Pyodide lockfile",
    )?;
    let lockfile_value: Value = serde_json::from_str(&lockfile_raw)
        .with_context(|| format!("parse {}", lockfile_path.display()))?;
    let info = lockfile_value
        .get("info")
        .and_then(Value::as_object)
        .context("pyodide-lock.json missing info object")?;

    let mut files = BTreeMap::new();
    collect_distribution_file_hashes(output_dir, output_dir, &mut files)?;

    let mut manifest = PyodideDistributionManifest {
        schema_version: 1,
        aardvark_version: env!("CARGO_PKG_VERSION").to_string(),
        pyodide_version: PYODIDE_VERSION.to_string(),
        adapter_version: PYODIDE_ADAPTER_VERSION.to_string(),
        variant: match variant {
            StageVariant::Core => PyodideDistributionVariant::Core,
            StageVariant::Full => PyodideDistributionVariant::Full,
        },
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
    let serialized = serde_json::to_vec_pretty(&manifest)?;
    fs::write(output_dir.join(DISTRIBUTION_MANIFEST), serialized)
        .context("write distribution manifest")?;
    Ok(())
}

fn collect_distribution_file_hashes(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
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

fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 131_072];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().encode_hex::<String>())
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() {
        let metadata = output
            .metadata()
            .with_context(|| format!("failed to stat {}", output.display()))?;
        if !metadata.is_dir() {
            bail!(
                "output path {} exists but is not a directory",
                output.display()
            );
        }
        if force {
            if path_has_entries(output)? {
                validate_force_removal_target(output)?;
                fs::remove_dir_all(output)
                    .with_context(|| format!("failed to remove {}", output.display()))?;
            }
        } else if path_has_entries(output)? {
            bail!(
                "output directory {} is not empty; re-run with --force to overwrite",
                output.display()
            );
        }
    }
    fs::create_dir_all(output).with_context(|| format!("failed to create {}", output.display()))?;
    Ok(())
}

fn path_has_entries(path: &Path) -> Result<bool> {
    Ok(path
        .read_dir()
        .with_context(|| format!("failed to read {}", path.display()))?
        .next()
        .is_some())
}

pub(crate) fn validate_force_removal_target(output: &Path) -> Result<()> {
    let canonical = output
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", output.display()))?;
    let cwd = env::current_dir()
        .context("failed to read current directory")?
        .canonicalize()
        .context("failed to canonicalize current directory")?;
    if canonical.parent().is_none() || cwd.starts_with(&canonical) {
        bail!(
            "refusing to force-remove dangerous output directory {}",
            output.display()
        );
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        if let Ok(home) = home.canonicalize() {
            if canonical == home {
                bail!(
                    "refusing to force-remove home directory {}",
                    output.display()
                );
            }
        }
    }
    if canonical.join(DISTRIBUTION_MANIFEST).is_file()
        || path_is_under_default_stage_root(&canonical, &cwd)
    {
        return Ok(());
    }
    bail!(
        "refusing to force-remove {}; only Aardvark staged distribution directories may be replaced with --force",
        output.display()
    )
}

fn path_is_under_default_stage_root(canonical_output: &Path, canonical_cwd: &Path) -> bool {
    let default_root = canonical_cwd
        .join(".aardvark")
        .join("pyodide-distributions");
    canonical_output.starts_with(default_root)
}

fn download_variant_archive(variant: StageVariant, workspace: &Path) -> Result<PathBuf> {
    fs::create_dir_all(workspace)
        .with_context(|| format!("failed to create {}", workspace.display()))?;
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
        .with_context(|| format!("downloading {url}"))?;
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
    let mut file =
        File::create(archive_path).with_context(|| format!("create {}", archive_path.display()))?;
    let written = std::io::copy(&mut limited, &mut file)
        .with_context(|| format!("write {}", archive_path.display()))?;
    if written > limit {
        let _ = fs::remove_file(archive_path);
        bail!(
            "refusing to download {context}: archive exceeded the {} byte limit",
            limit
        );
    }
    Ok(written)
}

fn unpack_archive(archive_path: &Path, workspace: &Path) -> Result<()> {
    let file =
        File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let decoder = BzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(workspace).with_context(|| {
        format!(
            "unpack {} into {}",
            archive_path.display(),
            workspace.display()
        )
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
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} -> {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let actual = digest.encode_hex::<String>();
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
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
}
