#!/usr/bin/env python3
"""Score a compare.rs JSONL output for the policy track (lower is better).

score = mean over (task, method) of log(1 + mean regret) for the adaptive arms.
Fails (exit 3) if any fixed-budget arm did not spend exactly the protocol budget,
or if seeds are missing/duplicated: a cheaper or partial run must not look better.
"""
import collections
import json
import math
import sys

ARMS = ("adaptive_cache", "adaptive_strict")
EQUAL_BUDGET = ARMS + ("refine_best", "successive_halving")

rows = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
proto = {r["task"]: r for r in rows if r["type"] == "protocol"}
res = collections.defaultdict(dict)
for r in rows:
    if r["type"] != "result":
        continue
    key = r["task"], r["method"]
    if r["seed"] in res[key]:
        print(f"duplicate seed {r['seed']} for {key}", file=sys.stderr)
        sys.exit(3)
    res[key][r["seed"]] = r

if set(proto) != {"smooth", "rugged", "deceptive"}:
    print(f"unexpected task set {sorted(proto)}", file=sys.stderr)
    sys.exit(3)

metrics, terms = {}, []
for task, p in sorted(proto.items()):
    want = p["test_seeds"]
    for method in EQUAL_BUDGET:
        got = res.get((task, method), {})
        if len(got) != want:
            print(f"{task}/{method}: {len(got)} seeds, expected {want}", file=sys.stderr)
            sys.exit(3)
        bad = [s for s, r in got.items() if r["evaluations"] != p["evaluation_budget"]]
        if bad:
            print(f"{task}/{method}: unequal budget on seeds {bad[:5]}", file=sys.stderr)
            sys.exit(3)
        mean = sum(r["regret"] for r in got.values()) / want
        metrics[f"{task}.{method}"] = round(mean, 4)
        if method in ARMS:
            terms.append(math.log1p(mean))

print(json.dumps({"score": round(sum(terms) / len(terms), 6), "status": "ok", **metrics}))
