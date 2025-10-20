#!/usr/bin/env python3
"""Render benchmark JSON output as Markdown tables."""
import argparse
import json
from collections import defaultdict


def _key(item, key, default="-"):
    value = item.get(key)
    if value is None:
        return default
    return value


def load(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def render(results):
    grouped = defaultdict(lambda: defaultdict(list))
    for entry in results:
        scenario = entry.get("scenario", "unknown").capitalize()
        profile = entry.get("profile", "-")
        grouped[scenario][profile].append(entry)

    lines = []
    for scenario in sorted(grouped):
        lines.append(f"### {scenario}")
        for profile in sorted(grouped[scenario]):
            lines.append(f"#### Profile: {profile}")
            lines.append(
                "| Mode | Invocation | Path | Cleanup | Iter | Avg ms | Min ms | Max ms | Std ms | P50 ms | P95 ms | P99 ms | RSS (MiB) |"
            )
            lines.append(
                "|------|------------|------|---------|-----:|-------:|-------:|-------:|-------:|-------:|-------:|-------:|-----------:|"
            )
            entries = sorted(
                grouped[scenario][profile],
                key=lambda e: (e.get("mode", ""), e.get("path", "")),
            )
            for item in entries:
                total = item.get("total", {})
                lines.append(
                    "| {mode} | {invocation} | {path} | {cleanup} | {iterations} | {avg:.2f} | {min:.2f} | {max:.2f} | {std:.2f} | {p50:.2f} | {p95:.2f} | {p99:.2f} | {rss} |".format(
                        mode=item.get("mode", "-"),
                        invocation=_key(item, "invocation"),
                        path=_key(item, "path"),
                        cleanup=_key(item, "cleanup"),
                        iterations=item.get("iterations", 0),
                        avg=total.get("avg_ms", 0.0),
                        min=total.get("min_ms", 0.0),
                        max=total.get("max_ms", 0.0),
                        std=total.get("std_ms", 0.0),
                        p50=total.get("p50_ms", 0.0),
                        p95=total.get("p95_ms", 0.0),
                        p99=total.get("p99_ms", 0.0),
                        rss=f"{item.get('rss_mib', ''):.2f}" if item.get("rss_mib") is not None else "-",
                    )
                )
            lines.append("")
        lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results_json", help="Path to JSON produced by aardvark-perf")
    args = parser.parse_args()

    results = load(args.results_json)
    print(render(results))


if __name__ == "__main__":
    main()
