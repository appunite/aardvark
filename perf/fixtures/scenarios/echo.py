import builtins
import copy
from typing import Optional

DEFAULT_BYTES = b"aardvark"


def _rawctx_inputs() -> Optional[dict]:
    source = getattr(builtins, "__aardvark_rawctx_inputs", None)
    return source if isinstance(source, dict) else None


def _rawctx_payload(field: str) -> Optional[bytes]:
    source = _rawctx_inputs()
    if source is None:
        return None
    record = source.get(field)
    if isinstance(record, dict):
        data = record.get("data")
        if isinstance(data, memoryview):
            return data.tobytes()
        if isinstance(data, (bytes, bytearray)):
            return bytes(data)
    return None


def _json_payload() -> Optional[object]:
    return getattr(builtins, "__aardvark_input", None)


def _publish_raw(data: bytes):
    factory = globals().get("__aardvark_output_buffer")
    if callable(factory):
        buffer = factory(len(data), id="echo-output")
        buffer[: len(data)] = data
        return buffer
    return data


def main(input):  # noqa: D401 - benchmark echo handler
    """Return a deep copy of the provided input."""
    return copy.deepcopy(input)


def entrypoint():
    if _rawctx_inputs() is not None:
        raw = _rawctx_payload("payload")
        if raw is None:
            raw = DEFAULT_BYTES
        result = main(raw)
        if isinstance(result, memoryview):
            return _publish_raw(result.tobytes())
        if isinstance(result, (bytes, bytearray)):
            return _publish_raw(bytes(result))
        return _publish_raw(bytes(str(result), "utf-8"))

    json_value = _json_payload()
    if json_value is None:
        return main(DEFAULT_BYTES.decode("utf-8"))
    return main(copy.deepcopy(json_value))
