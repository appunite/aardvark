//! Invocation strategy abstraction allowing custom adapters to participate in execution.

use crate::engine::{ExecutionOutput, JsRuntime};
use crate::error::{PyRunnerError, Result};
use crate::invocation::FieldDescriptor;
use crate::outcome::{ResultPayload, SharedBufferHandle};
use crate::runtime_language::RuntimeLanguage;
use crate::session::PySession;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{self, Map as JsonMap, Value as JsonValue};
use v8;

/// Shared context passed to invocation strategies.
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
#[derive(Default)]
pub struct DefaultInvocationStrategy;

impl PyInvocationStrategy for DefaultInvocationStrategy {
    fn name(&self) -> &str {
        "default"
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        let execution = ctx.runtime().run_python_entrypoint(&entrypoint)?;
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
#[derive(Default)]
pub struct JsonInvocationStrategy {
    input: Option<JsonValue>,
}

/// Strategy that executes JavaScript module exports.
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
        Self { input }
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
            Some(value) => Some(serde_json::to_string(value).map_err(|err| {
                PyRunnerError::Execution(format!("failed to encode json input: {err}"))
            })?),
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
        if let Some(ref value) = self.input {
            let encoded = serde_json::to_string(value).map_err(|err| {
                PyRunnerError::Execution(format!("failed to encode json input: {err}"))
            })?;
            let safe = encoded.replace("'''", "\\'\\'\\'");
            let script = format!(
                "import json\n__aardvark_input = json.loads(r'''{safe}''')\n",
                safe = safe
            );
            ctx.runtime().run_python_snippet(&script)?;
        }
        Ok(())
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        match ctx.language() {
            RuntimeLanguage::Python => {
                let mut execution = ctx.runtime().run_python_entrypoint(&entrypoint)?;
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

/// RawCtx input buffer descriptor provided by the host.
#[derive(Clone, Debug)]
pub struct RawCtxMetadata {
    pub dtype: String,
    pub shape: Option<Vec<usize>>,
    pub nullable: Option<bool>,
    pub extra: Option<JsonValue>,
}

impl RawCtxMetadata {
    /// Create metadata with a required dtype.
    pub fn new(dtype: impl Into<String>) -> Self {
        let owned = dtype.into();
        Self {
            dtype: owned.trim().to_owned(),
            shape: None,
            nullable: None,
            extra: None,
        }
    }

    /// Attach an optional shape (e.g. `[rows, cols]`).
    pub fn with_shape(mut self, shape: Vec<usize>) -> Self {
        self.shape = Some(shape);
        self
    }

    /// Mark whether the data is nullable.
    pub fn with_nullable(mut self, nullable: bool) -> Self {
        self.nullable = Some(nullable);
        self
    }

    /// Merge additional metadata fields (must be an object).
    pub fn with_extra(mut self, extra: JsonValue) -> Result<Self> {
        if !extra.is_object() {
            return Err(PyRunnerError::Execution(
                "RawCtx metadata extras must be a JSON object".into(),
            ));
        }
        self.extra = Some(extra);
        Ok(self)
    }

    fn validate(&self) -> Result<()> {
        let dtype = self.dtype.trim();
        if dtype.is_empty() {
            return Err(PyRunnerError::Execution(
                "RawCtx metadata dtype cannot be empty".into(),
            ));
        }
        if let Some(shape) = &self.shape {
            if shape.is_empty() {
                return Err(PyRunnerError::Execution(
                    "RawCtx metadata shape cannot be empty".into(),
                ));
            }
            if shape.contains(&0) {
                return Err(PyRunnerError::Execution(
                    "RawCtx metadata shape dimensions must be positive".into(),
                ));
            }
        }
        if let Some(extra) = &self.extra {
            if !extra.is_object() {
                return Err(PyRunnerError::Execution(
                    "RawCtx metadata extras must be a JSON object".into(),
                ));
            }
        }
        Ok(())
    }

    fn to_json_value(&self) -> Result<JsonValue> {
        self.validate()?;
        let mut map = JsonMap::new();
        map.insert("dtype".to_owned(), JsonValue::String(self.dtype.clone()));
        if let Some(shape) = &self.shape {
            let shape_values = shape.iter().map(|dim| JsonValue::from(*dim)).collect();
            map.insert("shape".to_owned(), JsonValue::Array(shape_values));
        }
        if let Some(nullable) = self.nullable {
            map.insert("nullable".to_owned(), JsonValue::Bool(nullable));
        }
        if let Some(extra) = &self.extra {
            if let Some(obj) = extra.as_object() {
                for (key, value) in obj {
                    if matches!(key.as_str(), "dtype" | "shape" | "nullable") {
                        continue;
                    }
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(JsonValue::Object(map))
    }
}

#[derive(Clone, Debug)]
pub struct RawCtxInput {
    pub name: String,
    pub buffer: Bytes,
    pub metadata: Option<RawCtxMetadata>,
}

impl RawCtxInput {
    /// Construct a new RawCtx input buffer.
    pub fn new(
        name: impl Into<String>,
        buffer: Bytes,
        metadata: Option<RawCtxMetadata>,
    ) -> Result<Self> {
        if let Some(meta) = metadata.as_ref() {
            meta.validate()?;
        }
        Ok(Self {
            name: name.into(),
            buffer,
            metadata,
        })
    }
}

/// Strategy that hydrates RawCtx-style buffers into Python and collects shared-buffer results.
#[derive(Default)]
pub struct RawCtxInvocationStrategy {
    inputs: Vec<RawCtxInput>,
}

impl RawCtxInvocationStrategy {
    /// Create a RawCtx strategy with the provided input buffers.
    pub fn new(inputs: Vec<RawCtxInput>) -> Self {
        Self { inputs }
    }

    /// Replace the current inputs with a new set.
    pub fn with_inputs(mut self, inputs: Vec<RawCtxInput>) -> Self {
        self.inputs = inputs;
        self
    }

    fn publish_inputs(&self, runtime: &mut JsRuntime) -> Result<()> {
        publish_rawctx_inputs(runtime, &self.inputs)
    }

    fn materialize_python_views(&self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        static PRELUDE: &str = r#"
from js import globalThis as _js
import builtins

__aardvark_rawctx_inputs = {}
if hasattr(_js, "__aardvarkInputBuffers"):
    _buffers = _js.__aardvarkInputBuffers.to_py()
    _meta_source = {}
    if hasattr(_js, "__aardvarkInputMetadata"):
        _meta_source = _js.__aardvarkInputMetadata.to_py()
    _view = None
    _buffer = None
    _memory = None
    _meta = None
    for _name, _view in _buffers.items():
        if hasattr(_view, "to_py"):
            _buffer = _view.to_py()
        else:
            _buffer = _view
        try:
            _memory = memoryview(_buffer)
        except TypeError:
            _memory = memoryview(bytearray(_buffer))
        _meta = None
        if isinstance(_meta_source, dict):
            _meta = _meta_source.get(_name)
            if hasattr(_meta, "to_py"):
                _meta = _meta.to_py()
        __aardvark_rawctx_inputs[_name] = {"data": _memory, "metadata": _meta}
builtins.__aardvark_rawctx_inputs = __aardvark_rawctx_inputs
del _js, _buffers, _view, _buffer, _memory, _meta, _meta_source, builtins
"#;
        ctx.runtime().run_python_snippet(PRELUDE)
    }

    fn install_auto_wrapper(&self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        let session = ctx.session();
        let spec = build_rawctx_auto_spec(session)?;
        if let Some(spec) = spec {
            let payload = serde_json::to_string(&spec).map_err(|err| {
                PyRunnerError::Execution(format!(
                    "failed to serialise rawctx auto-wrapper spec: {err}"
                ))
            })?;
            let safe_payload = payload.replace("'''", "\\'\\'\\'");
            let script = format!(
                "{prelude}\n",
                prelude = RAWCTX_AUTO_WRAPPER_SNIPPET.replace("{spec_json}", &safe_payload)
            );
            ctx.runtime().run_python_snippet(&script)?;
        }
        Ok(())
    }
}

impl PyInvocationStrategy for RawCtxInvocationStrategy {
    fn name(&self) -> &str {
        "rawctx"
    }

    fn pre_execute_js(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        self.publish_inputs(ctx.runtime())
    }

    fn pre_execute_py(&mut self, ctx: &mut InvocationContext<'_>) -> Result<()> {
        if ctx.language() != RuntimeLanguage::Python {
            return Ok(());
        }
        self.materialize_python_views(ctx)?;
        self.install_auto_wrapper(ctx)
    }

    fn post_execute_js(
        &mut self,
        ctx: &mut InvocationContext<'_>,
        _result: &StrategyResult,
    ) -> Result<()> {
        clear_rawctx_inputs(ctx.runtime())
    }

    fn invoke(&mut self, ctx: &mut InvocationContext<'_>) -> Result<StrategyResult> {
        let entrypoint = ctx.session().entrypoint().to_owned();
        match ctx.language() {
            RuntimeLanguage::Python => {
                let mut execution = ctx.runtime().run_python_entrypoint(&entrypoint)?;
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

/// Builder that emits invocation-descriptor metadata for RawCtx inputs.
#[derive(Clone, Debug, Default)]
pub struct RawCtxBindingBuilder {
    arg: Option<String>,
    mode: Option<String>,
    decoder: Option<String>,
    options: Option<JsonMap<String, JsonValue>>,
    metadata_arg: Option<String>,
    raw_arg: Option<String>,
    python_loader: Option<String>,
    default: Option<JsonValue>,
    optional: Option<bool>,
    enabled: Option<bool>,
    table: Option<RawCtxTableSpec>,
}

impl RawCtxBindingBuilder {
    /// Create an empty binding builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience constructor for keyword arguments.
    pub fn keyword(arg: impl Into<String>) -> Self {
        Self::new().arg(arg).mode("keyword")
    }

    /// Convenience constructor for positional arguments.
    pub fn positional() -> Self {
        Self::new().mode("positional")
    }

    /// Assign the argument name that should receive the decoded value.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.arg = Some(arg.into());
        self
    }

    /// Override the binding mode (`keyword` or `positional`).
    pub fn mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Configure the decoder to use (`utf8`, `json`, `bytes`, `memoryview`, `float64`, etc.).
    pub fn decoder(mut self, decoder: impl Into<String>) -> Self {
        self.decoder = Some(decoder.into());
        self
    }

    /// Attach decoder-specific options (stored under `options`).
    pub fn option(mut self, key: impl Into<String>, value: JsonValue) -> Self {
        let map = self.options.get_or_insert_with(JsonMap::new);
        map.insert(key.into(), value);
        self
    }

    /// Name the keyword argument that should receive metadata for the buffer.
    pub fn metadata_arg(mut self, name: impl Into<String>) -> Self {
        self.metadata_arg = Some(name.into());
        self
    }

    /// Name the keyword argument that should receive the raw payload record.
    pub fn raw_arg(mut self, name: impl Into<String>) -> Self {
        self.raw_arg = Some(name.into());
        self
    }

    /// Provide a Python loader expression evaluated with `buffer`, `metadata`, and `payload`.
    pub fn python_loader(mut self, expression: impl Into<String>) -> Self {
        self.python_loader = Some(expression.into());
        self
    }

    /// Fallback value used when the payload is missing or the decoder returns `None`.
    pub fn default_value(mut self, value: JsonValue) -> Self {
        self.default = Some(value);
        self
    }

    /// Mark the binding optional (missing payload becomes `None` instead of error).
    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = Some(optional);
        self
    }

    /// Enable or disable the binding.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// Attach a table schema describing a dict-of-columns structure that the shim should build.
    pub fn table(mut self, table: RawCtxTableSpec) -> Self {
        self.table = Some(table);
        self
    }

    /// Serialise the builder into descriptor metadata (`serde_json::Value`).
    pub fn build(self) -> JsonValue {
        wrap_rawctx_metadata(build_binding_metadata(self))
    }

    /// Merge the binding into an existing metadata object (mutating it in place).
    pub fn merge_into(self, metadata: &mut JsonValue) {
        merge_metadata(metadata, self.build());
    }
}

/// Declarative schema describing a tabular payload decoded from a RawCtx buffer.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RawCtxTableSpec {
    #[serde(default)]
    columns: Vec<RawCtxTableColumnSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orient: Option<String>,
}

impl RawCtxTableSpec {
    fn into_json(self) -> JsonValue {
        serde_json::to_value(self).expect("serialize rawctx table spec")
    }

    fn validate(&self) -> Result<()> {
        if self.columns.is_empty() {
            return Err(PyRunnerError::Execution(
                "rawctx table spec requires at least one column".into(),
            ));
        }
        if let Some(orient) = &self.orient {
            let lowered = orient.to_ascii_lowercase();
            if lowered != "records" && lowered != "columns" {
                return Err(PyRunnerError::Execution(format!(
                    "rawctx table orient must be 'records' or 'columns' (got {orient})"
                )));
            }
        }
        for column in &self.columns {
            if column.name.trim().is_empty() {
                return Err(PyRunnerError::Execution(
                    "rawctx table column name cannot be empty".into(),
                ));
            }
            if let Some(options) = &column.options {
                if !options.is_object() {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' options must be a JSON object",
                        column.name
                    )));
                }
            }
            if let Some(dtype) = &column.dtype {
                if dtype.trim().is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' dtype cannot be empty",
                        column.name
                    )));
                }
            }
            if let Some(metadata) = &column.metadata {
                if !metadata.is_object() {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' metadata must be a JSON object",
                        column.name
                    )));
                }
            }
            if let Some(shape) = &column.shape {
                if shape.is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' shape cannot be empty",
                        column.name
                    )));
                }
                if shape.contains(&0) {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' shape dimensions must be positive",
                        column.name
                    )));
                }
            }
            if let Some(manifest) = &column.manifest {
                if !manifest.is_object() {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table column '{}' manifest hints must be a JSON object",
                        column.name
                    )));
                }
            }
            validate_decoder_options(
                column.decoder.as_deref(),
                column.options.as_ref(),
                &format!("rawctx table column '{}'", column.name),
            )?;
        }
        Ok(())
    }

    /// Construct a table specification from a manifest-style JSON object.
    pub fn from_manifest(manifest: &JsonValue) -> Result<Self> {
        build_table_spec_from_manifest(manifest)
    }
}

fn build_table_spec_from_manifest(manifest: &JsonValue) -> Result<RawCtxTableSpec> {
    let manifest_obj = manifest.as_object().ok_or_else(|| {
        PyRunnerError::Execution("rawctx table manifest must be a JSON object".into())
    })?;

    let mut builder = RawCtxTableSpecBuilder::new();
    if let Some(orient_value) = manifest_obj.get("orient") {
        let orient = orient_value.as_str().ok_or_else(|| {
            PyRunnerError::Execution("rawctx table manifest orient must be a string".into())
        })?;
        builder = builder.orient(orient);
    }

    let columns_value = manifest_obj.get("columns").ok_or_else(|| {
        PyRunnerError::Execution("rawctx table manifest requires a 'columns' array".into())
    })?;
    let columns = columns_value.as_array().ok_or_else(|| {
        PyRunnerError::Execution("rawctx table manifest 'columns' must be an array".into())
    })?;
    if columns.is_empty() {
        return Err(PyRunnerError::Execution(
            "rawctx table manifest must describe at least one column".into(),
        ));
    }

    for (index, column_value) in columns.iter().enumerate() {
        let column_obj = column_value.as_object().ok_or_else(|| {
            PyRunnerError::Execution(format!(
                "rawctx table manifest column {index} must be a JSON object"
            ))
        })?;

        let name_value = column_obj
            .get("field")
            .or_else(|| column_obj.get("name"))
            .ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column {index} requires a 'field' or 'name'"
                ))
            })?;
        let name = name_value.as_str().ok_or_else(|| {
            PyRunnerError::Execution(format!(
                "rawctx table manifest column {index} field/name must be a string"
            ))
        })?;
        if name.trim().is_empty() {
            return Err(PyRunnerError::Execution(format!(
                "rawctx table manifest column {index} name cannot be empty"
            )));
        }

        let mut column = RawCtxTableColumnBuilder::new(name);

        if let Some(decoder_value) = column_obj.get("decoder") {
            let decoder = decoder_value.as_str().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' decoder must be a string"
                ))
            })?;
            column = column.decoder(decoder);
        }

        if let Some(options_value) = column_obj.get("options") {
            let options = options_value.as_object().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' options must be a JSON object"
                ))
            })?;
            for (key, value) in options {
                column = column.option(key.clone(), value.clone());
            }
        }

        if let Some(default_value) = column_obj.get("default") {
            column = column.default_value(default_value.clone());
        }

        let optional_flag = match column_obj.get("optional") {
            Some(value) => Some(value.as_bool().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' optional flag must be boolean"
                ))
            })?),
            None => match column_obj.get("required") {
                Some(value) => {
                    let required = value.as_bool().ok_or_else(|| {
                        PyRunnerError::Execution(format!(
                            "rawctx table manifest column '{name}' required flag must be boolean"
                        ))
                    })?;
                    Some(!required)
                }
                None => None,
            },
        };
        if let Some(optional) = optional_flag {
            column = column.optional(optional);
        }

        if let Some(dtype_value) = column_obj.get("dtype") {
            let dtype = dtype_value.as_str().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' dtype must be a string"
                ))
            })?;
            column = column.dtype(dtype);
        }

        if let Some(nullable_value) = column_obj.get("nullable") {
            let nullable = nullable_value.as_bool().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' nullable must be boolean"
                ))
            })?;
            column = column.nullable(nullable);
        }

        if let Some(metadata_value) = column_obj.get("metadata") {
            if !metadata_value.is_object() {
                return Err(PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' metadata must be a JSON object"
                )));
            }
            column = column.schema_metadata(metadata_value.clone());
        }

        if let Some(shape_value) = column_obj.get("shape") {
            let shape = shape_value.as_array().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' shape must be an array"
                ))
            })?;
            if shape.is_empty() {
                return Err(PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' shape cannot be empty"
                )));
            }
            let mut dims = Vec::with_capacity(shape.len());
            for dim in shape {
                let value = dim.as_u64().ok_or_else(|| {
                    PyRunnerError::Execution(format!(
                        "rawctx table manifest column '{name}' shape entries must be positive integers"
                    ))
                })?;
                if value == 0 {
                    return Err(PyRunnerError::Execution(format!(
                        "rawctx table manifest column '{name}' shape entries must be positive"
                    )));
                }
                dims.push(value as usize);
            }
            column = column.shape(dims);
        }

        if let Some(manifest_value) = column_obj.get("manifest") {
            if !manifest_value.is_object() {
                return Err(PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' manifest must be a JSON object"
                )));
            }
            column = column.manifest(manifest_value.clone());
        } else if let Some(source_value) = column_obj
            .get("source")
            .or_else(|| column_obj.get("manifest_column"))
        {
            let source = source_value.as_str().ok_or_else(|| {
                PyRunnerError::Execution(format!(
                    "rawctx table manifest column '{name}' source must be a string"
                ))
            })?;
            column = column.manifest_column(source);
        }

        builder = builder.column(column);
    }

    Ok(builder.build())
}

/// Column descriptor used within a table schema.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct RawCtxTableColumnSpec {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decoder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    optional: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dtype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nullable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manifest: Option<JsonValue>,
}

/// Builder for table schemas assembled in host code.
#[derive(Clone, Debug, Default)]
pub struct RawCtxTableSpecBuilder {
    columns: Vec<RawCtxTableColumnSpec>,
    orient: Option<String>,
}

impl RawCtxTableSpecBuilder {
    /// Begin a new table specification.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the expected orientation (`records`, `columns`, ...). Defaults to `records`.
    pub fn orient(mut self, orient: impl Into<String>) -> Self {
        self.orient = Some(orient.into());
        self
    }

    /// Add a column definition to the schema (chainable).
    pub fn column(mut self, column: RawCtxTableColumnBuilder) -> Self {
        self.columns.push(column.build());
        self
    }

    /// Add a column definition to the schema in builder style (mutable).
    pub fn add_column(&mut self, column: RawCtxTableColumnBuilder) -> &mut Self {
        self.columns.push(column.build());
        self
    }

    /// Finalise the table specification.
    pub fn build(self) -> RawCtxTableSpec {
        let spec = RawCtxTableSpec {
            columns: self.columns,
            orient: self.orient,
        };
        spec.validate().expect("table spec validation");
        spec
    }
}

/// Builder for individual table columns referenced by a table schema.
#[derive(Clone, Debug)]
pub struct RawCtxTableColumnBuilder {
    name: String,
    decoder: Option<String>,
    options: Option<JsonMap<String, JsonValue>>,
    default: Option<JsonValue>,
    optional: Option<bool>,
    dtype: Option<String>,
    nullable: Option<bool>,
    metadata: Option<JsonValue>,
    shape: Option<Vec<usize>>,
    manifest: Option<JsonValue>,
}

impl RawCtxTableColumnBuilder {
    /// Create a column builder for the specified column name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            decoder: None,
            options: None,
            default: None,
            optional: None,
            dtype: None,
            nullable: None,
            metadata: None,
            shape: None,
            manifest: None,
        }
    }

    /// Convenience constructor for UTF-8 string columns.
    pub fn utf8(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("utf8")
            .dtype("string")
            .nullable(false)
    }

    /// Convenience constructor for raw bytes columns.
    pub fn bytes(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("bytes")
            .dtype("bytes")
            .nullable(false)
    }

    /// Convenience constructor for base64-encoded binary columns.
    pub fn base64(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("base64")
            .dtype("bytes")
            .nullable(false)
            .option("as_memoryview", JsonValue::Bool(true))
    }

    /// Convenience constructor for float64 columns.
    pub fn float64(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("float64")
            .dtype("float64")
            .nullable(false)
            .option("struct_format", JsonValue::String("<d".into()))
    }

    /// Convenience constructor for float32 columns.
    pub fn float32(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("float32")
            .dtype("float32")
            .nullable(false)
            .option("struct_format", JsonValue::String("<f".into()))
    }

    /// Convenience constructor for int64 columns.
    pub fn int64(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("int64")
            .dtype("int64")
            .nullable(false)
            .option("byteorder", JsonValue::String("little".into()))
    }

    /// Convenience constructor for int32 columns.
    pub fn int32(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("int32")
            .dtype("int32")
            .nullable(false)
            .option("struct_format", JsonValue::String("<i".into()))
    }

    /// Convenience constructor for boolean columns.
    pub fn boolean(name: impl Into<String>) -> Self {
        Self::new(name)
            .decoder("bool")
            .dtype("bool")
            .nullable(false)
            .option("byteorder", JsonValue::String("little".into()))
    }

    /// Override the column decoder used when normalising values (`json`, `utf8`, ...).
    pub fn decoder(mut self, decoder: impl Into<String>) -> Self {
        self.decoder = Some(decoder.into());
        self
    }

    /// Attach decoder-specific options (stored under `options`).
    pub fn option(mut self, key: impl Into<String>, value: JsonValue) -> Self {
        let map = self.options.get_or_insert_with(JsonMap::new);
        map.insert(key.into(), value);
        self
    }

    /// Provide a fallback value used when column data is missing.
    pub fn default_value(mut self, value: JsonValue) -> Self {
        self.default = Some(value);
        self
    }

    /// Mark the column optional (missing entries use `default` or `None`).
    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = Some(optional);
        self
    }

    /// Declare the logical dtype associated with the column (e.g. `string`, `float64`).
    pub fn dtype(mut self, dtype: impl Into<String>) -> Self {
        self.dtype = Some(dtype.into());
        self
    }

    /// Indicate whether the column permits null values.
    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = Some(nullable);
        self
    }

    /// Attach additional schema metadata to the column.
    pub fn schema_metadata(mut self, metadata: JsonValue) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Record an expected shape for vector-valued columns.
    pub fn shape<I>(mut self, shape: I) -> Self
    where
        I: Into<Vec<usize>>,
    {
        self.shape = Some(shape.into());
        self
    }

    /// Associate manifest-derived hints with the column (e.g. upstream dataset name).
    pub fn manifest(mut self, manifest: JsonValue) -> Self {
        self.manifest = Some(manifest);
        self
    }

    /// Convenience helper for binding a manifest column name.
    pub fn manifest_column(mut self, column: impl Into<String>) -> Self {
        let mut map = JsonMap::new();
        map.insert("column".into(), JsonValue::String(column.into()));
        self.manifest = Some(JsonValue::Object(map));
        self
    }

    fn build(self) -> RawCtxTableColumnSpec {
        RawCtxTableColumnSpec {
            name: self.name,
            decoder: self.decoder,
            options: self.options.map(JsonValue::Object),
            default: self.default,
            optional: self.optional,
            dtype: self.dtype,
            nullable: self.nullable,
            metadata: self.metadata,
            shape: self.shape,
            manifest: self.manifest,
        }
    }
}

/// Builder that emits descriptor metadata for RawCtx outputs / shared buffers.
#[derive(Clone, Debug)]
pub struct RawCtxPublishBuilder {
    id: String,
    mode: Option<String>,
    transform: Option<String>,
    metadata: Option<JsonValue>,
    python_transform: Option<String>,
    return_behavior: Option<String>,
    when_none: Option<String>,
    encoding: Option<String>,
    enabled: Option<bool>,
}

impl RawCtxPublishBuilder {
    /// Create a publish builder for the provided shared-buffer identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            mode: None,
            transform: None,
            metadata: None,
            python_transform: None,
            return_behavior: None,
            when_none: None,
            encoding: None,
            enabled: None,
        }
    }

    /// Override the publish mode (defaults to `publish-buffer`).
    pub fn mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Select the transform applied to the return value before publishing.
    pub fn transform(mut self, transform: impl Into<String>) -> Self {
        self.transform = Some(transform.into());
        self
    }

    /// Attach metadata emitted alongside the shared buffer.
    pub fn metadata(mut self, metadata: JsonValue) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Provide a Python expression executed to transform the result (`result` in scope).
    pub fn python_transform(mut self, expression: impl Into<String>) -> Self {
        self.python_transform = Some(expression.into());
        self
    }

    /// Control what the wrapper returns (`none`, `original`, `buffer`).
    pub fn return_behavior(mut self, behaviour: impl Into<String>) -> Self {
        self.return_behavior = Some(behaviour.into());
        self
    }

    /// Specify behaviour when the user returns `None` (`skip`, `error`, `publish-empty`, `propagate`).
    pub fn when_none(mut self, mode: impl Into<String>) -> Self {
        self.when_none = Some(mode.into());
        self
    }

    /// Provide an encoding hint when using the UTF-8 transform.
    pub fn encoding(mut self, encoding: impl Into<String>) -> Self {
        self.encoding = Some(encoding.into());
        self
    }

    /// Enable/disable the publish binding.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// Serialise the builder into descriptor metadata (`serde_json::Value`).
    pub fn build(self) -> JsonValue {
        wrap_rawctx_metadata(build_publish_metadata(self))
    }

    /// Merge the publish metadata into an existing descriptor metadata object.
    pub fn merge_into(self, metadata: &mut JsonValue) {
        merge_metadata(metadata, self.build());
    }
}

fn build_binding_metadata(builder: RawCtxBindingBuilder) -> JsonMap<String, JsonValue> {
    let mut rawctx = JsonMap::new();
    if let Some(enabled) = builder.enabled {
        rawctx.insert("enabled".into(), JsonValue::Bool(enabled));
    }
    let mut binding = JsonMap::new();
    if let Some(arg) = builder.arg {
        binding.insert("arg".into(), JsonValue::String(arg));
    }
    if let Some(mode) = builder.mode {
        binding.insert("mode".into(), JsonValue::String(mode));
    }
    if let Some(decoder) = builder.decoder {
        binding.insert("decoder".into(), JsonValue::String(decoder));
    }
    if let Some(options) = builder.options {
        binding.insert("options".into(), JsonValue::Object(options));
    }
    if let Some(name) = builder.metadata_arg {
        binding.insert("metadata_arg".into(), JsonValue::String(name));
    }
    if let Some(name) = builder.raw_arg {
        binding.insert("raw_arg".into(), JsonValue::String(name));
    }
    if let Some(loader) = builder.python_loader {
        binding.insert("python_loader".into(), JsonValue::String(loader));
    }
    if let Some(default) = builder.default {
        binding.insert("default".into(), default);
    }
    if let Some(optional) = builder.optional {
        binding.insert("optional".into(), JsonValue::Bool(optional));
    }
    if let Some(table) = builder.table {
        table.validate().expect("rawctx table spec must be valid");
        binding.insert("table".into(), table.into_json());
    }
    if !binding.is_empty() {
        rawctx.insert("binding".into(), JsonValue::Object(binding));
    }
    rawctx
}

fn build_publish_metadata(builder: RawCtxPublishBuilder) -> JsonMap<String, JsonValue> {
    let mut rawctx = JsonMap::new();
    if let Some(enabled) = builder.enabled {
        rawctx.insert("enabled".into(), JsonValue::Bool(enabled));
    }
    let mut publish = JsonMap::new();
    publish.insert("id".into(), JsonValue::String(builder.id));
    if let Some(mode) = builder.mode {
        publish.insert("mode".into(), JsonValue::String(mode));
    }
    if let Some(transform) = builder.transform {
        publish.insert("transform".into(), JsonValue::String(transform));
    }
    if let Some(metadata) = builder.metadata {
        publish.insert("metadata".into(), metadata);
    }
    if let Some(expression) = builder.python_transform {
        publish.insert("python_transform".into(), JsonValue::String(expression));
    }
    if let Some(behaviour) = builder.return_behavior {
        publish.insert("return".into(), JsonValue::String(behaviour));
    }
    if let Some(mode) = builder.when_none {
        publish.insert("when_none".into(), JsonValue::String(mode));
    }
    if let Some(encoding) = builder.encoding {
        publish.insert("encoding".into(), JsonValue::String(encoding));
    }
    rawctx.insert("publish".into(), JsonValue::Object(publish));
    rawctx
}

fn wrap_rawctx_metadata(rawctx: JsonMap<String, JsonValue>) -> JsonValue {
    let mut inner = JsonMap::new();
    inner.insert("rawctx".into(), JsonValue::Object(rawctx));
    let mut outer = JsonMap::new();
    outer.insert("aardvark".into(), JsonValue::Object(inner));
    JsonValue::Object(outer)
}

fn merge_metadata(target: &mut JsonValue, incoming: JsonValue) {
    match (target, incoming) {
        (JsonValue::Object(target_map), JsonValue::Object(incoming_map)) => {
            for (key, value) in incoming_map.into_iter() {
                match target_map.get_mut(&key) {
                    Some(existing) => merge_metadata(existing, value),
                    None => {
                        target_map.insert(key, value);
                    }
                }
            }
        }
        (target_value, incoming_value) => {
            *target_value = incoming_value;
        }
    }
}

#[derive(Debug, Serialize)]
struct RawCtxAutoSpec {
    entrypoint: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    inputs: Vec<RawCtxInputBindingSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<RawCtxOutputSpec>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    outputs: Vec<RawCtxOutputSpec>,
}

#[derive(Debug, Serialize)]
struct RawCtxInputBindingSpec {
    field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python_loader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    optional: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    table: Option<RawCtxTableSpec>,
}

#[derive(Clone, Debug, Serialize)]
struct RawCtxOutputSpec {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python_transform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_behavior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_none: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
}

fn build_rawctx_auto_spec(session: &PySession) -> Result<Option<RawCtxAutoSpec>> {
    let descriptor = session.descriptor();
    let entrypoint = descriptor.entrypoint();
    if !entrypoint.contains(':') {
        return Ok(None);
    }

    let mut inputs = Vec::new();
    for field in &descriptor.inputs {
        if let Some(binding) = parse_input_binding(field)? {
            inputs.push(binding);
        }
    }

    let mut outputs = Vec::new();
    for field in &descriptor.outputs {
        if let Some(output) = parse_output_binding(field)? {
            outputs.push(output);
        }
    }

    let primary_output = outputs.first().cloned();

    if inputs.is_empty() && outputs.is_empty() {
        return Ok(None);
    }

    Ok(Some(RawCtxAutoSpec {
        entrypoint: entrypoint.to_owned(),
        inputs,
        output: primary_output,
        outputs,
    }))
}

fn parse_table_spec(value: &JsonValue) -> Result<RawCtxTableSpec> {
    let spec: RawCtxTableSpec = serde_json::from_value(value.clone())
        .map_err(|err| PyRunnerError::Execution(format!("invalid rawctx table spec: {err}")))?;
    spec.validate()?;
    Ok(spec)
}

fn parse_input_binding(field: &FieldDescriptor) -> Result<Option<RawCtxInputBindingSpec>> {
    let metadata = match &field.metadata {
        Some(value) => value,
        None => return Ok(None),
    };
    let rawctx = match extract_rawctx_metadata(metadata) {
        Some(value) => value,
        None => return Ok(None),
    };

    if matches!(
        rawctx
            .get("mode")
            .and_then(|value| value.as_str())
            .map(|mode| mode.eq_ignore_ascii_case("manual")
                || mode.eq_ignore_ascii_case("skip")
                || mode.eq_ignore_ascii_case("disabled")),
        Some(true)
    ) {
        return Ok(None);
    }

    let enabled = rawctx
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Ok(None);
    }

    let binding = if let Some(value) = rawctx.get("binding") {
        value.as_object().ok_or_else(|| {
            PyRunnerError::Execution("rawctx binding metadata must be a JSON object".into())
        })?
    } else {
        rawctx
    };

    let arg = match binding.get("arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx binding arg must be a string when provided".into())
            })?;
            Some(string.to_owned())
        }
        None => None,
    };
    let arg = arg.or_else(|| Some(field.name.clone()));

    let mode = binding
        .get("mode")
        .map(|value| {
            let mode = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx binding mode must be a string".into())
            })?;
            let lowered = mode.to_ascii_lowercase();
            if lowered != "keyword" && lowered != "positional" {
                return Err(PyRunnerError::Execution(format!(
                    "unsupported rawctx binding mode '{mode}' (expected 'keyword' or 'positional')"
                )));
            }
            Ok(lowered)
        })
        .transpose()?;

    if mode.as_deref() == Some("positional") && arg.is_none() {
        return Err(PyRunnerError::Execution(
            "rawctx binding cannot be positional without an argument name".into(),
        ));
    }

    let decoder = binding
        .get("decoder")
        .map(|value| {
            value
                .as_str()
                .map(|s| s.to_owned())
                .ok_or_else(|| PyRunnerError::Execution("rawctx decoder must be a string".into()))
        })
        .transpose()?;

    let options = match binding.get("options") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            if value.is_object() {
                Some(value.clone())
            } else {
                return Err(PyRunnerError::Execution(
                    "rawctx binding options must be a JSON object".into(),
                ));
            }
        }
        None => None,
    };

    validate_decoder_options(
        decoder.as_deref(),
        options.as_ref(),
        &format!("rawctx binding '{}'", field.name),
    )?;

    let metadata_arg = match binding.get("metadata_arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution(
                    "rawctx metadata_arg must be a string when provided".into(),
                )
            })?;
            Some(string.to_owned())
        }
        None => None,
    };

    let raw_arg = match binding.get("raw_arg") {
        Some(value) if value.is_null() => None,
        Some(value) => {
            let string = value.as_str().ok_or_else(|| {
                PyRunnerError::Execution("rawctx raw_arg must be a string when provided".into())
            })?;
            Some(string.to_owned())
        }
        None => None,
    };

    let python_loader = binding
        .get("python_loader")
        .map(|value| {
            value.as_str().map(|s| s.to_owned()).ok_or_else(|| {
                PyRunnerError::Execution("rawctx python_loader must be a string".into())
            })
        })
        .transpose()?;

    let default = binding.get("default").cloned();

    let optional = binding
        .get("optional")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| PyRunnerError::Execution("rawctx optional must be a boolean".into()))
        })
        .transpose()?;

    let mut table = match binding.get("table") {
        Some(value) => Some(parse_table_spec(value)?),
        None => None,
    };

    if table.is_none() {
        if let Some(manifest_value) = binding
            .get("table_manifest")
            .or_else(|| rawctx.get("table_manifest"))
        {
            table = Some(RawCtxTableSpec::from_manifest(manifest_value)?);
        }
    }

    if arg.is_none()
        && metadata_arg.is_none()
        && raw_arg.is_none()
        && python_loader.is_none()
        && default.is_none()
        && table.is_none()
    {
        return Err(PyRunnerError::Execution(
            "rawctx binding must project at least one argument or provide a custom loader/default"
                .into(),
        ));
    }

    Ok(Some(RawCtxInputBindingSpec {
        field: field.name.clone(),
        arg,
        mode,
        decoder,
        options,
        metadata_arg,
        raw_arg,
        python_loader,
        default,
        optional,
        table,
    }))
}

fn parse_output_binding(field: &FieldDescriptor) -> Result<Option<RawCtxOutputSpec>> {
    let metadata = match &field.metadata {
        Some(value) => value,
        None => return Ok(None),
    };
    let rawctx = match extract_rawctx_metadata(metadata) {
        Some(value) => value,
        None => return Ok(None),
    };

    if matches!(
        rawctx
            .get("mode")
            .and_then(|value| value.as_str())
            .map(|mode| mode.eq_ignore_ascii_case("manual")
                || mode.eq_ignore_ascii_case("skip")
                || mode.eq_ignore_ascii_case("disabled")),
        Some(true)
    ) {
        return Ok(None);
    }

    let publish_obj = if let Some(value) = rawctx.get("publish") {
        value.as_object().ok_or_else(|| {
            PyRunnerError::Execution("rawctx publish metadata must be a JSON object".into())
        })?
    } else {
        rawctx
    };

    let enabled = publish_obj
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Ok(None);
    }

    let mode = publish_obj
        .get("mode")
        .and_then(|value| value.as_str())
        .map(|mode| mode.to_ascii_lowercase());

    let mode_value = mode.as_deref().unwrap_or("publish-buffer");
    if mode_value != "publish-buffer" {
        return Err(PyRunnerError::Execution(format!(
            "unsupported rawctx output mode '{mode_value}'"
        )));
    }

    let id = publish_obj
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            PyRunnerError::Execution("rawctx output publish requires an 'id' field".into())
        })?
        .to_owned();

    let transform = publish_obj
        .get("transform")
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref transform_value) = transform {
        let supported = ["memoryview", "bytes", "utf8", "identity"];
        if !supported
            .iter()
            .any(|item| item.eq_ignore_ascii_case(transform_value))
        {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx output transform '{transform_value}'"
            )));
        }
    }

    let python_transform = publish_obj
        .get("python_transform")
        .and_then(|value| value.as_str())
        .map(|value| value.to_owned());

    let return_behavior = publish_obj
        .get("return")
        .or_else(|| publish_obj.get("return_behavior"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref behaviour) = return_behavior {
        let supported = ["none", "original", "buffer"];
        if !supported
            .iter()
            .any(|item| item.eq_ignore_ascii_case(behaviour))
        {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx return behaviour '{behaviour}'"
            )));
        }
    }

    let when_none = publish_obj
        .get("when_none")
        .and_then(|value| value.as_str())
        .map(|value| value.to_ascii_lowercase());

    if let Some(ref mode) = when_none {
        let supported = ["skip", "error", "publish-empty", "propagate"];
        if !supported.iter().any(|item| item.eq_ignore_ascii_case(mode)) {
            return Err(PyRunnerError::Execution(format!(
                "unsupported rawctx when_none behaviour '{mode}'"
            )));
        }
    }

    let encoding = publish_obj
        .get("encoding")
        .and_then(|value| value.as_str())
        .map(|value| value.to_owned());

    Ok(Some(RawCtxOutputSpec {
        id,
        mode: mode.filter(|m| m != "publish-buffer"),
        transform,
        metadata: publish_obj.get("metadata").cloned(),
        python_transform,
        return_behavior,
        when_none,
        encoding,
    }))
}

fn extract_rawctx_metadata(value: &JsonValue) -> Option<&serde_json::Map<String, JsonValue>> {
    let object = value.as_object()?;
    if let Some(aardvark) = object.get("aardvark").and_then(|value| value.as_object()) {
        if let Some(rawctx) = aardvark.get("rawctx").and_then(|value| value.as_object()) {
            return Some(rawctx);
        }
    }
    object.get("rawctx").and_then(|value| value.as_object())
}

fn validate_decoder_options(
    decoder: Option<&str>,
    options: Option<&JsonValue>,
    context: &str,
) -> Result<()> {
    let Some(raw_decoder) = decoder else {
        if let Some(value) = options {
            if !value.is_null() && !value.is_object() {
                return Err(PyRunnerError::Execution(format!(
                    "{context} decoder options must be a JSON object"
                )));
            }
        }
        return Ok(());
    };

    let trimmed = raw_decoder.trim();
    if trimmed.is_empty() {
        return Err(PyRunnerError::Execution(format!(
            "{context} decoder cannot be empty"
        )));
    }

    let decoder = trimmed.to_ascii_lowercase();
    let Some(options_value) = options else {
        return Ok(());
    };

    let object = options_value.as_object().ok_or_else(|| {
        PyRunnerError::Execution(format!("{context} decoder options must be a JSON object"))
    })?;

    if object.is_empty() {
        return Ok(());
    }

    match decoder.as_str() {
        "utf8" | "string" | "json" => {
            if let Some(value) = object.get("encoding") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'encoding' must be a string"
                    )));
                };
                if string.trim().is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'encoding' cannot be empty"
                    )));
                }
            }
            if let Some(value) = object.get("errors") {
                if value.as_str().is_none() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'errors' must be a string"
                    )));
                }
            }
        }
        "float32" | "f32" | "float64" | "f64" | "int32" | "i32" | "uint32" | "u32" => {
            if let Some(value) = object.get("struct_format") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' must be a string"
                    )));
                };
                let trimmed = string.trim();
                if trimmed.is_empty() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' cannot be empty"
                    )));
                }

                let expected = match decoder.as_str() {
                    "float32" | "f32" => 'f',
                    "float64" | "f64" => 'd',
                    "int32" | "i32" => 'i',
                    "uint32" | "u32" => 'I',
                    other => {
                        debug_assert!(matches!(
                            other,
                            "float32"
                                | "f32"
                                | "float64"
                                | "f64"
                                | "int32"
                                | "i32"
                                | "uint32"
                                | "u32"
                        ));
                        'f'
                    }
                };
                let type_char = trimmed.chars().last().unwrap();
                if !type_char.eq_ignore_ascii_case(&expected) {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'struct_format' must end with '{}'",
                        expected
                    )));
                }

                if trimmed.len() > type_char.len_utf8() {
                    let prefix = &trimmed[..trimmed.len() - type_char.len_utf8()];
                    if !prefix.is_empty() {
                        let mut chars = prefix.chars();
                        let first = chars.next().unwrap();
                        let allowed = ['<', '>', '!', '=', '@'];
                        if allowed.contains(&first) {
                            if chars.any(|c| !c.is_ascii_digit()) {
                                return Err(PyRunnerError::Execution(format!(
                                    "{context} decoder option 'struct_format' prefix must contain only digits after the byteorder flag"
                                )));
                            }
                        } else if !first.is_ascii_digit() || chars.any(|c| !c.is_ascii_digit()) {
                            return Err(PyRunnerError::Execution(format!(
                                "{context} decoder option 'struct_format' prefix must be digits or a byteorder flag"
                            )));
                        }
                    }
                }
            }
        }
        "int64" | "i64" => {
            if let Some(value) = object.get("byteorder") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be a string"
                    )));
                };
                let lowered = string.trim().to_ascii_lowercase();
                if lowered != "little" && lowered != "big" {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be 'little' or 'big'"
                    )));
                }
            }
            if let Some(value) = object.get("signed") {
                if !value.is_boolean() {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'signed' must be a boolean"
                    )));
                }
            }
        }
        "bool" | "boolean" => {
            if let Some(value) = object.get("byteorder") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be a string"
                    )));
                };
                let lowered = string.trim().to_ascii_lowercase();
                if lowered != "little" && lowered != "big" {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'byteorder' must be 'little' or 'big'"
                    )));
                }
            }
        }
        "base64" | "b64" => {
            if let Some(value) = object.get("altchars") {
                let Some(string) = value.as_str() else {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'altchars' must be a string"
                    )));
                };
                if string.chars().count() != 2 {
                    return Err(PyRunnerError::Execution(format!(
                        "{context} decoder option 'altchars' must contain exactly two characters"
                    )));
                }
            }
            for key in ["validate", "as_memoryview", "as_bytearray"] {
                if let Some(value) = object.get(key) {
                    if !value.is_boolean() {
                        return Err(PyRunnerError::Execution(format!(
                            "{context} decoder option '{key}' must be a boolean"
                        )));
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

const RAWCTX_AUTO_WRAPPER_SNIPPET: &str = r#"
import builtins, importlib, json

__aardvark_rawctx_spec = json.loads(r'''{spec_json}''')

def __aardvark__decode_rawctx(binding, payload):
    value, metadata, raw_payload = __aardvark__decode_scalar(binding, payload)
    table_spec = binding.get("table")
    if table_spec:
        table_value, table_metadata = __aardvark__materialize_table(table_spec, value)
        value = table_value
        if table_metadata is not None:
            if metadata is None:
                metadata = table_metadata
            elif isinstance(metadata, dict) and isinstance(table_metadata, dict):
                merged = dict(metadata)
                merged.update(table_metadata)
                metadata = merged
            else:
                metadata = table_metadata
    return value, metadata, raw_payload


def __aardvark__decode_scalar(binding, payload):
    if payload is None:
        return None, None, None
    data = payload.get("data")
    metadata = payload.get("metadata")
    raw_payload = payload
    if data is None:
        return None, metadata, raw_payload
    if binding.get("python_loader"):
        namespace = {
            "buffer": data,
            "metadata": metadata,
            "payload": raw_payload,
            "memoryview": memoryview,
        }
        return eval(binding["python_loader"], {}, namespace), metadata, raw_payload
    decoder = binding.get("decoder") or "memoryview"
    options = binding.get("options") or {}
    if decoder in ("memoryview", None):
        value = data
    elif decoder == "bytes":
        value = data.tobytes()
    elif decoder in ("utf8", "string"):
        encoding = options.get("encoding", "utf-8")
        errors = options.get("errors", "strict")
        value = data.tobytes().decode(encoding, errors)
    elif decoder in ("float32", "f32"):
        import struct as _struct
        fmt = options.get("struct_format", "<f")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("float64", "f64"):
        import struct as _struct
        fmt = options.get("struct_format", "<d")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("int32", "i32"):
        import struct as _struct
        fmt = options.get("struct_format", "<i")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("uint32", "u32"):
        import struct as _struct
        fmt = options.get("struct_format", "<I")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("int64", "i64"):
        byteorder = options.get("byteorder", "little")
        signed = bool(options.get("signed", True))
        value = int.from_bytes(data.tobytes(), byteorder=byteorder, signed=signed)
    elif decoder in ("bool", "boolean"):
        byteorder = options.get("byteorder", "little")
        value = bool(int.from_bytes(data.tobytes(), byteorder=byteorder, signed=False))
    elif decoder == "json":
        import json as _json
        encoding = options.get("encoding", "utf-8")
        errors = options.get("errors", "strict")
        value = _json.loads(data.tobytes().decode(encoding, errors))
    elif decoder in ("base64", "b64"):
        import base64 as _base64
        raw_bytes = data.tobytes()
        altchars = options.get("altchars")
        if altchars is not None and not isinstance(altchars, (bytes, bytearray)):
            altchars = str(altchars).encode()
        validate = bool(options.get("validate", False))
        decoded = _base64.b64decode(raw_bytes, altchars=altchars, validate=validate)
        if options.get("as_memoryview"):
            value = memoryview(decoded)
        elif options.get("as_bytearray"):
            value = bytearray(decoded)
        else:
            value = decoded
    elif decoder in ("bytearray", "bytesarray"):
        value = bytearray(data.tobytes())
    else:
        value = data
    return value, metadata, raw_payload


def __aardvark__materialize_table(spec, value):
    if value is None:
        return None, None
    columns = spec.get("columns") or []
    orient = (spec.get("orient") or "records").lower()
    if orient not in ("records", "columns"):
        raise ValueError(f"unsupported rawctx table orientation: {orient}")
    column_schema = {}
    for column in columns:
        name = column.get("name")
        if not name:
            continue
        column_meta = {}
        if "dtype" in column:
            column_meta["dtype"] = column["dtype"]
        if "nullable" in column:
            column_meta["nullable"] = column["nullable"]
        if "metadata" in column and isinstance(column.get("metadata"), dict):
            column_meta["metadata"] = column["metadata"]
        if "shape" in column:
            column_meta["shape"] = column["shape"]
        if "manifest" in column and isinstance(column.get("manifest"), dict):
            column_meta["manifest"] = column["manifest"]
        if column_meta:
            column_schema[name] = column_meta
    table_metadata = {"orient": orient}
    if column_schema:
        table_metadata["schema"] = {"columns": column_schema}
    if orient == "records":
        if not isinstance(value, (list, tuple)):
            raise TypeError("rawctx table expects a list of record dicts")
        result = {column.get("name"): [] for column in columns}
        for record in value:
            if not isinstance(record, dict):
                raise TypeError("rawctx table records must be dictionaries")
            for column in columns:
                name = column.get("name")
                if not name:
                    continue
                if name in record:
                    result[name].append(record[name])
                elif column.get("optional") or column.get("default") is not None:
                    result[name].append(column.get("default"))
                else:
                    raise KeyError(f"rawctx table column '{name}' is required")
        __aardvark__apply_column_decoders(result, columns)
        return result, table_metadata
    # columns orient
    if not isinstance(value, dict):
        raise TypeError("rawctx table expects a dict of columns")
    result = {}
    for column in columns:
        name = column.get("name")
        if not name:
            continue
        if name in value:
            result[name] = value[name]
        elif column.get("optional") or column.get("default") is not None:
            result[name] = column.get("default")
        else:
            raise KeyError(f"rawctx table column '{name}' is required")
    __aardvark__apply_column_decoders(result, columns)
    return result, table_metadata


def __aardvark__apply_column_decoders(result, columns):
    for column in columns:
        name = column.get("name")
        if not name or name not in result:
            continue
        decoder = column.get("decoder")
        if not decoder:
            continue
        series = result[name]
        options = column.get("options") or {}
        if isinstance(series, list):
            converted = []
            for item in series:
                payload = __aardvark__prepare_decoder_payload(item, options)
                if payload is None:
                    converted.append(item)
                    continue
                value, _, _ = __aardvark__decode_scalar({"decoder": decoder, "options": options}, payload)
                converted.append(value)
            result[name] = converted
        else:
            payload = __aardvark__prepare_decoder_payload(series, options)
            if payload is None:
                continue
            value, _, _ = __aardvark__decode_scalar({"decoder": decoder, "options": options}, payload)
            result[name] = value


def __aardvark__prepare_decoder_payload(item, options):
    if isinstance(item, memoryview):
        return {"data": item, "metadata": None}
    if isinstance(item, bytes):
        return {"data": memoryview(item), "metadata": None}
    if isinstance(item, bytearray):
        return {"data": memoryview(bytes(item)), "metadata": None}
    if isinstance(item, str):
        encoding = options.get("encoding", "utf-8") if isinstance(options, dict) else "utf-8"
        return {"data": memoryview(item.encode(encoding)), "metadata": None}
    return None

def __aardvark__apply_outputs(spec, result):
    if not spec:
        return result, False
    if isinstance(spec, dict):
        return __aardvark__apply_single_output(spec, result)
    if not isinstance(spec, (list, tuple)):
        raise TypeError("rawctx outputs must be a dict or list of dicts")
    final_result = result
    handled_any = False
    for item in spec:
        if item is None:
            continue
        candidate, handled = __aardvark__apply_single_output(item, result)
        if handled:
            handled_any = True
            final_result = candidate
    return final_result, handled_any


def __aardvark__apply_single_output(spec, result):
    if not spec:
        return result, False
    if not isinstance(spec, dict):
        raise TypeError("rawctx output spec must be a dict")
    mode = spec.get("mode") or "publish-buffer"
    if mode != "publish-buffer":
        return result, False
    when_none = spec.get("when_none", "skip")
    if result is None:
        if when_none == "error":
            raise ValueError("rawctx output requires a non-None result")
        if when_none == "publish-empty":
            data_value = memoryview(b"")
        elif when_none == "propagate":
            return None, False
        else:
            return None, False
    else:
        data_value = result
    metadata = spec.get("metadata")
    if spec.get("python_transform"):
        namespace = {
            "result": result,
            "metadata": metadata,
            "memoryview": memoryview,
        }
        transformed = eval(spec["python_transform"], {}, namespace)
        if isinstance(transformed, tuple) and len(transformed) == 2:
            data_value, metadata = transformed
        else:
            data_value = transformed
    transform = spec.get("transform", "memoryview")
    if transform == "memoryview":
        if not isinstance(data_value, memoryview):
            try:
                data_value = memoryview(data_value)
            except TypeError:
                if isinstance(data_value, (bytes, bytearray)):
                    data_value = memoryview(data_value)
                else:
                    data_value = memoryview(bytes(data_value))
    elif transform == "bytes":
        if not isinstance(data_value, memoryview):
            try:
                data_value = memoryview(data_value)
            except TypeError:
                data_value = memoryview(bytes(data_value))
        try:
            data_value = data_value.cast("B")
        except (TypeError, ValueError):
            data_value = memoryview(data_value.tobytes())
        if not data_value.contiguous:
            data_value = memoryview(data_value.tobytes())
    elif transform == "utf8":
        if not isinstance(data_value, str):
            raise TypeError("rawctx output expected str for utf8 transform")
        encoding = spec.get("encoding", "utf-8")
        data_value = memoryview(data_value.encode(encoding))
    elif transform == "identity":
        pass
    else:
        raise ValueError(f"unsupported rawctx output transform: {transform}")
    publish_id = spec.get("id")
    if not publish_id:
        raise ValueError("rawctx output publish-buffer requires an id")
    from js import globalThis as _js
    _js.__aardvarkPublishBuffer(publish_id, data_value, metadata)
    behaviour = spec.get("return_behavior") or "none"
    if behaviour == "original":
        return result, True
    if behaviour == "buffer":
        return data_value, True
    return None, True

_module_name, _, _func_name = (__aardvark_rawctx_spec.get("entrypoint") or "").partition(":")
if _module_name and _func_name:
    _inputs = __aardvark_rawctx_spec.get("inputs") or []
    _output_specs = __aardvark_rawctx_spec.get("outputs") or []
    _legacy_output_spec = __aardvark_rawctx_spec.get("output")
    if not _output_specs and _legacy_output_spec:
        _output_specs = [_legacy_output_spec]
    if _inputs or _output_specs:
        _module = importlib.import_module(_module_name)
        _target = getattr(_module, _func_name)

        def __aardvark_rawctx_wrapper(
            __aardvark_target=_target,
            __aardvark_inputs=_inputs,
            __aardvark_outputs=tuple(_output_specs),
        ):
            source = getattr(builtins, "__aardvark_rawctx_inputs", {})
            args = []
            kwargs = {}
            for binding in __aardvark_inputs:
                payload = source.get(binding["field"])
                if payload is None:
                    if "default" in binding:
                        value = binding["default"]
                        metadata = None
                        raw_payload = None
                    elif binding.get("optional"):
                        value = None
                        metadata = None
                        raw_payload = None
                    else:
                        raise KeyError(f"rawctx input '{binding['field']}' is required")
                else:
                    value, metadata, raw_payload = __aardvark__decode_rawctx(binding, payload)
                    if value is None and "default" in binding:
                        value = binding["default"]
                if binding.get("metadata_arg"):
                    kwargs[binding["metadata_arg"]] = metadata
                if binding.get("raw_arg"):
                    kwargs[binding["raw_arg"]] = payload
                arg_name = binding.get("arg")
                if arg_name is not None:
                    mode = binding.get("mode", "keyword")
                    if mode == "positional":
                        args.append(value)
                    else:
                        kwargs[arg_name] = value
            result = __aardvark_target(*args, **kwargs)
            result, _handled = __aardvark__apply_outputs(__aardvark_outputs, result)
            return result

        setattr(_module, _func_name, __aardvark_rawctx_wrapper)
        del __aardvark_rawctx_wrapper, _module, _target, _inputs, _output_specs, _legacy_output_spec

del __aardvark_rawctx_spec
"#;

fn publish_rawctx_inputs(runtime: &mut JsRuntime, inputs: &[RawCtxInput]) -> Result<()> {
    runtime.with_context(|scope, _| {
        let global = scope.get_current_context().global(scope);

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

        let register_key = v8::String::new(scope, "__aardvarkRegisterInputBuffer")
            .ok_or_else(|| PyRunnerError::Execution("failed to allocate register key".into()))?;
        let register_value = global.get(scope, register_key.into()).ok_or_else(|| {
            PyRunnerError::Execution("__aardvarkRegisterInputBuffer is not defined".into())
        })?;
        let register_fn = v8::Local::<v8::Function>::try_from(register_value).map_err(|_| {
            PyRunnerError::Execution("__aardvarkRegisterInputBuffer is not a function".into())
        })?;

        for input in inputs {
            let name_value = v8::String::new(scope, &input.name).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate input buffer name".into())
            })?;

            let vec = input.buffer.clone().to_vec();
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(vec);
            let shared = backing.make_shared();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let typed = v8::Uint8Array::new(scope, array_buffer, 0, input.buffer.len())
                .ok_or_else(|| {
                    PyRunnerError::Execution(
                        "failed to allocate Uint8Array for input buffer".into(),
                    )
                })?;

            let metadata_value: v8::Local<v8::Value> = if let Some(meta) = &input.metadata {
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
        Ok(())
    })
}
