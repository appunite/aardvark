//! Error types exposed by the runtime.

use thiserror::Error;

/// Library-level error enumeration.
#[derive(Debug, Error)]
pub enum PyRunnerError {
    #[error("initialization error: {0}")]
    Init(String),
    #[error("bundle error: {0}")]
    Bundle(String),
    #[error(
        "bundle {kind} limit exceeded{path_display}: actual {actual} bytes, limit {limit} bytes",
        path_display = BundleLimitPath(path.as_deref())
    )]
    BundleLimitExceeded {
        kind: BundleLimitKind,
        path: Option<String>,
        actual: u64,
        limit: u64,
    },
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

struct BundleLimitPath<'a>(Option<&'a str>);

impl std::fmt::Display for BundleLimitPath<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(path) => write!(f, " for {path}"),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleLimitKind {
    ArchiveBytes,
    EntryCount,
    EntryBytes,
    TotalUncompressedBytes,
}

impl std::fmt::Display for BundleLimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BundleLimitKind::ArchiveBytes => "archive byte",
            BundleLimitKind::EntryCount => "entry count",
            BundleLimitKind::EntryBytes => "entry byte",
            BundleLimitKind::TotalUncompressedBytes => "total uncompressed byte",
        })
    }
}
