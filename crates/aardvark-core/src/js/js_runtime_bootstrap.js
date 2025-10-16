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
