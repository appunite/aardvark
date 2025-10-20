import argparse
import builtins
import json
from pathlib import Path
import resource
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


def _load_pandas(profile: str):
    path = FIXTURE_DIR / f"pandas_{profile}.txt"
    return {"rows": int(path.read_text().strip())}


def build_payload(scenario: str, profile: str):
    if profile == "none":
        return None
    if scenario == "echo":
        return _load_echo(profile)
    if scenario == "numpy":
        return _load_numpy(profile)
    if scenario == "pandas":
        return _load_pandas(profile)
    raise RuntimeError(f"unknown scenario '{scenario}'")


def timing_stats(samples):
    if not samples:
        return {"avg_ms": 0.0, "min_ms": 0.0, "max_ms": 0.0}
    avg = sum(samples) / len(samples)
    return {
        "avg_ms": avg * 1000.0,
        "min_ms": min(samples) * 1000.0,
        "max_ms": max(samples) * 1000.0,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--iterations", type=int, default=10)
    parser.add_argument("--profile", default="none")
    args = parser.parse_args()

    scenario = args.scenario.lower()
    try:
        handler = load_handler(scenario)
    except RuntimeError as exc:
        raise SystemExit(str(exc)) from exc

    payload = build_payload(scenario, args.profile)
    samples = []
    for _ in range(args.iterations):
        start = time.perf_counter()
        if payload is not None:
            builtins.__aardvark_input = payload
        elif hasattr(builtins, "__aardvark_input"):
            del builtins.__aardvark_input
        result = handler()
        _ = result  # ensure work executes; result ignored
        samples.append(time.perf_counter() - start)

    if hasattr(builtins, "__aardvark_input"):
        del builtins.__aardvark_input

    usage = resource.getrusage(resource.RUSAGE_SELF)
    rss = usage.ru_maxrss
    if sys.platform == "darwin":
        rss_kib = rss // 1024
    else:
        rss_kib = rss

    payload = {
        "scenario": scenario,
        "iterations": args.iterations,
        "total": timing_stats(samples),
        "rss_kib": int(rss_kib),
        "python_version": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
        "profile": args.profile,
    }
    json.dump(payload, fp=sys.stdout)


if __name__ == "__main__":
    main()
