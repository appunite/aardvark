import builtins
import struct
from typing import Optional

import numpy as np

DEFAULT_SIZE = 64


def _coerce_bytes(value: object) -> Optional[bytes]:
    if value is None:
        return None
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, memoryview):
        return value.tobytes()
    if isinstance(value, dict):
        data = value.get("data") if "data" in value else None
        return _coerce_bytes(data)
    return None


def _json_size(default: int) -> int:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("size")
        if isinstance(value, int):
            return max(value, 1)
    return default


def _rawctx_bytes(field: str) -> Optional[bytes]:
    source = getattr(builtins, "__aardvark_rawctx_inputs", None)
    if isinstance(source, dict):
        record = source.get(field)
        if isinstance(record, dict):
            return _coerce_bytes(record.get("data"))
    return None


def _decode_size(blob: Optional[bytes], default: int) -> int:
    if blob:
        width = len(blob)
        if width >= 8:
            return max(int.from_bytes(blob[:8], "little"), 1)
    return default


def _output_buffer(size: int):
    factory = globals().get("__aardvark_output_buffer")
    if callable(factory):
        return factory(size, id="numpy-output")
    return None


def main(payload=None, control=None, **kwargs):
    size = DEFAULT_SIZE
    if isinstance(payload, dict):
        value = payload.get("size")
        if isinstance(value, int):
            size = max(value, 1)

    raw_mode = False
    raw_bytes = None
    if control is not None:
        raw_mode = True
        raw_bytes = _coerce_bytes(control)
    if raw_bytes is None:
        candidate = _rawctx_bytes("control")
        if candidate is not None:
            raw_mode = True
            raw_bytes = candidate
    explicit = _coerce_bytes(kwargs.get("data"))
    if raw_bytes is None and explicit is not None:
        raw_mode = True
        raw_bytes = explicit

    size = _decode_size(raw_bytes, size)
    if size == DEFAULT_SIZE:
        size = _json_size(size)

    data = np.linspace(0.0, 1.0, size, dtype=np.float64)
    total = float(np.sin(data).sum())

    if raw_mode:
        buffer = _output_buffer(8)
        if buffer is not None:
            struct.pack_into("<d", buffer, 0, total)
            return buffer
    return total
