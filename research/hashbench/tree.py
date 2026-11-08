"""Discovery tree: what a round of exploration produces and what policies see."""

from dataclasses import dataclass, field


@dataclass
class Node:
    id: int
    parent: int | None  # parent node id (None for the root)
    k: int | None  # this node is the k-th expansion of its parent
    design: str
    score: float | None  # ns/op, lower is better; None = invalid / unknown
    status: str = "ok"  # ok | full | miss (replay: outcome not in history)
    change: str = ""
    n_exp: int = 0  # how many times this node has been continued
    sha: str = ""  # git commit of this attempt (live / loaded trees)
    rec: int | None = None  # replay: id of the matching recorded node (strict mode)


@dataclass
class Tree:
    arm: str
    round: int
    budget: int
    nodes: list = field(default_factory=list)
    policy: str = ""
    best_trace: list = field(default_factory=list)  # best score after each call (index 0 = root)

    @property
    def calls(self):
        return len(self.nodes) - 1

    @property
    def root(self):
        return self.nodes[0]

    def valid(self):
        return [n for n in self.nodes if n.score is not None]

    def best(self):
        v = self.valid()
        return min(v, key=lambda n: n.score) if v else None

    def add(self, node):
        self.nodes.append(node)
        b = self.best()
        self.best_trace.append(b.score if b else None)

    def stalled(self, eps):
        """Calls since the best score last improved by a relative margin > eps."""
        tr = self.best_trace
        ref, since = tr[0], 0
        for s in tr[1:]:
            if s is not None and (ref is None or s < ref * (1 - eps)):
                ref, since = s, 0
            else:
                since += 1
        return since

    def edges(self):
        return {(n.parent, n.k): n for n in self.nodes if n.parent is not None}
