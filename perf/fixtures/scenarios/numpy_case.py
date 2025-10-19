import numpy as np


def main():
    # Deterministic workload to keep warm snapshots stable.
    a = np.arange(200 * 200, dtype=np.float64).reshape(200, 200)
    b = np.sin(a * 0.001)
    c = a @ b.T
    # Return a representative scalar to keep payload small.
    return float(c[0, 0])
