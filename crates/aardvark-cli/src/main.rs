//! CLI wrapper for running Aardvark bundles and managing local runtime assets.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Instant;

use aardvark_core::{
    Bundle, BundleArtifact, BundleLimits, CleanupMode, ExecutionOutcome, FailureKind,
    InvocationDescriptor, IsolateConfig, JsonInvocationStrategy, OutcomeStatus, PoolOptions,
    PyRuntime, PyRuntimeConfig, ResultPayload, WarmedBundleHostOptions, WarmedBundleHostRegistry,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::Value;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

mod args;
mod assets;
mod overlay;
mod read_limited;

use args::{Cli, CliCommand, ExecutionBackend, RunArgs};
use assets::handle_assets_command;
use overlay::{
    export_snapshot_overlay, hydrate_overlay_from_catalog, restore_snapshot_overlay,
    write_snapshot_metadata,
};
use read_limited::{read_file_limited, read_utf8_limited};

pub(crate) const DEFAULT_ENTRYPOINT: &str = "main:handler";
const MAX_DESCRIPTOR_BYTES: u64 = 1024 * 1024;
const MAX_JSON_INPUT_BYTES: u64 = 16 * 1024 * 1024;

fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let cli = Cli::parse();
    if let Some(command) = cli.command {
        let CliCommand::Assets(assets) = command;
        return handle_assets_command(assets.command);
    }

    let args = cli.run;
    let bundle = args
        .bundle
        .as_deref()
        .ok_or_else(|| anyhow!("--bundle is required unless a subcommand is used"))?;
    let bundle_path = Path::new(bundle);
    let bundle_limits = BundleLimits::default();
    let bundle = read_bundle_from_path(bundle_path, bundle_limits)?;

    let mut config = PyRuntimeConfig::default();
    if let Some(path) = args.snapshot.as_deref() {
        config.snapshot.load_from = Some(path.into());
    }
    if let Some(path) = args.write_snapshot.as_deref() {
        config.snapshot.save_to = Some(path.into());
    }
    let write_snapshot_path = config.snapshot.save_to.clone();

    let mut descriptor = if let Some(path) = args.descriptor.as_deref() {
        load_descriptor_from_path(path)?
    } else {
        InvocationDescriptor::trivial(&args.entrypoint)
    };
    if let Some(limit) = args.limit_wall_ms {
        descriptor.limits.wall_ms = Some(limit);
    }
    if let Some(limit) = args.limit_heap_mb {
        descriptor.limits.heap_mb = Some(limit);
    }

    // Inspect bundle manifest (if present) for defaults.
    let manifest = bundle.manifest()?;

    for profile_dir in &args.pyodide_profile_dirs {
        let (profile, path) = parse_pyodide_profile_dir(profile_dir)?;
        config.set_pyodide_distribution_profile_dir(profile, path)?;
    }
    let pyodide_profile = args.pyodide_profile.as_deref().or_else(|| {
        manifest
            .as_ref()
            .and_then(|m| m.pyodide_distribution_profile())
    });
    if let Some(profile) = pyodide_profile {
        config.set_pyodide_distribution_profile(profile)?;
    }

    let mut packages = args.packages.clone();
    if packages.is_empty() {
        if let Some(m) = &manifest {
            packages = m.packages().to_vec();
        }
    }

    if args.descriptor.is_none() && args.entrypoint == DEFAULT_ENTRYPOINT {
        if let Some(m) = &manifest {
            descriptor = InvocationDescriptor::new(m.entrypoint().to_string());
        }
    }

    if !packages.is_empty() && config.pyodide_dist_dir().is_none() {
        bail!(
            "Pyodide package loading requires AARDVARK_PYODIDE_DIST_DIR or PyRuntimeConfig::with_pyodide_dist_dir"
        );
    }

    if args.execution_backend == ExecutionBackend::WarmedHost {
        let bundle_bytes = read_bundle_bytes(bundle_path, bundle_limits)?;
        return run_warmed_host_backend(&args, bundle_bytes, config, descriptor);
    }

    let mut runtime = PyRuntime::new(config)?;
    let active_fingerprint = runtime
        .pyodide_compatibility_fingerprint()
        .map(str::to_string);
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;

    let mut overlay_restored = false;
    let mut overlay_hydrated = false;

    if let Some(snapshot_path_str) = args.snapshot.as_deref() {
        let snapshot_path = Path::new(snapshot_path_str);
        overlay_restored =
            restore_snapshot_overlay(&mut runtime, snapshot_path, active_fingerprint.as_deref())?;
        if !overlay_restored {
            runtime.js_runtime().prepare_dynlibs()?;
        }
    }

    if !overlay_restored && !packages.is_empty() {
        match hydrate_overlay_from_catalog(&mut runtime, &packages, active_fingerprint.as_deref()) {
            Ok(true) => {
                overlay_hydrated = true;
                runtime.js_runtime().prepare_dynlibs()?;
            }
            Ok(false) => {
                tracing::info!(
                    target = "aardvark::overlay",
                    overlay.event = "hydrate",
                    overlay.hydrated = false,
                    "overlay hydration skipped"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target = "aardvark::overlay",
                    overlay.event = "hydrate",
                    overlay.error = %error,
                    "overlay catalog hydration failed"
                );
            }
        }
    }

    if (overlay_restored || overlay_hydrated) && !packages.is_empty() {
        info!(
            "overlay already contains installed packages; skipping explicit load of {:?}",
            packages
        );
        packages.clear();
    }

    if !packages.is_empty() {
        info!("loading packages: {:?}", packages);
        runtime.js_runtime().load_packages(&packages)?;
    }

    runtime.js_runtime().prepare_dynlibs()?;

    if let Some(path) = write_snapshot_path.as_ref() {
        runtime.js_runtime().prepare_dynlibs()?;
        match runtime.js_runtime().collect_snapshot() {
            Ok(bytes) => {
                std::fs::write(path, &bytes)?;
                info!("wrote snapshot to {}", path.display());
                if let Some(fingerprint) = active_fingerprint.as_deref() {
                    if let Err(error) = write_snapshot_metadata(path, fingerprint) {
                        warn!(
                            "failed to write snapshot metadata for {}: {error}",
                            path.display()
                        );
                    }
                }
            }
            Err(error) => {
                warn!("snapshot collection failed: {error}");
            }
        }

        export_snapshot_overlay(&mut runtime, path, active_fingerprint.as_deref());
    }

    info!(
        "bundle ready with entrypoint '{}' ({} files)",
        session.entrypoint(),
        session.bundle().entries().len()
    );
    for (path, size) in session.manifest() {
        println!("{path} ({size} bytes)");
    }

    let json_payload = match args.json_input.as_deref() {
        Some(path) => Some(load_json_input(path)?),
        None => None,
    };
    let mut strategy = JsonInvocationStrategy::new(json_payload);
    let outcome = runtime.run_session_with_strategy(&session, &mut strategy)?;
    render_outcome(&outcome)?;

    Ok(())
}

fn read_bundle_from_path(path: &Path, limits: BundleLimits) -> Result<Bundle> {
    let file = open_limited_bundle_file(path, limits)?;
    Bundle::from_reader(file).with_context(|| format!("failed to parse bundle {}", path.display()))
}

fn read_bundle_bytes(path: &Path, limits: BundleLimits) -> Result<Vec<u8>> {
    let file = open_limited_bundle_file(path, limits)?;
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(len.min(8 * 1024 * 1024) as usize);
    let mut reader = BufReader::new(file).take(limits.max_archive_bytes.saturating_add(1));
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read bundle {}", path.display()))?;
    if bytes.len() as u64 > limits.max_archive_bytes {
        bail!(
            "bundle {} exceeded the configured archive limit of {} bytes while reading",
            path.display(),
            limits.max_archive_bytes
        );
    }
    Ok(bytes)
}

fn open_limited_bundle_file(path: &Path, limits: BundleLimits) -> Result<File> {
    let file =
        File::open(path).with_context(|| format!("failed to open bundle {}", path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("failed to stat bundle {}", path.display()))?
        .len();
    if len > limits.max_archive_bytes {
        bail!(
            "bundle {} is {} bytes, above the configured archive limit of {} bytes",
            path.display(),
            len,
            limits.max_archive_bytes
        );
    }
    Ok(file)
}

fn run_warmed_host_backend(
    args: &RunArgs,
    bundle_bytes: Vec<u8>,
    config: PyRuntimeConfig,
    descriptor: InvocationDescriptor,
) -> Result<()> {
    if args.snapshot.is_some() || args.write_snapshot.is_some() {
        bail!(
            "the warmed-host execution backend does not support --snapshot or --write-snapshot; use --execution-backend direct"
        );
    }
    if !args.packages.is_empty() {
        bail!(
            "the warmed-host execution backend uses bundle manifest packages; explicit --package is only supported by --execution-backend direct"
        );
    }

    let artifact = BundleArtifact::from_bytes(&bundle_bytes)?;
    let registry = WarmedBundleHostRegistry::new(
        WarmedBundleHostOptions::pooled(PoolOptions {
            isolate: IsolateConfig {
                runtime: config,
                cleanup: CleanupMode::SharedBuffersOnly,
            },
            desired_size: 1,
            max_size: 1,
            telemetry_interval: None,
            ..PoolOptions::default()
        })
        .with_descriptor(descriptor),
    );

    let started = Instant::now();
    let prewarmed = registry.prewarm_artifact(artifact.clone())?;
    let ready = registry
        .ready_host_for_artifact(&artifact)?
        .ok_or_else(|| {
            anyhow!("warmed-host registry did not publish a ready host after prewarm")
        })?;
    if !std::sync::Arc::ptr_eq(&prewarmed, &ready) {
        bail!("warmed-host registry returned a different ready host after prewarm");
    }

    info!(
        backend = "warmed-host",
        setup_ms = started.elapsed().as_secs_f64() * 1000.0,
        "warmed host ready"
    );
    for entry in ready.artifact().bundle().entries() {
        println!("{} ({} bytes)", entry.path(), entry.contents().len());
    }

    let json_payload = match args.json_input.as_deref() {
        Some(path) => Some(load_json_input(path)?),
        None => None,
    };
    let outcome = ready.call_json(json_payload)?;
    render_outcome(&outcome)?;
    Ok(())
}

fn parse_pyodide_profile_dir(value: &str) -> Result<(&str, &str)> {
    let Some((profile, path)) = value.split_once('=') else {
        bail!("--pyodide-profile-dir expects NAME=PATH, got '{value}'");
    };
    let profile = profile.trim();
    let path = path.trim();
    if profile.is_empty() || path.is_empty() {
        bail!("--pyodide-profile-dir expects non-empty NAME=PATH, got '{value}'");
    }
    Ok((profile, path))
}

fn load_descriptor_from_path(path: &str) -> Result<InvocationDescriptor> {
    let bytes = read_file_limited(Path::new(path), MAX_DESCRIPTOR_BYTES, "descriptor")?;
    let descriptor = serde_json::from_slice::<InvocationDescriptor>(&bytes)
        .with_context(|| format!("failed to parse descriptor {path} as JSON"))?;
    Ok(descriptor)
}

fn load_json_input(path: &str) -> Result<Value> {
    let contents = if path == "-" {
        let stdin = std::io::stdin();
        read_utf8_limited(stdin.lock(), MAX_JSON_INPUT_BYTES, "JSON input from stdin")?
    } else {
        let bytes = read_file_limited(Path::new(path), MAX_JSON_INPUT_BYTES, "JSON input")?;
        String::from_utf8(bytes).with_context(|| format!("JSON input {path} is not UTF-8"))?
    };
    let value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse JSON input from {path}"))?;
    Ok(value)
}

fn render_outcome(outcome: &ExecutionOutcome) -> Result<()> {
    let diagnostics = &outcome.diagnostics;
    if !diagnostics.stdout.is_empty() {
        println!("\n--- stdout ---\n{}", diagnostics.stdout);
    }
    if !diagnostics.stderr.is_empty() {
        eprintln!("\n--- stderr ---\n{}", diagnostics.stderr);
    }

    match &outcome.status {
        OutcomeStatus::Success(payload) => match payload {
            ResultPayload::Json(value) => println!(
                "\n--- json result ---\n{}",
                serde_json::to_string_pretty(value)?
            ),
            ResultPayload::Text(text) => println!("\n--- result ---\n{text}"),
            ResultPayload::Binary(_) => println!("\n--- result ---\n<binary payload>"),
            ResultPayload::SharedBuffers(buffers) => {
                println!("\n--- shared buffers ---");
                for handle in buffers {
                    let mut line = format!("  {} ({} bytes)", handle.id, handle.length);
                    if let Some(meta) = &handle.metadata {
                        let meta_json = serde_json::to_string(meta)?;
                        line.push_str(&format!(" metadata={meta_json}"));
                    }
                    println!("{line}");
                }
            }
            ResultPayload::None => {}
        },
        OutcomeStatus::Failure(kind) => {
            println!("\n--- failure ---");
            match kind {
                FailureKind::PythonException(exc) => {
                    println!(
                        "python exception: {}: {}",
                        exc.typ.as_deref().unwrap_or("<unknown>"),
                        exc.value.as_deref().unwrap_or("")
                    );
                    if let Some(tb) = exc.traceback.as_deref() {
                        println!("{tb}");
                    }
                }
                FailureKind::AdapterError { message } => println!("adapter error: {message}"),
                FailureKind::TimeoutExceeded { requested_ms } => {
                    println!("timeout exceeded ({} ms)", requested_ms)
                }
                FailureKind::CpuLimitExceeded {
                    requested_ms,
                    used_ms,
                } => {
                    println!(
                        "cpu limit exceeded (budget {} ms, used {} ms)",
                        requested_ms, used_ms
                    )
                }
                FailureKind::HeapLimitExceeded { requested_mb } => {
                    println!("heap limit exceeded ({} MiB)", requested_mb)
                }
                FailureKind::Other { message } => println!("failure: {message}"),
            }
        }
    }

    if let Some(exc) = diagnostics.exception.as_ref() {
        if outcome.is_success() {
            println!(
                "\n--- exception ---\n{}: {}",
                exc.typ.as_deref().unwrap_or("<unknown>"),
                exc.value.as_deref().unwrap_or("")
            );
            if let Some(tb) = exc.traceback.as_deref() {
                println!("{tb}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{scan_binary_feature_bytes, validate_force_removal_target};
    use aardvark_core::pyodide_distribution::{PackageFeatures, DISTRIBUTION_MANIFEST};
    use std::env;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn binary_feature_scan_detects_openblas_without_wasm() {
        let mut features = PackageFeatures::default();
        scan_binary_feature_bytes(Some("libopenblas.so"), b"native payload", &mut features)
            .unwrap();
        assert!(features.openblas);
        assert!(!features.wasm_simd);
        assert_eq!(features.wasm_modules, 0);
    }

    #[test]
    fn parse_pyodide_profile_dir_requires_name_and_path() {
        assert_eq!(
            parse_pyodide_profile_dir(" blas = /tmp/blas ").unwrap(),
            ("blas", "/tmp/blas")
        );
        assert!(parse_pyodide_profile_dir("blas").is_err());
        assert!(parse_pyodide_profile_dir("blas=").is_err());
        assert!(parse_pyodide_profile_dir("=/tmp/blas").is_err());
    }

    #[test]
    fn force_output_guard_rejects_unmarked_custom_directory() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("custom");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("not-a-distribution.txt"), b"data").unwrap();

        assert!(validate_force_removal_target(&output).is_err());
    }

    #[test]
    fn force_output_guard_allows_existing_distribution_directory() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("existing-dist");
        fs::create_dir(&output).unwrap();
        fs::write(output.join(DISTRIBUTION_MANIFEST), b"{}").unwrap();

        validate_force_removal_target(&output).unwrap();
    }

    #[test]
    fn force_output_guard_rejects_current_directory() {
        let cwd = env::current_dir().unwrap();

        assert!(validate_force_removal_target(&cwd).is_err());
    }

    #[test]
    fn wasm_simd_detector_requires_parsed_simd_operator() {
        let mut scalar_features = PackageFeatures::default();
        scan_binary_feature_bytes(
            Some("scalar.wasm"),
            &minimal_scalar_wasm(),
            &mut scalar_features,
        )
        .unwrap();
        assert_eq!(scalar_features.wasm_modules, 1);
        assert!(!scalar_features.wasm_simd);

        let mut simd_features = PackageFeatures::default();
        scan_binary_feature_bytes(Some("simd.wasm"), &minimal_simd_wasm(), &mut simd_features)
            .unwrap();
        assert_eq!(simd_features.wasm_modules, 1);
        assert!(simd_features.wasm_simd);
    }

    fn minimal_scalar_wasm() -> Vec<u8> {
        use wasm_encoder::{
            CodeSection, Function, FunctionSection, Instruction, Module, TypeSection, ValType,
        };

        let mut types = TypeSection::new();
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
        let mut functions = FunctionSection::new();
        functions.function(0);
        let mut func = Function::new([]);
        func.instruction(&Instruction::I32Const(1))
            .instruction(&Instruction::Drop)
            .instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&func);
        let mut module = Module::new();
        module.section(&types).section(&functions).section(&code);
        module.finish()
    }

    fn minimal_simd_wasm() -> Vec<u8> {
        use wasm_encoder::{
            CodeSection, Function, FunctionSection, Instruction, Module, TypeSection, ValType,
        };

        let mut types = TypeSection::new();
        types
            .ty()
            .function(Vec::<ValType>::new(), Vec::<ValType>::new());
        let mut functions = FunctionSection::new();
        functions.function(0);
        let mut func = Function::new([]);
        func.instruction(&Instruction::V128Const(0))
            .instruction(&Instruction::Drop)
            .instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&func);
        let mut module = Module::new();
        module.section(&types).section(&functions).section(&code);
        module.finish()
    }
}
