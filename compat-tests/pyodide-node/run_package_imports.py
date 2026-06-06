#!/usr/bin/env python3
"""Run Pyodide's lockfile-driven package import matrix against Aardvark."""

from __future__ import annotations

import argparse
import json
import tomllib
from pathlib import Path
from typing import Any

from aardvark_selenium import AardvarkGuestError, AardvarkSelenium


UPSTREAM_TEST_ID = "packages/_tests/test_packages_common.py::test_import"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", default="0.29.4")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--dist-dir", type=Path)
    parser.add_argument("--expectations", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument(
        "--package",
        action="append",
        dest="packages",
        help="only run a specific package; may be provided more than once",
    )
    parser.add_argument(
        "--include-upstream-xfail",
        action="store_true",
        help="probe packages Pyodide marks xfail/unsupported on Node",
    )
    parser.add_argument(
        "--reuse-runtime",
        action="store_true",
        help="do not reset the Aardvark runtime between package import cases",
    )
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    dist_dir = resolve_dist_dir(repo_root, args.dist_dir, args.version)
    expectations_path = args.expectations or (
        repo_root
        / "compat-tests"
        / "pyodide-node"
        / "expectations"
        / f"{args.version}-package-imports.toml"
    )
    report_path = args.report or (
        repo_root
        / "target"
        / "compat"
        / "pyodide-node"
        / f"{args.version}-package-imports.json"
    )

    expectations = load_expectations(expectations_path)
    cases = build_package_cases(dist_dir / "pyodide-lock.json")
    if args.packages:
        requested = set(args.packages)
        cases = [case for case in cases if case["name"] in requested]
        missing = sorted(requested - {case["name"] for case in cases})
        if missing:
            raise SystemExit(f"package(s) not found in lockfile: {', '.join(missing)}")

    results: list[dict[str, Any]] = []
    with AardvarkSelenium(
        repo_root,
        dist_dir=dist_dir,
        command_timeout_seconds=args.timeout_seconds,
    ) as selenium:
        for case in cases:
            result = run_package_case(
                selenium,
                case,
                expectations,
                isolate=not args.reuse_runtime,
                include_upstream_xfail=args.include_upstream_xfail,
            )
            results.append(result)
            print(f"{result['status']:9} {result['id']}", flush=True)
            if result.get("reason"):
                print(f"          {result['reason']}", flush=True)

    report = {
        "version": args.version,
        "source": UPSTREAM_TEST_ID,
        "distDir": str(dist_dir),
        "cases": results,
        "summary": summarize(results),
    }
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote report to {report_path}", flush=True)

    return 1 if any(item["status"] == "fail" for item in results) else 0


def run_package_case(
    selenium: AardvarkSelenium,
    case: dict[str, Any],
    expectations: dict[str, dict[str, str]],
    *,
    isolate: bool,
    include_upstream_xfail: bool,
) -> dict[str, Any]:
    name = case["name"]
    imports = case["imports"]
    result_base = {
        "id": f"{UPSTREAM_TEST_ID}[{name}]",
        "package": name,
        "imports": imports,
        "packageType": case["packageType"],
    }

    xfail_reason = expected_xfail_reason(name, expectations)
    if xfail_reason and not include_upstream_xfail:
        return {
            **result_base,
            "status": "xfail",
            "reason": xfail_reason,
        }

    if not imports:
        return {
            **result_base,
            "status": "no_imports",
            "reason": "package has no declared import names in pyodide-lock.json",
        }

    try:
        if isolate:
            selenium.reset()
        selenium.load_package([name])
        for import_name in imports:
            selenium.run(import_statement(import_name))
    except AardvarkGuestError as exc:
        return {
            **result_base,
            "status": "fail",
            "reason": str(exc),
            "response": exc.response,
        }
    except Exception as exc:  # noqa: BLE001 - local runner should report all failures.
        return {
            **result_base,
            "status": "fail",
            "reason": f"{type(exc).__name__}: {exc}",
        }

    return {
        **result_base,
        "status": "pass",
    }


def build_package_cases(lockfile_path: Path) -> list[dict[str, Any]]:
    if not lockfile_path.exists():
        raise SystemExit(f"missing Pyodide lockfile: {lockfile_path}")
    lockfile = json.loads(lockfile_path.read_text(encoding="utf-8"))
    packages = lockfile.get("packages") or {}
    cases: list[dict[str, Any]] = []
    for key in sorted(packages):
        package = packages[key]
        package_type = package.get("package_type") or package.get("packageType")
        if package_type not in {"package", "cpython_module"}:
            continue
        name = str(package.get("name") or key)
        imports = [normalize_import_name(str(item)) for item in package.get("imports") or []]
        cases.append(
            {
                "name": name,
                "packageType": package_type,
                "imports": imports,
            }
        )
    return cases


def import_statement(import_name: str) -> str:
    return "\n".join(
        [
            "import importlib",
            f"importlib.import_module({import_name!r})",
            "True",
        ]
    )


def normalize_import_name(name: str) -> str:
    return name.replace("-", "_").replace(".", "_")


def load_expectations(path: Path) -> dict[str, dict[str, str]]:
    if not path.exists():
        raise SystemExit(f"missing package import expectations: {path}")
    with path.open("rb") as file:
        data = tomllib.load(file)
    return {
        "upstream_xfail": dict(data.get("upstream_xfail", {})),
        "node_unsupported": dict(data.get("node_unsupported", {})),
    }


def expected_xfail_reason(name: str, expectations: dict[str, dict[str, str]]) -> str | None:
    for bucket in ("upstream_xfail", "node_unsupported"):
        reason = expectations.get(bucket, {}).get(name)
        if reason:
            return reason
    return None


def resolve_dist_dir(repo_root: Path, dist_dir: Path | None, pyodide_version: str) -> Path:
    if dist_dir is None:
        dist_dir = default_dist_dir(repo_root, pyodide_version)
    elif not dist_dir.is_absolute():
        dist_dir = repo_root / dist_dir
    if not dist_dir.exists():
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
