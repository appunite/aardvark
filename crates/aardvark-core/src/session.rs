//! Session state prepared for execution inside the runtime.

use crate::bundle::Bundle;
use crate::invocation::InvocationDescriptor;
use once_cell::sync::OnceCell;
use std::sync::Arc;

/// Represents a prepared execution context for a specific bundle.
pub struct PySession {
    bundle: Bundle,
    descriptor: InvocationDescriptor,
    rawctx_spec_json: OnceCell<Option<Arc<String>>>,
}

impl PySession {
    pub(crate) fn new(bundle: Bundle, descriptor: InvocationDescriptor) -> Self {
        Self {
            bundle,
            descriptor,
            rawctx_spec_json: OnceCell::new(),
        }
    }

    /// Returns the canonical entrypoint (module:function or script) to execute.
    pub fn entrypoint(&self) -> &str {
        self.descriptor.entrypoint()
    }

    /// Provides read-only access to bundle entries.
    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    /// Returns the invocation descriptor driving this session.
    pub fn descriptor(&self) -> &InvocationDescriptor {
        &self.descriptor
    }

    pub(crate) fn rawctx_spec_json<E, F>(&self, build: F) -> Result<Option<Arc<String>>, E>
    where
        F: FnOnce() -> Result<Option<String>, E>,
    {
        Ok(self
            .rawctx_spec_json
            .get_or_try_init(|| build().map(|value| value.map(Arc::new)))?
            .clone())
    }

    /// Returns a simple manifest of the bundle contents.
    pub fn manifest(&self) -> impl Iterator<Item = (&str, usize)> {
        self.bundle
            .entries()
            .iter()
            .map(|entry| (entry.path(), entry.contents().len()))
    }
}
