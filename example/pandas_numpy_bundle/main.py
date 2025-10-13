import numpy as np
import pandas as pd


def main():
    """
    Exercise NumPy + pandas inside Pyodide.

    Returns group statistics so the CLI surfaces a structured payload.
    """
    rng = np.random.default_rng(seed=42)
    categories = np.array(["alpha", "beta", "gamma"], dtype=np.str_)
    rows = []
    for idx in range(12):
        rows.append(
            {
                "category": categories[idx % len(categories)],
                "value": float(rng.normal(loc=10.0 + idx, scale=1.5)),
            }
        )
    frame = pd.DataFrame(rows)
    grouped = frame.groupby("category")["value"].agg(["count", "mean", "std"])
    summary = grouped.round(3).reset_index().to_dict(orient="records")
    print("numpy version:", np.__version__)
    print("pandas version:", pd.__version__)
    return {"summary": summary}
