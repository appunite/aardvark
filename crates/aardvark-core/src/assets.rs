//! Embedded Pyodide assets exposed to the runtime.

/// Pyodide version embedded in this build.
pub const PYODIDE_VERSION: &str = env!("AARDVARK_PYODIDE_VERSION");

/// Returns the raw `pyodide.asm.wasm` binary.
pub fn wasm() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/pyodide/pyodide.asm.wasm"))
}

/// Returns the loader module (`pyodide.mjs`).
pub fn loader_mjs() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide.mjs"))
}

/// Returns the legacy asm.js shim (used by Pyodide's loader).
pub fn pyodide_asm_js() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide.asm.js"))
}

/// Returns the ES module variant of `pyodide.asm.js` with ES module compatibility patches applied.
pub fn pyodide_asm_patched_js() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide.asm.patched.js"))
}

/// Returns the standard library archive included with Pyodide.
pub fn python_stdlib_zip() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/pyodide/python_stdlib.zip"))
}

/// Returns the Pyodide package lock file as distributed by Pyodide.
pub(crate) fn lockfile_json_raw() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide-lock.json"))
}

/// Returns the stock `pyodide.js` loader JavaScript (unused for now).
pub fn loader_js() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide.js"))
}

/// Returns helper functions required by the patched asm module.
pub fn builtin_wrappers_js() -> &'static str {
    include_str!(concat!(
        env!("OUT_DIR"),
        "/pyodide/pyodide_builtin_wrappers.js"
    ))
}

/// Returns the minimal bootstrap loader that instantiates Pyodide.
pub fn bootstrap_js() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/pyodide/pyodide_bootstrap.js"))
}

/// Returns the bootstrap helpers for the standalone JavaScript engine.
pub fn js_runtime_bootstrap_js() -> &'static str {
    include_str!("js/js_runtime_bootstrap.js")
}

/// Returns the trimmed Emscripten setup helper.
pub fn emscripten_setup_js() -> &'static str {
    include_str!(concat!(
        env!("OUT_DIR"),
        "/pyodide/pyodide_emscripten_setup.js"
    ))
}

/// Returns helper utilities for package loading and sys.path tweaks.
pub fn packages_js() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/js/pyodide_packages.js"
    ))
}

/// Returns the `_cloudflare/allow_entropy.py` helper.
pub fn entropy_allow_py() -> &'static str {
    include_str!("py/entropy/allow_entropy.py")
}

/// Returns the `_cloudflare/entropy_import_context.py` helper.
pub fn entropy_import_context_py() -> &'static str {
    include_str!("py/entropy/entropy_import_context.py")
}

/// Returns the `_cloudflare/entropy_patches.py` helper.
pub fn entropy_patches_py() -> &'static str {
    include_str!("py/entropy/entropy_patches.py")
}

/// Returns the `_cloudflare/import_patch_manager.py` helper.
pub fn entropy_import_patch_manager_py() -> &'static str {
    include_str!("py/entropy/import_patch_manager.py")
}
