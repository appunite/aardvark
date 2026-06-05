use std::fs;
use std::path::Path;

use aardvark_core::{Bundle, BUNDLE_MANIFEST_BASENAME};

#[test]
fn shipped_example_bundles_are_manifest_backed() -> anyhow::Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate should live under workspace/crates/aardvark-core");

    for case in [
        ExampleCase {
            name: "numpy_bundle",
            entrypoint: "main:main",
            packages: &["numpy"],
        },
        ExampleCase {
            name: "pandas_numpy_bundle",
            entrypoint: "main:main",
            packages: &["numpy", "pandas"],
        },
        ExampleCase {
            name: "sklearn_bundle",
            entrypoint: "main:handler",
            packages: &["scikit-learn"],
        },
    ] {
        let source_manifest_path = workspace
            .join("example")
            .join(case.name)
            .join(BUNDLE_MANIFEST_BASENAME);
        let source_manifest = fs::read(&source_manifest_path)?;

        let zip_path = workspace.join("example").join(format!("{}.zip", case.name));
        let bundle = Bundle::from_zip_bytes(fs::read(&zip_path)?)?;

        let zip_manifest = bundle
            .entries()
            .iter()
            .find(|entry| entry.path() == BUNDLE_MANIFEST_BASENAME)
            .unwrap_or_else(|| {
                panic!(
                    "{} missing {}",
                    zip_path.display(),
                    BUNDLE_MANIFEST_BASENAME
                )
            });
        assert_eq!(
            zip_manifest.contents(),
            source_manifest.as_slice(),
            "{} manifest is out of sync with {}",
            zip_path.display(),
            source_manifest_path.display()
        );

        let manifest = bundle
            .manifest()?
            .unwrap_or_else(|| panic!("{} did not parse a manifest", zip_path.display()));
        let packages: Vec<_> = manifest.packages().iter().map(String::as_str).collect();
        assert_eq!(manifest.entrypoint(), case.entrypoint);
        assert_eq!(packages, case.packages);
    }

    Ok(())
}

struct ExampleCase {
    name: &'static str,
    entrypoint: &'static str,
    packages: &'static [&'static str],
}
