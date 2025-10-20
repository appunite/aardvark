import builtins
import json
from typing import Optional

import numpy as np
import pandas as pd

DEFAULT_ROWS = 128


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


def _json_rows(default: int) -> int:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("rows")
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


def _decode_rows(blob: Optional[bytes], default: int) -> int:
    if blob:
        width = len(blob)
        if width >= 8:
            return max(int.from_bytes(blob[:8], "little"), 1)
    return default


def _output_buffer(size: int):
    factory = globals().get("__aardvark_output_buffer")
    if callable(factory):
        return factory(size, id="pandas-output")
    return None


def _build_summary(rows: int):
    rows = max(rows, 1)
    categories = (np.arange(rows) % 128).astype(np.int32)
    values = np.cos(np.arange(rows, dtype=np.float64) * 0.01)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    return {int(idx): float(val) for idx, val in summary["value_mean"].items()}


def main(payload=None, control=None, **kwargs):
    rows = DEFAULT_ROWS
    if isinstance(payload, dict):
        value = payload.get("rows")
        if isinstance(value, int):
            rows = max(value, 1)

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

    rows = _decode_rows(raw_bytes, rows)
    if rows == DEFAULT_ROWS:
        rows = _json_rows(rows)

    summary = _build_summary(rows)
    if raw_mode:
        payload_bytes = json.dumps(summary, separators=(",", ":")).encode("utf-8")
        buffer = _output_buffer(len(payload_bytes))
        if buffer is not None:
            buffer[: len(payload_bytes)] = payload_bytes
            return buffer
    return summary
