import importlib
from types import ModuleType
from typing import Callable

_SCENARIO_MODULES = {
    "echo": ".echo",
    "numpy": ".numpy",
    "pandas": ".pandas",
    "tensor": ".tensor",
    "matplotlib": ".matplotlib",
}


def load_handler(name: str) -> Callable[[], object]:
    module_path = _SCENARIO_MODULES.get(name.lower())
    if module_path is None:
        raise RuntimeError(f"unknown scenario '{name}'")
    module: ModuleType = importlib.import_module(module_path, package=__name__)
    if not hasattr(module, "entrypoint"):
        raise RuntimeError(f"scenario module '{module_path}' does not expose entrypoint()")
    return module.entrypoint


__all__ = ["load_handler"]
