use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use bzip2::read::BzDecoder;
use hex::ToHex;
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::Archive;
use ureq::Agent;

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/pyodide_manifest.rs"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PyodideVariant {
    Core,
}

#[derive(Clone, Copy, Debug)]
struct PyodideArchiveSpec {
    archive_name: &'static str,
    sha256: &'static str,
    variant: PyodideVariant,
}

impl PyodideArchiveSpec {
    fn active() -> Self {
        Self {
            archive_name: PYODIDE_CORE_ARCHIVE_NAME,
            sha256: PYODIDE_CORE_ARCHIVE_SHA256,
            variant: PyodideVariant::Core,
        }
    }

    fn download_url(&self) -> String {
        format!(
            "{base}/{version}/{name}",
            base = PYODIDE_RELEASE_BASE_URL,
            version = PYODIDE_VERSION,
            name = self.archive_name
        )
    }
}

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/js/pyodide_builtin_wrappers.js");
    println!("cargo:rerun-if-changed=src/js/pyodide_bootstrap.js");
    println!("cargo:rerun-if-changed=src/js/pyodide_emscripten_setup.js");
    println!("cargo:rerun-if-changed=src/js/pyodide_packages.js");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    println!("cargo:rerun-if-env-changed=AARDVARK_PYODIDE_ARCHIVE");
    println!("cargo:rerun-if-env-changed=AARDVARK_PYODIDE_DIR");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let pyodide_out_dir = out_dir.join("pyodide");
    if pyodide_out_dir.exists() {
        fs::remove_dir_all(&pyodide_out_dir)
            .with_context(|| format!("remove existing {}", pyodide_out_dir.display()))?;
    }
    fs::create_dir_all(&pyodide_out_dir)
        .with_context(|| format!("create {}", pyodide_out_dir.display()))?;

    let archive_spec = PyodideArchiveSpec::active();
    let docs_rs_build = env::var_os("DOCS_RS").is_some();

    let overwrite_sources = env::var_os("AARDVARK_PYODIDE_DIR");
    if docs_rs_build {
        println!("cargo:warning=DOCS_RS detected; generating placeholder Pyodide assets");
        generate_docs_placeholders(&pyodide_out_dir)?;
    } else if let Some(dir) = overwrite_sources {
        let dir = PathBuf::from(dir);
        copy_dir_recursive(&dir, &pyodide_out_dir)
            .with_context(|| format!("copying Pyodide assets from {}", dir.to_string_lossy()))?;
    } else {
        let archive_path = match env::var_os("AARDVARK_PYODIDE_ARCHIVE") {
            Some(path) => PathBuf::from(path),
            None => download_pyodide_archive(&archive_spec)?,
        };
        unpack_archive(&archive_path, &pyodide_out_dir)?;
    }

    copy_builtin_wrappers(&pyodide_out_dir)?;
    copy_bootstrap_script(&pyodide_out_dir)?;
    copy_emscripten_setup(&pyodide_out_dir)?;
    copy_packages_script(&pyodide_out_dir)?;
    generate_patched_pyodide(&pyodide_out_dir)?;
    generate_distribution_manifest(&pyodide_out_dir, archive_spec.variant)?;

    println!("cargo:rustc-env=AARDVARK_PYODIDE_VERSION={PYODIDE_VERSION}");
    Ok(())
}

fn download_pyodide_archive(spec: &PyodideArchiveSpec) -> Result<PathBuf> {
    let tmp_dir = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .expect("OUT_DIR not set")
        .join("pyodide-download");
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join("pyodide.tar.bz2");

    let agent: Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(120))
        .build();
    let url = spec.download_url();
    let mut response = agent
        .get(&url)
        .call()
        .with_context(|| format!("downloading {}", url))?
        .into_reader();
    let mut file = fs::File::create(&archive_path)?;
    io::copy(&mut response, &mut file)?;

    verify_sha256(&archive_path, spec.sha256)?;
    Ok(archive_path)
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
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
    let digest_hex = digest.encode_hex::<String>();
    if digest_hex != expected {
        anyhow::bail!(
            "Pyodide archive checksum mismatch: expected {}, got {}",
            expected,
            digest_hex
        );
    }
    Ok(())
}

fn unpack_archive(archive_path: &Path, out_dir: &Path) -> Result<()> {
    let file =
        fs::File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let decoder = BzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let tmp_root = out_dir.join("tmp");
    if tmp_root.exists() {
        fs::remove_dir_all(&tmp_root)?;
    }
    fs::create_dir_all(&tmp_root)?;

    archive.unpack(&tmp_root)?;

    let pyodide_dir =
        find_pyodide_dir(&tmp_root).context("could not find `pyodide` directory inside archive")?;
    copy_dir_recursive(&pyodide_dir, out_dir)?;
    fs::remove_dir_all(&tmp_root)?;
    Ok(())
}

fn find_pyodide_dir(base: &Path) -> Option<PathBuf> {
    let mut stack = vec![base.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path.file_name().map(|n| n == "pyodide").unwrap_or(false) {
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

fn generate_docs_placeholders(out_dir: &Path) -> Result<()> {
    const ASM_PLACEHOLDER: &str = r#"// docs.rs placeholder - Pyodide assets are unavailable in documentation builds.
var Module = { API: {} };
var _createPyodideModule = function () { throw new Error("Pyodide runtime is unavailable in docs.rs builds"); };
globalThis._createPyodideModule = _createPyodideModule;
const API=Module.API;
reportUndefinedSymbols();
crypto.getRandomValues(Module, []);
new WebAssembly.Module({});
WebAssembly.instantiate({});
Date.now();
eval(func);
eval(data);
eval(UTF8ToString(ptr));
"#;

    const MJS_PLACEHOLDER: &str = r#"export async function loadPyodide() {
    throw new Error("Pyodide runtime is unavailable in docs.rs builds");
}
"#;

    const JS_PLACEHOLDER: &str = r#"export function loadPyodide() {
    throw new Error("Pyodide runtime is unavailable in docs.rs builds");
}
"#;

    fs::write(out_dir.join("pyodide.asm.js"), ASM_PLACEHOLDER)
        .context("write docs placeholder pyodide.asm.js")?;
    fs::write(out_dir.join("pyodide.mjs"), MJS_PLACEHOLDER)
        .context("write docs placeholder pyodide.mjs")?;
    fs::write(out_dir.join("pyodide.js"), JS_PLACEHOLDER)
        .context("write docs placeholder pyodide.js")?;
    fs::write(
        out_dir.join("pyodide-lock.json"),
        format!(
            r#"{{"info":{{"abi_version":"","arch":"","platform":"","python":"","version":"{PYODIDE_VERSION}"}},"packages":{{}}}}"#
        ),
    )
    .context("write docs placeholder pyodide-lock.json")?;
    fs::write(out_dir.join("pyodide.asm.wasm"), &[] as &[u8])
        .context("write docs placeholder pyodide.asm.wasm")?;
    fs::write(out_dir.join("python_stdlib.zip"), &[] as &[u8])
        .context("write docs placeholder python_stdlib.zip")?;
    Ok(())
}

fn copy_builtin_wrappers(out_dir: &Path) -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src = manifest_dir.join("src/js/pyodide_builtin_wrappers.js");
    let dst = out_dir.join("pyodide_builtin_wrappers.js");
    fs::copy(&src, &dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_bootstrap_script(out_dir: &Path) -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src = manifest_dir.join("src/js/pyodide_bootstrap.js");
    let dst = out_dir.join("pyodide_bootstrap.js");
    fs::copy(&src, &dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_emscripten_setup(out_dir: &Path) -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src = manifest_dir.join("src/js/pyodide_emscripten_setup.js");
    let dst = out_dir.join("pyodide_emscripten_setup.js");
    fs::copy(&src, &dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn copy_packages_script(out_dir: &Path) -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src = manifest_dir.join("src/js/pyodide_packages.js");
    let dst = out_dir.join("pyodide_packages.js");
    fs::copy(&src, &dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn generate_patched_pyodide(out_dir: &Path) -> Result<()> {
    let original_path = out_dir.join("pyodide.asm.js");
    let target_path = out_dir.join("pyodide.asm.patched.js");
    let source = fs::read_to_string(&original_path)
        .with_context(|| format!("read {}", original_path.display()))?;
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

    let required_replacements: [(&str, String); 11] = [
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
        (
            "crypto.getRandomValues(",
            "getRandomValues(Module, ".into(),
        ),
        (
            "eval(func)",
            r#"(() => {throw new Error('Internal Emscripten code tried to eval, this should not happen, please file a bug report with your requirements.txt file\'s contents')})()"#
                .into(),
        ),
        (
            "eval(data)",
            r#"(() => {throw new Error('Internal Emscripten code tried to eval, this should not happen, please file a bug report with your requirements.txt file\'s contents')})()"#
                .into(),
        ),
        (
            "eval(UTF8ToString(ptr))",
            r#"(() => {throw new Error('Internal Emscripten code tried to eval, this should not happen, please file a bug report with your requirements.txt file\'s contents')})()"#
                .into(),
        ),
        (
            "const API=Module.API;",
            "const API=Module.API||(Module.API={});if(!API.runtimeEnv){API.runtimeEnv={IN_BUN:false,IN_DENO:false,IN_NODE:false,IN_SAFARI:false,IN_SHELL:false,IN_BROWSER:true,IN_BROWSER_MAIN_THREAD:true,IN_BROWSER_WEB_WORKER:false,IN_NODE_COMMONJS:false,IN_NODE_ESM:false};}"
                .into(),
        ),
    ];

    let mut result = source.to_owned();
    for (needle, replacement) in required_replacements {
        if result.contains(needle) {
            result = result.replace(needle, &replacement);
        } else {
            anyhow::bail!("required Pyodide patch pattern missing: {needle}");
        }
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
        println!("cargo:warning=pyodide patch skipped missing tableBase pattern");
    }

    Ok(result)
}

fn generate_distribution_manifest(out_dir: &Path, variant: PyodideVariant) -> Result<()> {
    let lock_path = out_dir.join("pyodide-lock.json");
    let lock_raw =
        fs::read_to_string(&lock_path).with_context(|| format!("read {}", lock_path.display()))?;
    let lock_json: serde_json::Value =
        serde_json::from_str(&lock_raw).context("parse pyodide-lock.json")?;
    let info = lock_json
        .get("info")
        .and_then(|value| value.as_object())
        .context("pyodide-lock.json missing info object")?;
    let python = info
        .get("python")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let abi = info
        .get("abi_version")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let platform = info
        .get("platform")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let arch = info
        .get("arch")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let lock_sha = sha256_file(&lock_path)?;

    let mut files = serde_json::Map::new();
    for rel in required_distribution_files() {
        let path = out_dir.join(rel);
        if !path.exists() {
            anyhow::bail!("distribution required file missing: {}", path.display());
        }
        files.insert(
            rel.to_string(),
            json!(format!("sha256:{}", sha256_file(&path)?)),
        );
    }

    let mut manifest = json!({
        "schemaVersion": 1,
        "aardvarkVersion": env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()),
        "pyodideVersion": PYODIDE_VERSION,
        "adapterVersion": PYODIDE_ADAPTER_VERSION,
        "variant": match variant {
            PyodideVariant::Core => "core",
        },
        "upstream": {
            "baseUrl": PYODIDE_RELEASE_BASE_URL,
            "core": {
                "name": PYODIDE_CORE_ARCHIVE_NAME,
                "sha256": PYODIDE_CORE_ARCHIVE_SHA256
            },
            "full": {
                "name": PYODIDE_FULL_ARCHIVE_NAME,
                "sha256": PYODIDE_FULL_ARCHIVE_SHA256
            }
        },
        "python": {
            "version": python,
            "abi": abi,
            "platform": platform,
            "arch": arch
        },
        "lockfile": {
            "path": "pyodide-lock.json",
            "sha256": format!("sha256:{lock_sha}")
        },
        "packageRoot": serde_json::Value::Null,
        "files": files,
        "compatibilityFingerprint": ""
    });
    let fingerprint = compute_distribution_fingerprint(&manifest)?;
    manifest["compatibilityFingerprint"] = json!(fingerprint);
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(out_dir.join("aardvark-pyodide-dist.json"), bytes)
        .context("write aardvark-pyodide-dist.json")?;
    Ok(())
}

fn required_distribution_files() -> &'static [&'static str] {
    &[
        "pyodide.asm.wasm",
        "pyodide.asm.js",
        "pyodide.asm.patched.js",
        "pyodide_builtin_wrappers.js",
        "pyodide_bootstrap.js",
        "pyodide_emscripten_setup.js",
        "pyodide_packages.js",
        "pyodide.mjs",
        "pyodide.js",
        "python_stdlib.zip",
        "pyodide-lock.json",
    ]
}

fn compute_distribution_fingerprint(manifest: &serde_json::Value) -> Result<String> {
    let pyodide = json_str(manifest, "pyodideVersion")?;
    let adapter = json_str(manifest, "adapterVersion")?;
    let variant = json_str(manifest, "variant")?;
    let python = manifest
        .get("python")
        .and_then(|value| value.as_object())
        .context("manifest missing python object")?;
    let python_version = object_str(python, "version")?;
    let abi = object_str(python, "abi")?;
    let platform = object_str(python, "platform")?;
    let arch = object_str(python, "arch")?;
    let lockfile = manifest
        .get("lockfile")
        .and_then(|value| value.as_object())
        .context("manifest missing lockfile object")?;
    let lock_sha = object_str(lockfile, "sha256")?;

    let mut hasher = Sha256::new();
    hasher.update(b"aardvark-pyodide-distribution-v1\n");
    hasher.update(format!("pyodide={pyodide}\n").as_bytes());
    hasher.update(format!("adapter={adapter}\n").as_bytes());
    hasher.update(format!("variant={variant}\n").as_bytes());
    hasher.update(format!("python={python_version}\n").as_bytes());
    hasher.update(format!("abi={abi}\n").as_bytes());
    hasher.update(format!("platform={platform}\n").as_bytes());
    hasher.update(format!("arch={arch}\n").as_bytes());
    hasher.update(format!("lockfile={lock_sha}\n").as_bytes());
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn json_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .with_context(|| format!("manifest missing string field {key}"))
}

fn object_str<'a>(
    value: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .with_context(|| format!("manifest missing string field {key}"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().encode_hex::<String>())
}
