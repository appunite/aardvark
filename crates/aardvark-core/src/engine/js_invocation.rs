use std::convert::TryFrom;

use crate::error::{PyRunnerError, Result};
use v8::{self, Function, Promise, PromiseState};

use super::modules::{
    compile_module_from_assets, evaluate_module, instantiate_module, normalize_specifier,
};
use super::output::{populate_execution_output, ExecutionOutput};
use super::shared_buffers::{drain_shared_buffers, SharedBuffer};
use super::snippets::{javascript_value_error_details, js_snippet_wrapper};
use super::JsRuntime;

impl JsRuntime {
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

        self.reset_shared_buffers()?;

        let mut invocation: Option<InvocationResult> = None;

        self.with_context(|scope, _| {
            v8::tc_scope!(let try_catch, scope);
            try_catch.set_verbose(false);
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
            let wrap_key =
                v8::String::new(try_catch, "__aardvarkWrapRawctxFunction").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate RawCtx wrapper key".into())
                })?;
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

                try_catch.reset();
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
                                promise.mark_as_handled();
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
            }
        }

        let shared_buffers = self.with_context(|scope, _| -> Result<Vec<SharedBuffer>> {
            let global = scope.get_current_context().global(scope);
            drain_shared_buffers(scope, global)
        })?;
        execution.shared_buffers = shared_buffers;

        execution.stdout = ctx_state.take_stdout();
        execution.stderr = ctx_state.take_stderr();

        Ok(execution)
    }

    /// Executes an upstream-style Selenium JavaScript snippet in the active Pyodide context.
    pub fn run_js_snippet(&mut self, code: &str) -> Result<ExecutionOutput> {
        enum InvocationResult {
            Immediate(v8::Global<v8::Value>),
            Promise(v8::Global<Promise>),
            Exception {
                typ: String,
                value: String,
                stack: Option<String>,
            },
        }

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

        self.reset_shared_buffers()?;

        let source = js_snippet_wrapper(code);
        let mut invocation: Option<InvocationResult> = None;

        self.with_context(|scope, _| {
            v8::tc_scope!(let try_catch, scope);
            macro_rules! try_catch_details {
                ($default_typ:expr, $default_message:expr) => {{
                    let mut typ = $default_typ.to_string();
                    let mut message = try_catch
                        .exception()
                        .and_then(|value| value.to_string(try_catch))
                        .map(|s| s.to_rust_string_lossy(try_catch))
                        .unwrap_or_else(|| $default_message.to_string());
                    let mut stack: Option<String> = None;
                    if let Some(object) = try_catch
                        .exception()
                        .and_then(|value| value.to_object(try_catch))
                    {
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
                        if let Some(stack_value) = object.get(
                            try_catch,
                            v8::String::new(try_catch, "stack")
                                .ok_or_else(|| {
                                    PyRunnerError::Execution(
                                        "failed to allocate error stack string".into(),
                                    )
                                })?
                                .into(),
                        ) {
                            if let Some(stack_str) = stack_value.to_string(try_catch) {
                                stack = Some(stack_str.to_rust_string_lossy(try_catch));
                            }
                        }
                    }
                    if stack.is_none() {
                        stack = try_catch
                            .stack_trace()
                            .and_then(|value| value.to_string(try_catch))
                            .map(|s| s.to_rust_string_lossy(try_catch));
                    }
                    (typ, message, stack)
                }};
            }

            let global = try_catch.get_current_context().global(try_catch);
            let pyodide = ctx_state
                .pyodide_local(try_catch)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;
            let pyodide_key =
                v8::String::new(try_catch, "__aardvarkCompatPyodide").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate Pyodide snippet key".into())
                })?;
            global.set(try_catch, pyodide_key.into(), pyodide.into());

            let source_value = v8::String::new(try_catch, &source).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JavaScript snippet".into())
            })?;
            let resource_name = v8::String::new(try_catch, "aardvark-compat-run-js.js")
                .ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate JavaScript snippet name".into())
                })?;
            let origin = v8::ScriptOrigin::new(
                try_catch,
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

            let Some(script) = v8::Script::compile(try_catch, source_value, Some(&origin)) else {
                let (typ, value, stack) = try_catch_details!("SyntaxError", "compile failed");
                invocation = Some(InvocationResult::Exception { typ, value, stack });
                return Ok(());
            };

            let Some(value) = script.run(try_catch) else {
                let (typ, value, stack) =
                    try_catch_details!("JavaScriptError", "javascript snippet failed");
                invocation = Some(InvocationResult::Exception { typ, value, stack });
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
            value: "javascript snippet failed".to_string(),
            stack: None,
        });

        let mut execution = ExecutionOutput::success_without_payload();

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
                                let (typ, value, stack) = javascript_value_error_details(
                                    scope,
                                    reason,
                                    "JavaScriptError",
                                    "javascript promise rejected",
                                );
                                Ok(PromiseOutcome::Rejected { typ, value, stack })
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
            }
        }

        execution.shared_buffers = self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            drain_shared_buffers(scope, global)
        })?;
        execution.stdout = ctx_state.take_stdout();
        execution.stderr = ctx_state.take_stderr();
        Ok(execution)
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
