import builtins
import struct
from typing import Optional

import matplotlib

matplotlib.use("Agg")

from matplotlib.backends.backend_agg import FigureCanvasAgg
from matplotlib.figure import Figure
import numpy as np

DEFAULT_POINTS = 128


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


def _json_points() -> Optional[int]:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("points")
        if isinstance(value, int):
            return max(value, 1)
    return None


def _decode_points(data: Optional[bytes]) -> Optional[int]:
    if not data:
        return None
    if len(data) >= 8:
        return max(int.from_bytes(data[:8], "little"), 1)
    return None


def _publish_raw(byte_count: int):
    metadata = {"format": "u64_le"}
    factory = getattr(builtins, "__aardvark_output_buffer", None)
    if callable(factory):
        buffer = factory(8, id="matplotlib-output", metadata=metadata)
        struct.pack_into("<Q", buffer, 0, int(byte_count))
        return buffer
    publisher = getattr(builtins, "__aardvark_publish_buffer", None)
    if callable(publisher):
        payload = struct.pack("<Q", int(byte_count))
        publisher("matplotlib-output", payload, metadata)
        return None
    return byte_count


def main(input_points):
    points = max(int(input_points), 1)
    x = np.linspace(0.0, np.pi * 8.0, points, dtype=np.float64)
    y = np.sin(x) * np.cos(x * 0.25)

    figure = Figure(figsize=(4.0, 3.0), dpi=96)
    canvas = FigureCanvasAgg(figure)
    axis = figure.add_subplot(1, 1, 1)
    axis.plot(x, y, linewidth=1.2)
    axis.fill_between(x, y, 0.0, alpha=0.2)
    axis.set_xlim(float(x[0]), float(x[-1]))
    axis.set_ylim(-1.1, 1.1)
    axis.grid(True, linewidth=0.4, alpha=0.35)
    canvas.draw()

    return canvas.buffer_rgba().nbytes


def entrypoint():
    if _rawctx_inputs() is not None:
        points = _decode_points(_rawctx_payload("control"))
        if points is None:
            points = DEFAULT_POINTS
        return _publish_raw(main(points))

    json_points = _json_points()
    if json_points is not None:
        return main(json_points)

    return main(DEFAULT_POINTS)
