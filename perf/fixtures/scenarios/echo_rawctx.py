def main():
    payload = b"x" * 1000
    buf = __aardvark_output_buffer(len(payload), id="echo-output")
    buf[:] = payload
    return buf
