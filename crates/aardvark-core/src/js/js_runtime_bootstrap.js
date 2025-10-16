const hostCapabilityState = {
  enabled: new Set(),
};

function normalizeCapabilityName(value) {
  return String(value ?? "").trim().toLowerCase();
}

function requireCapability(name) {
  const canonical = normalizeCapabilityName(name);
  if (!hostCapabilityState.enabled.has(canonical)) {
    throw new Error(`host capability '${canonical}' is not enabled`);
  }
}

globalThis.__aardvarkSetHostCapabilities = function setHostCapabilities(list) {
  hostCapabilityState.enabled.clear();
  if (Array.isArray(list)) {
    for (const entry of list) {
      const canonical = normalizeCapabilityName(entry);
      if (canonical) {
        hostCapabilityState.enabled.add(canonical);
      }
    }
  }
};

globalThis.__aardvarkGetJsonInput = function getJsonInput() {
  return globalThis.__aardvarkJsonInput ?? null;
};

globalThis.__aardvarkConsumeJsonInput = function consumeJsonInput() {
  const value = globalThis.__aardvarkJsonInput ?? null;
  globalThis.__aardvarkJsonInput = undefined;
  return value;
};

const sharedBufferState = {
  map: new Map(),
  nextId: 1,
};

const inputBufferState = {
  buffers: Object.create(null),
  metadata: Object.create(null),
};

globalThis.__aardvarkClearInputBuffers = function clearInputBuffers() {
  inputBufferState.buffers = Object.create(null);
  inputBufferState.metadata = Object.create(null);
  globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
  globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
  return undefined;
};

globalThis.__aardvarkRegisterInputBuffer = function registerInputBuffer(
  name,
  buffer,
  metadata,
) {
  const key = String(name ?? "").trim();
  if (!key) {
    throw new Error("input buffer requires a non-empty name");
  }
  if (!(buffer instanceof Uint8Array)) {
    throw new TypeError("input buffer payload must be a Uint8Array");
  }
  inputBufferState.buffers[key] = buffer;
  if (metadata === undefined || metadata === null) {
    delete inputBufferState.metadata[key];
  } else {
    inputBufferState.metadata[key] = metadata;
  }
  globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
  globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
  return undefined;
};

globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
globalThis.__aardvarkInputMetadata = inputBufferState.metadata;

const rawctxState = {
  spec: null,
};

const textDecoderCache = new Map();
function getTextDecoder(label) {
  const key = (label || "utf-8").toLowerCase();
  if (!textDecoderCache.has(key)) {
    textDecoderCache.set(key, new TextDecoder(key));
  }
  return textDecoderCache.get(key);
}

function sliceUint8(arrayBuffer, offset, length) {
  return new Uint8Array(arrayBuffer, offset, length);
}

function ensureUint8Array(value) {
  if (value instanceof Uint8Array) {
    return value;
  }
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset ?? 0, value.byteLength ?? value.length ?? 0);
  }
  if (value instanceof ArrayBuffer || value instanceof SharedArrayBuffer) {
    return new Uint8Array(value);
  }
  if (Array.isArray(value)) {
    return Uint8Array.from(value);
  }
  throw new TypeError("expected a bytes-like value");
}

function decodeNumeric(dataView, method, littleEndian) {
  switch (method) {
    case "getFloat32":
      return dataView.getFloat32(0, littleEndian);
    case "getFloat64":
      return dataView.getFloat64(0, littleEndian);
    case "getInt32":
      return dataView.getInt32(0, littleEndian);
    case "getUint32":
      return dataView.getUint32(0, littleEndian);
    case "getInt16":
      return dataView.getInt16(0, littleEndian);
    case "getUint16":
      return dataView.getUint16(0, littleEndian);
    case "getInt8":
      return dataView.getInt8(0);
    case "getUint8":
      return dataView.getUint8(0);
    default:
      throw new Error(`unsupported decoder method ${method}`);
  }
}

function decodeRawctxScalar(binding, payload) {
  const decoder = (binding?.decoder || "memoryview").toLowerCase();
  const options = binding?.options || {};
  const buffer = ensureUint8Array(payload?.data ?? new Uint8Array());
  const metadata = payload?.metadata ?? null;

  const littleEndian = (options.byteorder || options.endianness || "little").toLowerCase() !== "big";
  const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);

  switch (decoder) {
    case "memoryview":
    case "bytes":
    case "binary":
      return { value: buffer, metadata, rawPayload: payload };
    case "utf8":
    case "text": {
      const encoding = options.encoding || "utf-8";
      const value = getTextDecoder(encoding).decode(buffer);
      return { value, metadata, rawPayload: payload };
    }
    case "json": {
      const encoding = options.encoding || "utf-8";
      const text = getTextDecoder(encoding).decode(buffer);
      const value = JSON.parse(text);
      return { value, metadata, rawPayload: payload };
    }
    case "float32":
    case "f32":
      return { value: decodeNumeric(view, "getFloat32", littleEndian), metadata, rawPayload: payload };
    case "float64":
    case "f64":
      return { value: decodeNumeric(view, "getFloat64", littleEndian), metadata, rawPayload: payload };
    case "int32":
    case "i32":
      return { value: decodeNumeric(view, "getInt32", littleEndian), metadata, rawPayload: payload };
    case "uint32":
    case "u32":
      return { value: decodeNumeric(view, "getUint32", littleEndian), metadata, rawPayload: payload };
    case "int16":
    case "i16":
      return { value: decodeNumeric(view, "getInt16", littleEndian), metadata, rawPayload: payload };
    case "uint16":
    case "u16":
      return { value: decodeNumeric(view, "getUint16", littleEndian), metadata, rawPayload: payload };
    case "int8":
    case "i8":
      return { value: decodeNumeric(view, "getInt8", littleEndian), metadata, rawPayload: payload };
    case "uint8":
    case "u8":
      return { value: decodeNumeric(view, "getUint8", littleEndian), metadata, rawPayload: payload };
    case "bool":
    case "boolean": {
      const value = buffer.length > 0 && buffer[0] !== 0;
      return { value, metadata, rawPayload: payload };
    }
    case "base64":
    case "b64": {
      const encoding = options.encoding || "utf-8";
      const text = getTextDecoder(encoding).decode(buffer);
      const decoded = atob(text);
      const result = new Uint8Array(decoded.length);
      for (let i = 0; i < decoded.length; i += 1) {
        result[i] = decoded.charCodeAt(i);
      }
      return { value: result, metadata, rawPayload: payload };
    }
    case "identity":
      return { value: buffer, metadata, rawPayload: payload };
    default:
      return { value: buffer, metadata, rawPayload: payload };
  }
}

function materializeRawPayload(binding, sourceBuffers, sourceMetadata) {
  const field = binding.field;
  const data = sourceBuffers[field];
  if (!data) {
    return null;
  }
  return {
    data,
    metadata: sourceMetadata[field] ?? null,
  };
}

function applyRawctxOutputs(spec, result) {
  const outputs = spec.outputs && spec.outputs.length
    ? spec.outputs
    : spec.output
      ? [spec.output]
      : [];

  if (!outputs.length) {
    return result;
  }

  const handleResult = (value) => {
    let current = value;
    for (const output of outputs) {
      const mode = (output?.mode || "publish-buffer").toLowerCase();
      if (mode !== "publish-buffer") {
        continue;
      }
      let dataValue = current;
      if (dataValue == null) {
        const whenNone = (output?.when_none || "skip").toLowerCase();
        if (whenNone === "error") {
          throw new Error("rawctx output requires a non-null result");
        }
        if (whenNone === "publish-empty") {
          dataValue = new Uint8Array();
        } else if (whenNone === "propagate") {
          continue;
        } else {
          continue;
        }
      }

      const transform = (output?.transform || "memoryview").toLowerCase();
      let bytes;
      if (transform === "utf8") {
        if (typeof dataValue !== "string") {
          throw new TypeError("rawctx output expected string for utf8 transform");
        }
        const encoder = new TextEncoder();
        bytes = encoder.encode(dataValue);
      } else if (transform === "bytes" || transform === "memoryview") {
        bytes = ensureUint8Array(dataValue);
      } else if (transform === "identity") {
        bytes = dataValue;
      } else {
        throw new Error(`unsupported rawctx output transform: ${transform}`);
      }

      globalThis.__aardvarkPublishBuffer(
        output?.id,
        bytes,
        output?.metadata ?? null,
      );

      const behaviour = (output?.return_behavior || "none").toLowerCase();
      if (behaviour === "original") {
        current = value;
      } else if (behaviour === "buffer") {
        current = bytes;
      } else {
        current = null;
      }
    }
    return current;
  };

  if (result && typeof result.then === "function") {
    return Promise.resolve(result).then(handleResult);
  }
  return handleResult(result);
}

globalThis.__aardvarkSetRawctxSpec = function setRawctxSpec(spec) {
  rawctxState.spec = spec || null;
};

globalThis.__aardvarkWrapRawctxFunction = function wrapRawctxFunction(fn, moduleName, exportName) {
  if (typeof fn !== "function") {
    rawctxState.spec = null;
    return fn;
  }
  const spec = rawctxState.spec;
  if (!spec) {
    return fn;
  }
  const expected = (spec.entrypoint || "").trim().toLowerCase();
  const actualModule = String(moduleName ?? "").trim().toLowerCase();
  const actualExport = String(exportName ?? "").trim().toLowerCase();
  if (expected && expected !== `${actualModule}:${actualExport}`) {
    return fn;
  }

  rawctxState.spec = null;
  return function aardvarkRawctxWrapper(...callArgs) {
    const buffers = globalThis.__aardvarkInputBuffers || {};
    const metadataSource = globalThis.__aardvarkInputMetadata || {};
    const args = [];
    const kwargs = {};

    for (const binding of spec.inputs || []) {
      const payload = materializeRawPayload(binding, buffers, metadataSource);
      let value;
      let metadata = null;
      let rawPayload = null;

      if (payload == null) {
        if (Object.prototype.hasOwnProperty.call(binding, "default")) {
          value = binding.default;
        } else if (binding.optional) {
          value = null;
        } else {
          throw new Error(`rawctx input '${binding.field}' is required`);
        }
      } else {
        const decoded = decodeRawctxScalar(binding, payload);
        value = decoded.value;
        metadata = decoded.metadata;
        rawPayload = decoded.rawPayload;
        if (value == null && Object.prototype.hasOwnProperty.call(binding, "default")) {
          value = binding.default;
        }
      }

      if (binding.metadata_arg) {
        kwargs[binding.metadata_arg] = metadata;
      }
      if (binding.raw_arg) {
        kwargs[binding.raw_arg] = rawPayload;
      }

      const argName = binding.arg;
      const mode = (binding.mode || "keyword").toLowerCase();
      if (mode === "positional") {
        args.push(value);
      } else if (argName) {
        kwargs[argName] = value;
      } else {
        args.push(value);
      }
    }

    const finalArgs = args.slice();
    if (Object.keys(kwargs).length > 0) {
      finalArgs.push(kwargs);
    }
    const invocation = fn.apply(this, finalArgs.length ? finalArgs : callArgs);
    return applyRawctxOutputs(spec, invocation);
  };
};

function normalizeSharedBufferInput(data) {
  if (data instanceof Uint8Array) {
    return data.subarray(0, data.byteLength);
  }
  if (ArrayBuffer.isView(data)) {
    const view = data;
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  if (typeof SharedArrayBuffer !== "undefined" && data instanceof SharedArrayBuffer) {
    return new Uint8Array(data);
  }
  if (typeof data === "string") {
    return new TextEncoder().encode(data);
  }
  if (data == null) {
    return new Uint8Array();
  }
  if (Array.isArray(data)) {
    return Uint8Array.from(data);
  }
  throw new TypeError(
    "publish-buffer expects a Uint8Array, ArrayBuffer view, ArrayBuffer, string, or array",
  );
}

function normalizeMetadataInput(metadata) {
  if (metadata == null) {
    return null;
  }
  try {
    return JSON.parse(JSON.stringify(metadata));
  } catch (_err) {
    throw new TypeError("metadata must be JSON-serializable");
  }
}

function recordSharedBufferEvent(event, id, size, metadata) {
  if (typeof globalThis.__aardvarkRecordBufferEvent === "function") {
    try {
      globalThis.__aardvarkRecordBufferEvent(event, id, size, metadata ?? null);
    } catch (error) {
      try {
        console.warn("[buffers] failed to record event", event, id, error);
      } catch (_logErr) {
        // ignore logging failures
      }
    }
  }
}

globalThis.__aardvarkPublishBuffer = function publishBuffer(bufferId, data, metadata) {
  requireCapability("rawctx_buffers");
  const assigned =
    bufferId != null && bufferId !== ""
      ? String(bufferId)
      : `buffer-${sharedBufferState.nextId++}`;
  const view = normalizeSharedBufferInput(data);
  const metaObject = normalizeMetadataInput(metadata);
  sharedBufferState.map.set(assigned, {
    view,
    metadata: metaObject,
  });
  recordSharedBufferEvent("publish", assigned, view.byteLength, metaObject);
  return assigned;
};

globalThis.__aardvarkCollectSharedBuffers = function collectSharedBuffers() {
  requireCapability("rawctx_buffers");
  const result = [];
  for (const [id, entry] of sharedBufferState.map.entries()) {
    result.push({
      id,
      buffer: entry.view,
      metadata: entry.metadata ?? null,
    });
  }
  return result;
};

globalThis.__aardvarkReleaseSharedBuffers = function releaseSharedBuffers(ids) {
  requireCapability("rawctx_buffers");
  const pending = Array.isArray(ids) ? ids : Array.from(sharedBufferState.map.keys());
  for (const id of pending) {
    const key = String(id);
    if (!sharedBufferState.map.has(key)) {
      continue;
    }
    const entry = sharedBufferState.map.get(key);
    recordSharedBufferEvent("release", key, entry?.view?.byteLength ?? 0, entry?.metadata ?? null);
    sharedBufferState.map.delete(key);
  }
};

globalThis.__aardvarkResetSharedBuffers = function resetSharedBuffers() {
  globalThis.__aardvarkReleaseSharedBuffers();
};

const filesystemState = {
  mode: "read",
  quotaBytes: null,
  usageBytes: 0,
};

globalThis.__aardvarkFilesystemSetPolicy = function setFilesystemPolicy(policy) {
  const mode =
    policy && typeof policy.mode === "string"
      ? policy.mode.toLowerCase()
      : "read";
  filesystemState.mode = mode === "readwrite" ? "readWrite" : "read";
  if (
    policy &&
    Object.prototype.hasOwnProperty.call(policy, "quotaBytes") &&
    policy.quotaBytes != null
  ) {
    const numeric = Number(policy.quotaBytes);
    filesystemState.quotaBytes = Number.isFinite(numeric) && numeric >= 0 ? numeric : null;
  } else {
    filesystemState.quotaBytes = null;
  }
  filesystemState.usageBytes = 0;
  return filesystemState.usageBytes;
};

globalThis.__aardvarkFilesystemReset = function resetFilesystem() {
  filesystemState.usageBytes = 0;
  return filesystemState.usageBytes;
};

globalThis.__aardvarkFilesystemGetUsage = function getFilesystemUsage() {
  return filesystemState.usageBytes;
};
