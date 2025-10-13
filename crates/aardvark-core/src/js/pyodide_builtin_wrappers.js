// Minimal helpers referenced by the patched Pyodide Emscripten bundle.
// Helper utilities expected by the patched Pyodide asm bundle. These mirror the
// minimal surface expected by the upstream sandbox but intentionally omit the
// more security-sensitive wiring (unsafe eval) for now.

const DEFAULT_LOCATION = {
  href: "https://pyodide.local/",
  origin: "https://pyodide.local",
  protocol: "https:",
  host: "pyodide.local",
  hostname: "pyodide.local",
  port: "",
  pathname: "/",
  search: "",
  hash: "",
};

export const location = globalThis.location ?? { ...DEFAULT_LOCATION };

let lastNow = Date.now();
let extraTicks = 0;
export function monotonicDateNow() {
  const now = Date.now();
  if (now === lastNow) {
    extraTicks += 1;
  } else {
    lastNow = now;
    extraTicks = 0;
  }
  return now + extraTicks;
}

export function addEventListener() {
  // The bootstrap only checks for existence.
}

let randomValueSource = (_Module, array) => {
  if (globalThis.crypto?.getRandomValues) {
    return globalThis.crypto.getRandomValues(array);
  }
  for (let i = 0; i < array.length; i += 1) {
    array[i] = Math.floor(Math.random() * 256);
  }
  return array;
};

export function setGetRandomValues(fn) {
  randomValueSource = fn;
}

export function getRandomValues(Module, array) {
  return randomValueSource(Module, array);
}

let timerApi = {
  setTimeout: globalThis.setTimeout?.bind(globalThis) ?? (() => 0),
  clearTimeout: globalThis.clearTimeout?.bind(globalThis) ?? (() => {}),
  setInterval: globalThis.setInterval?.bind(globalThis) ?? (() => 0),
  clearInterval: globalThis.clearInterval?.bind(globalThis) ?? (() => {}),
};

export function setSetTimeout(st, ct, si, ci) {
  timerApi = {
    setTimeout: st,
    clearTimeout: ct,
    setInterval: si,
    clearInterval: ci,
  };
}

export function newWasmModule(bytes) {
  return new WebAssembly.Module(bytes);
}

export async function wasmInstantiate(moduleOrBytes, imports) {
  if (moduleOrBytes instanceof WebAssembly.Module) {
    const instance = await WebAssembly.instantiate(moduleOrBytes, imports);
    return { module: moduleOrBytes, instance };
  }
  const { module, instance } = await WebAssembly.instantiate(moduleOrBytes, imports);
  return { module, instance };
}

export function reportUndefinedSymbolsPatched(Module) {
  if (typeof Module.reportUndefinedSymbols === "function") {
    Module.reportUndefinedSymbols();
  }
}

export function patchDynlibLookup(Module, libName) {
  if (Module?.FS?.readFile) {
    try {
      return Module.FS.readFile(`/usr/lib/${libName}`);
    } catch (err) {
      console.error(`Failed to read dynamic library ${libName}`, err);
      throw err;
    }
  }
  throw new Error(`Dynamic library ${libName} is not available`);
}

export function patchedApplyFunc(_API, func, thisArg, args) {
  return Function.prototype.apply.call(func, thisArg, args);
}

export function patched_PyEM_CountFuncParams(Module, func) {
  if (typeof Module._PyEM_CountFuncParams === "function") {
    return Module._PyEM_CountFuncParams(func);
  }
  return typeof func === "function" ? func.length : 0;
}

export function finishSetup() {
  // Workerd flips an internal flag here; nothing for us to do yet.
}

export function setUnsafeEval() {
  // Unsafe eval is intentionally unavailable until we design a safe surface.
}

export { timerApi as _timerApi };
