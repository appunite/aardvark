import numpy as np
import struct

DEFAULT_SIZE = 64


def _size_from_payload(payload, default):
    if payload and isinstance(payload, dict):
        data = payload.get("data")
        if data is not None:
            return max(int.from_bytes(bytes(data), "little"), 1)
    return default


def main(payload=None):
    size = _size_from_payload(payload, DEFAULT_SIZE)
    data = np.linspace(0.0, 1.0, size, dtype=np.float64)
    total = float(np.sin(data).sum())
    buf = __aardvark_output_buffer(8, id="numpy-output")
    struct.pack_into("<d", buf, 0, total)
    return buf
