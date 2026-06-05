use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};
use serde_json::Value as JsonValue;

use crate::error::{PyRunnerError, Result};
use crate::invocation::InvocationDescriptor;
use crate::persistent::{BundleArtifact, BundlePool, BundlePoolKey, HandlerSession, PoolOptions};
use crate::strategy::{JsonInput, RawCtxInput};

/// Representative invocation to run during startup warmup.
#[derive(Clone, Debug)]
pub enum WarmedBundleHostWarmup {
    Default,
    Json(Option<JsonInput>),
    RawCtx(Vec<RawCtxInput>),
}

impl WarmedBundleHostWarmup {
    /// Warm with the default invocation strategy.
    pub fn default_call() -> Self {
        Self::Default
    }

    /// Warm with ordinary JSON input.
    pub fn json(input: Option<JsonValue>) -> Self {
        Self::Json(input.map(JsonInput::Value))
    }

    /// Warm with a prepared JSON input.
    pub fn json_input(input: Option<JsonInput>) -> Self {
        Self::Json(input)
    }

    /// Warm with RawCtx inputs.
    pub fn rawctx(inputs: Vec<RawCtxInput>) -> Self {
        Self::RawCtx(inputs)
    }
}

/// Options for constructing a fully prepared bundle host.
#[derive(Clone, Default)]
pub struct WarmedBundleHostOptions {
    /// BundlePool options for the warmed host.
    pub pool: PoolOptions,
    /// Optional invocation descriptor override. If absent, the bundle manifest
    /// entrypoint is used.
    pub descriptor: Option<InvocationDescriptor>,
    /// Optional representative warmup that runs once on every created isolate
    /// before the host is returned.
    pub warmup: Option<WarmedBundleHostWarmup>,
}

/// Cache policy for [`WarmedBundleHostRegistry`].
#[derive(Clone)]
pub struct WarmedBundleHostRegistryOptions {
    /// Template used to construct cached warmed hosts.
    pub host: WarmedBundleHostOptions,
    /// Maximum ready hosts to retain. `None` means unbounded.
    pub max_ready_hosts: Option<usize>,
    /// Maximum parsed bundle artifacts to retain. `None` means unbounded.
    ///
    /// When both limits are set, this is coerced to at least
    /// `max_ready_hosts` so byte-based ready checks can still find retained
    /// hosts without reparsing bundles.
    pub max_artifacts: Option<usize>,
}

impl WarmedBundleHostRegistryOptions {
    /// Create an unbounded registry policy from a warmed-host template.
    pub fn new(host: WarmedBundleHostOptions) -> Self {
        Self {
            host,
            max_ready_hosts: None,
            max_artifacts: None,
        }
    }

    /// Bound the number of ready warmed hosts retained by the registry.
    ///
    /// A zero value is treated as one; a prewarm cache that cannot retain a
    /// ready host cannot satisfy the registry's request-path contract.
    pub fn with_max_ready_hosts(mut self, max_ready_hosts: usize) -> Self {
        self.max_ready_hosts = Some(max_ready_hosts.max(1));
        self
    }

    /// Bound the number of parsed bundle artifacts retained by the registry.
    ///
    /// A zero value is treated as one.
    pub fn with_max_artifacts(mut self, max_artifacts: usize) -> Self {
        self.max_artifacts = Some(max_artifacts.max(1));
        self
    }
}

impl From<WarmedBundleHostOptions> for WarmedBundleHostRegistryOptions {
    fn from(host: WarmedBundleHostOptions) -> Self {
        Self::new(host)
    }
}

impl WarmedBundleHostOptions {
    /// Create options for the mature BundlePool-backed host.
    pub fn pooled(pool: PoolOptions) -> Self {
        Self {
            pool,
            ..Self::default()
        }
    }

    /// Set an invocation descriptor override.
    pub fn with_descriptor(mut self, descriptor: InvocationDescriptor) -> Self {
        self.descriptor = Some(descriptor);
        self
    }

    /// Set a representative warmup invocation.
    pub fn with_warmup(mut self, warmup: WarmedBundleHostWarmup) -> Self {
        self.warmup = Some(warmup);
        self
    }
}

/// A prepared and optionally warmed execution host for one bundle.
///
/// This is the high-level host-facing shape for latency-sensitive services:
/// construct it during deploy/startup, then use the `call_*` methods on live
/// requests. The bundle still executes through Pyodide inside V8 isolates.
pub struct WarmedBundleHost {
    artifact: Arc<BundleArtifact>,
    pool: BundlePool,
    handler: Box<HandlerSession>,
}

/// Registry for bundle-scoped warmed hosts.
///
/// Use this at service startup or deploy boundaries when live requests should
/// reuse an already prepared Pyodide/V8 host. The registry caches one
/// [`WarmedBundleHost`] per bundle artifact and descriptor for the option
/// template it was created with.
#[derive(Clone)]
pub struct WarmedBundleHostRegistry {
    inner: Arc<WarmedBundleHostRegistryInner>,
}

struct WarmedBundleHostRegistryInner {
    options: WarmedBundleHostOptions,
    max_ready_hosts: Option<usize>,
    max_artifacts: Option<usize>,
    artifacts: Mutex<WarmedArtifactCache>,
    hosts: Mutex<WarmedHostCache>,
    condvar: Condvar,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WarmedBundleHostKey {
    pool: BundlePoolKey,
    descriptor: String,
}

enum WarmedBundleHostSlot {
    Creating { generation: u64 },
    Ready(Arc<WarmedBundleHost>),
}

#[derive(Default)]
struct WarmedArtifactCache {
    entries: HashMap<[u8; 32], Arc<BundleArtifact>>,
    order: VecDeque<[u8; 32]>,
}

impl WarmedArtifactCache {
    fn get(&mut self, digest: &[u8; 32]) -> Option<Arc<BundleArtifact>> {
        let artifact = self.entries.get(digest).cloned()?;
        touch_lru(&mut self.order, digest);
        Some(artifact)
    }

    fn insert(
        &mut self,
        digest: [u8; 32],
        artifact: Arc<BundleArtifact>,
        max_artifacts: Option<usize>,
    ) -> Arc<BundleArtifact> {
        if let Some(existing) = self.entries.get(&digest).cloned() {
            touch_lru(&mut self.order, &digest);
            return existing;
        }
        self.entries.insert(digest, artifact.clone());
        touch_lru(&mut self.order, &digest);
        self.evict_over_limit(max_artifacts);
        artifact
    }

    fn evict_over_limit(&mut self, max_artifacts: Option<usize>) {
        let Some(max_artifacts) = max_artifacts else {
            return;
        };
        while self.entries.len() > max_artifacts {
            let Some(digest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&digest);
        }
    }

    fn remove_digest(&mut self, digest: &[u8; 32]) -> Option<Arc<BundleArtifact>> {
        self.order.retain(|cached| cached != digest);
        self.entries.remove(digest)
    }

    fn remove_pool_artifacts(&mut self, pool_key: &BundlePoolKey) -> usize {
        let mut removed = Vec::new();
        self.entries.retain(|digest, artifact| {
            let should_remove = BundlePoolKey::from_artifact(artifact) == *pool_key;
            if should_remove {
                removed.push(*digest);
            }
            !should_remove
        });
        if removed.is_empty() {
            return 0;
        }
        self.order
            .retain(|digest| !removed.iter().any(|removed| removed == digest));
        removed.len()
    }

    fn clear(&mut self) -> usize {
        let removed = self.entries.len();
        self.entries.clear();
        self.order.clear();
        removed
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Default)]
struct WarmedHostCache {
    entries: HashMap<WarmedBundleHostKey, WarmedBundleHostSlot>,
    order: VecDeque<WarmedBundleHostKey>,
    generation: u64,
}

impl WarmedHostCache {
    fn ready_count(&self) -> usize {
        self.entries
            .values()
            .filter(|slot| matches!(slot, WarmedBundleHostSlot::Ready(_)))
            .count()
    }

    fn touch_ready(&mut self, key: &WarmedBundleHostKey) {
        touch_lru(&mut self.order, key);
    }

    fn evict_ready_over_limit(&mut self, max_ready_hosts: Option<usize>) {
        let Some(max_ready_hosts) = max_ready_hosts else {
            return;
        };
        while self.ready_count() > max_ready_hosts {
            let Some(index) = self.order.iter().position(|key| {
                matches!(self.entries.get(key), Some(WarmedBundleHostSlot::Ready(_)))
            }) else {
                break;
            };
            if let Some(key) = self.order.remove(index) {
                self.entries.remove(&key);
            }
        }
    }

    fn remove_ready(&mut self, key: &WarmedBundleHostKey) -> Option<Arc<WarmedBundleHost>> {
        if !matches!(self.entries.get(key), Some(WarmedBundleHostSlot::Ready(_))) {
            return None;
        }
        self.order.retain(|cached| cached != key);
        match self.entries.remove(key) {
            Some(WarmedBundleHostSlot::Ready(host)) => Some(host),
            Some(WarmedBundleHostSlot::Creating { .. }) | None => None,
        }
    }

    fn clear_ready_and_invalidate_creating(&mut self) -> usize {
        let removed = self.ready_count();
        self.entries
            .retain(|_, slot| matches!(slot, WarmedBundleHostSlot::Creating { .. }));
        self.order.clear();
        self.generation = self.generation.wrapping_add(1);
        removed
    }
}

fn touch_lru<K>(order: &mut VecDeque<K>, key: &K)
where
    K: Clone + Eq,
{
    order.retain(|candidate| candidate != key);
    order.push_back(key.clone());
}

impl WarmedBundleHostRegistry {
    /// Creates a registry using `options` as the fixed host template.
    ///
    /// For multiple descriptors, warmup strategies, or backends, create
    /// separate registries so each cache has an unambiguous startup contract.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(options: WarmedBundleHostOptions) -> Self {
        Self::with_options(options.into())
    }

    /// Creates a registry with explicit cache policy.
    ///
    /// Use this for long-running hosts that need bounded memory while still
    /// keeping the live request path on `ready_host_*` cache hits.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn with_options(options: WarmedBundleHostRegistryOptions) -> Self {
        let max_ready_hosts = options.max_ready_hosts.map(|max| max.max(1));
        let max_artifacts = match (options.max_artifacts, max_ready_hosts) {
            (Some(max_artifacts), Some(max_ready_hosts)) => {
                Some(max_artifacts.max(1).max(max_ready_hosts))
            }
            (Some(max_artifacts), None) => Some(max_artifacts.max(1)),
            (None, _) => None,
        };
        Self {
            inner: Arc::new(WarmedBundleHostRegistryInner {
                options: options.host,
                max_ready_hosts,
                max_artifacts,
                artifacts: Mutex::new(WarmedArtifactCache::default()),
                hosts: Mutex::new(WarmedHostCache::default()),
                condvar: Condvar::new(),
            }),
        }
    }

    /// Creates a registry with ready-host and artifact cache limits.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn with_cache_limits(
        options: WarmedBundleHostOptions,
        max_ready_hosts: usize,
        max_artifacts: usize,
    ) -> Self {
        Self::with_options(
            WarmedBundleHostRegistryOptions::new(options)
                .with_max_ready_hosts(max_ready_hosts)
                .with_max_artifacts(max_artifacts),
        )
    }

    /// Parses bundle bytes and returns the cached warmed host.
    pub fn host_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<Arc<WarmedBundleHost>> {
        let artifact = self.artifact_for_bytes(bytes)?;
        self.host_for_artifact(artifact)
    }

    /// Returns a ready warmed host for `bytes` without parsing or creating one.
    ///
    /// Use this on latency-critical request paths that must not accidentally
    /// pay a cache-miss startup cost. If the bundle was not prewarmed, or a
    /// prewarm is still in progress, this returns `Ok(None)`.
    pub fn ready_host_for_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Option<Arc<WarmedBundleHost>>> {
        let digest = *blake3::hash(bytes.as_ref()).as_bytes();
        let artifact = {
            let mut artifacts = self.inner.artifacts.lock();
            artifacts.get(&digest)
        };
        match artifact {
            Some(artifact) => self.ready_host_for_artifact(&artifact),
            None => Ok(None),
        }
    }

    /// Returns true when `bytes` already has a ready warmed host.
    pub fn is_ready_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<bool> {
        self.ready_host_for_bytes(bytes).map(|host| host.is_some())
    }

    /// Removes a ready warmed host for `bytes` from the registry.
    ///
    /// This is a non-creating invalidation path: if the bundle bytes have not
    /// already been parsed/prewarmed by this registry, it returns `Ok(None)`
    /// instead of parsing on the caller's thread.
    pub fn remove_for_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Option<Arc<WarmedBundleHost>>> {
        let digest = *blake3::hash(bytes.as_ref()).as_bytes();
        let artifact = {
            let mut artifacts = self.inner.artifacts.lock();
            artifacts.remove_digest(&digest)
        };
        match artifact {
            Some(artifact) => self.remove_for_artifact(&artifact),
            None => Ok(None),
        }
    }

    /// Parses bundle bytes and creates the warmed host immediately.
    ///
    /// This is the startup/deploy-time spelling of [`Self::host_for_bytes`].
    /// Use it before live traffic when the first request must not pay bundle
    /// parsing, Pyodide/V8 host construction, handler preparation, or warmup.
    pub fn prewarm_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<Arc<WarmedBundleHost>> {
        self.host_for_bytes(bytes)
    }

    /// Prewarms multiple bundle byte payloads in iteration order.
    ///
    /// Duplicate bundle payloads reuse the existing warmed host. If any bundle
    /// fails to parse or warm, the error is returned and already-created hosts
    /// remain cached.
    pub fn prewarm_many_bytes<I, B>(&self, bundles: I) -> Result<Vec<Arc<WarmedBundleHost>>>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        bundles
            .into_iter()
            .map(|bytes| self.prewarm_bytes(bytes))
            .collect()
    }

    /// Returns the cached warmed host for a normalized bundle artifact.
    ///
    /// Concurrent callers for the same bundle/descriptor wait for the first
    /// creation attempt instead of booting duplicate Pyodide/V8 hosts.
    pub fn host_for_artifact(
        &self,
        artifact: Arc<BundleArtifact>,
    ) -> Result<Arc<WarmedBundleHost>> {
        let descriptor =
            warmed_host_descriptor_for_artifact(&artifact, self.inner.options.descriptor.clone());
        let key = self.host_key_for_artifact(&artifact, &descriptor)?;

        loop {
            let mut hosts = self.inner.hosts.lock();
            match hosts.entries.get(&key) {
                Some(WarmedBundleHostSlot::Ready(host)) => {
                    let host = host.clone();
                    hosts.touch_ready(&key);
                    return Ok(host);
                }
                Some(WarmedBundleHostSlot::Creating { .. }) => {
                    self.inner.condvar.wait(&mut hosts);
                }
                None => {
                    let generation = hosts.generation;
                    hosts
                        .entries
                        .insert(key.clone(), WarmedBundleHostSlot::Creating { generation });
                    drop(hosts);

                    let mut options = self.inner.options.clone();
                    options.descriptor = Some(descriptor.clone());
                    let created =
                        WarmedBundleHost::from_artifact(artifact.clone(), options).map(Arc::new);
                    let mut hosts = self.inner.hosts.lock();
                    match created {
                        Ok(host) => {
                            let current_generation = hosts.generation;
                            let should_publish = matches!(
                                hosts.entries.get(&key),
                                Some(WarmedBundleHostSlot::Creating { generation: slot_generation })
                                    if *slot_generation == generation
                                        && current_generation == generation
                            );
                            if should_publish {
                                hosts
                                    .entries
                                    .insert(key.clone(), WarmedBundleHostSlot::Ready(host.clone()));
                                hosts.touch_ready(&key);
                                hosts.evict_ready_over_limit(self.inner.max_ready_hosts);
                            } else {
                                hosts.entries.remove(&key);
                                hosts.order.retain(|cached| cached != &key);
                            }
                            self.inner.condvar.notify_all();
                            return Ok(host);
                        }
                        Err(err) => {
                            let should_remove = matches!(
                                hosts.entries.get(&key),
                                Some(WarmedBundleHostSlot::Creating { generation: slot_generation })
                                    if *slot_generation == generation
                            );
                            if should_remove {
                                hosts.entries.remove(&key);
                            }
                            hosts.order.retain(|cached| cached != &key);
                            self.inner.condvar.notify_all();
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    /// Creates the warmed host for a normalized bundle artifact immediately.
    ///
    /// This is the startup/deploy-time spelling of [`Self::host_for_artifact`].
    pub fn prewarm_artifact(&self, artifact: Arc<BundleArtifact>) -> Result<Arc<WarmedBundleHost>> {
        self.host_for_artifact(artifact)
    }

    /// Returns a ready warmed host for `artifact` without creating one.
    ///
    /// If another caller is currently creating the host, this returns `Ok(None)`
    /// instead of waiting.
    pub fn ready_host_for_artifact(
        &self,
        artifact: &Arc<BundleArtifact>,
    ) -> Result<Option<Arc<WarmedBundleHost>>> {
        let descriptor =
            warmed_host_descriptor_for_artifact(artifact, self.inner.options.descriptor.clone());
        let key = self.host_key_for_artifact(artifact, &descriptor)?;
        let mut hosts = self.inner.hosts.lock();
        match hosts.entries.get(&key) {
            Some(WarmedBundleHostSlot::Ready(host)) => {
                let host = host.clone();
                hosts.touch_ready(&key);
                Ok(Some(host))
            }
            Some(WarmedBundleHostSlot::Creating { .. }) | None => Ok(None),
        }
    }

    /// Returns true when `artifact` already has a ready warmed host.
    pub fn is_ready_for_artifact(&self, artifact: &Arc<BundleArtifact>) -> Result<bool> {
        self.ready_host_for_artifact(artifact)
            .map(|host| host.is_some())
    }

    /// Removes a ready warmed host for `artifact` from the registry.
    ///
    /// In-flight creation slots are not removed. This keeps concurrent callers
    /// waiting on the same creation attempt instead of racing a second Pyodide/V8
    /// boot for the same bundle.
    pub fn remove_for_artifact(
        &self,
        artifact: &BundleArtifact,
    ) -> Result<Option<Arc<WarmedBundleHost>>> {
        let descriptor =
            warmed_host_descriptor_for_artifact(artifact, self.inner.options.descriptor.clone());
        let key = self.host_key_for_artifact(artifact, &descriptor)?;
        let removed = {
            let mut hosts = self.inner.hosts.lock();
            hosts.remove_ready(&key)
        };
        if removed.is_some() {
            let pool_key = BundlePoolKey::from_artifact(artifact);
            self.inner.artifacts.lock().remove_pool_artifacts(&pool_key);
        }
        Ok(removed)
    }

    /// Clears all ready warmed hosts and parsed bundle artifacts from the registry.
    ///
    /// In-flight creations are not cancelled. If an older creation finishes
    /// after this call, it is returned to its original caller but is not
    /// published into the cleared cache.
    pub fn clear(&self) -> usize {
        let removed = self
            .inner
            .hosts
            .lock()
            .clear_ready_and_invalidate_creating();
        self.inner.artifacts.lock().clear();
        self.inner.condvar.notify_all();
        removed
    }

    /// Returns the number of ready warmed hosts currently tracked.
    pub fn host_count(&self) -> usize {
        self.inner.hosts.lock().ready_count()
    }

    /// Returns the number of parsed bundle artifacts currently retained.
    pub fn artifact_count(&self) -> usize {
        self.inner.artifacts.lock().len()
    }

    /// Returns true when the registry has no ready warmed hosts.
    pub fn is_empty(&self) -> bool {
        self.host_count() == 0
    }

    fn artifact_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<Arc<BundleArtifact>> {
        let bytes = bytes.as_ref();
        let digest = *blake3::hash(bytes).as_bytes();
        if let Some(artifact) = self.inner.artifacts.lock().get(&digest) {
            return Ok(artifact);
        }

        let parsed = BundleArtifact::from_bytes(bytes)?;
        let mut artifacts = self.inner.artifacts.lock();
        Ok(artifacts.insert(digest, parsed, self.inner.max_artifacts))
    }

    fn host_key_for_artifact(
        &self,
        artifact: &BundleArtifact,
        descriptor: &InvocationDescriptor,
    ) -> Result<WarmedBundleHostKey> {
        Ok(WarmedBundleHostKey {
            pool: BundlePoolKey::from_artifact(artifact),
            descriptor: warmed_host_descriptor_key(descriptor)?,
        })
    }
}

impl WarmedBundleHost {
    /// Build a warmed host from ZIP bundle bytes.
    pub fn from_bytes(bytes: impl AsRef<[u8]>, options: WarmedBundleHostOptions) -> Result<Self> {
        let artifact = BundleArtifact::from_bytes(bytes)?;
        Self::from_artifact(artifact, options)
    }

    /// Build a warmed host from a normalized bundle artifact.
    pub fn from_artifact(
        artifact: Arc<BundleArtifact>,
        options: WarmedBundleHostOptions,
    ) -> Result<Self> {
        let warmup = options.warmup.clone();
        let pool = BundlePool::from_artifact(artifact.clone(), options.pool)?;
        let handler = pool.prepare_handler(options.descriptor)?;
        let host = Self {
            artifact,
            pool,
            handler: Box::new(handler),
        };

        if let Some(warmup) = warmup {
            host.warm_all(warmup)?;
        }

        Ok(host)
    }

    /// Returns the normalized bundle artifact backing this host.
    pub fn artifact(&self) -> Arc<BundleArtifact> {
        self.artifact.clone()
    }

    /// Invoke the prepared handler using the default strategy.
    pub fn call_default(&self) -> Result<crate::ExecutionOutcome> {
        self.pool.call_default(&self.handler)
    }

    /// Invoke the prepared handler using ordinary JSON input.
    pub fn call_json(&self, input: Option<JsonValue>) -> Result<crate::ExecutionOutcome> {
        self.call_json_input(input.map(JsonInput::Value))
    }

    /// Invoke the prepared handler using a prepared JSON input.
    pub fn call_json_input(&self, input: Option<JsonInput>) -> Result<crate::ExecutionOutcome> {
        self.pool.call_json_input(&self.handler, input)
    }

    /// Invoke the prepared handler using RawCtx inputs.
    pub fn call_rawctx(&self, inputs: Vec<RawCtxInput>) -> Result<crate::ExecutionOutcome> {
        self.pool.call_rawctx(&self.handler, inputs)
    }

    /// Execute a representative invocation once on every created isolate.
    pub fn warm_all(&self, warmup: WarmedBundleHostWarmup) -> Result<Vec<crate::ExecutionOutcome>> {
        match warmup {
            WarmedBundleHostWarmup::Default => self.pool.warm_all_default(&self.handler),
            WarmedBundleHostWarmup::Json(input) => {
                self.pool.warm_all_json_input(&self.handler, input)
            }
            WarmedBundleHostWarmup::RawCtx(inputs) => {
                self.pool.warm_all_rawctx(&self.handler, inputs)
            }
        }
    }
}

fn warmed_host_descriptor_key(descriptor: &InvocationDescriptor) -> Result<String> {
    serde_json::to_string(descriptor).map_err(|err| {
        PyRunnerError::Descriptor(format!("failed to serialize invocation descriptor: {err}"))
    })
}

fn warmed_host_descriptor_for_artifact(
    artifact: &BundleArtifact,
    descriptor: Option<InvocationDescriptor>,
) -> InvocationDescriptor {
    let mut descriptor = descriptor.unwrap_or_else(|| artifact.default_descriptor());
    descriptor.language = descriptor.language.or(Some(artifact.language()));
    descriptor
}
