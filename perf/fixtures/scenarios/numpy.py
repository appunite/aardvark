import builtins
import struct
from typing import Optional

import numpy as np

DEFAULT_SIZE = 64


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


def _json_size() -> Optional[int]:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("size")
        if isinstance(value, int):
            return max(value, 1)
    return None


def _decode_size(data: Optional[bytes]) -> Optional[int]:
    if not data:
        return None
    width = len(data)
    if width >= 8:
        return max(int.from_bytes(data[:8], "little"), 1)
    return None


def _publish_raw(total: float):
    factory = getattr(builtins, "__aardvark_output_buffer", None)
    if callable(factory):
        buffer = factory(8, id="numpy-output")
        struct.pack_into("<d", buffer, 0, float(total))
        return buffer
    return total


def main(input_size):
    size = max(int(input_size), 1)
    data = np.linspace(0.0, 1.0, size, dtype=np.float64)
    return float(np.sin(data).sum())


def entrypoint():
    if _rawctx_inputs() is not None:
        size = _decode_size(_rawctx_payload("control"))
        if size is None:
            size = DEFAULT_SIZE
        total = main(size)
        return _publish_raw(total)

    json_size = _json_size()
    if json_size is not None:
        return main(json_size)

    return main(DEFAULT_SIZE)
