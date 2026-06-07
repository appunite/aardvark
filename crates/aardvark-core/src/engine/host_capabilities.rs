use crate::error::{PyRunnerError, Result};

use super::{host_hooks::get_host_hook, JsRuntime};

impl JsRuntime {
    /// Applies the active host capabilities for native APIs exposed to guest code.
    pub fn set_host_capabilities(&mut self, capabilities: &[String]) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let func = get_host_hook(scope, "setHostCapabilities")?.ok_or_else(|| {
                PyRunnerError::Execution("host capability hook unavailable".into())
            })?;
            let array = v8::Array::new(scope, capabilities.len() as i32);
            for (index, capability) in capabilities.iter().enumerate() {
                let value = v8::String::new(scope, capability).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate host capability value".into())
                })?;
                let stored = array
                    .set_index(scope, index as u32, value.into())
                    .ok_or_else(|| {
                        PyRunnerError::Execution("failed to store host capability value".into())
                    })?;
                if !stored {
                    return Err(PyRunnerError::Execution(
                        "host capability value was not stored".into(),
                    ));
                }
            }
            func.call(scope, global.into(), &[array.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("applying host capabilities failed".into())
                })?;
            Ok(())
        })
    }
}
