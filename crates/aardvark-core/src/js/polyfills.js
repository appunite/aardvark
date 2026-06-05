// Minimal host polyfills for the Pyodide runtime environment.
globalThis.self = globalThis;

if (typeof globalThis.crossOriginIsolated === "undefined") {
  globalThis.crossOriginIsolated = false;
}

const __pyRunnerForwardLogFactory = (stream) => (...args) => {
  const message = args
    .map((value) => {
      try {
        if (value instanceof Error) {
          return value.stack ?? `${value.name}: ${value.message}`;
        }
        if (typeof value === "string") {
          return value;
        }
        if (
          value != null &&
          typeof value === "object" &&
          typeof value.toString === "function" &&
          value.toString !== Object.prototype.toString
        ) {
          return value.toString();
        }
        return JSON.stringify(value);
      } catch (_err) {
        return String(value);
      }
    })
    .join(" ");
  if (typeof globalThis.__pyRunnerNativeLog === "function") {
    globalThis.__pyRunnerNativeLog(stream, message);
  }
};

if (!globalThis.console) {
  globalThis.console = {};
}
const __pyRunnerStdoutLog = __pyRunnerForwardLogFactory("__stdout__");
const __pyRunnerStderrLog = __pyRunnerForwardLogFactory("__stderr__");
globalThis.console.log = __pyRunnerStdoutLog;
globalThis.console.info = __pyRunnerStdoutLog;
globalThis.console.warn = __pyRunnerStderrLog;
globalThis.console.error = __pyRunnerStderrLog;

if (typeof globalThis.addEventListener !== "function") {
  globalThis.addEventListener = () => {};
}

if (!globalThis.crypto) {
  globalThis.crypto = {
    getRandomValues(buffer) {
      if (!(buffer instanceof Uint8Array)) {
        throw new TypeError("getRandomValues expects a Uint8Array");
      }
      for (let i = 0; i < buffer.length; i += 1) {
        buffer[i] = Math.floor(Math.random() * 256);
      }
      return buffer;
    },
  };
}

if (!globalThis.performance) {
  const start = Date.now();
  globalThis.performance = {
    now() {
      return Date.now() - start;
    },
  };
}

if (typeof globalThis.location === "undefined") {
  const origin = "https://pyodide.local";
  globalThis.location = {
    href: `${origin}/`,
    origin,
    protocol: "https:",
    host: "pyodide.local",
    hostname: "pyodide.local",
    port: "",
    pathname: "/",
    search: "",
    hash: "",
  };
}

if (typeof globalThis.URL === "undefined") {
  class SimpleURL {
    constructor(input, base = globalThis.location.href) {
      const baseUrl = base instanceof SimpleURL ? base.href : String(base ?? "");
      if (!input) {
        this.href = baseUrl;
      } else if (String(input).includes("://")) {
        this.href = String(input);
      } else {
        const originMatch = /^([a-zA-Z0-9+.-]+:\/\/[^/]+)(.*)$/.exec(baseUrl) || [];
        const origin = originMatch[1] ?? "https://pyodide.local";
        let prefix = originMatch[2] ?? "/";
        if (!prefix.endsWith("/")) {
          prefix = prefix.substring(0, prefix.lastIndexOf("/") + 1);
        }
        if (String(input).startsWith("/")) {
          this.href = origin + input;
        } else {
          this.href = origin + prefix + input;
        }
      }
      const match = /^([a-zA-Z0-9+.-]+:)(\/\/[^/]+)?(.*)$/.exec(this.href) || [];
      this.protocol = match[1] ?? "";
      this.host = (match[2] ?? "").replace(/^\/\//, "");
      this.hostname = this.host.split(":")[0] ?? "";
      this.port = this.host.includes(":") ? this.host.split(":")[1] : "";
      this.origin = this.host ? `${this.protocol}//${this.host}` : "";
      const rest = match[3] ?? "/";
      const hashIndex = rest.indexOf("#");
      const searchIndex = rest.indexOf("?");
      this.hash = hashIndex !== -1 ? rest.substring(hashIndex) : "";
      this.search = searchIndex !== -1
        ? rest.substring(searchIndex, hashIndex !== -1 ? hashIndex : undefined)
        : "";
      const pathEnd = searchIndex !== -1
        ? searchIndex
        : hashIndex !== -1
          ? hashIndex
          : undefined;
      this.pathname = rest.substring(0, pathEnd) || "/";
      this.href = `${this.origin}${this.pathname}${this.search}${this.hash}`;
    }

    toString() {
      return this.href;
    }

    toJSON() {
      return this.href;
    }
  }

  globalThis.URL = SimpleURL;
}

if (typeof globalThis.__pyRunnerFetchAsset !== "function") {
  globalThis.__pyRunnerFetchAsset = () => undefined;
}

const __pyRunnerGlobalEval = (0, eval);

if (typeof globalThis.btoa !== "function") {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
  globalThis.btoa = function btoaPolyfill(input = "") {
    let str = String(input);
    let output = "";
    for (
      let block = 0, charCode, i = 0, map = chars;
      str.charAt(i | 0) || ((map = "="), i % 1);
      output += map.charAt(63 & (block >> (8 - (i % 1) * 8)))
    ) {
      charCode = str.charCodeAt((i += 3 / 4));
      if (charCode > 0xff) {
        throw new Error("btoa polyfill received invalid character");
      }
      block = (block << 8) | charCode;
    }
    return output;
  };
}

if (typeof globalThis.atob !== "function") {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
  globalThis.atob = function atobPolyfill(input = "") {
    let str = String(input).replace(/=+$/, "");
    if (str.length % 4 === 1) {
      throw new Error("atob polyfill received invalid base64 input");
    }
    let output = "";
    for (let bc = 0, bs = 0, buffer, i = 0; (buffer = str.charAt(i++)); ) {
      buffer = chars.indexOf(buffer);
      if (buffer === -1) {
        continue;
      }
      bs = bc % 4 ? bs * 64 + buffer : buffer;
      if (bc++ % 4) {
        output += String.fromCharCode(255 & (bs >> ((-2 * bc) & 6)));
      }
    }
    return output;
  };
}

const normalizePathKey = (path) => {
  if (!path) {
    return path;
  }
  return path
    .replace(/\\/g, "/")
    .replace(/\/\.\//g, "/")
    .replace(/^\.\//, "")
    .replace(/^\/+/, "");
};

const __pyRunnerResolveAsset = (input) => {
  const asString = String(input ?? "");
  try {
    const base = globalThis.location?.href ?? "https://pyodide.local/";
    const url = new URL(asString, base);
    const key = normalizePathKey(
      url.pathname.startsWith("/") ? url.pathname.slice(1) : url.pathname,
    );
    return { key: key || asString, href: url.href };
  } catch (_err) {
    return { key: normalizePathKey(asString), href: asString };
  }
};

const __pyRunnerGetTextAsset = (specifier) => {
  const { key } = __pyRunnerResolveAsset(specifier);
  const asset =
    globalThis.__pyRunnerFetchAsset(key) ??
    globalThis.__pyRunnerFetchAsset(specifier);
  if (globalThis.console && typeof globalThis.console.log === "function") {
    globalThis.console.log(`[polyfill] load text asset '${specifier}' -> '${key}'`);
  }
  if (!asset) {
    throw new Error(`Asset not found: ${specifier}`);
  }
  if (asset.kind === "binary") {
    throw new TypeError(`Asset '${specifier}' is not text`);
  }
  return asset.data;
};

if (!globalThis.fetch) {
  const lower = (value) => String(value ?? "").toLowerCase();

  const cloneBuffer = (view) => {
    if (view instanceof Uint8Array) {
      return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength);
    }
    if (view instanceof ArrayBuffer) {
      return view.slice(0);
    }
    return new Uint8Array(0).buffer;
  };

  globalThis.fetch = async function fetchAsset(input, init = {}) {
    const { key, href } = __pyRunnerResolveAsset(
      input && typeof input === "object" && "url" in input ? input.url : input
    );
    let asset =
      globalThis.__pyRunnerFetchAsset(key) ??
      globalThis.__pyRunnerFetchAsset(href);

    if (!asset && /^https?:/i.test(href) && typeof globalThis.__pyRunnerNativeFetch === "function") {
      try {
        const remote = await globalThis.__pyRunnerNativeFetch(href, init);
        if (remote && remote.body) {
          asset = {
            kind: remote.binary ? "binary" : "text",
            data: remote.body,
            contentType: remote.contentType,
            status: remote.status,
            statusText: remote.statusText,
            headers: remote.headers,
          };
        }
      } catch (err) {
        console.warn("[polyfill] native fetch failed", err);
      }
    }
    if (globalThis.console && typeof globalThis.console.log === "function") {
      globalThis.console.log(`[polyfill] fetch '${key}' (${href})`);
    }
    if (!asset) {
      throw new Error(`Asset not found: ${key}`);
    }

    const isBinary = asset.kind === "binary";
    const headers = new Map();
    if (Array.isArray(asset.headers)) {
      for (const entry of asset.headers) {
        if (Array.isArray(entry) && entry.length === 2) {
          headers.set(String(entry[0]).toLowerCase(), String(entry[1]));
        }
      }
    }
    if (asset.contentType) {
      headers.set("content-type", asset.contentType);
    }
    if (!headers.has("content-type")) {
      headers.set(
        "content-type",
        isBinary ? "application/octet-stream" : "text/plain; charset=utf-8",
      );
    }

    const status = asset.status ?? 200;
    const statusText = asset.statusText ?? "OK";

    let cachedBuffer = null;
    let cachedText = isBinary ? null : asset.data;

    async function ensureBuffer() {
      if (cachedBuffer) {
        return cachedBuffer.slice(0);
      }
      if (isBinary) {
        cachedBuffer = cloneBuffer(asset.data);
        return cachedBuffer.slice(0);
      }
      const encoder = new TextEncoder();
      const encoded = encoder.encode(asset.data);
      cachedBuffer = encoded.buffer;
      return encoded.buffer.slice(0);
    }

    const response = {
      ok: status >= 200 && status < 300,
      status,
      statusText,
      url: href,
      headers: {
        get(name) {
          const normalized = lower(name);
          return headers.has(normalized) ? headers.get(normalized) : null;
        },
        has(name) {
          return headers.has(lower(name));
        },
        entries() {
          return headers.entries();
        },
      },
      async arrayBuffer() {
        return ensureBuffer();
      },
      async text() {
        if (!isBinary && cachedText != null) {
          return cachedText;
        }
        const buffer = await ensureBuffer();
        const decoder = new TextDecoder("utf-8");
        const text = decoder.decode(new Uint8Array(buffer));
        cachedText = text;
        return text;
      },
      async json() {
        return JSON.parse(await this.text());
      },
      async bytes() {
        const buffer = await ensureBuffer();
        return new Uint8Array(buffer);
      },
      clone() {
        return this;
      },
    };

    return response;
  };
}

if (typeof globalThis.XMLHttpRequest !== "function") {
  const XHR_UNSENT = 0;
  const XHR_OPENED = 1;
  const XHR_HEADERS_RECEIVED = 2;
  const XHR_LOADING = 3;
  const XHR_DONE = 4;

  const createXhrError = (name, message) => {
    const error = new Error(message);
    error.name = name;
    return error;
  };

  const xhrCloneBuffer = (view) => {
    if (view instanceof Uint8Array) {
      return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength);
    }
    if (ArrayBuffer.isView(view)) {
      return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength);
    }
    if (view instanceof ArrayBuffer) {
      return view.slice(0);
    }
    if (typeof view === "string") {
      return new TextEncoder().encode(view).buffer;
    }
    return new Uint8Array(0).buffer;
  };

  const xhrBytesFromBody = (body, binaryHint = true) => {
    if (body instanceof Uint8Array) {
      return new Uint8Array(xhrCloneBuffer(body));
    }
    if (ArrayBuffer.isView(body)) {
      return new Uint8Array(xhrCloneBuffer(body));
    }
    if (body instanceof ArrayBuffer) {
      return new Uint8Array(body.slice(0));
    }
    if (typeof body === "string") {
      return new TextEncoder().encode(body);
    }
    if (!body) {
      return new Uint8Array(0);
    }
    if (binaryHint && typeof body.length === "number") {
      try {
        return new Uint8Array(body);
      } catch (_err) {
        // fall through to string conversion
      }
    }
    return new TextEncoder().encode(String(body));
  };

  const xhrHeadersFromResponse = (response) => {
    const headers = new Map();
    const addHeader = (name, value) => {
      if (name == null || value == null) {
        return;
      }
      headers.set(String(name).toLowerCase(), String(value));
    };

    if (Array.isArray(response?.headers)) {
      for (const entry of response.headers) {
        if (Array.isArray(entry) && entry.length >= 2) {
          addHeader(entry[0], entry[1]);
        }
      }
    } else if (response?.headers && typeof response.headers.entries === "function") {
      for (const [name, value] of response.headers.entries()) {
        addHeader(name, value);
      }
    } else if (response?.headers && typeof response.headers === "object") {
      for (const name of Object.keys(response.headers)) {
        addHeader(name, response.headers[name]);
      }
    }

    if (response?.contentType && !headers.has("content-type")) {
      addHeader("content-type", response.contentType);
    }
    return headers;
  };

  const parseDataUrlForXhr = (href) => {
    const comma = href.indexOf(",");
    if (comma === -1) {
      throw createXhrError("NetworkError", "invalid data URL");
    }
    const meta = href.slice(5, comma);
    const payload = href.slice(comma + 1);
    const contentType = meta.split(";")[0] || "text/plain;charset=US-ASCII";
    const isBase64 = /(?:^|;)base64(?:;|$)/i.test(meta);
    let bytes;
    if (isBase64) {
      const decoded = atob(payload);
      bytes = new Uint8Array(decoded.length);
      for (let i = 0; i < decoded.length; i += 1) {
        bytes[i] = decoded.charCodeAt(i);
      }
    } else {
      let text;
      try {
        text = decodeURIComponent(payload);
      } catch (_err) {
        text = payload;
      }
      bytes = new TextEncoder().encode(text);
    }
    return {
      status: 200,
      statusText: "OK",
      url: href,
      binary: true,
      body: bytes,
      headers: [["content-type", contentType]],
      contentType,
    };
  };

  const resolveXhrResponse = (url, init) => {
    if (/^data:/i.test(String(url ?? ""))) {
      return parseDataUrlForXhr(String(url));
    }

    const { key, href } = __pyRunnerResolveAsset(url);
    if (/^data:/i.test(href)) {
      return parseDataUrlForXhr(href);
    }

    const asset =
      globalThis.__pyRunnerFetchAsset(key) ??
      globalThis.__pyRunnerFetchAsset(href);
    if (asset) {
      const contentType = asset.contentType ??
        (asset.kind === "binary" ? "application/octet-stream" : "text/plain; charset=utf-8");
      return {
        status: asset.status ?? 200,
        statusText: asset.statusText ?? "OK",
        url: href,
        binary: asset.kind === "binary",
        body: asset.data,
        headers: asset.headers ?? [["content-type", contentType]],
        contentType,
      };
    }

    if (/^https?:/i.test(href) && typeof globalThis.__pyRunnerNativeFetch === "function") {
      const response = globalThis.__pyRunnerNativeFetch(href, init);
      if (response) {
        return response;
      }
    }

    throw createXhrError("NetworkError", `XMLHttpRequest failed for ${href}`);
  };

  class XMLHttpRequestPolyfill {
    constructor() {
      this.readyState = XHR_UNSENT;
      this.response = null;
      this.responseText = "";
      this.responseType = "";
      this.responseURL = "";
      this.status = 0;
      this.statusText = "";
      this.timeout = 0;
      this.withCredentials = false;
      this.onreadystatechange = null;
      this.onload = null;
      this.onerror = null;
      this.onabort = null;
      this.onloadend = null;
      this._listeners = new Map();
      this._headers = new Map();
      this._responseHeaders = new Map();
      this._method = "GET";
      this._url = "";
      this._async = true;
      this._aborted = false;
    }

    static new() {
      return new XMLHttpRequestPolyfill();
    }

    open(method, url, async = true) {
      this._method = String(method || "GET").toUpperCase();
      this._url = String(url ?? "");
      this._async = async !== false;
      this._headers.clear();
      this._responseHeaders.clear();
      this._aborted = false;
      this.status = 0;
      this.statusText = "";
      this.response = null;
      this.responseText = "";
      this.responseURL = "";
      this._setReadyState(XHR_OPENED);
    }

    setRequestHeader(name, value) {
      if (this.readyState !== XHR_OPENED) {
        throw createXhrError("InvalidStateError", "XMLHttpRequest is not open");
      }
      const key = String(name);
      const existing = this._headers.get(key);
      this._headers.set(key, existing ? `${existing}, ${value}` : String(value));
    }

    send(body = null) {
      if (this.readyState !== XHR_OPENED) {
        throw createXhrError("InvalidStateError", "XMLHttpRequest is not open");
      }

      const init = {
        method: this._method,
        headers: Array.from(this._headers.entries()),
        body,
      };

      const perform = () => {
        if (this._aborted) {
          return;
        }
        try {
          const response = resolveXhrResponse(this._url, init);
          if (response && typeof response.then === "function") {
            response.then(
              (resolved) => this._complete(resolved),
              (error) => this._fail(error),
            );
          } else {
            this._complete(response);
          }
        } catch (error) {
          const xhrError =
            error && typeof error === "object" && error.name
              ? error
              : createXhrError(
                  "NetworkError",
                  error && typeof error === "object" && "message" in error
                    ? error.message
                    : String(error),
                );
          this._fail(xhrError);
          throw xhrError;
        }
      };

      if (this._async) {
        queueMicrotask(() => {
          try {
            perform();
          } catch (_err) {
            // Async XHR reports network errors through events, not a thrown send().
          }
        });
        return;
      }

      perform();
    }

    abort() {
      this._aborted = true;
      this.status = 0;
      this.statusText = "";
      this.response = null;
      this.responseText = "";
      this._setReadyState(XHR_DONE);
      this._dispatch("abort");
      this._dispatch("loadend");
    }

    getResponseHeader(name) {
      if (this.readyState < XHR_HEADERS_RECEIVED) {
        return null;
      }
      const normalized = String(name ?? "").toLowerCase();
      return this._responseHeaders.has(normalized)
        ? this._responseHeaders.get(normalized)
        : null;
    }

    getAllResponseHeaders() {
      if (this.readyState < XHR_HEADERS_RECEIVED) {
        return "";
      }
      let output = "";
      for (const [name, value] of this._responseHeaders.entries()) {
        output += `${name}: ${value}\r\n`;
      }
      return output;
    }

    overrideMimeType(_mimeType) {
      // MIME override does not affect the current in-memory response model.
    }

    addEventListener(type, listener) {
      if (typeof listener !== "function") {
        return;
      }
      const key = String(type);
      const listeners = this._listeners.get(key) ?? new Set();
      listeners.add(listener);
      this._listeners.set(key, listeners);
    }

    removeEventListener(type, listener) {
      this._listeners.get(String(type))?.delete(listener);
    }

    _complete(response) {
      this.status = Number(response?.status ?? 0);
      this.statusText = String(response?.statusText ?? "");
      this.responseURL = String(response?.url ?? this._url);
      this._responseHeaders = xhrHeadersFromResponse(response);
      this._setReadyState(XHR_HEADERS_RECEIVED);

      const bytes = xhrBytesFromBody(response?.body, response?.binary !== false);
      const text = new TextDecoder("utf-8").decode(bytes);
      this.responseText = text;
      if (this.responseType === "arraybuffer") {
        this.response = xhrCloneBuffer(bytes);
      } else if (this.responseType === "json") {
        this.response = text ? JSON.parse(text) : null;
      } else {
        this.response = text;
      }

      this._setReadyState(XHR_LOADING);
      this._setReadyState(XHR_DONE);
      this._dispatch("load");
      this._dispatch("loadend");
    }

    _fail(error) {
      this.status = 0;
      this.statusText = "";
      this.response = null;
      this.responseText = "";
      this._responseHeaders.clear();
      this._setReadyState(XHR_DONE);
      this._dispatch("error", error);
      this._dispatch("loadend", error);
    }

    _setReadyState(state) {
      this.readyState = state;
      this._dispatch("readystatechange");
    }

    _dispatch(type, error = null) {
      const event = {
        type,
        target: this,
        currentTarget: this,
        error,
      };
      const handler = this[`on${type}`];
      if (typeof handler === "function") {
        handler.call(this, event);
      }
      const listeners = this._listeners.get(type);
      if (listeners) {
        for (const listener of Array.from(listeners)) {
          listener.call(this, event);
        }
      }
    }
  }

  XMLHttpRequestPolyfill.UNSENT = XHR_UNSENT;
  XMLHttpRequestPolyfill.OPENED = XHR_OPENED;
  XMLHttpRequestPolyfill.HEADERS_RECEIVED = XHR_HEADERS_RECEIVED;
  XMLHttpRequestPolyfill.LOADING = XHR_LOADING;
  XMLHttpRequestPolyfill.DONE = XHR_DONE;
  XMLHttpRequestPolyfill.prototype.UNSENT = XHR_UNSENT;
  XMLHttpRequestPolyfill.prototype.OPENED = XHR_OPENED;
  XMLHttpRequestPolyfill.prototype.HEADERS_RECEIVED = XHR_HEADERS_RECEIVED;
  XMLHttpRequestPolyfill.prototype.LOADING = XHR_LOADING;
  XMLHttpRequestPolyfill.prototype.DONE = XHR_DONE;

  globalThis.XMLHttpRequest = XMLHttpRequestPolyfill;
}

if (typeof globalThis.importScripts !== "function") {
  globalThis.importScripts = function importScripts(...urls) {
    for (const spec of urls) {
      const { href } = __pyRunnerResolveAsset(spec);
      if (globalThis.console && typeof globalThis.console.log === "function") {
        globalThis.console.log(`[polyfill] importScripts '${spec}' (${href})`);
      }
      const code = __pyRunnerGetTextAsset(spec);
      __pyRunnerGlobalEval(`${code}\n//# sourceURL=${href}`);
    }
  };
}

if (typeof globalThis.queueMicrotask !== "function") {
  globalThis.queueMicrotask = (callback) => Promise.resolve().then(callback);
}

if (typeof WebAssembly.instantiateStreaming !== "function") {
  WebAssembly.instantiateStreaming = async function instantiateStreaming(source, imports = {}) {
    const response = await source;
    if (
      response &&
      typeof response.arrayBuffer === "function"
    ) {
      const buffer = await response.arrayBuffer();
      return WebAssembly.instantiate(buffer, imports);
    }
    if (
      response instanceof ArrayBuffer ||
      ArrayBuffer.isView(response)
    ) {
      return WebAssembly.instantiate(response, imports);
    }
    throw new TypeError("instantiateStreaming fallback expects a fetch Response or buffer-like object");
  };
}

if (typeof globalThis.navigator === "undefined") {
  globalThis.navigator = {
    hardwareConcurrency: 1,
    language: "en-US",
    languages: ["en-US"],
    platform: "Aardvark",
    userAgent: "Aardvark/0.1 (+https://aardvark.invalid)",
  };
}

if (typeof globalThis.TextEncoder === "undefined") {
  globalThis.TextEncoder = class TextEncoder {
    encode(input = "") {
      const str = String(input);
      const encoded = unescape(encodeURIComponent(str));
      const out = new Uint8Array(encoded.length);
      for (let i = 0; i < encoded.length; i += 1) {
        out[i] = encoded.charCodeAt(i);
      }
      return out;
    }

    encodeInto(source = "", destination = new Uint8Array()) {
      const bytes = this.encode(source);
      if (!(destination instanceof Uint8Array)) {
        throw new TypeError("encodeInto expects a Uint8Array destination");
      }
      const written = Math.min(bytes.length, destination.length);
      destination.set(bytes.subarray(0, written));
      return { read: source.length, written };
    }
  };
}

if (typeof globalThis.TextDecoder === "undefined") {
  globalThis.TextDecoder = class TextDecoder {
    decode(input = new Uint8Array()) {
      const view =
        input instanceof Uint8Array ? input : new Uint8Array(input ?? 0);
      let binary = "";
      for (let i = 0; i < view.length; i += 1) {
        binary += String.fromCharCode(view[i]);
      }
      return decodeURIComponent(escape(binary));
    }
  };
}

if (typeof globalThis.DOMException === "undefined") {
  globalThis.DOMException = class DOMException extends Error {
    constructor(message = "", name = "Error") {
      super(message);
      this.name = name;
    }
  };
}

if (typeof globalThis.AbortSignal === "undefined") {
  class AbortSignal {
    constructor() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
    }

    throwIfAborted() {
      if (this.aborted) {
        throw this.reason ?? new globalThis.DOMException("Aborted", "AbortError");
      }
    }
  }

  AbortSignal.abort = function abort(reason) {
    const signal = new AbortSignal();
    signal.aborted = true;
    signal.reason =
      reason ?? new globalThis.DOMException("Aborted", "AbortError");
    return signal;
  };

  AbortSignal.timeout = function timeout(_ms) {
    return AbortSignal.abort(
      new globalThis.DOMException("Timed out", "TimeoutError")
    );
  };

  globalThis.AbortSignal = AbortSignal;
}

if (typeof globalThis.AbortController === "undefined") {
  class AbortController {
    constructor() {
      this.signal = new globalThis.AbortSignal();
    }

    abort(reason) {
      if (!this.signal.aborted) {
        this.signal.aborted = true;
        this.signal.reason =
          reason ?? new globalThis.DOMException("Aborted", "AbortError");
        if (typeof this.signal.onabort === "function") {
          this.signal.onabort({ type: "abort" });
        }
      }
    }
  }

  globalThis.AbortController = AbortController;
}

if (typeof globalThis.structuredClone !== "function") {
  const cloneValue = (value, seen) => {
    if (value === null || typeof value !== "object") {
      return value;
    }
    if (seen.has(value)) {
      return seen.get(value);
    }
    if (value instanceof Date) {
      return new Date(value.getTime());
    }
    if (value instanceof RegExp) {
      return new RegExp(value);
    }
    if (value instanceof ArrayBuffer) {
      return value.slice(0);
    }
    if (ArrayBuffer.isView(value)) {
      return new value.constructor(value.buffer.slice(0), value.byteOffset, value.length);
    }
    if (value instanceof Map) {
      const result = new Map();
      seen.set(value, result);
      for (const [key, entry] of value.entries()) {
        result.set(cloneValue(key, seen), cloneValue(entry, seen));
      }
      return result;
    }
    if (value instanceof Set) {
      const result = new Set();
      seen.set(value, result);
      for (const entry of value.values()) {
        result.add(cloneValue(entry, seen));
      }
      return result;
    }
    const result = Array.isArray(value) ? [] : {};
    seen.set(value, result);
    for (const key of Object.keys(value)) {
      result[key] = cloneValue(value[key], seen);
    }
    return result;
  };

  globalThis.structuredClone = function structuredClone(value) {
    return cloneValue(value, new WeakMap());
  };
}

if (typeof globalThis.setTimeout !== "function") {
  let __pyRunnerTimerId = 1;
  const timeouts = new Map();
  const intervals = new Map();

  globalThis.setTimeout = function setTimeout(handler, timeout, ...args) {
    const id = __pyRunnerTimerId++;
    const callable =
      typeof handler === "function"
        ? () => handler(...args)
        : () => {
            __pyRunnerGlobalEval(String(handler));
          };
    const invoke = () => {
      if (!timeouts.has(id)) {
        return;
      }
      timeouts.delete(id);
      callable();
    };
    timeouts.set(id, invoke);
    queueMicrotask(invoke);
    return id;
  };

  globalThis.clearTimeout = function clearTimeout(id) {
    timeouts.delete(id);
  };

  globalThis.setInterval = function setInterval(handler, timeout, ...args) {
    const id = __pyRunnerTimerId++;
    const callable =
      typeof handler === "function"
        ? () => handler(...args)
        : () => {
            __pyRunnerGlobalEval(String(handler));
          };
    const tick = () => {
      if (!intervals.has(id)) {
        return;
      }
      callable();
      queueMicrotask(tick);
    };
    intervals.set(id, tick);
    queueMicrotask(tick);
    return id;
  };

  globalThis.clearInterval = function clearInterval(id) {
    intervals.delete(id);
  };
}

globalThis.__pyRunnerMountFiles = function __pyRunnerMountFiles(
  pyodide,
  files,
  rootDir = "/app"
) {
  if (!pyodide || !pyodide.FS) {
    throw new Error("pyodide FS is not available");
  }
  const FS = pyodide.FS;
  const normalize = (path) => {
    if (!path || path === ".") {
      return rootDir;
    }
    if (path.startsWith("/")) {
      return path;
    }
    if (rootDir.endsWith("/")) {
      return `${rootDir}${path}`;
    }
    return `${rootDir}/${path}`;
  };

  FS.mkdirTree(rootDir);
  for (const file of files) {
    const fullPath = normalize(file.path);
    const segments = fullPath.split("/").filter(Boolean);
    let current = fullPath.startsWith("/") ? "/" : "";
    for (let i = 0; i < segments.length - 1; i += 1) {
      current += (current === "/" ? "" : "/") + segments[i];
      try {
        FS.mkdirTree(current);
      } catch (err) {
        if (err?.code !== "EEXIST") {
          throw err;
        }
      }
    }
    FS.writeFile(fullPath, file.data, { canOwn: true });
  }
};
