// Per-call cap on WebAssembly memory. Installed once per context, before
// Pyodide or any guest code runs, and evaluated as a function expression that
// the host calls with its native `allow(requestedTotalBytes)` check. The
// check is captured in this closure, never published on the global object, so
// guest code cannot replace it.
//
// Pyodide grows its heap only through `wasmMemory.grow()` (Emscripten's
// `emscripten_resize_heap`), so refusing a grow makes `malloc` return NULL and
// Python raise MemoryError. `WebAssembly.Memory` and its `grow` are replaced
// with non-writable, non-configurable wrappers; every intrinsic the wrappers
// use is captured here first, so later monkeypatching cannot steer them.
//
// The returned function reports the total size of every tracked memory, which
// the host checks again after each call.
(function installMemoryGuard(allow) {
  "use strict";
  const PAGE_BYTES = 65536;
  const NativeMemory = WebAssembly.Memory;
  const nativeGrow = NativeMemory.prototype.grow;
  const bufferGetter = Object.getOwnPropertyDescriptor(NativeMemory.prototype, "buffer").get;
  const byteLengthGetter = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength").get;
  // Pyodide's heap is a shared memory, backed by a SharedArrayBuffer.
  const sharedByteLengthGetter =
    typeof SharedArrayBuffer === "function"
      ? Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, "byteLength").get
      : undefined;
  const reflectApply = Reflect.apply;
  const reflectConstruct = Reflect.construct;
  const defineProperty = Object.defineProperty;
  const WeakRefCtor = WeakRef;
  const weakRefDeref = WeakRef.prototype.deref;
  const RangeErrorCtor = RangeError;
  const TypeErrorCtor = TypeError;
  const tracked = [];

  function bytesOf(memory) {
    const buffer = reflectApply(bufferGetter, memory, []);
    try {
      return reflectApply(byteLengthGetter, buffer, []);
    } catch (error) {
      if (sharedByteLengthGetter === undefined) {
        throw error;
      }
      return reflectApply(sharedByteLengthGetter, buffer, []);
    }
  }

  function trackedBytes(exclude) {
    let total = 0;
    for (let i = 0; i < tracked.length; i++) {
      const memory = reflectApply(weakRefDeref, tracked[i], []);
      if (memory !== undefined && memory !== exclude) {
        total += bytesOf(memory);
      }
    }
    return total;
  }

  function grow(delta) {
    // Coerce once and hand the primitive on, so a guest `valueOf` can't
    // report one size to the check and another to the native grow.
    const pages = +delta;
    const current = bytesOf(this);
    const requested = trackedBytes(this) + current + pages * PAGE_BYTES;
    if (!allow(requested)) {
      throw new RangeErrorCtor("WebAssembly.Memory.grow(): memory limit exceeded");
    }
    return reflectApply(nativeGrow, this, [pages]);
  }

  function Memory(descriptor) {
    if (new.target === undefined) {
      throw new TypeErrorCtor("WebAssembly.Memory must be invoked with 'new'");
    }
    if (descriptor === null || typeof descriptor !== "object") {
      throw new TypeErrorCtor("WebAssembly.Memory(): Argument 0 must be a memory descriptor");
    }
    // Read every field once into a plain object for the same reason as above.
    const sanitized = { initial: +descriptor.initial };
    const maximum = descriptor.maximum;
    if (maximum !== undefined) {
      sanitized.maximum = +maximum;
    }
    const shared = descriptor.shared;
    if (shared !== undefined) {
      sanitized.shared = !!shared;
    }
    if (!allow(trackedBytes(undefined) + sanitized.initial * PAGE_BYTES)) {
      throw new RangeErrorCtor("WebAssembly.Memory(): memory limit exceeded");
    }
    const memory = reflectConstruct(NativeMemory, [sanitized], new.target);
    tracked.push(new WeakRefCtor(memory));
    return memory;
  }

  const locked = { writable: false, enumerable: false, configurable: false };
  defineProperty(Memory, "prototype", { value: NativeMemory.prototype, ...locked });
  defineProperty(NativeMemory.prototype, "constructor", { value: Memory, ...locked });
  defineProperty(NativeMemory.prototype, "grow", { value: grow, ...locked });
  defineProperty(WebAssembly, "Memory", { value: Memory, ...locked });

  return function trackedMemoryBytes() {
    return trackedBytes(undefined);
  };
})
