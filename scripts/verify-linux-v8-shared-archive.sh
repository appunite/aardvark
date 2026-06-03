#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

default_v8_version() {
  awk -F'"' '/^[[:space:]]*v8 = "=/ { gsub(/^=/, "", $2); print $2; exit }' "$REPO_ROOT/Cargo.toml"
}

print_usage() {
  cat <<'EOF'
verify-linux-v8-shared-archive.sh - verify a rusty_v8 archive links into a Linux cdylib.

Usage:
  verify-linux-v8-shared-archive.sh <archive.a> [options]

Options:
  --v8-version VERSION     v8 crate version to verify (default: workspace pin)
  --work-dir DIR           scratch directory (default: tmp/v8-linux-shared-archive/verify)
  --keep-work-dir          keep scratch directory after the run
  -h, --help               show this help

The verifier builds a minimal Rust cdylib, links it with RUSTY_V8_ARCHIVE,
loads the resulting .so with libloading/dlopen, and runs V8 on the main thread
and a worker thread in release mode.
EOF
}

require_option_value() {
  local option="$1"
  local value="${2:-}"

  if [[ -z "$value" ]]; then
    echo "error: $option requires a value" >&2
    exit 1
  fi
}

if [[ $# -gt 0 ]]; then
  case "$1" in
    -h|--help|help)
      print_usage
      exit 0
      ;;
  esac
fi

if [[ $# -lt 1 ]]; then
  print_usage >&2
  exit 1
fi

ARCHIVE="$1"
shift

V8_VERSION=$(default_v8_version)
WORK_DIR="$REPO_ROOT/tmp/v8-linux-shared-archive/verify"
KEEP_WORK_DIR=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --v8-version)
      require_option_value "$1" "${2:-}"
      V8_VERSION="$2"
      shift 2
      ;;
    --work-dir)
      require_option_value "$1" "${2:-}"
      WORK_DIR="$2"
      shift 2
      ;;
    --keep-work-dir)
      KEEP_WORK_DIR=1
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

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "error: this verifier must run on x86_64 Linux" >&2
  echo "host: $(uname -s) $(uname -m)" >&2
  exit 1
fi

for tool in cargo rustc sha256sum file; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: required tool '$tool' is missing" >&2
    exit 1
  fi
done

if [[ ! -s "$ARCHIVE" ]]; then
  echo "error: archive does not exist or is empty: $ARCHIVE" >&2
  exit 1
fi

ARCHIVE=$(cd "$(dirname "$ARCHIVE")" && pwd)/$(basename "$ARCHIVE")
mkdir -p "$(dirname "$WORK_DIR")"
WORK_DIR=$(cd "$(dirname "$WORK_DIR")" && pwd)/$(basename "$WORK_DIR")

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/smoke_lib/src" "$WORK_DIR/smoke_runner/src"

if [[ "$KEEP_WORK_DIR" -eq 0 ]]; then
  trap 'rm -rf "$WORK_DIR"' EXIT
fi

cat > "$WORK_DIR/Cargo.toml" <<EOF
[workspace]
members = ["smoke_lib", "smoke_runner"]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.96"
EOF

cat > "$WORK_DIR/smoke_lib/Cargo.toml" <<EOF
[package]
name = "smoke_lib"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
v8 = "=$V8_VERSION"
EOF

cat > "$WORK_DIR/smoke_lib/src/lib.rs" <<'EOF'
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::OnceLock;

static V8_PLATFORM: OnceLock<v8::SharedRef<v8::Platform>> = OnceLock::new();

fn initialize_v8_once() {
    V8_PLATFORM.get_or_init(|| {
        let platform = v8::new_default_platform(0, false);
        let shared = platform.make_shared();
        v8::V8::initialize_platform(shared.clone());
        v8::V8::initialize();
        shared
    });
}

fn set_message(out: *mut *mut c_char, message: &str) {
    if out.is_null() {
        return;
    }

    let c_message = CString::new(message).unwrap_or_else(|_| CString::new("invalid smoke message").unwrap());
    unsafe {
        *out = c_message.into_raw();
    }
}

fn run_script() -> Result<i64, String> {
    initialize_v8_once();

    let create_params = v8::CreateParams::default().array_buffer_allocator(v8::new_default_allocator());
    let mut isolate = v8::Isolate::new(create_params);
    let result = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let mut context_scope = v8::ContextScope::new(scope, context);
        let scope = &mut context_scope;

        let source = v8::String::new(scope, "const xs = [1, 2, 3, 4]; xs.reduce((a, b) => a + b, 0) + 32;")
            .ok_or_else(|| "failed to create V8 source string".to_string())?;
        let script = v8::Script::compile(scope, source, None)
            .ok_or_else(|| "failed to compile V8 script".to_string())?;
        let value = script
            .run(scope)
            .ok_or_else(|| "failed to run V8 script".to_string())?;
        value
            .integer_value(scope)
            .ok_or_else(|| "failed to convert V8 result to integer".to_string())?
    };

    Ok(result)
}

#[no_mangle]
pub extern "C" fn aardvark_v8_smoke(out: *mut *mut c_char) -> i32 {
    match std::panic::catch_unwind(run_script) {
        Ok(Ok(42)) => {
            set_message(out, "ok: V8 returned 42");
            0
        }
        Ok(Ok(value)) => {
            set_message(out, &format!("unexpected V8 result: {value}"));
            2
        }
        Ok(Err(error)) => {
            set_message(out, &error);
            3
        }
        Err(_) => {
            set_message(out, "panic while running V8 smoke");
            4
        }
    }
}

#[no_mangle]
pub extern "C" fn aardvark_v8_smoke_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}
EOF

cat > "$WORK_DIR/smoke_runner/Cargo.toml" <<'EOF'
[package]
name = "smoke_runner"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
libloading = "0.8"
EOF

cat > "$WORK_DIR/smoke_runner/src/main.rs" <<'EOF'
use libloading::Library;
use std::env;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread;

type SmokeFn = unsafe extern "C" fn(*mut *mut c_char) -> i32;
type FreeFn = unsafe extern "C" fn(*mut c_char);

#[derive(Clone, Copy)]
struct SmokeSymbols {
    smoke: SmokeFn,
    free: FreeFn,
}

fn load_symbols(path: &Path) -> Result<SmokeSymbols, String> {
    let library = unsafe { Library::new(path) }.map_err(|error| format!("dlopen failed: {error}"))?;
    let library = Box::leak(Box::new(library));
    let smoke = unsafe {
        *library
            .get::<SmokeFn>(b"aardvark_v8_smoke")
            .map_err(|error| format!("dlsym smoke failed: {error}"))?
    };
    let free = unsafe {
        *library
            .get::<FreeFn>(b"aardvark_v8_smoke_free")
            .map_err(|error| format!("dlsym free failed: {error}"))?
    };
    Ok(SmokeSymbols { smoke, free })
}

fn call_smoke(label: &str, symbols: SmokeSymbols) -> Result<(), String> {
    let mut raw_message = ptr::null_mut();
    let code = unsafe { (symbols.smoke)(&mut raw_message) };
    let message = if raw_message.is_null() {
        String::from("<no message>")
    } else {
        unsafe { CStr::from_ptr(raw_message).to_string_lossy().into_owned() }
    };
    if !raw_message.is_null() {
        unsafe { (symbols.free)(raw_message) };
    }

    println!("{label}: code={code}; message={message}");
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{label} failed with code {code}: {message}"))
    }
}

fn run(path: PathBuf) -> Result<(), String> {
    println!("runner_arch={}", env::consts::ARCH);
    println!("library_path={}", path.display());

    let main_symbols = load_symbols(&path)?;
    call_smoke("main-thread", main_symbols)?;

    let worker_path = path.clone();
    let worker = thread::spawn(move || -> Result<(), String> {
        let worker_symbols = load_symbols(&worker_path)?;
        for iteration in 1..=3 {
            call_smoke(&format!("worker-thread-{iteration}"), worker_symbols)?;
        }
        Ok(())
    });

    worker.join().map_err(|_| "worker thread panicked".to_string())??;
    Ok(())
}

fn main() {
    let path = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: smoke_runner <path-to-libsmoke_lib.so>");
        std::process::exit(64);
    });

    if let Err(error) = run(path) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
EOF

SHA256=$(sha256sum "$ARCHIVE" | awk '{print $1}')

echo "Verifying archive:"
echo "  archive=$ARCHIVE"
echo "  sha256=$SHA256"
echo "  v8=$V8_VERSION"
echo "  rustc=$(rustc --version)"
echo "  cargo=$(cargo --version)"

(
  cd "$WORK_DIR"
  RUSTY_V8_ARCHIVE="$ARCHIVE" cargo build -p smoke_lib -p smoke_runner --release
  file target/release/libsmoke_lib.so target/release/smoke_runner
  if command -v readelf >/dev/null 2>&1; then
    readelf -d target/release/libsmoke_lib.so
  else
    if ! command -v ldd >/dev/null 2>&1; then
      echo "error: readelf is missing and ldd fallback is unavailable" >&2
      exit 1
    fi
    echo "warning: readelf is unavailable; falling back to ldd" >&2
    ldd target/release/libsmoke_lib.so
  fi
  ./target/release/smoke_runner target/release/libsmoke_lib.so
)

echo "Linux cdylib verification passed."
