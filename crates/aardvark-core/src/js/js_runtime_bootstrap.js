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

function setHostCapabilities(list) {
  hostCapabilityState.enabled.clear();
  if (Array.isArray(list)) {
    for (const entry of list) {
      const canonical = normalizeCapabilityName(entry);
      if (canonical) {
        hostCapabilityState.enabled.add(canonical);
      }
    }
  }
}

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
  names: [],
};

globalThis.__aardvarkClearInputBuffers = function clearInputBuffers() {
  inputBufferState.buffers = Object.create(null);
  inputBufferState.metadata = Object.create(null);
  inputBufferState.names = [];
  globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
  globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
  globalThis.__aardvarkInputBufferNames = inputBufferState.names;
  globalThis.__aardvarkRawctxInputsAvailable = false;
  globalThis.__aardvarkRawctxInputViewMode = null;
  globalThis.__aardvarkSharedBufferMetadataMode = "full";
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
  if (!Object.prototype.hasOwnProperty.call(inputBufferState.buffers, key)) {
    inputBufferState.names.push(key);
  }
  inputBufferState.buffers[key] = buffer;
  if (metadata === undefined || metadata === null) {
    delete inputBufferState.metadata[key];
  } else {
    inputBufferState.metadata[key] = metadata;
  }
  globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
  globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
  globalThis.__aardvarkInputBufferNames = inputBufferState.names;
  return undefined;
};

globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
globalThis.__aardvarkInputBufferNames = inputBufferState.names;

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

function attachBufferId(view, id) {
  if (!view || typeof view !== "object") {
    return;
  }
  if (Object.prototype.hasOwnProperty.call(view, "__aardvarkBufferId") && view.__aardvarkBufferId === id) {
    return;
  }
  try {
    Object.defineProperty(view, "__aardvarkBufferId", {
      value: id,
      enumerable: false,
      configurable: true,
      writable: false,
    });
  } catch (_err) {
    view.__aardvarkBufferId = id;
  }
}

function ensureUint8Array(value) {
  if (value instanceof Uint8Array) {
    return value;
  }
  if (ArrayBuffer.isView(value)) {
    if (typeof value.byteLength !== "number") {
      throw new TypeError("ArrayBuffer view missing byteLength property");
    }
    return new Uint8Array(value.buffer, value.byteOffset ?? 0, value.byteLength);
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
      let decoded;
      try {
        decoded = atob(text);
      } catch (error) {
        throw new TypeError("invalid base64 payload");
      }
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

    const finalArgs = args.length > 0 ? args : callArgs;
    if (Object.keys(kwargs).length > 0) {
      finalArgs.push(kwargs);
    }
    const invocation = fn.apply(this, finalArgs);
    return applyRawctxOutputs(spec, invocation);
  };
};

function normalizeSharedBufferInput(data) {
  if (data instanceof Uint8Array) {
    return data;
  }
  if (ArrayBuffer.isView(data)) {
    const view = data;
    const slice = new Uint8Array(view.buffer, view.byteOffset ?? 0, view.byteLength);
    if (Object.prototype.hasOwnProperty.call(view, "__aardvarkBufferId")) {
      attachBufferId(slice, view.__aardvarkBufferId);
    }
    return slice;
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

function normalizeSharedBufferMetadata(metadata) {
  if (globalThis.__aardvarkSharedBufferMetadataMode === "none") {
    return null;
  }
  return normalizeMetadataInput(metadata);
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

globalThis.__aardvarkAcquireOutputBuffer = function acquireOutputBuffer(bufferId, size, metadata) {
  requireCapability("rawctx_buffers");
  const length = Number(size);
  if (!Number.isFinite(length)) {
    throw new TypeError("size must be a finite number");
  }
  if (length < 0) {
    throw new RangeError("size must be non-negative");
  }
  const byteLength = Math.trunc(length);
  const assigned =
    bufferId != null && bufferId !== ""
      ? String(bufferId)
      : `buffer-${sharedBufferState.nextId++}`;
  const backing = typeof SharedArrayBuffer !== "undefined"
    ? new SharedArrayBuffer(byteLength)
    : new ArrayBuffer(byteLength);
  const view = new Uint8Array(backing);
  attachBufferId(view, assigned);
  const metaObject = normalizeSharedBufferMetadata(metadata);
  sharedBufferState.map.set(assigned, {
    view,
    metadata: metaObject,
  });
  recordSharedBufferEvent("acquire", assigned, view.byteLength, metaObject);
  return view;
};

globalThis.__aardvarkPublishBuffer = function publishBuffer(bufferId, data, metadata) {
  requireCapability("rawctx_buffers");
  const explicitId = bufferId != null && bufferId !== "" ? String(bufferId) : null;
  const metaObject = normalizeSharedBufferMetadata(metadata);

  let candidateId = null;
  if (data && typeof data === "object" && Object.prototype.hasOwnProperty.call(data, "__aardvarkBufferId")) {
    candidateId = String(data.__aardvarkBufferId);
  }
  const assigned = explicitId ?? candidateId ?? `buffer-${sharedBufferState.nextId++}`;

  if (candidateId && sharedBufferState.map.has(candidateId)) {
    const entry = sharedBufferState.map.get(candidateId);
    if (entry) {
      if (metaObject !== null) {
        entry.metadata = metaObject;
      }
      recordSharedBufferEvent("publish", candidateId, entry.view.byteLength, entry.metadata ?? null);
      return candidateId;
    }
  }

  const view = normalizeSharedBufferInput(data ?? new Uint8Array());
  attachBufferId(view, assigned);
  sharedBufferState.map.set(assigned, {
    view,
    metadata: metaObject,
  });
  recordSharedBufferEvent("publish", assigned, view.byteLength, metaObject);
  return assigned;
};

function collectSharedBuffersUnchecked() {
  const result = [];
  for (const [id, entry] of sharedBufferState.map.entries()) {
    result.push({
      id,
      buffer: entry.view,
      metadata: entry.metadata ?? null,
    });
  }
  return result;
}

function drainSharedBuffersUnchecked() {
  const result = [];
  for (const [id, entry] of Array.from(sharedBufferState.map.entries())) {
    result.push({
      id,
      buffer: entry.view,
      metadata: entry.metadata ?? null,
    });
    recordSharedBufferEvent("release", id, entry?.view?.byteLength ?? 0, entry?.metadata ?? null);
    sharedBufferState.map.delete(id);
  }
  return result;
}

function releaseSharedBuffersUnchecked(ids) {
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
}

globalThis.__aardvarkCollectSharedBuffers = function collectSharedBuffers() {
  requireCapability("rawctx_buffers");
  return collectSharedBuffersUnchecked();
};

globalThis.__aardvarkDrainSharedBuffers = function drainSharedBuffers() {
  requireCapability("rawctx_buffers");
  return drainSharedBuffersUnchecked();
};

globalThis.__aardvarkReleaseSharedBuffers = function releaseSharedBuffers(ids) {
  requireCapability("rawctx_buffers");
  releaseSharedBuffersUnchecked(ids);
};

globalThis.__aardvarkResetSharedBuffers = function resetSharedBuffers() {
  requireCapability("rawctx_buffers");
  releaseSharedBuffersUnchecked();
};

const filesystemState = {
  mode: "read",
  quotaBytes: null,
  usageBytes: 0,
};

function setFilesystemPolicy(policy) {
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
}

function resetFilesystem() {
  filesystemState.usageBytes = 0;
  return filesystemState.usageBytes;
}

function getFilesystemUsage() {
  return filesystemState.usageBytes;
}

globalThis.__aardvarkHostHooks = Object.freeze({
  setHostCapabilities,
  filesystem: Object.freeze({
    setPolicy: setFilesystemPolicy,
    reset: resetFilesystem,
    getUsage: getFilesystemUsage,
  }),
  sharedBuffers: Object.freeze({
    collect: collectSharedBuffersUnchecked,
    drain: drainSharedBuffersUnchecked,
    release: releaseSharedBuffersUnchecked,
    reset() {
      releaseSharedBuffersUnchecked();
    },
  }),
});
