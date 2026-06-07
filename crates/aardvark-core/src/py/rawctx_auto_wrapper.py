import builtins, importlib, json

__aardvark_rawctx_spec = json.loads(r'''{spec_json}''')

def __aardvark__acquire_output_buffer(size, *, id=None, metadata=None):
    if size is None:
        raise ValueError("size is required")
    length = int(size)
    if length < 0:
        raise ValueError("size must be non-negative")
    from js import globalThis as _js
    view = _js.__aardvarkAcquireOutputBuffer(id, length, metadata)
    py_view = None
    if hasattr(view, "to_memoryview"):
        try:
            py_view = view.to_memoryview()
        except TypeError:
            py_view = None
    if py_view is None:
        try:
            from pyodide.ffi import to_memoryview as _to_memoryview

            py_view = _to_memoryview(view)
        except (ImportError, TypeError):
            py_view = None
    if py_view is None and hasattr(view, "to_py"):
        py_view = view.to_py()
    if py_view is None:
        py_view = view
    if isinstance(py_view, memoryview):
        return py_view
    return memoryview(py_view)

builtins.__aardvark_output_buffer = __aardvark__acquire_output_buffer

def __aardvark__decode_rawctx(binding, payload):
    value, metadata, raw_payload = __aardvark__decode_scalar(binding, payload)
    table_spec = binding.get("table")
    if table_spec:
        table_value, table_metadata = __aardvark__materialize_table(table_spec, value)
        value = table_value
        if table_metadata is not None:
            if metadata is None:
                metadata = table_metadata
            elif isinstance(metadata, dict) and isinstance(table_metadata, dict):
                merged = dict(metadata)
                merged.update(table_metadata)
                metadata = merged
            else:
                metadata = table_metadata
    return value, metadata, raw_payload


def __aardvark__decode_scalar(binding, payload):
    if payload is None:
        return None, None, None
    data = payload.get("data")
    metadata = payload.get("metadata")
    raw_payload = payload
    if data is None:
        return None, metadata, raw_payload
    if binding.get("python_loader"):
        namespace = {
            "buffer": data,
            "metadata": metadata,
            "payload": raw_payload,
            "memoryview": memoryview,
        }
        return eval(binding["python_loader"], {}, namespace), metadata, raw_payload
    decoder = binding.get("decoder") or "memoryview"
    options = binding.get("options") or {}
    if decoder in ("memoryview", None):
        value = data
    elif decoder == "bytes":
        value = data.tobytes()
    elif decoder in ("utf8", "string"):
        encoding = options.get("encoding", "utf-8")
        errors = options.get("errors", "strict")
        value = data.tobytes().decode(encoding, errors)
    elif decoder in ("float32", "f32"):
        import struct as _struct
        fmt = options.get("struct_format", "<f")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("float64", "f64"):
        import struct as _struct
        fmt = options.get("struct_format", "<d")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("int32", "i32"):
        import struct as _struct
        fmt = options.get("struct_format", "<i")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("uint32", "u32"):
        import struct as _struct
        fmt = options.get("struct_format", "<I")
        value = _struct.unpack(fmt, data.tobytes())[0]
    elif decoder in ("int64", "i64"):
        byteorder = options.get("byteorder", "little")
        signed = bool(options.get("signed", True))
        value = int.from_bytes(data.tobytes(), byteorder=byteorder, signed=signed)
    elif decoder in ("bool", "boolean"):
        byteorder = options.get("byteorder", "little")
        value = bool(int.from_bytes(data.tobytes(), byteorder=byteorder, signed=False))
    elif decoder == "json":
        import json as _json
        encoding = options.get("encoding", "utf-8")
        errors = options.get("errors", "strict")
        value = _json.loads(data.tobytes().decode(encoding, errors))
    elif decoder in ("base64", "b64"):
        import base64 as _base64
        raw_bytes = data.tobytes()
        altchars = options.get("altchars")
        if altchars is not None and not isinstance(altchars, (bytes, bytearray)):
            altchars = str(altchars).encode()
        validate = bool(options.get("validate", False))
        decoded = _base64.b64decode(raw_bytes, altchars=altchars, validate=validate)
        if options.get("as_memoryview"):
            value = memoryview(decoded)
        elif options.get("as_bytearray"):
            value = bytearray(decoded)
        else:
            value = decoded
    elif decoder in ("bytearray", "bytesarray"):
        value = bytearray(data.tobytes())
    else:
        value = data
    return value, metadata, raw_payload


def __aardvark__materialize_table(spec, value):
    if value is None:
        return None, None
    columns = spec.get("columns") or []
    orient = (spec.get("orient") or "records").lower()
    if orient not in ("records", "columns"):
        raise ValueError(f"unsupported rawctx table orientation: {orient}")
    column_schema = {}
    for column in columns:
        name = column.get("name")
        if not name:
            continue
        column_meta = {}
        if "dtype" in column:
            column_meta["dtype"] = column["dtype"]
        if "nullable" in column:
            column_meta["nullable"] = column["nullable"]
        if "metadata" in column and isinstance(column.get("metadata"), dict):
            column_meta["metadata"] = column["metadata"]
        if "shape" in column:
            column_meta["shape"] = column["shape"]
        if "manifest" in column and isinstance(column.get("manifest"), dict):
            column_meta["manifest"] = column["manifest"]
        if column_meta:
            column_schema[name] = column_meta
    table_metadata = {"orient": orient}
    if column_schema:
        table_metadata["schema"] = {"columns": column_schema}
    if orient == "records":
        if not isinstance(value, (list, tuple)):
            raise TypeError("rawctx table expects a list of record dicts")
        result = {column.get("name"): [] for column in columns}
        for record in value:
            if not isinstance(record, dict):
                raise TypeError("rawctx table records must be dictionaries")
            for column in columns:
                name = column.get("name")
                if not name:
                    continue
                if name in record:
                    result[name].append(record[name])
                elif column.get("optional") or column.get("default") is not None:
                    result[name].append(column.get("default"))
                else:
                    raise KeyError(f"rawctx table column '{name}' is required")
        __aardvark__apply_column_decoders(result, columns)
        return result, table_metadata
    # columns orient
    if not isinstance(value, dict):
        raise TypeError("rawctx table expects a dict of columns")
    result = {}
    for column in columns:
        name = column.get("name")
        if not name:
            continue
        if name in value:
            result[name] = value[name]
        elif column.get("optional") or column.get("default") is not None:
            result[name] = column.get("default")
        else:
            raise KeyError(f"rawctx table column '{name}' is required")
    __aardvark__apply_column_decoders(result, columns)
    return result, table_metadata


def __aardvark__apply_column_decoders(result, columns):
    for column in columns:
        name = column.get("name")
        if not name or name not in result:
            continue
        decoder = column.get("decoder")
        if not decoder:
            continue
        series = result[name]
        options = column.get("options") or {}
        if isinstance(series, list):
            converted = []
            for item in series:
                payload = __aardvark__prepare_decoder_payload(item, options)
                if payload is None:
                    converted.append(item)
                    continue
                value, _, _ = __aardvark__decode_scalar({"decoder": decoder, "options": options}, payload)
                converted.append(value)
            result[name] = converted
        else:
            payload = __aardvark__prepare_decoder_payload(series, options)
            if payload is None:
                continue
            value, _, _ = __aardvark__decode_scalar({"decoder": decoder, "options": options}, payload)
            result[name] = value


def __aardvark__prepare_decoder_payload(item, options):
    if isinstance(item, memoryview):
        return {"data": item, "metadata": None}
    if isinstance(item, bytes):
        return {"data": memoryview(item), "metadata": None}
    if isinstance(item, bytearray):
        return {"data": memoryview(bytes(item)), "metadata": None}
    if isinstance(item, str):
        encoding = options.get("encoding", "utf-8") if isinstance(options, dict) else "utf-8"
        return {"data": memoryview(item.encode(encoding)), "metadata": None}
    return None

def __aardvark__apply_outputs(spec, result):
    if not spec:
        return result, False
    if isinstance(spec, dict):
        return __aardvark__apply_single_output(spec, result)
    if not isinstance(spec, (list, tuple)):
        raise TypeError("rawctx outputs must be a dict or list of dicts")
    final_result = result
    handled_any = False
    for item in spec:
        if item is None:
            continue
        candidate, handled = __aardvark__apply_single_output(item, result)
        if handled:
            handled_any = True
            final_result = candidate
    return final_result, handled_any


def __aardvark__apply_single_output(spec, result):
    if not spec:
        return result, False
    if not isinstance(spec, dict):
        raise TypeError("rawctx output spec must be a dict")
    mode = spec.get("mode") or "publish-buffer"
    if mode != "publish-buffer":
        return result, False
    when_none = spec.get("when_none", "skip")
    if result is None:
        if when_none == "error":
            raise ValueError("rawctx output requires a non-None result")
        if when_none == "publish-empty":
            data_value = memoryview(b"")
        elif when_none == "propagate":
            return None, False
        else:
            return None, False
    else:
        data_value = result
    metadata = spec.get("metadata")
    if spec.get("python_transform"):
        namespace = {
            "result": result,
            "metadata": metadata,
            "memoryview": memoryview,
        }
        transformed = eval(spec["python_transform"], {}, namespace)
        if isinstance(transformed, tuple) and len(transformed) == 2:
            data_value, metadata = transformed
        else:
            data_value = transformed
    transform = spec.get("transform", "memoryview")
    if transform == "memoryview":
        if not isinstance(data_value, memoryview):
            if isinstance(data_value, (bytes, bytearray)):
                data_value = memoryview(data_value)
            else:
                try:
                    data_value = memoryview(data_value)
                except TypeError:
                    data_value = memoryview(bytes(data_value))
    elif transform == "bytes":
        if not isinstance(data_value, memoryview):
            if isinstance(data_value, (bytes, bytearray)):
                data_value = memoryview(data_value)
            else:
                try:
                    data_value = memoryview(data_value)
                except TypeError:
                    data_value = memoryview(bytes(data_value))

        try:
            cast_view = data_value.cast("B")
        except (TypeError, ValueError):
            cast_view = None

        if cast_view is not None and cast_view.contiguous:
            data_value = cast_view
        else:
            source_view = cast_view if cast_view is not None else data_value
            data_value = memoryview(source_view.tobytes())
    elif transform == "utf8":
        if not isinstance(data_value, str):
            raise TypeError("rawctx output expected str for utf8 transform")
        encoding = spec.get("encoding", "utf-8")
        data_value = memoryview(data_value.encode(encoding))
    elif transform == "identity":
        pass
    else:
        raise ValueError(f"unsupported rawctx output transform: {transform}")
    publish_id = spec.get("id")
    if not publish_id:
        raise ValueError("rawctx output publish-buffer requires an id")
    from js import globalThis as _js
    _js.__aardvarkPublishBuffer(publish_id, data_value, metadata)
    behaviour = spec.get("return_behavior") or "none"
    if behaviour == "original":
        return result, True
    if behaviour == "buffer":
        return data_value, True
    return None, True

_module_name, _, _func_name = (__aardvark_rawctx_spec.get("entrypoint") or "").partition(":")
if _module_name and _func_name:
    _inputs = __aardvark_rawctx_spec.get("inputs") or []
    _output_specs = __aardvark_rawctx_spec.get("outputs") or []
    if _inputs or _output_specs:
        _module = importlib.import_module(_module_name)
        _originals = getattr(_module, "__aardvark_rawctx_original_entrypoints__", None)
        if not isinstance(_originals, dict):
            _originals = {}
            setattr(_module, "__aardvark_rawctx_original_entrypoints__", _originals)
        _target = _originals.get(_func_name)
        if _target is None:
            _target = getattr(_module, _func_name)
            if getattr(_target, "__aardvark_rawctx_wrapper__", False):
                _target = getattr(_target, "__aardvark_rawctx_original__", _target)
            _originals[_func_name] = _target

        def __aardvark_rawctx_wrapper(
            __aardvark_target=_target,
            __aardvark_inputs=_inputs,
            __aardvark_outputs=tuple(_output_specs),
        ):
            source = getattr(builtins, "__aardvark_rawctx_inputs", {})
            args = []
            kwargs = {}
            for binding in __aardvark_inputs:
                payload = source.get(binding["field"])
                if payload is None:
                    if "default" in binding:
                        value = binding["default"]
                        metadata = None
                        raw_payload = None
                    elif binding.get("optional"):
                        value = None
                        metadata = None
                        raw_payload = None
                    else:
                        raise KeyError(f"rawctx input '{binding['field']}' is required")
                else:
                    value, metadata, raw_payload = __aardvark__decode_rawctx(binding, payload)
                    if value is None and "default" in binding:
                        value = binding["default"]
                if binding.get("metadata_arg"):
                    kwargs[binding["metadata_arg"]] = metadata
                if binding.get("raw_arg"):
                    kwargs[binding["raw_arg"]] = payload
                arg_name = binding.get("arg")
                if arg_name is not None:
                    mode = binding.get("mode", "keyword")
                    if mode == "positional":
                        args.append(value)
                    else:
                        kwargs[arg_name] = value
            result = __aardvark_target(*args, **kwargs)
            result, _handled = __aardvark__apply_outputs(__aardvark_outputs, result)
            return result

        __aardvark_rawctx_wrapper.__aardvark_rawctx_wrapper__ = True
        __aardvark_rawctx_wrapper.__aardvark_rawctx_original__ = _target
        setattr(_module, _func_name, __aardvark_rawctx_wrapper)
        _entrypoint_cache = globals().get("__aardvark_entrypoint_cache")
        if isinstance(_entrypoint_cache, dict):
            _entrypoint_cache[__aardvark_rawctx_spec.get("entrypoint")] = __aardvark_rawctx_wrapper
        del __aardvark_rawctx_wrapper, _module, _target, _inputs, _output_specs
        del _originals
        del _entrypoint_cache

del __aardvark_rawctx_spec
