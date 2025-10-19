import argparse
import json
import resource
import sys
import time

from scenarios import SCENARIOS


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
    args = parser.parse_args()

    scenario = args.scenario.lower()
    try:
        handler = SCENARIOS[scenario]()
    except KeyError as exc:
        raise SystemExit(f"unknown scenario: {scenario}") from exc

    samples = []
    for _ in range(args.iterations):
        start = time.perf_counter()
        result = handler()
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
    }
    json.dump(payload, fp=sys.stdout)


if __name__ == "__main__":
    main()
