use aardvark_core::{
    config::PyRuntimeConfig,
    invocation::{InvocationDescriptor, InvocationLimits},
    outcome::{FailureKind, OutcomeStatus, ResultPayload},
    strategy::JsonInvocationStrategy,
    Bundle, ExecutionOutcome, PyRunnerError, PyRuntime, Result,
};
use serde_json::json;
use std::{env, io::Write, path::PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn timeout_failure() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let descriptor = InvocationDescriptor::trivial("main:main").with_limits(InvocationLimits {
        wall_ms: Some(50),
        heap_mb: None,
        cpu_ms: None,
    });
    let outcome = run_with_descriptor(
        &mut runtime,
        r#"
import time

def main():
    time.sleep(0.2)
    return "done"
"#,
        descriptor,
    )?;

    match outcome.status {
        OutcomeStatus::Failure(FailureKind::TimeoutExceeded { requested_ms }) => {
            assert_eq!(requested_ms, 50);
        }
        status => panic!("expected TimeoutExceeded failure, got {:?}", status),
    }

    assert!(
        outcome.diagnostics.stdout.is_empty() && outcome.diagnostics.stderr.is_empty(),
        "no stdout/stderr expected after timeout"
    );
    Ok(())
}

#[test]
fn cpu_limit_failure_descriptor_and_recovery() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;

    let failure_limits = InvocationLimits {
        wall_ms: None,
        heap_mb: None,
        cpu_ms: Some(10),
    };
    let fail_descriptor =
        InvocationDescriptor::trivial("main:main").with_limits(failure_limits.clone());
    let failure_outcome = run_with_descriptor(&mut runtime, CPU_SPIN_LOOP, fail_descriptor)?;
    match failure_outcome.status {
        OutcomeStatus::Failure(FailureKind::CpuLimitExceeded {
            requested_ms,
            used_ms,
        }) => {
            assert_eq!(requested_ms, failure_limits.cpu_ms.unwrap());
            assert!(
                used_ms >= requested_ms,
                "expected used_ms >= requested_ms, got {} >= {}",
                used_ms,
                requested_ms
            );
        }
        other => panic!("expected CpuLimitExceeded, got {:?}", other),
    }
    assert!(
        failure_outcome.diagnostics.cpu_ms_used.is_some(),
        "expected diagnostics to record cpu usage"
    );
    assert_eq!(
        failure_outcome
            .diagnostics
            .filesystem_bytes_written
            .expect("filesystem usage should be tracked"),
        0,
        "expected filesystem usage to remain zero for cpu spin loop"
    );
    assert!(
        failure_outcome
            .diagnostics
            .network_hosts_contacted
            .is_empty(),
        "network diagnostics should be empty when no fetch occurs"
    );

    let success_descriptor =
        InvocationDescriptor::trivial("main:main").with_limits(InvocationLimits {
            wall_ms: None,
            heap_mb: None,
            cpu_ms: Some(5_000),
        });
    let success_outcome = run_with_descriptor(&mut runtime, SIMPLE_SUCCESS, success_descriptor)?;
    assert!(
        matches!(success_outcome.status, OutcomeStatus::Success(_)),
        "expected successful outcome after CPU failure"
    );
    assert!(
        success_outcome.diagnostics.cpu_ms_used.is_some(),
        "expected cpu usage to be reported on success"
    );
    assert_eq!(
        success_outcome
            .diagnostics
            .filesystem_bytes_written
            .expect("filesystem usage should be tracked"),
        0,
        "expected filesystem usage to default to zero for simple success"
    );

    Ok(())
}

#[test]
fn cpu_limit_failure_manifest_resources() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "cpu": {
                "defaultLimitMs": 12
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(CPU_SPIN_LOOP, manifest);
    let (session, parsed_manifest) = runtime.prepare_session_with_manifest(bundle)?;
    let manifest = parsed_manifest.expect("manifest should be returned");
    assert_eq!(
        manifest
            .resources()
            .and_then(|resources| resources.cpu.as_ref())
            .and_then(|cpu| cpu.default_limit_ms),
        Some(12)
    );
    assert_eq!(session.descriptor().limits.cpu_ms, Some(12));

    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Failure(FailureKind::CpuLimitExceeded {
            requested_ms,
            used_ms,
        }) => {
            assert_eq!(requested_ms, 12);
            assert!(
                used_ms >= requested_ms,
                "expected used_ms >= requested_ms, got {} >= {}",
                used_ms,
                requested_ms
            );
        }
        other => panic!("expected CpuLimitExceeded, got {:?}", other),
    }
    assert!(
        outcome.diagnostics.cpu_ms_used.is_some(),
        "expected cpu usage diagnostics when manifest enforces limit"
    );
    assert_eq!(
        outcome
            .diagnostics
            .filesystem_bytes_written
            .expect("filesystem usage should be tracked"),
        0
    );
    assert!(
        outcome.diagnostics.network_hosts_contacted.is_empty(),
        "expected no network contacts for cpu spin loop"
    );
    Ok(())
}

#[test]
fn manifest_pyodide_profile_must_match_constructed_runtime() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "runtime": {
            "language": "python",
            "pyodide": {
                "profile": "blas"
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(SIMPLE_SUCCESS, manifest);
    let err = match runtime.prepare_session_with_manifest(bundle) {
        Ok(_) => panic!("profile mismatch should be rejected before preparing a session"),
        Err(err) => err,
    };

    match err {
        PyRunnerError::Validation(message) => {
            assert!(
                message.contains("Pyodide distribution profile 'blas'"),
                "unexpected validation message: {message}"
            );
            assert!(
                message.contains("PyRuntime::new_for_bundle"),
                "expected remediation hint in message: {message}"
            );
        }
        other => panic!("expected profile validation error, got {other:?}"),
    }

    Ok(())
}

#[test]
fn network_denies_hosts_not_in_allowlist() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "network": {
                "allow": [],
                "httpsOnly": true
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(NETWORK_FETCH_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            let message = info.value.unwrap_or_default();
            assert!(
                message.contains("not permitted"),
                "expected policy message, got {message:?}"
            );
        }
        other => panic!("expected failure due to network policy, got {:?}", other),
    }
    assert_eq!(
        outcome.diagnostics.network_hosts_blocked.len(),
        1,
        "expected blocked host to be recorded"
    );
    let blocked = &outcome.diagnostics.network_hosts_blocked[0];
    assert_eq!(blocked.host, "blocked.example");
    assert_eq!(blocked.reason, "no-allowlist");
    assert!(!blocked.https_required);
    assert!(outcome.diagnostics.network_hosts_contacted.is_empty());
    Ok(())
}

#[test]
fn network_enforces_https_only_policy() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "network": {
                "allow": ["allowed.test"],
                "httpsOnly": true
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(NETWORK_HTTP_FETCH_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            let message = info.value.unwrap_or_default();
            assert!(
                message.contains("requires https"),
                "expected https-only error, got {message:?}"
            );
        }
        other => panic!("expected https-only failure, got {:?}", other),
    }
    assert_eq!(
        outcome.diagnostics.network_hosts_blocked.len(),
        1,
        "expected blocked host to be recorded for http request"
    );
    let blocked = &outcome.diagnostics.network_hosts_blocked[0];
    assert_eq!(blocked.host, "allowed.test");
    assert_eq!(blocked.reason, "scheme-not-allowed");
    assert!(blocked.https_required);
    Ok(())
}

#[test]
fn diagnostics_capture_resource_usage() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "filesystem": {
                "mode": "readWrite",
                "quotaBytes": 65536
            },
            "network": {
                "allow": ["allowed.test"],
                "httpsOnly": false
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(DIAGNOSTICS_RESOURCE_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Success(_) => {}
        other => panic!("expected success, got {:?}", other),
    }
    let diagnostics = outcome.diagnostics;
    assert!(
        diagnostics.cpu_ms_used.is_some(),
        "cpu usage should be reported in diagnostics"
    );
    let fs_bytes = diagnostics
        .filesystem_bytes_written
        .expect("filesystem usage should be reported");
    assert!(
        fs_bytes >= 16,
        "expected filesystem usage to be at least 16 bytes, got {}",
        fs_bytes
    );
    assert!(
        diagnostics
            .network_hosts_contacted
            .iter()
            .any(|contact| contact.host == "allowed.test" && !contact.https),
        "expected allowed.test to appear in network diagnostics"
    );
    assert!(
        diagnostics.network_hosts_blocked.is_empty(),
        "no network denials expected in success scenario"
    );
    Ok(())
}

#[test]
fn pyodide_pyxhr_uses_xmlhttprequest_polyfill() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(PYODIDE_PYXHR_SCRIPT);
    let session = runtime.prepare_session(bundle, "main:main")?;
    let mut strategy = JsonInvocationStrategy::new(None);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], json!(200));
            assert_eq!(value["text"], json!("Hello from XHR"));
            assert_eq!(value["content_type"], json!("text/plain"));
        }
        other => panic!(
            "expected json payload, got {:?}; status {:?}; stdout {:?}; stderr {:?}",
            other, outcome.status, outcome.diagnostics.stdout, outcome.diagnostics.stderr
        ),
    }
    Ok(())
}

#[test]
fn pyodide_http_patch_all_uses_xmlhttprequest_polyfill() -> Result<()> {
    let Some(runtime_config) = runtime_config_with_pyodide_dist() else {
        return Ok(());
    };
    let mut runtime = PyRuntime::new(runtime_config)?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "packages": ["pyodide-http"],
        "runtime": {"language": "python"}
    }"#;
    let bundle = bundle_with_main_and_manifest(PYODIDE_HTTP_PATCH_ALL_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let mut strategy = JsonInvocationStrategy::new(None);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["should_patch"], json!(true));
            assert_eq!(value["body"], json!("Hello from pyodide-http"));
        }
        other => panic!(
            "expected json payload, got {:?}; status {:?}; stdout {:?}; stderr {:?}",
            other, outcome.status, outcome.diagnostics.stdout, outcome.diagnostics.stderr
        ),
    }
    Ok(())
}

#[test]
fn host_capability_gates_rawctx_buffers() -> Result<()> {
    let mut runtime_allowed = PyRuntime::new(PyRuntimeConfig::default())?;
    let allowed = run_main(&mut runtime_allowed, RAWCTX_PUBLISH_SCRIPT)?;
    assert!(
        allowed.is_success(),
        "expected rawctx publish to succeed with default capabilities"
    );

    let mut restricted_config = PyRuntimeConfig::default();
    restricted_config.host_capabilities.clear();
    let mut runtime_denied = PyRuntime::new(restricted_config)?;
    let denied = run_main(&mut runtime_denied, RAWCTX_PUBLISH_SCRIPT)?;
    assert!(matches!(denied.status, OutcomeStatus::Failure(_)));
    Ok(())
}

#[test]
fn filesystem_blocks_writes_in_read_mode() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "filesystem": {
                "mode": "read"
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(FILESYSTEM_CREATE_FILE_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(_)) => {}
        OutcomeStatus::Failure(FailureKind::AdapterError { .. }) => {}
        other => panic!("expected filesystem permission failure, got {:?}", other),
    }
    assert!(
        !outcome.diagnostics.filesystem_violations.is_empty(),
        "expected filesystem violation to be recorded"
    );
    let violation = &outcome.diagnostics.filesystem_violations[0];
    assert!(
        violation
            .message
            .contains("writes are disabled in read-only mode"),
        "unexpected violation message: {:?}",
        violation.message
    );
    assert_eq!(
        violation.path.as_deref(),
        Some("/session/runtime-test/test.txt"),
        "unexpected violation path"
    );
    Ok(())
}

#[test]
fn filesystem_enforces_quota() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "filesystem": {
                "mode": "readWrite",
                "quotaBytes": 8
            }
        }
    }"#;
    let bundle = bundle_with_main_and_manifest(FILESYSTEM_EXCEED_QUOTA_SCRIPT, manifest);
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;
    let outcome = runtime.run_session(&session)?;
    match outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(_)) => {}
        OutcomeStatus::Failure(FailureKind::AdapterError { .. }) => {}
        other => panic!("expected filesystem quota failure, got {:?}", other),
    }
    assert!(
        !outcome.diagnostics.filesystem_violations.is_empty(),
        "expected quota violation to be recorded"
    );
    let violation = &outcome.diagnostics.filesystem_violations[0];
    assert!(
        violation.message.contains("quota exceeded"),
        "unexpected quota violation message: {:?}",
        violation.message
    );
    assert_eq!(
        violation.path.as_deref(),
        Some("/session/runtime-test/big.txt"),
        "unexpected quota violation path"
    );
    Ok(())
}

#[test]
fn filesystem_cleanup_removes_session_files() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:main",
        "resources": {
            "filesystem": {
                "mode": "readWrite",
                "quotaBytes": 65536
            }
        }
    }"#;

    let mut runtime_create = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle_create = bundle_with_main_and_manifest(FILESYSTEM_CREATE_PERSIST_SCRIPT, manifest);
    let (session_create, _) = runtime_create.prepare_session_with_manifest(bundle_create)?;
    let outcome_create = runtime_create.run_session(&session_create)?;
    assert!(
        outcome_create.is_success(),
        "expected creation success but got {:?}",
        outcome_create.status
    );

    drop(runtime_create);

    let mut runtime_check = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle_check = bundle_with_main_and_manifest(FILESYSTEM_CHECK_PERSIST_SCRIPT, manifest);
    let (session_check, _) = runtime_check.prepare_session_with_manifest(bundle_check)?;
    let outcome_check = runtime_check.run_session(&session_check)?;
    match outcome_check.status {
        OutcomeStatus::Success(ResultPayload::Text(value)) => {
            let normalized = value.trim_matches('\'');
            assert_eq!(
                normalized, "check:False",
                "expected cleanup to remove session file"
            );
        }
        other => panic!("expected success payload after cleanup, got {:?}", other),
    }
    Ok(())
}

fn bundle_with_main(code: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.py",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start bundle entry");
    writer
        .write_all(code.as_bytes())
        .expect("failed to write bundle entry");
    let cursor = writer.finish().expect("failed to finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("failed to parse bundle")
}

fn bundle_with_main_and_manifest(code: &str, manifest: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.py",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start main entry");
    writer
        .write_all(code.as_bytes())
        .expect("failed to write main entry");

    writer
        .start_file(
            "aardvark.manifest.json",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start manifest entry");
    writer
        .write_all(manifest.as_bytes())
        .expect("failed to write manifest");

    let cursor = writer.finish().expect("failed to finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("failed to parse bundle")
}

fn runtime_config_with_pyodide_dist() -> Option<PyRuntimeConfig> {
    let dist_dir = pyodide_dist_dir();
    if !dist_dir.exists() {
        eprintln!(
            "skipping pyodide-http package test; expected Pyodide distribution at {:?}; set AARDVARK_PYODIDE_DIST_DIR or run `cargo run -p aardvark-cli -- assets stage --variant full`",
            dist_dir
        );
        return None;
    }
    Some(PyRuntimeConfig::default().with_pyodide_dist_dir(dist_dir))
}

fn pyodide_dist_dir() -> PathBuf {
    env::var_os("AARDVARK_PYODIDE_DIST_DIR").map_or_else(
        || {
            workspace_root()
                .join(".aardvark/pyodide-distributions/aardvark-0.1.1-pyodide-v0.29.4-full")
        },
        PathBuf::from,
    )
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .expect("core crate should live under workspace crates/")
        .to_path_buf()
}

fn run_main(runtime: &mut PyRuntime, code: &str) -> Result<ExecutionOutcome> {
    let bundle = bundle_with_main(code);
    let session = runtime.prepare_session(bundle, "main:main")?;
    runtime.run_session(&session)
}

fn run_with_descriptor(
    runtime: &mut PyRuntime,
    code: &str,
    descriptor: InvocationDescriptor,
) -> Result<ExecutionOutcome> {
    let bundle = bundle_with_main(code);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    runtime.run_session(&session)
}

const CPU_SPIN_LOOP: &str = r#"
import time

def main():
    start = time.perf_counter()
    iterations = 0
    while True:
        iterations += 1
        _ = sum(i * i for i in range(128))
        if time.perf_counter() - start > 1.5:
            return iterations
"#;

const SIMPLE_SUCCESS: &str = r#"
def main():
    return "ok"
"#;

const NETWORK_FETCH_SCRIPT: &str = r#"
import js

def main():
    js.__pyRunnerNativeFetch("https://blocked.example/resource")
"#;

const NETWORK_HTTP_FETCH_SCRIPT: &str = r#"
import js

def main():
    js.__pyRunnerNativeFetch("http://allowed.test/resource")
"#;

const DIAGNOSTICS_RESOURCE_SCRIPT: &str = r#"
import js
from pathlib import Path

def main():
    root = Path("/session/diag-test")
    root.mkdir(parents=True, exist_ok=True)
    (root / "note.txt").write_text("hello diagnostics")
    try:
        js.__pyRunnerNativeFetch("http://allowed.test/resource")
    except Exception:
        pass
    return "done"
"#;

const PYODIDE_PYXHR_SCRIPT: &str = r#"
from pyodide.http import pyxhr

def main():
    response = pyxhr.get("data:text/plain,Hello%20from%20XHR")
    return {
        "status": response.status_code,
        "text": response.text,
        "content_type": response.headers.get("content-type"),
    }
"#;

const PYODIDE_HTTP_PATCH_ALL_SCRIPT: &str = r#"
import pyodide_http
import urllib.request

def main():
    should_patch = pyodide_http.should_patch()
    pyodide_http.patch_all()
    with urllib.request.urlopen("data:text/plain,Hello%20from%20pyodide-http") as response:
        body = response.read().decode("utf-8")
    return {
        "should_patch": should_patch,
        "body": body,
    }
"#;

const RAWCTX_PUBLISH_SCRIPT: &str = r#"
import js

def main():
    js.__aardvarkPublishBuffer("buf", b"abc", None)
    return "ok"
"#;

const FILESYSTEM_CREATE_FILE_SCRIPT: &str = r#"
from pathlib import Path

def main():
    root = Path("/session/runtime-test")
    root.mkdir(parents=True, exist_ok=True)
    (root / "test.txt").write_text("hello")
    return "ok"
"#;

const FILESYSTEM_EXCEED_QUOTA_SCRIPT: &str = r#"
from pathlib import Path

def main():
    root = Path("/session/runtime-test")
    root.mkdir(parents=True, exist_ok=True)
    (root / "big.txt").write_text("x" * 32)
    return "ok"
"#;

const FILESYSTEM_CREATE_PERSIST_SCRIPT: &str = r#"
from pathlib import Path

def main():
    root = Path("/session/runtime-test")
    root.mkdir(parents=True, exist_ok=True)
    (root / "persist.txt").write_text("persist")
    return "done"
"#;

const FILESYSTEM_CHECK_PERSIST_SCRIPT: &str = r#"
from pathlib import Path

def main():
    root = Path("/session/runtime-test")
    return "check:" + str((root / "persist.txt").exists())
"#;
