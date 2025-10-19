#!/usr/bin/env python3
"""Render benchmark JSON output as Markdown tables."""
import argparse
import json
from collections import defaultdict


def load(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def render(results):
    grouped = defaultdict(list)
    for entry in results:
        grouped[entry["scenario"]].append(entry)

    lines = []
    for scenario in sorted(grouped):
        lines.append(f"### {scenario.capitalize()}")
        lines.append("| Mode | Avg ms | Min ms | Max ms | RSS (KiB) |")
        lines.append("|------|--------|--------|--------|-----------|")
        for item in sorted(grouped[scenario], key=lambda e: e["mode"]):
            lines.append(
                "| {mode} | {avg:.2f} | {min:.2f} | {max:.2f} | {rss} |".format(
                    mode=item["mode"],
                    avg=item["total"]["avg_ms"],
                    min=item["total"]["min_ms"],
                    max=item["total"]["max_ms"],
                    rss=item.get("rss_kib", ""),
                )
            )
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
