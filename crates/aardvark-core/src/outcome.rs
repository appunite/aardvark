//! Structured execution outcome returned by the runtime.

use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::engine::{SharedBuffer, SharedBufferBacking};
use crate::host::SandboxTelemetry;

/// Aggregated result of an invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub diagnostics: Diagnostics,
    pub status: OutcomeStatus,
}

impl ExecutionOutcome {
    pub fn success(payload: ResultPayload, diagnostics: Diagnostics) -> Self {
        Self {
            diagnostics,
            status: OutcomeStatus::Success(payload),
        }
    }

    pub fn failure(kind: FailureKind, diagnostics: Diagnostics) -> Self {
        Self {
            diagnostics,
            status: OutcomeStatus::Failure(kind),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self.status, OutcomeStatus::Success(_))
    }

    pub fn payload(&self) -> Option<&ResultPayload> {
        match &self.status {
            OutcomeStatus::Success(payload) => Some(payload),
            OutcomeStatus::Failure(_) => None,
        }
    }

    /// Returns a host-friendly telemetry snapshot derived from the diagnostics.
    pub fn sandbox_telemetry(&self) -> SandboxTelemetry {
        self.diagnostics.to_telemetry()
    }
}

/// Diagnostic streams captured during execution.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stdout: String,
    pub stderr: String,
    pub exception: Option<ExceptionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_bytes_written: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_hosts_contacted: Vec<NetworkHostContact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_hosts_blocked: Vec<NetworkDeniedHost>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystem_violations: Vec<FilesystemViolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<ResetSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub py_heap_kib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_kib_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_kib_after: Option<u64>,
}

impl Diagnostics {
    /// Converts the diagnostics into a structured telemetry snapshot suitable for hosts.
    pub fn to_telemetry(&self) -> SandboxTelemetry {
        SandboxTelemetry::from(self)
    }
}

/// Summary of the reset that prepared the runtime for this invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResetSummary {
    pub mode: ResetMode,
    pub duration_ms: u64,
    pub engine_generation: u64,
}

/// Reset mechanism used before the invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ResetMode {
    RecreateEngine,
    InPlace,
}

/// Structured status of the execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OutcomeStatus {
    Success(ResultPayload),
    Failure(FailureKind),
}

/// Payload returned by successful invocations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ResultPayload {
    None,
    Text(String),
    Json(JsonValue),
    Binary(Vec<u8>),
    SharedBuffers(Vec<SharedBufferHandle>),
}

impl ResultPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            ResultPayload::None => "none",
            ResultPayload::Text(_) => "text",
            ResultPayload::Json(_) => "json",
            ResultPayload::Binary(_) => "binary",
            ResultPayload::SharedBuffers(_) => "shared-buffers",
        }
    }
}

/// Metadata for zero-copy buffers exposed through the runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedBufferHandle {
    pub id: String,
    pub length: usize,
    #[serde(default, skip_serializing, skip_deserializing)]
    data: Option<Bytes>,
    #[serde(default, skip_serializing, skip_deserializing)]
    backing: Option<Arc<SharedBufferBacking>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonValue>,
}

impl SharedBufferHandle {
    /// Construct a handle backed by Bytes data and optional metadata.
    pub fn with_bytes(id: impl Into<String>, bytes: Bytes, metadata: Option<JsonValue>) -> Self {
        Self {
            id: id.into(),
            length: bytes.len(),
            data: Some(bytes),
            backing: None,
            metadata,
        }
    }

    pub(crate) fn from_shared_buffer(buffer: &SharedBuffer) -> Self {
        Self {
            id: buffer.id.clone(),
            length: buffer.length,
            data: buffer.bytes.clone(),
            backing: buffer.backing.clone(),
            metadata: buffer.metadata.clone(),
        }
    }

    /// Returns the buffer contents if they are still owned by this handle.
    pub fn as_bytes(&self) -> Option<&Bytes> {
        self.data.as_ref()
    }

    /// Returns a borrowed slice of the buffer if zero-copy storage is available.
    pub fn as_slice(&self) -> Option<&[u8]> {
        if let Some(bytes) = self.data.as_ref() {
            Some(bytes.as_ref())
        } else {
            self.backing.as_ref().map(|backing| backing.as_slice())
        }
    }

    /// Consume the handle and return the owned Bytes, if present.
    pub fn into_bytes(self) -> Option<Bytes> {
        match (self.data, self.backing) {
            (Some(bytes), _) => Some(bytes),
            (None, Some(backing)) => Some(Bytes::copy_from_slice(backing.as_slice())),
            _ => None,
        }
    }

    /// Drop the in-memory data but keep metadata/id for out-of-band transports.
    pub fn without_data(self) -> Self {
        Self {
            id: self.id,
            length: self.length,
            data: None,
            backing: self.backing,
            metadata: self.metadata,
        }
    }
}

/// Failure reasons surfaced through the outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FailureKind {
    PythonException(ExceptionInfo),
    AdapterError { message: String },
    TimeoutExceeded { requested_ms: u64 },
    CpuLimitExceeded { requested_ms: u64, used_ms: u64 },
    HeapLimitExceeded { requested_mb: u64 },
    Other { message: String },
}

/// Captured Python exception details.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExceptionInfo {
    pub typ: Option<String>,
    pub value: Option<String>,
    pub traceback: Option<String>,
}

/// Network destinations contacted during execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkHostContact {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub https: bool,
}

/// Network requests blocked by policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkDeniedHost {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub https_required: bool,
    pub reason: String,
}

/// Filesystem policy violations encountered during execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesystemViolation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}
