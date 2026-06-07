use std::cell::Cell;
use std::convert::TryFrom;
use std::rc::Rc;

use crate::asset_store::Asset;
use crate::error::{PyRunnerError, Result};
use v8::{
    self, script_compiler, Context, FixedArray, Local, Module, ModuleRequest, Object, PinScope,
    String as V8String,
};

use super::RuntimeContext;

thread_local! {
    static TLS_RUNTIME_CONTEXT: Cell<*const RuntimeContext> = const { Cell::new(std::ptr::null()) };
    static TLS_SCOPE: Cell<*mut std::ffi::c_void> = const { Cell::new(std::ptr::null_mut()) };
}

pub(super) fn compile_module_from_assets<'a>(
    scope: &mut PinScope<'a, '_>,
    ctx: &Rc<RuntimeContext>,
    specifier: &str,
) -> Result<Local<'a, Module>> {
    if let Some(existing) = ctx.modules.borrow().get(specifier) {
        return Ok(Local::new(scope, existing));
    }
    let asset = ctx
        .assets
        .get(specifier)
        .ok_or_else(|| PyRunnerError::Execution(format!("module asset not found: {specifier}")))?;
    let source_text = match asset {
        Asset::Text(text) => text,
        Asset::Binary(_) => {
            return Err(PyRunnerError::Execution(format!(
                "module asset '{specifier}' is binary"
            )))
        }
    };
    let source_str: &str = &source_text;
    let source_string = v8::String::new(scope, source_str).ok_or_else(|| {
        PyRunnerError::Execution(format!("failed to allocate source string for {specifier}"))
    })?;
    let resource_name = v8::String::new(scope, specifier).ok_or_else(|| {
        PyRunnerError::Execution(format!("failed to allocate resource name for {specifier}"))
    })?;
    let origin = v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        true,
        None,
    );
    let mut source = script_compiler::Source::new(source_string, Some(&origin));
    let module = script_compiler::compile_module(scope, &mut source)
        .ok_or_else(|| PyRunnerError::Execution(format!("failed to compile module {specifier}")))?;
    let global = v8::Global::new(scope, module);
    ctx.modules
        .borrow_mut()
        .insert(specifier.to_owned(), global);
    ctx.module_by_hash
        .borrow_mut()
        .insert(module.get_identity_hash().get(), specifier.to_owned());
    // Recursively compile dependencies so instantiation can resolve them from the cache.
    let requests = module.get_module_requests();
    let len = requests.length();
    for i in 0..len {
        if let Some(data) = requests.get(scope, i) {
            if let Ok(request) = Local::<ModuleRequest>::try_from(data) {
                let request_spec = request.get_specifier().to_rust_string_lossy(scope);
                let resolved = resolve_specifier(specifier, &request_spec);
                compile_module_from_assets(scope, ctx, &resolved)?;
            }
        }
    }

    Ok(module)
}

pub(super) fn instantiate_module(
    scope: &mut PinScope<'_, '_>,
    ctx: &Rc<RuntimeContext>,
    module: Local<Module>,
) -> Result<()> {
    let scope_ptr = scope as *mut _ as *mut std::ffi::c_void;
    TLS_RUNTIME_CONTEXT.with(|cell| cell.set(Rc::as_ptr(ctx)));
    TLS_SCOPE.with(|cell| cell.set(scope_ptr));
    let instantiated = module.instantiate_module(scope, resolve_module_callback);
    TLS_SCOPE.with(|cell| cell.set(std::ptr::null_mut()));
    TLS_RUNTIME_CONTEXT.with(|cell| cell.set(std::ptr::null()));
    if instantiated.is_some() {
        Ok(())
    } else {
        Err(PyRunnerError::Execution(
            "failed to instantiate module".into(),
        ))
    }
}

pub(super) fn evaluate_module(
    scope: &mut PinScope<'_, '_>,
    ctx: &Rc<RuntimeContext>,
    module: Local<Module>,
    specifier: &str,
) -> Result<()> {
    module
        .evaluate(scope)
        .ok_or_else(|| PyRunnerError::Execution("module evaluation failed".into()))?;
    let namespace_value = module.get_module_namespace();
    let namespace_obj = v8::Local::<Object>::try_from(namespace_value).map_err(|_| {
        PyRunnerError::Execution(format!(
            "module namespace for {specifier} was not an object"
        ))
    })?;
    ctx.module_namespaces
        .borrow_mut()
        .insert(specifier.to_owned(), v8::Global::new(scope, namespace_obj));
    Ok(())
}

fn resolve_module_callback<'a>(
    _context: Local<'a, Context>,
    specifier: Local<'a, V8String>,
    _import_assertions: Local<'a, FixedArray>,
    referencing_module: Local<'a, Module>,
) -> Option<Local<'a, Module>> {
    let scope_ptr = TLS_SCOPE.with(|cell| cell.get()) as *mut PinScope<'static, 'static>;
    let ctx_ptr = TLS_RUNTIME_CONTEXT.with(|cell| cell.get());
    if scope_ptr.is_null() || ctx_ptr.is_null() {
        return None;
    }
    // SAFETY: V8 invokes the resolver synchronously while `instantiate_module`
    // has installed these thread-local pointers.
    let scope_ref = unsafe { &mut *scope_ptr };
    // SAFETY: The context pointer is installed for the same resolver call and
    // outlives the callback frame.
    let ctx_ref = unsafe { &*ctx_ptr };
    let request = specifier.to_rust_string_lossy(scope_ref);
    let parent_hash = referencing_module.get_identity_hash().get();
    let base = ctx_ref
        .module_by_hash
        .borrow()
        .get(&parent_hash)
        .cloned()
        .unwrap_or_default();
    let resolved = resolve_specifier(&base, &request);
    let maybe_module = ctx_ref
        .modules
        .borrow()
        .get(&resolved)
        .cloned()
        .map(|global| Local::new(scope_ref, &global));
    if let Some(module) = maybe_module {
        Some(module)
    } else {
        if let Some(message) = v8::String::new(
            scope_ref,
            &format!("unresolved module specifier: {resolved}"),
        ) {
            scope_ref.throw_exception(message.into());
        }
        None
    }
}

fn resolve_specifier(base: &str, request: &str) -> String {
    if request.starts_with("./") || request.starts_with("../") {
        let mut parts: Vec<&str> = if base.is_empty() {
            Vec::new()
        } else {
            base.rsplit_once('/')
                .map(|(prefix, _)| prefix.split('/').collect())
                .unwrap_or_default()
        };
        for segment in request.split('/') {
            match segment {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        return parts.join("/");
    }
    request.trim_start_matches("./").to_owned()
}

pub(super) fn normalize_specifier(spec: &str) -> String {
    if spec.starts_with("./") {
        spec.trim_start_matches("./").to_owned()
    } else {
        spec.to_owned()
    }
}
