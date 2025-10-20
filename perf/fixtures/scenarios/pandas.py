import builtins
import json
from typing import Optional

import numpy as np
import pandas as pd

DEFAULT_ROWS = 128


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


def _json_rows() -> Optional[int]:
    payload = getattr(builtins, "__aardvark_input", None)
    if isinstance(payload, dict):
        value = payload.get("rows")
        if isinstance(value, int):
            return max(value, 1)
    return None


def _decode_rows(data: Optional[bytes]) -> Optional[int]:
    if not data:
        return None
    width = len(data)
    if width >= 8:
        return max(int.from_bytes(data[:8], "little"), 1)
    return None


def _summary(rows: int):
    rows = max(int(rows), 1)
    categories = (np.arange(rows) % 128).astype(np.int32)
    values = np.cos(np.arange(rows, dtype=np.float64) * 0.01)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    return {int(idx): float(val) for idx, val in summary["value_mean"].items()}


def _publish_raw(summary):
    payload = json.dumps(summary, separators=(",", ":")).encode("utf-8")
    factory = globals().get("__aardvark_output_buffer")
    if callable(factory):
        buffer = factory(len(payload), id="pandas-output")
        buffer[: len(payload)] = payload
        return buffer
    return summary


def main(input_rows):
    return _summary(input_rows)


def entrypoint():
    if _rawctx_inputs() is not None:
        rows = _decode_rows(_rawctx_payload("control"))
        if rows is None:
            rows = DEFAULT_ROWS
        summary = main(rows)
        return _publish_raw(summary)

    json_rows = _json_rows()
    if json_rows is not None:
        return main(json_rows)

    return main(DEFAULT_ROWS)
