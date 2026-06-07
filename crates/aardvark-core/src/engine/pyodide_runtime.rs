use std::convert::TryFrom;

use crate::bundle::Bundle;
use crate::error::{PyRunnerError, Result};
use v8::{self, Function, Local, Object, Promise, PromiseState, Uint8Array};

use super::bootstrap_sources::{JSON_RESULT_BUFFER_HELPER, PYTHON_ENTRYPOINT_HELPER};
use super::{
    copy_typed_array, exec_script, JsRuntime, OverlayBlob, OverlayExport, PyodideLoadOptions,
};

impl JsRuntime {
    /// Loads the Pyodide runtime by calling the embedded loader module.
    pub fn load_pyodide(&mut self, options: PyodideLoadOptions<'_>) -> Result<()> {
        if self.context_state.pyodide_instance.borrow().is_some() {
            return Ok(());
        }
        let ctx_state = self.context_state.clone();
        self.ensure_module("pyodide.mjs")?;
        self.ensure_module("pyodide_bootstrap.js")?;
        let mut promise_handle: Option<v8::Global<Promise>> = None;
        self.with_context(|scope, _| {
            let bootstrap = ctx_state
                .module_namespace(scope, "pyodide_bootstrap.js")
                .ok_or_else(|| {
                    PyRunnerError::Execution("pyodide_bootstrap.js namespace unavailable".into())
                })?;
            let load_key = v8::String::new(scope, "loadPyRunnerPyodide").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate Pyodide load key".into())
            })?;
            let load_value = bootstrap.get(scope, load_key.into());
            let load_fn = load_value
                .and_then(|value| Local::<Function>::try_from(value).ok())
                .ok_or_else(|| {
                    PyRunnerError::Execution(
                        "pyodide_bootstrap.js does not export loadPyRunnerPyodide".into(),
                    )
                })?;
            let js_options = v8::Object::new(scope);
            let index_key = v8::String::new(scope, "indexURL").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate Pyodide indexURL key".into())
            })?;
            let index_value = v8::String::new(scope, ".").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate Pyodide indexURL value".into())
            })?;
            let _ = js_options.set(scope, index_key.into(), index_value.into());
            if let Some(snapshot) = options.snapshot {
                let snapshot_key = v8::String::new(scope, "snapshot").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate Pyodide snapshot key".into())
                })?;
                let backing = v8::ArrayBuffer::new_backing_store_from_vec(snapshot.to_vec());
                let shared = backing.make_shared();
                let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
                let length = array_buffer.byte_length();
                let typed = Uint8Array::new(scope, array_buffer, 0, length).ok_or_else(|| {
                    PyRunnerError::Execution(
                        "failed to allocate Pyodide snapshot typed array".into(),
                    )
                })?;
                let _ = js_options.set(scope, snapshot_key.into(), typed.into());
            }
            if options.make_snapshot {
                let make_key = v8::String::new(scope, "makeSnapshot").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate Pyodide makeSnapshot key".into())
                })?;
                let make_value = v8::Boolean::new(scope, true);
                let _ = js_options.set(scope, make_key.into(), make_value.into());
            }
            let value = load_fn
                .call(scope, bootstrap.into(), &[js_options.into()])
                .ok_or_else(|| PyRunnerError::Execution("loadPyodide invocation failed".into()))?;
            let promise = v8::Local::<Promise>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("loadPyodide did not return a Promise".into())
            })?;
            promise_handle = Some(v8::Global::new(scope, promise));
            Ok(())
        })?;

        let promise_global = promise_handle
            .ok_or_else(|| PyRunnerError::Execution("missing loadPyodide promise handle".into()))?;

        loop {
            let done = self.with_context(|scope, _| -> Result<Option<()>> {
                let promise = v8::Local::new(scope, &promise_global);
                match promise.state() {
                    PromiseState::Pending => Ok(None),
                    PromiseState::Fulfilled => {
                        let result = promise.result(scope);
                        let obj = v8::Local::<Object>::try_from(result).map_err(|_| {
                            PyRunnerError::Execution(
                                "loadPyodide fulfilled with non-object result".into(),
                            )
                        })?;
                        let global = scope.get_current_context().global(scope);
                        let pyodide_key = v8::String::new(scope, "pyodide").ok_or_else(|| {
                            PyRunnerError::Execution("failed to allocate global Pyodide key".into())
                        })?;
                        global.set(scope, pyodide_key.into(), obj.into());
                        ctx_state
                            .pyodide_instance
                            .replace(Some(v8::Global::new(scope, obj)));
                        Ok(Some(()))
                    }
                    PromiseState::Rejected => {
                        promise.mark_as_handled();
                        let reason = promise.result(scope);
                        let message = reason
                            .to_string(scope)
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_else(|| "unknown rejection".to_string());
                        let detailed = reason
                            .to_object(scope)
                            .and_then(|obj| {
                                let stack_key = v8::String::new(scope, "stack")?;
                                obj.get(scope, stack_key.into())
                            })
                            .and_then(|value| value.to_string(scope))
                            .map(|s| s.to_rust_string_lossy(scope));
                        let message = detailed
                            .map(|stack| format!("{message}\n{stack}"))
                            .unwrap_or(message);
                        Err(PyRunnerError::Execution(format!(
                            "loadPyodide rejected: {message}"
                        )))
                    }
                }
            })?;
            if done.is_some() {
                break;
            }
            self.pump_event_loop()?;
        }

        self.seal_host_hooks()?;
        self.prepare_dynlibs()?;
        Ok(())
    }

    /// Invokes the JS helper to load one or more Pyodide packages via the package manager.
    pub fn load_packages(&mut self, packages: &[String]) -> Result<()> {
        if packages.is_empty() {
            return Ok(());
        }

        let mut promise_handle: Option<v8::Global<Promise>> = None;
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerLoadPackages").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate package loader key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerLoadPackages is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerLoadPackages is not a function".into())
            })?;

            let array = v8::Array::new(scope, packages.len() as i32);
            for (index, name) in packages.iter().enumerate() {
                let value = v8::String::new(scope, name).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate package name".into())
                })?;
                array.set_index(scope, index as u32, value.into());
            }

            let promise_value = func
                .call(scope, global.into(), &[array.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("package loader invocation failed".into())
                })?;
            let promise = v8::Local::<Promise>::try_from(promise_value).map_err(|_| {
                PyRunnerError::Execution("package loader did not return a Promise".into())
            })?;
            promise_handle = Some(v8::Global::new(scope, promise));
            Ok(())
        })?;

        let promise_global = promise_handle.ok_or_else(|| {
            PyRunnerError::Execution("missing package loader promise handle".into())
        })?;

        loop {
            let done = self.with_context(|scope, _| -> Result<Option<()>> {
                let promise = v8::Local::new(scope, &promise_global);
                match promise.state() {
                    PromiseState::Pending => Ok(None),
                    PromiseState::Fulfilled => Ok(Some(())),
                    PromiseState::Rejected => {
                        promise.mark_as_handled();
                        let reason = promise.result(scope);
                        let message = reason
                            .to_string(scope)
                            .map(|s| s.to_rust_string_lossy(scope))
                            .unwrap_or_else(|| "unknown rejection".to_string());
                        let detailed = reason
                            .to_object(scope)
                            .and_then(|obj| {
                                let stack_key = v8::String::new(scope, "stack")?;
                                obj.get(scope, stack_key.into())
                            })
                            .and_then(|value| value.to_string(scope))
                            .map(|s| s.to_rust_string_lossy(scope));
                        let message = detailed
                            .map(|stack| format!("{message}\n{stack}"))
                            .unwrap_or(message);
                        Err(PyRunnerError::Execution(format!(
                            "loadPackages rejected: {message}"
                        )))
                    }
                }
            })?;
            if done.is_some() {
                break;
            }
            self.pump_event_loop()?;
        }

        Ok(())
    }

    /// Captures a Pyodide memory snapshot and returns the raw bytes.
    pub fn collect_snapshot(&mut self) -> Result<Vec<u8>> {
        let mut snapshot: Option<Vec<u8>> = None;
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerMakeSnapshot").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate snapshot key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerMakeSnapshot is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerMakeSnapshot is not a function".into())
            })?;
            let result = func
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("snapshot invocation failed".into()))?;
            let array = Local::<Uint8Array>::try_from(result).map_err(|_| {
                PyRunnerError::Execution(
                    "__pyRunnerMakeSnapshot did not return a Uint8Array".into(),
                )
            })?;
            snapshot = Some(copy_typed_array(array));
            Ok(())
        })?;

        snapshot.ok_or_else(|| PyRunnerError::Execution("snapshot helper returned no data".into()))
    }

    /// Exports the overlay metadata (site-packages + /usr/lib) and tar payload.
    pub fn export_overlay(&mut self) -> Result<OverlayExport> {
        let mut metadata: Option<Vec<u8>> = None;
        let mut blobs: Vec<OverlayBlob> = Vec::new();
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key_meta = v8::String::new(scope, "__pyRunnerExportOverlay").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate overlay export key".into())
            })?;
            let value_meta = global.get(scope, key_meta.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerExportOverlay is not defined".into())
            })?;
            let func_meta = Local::<Function>::try_from(value_meta).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerExportOverlay is not a function".into())
            })?;
            let meta_result = func_meta
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("overlay export failed".into()))?;
            if meta_result.is_null_or_undefined() {
                metadata = Some(Vec::new());
            } else {
                let array = Local::<Uint8Array>::try_from(meta_result).map_err(|_| {
                    PyRunnerError::Execution(
                        "__pyRunnerExportOverlay did not return a Uint8Array".into(),
                    )
                })?;
                metadata = Some(copy_typed_array(array));
            }

            let key_blobs =
                v8::String::new(scope, "__pyRunnerExportOverlayBlobs").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate overlay blob export key".into())
                })?;
            let value_blobs = global.get(scope, key_blobs.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerExportOverlayBlobs is not defined".into())
            })?;
            let func_blobs = Local::<Function>::try_from(value_blobs).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerExportOverlayBlobs is not a function".into())
            })?;
            let blobs_result = func_blobs
                .call(scope, global.into(), &[])
                .ok_or_else(|| PyRunnerError::Execution("overlay blob export failed".into()))?;
            if !blobs_result.is_null_or_undefined() {
                let array = Local::<v8::Array>::try_from(blobs_result).map_err(|_| {
                    PyRunnerError::Execution(
                        "__pyRunnerExportOverlayBlobs did not return an Array".into(),
                    )
                })?;
                let key_prop = v8::String::new(scope, "key").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob key".into())
                })?;
                let digest_prop = v8::String::new(scope, "digest").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob digest".into())
                })?;
                let data_prop = v8::String::new(scope, "data").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate blob data".into())
                })?;
                let length = array.length();
                for index in 0..length {
                    let value = array
                        .get_index(scope, index)
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    if value.is_null_or_undefined() {
                        continue;
                    }
                    let object = Local::<Object>::try_from(value).map_err(|_| {
                        PyRunnerError::Execution("overlay blob entry is not an object".into())
                    })?;
                    let key_value = object
                        .get(scope, key_prop.into())
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    let key = if key_value.is_null_or_undefined() {
                        String::new()
                    } else {
                        key_value
                            .to_string(scope)
                            .ok_or_else(|| {
                                PyRunnerError::Execution(
                                    "failed to convert overlay blob key to string".into(),
                                )
                            })?
                            .to_rust_string_lossy(scope)
                    };
                    let digest_value = object
                        .get(scope, digest_prop.into())
                        .unwrap_or_else(|| v8::undefined(scope).into());
                    let digest = if digest_value.is_null_or_undefined() {
                        None
                    } else {
                        Some(
                            digest_value
                                .to_string(scope)
                                .ok_or_else(|| {
                                    PyRunnerError::Execution(
                                        "failed to convert overlay blob digest to string".into(),
                                    )
                                })?
                                .to_rust_string_lossy(scope),
                        )
                    };
                    let data_value = object.get(scope, data_prop.into()).ok_or_else(|| {
                        PyRunnerError::Execution(
                            "overlay blob entry missing 'data' property".into(),
                        )
                    })?;
                    let data_array = Local::<Uint8Array>::try_from(data_value).map_err(|_| {
                        PyRunnerError::Execution("overlay blob 'data' is not a Uint8Array".into())
                    })?;
                    let bytes = copy_typed_array(data_array);
                    blobs.push(OverlayBlob { key, digest, bytes });
                }
            }
            Ok(())
        })?;

        Ok(OverlayExport {
            metadata: metadata.ok_or_else(|| {
                PyRunnerError::Execution("overlay export returned no data".into())
            })?,
            blobs,
        })
    }

    /// Imports overlay metadata and refreshes the dynamic library bindings.
    pub fn import_overlay(&mut self, metadata: &[u8], blobs: &[OverlayBlob]) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerImportOverlay").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate overlay import key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerImportOverlay is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerImportOverlay is not a function".into())
            })?;
            let meta_backing = v8::ArrayBuffer::new_backing_store_from_vec(metadata.to_vec());
            let meta_shared = meta_backing.make_shared();
            let meta_buffer = v8::ArrayBuffer::with_backing_store(scope, &meta_shared);
            let meta_typed =
                Uint8Array::new(scope, meta_buffer, 0, metadata.len()).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate overlay metadata buffer".into())
                })?;

            let blob_array = v8::Array::new(scope, blobs.len() as i32);
            let key_prop = v8::String::new(scope, "key")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob key".into()))?;
            let digest_prop = v8::String::new(scope, "digest")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob digest".into()))?;
            let data_prop = v8::String::new(scope, "data")
                .ok_or_else(|| PyRunnerError::Execution("failed to allocate blob data".into()))?;
            for (index, blob) in blobs.iter().enumerate() {
                let object = v8::Object::new(scope);
                let key_string = v8::String::new(scope, blob.key.as_str())
                    .ok_or_else(|| PyRunnerError::Execution("failed to convert blob key".into()))?;
                object.set(scope, key_prop.into(), key_string.into());
                if let Some(digest) = &blob.digest {
                    let digest_string =
                        v8::String::new(scope, digest.as_str()).ok_or_else(|| {
                            PyRunnerError::Execution("failed to convert blob digest".into())
                        })?;
                    object.set(scope, digest_prop.into(), digest_string.into());
                } else {
                    let null_value = v8::null(scope);
                    object.set(scope, digest_prop.into(), null_value.into());
                }
                let data_backing = v8::ArrayBuffer::new_backing_store_from_vec(blob.bytes.clone());
                let data_shared = data_backing.make_shared();
                let data_buffer = v8::ArrayBuffer::with_backing_store(scope, &data_shared);
                let data_array = Uint8Array::new(scope, data_buffer, 0, blob.bytes.len())
                    .ok_or_else(|| {
                        PyRunnerError::Execution(
                            "failed to allocate overlay blob data buffer".into(),
                        )
                    })?;
                object.set(scope, data_prop.into(), data_array.into());
                blob_array.set_index(scope, index as u32, object.into());
            }

            let _ = func.call(
                scope,
                global.into(),
                &[meta_typed.into(), blob_array.into()],
            );
            Ok(())
        })?;
        self.prepare_dynlibs()
    }

    /// Refreshes dynamic library bindings after package or snapshot operations.
    pub fn prepare_dynlibs(&mut self) -> Result<()> {
        self.with_context(|scope, _| {
            let global = scope.get_current_context().global(scope);
            let key = v8::String::new(scope, "__pyRunnerPrepareDynlibs").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate dynlib preparation key".into())
            })?;
            let value = global.get(scope, key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerPrepareDynlibs is not defined".into())
            })?;
            let func = Local::<Function>::try_from(value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerPrepareDynlibs is not a function".into())
            })?;
            let _ = func.call(scope, global.into(), &[]);
            Ok(())
        })
    }

    /// Mounts bundle files into the Pyodide virtual filesystem at the given root directory.
    pub fn mount_bundle(&mut self, bundle: &Bundle, root: &str) -> Result<()> {
        self.clear_rawctx_auto_wrapper_cache();
        let ctx_state = self.context_state.clone();
        let root_owned = root.to_owned();
        self.with_context(|scope, _| {
            let pyodide = ctx_state
                .pyodide_local(scope)
                .ok_or_else(|| PyRunnerError::Execution("Pyodide is not loaded".into()))?;

            let files = v8::Array::new(scope, bundle.entries().len() as i32);
            for (index, entry) in bundle.entries().iter().enumerate() {
                let obj = v8::Object::new(scope);
                let rel_path = v8::String::new(scope, entry.path()).ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate file path string".into())
                })?;
                let path_key = v8::String::new(scope, "path").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate bundle path key".into())
                })?;
                let data_key = v8::String::new(scope, "data").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate bundle data key".into())
                })?;
                let size_key = v8::String::new(scope, "size").ok_or_else(|| {
                    PyRunnerError::Execution("failed to allocate bundle size key".into())
                })?;

                let buffer = entry.contents().to_vec();
                let backing = v8::ArrayBuffer::new_backing_store_from_bytes(buffer);
                let shared = backing.make_shared();
                let array_buffer = v8::ArrayBuffer::with_backing_store(scope, &shared);
                let uint8 = Uint8Array::new(scope, array_buffer, 0, entry.contents().len())
                    .ok_or_else(|| {
                        PyRunnerError::Execution("failed to allocate typed array".into())
                    })?;
                let size_value = v8::Number::new(scope, entry.contents().len() as f64);

                let _ = obj.set(scope, path_key.into(), rel_path.into());
                let _ = obj.set(scope, data_key.into(), uint8.into());
                let _ = obj.set(scope, size_key.into(), size_value.into());
                files.set_index(scope, index as u32, obj.into());
            }

            let global = scope.get_current_context().global(scope);
            let mount_fn_key = v8::String::new(scope, "__pyRunnerMountFiles").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate mount hook key".into())
            })?;
            let mount_fn_value = global.get(scope, mount_fn_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("__pyRunnerMountFiles is not defined".into())
            })?;
            let mount_fn = Local::<Function>::try_from(mount_fn_value).map_err(|_| {
                PyRunnerError::Execution("__pyRunnerMountFiles is not a function".into())
            })?;
            let root_value = v8::String::new(scope, &root_owned).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate mount root string".into())
            })?;
            mount_fn
                .call(
                    scope,
                    global.into(),
                    &[pyodide.into(), files.into(), root_value.into()],
                )
                .ok_or_else(|| PyRunnerError::Execution("mount files call failed".into()))?;

            exec_script(
                scope,
                "aardvark_json_result_buffer_helper.js",
                JSON_RESULT_BUFFER_HELPER,
            )
            .map_err(|err| {
                PyRunnerError::Execution(format!("installing JSON result helper failed: {err}"))
            })?;

            let run_key = v8::String::new(scope, "runPython").ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate runPython key".into())
            })?;
            let run_value = pyodide.get(scope, run_key.into()).ok_or_else(|| {
                PyRunnerError::Execution("pyodide.runPython is not available".into())
            })?;
            let run_fn = Local::<Function>::try_from(run_value).map_err(|_| {
                PyRunnerError::Execution("pyodide.runPython is not a function".into())
            })?;
            let script = v8::String::new(scope, PYTHON_ENTRYPOINT_HELPER).ok_or_else(|| {
                PyRunnerError::Execution("failed to allocate sys.path script".into())
            })?;
            run_fn
                .call(scope, pyodide.into(), &[script.into()])
                .ok_or_else(|| {
                    PyRunnerError::Execution("installing Python entrypoint helper failed".into())
                })?;

            Ok(())
        })?;
        Ok(())
    }
}
