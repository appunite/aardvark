use std::convert::TryFrom;
use std::rc::Rc;

use v8::{self, Function, Local, Object, PinScope, Value};

use crate::error::{PyRunnerError, Result};

use super::JsRuntime;

const HOST_HOOKS_GLOBAL: &str = "__aardvarkHostHooks";
const LEGACY_HOST_HOOK_GLOBALS: &[&str] = &[
    "__aardvarkSetHostCapabilities",
    "__aardvarkFilesystemSetPolicy",
    "__aardvarkFilesystemReset",
    "__aardvarkFilesystemGetUsage",
    "__aardvarkHostCollectSharedBuffers",
    "__aardvarkHostDrainSharedBuffers",
    "__aardvarkHostReleaseSharedBuffers",
    "__aardvarkHostResetSharedBuffers",
];

struct HostHooks {
    object: v8::Global<Object>,
}

impl JsRuntime {
    /// Moves bootstrap-created host-only hooks out of the guest-visible global namespace.
    pub(crate) fn seal_host_hooks(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            seal_host_hooks(scope, global)
        })
    }
}

pub(super) fn get_host_hook<'a>(
    scope: &mut PinScope<'a, '_>,
    name: &str,
) -> Result<Option<Local<'a, Function>>> {
    let Some(hooks) = host_hooks(scope) else {
        return Ok(None);
    };
    function_property(scope, hooks, name)
}

pub(super) fn get_nested_host_hook<'a>(
    scope: &mut PinScope<'a, '_>,
    namespace: &str,
    name: &str,
) -> Result<Option<Local<'a, Function>>> {
    let Some(hooks) = host_hooks(scope) else {
        return Ok(None);
    };
    let Some(namespace_value) = object_property(scope, hooks, namespace)? else {
        return Ok(None);
    };
    let Ok(namespace_object) = Local::<Object>::try_from(namespace_value) else {
        return Ok(None);
    };
    function_property(scope, namespace_object, name)
}

fn seal_host_hooks<'a>(scope: &mut PinScope<'a, '_>, global: Local<'a, Object>) -> Result<()> {
    let context = scope.get_current_context();
    if context.get_slot::<HostHooks>().is_some() {
        return Ok(());
    }

    let handoff_key = v8::String::new(scope, HOST_HOOKS_GLOBAL).ok_or_else(|| {
        PyRunnerError::Execution("failed to allocate host hooks handoff key".into())
    })?;
    let hooks = match global.get(scope, handoff_key.into()) {
        Some(value) if !value.is_null_or_undefined() => Local::<Object>::try_from(value)
            .map_err(|_| PyRunnerError::Execution("host hooks handoff is not an object".into()))?,
        _ => legacy_host_hooks(scope, global)?
            .ok_or_else(|| PyRunnerError::Execution("host hooks handoff missing".into()))?,
    };

    context.set_slot(Rc::new(HostHooks {
        object: v8::Global::new(scope, hooks),
    }));
    let _ = global.delete(scope, handoff_key.into());
    for name in LEGACY_HOST_HOOK_GLOBALS {
        delete_property(scope, global, name)?;
    }
    Ok(())
}

fn host_hooks<'a>(scope: &mut PinScope<'a, '_>) -> Option<Local<'a, Object>> {
    let hooks = scope.get_current_context().get_slot::<HostHooks>()?;
    Some(v8::Local::new(scope, &hooks.object))
}

fn function_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
) -> Result<Option<Local<'a, Function>>> {
    let Some(value) = object_property(scope, object, name)? else {
        return Ok(None);
    };
    Ok(Local::<Function>::try_from(value).ok())
}

fn object_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
) -> Result<Option<Local<'a, Value>>> {
    let key = v8::String::new(scope, name)
        .ok_or_else(|| PyRunnerError::Execution("failed to allocate host hook key".into()))?;
    Ok(object.get(scope, key.into()))
}

fn legacy_host_hooks<'a>(
    scope: &mut PinScope<'a, '_>,
    global: Local<'a, Object>,
) -> Result<Option<Local<'a, Object>>> {
    let hooks = v8::Object::new(scope);
    let mut found = false;

    if let Some(func) = function_property(scope, global, "__aardvarkSetHostCapabilities")? {
        set_property(scope, hooks, "setHostCapabilities", func.into())?;
        found = true;
    }

    let filesystem = v8::Object::new(scope);
    let mut has_filesystem = false;
    if let Some(func) = function_property(scope, global, "__aardvarkFilesystemSetPolicy")? {
        set_property(scope, filesystem, "setPolicy", func.into())?;
        has_filesystem = true;
    }
    if let Some(func) = function_property(scope, global, "__aardvarkFilesystemReset")? {
        set_property(scope, filesystem, "reset", func.into())?;
        has_filesystem = true;
    }
    if let Some(func) = function_property(scope, global, "__aardvarkFilesystemGetUsage")? {
        set_property(scope, filesystem, "getUsage", func.into())?;
        has_filesystem = true;
    }
    if has_filesystem {
        set_property(scope, hooks, "filesystem", filesystem.into())?;
        found = true;
    }

    let shared_buffers = v8::Object::new(scope);
    let mut has_shared_buffers = false;
    if let Some(func) = function_property(scope, global, "__aardvarkHostCollectSharedBuffers")? {
        set_property(scope, shared_buffers, "collect", func.into())?;
        has_shared_buffers = true;
    }
    if let Some(func) = function_property(scope, global, "__aardvarkHostDrainSharedBuffers")? {
        set_property(scope, shared_buffers, "drain", func.into())?;
        has_shared_buffers = true;
    }
    if let Some(func) = function_property(scope, global, "__aardvarkHostReleaseSharedBuffers")? {
        set_property(scope, shared_buffers, "release", func.into())?;
        has_shared_buffers = true;
    }
    if let Some(func) = function_property(scope, global, "__aardvarkHostResetSharedBuffers")? {
        set_property(scope, shared_buffers, "reset", func.into())?;
        has_shared_buffers = true;
    }
    if has_shared_buffers {
        set_property(scope, hooks, "sharedBuffers", shared_buffers.into())?;
        found = true;
    }

    Ok(found.then_some(hooks))
}

fn set_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
    value: Local<'a, Value>,
) -> Result<()> {
    let key = v8::String::new(scope, name)
        .ok_or_else(|| PyRunnerError::Execution("failed to allocate host hook key".into()))?;
    let stored = object
        .set(scope, key.into(), value)
        .ok_or_else(|| PyRunnerError::Execution("failed to set host hook property".into()))?;
    if stored {
        Ok(())
    } else {
        Err(PyRunnerError::Execution(
            "host hook property was not stored".into(),
        ))
    }
}

fn delete_property<'a>(
    scope: &mut PinScope<'a, '_>,
    object: Local<'a, Object>,
    name: &str,
) -> Result<()> {
    let key = v8::String::new(scope, name)
        .ok_or_else(|| PyRunnerError::Execution("failed to allocate host hook key".into()))?;
    let _ = object.delete(scope, key.into());
    Ok(())
}
