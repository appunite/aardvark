// Minimal package helpers for Aardvark's Pyodide integration.

export async function loadTransitivePackages(Module) {
  // This primes the virtualized package view. We don't bundle a packages tar
  // yet, so we just record that no additional packages are required.
  Module.__pyRunnerPackages = [];
}

export function ensureSessionMetadata(Module) {
  Module.FS.mkdirTree("/session/metadata");
  Module.FS.mkdirTree("/session/metadata/python_modules");
  Module.FS.mkdirTree("/session/metadata/vendor");
}

export function adjustSysPathPostBootstrap(pyodide) {
  pyodide.runPython(`
def __py_runner_adjust_sys_path():
    import sys
    session_path = "/session"
    metadata_path = "/session/metadata"
    python_modules = "/session/metadata/python_modules"
    vendor_path = "/session/metadata/vendor"

    if session_path not in sys.path:
        sys.path.insert(0, session_path)

    # Ensure bundled metadata paths precede site-packages.
    insert_index = None
    for i, entry in enumerate(sys.path):
        if "site-packages" in entry:
            insert_index = i
            break

    if insert_index is not None:
        for path in (vendor_path, python_modules, metadata_path):
            if path in sys.path:
                sys.path.remove(path)
            sys.path.insert(insert_index, path)
    else:
        for path in (metadata_path, python_modules, vendor_path):
            if path not in sys.path:
                sys.path.append(path)

__py_runner_adjust_sys_path()
del __py_runner_adjust_sys_path
`);
}
