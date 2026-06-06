#!/usr/bin/env python3
"""Generate a local static inventory for an upstream Pyodide test checkout."""

from __future__ import annotations

import argparse
import ast
import json
from pathlib import Path
from typing import Any


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", default="0.29.4")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    upstream = repo_root / "third_party" / "pyodide" / args.version
    tests_root = upstream / "src" / "tests"
    if not tests_root.exists():
        raise SystemExit(f"missing upstream tests directory: {tests_root}")

    records: list[dict[str, Any]] = []
    for path in sorted(tests_root.glob("test_*.py")):
        source = path.read_text(encoding="utf-8")
        tree = ast.parse(source, filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith(
                "test_"
            ):
                rel = path.relative_to(upstream).as_posix()
                decorators = [
                    ast.get_source_segment(source, decorator) or ast.dump(decorator)
                    for decorator in node.decorator_list
                ]
                fixtures = [arg.arg for arg in node.args.args]
                records.append(
                    {
                        "id": f"{rel}::{node.name}",
                        "file": rel,
                        "name": node.name,
                        "category": categorize(path.name, node.name, decorators, fixtures),
                        "fixtures": fixtures,
                        "nodeStatusHint": node_status_hint(decorators),
                        "decorators": decorators,
                        "lineno": node.lineno,
                    }
                )

    output = args.output or (
        repo_root / "compat-tests" / "pyodide-node" / "inventory" / f"{args.version}.json"
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(records, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {len(records)} records to {output}")
    return 0


def node_status_hint(decorators: list[str]) -> str:
    joined = "\n".join(decorators)
    if "xfail_browsers" in joined and "node" in joined:
        return "xfail_node"
    if "only_chrome" in joined:
        return "xfail_node"
    if "only_node" in joined:
        return "node_only"
    return "candidate"


def categorize(
    filename: str, test_name: str, decorators: list[str], fixtures: list[str]
) -> str:
    text = " ".join([filename, test_name, *decorators, *fixtures]).lower()
    if "webworker" in text or "worker" in text:
        return "worker"
    if any(token in text for token in ["canvas", "document", "window", "dom"]):
        return "browser_api"
    if any(token in text for token in ["http", "fetch", "xhr", "cors", "url"]):
        return "network"
    if any(token in text for token in ["package", "micropip", "load_package"]):
        return "package"
    if "filesystem" in text or "fs" in text:
        return "filesystem"
    if any(token in text for token in ["jsproxy", "ffi", "run_js"]):
        return "js_ffi"
    if "stdlib" in text or "core_python" in text:
        return "python_core"
    return "python"


if __name__ == "__main__":
    raise SystemExit(main())
