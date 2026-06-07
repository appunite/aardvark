use std::convert::TryFrom;

use crate::error::{PyRunnerError, Result};
use v8::{self, Function, Local, Object, PinScope, Value};

pub(super) fn run_python_entrypoint_script<'a>(
    scope: &mut PinScope<'a, '_>,
    pyodide: Local<'a, Object>,
    script: &str,
) -> Result<Option<String>> {
    v8::tc_scope!(let try_catch, scope);
    let Some(run_key) = v8::String::new(try_catch, "runPython") else {
        return Ok(None);
    };
    let Some(run_value) = pyodide.get(try_catch, run_key.into()) else {
        return Ok(None);
    };
    let Ok(run_fn) = Local::<Function>::try_from(run_value) else {
        return Ok(None);
    };
    let Some(script_value) = v8::String::new(try_catch, script) else {
        return Ok(None);
    };
    let Some(result) = run_fn.call(try_catch, pyodide.into(), &[script_value.into()]) else {
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
        return Err(PyRunnerError::Execution(format!(
            "python entrypoint script failed: {message}"
        )));
    };
    Ok(result
        .to_string(try_catch)
        .map(|string| string.to_rust_string_lossy(try_catch)))
}

pub(super) fn run_python_entrypoint_callable<'a>(
    scope: &mut PinScope<'a, '_>,
    pyodide: Local<'a, Object>,
    global: Local<'a, Object>,
    entrypoint: &str,
    include_text_result: bool,
    capture_stdio: bool,
    prefer_cached: bool,
) -> Result<Option<String>> {
    v8::tc_scope!(let try_catch, scope);
    try_catch.set_verbose(false);
    macro_rules! try_catch_message {
        () => {{
            let message = try_catch
                .exception()
                .and_then(|value| value.to_string(try_catch))
                .map(|s| s.to_rust_string_lossy(try_catch))
                .unwrap_or_else(|| "unknown exception".to_owned());
            try_catch
                .stack_trace()
                .and_then(|value| value.to_string(try_catch))
                .map(|s| format!("{message}\n{}", s.to_rust_string_lossy(try_catch)))
                .unwrap_or(message)
        }};
    }
    macro_rules! call_entrypoint_callable {
        ($callable_fn:expr) => {{
            'call: {
                let Some(entry_value) = v8::String::new(try_catch, entrypoint) else {
                    break 'call Ok(None);
                };
                let include_value = v8::Boolean::new(try_catch, include_text_result);
                let capture_stdio_value = v8::Boolean::new(try_catch, capture_stdio);
                let Some(result) = $callable_fn.call(
                    try_catch,
                    v8::undefined(try_catch).into(),
                    &[
                        entry_value.into(),
                        include_value.into(),
                        capture_stdio_value.into(),
                    ],
                ) else {
                    let message = try_catch_message!();
                    try_catch.reset();
                    break 'call Err(PyRunnerError::Execution(format!(
                        "python entrypoint callable failed: {message}"
                    )));
                };
                break 'call Ok(result
                    .to_string(try_catch)
                    .map(|string| string.to_rust_string_lossy(try_catch)));
            }
        }};
    }

    let cache_key = if prefer_cached {
        let Some(key) = v8::String::new(try_catch, "__aardvarkCallEntrypoint") else {
            return Ok(None);
        };
        Some(key)
    } else {
        None
    };
    if let Some(cache_key) = cache_key {
        if let Some(cached_value) = global.get(try_catch, cache_key.into()) {
            if let Ok(cached_fn) = Local::<Function>::try_from(cached_value) {
                let output: Result<Option<String>> = call_entrypoint_callable!(cached_fn);
                return output;
            }
        }
    }

    let Some(globals_key) = v8::String::new(try_catch, "globals") else {
        return Ok(None);
    };
    let Some(globals_value) = pyodide.get(try_catch, globals_key.into()) else {
        return Ok(None);
    };
    let Ok(globals) = Local::<Object>::try_from(globals_value) else {
        return Ok(None);
    };
    let Some(get_key) = v8::String::new(try_catch, "get") else {
        return Ok(None);
    };
    let Some(get_value) = globals.get(try_catch, get_key.into()) else {
        return Ok(None);
    };
    let Ok(get_fn) = Local::<Function>::try_from(get_value) else {
        return Ok(None);
    };
    let Some(helper_key) = v8::String::new(try_catch, "__aardvark_call_entrypoint") else {
        return Ok(None);
    };
    let Some(callable_value) = get_fn.call(try_catch, globals.into(), &[helper_key.into()]) else {
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
        return Err(PyRunnerError::Execution(format!(
            "python entrypoint helper lookup failed: {message}"
        )));
    };
    let Ok(callable_fn) = Local::<Function>::try_from(callable_value) else {
        return Ok(None);
    };
    let output: Result<Option<String>> = call_entrypoint_callable!(callable_fn);
    if let Some(cache_key) = cache_key {
        global.set(try_catch, cache_key.into(), callable_value);
    } else {
        destroy_pyproxy_value(try_catch, callable_value);
    }
    output
}

pub(super) fn run_python_entrypoint_shared_buffer_only_callable<'a>(
    scope: &mut PinScope<'a, '_>,
    pyodide: Local<'a, Object>,
    global: Local<'a, Object>,
    entrypoint: &str,
) -> Result<Option<String>> {
    v8::tc_scope!(let try_catch, scope);
    try_catch.set_verbose(false);
    macro_rules! try_catch_message {
        () => {{
            let message = try_catch
                .exception()
                .and_then(|value| value.to_string(try_catch))
                .map(|s| s.to_rust_string_lossy(try_catch))
                .unwrap_or_else(|| "unknown exception".to_owned());
            try_catch
                .stack_trace()
                .and_then(|value| value.to_string(try_catch))
                .map(|s| format!("{message}\n{}", s.to_rust_string_lossy(try_catch)))
                .unwrap_or(message)
        }};
    }
    macro_rules! call_entrypoint_callable {
        ($callable_fn:expr) => {{
            'call: {
                let Some(entry_value) = v8::String::new(try_catch, entrypoint) else {
                    break 'call Ok(None);
                };
                let Some(result) = $callable_fn.call(
                    try_catch,
                    v8::undefined(try_catch).into(),
                    &[entry_value.into()],
                ) else {
                    let message = try_catch_message!();
                    try_catch.reset();
                    break 'call Err(PyRunnerError::Execution(format!(
                        "python shared-buffer entrypoint callable failed: {message}"
                    )));
                };
                break 'call Ok(result
                    .to_string(try_catch)
                    .map(|string| string.to_rust_string_lossy(try_catch)));
            }
        }};
    }

    let Some(cache_key) = v8::String::new(try_catch, "__aardvarkCallSharedBufferEntrypoint") else {
        return Ok(None);
    };
    if let Some(cached_value) = global.get(try_catch, cache_key.into()) {
        if let Ok(cached_fn) = Local::<Function>::try_from(cached_value) {
            let output: Result<Option<String>> = call_entrypoint_callable!(cached_fn);
            return output;
        }
    }

    let Some(globals_key) = v8::String::new(try_catch, "globals") else {
        return Ok(None);
    };
    let Some(globals_value) = pyodide.get(try_catch, globals_key.into()) else {
        return Ok(None);
    };
    let Ok(globals) = Local::<Object>::try_from(globals_value) else {
        return Ok(None);
    };
    let Some(get_key) = v8::String::new(try_catch, "get") else {
        return Ok(None);
    };
    let Some(get_value) = globals.get(try_catch, get_key.into()) else {
        return Ok(None);
    };
    let Ok(get_fn) = Local::<Function>::try_from(get_value) else {
        return Ok(None);
    };
    let Some(helper_key) =
        v8::String::new(try_catch, "__aardvark_call_entrypoint_shared_buffer_only")
    else {
        return Ok(None);
    };
    let Some(callable_value) = get_fn.call(try_catch, globals.into(), &[helper_key.into()]) else {
        let message = try_catch_message!();
        return Err(PyRunnerError::Execution(format!(
            "python shared-buffer entrypoint helper lookup failed: {message}"
        )));
    };
    let Ok(callable_fn) = Local::<Function>::try_from(callable_value) else {
        return Ok(None);
    };
    let output: Result<Option<String>> = call_entrypoint_callable!(callable_fn);
    global.set(try_catch, cache_key.into(), callable_value);
    output
}

pub(super) fn python_entrypoint_fallback_script(
    entrypoint: &str,
    include_text_result: bool,
    capture_stdio: bool,
) -> Result<String> {
    let entry_literal = serde_json::to_string(entrypoint)
        .map_err(|err| PyRunnerError::Execution(format!("failed to encode entrypoint: {err}")))?;
    Ok(format!(
        "__aardvark_call_entrypoint({entry_literal}, {include_text_result}, {capture_stdio})",
        include_text_result = if include_text_result { "True" } else { "False" },
        capture_stdio = if capture_stdio { "True" } else { "False" }
    ))
}

pub(super) fn python_entrypoint_shared_buffer_only_fallback_script(
    entrypoint: &str,
) -> Result<String> {
    let entry_literal = serde_json::to_string(entrypoint)
        .map_err(|err| PyRunnerError::Execution(format!("failed to encode entrypoint: {err}")))?;
    Ok(format!(
        "__aardvark_call_entrypoint_shared_buffer_only({entry_literal})"
    ))
}

fn destroy_pyproxy_value<'a>(scope: &mut PinScope<'a, '_>, value: Local<'a, Value>) {
    let Ok(object) = Local::<Object>::try_from(value) else {
        return;
    };
    let Some(destroy_key) = v8::String::new(scope, "destroy") else {
        return;
    };
    let Some(destroy_value) = object.get(scope, destroy_key.into()) else {
        return;
    };
    let Ok(destroy_fn) = Local::<Function>::try_from(destroy_value) else {
        return;
    };
    let _ = destroy_fn.call(scope, object.into(), &[]);
}
