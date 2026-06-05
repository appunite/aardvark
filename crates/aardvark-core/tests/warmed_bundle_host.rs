use aardvark_core::{
    outcome::ResultPayload,
    persistent::{
        BundleArtifact, CleanupMode, IsolateConfig, PoolOptions, WarmedBundleHost,
        WarmedBundleHostOptions, WarmedBundleHostRegistry, WarmedBundleHostWarmup,
    },
    ExecutionOutcome, Result,
};
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[test]
fn pooled_zip_path_warms_before_live_call() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
counter = 0

def handler():
    global counter
    counter += 1
    return counter
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let host = WarmedBundleHost::from_bytes(
        &bytes,
        WarmedBundleHostOptions::pooled(PoolOptions {
            desired_size: 1,
            max_size: 1,
            isolate: IsolateConfig {
                cleanup: CleanupMode::None,
                ..IsolateConfig::default()
            },
            telemetry_interval: None,
            ..PoolOptions::default()
        })
        .with_warmup(WarmedBundleHostWarmup::default_call()),
    )?;

    let live = host.call_default()?;
    match live.payload() {
        Some(ResultPayload::Text(value)) => assert_eq!(value, "2"),
        other => panic!(
            "expected warmed pooled host text counter payload, got {:?}",
            other
        ),
    }
    Ok(())
}

#[test]
fn registry_reuses_prepared_zip_host() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
counter = 0

def handler():
    global counter
    counter += 1
    return counter
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let registry = WarmedBundleHostRegistry::new(
        WarmedBundleHostOptions::pooled(PoolOptions {
            desired_size: 1,
            max_size: 1,
            isolate: IsolateConfig {
                cleanup: CleanupMode::None,
                ..IsolateConfig::default()
            },
            telemetry_interval: None,
            ..PoolOptions::default()
        })
        .with_warmup(WarmedBundleHostWarmup::default_call()),
    );

    assert!(registry.is_empty());
    assert!(registry.ready_host_for_bytes(&bytes)?.is_none());
    assert!(!registry.is_ready_for_bytes(&bytes)?);
    let first = registry.prewarm_bytes(&bytes)?;
    let ready = registry
        .ready_host_for_bytes(&bytes)?
        .expect("prewarmed bundle should have a ready host");
    assert!(Arc::ptr_eq(&first, &ready));
    assert!(registry.is_ready_for_bytes(&bytes)?);
    let prewarmed_many = registry.prewarm_many_bytes([&bytes])?;
    assert!(Arc::ptr_eq(&first, &prewarmed_many[0]));
    let second = registry.host_for_bytes(&bytes)?;
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(registry.host_count(), 1);

    let first_live = first.call_default()?;
    let second_live = second.call_default()?;
    assert_eq!(payload_text(&first_live), "2");
    assert_eq!(payload_text(&second_live), "3");
    Ok(())
}

#[test]
fn registry_evicts_lru_ready_hosts() -> Result<()> {
    let first_bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler():
    return "first"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );
    let second_bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler():
    return "second"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let registry = WarmedBundleHostRegistry::with_cache_limits(
        WarmedBundleHostOptions::pooled(PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        }),
        1,
        1,
    );

    let first = registry.prewarm_bytes(&first_bytes)?;
    assert!(registry.is_ready_for_bytes(&first_bytes)?);
    assert_eq!(registry.host_count(), 1);
    assert_eq!(registry.artifact_count(), 1);

    let second = registry.prewarm_bytes(&second_bytes)?;
    assert!(registry.is_ready_for_bytes(&second_bytes)?);
    assert!(!registry.is_ready_for_bytes(&first_bytes)?);
    assert_eq!(registry.host_count(), 1);
    assert_eq!(registry.artifact_count(), 1);

    let first_again = registry.prewarm_bytes(&first_bytes)?;
    assert!(!Arc::ptr_eq(&first, &first_again));
    assert!(!Arc::ptr_eq(&second, &first_again));
    assert!(registry.is_ready_for_bytes(&first_bytes)?);
    assert!(!registry.is_ready_for_bytes(&second_bytes)?);
    assert_eq!(registry.host_count(), 1);
    assert_eq!(registry.artifact_count(), 1);
    Ok(())
}

#[test]
fn registry_removes_ready_hosts() -> Result<()> {
    let bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler():
    return "manual-remove"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let registry = WarmedBundleHostRegistry::with_cache_limits(
        WarmedBundleHostOptions::pooled(PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        }),
        2,
        2,
    );

    let first = registry.prewarm_bytes(&bytes)?;
    assert!(registry.is_ready_for_bytes(&bytes)?);
    assert_eq!(registry.host_count(), 1);
    assert_eq!(registry.artifact_count(), 1);

    let removed = registry
        .remove_for_bytes(&bytes)?
        .expect("ready host should be removed");
    assert!(Arc::ptr_eq(&first, &removed));
    assert!(!registry.is_ready_for_bytes(&bytes)?);
    assert_eq!(registry.host_count(), 0);
    assert_eq!(registry.artifact_count(), 0);
    assert!(registry.remove_for_bytes(&bytes)?.is_none());

    let artifact = BundleArtifact::from_bytes(&bytes)?;
    let second = registry.prewarm_artifact(artifact.clone())?;
    assert!(registry.is_ready_for_artifact(&artifact)?);
    let removed_again = registry
        .remove_for_artifact(&artifact)?
        .expect("artifact ready host should be removed");
    assert!(Arc::ptr_eq(&second, &removed_again));
    assert!(!registry.is_ready_for_artifact(&artifact)?);
    assert_eq!(registry.host_count(), 0);
    Ok(())
}

#[test]
fn registry_clear_flushes_ready_hosts() -> Result<()> {
    let first_bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler():
    return "clear-first"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );
    let second_bytes = bundle_bytes_with_main_and_manifest(
        r#"
def handler():
    return "clear-second"
"#,
        &json!({
            "schemaVersion": "1.0",
            "entrypoint": "main:handler",
            "runtime": {"language": "python"}
        })
        .to_string(),
    );

    let registry = WarmedBundleHostRegistry::with_cache_limits(
        WarmedBundleHostOptions::pooled(PoolOptions {
            desired_size: 0,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        }),
        4,
        4,
    );

    let first = registry.prewarm_bytes(&first_bytes)?;
    let second = registry.prewarm_bytes(&second_bytes)?;
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(registry.host_count(), 2);
    assert_eq!(registry.artifact_count(), 2);

    assert_eq!(registry.clear(), 2);
    assert!(registry.is_empty());
    assert_eq!(registry.artifact_count(), 0);
    assert!(!registry.is_ready_for_bytes(&first_bytes)?);
    assert!(!registry.is_ready_for_bytes(&second_bytes)?);
    assert_eq!(registry.clear(), 0);
    Ok(())
}

fn payload_text(outcome: &ExecutionOutcome) -> &str {
    match outcome.payload() {
        Some(ResultPayload::Text(value)) => value,
        other => panic!("expected text payload, got {:?}", other),
    }
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
