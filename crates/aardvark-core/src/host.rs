//! Host-facing helpers for consuming sandbox diagnostics.

use crate::outcome::{Diagnostics, FilesystemViolation, NetworkDeniedHost, NetworkHostContact};

/// Aggregated telemetry derived from [`Diagnostics`] for host integrations.
#[derive(Clone, Debug, Default)]
pub struct SandboxTelemetry {
    pub cpu_ms_used: Option<u64>,
    pub filesystem: FilesystemTelemetry,
    pub network: NetworkTelemetry,
}

/// Filesystem usage and violation details.
#[derive(Clone, Debug, Default)]
pub struct FilesystemTelemetry {
    pub bytes_written: Option<u64>,
    pub violations: Vec<FilesystemViolation>,
}

/// Network allow/deny observations captured during execution.
#[derive(Clone, Debug, Default)]
pub struct NetworkTelemetry {
    pub allowed: Vec<NetworkHostContact>,
    pub blocked: Vec<NetworkDeniedHost>,
}

impl From<&Diagnostics> for SandboxTelemetry {
    fn from(value: &Diagnostics) -> Self {
        Self {
            cpu_ms_used: value.cpu_ms_used,
            filesystem: FilesystemTelemetry {
                bytes_written: value.filesystem_bytes_written,
                violations: value.filesystem_violations.clone(),
            },
            network: NetworkTelemetry {
                allowed: value.network_hosts_contacted.clone(),
                blocked: value.network_hosts_blocked.clone(),
            },
        }
    }
}

impl SandboxTelemetry {
    /// Returns `true` when any sandbox policy blocked the invocation.
    pub fn has_policy_violations(&self) -> bool {
        !self.network.blocked.is_empty() || !self.filesystem.violations.is_empty()
    }
}
