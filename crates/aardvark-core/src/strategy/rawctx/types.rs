use bytes::Bytes;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::{PyRunnerError, Result};

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

    pub(super) fn to_json_value(&self) -> Result<JsonValue> {
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

    /// Construct a RawCtx input from an owned byte vector.
    ///
    /// Prefer this for hot RawCtx calls when the host can hand request bytes to
    /// the runtime. Unique ownership lets the V8 backing store take over the
    /// allocation instead of copying from a shared `Bytes` clone.
    pub fn from_vec(
        name: impl Into<String>,
        buffer: Vec<u8>,
        metadata: Option<RawCtxMetadata>,
    ) -> Result<Self> {
        Self::new(name, Bytes::from(buffer), metadata)
    }
}
