//! Host-facing helpers for consuming sandbox diagnostics.

use crate::outcome::{
    Diagnostics, FilesystemViolation, NetworkDeniedHost, NetworkHostContact, ResetMode,
    ResetSummary,
};
use crate::persistent::PoolStats;

/// Aggregated telemetry derived from [`Diagnostics`] for host integrations.
#[derive(Clone, Debug, Default)]
pub struct SandboxTelemetry {
    pub cpu_ms_used: Option<u64>,
    pub queue_wait_ms: Option<u64>,
    pub prepare_ms: Option<u64>,
    pub cleanup_ms: Option<u64>,
    pub filesystem: FilesystemTelemetry,
    pub network: NetworkTelemetry,
    pub reset: Option<ResetTelemetry>,
    pub memory: MemoryTelemetry,
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

/// Memory usage snapshots captured during execution.
#[derive(Clone, Debug, Default)]
pub struct MemoryTelemetry {
    pub py_heap_kib: Option<u64>,
    pub rss_kib_before: Option<u64>,
    pub rss_kib_after: Option<u64>,
}

/// Reset data captured prior to invocation.
#[derive(Clone, Debug)]
pub struct ResetTelemetry {
    pub mode: ResetMode,
    pub duration_ms: u64,
    pub engine_generation: u64,
}

/// Aggregated pool-level telemetry derived from [`PoolStats`].
#[derive(Clone, Debug, Default)]
pub struct PoolTelemetry {
    pub total_isolates: usize,
    pub idle_isolates: usize,
    pub busy_isolates: usize,
    pub waiting_calls: usize,
    pub invocations: u64,
    pub average_queue_wait_ms: f64,
    pub queue_wait_p50_ms: Option<f64>,
    pub queue_wait_p95_ms: Option<f64>,
    pub quarantine_events: u64,
    pub quarantine_heap_hits: u64,
    pub quarantine_rss_hits: u64,
    pub scaledown_events: u64,
}

impl From<&Diagnostics> for SandboxTelemetry {
    fn from(value: &Diagnostics) -> Self {
        Self {
            cpu_ms_used: value.cpu_ms_used,
            queue_wait_ms: value.queue_wait_ms,
            prepare_ms: value.prepare_ms,
            cleanup_ms: value.cleanup_ms,
            filesystem: FilesystemTelemetry {
                bytes_written: value.filesystem_bytes_written,
                violations: value.filesystem_violations.clone(),
            },
            network: NetworkTelemetry {
                allowed: value.network_hosts_contacted.clone(),
                blocked: value.network_hosts_blocked.clone(),
            },
            reset: value.reset.as_ref().map(ResetTelemetry::from),
            memory: MemoryTelemetry {
                py_heap_kib: value.py_heap_kib,
                rss_kib_before: value.rss_kib_before,
                rss_kib_after: value.rss_kib_after,
            },
        }
    }
}

impl From<&ResetSummary> for ResetTelemetry {
    fn from(summary: &ResetSummary) -> Self {
        Self {
            mode: summary.mode.clone(),
            duration_ms: summary.duration_ms,
            engine_generation: summary.engine_generation,
        }
    }
}

impl SandboxTelemetry {
    /// Returns `true` when any sandbox policy blocked the invocation.
    pub fn has_policy_violations(&self) -> bool {
        !self.network.blocked.is_empty() || !self.filesystem.violations.is_empty()
    }
}

impl From<&PoolStats> for PoolTelemetry {
    fn from(stats: &PoolStats) -> Self {
        Self {
            total_isolates: stats.total,
            idle_isolates: stats.idle,
            busy_isolates: stats.busy,
            waiting_calls: stats.waiting,
            invocations: stats.invocations,
            average_queue_wait_ms: stats.average_queue_wait_ms,
            queue_wait_p50_ms: stats.queue_wait_p50_ms,
            queue_wait_p95_ms: stats.queue_wait_p95_ms,
            quarantine_events: stats.quarantine_events,
            quarantine_heap_hits: stats.quarantine_heap_hits,
            quarantine_rss_hits: stats.quarantine_rss_hits,
            scaledown_events: stats.scaledown_events,
        }
    }
}
