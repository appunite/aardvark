import numpy as np
import struct


def main():
    a = np.arange(200 * 200, dtype=np.float64).reshape(200, 200)
    b = np.sin(a * 0.001)
    c = a @ b.T
    value = float(c[0, 0])
    buf = __aardvark_output_buffer(8, id="numpy-output")
    struct.pack_into("<d", buf, 0, value)
    return buf
