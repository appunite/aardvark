import numpy as np


def _tensor_from_json(payload):
    if payload is None:
        return np.empty(0, dtype=np.float32)
    return np.asarray(payload, dtype=np.float32)


def _tensor_from_raw(payload):
    if payload is None:
        return None
    if isinstance(payload, memoryview):
        view = payload.cast("f") if payload.format != "f" else payload
        return np.frombuffer(view, dtype=np.float32)
    if isinstance(payload, (bytes, bytearray)):
        return np.frombuffer(memoryview(payload).cast("f"), dtype=np.float32)
    if isinstance(payload, dict):
        data = payload.get("data")
        if data is None:
            return None
        return _tensor_from_raw(memoryview(data))
    return None


def _compute(array: np.ndarray) -> np.ndarray:
    return np.tanh(array) * np.sqrt(array + 1.0, dtype=np.float32)


def _publish_raw(result: np.ndarray):
    buffer_factory = globals().get("__aardvark_output_buffer")
    if not callable(buffer_factory):
        return result.astype(np.float32, copy=False).tobytes()
    view = np.asarray(result, dtype=np.float32, order="C")
    buffer = buffer_factory(view.nbytes, id="tensor-output", metadata={"format": "f32_le"})
    memoryview(buffer).cast("f")[:] = view
    return buffer


def main(tensor: np.ndarray):
    return _compute(tensor)


def entrypoint(input=None, tensor_payload=None, tensor=None):
    raw_tensor = _tensor_from_raw(tensor_payload or tensor)
    if raw_tensor is None:
        raw_tensor = _tensor_from_json(input)
        result = main(raw_tensor)
        return result.tolist()

    result = main(raw_tensor)
    return _publish_raw(result)
