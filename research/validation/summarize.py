#!/usr/bin/env python3
"""Paired bootstrap intervals across seeds; never pool unlike objective scales."""
import collections
import json
import random
import statistics
import sys

rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
groups = collections.defaultdict(dict)
for row in rows:
    if row["type"] == "result":
        key = row["task"], row["method"]
        if row["seed"] in groups[key]:
            raise ValueError(f"duplicate seed for {key}")
        groups[key][row["seed"]] = row
print("| Task | Method | Mean regret ↓ | Paired regret difference vs refine (95% bootstrap CI) | Mean evaluations |")
print("|---|---|---:|---:|---:|")
for (task, method), results in sorted(groups.items()):
    baseline = groups[task, "refine_best"]
    assert results.keys() == baseline.keys(), "unpaired seeds"
    diff = [results[s]["regret"] - baseline[s]["regret"] for s in sorted(results)]
    rng = random.Random(42)
    means = sorted(statistics.mean(rng.choices(diff, k=len(diff))) for _ in range(5000))
    lo, hi = means[125], means[4874]
    regret = statistics.mean(r["regret"] for r in results.values())
    cost = statistics.mean(r["evaluations"] for r in results.values())
    print(f"| {task} | {method} | {regret:.3f} | {statistics.mean(diff):+.3f} [{lo:+.3f}, {hi:+.3f}] | {cost:.1f} |")
