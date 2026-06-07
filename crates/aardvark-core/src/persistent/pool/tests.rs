use super::*;
use crate::BUNDLE_MANIFEST_BASENAME;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

fn artifact_with_pyodide_profile(profile: &str) -> Arc<BundleArtifact> {
    BundleArtifact::from_bytes(bundle_bytes_with_pyodide_profile(
        Some(profile),
        b"def handler():\n    return 1\n",
    ))
    .unwrap()
}

fn bundle_bytes_with_pyodide_profile(profile: Option<&str>, code: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut bytes);
        let mut writer = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("main.py", options).unwrap();
        writer.write_all(code).unwrap();
        writer
            .start_file(BUNDLE_MANIFEST_BASENAME, options)
            .unwrap();
        let pyodide = profile
            .map(|profile| format!(r#", "pyodide": {{"profile": "{profile}"}}"#))
            .unwrap_or_default();
        writer
            .write_all(
                format!(
                    r#"{{
                        "schemaVersion": "1.0",
                        "entrypoint": "main:handler",
                        "runtime": {{
                            "language": "python"{pyodide}
                        }}
                    }}"#
                )
                .as_bytes(),
            )
            .unwrap();
        writer.finish().unwrap();
    }
    bytes
}

fn lazy_pool_options() -> PoolOptions {
    PoolOptions {
        desired_size: 0,
        telemetry_interval: None,
        ..PoolOptions::default()
    }
}

#[test]
fn pool_applies_bundle_pyodide_distribution_profile_before_isolate_creation() -> Result<()> {
    let artifact = artifact_with_pyodide_profile("blas");
    let mut options = lazy_pool_options();
    let blas_path = PathBuf::from("/tmp/aardvark-blas-dist");
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", blas_path.clone())?;

    let pool = BundlePool::from_artifact(artifact, options)?;
    let options = pool.inner.options.lock();
    assert_eq!(
        options
            .isolate
            .runtime
            .pyodide_distribution_profile
            .as_deref(),
        Some("blas")
    );
    assert_eq!(
        options.isolate.runtime.pyodide_dist_dir.as_deref(),
        Some(Path::new("/tmp/aardvark-blas-dist"))
    );
    Ok(())
}

#[test]
fn pool_rejects_unregistered_bundle_pyodide_distribution_profile() {
    let artifact = artifact_with_pyodide_profile("blas");
    let options = lazy_pool_options();

    let Err(err) = BundlePool::from_artifact(artifact, options) else {
        panic!("pool should reject unregistered profile");
    };
    assert!(matches!(err, PyRunnerError::Validation(_)));
}

#[test]
fn registry_reuses_pool_for_same_bundle_and_profile() -> Result<()> {
    let bytes = bundle_bytes_with_pyodide_profile(
        Some("blas"),
        b"def handler():\n    return 'same-profile'\n",
    );
    let artifact = BundleArtifact::from_bytes(&bytes)?;
    let mut options = lazy_pool_options();
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
    let registry = BundlePoolRegistry::new(options)?;
    let key = BundlePoolKey::from_artifact(&artifact);

    let first = registry.pool_for_artifact(artifact)?;
    let second = registry.pool_for_bytes(&bytes)?;

    assert_eq!(registry.pool_count(), 1);
    assert!(registry.get(&key).is_some());
    assert!(Arc::ptr_eq(&first.inner, &second.inner));
    Ok(())
}

#[test]
fn registry_caches_artifact_for_repeated_bundle_bytes() -> Result<()> {
    let bytes = bundle_bytes_with_pyodide_profile(
        Some("blas"),
        b"def handler():\n    return 'cached-artifact'\n",
    );
    let mut options = lazy_pool_options();
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
    let registry = BundlePoolRegistry::new(options)?;

    let first = registry.pool_for_bytes(&bytes)?;
    let second = registry.pool_for_bytes(&bytes)?;

    assert_eq!(registry.inner.artifacts.lock().len(), 1);
    assert_eq!(registry.pool_count(), 1);
    assert!(Arc::ptr_eq(&first.inner, &second.inner));
    Ok(())
}

#[test]
fn registry_caches_prepared_handler_for_repeated_bundle_bytes() -> Result<()> {
    let bytes = bundle_bytes_with_pyodide_profile(
        Some("blas"),
        b"def handler():\n    return 'cached-handler'\n",
    );
    let mut options = lazy_pool_options();
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
    let registry = BundlePoolRegistry::new(options)?;

    let first = registry.prepare_default_handler_for_bytes(&bytes)?;
    let second = registry.prepare_default_handler_for_bytes(&bytes)?;

    assert_eq!(registry.inner.artifacts.lock().len(), 1);
    assert_eq!(registry.inner.handlers.lock().len(), 1);
    assert_eq!(registry.pool_count(), 1);
    assert!(Arc::ptr_eq(&first.pool.inner, &second.pool.inner));
    assert!(Arc::ptr_eq(&first.handler, &second.handler));
    Ok(())
}

#[test]
fn registry_remove_evicts_cached_prepared_handlers() -> Result<()> {
    let bytes = bundle_bytes_with_pyodide_profile(
        Some("blas"),
        b"def handler():\n    return 'evict-handler'\n",
    );
    let artifact = BundleArtifact::from_bytes(&bytes)?;
    let mut options = lazy_pool_options();
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
    let registry = BundlePoolRegistry::new(options)?;
    let key = BundlePoolKey::from_artifact(&artifact);

    let first = registry.prepare_default_handler_for_bytes(&bytes)?;
    assert_eq!(registry.inner.handlers.lock().len(), 1);

    assert!(registry.remove(&key).is_some());
    assert_eq!(registry.pool_count(), 0);
    assert_eq!(registry.inner.handlers.lock().len(), 0);

    let second = registry.prepare_default_handler_for_bytes(&bytes)?;
    assert_eq!(registry.inner.handlers.lock().len(), 1);
    assert!(!Arc::ptr_eq(&first.pool.inner, &second.pool.inner));
    assert!(!Arc::ptr_eq(&first.handler, &second.handler));
    Ok(())
}

#[test]
fn registry_separates_pools_by_bundle_profile() -> Result<()> {
    let blas = artifact_with_pyodide_profile("blas");
    let tensor = artifact_with_pyodide_profile("tensor");
    let mut options = lazy_pool_options();
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("blas", "/tmp/aardvark-blas-dist")?;
    options
        .isolate
        .runtime
        .set_pyodide_distribution_profile_dir("tensor", "/tmp/aardvark-tensor-dist")?;
    let registry = BundlePoolRegistry::new(options)?;

    let blas_pool = registry.pool_for_artifact(blas.clone())?;
    let tensor_pool = registry.pool_for_artifact(tensor.clone())?;
    let blas_key = BundlePoolKey::from_artifact(&blas);
    let tensor_key = BundlePoolKey::from_artifact(&tensor);

    assert_eq!(registry.pool_count(), 2);
    assert_eq!(blas_key.pyodide_distribution_profile(), Some("blas"));
    assert_eq!(tensor_key.pyodide_distribution_profile(), Some("tensor"));
    assert!(!Arc::ptr_eq(&blas_pool.inner, &tensor_pool.inner));
    Ok(())
}

#[test]
fn registry_drops_failed_creation_slot_for_unregistered_profile() -> Result<()> {
    let artifact = artifact_with_pyodide_profile("blas");
    let registry = BundlePoolRegistry::new(lazy_pool_options())?;

    let Err(err) = registry.pool_for_artifact(artifact.clone()) else {
        panic!("registry should reject unregistered profile");
    };

    assert!(matches!(err, PyRunnerError::Validation(_)));
    assert!(registry.is_empty());

    let Err(err) = registry.pool_for_artifact(artifact) else {
        panic!("registry should retry and reject unregistered profile");
    };
    assert!(matches!(err, PyRunnerError::Validation(_)));
    assert!(registry.is_empty());
    Ok(())
}
