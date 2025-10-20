import builtins
from typing import Optional

DEFAULT_BYTES = b"aardvark"


def _coerce_bytes(value: object) -> Optional[bytes]:
    if value is None:
        return None
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, memoryview):
        return value.tobytes()
    if isinstance(value, str):
        return value.encode("utf-8")
    if isinstance(value, dict):
        data = value.get("data") if "data" in value else None
        return _coerce_bytes(data)
    return None


def _rawctx_bytes(field: str) -> Optional[bytes]:
    source = getattr(builtins, "__aardvark_rawctx_inputs", None)
    if isinstance(source, dict):
        record = source.get(field)
        if isinstance(record, dict):
            return _coerce_bytes(record.get("data"))
    return None


def _json_payload() -> Optional[bytes]:
    payload = getattr(builtins, "__aardvark_input", None)
    return _coerce_bytes(payload)


def _output_buffer(size: int):
    factory = globals().get("__aardvark_output_buffer")
    if callable(factory):
        return factory(size, id="echo-output")
    return None


def main(payload=None, control=None, **kwargs):
    raw_mode = False
    data = None
    if isinstance(payload, dict):
        raw_mode = True
        data = _coerce_bytes(payload)
    if data is None and control is not None:
        raw_mode = True
        data = _coerce_bytes(control)
    if data is None:
        raw_bytes = _rawctx_bytes("payload")
        if raw_bytes is not None:
            raw_mode = True
            data = raw_bytes
    if data is None:
        explicit = kwargs.get("data")
        data = _coerce_bytes(explicit)
    if data is None:
        data = _json_payload()
    if data is None:
        data = DEFAULT_BYTES

    if raw_mode:
        buffer = _output_buffer(len(data))
        if buffer is not None:
            buffer[: len(data)] = data
            return buffer
    return data.decode("utf-8", errors="ignore")
