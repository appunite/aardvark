use std::env;
use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "Aardvark automation tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build release binaries for the CLI (full + downloader) for macOS and Linux.
    ReleaseCli(ReleaseCliArgs),
}

#[derive(Args, Debug)]
struct ReleaseCliArgs {
    /// Output directory for the built artifacts.
    #[arg(long, default_value = "dist")]
    out_dir: Utf8PathBuf,

    /// Target triples to build. Defaults to macOS + Linux if omitted.
    #[arg(long, value_delimiter = ',', value_name = "TRIPLE")]
    targets: Vec<String>,

    /// Skip building the full (runtime) CLI binary.
    #[arg(long)]
    skip_full: bool,

    /// Skip building the downloader-only variant.
    #[arg(long)]
    skip_fetcher: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::ReleaseCli(args) => release_cli(args),
    }
}

fn release_cli(args: ReleaseCliArgs) -> Result<()> {
    let ReleaseCliArgs {
        out_dir,
        targets,
        skip_full,
        skip_fetcher,
    } = args;

    let workspace_root = workspace_root();
    let host_triple = detect_host_triple()?;
    let targets = if targets.is_empty() {
        vec![
            "x86_64-apple-darwin".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
        ]
    } else {
        targets
    };

    if skip_full && skip_fetcher {
        bail!("both variants were skipped; nothing to build");
    }

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("failed to create output directory {}", out_dir))?;

    if !skip_full {
        for target in &targets {
            build_variant(
                &workspace_root,
                &host_triple,
                target,
                Variant {
                    name: "full",
                    binary: "aardvark-cli",
                    extra_args: &[],
                },
                &out_dir,
            )?;
        }
    }

    if !skip_fetcher {
        let extra_args = ["--no-default-features", "--features", "fetcher"];
        for target in &targets {
            build_variant(
                &workspace_root,
                &host_triple,
                target,
                Variant {
                    name: "fetcher",
                    binary: "cargo-aardvark",
                    extra_args: &extra_args,
                },
                &out_dir,
            )?;
        }
    }

    Ok(())
}

struct Variant<'a> {
    name: &'static str,
    binary: &'static str,
    extra_args: &'a [&'a str],
}

fn build_variant(
    workspace_root: &Utf8PathBuf,
    host_triple: &str,
    target: &str,
    variant: Variant<'_>,
    out_dir: &Utf8PathBuf,
) -> Result<()> {
    println!(
        "Building {} ({}) for {}",
        variant.binary, variant.name, target
    );

    ensure_target_installed(target)?;
    let tool = select_build_tool(host_triple, target)?;
    if tool == BuildTool::Cross {
        ensure_cross_toolchain(target)?;
    }
    let mut cmd = Command::new(tool.binary());
    cmd.arg("build")
        .arg("--release")
        .arg("-p")
        .arg("aardvark-cli")
        .arg("--target")
        .arg(target)
        .current_dir(workspace_root);

    for arg in variant.extra_args {
        cmd.arg(arg);
    }

    let status = cmd.status().with_context(|| {
        format!(
            "failed to spawn cargo for {} variant targeting {}",
            variant.name, target
        )
    })?;

    if !status.success() {
        bail!(
            "cargo build failed for {} variant targeting {}",
            variant.name,
            target
        );
    }

    let binary_name = with_exe_suffix(variant.binary, target);
    let built_path = workspace_root
        .join("target")
        .join(target)
        .join("release")
        .join(&binary_name);
    if !built_path.exists() {
        bail!("expected binary at {}", built_path);
    }

    let dest_name = format!("{}-{}-{}", variant.binary, variant.name, target);
    let dest_path = out_dir.join(dest_name);
    fs::copy(&built_path, &dest_path)
        .with_context(|| format!("failed to copy {} to {}", built_path, dest_path))?;

    println!("  ⇒ {}", dest_path);
    Ok(())
}

fn with_exe_suffix(binary: &str, target: &str) -> String {
    if target.contains("windows") {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    }
}

fn workspace_root() -> Utf8PathBuf {
    let dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.parent()
        .expect("xtask manifest must have a parent")
        .to_owned()
}

fn detect_host_triple() -> Result<String> {
    if let Ok(host) = env::var("HOST") {
        return Ok(host);
    }
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("failed to invoke rustc to discover host triple")?;
    if !output.status.success() {
        bail!("rustc -vV exited with status {}", output.status);
    }
    let stdout = String::from_utf8(output.stdout)?;
    for line in stdout.lines() {
        if let Some(triple) = line.strip_prefix("host: ") {
            return Ok(triple.trim().to_string());
        }
    }
    bail!("unable to parse host triple from rustc output");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildTool {
    Cargo,
    Cross,
}

impl BuildTool {
    fn binary(self) -> &'static str {
        match self {
            BuildTool::Cargo => "cargo",
            BuildTool::Cross => "cross",
        }
    }
}

fn select_build_tool(host: &str, target: &str) -> Result<BuildTool> {
    if host == target {
        return Ok(BuildTool::Cargo);
    }
    if cross_available() {
        Ok(BuildTool::Cross)
    } else {
        bail!(
            "cross not found; install it with `cargo install cross` to build target {}",
            target
        );
    }
}

fn cross_available() -> bool {
    Command::new("cross")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn ensure_target_installed(target: &str) -> Result<()> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .context("failed to query installed Rust targets")?;
    if !output.status.success() {
        bail!(
            "`rustup target list --installed` exited with status {}",
            output.status
        );
    }
    let installed = String::from_utf8(output.stdout)?;
    if installed.lines().any(|line| line.trim() == target) {
        return Ok(());
    }

    println!("Installing Rust target {target}…");
    let status = Command::new("rustup")
        .args(["target", "add", target])
        .status()
        .context("failed to run `rustup target add`")?;
    if status.success() {
        Ok(())
    } else {
        bail!("`rustup target add {target}` exited with status {status}")
    }
}

fn ensure_cross_toolchain(target: &str) -> Result<()> {
    let channel = active_toolchain_channel()?;
    let toolchain = format!("{channel}-{target}");

    if toolchain_installed(&toolchain)? {
        return Ok(());
    }

    println!("Installing Rust toolchain {toolchain} (for cross)…");
    let status = Command::new("rustup")
        .args([
            "toolchain",
            "add",
            &toolchain,
            "--profile",
            "minimal",
            "--force-non-host",
        ])
        .status()
        .context("failed to install cross toolchain")?;
    if status.success() {
        Ok(())
    } else {
        bail!(
            "`rustup toolchain add {toolchain} --profile minimal --force-non-host` exited with status {status}"
        )
    }
}

fn active_toolchain_channel() -> Result<String> {
    let output = Command::new("rustup")
        .args(["show", "active-toolchain"])
        .output()
        .context("failed to query active rustup toolchain")?;
    if !output.status.success() {
        bail!(
            "`rustup show active-toolchain` exited with status {}",
            output.status
        );
    }
    let stdout = String::from_utf8(output.stdout)?;
    let token = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("unable to parse active toolchain"))?;
    let channel = token
        .rsplit_once('-')
        .map(|(channel, _)| channel)
        .unwrap_or(token);
    Ok(channel.to_string())
}

fn toolchain_installed(toolchain: &str) -> Result<bool> {
    let output = Command::new("rustup")
        .args(["toolchain", "list"])
        .output()
        .context("failed to list rustup toolchains")?;
    if !output.status.success() {
        bail!(
            "`rustup toolchain list` exited with status {}",
            output.status
        );
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .map(|line| line.split_whitespace().next().unwrap_or(""))
        .any(|entry| entry == toolchain))
}
