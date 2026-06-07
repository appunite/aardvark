use crate::error::{PyRunnerError, Result};
use v8::{self};

use super::{host_hooks::get_nested_host_hook, FilesystemModeConfig, JsRuntime};

impl JsRuntime {
    /// Applies filesystem mode and quota before executing user code.
    pub fn set_filesystem_policy(
        &mut self,
        mode: FilesystemModeConfig,
        quota_bytes: Option<u64>,
    ) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let func =
                get_nested_host_hook(scope, "filesystem", "setPolicy")?.ok_or_else(|| {
                    PyRunnerError::Execution("filesystem policy hook unavailable".into())
                })?;
            let policy = v8::Object::new(scope);
            let mode_key = v8::String::new(scope, "mode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate filesystem mode key".into())
            })?;
            let mode_value = v8::String::new(
                scope,
                match mode {
                    FilesystemModeConfig::Read => "read",
                    FilesystemModeConfig::ReadWrite => "readWrite",
                },
            )
            .ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate filesystem mode value".into())
            })?;
            let stored = policy
                .set(scope, mode_key.into(), mode_value.into())
                .ok_or_else(|| {
                    PyRunnerError::Execution("failed to set filesystem mode policy".into())
                })?;
            if !stored {
                return Err(PyRunnerError::Execution(
                    "filesystem mode policy was not stored".into(),
                ));
            }
            if let Some(quota) = quota_bytes {
                let quota_key = v8::String::new(scope, "quotaBytes").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate filesystem quota key".into())
                })?;
                let quota_value = v8::Number::new(scope, quota as f64);
                let stored = policy
                    .set(scope, quota_key.into(), quota_value.into())
                    .ok_or_else(|| {
                        PyRunnerError::Execution("failed to set filesystem quota policy".into())
                    })?;
                if !stored {
                    return Err(PyRunnerError::Execution(
                        "filesystem quota policy was not stored".into(),
                    ));
                }
            }
            func.call(scope, global.into(), &[policy.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("filesystem policy update failed".into())
                })?;
            Ok(())
        })
    }

    /// Resets the session scratch filesystem after an invocation completes.
    pub fn reset_filesystem(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let func = get_nested_host_hook(scope, "filesystem", "reset")?.ok_or_else(|| {
                PyRunnerError::Execution("filesystem reset hook unavailable".into())
            })?;
            func.call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("filesystem reset failed".into()))?;
            Ok(())
        })
    }

    /// Returns the current byte usage of the session scratch filesystem.
    pub fn filesystem_usage_bytes(&mut self) -> Result<u64> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let func = get_nested_host_hook(scope, "filesystem", "getUsage")?.ok_or_else(|| {
                PyRunnerError::Execution("filesystem usage hook unavailable".into())
            })?;
            let usage_value = func
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("filesystem usage query failed".into()))?;
            let number = usage_value
                .to_number(scope)
                .ok_or_else(|| PyRunnerError::Execution("filesystem usage not a number".into()))?;
            let value = number.value();
            Ok(if value <= 0.0 { 0 } else { value as u64 })
        })
    }
}
