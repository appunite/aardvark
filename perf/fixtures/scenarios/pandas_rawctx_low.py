import json
import numpy as np
import pandas as pd

DEFAULT_ROWS = 128


def _rows_from_payload(payload, default):
    if payload and isinstance(payload, dict):
        data = payload.get("data")
        if data is not None:
            return max(int.from_bytes(bytes(data), "little"), 1)
    return default


def main(payload=None):
    rows = _rows_from_payload(payload, DEFAULT_ROWS)
    categories = (np.arange(rows) % 20).astype(np.int32)
    values = np.cos(np.arange(rows, dtype=np.float64) * 0.01)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    payload = json.dumps({int(idx): float(val) for idx, val in summary["value_mean"].items()}, separators=(",", ":")).encode("utf-8")
    buf = __aardvark_output_buffer(len(payload), id="pandas-output")
    buf[:] = payload
    return buf
