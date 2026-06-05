use aardvark_core::{
    invocation::{FieldDescriptor, InvocationDescriptor, InvocationLimits},
    outcome::ResultPayload,
    persistent::{
        BundleArtifact, BundlePool, BundlePoolRegistry, CleanupMode, IsolateConfig, PoolOptions,
    },
    strategy::{RawCtxBindingBuilder, RawCtxInput, RawCtxMetadata, RawCtxPublishBuilder},
    Bundle, ExecutionOutcome, Result,
};
use bytes::Bytes;
use serde_json::json;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn registry_zip_path_executes_handler() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
import builtins

def handler():
    payload = builtins.__aardvark_input
    return payload["value"] + 1
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );
    let registry = BundlePoolRegistry::new(PoolOptions {
        desired_size: 0,
        telemetry_interval: None,
        ..PoolOptions::default()
    })?;

    assert!(registry.is_empty());
    let prepared = registry.prepare_default_handler_for_bytes(&bytes)?;
    assert_eq!(registry.pool_count(), 1);

    let outcome = prepared.call_json(Some(json!({"value": 41})))?;

    match outcome.payload() {
        Some(ResultPayload::Json(value)) => assert_eq!(value, &json!(42)),
        other => panic!("expected json payload, got {:?}", other),
    }
    assert_eq!(prepared.pool().stats().invocations, 1);
    Ok(())
}

#[test]
fn handler_descriptor_applies_manifest_cpu_limit() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
def handler():
    return "ok"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"},
            "resources": {"cpu": {"defaultLimitMs": 12}}
        })
        .to_string(),
    );
    let artifact = BundleArtifact::from_bundle(bundle)?;
    let pool = BundlePool::from_artifact(
        artifact,
        PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        },
    )?;

    let handler = pool.prepare_default_handler()?;
    assert_eq!(handler.descriptor().limits.cpu_ms, Some(12));

    let override_descriptor =
        InvocationDescriptor::new("main:handler").with_limits(InvocationLimits {
            wall_ms: None,
            heap_mb: None,
            cpu_ms: Some(1_000),
        });
    let handler = pool.prepare_handler(Some(override_descriptor))?;
    assert_eq!(handler.descriptor().limits.cpu_ms, Some(12));

    Ok(())
}

#[test]
fn registry_retained_rawctx_owned_buffer() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler(value):
    data = bytes(value)
    return data[::-1]
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"},
            "resources": {"hostCapabilities": ["rawctx_buffers"]}
        })
        .to_string(),
    );
    let registry = BundlePoolRegistry::new(PoolOptions {
        desired_size: 1,
        telemetry_interval: None,
        ..PoolOptions::default()
    })?;
    let mut descriptor = InvocationDescriptor::new("main:handler").with_capture_stdio(false);
    descriptor.inputs.push(FieldDescriptor {
        name: "payload".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("value")
                .decoder("memoryview")
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "result".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("result")
                .transform("memoryview")
                .build(),
        ),
    });

    let prepared = registry.prepare_handler_for_bytes(&bytes, Some(descriptor))?;
    let first = prepared.call_rawctx(vec![RawCtxInput::from_vec(
        "payload",
        b"abcdef".to_vec(),
        Some(RawCtxMetadata::new("bytes")),
    )?])?;
    assert_eq!(single_shared_buffer_bytes(&first), b"fedcba");

    let second = prepared.call_rawctx(vec![RawCtxInput::from_vec(
        "payload",
        b"012345".to_vec(),
        Some(RawCtxMetadata::new("bytes")),
    )?])?;
    assert_eq!(single_shared_buffer_bytes(&second), b"543210");
    assert_eq!(prepared.pool().stats().invocations, 2);
    Ok(())
}

#[test]
fn registry_warm_all_json_executes_each_idle_isolate() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
import builtins

counter = 0

def handler():
    global counter
    counter += 1
    payload = builtins.__aardvark_input
    return {"counter": counter, "value": payload["value"]}
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );
    let registry = BundlePoolRegistry::new(PoolOptions {
        desired_size: 2,
        max_size: 2,
        isolate: IsolateConfig {
            cleanup: CleanupMode::None,
            ..IsolateConfig::default()
        },
        telemetry_interval: None,
        ..PoolOptions::default()
    })?;

    let prepared = registry.prepare_default_handler_for_bytes(&bytes)?;
    let outcomes = prepared.warm_all_json(Some(json!({"value": "warm"})))?;

    assert_eq!(outcomes.len(), 2);
    for outcome in &outcomes {
        match outcome.payload() {
            Some(ResultPayload::Json(value)) => {
                assert_eq!(value["counter"], json!(1));
                assert_eq!(value["value"], json!("warm"));
            }
            other => panic!("expected json payload from warm-all call, got {:?}", other),
        }
    }
    assert_eq!(prepared.pool().stats().invocations, 2);

    let live = prepared.call_json(Some(json!({"value": "live"})))?;
    match live.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["counter"], json!(2));
            assert_eq!(value["value"], json!("live"));
        }
        other => panic!("expected json payload from live call, got {:?}", other),
    }
    assert_eq!(prepared.pool().stats().invocations, 3);
    Ok(())
}

#[test]
fn registry_warm_all_json_starts_lazy_pool() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
import builtins

def handler():
    payload = builtins.__aardvark_input
    return payload["value"]
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );
    let registry = BundlePoolRegistry::new(PoolOptions {
        desired_size: 0,
        max_size: 1,
        telemetry_interval: None,
        ..PoolOptions::default()
    })?;

    let prepared = registry.prepare_default_handler_for_bytes(&bytes)?;
    assert_eq!(prepared.pool().stats().total, 0);

    let outcomes = prepared.warm_all_json(Some(json!({"value": "warm"})))?;

    assert_eq!(outcomes.len(), 1);
    match outcomes[0].payload() {
        Some(ResultPayload::Json(value)) => assert_eq!(value, &json!("warm")),
        other => panic!(
            "expected json payload from lazy warm-all call, got {:?}",
            other
        ),
    }
    let stats = prepared.pool().stats();
    assert_eq!(stats.total, 1);
    assert_eq!(stats.invocations, 1);
    Ok(())
}

#[test]
fn warm_call_executes_through_pool() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
import builtins

counter = 0

def handler():
    global counter
    counter += 1
    payload = getattr(builtins, "__aardvark_input", {})
    return {"counter": counter, "size": payload.get("size")}

def echo(value):
    return value.encode("utf-8")
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"},
            "resources": {"hostCapabilities": ["rawctx_buffers"]}
        })
        .to_string(),
    );

    let artifact = BundleArtifact::from_bundle(bundle)?;
    let mut options = PoolOptions::default();
    options.isolate.cleanup = CleanupMode::None;
    let pool = BundlePool::from_artifact(artifact, options)?;
    let json_handler = pool.prepare_default_handler()?;

    let warm = pool.warm_json(&json_handler, Some(json!({"size": 11})))?;
    match warm.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["counter"], json!(1));
            assert_eq!(value["size"], json!(11));
        }
        other => panic!("expected json payload from warm call, got {:?}", other),
    }

    let live = pool.call_json(&json_handler, Some(json!({"size": 23})))?;
    match live.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["counter"], json!(2));
            assert_eq!(value["size"], json!(23));
        }
        other => panic!("expected json payload from live call, got {:?}", other),
    }

    let mut descriptor = InvocationDescriptor::new("main:echo");
    descriptor.inputs.push(FieldDescriptor {
        name: "value".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("value")
                .decoder("utf8")
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "result".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("result")
                .transform("memoryview")
                .build(),
        ),
    });
    let rawctx_handler = pool.prepare_handler(Some(descriptor))?;

    let warm_rawctx = pool.warm_rawctx(
        &rawctx_handler,
        vec![RawCtxInput::new(
            "value",
            Bytes::from_static(b"warm"),
            Some(RawCtxMetadata::new("utf8")),
        )?],
    )?;
    assert_eq!(single_shared_buffer_bytes(&warm_rawctx), b"warm");

    let live_rawctx = pool.call_rawctx(
        &rawctx_handler,
        vec![RawCtxInput::new(
            "value",
            Bytes::from_static(b"live"),
            Some(RawCtxMetadata::new("utf8")),
        )?],
    )?;
    assert_eq!(single_shared_buffer_bytes(&live_rawctx), b"live");

    assert_eq!(pool.stats().invocations, 4);
    Ok(())
}

#[test]
fn handler_descriptor_mut_invalidates_rawctx_cache() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
def handler(value):
    return value.encode("utf-8")
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"},
            "resources": {"hostCapabilities": ["rawctx_buffers"]}
        })
        .to_string(),
    );

    let artifact = BundleArtifact::from_bundle(bundle)?;
    let mut options = PoolOptions::default();
    options.isolate.cleanup = CleanupMode::None;
    let pool = BundlePool::from_artifact(artifact, options)?;
    let handle = pool.handle();

    let mut descriptor = InvocationDescriptor::new("main:handler");
    descriptor.inputs.push(FieldDescriptor {
        name: "first".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("value")
                .decoder("utf8")
                .build(),
        ),
    });
    descriptor.outputs.push(FieldDescriptor {
        name: "result".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxPublishBuilder::new("result")
                .transform("memoryview")
                .build(),
        ),
    });

    let mut handler = handle.prepare_handler(Some(descriptor));
    pool.prewarm_handler(&handler)?;

    let first = pool.call_rawctx(
        &handler,
        vec![RawCtxInput::new(
            "first",
            Bytes::from_static(b"one"),
            Some(RawCtxMetadata::new("utf8")),
        )?],
    )?;
    assert_eq!(single_shared_buffer_bytes(&first), b"one");

    let descriptor = handler.descriptor_mut();
    descriptor.inputs.clear();
    descriptor.inputs.push(FieldDescriptor {
        name: "second".to_owned(),
        type_tag: None,
        metadata: Some(
            RawCtxBindingBuilder::keyword("value")
                .decoder("utf8")
                .build(),
        ),
    });

    let second = pool.call_rawctx(
        &handler,
        vec![RawCtxInput::new(
            "second",
            Bytes::from_static(b"two"),
            Some(RawCtxMetadata::new("utf8")),
        )?],
    )?;
    assert_eq!(single_shared_buffer_bytes(&second), b"two");

    Ok(())
}

#[test]
fn python_json_fresh_input_with_cleanup_none() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
import builtins

counter = 0

def handler():
    global counter
    counter += 1
    rawctx = getattr(builtins, "__aardvark_rawctx_inputs", None)
    if isinstance(rawctx, dict):
        return {"counter": counter, "path": "rawctx"}
    payload = getattr(builtins, "__aardvark_input", {})
    return {"counter": counter, "path": "json", "size": payload.get("size")}
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let artifact = BundleArtifact::from_bundle(bundle)?;
    let mut options = PoolOptions::default();
    options.isolate.cleanup = CleanupMode::None;
    let pool = BundlePool::from_artifact(artifact, options)?;
    let handler = pool.prepare_default_handler()?;

    let first = pool.call_json(&handler, Some(json!({"size": 11})))?;
    match first.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["counter"], json!(1));
            assert_eq!(value["path"], json!("json"));
            assert_eq!(value["size"], json!(11));
        }
        other => panic!("expected json payload from first call, got {:?}", other),
    }

    let second = pool.call_json(&handler, Some(json!({"size": 23})))?;
    match second.payload() {
        Some(ResultPayload::Json(value)) => {
            assert_eq!(value["counter"], json!(2));
            assert_eq!(value["path"], json!("json"));
            assert_eq!(value["size"], json!(23));
        }
        other => panic!("expected json payload from second call, got {:?}", other),
    }

    Ok(())
}

#[test]
fn lazy_generic_pool_starts_on_first_call() -> Result<()> {
    let bundle = bundle_with_main_and_manifest(
        r#"
def handler():
    return "lazy"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let artifact = BundleArtifact::from_bundle(bundle)?;
    let pool = BundlePool::from_artifact(
        artifact,
        PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        },
    )?;
    let initial = pool.stats();
    assert_eq!(initial.total, 0);
    assert_eq!(initial.idle, 0);

    let handler = pool.prepare_default_handler()?;
    let prepared = pool.stats();
    assert_eq!(prepared.total, 0);
    assert_eq!(prepared.idle, 0);

    let outcome = pool.call_default(&handler)?;
    assert_eq!(payload_text(&outcome), "'lazy'");
    let after_call = pool.stats();
    assert_eq!(after_call.total, 1);
    assert_eq!(after_call.idle, 1);
    assert_eq!(after_call.busy, 0);

    Ok(())
}

#[test]
fn lazy_prewarm_applies_to_first_spawned_isolate() -> Result<()> {
    let bundle = bundle_with_files_and_manifest(
        &[
            (
                "main.py",
                r#"
import sys

def handler():
    return "prewarmed" if "warm_target" in sys.modules else "cold"
"#,
            ),
            (
                "warm_target.py",
                r#"
def warm():
    return "loaded"
"#,
            ),
        ],
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let artifact = BundleArtifact::from_bundle(bundle)?;
    let pool = BundlePool::from_artifact(
        artifact,
        PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        },
    )?;

    let _warm_handler =
        pool.prepare_handler(Some(InvocationDescriptor::new("warm_target:warm")))?;
    let prepared = pool.stats();
    assert_eq!(prepared.total, 0);
    assert_eq!(prepared.idle, 0);

    let default_handler = pool.prepare_default_handler()?;
    let outcome = pool.call_default(&default_handler)?;
    assert_eq!(payload_text(&outcome), "'prewarmed'");

    Ok(())
}

fn payload_text(outcome: &ExecutionOutcome) -> &str {
    match outcome.payload() {
        Some(ResultPayload::Text(value)) => value,
        other => panic!("expected text payload, got {:?}", other),
    }
}

fn single_shared_buffer_bytes(outcome: &ExecutionOutcome) -> &[u8] {
    match outcome.payload() {
        Some(ResultPayload::SharedBuffers(buffers)) if buffers.len() == 1 => buffers[0]
            .as_slice()
            .expect("shared buffer should retain bytes"),
        other => panic!("expected one shared buffer, got {:?}", other),
    }
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

fn bundle_with_files_and_manifest(files: &[(&str, &str)], manifest: &str) -> Bundle {
    use std::io::Cursor;

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    for (path, code) in files {
        writer
            .start_file(
                *path,
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .expect("failed to start bundle entry");
        writer
            .write_all(code.as_bytes())
            .expect("failed to write bundle entry");
    }

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
