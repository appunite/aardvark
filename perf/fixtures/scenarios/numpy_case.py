import numpy as np


def main():
    rng = np.random.default_rng(42)
    a = rng.random((200, 200), dtype=np.float64)
    b = rng.random((200, 200), dtype=np.float64)
    c = a @ b
    # Return a representative scalar to keep payload small
    return float(c[0, 0])
