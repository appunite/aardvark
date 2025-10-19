import numpy as np
import pandas as pd


def main():
    rng = np.random.default_rng(123)
    rows = 50_000
    categories = rng.integers(0, 20, size=rows)
    values = rng.normal(loc=0.0, scale=1.0, size=rows)
    df = pd.DataFrame({"category": categories, "value": values})
    summary = df.groupby("category").agg(value_mean=("value", "mean"))
    # Convert to a plain mapping to keep payload JSON-friendly
    return {int(idx): float(val) for idx, val in summary["value_mean"].items()}
