import builtins, importlib, io, json, sys, traceback
from pathlib import Path
from js import globalThis as __aardvark_js
try:
    from pyodide.ffi import to_memoryview as __aardvark_to_memoryview
except ImportError:
    __aardvark_to_memoryview = None

app = Path('/app')
if str(app) not in sys.path:
    sys.path.insert(0, str(app))

if '__aardvark_entrypoint_cache' not in globals():
    __aardvark_entrypoint_cache = {}

if '__aardvark_publish_buffer' not in globals():
    def __aardvark_publish_buffer(buffer_id, data, metadata=None, _js=__aardvark_js):
        return _js.__aardvarkPublishBuffer(buffer_id, data, metadata)

if not hasattr(builtins, "__aardvark_publish_buffer"):
    builtins.__aardvark_publish_buffer = __aardvark_publish_buffer

if '__aardvark_acquire_output_buffer' not in globals():
    def __aardvark_acquire_output_buffer(
        size,
        *,
        id=None,
        metadata=None,
        _js=__aardvark_js,
        _to_memoryview=__aardvark_to_memoryview,
    ):
        if size is None:
            raise ValueError("size is required")
        length = int(size)
        if length < 0:
            raise ValueError("size must be non-negative")
        view = _js.__aardvarkAcquireOutputBuffer(id, length, metadata)
        py_view = None
        if hasattr(view, "to_memoryview"):
            try:
                py_view = view.to_memoryview()
            except TypeError:
                py_view = None
        if py_view is None and _to_memoryview is not None:
            try:
                py_view = _to_memoryview(view)
            except TypeError:
                py_view = None
        if py_view is None and hasattr(view, "to_py"):
            py_view = view.to_py()
        if py_view is None:
            py_view = view
        if isinstance(py_view, memoryview):
            return py_view
        return memoryview(py_view)

def __aardvark_clear_rawctx_inputs(_builtins=builtins):
    if hasattr(_builtins, "__aardvark_rawctx_inputs"):
        delattr(_builtins, "__aardvark_rawctx_inputs")

def __aardvark_set_empty_rawctx_inputs(_builtins=builtins):
    _builtins.__aardvark_rawctx_inputs = {}

def __aardvark_view_to_memoryview(
    _view,
    _to_memoryview=__aardvark_to_memoryview,
):
    _memory = None
    if hasattr(_view, "to_memoryview"):
        try:
            _memory = _view.to_memoryview()
        except TypeError:
            _memory = None
    if _memory is None and _to_memoryview is not None:
        try:
            _memory = _to_memoryview(_view)
        except TypeError:
            _memory = None
    if _memory is None:
        if hasattr(_view, "to_py"):
            _candidate = _view.to_py()
        else:
            _candidate = _view
        try:
            _memory = memoryview(_candidate)
        except TypeError:
            _memory = memoryview(bytearray(_candidate))
    return _memory

def __aardvark_materialize_rawctx_inputs(
    _builtins=builtins,
    _js=__aardvark_js,
    _to_memoryview=__aardvark_to_memoryview,
    _view_to_memoryview=__aardvark_view_to_memoryview,
):
    __aardvark_rawctx_inputs = {}
    if hasattr(_js, "__aardvarkInputBuffers"):
        _names = None
        if hasattr(_js, "__aardvarkInputBufferNames"):
            try:
                _names = list(_js.__aardvarkInputBufferNames.to_py())
            except Exception:
                _names = None
        if _names is None:
            _buffers = _js.__aardvarkInputBuffers.to_py()
            _meta_source = {}
            if hasattr(_js, "__aardvarkInputMetadata"):
                try:
                    _meta_source = _js.__aardvarkInputMetadata.to_py()
                except Exception:
                    _meta_source = {}
            for _name, _view in _buffers.items():
                _memory = _view_to_memoryview(_view, _to_memoryview)
                _meta = None
                if isinstance(_meta_source, dict):
                    _meta = _meta_source.get(_name)
                    if hasattr(_meta, "to_py"):
                        _meta = _meta.to_py()
                __aardvark_rawctx_inputs[_name] = {"data": _memory, "metadata": _meta}
        else:
            _buffers = _js.__aardvarkInputBuffers
            _meta_source = getattr(_js, "__aardvarkInputMetadata", None)
            _reflect_get = _js.Reflect.get
            for _name in _names:
                _view = _reflect_get(_buffers, _name)
                _memory = _view_to_memoryview(_view, _to_memoryview)
                _meta = None
                if _meta_source is not None:
                    try:
                        _meta = _reflect_get(_meta_source, _name)
                        if hasattr(_meta, "to_py"):
                            _meta = _meta.to_py()
                    except Exception:
                        _meta = None
                __aardvark_rawctx_inputs[_name] = {"data": _memory, "metadata": _meta}
    _builtins.__aardvark_rawctx_inputs = __aardvark_rawctx_inputs

def __aardvark_materialize_rawctx_flat_inputs(
    _builtins=builtins,
    _js=__aardvark_js,
    _to_memoryview=__aardvark_to_memoryview,
    _view_to_memoryview=__aardvark_view_to_memoryview,
):
    __aardvark_rawctx_inputs = {}
    if hasattr(_js, "__aardvarkInputBuffers"):
        _names = None
        if hasattr(_js, "__aardvarkInputBufferNames"):
            try:
                _names = list(_js.__aardvarkInputBufferNames.to_py())
            except Exception:
                _names = None
        if _names is None:
            _buffers = _js.__aardvarkInputBuffers.to_py()
            for _name, _view in _buffers.items():
                __aardvark_rawctx_inputs[_name] = _view_to_memoryview(
                    _view,
                    _to_memoryview,
                )
        else:
            _buffers = _js.__aardvarkInputBuffers
            _reflect_get = _js.Reflect.get
            for _name in _names:
                _view = _reflect_get(_buffers, _name)
                __aardvark_rawctx_inputs[_name] = _view_to_memoryview(
                    _view,
                    _to_memoryview,
                )
    _builtins.__aardvark_rawctx_inputs = __aardvark_rawctx_inputs

def __aardvark_set_json_input_from_f32_buffer(
    _builtins=builtins,
    _js=__aardvark_js,
    _to_memoryview=__aardvark_to_memoryview,
):
    _view = _js.__aardvarkJsonInputBuffer
    _memory = None
    if hasattr(_view, "to_memoryview"):
        try:
            _memory = _view.to_memoryview()
        except TypeError:
            _memory = None
    if _memory is None and _to_memoryview is not None:
        try:
            _memory = _to_memoryview(_view)
        except TypeError:
            _memory = None
    if _memory is None:
        if hasattr(_view, "to_py"):
            _candidate = _view.to_py()
        else:
            _candidate = _view
        try:
            _memory = memoryview(_candidate)
        except TypeError:
            _memory = memoryview(bytearray(_candidate))
    if getattr(_memory, "format", None) != "f":
        _memory = _memory.cast("f")
    _builtins.__aardvark_input = _memory

def __aardvark_set_json_input_from_utf8_buffer(
    _builtins=builtins,
    _js=__aardvark_js,
    _to_memoryview=__aardvark_to_memoryview,
):
    _view = _js.__aardvarkJsonInputBuffer
    _memory = None
    if hasattr(_view, "to_memoryview"):
        try:
            _memory = _view.to_memoryview()
        except TypeError:
            _memory = None
    if _memory is None and _to_memoryview is not None:
        try:
            _memory = _to_memoryview(_view)
        except TypeError:
            _memory = None
    if _memory is None:
        if hasattr(_view, "to_py"):
            _candidate = _view.to_py()
        else:
            _candidate = _view
        try:
            _memory = memoryview(_candidate)
        except TypeError:
            _memory = memoryview(bytearray(_candidate))
    if getattr(_memory, "format", None) not in ("B", "b", "c"):
        _memory = _memory.cast("B")
    _builtins.__aardvark_input = _memory.tobytes().decode("utf-8")

def __aardvark_set_json_input_from_bytes_buffer(
    _builtins=builtins,
    _js=__aardvark_js,
    _to_memoryview=__aardvark_to_memoryview,
):
    _view = _js.__aardvarkJsonInputBuffer
    _memory = None
    if hasattr(_view, "to_memoryview"):
        try:
            _memory = _view.to_memoryview()
        except TypeError:
            _memory = None
    if _memory is None and _to_memoryview is not None:
        try:
            _memory = _to_memoryview(_view)
        except TypeError:
            _memory = None
    if _memory is None:
        if hasattr(_view, "to_py"):
            _candidate = _view.to_py()
        else:
            _candidate = _view
        try:
            _memory = memoryview(_candidate)
        except TypeError:
            _memory = memoryview(bytearray(_candidate))
    if getattr(_memory, "format", None) not in ("B", "b", "c"):
        _memory = _memory.cast("B")
    _builtins.__aardvark_input = _memory.tobytes()

def __aardvark_prepare_pending_inputs(
    _builtins=builtins,
    _json=json,
    _js=__aardvark_js,
):
    _mode = getattr(_js, "__aardvarkJsonInputMode", None)
    if hasattr(_mode, "to_py"):
        _mode = _mode.to_py()
    if _mode == "json":
        _encoded = _js.__aardvarkJsonInputEncoded
        if hasattr(_encoded, "to_py"):
            _encoded = _encoded.to_py()
        else:
            _encoded = str(_encoded)
        _builtins.__aardvark_input = _json.loads(_encoded)
    elif _mode == "f32":
        __aardvark_set_json_input_from_f32_buffer()
    elif _mode == "utf8":
        __aardvark_set_json_input_from_utf8_buffer()
    elif _mode == "bytes":
        __aardvark_set_json_input_from_bytes_buffer()
    elif _mode == "single_i64_object":
        _key = _js.__aardvarkJsonInputKey
        if hasattr(_key, "to_py"):
            _key = _key.to_py()
        else:
            _key = str(_key)
        _value = _js.__aardvarkJsonInputI64
        if hasattr(_value, "to_py"):
            _value = _value.to_py()
        _builtins.__aardvark_input = {_key: int(_value)}
    elif _mode == "none":
        if hasattr(_builtins, "__aardvark_input"):
            delattr(_builtins, "__aardvark_input")

    _rawctx_available = getattr(_js, "__aardvarkRawctxInputsAvailable", False)
    if hasattr(_rawctx_available, "to_py"):
        _rawctx_available = _rawctx_available.to_py()
    if _rawctx_available == "empty":
        __aardvark_set_empty_rawctx_inputs()
    elif _rawctx_available:
        _rawctx_input_mode = getattr(_js, "__aardvarkRawctxInputViewMode", None)
        if hasattr(_rawctx_input_mode, "to_py"):
            _rawctx_input_mode = _rawctx_input_mode.to_py()
        if _rawctx_input_mode == "flat":
            __aardvark_materialize_rawctx_flat_inputs()
        else:
            __aardvark_materialize_rawctx_inputs()

def __aardvark_try_json_buffer_result(
    value,
    _js=__aardvark_js,
):
    if not hasattr(_js, "__aardvarkSetJsonResultBuffer"):
        return None
    if isinstance(value, (bytes, bytearray, memoryview)):
        try:
            _view = value if isinstance(value, memoryview) else memoryview(value)
            if getattr(_view, "format", None) not in ("B", "b", "c"):
                _view = _view.cast("B")
            _size = int(getattr(_view, "nbytes", len(_view)))
        except Exception:
            return None
        if _size < 4096:
            return None
        try:
            _js.__aardvarkSetJsonResultBuffer(
                "bytes",
                _view,
                {"dtype": "bytes", "length": _size},
            )
            return "bytes"
        except Exception:
            return None
    _dtype = getattr(value, "dtype", None)
    _size = getattr(value, "size", None)
    if _dtype is None or _size is None:
        return None
    try:
        _size = int(_size)
    except Exception:
        return None
    if _size < 4096:
        return None
    _dtype_name = str(_dtype)
    if _dtype_name == "float32":
        _kind = "f32-array"
    elif _dtype_name == "float64":
        _kind = "f64-array"
    else:
        return None
    try:
        _view = value
        if hasattr(_view, "ravel"):
            _view = _view.ravel()
        if hasattr(_view, "flags") and not bool(getattr(_view.flags, "c_contiguous", False)):
            return None
        _js.__aardvarkSetJsonResultBuffer(
            _kind,
            memoryview(_view),
            {"dtype": _dtype_name, "length": _size},
        )
        return _kind
    except Exception:
        return None

def __aardvark_resolve_entrypoint(
    entrypoint,
    _cache=__aardvark_entrypoint_cache,
    _importlib=importlib,
):
    if entrypoint in _cache:
        return _cache[entrypoint]
    module_name, sep, func_name = entrypoint.partition(':')
    if not module_name:
        raise ValueError('entrypoint must specify a module')
    module = _importlib.import_module(module_name)
    if sep:
        target = getattr(module, func_name)
    elif hasattr(module, 'main'):
        target = module.main
    else:
        target = None
    _cache[entrypoint] = target
    return target

def __aardvark_call_target(
    target,
    include_text_result=True,
    capture_stdio=True,
    _builtins=builtins,
    _io=io,
    _json=json,
    _js=__aardvark_js,
    _sys=sys,
    _traceback=traceback,
):
    _stdout = _io.StringIO() if capture_stdio else None
    _stderr = _io.StringIO() if capture_stdio else None
    _old_out = _old_err = None
    value = None
    completed = False
    exc_type = None
    exc_value = None
    exc_traceback = None
    try:
        if capture_stdio:
            _old_out, _old_err = _sys.stdout, _sys.stderr
            _sys.stdout = _stdout
            _sys.stderr = _stderr
        __aardvark_prepare_pending_inputs()
        value = target() if target is not None else None
        completed = True
    except Exception as exc:
        exc_type = exc.__class__.__name__
        exc_value = repr(exc)
        exc_traceback = _traceback.format_exc()
    finally:
        if capture_stdio:
            _sys.stdout = _old_out
            _sys.stderr = _old_err
        try:
            if hasattr(_builtins, '__aardvark_input'):
                delattr(_builtins, '__aardvark_input')
            __aardvark_clear_rawctx_inputs()
            _js.__aardvarkJsonInputMode = None
            _js.__aardvarkJsonInputBuffer = None
            _js.__aardvarkJsonInputEncoded = None
            _js.__aardvarkRawctxInputsAvailable = False
            _js.__aardvarkRawctxInputViewMode = None
            _js.__aardvarkSharedBufferMetadataMode = "full"
        except Exception:
            pass
    if completed and exc_type is None and not include_text_result and not capture_stdio:
        if isinstance(value, str) and len(value) >= 4096:
            _js.__aardvarkJsonResultKind = "string"
            _js.__aardvarkJsonResultValue = value
            return "__aardvark_json_side_channel__:string"
        _side_channel = __aardvark_try_json_buffer_result(value)
        if _side_channel is not None:
            return "__aardvark_json_side_channel__:" + _side_channel
    payload = {
        'stdout': _stdout.getvalue() if capture_stdio else '',
        'stderr': _stderr.getvalue() if capture_stdio else '',
        'result': None,
        'json': None,
        'json_ready': False,
        'exception_type': exc_type,
        'exception_value': exc_value,
        'traceback': exc_traceback,
    }
    if completed and exc_type is None:
        if include_text_result and (value is None or isinstance(value, (str, int, float, bool))):
            payload['result'] = repr(value)
        if not include_text_result and isinstance(value, str) and len(value) >= 4096:
            _js.__aardvarkJsonResultKind = "string"
            _js.__aardvarkJsonResultValue = value
            payload['json_ready'] = True
            payload['json_side_channel'] = "string"
            return _json.dumps(payload)
        if not include_text_result:
            _side_channel = __aardvark_try_json_buffer_result(value)
            if _side_channel is not None:
                payload['json_ready'] = True
                payload['json_side_channel'] = _side_channel
                return _json.dumps(payload)
        payload['json'] = value
        payload['json_ready'] = True
        try:
            return _json.dumps(payload)
        except (TypeError, ValueError):
            payload['json'] = None
            payload['json_ready'] = False
            payload['result'] = repr(value)
    return _json.dumps(payload)

def __aardvark_call_entrypoint(
    entrypoint,
    include_text_result=True,
    capture_stdio=True,
    _resolve=__aardvark_resolve_entrypoint,
    _call_target=__aardvark_call_target,
):
    return _call_target(_resolve(entrypoint), include_text_result, capture_stdio)

def __aardvark_call_target_shared_buffer_only(
    target,
    _builtins=builtins,
    _json=json,
    _js=__aardvark_js,
    _traceback=traceback,
):
    exc_type = None
    exc_value = None
    exc_traceback = None
    try:
        __aardvark_prepare_pending_inputs()
        if target is not None:
            target()
        return "__aardvark_shared_buffers_ok__"
    except Exception as exc:
        exc_type = exc.__class__.__name__
        exc_value = repr(exc)
        exc_traceback = _traceback.format_exc()
    finally:
        try:
            if hasattr(_builtins, '__aardvark_input'):
                delattr(_builtins, '__aardvark_input')
            __aardvark_clear_rawctx_inputs()
            _js.__aardvarkJsonInputMode = None
            _js.__aardvarkJsonInputBuffer = None
            _js.__aardvarkJsonInputEncoded = None
            _js.__aardvarkRawctxInputsAvailable = False
            _js.__aardvarkRawctxInputViewMode = None
            _js.__aardvarkSharedBufferMetadataMode = "full"
        except Exception:
            pass
    return _json.dumps({
        'stdout': '',
        'stderr': '',
        'result': None,
        'json': None,
        'json_ready': False,
        'exception_type': exc_type,
        'exception_value': exc_value,
        'traceback': exc_traceback,
    })

def __aardvark_call_entrypoint_shared_buffer_only(
    entrypoint,
    _resolve=__aardvark_resolve_entrypoint,
    _call_target=__aardvark_call_target_shared_buffer_only,
):
    return _call_target(_resolve(entrypoint))
