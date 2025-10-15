use aardvark_core::{
    config::{PyRuntimeConfig, ResetPolicy},
    invocation::{FieldDescriptor, InvocationDescriptor, InvocationLimits},
    outcome::{FailureKind, OutcomeStatus, ResultPayload},
    pool::PoolConfig,
    strategy::{
        JsonInvocationStrategy, RawCtxBindingBuilder, RawCtxInput, RawCtxInvocationStrategy,
        RawCtxMetadata, RawCtxPublishBuilder, RawCtxTableColumnBuilder, RawCtxTableSpecBuilder,
    },
    Bundle, ExecutionOutcome, PyRuntime, PyRuntimePool, Result,
};
use bytes::Bytes;
use serde_json::json;
use std::env;
use std::io::Write;
use zip::write::FileOptions;
use zip::CompressionMethod;

#[test]
fn runtime_pool_and_outcome_behaviour() -> Result<()> {
    verify_pooled_runtime_manual_reset()?;
    verify_after_invocation_reset_policy()?;
    verify_python_exception_outcome()?;
    verify_timeout_failure()?;
    verify_shared_buffer_payload()?;
    verify_javascript_default_entrypoint()?;
    verify_rawctx_adapter_roundtrip()?;
    verify_prepare_session_with_manifest_defaults()?;
    verify_rawctx_auto_wrapper()?;
    verify_rawctx_multi_output_publish()?;
    verify_rawctx_table_records()?;
    verify_rawctx_table_missing_column()?;
    verify_rawctx_table_column_decoder()?;
    verify_rawctx_auto_wrapper_base64()?;
    verify_rawctx_auto_wrapper_missing_required()?;
    verify_rawctx_table_metadata()?;
    verify_rawctx_table_manifest_derivation()?;
    verify_rawctx_decoder_invalid_option()?;
    verify_rawctx_decoder_invalid_base64_option()?;
    verify_rawctx_table_invalid_schema()?;
    verify_after_invocation_reset_failure()?;
    verify_pool_reset_failure_removes_runtime()?;
    Ok(())
}

fn verify_pooled_runtime_manual_reset() -> Result<()> {
    let config = PyRuntimeConfig {
        reset_policy: ResetPolicy::Manual,
        ..PyRuntimeConfig::default()
    };
    let pool = PyRuntimePool::new(PoolConfig::new(1, config))?;

    let first_runtime_id = {
        let mut pooled = pool.checkout()?;
        let runtime_id = pooled
            .runtime()
            .runtime_id()
            .map(|id| id.to_owned())
            .expect("runtime id must be set");
        let outcome = run_main(
            pooled.runtime(),
            r#"
def main():
    import builtins
    builtins.__aardvark_marker = "set"
    return "set"
"#,
        )?;
        assert!(outcome.is_success(), "expected success outcome");
        assert_eq!(payload_text(&outcome), "'set'");
        runtime_id
    };

    let second_runtime_id = {
        let mut pooled = pool.checkout()?;
        let runtime_id = pooled
            .runtime()
            .runtime_id()
            .map(|id| id.to_owned())
            .expect("runtime id must be set");
        let outcome = run_main(
            pooled.runtime(),
            r#"
def main():
    import builtins
    return "stale" if hasattr(builtins, "__aardvark_marker") else "fresh"
"#,
        )?;
        assert!(outcome.is_success(), "expected success outcome");
        assert_eq!(payload_text(&outcome), "'fresh'");
        runtime_id
    };

    assert_eq!(
        first_runtime_id, second_runtime_id,
        "pool should reuse the same runtime instance"
    );
    Ok(())
}

fn verify_shared_buffer_payload() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let outcome = run_main(
        &mut runtime,
        r#"
from js import globalThis as js

def main():
    data = bytearray(b"shared-data")
    js.__aardvarkPublishBuffer("buf-1", memoryview(data), {"dtype": "u8"})
    return None
"#,
    )?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "buf-1");
            assert_eq!(handle.length, 11);
            assert!(
                matches!(
                    handle.metadata.as_ref().and_then(|meta| meta.get("dtype")),
                    Some(serde_json::Value::String(value)) if value == "u8"
                ),
                "metadata should include dtype='u8'"
            );
            let bytes = handle
                .as_bytes()
                .expect("shared buffer should retain bytes");
            assert_eq!(bytes.as_ref(), b"shared-data");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_javascript_default_entrypoint() -> Result<()> {
    let manifest = r#"{
        "schemaVersion": "1.0",
        "entrypoint": "main:default",
        "runtime": { "language": "javascript" }
    }"#;

    let bundle = bundle_with_js_main_and_manifest(
        r#"
export default function main() {
    return { greeting: "hello" };
}
"#,
        manifest,
    );

    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let (session, manifest_opt) = runtime.prepare_session_with_manifest(bundle)?;
    assert!(manifest_opt.is_some(), "manifest should be detected");
    assert_eq!(
        session.descriptor().language,
        Some(aardvark_core::RuntimeLanguage::JavaScript)
    );
    let outcome = runtime.run_session(&session)?;
    match outcome.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!({ "greeting": "hello" }));
        }
        other => panic!("expected json payload, got {:?}", other),
    }
    Ok(())
}

fn verify_prepare_session_with_manifest_defaults() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main_and_manifest(
        r#"
def handler():
    return {"status": "ok"}
"#,
        r#"{
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "packages": []
        }"#,
    );

    let (session, manifest) = runtime.prepare_session_with_manifest(bundle)?;
    assert!(manifest.is_some());
    assert_eq!(manifest.as_ref().unwrap().entrypoint(), "main:handler");

    let mut strategy = JsonInvocationStrategy::new(None);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::Json(value)) => {
            assert_eq!(value["status"], "ok");
        }
        OutcomeStatus::Success(ResultPayload::Text(text)) => {
            assert!(text.contains("status"));
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_after_invocation_reset_failure() -> Result<()> {
    let config = PyRuntimeConfig {
        reset_policy: ResetPolicy::AfterInvocation,
        ..PyRuntimeConfig::default()
    };
    let mut runtime = PyRuntime::new(config)?;
    env::set_var("AARDVARK_TEST_FORCE_RESET_FAILURE", "after_invocation");

    let outcome = run_main(
        &mut runtime,
        r#"
def main():
    return "ok"
"#,
    )?;

    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::Other { message }) => {
            assert!(
                message.contains("reset failed"),
                "expected reset failure message, got {message:?}"
            );
        }
        other => panic!("expected FailureKind::Other from reset, got {:?}", other),
    }

    // Ensure no lingering environment flag.
    env::remove_var("AARDVARK_TEST_FORCE_RESET_FAILURE");
    Ok(())
}

fn verify_pool_reset_failure_removes_runtime() -> Result<()> {
    let config = PyRuntimeConfig {
        reset_policy: ResetPolicy::Manual,
        ..PyRuntimeConfig::default()
    };
    let pool = PyRuntimePool::new(PoolConfig::new(1, config.clone()))?;

    let first_runtime_id;
    {
        let mut pooled = pool.checkout()?;
        first_runtime_id = pooled
            .runtime()
            .runtime_id()
            .map(|id| id.to_owned())
            .expect("runtime id must be set");
        let outcome = run_main(
            pooled.runtime(),
            r#"
def main():
    return "ok"
"#,
        )?;
        assert!(
            outcome.is_success(),
            "expected success outcome before reset failure"
        );
        env::set_var("AARDVARK_TEST_FORCE_RESET_FAILURE", "pool_drop");
    }

    let second_runtime_id = {
        let mut pooled = pool.checkout()?;
        let runtime_id = pooled
            .runtime()
            .runtime_id()
            .map(|id| id.to_owned())
            .expect("runtime id must be set");
        let outcome = run_main(
            pooled.runtime(),
            r#"
def main():
    return "ok"
"#,
        )?;
        assert!(
            outcome.is_success(),
            "expected fresh runtime to run successfully"
        );
        runtime_id
    };

    assert_ne!(
        first_runtime_id, second_runtime_id,
        "pool should allocate a new runtime after reset failure"
    );
    env::remove_var("AARDVARK_TEST_FORCE_RESET_FAILURE");
    Ok(())
}

fn verify_rawctx_adapter_roundtrip() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
import builtins
from js import globalThis as js

def main():
    inputs = builtins.__aardvark_rawctx_inputs
    payload = inputs["payload"]
    data = bytes(payload["data"])
    meta = payload["metadata"]
    assert meta["dtype"] == "u8"
    control = bytes(inputs["control"]["data"])
    assert control == b"control"
    js.__aardvarkPublishBuffer("echo", data, {"source": meta["source"]})
    return None
"#,
    );
    let session = runtime.prepare_session(bundle, "main:main")?;
    let payload_meta = RawCtxMetadata::new("u8")
        .with_shape(vec![11])
        .with_nullable(false);
    let payload_meta = payload_meta.with_extra(json!({"source": "rawctx-test"}))?;
    let control_meta = RawCtxMetadata::new("u8");
    let inputs = vec![
        RawCtxInput::new(
            "payload",
            Bytes::from_static(b"rawctx-bytes"),
            Some(payload_meta),
        )?,
        RawCtxInput::new(
            "control",
            Bytes::from_static(b"control"),
            Some(control_meta),
        )?,
    ];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "echo");
            assert_eq!(handle.length, b"rawctx-bytes".len());
            let bytes = handle
                .as_bytes()
                .expect("shared buffer should retain bytes");
            assert_eq!(bytes.as_ref(), b"rawctx-bytes");
            let source = handle
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("source"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(source, "rawctx-test");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }
    Ok(())
}

fn verify_rawctx_auto_wrapper() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(text, amount, blob, text_meta=None, blob_record=None):
    assert text == "hello world"
    assert isinstance(amount, float)
    assert abs(amount - 42.5) < 1e-9
    assert isinstance(blob, memoryview)
    assert bytes(blob) == b"raw-bytes"
    assert isinstance(text_meta, dict)
    assert text_meta.get("dtype") == "utf8"
    assert isinstance(blob_record, dict)
    assert blob_record.get("metadata", {}).get("dtype") == "binary"
    return blob
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "arg_text".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("text")
                .decoder("utf8")
                .metadata_arg("text_meta")
                .build(),
        ),
    });
    descriptor.inputs.push(FieldDescriptor {
        name: "arg_amount".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("amount")
                .decoder("float64")
                .option("struct_format", json!("<d"))
                .build(),
        ),
    });
    descriptor.inputs.push(FieldDescriptor {
        name: "arg_blob".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("blob")
                .raw_arg("blob_record")
                .decoder("memoryview")
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "response".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("response")
                .transform("memoryview")
                .metadata(json!({"dtype": "u8"}))
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;

    let text_meta = RawCtxMetadata::new("utf8")
        .with_nullable(false)
        .with_extra(json!({"dtype": "utf8"}))?;
    let blob_meta = RawCtxMetadata::new("binary").with_extra(json!({"dtype": "binary"}))?;

    let inputs = vec![
        RawCtxInput::new(
            "arg_text",
            Bytes::from_static(b"hello world"),
            Some(text_meta),
        )?,
        RawCtxInput::new(
            "arg_amount",
            Bytes::copy_from_slice(&42.5f64.to_le_bytes()),
            None,
        )?,
        RawCtxInput::new(
            "arg_blob",
            Bytes::from_static(b"raw-bytes"),
            Some(blob_meta),
        )?,
    ];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "response");
            let bytes = handle
                .as_bytes()
                .expect("shared buffer should retain bytes");
            assert_eq!(bytes.as_ref(), b"raw-bytes");
            let dtype = handle
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("dtype"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(dtype, "u8");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_rawctx_multi_output_publish() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler():
    return {
        "text": "hello-multi",
        "blob": b"\x00\x01\x02\x03",
    }
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.outputs.push(FieldDescriptor {
        name: "text".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("text-output")
                .transform("utf8")
                .python_transform("result['text']")
                .metadata(json!({"kind": "text"}))
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "blob".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("blob-output")
                .transform("memoryview")
                .python_transform("result['blob']")
                .metadata(json!({"kind": "bytes"}))
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let mut strategy = RawCtxInvocationStrategy::default();
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 2);
            let text = buffers[0]
                .as_bytes()
                .expect("text output should expose bytes");
            assert_eq!(text.as_ref(), b"hello-multi");
            let text_kind = buffers[0]
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(text_kind, "text");

            let blob = buffers[1]
                .as_bytes()
                .expect("blob output should expose bytes");
            assert_eq!(blob.as_ref(), b"\x00\x01\x02\x03");
            let blob_kind = buffers[1]
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("kind"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            assert_eq!(blob_kind, "bytes");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_rawctx_table_records() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table):
    assert table["city"] == ["Warsaw", "Krakow"]
    assert table["value"] == [1, 2]
    return len(table["value"])
"#,
    );

    let table_spec = RawCtxTableSpecBuilder::new()
        .column(RawCtxTableColumnBuilder::new("city"))
        .column(RawCtxTableColumnBuilder::new("value"))
        .build();

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("table")
                .decoder("json")
                .table(table_spec)
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let json_payload =
        Bytes::from_static(br#"[{"city":"Warsaw","value":1},{"city":"Krakow","value":2}]"#);
    let inputs = vec![RawCtxInput::new(
        "records",
        json_payload,
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!(2));
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_rawctx_table_missing_column() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table):
    return table
"#,
    );

    let table_spec = RawCtxTableSpecBuilder::new()
        .column(RawCtxTableColumnBuilder::new("city"))
        .column(RawCtxTableColumnBuilder::new("value").optional(false))
        .build();

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("table")
                .decoder("json")
                .table(table_spec)
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let json_payload = Bytes::from_static(br#"[{"city":"Warsaw"}]"#);
    let inputs = vec![RawCtxInput::new(
        "records",
        json_payload,
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            let traceback = info
                .traceback
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            assert!(
                traceback.contains("keyerror") && traceback.contains("value"),
                "traceback should mention missing 'value' column: {traceback}"
            );
        }
        other => panic!(
            "expected python exception for missing column, got {:?}",
            other
        ),
    }

    Ok(())
}

fn verify_rawctx_table_column_decoder() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table):
    assert table["payload"] == [b"row-1", b"row-2"]
    return table
"#,
    );

    let table_spec = RawCtxTableSpecBuilder::new()
        .column(RawCtxTableColumnBuilder::new("payload").decoder("base64"))
        .build();

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.outputs.push(FieldDescriptor {
        name: "table".to_owned(),
        type_tag: None,
        metadata: None,
    });
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("table")
                .decoder("json")
                .table(table_spec)
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let json_payload = Bytes::from_static(br#"[{"payload":"cm93LTE="},{"payload":"cm93LTI="}]"#);
    let inputs = vec![RawCtxInput::new(
        "records",
        json_payload,
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    assert!(matches!(outcome.status, OutcomeStatus::Success(_)));

    Ok(())
}

fn verify_rawctx_decoder_invalid_option() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(value):
    return value
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "value".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("value")
                .decoder("float64")
                .option("struct_format", json!("<invalid"))
                .build(),
        ),
    });
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "value",
        Bytes::from_static(&[0u8; 8]),
        Some(RawCtxMetadata::new("binary")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy);
    match outcome {
        Ok(_) => panic!("expected validation failure for invalid struct_format"),
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("struct_format"),
                "error should mention struct_format, got {message:?}"
            );
        }
    }
    Ok(())
}

fn verify_rawctx_decoder_invalid_base64_option() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(blob):
    return blob
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "payload".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("blob")
                .decoder("base64")
                .option("altchars", json!("?"))
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"cmFpbHVy"),
        Some(RawCtxMetadata::new("base64")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy);
    match outcome {
        Ok(_) => panic!("expected validation failure for invalid altchars"),
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("altchars"),
                "error should mention altchars, got {message:?}"
            );
        }
    }
    Ok(())
}

fn verify_rawctx_table_invalid_schema() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table):
    return table
"#,
    );

    let metadata = json!({
        "aardvark": {
            "rawctx": {
                "binding": {
                    "arg": "table",
                    "decoder": "json",
                    "table": {
                        "columns": [
                            {"name": "value", "dtype": "   ", "decoder": "float64"}
                        ]
                    }
                }
            }
        }
    });

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(metadata),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "records",
        Bytes::from_static(br#"[{"value":1}]"#),
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy);
    match outcome {
        Ok(_) => panic!("expected validation failure for invalid table dtype"),
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.contains("dtype"),
                "error should mention dtype, got {message:?}"
            );
        }
    }
    Ok(())
}

fn verify_rawctx_auto_wrapper_base64() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(blob):
    assert isinstance(blob, bytes)
    return blob
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "payload".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("blob")
                .decoder("base64")
                .option("validate", json!(true))
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "decoded".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("decoded")
                .transform("memoryview")
                .metadata(json!({"dtype": "u8"}))
                .return_behavior("none")
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let inputs = vec![RawCtxInput::new(
        "payload",
        Bytes::from_static(b"cmF3LWJ5dGVz"),
        Some(RawCtxMetadata::new("base64")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::SharedBuffers(buffers)) => {
            assert_eq!(buffers.len(), 1);
            let handle = &buffers[0];
            assert_eq!(handle.id, "decoded");
            let bytes = handle
                .as_bytes()
                .expect("shared buffer should retain bytes");
            assert_eq!(bytes.as_ref(), b"raw-bytes");
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_rawctx_auto_wrapper_missing_required() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(required):
    return required
"#,
    );

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "required".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("required")
                .decoder("utf8")
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let mut strategy = RawCtxInvocationStrategy::new(Vec::new());
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(info)) => {
            let traceback = info
                .traceback
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            assert!(
                traceback.contains("keyerror") && traceback.contains("required"),
                "traceback should mention missing 'required' input: {traceback}"
            );
        }
        other => panic!(
            "expected python exception for missing input, got {:?}",
            other
        ),
    }

    Ok(())
}

fn verify_rawctx_table_metadata() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table, meta, raw):
    assert meta["orient"] == "records"
    columns = meta["schema"]["columns"]
    assert columns["city"]["dtype"] == "string"
    assert columns["city"]["manifest"]["column"] == "City"
    assert columns["value"]["dtype"] == "float64"
    assert columns["value"]["metadata"]["unit"] == "PLN"
    assert columns["value"]["shape"] == [1]
    assert table["city"] == ["Warsaw", "Krakow"]
    assert table["value"] == [1.0, 2.0]
    return len(table["value"])
"#,
    );

    let table_spec = RawCtxTableSpecBuilder::new()
        .column(RawCtxTableColumnBuilder::utf8("city").manifest_column("City"))
        .column(
            RawCtxTableColumnBuilder::float64("value")
                .schema_metadata(json!({"unit": "PLN"}))
                .shape(vec![1]),
        )
        .build();

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("table")
                .decoder("json")
                .metadata_arg("meta")
                .raw_arg("raw")
                .table(table_spec)
                .build(),
        ),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let payload =
        Bytes::from_static(br#"[{"city":"Warsaw","value":1.0},{"city":"Krakow","value":2.0}]"#);
    let inputs = vec![RawCtxInput::new(
        "records",
        payload,
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!(2));
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_rawctx_table_manifest_derivation() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let bundle = bundle_with_main(
        r#"
def handler(table, meta):
    assert meta["orient"] == "records"
    columns = meta["schema"]["columns"]
    assert columns["city"]["manifest"]["column"] == "CityName"
    assert columns["value"]["nullable"] is True
    assert columns["value"]["metadata"]["unit"] == "PLN"
    assert columns["value"]["shape"] == [1]
    assert table["city"] == ["Warsaw", "Krakow"]
    assert table["value"] == [1, 2]
    return len(table["value"])
"#,
    );

    let table_manifest = json!({
        "orient": "records",
        "columns": [
            {
                "field": "city",
                "source": "CityName",
                "decoder": "utf8",
                "dtype": "string",
                "nullable": false
            },
            {
                "field": "value",
                "dtype": "float64",
                "nullable": true,
                "metadata": {"unit": "PLN"},
                "shape": [1]
            }
        ]
    });

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "records".to_owned(),
        type_tag: None,
        metadata: Some(json!({
            "aardvark": {
                "rawctx": {
                    "binding": {
                        "arg": "table",
                        "decoder": "json",
                        "metadata_arg": "meta",
                        "table_manifest": table_manifest
                    }
                }
            }
        })),
    });

    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;
    let payload =
        Bytes::from_static(br#"[{"city":"Warsaw","value":1},{"city":"Krakow","value":2}]"#);
    let inputs = vec![RawCtxInput::new(
        "records",
        payload,
        Some(RawCtxMetadata::new("json")),
    )?];
    let mut strategy = RawCtxInvocationStrategy::new(inputs);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;

    match &outcome.status {
        OutcomeStatus::Success(ResultPayload::Json(value)) => {
            assert_eq!(value, &json!(2));
        }
        other => panic!("unexpected payload variant: {:?}", other),
    }

    Ok(())
}

fn verify_after_invocation_reset_policy() -> Result<()> {
    let config = PyRuntimeConfig {
        reset_policy: ResetPolicy::AfterInvocation,
        ..PyRuntimeConfig::default()
    };
    let mut runtime = PyRuntime::new(config)?;

    let outcome = run_main(
        &mut runtime,
        r#"
def main():
    import builtins
    builtins.__aardvark_marker = "set"
    return "set"
"#,
    )?;
    assert!(outcome.is_success(), "expected success outcome");
    assert_eq!(payload_text(&outcome), "'set'");

    let outcome = run_main(
        &mut runtime,
        r#"
def main():
    import builtins
    return "stale" if hasattr(builtins, "__aardvark_marker") else "fresh"
"#,
    )?;
    assert!(outcome.is_success(), "expected success outcome");
    assert_eq!(payload_text(&outcome), "'fresh'");
    Ok(())
}

fn verify_python_exception_outcome() -> Result<()> {
    let mut runtime = PyRuntime::new(PyRuntimeConfig::default())?;
    let outcome = run_main(
        &mut runtime,
        r#"
def main():
    raise ValueError("boom")
"#,
    )?;

    match outcome.status {
        OutcomeStatus::Failure(FailureKind::PythonException(ref info)) => {
            assert!(info
                .typ
                .as_deref()
                .is_some_and(|t| t.contains("ValueError")));
            assert!(info.value.as_deref().is_some_and(|v| v.contains("boom")));
        }
        status => panic!("expected PythonException failure, got {:?}", status),
    }
    assert!(
        outcome
            .diagnostics
            .exception
            .as_ref()
            .and_then(|info| info.traceback.as_ref())
            .is_some(),
        "traceback should be captured in diagnostics"
    );
    Ok(())
}

#[test]
fn rawctx_metadata_validation() {
    let err = RawCtxInput::new(
        "payload",
        Bytes::from_static(b"data"),
        Some(RawCtxMetadata::new("   ")),
    );
    assert!(err.is_err());

    let err = RawCtxInput::new(
        "payload",
        Bytes::from_static(b"data"),
        Some(RawCtxMetadata::new("u8").with_shape(vec![0])),
    );
    assert!(err.is_err());
}

#[test]
fn rawctx_table_column_builder_presets() {
    let spec = RawCtxTableSpecBuilder::new()
        .column(RawCtxTableColumnBuilder::utf8("city").manifest_column("City"))
        .column(
            RawCtxTableColumnBuilder::float64("value")
                .schema_metadata(json!({"unit": "PLN"}))
                .shape(vec![1])
                .nullable(true),
        )
        .build();

    let json = serde_json::to_value(&spec).expect("serialize table spec");
    let columns = json
        .get("columns")
        .and_then(|value| value.as_array())
        .expect("columns array present");
    assert_eq!(columns.len(), 2);

    let city = columns
        .iter()
        .find(|value| value.get("name") == Some(&json!("city")))
        .expect("city column present");
    assert_eq!(city.get("dtype"), Some(&json!("string")));
    let manifest = city
        .get("manifest")
        .and_then(|value| value.get("column"))
        .and_then(|value| value.as_str())
        .expect("manifest column present");
    assert_eq!(manifest, "City");

    let value = columns
        .iter()
        .find(|col| col.get("name") == Some(&json!("value")))
        .expect("value column present");
    assert_eq!(value.get("dtype"), Some(&json!("float64")));
    assert_eq!(value.get("nullable"), Some(&json!(true)));
    assert_eq!(value.get("shape"), Some(&json!([1])));
    let metadata = value
        .get("metadata")
        .and_then(|meta| meta.get("unit"))
        .and_then(|value| value.as_str())
        .expect("unit metadata present");
    assert_eq!(metadata, "PLN");
}

#[test]
fn rawctx_binding_builder_serialization() {
    let mut metadata = RawCtxBindingBuilder::keyword("value")
        .decoder("utf8")
        .metadata_arg("meta")
        .default_value(json!("fallback"))
        .optional(true)
        .build();
    RawCtxBindingBuilder::new()
        .enabled(false)
        .merge_into(&mut metadata);

    let binding = metadata
        .get("aardvark")
        .and_then(|value| value.get("rawctx"))
        .and_then(|value| value.get("binding"))
        .and_then(|value| value.as_object())
        .expect("binding metadata present");
    assert_eq!(
        binding.get("arg").and_then(|value| value.as_str()).unwrap(),
        "value"
    );
    assert_eq!(
        binding
            .get("metadata_arg")
            .and_then(|value| value.as_str())
            .unwrap(),
        "meta"
    );
    assert_eq!(
        binding
            .get("default")
            .and_then(|value| value.as_str())
            .unwrap(),
        "fallback"
    );
    assert!(binding
        .get("optional")
        .and_then(|value| value.as_bool())
        .unwrap());
    let enabled = metadata
        .get("aardvark")
        .and_then(|value| value.get("rawctx"))
        .and_then(|value| value.get("enabled"))
        .and_then(|value| value.as_bool())
        .unwrap();
    assert!(!enabled, "expected binding to be disabled");
}

#[test]
fn rawctx_publish_builder_serialization() {
    let metadata = RawCtxPublishBuilder::new("result")
        .transform("utf8")
        .metadata(json!({"dtype": "u8"}))
        .when_none("error")
        .return_behavior("buffer")
        .python_transform("result.upper() if isinstance(result, str) else result")
        .encoding("utf-8")
        .build();

    let publish = metadata
        .get("aardvark")
        .and_then(|value| value.get("rawctx"))
        .and_then(|value| value.get("publish"))
        .and_then(|value| value.as_object())
        .expect("publish metadata present");
    assert_eq!(
        publish.get("id").and_then(|value| value.as_str()).unwrap(),
        "result"
    );
    assert_eq!(
        publish
            .get("transform")
            .and_then(|value| value.as_str())
            .unwrap(),
        "utf8"
    );
    assert_eq!(
        publish
            .get("when_none")
            .and_then(|value| value.as_str())
            .unwrap(),
        "error"
    );
    assert_eq!(
        publish
            .get("return")
            .and_then(|value| value.as_str())
            .unwrap(),
        "buffer"
    );
    assert!(
        publish.get("metadata").is_some(),
        "expected metadata to be preserved"
    );
}

fn verify_timeout_failure() -> Result<()> {
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

    // Ensure the runtime can execute normally after a CPU limit failure.
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

fn payload_text(outcome: &ExecutionOutcome) -> &str {
    match outcome.payload() {
        Some(ResultPayload::Text(value)) => value,
        other => panic!("expected text payload, got {:?}", other),
    }
}

fn bundle_with_main(code: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.py",
            FileOptions::default().compression_method(CompressionMethod::Stored),
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
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start main entry");
    writer
        .write_all(code.as_bytes())
        .expect("failed to write main entry");

    writer
        .start_file(
            "aardvark.manifest.json",
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start manifest entry");
    writer
        .write_all(manifest.as_bytes())
        .expect("failed to write manifest");

    let cursor = writer.finish().expect("failed to finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("failed to parse bundle")
}

fn bundle_with_js_main_and_manifest(code: &str, manifest: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(
            "main.js",
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start main.js entry");
    writer
        .write_all(code.as_bytes())
        .expect("failed to write main.js");

    writer
        .start_file(
            "aardvark.manifest.json",
            FileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .expect("failed to start manifest entry");
    writer
        .write_all(manifest.as_bytes())
        .expect("failed to write manifest");

    let cursor = writer.finish().expect("failed to finish bundle");
    Bundle::from_zip_bytes(cursor.into_inner()).expect("failed to parse bundle")
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
