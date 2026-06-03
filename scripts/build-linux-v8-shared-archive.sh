#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

default_v8_version() {
  awk -F'"' '/^[[:space:]]*v8 = "=/ { gsub(/^=/, "", $2); print $2; exit }' "$REPO_ROOT/Cargo.toml"
}

print_usage() {
  cat <<'EOF'
build-linux-v8-shared-archive.sh - build a Linux rusty_v8 archive for cdylib use.

Usage:
  build-linux-v8-shared-archive.sh [options]

Options:
  --v8-version VERSION     v8 crate version to build (default: workspace pin)
  --build-root DIR         scratch build root (default: tmp/v8-linux-shared-archive/build)
  --cargo-home DIR         isolated Cargo home for registry sources (default: under build root)
  --out-dir DIR            artifact output directory (default: target/v8-linux-shared-archive)
  --gn-args ARGS           GN args for V8 (default: v8_monolithic_for_shared_library=true)
  --icu-revision SHA       chromium/deps/icu revision to use for common/icudtl.dat
  --rust-revision SHA      chromium/src/third_party/rust revision to stage
  --skip-icu-data-stage    assume the v8 crate source already has common/icudtl.dat
  --skip-rust-vendor-stage assume the v8 crate source already has Rust vendor files
  --reuse-build-root       keep an existing scratch workspace instead of recreating it
  -h, --help               show this help

Environment:
  RUSTY_V8_ICU_REVISION    chromium/deps/icu revision override
  RUSTY_V8_RUST_REVISION   chromium/src/third_party/rust revision override
  CARGO_BUILD_JOBS         optional Cargo parallelism limit passed through to cargo
  RUSTFLAGS                passed through to cargo
  SCCACHE, CCACHE          passed through to rusty_v8 build.rs if configured

The script uses an isolated Cargo home by default. It stages missing v8 crate
source-package files there and avoids mutating a developer's normal Cargo
registry cache.

Output:
  <out-dir>/librusty_v8_release_x86_64-unknown-linux-gnu-v<VERSION>-shared.a
  <out-dir>/librusty_v8_release_x86_64-unknown-linux-gnu-v<VERSION>-shared.a.metadata

This script must run on x86_64 Linux.
EOF
}

V8_VERSION=$(default_v8_version)
BUILD_ROOT="$REPO_ROOT/tmp/v8-linux-shared-archive/build"
CARGO_HOME_DIR=""
OUT_DIR="$REPO_ROOT/target/v8-linux-shared-archive"
GN_ARGS_VALUE="v8_monolithic_for_shared_library=true"
CLEAN_BUILD_ROOT=1
STAGE_ICU_DATA=1
STAGE_RUST_VENDOR=1
ICU_REVISION="${RUSTY_V8_ICU_REVISION:-}"
RUST_REVISION="${RUSTY_V8_RUST_REVISION:-}"
ICU_DATA_PATH=""
ICU_DATA_SHA256=""
ICU_DATA_SIZE_BYTES=""
ICU_DATA_URL=""
RUST_VENDOR_PATH=""
RUST_VENDOR_URL=""
RUST_VENDOR_CRATE_COUNT=""
V8_SRC=""

require_option_value() {
  local option="$1"
  local value="${2:-}"

  if [[ -z "$value" ]]; then
    echo "error: $option requires a value" >&2
    exit 1
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --v8-version)
      require_option_value "$1" "${2:-}"
      V8_VERSION="$2"
      shift 2
      ;;
    --build-root)
      require_option_value "$1" "${2:-}"
      BUILD_ROOT="$2"
      shift 2
      ;;
    --cargo-home)
      require_option_value "$1" "${2:-}"
      CARGO_HOME_DIR="$2"
      shift 2
      ;;
    --out-dir)
      require_option_value "$1" "${2:-}"
      OUT_DIR="$2"
      shift 2
      ;;
    --gn-args)
      require_option_value "$1" "${2:-}"
      GN_ARGS_VALUE="$2"
      shift 2
      ;;
    --icu-revision)
      require_option_value "$1" "${2:-}"
      ICU_REVISION="$2"
      shift 2
      ;;
    --rust-revision)
      require_option_value "$1" "${2:-}"
      RUST_REVISION="$2"
      shift 2
      ;;
    --skip-icu-data-stage)
      STAGE_ICU_DATA=0
      shift
      ;;
    --skip-rust-vendor-stage)
      STAGE_RUST_VENDOR=0
      shift
      ;;
    --reuse-build-root)
      CLEAN_BUILD_ROOT=0
      shift
      ;;
    -h|--help|help)
      print_usage
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1'" >&2
      print_usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$V8_VERSION" ]]; then
  echo "error: could not determine v8 version" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "error: this archive must be built on x86_64 Linux" >&2
  echo "host: $(uname -s) $(uname -m)" >&2
  exit 1
fi

for tool in cargo rustc python3 curl base64 tar sha256sum grep; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required tool '$tool' is missing" >&2
    exit 1
  fi
done

mkdir -p "$(dirname "$BUILD_ROOT")"
BUILD_ROOT=$(cd "$(dirname "$BUILD_ROOT")" && pwd)/$(basename "$BUILD_ROOT")
OUT_DIR=$(mkdir -p "$OUT_DIR" && cd "$OUT_DIR" && pwd)
WORKSPACE="$BUILD_ROOT/v8-$V8_VERSION-shared"
RUSTY_V8_TREE_JSON="$WORKSPACE/rusty_v8-v$V8_VERSION-tree.json"
if [[ -z "$CARGO_HOME_DIR" ]]; then
  CARGO_HOME_DIR="$WORKSPACE/cargo-home"
else
  mkdir -p "$(dirname "$CARGO_HOME_DIR")"
  CARGO_HOME_DIR=$(cd "$(dirname "$CARGO_HOME_DIR")" && pwd)/$(basename "$CARGO_HOME_DIR")
fi

ensure_rusty_v8_tree_json() {
  local tag="v$V8_VERSION"
  local api_url="https://api.github.com/repos/denoland/rusty_v8/git/trees/$tag?recursive=1"
  local tmp

  if [[ -s "$RUSTY_V8_TREE_JSON" ]]; then
    return
  fi

  mkdir -p "$(dirname "$RUSTY_V8_TREE_JSON")"
  tmp="$RUSTY_V8_TREE_JSON.tmp.$$"
  curl -fsSL "$api_url" > "$tmp"
  mv "$tmp" "$RUSTY_V8_TREE_JSON"
}

resolve_submodule_revision() {
  local submodule_path="$1"
  local tag="v$V8_VERSION"
  local revision

  ensure_rusty_v8_tree_json
  revision=$(
    SUBMODULE_PATH="$submodule_path" python3 - "$RUSTY_V8_TREE_JSON" <<'PY'
import json
import os
import sys

submodule_path = os.environ["SUBMODULE_PATH"]
tree_path = sys.argv[1]
with open(tree_path, encoding="utf-8") as handle:
    tree = json.load(handle).get("tree", [])
for entry in tree:
    if entry.get("path") == submodule_path and entry.get("type") == "commit":
        print(entry["sha"])
        break
PY
  )

  if [[ -z "$revision" ]]; then
    echo "error: could not resolve $submodule_path revision for rusty_v8 $tag" >&2
    echo "Set the matching revision explicitly." >&2
    exit 1
  fi

  echo "$revision"
}

locate_v8_crate_source() {
  find "$CARGO_HOME_DIR/registry/src" -maxdepth 2 -type d -name "v8-$V8_VERSION" -print -quit 2>/dev/null
}

ensure_v8_crate_source() {
  if [[ -n "$V8_SRC" ]]; then
    return
  fi
  (
    cd "$WORKSPACE"
    CARGO_HOME="$CARGO_HOME_DIR" cargo fetch
  )

  V8_SRC=$(locate_v8_crate_source)
  if [[ -z "$V8_SRC" ]]; then
    echo "error: could not locate v8-$V8_VERSION source under Cargo registry" >&2
    exit 1
  fi
}

stage_icu_data() {
  local tmp

  ensure_v8_crate_source

  if [[ -z "$ICU_REVISION" ]]; then
    ICU_REVISION=$(resolve_submodule_revision "third_party/icu")
  fi

  ICU_DATA_PATH="$V8_SRC/third_party/icu/common/icudtl.dat"
  ICU_DATA_URL="https://chromium.googlesource.com/chromium/deps/icu/+/$ICU_REVISION/common/icudtl.dat?format=TEXT"
  mkdir -p "$(dirname "$ICU_DATA_PATH")"

  if [[ ! -s "$ICU_DATA_PATH" ]]; then
    tmp="$ICU_DATA_PATH.tmp.$$"
    curl -fsSL "$ICU_DATA_URL" | base64 -d > "$tmp"
    mv "$tmp" "$ICU_DATA_PATH"
  fi

  ICU_DATA_SHA256=$(sha256sum "$ICU_DATA_PATH" | awk '{print $1}')
  ICU_DATA_SIZE_BYTES=$(wc -c < "$ICU_DATA_PATH" | tr -d ' ')
}

stage_rust_vendor() {
  local marker

  ensure_v8_crate_source

  if [[ -z "$RUST_REVISION" ]]; then
    RUST_REVISION=$(resolve_submodule_revision "third_party/rust")
  fi

  RUST_VENDOR_PATH="$V8_SRC/third_party/rust"
  RUST_VENDOR_URL="https://chromium.googlesource.com/chromium/src/third_party/rust/+archive/$RUST_REVISION.tar.gz"
  marker="$RUST_VENDOR_PATH/chromium_crates_io/vendor/icu_calendar_data-v2/build.rs"
  mkdir -p "$RUST_VENDOR_PATH"

  if [[ ! -s "$marker" ]]; then
    curl -fsSL "$RUST_VENDOR_URL" | tar -xzf - -C "$RUST_VENDOR_PATH"
  fi

  if [[ ! -s "$marker" ]]; then
    echo "error: staged Rust vendor tree is still missing $marker" >&2
    exit 1
  fi

  RUST_VENDOR_CRATE_COUNT=$(
    find "$RUST_VENDOR_PATH/chromium_crates_io/vendor" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' '
  )
}

if [[ "$CLEAN_BUILD_ROOT" -eq 1 ]]; then
  rm -rf "$WORKSPACE"
fi

mkdir -p "$WORKSPACE/src" "$CARGO_HOME_DIR" "$OUT_DIR"

cat > "$WORKSPACE/Cargo.toml" <<EOF
[workspace]

[package]
name = "v8_archive_builder"
version = "0.1.0"
edition = "2021"
rust-version = "1.96"

[dependencies]
v8 = "=$V8_VERSION"
EOF

cat > "$WORKSPACE/src/main.rs" <<'EOF'
use std::sync::OnceLock;

static V8_PLATFORM: OnceLock<v8::SharedRef<v8::Platform>> = OnceLock::new();

fn init_v8() {
    V8_PLATFORM.get_or_init(|| {
        let platform = v8::new_default_platform(0, false);
        let shared = platform.make_shared();
        v8::V8::initialize_platform(shared.clone());
        v8::V8::initialize();
        shared
    });
}

fn main() {
    init_v8();
    println!("v8 archive builder probe ok");
}
EOF

if [[ "$STAGE_ICU_DATA" -eq 1 ]]; then
  stage_icu_data
fi

if [[ "$STAGE_RUST_VENDOR" -eq 1 ]]; then
  stage_rust_vendor
fi

echo "Building rusty_v8 archive"
echo "  v8 version: $V8_VERSION"
echo "  build root: $WORKSPACE"
echo "  cargo home: $CARGO_HOME_DIR"
echo "  output dir: $OUT_DIR"
echo "  GN_ARGS: $GN_ARGS_VALUE"
if [[ "$STAGE_ICU_DATA" -eq 1 ]]; then
  echo "  ICU revision: $ICU_REVISION"
  echo "  ICU data: $ICU_DATA_PATH"
  echo "  ICU data sha256: $ICU_DATA_SHA256"
else
  echo "  ICU data staging: skipped"
fi
if [[ "$STAGE_RUST_VENDOR" -eq 1 ]]; then
  echo "  Rust vendor revision: $RUST_REVISION"
  echo "  Rust vendor crates: $RUST_VENDOR_CRATE_COUNT"
else
  echo "  Rust vendor staging: skipped"
fi
echo "  rustc: $(rustc --version)"
echo "  cargo: $(cargo --version)"

ARCHIVE="$WORKSPACE/target/release/gn_out/obj/librusty_v8.a"
GN_OUT="$WORKSPACE/target/release/gn_out"
rm -f "$ARCHIVE"

BUILD_STATUS=0
(
  cd "$WORKSPACE"
  CARGO_HOME="$CARGO_HOME_DIR" \
    V8_FROM_SOURCE=1 \
    PRINT_GN_ARGS=1 \
    GN_ARGS="$GN_ARGS_VALUE" \
    cargo build --release
) || BUILD_STATUS=$?

if [[ ! -s "$ARCHIVE" ]]; then
  echo "error: expected archive was not produced: $ARCHIVE" >&2
  echo "cargo build exit status: $BUILD_STATUS" >&2
  exit 1
fi

if [[ "$BUILD_STATUS" -ne 0 ]]; then
  echo "warning: cargo build exited with status $BUILD_STATUS after producing $ARCHIVE" >&2
  echo "warning: continuing because the requested output is the V8 static archive" >&2
fi

if ! grep -q 'v8_monolithic_for_shared_library = true' "$GN_OUT/args.gn"; then
  echo "error: generated args.gn does not enable v8_monolithic_for_shared_library" >&2
  echo "args.gn path: $GN_OUT/args.gn" >&2
  exit 1
fi

if ! grep -R -q 'V8_TLS_USED_IN_LIBRARY' "$GN_OUT/obj/v8"; then
  echo "error: V8_TLS_USED_IN_LIBRARY was not found in generated V8 ninja files" >&2
  echo "This means the archive may still use Linux TLS relocations that fail in cdylibs." >&2
  exit 1
fi

DEST="$OUT_DIR/librusty_v8_release_x86_64-unknown-linux-gnu-v$V8_VERSION-shared.a"
cp "$ARCHIVE" "$DEST"

SHA256=$(sha256sum "$DEST" | awk '{print $1}')
SIZE_BYTES=$(wc -c < "$DEST" | tr -d ' ')
METADATA="$DEST.metadata"

cat > "$METADATA" <<EOF
artifact=$DEST
sha256=$SHA256
size_bytes=$SIZE_BYTES
v8_crate_version=$V8_VERSION
rustc=$(rustc --version)
cargo=$(cargo --version)
cargo_home=$CARGO_HOME_DIR
host_kernel=$(uname -srmo)
gn_args=$GN_ARGS_VALUE
source_build=V8_FROM_SOURCE=1
cargo_build_exit_status=$BUILD_STATUS
archive_source=$ARCHIVE
generated_args_gn=$GN_OUT/args.gn
rusty_v8_tree_json=$RUSTY_V8_TREE_JSON
icu_data_staged=$STAGE_ICU_DATA
icu_revision=$ICU_REVISION
icu_data_path=$ICU_DATA_PATH
icu_data_sha256=$ICU_DATA_SHA256
icu_data_size_bytes=$ICU_DATA_SIZE_BYTES
icu_data_url=$ICU_DATA_URL
rust_vendor_staged=$STAGE_RUST_VENDOR
rust_vendor_revision=$RUST_REVISION
rust_vendor_path=$RUST_VENDOR_PATH
rust_vendor_crate_count=$RUST_VENDOR_CRATE_COUNT
rust_vendor_url=$RUST_VENDOR_URL
EOF

echo "Archive built:"
echo "  $DEST"
echo "  sha256=$SHA256"
echo "  metadata=$METADATA"
