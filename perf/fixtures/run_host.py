import argparse
import builtins
import json
import math
from pathlib import Path
import resource
import subprocess
import sys
import time

from scenarios import load_handler


FIXTURE_DIR = Path(__file__).resolve().parent / "inputs"


def _load_echo(profile: str):
    path = FIXTURE_DIR / f"echo_{profile}.txt"
    data = path.read_text()
    return data


def _load_numpy(profile: str):
    path = FIXTURE_DIR / f"numpy_{profile}.txt"
    return {"size": int(path.read_text().strip())}


def _matrix_size(profile: str) -> int:
    if profile == "low":
        return 64
    if profile == "medium":
        return 128
    if profile == "high":
        return 256
    raise RuntimeError(f"unsupported matrix profile '{profile}'")


def _load_matrix(profile: str):
    return {"size": _matrix_size(profile)}


def _load_pandas(profile: str):
    path = FIXTURE_DIR / f"pandas_{profile}.txt"
    return {"rows": int(path.read_text().strip())}


def _load_matplotlib(profile: str):
    path = FIXTURE_DIR / f"matplotlib_{profile}.txt"
    return {"points": int(path.read_text().strip())}


def _tensor_length(profile: str) -> int:
    if profile == "low":
        return 256
    if profile == "medium":
        return 16_384
    if profile == "high":
        return 262_144
    raise RuntimeError(f"unsupported tensor profile '{profile}'")


def _load_tensor(profile: str):
    length = _tensor_length(profile)
    return [
        (math.sin(index * 0.001953125) + math.cos(index * 0.001953125)) * 0.5
        for index in range(length)
    ]


def build_payload(scenario: str, profile: str):
    if profile == "none":
        return None
    if scenario == "echo":
        return _load_echo(profile)
    if scenario == "numpy":
        return _load_numpy(profile)
    if scenario in {"numpy-matmul", "scipy-sgemm"}:
        return _load_matrix(profile)
    if scenario == "pandas":
        return _load_pandas(profile)
    if scenario == "tensor":
        return _load_tensor(profile)
    if scenario == "matplotlib":
        return _load_matplotlib(profile)
    raise RuntimeError(f"unknown scenario '{scenario}'")


def timing_stats(samples):
    if not samples:
        return {
            "avg_ms": 0.0,
            "min_ms": 0.0,
            "max_ms": 0.0,
            "std_ms": 0.0,
            "p50_ms": 0.0,
            "p95_ms": 0.0,
            "p99_ms": 0.0,
        }
    avg = sum(samples) / len(samples)
    return {
        "avg_ms": avg * 1000.0,
        "min_ms": min(samples) * 1000.0,
        "max_ms": max(samples) * 1000.0,
        "std_ms": (sum((x - avg) ** 2 for x in samples) / len(samples)) ** 0.5 * 1000.0,
        "p50_ms": _percentile(samples, 0.50) * 1000.0,
        "p95_ms": _percentile(samples, 0.95) * 1000.0,
        "p99_ms": _percentile(samples, 0.99) * 1000.0,
    }


def _percentile(samples, fraction):
    if not samples:
        return 0.0
    ordered = sorted(samples)
    if len(ordered) == 1:
        return ordered[0]
    position = fraction * (len(ordered) - 1)
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    weight = position - lower
    return ordered[lower] * (1.0 - weight) + ordered[upper] * weight


def _set_input(payload):
    if payload is not None:
        builtins.__aardvark_input = payload
    elif hasattr(builtins, "__aardvark_input"):
        del builtins.__aardvark_input


def _clear_input():
    if hasattr(builtins, "__aardvark_input"):
        del builtins.__aardvark_input


def _run_warm_handler(scenario: str, profile: str, iterations: int):
    handler = load_handler(scenario)
    payload = build_payload(scenario, profile)
    samples = []
    for _ in range(iterations):
        start = time.perf_counter()
        _set_input(payload)
        result = handler()
        _ = result
        samples.append(time.perf_counter() - start)
    _clear_input()
    return {
        "total": timing_stats(samples),
        "prepare": timing_stats([]),
        "run": timing_stats(samples),
    }


def _run_prepare_and_handler(scenario: str, profile: str, iterations: int):
    prepare_samples = []
    run_samples = []
    total_samples = []
    for _ in range(iterations):
        start = time.perf_counter()
        handler = load_handler(scenario)
        payload = build_payload(scenario, profile)
        prepared_at = time.perf_counter()
        _set_input(payload)
        result = handler()
        _ = result
        finished_at = time.perf_counter()
        prepare_samples.append(prepared_at - start)
        run_samples.append(finished_at - prepared_at)
        total_samples.append(finished_at - start)
    _clear_input()
    return {
        "total": timing_stats(total_samples),
        "prepare": timing_stats(prepare_samples),
        "run": timing_stats(run_samples),
    }


def _run_process(scenario: str, profile: str, iterations: int):
    samples = []
    script = Path(__file__).resolve()
    for _ in range(iterations):
        start = time.perf_counter()
        child = subprocess.run(
            [
                sys.executable,
                str(script),
                "--scenario",
                scenario,
                "--profile",
                profile,
                "--iterations",
                "1",
                "--host-mode",
                "prepare-run",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        elapsed = time.perf_counter() - start
        if child.returncode != 0:
            raise RuntimeError(child.stderr or child.stdout or "host child process failed")
        samples.append(elapsed)
    return {
        "total": timing_stats(samples),
        "prepare": None,
        "run": None,
    }


def _rss_mib(host_mode: str) -> float:
    usage_kind = resource.RUSAGE_CHILDREN if host_mode == "process" else resource.RUSAGE_SELF
    usage = resource.getrusage(usage_kind)
    rss = usage.ru_maxrss
    if sys.platform == "darwin":
        rss_kib = rss // 1024
    else:
        rss_kib = rss
    return float(rss_kib) / 1024.0


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--iterations", type=int, default=10)
    parser.add_argument("--profile", default="none")
    parser.add_argument(
        "--host-mode",
        choices=["warm-handler", "prepare-run", "process"],
        default="warm-handler",
    )
    args = parser.parse_args()

    scenario = args.scenario.lower()
    try:
        if args.host_mode == "warm-handler":
            result = _run_warm_handler(scenario, args.profile, args.iterations)
        elif args.host_mode == "prepare-run":
            result = _run_prepare_and_handler(scenario, args.profile, args.iterations)
        elif args.host_mode == "process":
            result = _run_process(scenario, args.profile, args.iterations)
        else:
            raise RuntimeError(f"unsupported host mode '{args.host_mode}'")
    except RuntimeError as exc:
        raise SystemExit(str(exc)) from exc

    payload = {
        "scenario": scenario,
        "iterations": args.iterations,
        "host_mode": args.host_mode,
        "total": result["total"],
        "prepare": result["prepare"],
        "run": result["run"],
        "rss_mib": _rss_mib(args.host_mode),
        "python_version": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
        "profile": args.profile,
    }
    json.dump(payload, fp=sys.stdout)


if __name__ == "__main__":
    main()
