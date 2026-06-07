(() => {
  const state = { proxy: null, buffer: null };
  function releaseBuffer() {
    try {
      state.buffer?.release?.();
    } catch (_err) {
      // ignore
    }
    state.buffer = null;
  }
  function destroyProxy() {
    try {
      state.proxy?.destroy?.();
    } catch (_err) {
      // ignore
    }
    state.proxy = null;
  }
  function resultBufferType(kind) {
    if (kind === "f32-array") {
      return "f32";
    }
    if (kind === "f64-array") {
      return "f64";
    }
    return "u8";
  }
  function bytesViewFor(data, buffer) {
    if (ArrayBuffer.isView(data)) {
      const elementSize = Number(data.BYTES_PER_ELEMENT ?? 1);
      const byteLength = Number(buffer?.nbytes ?? data.byteLength);
      let offset = Number(buffer?.offset ?? 0) * elementSize;
      if (!Number.isFinite(offset) || offset === 0) {
        const inferredOffset = data.byteLength - byteLength;
        if (inferredOffset > 0) {
          offset = inferredOffset;
        }
      }
      if (!Number.isFinite(byteLength) || byteLength < 0 || offset < 0 || offset + byteLength > data.byteLength) {
        return null;
      }
      const byteOffset = data.byteOffset + offset;
      return new Uint8Array(data.buffer, byteOffset, byteLength);
    }
    if (data instanceof ArrayBuffer) {
      return new Uint8Array(data);
    }
    return null;
  }
  function normalizeBytesLike(value, kind) {
    if (value == null) {
      throw new TypeError("JSON result buffer payload must be provided");
    }
    let proxy = null;
    let buffer = null;
    let candidate = value;
    if (candidate && typeof candidate.getBuffer === "function") {
      proxy = candidate;
      buffer = proxy.getBuffer(resultBufferType(kind));
      try {
        const view = bytesViewFor(buffer.data, buffer);
        if (view == null) {
          throw new TypeError("JSON result buffer payload must expose typed-array data");
        }
        return { view: view.slice(), proxy: null, buffer: null };
      } finally {
        buffer?.release?.();
        proxy?.destroy?.();
      }
    }
    if (candidate && typeof candidate.toJs === "function") {
      proxy = candidate;
      candidate = proxy.toJs({ create_proxies: false });
    }
    if (candidate instanceof Uint8Array) {
      return { view: candidate, proxy, buffer };
    }
    if (ArrayBuffer.isView(candidate)) {
      return {
        view: new Uint8Array(
          candidate.buffer,
          candidate.byteOffset ?? 0,
          candidate.byteLength ?? candidate.length ?? 0
        ),
        proxy,
        buffer,
      };
    }
    if (candidate instanceof ArrayBuffer) {
      return { view: new Uint8Array(candidate), proxy, buffer };
    }
    throw new TypeError("JSON result buffer payload must be bytes-like");
  }
  function normalizeMetadata(value) {
    if (value == null) {
      return null;
    }
    if (value && typeof value.toJs === "function") {
      const proxy = value;
      try {
        const converted = proxy.toJs({ create_proxies: false });
        proxy.destroy?.();
        return converted ?? null;
      } catch (error) {
        proxy.destroy?.();
        throw error;
      }
    }
    return value;
  }
  globalThis.__aardvarkSetJsonResultBuffer = function setJsonResultBuffer(kind, data, metadata) {
    const { view, proxy, buffer } = normalizeBytesLike(data, String(kind));
    releaseBuffer();
    destroyProxy();
    state.buffer = buffer ?? null;
    state.proxy = proxy ?? null;
    globalThis.__aardvarkJsonResultKind = String(kind);
    globalThis.__aardvarkJsonResultValue = view;
    globalThis.__aardvarkJsonResultMetadata = normalizeMetadata(metadata);
  };
  globalThis.__aardvarkClearJsonResultBuffer = function clearJsonResultBuffer() {
    releaseBuffer();
    destroyProxy();
    globalThis.__aardvarkJsonResultKind = null;
    globalThis.__aardvarkJsonResultValue = null;
    globalThis.__aardvarkJsonResultMetadata = null;
  };
})();
