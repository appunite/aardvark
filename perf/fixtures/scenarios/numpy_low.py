import numpy as np

DEFAULT_SIZE = 64


def main(payload=None):
    size = DEFAULT_SIZE
    if payload and isinstance(payload, dict):
        size = int(payload.get("size", size))
    data = np.linspace(0.0, 1.0, max(size, 1), dtype=np.float64)
    return float(np.sin(data).sum())
