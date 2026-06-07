use std::convert::TryFrom;
use std::sync::Arc;

use crate::error::{PyRunnerError, Result};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use v8::{self, Function, Local, Object, PinScope, Uint8Array};

use super::{JsRuntime, SharedBuffer, SharedBufferBacking};

#[derive(Debug, Deserialize)]
pub(super) struct PythonCallResult {
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    #[serde(default)]
    pub json: JsonValue,
    #[serde(default)]
    pub json_ready: bool,
    #[serde(default)]
    pub json_side_channel: Option<String>,
    #[serde(default)]
    pub exception_type: Option<String>,
    #[serde(default)]
    pub exception_value: Option<String>,
    #[serde(default)]
    pub traceback: Option<String>,
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

impl ExecutionOutput {
    pub(super) fn success_without_payload() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            exception_type: None,
            exception_value: None,
            traceback: None,
            json: None,
            shared_buffers: Vec::new(),
        }
    }
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
            json: value.json_ready.then_some(value.json),
            shared_buffers: Vec::new(),
        }
    }
}

pub(super) enum JsonSideChannelPayload {
    Json(JsonValue),
    SharedBuffer(SharedBuffer),
}

pub(super) fn populate_execution_output(
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

pub(super) fn take_json_result_side_channel<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
    kind: &str,
) -> Result<JsonSideChannelPayload> {
    let kind_key = v8::String::new(scope, "__aardvarkJsonResultKind").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate JSON result kind key".into())
    })?;
    let value_key = v8::String::new(scope, "__aardvarkJsonResultValue").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate JSON result value key".into())
    })?;
    let clear_key = v8::String::new(scope, "__aardvarkClearJsonResultBuffer").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate JSON result clear key".into())
    })?;

    let payload = match kind {
        "string" => {
            let value = global.get(scope, value_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("JSON result side-channel value missing".into())
            })?;
            let text = value
                .to_string(scope)
                .ok_or_else(|| {
                    PyRunnerError::Execution(
                        "failed to stringify JSON result side-channel value".into(),
                    )
                })?
                .to_rust_string_lossy(scope);
            JsonSideChannelPayload::Json(JsonValue::String(text))
        }
        "f32-array" | "f64-array" | "bytes" => {
            let value = global.get(scope, value_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("JSON result side-channel value missing".into())
            })?;
            let typed_array = Local::<Uint8Array>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("JSON numeric side-channel is not a Uint8Array".into())
            })?;
            let byte_len = typed_array.byte_length();
            let array_buffer = typed_array.buffer(scope).ok_or_else(|| {
                PyRunnerError::Execution("JSON numeric side-channel missing backing store".into())
            })?;
            let backing_store = array_buffer.get_backing_store();
            let offset = typed_array.byte_offset();
            let metadata = json_result_side_channel_metadata(scope, global, kind)?;
            JsonSideChannelPayload::SharedBuffer(SharedBuffer {
                id: "json-result".to_owned(),
                length: byte_len,
                metadata,
                backing: Some(Arc::new(SharedBufferBacking::new(
                    backing_store,
                    offset,
                    byte_len,
                ))),
                bytes: None,
            })
        }
        other => {
            return Err(PyRunnerError::Execution(format!(
                "unsupported JSON result side-channel kind: {other}"
            )))
        }
    };

    if let Some(clear_value) = global.get(scope, clear_key.into()) {
        if let Ok(clear_fn) = Local::<Function>::try_from(clear_value) {
            let _ = clear_fn.call(scope, global.into(), &[]);
        }
    } else {
        let null_value = v8::null(scope);
        global.set(scope, kind_key.into(), null_value.into());
        global.set(scope, value_key.into(), null_value.into());
    }
    Ok(payload)
}

fn json_result_side_channel_metadata<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
    kind: &str,
) -> Result<Option<JsonValue>> {
    let metadata_key = v8::String::new(scope, "__aardvarkJsonResultMetadata").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate JSON result metadata key".into())
    })?;
    let mut metadata = match global.get(scope, metadata_key.into()) {
        Some(value) if !value.is_null_or_undefined() => {
            let json_value = v8::json::stringify(scope, value).ok_or_else(|| {
                PyRunnerError::Execution("failed to stringify JSON result metadata".into())
            })?;
            let json_str = json_value.to_rust_string_lossy(scope);
            serde_json::from_str::<JsonValue>(&json_str).map_err(|err| {
                PyRunnerError::Execution(format!("failed to parse JSON result metadata: {err}"))
            })?
        }
        _ => JsonValue::Object(serde_json::Map::new()),
    };
    if let JsonValue::Object(object) = &mut metadata {
        object.insert(
            "side_channel".to_owned(),
            JsonValue::String(kind.to_owned()),
        );
        object.insert(
            "format".to_owned(),
            JsonValue::String(
                match kind {
                    "f32-array" => "f32_le",
                    "f64-array" => "f64_le",
                    "bytes" => "bytes",
                    _ => kind,
                }
                .to_owned(),
            ),
        );
    }
    Ok(Some(metadata))
}
