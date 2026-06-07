use serde::{Deserialize, Serialize};
use serde_json::{self, Map as JsonMap, Value as JsonValue};

use crate::error::{PyRunnerError, Result};

use super::decoder::validate_decoder_options;

/// Declarative schema describing a tabular payload decoded from a RawCtx buffer.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RawCtxTableSpec {
    #[serde(default)]
    columns: Vec<RawCtxTableColumnSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orient: Option<String>,
}

impl RawCtxTableSpec {
    pub(in crate::strategy::rawctx) fn into_json(self) -> JsonValue {
        serde_json::to_value(self).expect("serialize rawctx table spec")
    }

    pub(in crate::strategy::rawctx) fn validate(&self) -> Result<()> {
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
