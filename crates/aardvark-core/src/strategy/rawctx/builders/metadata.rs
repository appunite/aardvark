use serde_json::{Map as JsonMap, Value as JsonValue};

use super::table::RawCtxTableSpec;

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
