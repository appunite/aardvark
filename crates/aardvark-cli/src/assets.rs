use aardvark_core::pyodide_distribution::{
    default_pyodide_distribution_stage_output_dir, stage_pyodide_distribution,
    DistributionFeatures, PackageFeatures, PyodideDistribution, PyodideDistributionStageOptions,
    PyodideDistributionVariant,
};
use anyhow::{Context, Result};

use crate::args::{AssetsCommand, AssetsStageArgs, AssetsVerifyArgs, StageVariant};

pub(crate) fn handle_assets_command(command: AssetsCommand) -> Result<()> {
    match command {
        AssetsCommand::Stage(args) => stage_assets(args),
        AssetsCommand::Verify(args) => verify_assets(args),
    }
}

fn stage_assets(args: AssetsStageArgs) -> Result<()> {
    let variant = pyodide_variant(args.variant);
    let output_dir = args
        .output
        .unwrap_or_else(|| default_pyodide_distribution_stage_output_dir(variant));
    stage_pyodide_distribution(PyodideDistributionStageOptions {
        variant,
        output_dir,
        archive: args.archive,
        force: args.force,
    })
    .map(|_| ())
    .map_err(anyhow::Error::from)
}

fn verify_assets(args: AssetsVerifyArgs) -> Result<()> {
    let dist = PyodideDistribution::external(&args.path)
        .with_context(|| format!("verify distribution {}", args.path.display()))?;
    println!(
        "verified {} ({}, fingerprint {})",
        args.path.display(),
        dist.manifest().variant.as_str(),
        dist.compatibility_fingerprint()
    );
    print_distribution_features(&dist.manifest().features);
    Ok(())
}

fn pyodide_variant(variant: StageVariant) -> PyodideDistributionVariant {
    match variant {
        StageVariant::Core => PyodideDistributionVariant::Core,
        StageVariant::Full => PyodideDistributionVariant::Full,
    }
}

fn print_distribution_features(features: &DistributionFeatures) {
    let mut enabled = Vec::new();
    if features.wasm_simd {
        enabled.push("wasm-simd");
    }
    if features.openblas {
        enabled.push("openblas");
    }
    if enabled.is_empty() {
        println!("features: none detected");
        return;
    }
    println!("features: {}", enabled.join(", "));

    let package_count = features.packages.len();
    let wasm_module_count: u32 = features
        .packages
        .values()
        .map(|package_features| package_features.wasm_modules)
        .sum();
    let simd_package_count = features
        .packages
        .values()
        .filter(|package_features| package_features.wasm_simd)
        .count();
    let openblas_package_count = features
        .packages
        .values()
        .filter(|package_features| package_features.openblas)
        .count();
    println!(
        "feature package summary: {package_count} packages, {wasm_module_count} wasm modules, {simd_package_count} wasm-simd packages, {openblas_package_count} openblas-linked packages"
    );

    let highlights = features
        .packages
        .iter()
        .filter(|(name, package_features)| {
            package_features.openblas
                || matches!(
                    name.as_str(),
                    "numpy" | "pandas" | "scipy" | "scikit-learn" | "matplotlib" | "libopenblas"
                )
        })
        .map(|(name, package_features)| {
            format!("{name}({})", format_package_features(package_features))
        })
        .collect::<Vec<_>>();
    if !highlights.is_empty() {
        println!("feature package highlights: {}", highlights.join(", "));
    }
}

fn format_package_features(features: &PackageFeatures) -> String {
    let mut flags = Vec::new();
    if features.wasm_simd {
        flags.push("wasm-simd");
    }
    if features.openblas {
        flags.push("openblas");
    }
    if features.wasm_modules > 0 {
        flags.push("wasm-modules");
    }
    flags.join("+")
}
