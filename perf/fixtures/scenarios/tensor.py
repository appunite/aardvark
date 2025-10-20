import numpy as np


def _as_numpy_from_json(payload):
    if payload is None:
        return np.array([], dtype=np.float32)
    return np.asarray(payload, dtype=np.float32)


def _as_numpy_from_raw(payload_record):
    if payload_record is None:
        return None
    if isinstance(payload_record, memoryview):
        view = payload_record.cast("f") if payload_record.format != "f" else payload_record
        return np.frombuffer(view, dtype=np.float32)
    if isinstance(payload_record, (bytes, bytearray)):
        view = memoryview(payload_record).cast("f")
        return np.frombuffer(view, dtype=np.float32)
    if isinstance(payload_record, dict):
        data = payload_record.get("data")
        if data is None:
            return None
        return _as_numpy_from_raw(memoryview(data))
    return None


def _compute(array):
    return np.tanh(array) * np.sqrt(array + 1.0)


def _publish_raw(result):
    buffer_factory = globals().get("__aardvark_output_buffer")
    if not callable(buffer_factory):
        return memoryview(result.astype(np.float32, copy=False).tobytes())
    # ensure contiguous float32 view
    view = np.asarray(result, dtype=np.float32)
    out = buffer_factory(view.nbytes, id="tensor-output", metadata={"format": "f32_le"})
    memoryview(out).cast("f")[:] = view
    return out


def main(input=None, tensor_payload=None, tensor=None):
    if tensor_payload is None and tensor is not None:
        tensor_payload = tensor
    raw_array = _as_numpy_from_raw(tensor_payload)
    if raw_array is None:
        array = _as_numpy_from_json(input)
        result = _compute(array)
        return result.tolist()

    result = _compute(raw_array)
    published = _publish_raw(result)
    if published is not None:
        return published
    return result.tolist()


def entrypoint(input=None, tensor_payload=None, tensor=None):
    return main(input=input, tensor_payload=tensor_payload, tensor=tensor)
