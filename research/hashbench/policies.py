"""Exploration policies: executable code that decides which attempt to continue
next and when to stop. A policy never runs anything itself, so it can be run
live (real evaluations) or dreamt (replayed over recorded history) unchanged.

    policy.reset(seed); node_id_or_None = policy.select(tree)

`spec()` / `make(spec)` round-trip a policy through JSON so the exact policy
used for every round is versioned in git.
"""

import math
import random

INF = 10**9


class Policy:
    family = "base"
    grid = {}  # param -> candidate values (used by the dream search)

    def __init__(self, **params):
        self.params = params
        self.rng = random.Random(0)

    def reset(self, seed):
        self.rng = random.Random(seed)

    def spec(self):
        return {"family": self.family, "params": self.params}

    def __repr__(self):
        p = ",".join(f"{k}={'inf' if v == INF else v}" for k, v in self.params.items())
        return f"{self.family}({p})"

    # shared helpers ------------------------------------------------------
    def candidates(self, t):
        kmax = self.params.get("kmax", INF)
        return [n for n in t.nodes if n.score is not None and n.n_exp < kmax]

    def should_stop(self, t):
        pat = self.params.get("patience", INF)
        return t.stalled(self.params.get("eps", 0.0)) >= pat


class RefineBest(Policy):
    """pi_0, hand-written: always continue the best attempt found so far and
    spend the whole budget (a sequential 'refine the incumbent' loop)."""

    family = "refine_best"

    def select(self, t):
        c = self.candidates(t)
        return min(c, key=lambda n: (n.score, -n.id)).id if c else None


class UCB(Policy):
    """Tree-search bandit: value = relative quality + c * exploration bonus,
    at most `kmax` continuations per node, stop after `patience` calls without
    a relative improvement > eps."""

    family = "ucb"
    grid = {
        "c": [0.0, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5],
        "kmax": [1, 2, 3, 4, 6, 8, INF],
        "patience": [4, 6, 8, 12, 16, 24, INF],
        "eps": [0.0, 0.005, 0.01, 0.02, 0.05],
    }

    def select(self, t):
        if self.should_stop(t):
            return None
        c = self.candidates(t)
        if not c:
            return None
        best = t.best().score
        lnN = math.log(t.calls + 2)
        cc = self.params["c"]
        return max(c, key=lambda n: (-(n.score / best - 1) + cc * math.sqrt(lnN / (1 + n.n_exp)), n.id)).id


class Beam(Policy):
    """Keep the top-m attempts; continue the least-continued of them."""

    family = "beam"
    grid = {
        "m": [1, 2, 3, 4, 6, 8],
        "kmax": [1, 2, 3, 4, 6, 8, INF],
        "patience": [4, 6, 8, 12, 16, 24, INF],
        "eps": [0.0, 0.005, 0.01, 0.02, 0.05],
    }

    def select(self, t):
        if self.should_stop(t):
            return None
        c = sorted(self.candidates(t), key=lambda n: n.score)[: self.params["m"]]
        if not c:
            return None
        return min(c, key=lambda n: (n.n_exp, n.score)).id


class Softmax(Policy):
    """Sample an attempt with probability ~ exp(-(score/best - 1) / T)."""

    family = "softmax"
    grid = {
        "T": [0.005, 0.01, 0.02, 0.05, 0.1, 0.3],
        "kmax": [1, 2, 3, 4, 6, 8, INF],
        "patience": [4, 6, 8, 12, 16, 24, INF],
        "eps": [0.0, 0.005, 0.01, 0.02, 0.05],
    }

    def select(self, t):
        if self.should_stop(t):
            return None
        c = self.candidates(t)
        if not c:
            return None
        best = min(n.score for n in c)  # relative to the best *candidate*: no underflow
        w = [math.exp(-(n.score / best - 1) / self.params["T"]) for n in c]
        return self.rng.choices(c, weights=w)[0].id


class Repair(Policy):
    """v2, written by the policy-development agent (Claude) after dreams r2-r6
    over the v1 families kept selecting pi_0. Diagnosis from the recorded trees:

    * ~45% of calls were designs that overflow ('full'), and round 1's
      breakthrough (L 0->2) took pi_0 32 calls of hammering one node. A failed
      attempt is often one mutation away from a design that fits (L1 -> L2), so
      failed attempts may be continued too: valued as their nearest valid
      ancestor's score times (1 + fp), at most `kfail` times.
    * Only 11 distinct designs in round 1's 41 nodes: the agent's k-th proposal
      depends on (design, k), so continuing a fresh *copy* of an already
      explored design replays old proposals. With `dedup`, all copies of a
      design act as one: the policy continues the copy that has explored
      furthest and values the design by the mean of its measurements.

    Plus a UCB bonus `c`, at most `kmax` continuations per design, and
    patience-based stopping."""

    family = "repair"
    grid = {
        "c": [0.0, 0.01, 0.02, 0.05, 0.1],
        "fp": [0.0, 0.01, 0.03, 0.1, 0.3],
        "kmax": [2, 3, 4, 6, 8, INF],
        "kfail": [0, 1, 2, 3, 4],
        "patience": [6, 8, 12, 16, 24, INF],
        "eps": [0.0, 0.005, 0.01, 0.02],
        "dedup": [False, True],
    }

    def select(self, t):
        if self.should_stop(t):
            return None
        p = self.params
        groups = {}
        for n in t.nodes:
            key = n.design if p.get("dedup") else n.id
            groups.setdefault(key, []).append(n)
        best = t.best().score
        lnN = math.log(t.calls + 2)
        top = None
        for g in groups.values():
            n = max(g, key=lambda x: (x.n_exp, -x.id))  # furthest-explored copy
            ok = [x.score for x in g if x.score is not None]
            if ok:
                v, lim = sum(ok) / len(ok), p["kmax"]
            elif any(x.status == "full" for x in g):
                a = n
                while a.parent is not None and a.score is None:
                    a = t.nodes[a.parent]
                if a.score is None:
                    continue
                v, lim = a.score * (1 + p["fp"]), p["kfail"]
            else:
                continue
            if n.n_exp >= lim:
                continue
            val = -(v / best - 1) + p["c"] * math.sqrt(lnN / (1 + n.n_exp))
            if top is None or val >= top[0]:  # ties -> newest attempt
                top = (val, n.id)
        return top[1] if top else None


FAMILIES = {p.family: p for p in (RefineBest, UCB, Beam, Softmax, Repair)}
SEARCHABLE_V1 = [UCB, Beam, Softmax]
SEARCHABLE = [UCB, Beam, Softmax, Repair]


def make(spec):
    return FAMILIES[spec["family"]](**spec.get("params", {}))


def pi0():
    return RefineBest()


def sample(rng, families=None):
    fam = rng.choice(families or SEARCHABLE)
    return fam(**{k: rng.choice(v) for k, v in fam.grid.items()})
