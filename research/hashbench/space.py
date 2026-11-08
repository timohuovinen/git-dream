"""Design space and the (fixed) discovery agent.

A *design* is a point on a discrete grid of funnel/SwissTable knobs, written as a
key such as ``W64-b1-L3-r0.5-o0.02`` (the same key `dream-eval` parses).

The *discovery agent* is the thing the exploration policy steers but never
changes. In the Dream-RSI paper it is an LLM coding agent; here it is a
deterministic local-mutation operator: ``agent(parent_design, k)`` returns the
k-th proposal the agent makes when asked to continue from `parent_design`.
Because it is a pure function of (salt, parent design, k), "what would have
happened if we had continued node n one more time" is always computable — only
its *score* may be unknown (that requires a real evaluation).
"""

import hashlib
import itertools
import random

KNOBS = {
    "W": [16, 64],  # SIMD group width (SSE2 16 / AVX-512 64 control bytes)
    "b": [1, 2],  # SIMD groups per level bucket
    "L": [0, 1, 2, 3, 4, 5, 6, 8],  # funnel levels (0 = SwissTable-style)
    "r": [0.25, 0.375, 0.5, 0.625, 0.75],  # level size ratio
    "o": [0.005, 0.01, 0.02, 0.03, 0.05, 0.08, 0.12],  # overflow fraction
}
ORDER = "WbLro"
# With L = 0 every slot is overflow region, so b / r / o are inert.
L0_DEFAULTS = {"b": 1, "r": 0.5, "o": 0.05}


def fmt(v):
    return "%g" % v


def key(d):
    return "-".join(f"{k}{fmt(d[k])}" for k in ORDER)


def parse(s):
    d = {}
    for part in s.split("-"):
        k, v = part[0], part[1:]
        d[k] = type(KNOBS[k][0])(float(v)) if isinstance(KNOBS[k][0], int) else float(v)
    return d


def canon(d):
    d = dict(d)
    if d["L"] == 0:
        d.update(L0_DEFAULTS)
    return d


def canon_key(s):
    return key(canon(parse(s)))


def all_designs():
    seen = []
    for vals in itertools.product(*(KNOBS[k] for k in ORDER)):
        k = key(canon(dict(zip(ORDER, vals))))
        if k not in seen:
            seen.append(k)
    return seen


ROOT = key(canon({"W": 16, "b": 1, "L": 0, "r": 0.5, "o": 0.05}))  # swiss-fixed16


def active(d):
    return ["W", "L"] if d["L"] == 0 else list(ORDER)


_cache = {}


def agent(parent, k, salt=""):
    """k-th proposal from `parent`: change 1 (70%) or 2 knobs by +-1 (80%) or
    +-2 grid steps. Returns (child_design_key, human-readable change)."""
    ck = (salt, parent, k)
    if ck in _cache:
        return _cache[ck]
    seed = int.from_bytes(hashlib.sha256(f"{salt}|{parent}|{k}".encode()).digest()[:8], "little")
    rng = random.Random(seed)
    d = parse(parent)
    out = (parent, "no-op")
    for _ in range(32):
        c = dict(d)
        knobs = active(c)
        n = 1 if rng.random() < 0.7 else 2
        for kn in rng.sample(knobs, min(n, len(knobs))):
            vals = KNOBS[kn]
            i = vals.index(c[kn])
            step = rng.choice([-1, 1]) * (1 if rng.random() < 0.8 else 2)
            c[kn] = vals[min(max(i + step, 0), len(vals) - 1)]
        c = canon(c)
        child = key(c)
        if child != parent:
            change = ", ".join(f"{kn} {fmt(d[kn])}->{fmt(c[kn])}" for kn in ORDER if c[kn] != d[kn])
            out = (child, change)
            break
    _cache[ck] = out
    return out
