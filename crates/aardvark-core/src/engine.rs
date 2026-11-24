//! Lightweight V8 runtime utilities.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::asset_store::AssetStore;
use crate::bundle_manifest::ManifestNetworkResources;
use crate::error::{PyRunnerError, Result};
use crate::network::{NetworkContactRecord, NetworkDeniedRecord, NetworkDenyReason, NetworkPolicy};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use v8::{self, ContextScope, Local, Module, Object, PinScope, Uint8Array, Value};
mod array_buffer_budget;
mod bootstrap_sources;
mod filesystem;
mod host_callbacks;
mod host_capabilities;
mod host_hooks;
mod inputs;
mod js_invocation;
mod modules;
mod native_fetch;
mod output;
mod package_assets;
mod pyodide_runtime;
mod python_entrypoint;
mod python_invocation;
mod shared_buffers;
mod snippets;

use host_callbacks::{
    asset_fetch_callback, filesystem_violation_callback, memory_guard_callback,
    native_log_callback, record_buffer_event_callback,
};
use native_fetch::native_fetch_callback;
pub use output::ExecutionOutput;
#[cfg(test)]
use package_assets::is_pyodide_package_asset_url;
use package_assets::normalize_package_root;
pub use shared_buffers::{SharedBuffer, SharedBufferBacking};

static V8_PLATFORM: OnceCell<v8::SharedRef<v8::Platform>> = OnceCell::new();

unsafe extern "C" fn aardvark_v8_message_listener(
    _message: Local<v8::Message>,
    _value: Local<Value>,
) {
}

#[derive(Debug, Clone)]
pub struct FilesystemViolationRecord {
    pub path: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemModeConfig {
    Read,
    ReadWrite,
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

fn copy_typed_array(array: Local<Uint8Array>) -> Vec<u8> {
    let length = array.length();
    let mut data = vec![0u8; length];
    array.copy_contents(&mut data);
    data
}

/// Ensure the global V8 platform is initialized exactly once.
/// V8 flags set once, before the platform starts. `--wasm-max-mem-pages`
/// caps every WebAssembly memory, memory64 included, at 4 GiB: wasm32's
/// ceiling, so Pyodide's own heap is unaffected, but a guest-compiled
/// memory64 module can't declare more.
const V8_FLAGS: &str = "--wasm-max-mem-pages=65536";

fn init_v8() {
    V8_PLATFORM.get_or_init(|| {
        v8::V8::set_flags_from_string(V8_FLAGS);
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
    /// Target of the isolate's near-heap-limit callback. Declared after
    /// `isolate` so it is dropped after the isolate that points at it.
    heap_guard: Option<Box<HeapGuard>>,
    /// Live ArrayBuffer bytes of this isolate and the per-call allowance.
    array_buffers: Arc<array_buffer_budget::ArrayBufferBudget>,
}

/// State shared with the near-heap-limit callback of an isolate created with
/// a JS heap limit.
struct HeapGuard {
    handle: v8::IsolateHandle,
    fired: std::sync::atomic::AtomicBool,
}

/// Headroom granted past the JS heap limit so a terminated call can unwind.
const HEAP_LIMIT_HEADROOM_BYTES: usize = 32 * 1024 * 1024;

extern "C" fn near_heap_limit_callback(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    // SAFETY: `data` is the `HeapGuard` boxed in the `JsRuntime` that owns
    // this isolate. The box never moves and is dropped after the isolate, so
    // it outlives every invocation of this callback.
    let guard = unsafe { &*(data as *const HeapGuard) };
    guard.fired.store(true, std::sync::atomic::Ordering::SeqCst);
    guard.handle.terminate_execution();
    current_heap_limit.saturating_add(HEAP_LIMIT_HEADROOM_BYTES)
}

struct IsolateEntryGuard {
    isolate: *const v8::Isolate,
}

impl IsolateEntryGuard {
    fn enter(isolate: &v8::OwnedIsolate) -> Self {
        // rusty_v8 enters an OwnedIsolate at creation time, but the current
        // isolate on this thread is the most recently entered one. Pools keep
        // multiple isolates alive, so every operation must temporarily re-enter
        // the isolate it is about to touch.
        // SAFETY: `isolate` is an initialized `OwnedIsolate` borrowed for the
        // duration of the guard, and the matching `exit` runs in `Drop`.
        unsafe {
            isolate.enter();
        }
        Self {
            isolate: &**isolate as *const v8::Isolate,
        }
    }
}

impl Drop for IsolateEntryGuard {
    fn drop(&mut self) {
        // SAFETY: `self.isolate` was captured from the live `OwnedIsolate` in
        // `enter`, and the guard's lifetime is scoped to that borrow.
        unsafe {
            (&*self.isolate).exit();
        }
    }
}

struct RuntimeContext {
    assets: AssetStore,
    modules: RefCell<HashMap<String, v8::Global<Module>>>,
    module_by_hash: RefCell<HashMap<i32, String>>,
    module_namespaces: RefCell<HashMap<String, v8::Global<Object>>>,
    installed_rawctx_specs: RefCell<HashSet<String>>,
    pyodide_instance: RefCell<Option<v8::Global<Object>>>,
    stdout_log: RefCell<String>,
    stderr_log: RefCell<String>,
    package_root: RwLock<Option<PathBuf>>,
    network_policy: RwLock<NetworkPolicy>,
    network_contacts: RwLock<Vec<NetworkContactRecord>>,
    network_denied: RwLock<Vec<NetworkDeniedRecord>>,
    filesystem_violations: RwLock<Vec<FilesystemViolationRecord>>,
    /// Cap on total WebAssembly memory for the running invocation.
    memory_limit_bytes: Cell<Option<u64>>,
    /// Requested totals the memory guard refused.
    memory_denials: RefCell<Vec<u64>>,
    /// The guard's `trackedMemoryBytes` function.
    memory_tracker: RefCell<Option<v8::Global<v8::Function>>>,
}

impl JsRuntime {
    /// Creates a new isolate with an empty context and basic polyfills.
    pub fn new() -> Result<Self> {
        Self::with_heap_limit(None)
    }

    /// Creates a new isolate whose JS heap is capped at `max_heap_bytes`.
    /// Nearing the cap terminates the running call instead of aborting the
    /// process; [`JsRuntime::take_heap_limit_hit`] reports it.
    pub fn with_heap_limit(max_heap_bytes: Option<usize>) -> Result<Self> {
        init_v8();
        let context_state = Rc::new(RuntimeContext::new());
        let array_buffers = array_buffer_budget::ArrayBufferBudget::new();
        let mut create_params = v8::CreateParams::default()
            .array_buffer_allocator(array_buffer_budget::allocator(&array_buffers));
        if let Some(max) = max_heap_bytes {
            create_params = create_params.heap_limits(0, max);
        }
        let mut isolate = v8::Isolate::new(create_params);
        let heap_guard = max_heap_bytes.map(|_| {
            let guard = Box::new(HeapGuard {
                handle: isolate.thread_safe_handle(),
                fired: std::sync::atomic::AtomicBool::new(false),
            });
            isolate.add_near_heap_limit_callback(
                near_heap_limit_callback,
                &*guard as *const HeapGuard as *mut std::ffi::c_void,
            );
            guard
        });
        isolate.add_message_listener(aardvark_v8_message_listener);
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
            heap_guard,
            array_buffers,
        };
        runtime.install_polyfills()?;
        Ok(runtime)
    }

    /// Sets the cap on the total size of the context's WebAssembly memories
    /// for the next invocation; `None` lifts it. Growth past the cap is
    /// refused (Pyodide's `malloc` then fails and Python raises MemoryError)
    /// and recorded for [`JsRuntime::take_memory_limit_denials`].
    pub fn set_memory_limit(&self, limit_bytes: Option<u64>) {
        self.context_state.memory_limit_bytes.set(limit_bytes);
    }

    /// Lets live ArrayBuffer bytes grow by at most `allowance_bytes` from now
    /// (`None` lifts the cap). Allocations past it fail with a `RangeError`
    /// and are counted for [`JsRuntime::take_array_buffer_denials`].
    pub fn set_array_buffer_allowance(&self, allowance_bytes: Option<u64>) {
        self.array_buffers.set_allowance(
            allowance_bytes.map(|bytes| usize::try_from(bytes).unwrap_or(usize::MAX)),
        );
    }

    /// ArrayBuffer allocations refused since the last call.
    pub fn take_array_buffer_denials(&self) -> usize {
        self.array_buffers.take_denials()
    }

    /// Consumes the WebAssembly memory requests refused since the last call,
    /// as requested total bytes.
    pub fn take_memory_limit_denials(&self) -> Vec<u64> {
        std::mem::take(&mut *self.context_state.memory_denials.borrow_mut())
    }

    /// Total bytes of the WebAssembly memories created through
    /// `WebAssembly.Memory` in this context (Pyodide's heap among them).
    pub fn tracked_memory_bytes(&mut self) -> Option<u64> {
        let tracker = self.context_state.memory_tracker.borrow().clone()?;
        self.with_context(|scope, context| {
            let function = Local::new(scope, &tracker);
            let receiver = context.global(scope);
            v8::tc_scope!(let try_catch, scope);
            Ok(function
                .call(try_catch, receiver.into(), &[])
                .and_then(|value| value.number_value(try_catch))
                .map(|bytes| bytes as u64))
        })
        .ok()
        .flatten()
    }

    /// Whether the JS heap limit terminated execution since the last call.
    /// Clears the flag.
    pub fn take_heap_limit_hit(&self) -> bool {
        self.heap_guard
            .as_ref()
            .is_some_and(|guard| guard.fired.swap(false, std::sync::atomic::Ordering::SeqCst))
    }

    /// Reinitializes the isolate in place, keeping the outer runtime alive.
    pub fn reset(&mut self) -> Result<()> {
        let package_root = self.context_state.package_root();
        // Drop the previous context state so any module caches or globals are released.
        let new_state = Rc::new(RuntimeContext::new());
        new_state.set_package_root(package_root);
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

    /// Configures the local Pyodide package root for this runtime.
    pub(crate) fn set_package_root(&self, path: Option<PathBuf>) {
        self.context_state.set_package_root(path);
    }

    /// Configures the network allowlist for subsequent native fetches.
    pub fn set_network_policy(&self, allow: &[String], https_only: bool) {
        self.context_state.set_network_policy(allow, https_only);
    }

    /// Configures manifest-derived network allowlist and native fetch budgets.
    pub(crate) fn set_manifest_network_policy(&self, network: &ManifestNetworkResources) {
        self.context_state.set_manifest_network_policy(network);
    }

    /// Clears any recorded network contacts before a new invocation begins.
    pub fn clear_network_contacts(&self) {
        self.context_state.clear_network_contacts();
    }

    /// Consumes and returns the recorded network contacts from the last invocation.
    pub(crate) fn drain_network_contacts(&self) -> Vec<NetworkContactRecord> {
        self.context_state.take_network_contacts()
    }

    /// Clears any recorded denied network attempts before a new invocation begins.
    pub fn clear_network_denied(&self) {
        self.context_state.clear_network_denied();
    }

    /// Consumes and returns network attempts that were blocked by policy.
    pub(crate) fn drain_network_denied(&self) -> Vec<NetworkDeniedRecord> {
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

    pub(crate) fn is_rawctx_auto_wrapper_installed(&self, spec_key: &str) -> bool {
        self.context_state
            .installed_rawctx_specs
            .borrow()
            .contains(spec_key)
    }

    pub(crate) fn mark_rawctx_auto_wrapper_installed(&self, spec_key: impl Into<String>) {
        self.context_state
            .installed_rawctx_specs
            .borrow_mut()
            .insert(spec_key.into());
    }

    pub(crate) fn clear_rawctx_auto_wrapper_cache(&self) {
        self.context_state
            .installed_rawctx_specs
            .borrow_mut()
            .clear();
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

            // The memory guard receives its native check as an argument, so
            // the check never appears on the global object.
            let guard_installer =
                eval_script(scope, "memory_guard.js", include_str!("js/memory_guard.js"))
                    .map_err(|msg| PyRunnerError::Init(format!("memory guard error: {msg}")))?;
            let guard_installer = Local::<v8::Function>::try_from(guard_installer)
                .map_err(|_| PyRunnerError::Init("memory guard is not a function".into()))?;
            let allow = v8::FunctionTemplate::new(scope, memory_guard_callback)
                .get_function(scope)
                .ok_or_else(|| PyRunnerError::Init("failed to create memory guard hook".into()))?;
            let tracker = guard_installer
                .call(scope, global.into(), &[allow.into()])
                .and_then(|value| Local::<v8::Function>::try_from(value).ok())
                .ok_or_else(|| PyRunnerError::Init("memory guard install failed".into()))?;
            if let Some(state) = scope.get_slot::<Rc<RuntimeContext>>() {
                *state.memory_tracker.borrow_mut() = Some(v8::Global::new(scope, tracker));
            }
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
        let _entry = IsolateEntryGuard::enter(&self.isolate);
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

    /// Registers a binary asset backed by a shared immutable buffer.
    pub fn insert_binary_asset_shared(&self, name: &str, bytes: Arc<[u8]>) {
        self.context_state.assets.insert_bytes(name, bytes);
    }
}

/// Most bytes of guest console output (stdout, stderr) kept per invocation.
pub(crate) const MAX_CAPTURED_OUTPUT_BYTES: usize = 1024 * 1024;
const TRUNCATION_MARKER: &str = "\n[aardvark: output truncated]\n";

/// Append a console line to a capture buffer, stopping at
/// [`MAX_CAPTURED_OUTPUT_BYTES`] with a single truncation marker.
fn append_capped(buffer: &mut String, message: &str) {
    if message.is_empty() || buffer.ends_with(TRUNCATION_MARKER) {
        return;
    }
    let room = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(buffer.len());
    if message.len() < room {
        buffer.push_str(message);
        buffer.push('\n');
    } else {
        let mut cut = room;
        while cut > 0 && !message.is_char_boundary(cut) {
            cut -= 1;
        }
        buffer.push_str(&message[..cut]);
        buffer.push_str(TRUNCATION_MARKER);
    }
}

/// `text` cut to [`MAX_CAPTURED_OUTPUT_BYTES`] on a character boundary, with
/// the truncation marker when anything was dropped.
pub(crate) fn truncate_captured(text: &str) -> String {
    if text.len() <= MAX_CAPTURED_OUTPUT_BYTES {
        return text.to_owned();
    }
    let mut cut = MAX_CAPTURED_OUTPUT_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &text[..cut])
}

/// Compile and run `source`, returning its completion value.
fn eval_script<'s>(
    scope: &mut PinScope<'s, '_>,
    name: &str,
    source: &str,
) -> std::result::Result<Local<'s, Value>, String> {
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
    let script = v8::Script::compile(scope, code, Some(&origin))
        .ok_or_else(|| format!("failed to compile {name}"))?;
    script.run(scope).ok_or_else(|| format!("{name} threw"))
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

impl RuntimeContext {
    fn new() -> Self {
        Self {
            assets: AssetStore::new(),
            modules: RefCell::new(HashMap::new()),
            module_by_hash: RefCell::new(HashMap::new()),
            module_namespaces: RefCell::new(HashMap::new()),
            installed_rawctx_specs: RefCell::new(HashSet::new()),
            pyodide_instance: RefCell::new(None),
            stdout_log: RefCell::new(String::new()),
            stderr_log: RefCell::new(String::new()),
            package_root: RwLock::new(None),
            network_policy: RwLock::new(NetworkPolicy::default()),
            network_contacts: RwLock::new(Vec::new()),
            network_denied: RwLock::new(Vec::new()),
            filesystem_violations: RwLock::new(Vec::new()),
            memory_limit_bytes: Cell::new(None),
            memory_denials: RefCell::new(Vec::new()),
            memory_tracker: RefCell::new(None),
        }
    }

    /// The memory guard's check: may the context's WebAssembly memories grow
    /// to `requested_bytes` in total?
    fn allow_memory(&self, requested_bytes: u64) -> bool {
        match self.memory_limit_bytes.get() {
            Some(limit) if requested_bytes > limit => {
                self.memory_denials.borrow_mut().push(requested_bytes);
                false
            }
            _ => true,
        }
    }

    fn clear_console(&self) {
        self.stdout_log.borrow_mut().clear();
        self.stderr_log.borrow_mut().clear();
    }

    fn set_package_root(&self, path: Option<PathBuf>) {
        let normalized = path.map(normalize_package_root);
        {
            let mut guard = self.package_root.write();
            *guard = normalized.clone();
        }
        match normalized {
            Some(ref path) => tracing::debug!(
                target: "aardvark::packages",
                path = %path.display(),
                "runtime package root set"
            ),
            None => tracing::debug!(
                target: "aardvark::packages",
                "runtime package root cleared"
            ),
        }
    }

    fn package_root(&self) -> Option<PathBuf> {
        self.package_root.read().as_ref().cloned()
    }

    fn append_stdout(&self, message: &str) {
        append_capped(&mut self.stdout_log.borrow_mut(), message);
    }

    fn append_stderr(&self, message: &str) {
        append_capped(&mut self.stderr_log.borrow_mut(), message);
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

    fn set_manifest_network_policy(&self, network: &ManifestNetworkResources) {
        let mut policy = self.network_policy.write();
        *policy = NetworkPolicy::from_manifest(network);
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

#[cfg(test)]
mod tests {
    use super::*;

    mod captured_output {
        use super::*;

        #[test]
        fn console_capture_stops_at_the_cap_with_one_marker() {
            let mut buffer = String::new();
            let line = "x".repeat(1000);
            for _ in 0..5_000 {
                append_capped(&mut buffer, &line);
            }
            assert!(buffer.len() <= MAX_CAPTURED_OUTPUT_BYTES + TRUNCATION_MARKER.len());
            assert!(buffer.ends_with(TRUNCATION_MARKER));
            assert_eq!(buffer.matches("output truncated").count(), 1);
        }

        #[test]
        fn truncation_respects_character_boundaries() {
            let text = "ą".repeat(MAX_CAPTURED_OUTPUT_BYTES);
            let cut = truncate_captured(&text);
            assert!(cut.ends_with(TRUNCATION_MARKER));
            assert!(cut.len() <= MAX_CAPTURED_OUTPUT_BYTES + TRUNCATION_MARKER.len());
            assert_eq!(truncate_captured("short"), "short");
        }
    }

    mod memory_guard {
        use super::*;

        const PAGE: u64 = 65536;

        #[test]
        fn tracks_memories_and_refuses_growth_past_the_limit() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            runtime
                .execute_script(
                    "setup.js",
                    "globalThis.m = new WebAssembly.Memory({ initial: 2 });",
                )
                .expect("create memory");
            assert_eq!(runtime.tracked_memory_bytes(), Some(2 * PAGE));

            runtime.set_memory_limit(Some(3 * PAGE));
            runtime
                .execute_script("grow.js", "m.grow(1);")
                .expect("growth within the limit");
            let refused = runtime.execute_script("grow.js", "m.grow(1);");
            assert!(refused.is_err(), "growth past the limit throws");
            assert_eq!(runtime.take_memory_limit_denials(), vec![4 * PAGE]);
            assert_eq!(runtime.tracked_memory_bytes(), Some(3 * PAGE));

            runtime.set_memory_limit(None);
            runtime
                .execute_script("grow.js", "m.grow(1);")
                .expect("no limit, no refusal");
        }

        #[test]
        fn shared_memories_are_tracked_and_capped() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            runtime
                .execute_script(
                    "setup.js",
                    "globalThis.s = new WebAssembly.Memory({ initial: 1, maximum: 8, shared: true });",
                )
                .expect("create shared memory");
            assert_eq!(runtime.tracked_memory_bytes(), Some(PAGE));
            runtime.set_memory_limit(Some(2 * PAGE));
            runtime
                .execute_script("grow.js", "s.grow(1);")
                .expect("within the limit");
            assert!(runtime.execute_script("grow.js", "s.grow(1);").is_err());
            assert_eq!(runtime.tracked_memory_bytes(), Some(2 * PAGE));
        }

        #[test]
        fn array_buffers_past_the_allowance_throw_and_are_counted() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            runtime
                .execute_script("baseline.js", "globalThis.keep = new ArrayBuffer(8 << 20);")
                .expect("no allowance outside a call");
            runtime.set_array_buffer_allowance(Some(4 << 20));
            runtime
                .execute_script("small.js", "globalThis.a = new ArrayBuffer(1 << 20);")
                .expect("within the allowance, despite the 8 MiB baseline");
            assert!(runtime
                .execute_script("big.js", "globalThis.b = new ArrayBuffer(16 << 20);")
                .is_err());
            // V8 retries a refused allocation after collecting garbage.
            assert!(runtime.take_array_buffer_denials() >= 1);
            runtime.set_array_buffer_allowance(None);
            runtime
                .execute_script("after.js", "globalThis.c = new ArrayBuffer(16 << 20);")
                .expect("allowance lifted");
        }

        /// A guest module that defines and exports its own memory is tracked,
        /// so growth through the `memory.grow` instruction shows in the total.
        #[test]
        fn exported_memories_of_guest_modules_are_tracked() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            // (module (memory (export "m") 1) (func (export "g") (result i32)
            //   i32.const 3 memory.grow))
            runtime
                .execute_script(
                    "module.js",
                    "const bytes = new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,127,3,2,1,0,\
                     5,3,1,0,1,7,9,2,1,109,2,0,1,103,0,0,10,8,1,6,0,65,3,64,0,11]);\
                     globalThis.inst = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});",
                )
                .expect("instantiate");
            assert_eq!(runtime.tracked_memory_bytes(), Some(PAGE));
            runtime
                .execute_script("grow.js", "inst.exports.g();")
                .expect("grow by instruction");
            assert_eq!(runtime.tracked_memory_bytes(), Some(4 * PAGE));
        }

        /// memory64 modules can't declare more than wasm32's 4 GiB.
        #[test]
        fn memory64_is_capped_at_four_gib() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            // Control: memory64 itself works, so the refusal below is the cap.
            runtime
                .execute_script(
                    "mem64-small.js",
                    "new WebAssembly.Instance(new WebAssembly.Module(\
                     new Uint8Array([0,97,115,109,1,0,0,0,5,3,1,4,1])), {});",
                )
                .expect("a 1-page memory64 instantiates");
            // (module (memory i64 70000)): 70000 pages is past 65536.
            let result = runtime.execute_script(
                "mem64.js",
                "new WebAssembly.Instance(new WebAssembly.Module(\
                 new Uint8Array([0,97,115,109,1,0,0,0,5,5,1,4,240,162,4])), {});",
            );
            assert!(result.is_err(), "a 70000-page memory64 must be refused");
        }

        #[test]
        fn the_guard_cannot_be_replaced() {
            let mut runtime = JsRuntime::new().expect("create runtime");
            runtime.set_memory_limit(Some(PAGE));
            runtime
                .execute_script(
                    "unpatch.js",
                    "WebAssembly.Memory.prototype.grow = function () { return 0; };\n\
                     try { Object.defineProperty(WebAssembly, 'Memory', { value: function () {} }); } catch (e) {}\n\
                     try { delete WebAssembly.Memory.prototype.grow; } catch (e) {}",
                )
                .expect("sloppy-mode writes fail silently");
            assert!(runtime
                .execute_script("create.js", "new WebAssembly.Memory({ initial: 4 });")
                .is_err());
            assert_eq!(runtime.take_memory_limit_denials(), vec![4 * PAGE]);
        }
    }

    mod runtime_local_package_root {
        use super::*;
        use std::fs;
        use std::path::Path;
        use tempfile::tempdir;

        #[test]
        fn sets_local_distribution_root() {
            let temp = tempdir().expect("create tempdir");
            let cache_dir = temp.path().join("cache");
            fs::create_dir(&cache_dir).expect("create cache dir");

            let runtime = JsRuntime::new().expect("create runtime");
            runtime.set_package_root(Some(cache_dir.clone()));
            let resolved = runtime
                .context_state
                .package_root()
                .expect("package root available");
            assert_eq!(resolved, cache_dir);
        }

        #[test]
        fn native_fetch_uses_runtime_local_package_root() {
            let temp = tempdir().expect("create tempdir");
            let first_root = temp.path().join("first");
            let second_root = temp.path().join("second");
            fs::create_dir(&first_root).expect("create first root");
            fs::create_dir(&second_root).expect("create second root");
            let package_name = "numpy-2.4.6-cp314-cp314-pyemscripten_2026_0_wasm32.whl";
            fs::write(first_root.join(package_name), [11_u8]).expect("write first package");
            fs::write(second_root.join(package_name), [22_u8]).expect("write second package");

            assert_runtime_fetches_package_byte(&first_root, package_name, 11);
            assert_runtime_fetches_package_byte(&second_root, package_name, 22);
            assert_runtime_fetches_package_byte(&first_root, package_name, 11);
        }

        fn assert_runtime_fetches_package_byte(root: &Path, package_name: &str, expected: u8) {
            let mut runtime = JsRuntime::new().expect("create runtime");
            runtime.set_package_root(Some(root.to_path_buf()));
            let source = format!(
                r#"
const response = globalThis.__pyRunnerNativeFetch("https://cdn.jsdelivr.net/pyodide/v314.0.7/full/{package_name}");
if (!response || response.status !== 200 || !response.body) {{
  throw new Error("expected native package fetch response");
}}
if (response.body[0] !== {expected}) {{
  throw new Error(`expected package byte {expected}, got ${{response.body[0]}}`);
}}
"#
            );
            runtime
                .execute_script("runtime-package-root-fetch-test.js", &source)
                .expect("runtime-local package fetch should resolve");
        }
    }

    #[test]
    fn pyodide_package_asset_url_matches_pyodide_archives_only() {
        assert!(is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/pyodide/v314.0.7/full/numpy-2.4.6-cp314-cp314-pyemscripten_2026_0_wasm32.whl"
        ));
        assert!(is_pyodide_package_asset_url(
            "https://github.com/pyodide/pyodide/releases/download/314.0.7/pyodide-core-314.0.7.tar.bz2"
        ));
        assert!(!is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/npm/some-package/dist/archive.tar.gz"
        ));
        assert!(!is_pyodide_package_asset_url(
            "https://cdn.jsdelivr.net/gh/example/project/data.zip"
        ));
    }
}
