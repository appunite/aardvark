//! Per-call memory limits: `InvocationLimits::heap_mb` caps the context's
//! WebAssembly memory (Pyodide's heap) while a call runs, and
//! `PyRuntimeConfig::js_heap_limit_mb` caps the JS heap, without letting a
//! guest abort the process or switch the cap off.

use aardvark_core::outcome::{FailureKind, OutcomeStatus, ResultPayload};
use aardvark_core::{
    Bundle, ExecutionOutcome, InvocationDescriptor, InvocationLimits, JsonInvocationStrategy,
    PyRuntime, PyRuntimeConfig,
};
use std::env;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const HANDLER: &str = r#"
import builtins
import json

import js

def handler():
    request = builtins.__aardvark_input
    mode, mib = request["mode"], request["mib"]
    held = []
    if mode == "probe":
        pass
    elif mode == "python":
        # Allocate `mib` MiB from Python, catching the MemoryError: the cap
        # must fail the call anyway.
        try:
            held.append(bytearray(mib * 1024 * 1024))
        except MemoryError:
            return json.dumps({"caught": True})
    elif mode == "unpatch":
        # Try to put an unlimited grow back (the locked properties refuse),
        # then allocate.
        for attempt in (
            "WebAssembly.Memory.prototype.grow = function (pages) { return 0; }",
            "Object.defineProperty(WebAssembly, 'Memory', { value: function () {} })",
            "delete WebAssembly.Memory.prototype.grow",
        ):
            try:
                js.eval(attempt)
            except Exception:
                pass
        held.append(bytearray(mib * 1024 * 1024))
    elif mode == "new_memory":
        js.eval(f"new WebAssembly.Memory({{ initial: {mib * 16} }})")
    elif mode == "array_buffers":
        # Host-side ArrayBuffers, outside both the JS heap and WebAssembly
        # memory, caught one by one.
        for _ in range(mib // 64):
            try:
                held.append(js.ArrayBuffer.new(64 * 1024 * 1024))
            except Exception:
                pass
    elif mode == "print_flood":
        line = "x" * 1023
        for _ in range(mib * 1024):
            print(line)
    elif mode == "js_heap":
        js.eval("const hoard = []; for (;;) { hoard.push(new Array(1 << 16).fill(Math.random())); }")
    return json.dumps({"caught": False, "held": len(held)})
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

fn runtime(js_heap_limit_mb: Option<u64>) -> PyRuntime {
    let dist_dir = pyodide_dist_dir();
    assert!(
        dist_dir.exists(),
        "expected Pyodide distribution at {dist_dir:?}; set AARDVARK_PYODIDE_DIST_DIR or run `cargo run -p aardvark-cli -- assets stage --variant full`"
    );
    let config = PyRuntimeConfig {
        js_heap_limit_mb,
        ..PyRuntimeConfig::default().with_pyodide_dist_dir(dist_dir)
    };
    PyRuntime::new_for_bundle(config, &bundle()).expect("runtime")
}

fn call(runtime: &mut PyRuntime, mode: &str, mib: u64, heap_mb: Option<u64>) -> ExecutionOutcome {
    let descriptor = InvocationDescriptor::new("main:handler").with_limits(InvocationLimits {
        wall_ms: Some(20_000),
        heap_mb,
        cpu_ms: None,
    });
    let session = runtime
        .prepare_session_with_descriptor(bundle(), descriptor)
        .expect("session");
    let mut strategy =
        JsonInvocationStrategy::new(Some(serde_json::json!({"mode": mode, "mib": mib})));
    runtime
        .run_session_with_strategy(&session, &mut strategy)
        .expect("run")
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

fn assert_heap_limit(outcome: &ExecutionOutcome, limit_mb: u64) {
    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::HeapLimitExceeded { requested_mb }) => {
            assert_eq!(*requested_mb, limit_mb);
        }
        other => panic!(
            "expected HeapLimitExceeded, got {other:?} (exception {:?})",
            outcome.diagnostics.exception
        ),
    }
}

/// The context's WebAssembly memory right now, in MiB, plus the cap under
/// test: room for `headroom_mb` more.
fn cap_with_headroom(runtime: &mut PyRuntime, headroom_mb: u64) -> u64 {
    let probe = call(runtime, "probe", 0, None);
    payload(&probe);
    let wasm_kib = probe
        .diagnostics
        .wasm_memory_kib
        .expect("Pyodide's heap is a tracked WebAssembly memory");
    wasm_kib / 1024 + headroom_mb
}

#[test]
fn python_allocations_past_heap_mb_fail_the_call_even_when_caught() {
    let mut runtime = runtime(None);
    let cap = cap_with_headroom(&mut runtime, 64);

    let within = call(&mut runtime, "python", 16, Some(cap));
    assert_eq!(
        payload(&within)["caught"],
        false,
        "16 MiB fits in the headroom"
    );

    let over = call(&mut runtime, "python", 512, Some(cap));
    assert_heap_limit(&over, cap);
    assert!(over.diagnostics.wasm_memory_kib.unwrap() <= cap * 1024);

    // The cap is per call: the same runtime still serves.
    let after = call(&mut runtime, "python", 16, Some(cap));
    assert_eq!(payload(&after)["caught"], false);
}

#[test]
fn guests_cannot_unpatch_the_memory_guard() {
    let mut runtime = runtime(None);
    let cap = cap_with_headroom(&mut runtime, 64);
    let outcome = call(&mut runtime, "unpatch", 512, Some(cap));
    assert_heap_limit(&outcome, cap);
}

#[test]
fn guests_cannot_create_memory_past_the_cap() {
    let mut runtime = runtime(None);
    let cap = cap_with_headroom(&mut runtime, 64);
    let outcome = call(&mut runtime, "new_memory", 256, Some(cap));
    assert_heap_limit(&outcome, cap);
}

#[test]
fn a_js_heap_bomb_ends_the_call_instead_of_the_process() {
    let mut runtime = runtime(Some(256));
    let outcome = call(&mut runtime, "js_heap", 0, None);
    assert_heap_limit(&outcome, 256);
}

#[test]
fn array_buffers_past_heap_mb_fail_the_call() {
    let mut runtime = runtime(None);
    let cap = cap_with_headroom(&mut runtime, 64);
    let outcome = call(&mut runtime, "array_buffers", 2048, Some(cap));
    assert_heap_limit(&outcome, cap);
    let after = call(&mut runtime, "python", 16, Some(cap));
    assert_eq!(payload(&after)["caught"], false, "the runtime still serves");
}

#[test]
fn captured_output_is_capped() {
    let mut runtime = runtime(None);
    let outcome = call(&mut runtime, "print_flood", 8, None);
    payload(&outcome);
    let stdout = &outcome.diagnostics.stdout;
    assert!(
        stdout.len() <= 1024 * 1024 + 64,
        "{} bytes of stdout",
        stdout.len()
    );
    assert!(stdout.ends_with("[aardvark: output truncated]\n"));
}
