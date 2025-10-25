//! Minimal CLI wrapper for the Pyodide runtime.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use aardvark_core::pyodide::{
    PYODIDE_CORE_ARCHIVE_NAME, PYODIDE_CORE_ARCHIVE_SHA256, PYODIDE_FULL_ARCHIVE_NAME,
    PYODIDE_FULL_ARCHIVE_SHA256, PYODIDE_RELEASE_BASE_URL, PYODIDE_VERSION,
};
use aardvark_core::{
    Bundle, ExecutionOutcome, FailureKind, InvocationDescriptor, JsonInvocationStrategy,
    OutcomeStatus, OverlayBlob, OverlayExport, PyRuntime, PyRuntimeConfig, ResultPayload,
};
use anyhow::{anyhow, bail, Context, Result};
use bzip2::read::BzDecoder;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map as JsonMap, Value};
use sha2::{Digest, Sha256};
use tar::Archive;
use tracing::{debug, info, info_span, warn, Level};
use tracing_subscriber::FmtSubscriber;
use ureq::{Agent, AgentBuilder};

const DEFAULT_ENTRYPOINT: &str = "main:handler";

/// CLI arguments.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct RunArgs {
    /// Path to a bundle archive.
    #[arg(short, long)]
    bundle: String,

    /// Entrypoint to execute (module:function).
    #[arg(short, long, default_value = "main:handler")]
    entrypoint: String,

    /// Additional Pyodide packages to load before executing the bundle.
    #[arg(short = 'p', long = "package", value_name = "NAME", action = clap::ArgAction::Append)]
    packages: Vec<String>,

    /// Path to a snapshot to preload before running (optional).
    #[arg(long, value_name = "PATH")]
    snapshot: Option<String>,

    /// Path to write a snapshot after packages are loaded (optional).
    #[arg(long = "write-snapshot", value_name = "PATH")]
    write_snapshot: Option<String>,

    /// Optional invocation descriptor describing entrypoint and budgets.
    #[arg(long = "descriptor", value_name = "PATH")]
    descriptor: Option<String>,

    /// Override wall-clock limit in milliseconds.
    #[arg(long = "limit-wall-ms")]
    limit_wall_ms: Option<u64>,

    /// Override heap limit in MiB.
    #[arg(long = "limit-heap-mb")]
    limit_heap_mb: Option<u64>,

    /// Path to JSON input the adapter should expose to Python (optional).
    #[arg(long = "json-input", value_name = "PATH")]
    json_input: Option<String>,
}

/// Asset management commands.
#[derive(Parser, Debug)]
#[command(
    name = "aardvark-cli assets",
    about = "Manage Pyodide asset caches",
    disable_help_subcommand = true
)]
struct AssetsCli {
    #[command(subcommand)]
    command: AssetsCommand,
}

#[derive(Subcommand, Debug)]
enum AssetsCommand {
    /// Download and stage a Pyodide package cache locally.
    Stage(AssetsStageArgs),
}

#[derive(clap::Args, Debug)]
struct AssetsStageArgs {
    /// Which Pyodide cache variant to stage.
    #[arg(long, value_enum, default_value = "full")]
    variant: StageVariant,

    /// Destination directory for the staged packages (defaults to .aardvark/pyodide/<version>).
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Use an existing archive instead of downloading the release tarball.
    #[arg(long, value_name = "PATH")]
    archive: Option<PathBuf>,

    /// Replace existing contents within the output directory.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum StageVariant {
    Core,
    Full,
}

impl StageVariant {
    fn archive_name(self) -> &'static str {
        match self {
            StageVariant::Core => PYODIDE_CORE_ARCHIVE_NAME,
            StageVariant::Full => PYODIDE_FULL_ARCHIVE_NAME,
        }
    }

    fn expected_sha(self) -> &'static str {
        match self {
            StageVariant::Core => PYODIDE_CORE_ARCHIVE_SHA256,
            StageVariant::Full => PYODIDE_FULL_ARCHIVE_SHA256,
        }
    }

    fn subdir(self) -> &'static str {
        match self {
            StageVariant::Core => "core",
            StageVariant::Full => "full",
        }
    }

    fn archive_url(self) -> String {
        format!(
            "{base}/{version}/{name}",
            base = PYODIDE_RELEASE_BASE_URL,
            version = PYODIDE_VERSION,
            name = self.archive_name()
        )
    }
}

impl fmt::Display for StageVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StageVariant::Core => "core",
            StageVariant::Full => "full",
        })
    }
}

fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let argv: Vec<OsString> = std::env::args_os().collect();
    if matches!(argv.get(1).map(|arg| arg.as_os_str()), Some(arg) if arg == OsStr::new("assets")) {
        let assets = AssetsCli::parse_from(argv.clone());
        return handle_assets_command(assets.command);
    }

    let args = RunArgs::parse_from(argv);
    let file = File::open(&args.bundle)?;
    let reader = BufReader::new(file);
    let bundle = Bundle::from_reader(reader)?;

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

    let mut runtime = PyRuntime::new(config)?;
    let session = runtime.prepare_session_with_descriptor(bundle, descriptor)?;

    let mut overlay_restored = false;
    let mut overlay_hydrated = false;

    if let Some(snapshot_path_str) = args.snapshot.as_deref() {
        let snapshot_path = Path::new(snapshot_path_str);
        let overlay_path = snapshot_overlay_path(snapshot_path);
        if overlay_path.exists() {
            match fs::read(&overlay_path) {
                Ok(meta_bytes) => match serde_json::from_slice::<Value>(&meta_bytes) {
                    Ok(metadata_value) => {
                        match collect_overlay_blobs(snapshot_path, &overlay_path, &metadata_value) {
                            Ok(blob_list) => {
                                match runtime.js_runtime().import_overlay(&meta_bytes, &blob_list) {
                                    Ok(()) => {
                                        let package_count = metadata_package_count(&metadata_value);
                                        overlay_restored = package_count > 0;
                                        let blob_bytes: usize =
                                            blob_list.iter().map(|blob| blob.bytes.len()).sum();
                                        tracing::info!(
                                            target = "aardvark::overlay",
                                            overlay.restored = true,
                                            overlay.packages = package_count,
                                            overlay.meta_bytes = meta_bytes.len(),
                                            overlay.blob_bytes = blob_bytes,
                                            overlay.blob_count = blob_list.len(),
                                            "overlay import completed"
                                        );
                                        info!(
                                        "restored overlay from {} (meta {} bytes, {} blobs, {} bytes total, {} packages)",
                                        overlay_path.display(),
                                        meta_bytes.len(),
                                        blob_list.len(),
                                        blob_bytes,
                                        package_count
                                    );
                                    }
                                    Err(error) => {
                                        warn!(
                                            "failed to import overlay {}: {error}",
                                            overlay_path.display()
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                warn!(
                                    "overlay blobs unavailable for {}: {error}",
                                    overlay_path.display()
                                );
                            }
                        }
                    }
                    Err(error) => {
                        warn!(
                            "failed to parse overlay metadata {}: {error}",
                            overlay_path.display()
                        );
                    }
                },
                Err(error) => {
                    warn!("failed to read overlay {}: {error}", overlay_path.display());
                }
            }
        }
        if !overlay_restored {
            runtime.js_runtime().prepare_dynlibs()?;
            tracing::info!(
                target = "aardvark::overlay",
                overlay.restored = false,
                "overlay import skipped"
            );
        }
    }

    if !overlay_restored && !packages.is_empty() {
        match hydrate_overlay_from_catalog(&mut runtime, &packages) {
            Ok(true) => {
                overlay_hydrated = true;
                runtime.js_runtime().prepare_dynlibs()?;
                let cache_root = overlay_cache_config(None).root;
                tracing::info!(
                    target = "aardvark::overlay",
                    overlay.event = "hydrate",
                    overlay.hydrated = true,
                    overlay.cache_root = %cache_root.display(),
                    overlay.packages = packages.len(),
                    "overlay hydrated from catalog"
                );
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
            }
            Err(error) => {
                warn!("snapshot collection failed: {error}");
            }
        }

        match runtime.js_runtime().export_overlay() {
            Ok(OverlayExport { metadata, blobs }) => {
                let overlay_path = snapshot_overlay_path(Path::new(path));
                if metadata.is_empty() || blobs.is_empty() {
                    if overlay_path.exists() {
                        if let Err(error) = fs::remove_file(&overlay_path) {
                            warn!(
                                "failed to remove old overlay {}: {error}",
                                overlay_path.display()
                            );
                        }
                    }
                } else {
                    let mut metadata_value =
                        serde_json::from_slice::<Value>(&metadata).unwrap_or_else(|_| json!({}));
                    if !metadata_value.is_object() {
                        metadata_value = json!({});
                    }
                    let snapshot_fs_path = Path::new(path);
                    let cache_config = overlay_cache_config(Some(snapshot_fs_path));
                    if let Err(error) = fs::create_dir_all(&cache_config.root) {
                        warn!(
                            "failed to create overlay blob dir {}: {error}",
                            cache_config.root.display()
                        );
                    }

                    let mut canonical_to_digest: HashMap<String, String> = HashMap::new();
                    let mut digest_entries: HashMap<String, OverlayBlobInfo> = HashMap::new();
                    for blob in &blobs {
                        let digest = blob
                            .digest
                            .clone()
                            .filter(|value| !value.is_empty())
                            .unwrap_or_else(|| {
                                let digest_bytes = Sha256::digest(&blob.bytes);
                                format!("sha256:{:x}", digest_bytes)
                            });
                        let file_name = overlay_blob_filename(&digest);
                        let target_path = cache_config.root.join(&file_name);
                        let mut stored = false;
                        if target_path.exists() {
                            match validate_blob_file(&target_path, &digest) {
                                Ok(()) => {
                                    stored = true;
                                }
                                Err(error) => {
                                    warn!(
                                        "existing overlay blob {} failed validation: {error}; rewriting",
                                        target_path.display()
                                    );
                                    let _ = fs::remove_file(&target_path);
                                }
                            }
                        }
                        if !stored {
                            if let Err(error) = fs::write(&target_path, &blob.bytes) {
                                warn!(
                                    "failed to write overlay tar {}: {error}",
                                    target_path.display()
                                );
                                continue;
                            }
                            if let Err(error) = validate_blob_bytes(&blob.bytes, &digest) {
                                warn!(
                                    "overlay blob {} failed digest check: {error}",
                                    target_path.display()
                                );
                                let _ = fs::remove_file(&target_path);
                                continue;
                            }
                        }
                        let size = fs::metadata(&target_path)
                            .map(|meta| meta.len() as usize)
                            .unwrap_or(blob.bytes.len());
                        canonical_to_digest.insert(blob.key.clone(), digest.clone());
                        digest_entries.insert(
                            digest.clone(),
                            OverlayBlobInfo {
                                path: target_path.clone(),
                                file_name: file_name.clone(),
                                size,
                            },
                        );
                    }

                    let eviction_stats = match enforce_overlay_cache_policy(&cache_config) {
                        Ok(stats) => stats,
                        Err(error) => {
                            warn!(
                                "overlay cache policy enforcement failed for {}: {error}",
                                cache_config.root.display()
                            );
                            OverlayEvictionStats::default()
                        }
                    };
                    let overlay_evicted = eviction_stats.evicted_files;
                    let overlay_evicted_bytes =
                        eviction_stats.evicted_bytes.min(u64::MAX as u128) as u64;

                    metadata_value
                        .as_object_mut()
                        .unwrap()
                        .insert("version".to_string(), json!(3));
                    metadata_value
                        .as_object_mut()
                        .unwrap()
                        .insert("format".to_string(), json!("catalog"));

                    let mut available_digests: HashSet<String> = HashSet::new();
                    for (digest, info) in &digest_entries {
                        if info.path.exists() {
                            available_digests.insert(digest.clone());
                        }
                    }

                    if let Some(packages) = metadata_value
                        .get_mut("packages")
                        .and_then(|value| value.as_array_mut())
                    {
                        for package in packages {
                            if let Some(obj) = package.as_object_mut() {
                                let canonical = obj
                                    .get("canonical")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or_default();
                                if let Some(digest) = canonical_to_digest.get(canonical) {
                                    if !available_digests.contains(digest) {
                                        warn!(
                                            "overlay digest {} missing on disk; skipping metadata update for {}",
                                            digest, canonical
                                        );
                                        continue;
                                    }
                                    obj.insert("digest".to_string(), json!(digest));
                                    obj.insert("blob".to_string(), json!(digest));
                                }
                            }
                        }
                    }

                    let mut blob_map = serde_json::Map::new();
                    let mut total_blob_bytes = 0usize;
                    let mut inserted_count = 0usize;
                    for (digest, info) in &digest_entries {
                        if !info.path.exists() {
                            warn!(
                                "overlay tar {} missing after cache enforcement; metadata may be incomplete",
                                info.path.display()
                            );
                            continue;
                        }
                        let size = fs::metadata(&info.path)
                            .map(|meta| meta.len() as usize)
                            .unwrap_or(info.size);
                        total_blob_bytes += size;
                        inserted_count += 1;
                        blob_map.insert(
                            digest.to_string(),
                            json!({
                                "digest": digest,
                                "blob": info.file_name,
                                "size": size
                            }),
                        );
                    }
                    metadata_value
                        .as_object_mut()
                        .unwrap()
                        .insert("blobs".to_string(), Value::Object(blob_map));

                    let mut index_updates = collect_index_updates(&metadata_value, &digest_entries);
                    if index_updates.is_empty() {
                        let dynlib_entries = parse_dynlibs_from_metadata(&metadata_value);
                        for (canonical, digest) in &canonical_to_digest {
                            if digest.is_empty() {
                                continue;
                            }
                            if let Some(info) = digest_entries.get(digest) {
                                if !info.path.exists() {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                            let dynlibs = if dynlib_entries.is_empty() {
                                None
                            } else {
                                Some(dynlib_entries.clone())
                            };
                            index_updates.insert(
                                canonicalize_package_name(canonical),
                                OverlayIndexEntry {
                                    digest: digest.clone(),
                                    blob: overlay_blob_filename(digest),
                                    dynlibs,
                                },
                            );
                        }
                    }
                    let index_update_count = index_updates.len();

                    tracing::info!(
                        target = "aardvark::overlay",
                        overlay.meta_bytes = metadata.len(),
                        overlay.blob_bytes = total_blob_bytes,
                        overlay.blob_count = inserted_count,
                        overlay.packages = canonical_to_digest.len(),
                        overlay.index_updates = index_update_count,
                        overlay.evicted = overlay_evicted,
                        overlay.evicted_bytes = overlay_evicted_bytes,
                        overlay.cache_root = %cache_config.root.display(),
                        "overlay export completed"
                    );

                    if let Err(error) = update_overlay_index(&cache_config.root, &index_updates) {
                        warn!(
                            "failed to update overlay index at {}: {error}",
                            cache_config.root.display()
                        );
                    }

                    match serde_json::to_vec_pretty(&metadata_value) {
                        Ok(serialized) => {
                            if let Err(error) = fs::write(&overlay_path, &serialized) {
                                warn!(
                                    "failed to write overlay {}: {error}",
                                    overlay_path.display()
                                );
                            } else {
                                info!(
                                    "wrote overlay metadata to {} ({} blobs, {} bytes total) [cache root: {}]",
                                    overlay_path.display(),
                                    inserted_count,
                                    total_blob_bytes,
                                    cache_config.root.display()
                                );
                            }
                        }
                        Err(error) => {
                            warn!(
                                "failed to serialize overlay metadata {}: {error}",
                                overlay_path.display()
                            );
                        }
                    }
                }
            }
            Err(error) => {
                warn!("failed to export overlay: {error}");
            }
        }
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

fn snapshot_overlay_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".overlay.json");
    PathBuf::from(os)
}

fn load_descriptor_from_path(path: &str) -> Result<InvocationDescriptor> {
    let bytes = fs::read(path).with_context(|| format!("failed to read descriptor {path}"))?;
    let descriptor = serde_json::from_slice::<InvocationDescriptor>(&bytes)
        .with_context(|| format!("failed to parse descriptor {path} as JSON"))?;
    Ok(descriptor)
}

fn load_json_input(path: &str) -> Result<Value> {
    let contents = if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read JSON input from stdin")?;
        buf
    } else {
        fs::read_to_string(path).with_context(|| format!("failed to read JSON input {path}"))?
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

fn env_var(key: &str) -> Option<String> {
    let value = env::var(key).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn env_var_os(key: &str) -> Option<OsString> {
    let value = env::var_os(key)?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn metadata_package_count(metadata: &Value) -> usize {
    metadata
        .get("packages")
        .and_then(|value| value.as_array())
        .map(|array| array.len())
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
struct OverlayCacheConfig {
    root: PathBuf,
    max_bytes: Option<u64>,
    max_age: Option<Duration>,
}

fn overlay_cache_config(snapshot_path: Option<&Path>) -> OverlayCacheConfig {
    let env_dir = env_var("AARDVARK_OVERLAY_CACHE_DIR");
    let root = env_dir
        .map(PathBuf::from)
        .or_else(|| {
            snapshot_path.and_then(|path| path.parent().map(|parent| parent.join("_overlay_cache")))
        })
        .unwrap_or_else(|| PathBuf::from("_overlay_cache"));

    let max_bytes =
        env_var("AARDVARK_OVERLAY_CACHE_MAX_BYTES").and_then(|value| parse_cache_size(&value));
    let max_age =
        env_var("AARDVARK_OVERLAY_CACHE_MAX_AGE").and_then(|value| parse_cache_duration(&value));

    OverlayCacheConfig {
        root,
        max_bytes,
        max_age,
    }
}

fn overlay_blob_dir(snapshot_path: &Path) -> PathBuf {
    overlay_cache_config(Some(snapshot_path)).root
}

fn parse_cache_size(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut idx = trimmed.len();
    for (i, ch) in trimmed.char_indices() {
        if !(ch.is_ascii_digit() || ch == '_' || ch == '.') {
            idx = i;
            break;
        }
    }
    let (number_part, unit_part) = trimmed.split_at(idx);
    let normalized = number_part.replace('_', "");
    if normalized.is_empty() {
        return None;
    }
    let number = normalized.parse::<f64>().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let unit = unit_part.trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let value = number * multiplier;
    if value > (u64::MAX as f64) {
        return None;
    }
    Some(value.round() as u64)
}

fn parse_cache_duration(input: &str) -> Option<Duration> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut idx = trimmed.len();
    for (i, ch) in trimmed.char_indices() {
        if !(ch.is_ascii_digit() || ch == '_' || ch == '.') {
            idx = i;
            break;
        }
    }
    let (number_part, unit_part) = trimmed.split_at(idx);
    let normalized = number_part.replace('_', "");
    if normalized.is_empty() {
        return None;
    }
    let number = normalized.parse::<f64>().ok()?;
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let unit = unit_part.trim().to_ascii_lowercase();
    let seconds = match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => number,
        "m" | "min" | "mins" | "minute" | "minutes" => number * 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => number * 3600.0,
        "d" | "day" | "days" => number * 86400.0,
        "w" | "week" | "weeks" => number * 604800.0,
        _ => return None,
    };
    if seconds > (u64::MAX as f64) {
        return None;
    }
    Some(Duration::from_secs(seconds.round() as u64))
}

fn overlay_blob_filename(digest: &str) -> String {
    let sanitized = digest.replace(':', "-");
    format!("{sanitized}.tar")
}

fn overlay_blob_path(snapshot_path: &Path, digest: &str) -> PathBuf {
    overlay_blob_dir(snapshot_path).join(overlay_blob_filename(digest))
}

fn normalize_sha256_digest(digest: &str) -> Option<String> {
    let trimmed = digest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_prefix = trimmed
        .strip_prefix("sha256:")
        .or_else(|| trimmed.strip_prefix("SHA256:"))
        .unwrap_or(trimmed);
    if without_prefix.len() != 64 {
        return None;
    }
    if !without_prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(without_prefix.to_ascii_lowercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{:x}", digest)
}

fn validate_blob_bytes(bytes: &[u8], digest: &str) -> Result<()> {
    if let Some(expected) = normalize_sha256_digest(digest) {
        let actual = sha256_hex(bytes);
        if actual != expected {
            bail!(
                "overlay blob digest mismatch (expected sha256:{expected}, found sha256:{actual})"
            );
        }
    }
    Ok(())
}

fn validate_blob_file(path: &Path, digest: &str) -> Result<()> {
    let Some(expected) = normalize_sha256_digest(digest) else {
        return Ok(());
    };
    let file = File::open(path)
        .with_context(|| format!("failed to open overlay blob {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 131_072];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to read overlay blob {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        bail!("overlay blob digest mismatch (expected sha256:{expected}, found sha256:{actual})");
    }
    Ok(())
}

struct CacheEntry {
    path: PathBuf,
    size: u64,
    modified_ns: u128,
}

struct OverlayBlobInfo {
    path: PathBuf,
    file_name: String,
    size: usize,
}

#[derive(Default, Debug, Clone, Copy)]
struct OverlayEvictionStats {
    evicted_files: usize,
    evicted_bytes: u128,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct OverlayIndex {
    #[serde(default)]
    packages: HashMap<String, OverlayIndexEntry>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
struct OverlayIndexEntry {
    #[serde(default)]
    digest: String,
    #[serde(default)]
    blob: String,
    #[serde(default)]
    dynlibs: Option<Vec<OverlayDynlibEntry>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
struct OverlayDynlibEntry {
    location: String,
    #[serde(rename = "relPath")]
    rel_path: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PyodideLockfile {
    #[serde(default)]
    packages: HashMap<String, LockPackage>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct LockPackage {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    depends: Vec<String>,
    #[serde(default)]
    install_dir: Option<String>,
    #[serde(default)]
    package_type: Option<String>,
    #[serde(default)]
    #[serde(alias = "digest")]
    sha256: Option<String>,
}

fn enforce_overlay_cache_policy(config: &OverlayCacheConfig) -> Result<OverlayEvictionStats> {
    if config.max_bytes.is_none() && config.max_age.is_none() {
        return Ok(OverlayEvictionStats::default());
    }
    let root = &config.root;
    if !root.exists() {
        return Ok(OverlayEvictionStats::default());
    }
    let now = SystemTime::now();
    let mut total_size: u128 = 0;
    let mut entries: Vec<CacheEntry> = Vec::new();
    let mut stats = OverlayEvictionStats::default();
    let iter = match fs::read_dir(root) {
        Ok(iter) => iter,
        Err(error) => {
            return Err(anyhow!(
                "failed to read overlay cache directory {}: {error}",
                root.display()
            ))
        }
    };
    for entry in iter {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!("failed to iterate overlay cache entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("tar"))
            != Some(true)
        {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            Err(error) => {
                warn!(
                    "failed to read metadata for overlay cache entry {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        let size = metadata.len();
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(max_age) = config.max_age {
            if let Ok(elapsed) = now.duration_since(modified) {
                if elapsed > max_age {
                    match fs::remove_file(&path) {
                        Ok(()) => {
                            stats.evicted_files += 1;
                            stats.evicted_bytes += size as u128;
                        }
                        Err(error) => {
                            warn!(
                                "failed to remove expired overlay blob {}: {error}",
                                path.display()
                            );
                        }
                    }
                    continue;
                }
            }
        }
        let modified_ns = modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos();
        total_size += size as u128;
        entries.push(CacheEntry {
            path,
            size,
            modified_ns,
        });
    }

    if let Some(limit) = config.max_bytes {
        let limit = limit as u128;
        if total_size > limit {
            entries.sort_by_key(|entry| entry.modified_ns);
            for entry in &entries {
                if total_size <= limit {
                    break;
                }
                match fs::remove_file(&entry.path) {
                    Ok(()) => {
                        stats.evicted_files += 1;
                        stats.evicted_bytes += entry.size as u128;
                        total_size = total_size.saturating_sub(entry.size as u128);
                    }
                    Err(error) => {
                        warn!(
                            "failed to remove overlay blob during eviction {}: {error}",
                            entry.path.display()
                        );
                        continue;
                    }
                }
            }
        }
    }
    Ok(stats)
}

fn canonicalize_package_name(name: &str) -> String {
    let mut canonical = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            canonical.push(lower);
            last_dash = false;
        } else if matches!(lower, '-' | '_' | '.') && !last_dash {
            canonical.push('-');
            last_dash = true;
        }
    }
    if canonical.ends_with('-') {
        canonical.pop();
    }
    canonical
}

fn load_lockfile() -> Result<Option<PyodideLockfile>> {
    let package_dir = match env_var_os("AARDVARK_PYODIDE_PACKAGE_DIR") {
        Some(value) => PathBuf::from(value),
        None => return Ok(None),
    };
    let lock_path = package_dir.join("pyodide-lock.json");
    if !lock_path.exists() {
        return Ok(None);
    }
    let file = File::open(&lock_path)
        .with_context(|| format!("failed to open lockfile {}", lock_path.display()))?;
    let reader = BufReader::new(file);
    let lock: PyodideLockfile = serde_json::from_reader(reader)
        .with_context(|| format!("failed to parse lockfile {}", lock_path.display()))?;
    Ok(Some(lock))
}

fn resolve_lockfile_requirements(requested: &[String], lockfile: &PyodideLockfile) -> Vec<String> {
    fn visit(
        name: &str,
        lockfile: &PyodideLockfile,
        seen: &mut HashSet<String>,
        ordered: &mut Vec<String>,
    ) {
        let canonical = canonicalize_package_name(name);
        if seen.contains(&canonical) {
            return;
        }
        let Some(meta) = lockfile.packages.get(&canonical) else {
            warn!("lockfile is missing metadata for {name}");
            return;
        };
        for dep in &meta.depends {
            visit(dep, lockfile, seen, ordered);
        }
        if seen.insert(canonical.clone()) {
            ordered.push(canonical);
        }
    }

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for name in requested {
        visit(name, lockfile, &mut seen, &mut ordered);
    }
    ordered
}

fn normalize_dynlib_location(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "usr" | "dynlib" | "/usr/lib" | "lib") {
        return Some("usr".to_string());
    }
    if matches!(
        lower.as_str(),
        "site" | "session" | "/session" | "/session/site-packages" | "site-packages"
    ) {
        return Some("site".to_string());
    }
    None
}

fn parse_dynlibs_from_metadata(metadata: &Value) -> Vec<OverlayDynlibEntry> {
    let mut result = Vec::new();
    let Some(array) = metadata.get("dynlibs").and_then(Value::as_array) else {
        return result;
    };
    let mut seen = HashSet::new();
    for entry in array {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(location_raw) = obj.get("location").and_then(Value::as_str) else {
            continue;
        };
        let Some(location) = normalize_dynlib_location(location_raw) else {
            continue;
        };
        let rel_path_value = obj
            .get("relPath")
            .or_else(|| obj.get("rel_path"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if rel_path_value.is_empty() {
            continue;
        }
        let rel_path = rel_path_value.replace('\\', "/");
        if rel_path.is_empty() {
            continue;
        }
        let key = format!("{}:{}", location, rel_path);
        if !seen.insert(key) {
            continue;
        }
        let path = obj
            .get("path")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        result.push(OverlayDynlibEntry {
            location,
            rel_path,
            path,
        });
    }
    result
}

fn overlay_index_from_metadata(cache_root: &Path) -> Result<OverlayIndex> {
    let mut index = OverlayIndex::default();
    let Some(parent) = cache_root.parent() else {
        return Ok(index);
    };
    if !parent.exists() {
        return Ok(index);
    }
    for entry in fs::read_dir(parent)? {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = match path.file_name().and_then(|name| name.to_str()) {
            Some(value) => value,
            None => continue,
        };
        if !file_name.ends_with(".overlay.json") {
            continue;
        }
        let file = match File::open(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let value: Value = match serde_json::from_reader(reader) {
            Ok(val) => val,
            Err(_) => continue,
        };
        let dynlibs = parse_dynlibs_from_metadata(&value);
        let dynlibs_opt = if dynlibs.is_empty() {
            None
        } else {
            Some(dynlibs)
        };
        if let Some(packages) = value.get("packages").and_then(Value::as_array) {
            for pkg in packages {
                let canonical = pkg
                    .get("canonical")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let digest = pkg
                    .get("digest")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if canonical.is_empty() || digest.is_empty() {
                    continue;
                }
                index.packages.insert(
                    canonical.to_string(),
                    OverlayIndexEntry {
                        digest: digest.to_string(),
                        blob: overlay_blob_filename(digest),
                        dynlibs: dynlibs_opt.clone(),
                    },
                );
            }
        }
    }
    Ok(index)
}

fn load_overlay_index(cache_root: &Path) -> Result<OverlayIndex> {
    let index_path = cache_root.join("index.json");
    if !index_path.exists() {
        return overlay_index_from_metadata(cache_root);
    }
    let file = File::open(&index_path)
        .with_context(|| format!("failed to open overlay index {}", index_path.display()))?;
    let reader = BufReader::new(file);
    let index = serde_json::from_reader(reader)
        .with_context(|| format!("failed to parse overlay index {}", index_path.display()))?;
    Ok(index)
}

fn save_overlay_index(cache_root: &Path, index: &OverlayIndex) -> Result<()> {
    let index_path = cache_root.join("index.json");
    let data = serde_json::to_vec_pretty(index)?;
    fs::write(&index_path, data)
        .with_context(|| format!("failed to write overlay index {}", index_path.display()))?;
    Ok(())
}

fn update_overlay_index(
    cache_root: &Path,
    updates: &HashMap<String, OverlayIndexEntry>,
) -> Result<()> {
    let mut index = load_overlay_index(cache_root)?;
    let mut changed = false;
    for (canonical, update_entry) in updates {
        if update_entry.digest.is_empty() || update_entry.blob.is_empty() {
            continue;
        }
        let canonical_key = canonicalize_package_name(canonical);
        let mut candidate = update_entry.clone();
        if candidate.blob.is_empty() {
            candidate.blob = overlay_blob_filename(&candidate.digest);
        }
        let entry_changed = match index.packages.get(&canonical_key) {
            Some(existing) => existing != &candidate,
            None => true,
        };
        if entry_changed {
            index.packages.insert(canonical_key.clone(), candidate);
            changed = true;
        }
    }
    index.packages.retain(|canonical, entry| {
        let exists = cache_root.join(&entry.blob).exists();
        if !exists {
            debug!(
            target = "aardvark::overlay",
                    overlay.cache_root = %cache_root.display(),
                    overlay.canonical = %canonical,
                    overlay.digest = %entry.digest,
                    "removing overlay index entry without blob"
                );
            changed = true;
        }
        exists
    });
    let index_path = cache_root.join("index.json");
    if !changed && index_path.exists() {
        return Ok(());
    }
    save_overlay_index(cache_root, &index)?;
    tracing::info!(
        target = "aardvark::overlay",
        overlay.index_entries = index.packages.len(),
        overlay.index_updates = updates.len(),
        overlay.cache_root = %cache_root.display(),
        "overlay catalog index updated"
    );
    Ok(())
}

fn collect_index_updates(
    metadata: &Value,
    digest_entries: &HashMap<String, OverlayBlobInfo>,
) -> HashMap<String, OverlayIndexEntry> {
    let mut updates = HashMap::new();
    let dynlib_entries = parse_dynlibs_from_metadata(metadata);
    let Some(packages) = metadata.get("packages").and_then(Value::as_array) else {
        return updates;
    };
    for package in packages {
        let Some(obj) = package.as_object() else {
            continue;
        };
        let canonical = match obj.get("canonical").and_then(Value::as_str) {
            Some(value) if !value.is_empty() => value,
            _ => continue,
        };
        let digest = match obj
            .get("digest")
            .and_then(Value::as_str)
            .or_else(|| obj.get("blob").and_then(Value::as_str))
        {
            Some(value) if !value.is_empty() => value,
            _ => continue,
        };
        let Some(info) = digest_entries.get(digest) else {
            continue;
        };
        if !info.path.exists() {
            continue;
        }
        let dynlibs = if dynlib_entries.is_empty() {
            None
        } else {
            Some(dynlib_entries.clone())
        };
        updates.insert(
            canonicalize_package_name(canonical),
            OverlayIndexEntry {
                digest: digest.to_string(),
                blob: info.file_name.clone(),
                dynlibs,
            },
        );
    }
    updates
}

fn hydrate_overlay_from_catalog(runtime: &mut PyRuntime, packages: &[String]) -> Result<bool> {
    let span = info_span!(
        target: "aardvark::overlay",
        "overlay.catalog.hydrate",
        overlay.packages_requested = packages.len() as u64,
        overlay.hydrate_packages = tracing::field::Empty,
        overlay.hydrate_blob_count = tracing::field::Empty,
        overlay.hydrate_blob_bytes = tracing::field::Empty,
        overlay.hydrate_duration_ms = tracing::field::Empty,
        overlay.hydrate_hit = tracing::field::Empty
    );
    let _guard = span.enter();
    let started = Instant::now();
    let mut metrics_blob_bytes: usize = 0;
    let mut metrics_blob_count: usize = 0;
    let mut metrics_packages: usize = 0;
    let mut metrics_cache_root: Option<PathBuf> = None;
    let mut eviction_stats: Option<OverlayEvictionStats> = None;
    let mut dynlib_accumulator: Vec<OverlayDynlibEntry> = Vec::new();
    let mut dynlib_seen: HashSet<String> = HashSet::new();

    let result = (|| -> Result<bool> {
        let Some(lockfile) = load_lockfile()? else {
            return Ok(false);
        };
        let requirements = resolve_lockfile_requirements(packages, &lockfile);
        if requirements.is_empty() {
            return Ok(false);
        }
        let cache_config = overlay_cache_config(None);
        metrics_cache_root = Some(cache_config.root.clone());
        if !cache_config.root.exists() {
            return Ok(false);
        }

        match enforce_overlay_cache_policy(&cache_config) {
            Ok(stats) => {
                if stats.evicted_files > 0 {
                    eviction_stats = Some(stats);
                }
            }
            Err(error) => {
                tracing::warn!(
                    target = "aardvark::overlay",
                    overlay.event = "evict",
                    overlay.error = %error,
                    overlay.cache_root = %cache_config.root.display(),
                    "overlay cache eviction during hydrate failed"
                );
            }
        }

        let prune_updates: HashMap<String, OverlayIndexEntry> = HashMap::new();
        if let Err(error) = update_overlay_index(&cache_config.root, &prune_updates) {
            tracing::warn!(
                target = "aardvark::overlay",
                overlay.event = "index-prune",
                overlay.error = %error,
                overlay.cache_root = %cache_config.root.display(),
                "overlay index prune failed during hydrate"
            );
        }

        let cache_root = cache_config.root.clone();
        let index = load_overlay_index(&cache_root)?;
        debug!(
            target = "aardvark::overlay",
            overlay.index_entries = index.packages.len(),
            overlay.cache_root = %cache_root.display(),
            "overlay catalog loaded"
        );
        let mut blobs = Vec::new();
        let mut package_entries = Vec::new();
        let mut blob_map = JsonMap::new();
        let mut total_bytes: usize = 0;
        let mut hydrated = Vec::new();
        for canonical in &requirements {
            let Some(meta) = lockfile.packages.get(canonical) else {
                continue;
            };
            let package_type = meta.package_type.as_deref().unwrap_or("package");
            if !matches!(package_type, "package" | "shared_library") {
                continue;
            }
            let Some(index_entry) = index.packages.get(canonical) else {
                continue;
            };
            if index_entry.digest.is_empty() || index_entry.blob.is_empty() {
                continue;
            }
            let digest = index_entry.digest.clone();
            let blob_filename = index_entry.blob.clone();
            let blob_path = cache_root.join(&blob_filename);
            if !blob_path.exists() {
                tracing::debug!(
                    target = "aardvark::overlay",
                    overlay.digest_miss = %digest,
                    overlay.cache_root = %cache_root.display(),
                    "catalog blob not present in cache"
                );
                continue;
            }
            let bytes = fs::read(&blob_path)
                .with_context(|| format!("failed to read overlay blob {}", blob_path.display()))?;
            let size = bytes.len();
            total_bytes += size;
            blobs.push(OverlayBlob {
                key: canonical.clone(),
                digest: Some(digest.clone()),
                bytes,
            });
            if let Some(entry_dynlibs) = &index_entry.dynlibs {
                for dynlib in entry_dynlibs {
                    let key = format!("{}:{}", dynlib.location, dynlib.rel_path);
                    if dynlib_seen.insert(key.clone()) {
                        dynlib_accumulator.push(dynlib.clone());
                    }
                }
            }
            package_entries.push(json!({
                "canonical": canonical,
                "name": meta.name.as_deref().unwrap_or(canonical),
                "digest": digest,
                "blob": digest,
                "mounts": [],
                "size": size,
            }));
            blob_map.insert(
                digest.clone(),
                json!({
                    "digest": digest,
                    "blob": blob_filename,
                    "size": size,
                }),
            );
            hydrated.push(canonical.clone());
        }

        if blobs.is_empty() {
            return Ok(false);
        }

        metrics_blob_bytes = total_bytes;
        metrics_blob_count = blobs.len();
        metrics_packages = hydrated.len();

        let mut metadata_obj = JsonMap::new();
        metadata_obj.insert("version".into(), Value::Number(3.into()));
        metadata_obj.insert("format".into(), Value::String("catalog".into()));
        metadata_obj.insert("packages".into(), Value::Array(package_entries));
        metadata_obj.insert("blobs".into(), Value::Object(blob_map));
        if !dynlib_accumulator.is_empty() {
            let dynlib_value = serde_json::to_value(&dynlib_accumulator)
                .context("failed to serialize overlay dynlibs")?;
            metadata_obj.insert("dynlibs".into(), dynlib_value);
        }
        let metadata = Value::Object(metadata_obj);
        let metadata_bytes = serde_json::to_vec(&metadata)?;
        runtime
            .js_runtime()
            .import_overlay(&metadata_bytes, &blobs)
            .context("failed to import overlay catalog")?;
        Ok(true)
    })();

    let elapsed = started.elapsed();
    span.record("overlay.hydrate_duration_ms", elapsed.as_millis() as u64);
    span.record("overlay.hydrate_blob_bytes", metrics_blob_bytes as u64);
    span.record("overlay.hydrate_packages", metrics_packages as u64);
    span.record("overlay.hydrate_blob_count", metrics_blob_count as u64);
    span.record("overlay.hydrate_hit", matches!(result, Ok(true)));

    if let (Some(stats), Some(root)) = (&eviction_stats, metrics_cache_root.as_ref()) {
        info!(
            target = "aardvark::overlay",
            overlay.event = "evict",
            overlay.evicted = stats.evicted_files,
            overlay.evicted_bytes = stats.evicted_bytes.min(u64::MAX as u128) as u64,
            overlay.cache_root = %root.display(),
            "overlay cache eviction applied during hydrate"
        );
    }

    if let (Ok(true), Some(root)) = (&result, metrics_cache_root.as_ref()) {
        info!(
            target = "aardvark::overlay",
            overlay.hydrated = true,
            overlay.packages = metrics_packages,
            overlay.blob_count = metrics_blob_count,
            overlay.blob_bytes = metrics_blob_bytes,
            overlay.duration_ms = elapsed.as_millis() as u64,
            overlay.cache_root = %root.display(),
            "overlay hydrated from catalog"
        );
    } else if let (Ok(false), Some(root)) = (&result, metrics_cache_root.as_ref()) {
        debug!(
            target = "aardvark::overlay",
            overlay.event = "hydrate",
            overlay.hydrated = false,
            overlay.cache_root = %root.display(),
            overlay.duration_ms = elapsed.as_millis() as u64,
            "overlay catalog miss"
        );
    }

    result
}

fn resolve_catalog_blob_path(
    snapshot_path: &Path,
    overlay_path: &Path,
    metadata: &Value,
    canonical: &str,
    digest: &str,
) -> PathBuf {
    let cache_root = overlay_cache_config(Some(snapshot_path)).root;
    let blobs_obj = metadata.get("blobs").and_then(Value::as_object);
    if let Some(map) = blobs_obj {
        if let Some(entry) = map.get(digest).or_else(|| {
            if canonical.is_empty() {
                None
            } else {
                map.get(canonical)
            }
        }) {
            if let Some(rel) = entry.get("blob").and_then(Value::as_str) {
                let rel_trimmed = rel.trim();
                if !rel_trimmed.is_empty() {
                    let candidate = Path::new(rel_trimmed);
                    if candidate.is_absolute() {
                        return candidate.to_path_buf();
                    }
                    let cache_candidate = cache_root.join(candidate);
                    if cache_candidate.exists() {
                        return cache_candidate;
                    }
                    if let Some(parent) = overlay_path.parent() {
                        return parent.join(candidate);
                    }
                    return cache_candidate;
                }
            }
            if let Some(digest_path) = entry
                .get("digest")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                let candidate = cache_root.join(overlay_blob_filename(digest_path));
                if candidate.exists() {
                    return candidate;
                }
                if let Some(parent) = overlay_path.parent() {
                    let fallback = parent.join(overlay_blob_filename(digest_path));
                    if fallback.exists() {
                        return fallback;
                    }
                }
                return candidate;
            }
        }
    }
    let fallback = cache_root.join(overlay_blob_filename(digest));
    if fallback.exists() {
        return fallback;
    }
    if let Some(parent) = overlay_path.parent() {
        let legacy = parent.join(overlay_blob_filename(digest));
        if legacy.exists() {
            return legacy;
        }
        if let Some(entry) = blobs_obj.and_then(|map| map.get(digest)) {
            if let Some(rel) = entry.get("blob").and_then(Value::as_str) {
                if let Some(parent) = overlay_path.parent() {
                    return parent.join(rel);
                }
            }
        }
    }
    cache_root.join(overlay_blob_filename(digest))
}

fn collect_overlay_blobs(
    snapshot_path: &Path,
    overlay_path: &Path,
    metadata: &Value,
) -> Result<Vec<OverlayBlob>> {
    let version = metadata.get("version").and_then(Value::as_u64).unwrap_or(2);
    let format = metadata
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("tar");

    if version >= 3 && format.eq_ignore_ascii_case("catalog") {
        let mut seen = HashSet::new();
        let mut blobs = Vec::new();
        if let Some(packages) = metadata.get("packages").and_then(Value::as_array) {
            for package in packages {
                let Some(obj) = package.as_object() else {
                    continue;
                };
                let canonical = obj
                    .get("canonical")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let digest = obj
                    .get("digest")
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("blob").and_then(Value::as_str))
                    .unwrap_or(canonical);
                if digest.is_empty() || !seen.insert(digest.to_string()) {
                    continue;
                }
                let blob_path = resolve_catalog_blob_path(
                    snapshot_path,
                    overlay_path,
                    metadata,
                    canonical,
                    digest,
                );
                if !blob_path.exists() {
                    tracing::warn!(
                        target = "aardvark::overlay",
                        overlay.digest_miss = %digest,
                        overlay.canonical = %canonical,
                        overlay.snapshot = %snapshot_path.display(),
                        "overlay blob missing"
                    );
                    bail!(
                        "overlay blob {} not found (expected for digest {})",
                        blob_path.display(),
                        digest
                    );
                }
                let bytes = fs::read(&blob_path).with_context(|| {
                    format!("failed to read overlay blob {}", blob_path.display())
                })?;
                validate_blob_bytes(&bytes, digest).with_context(|| {
                    format!(
                        "overlay blob {} failed digest verification",
                        blob_path.display()
                    )
                })?;
                blobs.push(OverlayBlob {
                    key: canonical.to_string(),
                    digest: Some(digest.to_string()),
                    bytes,
                });
            }
        }
        Ok(blobs)
    } else {
        let tar_value = metadata
            .get("tar")
            .ok_or_else(|| anyhow!("overlay metadata missing 'tar' section"))?;
        let digest = tar_value
            .get("digest")
            .and_then(Value::as_str)
            .unwrap_or("legacy");
        let blob_path = overlay_blob_path(snapshot_path, digest);
        if !blob_path.exists() {
            tracing::warn!(
            target = "aardvark::overlay",
                    overlay.digest_miss = %digest,
                    overlay.snapshot = %snapshot_path.display(),
                    "legacy overlay blob missing"
                );
            bail!("overlay tar {} not found", blob_path.display());
        }
        let bytes = fs::read(&blob_path)
            .with_context(|| format!("failed to read overlay tar {}", blob_path.display()))?;
        validate_blob_bytes(&bytes, digest).with_context(|| {
            format!(
                "overlay blob {} failed digest verification",
                blob_path.display()
            )
        })?;
        Ok(vec![OverlayBlob {
            key: digest.to_string(),
            digest: Some(digest.to_string()),
            bytes,
        }])
    }
}

fn handle_assets_command(command: AssetsCommand) -> Result<()> {
    match command {
        AssetsCommand::Stage(args) => stage_assets(args),
    }
}

fn stage_assets(args: AssetsStageArgs) -> Result<()> {
    let AssetsStageArgs {
        variant,
        output,
        archive,
        force,
    } = args;

    let output_dir = output.unwrap_or_else(default_stage_output_dir);
    let workspace = tempfile::tempdir().context("create staging workspace")?;
    let user_supplied = archive.is_some();
    let archive_path = match archive {
        Some(path) => path,
        None => download_variant_archive(variant, workspace.path())?,
    };

    if user_supplied {
        if let Err(error) = verify_sha256(&archive_path, variant.expected_sha()) {
            warn!(
                target = "aardvark::assets",
                variant = %variant,
                archive = %archive_path.display(),
                %error,
                "supplied Pyodide archive failed checksum; continuing"
            );
        }
    }

    info!(
        target = "aardvark::assets",
        variant = %variant,
        archive = %archive_path.display(),
        "unpacking Pyodide archive"
    );

    unpack_archive(&archive_path, workspace.path())?;
    let pyodide_root = find_pyodide_dir(workspace.path())
        .context("expected 'pyodide' directory inside archive")?;
    let version_dir = format!("v{PYODIDE_VERSION}");
    let variant_dir = pyodide_root
        .join("pyodide")
        .join(&version_dir)
        .join(variant.subdir());
    if !variant_dir.exists() {
        bail!(
            "archive missing variant directory {}",
            variant_dir.display()
        );
    }

    prepare_output_dir(&output_dir, force)?;
    copy_dir_recursive(&variant_dir, &output_dir)?;

    info!(
        target = "aardvark::assets",
        variant = %variant,
        output = %output_dir.display(),
        "staged Pyodide packages"
    );
    Ok(())
}

fn default_stage_output_dir() -> PathBuf {
    PathBuf::from(".aardvark/pyodide").join(PYODIDE_VERSION)
}

fn prepare_output_dir(output: &Path, force: bool) -> Result<()> {
    if output.exists() {
        if force {
            fs::remove_dir_all(output)
                .with_context(|| format!("failed to remove {}", output.display()))?;
        } else if output.read_dir()?.next().is_some() {
            bail!(
                "output directory {} is not empty; re-run with --force to overwrite",
                output.display()
            );
        }
    }
    fs::create_dir_all(output).with_context(|| format!("failed to create {}", output.display()))?;
    Ok(())
}

fn download_variant_archive(variant: StageVariant, workspace: &Path) -> Result<PathBuf> {
    fs::create_dir_all(workspace)
        .with_context(|| format!("failed to create {}", workspace.display()))?;
    let archive_path = workspace.join(variant.archive_name());
    let agent: Agent = AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(120))
        .build();
    let url = variant.archive_url();
    let mut response = agent
        .get(&url)
        .call()
        .with_context(|| format!("downloading {url}"))?
        .into_reader();
    let mut file = File::create(&archive_path)
        .with_context(|| format!("create {}", archive_path.display()))?;
    std::io::copy(&mut response, &mut file)
        .with_context(|| format!("write {}", archive_path.display()))?;
    verify_sha256(&archive_path, variant.expected_sha())?;
    Ok(archive_path)
}

fn unpack_archive(archive_path: &Path, workspace: &Path) -> Result<()> {
    let file =
        File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?;
    let decoder = BzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive.unpack(workspace).with_context(|| {
        format!(
            "unpack {} into {}",
            archive_path.display(),
            workspace.display()
        )
    })?;
    Ok(())
}

fn find_pyodide_dir(base: &Path) -> Option<PathBuf> {
    let mut stack = vec![base.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if path
                .file_name()
                .map(|name| name == "pyodide")
                .unwrap_or(false)
            {
                return Some(path);
            }
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} -> {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let actual = format!("{:x}", digest);
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use tempfile::tempdir;

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let original = env::var(key).ok();
            env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                env::set_var(self.key, value);
            } else {
                env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn parse_cache_size_supports_common_units() {
        assert_eq!(
            parse_cache_size("64M"),
            Some(64 * 1024 * 1024),
            "megabyte parsing failed"
        );
        assert_eq!(
            parse_cache_size("1.5G"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64),
            "gigabyte parsing failed"
        );
        assert_eq!(parse_cache_size(""), None);
        assert_eq!(parse_cache_size("abc"), None);
    }

    #[test]
    fn parse_cache_duration_supports_units() {
        assert_eq!(parse_cache_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_cache_duration("90m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_cache_duration(""), None);
        assert_eq!(parse_cache_duration("foo"), None);
    }

    #[test]
    fn overlay_cache_config_respects_env_override() {
        let dir = tempdir().unwrap();
        let _guard = EnvGuard::set("AARDVARK_OVERLAY_CACHE_DIR", dir.path());
        let config = overlay_cache_config(None);
        assert_eq!(config.root, dir.path());
    }

    #[test]
    fn canonicalize_package_name_normalizes() {
        assert_eq!(canonicalize_package_name("NumPy"), "numpy");
        assert_eq!(canonicalize_package_name("scikit-learn"), "scikit-learn");
        assert_eq!(canonicalize_package_name("Pandas_core"), "pandas-core");
        assert_eq!(canonicalize_package_name("foo.bar_baz"), "foo-bar-baz");
    }

    #[test]
    fn validate_blob_bytes_checks_digest() {
        let data = b"hello world";
        let digest = format!("sha256:{:x}", Sha256::digest(data));
        assert!(validate_blob_bytes(data, &digest).is_ok());
        let bad_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        assert!(validate_blob_bytes(data, bad_digest).is_err());
    }

    #[test]
    fn enforce_overlay_cache_policy_evicts_oldest_when_over_quota() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("sha256-first.tar");
        let second = dir.path().join("sha256-second.tar");
        fs::write(&first, vec![0u8; 2048]).unwrap();
        thread::sleep(Duration::from_millis(10));
        fs::write(&second, vec![0u8; 2048]).unwrap();
        let config = OverlayCacheConfig {
            root: dir.path().to_path_buf(),
            max_bytes: Some(2048),
            max_age: None,
        };
        let stats = enforce_overlay_cache_policy(&config).unwrap();
        assert!(
            !first.exists() && second.exists(),
            "expected oldest entry to be evicted when over quota"
        );
        assert!(stats.evicted_files >= 1, "expected at least one eviction");
        assert!(
            stats.evicted_bytes >= 2048,
            "expected eviction byte accounting"
        );
    }

    #[test]
    fn update_overlay_index_writes_manifest() {
        let dir = tempdir().unwrap();
        // seed blob so the index retains the entry
        fs::write(dir.path().join("sha256-dummy.tar"), vec![0u8; 16]).unwrap();
        let mut updates = HashMap::new();
        updates.insert(
            "dummy".to_string(),
            OverlayIndexEntry {
                digest: "sha256:dummy".to_string(),
                blob: overlay_blob_filename("sha256:dummy"),
                dynlibs: None,
            },
        );
        update_overlay_index(dir.path(), &updates).unwrap();
        let index_path = dir.path().join("index.json");
        assert!(index_path.exists(), "expected index.json to be written");
    }

    #[test]
    fn collect_index_updates_preserves_dynlibs() {
        let dir = tempdir().unwrap();
        let blob_name = overlay_blob_filename("sha256:pkg");
        let blob_path = dir.path().join(&blob_name);
        fs::write(&blob_path, b"blob").unwrap();
        let mut digest_entries = HashMap::new();
        digest_entries.insert(
            "sha256:pkg".to_string(),
            OverlayBlobInfo {
                path: blob_path,
                file_name: blob_name.clone(),
                size: 4,
            },
        );
        let metadata = json!({
            "packages": [
                {"canonical": "numpy", "digest": "sha256:pkg"}
            ],
            "dynlibs": [
                {"location": "usr", "relPath": "lib/libfoo.so", "path": "/usr/lib/libfoo.so"},
                {"location": "site", "relPath": "pkg.libs/libbar.so"}
            ]
        });
        let updates = collect_index_updates(&metadata, &digest_entries);
        let entry = updates
            .get("numpy")
            .expect("expected numpy entry in overlay updates");
        let dynlibs = entry
            .dynlibs
            .as_ref()
            .expect("expected dynlibs to be captured");
        assert_eq!(dynlibs.len(), 2, "expected two dynlib entries");
        assert_eq!(dynlibs[0].location, "usr");
        assert_eq!(dynlibs[0].rel_path, "lib/libfoo.so");
        assert_eq!(dynlibs[1].location, "site");
        assert_eq!(dynlibs[1].rel_path, "pkg.libs/libbar.so");
    }
}
