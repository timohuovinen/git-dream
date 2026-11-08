"""Git as Dream-RSI's persistent experimental memory.

Layout (all inside the ordinary repository):

  attempt            one commit per evaluated attempt. Its tree is the frozen
                     "world" commit (evaluator + tooling, tag `dream/world`) plus
                     `dream/attempt.json` naming the design that was evaluated.
  parent attempt     the commit's parent (the attempt the policy continued).
                     A round's root commit has the previous round's best
                     attempt as parent, so all rounds form one DAG.
  outcome            commit-message trailers (Dream-Score, Dream-Status, ...);
                     full evaluator output in git notes (refs/notes/dream).
  discovery tree     refs/dream/<arm>/r<NN>/n<NNN>  (one hidden ref per node)
  exploration path   branch dream/<arm>/r<NN>-best -> best attempt of the round
                     (`git log` on it shows the path that led there)
  policy versions    branch dream/<arm>/policy: one commit per round holding
                     policy.json (+ a snapshot of policies.py, the code it names),
                     tagged dream/<arm>/policy-r<NN>.
"""

import json
import os
import re
import subprocess
from pathlib import Path

from tree import Node, Tree

TRAILER = re.compile(r"^Dream-([A-Za-z-]+): ?(.*)$", re.M)
WORLD_TAG = "dream/world"


class Git:
    def __init__(self, repo):
        self.repo = str(repo)
        self._world = None

    def run(self, *args, input=None, check=True):
        r = subprocess.run(["git", *args], cwd=self.repo, input=input, capture_output=True, text=True)
        if check and r.returncode:
            raise RuntimeError(f"git {' '.join(args)}: {r.stderr.strip()}")
        return r.stdout.strip()

    # ---- world (frozen evaluator + tooling) ------------------------------
    def world(self):
        if self._world is None:
            sha = self.run("rev-parse", f"refs/tags/{WORLD_TAG}^{{commit}}")
            root = self.run("ls-tree", sha).splitlines()
            dream = [e for e in self.run("ls-tree", f"{sha}:dream").splitlines() if not e.endswith("\tattempt.json")]
            self._world = (sha, root, dream)
        return self._world

    def _blob(self, text):
        return self.run("hash-object", "-w", "--stdin", input=text)

    def _tree(self, entries):
        return self.run("mktree", input="\n".join(entries) + "\n")

    def commit(self, files, parent, message, dream_files=True):
        """Commit the world tree with extra files under dream/ (or a bare tree)."""
        wsha, root, dream = self.world()
        extra = [f"100644 blob {self._blob(c)}\t{name}" for name, c in files.items()]
        if dream_files:
            dtree = self._tree(sorted(dream + extra, key=lambda e: e.split("\t")[1]))
            entries = [e if not e.endswith("\tdream") else f"040000 tree {dtree}\tdream" for e in root]
            tree = self._tree(entries)
        else:
            tree = self._tree(extra)
        args = ["commit-tree", tree, "-F", "-"] + (["-p", parent] if parent else [])
        return self.run(*args, input=message)

    def note(self, sha, obj):
        self.run("notes", "--ref=dream", "add", "-f", "-F", "-", sha, input=json.dumps(obj, indent=1) + "\n")

    def set_ref(self, ref, sha):
        self.run("update-ref", ref, sha)

    # ---- attempts --------------------------------------------------------
    def commit_node(self, t, n, parent_sha, result=None):
        p = t.nodes[n.parent] if n.parent is not None else None
        score = "null" if n.score is None else f"{n.score:.3f}"
        if p is None:
            subject = f"root({t.arm} r{t.round}): {n.design}"
        else:
            subject = f"attempt({t.arm} r{t.round} n{n.id:03d}): {n.change}"
        body = [f"design: {n.design}"]
        if p is not None:
            rel = ""
            if n.score is not None and p.score is not None:
                rel = f" ({(n.score / p.score - 1) * 100:+.1f}% vs parent)"
            body += [
                f"parent: n{p.id:03d} {p.design} ({'-' if p.score is None else f'{p.score:.2f}'} ns/op)",
                f"result: {n.status} {'' if n.score is None else f'{n.score:.2f} ns/op'}{rel}",
            ]
        trailers = {
            "Arm": t.arm,
            "Round": t.round,
            "Node": n.id,
            "Parent-Node": "" if n.parent is None else n.parent,
            "Expansion": "" if n.k is None else n.k,
            "Design": n.design,
            "Policy": t.policy,
            "Budget": t.budget,
            "Status": n.status,
            "Score": score,
        }
        msg = subject + "\n\n" + "\n".join(body) + "\n\n" + "\n".join(f"Dream-{k}: {v}" for k, v in trailers.items()) + "\n"
        attempt = {"design": n.design, "arm": t.arm, "round": t.round, "node": n.id, "parent_node": n.parent, "expansion": n.k, "change": n.change}
        sha = self.commit({"attempt.json": json.dumps(attempt, indent=1) + "\n"}, parent_sha, msg)
        self.set_ref(f"refs/dream/{t.arm}/r{t.round:02d}/n{n.id:03d}", sha)
        if result is not None:
            self.note(sha, result)
        n.sha = sha
        return sha

    def load_tree(self, arm, rnd):
        """Rebuild a discovery tree purely from git (refs + commit trailers)."""
        refs = self.run("for-each-ref", "--format=%(objectname)", f"refs/dream/{arm}/r{rnd:02d}/").split()
        if not refs:
            return None
        out = self.run("log", "--no-walk=unsorted", "--stdin", "--format=%H%x1f%B%x1e", input="\n".join(refs) + "\n")
        nodes = []
        for rec in out.split("\x1e"):
            if not rec.strip():
                continue
            sha, body = rec.strip().split("\x1f", 1)
            tr = dict(TRAILER.findall(body))
            nodes.append(
                Node(
                    id=int(tr["Node"]),
                    parent=int(tr["Parent-Node"]) if tr["Parent-Node"] else None,
                    k=int(tr["Expansion"]) if tr["Expansion"] else None,
                    design=tr["Design"],
                    score=None if tr["Score"] == "null" else float(tr["Score"]),
                    status=tr["Status"],
                    change=body.split("\n", 1)[0].split(": ", 1)[-1],
                    sha=sha,
                )
            )
            meta = tr
        nodes.sort(key=lambda n: n.id)
        t = Tree(arm=arm, round=rnd, budget=int(meta["Budget"]), policy=meta["Policy"])
        for n in nodes:
            if n.parent is not None:
                nodes[n.parent].n_exp += 1
            if not t.nodes:
                t.nodes.append(n)
                t.best_trace.append(n.score)
            else:
                t.add(n)
        return t

    def rounds(self, arm):
        out = self.run("for-each-ref", "--format=%(refname)", f"refs/dream/{arm}/")
        return sorted({int(m) for m in re.findall(rf"refs/dream/{arm}/r(\d+)/", out)})

    # ---- policy versions -------------------------------------------------
    def commit_policy(self, arm, rnd, spec, info, code_path):
        branch = f"refs/heads/dream/{arm}/policy"
        parent = self.run("rev-parse", "-q", "--verify", branch, check=False) or None
        doc = {"arm": arm, "round": rnd, "policy": spec, **info}
        files = {"policy.json": json.dumps(doc, indent=1) + "\n", "policies.py": Path(code_path).read_text()}
        msg = f"policy({arm} r{rnd:02d}): {info['repr']}\n\n{info.get('why', '')}\n\nDream-Arm: {arm}\nDream-Round: {rnd}\nDream-Policy: {info['repr']}\n"
        sha = self.commit(files, parent, msg, dream_files=False)
        self.set_ref(branch, sha)
        self.run("tag", "-f", f"dream/{arm}/policy-r{rnd:02d}", sha)
        return sha


def repo_root():
    return Path(__file__).resolve().parent.parent


os.environ.setdefault("GIT_TERMINAL_PROMPT", "0")
