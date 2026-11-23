//! Warm snapshots that pre-import packages with native extensions (numpy,
//! pandas). The `.so` side modules live outside the wasm heap image, so a
//! restore has to re-instantiate them at their original memory and table
//! bases (HAV-3682).

use aardvark_core::outcome::ResultPayload;
use aardvark_core::{Bundle, ExecutionOutcome, PyRuntime, PyRuntimeConfig, WarmState};
use std::env;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const PANDAS_MANIFEST: &str = r#"{
    "schemaVersion": "1.0",
    "entrypoint": "main:handler",
    "packages": ["numpy", "pandas"],
    "runtime": {
        "language": "python",
        "pyodide": {"preloadImports": ["numpy", "pandas"]}
    }
}"#;

const PANDAS_HANDLER: &str = r#"
import json
import sys

# main.py is mounted after the restore, so this shows whether numpy and
# pandas came from the snapshot rather than a fresh import.
PRELOADED = "numpy" in sys.modules and "pandas" in sys.modules

def handler():
    import numpy as np
    import pandas as pd

    frame = pd.DataFrame({
        "group": ["a", "b", "a", "c", "b", "a"],
        "value": np.arange(6, dtype="float64") * 1.5,
    })
    frame["scaled"] = frame["value"].rolling(2, min_periods=1).mean()
    summary = frame.groupby("group")["scaled"].agg(["sum", "count"]).sort_index()
    # A numpy extension module that the warm capture never imported, loaded
    # through a fresh dlopen on top of the restored libraries.
    from numpy.random import default_rng
    sample = default_rng(7).integers(0, 10, size=4).tolist()
    return json.dumps({
        "sums": [round(float(v), 4) for v in summary["sum"]],
        "counts": [int(v) for v in summary["count"]],
        "matmul": float((np.ones((3, 3)) @ np.arange(3.0)).sum()),
        "sample_len": len(sample),
        "preloaded": PRELOADED,
    })
"#;

fn pyodide_dist_dir() -> PathBuf {
    env::var_os("AARDVARK_PYODIDE_DIST_DIR").map_or_else(
        || {
            workspace_root()
                .join(".aardvark/pyodide-distributions")
                .join(format!(
                    "aardvark-{}-pyodide-v{}-full",
                    aardvark_core::AARDVARK_VERSION,
                    aardvark_core::pyodide::PYODIDE_VERSION
                ))
        },
        PathBuf::from,
    )
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
}

fn runtime_config() -> PyRuntimeConfig {
    let dist_dir = pyodide_dist_dir();
    assert!(
        dist_dir.exists(),
        "expected Pyodide distribution at {dist_dir:?}; set AARDVARK_PYODIDE_DIST_DIR or run `cargo run -p aardvark-cli -- assets stage --variant full`"
    );
    PyRuntimeConfig::default().with_pyodide_dist_dir(dist_dir)
}

fn bundle(files: &[(&str, &str)]) -> Bundle {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (path, contents) in files {
        writer.start_file(*path, options).expect("start entry");
        writer.write_all(contents.as_bytes()).expect("write entry");
    }
    let cursor = writer.finish().expect("finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("parse bundle")
}

fn capture_warm_state(bundle: &Bundle) -> WarmState {
    let mut config = runtime_config();
    config.snapshot.save_to = Some(std::env::temp_dir().join("aardvark-native-warm.snapshot"));
    let mut runtime = PyRuntime::new_for_bundle(config, bundle).expect("capture runtime");
    runtime
        .prepare_session_with_manifest(bundle.clone())
        .expect("prepare capture session");
    runtime.capture_warm_state().expect("capture warm state")
}

fn run_restored(bundle: &Bundle, warm_state: &WarmState) -> ExecutionOutcome {
    let config = PyRuntimeConfig {
        warm_state: Some(warm_state.clone()),
        ..runtime_config()
    };
    let mut runtime = PyRuntime::new_for_bundle(config, bundle).expect("restored runtime");
    let (session, _) = runtime
        .prepare_session_with_manifest(bundle.clone())
        .expect("prepare restored session");
    runtime.run_session(&session).expect("run restored session")
}

fn json_payload(outcome: &ExecutionOutcome) -> serde_json::Value {
    assert!(
        outcome.is_success(),
        "expected success, got {:?} (exception: {:?}, stderr: {})",
        outcome.status,
        outcome.diagnostics.exception,
        outcome.diagnostics.stderr
    );
    match outcome.payload() {
        // The text payload is the repr of the returned str.
        Some(ResultPayload::Text(text)) => serde_json::from_str(text.trim_matches('\''))
            .unwrap_or_else(|err| panic!("handler returned {text:?}: {err}")),
        other => panic!("expected text payload, got {other:?}"),
    }
}

#[test]
fn warm_snapshot_with_pandas_preimported_runs_pandas_udf() {
    let bundle = bundle(&[
        ("main.py", PANDAS_HANDLER),
        ("aardvark.manifest.json", PANDAS_MANIFEST),
    ]);
    let warm_state = capture_warm_state(&bundle);

    let expected = serde_json::json!({
        "sums": [9.0, 6.0, 3.75],
        "counts": [3, 2, 1],
        "matmul": 9.0,
        "sample_len": 4,
        "preloaded": true,
    });

    // Two independent restores of the same warm state: replaying the side
    // modules must not depend on state left behind by an earlier restore.
    for attempt in 0..2 {
        let outcome = run_restored(&bundle, &warm_state);
        assert_eq!(
            json_payload(&outcome),
            expected,
            "restore attempt {attempt}"
        );
    }
}
