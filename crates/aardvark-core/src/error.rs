//! Error types exposed by the runtime.

use thiserror::Error;

/// Library-level error enumeration.
#[derive(Debug, Error)]
pub enum PyRunnerError {
    #[error("initialization error: {0}")]
    Init(String),
    #[error("bundle error: {0}")]
    Bundle(String),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("descriptor error: {0}")]
    Descriptor(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("bundle pool queue is full (queue length {queue_length}, limit {limit})")]
    PoolQueueFull { queue_length: usize, limit: usize },
    #[error("bundle pool is at capacity (active {active}, max {max_size})")]
    PoolAtCapacity { active: usize, max_size: usize },
    #[error("bundle pool is shutting down")]
    PoolShuttingDown,
    #[error("execution timed out after {requested_ms}ms")]
    TimeoutExceeded { requested_ms: u64 },
    #[error("execution exceeded heap budget (limit {requested_mb} MiB)")]
    HeapLimitExceeded { requested_mb: u64 },
}

pub type Result<T, E = PyRunnerError> = std::result::Result<T, E>;
