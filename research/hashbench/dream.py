#!/usr/bin/env python3
"""Lightweight Dream-RSI on git, applied to high-load hash-table design.

  explore  - the current policy steers the discovery agent; every attempt is
             really evaluated (dream-eval) and committed to git.
  remember - git commits / trailers / notes / refs (see gitmem.py).
  dream    - load history *from git*, replay the deployed policy and many
             candidate policies over it with zero new evaluations, keep the best.
  improve  - commit the selected policy and deploy it for the next round.

Commands
  experiment  run the live protocol: shared round 1, then per round an arm that
              keeps pi_0 ("fixed", = Recursive Fixed Exploration) and an arm
              that dreams ("dream", = Dream-RSI)
  oracle      measure every design on the grid (validation only)
  synthetic   repeat the protocol many times in an oracle-backed world
  fidelity    does replay rank policies the way the (oracle) world does?
  show        print a discovery tree reconstructed from git
  report      summarise live results against the oracle
"""

import argparse
import json
import random
import statistics
import subprocess
import sys
import time
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import policies as P  # noqa: E402
import sim  # noqa: E402
import space  # noqa: E402
from gitmem import Git, repo_root  # noqa: E402
from tree import Node  # noqa: E402

ROOT = repo_root()
RESULTS = ROOT / "dream" / "results"
EVAL = ROOT / "target" / "release" / "dream-eval"


# --------------------------------------------------------------------------
# Live world: real evaluations + git memory
# --------------------------------------------------------------------------
class Evaluator:
    def __init__(self, cpu="3"):
        self.p = subprocess.Popen(["taskset", "-c", cpu, str(EVAL)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.n = 0

    def __call__(self, design):
        self.p.stdin.write(design + "\n")
        self.p.stdin.flush()
        self.n += 1
        return json.loads(self.p.stdout.readline())


class LiveWorld:
    def __init__(self, ev):
        self.ev, self.last, self.misses = ev, None, 0

    def outcome(self, parent, k, design):
        self.last = self.ev(design)
        return self.last["score"], self.last["status"], None


def live_round(git, ev, arm, rnd, policy, root, budget, log, remeasure_root=False):
    """Explore one round with real evaluations, committing every attempt."""
    world = LiveWorld(ev)
    parent_of_root = root.sha
    res = None
    if remeasure_root or root.score is None:
        # Re-baseline: the machine's speed drifts (other jobs share DRAM/L3), so the
        # round's gains are measured against a fresh measurement of its root.
        res = ev(root.design)
        root = Node(0, None, None, root.design, res["score"], res["status"], sha=root.sha, change="root")
    t0 = time.time()

    def on_node(t, p, n):
        git.commit_node(t, n, p.sha, world.last)
        b = min(t.best().score, n.score if n.score is not None else 1e9)
        log(f"  {arm} r{rnd} n{n.id:03d} <- n{p.id:03d} {n.design:24} {n.status:4} {'' if n.score is None else f'{n.score:7.2f}'}  best {b:7.2f}")

    # commit the root first so children can point at it
    from tree import Tree

    tmp = Tree(arm=arm, round=rnd, budget=budget, policy=repr(policy), nodes=[root])
    git.commit_node(tmp, root, parent_of_root, res)
    t = sim.run_tree(policy, world, root, budget, arm, rnd, seed=rnd, on_node=on_node)
    best = t.best()
    git.set_ref(f"refs/heads/dream/{arm}/r{rnd:02d}-best", best.sha)
    log(f"  {arm} r{rnd}: {t.calls} calls, best {best.design} {best.score:.2f} ns/op ({time.time() - t0:.0f}s)")
    return t


def load_history(git, arm, upto):
    trees = [git.load_tree("shared", 1)]
    for r in range(2, upto + 1):
        t = git.load_tree(arm, r)
        if t:
            trees.append(t)
    return trees


def cmd_experiment(a):
    git = Git(ROOT)
    git.world()
    RESULTS.mkdir(parents=True, exist_ok=True)
    logf = open(RESULTS / "experiment.log", "a")

    def log(s):
        print(s, flush=True)
        logf.write(s + "\n")
        logf.flush()

    ev = Evaluator()
    summary = {"budget": a.budget, "lam": a.lam, "candidates": a.cands, "rounds": []}
    deployed = {}
    for arm in ("fixed", "dream"):  # resume: the deployed policy is whatever git says
        doc = git.run("show", f"refs/heads/dream/{arm}/policy:policy.json", check=False)
        deployed[arm] = P.make(json.loads(doc)["policy"]) if doc else P.pi0()
    # ---- round 1, shared by both arms (identical by construction) -------
    if not git.load_tree("shared", 1):
        log("round 1 (shared, pi_0)")
        for arm in ("fixed", "dream"):
            git.commit_policy(arm, 1, P.pi0().spec(), {"repr": repr(P.pi0()), "why": "hand-written initial policy pi_0"}, ROOT / "dream" / "policies.py")
        live_round(git, ev, "shared", 1, P.pi0(), Node(0, None, None, space.ROOT, None, "ok", sha=git.world()[0]), a.budget, log)
    for rnd in range(2, a.rounds + 1):
        for arm in ("fixed", "dream"):
            if git.load_tree(arm, rnd):
                log(f"{arm} r{rnd} already in git, skipping")
                continue
            hist = load_history(git, arm, rnd - 1)  # memory comes from git, nothing else
            info = {"repr": repr(deployed[arm])}
            if arm == "dream":
                t0 = time.time()
                best, ranked = sim.dream(deployed[arm], hist, a.budget, a.lam, a.cands, seed=rnd, mode=a.mode)
                cur = next(r for c, r in ranked if c is deployed[arm])
                win = ranked[0][1]
                info = {
                    "repr": repr(best),
                    "dream": {"history_trees": len(hist), "candidates": len(ranked), "mode": a.mode, "lam": a.lam, "seconds": round(time.time() - t0, 1),
                              "winner": win, "deployed_before": cur, "top5": [[repr(c), r] for c, r in ranked[:5]]},
                    "why": f"dreamt over {len(hist)} recorded trees, {len(ranked)} candidates; replay J {cur['J']:.4f} -> {win['J']:.4f}",
                }
                log(f"dream r{rnd}: {len(ranked)} candidates in {time.time() - t0:.1f}s; J {cur['J']:.4f} ({deployed[arm]!r}) -> {win['J']:.4f} ({best!r}), miss {win['miss_rate']:.2f}")
                deployed[arm] = best
            git.commit_policy(arm, rnd, deployed[arm].spec(), info, ROOT / "dream" / "policies.py")
            prev = hist[-1]
            root = prev.best()
            root = Node(0, None, None, root.design, root.score, "ok", sha=root.sha)
            log(f"{arm} r{rnd} policy {deployed[arm]!r} root {root.design} {root.score:.2f}")
            t = live_round(git, ev, arm, rnd, deployed[arm], root, a.budget, log, remeasure_root=rnd >= a.remeasure_from)
            summary["rounds"].append({"arm": arm, "round": rnd, "policy": repr(deployed[arm]), "calls": t.calls, "best": t.best().design, "best_score": t.best().score})
    log(f"evaluator calls this session: {ev.n}")


# --------------------------------------------------------------------------
# Oracle grid (validation only — the dream never sees it)
# --------------------------------------------------------------------------
def load_oracle(path=None):
    """One measurement per design from a single uniform sweep (repeats taken later,
    under different machine load, live in oracle-repeats.jsonl)."""
    path = path or RESULTS / "oracle.jsonl"
    o = {}
    for line in open(path):
        r = json.loads(line)
        o.setdefault(r["design"], r["score"])
    return o


def cmd_oracle(a):
    RESULTS.mkdir(parents=True, exist_ok=True)
    path = RESULTS / (a.out or "oracle.jsonl")
    done = set()
    if path.exists():
        done = {json.loads(l)["design"] for l in open(path)}
    if a.designs:  # explicit repeats go to a separate file
        path = RESULTS / (a.out or "oracle-repeats.jsonl")
    todo = [d for d in (a.designs.split(",") if a.designs else space.all_designs()) if d not in done or a.designs]
    ev = Evaluator()
    with open(path, "a") as f:
        for i, d in enumerate(todo):
            r = ev(d)
            f.write(json.dumps(r) + "\n")
            f.flush()
            if i % 50 == 0:
                print(f"{i}/{len(todo)} {d} {r['score']}", flush=True)


# --------------------------------------------------------------------------
# Synthetic replicates in an oracle-backed world
# --------------------------------------------------------------------------
def protocol(oracle, salt, rounds, budget, lam, cands, noise, mode, seed, arms=("fixed", "dream"), families=None):
    out = {}
    root = Node(0, None, None, space.ROOT, oracle[space.ROOT], "ok")
    w1 = sim.OracleWorld(oracle, noise, seed=zlib.crc32(f"{salt}|1".encode()))
    r1 = sim.run_tree(P.pi0(), w1, root, budget, "shared", 1, salt=salt, seed=1)
    for arm in arms:
        hist, pol, rows = [r1], P.pi0(), []
        for rnd in range(2, rounds + 1):
            if arm == "dream":
                pol, _ = sim.dream(pol, hist, budget, lam, cands, seed=seed * 100 + rnd, mode=mode, salt=salt, families=families)
            b = hist[-1].best()
            w = sim.OracleWorld(oracle, noise, seed=zlib.crc32(f"{salt}|{arm}|{rnd}".encode()))
            t = sim.run_tree(pol, w, Node(0, None, None, b.design, b.score, "ok"), budget, arm, rnd, salt=salt, seed=rnd)
            hist.append(t)
        out[arm] = hist
    return out


def true_best_curve(hist, oracle):
    """Cumulative calls and the oracle (true) score of the incumbent after each round."""
    calls, rows = 0, []
    for t in hist:
        calls += t.calls
        rows.append((calls, oracle[t.best().design], t.policy))
    return rows


def cmd_synthetic(a):
    oracle = load_oracle()
    best = min(v for v in oracle.values() if v is not None)
    res = {"fixed": [], "dream": []}
    t0 = time.time()
    for rep in range(a.reps):
        fams = P.SEARCHABLE_V1 if a.families == "v1" else P.SEARCHABLE
        out = protocol(oracle, f"rep{rep}", a.rounds, a.budget, a.lam, a.cands, a.noise, a.mode, seed=rep, families=fams)
        for arm, hist in out.items():
            res[arm].append({"curve": true_best_curve(hist, oracle), "policies": [t.policy for t in hist]})
        print(f"rep {rep}: " + "  ".join(f"{arm} {r[-1]['curve'][-1][0]} calls -> {r[-1]['curve'][-1][1] / best:.3f}x opt" for arm, r in res.items()), flush=True)
    (RESULTS / f"synthetic-{a.tag}.json").write_text(json.dumps({"args": vars(a), "oracle_best": best, "res": res}, indent=1))
    print(f"done in {time.time() - t0:.0f}s")
    summarize_synth(res, best, a.rounds)


def summarize_synth(res, best, rounds):
    print(f"{'round':>5} | {'fixed calls':>11} {'fixed /opt':>10} | {'dream calls':>11} {'dream /opt':>10}")
    for i in range(rounds):
        row = []
        for arm in ("fixed", "dream"):
            c = [r["curve"][i] for r in res[arm]]
            row += [statistics.fmean(x[0] for x in c), statistics.fmean(x[1] / best for x in c)]
        print(f"{i + 1:>5} | {row[0]:11.1f} {row[1]:10.4f} | {row[2]:11.1f} {row[3]:10.4f}")


# --------------------------------------------------------------------------
# Fidelity: does dreaming rank policies like the real world would?
# --------------------------------------------------------------------------
def cmd_fidelity(a):
    oracle = load_oracle()
    git = Git(ROOT)
    hist = load_history(git, a.arm, a.upto)
    rng = random.Random(7)
    cands = [P.pi0()] + [P.sample(rng) for _ in range(a.cands)]
    rows = []
    for c in cands:
        rs = sim.replay(c, hist, a.budget, a.lam, "strict")
        rc = sim.replay(c, hist, a.budget, a.lam, "cache")
        # "truth": the same policy run from the same roots against the oracle world
        trees = [sim.run_tree(c, sim.OracleWorld(oracle).bind(None), Node(0, None, None, t.root.design, oracle[t.root.design], "ok"), a.budget, seed=i) for i, t in enumerate(hist)]
        rows.append((repr(c), rs["J"], rc["J"], sim.objective(trees, a.budget, a.lam), rs["miss_rate"], rc["miss_rate"]))

    def rank(v):
        o = sorted(range(len(v)), key=lambda i: v[i])
        r = [0] * len(v)
        for k, i in enumerate(o):
            r[i] = k
        return r

    def spearman(x, y):
        return statistics.correlation(rank(x), rank(y))

    truth = [r[3] for r in rows]
    out = {
        "history_trees": len(hist),
        "candidates": len(rows),
        "spearman_strict": spearman([r[1] for r in rows], truth),
        "spearman_cache": spearman([r[2] for r in rows], truth),
        "miss_strict": statistics.fmean(r[4] for r in rows),
        "miss_cache": statistics.fmean(r[5] for r in rows),
    }
    for mode, col in (("strict", 1), ("cache", 2)):
        top = max(rows, key=lambda r: r[col])
        out[f"pick_{mode}"] = {"policy": top[0], "dream_J": top[col], "true_J": top[3], "true_rank_pct": sum(t <= top[3] for t in truth) / len(truth)}
    out["pi0"] = {"strict_J": rows[0][1], "cache_J": rows[0][2], "true_J": rows[0][3]}
    out["true_best"] = max(rows, key=lambda r: r[3])[0::3]
    print(json.dumps(out, indent=1))
    (RESULTS / f"fidelity-{a.arm}-r{a.upto}.json").write_text(json.dumps(out, indent=1))


# --------------------------------------------------------------------------
def cmd_show(a):
    t = Git(ROOT).load_tree(a.arm, a.round)
    print(f"{a.arm} r{a.round}: policy {t.policy}, {t.calls} calls, best {t.best().design} {t.best().score}")
    kids = {}
    for n in t.nodes[1:]:
        kids.setdefault(n.parent, []).append(n)
    b = t.best().id

    def rec(n, depth):
        mark = " *" if n.id == b else ""
        print(f"{'  ' * depth}n{n.id:03d} {n.design:24} {'-' if n.score is None else f'{n.score:.2f}':>7} {n.sha[:9]}{mark}")
        for c in kids.get(n.id, []):
            rec(c, depth + 1)

    rec(t.root, 0)


def cmd_report(a):
    git = Git(ROOT)
    oracle = load_oracle() if (RESULTS / "oracle.jsonl").exists() else {}
    opt = min((v for v in oracle.values() if v is not None), default=None)
    print(f"oracle optimum: {opt}  ({min(oracle, key=lambda d: oracle[d] or 1e9) if oracle else '-'})")
    for arm in ("fixed", "dream"):
        cum = 0
        print(f"--- {arm}")
        for t in load_history(git, arm, 99):
            cum += t.calls
            b = t.best()
            tv = oracle.get(b.design)
            print(f"r{t.round} {t.policy:58} calls {t.calls:3} cum {cum:4}  best {b.design:24} rec {b.score:6.2f}  oracle {tv if tv is None else f'{tv:6.2f}'}  {'' if not (tv and opt) else f'{tv / opt:.3f}x opt'}")


def main():
    ap = argparse.ArgumentParser()
    sp = ap.add_subparsers(dest="cmd", required=True)
    e = sp.add_parser("experiment")
    e.add_argument("--rounds", type=int, default=6)
    e.add_argument("--budget", type=int, default=40)
    e.add_argument("--lam", type=float, default=0.02)
    e.add_argument("--cands", type=int, default=1000)
    e.add_argument("--mode", default="cache", choices=["cache", "strict"])
    e.add_argument("--remeasure-from", type=int, default=7, help="re-evaluate each round's root from this round on")
    o = sp.add_parser("oracle")
    o.add_argument("--designs", default="")
    o.add_argument("--out", default="")
    s = sp.add_parser("synthetic")
    s.add_argument("--reps", type=int, default=20)
    s.add_argument("--rounds", type=int, default=6)
    s.add_argument("--budget", type=int, default=40)
    s.add_argument("--lam", type=float, default=0.02)
    s.add_argument("--cands", type=int, default=300)
    s.add_argument("--noise", type=float, default=0.06)
    s.add_argument("--mode", default="cache", choices=["cache", "strict"])
    s.add_argument("--tag", default="default")
    s.add_argument("--families", default="v2", choices=["v1", "v2"])
    f = sp.add_parser("fidelity")
    f.add_argument("--arm", default="dream")
    f.add_argument("--upto", type=int, default=6)
    f.add_argument("--budget", type=int, default=40)
    f.add_argument("--lam", type=float, default=0.02)
    f.add_argument("--cands", type=int, default=300)
    w = sp.add_parser("show")
    w.add_argument("--arm", default="shared")
    w.add_argument("--round", type=int, default=1)
    sp.add_parser("report")
    a = ap.parse_args()
    {"experiment": cmd_experiment, "oracle": cmd_oracle, "synthetic": cmd_synthetic, "fidelity": cmd_fidelity, "show": cmd_show, "report": cmd_report}[a.cmd](a)


if __name__ == "__main__":
    main()
