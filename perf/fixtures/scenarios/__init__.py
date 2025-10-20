import importlib

def load_handler(name: str, profile: str):
    normalized = profile.lower()
    module_name = f".{name.lower()}_{normalized}"
    try:
        module = importlib.import_module(module_name, package=__name__)
    except ModuleNotFoundError as exc:
        raise RuntimeError(f"unknown scenario/profile combination: {name}/{profile}") from exc
    return module.main
