//! Lightweight V8 runtime utilities.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crate::asset_store::{Asset, AssetStore};
use crate::bundle::Bundle;
use crate::error::{PyRunnerError, Result};
use bytes::Bytes;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use tracing::{debug, info, warn};
use url::Url;
use v8::{
    self, script_compiler, Array, Context, ContextScope, FixedArray, Function,
    FunctionCallbackArguments, Local, Module, ModuleRequest, Object, PinScope, Promise,
    PromiseState, ReturnValue, String as V8String, Uint8Array, Value,
};
use walkdir::WalkDir;
static V8_PLATFORM: OnceCell<v8::SharedRef<v8::Platform>> = OnceCell::new();
static PACKAGE_ROOT: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));

#[derive(Debug, Clone)]
struct HostPattern {
    kind: HostPatternKind,
    port: Option<u16>,
}

#[derive(Debug, Clone)]
enum HostPatternKind {
    Exact(String),
    WildcardSuffix(String),
}

#[derive(Debug, Clone)]
pub struct NetworkContactRecord {
    pub host: String,
    pub port: Option<u16>,
    pub https: bool,
}

#[derive(Debug, Clone)]
pub struct NetworkDeniedRecord {
    pub host: String,
    pub port: Option<u16>,
    pub reason: String,
    pub https_required: bool,
}

#[derive(Debug, Clone)]
struct NetworkPolicy {
    entries: Vec<HostPattern>,
    https_only: bool,
}

#[derive(Debug, Clone)]
pub struct FilesystemViolationRecord {
    pub path: Option<String>,
    pub message: String,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            https_only: true,
        }
    }
}

impl NetworkPolicy {
    fn new(allow: &[String], https_only: bool) -> Self {
        let entries = allow
            .iter()
            .filter_map(|value| HostPattern::from_pattern(value))
            .collect();
        Self {
            entries,
            https_only,
        }
    }

    fn evaluate(&self, host: &str, port: Option<u16>, is_https: bool) -> NetworkDecision {
        if self.entries.is_empty() {
            return NetworkDecision::Denied(NetworkDenyReason::NoAllowlist);
        }
        if self.https_only && !is_https {
            return NetworkDecision::Denied(NetworkDenyReason::SchemeNotAllowed);
        }
        let host_lc = host.to_ascii_lowercase();
        for pattern in &self.entries {
            if pattern.matches(&host_lc, port) {
                return NetworkDecision::Allowed;
            }
        }
        NetworkDecision::Denied(NetworkDenyReason::HostNotAllowed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkDecision {
    Allowed,
    Denied(NetworkDenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkDenyReason {
    NoAllowlist,
    SchemeNotAllowed,
    HostNotAllowed,
}

impl NetworkDenyReason {
    fn as_str(&self) -> &'static str {
        match self {
            NetworkDenyReason::NoAllowlist => "no-allowlist",
            NetworkDenyReason::SchemeNotAllowed => "scheme-not-allowed",
            NetworkDenyReason::HostNotAllowed => "host-not-allowed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemModeConfig {
    Read,
    ReadWrite,
}

impl HostPattern {
    fn from_pattern(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lowered = trimmed.to_ascii_lowercase();
        let (host_part, port) = split_host_and_port(&lowered);
        if host_part.is_empty() {
            return None;
        }
        if host_part.starts_with("*.") {
            let suffix = host_part.trim_start_matches("*.").to_owned();
            if suffix.is_empty() {
                return None;
            }
            Some(Self {
                kind: HostPatternKind::WildcardSuffix(suffix),
                port,
            })
        } else {
            Some(Self {
                kind: HostPatternKind::Exact(host_part),
                port,
            })
        }
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        let port_allowed = match (self.port, port) {
            (Some(expected), Some(actual)) => expected == actual,
            (Some(_), None) => false,
            _ => true,
        };
        if !port_allowed {
            return false;
        }
        match &self.kind {
            HostPatternKind::Exact(expected) => host == expected,
            HostPatternKind::WildcardSuffix(suffix) => host.ends_with(suffix),
        }
    }
}

fn split_host_and_port(value: &str) -> (String, Option<u16>) {
    if let Some(idx) = value.rfind(':') {
        let (host_part, port_part) = value.split_at(idx);
        let port_str = &port_part[1..];
        if !port_str.is_empty() && port_str.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(port) = port_str.parse::<u16>() {
                return (host_part.to_owned(), Some(port));
            }
        }
    }
    (value.to_owned(), None)
}

/// Options that influence how Pyodide is initialized inside the JS runtime.
pub struct PyodideLoadOptions<'a> {
    pub snapshot: Option<&'a [u8]>,
    pub make_snapshot: bool,
}

/// Overlay blob entry associated with a snapshot export/import.
pub struct OverlayBlob {
    pub key: String,
    pub digest: Option<String>,
    pub bytes: Vec<u8>,
}

/// Overlay export bundle containing metadata JSON and associated tar blobs.
pub struct OverlayExport {
    pub metadata: Vec<u8>,
    pub blobs: Vec<OverlayBlob>,
}

fn package_root_dir() -> Option<PathBuf> {
    PACKAGE_ROOT.read().as_ref().cloned()
}

pub(crate) fn set_package_root_override(path: Option<PathBuf>) {
    let normalized = path.map(normalize_package_root);
    {
        let mut guard = PACKAGE_ROOT.write();
        *guard = normalized.clone();
    }
    match normalized {
        Some(ref path) => tracing::debug!(
            target = "aardvark::packages",
            path = %path.display(),
            "package root override set"
        ),
        None => tracing::debug!(
            target = "aardvark::packages",
            "cleared package root override"
        ),
    }
}

fn normalize_package_root(path: PathBuf) -> PathBuf {
    if path.is_relative() {
        if let Ok(cwd) = env::current_dir() {
            return cwd.join(path);
        }
    }
    path
}

#[cfg(test)]
pub(crate) fn reset_package_root_for_tests() {
    *PACKAGE_ROOT.write() = None;
}

fn resolve_local_package_path(url: &str) -> Option<PathBuf> {
    let root = package_root_dir()?;
    let scheme_split = url.find("://").map(|idx| idx + 3).unwrap_or(0);
    let remainder = &url[scheme_split..];
    let path_part = remainder.split_once('/').map_or("", |(_, rest)| rest);
    if path_part.is_empty() {
        return None;
    }
    let trimmed = path_part
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let as_path = Path::new(trimmed);
    let mut attempts: Vec<PathBuf> = Vec::new();

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join(file_name));
    }

    if let Some(variant_relative) = strip_variant_prefix(as_path) {
        push_unique(&mut attempts, root.join(&variant_relative));
        if let Some(last) = variant_relative.file_name() {
            push_unique(&mut attempts, root.join(last));
        }
    }

    push_unique(&mut attempts, root.join(as_path));
    push_unique(&mut attempts, root.join("pyodide").join(as_path));

    if let Some(file_name) = as_path.file_name() {
        push_unique(&mut attempts, root.join("full").join(file_name));
    }

    for candidate in attempts {
        tracing::debug!(
            target = "aardvark::packages",
            path = %candidate.display(),
            exists = candidate.exists(),
            "checking local package candidate"
        );
        if candidate.exists() {
            tracing::debug!(
                target = "aardvark::packages",
                path = %candidate.display(),
                "resolved local package path"
            );
            return Some(candidate);
        }
    }
    if let Some(file_name) = as_path.file_name() {
        if let Some(found) = walk_for_file(&root, file_name) {
            tracing::debug!(
                target = "aardvark::packages",
                path = %found.display(),
                "resolved local package path via search"
            );
            return Some(found);
        }
    }
    None
}

fn is_pyodide_package_asset_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    let path = parsed.path().to_ascii_lowercase();
    let has_package_extension = matches!(
        Path::new(&path).extension().and_then(|ext| ext.to_str()),
        Some("whl" | "zip" | "tar")
    ) || path.ends_with(".tar.gz")
        || path.ends_with(".tar.bz2");
    if !has_package_extension {
        return false;
    }
    host.contains("pyodide")
        || path.contains("/pyodide/")
        || path.starts_with("/pyodide/")
        || path.starts_with("/pyodide@")
        || path.contains("/pyodide@")
}

fn strip_variant_prefix(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    match (components.next()?, components.next(), components.next()) {
        (
            Component::Normal(first),
            Some(Component::Normal(_version)),
            Some(Component::Normal(_variant)),
        ) if first == OsStr::new("pyodide") => {
            let remaining = components.as_path();
            if remaining.as_os_str().is_empty() {
                None
            } else {
                Some(remaining.to_path_buf())
            }
        }
        _ => None,
    }
}

fn push_unique(list: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !list.iter().any(|existing| existing == &candidate) {
        list.push(candidate);
    }
}

fn walk_for_file(root: &Path, needle: &OsStr) -> Option<PathBuf> {
    let walker = WalkDir::new(root).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.file_name() == Some(needle) {
            return Some(path.to_path_buf());
        }
    }
    None
}

fn guess_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("json") => "application/json",
        Some("js") => "application/javascript",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("py") => "text/x-python",
        Some("data") => "application/octet-stream",
        Some("whl") => "application/octet-stream",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("bz2") => "application/x-bzip2",
        Some("tar") => "application/x-tar",
        _ => "application/octet-stream",
    }
}

fn copy_typed_array(array: Local<Uint8Array>) -> Vec<u8> {
    let length = array.length();
    let mut data = vec![0u8; length];
    array.copy_contents(&mut data);
    data
}

thread_local! {
    static TLS_RUNTIME_CONTEXT: Cell<*const RuntimeContext> = const { Cell::new(std::ptr::null()) };
    static TLS_SCOPE: Cell<*mut std::ffi::c_void> = const { Cell::new(std::ptr::null_mut()) };
}

/// Ensure the global V8 platform is initialized exactly once.
fn init_v8() {
    V8_PLATFORM.get_or_init(|| {
        let platform = v8::new_default_platform(0, false);
        let shared = platform.make_shared();
        v8::V8::initialize_platform(shared.clone());
        v8::V8::initialize();
        shared
    });
}

/// A thin wrapper around an owned V8 isolate and context.
pub struct JsRuntime {
    isolate: v8::OwnedIsolate,
    context: v8::Global<v8::Context>,
    context_state: Rc<RuntimeContext>,
}

struct RuntimeContext {
    assets: AssetStore,
    modules: RefCell<HashMap<String, v8::Global<Module>>>,
    module_by_hash: RefCell<HashMap<i32, String>>,
    module_namespaces: RefCell<HashMap<String, v8::Global<Object>>>,
    pyodide_instance: RefCell<Option<v8::Global<Object>>>,
    stdout_log: RefCell<String>,
    stderr_log: RefCell<String>,
    network_policy: RwLock<NetworkPolicy>,
    network_contacts: RwLock<Vec<NetworkContactRecord>>,
    network_denied: RwLock<Vec<NetworkDeniedRecord>>,
    filesystem_violations: RwLock<Vec<FilesystemViolationRecord>>,
}

enum ConsoleStream {
    Stdout,
    Stderr,
}

impl JsRuntime {
    /// Creates a new isolate with an empty context and basic polyfills.
    pub fn new() -> Result<Self> {
        init_v8();
        let context_state = Rc::new(RuntimeContext::new());
        let create_params =
            v8::CreateParams::default().array_buffer_allocator(v8::new_default_allocator());
        let mut isolate = v8::Isolate::new(create_params);
        isolate.set_slot(context_state.clone());
        let global = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, v8::ContextOptions::default());
            v8::Global::new(scope, context)
        };

        let mut runtime = Self {
            isolate,
            context: global,
            context_state,
        };
        runtime.install_polyfills()?;
        Ok(runtime)
    }

    /// Reinitializes the isolate in place, keeping the outer runtime alive.
    pub fn reset(&mut self) -> Result<()> {
        // Drop the previous context state so any module caches or globals are released.
        let new_state = Rc::new(RuntimeContext::new());
        self.context_state = new_state.clone();
        self.isolate.set_slot(new_state);

        // Hint V8 to reclaim memory from the old context before installing a new one.
        self.isolate.low_memory_notification();

        let global = {
            v8::scope!(let scope, &mut self.isolate);
            let context = v8::Context::new(scope, v8::ContextOptions::default());
            v8::Global::new(scope, context)
        };
        self.context = global;
        self.install_polyfills()
    }

    /// Configures the network allowlist for subsequent native fetches.
    pub fn set_network_policy(&self, allow: &[String], https_only: bool) {
        self.context_state.set_network_policy(allow, https_only);
    }

    /// Clears any recorded network contacts before a new invocation begins.
    pub fn clear_network_contacts(&self) {
        self.context_state.clear_network_contacts();
    }

    /// Consumes and returns the recorded network contacts from the last invocation.
    pub fn drain_network_contacts(&self) -> Vec<NetworkContactRecord> {
        self.context_state.take_network_contacts()
    }

    /// Clears any recorded denied network attempts before a new invocation begins.
    pub fn clear_network_denied(&self) {
        self.context_state.clear_network_denied();
    }

    /// Consumes and returns network attempts that were blocked by policy.
    pub fn drain_network_denied(&self) -> Vec<NetworkDeniedRecord> {
        self.context_state.take_network_denied()
    }

    /// Clears filesystem violation events.
    pub fn clear_filesystem_events(&self) {
        self.context_state.clear_filesystem_violations();
    }

    /// Consumes filesystem violations captured during the invocation.
    pub fn drain_filesystem_violations(&self) -> Vec<FilesystemViolationRecord> {
        self.context_state.take_filesystem_violations()
    }

    /// Applies filesystem mode and quota before executing user code.
    pub fn set_filesystem_policy(
        &mut self,
        mode: FilesystemModeConfig,
        quota_bytes: Option<u64>,
    ) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkFilesystemSetPolicy").ok_or_else(|| {
                PyRunnerError::Execution("filesystem policy hook unavailable".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__aardvarkFilesystemSetPolicy missing".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__aardvarkFilesystemSetPolicy is not a function".into())
            })?;
            let policy = v8::Object::new(scope);
            let mode_key = v8::String::new(scope, "mode").unwrap();
            let mode_value = v8::String::new(
                scope,
                match mode {
                    FilesystemModeConfig::Read => "read",
                    FilesystemModeConfig::ReadWrite => "readWrite",
                },
            )
            .unwrap();
            let _ = policy.set(scope, mode_key.into(), mode_value.into());
            if let Some(quota) = quota_bytes {
                let quota_key = v8::String::new(scope, "quotaBytes").unwrap();
                let quota_value = v8::Number::new(scope, quota as f64);
                let _ = policy.set(scope, quota_key.into(), quota_value.into());
            }
            func.call(scope, global.into(), &[policy.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("filesystem policy update failed".into())
                })?;
            Ok(())
        })
    }

    /// Executes a JavaScript module export and returns the normalized output.
    pub fn run_js_entrypoint(&mut self, entrypoint: &str) -> Result<ExecutionOutput> {
        let entry_trimmed = entrypoint.trim();
        let (module_part, export_part) = entry_trimmed
            .split_once(':')
            .map(|(module, export)| (module.trim(), export.trim()))
            .unwrap_or((entry_trimmed, "default"));

        if module_part.is_empty() {
            return Err(PyRunnerError::Execution(
                "entrypoint must include a module name".into(),
            ));
        }

        let export_name = if export_part.is_empty() {
            "default"
        } else {
            export_part
        };

        let mut specifier = module_part.replace('.', "/");
        if !specifier.ends_with(".js") {
            specifier.push_str(".js");
        }
        let specifier = normalize_specifier(&specifier);
        self.ensure_module(&specifier)?;

        enum InvocationResult {
            Immediate(v8::Global<v8::Value>),
            Promise(v8::Global<Promise>),
            Exception {
                typ: String,
                value: String,
                stack: Option<String>,
            },
        }

        let ctx_state = self.context_state.clone();
        ctx_state.clear_console();

        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let reset_key = v8::String::new(scope, "__aardvarkResetSharedBuffers").unwrap();
            if let Some(reset_value) = global.get(scope, reset_key.into()) {
                if let Ok(reset_fn) = Local::<Function>::try_from(reset_value) {
                    let _ = reset_fn.call(scope, global.into(), &[]);
                }
            }
            Ok(())
        })?;

        let mut invocation: Option<InvocationResult> = None;

        self.with_context(|scope, _| {
            v8::tc_scope!(let try_catch, scope);
            let Some(namespace) = ctx_state.module_namespace(try_catch, &specifier) else {
                return Err(PyRunnerError::Execution(format!(
                    "module '{specifier}' not loaded"
                )));
            };

            let export_key = v8::String::new(try_catch, export_name)
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate export name".into()))?;
            let export_value = namespace.get(try_catch, export_key.into()).ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "module '{specifier}' missing export '{export_name}'"
                ))
            })?;

            let Ok(function) = v8::Local::<Function>::try_from(export_value) else {
                return Err(PyRunnerError::Execution(format!(
                    "export '{export_name}' is not callable"
                )));
            };

            let global = try_catch.get_current_context().global(try_catch);
            let wrap_key = v8::String::new(try_catch, "__aardvarkWrapRawctxFunction").unwrap();
            let mut callable = function;
            if let Some(wrap_value) = global.get(try_catch, wrap_key.into()) {
                if let Ok(wrap_fn) = v8::Local::<Function>::try_from(wrap_value) {
                    let module_name = v8::String::new(try_catch, module_part).ok_or_else(|| {
                        PyRunnerError::Execution("failed to allocate module name".into())
                    })?;
                    let export_js = v8::String::new(try_catch, export_name).ok_or_else(|| {
                        PyRunnerError::Execution("failed to allocate export name".into())
                    })?;
                    let wrapped = wrap_fn.call(
                        try_catch,
                        global.into(),
                        &[callable.into(), module_name.into(), export_js.into()],
                    );
                    if let Some(value) = wrapped {
                        if let Ok(func) = v8::Local::<Function>::try_from(value) {
                            callable = func;
                        }
                    }
                }
            }

            let call_result = callable.call(try_catch, namespace.into(), &[]);
            let Some(value) = call_result else {
                let exception = try_catch.exception();
                let mut typ = "JavaScriptError".to_string();
                let mut message = "javascript execution failed".to_string();
                let mut stack: Option<String> = None;

                if let Some(object) = exception.and_then(|value| value.to_object(try_catch)) {
                    if let Some(name_value) = object.get(
                        try_catch,
                        v8::String::new(try_catch, "name")
                            .ok_or_else(|| {
                                PyRunnerError::Execution(
                                    "failed to allocate error name string".into(),
                                )
                            })?
                            .into(),
                    ) {
                        if let Some(name_str) = name_value.to_string(try_catch) {
                            typ = name_str.to_rust_string_lossy(try_catch);
                        }
                    }
                    if let Some(message_value) = object.get(
                        try_catch,
                        v8::String::new(try_catch, "message")
                            .ok_or_else(|| {
                                PyRunnerError::Execution(
                                    "failed to allocate error message string".into(),
                                )
                            })?
                            .into(),
                    ) {
                        if let Some(msg_str) = message_value.to_string(try_catch) {
                            message = msg_str.to_rust_string_lossy(try_catch);
                        }
                    }
                } else if let Some(value) = exception {
                    if let Some(msg) = value.to_string(try_catch) {
                        message = msg.to_rust_string_lossy(try_catch);
                    }
                }

                if let Some(stack_value) = try_catch.stack_trace() {
                    if let Some(stack_str) = stack_value.to_string(try_catch) {
                        stack = Some(stack_str.to_rust_string_lossy(try_catch));
                    }
                }

                invocation = Some(InvocationResult::Exception {
                    typ,
                    value: message,
                    stack,
                });
                return Ok(());
            };

            if let Ok(promise) = v8::Local::<Promise>::try_from(value) {
                invocation = Some(InvocationResult::Promise(v8::Global::new(
                    try_catch, promise,
                )));
            } else {
                invocation = Some(InvocationResult::Immediate(v8::Global::new(
                    try_catch, value,
                )));
            }
            Ok(())
        })?;

        let invocation = invocation.unwrap_or_else(|| InvocationResult::Exception {
            typ: "JavaScriptError".to_string(),
            value: "javascript execution failed".to_string(),
            stack: None,
        });

        let mut execution = ExecutionOutput {
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            exception_type: None,
            exception_value: None,
            traceback: None,
            json: None,
            shared_buffers: Vec::new(),
        };

        match invocation {
            InvocationResult::Exception { typ, value, stack } => {
                execution.exception_type = Some(typ);
                execution.exception_value = Some(value);
                execution.traceback = stack;
            }
            InvocationResult::Immediate(value_global) => {
                populate_execution_output(self, value_global, &mut execution)?;
            }
            InvocationResult::Promise(promise_global) => {
                enum PromiseOutcome {
                    Pending,
                    Fulfilled(v8::Global<v8::Value>),
                    Rejected {
                        typ: String,
                        value: String,
                        stack: Option<String>,
                    },
                }

                let mut resolved_value: Option<v8::Global<v8::Value>> = None;

                loop {
                    let outcome = self.with_context(|scope, _| -> Result<PromiseOutcome> {
                        let promise = v8::Local::new(scope, &promise_global);
                        match promise.state() {
                            PromiseState::Pending => Ok(PromiseOutcome::Pending),
                            PromiseState::Fulfilled => {
                                let value = promise.result(scope);
                                Ok(PromiseOutcome::Fulfilled(v8::Global::new(scope, value)))
                            }
                            PromiseState::Rejected => {
                                let reason = promise.result(scope);
                                let mut typ = "JavaScriptError".to_string();
                                let mut message = reason
                                    .to_string(scope)
                                    .map(|s| s.to_rust_string_lossy(scope))
                                    .unwrap_or_else(|| "javascript promise rejected".into());
                                let mut stack: Option<String> = None;
                                if let Some(object) = reason.to_object(scope) {
                                    if let Some(name_value) = object.get(
                                        scope,
                                        v8::String::new(scope, "name")
                                            .ok_or_else(|| {
                                                PyRunnerError::Execution(
                                                    "failed to allocate error name string".into(),
                                                )
                                            })?
                                            .into(),
                                    ) {
                                        if let Some(name_str) = name_value.to_string(scope) {
                                            typ = name_str.to_rust_string_lossy(scope);
                                        }
                                    }
                                    if let Some(message_value) = object.get(
                                        scope,
                                        v8::String::new(scope, "message")
                                            .ok_or_else(|| {
                                                PyRunnerError::Execution(
                                                    "failed to allocate error message string"
                                                        .into(),
                                                )
                                            })?
                                            .into(),
                                    ) {
                                        if let Some(msg_str) = message_value.to_string(scope) {
                                            message = msg_str.to_rust_string_lossy(scope);
                                        }
                                    }
                                    if let Some(stack_value) = object.get(
                                        scope,
                                        v8::String::new(scope, "stack")
                                            .ok_or_else(|| {
                                                PyRunnerError::Execution(
                                                    "failed to allocate error stack string".into(),
                                                )
                                            })?
                                            .into(),
                                    ) {
                                        if let Some(stack_str) = stack_value.to_string(scope) {
                                            stack = Some(stack_str.to_rust_string_lossy(scope));
                                        }
                                    }
                                }
                                Ok(PromiseOutcome::Rejected {
                                    typ,
                                    value: message,
                                    stack,
                                })
                            }
                        }
                    })?;

                    match outcome {
                        PromiseOutcome::Pending => {
                            self.isolate.perform_microtask_checkpoint();
                        }
                        PromiseOutcome::Fulfilled(value) => {
                            resolved_value = Some(value);
                            break;
                        }
                        PromiseOutcome::Rejected { typ, value, stack } => {
                            execution.exception_type = Some(typ);
                            execution.exception_value = Some(value);
                            execution.traceback = stack;
                            break;
                        }
                    }
                }

                if let Some(value_global) = resolved_value {
                    populate_execution_output(self, value_global, &mut execution)?;
                }
            }
        }

        let shared_buffers = self.with_context(|scope, _| -> Result<Vec<SharedBuffer>> {
            let global = scope.get_current_context().global(scope);
            let buffers = collect_shared_buffers(scope, global)?;
            if !buffers.is_empty() {
                let release_ids: Vec<String> =
                    buffers.iter().map(|buffer| buffer.id.clone()).collect();
                release_shared_buffers(scope, global, &release_ids)?;
            }
            Ok(buffers)
        })?;
        execution.shared_buffers = shared_buffers;

        execution.stdout = ctx_state.take_stdout();
        execution.stderr = ctx_state.take_stderr();

        Ok(execution)
    }

    /// Clears published shared buffers between invocations.
    pub fn reset_shared_buffers(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkResetSharedBuffers").ok_or_else(|| {
                PyRunnerError::Execution("shared buffer reset hook unavailable".into())
            })?;
            if let Some(value) = global.get(scope, key.into()) {
                if let Ok(func) = Local::<Function>::try_from(value) {
                    func.call(scope, global.into(), &[]).ok_or_else(|| {
                        PyRunnerError::Execution("shared buffer reset failed".into())
                    })?;
                }
            }
            Ok(())
        })
    }

    /// Resets the session scratch filesystem after an invocation completes.
    pub fn reset_filesystem(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkFilesystemReset").ok_or_else(|| {
                PyRunnerError::Execution("filesystem reset hook unavailable".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__aardvarkFilesystemReset missing".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__aardvarkFilesystemReset is not a function".into())
            })?;
            func.call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("filesystem reset failed".into()))?;
            Ok(())
        })
    }

    /// Returns the current byte usage of the session scratch filesystem.
    pub fn filesystem_usage_bytes(&mut self) -> Result<u64> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkFilesystemGetUsage").ok_or_else(|| {
                PyRunnerError::Execution("filesystem usage hook unavailable".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__aardvarkFilesystemGetUsage missing".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__aardvarkFilesystemGetUsage is not a function".into())
            })?;
            let usage_value = func
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("filesystem usage query failed".into()))?;
            let number = usage_value
                .to_number(scope)
                .ok_or_else(|| PyRunnerError::Execution("filesystem usage not a number".into()))?;
            let value = number.value();
            Ok(if value <= 0.0 { 0 } else { value as u64 })
        })
    }

    /// Applies the active host capabilities for native APIs exposed to guest code.
    pub fn set_host_capabilities(&mut self, capabilities: &[String]) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkSetHostCapabilities").ok_or_else(|| {
                PyRunnerError::Execution("host capability hook unavailable".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__aardvarkSetHostCapabilities missing".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__aardvarkSetHostCapabilities is not a function".into())
            })?;
            let array = v8::Array::new(scope, capabilities.len() as i32);
            for (index, capability) in capabilities.iter().enumerate() {
                if let Some(value) = v8::String::new(scope, capability) {
                    array.set_index(scope, index as u32, value.into());
                }
            }
            func.call(scope, global.into(), &[array.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("applying host capabilities failed".into())
                })?;
            Ok(())
        })
    }

    pub(crate) fn isolate_handle(&mut self) -> v8::IsolateHandle {
        self.isolate.thread_safe_handle()
    }

    pub(crate) fn cancel_terminate_execution(&mut self) {
        let _ = self.isolate.cancel_terminate_execution();
    }

    pub(crate) fn heap_used_bytes(&mut self) -> usize {
        let stats = self.isolate.get_heap_statistics();
        stats.used_heap_size()
    }

    fn install_polyfills(&mut self) -> Result<()> {
        self.with_context(|scope, context| {
            let global = context.global(scope);
            let source = include_str!("js/polyfills.js");
            let script_name = "polyfills.js";
            exec_script(scope, script_name, source)
                .map_err(|msg| PyRunnerError::Init(format!("polyfill error: {msg}")))?;
            // Attach hooks placeholder for future assets.
            let fetch_name = v8::String::new(scope, "__pyRunnerFetchAsset")
                .ok_or_else(|| PyRunnerError::Init("failed to allocate v8 string".into()))?;
            let template = v8::FunctionTemplate::new(scope, asset_fetch_callback);
            let function = template
                .get_function(scope)
                .ok_or_else(|| PyRunnerError::Init("failed to create asset hook".into()))?;
            let _ = global.set(scope, fetch_name.into(), function.into());

            let log_name = v8::String::new(scope, "__pyRunnerNativeLog")
                .ok_or_else(|| PyRunnerError::Init("failed to allocate log string".into()))?;
            let log_template = v8::FunctionTemplate::new(scope, native_log_callback);
            let log_function = log_template
                .get_function(scope)
                .ok_or_else(|| PyRunnerError::Init("failed to create log hook".into()))?;
            let _ = global.set(scope, log_name.into(), log_function.into());

            let native_fetch_name =
                v8::String::new(scope, "__pyRunnerNativeFetch").ok_or_else(|| {
                    PyRunnerError::Init("failed to allocate native fetch string".into())
                })?;
            let native_fetch_template = v8::FunctionTemplate::new(scope, native_fetch_callback);
            let native_fetch_function = native_fetch_template
                .get_function(scope)
                .ok_or_else(|| PyRunnerError::Init("failed to create native fetch hook".into()))?;
            let _ = global.set(
                scope,
                native_fetch_name.into(),
                native_fetch_function.into(),
            );

            let record_name =
                v8::String::new(scope, "__aardvarkRecordBufferEvent").ok_or_else(|| {
                    PyRunnerError::Init("failed to allocate buffer event string".into())
                })?;
            let record_template = v8::FunctionTemplate::new(scope, record_buffer_event_callback);
            let record_function = record_template
                .get_function(scope)
                .ok_or_else(|| PyRunnerError::Init("failed to create buffer event hook".into()))?;
            let _ = global.set(scope, record_name.into(), record_function.into());

            let fs_violation_name = v8::String::new(scope, "__aardvarkFilesystemRecordViolation")
                .ok_or_else(|| {
                PyRunnerError::Init("failed to allocate fs violation string".into())
            })?;
            let fs_violation_template =
                v8::FunctionTemplate::new(scope, filesystem_violation_callback);
            let fs_violation_function =
                fs_violation_template.get_function(scope).ok_or_else(|| {
                    PyRunnerError::Init("failed to create filesystem violation hook".into())
                })?;
            let _ = global.set(
                scope,
                fs_violation_name.into(),
                fs_violation_function.into(),
            );
            Ok(())
        })
    }

    /// Execute a script within the isolate.
    pub fn execute_script(&mut self, name: &str, source: &str) -> Result<()> {
        self.with_context(|scope, _context| {
            exec_script(scope, name, source)
                .map_err(|msg| PyRunnerError::Execution(format!("javascript error: {msg}")))
        })
    }

    /// Utility to run closures with a borrowed handle scope inside the runtime context.
    pub fn with_context<F, R>(&mut self, f: F) -> Result<R>
    where
        F: for<'a> FnOnce(&mut PinScope<'a, '_>, Local<'a, v8::Context>) -> Result<R>,
    {
        let context_global = self.context.clone();
        v8::scope!(let isolate_scope, &mut self.isolate);
        let context = v8::Local::new(isolate_scope, context_global);
        let mut context_scope = ContextScope::new(isolate_scope, context);
        f(&mut context_scope, context)
    }

    /// Registers a text asset in the runtime asset store.
    pub fn insert_text_asset<S>(&self, name: &str, contents: S)
    where
        S: Into<Arc<str>>,
    {
        self.context_state.assets.insert_text(name, contents.into());
    }

    /// Registers a binary asset in the runtime asset store.
    pub fn insert_binary_asset(&self, name: &str, bytes: &'static [u8]) {
        self.context_state
            .assets
            .insert_bytes(name, Arc::<[u8]>::from(bytes));
    }

    /// Registers a binary asset backed by an owned buffer.
    pub fn insert_binary_asset_owned(&self, name: &str, bytes: Vec<u8>) {
        self.context_state
            .assets
            .insert_bytes(name, Arc::<[u8]>::from(bytes.into_boxed_slice()));
    }

    /// Loads the Pyodide runtime by calling the embedded loader module.
    pub fn load_pyodide(&mut self, options: PyodideLoadOptions<'_>) -> Result<()> {
        if self.context_state.pyodide_instance.borrow().is_some() {
            return Ok(());
        }
        let ctx_state = self.context_state.clone();
        self.ensure_module("pyodide.mjs")?;
        self.ensure_module("pyodide_bootstrap.js")?;
        let mut promise_handle: Option<v8::Global<Promise>> = None;
        self.with_context(|scope, _| {
            let bootstrap = ctx_state
                .module_namespace(scope, "pyodide_bootstrap.js")
                .ok_or_else(|| {
                    PyRunnerError::Execution("pyodide_bootstrap.js namespace unavailable".into())
                })?;
            let load_key = v8::String::new(scope, "loadPyRunnerPyodide").unwrap();
            let load_value = bootstrap.get(scope, load_key.into());
            let load_fn = load_value
                .and_then(|value| Local::<Function>::try_from(value).ok())
                .ok_or_else(|| {
                    PyRunnerError::Execution(
                        "pyodide_bootstrap.js does not export loadPyRunnerPyodide".into(),
                    )
                })?;
            let js_options = v8::Object::new(scope);
            let index_key = v8::String::new(scope, "indexURL").unwrap();
            let index_value = v8::String::new(scope, ".").unwrap();
            let _ = js_options.set(scope, index_key.into(), index_value.into());
            if let Some(snapshot) = options.snapshot {
                let snapshot_key = v8::String::new(scope, "snapshot").unwrap();
                let backing = v8::ArrayBuffer::new_backing_store_from_vec(snapshot.to_vec());
                let shared = backing.make_shared();
                let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
                let length = array_buffer.byte_length();
                let typed = Uint8Array::new(scope, array_buffer, 0, length).unwrap();
                let _ = js_options.set(scope, snapshot_key.into(), typed.into());
            }
            if options.make_snapshot {
                let make_key = v8::String::new(scope, "makeSnapshot").unwrap();
                let make_value = v8::Boolean::new(scope, true);
                let _ = js_options.set(scope, make_key.into(), make_value.into());
            }
            let value = load_fn
                .call(scope, bootstrap.into(), &[js_options.into()])
                .ok_or_else(|| PyRunnerError::Execution("loadPyodide invocation failed".into()))?;
            let promise = v8::Local::<Promise>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("loadPyodide did not return a Promise".into())
            })?;
            promise_handle = Some(v8::Global::new(scope, promise));
            Ok(())
        })?;

        let promise_global = promise_handle
            .ok_or_else(|| PyRunnerError::Execution("missing loadPyodide promise handle".into()))?;

        loop {
            let done = self.with_context(|scope, _| -> Result<Option<()>> {
                let promise = v8::Local::new(scope, &promise_global);
                match promise.state() {
                    PromiseState::Pending => Ok(None),
                    PromiseState::Fulfilled => {
                        let result = promise.result(scope);
                        let obj = v8::Local::<Object>::try_from(result).map_err(|_| {
                            PyRunnerError::Execution(
                                "loadPyodide fulfilled with non-object result".into(),
                            )
                        })?;
                        ctx_state
                            .pyodide_instance
                            .replace(Some(v8::Global::new(scope, obj)));
                        Ok(Some(()))
                    }
                    PromiseState::Rejected => {
                        let reason = promise.result(scope);
                        let message = reason
                            .to_string(scope)
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_else(|| "unknown rejection".to_string());
                        let detailed = reason
                            .to_object(scope)
                            .and_then(|obj| {
                                let stack_key = v8::String::new(scope, "stack")?;
                                obj.get(scope, stack_key.into())
                            })
                            .and_then(|value| value.to_string(scope))
                            .map(|s| s.to_rust_string_lossy(scope));
                        let message = detailed
                            .map(|stack| format!("{message}\n{stack}"))
                            .unwrap_or(message);
                        Err(PyRunnerError::Execution(format!(
                            "loadPyodide rejected: {message}"
                        )))
                    }
                }
            })?;
            if done.is_some() {
                break;
            }
            self.isolate.perform_microtask_checkpoint();
        }

        self.prepare_dynlibs()?;
        Ok(())
    }

    /// Invokes the JS helper to load one or more Pyodide packages via the package manager.
    pub fn load_packages(&mut self, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        let mut promise_handle: Option<v8::Global<Promise>> = None;
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerLoadPackages").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate package loader key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerLoadPackages is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerLoadPackages is not a function".into())
            })?;

            let array = v8::Array::new(scope, packages.len() as i32);
            for (index, name) in packages.iter().enumerate() {
                let value = v8::String::new(scope, name).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate package name".into())
                })?;
                array.set_index(scope, index as u32, value.into());
            }

            let promise_value = func
                .call(scope, global.into(), &[array.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("package loader invocation failed".into())
                })?;
            let promise = v8::Local::<Promise>::try_from(promise_value).map_err(|_| {
                PyRunnerError::Execution("package loader did not return a Promise".into())
            })?;
            promise_handle = Some(v8::Global::new(scope, promise));
            Ok(())
        })?;

        let promise_global = promise_handle.ok_or_else(|| {
            PyRunnerError::Execution("missing package loader promise handle".into())
        })?;

        loop {
            let done = self.with_context(|scope, _| -> Result<Option<()>> {
                let promise = v8::Local::new(scope, &promise_global);
                match promise.state() {
                    PromiseState::Pending => Ok(None),
                    PromiseState::Fulfilled => Ok(Some(())),
                    PromiseState::Rejected => {
                        let reason = promise.result(scope);
                        let message = reason
                            .to_string(scope)
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_else(|| "unknown rejection".to_string());
                        let detailed = reason
                            .to_object(scope)
                            .and_then(|obj| {
                                let stack_key = v8::String::new(scope, "stack")?;
                                obj.get(scope, stack_key.into())
                            })
                            .and_then(|value| value.to_string(scope))
                            .map(|s| s.to_rust_string_lossy(scope));
                        let message = detailed
                            .map(|stack| format!("{message}\n{stack}"))
                            .unwrap_or(message);
                        Err(PyRunnerError::Execution(format!(
                            "loadPackages rejected: {message}"
                        )))
                    }
                }
            })?;
            if done.is_some() {
                break;
            }
            self.isolate.perform_microtask_checkpoint();
        }

        Ok(())
    }

    /// Captures a Pyodide memory snapshot and returns the raw bytes.
    pub fn collect_snapshot(&mut self) -> Result<Vec<u8>> {
        let mut snapshot: Option<Vec<u8>> = None;
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerMakeSnapshot").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate snapshot key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerMakeSnapshot is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerMakeSnapshot is not a function".into())
            })?;
            let result = func
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("snapshot invocation failed".into()))?;
            let array = Local::<Uint8Array>::try_from(result).map_err(|_| {
                PyRunnerError::Execution(
                    "__pyRunnerMakeSnapshot did not return a Uint8Array".into(),
                )
            })?;
            snapshot = Some(copy_typed_array(array));
            Ok(())
        })?;

        snapshot.ok_or_else(|| PyRunnerError::Execution("snapshot helper returned no data".into()))
    }

    /// Exports the overlay metadata (site-packages + /usr/lib) and tar payload.
    pub fn export_overlay(&mut self) -> Result<OverlayExport> {
        let mut metadata: Option<Vec<u8>> = None;
        let mut blobs: Vec<OverlayBlob> = Vec::new();
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key_meta = v8::String::new(scope, "__pyRunnerExportOverlay").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate overlay export key".into())
            })?;
            let value_meta = global.get(scope, key_meta.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerExportOverlay is not defined".into())
            })?;
            let func_meta = Local::<Function>::try_from(value_meta).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerExportOverlay is not a function".into())
            })?;
            let meta_result = func_meta
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("overlay export failed".into()))?;
            if meta_result.is_null_or_undefined() {
                metadata = Some(Vec::new());
            } else {
                let array = Local::<Uint8Array>::try_from(meta_result).map_err(|_| {
                    PyRunnerError::Execution(
                        "__pyRunnerExportOverlay did not return a Uint8Array".into(),
                    )
                })?;
                metadata = Some(copy_typed_array(array));
            }

            let key_blobs =
                v8::String::new(scope, "__pyRunnerExportOverlayBlobs").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate overlay blob export key".into())
                })?;
            let value_blobs = global.get(scope, key_blobs.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerExportOverlayBlobs is not defined".into())
            })?;
            let func_blobs = Local::<Function>::try_from(value_blobs).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerExportOverlayBlobs is not a function".into())
            })?;
            let blobs_result = func_blobs
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("overlay blob export failed".into()))?;
            if !blobs_result.is_null_or_undefined() {
                let array = Local::<v8::Array>::try_from(blobs_result).map_err(|_| {
                    PyRunnerError::Execution(
                        "__pyRunnerExportOverlayBlobs did not return an Array".into(),
                    )
                })?;
                let key_prop = v8::String::new(scope, "key").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob key".into())
                })?;
                let digest_prop = v8::String::new(scope, "digest").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob digest".into())
                })?;
                let data_prop = v8::String::new(scope, "data").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob data".into())
                })?;
                let length = array.length();
                for index in 0..length {
                    let value = array
                        .get_index(scope, index)
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    if value.is_null_or_undefined() {
                        continue;
                    }
                    let object = Local::<Object>::try_from(value).map_err(|_| {
                        PyRunnerError::Execution("overlay blob entry is not an object".into())
                    })?;
                    let key_value = object
                        .get(scope, key_prop.into())
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    let key = if key_value.is_null_or_undefined() {
                        String::new()
                    } else {
                        key_value
                            .to_string(scope)
                            .ok_or_else(|| {
                                PyRunnerError::Execution(
                                    "failed to convert overlay blob key to string".into(),
                                )
                            })?
                            .to_rust_string_lossy(scope)
                    };
                    let digest_value = object
                        .get(scope, digest_prop.into())
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    let digest = if digest_value.is_null_or_undefined() {
                        None
                    } else {
                        Some(
                            digest_value
                                .to_string(scope)
                                .ok_or_else(|| {
                                    PyRunnerError::Execution(
                                        "failed to convert overlay blob digest to string".into(),
                                    )
                                })?
                                .to_rust_string_lossy(scope),
                        )
                    };
                    let data_value = object.get(scope, data_prop.into()).ok_or_else(|| {
                        PyRunnerError::Execution(
                            "overlay blob entry missing 'data' property".into(),
                        )
                    })?;
                    let data_array = Local::<Uint8Array>::try_from(data_value).map_err(|_| {
                        PyRunnerError::Execution("overlay blob 'data' is not a Uint8Array".into())
                    })?;
                    let bytes = copy_typed_array(data_array);
                    blobs.push(OverlayBlob { key, digest, bytes });
                }
            }
            Ok(())
        })?;

        Ok(OverlayExport {
            metadata: metadata.ok_or_else(|| {
                PyRunnerError::Execution("overlay export returned no data".into())
            })?,
            blobs,
        })
    }

    /// Imports overlay metadata and refreshes the dynamic library bindings.
    pub fn import_overlay(&mut self, metadata: &[u8], blobs: &[OverlayBlob]) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerImportOverlay").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate overlay import key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerImportOverlay is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerImportOverlay is not a function".into())
            })?;
            let meta_backing = v8::ArrayBuffer::new_backing_store_from_vec(metadata.to_vec());
            let meta_shared = meta_backing.make_shared();
            let meta_buffer = v8::ArrayBuffer::with_backing_store(scope, &meta_shared);
            let meta_typed =
                Uint8Array::new(scope, meta_buffer, 0, metadata.len()).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate overlay metadata buffer".into())
                })?;

            let blob_array = v8::Array::new(scope, blobs.len() as i32);
            let key_prop = v8::String::new(scope, "key")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob key".into()))?;
            let digest_prop = v8::String::new(scope, "digest")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob digest".into()))?;
            let data_prop = v8::String::new(scope, "data")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob data".into()))?;
            for (index, blob) in blobs.iter().enumerate() {
                let object = v8::Object::new(scope);
                let key_string = v8::String::new(scope, blob.key.as_str())
                    .ok_or_else(|| PyRunnerError::Execution("failed to convert blob key".into()))?;
                object.set(scope, key_prop.into(), key_string.into());
                if let Some(digest) = &blob.digest {
                    let digest_string =
                        v8::String::new(scope, digest.as_str()).ok_or_else(|| {
                            PyRunnerError::Execution("failed to convert blob digest".into())
                        })?;
                    object.set(scope, digest_prop.into(), digest_string.into());
                } else {
                    let null_value = v8::null(scope);
                    object.set(scope, digest_prop.into(), null_value.into());
                }
                let data_backing = v8::ArrayBuffer::new_backing_store_from_vec(blob.bytes.clone());
                let data_shared = data_backing.make_shared();
                let data_buffer = v8::ArrayBuffer::with_backing_store(scope, &data_shared);
                let data_array = Uint8Array::new(scope, data_buffer, 0, blob.bytes.len())
                    .ok_or_else(|| {
                        PyRunnerError::Execution(
                            "failed to allocate overlay blob data buffer".into(),
                        )
                    })?;
                object.set(scope, data_prop.into(), data_array.into());
                blob_array.set_index(scope, index as u32, object.into());
            }

            let _ = func.call(
                scope,
                global.into(),
                &[meta_typed.into(), blob_array.into()],
            );
            Ok(())
        })?;
        self.prepare_dynlibs()
    }

    /// Refreshes dynamic library bindings after package or snapshot operations.
    pub fn prepare_dynlibs(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerPrepareDynlibs").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate dynlib preparation key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerPrepareDynlibs is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerPrepareDynlibs is not a function".into())
            })?;
            let _ = func.call(scope, global.into(), &[]);
            Ok(())
        })
    }

    /// Mounts bundle files into the Pyodide virtual filesystem at the given root directory.
    pub fn mount_bundle(&mut self, bundle: &Bundle, root: &str) -> Result<()> {
        let ctx_state = self.context_state.clone();
        let root_owned = root.to_owned();
        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;

            let files = v8::Array::new(scope, bundle.entries().len() as i32);
            for (index, entry) in bundle.entries().iter().enumerate() {
                let obj = v8::Object::new(scope);
                let rel_path = v8::String::new(scope, entry.path()).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate file path string".into())
                })?;
                let path_key = v8::String::new(scope, "path").unwrap();
                let data_key = v8::String::new(scope, "data").unwrap();
                let size_key = v8::String::new(scope, "size").unwrap();

                let buffer = entry.contents().to_vec();
                let backing = v8::ArrayBuffer::new_backing_store_from_bytes(buffer);
                let shared = backing.make_shared();
                let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
                let uint8 = Uint8Array::new(scope, array_buffer, 0, entry.contents().len())
                    .ok_or_else(|| {
                        PyRunnerError::Execution("failed to allocate typed array".into())
                    })?;
                let size_value = v8::Number::new(scope, entry.contents().len() as f64);

                let _ = obj.set(scope, path_key.into(), rel_path.into());
                let _ = obj.set(scope, data_key.into(), uint8.into());
                let _ = obj.set(scope, size_key.into(), size_value.into());
                files.set_index(scope, index as u32, obj.into());
            }

            let global = scope.get_current_context().global(scope);
            let mount_fn_key = v8::String::new(scope, "__pyRunnerMountFiles").unwrap();
            let mount_fn_value = global
                .get(scope, mount_fn_key.into())
                .ok_or_else(|| {
                    PyRunnerError::Execution("__pyRunnerMountFiles is not defined".into())
                })?;
            let mount_fn = Local::<Function>::try_from(mount_fn_value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerMountFiles is not a function".into())
            })?;
            let root_value = v8::String::new(scope, &root_owned).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate mount root string".into())
            })?;
            mount_fn
                .call(scope, global.into(), &[pyodide.into(), files.into(), root_value.into()])
                .ok_or_else(|| PyRunnerError::Execution("mount files call failed".into()))?;

            let run_key = v8::String::new(scope, "runPython").unwrap();
            let run_value = pyodide.get(scope, run_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("pyodide.runPython is not available".into())
            })?;
            let run_fn = Local::<Function>::try_from(run_value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPython is not a function".into())
            })?;
            let script = v8::String::new(
                scope,
                "import sys\nfrom pathlib import Path\napp = Path('/app')\nif str(app) not in sys.path:\n    sys.path.insert(0, str(app))\n",
            )
            .ok_or_else(|| PyRunnerError::Execution("failed to allocate sys.path script".into()))?;
            let _ = run_fn.call(scope, pyodide.into(), &[script.into()]);

            Ok(())
        })?;
        Ok(())
    }

    /// Executes the specified Python module/function entrypoint, capturing stdout and stderr.
    pub fn run_python_entrypoint(&mut self, entrypoint: &str) -> Result<ExecutionOutput> {
        let ctx_state = self.context_state.clone();
        let entry_literal = serde_json::to_string(entrypoint).map_err(|err| {
            PyRunnerError::Execution(format!("failed to encode entrypoint: {err}"))
        })?;
        let script = format!(
            "import io, sys, importlib, json, traceback\nfrom js import globalThis as __aardvark_js\nif '__aardvark_publish_buffer' not in globals():\n    def __aardvark_publish_buffer(buffer_id, data, metadata=None):\n        return __aardvark_js.__aardvarkPublishBuffer(buffer_id, data, metadata)\nentrypoint = {entry}\n_stdout = io.StringIO()\n_stderr = io.StringIO()\n_old_out, _old_err = sys.stdout, sys.stderr\nresult_repr = None\nexc_type = None\nexc_value = None\nexc_traceback = None\ntry:\n    sys.stdout = _stdout\n    sys.stderr = _stderr\n    module_name, sep, func_name = entrypoint.partition(':')\n    if not module_name:\n        raise ValueError('entrypoint must specify a module')\n    module = importlib.import_module(module_name)\n    if sep:\n        target = getattr(module, func_name)\n        value = target()\n    elif hasattr(module, 'main'):\n        value = module.main()\n    else:\n        value = None\n    result_repr = repr(value)\nexcept Exception as exc:  # noqa: BLE001\n    exc_type = exc.__class__.__name__\n    exc_value = repr(exc)\n    exc_traceback = traceback.format_exc()\nfinally:\n    sys.stdout = _old_out\n    sys.stderr = _old_err\njson.dumps({{\"stdout\": _stdout.getvalue(), \"stderr\": _stderr.getvalue(), \"result\": result_repr, \"exception_type\": exc_type, \"exception_value\": exc_value, \"traceback\": exc_traceback}})\n",
            entry = entry_literal
        );

        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let global = scope.get_current_context().global(scope);
            let request_key = v8::String::new(scope, "__pyRunnerEnterRequestContext").unwrap();
            if let Some(request_value) = global.get(scope, request_key.into()) {
                if let Ok(request_fn) = Local::<Function>::try_from(request_value) {
                    let _ = request_fn.call(scope, global.into(), &[]);
                }
            }
            let reset_key = v8::String::new(scope, "__aardvarkResetSharedBuffers").unwrap();
            if let Some(reset_value) = global.get(scope, reset_key.into()) {
                if let Ok(reset_fn) = Local::<Function>::try_from(reset_value) {
                    let _ = reset_fn.call(scope, global.into(), &[]);
                }
            }
            let run_key = v8::String::new(scope, "runPython").unwrap();
            let run_value = pyodide.get(scope, run_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("pyodide.runPython is not available".into())
            })?;
            let run_fn = Local::<Function>::try_from(run_value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPython is not a function".into())
            })?;
            let script_value = v8::String::new(scope, &script).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate execution script".into())
            })?;
            let result_value = run_fn
                .call(scope, pyodide.into(), &[script_value.into()])
                .ok_or_else(|| PyRunnerError::Execution("running entrypoint failed".into()))?;
            let json_str = result_value.to_rust_string_lossy(scope);
            let parsed: PythonCallResult = serde_json::from_str(&json_str).map_err(|err| {
                PyRunnerError::Execution(format!("failed to parse execution output: {err}"))
            })?;
            let mut execution: ExecutionOutput = parsed.into();
            let shared_buffers = collect_shared_buffers(scope, global)?;
            if !shared_buffers.is_empty() {
                let release_ids: Vec<String> = shared_buffers
                    .iter()
                    .map(|buffer| buffer.id.clone())
                    .collect();
                release_shared_buffers(scope, global, &release_ids)?;
            }
            execution.shared_buffers = shared_buffers;
            Ok(execution)
        })
    }

    /// Executes an arbitrary Python snippet inside the active Pyodide context.
    pub fn run_python_snippet(&mut self, code: &str) -> Result<()> {
        let ctx_state = self.context_state.clone();
        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let run_key = v8::String::new(scope, "runPython").unwrap();
            let run_value = pyodide.get(scope, run_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("pyodide.runPython is not available".into())
            })?;
            let run_fn = Local::<Function>::try_from(run_value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPython is not a function".into())
            })?;
            let script_value = v8::String::new(scope, code).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate python snippet".into())
            })?;
            run_fn
                .call(scope, pyodide.into(), &[script_value.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("python snippet execution failed".into())
                })?;
            Ok(())
        })
    }

    /// Compiles, instantiates, and evaluates an ES module sourced from the asset store.
    pub fn ensure_module(&mut self, specifier: &str) -> Result<()> {
        let specifier = normalize_specifier(specifier);
        if self.context_state.modules.borrow().contains_key(&specifier) {
            return Ok(());
        }
        let ctx_state = self.context_state.clone();
        self.with_context(|scope, _context| {
            let module = compile_module_from_assets(scope, &ctx_state, &specifier)?;
            instantiate_module(scope, &ctx_state, module)?;
            evaluate_module(scope, &ctx_state, module, &specifier)?;
            Ok(())
        })
    }
}

fn populate_execution_output(
    runtime: &mut JsRuntime,
    value_global: v8::Global<v8::Value>,
    execution: &mut ExecutionOutput,
) -> Result<()> {
    runtime.with_context(|scope, _| {
        let value = v8::Local::new(scope, &value_global);
        if value.is_null_or_undefined() {
            execution.result = None;
            execution.json = None;
            return Ok(());
        }

        if let Some(json_value) = v8::json::stringify(scope, value) {
            let json_str = json_value.to_rust_string_lossy(scope);
            if let Ok(parsed) = serde_json::from_str(&json_str) {
                execution.json = Some(parsed);
            }
        }

        if let Some(string_value) = value.to_string(scope) {
            execution.result = Some(string_value.to_rust_string_lossy(scope));
        } else {
            execution.result = Some("<unprintable>".to_string());
        }

        Ok(())
    })
}

fn exec_script(
    scope: &mut PinScope<'_, '_>,
    name: &str,
    source: &str,
) -> std::result::Result<(), String> {
    let code = v8::String::new(scope, source).ok_or_else(|| "source too large".to_owned())?;
    let resource_name = v8::String::new(scope, name).ok_or_else(|| "name too long".to_owned())?;
    let origin = v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    v8::tc_scope!(let try_catch, scope);
    let script = v8::Script::compile(try_catch, code, Some(&origin))
        .ok_or_else(|| format!("failed to compile {name}"))?;
    if script.run(try_catch).is_some() {
        Ok(())
    } else {
        let message = try_catch
            .exception()
            .and_then(|value| value.to_string(try_catch))
            .map(|s| s.to_rust_string_lossy(try_catch))
            .unwrap_or_else(|| "script execution failed".into());
        Err(message)
    }
}

fn asset_fetch_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let name = if args.length() > 0 {
        args.get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    let Some(asset) = context_state.assets.get(&name) else {
        debug!("asset request for '{name}' not found");
        rv.set(v8::undefined(scope).into());
        return;
    };

    let result = v8::Object::new(scope);
    let kind_key = v8::String::new(scope, "kind").unwrap();
    let data_key = v8::String::new(scope, "data").unwrap();
    let size_key = v8::String::new(scope, "size").unwrap();

    match asset {
        Asset::Text(text) => {
            let kind_value = v8::String::new(scope, "text").unwrap();
            let data_value = v8::String::new(scope, &text).unwrap();
            let size_value = v8::Number::new(scope, text.len() as f64);
            let _ = result.set(scope, kind_key.into(), kind_value.into());
            let _ = result.set(scope, data_key.into(), data_value.into());
            let _ = result.set(scope, size_key.into(), size_value.into());
        }
        Asset::Binary(bytes) => {
            let kind_value = v8::String::new(scope, "binary").unwrap();
            let vec = bytes.as_ref().to_vec();
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(vec);
            let shared = backing.make_shared();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let length = array_buffer.byte_length();
            let typed = Uint8Array::new(scope, array_buffer, 0, length).unwrap();
            let size_value = v8::Number::new(scope, length as f64);
            let _ = result.set(scope, kind_key.into(), kind_value.into());
            let _ = result.set(scope, data_key.into(), typed.into());
            let _ = result.set(scope, size_key.into(), size_value.into());
        }
    }

    rv.set(result.into());
}

fn record_buffer_event_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let event = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if event.is_empty() {
        warn!(
            target = "aardvark::buffers",
            "shared buffer event missing name"
        );
        rv.set(v8::undefined(scope).into());
        return;
    }

    let buffer_id = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if buffer_id.is_empty() {
        warn!(
            target = "aardvark::buffers",
            buffers.event = event.as_str(),
            "shared buffer event missing id"
        );
        rv.set(v8::undefined(scope).into());
        return;
    }

    let size = if args.length() > 2 {
        args.get(2).number_value(scope).unwrap_or(0.0).max(0.0)
    } else {
        0.0
    } as u64;

    let mut metadata_json: Option<String> = None;
    if args.length() > 3 {
        let meta_value = args.get(3);
        if !meta_value.is_null_or_undefined() {
            if let Some(stringified) = v8::json::stringify(scope, meta_value) {
                metadata_json = Some(stringified.to_rust_string_lossy(scope));
            } else {
                warn!(
                    target = "aardvark::buffers",
                    buffers.event = event.as_str(),
                    buffers.id = buffer_id.as_str(),
                    "shared buffer metadata stringify failed"
                );
            }
        }
    }

    info!(
        target = "aardvark::buffers",
        buffers.event = event.as_str(),
        buffers.id = buffer_id.as_str(),
        buffers.size = size,
        buffers.metadata = metadata_json.as_deref(),
        "shared buffer event"
    );

    rv.set(v8::undefined(scope).into());
}

fn filesystem_violation_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let message = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "filesystem violation".to_string());
    let path_value = if args.length() > 1 {
        let value = args.get(1);
        if value.is_null_or_undefined() {
            None
        } else {
            value
                .to_string(scope)
                .map(|s| s.to_rust_string_lossy(scope))
        }
    } else {
        None
    };

    if let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() {
        context_state.record_filesystem_violation(path_value, message);
    } else {
        warn!(
            target = "aardvark::sandbox",
            "filesystem violation reported without runtime context"
        );
    }

    rv.set(v8::undefined(scope).into());
}

fn native_log_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let mut parts = Vec::with_capacity(args.length() as usize);
    for index in 0..args.length() {
        let value = args.get(index);
        if let Some(text) = value.to_string(scope) {
            parts.push(text.to_rust_string_lossy(scope));
        }
    }

    let mut stream = ConsoleStream::Stdout;
    let mut start_index = 0;
    if let Some(first) = parts.first() {
        match first.as_str() {
            "__stderr__" => {
                stream = ConsoleStream::Stderr;
                start_index = 1;
            }
            "__stdout__" => {
                stream = ConsoleStream::Stdout;
                start_index = 1;
            }
            _ => {}
        }
    }

    let message = if start_index >= parts.len() {
        String::new()
    } else {
        parts[start_index..].join(" ")
    };

    if let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() {
        match stream {
            ConsoleStream::Stdout => context_state.append_stdout(&message),
            ConsoleStream::Stderr => context_state.append_stderr(&message),
        }
    }

    if !message.is_empty() {
        match stream {
            ConsoleStream::Stdout => {
                info!(target = "aardvark::js", "{}", message);
            }
            ConsoleStream::Stderr => {
                warn!(target = "aardvark::js", "{}", message);
            }
        }
    }
    rv.set(v8::undefined(scope).into());
}

fn native_fetch_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let url = if args.length() > 0 {
        args.get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
    } else {
        String::new()
    };

    if !(url.starts_with("http://") || url.starts_with("https://")) {
        rv.set(v8::undefined(scope).into());
        return;
    }

    if let Some(local_path) = resolve_local_package_path(&url) {
        match fs::read(&local_path) {
            Ok(body) => {
                info!(
                    target = "aardvark::js",
                    %url,
                    path = %local_path.display(),
                    "serving package from local directory"
                );
                let backing = v8::ArrayBuffer::new_backing_store_from_vec(body);
                let byte_length = backing.len();
                let backing_shared = backing.make_shared();
                let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &backing_shared);
                let uint8 = Uint8Array::new(scope, array_buffer, 0, byte_length)
                    .expect("failed to create typed array for local fetch");

                let result = v8::Object::new(scope);
                let status_key = v8::String::new(scope, "status").unwrap();
                let status_value = v8::Integer::new(scope, 200);
                result.set(scope, status_key.into(), status_value.into());

                let status_text_key = v8::String::new(scope, "statusText").unwrap();
                let status_text_value = v8::String::new(scope, "OK").unwrap();
                result.set(scope, status_text_key.into(), status_text_value.into());

                let ok_key = v8::String::new(scope, "ok").unwrap();
                let ok_value = v8::Boolean::new(scope, true);
                result.set(scope, ok_key.into(), ok_value.into());

                let url_key = v8::String::new(scope, "url").unwrap();
                let url_value = v8::String::new(scope, &url).unwrap();
                result.set(scope, url_key.into(), url_value.into());

                let binary_key = v8::String::new(scope, "binary").unwrap();
                let binary_value = v8::Boolean::new(scope, true);
                result.set(scope, binary_key.into(), binary_value.into());

                let body_key = v8::String::new(scope, "body").unwrap();
                result.set(scope, body_key.into(), uint8.into());

                let headers_array = v8::Array::new(scope, 1);
                let header_pair = v8::Array::new(scope, 2);
                let name_value = v8::String::new(scope, "content-type").unwrap();
                let content_type = guess_content_type(&local_path);
                let value_value = v8::String::new(scope, content_type).unwrap();
                header_pair.set_index(scope, 0, name_value.into());
                header_pair.set_index(scope, 1, value_value.into());
                headers_array.set_index(scope, 0, header_pair.into());
                let headers_key = v8::String::new(scope, "headers").unwrap();
                result.set(scope, headers_key.into(), headers_array.into());

                let content_type_key = v8::String::new(scope, "contentType").unwrap();
                let content_type_value = v8::String::new(scope, content_type).unwrap();
                result.set(scope, content_type_key.into(), content_type_value.into());

                rv.set(result.into());
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %url,
                    path = %local_path.display(),
                    error = ?err,
                    "failed to read local package asset"
                );
            }
        }
    } else if is_pyodide_package_asset_url(&url) {
        warn!(
            target = "aardvark::packages",
            %url,
            "Pyodide package asset missing from local distribution"
        );
        let message = v8::String::new(
            scope,
            "Pyodide package asset is missing from AARDVARK_PYODIDE_DIST_DIR",
        )
        .unwrap();
        scope.throw_exception(message.into());
        return;
    }

    let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    let parsed = match Url::parse(&url) {
        Ok(value) => value,
        Err(err) => {
            warn!(target = "aardvark::sandbox", %url, error = ?err, "network request rejected: invalid url");
            let message = v8::String::new(scope, "network access denied").unwrap();
            scope.throw_exception(message.into());
            return;
        }
    };

    let host = match parsed.host_str() {
        Some(value) if !value.is_empty() => value.to_ascii_lowercase(),
        _ => {
            warn!(target = "aardvark::sandbox", %url, "network request rejected: missing host");
            let message = v8::String::new(scope, "network access denied").unwrap();
            scope.throw_exception(message.into());
            return;
        }
    };
    let port = parsed.port();
    let is_https = parsed.scheme().eq_ignore_ascii_case("https");

    let decision = {
        let policy = context_state.network_policy.read();
        policy.evaluate(&host, port, is_https)
    };

    if let NetworkDecision::Denied(reason) = decision {
        context_state.record_network_denial(&host, port, reason);
        let message_text = match reason {
            NetworkDenyReason::SchemeNotAllowed => {
                if let Some(p) = port {
                    format!("network access to '{}:{}' requires https", host, p)
                } else {
                    format!("network access to '{}' requires https", host)
                }
            }
            _ => {
                if let Some(p) = port {
                    format!("network access to '{}:{}' is not permitted", host, p)
                } else {
                    format!("network access to '{}' is not permitted", host)
                }
            }
        };
        warn!(
            target = "aardvark::sandbox",
            network.allowed = false,
            %url,
            host = host.as_str(),
            port,
            reason = ?reason,
            "network request blocked"
        );
        let message = v8::String::new(scope, &message_text).unwrap();
        scope.throw_exception(message.into());
        return;
    }

    info!(
        target = "aardvark::sandbox",
        network.allowed = true,
        %url,
        host = host.as_str(),
        port,
        https = is_https,
        "network request allowed"
    );

    context_state.record_network_contact(&host, port, is_https);

    let mut response = match ureq::get(&url).call() {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = ?err, "native fetch failed");
            rv.set(v8::undefined(scope).into());
            return;
        }
    };

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let mut headers_list = Vec::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers_list.push((name.as_str().to_ascii_lowercase(), value.to_string()));
        }
    }

    let body = match response.body_mut().read_to_vec() {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(%url, error = ?err, "native fetch read failed");
            rv.set(v8::undefined(scope).into());
            return;
        }
    };

    let is_binary = true;
    let backing = v8::ArrayBuffer::new_backing_store_from_vec(body);
    let byte_length = backing.len();
    let backing_shared = backing.make_shared();
    let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &backing_shared);
    let uint8 = Uint8Array::new(scope, array_buffer, 0, byte_length)
        .expect("failed to create typed array for native fetch");

    let result = v8::Object::new(scope);
    let status_key = v8::String::new(scope, "status").unwrap();
    let status_value = v8::Integer::new(scope, status as i32);
    result.set(scope, status_key.into(), status_value.into());

    let status_text_key = v8::String::new(scope, "statusText").unwrap();
    let status_text_value = v8::String::new(scope, &status_text).unwrap();
    result.set(scope, status_text_key.into(), status_text_value.into());

    let ok_key = v8::String::new(scope, "ok").unwrap();
    let ok_value = v8::Boolean::new(scope, (200..300).contains(&status));
    result.set(scope, ok_key.into(), ok_value.into());

    let url_key = v8::String::new(scope, "url").unwrap();
    let url_value = v8::String::new(scope, &url).unwrap();
    result.set(scope, url_key.into(), url_value.into());

    let binary_key = v8::String::new(scope, "binary").unwrap();
    let binary_value = v8::Boolean::new(scope, is_binary);
    result.set(scope, binary_key.into(), binary_value.into());

    let body_key = v8::String::new(scope, "body").unwrap();
    result.set(scope, body_key.into(), uint8.into());

    let headers_array = v8::Array::new(scope, headers_list.len() as i32);
    for (index, (name, value)) in headers_list.iter().enumerate() {
        let pair = v8::Array::new(scope, 2);
        let name_value = v8::String::new(scope, name).unwrap();
        let value_value = v8::String::new(scope, value).unwrap();
        pair.set_index(scope, 0, name_value.into());
        pair.set_index(scope, 1, value_value.into());
        headers_array.set_index(scope, index as u32, pair.into());
    }
    let headers_key = v8::String::new(scope, "headers").unwrap();
    result.set(scope, headers_key.into(), headers_array.into());

    // Provide a hint for the content type.
    if let Some((_, value)) = headers_list
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        let content_type_key = v8::String::new(scope, "contentType").unwrap();
        let content_type_value = v8::String::new(scope, value).unwrap();
        result.set(scope, content_type_key.into(), content_type_value.into());
    }

    rv.set(result.into());
}

fn compile_module_from_assets<'a>(
    scope: &mut PinScope<'a, '_>,
    ctx: &Rc<RuntimeContext>,
    specifier: &str,
) -> Result<Local<'a, Module>> {
    if let Some(existing) = ctx.modules.borrow().get(specifier) {
        return Ok(Local::new(scope, existing));
    }
    let asset = ctx
        .assets
        .get(specifier)
        .ok_or_else(|| PyRunnerError::Execution(format!("module asset not found: {specifier}")))?;
    let source_text = match asset {
        Asset::Text(text) => text,
        Asset::Binary(_) => {
            return Err(PyRunnerError::Execution(format!(
                "module asset '{specifier}' is binary"
            )))
        }
    };
    let source_str: &str = &source_text;
    let source_string = v8::String::new(scope, source_str).ok_or_else(|| {
        PyRunnerError::Execution(format!("failed to allocate source string for {specifier}"))
    })?;
    let resource_name = v8::String::new(scope, specifier).ok_or_else(|| {
        PyRunnerError::Execution(format!("failed to allocate resource name for {specifier}"))
    })?;
    let origin = v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        true,
        None,
    );
    let mut source = script_compiler::Source::new(source_string, Some(&origin));
    let module = script_compiler::compile_module(scope, &mut source)
        .ok_or_else(|| PyRunnerError::Execution(format!("failed to compile module {specifier}")))?;
    let global = v8::Global::new(scope, module);
    ctx.modules
        .borrow_mut()
        .insert(specifier.to_owned(), global);
    ctx.module_by_hash
        .borrow_mut()
        .insert(module.get_identity_hash().get(), specifier.to_owned());
    // Recursively compile dependencies so that instantiate_module can simply look them up.
    let requests = module.get_module_requests();
    let len = requests.length();
    for i in 0..len {
        if let Some(data) = requests.get(scope, i) {
            if let Ok(request) = Local::<ModuleRequest>::try_from(data) {
                let request_spec = request.get_specifier().to_rust_string_lossy(scope);
                let resolved = resolve_specifier(specifier, &request_spec);
                compile_module_from_assets(scope, ctx, &resolved)?;
            }
        }
    }

    Ok(module)
}

fn instantiate_module(
    scope: &mut PinScope<'_, '_>,
    ctx: &Rc<RuntimeContext>,
    module: Local<Module>,
) -> Result<()> {
    let scope_ptr = scope as *mut _ as *mut std::ffi::c_void;
    TLS_RUNTIME_CONTEXT.with(|cell| cell.set(Rc::as_ptr(ctx)));
    TLS_SCOPE.with(|cell| cell.set(scope_ptr));
    let instantiated = module.instantiate_module(scope, resolve_module_callback);
    TLS_SCOPE.with(|cell| cell.set(std::ptr::null_mut()));
    TLS_RUNTIME_CONTEXT.with(|cell| cell.set(std::ptr::null()));
    if instantiated.is_some() {
        Ok(())
    } else {
        Err(PyRunnerError::Execution(
            "failed to instantiate module".into(),
        ))
    }
}

fn evaluate_module(
    scope: &mut PinScope<'_, '_>,
    ctx: &Rc<RuntimeContext>,
    module: Local<Module>,
    specifier: &str,
) -> Result<()> {
    module
        .evaluate(scope)
        .ok_or_else(|| PyRunnerError::Execution("module evaluation failed".into()))?;
    let namespace_value = module.get_module_namespace();
    let namespace_obj = v8::Local::<Object>::try_from(namespace_value).map_err(|_| {
        PyRunnerError::Execution(format!(
            "module namespace for {specifier} was not an object"
        ))
    })?;
    ctx.module_namespaces
        .borrow_mut()
        .insert(specifier.to_owned(), v8::Global::new(scope, namespace_obj));
    Ok(())
}

fn resolve_module_callback<'a>(
    _context: Local<'a, Context>,
    specifier: Local<'a, V8String>,
    _import_assertions: Local<'a, FixedArray>,
    referencing_module: Local<'a, Module>,
) -> Option<Local<'a, Module>> {
    let scope_ptr = TLS_SCOPE.with(|cell| cell.get()) as *mut PinScope<'static, 'static>;
    let ctx_ptr = TLS_RUNTIME_CONTEXT.with(|cell| cell.get());
    if scope_ptr.is_null() || ctx_ptr.is_null() {
        return None;
    }
    let scope_ref = unsafe { &mut *scope_ptr };
    let ctx_ref = unsafe { &*ctx_ptr };
    let request = specifier.to_rust_string_lossy(scope_ref);
    let parent_hash = referencing_module.get_identity_hash().get();
    let base = ctx_ref
        .module_by_hash
        .borrow()
        .get(&parent_hash)
        .cloned()
        .unwrap_or_default();
    let resolved = resolve_specifier(&base, &request);
    let maybe_module = ctx_ref
        .modules
        .borrow()
        .get(&resolved)
        .cloned()
        .map(|global| Local::new(scope_ref, &global));
    if let Some(module) = maybe_module {
        Some(module)
    } else {
        if let Some(message) = v8::String::new(
            scope_ref,
            &format!("unresolved module specifier: {resolved}"),
        ) {
            scope_ref.throw_exception(message.into());
        }
        None
    }
}

fn resolve_specifier(base: &str, request: &str) -> String {
    if request.starts_with("./") || request.starts_with("../") {
        let mut parts: Vec<&str> = if base.is_empty() {
            Vec::new()
        } else {
            base.rsplit_once('/')
                .map(|(prefix, _)| prefix.split('/').collect())
                .unwrap_or_default()
        };
        for segment in request.split('/') {
            match segment {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        return parts.join("/");
    }
    request.trim_start_matches("./").to_owned()
}

fn normalize_specifier(spec: &str) -> String {
    if spec.starts_with("./") {
        spec.trim_start_matches("./").to_owned()
    } else {
        spec.to_owned()
    }
}

impl RuntimeContext {
    fn new() -> Self {
        Self {
            assets: AssetStore::new(),
            modules: RefCell::new(HashMap::new()),
            module_by_hash: RefCell::new(HashMap::new()),
            module_namespaces: RefCell::new(HashMap::new()),
            pyodide_instance: RefCell::new(None),
            stdout_log: RefCell::new(String::new()),
            stderr_log: RefCell::new(String::new()),
            network_policy: RwLock::new(NetworkPolicy::default()),
            network_contacts: RwLock::new(Vec::new()),
            network_denied: RwLock::new(Vec::new()),
            filesystem_violations: RwLock::new(Vec::new()),
        }
    }

    fn clear_console(&self) {
        self.stdout_log.borrow_mut().clear();
        self.stderr_log.borrow_mut().clear();
    }

    fn append_stdout(&self, message: &str) {
        if message.is_empty() {
            return;
        }
        let mut stdout = self.stdout_log.borrow_mut();
        stdout.push_str(message);
        stdout.push('\n');
    }

    fn append_stderr(&self, message: &str) {
        if message.is_empty() {
            return;
        }
        let mut stderr = self.stderr_log.borrow_mut();
        stderr.push_str(message);
        stderr.push('\n');
    }

    fn take_stdout(&self) -> String {
        let mut stdout = self.stdout_log.borrow_mut();
        std::mem::take(&mut *stdout)
    }

    fn take_stderr(&self) -> String {
        let mut stderr = self.stderr_log.borrow_mut();
        std::mem::take(&mut *stderr)
    }

    fn module_namespace<'a>(
        &self,
        scope: &mut PinScope<'a, '_>,
        specifier: &str,
    ) -> Option<Local<'a, Object>> {
        self.module_namespaces
            .borrow()
            .get(specifier)
            .map(|global| Local::new(scope, global))
    }

    fn pyodide_local<'a>(&self, scope: &mut PinScope<'a, '_>) -> Option<Local<'a, Object>> {
        self.pyodide_instance
            .borrow()
            .as_ref()
            .map(|global| Local::new(scope, global))
    }

    fn set_network_policy(&self, allow: &[String], https_only: bool) {
        let mut policy = self.network_policy.write();
        *policy = NetworkPolicy::new(allow, https_only);
        self.clear_network_contacts();
        self.clear_network_denied();
    }

    fn clear_network_contacts(&self) {
        self.network_contacts.write().clear();
    }

    fn clear_network_denied(&self) {
        self.network_denied.write().clear();
    }

    fn record_network_contact(&self, host: &str, port: Option<u16>, https: bool) {
        let mut contacts = self.network_contacts.write();
        if !contacts
            .iter()
            .any(|entry| entry.host == host && entry.port == port && entry.https == https)
        {
            contacts.push(NetworkContactRecord {
                host: host.to_owned(),
                port,
                https,
            });
        }
    }

    fn take_network_contacts(&self) -> Vec<NetworkContactRecord> {
        let mut contacts = self.network_contacts.write();
        std::mem::take(&mut *contacts)
    }

    fn record_network_denial(&self, host: &str, port: Option<u16>, reason: NetworkDenyReason) {
        let reason_str = reason.as_str().to_string();
        let https_required = matches!(reason, NetworkDenyReason::SchemeNotAllowed);
        let mut denied = self.network_denied.write();
        if !denied.iter().any(|entry| {
            entry.host == host
                && entry.port == port
                && entry.reason == reason_str
                && entry.https_required == https_required
        }) {
            denied.push(NetworkDeniedRecord {
                host: host.to_owned(),
                port,
                reason: reason_str,
                https_required,
            });
        }
    }

    fn take_network_denied(&self) -> Vec<NetworkDeniedRecord> {
        let mut denied = self.network_denied.write();
        std::mem::take(&mut *denied)
    }

    fn record_filesystem_violation(&self, path: Option<String>, message: String) {
        let mut violations = self.filesystem_violations.write();
        violations.push(FilesystemViolationRecord { path, message });
    }

    fn clear_filesystem_violations(&self) {
        self.filesystem_violations.write().clear();
    }

    fn take_filesystem_violations(&self) -> Vec<FilesystemViolationRecord> {
        let mut violations = self.filesystem_violations.write();
        std::mem::take(&mut *violations)
    }
}

#[derive(Debug, Deserialize)]
struct PythonCallResult {
    stdout: String,
    stderr: String,
    result: Option<String>,
    #[serde(default)]
    exception_type: Option<String>,
    #[serde(default)]
    exception_value: Option<String>,
    #[serde(default)]
    traceback: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SharedBuffer {
    pub id: String,
    pub length: usize,
    pub metadata: Option<JsonValue>,
    pub backing: Option<Arc<SharedBufferBacking>>,
    pub bytes: Option<Bytes>,
}

#[derive(Debug)]
pub struct SharedBufferBacking {
    store: v8::SharedRef<v8::BackingStore>,
    offset: usize,
    length: usize,
}

impl SharedBufferBacking {
    fn new(store: v8::SharedRef<v8::BackingStore>, offset: usize, length: usize) -> Self {
        Self {
            store,
            offset,
            length,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.length == 0 {
            return &[];
        }
        let Some(ptr) = self.store.data() else {
            return &[];
        };
        let store_size = self.store.byte_length();
        if self.offset > store_size || self.length > store_size {
            return &[];
        }
        if let Some(end) = self.offset.checked_add(self.length) {
            if end > store_size {
                return &[];
            }
        } else {
            return &[];
        }
        unsafe {
            let data = ptr.as_ptr().add(self.offset) as *const u8;
            std::slice::from_raw_parts(data, self.length)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    pub exception_type: Option<String>,
    pub exception_value: Option<String>,
    pub traceback: Option<String>,
    pub json: Option<JsonValue>,
    pub shared_buffers: Vec<SharedBuffer>,
}

impl From<PythonCallResult> for ExecutionOutput {
    fn from(value: PythonCallResult) -> Self {
        Self {
            stdout: value.stdout,
            stderr: value.stderr,
            result: value.result,
            exception_type: value.exception_type,
            exception_value: value.exception_value,
            traceback: value.traceback,
            json: None,
            shared_buffers: Vec::new(),
        }
    }
}

fn collect_shared_buffers<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
) -> Result<Vec<SharedBuffer>> {
    let mut buffers = Vec::new();
    let collect_key = v8::String::new(scope, "__aardvarkCollectSharedBuffers").unwrap();
    let Some(collect_value) = global.get(scope, collect_key.into()) else {
        return Ok(buffers);
    };
    let Ok(collect_fn) = Local::<Function>::try_from(collect_value) else {
        return Ok(buffers);
    };
    let result = collect_fn
        .call(scope, global.into(), &[])
        .ok_or_else(|| PyRunnerError::Execution("collect shared buffers call failed".into()))?;
    let Ok(array) = Local::<Array>::try_from(result) else {
        return Ok(buffers);
    };
    let length = array.length();
    if length == 0 {
        return Ok(buffers);
    }

    let id_key = v8::String::new(scope, "id").unwrap();
    let buffer_key = v8::String::new(scope, "buffer").unwrap();
    let metadata_key = v8::String::new(scope, "metadata").unwrap();

    for index in 0..length {
        let entry_value = array
            .get_index(scope, index)
            .ok_or_else(|| PyRunnerError::Execution("shared buffer entry missing".into()))?;
        let entry_obj = entry_value
            .to_object(scope)
            .ok_or_else(|| PyRunnerError::Execution("shared buffer entry not an object".into()))?;
        let id_value = entry_obj
            .get(scope, id_key.into())
            .ok_or_else(|| PyRunnerError::Execution("shared buffer missing id".into()))?;
        let id = id_value
            .to_string(scope)
            .ok_or_else(|| PyRunnerError::Execution("failed to stringify buffer id".into()))?
            .to_rust_string_lossy(scope);

        let buffer_value = entry_obj
            .get(scope, buffer_key.into())
            .ok_or_else(|| PyRunnerError::Execution("shared buffer missing payload".into()))?;
        let typed_array = Local::<Uint8Array>::try_from(buffer_value).map_err(|_| {
            PyRunnerError::Execution("shared buffer payload is not a Uint8Array".into())
        })?;
        let byte_len = typed_array.byte_length();
        let array_buffer = typed_array.buffer(scope).ok_or_else(|| {
            PyRunnerError::Execution("shared buffer missing backing store".into())
        })?;
        let backing_store = array_buffer.get_backing_store();
        let offset = typed_array.byte_offset();

        let metadata = match entry_obj.get(scope, metadata_key.into()) {
            Some(value) if !value.is_null_or_undefined() => {
                let json_value = v8::json::stringify(scope, value).ok_or_else(|| {
                    PyRunnerError::Execution("failed to stringify shared buffer metadata".into())
                })?;
                let json_str = json_value.to_rust_string_lossy(scope);
                Some(serde_json::from_str(&json_str).map_err(|err| {
                    PyRunnerError::Execution(format!(
                        "failed to parse shared buffer metadata: {err}"
                    ))
                })?)
            }
            _ => None,
        };

        buffers.push(SharedBuffer {
            id,
            length: byte_len,
            metadata,
            backing: Some(Arc::new(SharedBufferBacking::new(
                backing_store,
                offset,
                byte_len,
            ))),
            bytes: None,
        });
    }

    Ok(buffers)
}

fn release_shared_buffers<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
    ids: &[String],
) -> Result<()> {
    let release_key = v8::String::new(scope, "__aardvarkReleaseSharedBuffers").unwrap();
    let Some(release_value) = global.get(scope, release_key.into()) else {
        return Ok(());
    };
    let Ok(release_fn) = Local::<Function>::try_from(release_value) else {
        return Ok(());
    };

    let mut args: Vec<Local<Value>> = Vec::new();
    if !ids.is_empty() {
        let id_array = Array::new(scope, ids.len() as i32);
        for (index, id) in ids.iter().enumerate() {
            let id_value = v8::String::new(scope, id).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate buffer id string".into())
            })?;
            id_array.set_index(scope, index as u32, id_value.into());
        }
        args.push(id_array.into());
    }

    release_fn
        .call(scope, global.into(), &args)
        .ok_or_else(|| PyRunnerError::Execution("release shared buffers call failed".into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn package_root_override_sets_local_distribution_root() {
        reset_package_root_for_tests();
        let temp = tempdir().expect("create tempdir");
        let cache_dir = temp.path().join("cache");
        fs::create_dir(&cache_dir).expect("create cache dir");

        set_package_root_override(Some(cache_dir.clone()));
        let resolved = package_root_dir().expect("package root available");
        assert_eq!(resolved, cache_dir);

        reset_package_root_for_tests();
    }

    #[test]
    fn pyodide_package_asset_url_matches_pyodide_archives_only() {
        assert!(is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/pyodide/v0.29.4/full/numpy-2.2.5-cp313-cp313-pyodide_2025_0_wasm32.whl"
        ));
        assert!(is_pyodide_package_asset_url(
            "https://github.com/pyodide/pyodide/releases/download/0.29.4/pyodide-core-0.29.4.tar.bz2"
        ));
        assert!(!is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/npm/some-package/dist/archive.tar.gz"
        ));
        assert!(!is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/gh/example/project/data.zip"
        ));
    }
}
