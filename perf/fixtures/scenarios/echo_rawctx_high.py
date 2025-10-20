DEFAULT = b"x" * 1_000_000


def _extract(payload):
    if payload and isinstance(payload, dict):
        data = payload.get("data")
        if data is not None:
            return bytes(data)
    return DEFAULT


def main(payload=None):
    data = _extract(payload)
    buf = __aardvark_output_buffer(len(data), id="echo-output")
    buf[:] = data
    return buf
