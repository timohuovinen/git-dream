//! Running a policy against a world, and dreaming (offline policy improvement).
//!
//! A *world* answers one question: "the policy continues node `parent` for the
//! k-th time: what attempt results?" ([`World::step`] returns the child node).
//!
//! * `LiveWorld` (engine.rs): really runs the agent + evaluator and commits.
//! * [`ReplayWorld`]: answers only from recorded history. The recorded
//!   (parent attempt, k) edge is always used when it exists, so the deployed
//!   policy replays its own tree exactly. Off the recorded path:
//!   - `strict`: nothing is known: MISS (the only safe choice for stochastic agents);
//!   - `cache`: deterministic agents only: the agent's proposal is computable,
//!     and any earlier measurement of the same design is reused (optionally
//!     rescaled by the replayed tree's root to absorb machine drift).
//!
//!   A MISS is charged as a call and is a dead end, so a dreamt policy can
//!   never profit from leaving the region history actually explored.

use crate::agents::SpaceAgent;
use crate::policies::{Family, Policy, Registry};
use crate::rng::Rng;
use crate::tree::{Goal, Node, Tree, MISS, OK};
use anyhow::Result;
use std::collections::HashMap;

pub trait World {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Cache,
    Strict,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode> {
        match s {
            "cache" => Ok(Mode::Cache),
            "strict" => Ok(Mode::Strict),
            _ => anyhow::bail!("replay mode must be \"cache\" or \"strict\", got {s:?}"),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Cache => "cache",
            Mode::Strict => "strict",
        }
    }
}

pub struct ReplayWorld<'a> {
    agent: Option<&'a SpaceAgent>,
    pub mode: Mode,
    normalize_root: bool,
    scores: HashMap<String, f64>,
    failed: HashMap<String, String>,
    pub misses: usize,
    rec: Option<&'a Tree>,
    edges: HashMap<(usize, usize), usize>,
}

impl<'a> ReplayWorld<'a> {
    pub fn new(history: &[Tree], agent: Option<&'a SpaceAgent>, mode: Mode, normalize_root: bool) -> ReplayWorld<'a> {
        let mode = if agent.is_some() { mode } else { Mode::Strict };
        let mut acc: HashMap<String, (f64, usize)> = HashMap::new();
        let mut failed = HashMap::new();
        for t in history {
            let rs = t.root().score;
            for n in &t.nodes {
                match n.score {
                    Some(s) => {
                        let v = if normalize_root {
                            match rs {
                                Some(r) if r != 0.0 => s / r,
                                _ => continue,
                            }
                        } else {
                            s
                        };
                        let e = acc.entry(n.design.clone()).or_insert((0.0, 0));
                        e.0 += v;
                        e.1 += 1;
                    }
                    None if n.status != MISS => {
                        failed.insert(n.design.clone(), n.status.clone());
                    }
                    None => {}
                }
            }
        }
        let scores = acc.into_iter().map(|(d, (s, c))| (d, s / c as f64)).collect();
        ReplayWorld { agent, mode, normalize_root, scores, failed, misses: 0, rec: None, edges: HashMap::new() }
    }

    pub fn bind(&mut self, rec: &'a Tree) -> &mut Self {
        self.edges = rec.edges();
        self.rec = Some(rec);
        self
    }
}

impl World for ReplayWorld<'_> {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
        let rec = self.rec.expect("ReplayWorld::bind not called");
        let p = &t.nodes[parent];
        let nid = t.nodes.len();
        if let Some(pr) = p.rec {
            if let Some(&i) = self.edges.get(&(pr, k)) {
                let r = &rec.nodes[i];
                let mut n = Node::new(nid, Some(parent), Some(k), r.design.clone(), r.score, &r.status, r.change.clone());
                n.sha = r.sha.clone();
                n.rec = Some(r.id);
                return Ok(n);
            }
        }
        let proposal = self.agent.map(|a| a.propose(&p.design, k));
        if self.mode == Mode::Strict || proposal.is_none() {
            self.misses += 1;
            let design = proposal.map_or_else(|| format!("?{}/{k}", &p.design[..p.design.len().min(12)]), |x| x.0);
            return Ok(Node::new(nid, Some(parent), Some(k), design, None, MISS, ""));
        }
        let (design, change) = proposal.unwrap();
        if let Some(&s) = self.scores.get(&design) {
            let s = if self.normalize_root { s * rec.root().score.unwrap_or(1.0) } else { s };
            return Ok(Node::new(nid, Some(parent), Some(k), design, Some(s), OK, change));
        }
        if let Some(st) = self.failed.get(&design) {
            return Ok(Node::new(nid, Some(parent), Some(k), design, None, &st.clone(), change));
        }
        self.misses += 1;
        Ok(Node::new(nid, Some(parent), Some(k), design, None, MISS, change))
    }
}

/// Run `policy` from `root` until it stops or has spent `budget` calls.
#[allow(clippy::too_many_arguments)]
pub fn run_tree(policy: &mut Policy, world: &mut dyn World, root: &Node, budget: usize, run: &str, round: u32, goal: Goal, seed: u64, on_node: &mut dyn FnMut(&Tree, &Node)) -> Result<Tree> {
    let mut t = Tree::new(run, round, budget, goal, &policy.repr());
    let mut r = root.clone();
    r.id = 0;
    r.parent = None;
    r.k = None;
    r.n_exp = 0;
    r.rec = Some(0);
    t.set_root(r);
    policy.reset(seed);
    while t.calls() < budget {
        let Some(nid) = policy.select(&t) else { break };
        let k = t.nodes[nid].n_exp;
        t.nodes[nid].n_exp += 1;
        let n = world.step(&t, nid, k)?;
        on_node(&t, &n);
        t.add(n);
    }
    Ok(t)
}

pub fn gain(t: &Tree) -> f64 {
    match (t.best().and_then(|b| b.score), t.root().score) {
        (Some(b), Some(r)) => t.rel(b, r),
        _ => 0.0,
    }
}

#[derive(Clone, Debug)]
pub struct ReplayResult {
    /// J = mean over trees of [relative improvement found - lam * fraction of budget spent].
    pub j: f64,
    pub gain: f64,
    pub calls: usize,
    pub miss_rate: f64,
    pub trees: Vec<Tree>,
}

pub fn objective(trees: &[Tree], budget: usize, lam: f64) -> f64 {
    if trees.is_empty() {
        return 0.0;
    }
    trees.iter().map(|t| gain(t) - lam * t.calls() as f64 / budget as f64).sum::<f64>() / trees.len() as f64
}

#[allow(clippy::too_many_arguments)]
pub fn replay(policy: &Policy, history: &[Tree], budget: usize, lam: f64, agent: Option<&SpaceAgent>, mode: Mode, normalize_root: bool, keep_trees: bool) -> Result<ReplayResult> {
    let mut w = ReplayWorld::new(history, agent, mode, normalize_root);
    let mut trees = vec![];
    for rt in history.iter().filter(|t| t.root().score.is_some()) {
        let mut p = policy.clone();
        w.bind(rt);
        trees.push(run_tree(&mut p, &mut w, rt.root(), budget, &rt.run, rt.round, rt.goal, rt.round as u64, &mut |_, _| {})?);
    }
    let calls: usize = trees.iter().map(Tree::calls).sum();
    let j = objective(&trees, budget, lam);
    let g = if trees.is_empty() { 0.0 } else { trees.iter().map(gain).sum::<f64>() / trees.len() as f64 };
    Ok(ReplayResult { j, gain: g, calls, miss_rate: if calls > 0 { w.misses as f64 / calls as f64 } else { 0.0 }, trees: if keep_trees { trees } else { vec![] } })
}

pub struct DreamArgs<'a> {
    pub history: &'a [Tree],
    pub budget: usize,
    pub lam: f64,
    pub candidates: usize,
    pub seed: u64,
    pub agent: Option<&'a SpaceAgent>,
    pub mode: Mode,
    pub normalize_root: bool,
    pub registry: &'a Registry,
    pub families: &'a [&'a Family],
}

/// Score the deployed policy, `extra` policies and sampled ones by replay over
/// the whole history; return the ranking, best first. The deployed policy
/// (index 0 before sorting) wins ties, so the selection is never worse than
/// the current policy on replay.
pub fn dream(current: &Policy, extra: &[Policy], a: &DreamArgs) -> Result<Vec<(Policy, ReplayResult)>> {
    let mut rng = Rng::new(a.seed);
    let mut cands: Vec<Policy> = vec![current.clone()];
    cands.extend(extra.iter().cloned());
    if !a.families.is_empty() {
        for _ in 0..a.candidates {
            cands.push(a.registry.sample(&mut rng, a.families));
        }
    }
    let mut scored = vec![];
    for (i, c) in cands.into_iter().enumerate() {
        let r = replay(&c, a.history, a.budget, a.lam, a.agent, a.mode, a.normalize_root, false)?;
        scored.push((i, c, r));
    }
    scored.sort_by(|x, y| y.2.j.total_cmp(&x.2.j).then(x.0.cmp(&y.0)));
    Ok(scored.into_iter().map(|(_, c, r)| (c, r)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quadratic<'a>(&'a SpaceAgent);

    impl World for Quadratic<'_> {
        fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
            let (d, ch) = self.0.propose(&t.nodes[parent].design, k);
            let q = self.0.indices(&d).unwrap();
            let s = 1.0 + (q[0] as f64 - 7.0).powi(2) + (q[1] as f64 - 3.0).powi(2);
            Ok(Node::new(t.nodes.len(), Some(parent), Some(k), d, Some(s), OK, ch))
        }
    }

    #[test]
    fn exact_replay_and_dream_never_worse() {
        let knobs: toml::Table = toml::from_str("x = [0,1,2,3,4,5,6,7,8,9,10,11]\ny = [0,1,2,3,4,5,6,7,8,9,10,11]").unwrap();
        let start: toml::Table = toml::from_str("x = 0\ny = 0").unwrap();
        let agent = SpaceAgent::new(&knobs, &start, "", "p.json", 0.3, 0.2).unwrap();
        let reg = Registry::builtin();
        let root = Node::new(0, None, None, agent.root(), Some(59.0), OK, "");
        let mut hist = vec![];
        for (i, fam) in ["refine_best", "softmax"].iter().enumerate() {
            let mut p = reg.make(&serde_json::json!({"family": fam, "params": {"T": 0.05}})).unwrap();
            hist.push(run_tree(&mut p, &mut Quadratic(&agent), &root, 15, "r", i as u32 + 1, Goal::Min, i as u64 + 1, &mut |_, _| {}).unwrap());
        }
        // each recorded tree replays exactly under the policy that produced it, even a stochastic one
        for (i, fam) in ["refine_best", "softmax"].iter().enumerate() {
            let p = reg.make(&serde_json::json!({"family": fam, "params": {"T": 0.05}})).unwrap();
            let r = replay(&p, &hist[i..=i], 15, 0.02, Some(&agent), Mode::Strict, false, true).unwrap();
            assert_eq!(r.miss_rate, 0.0);
            let key = |t: &Tree| t.nodes.iter().map(|n| (n.parent, n.k, n.design.clone(), n.score)).collect::<Vec<_>>();
            assert_eq!(key(&r.trees[0]), key(&hist[i]));
        }
        let cur = reg.make(&serde_json::json!({"family": "refine_best"})).unwrap();
        let fams = reg.searchable(&[]);
        let a = DreamArgs { history: &hist, budget: 15, lam: 0.02, candidates: 60, seed: 0, agent: Some(&agent), mode: Mode::Cache, normalize_root: false, registry: &reg, families: &fams };
        let ranking = dream(&cur, &[], &a).unwrap();
        let jcur = ranking.iter().find(|(c, _)| c.repr() == cur.repr()).unwrap().1.j;
        assert!(ranking[0].1.j >= jcur);
    }
}
