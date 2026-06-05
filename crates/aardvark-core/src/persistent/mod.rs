//! Persistent-runtime helpers: isolates, bundle handles, and pools.

mod artifact;
mod inline;
mod isolate;
mod pool;
mod warmed_host;

pub use artifact::BundleArtifact;
pub use inline::InlinePythonOptions;
pub use isolate::{BundleHandle, CleanupMode, HandlerSession, IsolateConfig, PythonIsolate};
pub use pool::{
    BundlePool, BundlePoolKey, BundlePoolRegistry, CallContext, CallOutcome, IsolateId,
    LifecycleHooks, PoolOptions, PoolStats, PreparedBundleHandler, QueueMode, RecycleReason,
};
pub use warmed_host::{
    WarmedBundleHost, WarmedBundleHostOptions, WarmedBundleHostRegistry,
    WarmedBundleHostRegistryOptions, WarmedBundleHostWarmup,
};
