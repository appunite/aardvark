# Release Process

This checklist covers preparing a new public release of Aardvark.

## Versioning

- Crates follow semantic versioning.
- Coordinate version bumps across `aardvark-core`, `aardvark-cli`, and the
  workspace `Cargo.toml`.
- Update the manifest schema version only when necessary and after reviewing
  backwards compatibility.

## Changelog

- Maintain `CHANGELOG.md` at the repository root (create it if missing).
- Group entries under `### Added`, `### Changed`, `### Fixed`, etc.
- Ensure every release note references a pull request or issue number when
  possible.

## Pre-Release Checklist

1. `cargo fmt`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `cargo test -p integration-tests`
5. JS lint/build if assets changed
6. Verify documentation builds: `cargo doc -p aardvark-core`
7. Update public docs (`docs/architecture`, `docs/api`) and developer docs
   (`docs/dev`).
8. Update `README.md` with any new capabilities.
9. Regenerate [Pyodide](https://pyodide.org/) assets if the bundled version changed.

## Tagging

- Commit all changes and tag:

  ```bash
  git tag -s vX.Y.Z -m "Aardvark vX.Y.Z"
  git push origin vX.Y.Z
  ```

## Publishing to crates.io

1. Ensure you’re logged in: `cargo login`.
2. Publish in dependency order:
   ```bash
   cargo publish -p aardvark-core
   cargo publish -p aardvark-cli
   ```
3. Watch crates.io for indexing completion.

## Post-Release

- Create a GitHub release with changelog highlights and tarball.
- Announce internally and solicit feedback for the next iteration.
- Open follow-up issues for any TODOs deferred during the release crunch.
