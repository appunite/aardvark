use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::DEFAULT_ENTRYPOINT;

/// CLI arguments.
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    after_help = "Run mode requires --bundle unless a subcommand is used."
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) run: RunArgs,

    #[command(subcommand)]
    pub(crate) command: Option<CliCommand>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum CliCommand {
    /// Manage Aardvark Pyodide distributions.
    Assets(AssetsCli),
}

/// Bundle execution arguments.
#[derive(clap::Args, Debug)]
pub(crate) struct RunArgs {
    /// Path to a bundle archive.
    #[arg(short, long)]
    pub(crate) bundle: Option<String>,

    /// Entrypoint to execute (module:function).
    #[arg(short, long, default_value = DEFAULT_ENTRYPOINT)]
    pub(crate) entrypoint: String,

    /// Additional Pyodide packages to load before executing the bundle.
    #[arg(short = 'p', long = "package", value_name = "NAME", action = clap::ArgAction::Append)]
    pub(crate) packages: Vec<String>,

    /// Path to a snapshot to preload before running (optional).
    #[arg(long, value_name = "PATH")]
    pub(crate) snapshot: Option<String>,

    /// Path to write a snapshot after packages are loaded (optional).
    #[arg(long = "write-snapshot", value_name = "PATH")]
    pub(crate) write_snapshot: Option<String>,

    /// Optional invocation descriptor describing entrypoint and budgets.
    #[arg(long = "descriptor", value_name = "PATH")]
    pub(crate) descriptor: Option<String>,

    /// Override wall-clock limit in milliseconds.
    #[arg(long = "limit-wall-ms")]
    pub(crate) limit_wall_ms: Option<u64>,

    /// Override heap limit in MiB.
    #[arg(long = "limit-heap-mb")]
    pub(crate) limit_heap_mb: Option<u64>,

    /// Path to JSON input the adapter should expose to Python (optional).
    #[arg(long = "json-input", value_name = "PATH")]
    pub(crate) json_input: Option<String>,

    /// Override the bundle-requested Pyodide distribution profile.
    #[arg(long = "pyodide-profile", value_name = "NAME")]
    pub(crate) pyodide_profile: Option<String>,

    /// Register a Pyodide distribution profile as NAME=PATH.
    #[arg(long = "pyodide-profile-dir", value_name = "NAME=PATH", action = clap::ArgAction::Append)]
    pub(crate) pyodide_profile_dirs: Vec<String>,

    /// Execution backend to use for the run.
    #[arg(long = "execution-backend", value_enum, default_value = "direct")]
    pub(crate) execution_backend: ExecutionBackend,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub(crate) enum ExecutionBackend {
    /// Use the direct one-shot PyRuntime path.
    Direct,
    /// Use the warmed-host registry path.
    WarmedHost,
}

/// Asset management commands.
#[derive(clap::Args, Debug)]
pub(crate) struct AssetsCli {
    #[command(subcommand)]
    pub(crate) command: AssetsCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum AssetsCommand {
    /// Download and stage an Aardvark Pyodide distribution locally.
    Stage(AssetsStageArgs),
    /// Verify a staged Aardvark Pyodide distribution.
    Verify(AssetsVerifyArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct AssetsStageArgs {
    /// Which Pyodide distribution variant to stage.
    #[arg(long, value_enum, default_value = "full")]
    pub(crate) variant: StageVariant,

    /// Destination directory for staged packages (defaults under .aardvark/pyodide-distributions).
    #[arg(long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// Use an existing archive instead of downloading the release tarball.
    #[arg(long, value_name = "PATH")]
    pub(crate) archive: Option<PathBuf>,

    /// Replace existing contents within the output directory.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct AssetsVerifyArgs {
    /// Path to an unpacked Aardvark Pyodide distribution.
    #[arg(value_name = "PATH")]
    pub(crate) path: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub(crate) enum StageVariant {
    Core,
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn assets_subcommand_does_not_require_bundle() {
        let cli = Cli::try_parse_from(["aardvark-cli", "assets", "verify", "dist"])
            .expect("assets command should parse without --bundle");

        let Some(CliCommand::Assets(assets)) = cli.command else {
            panic!("expected assets command");
        };
        let AssetsCommand::Verify(args) = assets.command else {
            panic!("expected verify command");
        };
        assert_eq!(args.path, PathBuf::from("dist"));
    }

    #[test]
    fn run_options_still_parse_at_top_level() {
        let cli = Cli::try_parse_from(["aardvark-cli", "--bundle", "bundle.zip"])
            .expect("run arguments should parse at top level");

        assert!(cli.command.is_none());
        assert_eq!(cli.run.bundle.as_deref(), Some("bundle.zip"));
        assert_eq!(cli.run.entrypoint, DEFAULT_ENTRYPOINT);
    }
}
