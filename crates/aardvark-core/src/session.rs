//! Session state prepared for execution inside the runtime.

use crate::bundle::Bundle;
use crate::invocation::InvocationDescriptor;

/// Represents a prepared execution context for a specific bundle.
pub struct PySession {
    bundle: Bundle,
    descriptor: InvocationDescriptor,
}

impl PySession {
    pub(crate) fn new(bundle: Bundle, descriptor: InvocationDescriptor) -> Self {
        Self { bundle, descriptor }
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

    /// Returns a simple manifest of the bundle contents.
    pub fn manifest(&self) -> impl Iterator<Item = (&str, usize)> {
        self.bundle
            .entries()
            .iter()
            .map(|entry| (entry.path(), entry.contents().len()))
    }
}
