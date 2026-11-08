"""Running a policy against a world, and dreaming (offline policy improvement).

A *world* answers one question: "the agent proposed design d as the k-th
continuation of node n — what score does it get?"

* LiveWorld   (dream.py) : really evaluates it with `dream-eval` and commits it.
* ReplayWorld (here)     : answers only from recorded history. The exact
  recorded (parent attempt, k) edge is always used when it exists, so the
  deployed policy replays its own tree bit-for-bit. Off the recorded path:
    strict - nothing else is known: MISS
             (all the Dream-RSI paper can assume with a stochastic LLM agent);
    cache  - fall back to any recorded evaluation of the same design anywhere
             in history (valid here because our agent is a deterministic function of the
             parent design, so a design's score does not depend on how it was
             reached).
  An outcome that is not recorded is a MISS: the call is still charged and the
  attempt counts as a failure. A dreamt policy therefore can never profit from
  leaving the region history actually explored.
* OracleWorld (here)     : a full measured grid (+ optional noise); used only to
  validate how well dreaming predicts live behaviour, never by the dream itself.
"""

import random
import statistics

from space import agent
from tree import Node, Tree


class ReplayWorld:
    def __init__(self, history, mode="cache"):
        self.mode = mode
        # Drift correction: machine speed changes between rounds, so a design's
        # cached score is kept relative to the root of the tree that measured it
        # and rescaled to the root of the tree being replayed.
        self.scores = {}
        self.full = set()
        for t in history:
            for n in t.nodes:
                if n.status == "full":
                    self.full.add(n.design)
                elif n.score is not None and t.root.score:
                    self.scores.setdefault(n.design, []).append(n.score / t.root.score)
        self.scores = {d: statistics.fmean(v) for d, v in self.scores.items()}
        self.misses = 0

    def bind(self, rec_tree):
        """Replaying recorded tree `rec_tree` (strict mode needs its edges)."""
        self.rec_tree = rec_tree
        self.rec_edges = rec_tree.edges()
        return self

    def outcome(self, parent, k, design):
        # Exact recorded edge first (both modes): the deployed policy replays its
        # own tree bit-for-bit.
        r = self.rec_edges.get((parent.rec, k)) if parent.rec is not None else None
        if r is not None:
            return r.score, r.status, r.id
        if self.mode == "strict":
            self.misses += 1
            return None, "miss", None
        if design in self.full:
            return None, "full", None
        if design in self.scores:
            return self.scores[design] * self.rec_tree.root.score, "ok", None
        self.misses += 1
        return None, "miss", None


class OracleWorld:
    def __init__(self, oracle, noise=0.0, seed=0):
        self.oracle, self.noise = oracle, noise
        self.rng = random.Random(seed)
        self.misses = 0

    def bind(self, _):
        return self

    def outcome(self, parent, k, design):
        s = self.oracle.get(design)
        if s is None:
            return None, "full", None
        return s * (1 + self.rng.gauss(0, self.noise)) if self.noise else s, "ok", None


def run_tree(policy, world, root, budget, arm="", rnd=0, salt="", seed=0, on_node=None):
    """Run `policy` from root node until it stops or spends `budget` calls."""
    t = Tree(arm=arm, round=rnd, budget=budget, policy=repr(policy))
    r = Node(0, None, None, root.design, root.score, root.status, sha=root.sha, rec=0)
    t.nodes.append(r)
    t.best_trace.append(r.score)
    policy.reset(seed)
    while t.calls < budget:
        nid = policy.select(t)
        if nid is None:
            break
        p = t.nodes[nid]
        k = p.n_exp
        p.n_exp += 1
        design, change = agent(p.design, k, salt)
        score, status, rec = world.outcome(p, k, design)
        n = Node(len(t.nodes), p.id, k, design, score, status, change, rec=rec)
        if on_node:
            on_node(t, p, n)
        t.add(n)
    return t


def gain(t):
    b = t.best()
    return 0.0 if b is None or t.root.score is None else (t.root.score - b.score) / t.root.score


def objective(trees, budget, lam):
    """J = mean over trees of [relative speedup found - lam * fraction of budget spent]."""
    return statistics.fmean(gain(t) - lam * t.calls / budget for t in trees)


def replay(policy, history, budget, lam, mode="cache", salt=""):
    w = ReplayWorld(history, mode)
    trees = [run_tree(policy, w.bind(rt), rt.root, budget, salt=salt, seed=i) for i, rt in enumerate(history)]
    calls = sum(t.calls for t in trees)
    return {
        "J": objective(trees, budget, lam),
        "gain": statistics.fmean(gain(t) for t in trees),
        "calls": calls,
        "miss_rate": w.misses / calls if calls else 0.0,
        "trees": trees,
    }


def dream(current, history, budget, lam, n_candidates, seed, mode="cache", salt="", families=None, extra=()):
    """Score the deployed policy plus `n_candidates` alternatives by replay over the
    whole history; return the argmax (the deployed policy wins ties, so the
    selected policy is never worse than the current one *on replay*)."""
    import policies as P

    rng = random.Random(seed)
    cands = [current, *extra] + [P.sample(rng, families) for _ in range(n_candidates)]
    scored = []
    for i, c in enumerate(cands):
        r = replay(c, history, budget, lam, mode, salt)
        r.pop("trees")
        scored.append((r["J"], -i, c, r))
    scored.sort(key=lambda x: (x[0], x[1]), reverse=True)
    return scored[0][2], [(c, r) for _, _, c, r in scored]
