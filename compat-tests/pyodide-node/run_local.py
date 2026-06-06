#!/usr/bin/env python3
"""Run the local Aardvark compatibility corpus for a pinned Pyodide version."""

from __future__ import annotations

import argparse
import json
import tomllib
from pathlib import Path
from typing import Any

from aardvark_selenium import AardvarkGuestError, AardvarkSelenium


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", default="0.29.4")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--dist-dir", type=Path)
    parser.add_argument("--cases", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    parser.add_argument(
        "--reuse-runtime",
        action="store_true",
        help="do not reset the Aardvark runtime between adopted cases",
    )
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    cases_path = args.cases or (
        repo_root / "compat-tests" / "pyodide-node" / "cases" / f"{args.version}.json"
    )
    report_path = args.report or (
        repo_root / "target" / "compat" / "pyodide-node" / f"{args.version}.json"
    )
    dist_dir = resolve_dist_dir(repo_root, args.dist_dir, args.version)

    cases = json.loads(cases_path.read_text(encoding="utf-8"))
    results: list[dict[str, Any]] = []

    with AardvarkSelenium(
        repo_root,
        dist_dir=dist_dir,
        command_timeout_seconds=args.timeout_seconds,
    ) as selenium:
        for case in cases:
            result = run_case(
                selenium,
                case,
                dist_dir,
                isolate=not args.reuse_runtime,
            )
            results.append(result)
            print(f"{result['status']:5} {case['id']}")
            if result.get("reason"):
                print(f"      {result['reason']}")

    report = {
        "version": args.version,
        "distDir": str(dist_dir) if dist_dir else None,
        "cases": results,
        "summary": summarize(results),
    }
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote report to {report_path}")

    return 1 if any(item["status"] == "fail" for item in results) else 0


def run_case(
    selenium: AardvarkSelenium,
    case: dict[str, Any],
    dist_dir: Path | None,
    *,
    isolate: bool,
) -> dict[str, Any]:
    if case.get("requiresDist") and dist_dir is None:
        return {
            "id": case["id"],
            "status": "skip",
            "reason": "case requires a staged Aardvark Pyodide distribution",
        }

    try:
        if isolate and case.get("isolate", True):
            selenium.reset()
        packages = case.get("packages") or []
        if packages:
            selenium.load_package(packages)
        op = case.get("op", "runPython")
        code = case_code(case)
        if op == "runPython":
            value = selenium.run(code)
        elif op == "runPythonAsync":
            value = selenium.run_async(code)
        elif op == "runJs":
            value = selenium.run_js(code)
        else:
            raise ValueError(f"unsupported case op: {op}")
    except AardvarkGuestError as exc:
        expected_error = case.get("expectErrorContains")
        if expected_error and error_contains(exc, expected_error):
            return {
                "id": case["id"],
                "status": "pass",
                "reason": "expected guest error",
                "response": exc.response,
            }
        expected_failure = case.get("expectFailureContains")
        if expected_failure and expected_failure in str(exc):
            return {
                "id": case["id"],
                "status": "xfail",
                "reason": str(exc),
                "response": exc.response,
            }
        return {
            "id": case["id"],
            "status": "fail",
            "reason": str(exc),
            "response": exc.response,
        }
    except Exception as exc:  # noqa: BLE001 - local runner should report all failures.
        return {
            "id": case["id"],
            "status": "fail",
            "reason": f"{type(exc).__name__}: {exc}",
        }

    expected_error = case.get("expectErrorContains")
    if expected_error:
        return {
            "id": case["id"],
            "status": "fail",
            "reason": f"expected guest error containing {expected_error!r}, but case passed",
            "value": value,
            "response": selenium.last_response,
        }

    expected_failure = case.get("expectFailureContains")
    if expected_failure:
        return {
            "id": case["id"],
            "status": "fail",
            "reason": f"expected failure containing {expected_failure!r}, but case passed",
            "value": value,
            "response": selenium.last_response,
        }

    expected = case.get("expect")
    if value != expected:
        return {
            "id": case["id"],
            "status": "fail",
            "reason": f"expected {expected!r}, got {value!r}",
            "value": value,
            "response": selenium.last_response,
        }

    stdout_contains = case.get("expectStdoutContains")
    if stdout_contains:
        diagnostics = (selenium.last_response or {}).get("diagnostics") or {}
        stdout = diagnostics.get("stdout", "")
        if stdout_contains not in stdout:
            return {
                "id": case["id"],
                "status": "fail",
                "reason": f"stdout did not contain {stdout_contains!r}",
                "stdout": stdout,
            }

    return {
        "id": case["id"],
        "status": "pass",
        "value": value,
    }


def case_code(case: dict[str, Any]) -> str:
    if "code" in case:
        return str(case["code"])
    if "codeLines" in case:
        return "\n".join(str(line) for line in case["codeLines"])
    raise KeyError(f"case {case.get('id', '<unknown>')} has no code")


def error_contains(exc: AardvarkGuestError, expected: str | list[str]) -> bool:
    needles = [expected] if isinstance(expected, str) else expected
    response_text = json.dumps(exc.response, sort_keys=True)
    haystack = f"{exc}\n{response_text}"
    return all(needle in haystack for needle in needles)


def resolve_dist_dir(repo_root: Path, dist_dir: Path | None, pyodide_version: str) -> Path | None:
    if dist_dir is None:
        default = default_dist_dir(repo_root, pyodide_version)
        dist_dir = default if default.exists() else None
    elif not dist_dir.is_absolute():
        dist_dir = repo_root / dist_dir
    if dist_dir is not None and not dist_dir.exists():
        raise SystemExit(f"missing Pyodide distribution: {dist_dir}")
    return dist_dir


def default_dist_dir(repo_root: Path, pyodide_version: str) -> Path:
    return (
        repo_root
        / ".aardvark"
        / "pyodide-distributions"
        / f"aardvark-{workspace_package_version(repo_root)}-pyodide-v{pyodide_version}-full"
    )


def workspace_package_version(repo_root: Path) -> str:
    with (repo_root / "Cargo.toml").open("rb") as file:
        manifest = tomllib.load(file)
    version = manifest.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str) or not version:
        raise SystemExit("workspace package version not found in Cargo.toml")
    return version


def summarize(results: list[dict[str, Any]]) -> dict[str, int]:
    summary: dict[str, int] = {}
    for result in results:
        summary[result["status"]] = summary.get(result["status"], 0) + 1
    return summary


if __name__ == "__main__":
    raise SystemExit(main())
