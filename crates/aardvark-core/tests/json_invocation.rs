use aardvark_core::{
    config::PyRuntimeConfig,
    invocation::InvocationDescriptor,
    outcome::ResultPayload,
    persistent::{BundleArtifact, BundlePool, CleanupMode, PoolOptions},
    strategy::{JsonInput, JsonInvocationStrategy},
    Bundle, PyRuntime, Result,
};
use bytes::Bytes;
use serde_json::json;
use std::env;
use std::io::Write;
use std::path::PathBuf;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn large_numeric_array_value_preserves_json_list() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = builtins.__aardvark_input
    return {
        "is_list": isinstance(payload, list),
        "length": len(payload),
        "first": float(payload[0]),
        "last": float(payload[-1]),
    }
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let session = runtime.prepare_session(bundle, "main:handler")?;
    let values = (0..4096).map(|index| json!((index as f64) * 0.5)).collect();
    let mut strategy = JsonInvocationStrategy::new(Some(serde_json::Value::Array(values)));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["is_list"], json!(true));
            assert_eq!(value["length"], json!(4096));
            assert_eq!(value["first"], json!(0.0));
            assert_eq!(value["last"], json!(2047.5));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn bundle_pool_large_numeric_array_value_preserves_json_list() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = builtins.__aardvark_input
    return {
        "is_list": isinstance(payload, list),
        "length": len(payload),
        "first": float(payload[0]),
        "last": float(payload[-1]),
    }
"#,
    );
    let artifact = BundleArtifact::from_bundle(bundle)?;
    let mut options = PoolOptions::default();
    options.isolate.cleanup = CleanupMode::None;
    let pool = BundlePool::from_artifact(artifact, options)?;
    let handler = pool.prepare_default_handler()?;
    pool.prewarm_handler(&handler)?;

    let values = (0..4096).map(|index| json!((index as f64) * 0.5)).collect();
    let outcome = pool.call_json(&handler, Some(serde_json::Value::Array(values)))?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["is_list"], json!(true));
            assert_eq!(value["length"], json!(4096));
            assert_eq!(value["first"], json!(0.0));
            assert_eq!(value["last"], json!(2047.5));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn bundle_pool_prepared_f32_input() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = builtins.__aardvark_input
    return {
        "is_memoryview": isinstance(payload, memoryview),
        "format": getattr(payload, "format", None),
        "length": len(payload),
        "first": float(payload[0]),
        "last": float(payload[-1]),
    }
"#,
    );
    let artifact = BundleArtifact::from_bundle(bundle)?;
    let mut options = PoolOptions::default();
    options.isolate.cleanup = CleanupMode::None;
    let pool = BundlePool::from_artifact(artifact, options)?;
    let handler = pool.prepare_default_handler()?;

    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.25, 0.5, 0.75] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let outcome =
        pool.call_json_input(&handler, Some(JsonInput::F32LeBytes(Bytes::from(bytes))))?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["is_memoryview"], json!(true));
            assert_eq!(value["format"], json!("f"));
            assert_eq!(value["length"], json!(4));
            assert_eq!(value["first"], json!(0.0));
            assert_eq!(value["last"], json!(0.75));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn large_string_value_preserves_json_string() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    if not hasattr(builtins, "__aardvark_input"):
        return {"has_input": False}
    payload = builtins.__aardvark_input
    return {
        "has_input": True,
        "is_str": isinstance(payload, str),
        "length": len(payload),
        "prefix": payload[:18],
        "suffix": payload[-18:],
        "quote_count": payload.count('"'),
        "newline_count": payload.count('\n'),
    }
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let session = runtime.prepare_session(bundle, "main:handler")?;
    let text = format!(
        "{}{}{}",
        "prefix quote \" slash \\ newline\n",
        "echo-high-".repeat(512),
        "\nend quote \""
    );
    assert!(text.len() >= 4096);
    let mut strategy = JsonInvocationStrategy::new(Some(serde_json::Value::String(text.clone())));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["has_input"], json!(true));
            assert_eq!(value["is_str"], json!(true));
            assert_eq!(value["length"], json!(text.len()));
            assert_eq!(value["prefix"], json!(&text[..18]));
            assert_eq!(value["suffix"], json!(&text[text.len() - 18..]));
            assert_eq!(value["quote_count"], json!(2));
            assert_eq!(value["newline_count"], json!(2));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    let mut empty_strategy = JsonInvocationStrategy::new(None);
    let empty_outcome = runtime.run_session_with_strategy(&session, &mut empty_strategy)?;
    match empty_outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["has_input"], json!(false));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn large_bytes_result_side_channel() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = getattr(builtins, "__aardvark_input", b"")
    return payload
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let session = runtime.prepare_session(bundle, "main:handler")?;
    let text = format!(
        "{}{}{}",
        "prefix quote \" slash \\ newline\n",
        "echo-buffer-".repeat(512),
        "\nend quote \""
    );
    assert!(text.len() >= 4096);

    let mut strategy =
        JsonInvocationStrategy::with_input(Some(JsonInput::Bytes(Bytes::from(text.clone()))));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let buffer = &buffers[0];
            assert_eq!(buffer.id, "json-result");
            assert_eq!(buffer.length, text.len());
            assert_eq!(
                buffer
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("format"))
                    .and_then(|value| value.as_str()),
                Some("bytes")
            );
            let bytes = buffer.as_slice().expect("buffer bytes should be retained");
            assert_eq!(bytes, text.as_bytes());
        }
        other => panic!("expected shared-buffer payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn no_stdio_result_side_channel_fast_return() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = getattr(builtins, "__aardvark_input", b"")
    return payload
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let descriptor = InvocationDescriptor::new("main:handler").with_capture_stdio(false);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let text = "side-channel-fast-return-".repeat(256);
    assert!(text.len() >= 4096);

    let mut strategy =
        JsonInvocationStrategy::with_input(Some(JsonInput::Bytes(Bytes::from(text.clone()))));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let buffer = &buffers[0];
            assert_eq!(buffer.id, "json-result");
            assert_eq!(buffer.length, text.len());
            let bytes = buffer.as_slice().expect("buffer bytes should be retained");
            assert_eq!(bytes, text.as_bytes());
        }
        other => panic!("expected shared-buffer payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn large_string_output_side_channel() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = getattr(builtins, "__aardvark_input", None)
    if payload == "small":
        return "small-result"
    return "side-channel-" + ("x" * 8192)
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let session = runtime.prepare_session(bundle, "main:handler")?;
    let expected = format!("side-channel-{}", "x".repeat(8192));

    let mut strategy = JsonInvocationStrategy::new(None);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!(expected));
        }
        other => panic!("expected json string payload, got {:?}", other),
    }

    let mut small_strategy = JsonInvocationStrategy::new(Some(json!("small")));
    let small_outcome = runtime.run_session_with_strategy(&session, &mut small_strategy)?;
    match small_outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!("small-result"));
        }
        other => panic!("expected small json string payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn single_i64_object_input_fast_path() -> Result<()> {
    let bundle = bundle_with_main(
        r#"
import builtins

def handler():
    payload = getattr(builtins, "__aardvark_input", None)
    return {
        "is_dict": isinstance(payload, dict),
        "value": payload.get("rows") if isinstance(payload, dict) else None,
    }
"#,
    );
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let session = runtime.prepare_session(bundle, "main:handler")?;

    let mut strategy = JsonInvocationStrategy::new(Some(json!({"rows": 12345})));
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["is_dict"], json!(true));
            assert_eq!(value["value"], json!(12345));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    Ok(())
}

#[test]
fn numpy_array_output_side_channel() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
import numpy as np

def handler():
    return np.arange(4096, dtype=np.float32) * np.float32(0.25)
"#,
        r#"{
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "packages": ["numpy"],
            "runtime": {"language": "python"}
        }"#,
    );
    let Some(runtime_config) = runtime_config_with_pyodide_dist() else {
        return Ok(());
    };
    let mut runtime = PyRuntime::new(runtime_config)?;
    let (session, _) = runtime.prepare_session_with_manifest(bundle)?;

    let mut strategy = JsonInvocationStrategy::new(None);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match outcome.payload() {
        Some(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let buffer = &buffers[0];
            assert_eq!(buffer.id, "json-result");
            assert_eq!(buffer.length, 4096 * std::mem::size_of::<f32>());
            assert_eq!(
                buffer
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("format"))
                    .and_then(|value| value.as_str()),
                Some("f32_le")
            );
            let bytes = buffer.as_slice().expect("buffer bytes should be retained");
            let first = f32::from_le_bytes(bytes[0..4].try_into().unwrap());
            let second = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
            let last = f32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap());
            assert_eq!(first, 0.0);
            assert_eq!(second, 0.25);
            assert_eq!(last, 1023.75);
        }
        other => panic!("expected shared-buffer payload, got {:?}", other),
    }

    Ok(())
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|crates_dir| crates_dir.parent())
        .expect("core crate should live under workspace crates/")
        .to_path_buf()
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

fn runtime_config_with_pyodide_dist() -> Option<PyRuntimeConfig> {
    let dist_dir = pyodide_dist_dir();
    if !dist_dir.exists() {
        eprintln!(
            "skipping numpy side-channel test; expected Pyodide distribution at {:?}; set AARDVARK_PYODIDE_DIST_DIR or run `cargo run -p aardvark-cli -- assets stage --variant full`",
            dist_dir
        );
        return None;
    }
    Some(PyRuntimeConfig::default().with_pyodide_dist_dir(dist_dir))
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
    Bundle::from_zip_bytes(bundle_bytes_with_main_and_manifest(code, manifest))
        .expect("failed to parse bundle")
}

fn bundle_bytes_with_main_and_manifest(code: &str, manifest: &str) -> Vec<u8> {
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
    cursor.into_inner()
}
