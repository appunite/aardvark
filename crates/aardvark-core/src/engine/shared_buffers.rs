use std::convert::TryFrom;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value as JsonValue;
use v8::{self, Array, Local, Object, PinScope, Uint8Array, Value};

use crate::error::{PyRunnerError, Result};

use super::{host_hooks::get_nested_host_hook, JsRuntime};

#[derive(Debug, Clone)]
pub struct SharedBuffer {
    pub id: String,
    pub length: usize,
    pub metadata: Option<JsonValue>,
    pub backing: Option<Arc<SharedBufferBacking>>,
    pub bytes: Option<Bytes>,
}

#[derive(Debug)]
pub struct SharedBufferBacking {
    store: v8::SharedRef<v8::BackingStore>,
    offset: usize,
    length: usize,
}

impl SharedBufferBacking {
    pub(in crate::engine) fn new(
        store: v8::SharedRef<v8::BackingStore>,
        offset: usize,
        length: usize,
    ) -> Self {
        Self {
            store,
            offset,
            length,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.length == 0 {
            return &[];
        }
        let Some(ptr) = self.store.data() else {
            return &[];
        };
        let store_size = self.store.byte_length();
        if self.offset > store_size || self.length > store_size {
            return &[];
        }
        if let Some(end) = self.offset.checked_add(self.length) {
            if end > store_size {
                return &[];
            }
        } else {
            return &[];
        }
        // SAFETY: The backing store is retained by `self.store`, and the
        // offset/length checks above prove the returned slice is in-bounds.
        unsafe {
            let data = ptr.as_ptr().add(self.offset) as *const u8;
            std::slice::from_raw_parts(data, self.length)
        }
    }
}

pub(super) fn drain_shared_buffers<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
) -> Result<Vec<SharedBuffer>> {
    let mut buffers = Vec::new();
    if let Some(drain_fn) = get_nested_host_hook(scope, "sharedBuffers", "drain")? {
        let result = drain_fn
            .call(scope, global.into(), &[])
            .ok_or_else(|| PyRunnerError::Execution("drain shared buffers call failed".into()))?;
        shared_buffers_from_value(scope, result, &mut buffers)?;
        return Ok(buffers);
    }

    let buffers = collect_shared_buffers(scope, global)?;
    if !buffers.is_empty() {
        let release_ids: Vec<String> = buffers.iter().map(|buffer| buffer.id.clone()).collect();
        release_shared_buffers(scope, global, &release_ids)?;
    }
    Ok(buffers)
}

impl JsRuntime {
    /// Clears published shared buffers between invocations.
    pub fn reset_shared_buffers(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            if let Some(func) = get_nested_host_hook(scope, "sharedBuffers", "reset")? {
                func.call(scope, global.into(), &[])
                    .ok_or_else(|| PyRunnerError::Execution("shared buffer reset failed".into()))?;
            }
            Ok(())
        })
    }
}

fn collect_shared_buffers<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
) -> Result<Vec<SharedBuffer>> {
    let mut buffers = Vec::new();
    let Some(collect_fn) = get_nested_host_hook(scope, "sharedBuffers", "collect")? else {
        return Ok(buffers);
    };
    let result = collect_fn
        .call(scope, global.into(), &[])
        .ok_or_else(|| PyRunnerError::Execution("collect shared buffers call failed".into()))?;
    shared_buffers_from_value(scope, result, &mut buffers)?;
    Ok(buffers)
}

fn shared_buffers_from_value<'a>(
    scope: &mut PinScope<'a, '_>,
    value: Local<'a, Value>,
    buffers: &mut Vec<SharedBuffer>,
) -> Result<()> {
    let Ok(array) = Local::<Array>::try_from(value) else {
        return Ok(());
    };
    let length = array.length();
    if length == 0 {
        return Ok(());
    }

    let id_key = v8::String::new(scope, "id").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate shared buffer id key".into())
    })?;
    let buffer_key = v8::String::new(scope, "buffer").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate shared buffer payload key".into())
    })?;
    let metadata_key = v8::String::new(scope, "metadata").ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate shared buffer metadata key".into())
    })?;

    for index in 0..length {
        let entry_value = array
            .get_index(scope, index)
            .ok_or_else(|| PyRunnerError::Execution("shared buffer entry missing".into()))?;
        let entry_obj = entry_value
            .to_object(scope)
            .ok_or_else(|| PyRunnerError::Execution("shared buffer entry not an object".into()))?;
        let id_value = entry_obj
            .get(scope, id_key.into())
            .ok_or_else(|| PyRunnerError::Execution("shared buffer missing id".into()))?;
        let id = id_value
            .to_string(scope)
            .ok_or_else(|| PyRunnerError::Execution("failed to stringify buffer id".into()))?
            .to_rust_string_lossy(scope);

        let buffer_value = entry_obj
            .get(scope, buffer_key.into())
            .ok_or_else(|| PyRunnerError::Execution("shared buffer missing payload".into()))?;
        let typed_array = Local::<Uint8Array>::try_from(buffer_value).map_err(|_| {
            PyRunnerError::Execution("shared buffer payload is not a Uint8Array".into())
        })?;
        let byte_len = typed_array.byte_length();
        let array_buffer = typed_array.buffer(scope).ok_or_else(|| {
            PyRunnerError::Execution("shared buffer missing backing store".into())
        })?;
        let backing_store = array_buffer.get_backing_store();
        let offset = typed_array.byte_offset();

        let metadata = match entry_obj.get(scope, metadata_key.into()) {
            Some(value) if !value.is_null_or_undefined() => {
                let json_value = v8::json::stringify(scope, value).ok_or_else(|| {
                    PyRunnerError::Execution("failed to stringify shared buffer metadata".into())
                })?;
                let json_str = json_value.to_rust_string_lossy(scope);
                Some(serde_json::from_str(&json_str).map_err(|err| {
                    PyRunnerError::Execution(format!(
                        "failed to parse shared buffer metadata: {err}"
                    ))
                })?)
            }
            _ => None,
        };

        buffers.push(SharedBuffer {
            id,
            length: byte_len,
            metadata,
            backing: Some(Arc::new(SharedBufferBacking::new(
                backing_store,
                offset,
                byte_len,
            ))),
            bytes: None,
        });
    }

    Ok(())
}

fn release_shared_buffers<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
    ids: &[String],
) -> Result<()> {
    let Some(release_fn) = get_nested_host_hook(scope, "sharedBuffers", "release")? else {
        return Ok(());
    };

    let mut args: Vec<Local<Value>> = Vec::new();
    if !ids.is_empty() {
        let id_array = Array::new(scope, ids.len() as i32);
        for (index, id) in ids.iter().enumerate() {
            let id_value = v8::String::new(scope, id).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate buffer id string".into())
            })?;
            id_array.set_index(scope, index as u32, id_value.into());
        }
        args.push(id_array.into());
    }

    release_fn
        .call(scope, global.into(), &args)
        .ok_or_else(|| PyRunnerError::Execution("release shared buffers call failed".into()))?;
    Ok(())
}
