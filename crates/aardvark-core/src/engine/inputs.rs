use crate::error::{PyRunnerError, Result};
use v8::{self, Uint8Array};

use super::JsRuntime;

impl JsRuntime {
    pub fn set_python_json_f32_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(bytes);
            let shared = backing.make_shared();
            let byte_len = shared.byte_length();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let typed = Uint8Array::new(scope, array_buffer, 0, byte_len).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON f32 input buffer".into())
            })?;
            let key = v8::String::new(scope, "__aardvarkJsonInputBuffer").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input buffer key".into())
            })?;
            global.set(scope, key.into(), typed.into());
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "f32").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }

    pub fn set_python_json_utf8_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(bytes);
            let shared = backing.make_shared();
            let byte_len = shared.byte_length();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let typed = Uint8Array::new(scope, array_buffer, 0, byte_len).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON utf8 input buffer".into())
            })?;
            let key = v8::String::new(scope, "__aardvarkJsonInputBuffer").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input buffer key".into())
            })?;
            global.set(scope, key.into(), typed.into());
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "utf8").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }

    pub fn set_python_json_bytes_input(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(bytes);
            let shared = backing.make_shared();
            let byte_len = shared.byte_length();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let typed = Uint8Array::new(scope, array_buffer, 0, byte_len).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON bytes input buffer".into())
            })?;
            let key = v8::String::new(scope, "__aardvarkJsonInputBuffer").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input buffer key".into())
            })?;
            global.set(scope, key.into(), typed.into());
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "bytes").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }

    pub fn set_python_json_encoded_input(&mut self, encoded: String) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__aardvarkJsonInputEncoded").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON encoded input key".into())
            })?;
            let value = v8::String::new(scope, &encoded).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON encoded input".into())
            })?;
            global.set(scope, key.into(), value.into());
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "json").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }

    pub fn set_python_json_single_i64_object_input(&mut self, key: &str, value: i64) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key_name = v8::String::new(scope, "__aardvarkJsonInputKey").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input key name".into())
            })?;
            let key_value = v8::String::new(scope, key).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input object key".into())
            })?;
            global.set(scope, key_name.into(), key_value.into());
            let value_key = v8::String::new(scope, "__aardvarkJsonInputI64").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON i64 input key".into())
            })?;
            global.set(
                scope,
                value_key.into(),
                v8::Number::new(scope, value as f64).into(),
            );
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "single_i64_object").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }

    pub fn clear_python_json_input(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let mode_key = v8::String::new(scope, "__aardvarkJsonInputMode").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode key".into())
            })?;
            let mode_value = v8::String::new(scope, "none").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate JSON input mode value".into())
            })?;
            global.set(scope, mode_key.into(), mode_value.into());
            Ok(())
        })
    }
}
