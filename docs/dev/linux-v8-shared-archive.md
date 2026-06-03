# Linux V8 Shared Archive

Aardvark's target packaging contract is host-agnostic: hosts integrate through a
host-owned shared object that links `aardvark-core` and points at a configured
Pyodide distribution. Aardvark currently pins `v8 = "=149.2.0"`. The upstream
Linux prebuilt `rusty_v8` archive works for ordinary Linux executables, but it
is not suitable for Linux shared-library hosts that embed Aardvark through a
Rust `cdylib` on x86_64.

The workspace `Cargo.toml` `v8` dependency is the authoritative version pin.
Versioned artifact names and verification snippets in this document describe the
current verified archive and must be refreshed when that pin changes.

The host can be a C ABI plugin, a native extension for another language runtime,
an application-specific loader, or any equivalent mechanism. The relevant
boundary is Linux loading an Aardvark shared object, not any one host ecosystem.

The failing linker shape looks like:

```text
relocation R_X86_64_TPOFF32 against v8::internal::g_current_isolate_ cannot be used with -shared
relocation R_X86_64_TPOFF32 against v8::internal::g_current_local_heap_ cannot be used with -shared
```

This is an ELF shared-library issue, not a general V8 runtime failure. The
observed matrix was:

- macOS shared-library smoke passed with the upstream archive;
- Linux x86_64 executable smoke passed with the upstream archive;
- Linux x86_64 `cdylib` smoke failed at link time with the upstream archive.

Relevant upstream context:

- <https://github.com/denoland/rusty_v8/issues/1706>
- <https://github.com/denoland/rusty_v8/issues/1798>
- <https://github.com/denoland/rusty_v8/pull/1911>
- <https://github.com/denoland/rusty_v8/pull/1970>

The practical workaround is to build a Linux `librusty_v8.a` from source with
V8's shared-library TLS mode enabled, then point downstream Linux shared-library
builds at that archive with `RUSTY_V8_ARCHIVE`.

If you are integrating Aardvark into a host and only need to consume the archive,
start with [`../api/shared-library-host.md`](../api/shared-library-host.md).
This page is the maintainer procedure for rebuilding, verifying, and releasing
the Linux archive.

For Aardvark, the archive is acceptable only after this chain passes on Linux:

- the custom archive links into a Rust `cdylib`;
- the resulting shared object loads with `dlopen`;
- V8 runs on the main thread and on a worker thread;
- executable performance stays within the guard threshold versus the upstream
  Linux prebuilt archive;
- at least one downstream host loads an Aardvark shared object through its
  normal native-extension or plugin path;
- Aardvark starts Pyodide from that host boundary;
- Pyodide loads staged scientific Python packages such as `numpy` and `pandas`.

Do not use this archive for macOS builds or ordinary Linux executable builds
unless there is a separate reason. The workaround exists for Linux
shared-library packaging.

## Builder Host

Use an x86_64 Linux builder. The first verified build used Amazon Linux 2023 on
`c7i.8xlarge`:

- 32 vCPU;
- 64 GiB RAM;
- 150-250 GiB gp3 root volume recommended.

Install host packages on Amazon Linux 2023:

```bash
sudo dnf -y install \
  gcc gcc-c++ make pkgconf openssl-devel perl tar gzip xz bzip2 file git \
  python3 glib2-devel clang-devel llvm-devel rsync
```

Amazon Linux 2023 ships `curl-minimal`, which is sufficient. Avoid replacing it
with the full `curl` package unless you deliberately handle the package conflict.

Install the workspace Rust toolchain:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal --default-toolchain 1.96.0
. "$HOME/.cargo/env"
```

## Build The Archive

From the Aardvark checkout on the Linux builder:

```bash
scripts/build-linux-v8-shared-archive.sh
```

The default build uses:

```bash
V8_FROM_SOURCE=1
PRINT_GN_ARGS=1
GN_ARGS='v8_monolithic_for_shared_library=true'
```

The script verifies both:

- `target/release/gn_out/args.gn` contains
  `v8_monolithic_for_shared_library = true`;
- generated V8 Ninja files contain `V8_TLS_USED_IN_LIBRARY`.

The second check matters. `rusty_v8` PR #1911 tried to inject
`extra_cflags=["-DV8_TLS_USED_IN_LIBRARY"]`, but PR #1970 notes that
`extra_cflags` is not a real top-level GN arg and can be silently ineffective.

The script writes:

```text
target/v8-linux-shared-archive/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
target/v8-linux-shared-archive/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a.metadata
```

The default scratch workspace is:

```text
tmp/v8-linux-shared-archive/build/v8-149.2.0-shared
```

The script also creates an isolated Cargo home inside that workspace. That
isolated cache is where it unpacks and patches the `v8` crate source package.
It does not use or modify the operator's normal `~/.cargo/registry` unless
`--cargo-home` is explicitly pointed there.

If `--reuse-build-root` is used, the script keeps the scratch workspace and
isolated Cargo home, but removes the expected archive before invoking Cargo.
That prevents a stale archive from satisfying the post-build checks.

Do not commit the archive. Publish it as a release asset and keep the metadata
file next to it.

## Source Package Gaps

The `v8` crates.io source package is trimmed. For `149.2.0`, the source build
needs files that are not present in the package:

- `third_party/icu/common/icudtl.dat`;
- Chromium's `third_party/rust/chromium_crates_io/vendor` tree.

The build script stages both from the exact submodule revisions recorded by the
matching `denoland/rusty_v8` tag.

The staging step patches the unpacked Cargo registry source for the `v8` crate
inside the script's isolated Cargo home. If `--cargo-home` is provided, use a
dedicated cache for this builder workflow. Do not use a developer workstation
Cargo cache as the source of record for release artifacts.

For `v8 = "=149.2.0"` the resolved revisions are:

```text
third_party/icu:  ee5f27adc28bd3f15b2c293f726d14d2e336cbd5
third_party/rust: 2b055f4ecac78bbf34a0d34217c699b7b09b44dd
```

If GitHub API access is unavailable, pass them explicitly:

```bash
scripts/build-linux-v8-shared-archive.sh \
  --icu-revision ee5f27adc28bd3f15b2c293f726d14d2e336cbd5 \
  --rust-revision 2b055f4ecac78bbf34a0d34217c699b7b09b44dd
```

## Bindgen Note

When `V8_FROM_SOURCE=1`, the `v8` crate runs bindgen after V8's Ninja build.
Current Chromium libc++ headers require `libclang` 19 or newer for that step.
Amazon Linux 2023's `clang-devel` is older, so the full Cargo build can fail
after Ninja has already produced `gn_out/obj/librusty_v8.a`.

That post-archive bindgen failure does not invalidate the static archive. The
builder script treats the archive as the requested output and continues only if
the archive exists and the GN/TLS checks pass. Downstream builds that set
`RUSTY_V8_ARCHIVE` use the crate's prebuilt Rust bindings and do not run the
source-build bindgen path.

If a builder provides a sufficiently new `libclang`, the same script can finish
with Cargo exit status `0`; the archive verification steps stay the same.

## Verify The Archive

Run the shared-object verifier on x86_64 Linux:

```bash
scripts/verify-linux-v8-shared-archive.sh \
  target/v8-linux-shared-archive/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
```

The verifier creates a scratch workspace that:

- builds a minimal Rust `cdylib`;
- pins `v8 = "=149.2.0"`;
- links with `RUSTY_V8_ARCHIVE=<archive>`;
- loads the resulting `.so` with `libloading`/`dlopen`;
- runs a small V8 script on the main thread;
- runs the same V8 script three times from a worker thread;
- runs the check in release mode.

Passing this verifier confirms that the archive fixes the Linux `cdylib` class
of failure covered by the smoke test.

## Perf Guard

Run the executable perf guard on x86_64 Linux:

```bash
scripts/bench-linux-v8-archive.sh \
  target/v8-linux-shared-archive/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
```

The benchmark builds two release executables from the same scratch source:

- upstream prebuilt `v8 = "=149.2.0"`;
- custom archive via `RUSTY_V8_ARCHIVE`.

It compares median runtime for:

- isolate/context startup plus `21 * 2`;
- arithmetic loop;
- object allocation;
- JSON parse/stringify.

Default sampling is 75 measured iterations after 20 warmups per scenario.
Default acceptance threshold is:

```text
custom median <= 1.05 * upstream median
```

Results are written under:

```text
target/v8-linux-shared-archive/perf/upstream.csv
target/v8-linux-shared-archive/perf/custom.csv
target/v8-linux-shared-archive/perf/comparison.csv
```

Do not loosen `--max-ratio` without preserving the raw CSV evidence and
explaining the reason.

## Downstream Use

For Linux shared-library packaging in any host:

```bash
export RUSTY_V8_ARCHIVE=/path/to/librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
cargo build --release
```

Use the archive only for the matching `v8` crate version and target triple. For
a new `v8` crate release, rebuild and rerun the verification path.

## Downstream Host Smoke

The `cdylib` verifier proves the archive can link and run V8 through a minimal
Rust shared object. That is necessary, but it is not the full Aardvark contract.
Each release candidate also needs a host-level smoke that uses the same loading
shape as a real consumer.

A valid host smoke must:

- build a downstream shared object with `RUSTY_V8_ARCHIVE` set;
- load that shared object through the host's normal native-extension or plugin
  mechanism;
- call into `aardvark-core` from the host boundary;
- start Pyodide inside V8;
- load staged scientific Python packages such as `numpy` and `pandas`;
- return Python results back across the host boundary.

When a release is meant for a specific host integration, run the smoke through
that host's actual loader. A representative host smoke is acceptable for the
shared-library archive itself only when the integration-specific packaging is
not part of the release.

The first verified artifact used a Rustler-based host harness because it is a
real Linux native-extension loader and it exercised the failure class that
motivated this archive. That harness is evidence for the shared-library path; it
is not the product scope.

That first host harness used Rustler `0.37`, the custom `rusty_v8` archive, and
the staged full Aardvark Pyodide distribution.

Coverage:

- the host loads a Rust shared object through its native-extension loader;
- the shared object links `aardvark-core`;
- `aardvark-core` links the custom Linux `rusty_v8` archive;
- Aardvark starts Pyodide inside V8 from the host boundary;
- Pyodide loads staged `numpy` and `pandas`;
- Python code executes and returns results back to the host.

The resulting shared object was also checked for the forbidden TLS relocation
pattern:

```text
no forbidden V8 TLS relocations found in downstream shared object
```

## Release Asset Checklist

Publish the archive and metadata as GitHub Release assets. Suggested asset names:

```text
librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a.metadata
```

Release notes should include:

- `v8` crate version;
- target triple;
- archive SHA-256;
- build host OS and CPU architecture;
- `GN_ARGS`;
- ICU and Chromium Rust vendor revisions;
- `verify-linux-v8-shared-archive.sh` result;
- `bench-linux-v8-archive.sh` result;
- downstream host smoke result;
- staged Pyodide distribution fingerprint used by the host smoke.

Consumers should download the archive, verify the SHA-256, and set
`RUSTY_V8_ARCHIVE` before compiling Linux shared libraries.

## Verified Artifact

Published release:

```text
https://github.com/appunite/aardvark/releases/tag/v149.2.0
```

Published artifact:

```text
v8_crate_version=149.2.0
target=x86_64-unknown-linux-gnu
artifact=librusty_v8_release_x86_64-unknown-linux-gnu-v149.2.0-shared.a
sha256=fc454e99846bbcbab8b5f79d74ec5031004cba8df9691bf6d39104a69ba2dbe1
size_bytes=185668672
gn_args=v8_monolithic_for_shared_library=true
icu_revision=ee5f27adc28bd3f15b2c293f726d14d2e336cbd5
icu_data_sha256=1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612
rust_vendor_revision=2b055f4ecac78bbf34a0d34217c699b7b09b44dd
rust_vendor_crate_count=268
```

`verify-linux-v8-shared-archive.sh` result:

```text
main-thread: code=0; message=ok: V8 returned 42
worker-thread-1: code=0; message=ok: V8 returned 42
worker-thread-2: code=0; message=ok: V8 returned 42
worker-thread-3: code=0; message=ok: V8 returned 42
Linux cdylib verification passed.
```

`bench-linux-v8-archive.sh` result:

```text
scenario,upstream_median_ns,custom_median_ns,custom_to_upstream_ratio,status
arithmetic_loop,3397258,3402666,1.001592,pass
json_parse_stringify,8269134,8350275,1.009813,pass
object_alloc,6181075,5826044,0.942562,pass
startup_add,962479,960261,0.997696,pass
```

Downstream host smoke result:

```text
mix test --trace
2 tests, 0 failures

aardvark commit: 28b471018ca6a87745b5ae965ae90722f9f03d44
pyodide distribution fingerprint: sha256:c687ada5f575576c4e3d5bd59173ac7cfd7fc74fcb65201278948f466cbf343f
numpy version: 2.2.5
pandas version: 2.3.3
no forbidden V8 TLS relocations found in downstream Rustler shared object
```

The release also publishes
`aardvark-v8-linux-shared-archive-v149.2.0-evidence.tar.gz` with the build,
verify, benchmark, host-smoke logs, and raw perf CSVs.
