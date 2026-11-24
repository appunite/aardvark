//! Guest code reaching Aardvark's host hooks through `import js`: the native
//! fetch hook serves only files of the staged distribution, and guest JS that
//! runs after the call (here a published buffer's `destroy`) is bounded by the
//! cleanup deadline instead of hanging the thread.

use aardvark_core::outcome::ResultPayload;
use aardvark_core::{
    Bundle, ExecutionOutcome, InvocationDescriptor, InvocationLimits, JsonInvocationStrategy,
    PyRunnerError, PyRuntime, PyRuntimeConfig,
};
use std::env;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const HANDLER: &str = r#"
import builtins
import json

import js

def handler():
    mode = builtins.__aardvark_input["mode"]
    if mode == "read":
        leaked = []
        for url in builtins.__aardvark_input["urls"]:
            try:
                response = js.__pyRunnerNativeFetch(url)
            except Exception:
                continue
            if response is not None and response.status == 200:
                leaked.append(url)
        return json.dumps({"leaked": leaked})
    if mode == "hostile_destroy":
        js.eval(
            "__aardvarkPublishBuffer('trap', "
            "{ toJs() { return new Uint8Array(1); }, destroy() { for (;;) {} } }, null)"
        )
    return json.dumps({"ok": True})
"#;

const MANIFEST: &str = r#"{
    "schemaVersion": "1.0",
    "entrypoint": "main:handler",
    "runtime": {"language": "python"}
}"#;

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

fn bundle() -> Bundle {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (path, contents) in [("main.py", HANDLER), ("aardvark.manifest.json", MANIFEST)] {
        writer.start_file(path, options).expect("start entry");
        writer.write_all(contents.as_bytes()).expect("write entry");
    }
    Bundle::from_zip_bytes(writer.finish().expect("finish").into_inner()).expect("bundle")
}

fn runtime() -> PyRuntime {
    let dist_dir = pyodide_dist_dir();
    assert!(
        dist_dir.exists(),
        "expected Pyodide distribution at {dist_dir:?}; set AARDVARK_PYODIDE_DIST_DIR or run `cargo run -p aardvark-cli -- assets stage --variant full`"
    );
    let config = PyRuntimeConfig::default().with_pyodide_dist_dir(dist_dir);
    PyRuntime::new_for_bundle(config, &bundle()).expect("runtime")
}

fn call(
    runtime: &mut PyRuntime,
    input: serde_json::Value,
) -> Result<ExecutionOutcome, PyRunnerError> {
    let descriptor = InvocationDescriptor::new("main:handler").with_limits(InvocationLimits {
        wall_ms: Some(10_000),
        heap_mb: None,
        cpu_ms: None,
    });
    let session = runtime
        .prepare_session_with_descriptor(bundle(), descriptor)
        .expect("session");
    let mut strategy = JsonInvocationStrategy::new(Some(input));
    runtime.run_session_with_strategy(&session, &mut strategy)
}

fn payload(outcome: &ExecutionOutcome) -> serde_json::Value {
    match outcome.payload() {
        Some(ResultPayload::Text(text)) => serde_json::from_str(text.trim_matches('\'')).unwrap(),
        Some(ResultPayload::Json(serde_json::Value::String(text))) => {
            serde_json::from_str(text).unwrap()
        }
        other => panic!(
            "expected a text payload, got {other:?} (status {:?}, exception {:?})",
            outcome.status, outcome.diagnostics.exception
        ),
    }
}

#[test]
fn native_fetch_serves_no_file_outside_the_distribution() {
    let mut runtime = runtime();
    let dist = pyodide_dist_dir();
    let outside = dist
        .parent()
        .expect("distribution parent")
        .join("outside-secret.txt");
    std::fs::write(&outside, b"secret").expect("write outside file");
    let urls = [
        "http://x/../../../../../../../../etc/passwd".to_string(),
        "https://x/../../../../../../../../proc/self/environ".to_string(),
        "http://x/%2e%2e/%2e%2e/%2e%2e/etc/passwd".to_string(),
        "http://x/../outside-secret.txt".to_string(),
        "http://x/..%2foutside-secret.txt".to_string(),
    ];
    let outcome = call(
        &mut runtime,
        serde_json::json!({"mode": "read", "urls": urls}),
    )
    .expect("run");
    let _ = std::fs::remove_file(&outside);
    assert_eq!(payload(&outcome)["leaked"], serde_json::json!([]));
}

#[test]
fn a_guest_destroy_that_never_returns_is_cut_off_after_the_call() {
    let mut runtime = runtime();
    let started = Instant::now();
    let result = call(&mut runtime, serde_json::json!({"mode": "hostile_destroy"}));
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "cleanup ran unbounded: {:?}",
        started.elapsed()
    );
    match result {
        Err(PyRunnerError::TimeoutExceeded { requested_ms }) => assert_eq!(requested_ms, 2_000),
        other => panic!("expected a cleanup timeout, got {other:?}"),
    }
    // The hostile buffer was forgotten before its destroy ran, so the next
    // call's cleanup doesn't run it again.
    let next = call(&mut runtime, serde_json::json!({"mode": "ok"})).expect("next call runs");
    assert_eq!(payload(&next)["ok"], true);
}
