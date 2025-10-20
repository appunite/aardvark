import numpy as np
import pandas as pd

DEFAULT_ROWS = 1_000_000


def main(payload=None):
    rows = DEFAULT_ROWS
    if payload and isinstance(payload, dict):
        rows = int(payload.get("rows", rows))
    categories = (np.arange(rows) % 256).astype(np.int32)
    values = np.cos(np.arange(rows, dtype=np.float64) * 0.01)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    return {int(idx): float(val) for idx, val in summary["value_mean"].items()}
