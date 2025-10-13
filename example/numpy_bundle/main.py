import numpy as np

def main():
    data = np.arange(1, 6, dtype=np.int64)
    print(f"numpy version: {np.__version__}")
    return {"sum": int(data.sum()), "mean": float(data.mean())}
