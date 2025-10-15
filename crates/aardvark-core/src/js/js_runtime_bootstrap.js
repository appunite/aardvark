const hostCapabilityState = {
  enabled: new Set(),
};

function normalizeCapabilityName(value) {
  return String(value ?? "").trim().toLowerCase();
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

globalThis.__aardvarkPublishBuffer = function publishBuffer() {
  // JavaScript engine does not expose shared buffers yet.
  return undefined;
};

globalThis.__aardvarkCollectSharedBuffers = function collectSharedBuffers() {
  return [];
};

globalThis.__aardvarkReleaseSharedBuffers = function releaseSharedBuffers() {
  return undefined;
};

globalThis.__aardvarkResetSharedBuffers = function resetSharedBuffers() {
  return undefined;
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
