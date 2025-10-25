use assert_cmd::prelude::*;
use serde_json::Value;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;
use zip::write::FileOptions;
use zip::ZipWriter;

const ENTRYPOINT: &str = "main:main";

fn pyodide_dir() -> PathBuf {
    if let Some(dir) = env::var_os("AARDVARK_PYODIDE_PACKAGE_DIR") {
        PathBuf::from(dir)
    } else {
        workspace_root().join(".aardvark/pyodide/0.29.0")
    }
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
}

fn create_test_bundle(dir: &Path) -> PathBuf {
    let bundle_path = dir.join("bundle.zip");
    let mut file = File::create(&bundle_path).expect("create bundle");
    {
        let mut zip = ZipWriter::new(&mut file);
        let options = FileOptions::default();
        zip.start_file("main.py", options)
            .expect("start main.py entry");
        zip.write_all(b"def main():\n    return {'message': 'ok'}\n")
            .expect("write main.py");
        zip.finish().expect("finish zip");
    }
    bundle_path
}

fn cli(pyodide_dir: &PathBuf) -> Command {
    let mut cmd = Command::cargo_bin("aardvark-cli").expect("build aardvark-cli");
    cmd.env("AARDVARK_PYODIDE_PACKAGE_DIR", pyodide_dir);
    cmd.current_dir(workspace_root());
    cmd
}

#[test]
fn snapshot_restore_uses_catalog() {
    let pyodide_dir = pyodide_dir();
    assert!(
        pyodide_dir.exists(),
        "expected Pyodide cache at {:?}; set AARDVARK_PYODIDE_PACKAGE_DIR or stage .aardvark/pyodide/0.29.0",
        pyodide_dir
    );

    let integration_dir = tempdir().expect("create temp dir");
    let catalog_path = integration_dir.path().join("catalog");
    let snapshot_path = integration_dir.path().join("snapshot.bin");
    let bundle = create_test_bundle(integration_dir.path());

    let mut capture = cli(&pyodide_dir);
    capture
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn")
        .arg("--write-snapshot")
        .arg(&snapshot_path);
    capture.assert().success();

    let overlay_path = {
        let mut os = snapshot_path.as_os_str().to_os_string();
        os.push(".overlay.json");
        PathBuf::from(os)
    };
    let overlay_bytes = fs::read(&overlay_path).expect("read snapshot overlay metadata");
    let overlay: Value = serde_json::from_slice(&overlay_bytes).expect("parse snapshot overlay");
    let overlay_packages = overlay
        .get("packages")
        .and_then(Value::as_array)
        .expect("overlay packages array");
    assert!(
        overlay_packages
            .iter()
            .any(|pkg| pkg.get("canonical").and_then(Value::as_str) == Some("numpy")),
        "overlay metadata missing numpy package"
    );

    let index_path = catalog_path.join("index.json");
    let index_bytes = fs::read(&index_path).expect("read catalog index");
    let index: Value = serde_json::from_slice(&index_bytes).expect("parse catalog index");
    assert!(index["packages"].is_object(), "index missing package map");

    let mut restore = cli(&pyodide_dir);
    restore
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--snapshot")
        .arg(&snapshot_path);
    restore.assert().success();

    let mut warm = cli(&pyodide_dir);
    warm.env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn");
    warm.assert().success();

    assert!(catalog_path.read_dir().unwrap().any(|entry| {
        entry
            .ok()
            .and_then(|e| e.file_name().into_string().ok())
            .map(|name| name.starts_with("sha256-"))
            .unwrap_or(false)
    }));
}

#[test]
fn catalog_eviction_budget_trims_cache() {
    let pyodide_dir = pyodide_dir();
    assert!(
        pyodide_dir.exists(),
        "expected Pyodide cache at {:?}; set AARDVARK_PYODIDE_PACKAGE_DIR or stage .aardvark/pyodide/0.29.0",
        pyodide_dir
    );

    let integration_dir = tempdir().expect("create temp dir");
    let catalog_path = integration_dir.path().join("catalog");
    let snapshot_path = integration_dir.path().join("snapshot.bin");
    let bundle = create_test_bundle(integration_dir.path());

    let mut capture = cli(&pyodide_dir);
    capture
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn")
        .arg("--write-snapshot")
        .arg(&snapshot_path);
    capture.assert().success();

    let list_tar = |root: &Path| -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().into_string().ok()?;
                if name.ends_with(".tar") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect()
    };

    let initial_tars = list_tar(&catalog_path);
    assert!(
        initial_tars.len() > 1,
        "expected multiple overlay blobs, found {:?}",
        initial_tars
    );

    let mut hydrate = cli(&pyodide_dir);
    hydrate
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .env("AARDVARK_OVERLAY_CACHE_MAX_BYTES", "1")
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn")
        .assert()
        .success();

    let remaining_tars = list_tar(&catalog_path);
    assert!(
        remaining_tars.len() < initial_tars.len(),
        "expected eviction to trim catalog entries, initial {:?} remaining {:?}",
        initial_tars,
        remaining_tars
    );
}

#[test]
fn catalog_prunes_missing_blobs_during_hydrate() {
    let pyodide_dir = pyodide_dir();
    assert!(
        pyodide_dir.exists(),
        "expected Pyodide cache at {:?}",
        pyodide_dir
    );

    let integration_dir = tempdir().expect("create temp dir");
    let catalog_path = integration_dir.path().join("catalog");
    let snapshot_path = integration_dir.path().join("snapshot.bin");
    let bundle = create_test_bundle(integration_dir.path());

    let mut capture = cli(&pyodide_dir);
    capture
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn")
        .arg("--write-snapshot")
        .arg(&snapshot_path);
    capture.assert().success();

    let index_path = catalog_path.join("index.json");
    let index_bytes = fs::read(&index_path).expect("read catalog index");
    let index: Value = serde_json::from_slice(&index_bytes).expect("parse catalog index");
    let packages = index
        .get("packages")
        .and_then(Value::as_object)
        .expect("index packages map");
    let (canonical, entry) = packages.iter().next().expect("packages populated");
    let blob_name = entry
        .get("blob")
        .and_then(Value::as_str)
        .or_else(|| entry.get("digest").and_then(Value::as_str))
        .expect("blob name");
    fs::remove_file(catalog_path.join(blob_name)).expect("remove catalog blob");

    let mut hydrate = cli(&pyodide_dir);
    hydrate
        .env("AARDVARK_OVERLAY_CACHE_DIR", &catalog_path)
        .arg("--bundle")
        .arg(&bundle)
        .arg("--entrypoint")
        .arg(ENTRYPOINT)
        .arg("--package")
        .arg("numpy")
        .arg("--package")
        .arg("pandas")
        .arg("--package")
        .arg("scikit-learn")
        .assert()
        .success();

    let updated_bytes = fs::read(&index_path).expect("read updated index");
    let updated: Value = serde_json::from_slice(&updated_bytes).expect("parse updated index");
    let updated_packages = updated
        .get("packages")
        .and_then(Value::as_object)
        .expect("updated packages map");
    assert!(
        !updated_packages.contains_key(canonical),
        "expected missing blob entry for '{}' to be pruned",
        canonical
    );
}
