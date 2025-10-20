// Minimal bootstrap for Pyodide using the patched asm module and custom setup.
import { instantiatePyodideModule } from "./pyodide_emscripten_setup.js";
import {
  loadTransitivePackages,
  ensureSessionMetadata,
  adjustSysPathPostBootstrap,
} from "./pyodide_packages.js";

const noop = () => {};
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();
const BASE64_ALPHABET =
  "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_DECODE_TABLE = (() => {
  const table = new Uint8Array(256);
  table.fill(255);
  for (let i = 0; i < BASE64_ALPHABET.length; i += 1) {
    table[BASE64_ALPHABET.charCodeAt(i)] = i;
  }
  return table;
})();
const BASE64_PADDING = "=".charCodeAt(0);

function isBase64Whitespace(code) {
  return (
    code === 0x20 || // space
    code === 0x0a || // line feed
    code === 0x0d || // carriage return
    code === 0x09 // tab
  );
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

const DYN_LIB_SUFFIX_RE = /\.so(\.|$)/i;

function makeDirNode(mode = 0o755, modtime = nowSeconds()) {
  return { type: "dir", mode, modtime, children: new Map() };
}

function makeFileNode(data, mode = 0o644, modtime = nowSeconds()) {
  return { type: "file", mode, modtime, data };
}

function splitPath(path) {
  return path
    .split("/")
    .map((part) => part.trim())
    .filter(Boolean);
}

function base64ToUint8Array(base64) {
  if (!base64) {
    return new Uint8Array();
  }
  let sanitizedLength = 0;
  let lastCode = -1;
  let secondLastCode = -1;
  for (let i = 0; i < base64.length; i += 1) {
    const code = base64.charCodeAt(i);
    if (isBase64Whitespace(code)) {
      continue;
    }
    sanitizedLength += 1;
    secondLastCode = lastCode;
    lastCode = code;
  }
  if (sanitizedLength === 0) {
    return new Uint8Array();
  }
  if (sanitizedLength % 4 !== 0) {
    throw new Error("Invalid base64 length");
  }
  const padding =
    (lastCode === BASE64_PADDING ? 1 : 0) +
    (secondLastCode === BASE64_PADDING ? 1 : 0);
  const output = new Uint8Array((sanitizedLength / 4) * 3 - padding);
  let outIndex = 0;
  let quartetIndex = 0;
  let b0 = 0;
  let b1 = 0;
  let b2 = 0;
  for (let i = 0; i < base64.length; i += 1) {
    const code = base64.charCodeAt(i);
    if (isBase64Whitespace(code)) {
      continue;
    }
    if (quartetIndex === 0) {
      b0 = code;
    } else if (quartetIndex === 1) {
      b1 = code;
    } else if (quartetIndex === 2) {
      b2 = code;
    } else {
      const b3 = code;
      const v0 = BASE64_DECODE_TABLE[b0];
      const v1 = BASE64_DECODE_TABLE[b1];
      const v2 =
        b2 === BASE64_PADDING ? 0 : BASE64_DECODE_TABLE[b2];
      const v3 =
        b3 === BASE64_PADDING ? 0 : BASE64_DECODE_TABLE[b3];
      if (
        v0 === 255 ||
        v1 === 255 ||
        (b2 !== BASE64_PADDING && v2 === 255) ||
        (b3 !== BASE64_PADDING && v3 === 255)
      ) {
        throw new Error("Invalid base64 digit");
      }
      const triple = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
      output[outIndex++] = (triple >> 16) & 0xff;
      if (b2 !== BASE64_PADDING) {
        output[outIndex++] = (triple >> 8) & 0xff;
      }
      if (b3 !== BASE64_PADDING) {
        output[outIndex++] = triple & 0xff;
      }
      quartetIndex = -1;
    }
    quartetIndex += 1;
  }
  return output;
}

function uint8ArrayToBase64(bytes) {
  if (!bytes || bytes.length === 0) {
    return "";
  }
  const len = bytes.length;
  const remainder = len % 3;
  const mainLength = len - remainder;
  const segments = new Array(Math.ceil(len / 3));
  let segmentIndex = 0;
  for (let i = 0; i < mainLength; i += 3) {
    const triple = (bytes[i] << 16) | (bytes[i + 1] << 8) | bytes[i + 2];
    segments[segmentIndex++] =
      BASE64_ALPHABET[(triple >> 18) & 63] +
      BASE64_ALPHABET[(triple >> 12) & 63] +
      BASE64_ALPHABET[(triple >> 6) & 63] +
      BASE64_ALPHABET[triple & 63];
  }
  if (remainder === 1) {
    const value = bytes[len - 1];
    segments[segmentIndex++] =
      BASE64_ALPHABET[(value >> 2) & 63] +
      BASE64_ALPHABET[(value << 4) & 63] +
      "==";
  } else if (remainder === 2) {
    const value = (bytes[len - 2] << 8) | bytes[len - 1];
    segments[segmentIndex++] =
      BASE64_ALPHABET[(value >> 10) & 63] +
      BASE64_ALPHABET[(value >> 4) & 63] +
      BASE64_ALPHABET[(value << 2) & 63] +
      "=";
  }
  return segments.join("");
}

const filesystemState = {
  policy: {
    mode: "read",
    quotaBytes: null,
  },
  usageBytes: 0,
  sessionRoot: null,
  resetting: false,
};

let filesystemGuardsInstalled = false;

const hostCapabilityState = {
  enabled: new Set(),
};

function normalizeCapabilityName(value) {
  return String(value ?? "").trim().toLowerCase();
}

function setHostCapabilities(list) {
  hostCapabilityState.enabled = new Set(
    (Array.isArray(list) ? list : [])
      .map(normalizeCapabilityName)
      .filter((name) => name.length > 0)
  );
}

function requireCapability(name) {
  const canonical = normalizeCapabilityName(name);
  if (!hostCapabilityState.enabled.has(canonical)) {
    throw new Error(`host capability '${canonical}' is not enabled`);
  }
}

function resolveSessionRoot(module) {
  const raw = module?.FS?.sessionSitePackages
    ? String(module.FS.sessionSitePackages)
    : "/session/site-packages";
  const sanitized = raw.replace(/\\/g, "/").replace(/\/+/g, "/");
  const normalized = sanitized.endsWith("/") ? sanitized.replace(/\/+$/, "") : sanitized;
  return normalized === "" ? "/" : normalized;
}

function normalizePath(path) {
  const normalized = String(path ?? "")
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/");
  if (normalized.startsWith("/")) {
    return normalized;
  }
  return `/${normalized}`;
}

function pathWithinSession(module, candidate) {
  const root = filesystemState.sessionRoot || resolveSessionRoot(module);
  const normalized = normalizePath(candidate);
  if (!normalized.startsWith("/")) {
    return false;
  }
  if (normalized === root || normalized.startsWith(`${root}/`)) {
    return true;
  }
  return isSessionPath(normalized);
}

function isSessionPath(normalizedPath) {
  if (normalizedPath === "/session") {
    return true;
  }
  return normalizedPath.startsWith("/session/");
}

function isDirMode(module, stat) {
  return module?.FS?.isDir ? module.FS.isDir(stat.mode) : false;
}

function isFileMode(module, stat) {
  return module?.FS?.isFile ? module.FS.isFile(stat.mode) : false;
}

function computeDirectoryUsage(module, rootPath) {
  const root = normalizePath(rootPath);
  let total = 0;
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    let entries;
    try {
      entries = module.FS.readdir(current);
    } catch (_err) {
      continue;
    }
    for (const entry of entries) {
      if (entry === "." || entry === "..") {
        continue;
      }
      const child = normalizePath(`${current}/${entry}`);
      let stats;
      try {
        stats = module.FS.stat(child);
      } catch (_err) {
        continue;
      }
      if (isDirMode(module, stats)) {
        stack.push(child);
      } else if (isFileMode(module, stats)) {
        total += Number(stats.size ?? 0);
      }
    }
  }
  return total;
}

function removeTree(module, rootPath, preserveRoot = false) {
  let entries;
  try {
    entries = module.FS.readdir(rootPath);
  } catch (_err) {
    return;
  }
  for (const entry of entries) {
    if (entry === "." || entry === "..") {
      continue;
    }
    const child = normalizePath(`${rootPath}/${entry}`);
    let stats;
    try {
      stats = module.FS.stat(child);
    } catch (_err) {
      continue;
    }
    if (isDirMode(module, stats)) {
      removeTree(module, child, false);
    } else if (isFileMode(module, stats)) {
      try {
        module.FS.unlink(child);
      } catch (_err) {
        // ignore
      }
    }
  }
  if (!preserveRoot) {
    try {
      module.FS.rmdir(rootPath);
    } catch (_err) {
      // ignore
    }
  }
}

function computeSessionUsage(module) {
  try {
    module.FS.lookupPath("/session");
  } catch (_err) {
    return 0;
  }
  return computeDirectoryUsage(module, "/session");
}

function notifyFilesystemViolation(message, path) {
  const handler = globalThis.__aardvarkFilesystemRecordViolation;
  if (typeof handler === "function") {
    try {
      handler(message ?? "filesystem violation", path ?? null);
    } catch (err) {
      if (globalThis.console?.warn) {
        globalThis.console.warn("[sandbox] filesystem violation hook failed", err);
      }
    }
  }
}

function throwFilesystemError(message, path) {
  notifyFilesystemViolation(message, path);
  const error = new Error(message);
  error.name = "FilesystemPolicyError";
  throw error;
}

function enforceFilesystemWrite(module, path, beforeSize, projectedSize) {
  if (filesystemState.resetting) {
    return;
  }
  if (filesystemState.policy.mode !== "readWrite") {
    throwFilesystemError(
      "filesystem writes are disabled in read-only mode",
      path
    );
  }
  const quota = filesystemState.policy.quotaBytes;
  if (quota != null) {
    const predictedTotal = Math.max(
      0,
      filesystemState.usageBytes - beforeSize + projectedSize
    );
    if (predictedTotal > quota) {
      throwFilesystemError("filesystem quota exceeded", path);
    }
  }
}

function ensureFilesystemGuardsInstalled(module) {
  if (filesystemGuardsInstalled || !module?.FS) {
    return;
  }

  const fs = module.FS;
  const originalWrite = fs.write.bind(fs);
  fs.write = function write(stream, buffer, offset, length, position, canOwn) {
    if (filesystemState.resetting || !stream?.node) {
      return originalWrite(stream, buffer, offset, length, position, canOwn);
    }
    const path = fs.getPath(stream.node);
    if (!pathWithinSession(module, path)) {
      return originalWrite(stream, buffer, offset, length, position, canOwn);
    }
    const before = typeof stream.node.usedBytes === "number" ? stream.node.usedBytes : 0;
    const basePosition = typeof position === "number" ? position : stream.position ?? before;
    const projected = Math.max(before, basePosition + (length ?? 0));
    enforceFilesystemWrite(module, path, before, projected);
    const result = originalWrite(stream, buffer, offset, length, position, canOwn);
    const after = typeof stream.node.usedBytes === "number" ? stream.node.usedBytes : projected;
    filesystemState.usageBytes = Math.max(
      0,
      filesystemState.usageBytes - before + after
    );
    return result;
  };

  const originalUnlink = fs.unlink.bind(fs);
  fs.unlink = function unlink(path) {
    const normalized = normalizePath(path);
    let sizeBefore = 0;
    if (!filesystemState.resetting && pathWithinSession(module, normalized)) {
      try {
        const stats = module.FS.stat(normalized);
        if (isFileMode(module, stats)) {
          sizeBefore = Number(stats.size ?? 0);
        }
      } catch (_err) {
        sizeBefore = 0;
      }
      enforceFilesystemWrite(module, normalized, sizeBefore, 0);
    }
    const result = originalUnlink(path);
    if (pathWithinSession(module, normalized)) {
      filesystemState.usageBytes = Math.max(
        0,
        filesystemState.usageBytes - sizeBefore
      );
    }
    return result;
  };

  const originalRmdir = fs.rmdir.bind(fs);
  fs.rmdir = function rmdir(path) {
    const normalized = normalizePath(path);
    let removedBytes = 0;
    if (pathWithinSession(module, normalized)) {
      removedBytes = computeDirectoryUsage(module, normalized);
      if (!filesystemState.resetting) {
        enforceFilesystemWrite(module, normalized, removedBytes, 0);
      }
    }
    const result = originalRmdir(path);
    if (pathWithinSession(module, normalized)) {
      filesystemState.usageBytes = Math.max(
        0,
        filesystemState.usageBytes - removedBytes
      );
    }
    return result;
  };

  const originalTruncate = fs.truncate.bind(fs);
  fs.truncate = function truncate(path, length) {
    const normalized = normalizePath(path);
    if (!pathWithinSession(module, normalized) || filesystemState.resetting) {
      return originalTruncate(path, length);
    }
    let before = 0;
    try {
      const stats = module.FS.stat(normalized);
      before = Number(stats.size ?? 0);
    } catch (_err) {
      before = 0;
    }
    const projected = Math.max(0, Number(length ?? 0));
    enforceFilesystemWrite(module, normalized, before, projected);
    const result = originalTruncate(path, length);
    let after = projected;
    try {
      const stats = module.FS.stat(normalized);
      after = Number(stats.size ?? projected);
    } catch (_err) {
      after = projected;
    }
    filesystemState.usageBytes = Math.max(
      0,
      filesystemState.usageBytes - before + after
    );
    return result;
  };

  filesystemGuardsInstalled = true;
}

function setFilesystemPolicy(module, policy) {
  ensureFilesystemGuardsInstalled(module);
  filesystemState.sessionRoot = resolveSessionRoot(module);
  const mode =
    policy && policy.mode === "readWrite" ? "readWrite" : "read";
  let quota = null;
  if (
    policy &&
    Object.prototype.hasOwnProperty.call(policy, "quotaBytes") &&
    policy.quotaBytes != null
  ) {
    const numeric = Number(policy.quotaBytes);
    if (!Number.isNaN(numeric) && Number.isFinite(numeric) && numeric >= 0) {
      quota = Math.floor(numeric);
    }
  }
  filesystemState.policy = {
    mode,
    quotaBytes: quota,
  };
  filesystemState.usageBytes = computeSessionUsage(module);
  return filesystemState.usageBytes;
}

function resetFilesystem(module) {
  ensureFilesystemGuardsInstalled(module);
  filesystemState.resetting = true;
  try {
    removeTree(module, "/session", true);
  } finally {
    filesystemState.resetting = false;
    filesystemState.usageBytes = computeSessionUsage(module);
  }
  return filesystemState.usageBytes;
}

function createReadonlyFS(FSOps, Module) {
  const FS = Module.FS;
  const ReadOnlyFS = {
    mount(mount) {
      return ReadOnlyFS.createNode(null, "/", mount.opts.info);
    },
    createNode(parent, name, info) {
      let { permissions: mode, isDir } = FSOps.getNodeMode(parent, name, info);
      if (isDir) {
        mode |= 1 << 14;
      } else {
        mode |= 1 << 15;
      }
      const node = FS.createNode(parent, name, mode);
      node.node_ops = ReadOnlyFS.node_ops;
      node.stream_ops = ReadOnlyFS.stream_ops;
      FSOps.setNodeAttributes(node, info, isDir);
      return node;
    },
    node_ops: {
      getattr(node) {
        const size = node.usedBytes ?? 0;
        const mode = node.mode ?? 0;
        const t = new Date((node.modtime ?? nowSeconds()) * 1000);
        const blksize = 4096;
        const blocks = ((size + blksize - 1) / blksize) | 0;
        return {
          dev: 1,
          ino: node.id,
          mode,
          nlink: 1,
          uid: 0,
          gid: 0,
          rdev: 0,
          size,
          atime: t,
          mtime: t,
          ctime: t,
          blksize,
          blocks,
        };
      },
      readdir(node) {
        return FSOps.readdir(node);
      },
      lookup(parent, name) {
        const child = FSOps.lookup(parent, name);
        if (child === undefined) {
          if (FS.genericErrors?.[44]) {
            throw FS.genericErrors[44];
          }
          throw new FS.ErrnoError(44);
        }
        return ReadOnlyFS.createNode(parent, name, child);
      },
    },
    stream_ops: {
      llseek(stream, offset, whence) {
        let position = offset;
        if (whence === 1) {
          position += stream.position;
        } else if (whence === 2 && FS.isFile(stream.node.mode)) {
          position += stream.node.usedBytes ?? 0;
        }
        return position;
      },
      read(stream, buffer, offset, length, position) {
        const size = FSOps.read(stream, position, buffer.subarray(offset, offset + length));
        return size;
      },
    },
  };
  return ReadOnlyFS;
}

function createTreeFs(Module) {
  const treeOps = {
    getNodeMode(_parent, _name, info) {
      return {
        permissions: info.mode ?? 0o644,
        isDir: info.type === "dir",
      };
    },
    setNodeAttributes(node, info, isDir) {
      node.info = info;
      node.modtime = (info && info.modtime) || nowSeconds();
      node.usedBytes = isDir ? 0 : info.data.length;
    },
    readdir(node) {
      const { info } = node;
      if (!info || !info.children) {
        return [];
      }
      return Array.from(info.children.keys());
    },
    lookup(parent, name) {
      return parent.info.children?.get(name);
    },
    read(stream, position, buffer) {
      const info = stream.node.info;
      if (!info || info.type !== "file") {
        return 0;
      }
      const source = info.data;
      if (position >= source.length) {
        return 0;
      }
      const size = Math.min(buffer.length, source.length - position);
      buffer.set(source.subarray(position, position + size));
      return size;
    },
  };
  return createReadonlyFS(treeOps, Module);
}

function createTarFs(Module) {
  const tarOps = {
    getNodeMode(_parent, _name, info) {
      const isDir = info && info.children instanceof Map;
      const permissions =
        typeof info?.mode === "number"
          ? info.mode
          : isDir
          ? 0o755
          : 0o644;
      return {
        permissions,
        isDir,
      };
    },
    setNodeAttributes(node, info, isDir) {
      node.info = info;
      node.modtime = info?.modtime ?? nowSeconds();
      node.usedBytes = isDir ? 0 : info?.size ?? 0;
      node.contentsOffset = info?.contentsOffset ?? 0;
    },
    readdir(node) {
      if (!node.info?.children) {
        return [];
      }
      return Array.from(node.info.children.keys());
    },
    lookup(parent, name) {
      return parent.info?.children?.get(name);
    },
    read(stream, position, buffer) {
      const info = stream.node.info;
      if (!info || info.type !== "0" || !info.reader) {
        return 0;
      }
      const fileSize = Number.isFinite(info.size) ? info.size : 0;
      if (position >= fileSize) {
        buffer.fill(0);
        return 0;
      }
      const chunkLength = Math.min(buffer.length, fileSize - position);
      const view = buffer.subarray(0, chunkLength);
      const bytesRead = info.reader.read((info.contentsOffset ?? 0) + position, view);
      if (bytesRead < buffer.length) {
        buffer.fill(0, bytesRead);
      }
      return bytesRead;
    },
  };
  return createReadonlyFS(tarOps, Module);
}

function overlayAddFile(root, relPath, data, mode = 0o644, modtime = nowSeconds()) {
  const parts = splitPath(relPath);
  if (parts.length === 0) {
    return;
  }
  const location = root === overlayState.usrRoot ? "usr" : root === overlayState.siteRoot ? "site" : "other";
  let cursor = root;
  for (let i = 0; i < parts.length - 1; i += 1) {
    const name = parts[i];
    let next = cursor.children.get(name);
    if (!next) {
      next = makeDirNode(0o755, modtime);
      cursor.children.set(name, next);
    }
    if (next.type !== "dir") {
      next = makeDirNode(0o755, modtime);
      cursor.children.set(name, next);
    }
    cursor = next;
  }
  const leaf = parts.at(-1);
  const fileNode = makeFileNode(data, mode || 0o644, modtime);
  cursor.children.set(leaf, fileNode);
  if (fileNode.data instanceof Uint8Array) {
    overlayRegisterBlob(fileNode.data, location, relPath);
  }
  overlayState.cachedTar = null;
}

function overlayClear(node) {
  node.children.clear();
  overlayInvalidateTarCache();
}

const overlayState = {
  siteRoot: makeDirNode(0o755),
  usrRoot: makeDirNode(0o755),
  dynlibs: new Map(),
  packages: new Set(),
  mounted: false,
  mountType: null,
  treeFs: null,
  tarFs: null,
  cachedTar: null,
  tarReader: null,
  tarSiteRoot: null,
  tarUsrRoot: null,
  tarDynlibFiles: [],
  tarFileMap: new Map(),
  sysPathEnsured: false,
  packageCatalog: new Map(),
  blobByKey: new Map(),
  tarReaders: [],
};

let overlayBufferRegistry = new WeakMap();
const overlayBlobRegistry = new WeakMap();

function overlayRegisterBlob(data, location, relPath) {
  if (!(data instanceof Uint8Array)) {
    return;
  }
  const meta = {
    location,
    relPath,
    byteOffset: Number.isFinite(data.byteOffset) ? data.byteOffset : 0,
    byteLength: Number.isFinite(data.byteLength) ? data.byteLength : data.length,
  };
  overlayBlobRegistry.set(data, meta);
  const buffer = data.buffer;
  if (buffer instanceof ArrayBuffer) {
    let entries = overlayBufferRegistry.get(buffer);
    if (!entries) {
      entries = [];
      overlayBufferRegistry.set(buffer, entries);
    }
    entries.push(meta);
  }
}

function overlayLookupMetaForView(view) {
  if (!view || typeof view !== "object") {
    return null;
  }
  const direct = overlayBlobRegistry.get(view);
  if (direct) {
    return {
      location: direct.location,
      relPath: direct.relPath,
      byteOffset: Number.isFinite(direct.byteOffset) ? direct.byteOffset : 0,
      byteLength: Number.isFinite(direct.byteLength) ? direct.byteLength : 0,
    };
  }
  const buffer = view.buffer;
  if (!(buffer instanceof ArrayBuffer)) {
    return null;
  }
  const entries = overlayBufferRegistry.get(buffer);
  if (!Array.isArray(entries) || entries.length === 0) {
    return null;
  }
  const byteOffset = Number.isFinite(view.byteOffset) ? view.byteOffset : 0;
  const byteLength = Number.isFinite(view.byteLength) ? view.byteLength : 0;
  for (const entry of entries) {
    if (!entry) {
      continue;
    }
    const entryOffset = Number.isFinite(entry.byteOffset) ? entry.byteOffset : 0;
    const entryLength = Number.isFinite(entry.byteLength) ? entry.byteLength : 0;
    if (
      byteOffset >= entryOffset &&
      byteLength >= 0 &&
      byteOffset + byteLength <= entryOffset + entryLength
    ) {
      return {
        location: entry.location,
        relPath: entry.relPath,
        byteOffset: entryOffset,
        byteLength: entryLength,
      };
    }
  }
  return null;
}

function overlayInvalidateTarCache() {
  overlayState.cachedTar = null;
  overlayState.tarReader = null;
  overlayState.tarSiteRoot = null;
  overlayState.tarUsrRoot = null;
  overlayState.tarDynlibFiles = [];
  overlayState.tarFileMap.clear();
  overlayState.tarFs = null;
  if (overlayState.mountType === "tar") {
    overlayState.mountType = null;
  }
  overlayState.sysPathEnsured = false;
  overlayBufferRegistry = new WeakMap();
}

function createTarBuilder() {
  const directories = new Set();
  const files = [];
  const mounts = new Set();

  function registerDirectories(fullPath) {
    const segments = splitPath(fullPath);
    if (segments.length === 0) {
      return;
    }
    let cursor = "";
    for (let i = 0; i < segments.length - 1; i += 1) {
      cursor = cursor ? `${cursor}/${segments[i]}` : segments[i];
      directories.add(cursor);
    }
  }

  return {
    addFile(location, relPath, data, mode = 0o644, mtime = nowSeconds()) {
      const normalizedLocation = location === "usr" || location === "dynlib" ? "usr" : "site";
      mounts.add(normalizedLocation);
      const sanitized =
        typeof relPath === "string" && relPath.length > 0
          ? relPath.replace(/^\/*/, "")
          : "";
      const base = normalizedLocation;
      const path = sanitized ? `${base}/${sanitized}` : base;
      registerDirectories(path);
      directories.add(base);
      files.push({
        path,
        mode: Number.isFinite(mode) ? mode : 0o644,
        mtime: Number.isFinite(mtime) ? mtime : nowSeconds(),
        data: data instanceof Uint8Array ? data : new Uint8Array(),
      });
    },
    finalize() {
      const sortedDirs = Array.from(directories)
        .filter((entry) => entry && entry.length > 0)
        .sort();
      const sortedFiles = files.sort((a, b) =>
        a.path < b.path ? -1 : a.path > b.path ? 1 : 0
      );
      const chunks = [];
      let total = 0;
      for (const dir of sortedDirs) {
        const header = tarBuildHeader(
          dir.endsWith("/") ? dir : `${dir}/`,
          0o755,
          0,
          nowSeconds(),
          "5"
        );
        chunks.push(header);
        total += header.length;
      }
      for (const file of sortedFiles) {
        const data = file.data instanceof Uint8Array ? file.data : new Uint8Array();
        const header = tarBuildHeader(
          file.path,
          file.mode ?? 0o644,
          data.length,
          file.mtime ?? nowSeconds(),
          "0"
        );
        chunks.push(header);
        total += header.length;
        chunks.push(data);
        total += data.length;
        const remainder = data.length % 512;
        if (remainder !== 0) {
          const pad = 512 - remainder;
          const padding = new Uint8Array(pad);
          chunks.push(padding);
          total += pad;
        }
      }
      // End-of-archive marker
      const trailer = new Uint8Array(1024);
      chunks.push(trailer.subarray(0, 1024));
      total += 1024;
      const tar = new Uint8Array(total);
      let offset = 0;
      for (const chunk of chunks) {
        tar.set(chunk, offset);
        offset += chunk.length;
      }
      return { tar, mounts: Array.from(mounts) };
    },
  };
}

function stripTarPadding(bytes) {
  if (!(bytes instanceof Uint8Array)) {
    return new Uint8Array();
  }
  let end = bytes.length;
  while (end >= 512) {
    let zero = true;
    for (let i = end - 512; i < end; i += 1) {
      if (bytes[i] !== 0) {
        zero = false;
        break;
      }
    }
    if (!zero) {
      break;
    }
    end -= 512;
  }
  return bytes.subarray(0, end);
}

function concatTarArchives(blobs) {
  if (!Array.isArray(blobs) || blobs.length === 0) {
    return new Uint8Array();
  }
  const chunks = [];
  let total = 0;
  for (let i = 0; i < blobs.length; i += 1) {
    const chunk =
      blobs[i] instanceof Uint8Array ? stripTarPadding(blobs[i]) : new Uint8Array();
    if (chunk.length === 0) {
      continue;
    }
    chunks.push(chunk);
    total += chunk.length;
  }
  const trailer = new Uint8Array(1024);
  total += trailer.length;
  const combined = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    combined.set(chunk, offset);
    offset += chunk.length;
  }
  combined.set(trailer, offset);
  return combined;
}

function defaultSnapshotDeserializer(value) {
  if (value === null || value === undefined) {
    return value;
  }
  if (typeof value !== "object") {
    return value;
  }
  const tag = value.__type;
  if (!tag) {
    return value;
  }
  switch (tag) {
    case "uint8array": {
      const length = Number.isFinite(value.length) ? value.length : 0;
      return new Uint8Array(length);
    }
    case "overlay-typedarray": {
      const location =
        typeof value.location === "string" ? value.location : "other";
      const relPath = typeof value.path === "string" ? value.path : "";
      const ctorName =
        typeof value.ctor === "string" ? value.ctor : "Uint8Array";
      const ctor = globalThis[ctorName];
      const base = overlayReadBlob(location, relPath);
      if (typeof ctor !== "function") {
        return base;
      }
      const offset = Number.isFinite(value.offset) ? value.offset : 0;
      const requestedByteLength = Number.isFinite(value.byteLength)
        ? value.byteLength
        : 0;
      const bytesPerElement =
        Number.isFinite(value.bytesPerElement) && value.bytesPerElement > 0
          ? value.bytesPerElement
          : Number.isFinite(ctor.BYTES_PER_ELEMENT) && ctor.BYTES_PER_ELEMENT > 0
          ? ctor.BYTES_PER_ELEMENT
          : 1;
      const baseByteLength =
        Number.isFinite(base.byteLength) && base.byteLength >= 0
          ? base.byteLength
          : base.length ?? 0;
      const baseByteOffset =
        Number.isFinite(base.byteOffset) && base.byteOffset >= 0
          ? base.byteOffset
          : 0;
      const maxSpan = Math.max(
        0,
        Math.min(baseByteLength - offset, requestedByteLength || baseByteLength)
      );
      const requestedLength = Number.isFinite(value.length)
        ? value.length
        : Math.floor(maxSpan / bytesPerElement);
      const length = Math.max(
        0,
        Math.min(
          requestedLength,
          Math.floor(maxSpan / (bytesPerElement || 1))
        )
      );
      const buffer = base?.buffer;
      if (!(buffer instanceof ArrayBuffer) || length === 0) {
        return new ctor(length);
      }
      const start = baseByteOffset + Math.max(0, offset);
      try {
        return new ctor(buffer, start, length);
      } catch (_err) {
        try {
          const slice = buffer.slice(
            start,
            start + length * (bytesPerElement || 1)
          );
          return new ctor(slice);
        } catch (_err2) {
          return new ctor(length);
        }
      }
    }
    case "typedarray": {
      const ctorName = typeof value.ctor === "string" ? value.ctor : "Uint8Array";
      const length = Number.isFinite(value.length) ? value.length : 0;
      const ctor = globalThis[ctorName];
      try {
        if (typeof ctor === "function") {
          return new ctor(length);
        }
      } catch (_err) {
        // fall through
      }
      return new Uint8Array(length);
    }
    case "overlay-blob": {
      const location = typeof value.location === "string" ? value.location : "other";
      const relPath = typeof value.path === "string" ? value.path : "";
      return overlayReadBlob(location, relPath);
    }
    case "function": {
      const name = typeof value.name === "string" ? value.name : "pyRunnerSnapshotFn";
      const stub = function () {
        throw new Error(`snapshot placeholder function '${name}' invoked`);
      };
      Object.defineProperty(stub, "name", { value: name, configurable: true });
      return stub;
    }
    case "object": {
      if (value.ctor === "Map") {
        const map = new Map();
        const keys = Array.isArray(value.keys) ? value.keys : [];
        for (const key of keys) {
          map.set(key, undefined);
        }
        return map;
      }
      if (value.ctor === "Set") {
        return new Set(Array.isArray(value.keys) ? value.keys : []);
      }
      return {};
    }
    default:
      return value;
  }
}

function overlayFindNode(root, relPath) {
  const parts = splitPath(relPath);
  let cursor = root;
  for (const part of parts) {
    if (!cursor || cursor.type !== "dir") {
      return null;
    }
    cursor = cursor.children.get(part);
    if (!cursor) {
      return null;
    }
  }
  return cursor;
}

function overlayReadBlob(location, relPath) {
  const root =
    location === "usr"
      ? overlayState.usrRoot
      : location === "site"
      ? overlayState.siteRoot
      : null;
  if (!root) {
    return new Uint8Array();
  }
  const node = overlayFindNode(root, relPath);
  const data = node?.data;
  if (data instanceof Uint8Array) {
    overlayRegisterBlob(data, location, relPath);
    return data;
  }
  const normalized =
    typeof relPath === "string"
      ? relPath.replace(/^\/*/, "")
      : "";
  const key =
    location === "usr"
      ? `usr:${normalized}`
      : location === "site"
      ? `site:${normalized}`
      : null;
  if (key && overlayState.tarFileMap.has(key)) {
    const info = overlayState.tarFileMap.get(key);
    const tar = overlayState.cachedTar;
    if (info && info.type === "0" && tar instanceof Uint8Array) {
      const start = Math.max(0, info.contentsOffset || 0);
      const end = Math.min(tar.length, start + (info.size || 0));
      if (end > start) {
        const slice = tar.subarray(start, end);
        overlayRegisterBlob(slice, location, relPath);
        return slice;
      }
    }
    if (info && typeof info.reader?.read === "function") {
      const size = Number.isFinite(info.size) ? info.size : 0;
      if (size > 0) {
        const buffer = new Uint8Array(size);
        info.reader.read(info.contentsOffset || 0, buffer);
        overlayRegisterBlob(buffer, location, relPath);
        return buffer;
      }
    }
  }
  return new Uint8Array();
}

function overlayGatherEntries(root, basePrefix, directories, files) {
  directories.add(basePrefix);
  const stack = [[root, ""]];
  while (stack.length > 0) {
    const [node, prefix] = stack.pop();
    for (const [name, child] of node.children.entries()) {
      const rel = prefix ? `${prefix}/${name}` : name;
      const path = basePrefix ? `${basePrefix}/${rel}` : rel;
      if (child.type === "dir") {
        directories.add(path);
        stack.push([child, rel]);
      } else if (child.type === "file") {
        files.push({
          path,
          mode: child.mode ?? 0o644,
          mtime: child.modtime ?? nowSeconds(),
          data: child.data ?? new Uint8Array(),
        });
      }
    }
  }
}

function tarWriteString(buffer, offset, length, value) {
  const bytes = textEncoder.encode(value ?? "");
  const limit = Math.min(bytes.length, length);
  buffer.set(bytes.subarray(0, limit), offset);
}

function tarWriteOctal(buffer, offset, length, value) {
  const text =
    Number.isFinite(value) && value >= 0
      ? Math.trunc(value).toString(8)
      : "0";
  const encoded = textEncoder.encode(text);
  const start = offset + Math.max(0, length - encoded.length - 1);
  for (let i = 0; i < length; i += 1) {
    buffer[offset + i] = 0;
  }
  buffer.set(encoded.subarray(0, length - 1), start);
  buffer[offset + length - 1] = 0;
}

function tarSplitPath(path) {
  if (!path || path.length <= 100) {
    return { name: path, prefix: "" };
  }
  const parts = path.split("/");
  let name = parts.pop() || "";
  let prefix = parts.join("/");
  if (name.length > 100) {
    throw new Error(`Tar entry name too long: ${name}`);
  }
  if (prefix.length > 155) {
    throw new Error(`Tar entry prefix too long: ${prefix}`);
  }
  return { name, prefix };
}

function tarBuildHeader(path, mode, size, mtime, type) {
  const header = new Uint8Array(512);
  const view = header;
  const { name, prefix } = tarSplitPath(path);
  tarWriteString(view, 0, 100, name);
  tarWriteOctal(view, 100, 8, mode);
  tarWriteOctal(view, 108, 8, 0); // uid
  tarWriteOctal(view, 116, 8, 0); // gid
  tarWriteOctal(view, 124, 12, size);
  tarWriteOctal(view, 136, 12, mtime);
  view[156] = type === "5" ? 0x35 : 0x30;
  tarWriteString(view, 157, 100, "");
  tarWriteString(view, 257, 6, "ustar\0");
  tarWriteString(view, 263, 2, "00");
  tarWriteString(view, 265, 32, "aardvark");
  tarWriteString(view, 297, 32, "aardvark");
  tarWriteOctal(view, 329, 8, 0); // devmajor
  tarWriteOctal(view, 337, 8, 0); // devminor
  tarWriteString(view, 345, 155, prefix);
  for (let i = 148; i < 156; i += 1) {
    view[i] = 0x20;
  }
  let sum = 0;
  for (let i = 0; i < 512; i += 1) {
    sum += view[i];
  }
  tarWriteOctal(view, 148, 8, sum);
  return header;
}

function overlayBuildTar() {
  const directories = new Set();
  const files = [];
  overlayGatherEntries(overlayState.siteRoot, "site", directories, files);
  overlayGatherEntries(overlayState.usrRoot, "usr", directories, files);
  if (files.length === 0 && directories.size <= 2) {
    // nothing to serialize
    return new Uint8Array();
  }
  const sortedDirs = Array.from(directories)
    .filter((entry) => entry && entry.length > 0)
    .sort();
  const sortedFiles = files.sort((a, b) =>
    a.path < b.path ? -1 : a.path > b.path ? 1 : 0
  );
  const chunks = [];
  let total = 0;
  for (const dir of sortedDirs) {
    const header = tarBuildHeader(dir.endsWith("/") ? dir : `${dir}/`, 0o755, 0, nowSeconds(), "5");
    chunks.push(header);
    total += header.length;
  }
  for (const file of sortedFiles) {
    const data = file.data instanceof Uint8Array ? file.data : new Uint8Array();
    const header = tarBuildHeader(
      file.path,
      file.mode ?? 0o644,
      data.length,
      file.mtime ?? nowSeconds(),
      "0"
    );
    chunks.push(header);
    total += header.length;
    chunks.push(data);
    total += data.length;
    const remainder = data.length % 512;
    if (remainder !== 0) {
      const pad = 512 - remainder;
      chunks.push(new Uint8Array(pad));
      total += pad;
    }
  }
  // End of archive markers.
  chunks.push(new Uint8Array(512));
  chunks.push(new Uint8Array(512));
  total += 1024;
  const tar = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    tar.set(chunk, offset);
    offset += chunk.length;
  }
  return tar;
}

function overlayEnsureTar() {
  if (overlayState.cachedTar instanceof Uint8Array) {
    return overlayState.cachedTar;
  }
  const tar = overlayBuildTar();
  overlayState.cachedTar = tar;
  return tar;
}

function tarReadString(buffer, offset, length) {
  let end = offset + length;
  for (let i = offset; i < offset + length; i += 1) {
    if (buffer[i] === 0) {
      end = i;
      break;
    }
  }
  return textDecoder.decode(buffer.subarray(offset, end));
}

function tarReadOctal(buffer, offset, length) {
  const slice = buffer.subarray(offset, offset + length);
  let text = "";
  for (let i = 0; i < slice.length; i += 1) {
    const code = slice[i];
    if (code === 0 || code === 0x20) {
      if (text.length === 0) {
        continue;
      }
      break;
    }
    text += String.fromCharCode(code);
  }
  const parsed = parseInt(text.trim() || "0", 8);
  return Number.isFinite(parsed) ? parsed : 0;
}

function createTarBufferReader(buffer) {
  return {
    read(offset, dest) {
      if (!dest || typeof dest.length !== "number") {
        throw new TypeError("tar reader destination must be an array-like view");
      }
      let target = dest;
      let needsCopyBack = false;
      if (!(dest instanceof Uint8Array)) {
        if (
          dest.buffer instanceof ArrayBuffer &&
          typeof dest.byteOffset === "number"
        ) {
          target = new Uint8Array(
            dest.buffer,
            dest.byteOffset,
            dest.byteLength ?? dest.length
          );
          needsCopyBack = true;
        } else {
          target = new Uint8Array(dest.length);
          needsCopyBack = true;
        }
      }
      if (offset >= buffer.length) {
        target.fill(0);
        if (needsCopyBack) {
          for (let i = 0; i < target.length && i < dest.length; i += 1) {
            dest[i] = target[i];
          }
        }
        return 0;
      }
      const end = Math.min(buffer.length, offset + target.length);
      const view = buffer.subarray(offset, end);
      target.set(view);
      if (end - offset < target.length) {
        target.fill(0, end - offset);
      }
      if (needsCopyBack) {
        if (typeof dest.set === "function") {
          dest.set(target.subarray(0, Math.min(target.length, dest.length)));
        } else {
          const limit = Math.min(target.length, dest.length);
          for (let i = 0; i < limit; i += 1) {
            dest[i] = target[i];
          }
        }
      }
      return view.length;
    },
  };
}

function overlayLoadTar(tarBytes) {
  overlayClear(overlayState.siteRoot);
  overlayClear(overlayState.usrRoot);
  overlayState.tarReader = null;
  overlayState.tarSiteRoot = null;
  overlayState.tarUsrRoot = null;
  overlayState.tarDynlibFiles = [];
  if (!(tarBytes instanceof Uint8Array) || tarBytes.length === 0) {
    overlayState.cachedTar = new Uint8Array();
    return;
  }
  const tarReader = createTarBufferReader(tarBytes);
  const tarDirectories = new Map();
  const tarRoot = {
    path: "",
    name: "",
    mode: 0o755,
    size: 0,
    modtime: nowSeconds(),
    type: "5",
    parts: [],
    children: new Map(),
    reader: tarReader,
  };
  tarDirectories.set("", tarRoot);

  function ensureTarDir(path, mode = 0o755, modtime = nowSeconds()) {
    const normalized = path
      .replace(/\/+/, "/")
      .replace(/^\.\//, "")
      .replace(/\/$/, "");
    if (normalized === "") {
      return tarRoot;
    }
    if (tarDirectories.has(normalized)) {
      return tarDirectories.get(normalized);
    }
    const parentPath = normalized.includes("/")
      ? normalized.slice(0, normalized.lastIndexOf("/"))
      : "";
    const parent = ensureTarDir(parentPath, mode, modtime);
    const name = normalized.split("/").at(-1) ?? normalized;
    const info = {
      path: normalized,
      name,
      mode,
      size: 0,
      modtime,
      type: "5",
      parts: normalized.split("/"),
      children: new Map(),
      reader: tarReader,
    };
    parent.children.set(name, info);
    tarDirectories.set(normalized, info);
    return info;
  }

  let offset = 0;
  let fileCount = 0;
  const length = tarBytes.length;
  const headerBuffer = new Uint8Array(512);
  while (offset + 512 <= length) {
    tarReader.read(offset, headerBuffer);
    const header = headerBuffer;
    offset += 512;
    let empty = true;
    for (let i = 0; i < 512; i += 1) {
      if (header[i] !== 0) {
        empty = false;
        break;
      }
    }
    if (empty) {
      // reached end-of-archive marker
      break;
    }
    const name = tarReadString(header, 0, 100);
    const prefix = tarReadString(header, 345, 155);
    const path = (prefix ? `${prefix}/${name}` : name).replace(/\/+/, "/").replace(/^\//, "");
    const type = header[156];
    const size = tarReadOctal(header, 124, 12);
    const mode = tarReadOctal(header, 100, 8) || 0o644;
    const mtime = tarReadOctal(header, 136, 12) || nowSeconds();
    const dataOffset = offset;
    if (!path) {
      offset += Math.ceil(size / 512) * 512;
      continue;
    }
    const data = tarBytes.subarray(offset, offset + size);
    offset += Math.ceil(size / 512) * 512;
    if (type === 0x35) {
      // directory entry ensures parent nodes exist
      const target =
        path.startsWith("usr/")
          ? overlayState.usrRoot
          : path.startsWith("site/")
          ? overlayState.siteRoot
          : null;
      if (target) {
        const rel = path.replace(/^usr\//, "").replace(/^site\//, "");
        const parts = splitPath(rel);
        let cursor = target;
        for (const part of parts) {
          let next = cursor.children.get(part);
          if (!next || next.type !== "dir") {
            next = makeDirNode(0o755, mtime);
            cursor.children.set(part, next);
          }
          cursor = next;
        }
      }
      ensureTarDir(path, mode || 0o755, mtime);
      continue;
    }
    const locationKey = path.startsWith("usr/")
      ? "usr"
      : path.startsWith("site/")
      ? "site"
      : null;
    if (!locationKey) {
      continue;
    }
    const target = locationKey === "usr" ? overlayState.usrRoot : overlayState.siteRoot;
    const rel = path.replace(/^usr\//, "").replace(/^site\//, "");
    overlayAddFile(target, rel, data, mode, mtime);
    fileCount += 1;
    const parentPath = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    const parentInfo = ensureTarDir(parentPath, 0o755, mtime);
    const fileName = path.slice(parentPath ? parentPath.length + 1 : 0);
    const fileInfo = {
      path,
      name: fileName,
      mode,
      size,
      modtime: mtime,
      type: "0",
      parts: parentInfo.parts.concat(fileName),
      children: undefined,
      contentsOffset: dataOffset,
      reader: tarReader,
    };
    parentInfo.children.set(fileName, fileInfo);
    overlayState.tarFileMap.set(`${locationKey}:${rel}`, fileInfo);
    if (path.endsWith(".so")) {
      overlayState.tarDynlibFiles.push(path);
    }
  }
  overlayState.cachedTar = tarBytes;
  overlayState.tarReader = tarReader;
  overlayState.tarSiteRoot = tarDirectories.get("site") ?? ensureTarDir("site", 0o755, nowSeconds());
  overlayState.tarUsrRoot = tarDirectories.get("usr") ?? ensureTarDir("usr", 0o755, nowSeconds());
  try {
    const nativeLog =
      typeof globalThis.__pyRunnerNativeLog === "function"
        ? globalThis.__pyRunnerNativeLog
        : null;
    if (nativeLog) {
      nativeLog(`[overlay] restored tar entries: ${fileCount}`);
    } else if (globalThis.console?.info) {
      globalThis.console.info("[overlay] restored tar entries", fileCount);
    }
  } catch (_err) {
    // ignore logging failures
  }
}

function overlayEnsureMounted(Module) {
  const hasTarRoots =
    overlayState.tarReader &&
    overlayState.tarSiteRoot &&
    overlayState.tarUsrRoot;
  const desiredMount = hasTarRoots ? "tar" : "tree";
  if (overlayState.mounted && overlayState.mountType === desiredMount) {
    return;
  }
  if (overlayState.mounted) {
    try {
      Module.FS.unmount(Module.FS.sessionSitePackages);
    } catch (_err) {
      // ignore unmount failures
    }
    try {
      Module.FS.unmount("/usr/lib");
    } catch (_err) {
      // ignore unmount failures
    }
    overlayState.mounted = false;
    overlayState.sysPathEnsured = false;
  }
  let fsImpl;
  let siteInfo;
  let usrInfo;
  if (desiredMount === "tar" && hasTarRoots) {
    if (!overlayState.tarFs) {
      overlayState.tarFs = createTarFs(Module);
    }
    fsImpl = overlayState.tarFs;
    siteInfo = overlayState.tarSiteRoot;
    usrInfo = overlayState.tarUsrRoot;
  } else {
    if (!overlayState.treeFs) {
      overlayState.treeFs = createTreeFs(Module);
    }
    fsImpl = overlayState.treeFs;
    siteInfo = overlayState.siteRoot;
    usrInfo = overlayState.usrRoot;
  }
  Module.FS.mkdirTree(Module.FS.sessionSitePackages);
  Module.FS.mount(fsImpl, { info: siteInfo }, Module.FS.sessionSitePackages);
  Module.FS.mkdirTree("/usr/lib");
  Module.FS.mount(fsImpl, { info: usrInfo }, "/usr/lib");
  overlayState.mounted = true;
  overlayState.mountType = desiredMount;
}

function ensureSessionSitePackagesOnSysPath(Module, publicApi = null) {
  if (overlayState.sysPathEnsured) {
    return;
  }
  const sessionPath = Module?.FS?.sessionSitePackages;
  if (!sessionPath) {
    return;
  }
  const script = `import os\nimport sys\nimport importlib\n_path = os.path.normpath("${sessionPath}")\nif _path not in sys.path:\n    sys.path.insert(0, _path)\nsys.path_importer_cache.pop(_path, None)\nimportlib.invalidate_caches()\ndel os, sys, importlib, _path`;
  if (publicApi && typeof publicApi.runPython === "function") {
    try {
      publicApi.runPython(script);
      overlayState.sysPathEnsured = true;
      if (globalThis.console?.info) {
        globalThis.console.info(
          "[overlay] ensured session site-packages via publicApi",
          sessionPath
        );
      }
      return;
    } catch (err) {
      if (globalThis.console?.warn) {
        globalThis.console.warn(
          "[overlay] publicApi.runPython failed to ensure session site-packages",
          err
        );
      }
    }
  }
  if (typeof Module?.API?.rawRun === "function") {
    try {
      simpleRunPython(Module, script);
      overlayState.sysPathEnsured = true;
      if (globalThis.console?.info) {
        globalThis.console.info(
          "[overlay] ensured session site-packages via rawRun",
          sessionPath
        );
      }
      return;
    } catch (err) {
      if (globalThis.console?.warn) {
        globalThis.console.warn(
          "[overlay] failed to ensure session site-packages on sys.path",
          err
        );
      }
    }
  }
}

function overlayRecordDynlib(location, relPath) {
  if (!relPath) {
    return;
  }
  const normalizedLocation =
    location === "usr" || location === "dynlib" ? "usr" : location === "site" ? "site" : "other";
  if (normalizedLocation === "other") {
    return;
  }
  const cleanedRel = String(relPath)
    .replace(/^\.\//, "")
    .replace(/^\/+/, "")
    .replace(/\/+/, "/");
  if (!cleanedRel) {
    return;
  }
  const key = `${normalizedLocation}:${cleanedRel}`;
  if (!overlayState.dynlibs.has(key)) {
    overlayState.dynlibs.set(key, {
      location: normalizedLocation,
      relPath: cleanedRel,
    });
  }
}

function simpleRunPython(Module, code) {
  try {
    const [status, stderr] = Module.API.rawRun(code);
    if (status === -1) {
      throw new Error(`Python execution failed:\n${stderr}`);
    }
    return typeof stderr === "string" ? stderr.trim() : String(stderr ?? "");
  } catch (error) {
    console.error("[bootstrap] simpleRunPython failed for code:", code);
    throw error;
  }
}

async function fetchTextAsset(specifier) {
  const response = await fetch(specifier);
  if (!response.ok) {
    throw new Error(`Failed to load asset '${specifier}': ${response.status}`);
  }
  return response.text();
}

const entropyState = {
  allowedEntropyAddr: 0,
  inRequestContext: false,
  ready: false,
  realGetRandomValues: null,
};

async function entropyMountFiles(Module) {
  const cloudflareDir = `${Module.FS.sitePackages}/_cloudflare`;
  Module.FS.mkdirTree(cloudflareDir);
  const files = [
    { name: "__init__.py", contents: "" },
    {
      name: "allow_entropy.py",
      asset: "entropy/allow_entropy.py",
    },
    {
      name: "entropy_import_context.py",
      asset: "entropy/entropy_import_context.py",
    },
    {
      name: "entropy_patches.py",
      asset: "entropy/entropy_patches.py",
    },
    {
      name: "import_patch_manager.py",
      asset: "entropy/import_patch_manager.py",
    },
  ];

  for (const entry of files) {
    let data = entry.contents;
    if (entry.asset) {
      data = await fetchTextAsset(entry.asset);
    }
    const encoded = textEncoder.encode(data);
    Module.FS.writeFile(`${cloudflareDir}/${entry.name}`, encoded, {
      canOwn: true,
    });
  }
}

function setupShouldAllowBadEntropy(Module) {
  const result = simpleRunPython(
    Module,
    `
from _cloudflare.entropy_import_context import get_bad_entropy_flag
get_bad_entropy_flag()
del get_bad_entropy_flag
`
  );
  const value = Number.parseInt(result, 10);
  if (!Number.isFinite(value)) {
    throw new Error(
      `Failed to parse entropy flag address from '${result ?? "unknown"}'`
    );
  }
  console.log("[bootstrap] entropy flag address:", value);
  entropyState.allowedEntropyAddr = value;
}

function shouldAllowBadEntropy(Module) {
  const addr = entropyState.allowedEntropyAddr;
  if (!addr) {
    return false;
  }
  const view = Module.HEAP8;
  const current = view[addr];
  if (current > 0) {
    view[addr] = current - 1;
    return true;
  }
  return false;
}

function entropyGetRandomValues(Module, array) {
  const addr = entropyState.allowedEntropyAddr;
  const real = entropyState.realGetRandomValues;
  if (!addr) {
    console.warn("[bootstrap] entropyGetRandomValues without sentinel pointer");
    return real ? real(array) : array;
  }
  if (entropyState.inRequestContext) {
    return real ? real(array) : array;
  }
  if (!shouldAllowBadEntropy(Module)) {
    console.error("Entropy call denied outside request context");
    if (typeof Module._dump_traceback === "function") {
      Module._dump_traceback();
    }
    throw new Error(
      "Disallowed entropy usage before request context initialization"
    );
  }
  const remaining = Module.HEAP8[addr];
  if (globalThis.console?.debug) {
    globalThis.console.debug("[bootstrap] entropyGetRandomValues allowed", {
      addr,
      remaining,
    });
  }
  array.fill(43);
  return array;
}

function entropyAfterRuntimeInit(Module) {
  try {
    setupShouldAllowBadEntropy(Module);
  } catch (error) {
    console.error("[bootstrap] entropyAfterRuntimeInit failed", error);
    throw error;
  }
}

function entropyBeforeTopLevel(Module) {
  try {
    simpleRunPython(
      Module,
      `
from _cloudflare.entropy_patches import before_top_level
before_top_level()
del before_top_level
`
    );
  } catch (error) {
    console.error("[bootstrap] entropyBeforeTopLevel failed", error);
    throw error;
  }
}

function entropyBeforeRequest(Module) {
  if (entropyState.ready) {
    return;
  }
  entropyState.ready = true;
  entropyState.inRequestContext = true;
  try {
    simpleRunPython(
      Module,
      `
from _cloudflare.entropy_patches import before_first_request
before_first_request()
del before_first_request
`
    );
  } catch (error) {
    console.error("[bootstrap] entropyBeforeRequest failed", error);
    throw error;
  }
}

function installCryptoInterceptor(Module) {
  const cryptoObj = globalThis.crypto;
  if (!cryptoObj || typeof cryptoObj.getRandomValues !== "function") {
    return;
  }
  if (!entropyState.realGetRandomValues) {
    entropyState.realGetRandomValues = cryptoObj.getRandomValues.bind(cryptoObj);
  }
  if (cryptoObj.getRandomValues.__pyRunnerWrapped) {
    return;
  }
  const wrapped = function (array) {
    return entropyGetRandomValues(Module, array);
  };
  wrapped.__pyRunnerWrapped = true;
  cryptoObj.getRandomValues = wrapped;
}

const origSetTimeout =
  (typeof globalThis.setTimeout === "function" &&
    globalThis.setTimeout.bind(globalThis)) ||
  (() => 0);

function setTimeoutTopLevelPatch(handler, timeout) {
  if (typeof handler === "string") {
    return origSetTimeout(handler, timeout);
  }
  if (timeout && timeout > 0) {
    return origSetTimeout(handler, timeout);
  }
  queueMicrotask(handler);
  return 0;
}

export async function loadPyRunnerPyodide(options = {}) {
  const {
    indexURL = ".",
    env = {},
    stdout = globalThis.console?.log?.bind(globalThis.console) ?? noop,
    stderr = globalThis.console?.error?.bind(globalThis.console) ?? noop,
    stdin = () => "",
    snapshot,
    makeSnapshot = false,
    snapshotDeserializer,
  } = options;

  const snapshotBytes = snapshot
    ? snapshot instanceof Uint8Array
      ? snapshot
      : new Uint8Array(snapshot)
    : undefined;

  let module;
  try {
    module = await instantiatePyodideModule({
      indexURL,
      env,
      stdout,
      stderr,
      args: [],
      snapshot: snapshotBytes,
      makeSnapshot,
    });
  if (globalThis.console?.log) {
    const preview = Object.keys(module || {}).slice(0, 20);
    globalThis.console.log("[bootstrap] module keys:", preview);
    globalThis.console.log(
      "[bootstrap] module.ready typeof:",
      typeof module.ready
    );
  }
  if (module?.ready && typeof module.ready.then === "function") {
    console.log("[bootstrap] awaiting module.ready");
    await module.ready;
    console.log("[bootstrap] module.ready resolved");
  }
  if (typeof module._Py_IsInitialized === "function") {
    console.log("[bootstrap] Py_IsInitialized:", module._Py_IsInitialized());
  } else {
    console.log("[bootstrap] module._Py_IsInitialized unavailable");
  }
  } catch (error) {
    stderr("pyodide bootstrap failed:", error);
    if (error && error.stack) {
      stderr(error.stack);
    }
    throw error;
  }

  const Module = module;
  const api = module.API;
  if (!api) {
    throw new Error("Pyodide module did not expose API");
  }

  globalThis.__aardvarkFilesystemSetPolicy = (policy) =>
    setFilesystemPolicy(Module, policy || {});
  globalThis.__aardvarkFilesystemGetUsage = () => filesystemState.usageBytes;
  globalThis.__aardvarkFilesystemReset = () => resetFilesystem(Module);
  globalThis.__aardvarkSetHostCapabilities = (caps) => setHostCapabilities(caps);
  setFilesystemPolicy(Module, filesystemState.policy);

  api.config = api.config || {};
  if (globalThis.console?.log) {
    try {
      const apiKeys = Object.keys(api).slice(0, 20);
      globalThis.console.log("[snapshot] api keys:", apiKeys);
      const configKeys = Object.keys(api.config || {});
      globalThis.console.log("[snapshot] api.config keys:", configKeys);
    } catch (_err) {
      // ignore
    }
  }
  api.config.jsglobals = globalThis;
  if (snapshotBytes) {
    api.config._loadSnapshot = snapshotBytes;
  }
  if (makeSnapshot) {
    api.config._makeSnapshot = true;
  }

  installCryptoInterceptor(module);
  ensureSessionMetadata(module);
  overlayEnsureMounted(module);
  try {
    module.FS.mkdirTree("/usr/lib");
  } catch (_err) {
    // ignore if already present
  }
  globalThis.__pyRunnerEnterRequestContext = () => entropyBeforeRequest(module);

  let snapshotConfig;
  if (snapshotBytes && snapshotBytes.length > 0 && typeof api.restoreSnapshot === "function") {
    if (!api.config) {
      api.config = {};
    }
    if (!api.config.buildId) {
      api.config.buildId = api.config.BUILD_ID || "dev";
    }
    if (!api.config.BUILD_ID) {
      api.config.BUILD_ID = api.config.buildId;
    }
    const attemptRestore = () => api.restoreSnapshot(snapshotBytes);
    try {
      snapshotConfig = attemptRestore();
    } catch (error) {
      const message = String(error ?? "");
      const mismatchMatch = /Snapshot build id mismatch[^]*expected:\s*(\S+)[^]*got\s*:\s*(\S+)/m.exec(message);
      if (mismatchMatch) {
        const expected = mismatchMatch[1];
        const got = mismatchMatch[2];
        if (!api.config) {
          api.config = {};
        }
        if (globalThis.console?.warn) {
          globalThis.console.warn(
            `[snapshot] build id mismatch: expected '${expected}' but found '${got}', retrying with snapshot id`
          );
        }
        api.config.buildId = got;
        api.config.snapshotBuildId = got;
        api.config.BUILD_ID = got;
        snapshotConfig = attemptRestore();
      } else {
        throw error;
      }
    }
  }

  if ((!snapshotBytes || snapshotBytes.length === 0) && typeof module.callMain === "function") {
    console.log("[bootstrap] invoking module.callMain([])");
    module.callMain([]);
  }

  if (typeof module.setSetTimeout === "function") {
    module.setSetTimeout(
      setTimeoutTopLevelPatch,
      globalThis.clearTimeout?.bind(globalThis) ?? (() => {}),
      globalThis.setInterval?.bind(globalThis) ?? (() => 0),
      globalThis.clearInterval?.bind(globalThis) ?? (() => {})
    );
  }

  if (typeof module.setGetRandomValues === "function" && globalThis.crypto) {
    module.setGetRandomValues((ModuleRef, array) =>
      entropyGetRandomValues(ModuleRef, array)
    );
  }

  await entropyMountFiles(module);
  await loadTransitivePackages(module);
  entropyAfterRuntimeInit(module);
  entropyBeforeTopLevel(module);

  if (typeof module.removeRunDependency === "function") {
    try {
      module.removeRunDependency("dynlibs");
    } catch (_err) {
      // ignore; dependency may not have been registered.
    }
  }

  let publicApi = api.public_api;
  if (typeof api.finalizeBootstrap === "function") {
    const effectiveDeserializer =
      typeof snapshotDeserializer === "function"
        ? snapshotDeserializer
        : (value) => defaultSnapshotDeserializer(value);
    if (!api.config) {
      api.config = {};
    }
    api.config._snapshotDeserializer = effectiveDeserializer;
    stderr("[bootstrap] calling finalizeBootstrap");
    publicApi =
      api.finalizeBootstrap(snapshotConfig, effectiveDeserializer) ?? publicApi;
    stderr("[bootstrap] finalizeBootstrap done");
  }

  if (!publicApi && api.pyodide) {
    publicApi = api.pyodide;
  }
  if (!publicApi && api.public_api) {
    publicApi = api.public_api;
  }
  if (!publicApi) {
    throw new Error("Pyodide module did not expose a public API");
  }

  if (api.sys?.path?.insert) {
    api.sys.path.insert(0, "");
  }

  if (api._pyodide?.set_excepthook) {
    api._pyodide.set_excepthook();
  }

  if (typeof api.initializeStreams === "function") {
    stderr("[bootstrap] initializing streams");
    api.initializeStreams(stdin, stdout, stderr);
  }
  ensureSessionSitePackagesOnSysPath(module, publicApi);

  const versionTag =
    (publicApi && publicApi.version) ||
    globalThis.__pyRunnerPyodideVersion ||
    "0.28.2";
  if (!api.config.packageBaseUrl) {
    api.config.packageBaseUrl = `https://cdn.jsdelivr.net/pyodide/v${versionTag}/full/`;
  }

  adjustSysPathPostBootstrap(publicApi);

  const canonicalizePackageName = (name) =>
    (name || "")
      .toString()
      .trim()
      .replace(/[-_.]+/g, "-")
      .toLowerCase();

  const installedPackages = new Set();

  function readDynlibManifest() {
    const sessionRoot = Module?.FS?.sessionSitePackages
      ? String(Module.FS.sessionSitePackages).replace(/\/+$/, "")
      : "/session/site-packages";
    const manifest = [];
    for (const entry of overlayState.dynlibs.values()) {
      if (!entry || typeof entry.relPath !== "string") {
        continue;
      }
      const location = entry.location === "usr" ? "usr" : entry.location === "site" ? "site" : "other";
      const relPath = entry.relPath.replace(/^\/+/, "");
      if (!relPath) {
        continue;
      }
      let path;
      if (location === "usr") {
        path = `/usr/lib/${relPath}`;
      } else if (location === "site") {
          path = `${sessionRoot}/${relPath}`.replace(/\/+/g, "/");
      } else {
        path = relPath;
      }
      manifest.push({ path, location, relPath });
    }
    return manifest;
  }

  function normalizeDynlibManifest(entries) {
    if (!Array.isArray(entries) || entries.length === 0) {
      return [];
    }
    const sessionRoot = Module?.FS?.sessionSitePackages
      ? String(Module.FS.sessionSitePackages).replace(/\/+$/, "")
      : "/session/site-packages";
    const seen = new Set();
    const skipBasename = new Set(["libssl.so", "libcrypto.so"]);

    const normalized = [];
    for (const entry of entries) {
      let path;
      let location;
      let relPath;
      if (entry && typeof entry === "object") {
        if (typeof entry.path === "string" && entry.path.length > 0) {
          path = entry.path;
        }
        if (typeof entry.relPath === "string" && entry.relPath.length > 0) {
          relPath = entry.relPath.replace(/^\/+/, "");
        }
        if (entry.location === "usr" || entry.location === "dynlib") {
          location = "usr";
        } else if (entry.location === "site") {
          location = "site";
        }
      } else if (typeof entry === "string") {
        path = entry;
      }
      if (!path && relPath) {
        path = location === "usr" ? `/usr/lib/${relPath}` : `${sessionRoot}/${relPath}`;
      }
      if (!path) {
        continue;
      }
      if (!location) {
        if (path.startsWith("/usr/lib/")) {
          location = "usr";
          relPath = path.slice("/usr/lib/".length);
        } else if (path.startsWith(`${sessionRoot}/`)) {
          location = "site";
          relPath = path.slice(sessionRoot.length + 1);
        } else if (path.startsWith("/session/site-packages/")) {
          location = "site";
          relPath = path.slice("/session/site-packages/".length);
        } else {
          location = "site";
          relPath = path.replace(/^\/+/, "");
        }
      }
      if (!relPath) {
        relPath = path.replace(/^\/usr\/lib\//, "").replace(/^\/+/, "");
      }
      const key = `${location}:${relPath}`;
      if (!relPath || seen.has(key)) {
        continue;
      }
      seen.add(key);
      const basename = path.split("/").at(-1);
      if (basename && skipBasename.has(basename)) {
        continue;
      }
      normalized.push({ path, location, relPath });
    }
    const byLocation = {
      usr: [],
      site: [],
      other: [],
    };
    for (const entry of normalized) {
      if (entry.location === "usr") {
        byLocation.usr.push(entry);
      } else if (entry.location === "site") {
        byLocation.site.push(entry);
      } else {
        byLocation.other.push(entry);
      }
    }
    const sorter = (a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
    byLocation.usr.sort(sorter);
    byLocation.site.sort(sorter);
    byLocation.other.sort(sorter);
    return byLocation.usr.concat(byLocation.site, byLocation.other);
  }

  function preloadDynlibs(manifest) {
    if (!Array.isArray(manifest) || manifest.length === 0) {
      module.__pyRunnerDynlibManifest = [];
      return [];
    }
    let loadedSet = module.__pyRunnerLoadedDynlibs;
    if (!(loadedSet instanceof Set)) {
      loadedSet = new Set(Array.isArray(loadedSet) ? loadedSet : []);
    }
    module.__pyRunnerLoadedDynlibs = loadedSet;
    const sessionRoot = Module?.FS?.sessionSitePackages
      ? String(Module.FS.sessionSitePackages).replace(/\/+$/, "")
      : "/session/site-packages";
    const loader =
      typeof module.loadDynamicLibrary === "function"
        ? (path) => module.loadDynamicLibrary(path, { loadAsync: false, global: true })
        : module.API?._module?.loadDynamicLibrary
        ? (path) => module.API._module.loadDynamicLibrary(path, {
            loadAsync: false,
            global: true,
          })
        : null;
    const fallbackDlopen =
      !loader && typeof module.dlopen === "function"
        ? (path) => module.dlopen(path, 257)
        : null;
    const newlyLoaded = [];
    for (const entry of manifest) {
      if (!entry) {
        continue;
      }
      const location = entry?.location;
      let path = entry?.path;
      if ((!path || path.length === 0) && entry?.relPath) {
        if (location === "usr") {
          path = `/usr/lib/${entry.relPath}`;
        } else if (location === "site") {
          path = `${sessionRoot}/${entry.relPath}`;
        }
      }
      if (!path || loadedSet.has(path)) {
        continue;
      }
      if (location === "site") {
        continue;
      }
      let exists = false;
      try {
        const stat = module.FS.analyzePath(path);
        exists = Boolean(stat?.exists);
      } catch (_err) {
        exists = false;
      }
      if (!exists) {
        continue;
      }
      try {
        if (loader) {
          loader(path);
        } else if (fallbackDlopen) {
          fallbackDlopen(path);
        } else {
          continue;
        }
        loadedSet.add(path);
        newlyLoaded.push(path);
      } catch (error) {
        console.warn("[dynlib] failed to preload", path, error);
      }
    }
    module.__pyRunnerDynlibManifest = manifest;
    return newlyLoaded;
  }

  function refreshDynlibs() {
    const manifest = normalizeDynlibManifest(readDynlibManifest());
    const loaded = preloadDynlibs(manifest);
    if (loaded.length > 0 && globalThis.console?.info) {
      globalThis.console.info(
        "[dynlib] prepared dynamic libraries",
        loaded.length,
        "of",
        manifest.length
      );
    }
    return manifest;
  }

  await publicApi.runPythonAsync(
    `import datetime, importlib, io, zipfile
`
      + `_PYRUNNER_INSTALLED = set()

`
      + `def _py_runner_install_wheel(name, wheel_bytes, install_dir='site'):
`
      + `    canonical = name.lower().replace('-', '_').replace('.', '_')
`
      + `    if canonical in _PYRUNNER_INSTALLED:
`
      + `        return []
`
      + `    if isinstance(wheel_bytes, memoryview):
`
      + `        wheel_bytes = wheel_bytes.tobytes()
`
      + `    elif not isinstance(wheel_bytes, (bytes, bytearray)):
`
      + `        wheel_bytes = bytes(wheel_bytes)
`
      + `    data = io.BytesIO(wheel_bytes)
`
      + `    entries = []
`
      + `    with zipfile.ZipFile(data) as zf:
`
      + `        for info in zf.infolist():
`
      + `            if info.is_dir():
`
      + `                continue
`
      + `            rel = info.filename.lstrip('/')
`
      + `            if '..' in rel.split('/'):
`
      + `                continue
`
      + `            base = install_dir
`
      + `            if '.data/' in rel:
`
      + `                _, rest = rel.split('.data/', 1)
`
      + `                if '/' not in rest:
`
      + `                    continue
`
      + `                section, remainder = rest.split('/', 1)
`
      + `                if section in ('data', 'purelib', 'platlib'):
`
      + `                    rel = remainder
`
      + `                elif section == 'headers':
`
      + `                    rel = f"include/{remainder}"
`
      + `                elif section == 'lib':
`
      + `                    base = 'dynlib'
`
      + `                    rel = remainder
`
      + `                else:
`
      + `                    continue
`
      + `            payload = memoryview(zf.read(info))
`
      + `            mode = (info.external_attr >> 16) & 0o777
`
      + `            if not mode:
`
      + `                mode = 0o644
`
      + `            try:
`
      + `                mtime = int(datetime.datetime(*info.date_time).timestamp())
`
      + `            except Exception:
`
      + `                mtime = 0
`
      + `            entries.append((base, rel, payload, mode, mtime))
`
      + `    _PYRUNNER_INSTALLED.add(canonical)
`
      + `    importlib.invalidate_caches()
`
      + `    return entries
`
  );

  function resolveRequirements(requested, lockfile) {
    const packages = (lockfile && lockfile.packages) || {};
    const ordered = [];
    const seen = new Set();

    function visit(name) {
      const canonical = canonicalizePackageName(name);
      if (installedPackages.has(canonical) || seen.has(canonical)) {
        return;
      }
      const meta = packages[canonical];
      if (!meta) {
        console.warn(`[packages] missing '${name}' in lockfile`);
        return;
      }
      (meta.depends || []).forEach(visit);
      seen.add(canonical);
      ordered.push({ canonical, meta });
    }

    requested.forEach(visit);
    return ordered;
  }

  async function fetchPackageBuffer(meta, baseUrl) {
    const targetUrl = new URL(meta.file_name, baseUrl).toString();
    const response = await fetch(targetUrl);
    if (!response.ok) {
      throw new Error(`Failed to fetch package '${meta.name}' (${response.status} ${response.statusText})`);
    }
    return response.arrayBuffer();
  }

  async function installWheel(pyodide, requirement, buffer) {
    const { canonical, meta } = requirement;
    const installer = pyodide.globals.get("_py_runner_install_wheel");
    const wheelBytes = new Uint8Array(buffer);
    const wheelProxy = pyodide.toPy(wheelBytes);
    const packageName = meta.name ?? (meta.imports && meta.imports[0]) ?? meta.file_name;
    const nameProxy = pyodide.toPy(packageName);
    const installDir = meta.install_dir || "site";
    const installProxy = pyodide.toPy(installDir);
    const entriesProxy = installer(nameProxy, wheelProxy, installProxy);
    if (typeof wheelProxy?.destroy === "function") {
      wheelProxy.destroy();
    }
    if (typeof nameProxy?.destroy === "function") {
      nameProxy.destroy();
    }
    if (typeof installProxy?.destroy === "function") {
      installProxy.destroy();
    }
    overlayInvalidateTarCache();
    overlayEnsureMounted(module);
    const tarBuilder = createTarBuilder();
    if (entriesProxy && typeof entriesProxy[Symbol.iterator] === "function") {
      for (const entryProxy of entriesProxy) {
        let locationObj;
        let relObj;
        let payloadObj;
        let modeObj;
        let mtimeObj;
        try {
          locationObj = entryProxy?.get?.(0);
          relObj = entryProxy?.get?.(1);
          payloadObj = entryProxy?.get?.(2);
          modeObj = entryProxy?.get?.(3);
          mtimeObj = entryProxy?.get?.(4);
          const location = locationObj ? locationObj.toString() : "site";
          const relPath = relObj ? relObj.toString() : "";
          if (!relPath) {
            continue;
          }
          const modeValue = modeObj && typeof modeObj.toJs === "function"
            ? modeObj.toJs({ create_proxies: false })
            : modeObj;
          const mode = Number.isFinite(Number(modeValue)) ? Number(modeValue) : 0o644;
          const mtimeValue = mtimeObj && typeof mtimeObj.toJs === "function"
            ? mtimeObj.toJs({ create_proxies: false })
            : mtimeObj;
          const mtime = Number.isFinite(Number(mtimeValue)) ? Number(mtimeValue) : nowSeconds();
          const data = payloadObj
            ? payloadObj.toJs({ create_proxies: false }) ?? new Uint8Array()
            : new Uint8Array();
          const locationKey =
            location === "dynlib"
              ? "usr"
              : location === "usr"
              ? "usr"
              : "site";
          if (locationKey === "usr") {
            overlayAddFile(overlayState.usrRoot, relPath, data, mode, mtime);
          } else {
            overlayAddFile(overlayState.siteRoot, relPath, data, mode, mtime);
          }
          if (DYN_LIB_SUFFIX_RE.test(relPath)) {
            overlayRecordDynlib(locationKey, relPath);
          }
          tarBuilder.addFile(locationKey, relPath, data, mode, mtime);
        } finally {
          payloadObj?.destroy?.();
          relObj?.destroy?.();
          locationObj?.destroy?.();
          modeObj?.destroy?.();
          mtimeObj?.destroy?.();
          entryProxy?.destroy?.();
        }
      }
    }
    try {
      const { tar, mounts } = tarBuilder.finalize();
      if (tar instanceof Uint8Array && tar.length > 0) {
        overlayState.blobByKey.set(canonical, { data: tar, size: tar.length });
        const catalogEntry =
          overlayState.packageCatalog.get(canonical) ?? {
            name: meta.name ?? canonical,
            digest: meta.digest ?? null,
            mounts: new Set(),
            tar,
          };
        catalogEntry.tar = tar;
        if (!(catalogEntry.mounts instanceof Set)) {
          catalogEntry.mounts = new Set();
        }
        if (Array.isArray(mounts)) {
          for (const mount of mounts) {
            catalogEntry.mounts.add(mount);
          }
        }
        overlayState.packageCatalog.set(canonical, catalogEntry);
        overlayState.cachedTar = null;
      }
    } catch (error) {
      console.warn("[overlay] unable to finalize tar for package", canonical, error);
    }
    entriesProxy?.destroy?.();
    installer?.destroy?.();
    overlayState.packages.add(canonical);
  }

  const packageBaseUrl = api.config.packageBaseUrl;
  const lockfile = module.API?.lockFile ?? null;

  const sharedBufferState = {
    map: new Map(),
    nextId: 1,
  };

  const inputBufferState = {
    buffers: Object.create(null),
    metadata: Object.create(null),
  };

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

  function normalizeSharedBufferInput(value) {
    if (value == null) {
      throw new TypeError("shared buffer payload must be provided");
    }
    let proxy = null;
    let candidate = value;
    if (typeof candidate.toJs === "function") {
      proxy = candidate;
      candidate = proxy.toJs({ create_proxies: false });
    }
    if (candidate instanceof Uint8Array) {
      return { view: candidate, proxy };
    }
    if (ArrayBuffer.isView(candidate)) {
      const slice = new Uint8Array(
        candidate.buffer,
        candidate.byteOffset ?? 0,
        candidate.byteLength ?? candidate.length ?? 0
      );
      if (Object.prototype.hasOwnProperty.call(candidate, "__aardvarkBufferId")) {
        attachBufferId(slice, candidate.__aardvarkBufferId);
      }
      return { view: slice, proxy };
    }
    if (candidate instanceof ArrayBuffer) {
      return { view: new Uint8Array(candidate), proxy };
    }
    if (typeof candidate === "string") {
      return { view: textEncoder.encode(candidate), proxy };
    }
    throw new TypeError("shared buffer payload must be bytes-like");
  }

  function normalizeMetadataInput(value) {
    if (value == null) {
      return null;
    }
    if (typeof value.toJs === "function") {
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

  globalThis.__aardvarkAcquireOutputBuffer = function (bufferId, size, metadata) {
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
    const metaObject = normalizeMetadataInput(metadata);
    sharedBufferState.map.set(assigned, {
      view,
      proxy: null,
      metadata: metaObject,
    });
    if (typeof globalThis.__aardvarkRecordBufferEvent === "function") {
      try {
        globalThis.__aardvarkRecordBufferEvent("acquire", assigned, view.byteLength, metaObject ?? null);
      } catch (err) {
        console.warn("[aardvark] failed to record buffer acquire event", err);
      }
    }
    return view;
  };

  globalThis.__aardvarkPublishBuffer = function (bufferId, data, metadata) {
    requireCapability("rawctx_buffers");
    const explicitId = bufferId != null && bufferId !== "" ? String(bufferId) : null;
    const metaObject = normalizeMetadataInput(metadata);

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
        if (typeof globalThis.__aardvarkRecordBufferEvent === "function") {
          try {
            globalThis.__aardvarkRecordBufferEvent(
              "publish",
              candidateId,
              entry.view?.byteLength ?? 0,
              entry.metadata ?? null,
            );
          } catch (err) {
            console.warn("[aardvark] failed to record buffer publish event", err);
          }
        }
        return candidateId;
      }
    }

    const { view, proxy } = normalizeSharedBufferInput(data);
    attachBufferId(view, assigned);
    sharedBufferState.map.set(assigned, {
      view,
      proxy: proxy ?? null,
      metadata: metaObject ?? null,
    });
    if (typeof globalThis.__aardvarkRecordBufferEvent === "function") {
      try {
        globalThis.__aardvarkRecordBufferEvent("publish", assigned, view.byteLength, metaObject ?? null);
      } catch (err) {
        console.warn("[aardvark] failed to record buffer publish event", err);
      }
    }
    return assigned;
  };

globalThis.__aardvarkCollectSharedBuffers = function () {
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

globalThis.__aardvarkReleaseSharedBuffers = function (ids) {
    requireCapability("rawctx_buffers");
    const pending = Array.isArray(ids)
      ? ids
      : Array.from(sharedBufferState.map.keys());
    for (const id of pending) {
      const entry = sharedBufferState.map.get(id);
      if (!entry) {
        continue;
      }
      try {
        entry.proxy?.destroy?.();
      } catch (error) {
        console.warn("[buffers] failed to destroy proxy for", id, error);
      }
      sharedBufferState.map.delete(id);
    }
  };

globalThis.__aardvarkResetSharedBuffers = function () {
    requireCapability("rawctx_buffers");
    globalThis.__aardvarkReleaseSharedBuffers();
  };

  globalThis.__aardvarkClearInputBuffers = function () {
    requireCapability("rawctx_buffers");
    const buffers = Object.create(null);
    const metadata = Object.create(null);
    inputBufferState.buffers = buffers;
    inputBufferState.metadata = metadata;
    globalThis.__aardvarkInputBuffers = buffers;
    globalThis.__aardvarkInputMetadata = metadata;
  };

globalThis.__aardvarkRegisterInputBuffer = function (name, buffer, metadata) {
    requireCapability("rawctx_buffers");
    if (typeof name !== "string" || name.length === 0) {
      throw new TypeError("input buffer must have a non-empty name");
    }
    if (!(buffer instanceof Uint8Array)) {
      throw new TypeError("input buffer expects a Uint8Array payload");
    }
    if (!globalThis.__aardvarkInputBuffers) {
      globalThis.__aardvarkInputBuffers = inputBufferState.buffers;
    }
    if (!globalThis.__aardvarkInputMetadata) {
      globalThis.__aardvarkInputMetadata = inputBufferState.metadata;
    }
    globalThis.__aardvarkInputBuffers[name] = buffer;
    if (metadata === undefined) {
      delete globalThis.__aardvarkInputMetadata[name];
    } else {
      globalThis.__aardvarkInputMetadata[name] = metadata;
    }
  };

  globalThis.__pyRunnerLoadPackages = async (names) => {
    const list = Array.isArray(names) ? names.filter(Boolean) : [names];
    if (!list.length) {
      return;
    }
    if (!lockfile) {
      throw new Error("Package lockfile is unavailable; cannot load packages");
    }
    const requirements = resolveRequirements(list, lockfile);
    if (!requirements.length) {
      return;
    }
    console.info("[packages] loading", requirements.map((entry) => entry.meta.name));
    for (const entry of requirements) {
      if (installedPackages.has(entry.canonical)) {
        continue;
      }
      const { meta } = entry;
      if (
        meta.package_type &&
        meta.package_type !== "package" &&
        meta.package_type !== "shared_library"
      ) {
        console.warn(`[packages] skipping unsupported package type '${meta.package_type}' for ${meta.name}`);
        installedPackages.add(entry.canonical);
        continue;
      }
      if (!meta.file_name) {
        console.warn(`[packages] package '${meta.name}' is missing file metadata; skipping`);
        installedPackages.add(entry.canonical);
        continue;
      }
      try {
        const buffer = await fetchPackageBuffer(meta, packageBaseUrl);
        await installWheel(publicApi, entry, buffer);
        installedPackages.add(entry.canonical);
        console.info(`[packages] installed ${meta.name}`);
      } catch (error) {
        console.error(`[packages] failed to install ${meta.name}`, error);
        throw error;
      }
    }
    const manifest = refreshDynlibs();
    if (manifest.length === 0 && globalThis.console?.debug) {
      globalThis.console.debug("[dynlib] no dynamic libraries discovered");
    }
    ensureSessionSitePackagesOnSysPath(module, publicApi);
  };

  globalThis.__pyRunnerPrepareDynlibs = () => refreshDynlibs();
  globalThis.__pyRunnerExportOverlay = () => {
    try {
      const packages = [];
      for (const canonical of overlayState.packages) {
        const entry = overlayState.packageCatalog.get(canonical);
        if (!entry) {
          continue;
        }
        const mounts =
          entry.mounts instanceof Set
            ? Array.from(entry.mounts)
            : Array.isArray(entry.mounts)
            ? entry.mounts.slice()
            : [];
        packages.push({
          canonical,
          name: entry.name ?? canonical,
          digest: entry.digest ?? null,
          blob: canonical,
          mounts,
          size: entry.tar instanceof Uint8Array ? entry.tar.length : 0,
        });
      }
      const blobs = {};
      for (const [key, entry] of overlayState.packageCatalog.entries()) {
        if (!entry || !(entry.tar instanceof Uint8Array)) {
          continue;
        }
        blobs[key] = {
          size: entry.tar.length ?? 0,
          digest: entry.digest ?? null,
        };
      }
      const sessionRoot = Module?.FS?.sessionSitePackages
        ? String(Module.FS.sessionSitePackages).replace(/\/+$/, "")
        : "/session/site-packages";
      const dynlibs = [];
      for (const entry of overlayState.dynlibs.values()) {
        if (!entry || typeof entry.relPath !== "string") {
          continue;
        }
        const location = entry.location === "usr" ? "usr" : entry.location === "site" ? "site" : "other";
        const relPath = entry.relPath.replace(/^\/+/, "");
        if (!relPath) {
          continue;
        }
        let path;
        if (location === "usr") {
          path = `/usr/lib/${relPath}`;
        } else if (location === "site") {
          path = `${sessionRoot}/${relPath}`.replace(/\/+/g, "/");
        } else {
          path = relPath;
        }
        dynlibs.push({ path, location, relPath });
      }
      const payload = {
        version: 3,
        format: "catalog",
        dynlibs,
        packages,
        blobs,
      };
      return textEncoder.encode(JSON.stringify(payload));
    } catch (error) {
      console.warn("[snapshot] export overlay failed", error);
      return new Uint8Array();
    }
  };

  globalThis.__pyRunnerExportOverlayBlobs = () => {
    try {
      const result = [];
      for (const [key, entry] of overlayState.packageCatalog.entries()) {
        if (!entry || !(entry.tar instanceof Uint8Array)) {
          continue;
        }
        result.push({
          key,
          digest: entry.digest ?? null,
          data: entry.tar,
        });
      }
      return result;
    } catch (error) {
      console.warn("[snapshot] export overlay blobs failed", error);
      return [];
    }
  };

  globalThis.__pyRunnerExportOverlayTar = () => {
    try {
      const ordered = [];
      for (const canonical of overlayState.packages) {
        const entry = overlayState.packageCatalog.get(canonical);
        if (!entry || !(entry.tar instanceof Uint8Array)) {
          continue;
        }
        ordered.push(entry.tar);
      }
      if (ordered.length === 0) {
        return new Uint8Array();
      }
      return concatTarArchives(ordered);
    } catch (error) {
      console.warn("[snapshot] export overlay tar failed", error);
      return new Uint8Array();
    }
  };

  globalThis.__pyRunnerImportOverlay = (metadataInput, blobsInput) => {
    try {
      const nativeLog =
        typeof globalThis.__pyRunnerNativeLog === "function"
          ? globalThis.__pyRunnerNativeLog
          : null;
      const metaBytes =
        metadataInput instanceof Uint8Array
          ? metadataInput
          : new Uint8Array(metadataInput ?? 0);
      const blobArray = Array.isArray(blobsInput)
        ? blobsInput
        : blobsInput instanceof Uint8Array
        ? [{ key: "tar", data: blobsInput }]
        : blobsInput && typeof blobsInput.length === "number"
        ? [{ key: "tar", data: new Uint8Array(blobsInput) }]
        : [];
      if (metaBytes.length === 0) {
        overlayClear(overlayState.siteRoot);
        overlayClear(overlayState.usrRoot);
        overlayState.dynlibs.clear();
        overlayState.packages.clear();
        overlayState.packageCatalog.clear();
        overlayState.blobByKey.clear();
        installedPackages.clear();
        refreshDynlibs();
        return;
      }
      const json = textDecoder.decode(metaBytes);
      const payload = JSON.parse(json);
      let tarBytes;
      const useCatalog =
        payload && Number(payload.version) >= 3 && payload.format === "catalog";
      if (useCatalog) {
        overlayState.packageCatalog.clear();
        overlayState.blobByKey.clear();
        overlayState.cachedTar = null;
        overlayState.tarReaders = [];
        overlayState.tarFileMap.clear();
        overlayState.tarDynlibFiles = [];
        overlayState.tarReader = null;
        overlayState.tarSiteRoot = null;
        overlayState.tarUsrRoot = null;
        overlayState.dynlibs.clear();
        overlayState.packages.clear();
        installedPackages.clear();
        const dynlibEntries = Array.isArray(payload.dynlibs)
          ? payload.dynlibs
          : [];
        let populateDynlibsFromTar = dynlibEntries.length === 0;
        if (!populateDynlibsFromTar) {
          for (const entry of dynlibEntries) {
            if (!entry) {
              continue;
            }
            if (typeof entry === "string") {
              const value = String(entry);
              if (value.startsWith("/usr/lib/")) {
                overlayRecordDynlib("usr", value.slice(9));
              } else if (value.startsWith("/session/site-packages/")) {
                overlayRecordDynlib("site", value.slice(23));
              } else {
                overlayRecordDynlib("site", value.replace(/^\/+/, ""));
              }
              continue;
            }
            if (typeof entry === "object") {
              let location = entry.location === "usr" || entry.location === "dynlib" ? "usr" : entry.location === "site" ? "site" : null;
              let rel = typeof entry.relPath === "string" ? entry.relPath.replace(/^\/+/, "") : "";
              if (!rel && typeof entry.path === "string") {
                const pathStr = entry.path;
                if (pathStr.startsWith("/usr/lib/")) {
                  location = "usr";
                  rel = pathStr.slice(9);
                } else if (pathStr.startsWith("/session/site-packages/")) {
                  location = location ?? "site";
                  rel = pathStr.slice(23);
                } else {
                  rel = pathStr.replace(/^\/+/, "");
                }
              }
              if (location && rel) {
                overlayRecordDynlib(location, rel);
              }
            }
          }
        }
        const blobLookup = new Map();
        for (const blobEntry of blobArray) {
          if (!blobEntry) {
            continue;
          }
          const key =
            typeof blobEntry.digest === "string" && blobEntry.digest.length > 0
              ? blobEntry.digest
              : typeof blobEntry.key === "string" && blobEntry.key.length > 0
              ? blobEntry.key
              : "";
          if (!key) {
            continue;
          }
          const data =
            blobEntry.data instanceof Uint8Array
              ? blobEntry.data
              : blobEntry.bytes instanceof Uint8Array
              ? blobEntry.bytes
              : new Uint8Array(blobEntry.data ?? 0);
          blobLookup.set(key, data);
        }
        const packageEntries = Array.isArray(payload.packages)
          ? payload.packages
          : [];
        const orderedTars = [];
        for (const pkg of packageEntries) {
          if (!pkg) {
            continue;
          }
          const canonical =
            typeof pkg.canonical === "string" && pkg.canonical.length > 0
              ? pkg.canonical
              : typeof pkg === "string"
              ? pkg
              : "";
          if (!canonical) {
            continue;
          }
          const digestKey =
            typeof pkg.digest === "string" && pkg.digest.length > 0
              ? pkg.digest
              : typeof pkg.blob === "string" && pkg.blob.length > 0
              ? pkg.blob
              : canonical;
          let tarData = blobLookup.get(digestKey);
          if (!(tarData instanceof Uint8Array) && blobLookup.has(canonical)) {
            tarData = blobLookup.get(canonical);
          }
          if (!(tarData instanceof Uint8Array)) {
            console.warn(
              `[overlay] missing tar data for package '${canonical}' (key '${digestKey}')`
            );
            continue;
          }
          const mounts =
            Array.isArray(pkg.mounts) && pkg.mounts.length > 0
              ? pkg.mounts.map((value) => String(value))
              : [];
          const entry = {
            name: pkg.name ?? canonical,
            digest:
              typeof pkg.digest === "string" && pkg.digest.length > 0
                ? pkg.digest
                : null,
            mounts: new Set(mounts),
            tar: tarData,
          };
          overlayState.packageCatalog.set(canonical, entry);
          overlayState.blobByKey.set(canonical, {
            data: tarData,
            size: tarData.length,
          });
          overlayState.packages.add(canonical);
          installedPackages.add(canonical);
          orderedTars.push(tarData);
        }
        tarBytes =
          orderedTars.length > 0 ? concatTarArchives(orderedTars) : new Uint8Array();
      } else {
        const directTar =
          blobsInput instanceof Uint8Array
            ? blobsInput
            : new Uint8Array(blobsInput ?? 0);
        tarBytes = directTar;
        overlayState.dynlibs.clear();
        for (const entry of payload?.dynlibs || []) {
          if (typeof entry === "string") {
            const value = entry;
            if (value.startsWith("/usr/lib/")) {
              overlayRecordDynlib("usr", value.slice(9));
            } else if (value.startsWith("/session/site-packages/")) {
              overlayRecordDynlib("site", value.slice(23));
            } else {
              overlayRecordDynlib("site", value.replace(/^\/+/, ""));
            }
          } else if (entry && typeof entry === "object") {
            const location = entry.location === "usr" || entry.location === "dynlib" ? "usr" : "site";
            const rel = typeof entry.relPath === "string" ? entry.relPath.replace(/^\/+/, "") : "";
            if (rel) {
              overlayRecordDynlib(location, rel);
            }
          }
        }
        overlayState.packages.clear();
        installedPackages.clear();
        for (const pkg of payload?.packages || []) {
          const canonical = String(pkg);
          overlayState.packages.add(canonical);
          installedPackages.add(canonical);
        }
      }
      if (
        !useCatalog &&
        payload?.tar &&
        Number.isFinite(Number(payload.tar.size)) &&
        tarBytes.length !== Number(payload.tar.size)
      ) {
        console.warn(
          `[overlay] tar size mismatch, expected ${payload.tar.size} bytes but received ${tarBytes.length}`
        );
      }
      overlayLoadTar(tarBytes);
      if (populateDynlibsFromTar && overlayState.dynlibs.size === 0) {
        for (const tarPath of overlayState.tarDynlibFiles) {
          if (typeof tarPath !== "string") {
            continue;
          }
          const normalized = tarPath.replace(/^\/+/, "");
          if (normalized.startsWith("usr/")) {
            overlayRecordDynlib("usr", normalized.slice(4));
          } else if (normalized.startsWith("site/")) {
            overlayRecordDynlib("site", normalized.slice(5));
          }
        }
      }
      overlayEnsureMounted(module);
      refreshDynlibs();
      try {
        const sitePath = `${module.FS.sessionSitePackages}/numpy/__init__.py`;
        const stat = module.FS.analyzePath(sitePath);
        if (stat?.exists) {
          nativeLog?.(`[overlay] numpy present at ${sitePath}`);
        } else {
          nativeLog?.(`[overlay] numpy missing at ${sitePath}`);
        }
      } catch (err) {
        nativeLog?.(`[overlay] analyzePath failed: ${String(err)}`);
      }
      try {
        simpleRunPython(
          module,
          "import importlib\nimportlib.invalidate_caches()\ndel importlib"
        );
      } catch (err) {
        console.warn("[overlay] invalidate caches failed", err);
      }
      try {
        const ensureSysPathCode = `import sys\nimport importlib\npath = "${module.FS.sessionSitePackages}"\nif path not in sys.path:\n    sys.path.insert(0, path)\nsys.path_importer_cache.pop(path, None)\nimportlib.invalidate_caches()\ndel sys, importlib, path`;
        simpleRunPython(module, ensureSysPathCode);
      } catch (err) {
        nativeLog?.(`[overlay] failed to patch sys.path: ${String(err)}`);
      }
    } catch (error) {
      console.warn("[snapshot] import overlay failed", error);
    }
  };

  globalThis.__pyRunnerMakeSnapshot = () => {
    if (!publicApi.makeMemorySnapshot) {
      throw new Error("pyodide.makeMemorySnapshot is unavailable");
    }
    const nativeLog =
      typeof globalThis.__pyRunnerNativeLog === "function"
        ? globalThis.__pyRunnerNativeLog
        : null;
    if (nativeLog) {
      nativeLog(
        `[snapshot] config.buildId: ${String(module.API?.config?.buildId ?? "(unset)")}`
      );
      if (module.API?.config?.snapshotBuildId) {
        nativeLog(
          `[snapshot] config.snapshotBuildId: ${String(
            module.API.config.snapshotBuildId
          )}`
        );
      }
    }
  if (!module.API.config) {
    module.API.config = {};
  }
  if (!module.API.config.buildId) {
    module.API.config.buildId = "dev";
  }
  if (!module.API.config.snapshotBuildId) {
    module.API.config.snapshotBuildId = module.API.config.buildId;
  }
  if (!module.API.config.BUILD_ID) {
    module.API.config.BUILD_ID = module.API.config.buildId;
  }
    try {
      publicApi.runPython("import gc; gc.collect()");
    } catch (err) {
      if (nativeLog) {
        nativeLog(`[snapshot] gc.collect failed: ${String(err)}`);
      } else {
        console.warn("[snapshot] gc.collect failed", err);
      }
    }
    try {
      module.__hiwire_set?.(6, {});
    } catch (err) {
      if (nativeLog) {
        nativeLog(`[snapshot] failed to patch hiwire slot: ${String(err)}`);
      } else if (globalThis.console?.warn) {
        globalThis.console.warn("[snapshot] failed to patch hiwire slot", err);
      }
    }
    if (nativeLog) {
      for (let i = 0; i < 16; i += 1) {
        try {
          const value =
            module.__hiwire_get?.(i) ??
            module.__pyodide?._module?.__hiwire_get?.(i);
          let text;
          if (value === null) {
            text = "null";
          } else if (value === undefined) {
            text = "undefined";
          } else {
            const ctor =
              typeof value === "object" && value && value.constructor
                ? value.constructor.name
                : typeof value;
            const overlayBlob = overlayBlobRegistry.get(value);
            const overlayHint = overlayBlob
              ? ` overlay(${overlayBlob.location}:${overlayBlob.relPath})`
              : "";
            text = `[${ctor}]${overlayHint}`;
          }
          nativeLog(`[snapshot] hiwire ${i}: ${text}`);
        } catch (err) {
          nativeLog(`[snapshot] hiwire read failed ${i}: ${String(err)}`);
          break;
        }
      }
    } else if (globalThis.console?.log) {
      for (let i = 0; i < 10; i += 1) {
        try {
          const value =
            module.__hiwire_get?.(i) ??
            module.__pyodide?._module?.__hiwire_get?.(i);
          globalThis.console.log("[snapshot] hiwire", i, value);
        } catch (err) {
          globalThis.console.log("[snapshot] hiwire read failed", i, err);
          break;
        }
      }
    }
    try {
      const originalGetExpected = module.API?.getExpectedKeys?.bind(module.API);
      if (originalGetExpected) {
        const template = originalGetExpected();
        const patched = template.map((fallback, index) => {
          try {
            return (
              module.__hiwire_get?.(index) ??
              module.__pyodide?._module?.__hiwire_get?.(index) ??
              fallback
            );
          } catch (_err) {
            return fallback;
          }
        });
        module.API.getExpectedKeys = () => patched;
      }
    } catch (err) {
      if (globalThis.console?.warn) {
        globalThis.console.warn("[snapshot] unable to patch expected keys", err);
      }
    }
  return publicApi.makeMemorySnapshot({
    serializer(obj) {
      if (obj === null || obj === undefined) {
        return null;
      }
      if (typeof obj === "function") {
        return { __type: "function", name: obj.name || "" };
      }
      if (obj instanceof Uint8Array) {
        const overlayRef = overlayBlobRegistry.get(obj);
        if (overlayRef && typeof overlayRef.relPath === "string") {
          return {
            __type: "overlay-blob",
            location: overlayRef.location,
            path: overlayRef.relPath,
            length: obj.length,
          };
        }
        const preview = Array.from(obj.subarray(0, 16));
        return {
          __type: "uint8array",
          length: obj.length,
          preview,
        };
      }
      if (ArrayBuffer.isView(obj)) {
        const meta = overlayLookupMetaForView(obj);
        const ctorName = obj.constructor?.name ?? "TypedArray";
        const byteLength = Number.isFinite(obj.byteLength) ? obj.byteLength : 0;
        const byteOffset = Number.isFinite(obj.byteOffset) ? obj.byteOffset : 0;
        const bytesPerElement =
          Number.isFinite(obj.BYTES_PER_ELEMENT) && obj.BYTES_PER_ELEMENT > 0
            ? obj.BYTES_PER_ELEMENT
            : undefined;
        const elementLength =
          Number.isFinite(obj.length) && obj.length >= 0
            ? obj.length
            : bytesPerElement
            ? Math.floor(byteLength / bytesPerElement)
            : byteLength;
        if (
          meta &&
          Number.isFinite(elementLength) &&
          Number.isFinite(byteLength) &&
          elementLength >= 0 &&
          byteLength >= 0
        ) {
          const relativeOffset = byteOffset - meta.byteOffset;
          if (
            relativeOffset >= 0 &&
            relativeOffset + byteLength <= meta.byteLength
          ) {
            return {
              __type: "overlay-typedarray",
              ctor: ctorName,
              location: meta.location,
              path: meta.relPath,
              offset: relativeOffset,
              byteLength,
              length: elementLength,
              bytesPerElement,
            };
          }
        }
        const view = new Uint8Array(obj.buffer, obj.byteOffset, obj.byteLength);
        const preview = Array.from(view.subarray(0, 16));
        return {
          __type: "typedarray",
          ctor: ctorName,
          length: elementLength || view.length,
          preview,
        };
      }
      if (typeof obj === "object") {
        const ctor = obj.constructor ? obj.constructor.name : "Object";
        const keys = Object.keys(obj).slice(0, 8);
        return { __type: "object", ctor, keys };
      }
      return obj;
    },
  });
  };

  return publicApi;
}
