use super::*;

impl BundlePoolKey {
    /// Builds a registry key from a normalised bundle artifact.
    pub fn from_artifact(artifact: &BundleArtifact) -> Self {
        Self {
            fingerprint: artifact.fingerprint(),
            pyodide_distribution_profile: artifact
                .pyodide_distribution_profile()
                .map(str::to_owned),
        }
    }

    /// Returns the bundle fingerprint component of the key.
    pub fn fingerprint(&self) -> BundleFingerprint {
        self.fingerprint
    }

    /// Returns the manifest-requested Pyodide distribution profile, if any.
    pub fn pyodide_distribution_profile(&self) -> Option<&str> {
        self.pyodide_distribution_profile.as_deref()
    }
}

impl BundlePoolRegistry {
    /// Creates a registry using `options` as the template for every pool it creates.
    ///
    /// Bundle manifest requirements, including `runtime.pyodide.profile`, are
    /// still applied by `BundlePool::from_artifact` before any isolate starts.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(options: PoolOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            inner: Arc::new(BundlePoolRegistryInner {
                options,
                artifacts: Mutex::new(HashMap::new()),
                pools: Mutex::new(HashMap::new()),
                handlers: Mutex::new(HashMap::new()),
                condvar: Condvar::new(),
            }),
        })
    }

    /// Parses bundle bytes and returns the corresponding warmed pool.
    pub fn pool_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<BundlePool> {
        let artifact = self.artifact_for_bytes(bytes)?;
        self.pool_for_artifact(artifact)
    }

    /// Parses bundle bytes and returns a cached prepared default handler.
    pub fn prepare_default_handler_for_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<PreparedBundleHandler> {
        self.prepare_handler_for_bytes(bytes, None)
    }

    /// Parses bundle bytes and returns a cached prepared handler.
    pub fn prepare_handler_for_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
        descriptor: Option<InvocationDescriptor>,
    ) -> Result<PreparedBundleHandler> {
        let artifact = self.artifact_for_bytes(bytes)?;
        self.prepare_handler_for_artifact(artifact, descriptor)
    }

    /// Returns a cached prepared default handler for `artifact`.
    pub fn prepare_default_handler_for_artifact(
        &self,
        artifact: Arc<BundleArtifact>,
    ) -> Result<PreparedBundleHandler> {
        self.prepare_handler_for_artifact(artifact, None)
    }

    /// Returns a cached prepared handler for `artifact`.
    pub fn prepare_handler_for_artifact(
        &self,
        artifact: Arc<BundleArtifact>,
        descriptor: Option<InvocationDescriptor>,
    ) -> Result<PreparedBundleHandler> {
        let pool_key = BundlePoolKey::from_artifact(&artifact);
        let descriptor = handler_descriptor_for_artifact(&artifact, descriptor);
        let handler_key = BundleHandlerKey {
            pool: pool_key,
            descriptor: descriptor_registry_key(&descriptor)?,
        };
        if let Some(prepared) = self.inner.handlers.lock().get(&handler_key).cloned() {
            return Ok(prepared);
        }

        let pool = self.pool_for_artifact(artifact)?;
        let handler = Arc::new(pool.prepare_handler(Some(descriptor))?);
        let prepared = PreparedBundleHandler { pool, handler };
        let mut handlers = self.inner.handlers.lock();
        let prepared = handlers
            .entry(handler_key)
            .or_insert_with(|| prepared.clone())
            .clone();
        Ok(prepared)
    }

    fn artifact_for_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<Arc<BundleArtifact>> {
        let bytes = bytes.as_ref();
        let digest = *blake3::hash(bytes).as_bytes();
        let cached_artifact = {
            let artifacts = self.inner.artifacts.lock();
            artifacts.get(&digest).cloned()
        };
        let artifact = if let Some(artifact) = cached_artifact {
            artifact
        } else {
            let parsed = BundleArtifact::from_bytes(bytes)?;
            let mut artifacts = self.inner.artifacts.lock();
            artifacts
                .entry(digest)
                .or_insert_with(|| parsed.clone())
                .clone()
        };
        Ok(artifact)
    }

    /// Returns the warmed pool for `artifact`, creating it if needed.
    ///
    /// Concurrent callers for the same key wait for the first creation attempt
    /// instead of booting duplicate pools. Different bundle/profile keys can
    /// proceed independently.
    pub fn pool_for_artifact(&self, artifact: Arc<BundleArtifact>) -> Result<BundlePool> {
        let key = BundlePoolKey::from_artifact(&artifact);
        loop {
            let mut pools = self.inner.pools.lock();
            match pools.get(&key) {
                Some(RegistryPoolSlot::Ready(pool)) => return Ok(pool.clone()),
                Some(RegistryPoolSlot::Creating) => {
                    self.inner.condvar.wait(&mut pools);
                }
                None => {
                    pools.insert(key.clone(), RegistryPoolSlot::Creating);
                    drop(pools);

                    let created =
                        BundlePool::from_artifact(artifact.clone(), self.inner.options.clone());
                    let mut pools = self.inner.pools.lock();
                    match created {
                        Ok(pool) => {
                            pools.insert(key.clone(), RegistryPoolSlot::Ready(pool.clone()));
                            self.inner.condvar.notify_all();
                            return Ok(pool);
                        }
                        Err(err) => {
                            pools.remove(&key);
                            self.inner.condvar.notify_all();
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    /// Returns an already-created pool for `key`, if one is ready.
    pub fn get(&self, key: &BundlePoolKey) -> Option<BundlePool> {
        match self.inner.pools.lock().get(key) {
            Some(RegistryPoolSlot::Ready(pool)) => Some(pool.clone()),
            Some(RegistryPoolSlot::Creating) | None => None,
        }
    }

    /// Removes a ready pool from the registry.
    pub fn remove(&self, key: &BundlePoolKey) -> Option<BundlePool> {
        let mut pools = self.inner.pools.lock();
        if matches!(pools.get(key), Some(RegistryPoolSlot::Ready(_))) {
            self.inner
                .handlers
                .lock()
                .retain(|handler_key, _| handler_key.pool != *key);
            match pools.remove(key) {
                Some(RegistryPoolSlot::Ready(pool)) => Some(pool),
                Some(RegistryPoolSlot::Creating) | None => None,
            }
        } else {
            None
        }
    }

    /// Returns the number of ready pools currently tracked by the registry.
    pub fn pool_count(&self) -> usize {
        self.inner
            .pools
            .lock()
            .values()
            .filter(|slot| matches!(slot, RegistryPoolSlot::Ready(_)))
            .count()
    }

    /// Returns true when the registry has no ready pools.
    pub fn is_empty(&self) -> bool {
        self.pool_count() == 0
    }
}

impl PreparedBundleHandler {
    /// Returns the warmed pool backing this prepared handler.
    pub fn pool(&self) -> &BundlePool {
        &self.pool
    }

    /// Returns the cached handler session.
    pub fn handler(&self) -> &HandlerSession {
        self.handler.as_ref()
    }

    /// Invokes the handler using the default strategy.
    pub fn call_default(&self) -> Result<crate::ExecutionOutcome> {
        self.pool.call_default(self.handler())
    }

    /// Invokes the handler using JSON adapters.
    pub fn call_json(&self, input: Option<JsonValue>) -> Result<crate::ExecutionOutcome> {
        self.pool.call_json(self.handler(), input)
    }

    /// Invokes the handler using a prepared JSON adapter input.
    pub fn call_json_input(&self, input: Option<JsonInput>) -> Result<crate::ExecutionOutcome> {
        self.pool.call_json_input(self.handler(), input)
    }

    /// Invokes the handler using RawCtx adapters.
    pub fn call_rawctx(&self, inputs: Vec<RawCtxInput>) -> Result<crate::ExecutionOutcome> {
        self.pool.call_rawctx(self.handler(), inputs)
    }

    /// Executes a JSON warmup once on every currently-created idle isolate.
    pub fn warm_all_json(&self, input: Option<JsonValue>) -> Result<Vec<crate::ExecutionOutcome>> {
        self.pool.warm_all_json(self.handler(), input)
    }

    /// Executes a JSON warmup with a prepared input once on every currently-created idle isolate.
    pub fn warm_all_json_input(
        &self,
        input: Option<JsonInput>,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.pool.warm_all_json_input(self.handler(), input)
    }

    /// Executes a RawCtx warmup once on every currently-created idle isolate.
    pub fn warm_all_rawctx(
        &self,
        inputs: Vec<RawCtxInput>,
    ) -> Result<Vec<crate::ExecutionOutcome>> {
        self.pool.warm_all_rawctx(self.handler(), inputs)
    }

    /// Executes a RawCtx warmup once on every currently-created idle isolate.
    pub fn warm_all_rawctx_with<F>(
        &self,
        inputs_for_isolate: F,
    ) -> Result<Vec<crate::ExecutionOutcome>>
    where
        F: FnMut(usize) -> Result<Vec<RawCtxInput>>,
    {
        self.pool
            .warm_all_rawctx_with(self.handler(), inputs_for_isolate)
    }

    /// Executes a default-strategy warmup once on every currently-created idle isolate.
    pub fn warm_all_default(&self) -> Result<Vec<crate::ExecutionOutcome>> {
        self.pool.warm_all_default(self.handler())
    }
}
