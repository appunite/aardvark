import argparse
import json
import resource
import sys
import time

from scenarios import load_handler


SIZE_HINTS = {
    "echo": {"low": 16, "medium": 1_000, "high": 1_000_000},
    "numpy": {"low": 64, "medium": 4_096, "high": 1_000_000},
    "pandas": {"low": 128, "medium": 10_000, "high": 1_000_000},
}


def build_payload(scenario: str, profile: str):
    if profile == "none":
        return None
    hints = SIZE_HINTS.get(scenario)
    if hints is None:
        return None
    hint = hints.get(profile, 0)
    if scenario == "echo":
        return "x" * hint
    if scenario == "numpy":
        return {"size": hint}
    if scenario == "pandas":
        return {"rows": hint}
    return None


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
        handler = load_handler(scenario, args.profile)
    except RuntimeError as exc:
        raise SystemExit(str(exc)) from exc

    payload = build_payload(scenario, args.profile)
    samples = []
    for _ in range(args.iterations):
        start = time.perf_counter()
        result = handler(payload)
        _ = result  # ensure work executes; result ignored
        samples.append(time.perf_counter() - start)

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
