import { _createPyodideModule } from "./pyodide.asm.patched.js";

const DEFAULT_ENV = {
  HOME: "/session",
  PYTHONHASHSEED: "111",
};

const NOOP = () => {};

const SENTINEL_BYTES = new Uint8Array([
    0,  97, 115, 109,   1,   0,   0,   0,   1,  12,   3,  95,
    0,  96,   0,   1, 111,  96,   1, 111,   1, 127,   3,   3,
    2,   1,   2,   7,  33,   2,  15,  99, 114, 101,  97, 116,
  101,  95, 115, 101, 110, 116, 105, 110, 101, 108,   0,   0,
   11, 105, 115,  95, 115, 101, 110, 116, 105, 110, 101, 108,
    0,   1,  10,  19,   2,   7,   0, 251,   1,   0, 251,  27,
   11,   9,   0,  32,   0, 251,  26, 251,  20,   0,  11,
]);
const SENTINEL_EXPORTS = new WebAssembly.Instance(
  new WebAssembly.Module(SENTINEL_BYTES),
).exports;

function ensureTrailingSlash(url) {
  return url.endsWith("/") ? url : `${url}/`;
}

async function loadBinaryAsset(name) {
  console.log(`[emscripten_setup] loading binary asset ${name}`);
  const response = await fetch(name);
  if (!response.ok) {
    throw new Error(`Failed to load asset '${name}': ${response.status}`);
  }
  const buffer = await response.arrayBuffer();
  return new Uint8Array(buffer);
}

async function loadJsonAsset(name) {
  console.log(`[emscripten_setup] loading json asset ${name}`);
  try {
    const response = await fetch(name);
    if (!response.ok) {
      return null;
    }
    return await response.json();
  } catch (_) {
    return null;
  }
}

function sanitizeLockfile(lockfile) {
  return lockfile;
}

function computePythonVersionTuple(Module) {
  if (typeof Module._py_version_major === "function") {
    const major = Module._py_version_major();
    const minor = Module._py_version_minor?.() ?? 0;
    const micro = Module._py_version_micro?.() ?? 0;
    return [major, minor, micro];
  }
  if (Module._Py_Version) {
    const versionInt = Module.HEAPU32[Module._Py_Version >>> 2];
    const major = (versionInt >>> 24) & 0xff;
    const minor = (versionInt >>> 16) & 0xff;
    const micro = (versionInt >>> 8) & 0xff;
    return [major, minor, micro];
  }
  return [3, 12, 0];
}

function prepareFileSystem(stdlibBytes) {
  return function prepare(Module) {
    console.log("[emscripten_setup] prepareFileSystem");
    const FS = Module.FS;
    const homeDir = Module.API?.config?.env?.HOME ?? DEFAULT_ENV.HOME;
    const [pyMajor, pyMinor, pyMicro] = computePythonVersionTuple(Module);
    const stdlibPath = `/lib/python${pyMajor}${pyMinor}.zip`;
    const sitePackages = `/lib/python${pyMajor}.${pyMinor}/site-packages`;
    Module.API = Module.API || {};
    Module.API.pyVersionTuple = [pyMajor, pyMinor, pyMicro];
    Module.API.sitePackages = sitePackages;
    Module.FS.sitePackages = sitePackages;
    Module.FS.sessionSitePackages = `/session${sitePackages}`;
    FS.mkdirTree("/lib");
    FS.writeFile(stdlibPath, stdlibBytes, { canOwn: true });
    FS.mkdirTree(sitePackages);
    FS.mkdirTree(homeDir);
    Module.ENV = Module.ENV || {};
    Module.ENV.LD_LIBRARY_PATH = ["/usr/lib", sitePackages].join(":");
  };
}

function applyEnvironment(Module, env) {
  console.log("[emscripten_setup] applyEnvironment");
  Module.ENV = Module.ENV || {};
  Object.assign(Module.ENV, env);
  if (Module.API && Module.API.config) {
    Module.API.config.env = { ...env };
  }
}

function createModuleConfig(options, stdlibBytes, wasmBytes, lockfile) {
  const {
    indexURL = ".",
    env = {},
    stdout = NOOP,
    stderr = NOOP,
    args = [],
    packageBaseUrl: packageBaseUrlOverride,
    snapshot,
    makeSnapshot = false,
  } = options;

  const resolvedIndexURL = ensureTrailingSlash(indexURL);
  const mergedEnv = { ...DEFAULT_ENV, ...env };
  console.log("[emscripten_setup] creating module config", {
    indexURL: resolvedIndexURL,
    env: mergedEnv,
    hasLockfile: !!lockfile,
  });

  const versionTag = globalThis.__pyRunnerPyodideVersion ?? "0.29.0";
  const defaultPackageBaseUrl = `https://cdn.jsdelivr.net/pyodide/v${versionTag}/full/`;
  const packageBaseUrl = packageBaseUrlOverride ?? defaultPackageBaseUrl;

  const apiConfig = {
    indexURL: resolvedIndexURL,
    packageCacheDir: resolvedIndexURL,
    lockFileURL: `${resolvedIndexURL}pyodide-lock.json`,
    env: { ...mergedEnv },
    jsglobals: globalThis,
    stdin: () => "",
    stdout,
    stderr,
    checkAPIVersion: false,
    fullStdLib: true,
    packages: [],
    packageBaseUrl,
    args: [...args],
    enableRunUntilComplete: true,
    BUILD_ID: "dev",
    resolveLockFilePromise: lockfile
      ? (resolver) => {
          try {
            resolver(lockfile);
          } catch (error) {
            console.error("resolveLockFilePromise failed", error);
          }
        }
      : undefined,
  };
  if (snapshot) {
    apiConfig._loadSnapshot = snapshot;
  }
  if (makeSnapshot) {
    apiConfig._makeSnapshot = true;
  }

  const moduleConfig = {
    preRun: [],
    postRun: [],
    arguments: [...args],
    ENV: { ...mergedEnv },
    print: stdout,
    printErr: stderr,
    onExit(code) {
      moduleConfig.exitCode = code;
    },
    setStatus: NOOP,
    monitorRunDependencies: NOOP,
    locateFile: (path) => {
      if (path.startsWith("http://") || path.startsWith("https://")) {
        return path;
      }
      if (path.startsWith("./")) {
        return path.slice(2);
      }
      return `${resolvedIndexURL}${path}`;
    },
    wasmBinary: wasmBytes,
    instantiateWasm(imports, successCallback) {
      console.log("[emscripten_setup] instantiateWasm called");
      try {
        const module = new WebAssembly.Module(wasmBytes);
        const instance = new WebAssembly.Instance(module, {
          ...imports,
          sentinel: SENTINEL_EXPORTS,
        });
        console.log("[emscripten_setup] instantiateWasm success");
        successCallback(instance, module);
      } catch (error) {
        stderr("instantiateWasm failed", error);
        throw error;
      }
      return {};
    },
    API: {
      config: apiConfig,
      lockFilePromise: lockfile ? Promise.resolve(lockfile) : Promise.resolve(null),
      tests: {},
      sitePackages: "",
    },
  };
  if (snapshot && snapshot.length) {
    const pageSize = 64 * 1024;
    const aligned = Math.ceil(snapshot.length / pageSize) * pageSize;
    moduleConfig.noInitialRun = true;
    moduleConfig.INITIAL_MEMORY = aligned;
  }

  moduleConfig.preRun.push(prepareFileSystem(stdlibBytes));
  moduleConfig.preRun.push((Module) => {
    console.log("[emscripten_setup] preRun setup hook");
    applyEnvironment(Module, moduleConfig.ENV);
    Module.API = Module.API || moduleConfig.API;
    Module.API.config = Module.API.config || apiConfig;
    Module.API.lockFilePromise =
      Module.API.lockFilePromise ?? moduleConfig.API.lockFilePromise;
    Module.API.tests = Module.API.tests || moduleConfig.API.tests;
    if (lockfile) {
      Module.API.lockFile = lockfile;
    }
  });

  return moduleConfig;
}

export async function instantiatePyodideModule(options) {
  console.log("[emscripten_setup] instantiatePyodideModule start");
  const stdlibBytes = await loadBinaryAsset("python_stdlib.zip");
  const wasmBytes = await loadBinaryAsset("pyodide.asm.wasm");
  const lockfile = sanitizeLockfile(await loadJsonAsset("pyodide-lock.json"));

  const moduleConfig = createModuleConfig(options, stdlibBytes, wasmBytes, lockfile);
  const modulePromise = withBrowserLikeEnvironment(() =>
    _createPyodideModule(moduleConfig)
  ).catch((error) => {
    console.error("[emscripten_setup] _createPyodideModule rejected", error);
    throw error;
  });

  const module = await modulePromise;
  console.log("[emscripten_setup] modulePromise fulfilled");

  if (typeof module.setSetTimeout === "function") {
    module.setSetTimeout(
      globalThis.setTimeout?.bind(globalThis) ?? NOOP,
      globalThis.clearTimeout?.bind(globalThis) ?? NOOP,
      globalThis.setInterval?.bind(globalThis) ?? NOOP,
      globalThis.clearInterval?.bind(globalThis) ?? NOOP,
    );
  }

  if (typeof module.setGetRandomValues === "function" && globalThis.crypto?.getRandomValues) {
    module.setGetRandomValues((_Module, array) => globalThis.crypto.getRandomValues(array));
  }

  return module;
}

function withBrowserLikeEnvironment(fn) {
  const global = globalThis;
  const previous = {
    window: global.window,
    document: global.document,
    sessionStorage: global.sessionStorage,
    importScripts: global.importScripts,
    WorkerGlobalScope: global.WorkerGlobalScope,
  };
  try {
    global.window = { sessionStorage: {} };
    global.document = { createElement: () => ({}) };
    global.sessionStorage = {};
    global.importScripts = 1;
    global.WorkerGlobalScope = undefined;
    return fn();
  } finally {
    if (previous.window === undefined) {
      delete global.window;
    } else {
      global.window = previous.window;
    }
    if (previous.document === undefined) {
      delete global.document;
    } else {
      global.document = previous.document;
    }
    if (previous.sessionStorage === undefined) {
      delete global.sessionStorage;
    } else {
      global.sessionStorage = previous.sessionStorage;
    }
    if (previous.importScripts === undefined) {
      delete global.importScripts;
    } else {
      global.importScripts = previous.importScripts;
    }
    if (previous.WorkerGlobalScope === undefined) {
      delete global.WorkerGlobalScope;
    } else {
      global.WorkerGlobalScope = previous.WorkerGlobalScope;
    }
  }
}
