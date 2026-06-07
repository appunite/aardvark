use std::rc::Rc;

use crate::asset_store::Asset;
use tracing::{debug, info, warn};
use v8::{self, FunctionCallbackArguments, PinScope, ReturnValue, Uint8Array, Value};

use super::RuntimeContext;

enum ConsoleStream {
    Stdout,
    Stderr,
}

pub(super) fn asset_fetch_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let name = if args.length() > 0 {
        args.get(0)
            .to_string(scope)
            .map(|s| s.to_rust_string_lossy(scope))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    let Some(asset) = context_state.assets.get(&name) else {
        debug!("asset request for '{name}' not found");
        rv.set(v8::undefined(scope).into());
        return;
    };

    let result = v8::Object::new(scope);
    let Some(kind_key) = v8::String::new(scope, "kind") else {
        warn!("failed to allocate asset response kind key");
        rv.set(v8::undefined(scope).into());
        return;
    };
    let Some(data_key) = v8::String::new(scope, "data") else {
        warn!("failed to allocate asset response data key");
        rv.set(v8::undefined(scope).into());
        return;
    };
    let Some(size_key) = v8::String::new(scope, "size") else {
        warn!("failed to allocate asset response size key");
        rv.set(v8::undefined(scope).into());
        return;
    };

    match asset {
        Asset::Text(text) => {
            let Some(kind_value) = v8::String::new(scope, "text") else {
                warn!("failed to allocate text asset kind");
                rv.set(v8::undefined(scope).into());
                return;
            };
            let Some(data_value) = v8::String::new(scope, &text) else {
                warn!("failed to allocate text asset data");
                rv.set(v8::undefined(scope).into());
                return;
            };
            let size_value = v8::Number::new(scope, text.len() as f64);
            let _ = result.set(scope, kind_key.into(), kind_value.into());
            let _ = result.set(scope, data_key.into(), data_value.into());
            let _ = result.set(scope, size_key.into(), size_value.into());
        }
        Asset::Binary(bytes) => {
            let Some(kind_value) = v8::String::new(scope, "binary") else {
                warn!("failed to allocate binary asset kind");
                rv.set(v8::undefined(scope).into());
                return;
            };
            let vec = bytes.as_ref().to_vec();
            let backing = v8::ArrayBuffer::new_backing_store_from_vec(vec);
            let shared = backing.make_shared();
            let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
            let length = array_buffer.byte_length();
            let Some(typed) = Uint8Array::new(scope, array_buffer, 0, length) else {
                warn!("failed to allocate binary asset typed array");
                rv.set(v8::undefined(scope).into());
                return;
            };
            let size_value = v8::Number::new(scope, length as f64);
            let _ = result.set(scope, kind_key.into(), kind_value.into());
            let _ = result.set(scope, data_key.into(), typed.into());
            let _ = result.set(scope, size_key.into(), size_value.into());
        }
    }

    rv.set(result.into());
}

pub(super) fn record_buffer_event_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let event = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if event.is_empty() {
        warn!(
            target = "aardvark::buffers",
            "shared buffer event missing name"
        );
        rv.set(v8::undefined(scope).into());
        return;
    }

    let buffer_id = args
        .get(1)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default();
    if buffer_id.is_empty() {
        warn!(
            target = "aardvark::buffers",
            buffers.event = event.as_str(),
            "shared buffer event missing id"
        );
        rv.set(v8::undefined(scope).into());
        return;
    }

    let size = if args.length() > 2 {
        args.get(2).number_value(scope).unwrap_or(0.0).max(0.0)
    } else {
        0.0
    } as u64;

    let mut metadata_json: Option<String> = None;
    if args.length() > 3 {
        let meta_value = args.get(3);
        if !meta_value.is_null_or_undefined() {
            if let Some(stringified) = v8::json::stringify(scope, meta_value) {
                metadata_json = Some(stringified.to_rust_string_lossy(scope));
            } else {
                warn!(
                    target = "aardvark::buffers",
                    buffers.event = event.as_str(),
                    buffers.id = buffer_id.as_str(),
                    "shared buffer metadata stringify failed"
                );
            }
        }
    }

    info!(
        target = "aardvark::buffers",
        buffers.event = event.as_str(),
        buffers.id = buffer_id.as_str(),
        buffers.size = size,
        buffers.metadata = metadata_json.as_deref(),
        "shared buffer event"
    );

    rv.set(v8::undefined(scope).into());
}

pub(super) fn filesystem_violation_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let message = args
        .get(0)
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_else(|| "filesystem violation".to_string());
    let path_value = if args.length() > 1 {
        let value = args.get(1);
        if value.is_null_or_undefined() {
            None
        } else {
            value
                .to_string(scope)
                .map(|s| s.to_rust_string_lossy(scope))
        }
    } else {
        None
    };

    if let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() {
        context_state.record_filesystem_violation(path_value, message);
    } else {
        warn!(
            target = "aardvark::sandbox",
            "filesystem violation reported without runtime context"
        );
    }

    rv.set(v8::undefined(scope).into());
}

pub(super) fn native_log_callback(
    scope: &mut PinScope<'_, '_>,
    args: FunctionCallbackArguments<'_>,
    mut rv: ReturnValue<'_, Value>,
) {
    let mut parts = Vec::with_capacity(args.length() as usize);
    for index in 0..args.length() {
        let value = args.get(index);
        if let Some(text) = value.to_string(scope) {
            parts.push(text.to_rust_string_lossy(scope));
        }
    }

    let mut stream = ConsoleStream::Stdout;
    let mut start_index = 0;
    if let Some(first) = parts.first() {
        match first.as_str() {
            "__stderr__" => {
                stream = ConsoleStream::Stderr;
                start_index = 1;
            }
            "__stdout__" => {
                stream = ConsoleStream::Stdout;
                start_index = 1;
            }
            _ => {}
        }
    }

    let message = if start_index >= parts.len() {
        String::new()
    } else {
        parts[start_index..].join(" ")
    };

    if let Some(context_state) = scope.get_slot::<Rc<RuntimeContext>>() {
        match stream {
            ConsoleStream::Stdout => context_state.append_stdout(&message),
            ConsoleStream::Stderr => context_state.append_stderr(&message),
        }
    }

    if !message.is_empty() {
        match stream {
            ConsoleStream::Stdout => {
                info!(target = "aardvark::js", "{}", message);
            }
            ConsoleStream::Stderr => {
                warn!(target = "aardvark::js", "{}", message);
            }
        }
    }
    rv.set(v8::undefined(scope).into());
}
