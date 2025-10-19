import numpy as np
import pandas as pd


def main():
    rows = 50_000
    categories = (np.arange(rows) % 20).astype(np.int32)
    values = np.cos(np.arange(rows, dtype=np.float64) * 0.01)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    # Convert to a plain mapping to keep payload JSON-friendly.
    return {int(idx): float(val) for idx, val in summary["value_mean"].items()}
