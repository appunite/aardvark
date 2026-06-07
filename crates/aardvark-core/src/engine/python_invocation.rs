use std::convert::TryFrom;
use std::time::Duration;

use crate::error::{PyRunnerError, Result};
use v8::{self, Function, Local, Promise, PromiseState};

use super::bootstrap_sources::{
    PYTHON_JSON_SIDE_CHANNEL_PREFIX, PYTHON_SHARED_BUFFER_ONLY_SUCCESS,
};
use super::output::{
    populate_execution_output, take_json_result_side_channel, ExecutionOutput,
    JsonSideChannelPayload, PythonCallResult,
};
use super::python_entrypoint::{
    python_entrypoint_fallback_script, python_entrypoint_shared_buffer_only_fallback_script,
    run_python_entrypoint_callable, run_python_entrypoint_script,
    run_python_entrypoint_shared_buffer_only_callable,
};
use super::shared_buffers::drain_shared_buffers;
use super::JsRuntime;

#[derive(Copy, Clone, Debug)]
enum PythonEntryBridge {
    Script,
    CachedCallable,
    CachedCallableSharedBufferOnly,
}

impl JsRuntime {
    /// Executes the specified Python module/function entrypoint, capturing stdout and stderr.
    pub fn run_python_entrypoint(&mut self, entrypoint: &str) -> Result<ExecutionOutput> {
        self.run_python_entrypoint_with_stdio_capture(entrypoint, true)
    }

    pub(crate) fn run_python_entrypoint_with_stdio_capture(
        &mut self,
        entrypoint: &str,
        capture_stdio: bool,
    ) -> Result<ExecutionOutput> {
        self.run_python_entrypoint_inner(
            entrypoint,
            true,
            capture_stdio,
            PythonEntryBridge::Script,
            true,
        )
    }

    pub(crate) fn run_python_json_entrypoint_with_stdio_capture(
        &mut self,
        entrypoint: &str,
        capture_stdio: bool,
    ) -> Result<ExecutionOutput> {
        self.run_python_entrypoint_inner(
            entrypoint,
            false,
            capture_stdio,
            PythonEntryBridge::CachedCallable,
            false,
        )
    }

    pub(crate) fn run_python_rawctx_entrypoint_with_stdio_capture(
        &mut self,
        entrypoint: &str,
        capture_stdio: bool,
    ) -> Result<ExecutionOutput> {
        self.run_python_entrypoint_inner(
            entrypoint,
            false,
            capture_stdio,
            PythonEntryBridge::CachedCallable,
            true,
        )
    }

    pub(crate) fn run_python_rawctx_entrypoint_shared_buffer_only(
        &mut self,
        entrypoint: &str,
    ) -> Result<ExecutionOutput> {
        self.run_python_entrypoint_inner(
            entrypoint,
            false,
            false,
            PythonEntryBridge::CachedCallableSharedBufferOnly,
            true,
        )
    }

    fn run_python_entrypoint_inner(
        &mut self,
        entrypoint: &str,
        include_text_result: bool,
        capture_stdio: bool,
        bridge: PythonEntryBridge,
        collect_output_buffers: bool,
    ) -> Result<ExecutionOutput> {
        let ctx_state = self.context_state.clone();
        self.reset_shared_buffers()?;

        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let global = scope.get_current_context().global(scope);
            let request_key =
                v8::String::new(scope, "__pyRunnerEnterRequestContext").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate request context key".into())
                })?;
            if let Some(request_value) = global.get(scope, request_key.into()) {
                if let Ok(request_fn) = Local::<Function>::try_from(request_value) {
                    let _ = request_fn.call(scope, global.into(), &[]);
                }
            }

            let json_str = match bridge {
                PythonEntryBridge::CachedCallable => {
                    if let Some(output) = run_python_entrypoint_callable(
                        scope,
                        pyodide,
                        global,
                        entrypoint,
                        include_text_result,
                        capture_stdio,
                        true,
                    )? {
                        Some(output)
                    } else {
                        let fallback_script = python_entrypoint_fallback_script(
                            entrypoint,
                            include_text_result,
                            capture_stdio,
                        )?;
                        run_python_entrypoint_script(scope, pyodide, &fallback_script)?
                    }
                }
                PythonEntryBridge::Script => {
                    let fallback_script = python_entrypoint_fallback_script(
                        entrypoint,
                        include_text_result,
                        capture_stdio,
                    )?;
                    run_python_entrypoint_script(scope, pyodide, &fallback_script)?
                }
                PythonEntryBridge::CachedCallableSharedBufferOnly => {
                    if let Some(output) = run_python_entrypoint_shared_buffer_only_callable(
                        scope, pyodide, global, entrypoint,
                    )? {
                        Some(output)
                    } else {
                        let fallback_script =
                            python_entrypoint_shared_buffer_only_fallback_script(entrypoint)?;
                        run_python_entrypoint_script(scope, pyodide, &fallback_script)?
                    }
                }
            }
            .ok_or_else(|| PyRunnerError::Execution("running entrypoint failed".into()))?;
            let mut execution =
                if matches!(bridge, PythonEntryBridge::CachedCallableSharedBufferOnly)
                    && json_str == PYTHON_SHARED_BUFFER_ONLY_SUCCESS
                {
                    ExecutionOutput::success_without_payload()
                } else if let Some(kind) = json_str.strip_prefix(PYTHON_JSON_SIDE_CHANNEL_PREFIX) {
                    let mut execution = ExecutionOutput::success_without_payload();
                    match take_json_result_side_channel(scope, global, kind)? {
                        JsonSideChannelPayload::Json(json) => {
                            execution.json = Some(json);
                        }
                        JsonSideChannelPayload::SharedBuffer(buffer) => {
                            execution.shared_buffers.push(buffer);
                        }
                    }
                    execution
                } else {
                    let mut parsed: PythonCallResult =
                        serde_json::from_str(&json_str).map_err(|err| {
                            PyRunnerError::Execution(format!(
                                "failed to parse execution output: {err}"
                            ))
                        })?;
                    let side_channel = if let Some(kind) = parsed.json_side_channel.as_deref() {
                        Some(take_json_result_side_channel(scope, global, kind)?)
                    } else {
                        None
                    };
                    if let Some(JsonSideChannelPayload::Json(json)) = side_channel.as_ref() {
                        parsed.json = json.clone();
                        parsed.json_ready = true;
                    }
                    let mut execution: ExecutionOutput = parsed.into();
                    if let Some(JsonSideChannelPayload::SharedBuffer(buffer)) = side_channel {
                        execution.shared_buffers.push(buffer);
                    }
                    execution
                };
            if collect_output_buffers {
                execution.shared_buffers = drain_shared_buffers(scope, global)?;
            }
            Ok(execution)
        })
    }

    /// Resolves and caches a Python entrypoint without invoking user code.
    pub fn prewarm_python_entrypoint(&mut self, entrypoint: &str) -> Result<()> {
        let entry_literal = serde_json::to_string(entrypoint).map_err(|err| {
            PyRunnerError::Execution(format!("failed to encode entrypoint: {err}"))
        })?;
        self.run_python_snippet(&format!("__aardvark_resolve_entrypoint({entry_literal})"))
    }

    /// Executes an arbitrary Python snippet inside the active Pyodide context.
    pub fn run_python_snippet(&mut self, code: &str) -> Result<()> {
        let ctx_state = self.context_state.clone();
        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let run_key = v8::String::new(scope, "runPython").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate runPython key".into())
            })?;
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

    pub(in crate::engine) fn pump_event_loop(&mut self) -> Result<()> {
        self.isolate.perform_microtask_checkpoint();
        let next_delay_ms = self.with_context(|scope, _| -> Result<Option<u64>> {
            v8::tc_scope!(let try_catch, scope);
            let global = try_catch.get_current_context().global(try_catch);
            let Some(key) = v8::String::new(try_catch, "__pyRunnerPumpTimers") else {
                return Ok(None);
            };
            let Some(value) = global.get(try_catch, key.into()) else {
                return Ok(None);
            };
            let Ok(pump_fn) = Local::<Function>::try_from(value) else {
                return Ok(None);
            };
            let Some(result) = pump_fn.call(try_catch, global.into(), &[]) else {
                let message = try_catch
                    .exception()
                    .and_then(|value| value.to_string(try_catch))
                    .map(|s| s.to_rust_string_lossy(try_catch))
                    .unwrap_or_else(|| "timer pump failed".to_string());
                return Err(PyRunnerError::Execution(message));
            };
            if result.is_null_or_undefined() {
                return Ok(None);
            }
            let Some(delay) = result.number_value(try_catch) else {
                return Ok(None);
            };
            if !delay.is_finite() || delay <= 0.0 {
                return Ok(Some(0));
            }
            Ok(Some(delay.ceil() as u64))
        })?;
        self.isolate.perform_microtask_checkpoint();
        if let Some(delay_ms) = next_delay_ms {
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms.min(10)));
            }
        }
        Ok(())
    }

    /// Executes an arbitrary Python snippet through Pyodide's async API.
    pub fn run_python_async_snippet(&mut self, code: &str) -> Result<ExecutionOutput> {
        enum PromiseOutcome {
            Pending,
            Fulfilled(v8::Global<v8::Value>),
            Rejected {
                typ: String,
                value: String,
                stack: Option<String>,
            },
        }

        let ctx_state = self.context_state.clone();
        ctx_state.clear_console();

        let mut promise_handle: Option<v8::Global<Promise>> = None;
        self.with_context(|scope, _| {
            v8::tc_scope!(let try_catch, scope);
            let pyodide = ctx_state
                .pyodide_local(try_catch)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let run_key = v8::String::new(try_catch, "runPythonAsync").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate runPythonAsync key".into())
            })?;
            let run_value = pyodide.get(try_catch, run_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("pyodide.runPythonAsync is not available".into())
            })?;
            let run_fn = Local::<Function>::try_from(run_value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPythonAsync is not a function".into())
            })?;
            let script_value = v8::String::new(try_catch, code).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate async python snippet".into())
            })?;
            let value = run_fn
                .call(try_catch, pyodide.into(), &[script_value.into()])
                .ok_or_else(|| {
                    let message = try_catch
                        .exception()
                        .and_then(|value| value.to_string(try_catch))
                        .map(|s| s.to_rust_string_lossy(try_catch))
                        .unwrap_or_else(|| "unknown exception".to_owned());
                    let message = try_catch
                        .stack_trace()
                        .and_then(|value| value.to_string(try_catch))
                        .map(|s| format!("{message}\n{}", s.to_rust_string_lossy(try_catch)))
                        .unwrap_or(message);
                    PyRunnerError::Execution(format!("async python snippet failed: {message}"))
                })?;
            let promise = Local::<Promise>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPythonAsync did not return a Promise".into())
            })?;
            promise_handle = Some(v8::Global::new(try_catch, promise));
            Ok(())
        })?;

        let promise_global = promise_handle.ok_or_else(|| {
            PyRunnerError::Execution("missing runPythonAsync promise handle".into())
        })?;

        let mut execution = ExecutionOutput::success_without_payload();
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
                        promise.mark_as_handled();
                        let reason = promise.result(scope);
                        let mut typ = "PythonAsyncError".to_string();
                        let mut message = reason
                            .to_string(scope)
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_else(|| "python async snippet rejected".into());
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
                                            "failed to allocate error message string".into(),
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
                    self.pump_event_loop()?;
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
        execution.shared_buffers = self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            drain_shared_buffers(scope, global)
        })?;
        execution.stdout = ctx_state.take_stdout();
        execution.stderr = ctx_state.take_stderr();
        Ok(execution)
    }
}
