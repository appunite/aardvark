import builtins
import struct
from typing import Optional

import numpy as np

DEFAULT_SIZE = 64
_CACHE = {}


def _rawctx_inputs() -> Optional[dict]:
    source = getattr(builtins, "__aardvark_rawctx_inputs", None)
    return source if isinstance(source, dict) else None


def _rawctx_payload(field: str) -> Optional[bytes]:
    source = _rawctx_inputs()
    if source is None:
        return None
    record = source.get(field)
    if isinstance(record, dict):
        data = record.get("data")
        if isinstance(data, memoryview):
            return data.tobytes()
        if isinstance(data, (bytes, bytearray)):
            return bytes(data)
    return None


def _decode_size(data: Optional[bytes]) -> Optional[int]:
    if not data:
        return None
    if len(data) >= 8:
        return max(int.from_bytes(data[:8], "little"), 1)
    return None


def _json_size() -> Optional[int]:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("size")
        if isinstance(value, int):
            return max(value, 1)
    return None


def _matrices(size: int):
    cached = _CACHE.get(size)
    if cached is not None:
        return cached
    left = np.linspace(0.0, 1.0, size * size, dtype=np.float32).reshape(size, size)
    right = np.linspace(1.0, 2.0, size * size, dtype=np.float32).reshape(size, size)
    _CACHE[size] = (left, right)
    return left, right


def _publish_raw(value: float):
    publisher = getattr(builtins, "__aardvark_publish_buffer", None)
    if callable(publisher):
        publisher("numpy-matmul-output", struct.pack("<d", float(value)), {"format": "f64_le"})
        return None
    return float(value)


def main(input_size: int):
    size = max(int(input_size), 1)
    left, right = _matrices(size)
    return float(np.matmul(left, right)[0, 0])


def entrypoint():
    if _rawctx_inputs() is not None:
        size = _decode_size(_rawctx_payload("control")) or DEFAULT_SIZE
        return _publish_raw(main(size))

    return main(_json_size() or DEFAULT_SIZE)
