use aardvark_core::{
    config::PyRuntimeConfig,
    invocation::InvocationDescriptor,
    outcome::{OutcomeStatus, ResultPayload},
    strategy::{RawCtxInput, RawCtxInvocationStrategy, RawCtxMetadata},
    Bundle, PyRuntime, Result,
};
use bytes::Bytes;
use serde_json::json;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn owned_buffer_transfer() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    payload = builtins.__aardvark_rawctx_inputs["payload"]
    data = payload["data"]
    return {
        "length": len(data),
        "first": data[0],
        "last": data[-1],
        "sum": sum(data),
    }
"#,
    );
    let session = runtime.prepare_session(bundle, "main:main")?;
    let bytes = (0..=255).cycle().take(4096).collect::<Vec<u8>>();
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from(bytes),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::Json(value)) => {
            assert_eq!(value["length"], 4096);
            assert_eq!(value["first"], 0);
            assert_eq!(value["last"], 255);
            assert_eq!(value["sum"], 522240);
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }
    Ok(())
}

#[test]
fn direct_publish_buffer() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    payload = builtins.__aardvark_rawctx_inputs["payload"]
    data = payload["data"]
    publisher = builtins.__aardvark_publish_buffer
    publisher("direct-output", data, {"source": "direct-python"})
    return None
"#,
    );
    let session = runtime.prepare_session(bundle, "main:main")?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"direct-publish"),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "direct-output");
            assert_eq!(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("source"))
                    .and_then(|value| value.as_str()),
                Some("direct-python")
            );
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"direct-publish");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn shared_buffer_only_success_fast_path() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    payload = builtins.__aardvark_rawctx_inputs["payload"]
    data = payload["data"]
    publisher = builtins.__aardvark_publish_buffer
    publisher("fast-output", data, {"source": "shared-buffer-only"})
    return {"ignored": True}
"#,
    );
    let descriptor = InvocationDescriptor::new("main:main")
        .with_capture_stdio(false)
        .with_rawctx_shared_buffer_only_success(true);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"fast-publish"),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "fast-output");
            assert_eq!(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("source"))
                    .and_then(|value| value.as_str()),
                Some("shared-buffer-only")
            );
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"fast-publish");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn output_metadata_can_be_disabled() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    payload = builtins.__aardvark_rawctx_inputs["payload"]
    data = payload["data"]
    publisher = builtins.__aardvark_publish_buffer
    publisher("metadata-disabled-output", data, {"source": "should-not-materialize"})
    return {"ignored": True}
"#,
    );
    let descriptor = InvocationDescriptor::new("main:main")
        .with_capture_stdio(false)
        .with_rawctx_shared_buffer_only_success(true)
        .with_rawctx_output_metadata(false);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"metadata-disabled"),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "metadata-disabled-output");
            assert!(
                handle.metadata.is_none(),
                "output metadata should not be materialized"
            );
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"metadata-disabled");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    let metadata_bundle = bundle_with_main(
        r#"
import builtins

def main():
    builtins.__aardvark_publish_buffer(
        "metadata-restored-output",
        "metadata-restored",
        {"source": "metadata-restored"},
    )
    return None
"#,
    );
    let metadata_session = runtime.prepare_session(metadata_bundle, "main:main")?;
    let metadata_outcome = runtime.run_session(&metadata_session)?;
    match &metadata_outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "metadata-restored-output");
            assert_eq!(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("source"))
                    .and_then(|value| value.as_str()),
                Some("metadata-restored")
            );
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"metadata-restored");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

#[test]
fn flat_input_buffers() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    payload = builtins.__aardvark_rawctx_inputs["payload"]
    is_flat = isinstance(payload, memoryview)
    publisher = builtins.__aardvark_publish_buffer
    publisher("flat-output", payload if is_flat else payload["data"], {"flat": is_flat})
    return None
"#,
    );
    let descriptor = InvocationDescriptor::new("main:main")
        .with_capture_stdio(false)
        .with_rawctx_flat_input_buffers(true);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"flat-input"),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "flat-output");
            assert_eq!(
                handle
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("flat"))
                    .and_then(|value| value.as_bool()),
                Some(true)
            );
            let bytes = handle.as_slice().expect("shared buffer should retain data");
            assert_eq!(bytes, b"flat-input");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }
    Ok(())
}

#[test]
fn empty_inputs_do_not_reuse_previous_buffers() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins

def main():
    inputs = getattr(builtins, "__aardvark_rawctx_inputs", {})
    payload = inputs.get("payload")
    if payload is None:
        return {"has_payload": False, "count": len(inputs)}
    return {"has_payload": True, "length": len(bytes(payload["data"]))}
"#,
    );
    let session = runtime.prepare_session(bundle, "main:main")?;

    let mut input_strategy = RawCtxInvocationStrategy::new(vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"first"),
        Some(RawCtxMetadata::new("binary")),
    )?]);
    let first = runtime.run_session_with_strategy(&session, &mut input_strategy)?;
    match first.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["has_payload"], json!(true));
            assert_eq!(value["length"], json!(5));
        }
        other => panic!("expected json payload, got {:?}", other),
    }

    let mut empty_strategy = RawCtxInvocationStrategy::new(Vec::new());
    let second = runtime.run_session_with_strategy(&session, &mut empty_strategy)?;
    match second.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["has_payload"], json!(false));
            assert_eq!(value["count"], json!(0));
        }
        other => panic!("expected json payload, got {:?}", other),
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
