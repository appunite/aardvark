use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use bzip2::read::BzDecoder;
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::Archive;
use tempfile::TempDir;

const DEFAULT_VERSION: &str = "0.28.2";

#[derive(Parser, Debug)]
#[command(
    name = "cargo aardvark",
    version,
    author,
    about = "Aardvark workspace utility commands"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Download and verify Pyodide assets into a local cache.
    FetchPyodide(FetchArgs),
}

#[derive(Args, Debug)]
struct FetchArgs {
    /// Pyodide version to download.
    #[arg(long, default_value = DEFAULT_VERSION)]
    version: String,

    /// Runtime bundle to download (core or full).
    #[arg(long, value_enum, default_value_t = ArtifactVariant::Core)]
    variant: ArtifactVariant,

    /// Additional artifacts to download (comma separated).
    #[arg(
        long = "extra",
        value_enum,
        value_delimiter = ',',
        default_values_t = Vec::<ExtraArtifact>::new()
    )]
    extras: Vec<ExtraArtifact>,

    /// Destination directory for the cache (defaults to ./.aardvark/pyodide).
    #[arg(long)]
    dest: Option<PathBuf>,

    /// Override the download base URL (useful for mirrors).
    #[arg(long)]
    mirror: Option<String>,

    /// Force re-download even if the cache looks valid.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum ArtifactVariant {
    Core,
    Full,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum ExtraArtifact {
    #[clap(alias = "static-libs")]
    StaticLibraries,
    #[clap(alias = "xbuildenv")]
    XbuildEnv,
}

#[derive(Debug, Clone)]
struct ArtifactDescriptor {
    version: &'static str,
    label: ArtifactLabel,
    filename: &'static str,
    sha256: &'static str,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ArtifactLabel {
    Core,
    Full,
    StaticLibraries,
    XbuildEnv,
}

impl ArtifactLabel {
    fn install_dir(self) -> &'static str {
        match self {
            ArtifactLabel::Core => "core",
            ArtifactLabel::Full => "full",
            ArtifactLabel::StaticLibraries => "static-libraries",
            ArtifactLabel::XbuildEnv => "xbuildenv",
        }
    }
}

const ARTIFACTS: &[ArtifactDescriptor] = &[
    ArtifactDescriptor {
        version: "0.28.2",
        label: ArtifactLabel::Full,
        filename: "pyodide-0.28.2.tar.bz2",
        sha256: "31021174e8fdc9556c17e9d435e20d9c07f203ac542d9161ca3b8d9d5d04e7e7",
        size: 351_939_017,
    },
    ArtifactDescriptor {
        version: "0.28.2",
        label: ArtifactLabel::Core,
        filename: "pyodide-core-0.28.2.tar.bz2",
        sha256: "c9f6dd067d119e50850849f7428e3c636ecbc2684a0d2ff992f3bd48a1062b6c",
        size: 5_333_235,
    },
    ArtifactDescriptor {
        version: "0.28.2",
        label: ArtifactLabel::StaticLibraries,
        filename: "static-libraries-0.28.2.tar.bz2",
        sha256: "dc2cea5ea8da6a6e3fbb7a3dbf356f4c1c50e59810d3e1bf1d6fe76496257e4e",
        size: 15_241_044,
    },
    ArtifactDescriptor {
        version: "0.28.2",
        label: ArtifactLabel::XbuildEnv,
        filename: "xbuildenv-0.28.2.tar.bz2",
        sha256: "5fe6766bb9bdcf238b93271ee915250c17df7f33ef87026d07b1ea2fce8c054b",
        size: 5_765_504,
    },
];

#[derive(Debug, Serialize, Deserialize)]
struct CacheMetadata {
    filename: String,
    version: String,
    sha256: String,
    downloaded_at: String,
    source_url: String,
    size: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::FetchPyodide(args) => fetch_pyodide(args),
    }
}

fn fetch_pyodide(args: FetchArgs) -> Result<()> {
    let FetchArgs {
        version,
        variant,
        extras,
        dest,
        mirror,
        force,
    } = args;

    let mut selections = Vec::new();
    selections.push(lookup_artifact(&version, ArtifactLabel::from(variant))?);
    for extra in extras {
        selections.push(lookup_artifact(&version, ArtifactLabel::from(extra))?);
    }

    let cache_root = dest.unwrap_or(default_cache_dir()?);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    for descriptor in selections {
        download_and_extract(
            &client,
            &cache_root,
            &version,
            descriptor,
            mirror.as_deref(),
            force,
        )?;
    }

    println!(
        "Pyodide {} assets ready under {}",
        version,
        cache_root.join(&version).display()
    );
    Ok(())
}

fn lookup_artifact(version: &str, label: ArtifactLabel) -> Result<&'static ArtifactDescriptor> {
    ARTIFACTS
        .iter()
        .find(|entry| entry.version == version && entry.label == label)
        .context(format!(
            "no artifact metadata for version {} ({:?})",
            version, label
        ))
}

impl From<ArtifactVariant> for ArtifactLabel {
    fn from(value: ArtifactVariant) -> Self {
        match value {
            ArtifactVariant::Core => ArtifactLabel::Core,
            ArtifactVariant::Full => ArtifactLabel::Full,
        }
    }
}

impl From<ExtraArtifact> for ArtifactLabel {
    fn from(value: ExtraArtifact) -> Self {
        match value {
            ExtraArtifact::StaticLibraries => ArtifactLabel::StaticLibraries,
            ExtraArtifact::XbuildEnv => ArtifactLabel::XbuildEnv,
        }
    }
}

fn default_cache_dir() -> Result<PathBuf> {
    Ok(std::env::current_dir()?.join(".aardvark").join("pyodide"))
}

fn download_and_extract(
    client: &Client,
    cache_root: &Path,
    version: &str,
    artifact: &ArtifactDescriptor,
    mirror: Option<&str>,
    force: bool,
) -> Result<()> {
    let version_dir = cache_root.join(version);
    let install_dir = version_dir.join(artifact.label.install_dir());
    let metadata_path = version_dir.join(format!("{}.metadata.json", artifact.label.install_dir()));

    if install_dir.exists() && metadata_matches(&metadata_path, artifact) && !force {
        println!(
            "✔ {} already present ({}), skipping",
            artifact.filename,
            install_dir.display()
        );
        return Ok(());
    }

    fs::create_dir_all(&version_dir)
        .with_context(|| format!("failed to create {}", version_dir.display()))?;

    let url = build_download_url(version, artifact.filename, mirror);
    println!(
        "→ Fetching {} ({} MB)\n    from {}",
        artifact.filename,
        (artifact.size as f64) / 1_000_000.0,
        url
    );

    let mut response = client
        .get(&url)
        .header(USER_AGENT, "cargo-aardvark")
        .header(ACCEPT, "application/octet-stream")
        .send()
        .with_context(|| format!("failed to request {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;

    let temp_dir = TempDir::new().context("failed to create temp directory")?;
    let archive_path = temp_dir.path().join(artifact.filename);
    let mut archive_file = fs::File::create(&archive_path)
        .with_context(|| format!("failed to open {}", archive_path.display()))?;

    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .with_context(|| "failed to read download chunk")?;
        if read == 0 {
            break;
        }
        downloaded += read as u64;
        hasher.update(&buffer[..read]);
        archive_file
            .write_all(&buffer[..read])
            .with_context(|| "failed to write archive to disk")?;
        print_progress(downloaded, artifact.size)?;
    }
    println!();

    let digest = format!("{:x}", hasher.finalize());
    if digest != artifact.sha256 {
        bail!(
            "checksum mismatch for {} (expected {}, got {})",
            artifact.filename,
            artifact.sha256,
            digest
        );
    }

    let mut decoder = BzDecoder::new(fs::File::open(&archive_path)?);
    let mut archive = Archive::new(&mut decoder);
    let unpack_root = temp_dir.path().join("unpacked");
    fs::create_dir_all(&unpack_root)?;
    archive.unpack(&unpack_root)?;

    if install_dir.exists() {
        fs::remove_dir_all(&install_dir)
            .with_context(|| format!("failed to clean {}", install_dir.display()))?;
    }
    let extracted_root = find_single_child(&unpack_root)?;
    fs::rename(&extracted_root, &install_dir).with_context(|| {
        format!(
            "failed to move extracted files into {}",
            install_dir.display()
        )
    })?;

    let metadata = CacheMetadata {
        filename: artifact.filename.to_string(),
        version: version.to_string(),
        sha256: artifact.sha256.to_string(),
        downloaded_at: iso_timestamp()?,
        source_url: url,
        size: artifact.size,
    };
    fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)
        .with_context(|| format!("failed to write {}", metadata_path.display()))?;

    println!(
        "✔ Installed {} into {}",
        artifact.filename,
        install_dir.display()
    );
    Ok(())
}

fn metadata_matches(path: &Path, artifact: &ArtifactDescriptor) -> bool {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<CacheMetadata>(&bytes) {
            Ok(meta) => meta.sha256 == artifact.sha256 && meta.filename == artifact.filename,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

fn build_download_url(version: &str, filename: &str, mirror: Option<&str>) -> String {
    if let Some(base) = mirror {
        format!("{}/{}", base.trim_end_matches('/'), filename)
    } else {
        format!(
            "https://github.com/pyodide/pyodide/releases/download/{version}/{filename}",
            version = version,
            filename = filename
        )
    }
}

fn find_single_child(dir: &Path) -> Result<PathBuf> {
    let mut entries = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        bail!("archive unpacked without contents");
    }
    entries.sort();
    Ok(entries.remove(0))
}

fn print_progress(downloaded: u64, total: u64) -> Result<()> {
    let percent = if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64) * 100.0
    };
    print!(
        "\r    {:.1}% ({:.2}/{:.2} MB)",
        percent,
        downloaded as f64 / 1_000_000.0,
        total as f64 / 1_000_000.0
    );
    io::stdout().flush()?;
    Ok(())
}

fn iso_timestamp() -> Result<String> {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    let datetime: DateTime<Utc> = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH + now);
    Ok(datetime.to_rfc3339())
}
