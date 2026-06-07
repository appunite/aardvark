use v8;

use crate::engine::JsRuntime;
use crate::error::{PyRunnerError, Result};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use serde_json::Value as JsonValue;

use super::super::{
    payload_from_execution, InvocationContext, PyInvocationStrategy, StrategyResult,
};
use super::spec::cached_rawctx_spec;
use super::types::RawCtxInput;

const RAWCTX_AUTO_WRAPPER_SNIPPET: &str = include_str!("../../py/rawctx_auto_wrapper.py");

/// Strategy that hydrates RawCtx-style buffers into Python and collects shared-buffer results.
#[derive(Default)]
pub struct RawCtxInvocationStrategy {
    inputs: Vec<RawCtxInput>,
    has_inputs: bool,
}

impl RawCtxInvocationStrategy {
    /// Create a RawCtx strategy with the provided input buffers.
    ///
    /// RawCtx inputs are consumed during execution so owned request buffers can
    /// be transferred into the V8 backing store without an extra copy.
    pub fn new(inputs: Vec<RawCtxInput>) -> Self {
        Self {
            inputs,
            has_inputs: false,
        }
    }

    /// Replace the current inputs with a new set.
    pub fn with_inputs(mut self, inputs: Vec<RawCtxInput>) -> Self {
        self.inputs = inputs;
        self.has_inputs = false;
        self
    }

    fn publish_inputs(
        &mut self,
        runtime: &mut JsRuntime,
        collect_output_metadata: bool,
        flat_input_buffers: bool,
    ) -> Result<()> {
        self.has_inputs = false;
        let inputs = std::mem::take(&mut self.inputs);
        self.has_inputs = !inputs.is_empty();
        publish_rawctx_inputs(runtime, inputs, collect_output_metadata, flat_input_buffers)
    }

    fn install_auto_wrapper(&self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        let Some(spec_json) = cached_rawctx_spec(ctx.session())? else {
            return Ok(());
        };
        if ctx
            .runtime()
            .is_rawctx_auto_wrapper_installed(spec_json.as_str())
        {
            return Ok(());
        }
        let safe_payload = spec_json.replace("'''", "\\'\\'\\'");
        let wrapper = RAWCTX_AUTO_WRAPPER_SNIPPET.replace("{spec_json}", &safe_payload);
        let spec_key = serde_json::to_string(spec_json.as_str()).map_err(|err| {
            PyRunnerError::Execution(format!("failed to serialize rawctx spec key: {err}"))
        })?;
        let mut script = format!(
            "__aardvark_rawctx_spec_key = {spec_key}\nif globals().get('__aardvark_rawctx_installed_spec_key') != __aardvark_rawctx_spec_key:\n"
        );
        for line in wrapper.lines() {
            script.push_str("    ");
            script.push_str(line);
            script.push('\n');
        }
        script.push_str(
            "    globals()['__aardvark_rawctx_installed_spec_key'] = __aardvark_rawctx_spec_key\ndel __aardvark_rawctx_spec_key\n",
        );
        ctx.runtime().run_python_snippet(&script)?;
        ctx.runtime()
            .mark_rawctx_auto_wrapper_installed(spec_json.as_str().to_owned());
        Ok(())
    }

    pub(crate) fn prewarm_python_handler(
        session: &PySession,
        runtime: &mut JsRuntime,
    ) -> Result<()> {
        if cached_rawctx_spec(session)?.is_none() {
            return Ok(());
        }
        let strategy = Self::new(Vec::new());
        let mut ctx = InvocationContext::new(session, runtime, RuntimeLanguage::Python);
        strategy.install_auto_wrapper(&mut ctx)
    }
}

impl PyInvocationStrategy for RawCtxInvocationStrategy {
    fn name(&self) -> &str {
        "rawctx"
    }

    fn pre_execute_js(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        let descriptor = ctx.session().descriptor();
        let collect_output_metadata = descriptor.rawctx_output_metadata();
        let flat_input_buffers = descriptor.rawctx_flat_input_buffers();
        self.publish_inputs(ctx.runtime(), collect_output_metadata, flat_input_buffers)?;
        self.install_js_auto_wrapper(ctx)
    }

    fn pre_execute_py(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        if ctx.language() != RuntimeLanguage::Python {
            return Ok(());
        }
        self.install_auto_wrapper(ctx)
    }

    fn post_execute_py(
        &mut self,
        ctx: &mut InvocationContext<'_>,
        _result: &StrategyResult,
    ) -> Result<()> {
        if ctx.language() == RuntimeLanguage::Python {
            clear_rawctx_inputs(ctx.runtime())?;
        }
        Ok(())
    }

    fn post_execute_js(
        &mut self,
        ctx: &mut InvocationContext<'_>,
        _result: &StrategyResult,
    ) -> Result<()> {
        if ctx.language() == RuntimeLanguage::JavaScript {
            clear_rawctx_inputs(ctx.runtime())?;
        }
        Ok(())
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        match ctx.language() {
            RuntimeLanguage::Python => {
                let capture_stdio = ctx.session().descriptor().capture_stdio();
                let mut execution = if !capture_stdio
                    && ctx
                        .session()
                        .descriptor()
                        .rawctx_shared_buffer_only_success()
                {
                    ctx.runtime()
                        .run_python_rawctx_entrypoint_shared_buffer_only(&entrypoint)?
                } else {
                    ctx.runtime()
                        .run_python_rawctx_entrypoint_with_stdio_capture(
                            &entrypoint,
                            capture_stdio,
                        )?
                };
                if execution.json.is_none() {
                    if let Some(value) = execution
                        .result
                        .as_ref()
                        .and_then(|result| serde_json::from_str::<JsonValue>(result).ok())
                    {
                        execution.json = Some(value);
                    }
                }
                let payload = payload_from_execution(&execution);
                Ok(StrategyResult { execution, payload })
            }
            RuntimeLanguage::JavaScript => {
                let execution = ctx.runtime().run_js_entrypoint(&entrypoint)?;
                let payload = payload_from_execution(&execution);
                Ok(StrategyResult { execution, payload })
            }
        }
    }
}

impl RawCtxInvocationStrategy {
    fn install_js_auto_wrapper(&self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        if ctx.language() != RuntimeLanguage::JavaScript {
            return Ok(());
        }
        let spec_json = cached_rawctx_spec(ctx.session())?;
        let script = if let Some(spec_json) = spec_json {
            format!("globalThis.__aardvarkSetRawctxSpec({spec_json});")
        } else {
            "globalThis.__aardvarkSetRawctxSpec(null);".to_string()
        };
        ctx.runtime()
            .execute_script("__aardvark_rawctx_spec.js", &script)
            .map_err(|err| {
                PyRunnerError::Execution(format!("failed to configure rawctx auto-wrapper: {err}"))
            })
    }
}

fn publish_rawctx_inputs(
    runtime: &mut JsRuntime,
    inputs: Vec<RawCtxInput>,
    collect_output_metadata: bool,
    flat_input_buffers: bool,
) -> Result<()> {
    runtime.with_context(|scope, _| {
        let global = scope.get_current_context().global(scope);
        let has_inputs = !inputs.is_empty();

        if let Some(clear_value) = global.get(
            scope,
            v8::String::new(scope, "__aardvarkClearInputBuffers")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate clear key".into()))?
                .into(),
        ) {
            if let Ok(clear_fn) = v8::Local::<v8::Function>::try_from(clear_value) {
                let _ = clear_fn.call(scope, global.into(), &[]);
            }
        }

        let metadata_mode_key = v8::String::new(scope, "__aardvarkSharedBufferMetadataMode")
            .ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate shared buffer metadata key".into())
            })?;
        let metadata_mode = if collect_output_metadata {
            "full"
        } else {
            "none"
        };
        let metadata_mode_value = v8::String::new(scope, metadata_mode).ok_or_else(|| {
            PyRunnerError::Execution("failed to allocate shared buffer metadata mode".into())
        })?;
        global.set(scope, metadata_mode_key.into(), metadata_mode_value.into());

        let input_mode_key =
            v8::String::new(scope, "__aardvarkRawctxInputViewMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate rawctx input mode key".into())
            })?;
        let input_mode = if flat_input_buffers {
            "flat"
        } else {
            "records"
        };
        let input_mode_value = v8::String::new(scope, input_mode).ok_or_else(|| {
            PyRunnerError::Execution("failed to allocate rawctx input mode value".into())
        })?;
        global.set(scope, input_mode_key.into(), input_mode_value.into());

        let rawctx_available_key = v8::String::new(scope, "__aardvarkRawctxInputsAvailable")
            .ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate rawctx input flag key".into())
            })?;
        let rawctx_available_value: v8::Local<v8::Value> = if has_inputs {
            v8::Boolean::new(scope, true).into()
        } else {
            v8::String::new(scope, "empty")
                .ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate rawctx input flag value".into())
                })?
                .into()
        };
        global.set(scope, rawctx_available_key.into(), rawctx_available_value);

        if inputs.is_empty() {
            return Ok(());
        }

        let register_key = v8::String::new(scope, "__aardvarkRegisterInputBuffer")
            .ok_or_else(|| PyRunnerError::Execution("failed to allocate register key".into()))?;
        let register_value = global.get(scope, register_key.into()).ok_or_else(|| {
            PyRunnerError::Execution("__aardvarkRegisterInputBuffer is not defined".into())
        })?;
        let register_fn = v8::Local::<v8::Function>::try_from(register_value).map_err(|_| {
            PyRunnerError::Execution("__aardvarkRegisterInputBuffer is not a function".into())
        })?;

        for input in inputs {
            let RawCtxInput {
                name,
                buffer,
                metadata,
            } = input;
            let byte_len = buffer.len();
            let name_value = v8::String::new(scope, &name).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate input buffer name".into())
            })?;

            let backing = match buffer.try_into_mut() {
                Ok(bytes_mut) => v8::ArrayBuffer::new_backing_store_from_bytes(Box::new(bytes_mut)),
                Err(buffer) => {
                    let vec = buffer.to_vec();
                    v8::ArrayBuffer::new_backing_store_from_vec(vec)
                }
            };
            let shared = backing.make_shared();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let typed = v8::Uint8Array::new(scope, array_buffer, 0, byte_len).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate Uint8Array for input buffer".into())
            })?;

            let metadata_value: v8::Local<v8::Value> = if let Some(meta) = &metadata {
                let meta_json = meta.to_json_value()?;
                let meta_str = serde_json::to_string(&meta_json).map_err(|err| {
                    PyRunnerError::Execution(format!("failed to serialize rawctx metadata: {err}"))
                })?;
                let meta_js_str = v8::String::new(scope, &meta_str).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate metadata json string".into())
                })?;
                v8::json::parse(scope, meta_js_str).ok_or_else(|| {
                    PyRunnerError::Execution("failed to parse metadata JSON into JS value".into())
                })?
            } else {
                v8::undefined(scope).into()
            };

            register_fn
                .call(
                    scope,
                    global.into(),
                    &[name_value.into(), typed.into(), metadata_value],
                )
                .ok_or_else(|| {
                    PyRunnerError::Execution("registering rawctx input buffer failed".into())
                })?;
        }

        Ok(())
    })
}

fn clear_rawctx_inputs(runtime: &mut JsRuntime) -> Result<()> {
    runtime.with_context(|scope, _| {
        let global = scope.get_current_context().global(scope);
        let clear_key = v8::String::new(scope, "__aardvarkClearInputBuffers")
            .ok_or_else(|| PyRunnerError::Execution("failed to allocate clear key".into()))?;
        if let Some(value) = global.get(scope, clear_key.into()) {
            if let Ok(clear_fn) = v8::Local::<v8::Function>::try_from(value) {
                let _ = clear_fn.call(scope, global.into(), &[]);
            }
        }
        let metadata_mode_key = v8::String::new(scope, "__aardvarkSharedBufferMetadataMode")
            .ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate shared buffer metadata key".into())
            })?;
        let metadata_mode_value = v8::String::new(scope, "full").ok_or_else(|| {
            PyRunnerError::Execution("failed to allocate shared buffer metadata mode".into())
        })?;
        global.set(scope, metadata_mode_key.into(), metadata_mode_value.into());

        let input_mode_key =
            v8::String::new(scope, "__aardvarkRawctxInputViewMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate rawctx input mode key".into())
            })?;
        global.set(scope, input_mode_key.into(), v8::null(scope).into());

        let rawctx_available_key = v8::String::new(scope, "__aardvarkRawctxInputsAvailable")
            .ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate rawctx input flag key".into())
            })?;
        global.set(
            scope,
            rawctx_available_key.into(),
            v8::Boolean::new(scope, false).into(),
        );
        Ok(())
    })
}
