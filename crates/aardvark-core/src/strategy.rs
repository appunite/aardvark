//! Invocation strategy abstraction allowing custom adapters to participate in execution.

use crate::engine::{ExecutionOutput, JsRuntime};
use crate::error::{PyRunnerError, Result};
use crate::outcome::{ResultPayload, SharedBufferHandle};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use bytes::Bytes;
use serde_json::{self, Value as JsonValue};
use v8;

mod rawctx;

pub(crate) use rawctx::rawctx_spec_json_for_descriptor;
pub use rawctx::{
    RawCtxBindingBuilder, RawCtxInput, RawCtxInvocationStrategy, RawCtxMetadata,
    RawCtxPublishBuilder, RawCtxTableColumnBuilder, RawCtxTableSpec, RawCtxTableSpecBuilder,
};

/// Shared context passed to invocation strategies during a single invocation.
pub struct InvocationContext<'a> {
    session: &'a PySession,
    runtime: &'a mut JsRuntime,
    language: RuntimeLanguage,
}

impl<'a> InvocationContext<'a> {
    pub(crate) fn new(
        session: &'a PySession,
        runtime: &'a mut JsRuntime,
        language: RuntimeLanguage,
    ) -> Self {
        Self {
            session,
            runtime,
            language,
        }
    }

    /// Returns the prepared session, including descriptor metadata.
    pub fn session(&self) -> &PySession {
        self.session
    }

    /// Provides mutable access to the underlying JS runtime for advanced adapters.
    pub fn runtime(&mut self) -> &mut JsRuntime {
        self.runtime
    }

    /// Returns the guest language in use for this invocation.
    pub fn language(&self) -> RuntimeLanguage {
        self.language
    }
}

/// Trait implemented by host-provided invocation adapters.
///
/// Implementations can customise how arguments are materialised, how results
/// are captured, and which guest runtime APIs are exercised during execution.
pub trait PyInvocationStrategy {
    /// Human-readable identifier for telemetry.
    fn name(&self) -> &str {
        "unknown"
    }

    /// Hook executed before any Python code runs, while JS context is active.
    fn pre_execute_js(&mut self, _ctx: &mut InvocationContext<'_>) -> Result<()> {
        Ok(())
    }

    /// Hook executed inside the Python interpreter before the user entrypoint.
    fn pre_execute_py(&mut self, _ctx: &mut InvocationContext<'_>) -> Result<()> {
        Ok(())
    }

    /// Executes the user entrypoint and returns the raw execution output.
    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult>;

    /// Hook executed after the entrypoint inside Python.
    fn post_execute_py(
        &mut self,
        _ctx: &mut InvocationContext<'_>,
        _result: &StrategyResult,
    ) -> Result<()> {
        Ok(())
    }

    /// Hook executed after the entrypoint inside JS.
    fn post_execute_js(
        &mut self,
        _ctx: &mut InvocationContext<'_>,
        _result: &StrategyResult,
    ) -> Result<()> {
        Ok(())
    }
}

/// Simple strategy that executes the entrypoint with no additional hooks.
///
/// For Python handlers it forwards descriptor arguments positionally. When the
/// guest language is JavaScript it invokes the exported function with a single
/// descriptor argument, matching the semantics of `JavaScriptInvocationStrategy`.
#[derive(Default)]
pub struct DefaultInvocationStrategy;

impl PyInvocationStrategy for DefaultInvocationStrategy {
    fn name(&self) -> &str {
        "default"
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        let capture_stdio = ctx.session().descriptor().capture_stdio();
        let execution = ctx
            .runtime()
            .run_python_entrypoint_with_stdio_capture(&entrypoint, capture_stdio)?;
        let payload = if !execution.shared_buffers.is_empty() {
            let buffers = execution
                .shared_buffers
                .iter()
                .map(SharedBufferHandle::from_shared_buffer)
                .collect();
            ResultPayload::SharedBuffers(buffers)
        } else {
            execution
                .result
                .as_ref()
                .map(|text| ResultPayload::Text(text.clone()))
                .unwrap_or(ResultPayload::None)
        };
        Ok(StrategyResult { execution, payload })
    }
}

/// Strategy that marshals inputs/outputs via JSON helpers.
///
/// When targeting Python, the strategy injects a temporary global containing
/// the JSON-decoded payload. For JavaScript the payload is published to the
/// bootstrap so the handler receives a deserialised value via the descriptor.
#[derive(Default)]
pub struct JsonInvocationStrategy {
    input: Option<JsonInput>,
}

/// Input contract for the JSON invocation adapter.
///
/// `Value` preserves the ordinary JSON object/array/scalar contract. The typed
/// variants are for large payloads where the host has already proven a narrower
/// shape and can avoid rebuilding giant generic JSON values on every hot call.
#[derive(Clone, Debug)]
pub enum JsonInput {
    Value(JsonValue),
    F32LeBytes(Bytes),
    Utf8Bytes(Bytes),
    Bytes(Bytes),
    SingleI64Object { key: String, value: i64 },
}

/// Strategy that executes JavaScript module exports.
///
/// The entrypoint must refer to an exported function (`module:export`). The
/// runtime passes the invocation descriptor as the single argument, mirroring
/// the default Cloudflare Workers contract.
#[derive(Default)]
pub struct JavaScriptInvocationStrategy;

impl PyInvocationStrategy for JavaScriptInvocationStrategy {
    fn name(&self) -> &str {
        "javascript"
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        let execution = ctx.runtime().run_js_entrypoint(&entrypoint)?;
        let payload = payload_from_execution(&execution);
        Ok(StrategyResult { execution, payload })
    }
}

impl JsonInvocationStrategy {
    /// Constructs a JSON strategy with optional input payload.
    pub fn new(input: Option<JsonValue>) -> Self {
        Self {
            input: input.map(JsonInput::Value),
        }
    }

    /// Constructs a JSON strategy from a prepared input contract.
    pub fn with_input(input: Option<JsonInput>) -> Self {
        Self { input }
    }

    /// Constructs a JSON strategy with a prepared little-endian f32 input buffer.
    pub fn f32_le_bytes(bytes: impl Into<Bytes>) -> Self {
        Self {
            input: Some(JsonInput::F32LeBytes(bytes.into())),
        }
    }
}

impl PyInvocationStrategy for JsonInvocationStrategy {
    fn name(&self) -> &str {
        "json"
    }

    fn pre_execute_js(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        if ctx.language() != RuntimeLanguage::JavaScript {
            return Ok(());
        }

        let json_string = match &self.input {
            Some(JsonInput::Value(value)) => Some(serde_json::to_string(value).map_err(|err| {
                PyRunnerError::Execution(format!("failed to encode json input: {err}"))
            })?),
            Some(JsonInput::Utf8Bytes(bytes)) | Some(JsonInput::Bytes(bytes)) => Some(
                serde_json::to_string(std::str::from_utf8(bytes).map_err(|err| {
                    PyRunnerError::Execution(format!("JSON bytes input is not valid UTF-8: {err}"))
                })?)
                .map_err(|err| {
                    PyRunnerError::Execution(format!("failed to encode json input: {err}"))
                })?,
            ),
            Some(JsonInput::SingleI64Object { key, value }) => Some(
                serde_json::to_string(&serde_json::json!({key: value})).map_err(|err| {
                    PyRunnerError::Execution(format!("failed to encode json input: {err}"))
                })?,
            ),
            Some(JsonInput::F32LeBytes(_)) => {
                return Err(PyRunnerError::Validation(
                    "f32 JSON side-channel input is only supported for Python handlers".into(),
                ));
            }
            None => None,
        };

        ctx.runtime().with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkJsonInput").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate json input key".into())
            })?;

            let value: v8::Local<v8::Value> = if let Some(ref json) = json_string {
                let json_str = v8::String::new(scope, json).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate json input payload".into())
                })?;
                v8::json::parse(scope, json_str).ok_or_else(|| {
                    PyRunnerError::Execution("failed to parse json input payload".into())
                })?
            } else {
                v8::undefined(scope).into()
            };

            global.set(scope, key.into(), value);
            Ok(())
        })
    }

    fn pre_execute_py(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        if ctx.language() != RuntimeLanguage::Python {
            return Ok(());
        }
        if let Some(ref input) = self.input {
            match input {
                JsonInput::F32LeBytes(bytes) => {
                    ctx.runtime().set_python_json_f32_input(bytes.to_vec())?;
                    return Ok(());
                }
                JsonInput::Utf8Bytes(bytes) => {
                    ctx.runtime().set_python_json_utf8_input(bytes.to_vec())?;
                    return Ok(());
                }
                JsonInput::Bytes(bytes) => {
                    ctx.runtime().set_python_json_bytes_input(bytes.to_vec())?;
                    return Ok(());
                }
                JsonInput::SingleI64Object { key, value } => {
                    ctx.runtime()
                        .set_python_json_single_i64_object_input(key, *value)?;
                    return Ok(());
                }
                JsonInput::Value(value) => {
                    if let Some((key, value)) = json_single_i64_object(value) {
                        ctx.runtime()
                            .set_python_json_single_i64_object_input(key, value)?;
                        return Ok(());
                    }
                    let encoded = serde_json::to_string(value).map_err(|err| {
                        PyRunnerError::Execution(format!("failed to encode json input: {err}"))
                    })?;
                    ctx.runtime().set_python_json_encoded_input(encoded)?;
                }
            }
        }
        Ok(())
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        match ctx.language() {
            RuntimeLanguage::Python => {
                let capture_stdio = ctx.session().descriptor().capture_stdio();
                let mut execution = ctx
                    .runtime()
                    .run_python_json_entrypoint_with_stdio_capture(&entrypoint, capture_stdio)?;
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

fn json_single_i64_object(value: &JsonValue) -> Option<(&str, i64)> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let (key, value) = object.iter().next()?;
    Some((key.as_str(), value.as_i64()?))
}

/// Result produced by a strategy invocation.
pub struct StrategyResult {
    pub execution: ExecutionOutput,
    pub payload: ResultPayload,
}

fn payload_from_execution(execution: &ExecutionOutput) -> ResultPayload {
    if !execution.shared_buffers.is_empty() {
        let buffers = execution
            .shared_buffers
            .iter()
            .map(SharedBufferHandle::from_shared_buffer)
            .collect();
        ResultPayload::SharedBuffers(buffers)
    } else if let Some(json) = execution.json.clone() {
        ResultPayload::Json(json)
    } else if let Some(text) = execution.result.clone() {
        ResultPayload::Text(text)
    } else {
        ResultPayload::None
    }
}
