//! Invocation descriptor and budget definitions.
//!
//! These structures allow hosts to describe the contract for a Python bundle
//! without leaking platform-specific manifest details into the runtime. They
//! intentionally stay lightweight so adapters can extend them with
//! `metadata` fields as needed.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Describes the runtime contract for a single invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvocationDescriptor {
    entrypoint: String,
    /// Inputs passed positionally to the Python handler.
    #[serde(default)]
    pub inputs: Vec<FieldDescriptor>,
    /// Outputs captured from the handler.
    #[serde(default)]
    pub outputs: Vec<FieldDescriptor>,
    /// Free-form JSON for adapters that need extra parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonValue>,
    /// Optional rolling window configuration for stateful invocations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowConfig>,
    /// Resource budgets applied to the invocation.
    #[serde(default)]
    pub limits: InvocationLimits,
}

impl InvocationDescriptor {
    /// Create a descriptor with a fully-specified entrypoint and sensible defaults.
    pub fn new(entrypoint: impl Into<String>) -> Self {
        let entrypoint = entrypoint.into();
        Self {
            entrypoint: sanitize_entrypoint(entrypoint),
            inputs: Vec::new(),
            outputs: Vec::new(),
            params: None,
            window: None,
            limits: InvocationLimits::default(),
        }
    }

    /// Convenience constructor for legacy usage – only the entrypoint is provided.
    pub fn trivial(entrypoint: impl Into<String>) -> Self {
        Self::new(entrypoint)
    }

    /// Returns the canonical entrypoint (module:function path or script).
    pub fn entrypoint(&self) -> &str {
        &self.entrypoint
    }

    /// Returns a descriptor clone with the provided limits applied.
    pub fn with_limits(mut self, limits: InvocationLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Ensure the descriptor is well-formed.
    pub fn validate(&self) -> Result<(), DescriptorError> {
        if self.entrypoint.trim().is_empty() {
            return Err(DescriptorError::InvalidEntrypoint);
        }
        for field in self.inputs.iter().chain(self.outputs.iter()) {
            field.validate()?;
        }
        self.limits.validate()?;
        if let Some(window) = &self.window {
            window.validate()?;
        }
        Ok(())
    }
}

/// Simple descriptor for an input or output field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldDescriptor {
    /// Field name (also used for logging instrumentation).
    pub name: String,
    /// Optional type hint understood by host adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_tag: Option<String>,
    /// Optional metadata, typically used for decoder configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonValue>,
}

impl FieldDescriptor {
    fn validate(&self) -> Result<(), DescriptorError> {
        if self.name.trim().is_empty() {
            return Err(DescriptorError::InvalidFieldName);
        }
        Ok(())
    }
}

/// Optional rolling window configuration for descriptor-aware invocations.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Window size (number of events included per invocation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Step between successive windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u64>,
    /// Optional stride to skip records within the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stride: Option<u64>,
}

impl WindowConfig {
    fn validate(&self) -> Result<(), DescriptorError> {
        for value in [self.size, self.step, self.stride].into_iter().flatten() {
            if value == 0 {
                return Err(DescriptorError::InvalidWindowConfig);
            }
        }
        Ok(())
    }
}

/// Execution budget configuration derived from the descriptor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InvocationLimits {
    /// Maximum wall-clock time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    /// Maximum heap usage in MiB as reported by V8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_mb: Option<u64>,
    /// Maximum CPU time in milliseconds (per-thread).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "cpu_fuel")]
    pub cpu_ms: Option<u64>,
}

impl InvocationLimits {
    fn validate(&self) -> Result<(), DescriptorError> {
        if let Some(0) = self.wall_ms {
            return Err(DescriptorError::InvalidLimits);
        }
        if let Some(0) = self.heap_mb {
            return Err(DescriptorError::InvalidLimits);
        }
        if let Some(0) = self.cpu_ms {
            return Err(DescriptorError::InvalidLimits);
        }
        Ok(())
    }

    /// Merge the descriptor limits with an optional override, picking the tighter budget.
    pub fn merged_with(&self, override_limits: Option<&InvocationLimits>) -> InvocationLimits {
        fn merge(primary: Option<u64>, override_value: Option<u64>) -> Option<u64> {
            match (primary, override_value) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        }

        if let Some(override_limits) = override_limits {
            InvocationLimits {
                wall_ms: merge(self.wall_ms, override_limits.wall_ms),
                heap_mb: merge(self.heap_mb, override_limits.heap_mb),
                cpu_ms: merge(self.cpu_ms, override_limits.cpu_ms),
            }
        } else {
            self.clone()
        }
    }
}

/// Descriptor validation failures surfaced to callers.
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("descriptor entrypoint cannot be empty")]
    InvalidEntrypoint,
    #[error("descriptor field name cannot be empty")]
    InvalidFieldName,
    #[error("descriptor window configuration cannot contain zero values")]
    InvalidWindowConfig,
    #[error("descriptor limits must be positive when specified")]
    InvalidLimits,
}

fn sanitize_entrypoint(entrypoint: String) -> String {
    entrypoint.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{FieldDescriptor, InvocationDescriptor, InvocationLimits};

    #[test]
    fn trivial_descriptor_sanitizes_entrypoint() {
        let descriptor = InvocationDescriptor::trivial("  module:func  ");
        assert_eq!(descriptor.entrypoint(), "module:func");
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_entrypoint() {
        let descriptor = InvocationDescriptor::trivial("   ");
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn validate_accepts_multiple_outputs() {
        let mut descriptor = InvocationDescriptor::trivial("pkg:handler");
        descriptor.outputs.push(FieldDescriptor {
            name: "first".into(),
            type_tag: None,
            metadata: None,
        });
        descriptor.outputs.push(FieldDescriptor {
            name: "second".into(),
            type_tag: None,
            metadata: None,
        });
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn limits_merge_prefers_tighter_budget() {
        let base = InvocationLimits {
            wall_ms: Some(2_000),
            heap_mb: Some(512),
            cpu_ms: None,
        };
        let override_limits = InvocationLimits {
            wall_ms: Some(1_000),
            heap_mb: Some(1_024),
            cpu_ms: Some(10_000),
        };
        let merged = base.merged_with(Some(&override_limits));
        assert_eq!(merged.wall_ms, Some(1_000));
        assert_eq!(merged.heap_mb, Some(512));
        assert_eq!(merged.cpu_ms, Some(10_000));
    }
}
