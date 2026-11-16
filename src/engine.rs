//! Configuration, the live world (real agent + evaluator + git commits) and the
//! round loop: `[dream ->] explore -> promote`, repeated.

use crate::agents::{excerpt, failure, note_output, sh, Agent, CommandAgent, EvalResult, Evaluator, SpaceAgent};
use crate::memory::{check_run, Memory};
use crate::model;
use crate::policies::{Param, Policy, Registry};
use crate::rng::hash;
use crate::sim::{self, DreamArgs, Mode, ReplayResult};
use crate::tree::{Goal, Node, Tree, FAIL, NOOP, OK, PENDING};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub const CONFIG_NAME: &str = ".git-dream.toml";

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub run: String,
    pub agent: AgentCfg,
    pub evaluate: EvalCfg,
    pub explore: ExploreCfg,
    pub dream: DreamCfg,
    pub policies: PoliciesCfg,
    pub storage: StorageCfg,
}

impl Default for Config {
    fn default() -> Self {
        Config { run: "main".into(), agent: AgentCfg::default(), evaluate: EvalCfg::default(), explore: ExploreCfg::default(), dream: DreamCfg::default(), policies: PoliciesCfg::default(), storage: StorageCfg::default() }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCfg {
    pub kind: String,
    // space
    pub knobs: toml::Table,
    pub start: toml::Table,
    pub salt: String,
    pub file: String,
    pub p_two: f64,
    pub p_far: f64,
    // command
    pub command: String,
    pub prompt: String,
    pub timeout: f64,
}

impl Default for AgentCfg {
    fn default() -> Self {
        AgentCfg { kind: "space".into(), knobs: toml::Table::new(), start: toml::Table::new(), salt: String::new(), file: ".dream/params.json".into(), p_two: 0.3, p_far: 0.2, command: String::new(), prompt: String::new(), timeout: 1800.0 }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EvalCfg {
    /// Paths a command agent may not change (evaluator, tests, fixtures).
    pub protected: Vec<String>,
    pub command: String,
    pub goal: Goal,
    pub timeout: f64,
    /// Never re-measure a design already measured in this run.
    pub reuse: bool,
    /// Re-measure each round's root (noisy benchmarks, machine drift).
    pub remeasure_root: bool,
    /// Re-measure the top-k designs `confirm_reps` times before promoting.
    pub confirm_top: usize,
    pub confirm_reps: usize,
}

impl Default for EvalCfg {
    fn default() -> Self {
        EvalCfg { protected: vec![], command: String::new(), goal: Goal::Min, timeout: 600.0, reuse: false, remeasure_root: false, confirm_top: 0, confirm_reps: 1 }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ExploreCfg {
    pub budget: usize,
    pub min_calls: usize,
    /// `git dream run` adds `budget` attempts to the current round's tree
    /// instead of starting a new round from its best attempt. A new round keeps
    /// only the promoted attempt; an extended round keeps every attempt
    /// continuable (runners-up, failed attempts worth repairing).
    pub extend: bool,
    /// Attempts in flight at once, each in its own worktree. The policy picks
    /// the next attempt without the results of those still running.
    pub parallel: usize,
    /// What may run while an evaluation runs. "alone": nothing (a score that
    /// depends on machine load: agents work at once, then each result is
    /// measured by itself). "serial": no other evaluation, but agents may be
    /// working. "parallel": anything (a score that does not depend on load).
    pub evaluations: String,
}

impl Default for ExploreCfg {
    fn default() -> Self {
        ExploreCfg { budget: 20, min_calls: 0, extend: false, parallel: 1, evaluations: "alone".into() }
    }
}

/// Keeps evaluations as isolated as `[explore] evaluations` asks: agents hold
/// the lock shared, evaluations hold it exclusively.
struct Gate {
    alone: bool,
    serial: bool,
    lock: RwLock<()>,
}

impl Gate {
    fn new(evaluations: &str) -> Gate {
        Gate { alone: evaluations == "alone", serial: evaluations != "parallel", lock: RwLock::new(()) }
    }
    fn agent(&self) -> Option<RwLockReadGuard<'_, ()>> {
        self.alone.then(|| self.lock.read().unwrap_or_else(|e| e.into_inner()))
    }
    fn evaluation(&self) -> Option<RwLockWriteGuard<'_, ()>> {
        self.serial.then(|| self.lock.write().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A finished attempt, not yet committed: the node, its git tree and its note.
type Outcome = (Node, String, Value);

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct DreamCfg {
    /// Last N rounds per run reserved for a diagnostic deployment gate (0 disables).
    pub holdout: usize,
    pub lam: f64,
    pub candidates: usize,
    pub mode: String,
    /// "none" or "root": cached scores relative to each tree's root.
    pub normalize: String,
    /// "run" (this run's rounds) or "all" (every run in the repository).
    pub history: String,
    /// "auto": a `bandit` policy is refitted from measured outcomes (the yield
    /// model), any other family is tuned by replay. "replay": always replay.
    pub method: String,
}

impl Default for DreamCfg {
    fn default() -> Self {
        DreamCfg { holdout: 1, lam: 0.02, candidates: 300, mode: "cache".into(), normalize: "none".into(), history: "run".into(), method: "auto".into() }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct PoliciesCfg {
    pub initial: toml::Table,
    pub families: Vec<String>,
    /// name -> { command, grid, doc }
    pub external: toml::Table,
}

impl Default for PoliciesCfg {
    fn default() -> Self {
        let mut initial = toml::Table::new();
        initial.insert("family".into(), toml::Value::String(crate::model::BANDIT.into()));
        PoliciesCfg { initial, families: vec![], external: toml::Table::new() }
    }
}

/// Attempts are written with plumbing (hash-object, commit-tree, update-ref),
/// which never triggers `git gc --auto`: every version of a file stays a full
/// loose object until something packs it.
#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct StorageCfg {
    /// After each round, pack its loose objects into one pack (`git repack -d`),
    /// so near-identical attempts are stored as deltas.
    pub repack: bool,
    /// Every N rounds also run `git gc` to merge the per-round packs (0 = never).
    pub gc_every: u32,
}

impl Default for StorageCfg {
    fn default() -> Self {
        StorageCfg { repack: true, gc_every: 10 }
    }
}

pub fn load_config(path: &Path) -> Result<(Config, String)> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("{} not found: run `git dream init` first", path.display()))?;
    let cfg: Config = toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    Ok((cfg, raw))
}

fn toml_to_json(v: &toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// design -> (score, status, sha) measured in this run (evaluate.reuse).
type Seen = HashMap<String, (Option<f64>, String, String)>;

pub struct Engine {
    pub mem: Memory,
    pub cfg: Config,
    raw_cfg: String,
    pub run: String,
    pub agent: Agent,
    pub registry: Registry,
    pub goal: Goal,
    pub quiet: bool,
    seen: Mutex<Seen>,
    external_files: Vec<(String, String)>,
}

impl Engine {
    pub fn open(cwd: &Path, config: Option<&Path>, run: Option<&str>) -> Result<Engine> {
        let mem = Memory::open(cwd)?;
        let path = config.map(PathBuf::from).unwrap_or_else(|| mem.top.join(CONFIG_NAME));
        let (cfg, raw) = load_config(&path)?;
        let run = run.map(String::from).unwrap_or_else(|| cfg.run.clone());
        check_run(&run)?;
        let a = &cfg.agent;
        let agent = match a.kind.as_str() {
            "space" => Agent::Space(SpaceAgent::new(&a.knobs, &a.start, &a.salt, &a.file, a.p_two, a.p_far)?),
            "command" => {
                if a.command.is_empty() {
                    bail!("[agent] kind = \"command\" needs `command`");
                }
                Agent::Command(CommandAgent { command: a.command.clone(), prompt: a.prompt.clone(), timeout: a.timeout })
            }
            k => bail!("unknown agent kind {k:?} (space | command)"),
        };
        Mode::parse(&cfg.dream.mode)?;
        anyhow::ensure!(cfg.explore.budget > 0, "explore.budget must be positive");
        anyhow::ensure!(cfg.explore.min_calls <= cfg.explore.budget, "explore.min_calls exceeds budget");
        anyhow::ensure!(cfg.dream.lam.is_finite() && cfg.dream.lam >= 0.0, "dream.lam must be finite and nonnegative");
        anyhow::ensure!(["none", "root"].contains(&cfg.dream.normalize.as_str()), "dream.normalize must be none or root");
        anyhow::ensure!(["run", "all"].contains(&cfg.dream.history.as_str()), "dream.history must be run or all");
        anyhow::ensure!(["auto", "replay"].contains(&cfg.dream.method.as_str()), "dream.method must be auto or replay");
        anyhow::ensure!((1..=64).contains(&cfg.explore.parallel), "explore.parallel must be between 1 and 64");
        anyhow::ensure!(["alone", "serial", "parallel"].contains(&cfg.explore.evaluations.as_str()), "explore.evaluations must be alone, serial or parallel");
        for timeout in [cfg.agent.timeout, cfg.evaluate.timeout] {
            anyhow::ensure!(timeout.is_finite() && timeout > 0.0 && timeout < 1e9, "timeouts must be finite, positive and below 1e9 seconds");
        }
        anyhow::ensure!(cfg.evaluate.confirm_top == 0 || cfg.evaluate.confirm_reps > 0, "confirmation needs confirm_reps > 0");
        let mut registry = Registry::builtin();
        let mut external_files = vec![];
        for (name, v) in &cfg.policies.external {
            let t = v.as_table().ok_or_else(|| anyhow!("[policies.external.{name}] must be a table"))?;
            let command = t.get("command").and_then(|c| c.as_str()).ok_or_else(|| anyhow!("[policies.external.{name}] needs `command`"))?;
            let mut grid = vec![];
            if let Some(g) = t.get("grid").and_then(|g| g.as_table()) {
                for (k, vals) in g {
                    let vals = vals.as_array().ok_or_else(|| anyhow!("grid {name}.{k} must be an array"))?;
                    anyhow::ensure!(!vals.is_empty(), "grid {name}.{k} must not be empty");
                    grid.push((k.clone(), vals.iter().map(|x| Param::from_json(&toml_to_json(x))).collect::<Result<Vec<_>>>()?));
                }
            }
            let doc = t.get("doc").and_then(|d| d.as_str()).unwrap_or("external policy");
            // run relative commands from the repository root
            registry.add_external_with_timeout(name, &format!("cd {} && {command}", shell_quote(&mem.top.to_string_lossy())), grid, doc, t.get("timeout").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64))).unwrap_or(10.0))?;
            if let Some(src) = t.get("source").and_then(|s| s.as_str()) {
                if let Ok(text) = std::fs::read_to_string(mem.top.join(src)) {
                    external_files.push((Path::new(src).file_name().unwrap().to_string_lossy().into_owned(), text));
                }
            }
        }
        let goal = cfg.evaluate.goal;
        let mut seen = HashMap::new();
        if cfg.evaluate.reuse {
            for t in mem.history(&run)? {
                for n in t.nodes {
                    if n.status != NOOP {
                        seen.entry(n.design).or_insert((n.score, n.status, n.sha));
                    }
                }
            }
        }
        Ok(Engine { mem, cfg, raw_cfg: raw, run, agent, registry, goal, quiet: false, seen: Mutex::new(seen), external_files })
    }

    fn log(&self, s: &str) {
        if !self.quiet {
            println!("{s}");
        }
    }

    fn evaluator(&self) -> Result<Evaluator> {
        if self.cfg.evaluate.command.is_empty() {
            bail!("[evaluate] needs `command`");
        }
        Ok(Evaluator { command: self.cfg.evaluate.command.clone(), timeout: self.cfg.evaluate.timeout })
    }

    pub fn base(&self) -> Result<String> {
        self.mem.base(&self.run)
    }

    /// The scratch worktree of `lane` (attempts running at once each have one).
    pub fn wt(&self, lane: usize) -> Result<PathBuf> {
        self.mem.worktree(&self.run, lane)
    }

    /// How many attempts are in flight at once. A parameter-space agent proposes
    /// instantly, so its attempts only overlap if evaluations may.
    pub fn lanes(&self) -> usize {
        let x = &self.cfg.explore;
        if self.agent.space().is_some() && x.evaluations != "parallel" { 1 } else { x.parallel.max(1) }
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn mode(&self) -> Mode {
        Mode::parse(&self.cfg.dream.mode).unwrap_or(Mode::Cache)
    }

    fn normalize_root(&self) -> bool {
        self.cfg.dream.normalize == "root"
    }

    pub fn evaluate(&self, tree: &str, design: &str, round: u32, nid: usize, reuse: bool) -> Result<EvalResult> {
        self.evaluate_in(0, tree, design, round, nid, reuse)
    }

    /// Evaluate `tree` in the worktree of `lane`.
    pub fn evaluate_in(&self, lane: usize, tree: &str, design: &str, round: u32, nid: usize, reuse: bool) -> Result<EvalResult> {
        if reuse && self.cfg.evaluate.reuse {
            if let Some((s, st, sha)) = self.seen().get(design) {
                return Ok(EvalResult { score: *s, status: st.clone(), note: json!({"score": s, "status": st, "reused": sha}) });
            }
        }
        let wt = self.wt(lane)?;
        self.mem.materialize(&wt, tree)?;
        let mut env = vec![
            ("DREAM_RUN".to_string(), self.run.clone()),
            ("DREAM_ROUND".into(), round.to_string()),
            ("DREAM_NODE".into(), nid.to_string()),
            ("DREAM_DESIGN".into(), design.to_string()),
            ("DREAM_WORKTREE".into(), wt.to_string_lossy().into_owned()),
            ("DREAM_LANE".into(), lane.to_string()),
        ];
        env.extend(self.agent.env(design));
        self.evaluator()?.run(&wt, &env)
    }

    pub fn initial_policy(&self) -> Result<Policy> {
        let mut p = self.registry.make(&toml_to_json(&toml::Value::Table(self.cfg.policies.initial.clone())))?;
        p.set_min_calls(self.cfg.explore.min_calls);
        Ok(p)
    }

    pub fn deployed(&self, round: Option<u32>) -> Result<Option<Policy>> {
        match self.mem.policy_doc(&self.run, round) {
            Some(doc) => {
                let mut p = self.registry.make(&doc["policy"])?;
                p.set_min_calls(self.cfg.explore.min_calls);
                Ok(Some(p))
            },
            None => Ok(None),
        }
    }

    pub fn current_policy(&self) -> Result<Policy> {
        Ok(match self.deployed(None)? {
            Some(p) => p,
            None => self.initial_policy()?,
        })
    }

    fn policy_files(&self) -> Vec<(String, String)> {
        let mut f = vec![("git-dream.toml".to_string(), self.raw_cfg.clone())];
        f.extend(self.external_files.iter().cloned());
        f
    }

    pub fn history(&self) -> Result<Vec<Tree>> {
        if self.cfg.dream.history == "all" {
            let mut v = vec![];
            for r in self.mem.runs()? {
                v.extend(self.mem.history(&r)?);
            }
            Ok(v)
        } else {
            self.mem.history(&self.run)
        }
    }

    /// A design key alone does not identify its objective or proposal operator.
    /// Reject pooling incompatible experiments instead of silently reusing scores.
    pub fn replay_history(&self) -> Result<Vec<Tree>> {
        let hist = self.history()?;
        let base = self.mem.rev(&format!("refs/dream/{}/base", self.run)).or_else(|| self.mem.rev("HEAD^{commit}"));
        for t in &hist {
            let path = format!("refs/tags/dream/{}/policy-r{:02}:git-dream.toml", t.run, t.round);
            let raw = self.mem.git(&["show", &path]).with_context(|| format!("cannot verify experiment configuration for {} r{}", t.run, t.round))?;
            let recorded: Config = toml::from_str(&raw)?;
            anyhow::ensure!(recorded.agent == self.cfg.agent && recorded.evaluate == self.cfg.evaluate,
                "incompatible agent/evaluator configuration in {} r{}; use a separate run and history = run", t.run, t.round);
            anyhow::ensure!(self.mem.rev(&format!("refs/dream/{}/base", t.run)) == base,
                "cannot replay runs with different base commits ({})", t.run);
        }
        Ok(hist)
    }

    pub fn next_round(&self) -> Result<u32> {
        Ok(self.mem.rounds(&self.run)?.last().map_or(1, |r| r + 1))
    }

    // ---- dreaming ------------------------------------------------------------------
    /// Replay the deployed policy and sampled alternatives over recorded history
    /// (zero new evaluations) and, if `commit`, deploy the best for `round`.
    pub fn dream(&self, round: Option<u32>, candidates: Option<usize>, mode: Option<Mode>, budget: Option<usize>, commit: bool) -> Result<Vec<(Policy, ReplayResult)>> {
        let round = match round {
            Some(r) => r,
            None => self.next_round()?,
        };
        let hist = self.replay_history()?;
        let current = self.current_policy()?;
        if hist.is_empty() {
            self.log(&format!("dream: no history yet, keeping {}", current.repr()));
            return Ok(vec![]);
        }
        if self.cfg.dream.method != "replay" {
            if let Some((best, y)) = model::learn(&current, &hist) {
                return self.dream_model(round, &hist, &current, &best, &y, commit);
            }
        }
        let budget = budget.unwrap_or(self.cfg.explore.budget);
        let mode = mode.unwrap_or(self.mode());
        let eff = if self.agent.space().is_some() { mode } else { Mode::Strict };
        let partition = if self.cfg.dream.holdout > 0 { sim::split_history(&hist, self.cfg.dream.holdout).ok() } else { None };
        let training = partition.as_ref().map_or(hist.as_slice(), |(train, _)| train.as_slice());
        let fams = self.registry.searchable(&self.cfg.policies.families);
        let a = DreamArgs {
            history: training,
            budget,
            lam: self.cfg.dream.lam,
            candidates: candidates.unwrap_or(self.cfg.dream.candidates),
            min_calls: self.cfg.explore.min_calls,
            seed: hash(&format!("{}|{round}", self.run)),
            agent: self.agent.space(),
            mode: eff,
            normalize_root: self.normalize_root(),
            registry: &self.registry,
            families: &fams,
            threads: crate::par::threads(),
        };
        let ranking = sim::dream(&current, &[], &a)?;
        let (candidate, top) = (&ranking[0].0, &ranking[0].1);
        let (accepted, validation) = if let Some((_, test)) = &partition {
            let replay = |p| sim::replay(p, test, budget, self.cfg.dream.lam, self.agent.space(), eff, self.normalize_root(), false);
            let proposed = replay(candidate)?;
            let incumbent = replay(&current)?;
            (proposed.j > incumbent.j && proposed.miss_rate <= incumbent.miss_rate, json!({
                "candidate_J": proposed.j, "current_J": incumbent.j,
                "candidate_miss_rate": proposed.miss_rate, "current_miss_rate": incumbent.miss_rate,
                "rounds": test.iter().map(|t| json!({"run": t.run, "round": t.round})).collect::<Vec<_>>(),
                "caveat": "diagnostic gate only; repeated use and behavior-policy selection prevent an unbiased generalization estimate"
            }))
        } else { (self.cfg.dream.holdout == 0, json!({"reason": "held-out gate disabled or insufficient history"})) };
        let best = if accepted { candidate } else { &current };
        self.log(&format!("held-out deployment gate: {} ({validation})", if accepted { "accepted" } else { "keeping incumbent" }));
        let cur = &ranking.iter().find(|(c, _)| c.repr() == current.repr()).unwrap().1;
        self.log(&format!("dream r{round}: {} policies over {} trees ({}); current J={:.4}, best J={:.4} -> {}", ranking.len(), hist.len(), eff.as_str(), cur.j, top.j, best.repr()));
        if commit {
            let doc = json!({
                "run": self.run, "round": round, "policy": best.spec(), "repr": best.repr(),
                "validation": validation, "accepted": accepted, "candidate": candidate.spec(),
                "why": format!("dreamt over {} recorded trees, {} candidate policies, {} replay", hist.len(), ranking.len(), eff.as_str()),
                "dream": {"J": if accepted { top.j } else { cur.j }, "candidate_J": top.j, "gain": if accepted { top.gain } else { cur.gain }, "calls": if accepted { top.calls } else { cur.calls }, "miss_rate": if accepted { top.miss_rate } else { cur.miss_rate }, "J_current": cur.j, "current": current.repr(),
                          "mode": eff.as_str(), "lam": self.cfg.dream.lam, "budget": budget, "trees": hist.len(), "candidates": ranking.len()},
                "top": ranking.iter().take(10).map(|(c, r)| json!({"policy": c.repr(), "spec": c.spec(), "J": r.j, "gain": r.gain, "calls": r.calls, "miss_rate": r.miss_rate})).collect::<Vec<_>>(),
            });
            self.mem.commit_policy(&self.run, round, &doc, &self.policy_files())?;
        }
        Ok(ranking)
    }

    /// Dreaming for a `bandit` policy: no candidate is scored on outcomes that
    /// were never observed. The policy for `round` carries the yield of each
    /// kind of move, measured over every recorded attempt.
    fn dream_model(&self, round: u32, hist: &[Tree], current: &Policy, best: &Policy, y: &model::Yield, commit: bool) -> Result<Vec<(Policy, ReplayResult)>> {
        let m = current.p("m", model::STRENGTH);
        let attempts: usize = hist.iter().map(Tree::calls).sum();
        self.log(&format!("dream r{round}: yield model over {attempts} recorded attempts in {} rounds (weight halves every {} attempts)", hist.len(), fs(Some(current.p("halflife", model::HALFLIFE)))));
        let mut table = vec![];
        for b in 0..model::NB {
            if y.n[b] <= 0.0 {
                continue;
            }
            let (a, c) = y.posterior(b, m);
            let (lo, hi) = model::interval(a, c, 0.9);
            self.log(&format!("  {:6} {:52} weight {:6.1}  new best {:5.1}%  (90% interval {:.1}-{:.1}%, prior {:.1}%)", model::name(b), model::describe(b), y.n[b], 100.0 * a / (a + c), 100.0 * lo, 100.0 * hi, 100.0 * model::prior(b)));
            table.push(json!({"move": model::name(b), "what": model::describe(b), "weight": y.n[b], "credit": y.s[b], "yield": a / (a + c), "interval90": [lo, hi], "prior": model::prior(b)}));
        }
        if commit {
            let doc = json!({
                "run": self.run, "round": round, "policy": best.spec(), "repr": best.repr(), "method": "model",
                "why": format!("yield model fitted over {attempts} recorded attempts in {} rounds: measured outcomes only, no replay of unobserved decisions", hist.len()),
                "model": table,
            });
            self.mem.commit_policy(&self.run, round, &doc, &self.policy_files())?;
        }
        Ok(vec![])
    }

    pub fn replay(&self, policy: &Policy, mode: Option<Mode>, budget: Option<usize>) -> Result<ReplayResult> {
        let hist = self.replay_history()?;
        sim::replay(policy, &hist, budget.unwrap_or(self.cfg.explore.budget), self.cfg.dream.lam, self.agent.space(), mode.unwrap_or(self.mode()), self.normalize_root(), true)
    }

    // ---- exploring -------------------------------------------------------------------
    fn policy_for(&self, round: u32) -> Result<Policy> {
        if let Some(p) = self.deployed(Some(round))? {
            return Ok(p);
        }
        let p = self.current_policy()?;
        let why = if round == 1 { "initial policy" } else { "carried over (no dream before this round)" };
        self.mem.commit_policy(&self.run, round, &json!({"run": self.run, "round": round, "policy": p.spec(), "repr": p.repr(), "why": why}), &self.policy_files())?;
        Ok(p)
    }

    fn params_file(&self, s: &SpaceAgent, design: &str) -> Vec<(String, String)> {
        vec![(s.file.clone(), serde_json::to_string_pretty(&s.params_json(design)).unwrap() + "\n")]
    }

    fn make_root(&self, round: u32, budget: usize, policy: &Policy) -> Result<Node> {
        let base = self.base()?;
        let mut t = Tree::new(&self.run, round, budget, self.goal, &policy.repr());
        let (parent, tree, design, change, res) = if round == 1 {
            let (tree, design) = match &self.agent {
                Agent::Space(s) => (self.mem.tree_with_files(Some(&base), &self.params_file(s, &s.root()))?, s.root()),
                Agent::Command(_) => {
                    let tr = self.mem.tree_of(&base)?;
                    (tr.clone(), tr)
                }
            };
            (base, tree, design, "base".to_string(), None)
        } else {
            let prev_ref = format!("refs/dream/{}/r{:02}/best", self.run, round - 1);
            let parent = self.mem.rev(&prev_ref).ok_or_else(|| anyhow!("round {} has no promoted attempt", round - 1))?;
            let prev = self.mem.load_tree(&self.run, round - 1)?.ok_or_else(|| anyhow!("round {} not found", round - 1))?;
            let pn = prev.nodes.iter().find(|n| n.sha == parent).ok_or_else(|| anyhow!("promoted attempt of round {} not in its tree", round - 1))?.clone();
            let score = self.mem.read_note(&parent).and_then(|n| n["confirmation"]["score"].as_f64()).or(pn.score);
            let res = if self.cfg.evaluate.remeasure_root { None } else { Some(EvalResult { score, status: pn.status.clone(), note: json!({"score": score, "status": pn.status, "copied_from": parent}) }) };
            (parent.clone(), self.mem.tree_of(&parent)?, pn.design.clone(), format!("from r{:02} n{:03}", round - 1, pn.id), res)
        };
        let res = match res {
            Some(r) => r,
            None => self.evaluate(&tree, &design, round, 0, !self.cfg.evaluate.remeasure_root)?,
        };
        let mut root = Node::new(0, None, None, design, res.score, &res.status, change);
        t.set_root(root.clone());
        root.sha = self.mem.commit_attempt(&t, &root, &tree, &parent, Some(&res.note))?;
        if root.score.is_none() {
            bail!("the root of round {round} did not evaluate ({}):\n{}", root.status, note_output(&res.note));
        }
        self.seen().insert(root.design.clone(), (root.score, root.status.clone(), root.sha.clone()));
        Ok(root)
    }

    pub fn explore(&self, round: Option<u32>, budget: Option<usize>) -> Result<Tree> {
        let round = match round {
            Some(r) => r,
            None => {
                self.resume()?;
                self.next_round()?
            }
        };
        if round > 1 { self.replay_history()?; }
        let budget = budget.unwrap_or(self.cfg.explore.budget);
        anyhow::ensure!(budget > 0 && budget >= self.cfg.explore.min_calls, "budget must be positive and at least explore.min_calls");
        let mut policy = self.policy_for(round)?;
        self.log(&format!("explore r{round}: {}, budget {budget}{}", policy.repr(), self.lanes_note()));
        let root = self.make_root(round, budget, &policy)?;
        self.log(&format!("  r{round} n000 root {} {}", short(&root.design, 24), fs(root.score)));
        let mut t = Tree::new(&self.run, round, budget, self.goal, &policy.repr());
        t.set_root(root);
        policy.reset(round as u64);
        self.search(&mut policy, &mut t, VecDeque::new())?;
        let best = self.promote(&t)?;
        self.log(&format!("  r{round}: {} calls, promoted n{:03} {} ({:+.2}% vs root)  {}", t.calls(), best.id, fs(best.score), best.score.zip(t.root().score).map_or(0.0, |(b, r)| t.rel(b, r) * 100.0), &best.sha[..10]));
        self.compact(round);
        Ok(t)
    }

    /// Add `budget` attempts to the latest round's tree. The round's policy is
    /// driven through the recorded attempts first (zero evaluations), so it is
    /// in exactly the state it would be in had the round never stopped, then
    /// continues live. Every recorded attempt stays continuable: nothing is
    /// lost at a round boundary. Also finishes a round that was killed
    /// (attempts that were running when it died are run again).
    pub fn extend(&self, budget: Option<usize>) -> Result<Tree> {
        let Some(&round) = self.mem.rounds(&self.run)?.last() else { return self.explore(Some(1), budget) };
        self.replay_history()?;
        let rec = self.mem.load_tree(&self.run, round)?.ok_or_else(|| anyhow!("round {round} not found"))?;
        let add = budget.unwrap_or(self.cfg.explore.budget);
        anyhow::ensure!(add > 0, "budget must be positive");
        let total = rec.calls() + add;
        let mut policy = self.policy_for(round)?;
        self.log(&format!("extend r{round}: {}, {} recorded attempts + {add}{}", policy.repr(), rec.calls(), self.lanes_note()));
        let recorded = rec.nodes.len();
        let (mut t, unfinished) = self.redrive(&mut policy, &rec, total)?;
        self.search(&mut policy, &mut t, unfinished)?;
        let best = self.promote(&t)?;
        self.log(&format!("  r{round}: {} calls ({} new), promoted n{:03} {} ({:+.2}% vs root)  {}", t.calls(), t.nodes.len() - recorded, best.id, fs(best.score), best.score.zip(t.root().score).map_or(0.0, |(b, r)| t.rel(b, r) * 100.0), &best.sha[..10]));
        self.compact(round);
        Ok(t)
    }

    /// `n` steps of search: new rounds, or extensions of the current round
    /// when `extend` is set (then only `new_round` starts a round, after dreaming).
    #[allow(clippy::too_many_arguments)]
    pub fn run(&self, n: usize, dream: bool, budget: Option<usize>, candidates: Option<usize>, mode: Option<Mode>, extend: bool, new_round: bool) -> Result<Vec<Tree>> {
        if !extend {
            return self.rounds(n, dream, budget, candidates, mode);
        }
        let mut out = vec![];
        for i in 0..n {
            if i == 0 && new_round && !self.mem.rounds(&self.run)?.is_empty() {
                out.extend(self.rounds(1, dream, budget, candidates, mode)?);
            } else {
                out.push(self.extend(budget)?);
            }
        }
        Ok(out)
    }

    fn lanes_note(&self) -> String {
        match self.lanes() {
            1 => String::new(),
            n => format!(", {n} attempts at once (evaluations: {})", self.cfg.explore.evaluations),
        }
    }

    /// Rebuild the tree of a recorded round by driving its policy through the
    /// recorded attempts, showing it at each step exactly what it saw then:
    /// attempts whose results had not arrived yet are pending. Returns the tree
    /// and the attempts that never finished (the round was killed).
    fn redrive(&self, policy: &mut Policy, rec: &Tree, total: usize) -> Result<(Tree, VecDeque<usize>)> {
        let mut t = Tree::new(&self.run, rec.round, total, self.goal, &policy.repr());
        let mut root = rec.root().clone();
        root.n_exp = 0;
        t.set_root(root);
        policy.reset(rec.round as u64);
        let finished = |j: usize| rec.nodes[j].status != PENDING;
        let mut shown = vec![false; rec.nodes.len()];
        for i in 1..=rec.nodes.len() {
            for (j, shown) in shown.iter_mut().enumerate().take(i).skip(1) {
                if !*shown && finished(j) && (i == rec.nodes.len() || rec.nodes[j].visible() <= i) {
                    *shown = true;
                    t.resolve(rec.nodes[j].clone());
                }
            }
            let Some(r) = rec.nodes.get(i) else { break };
            let again = "start a new round instead (git dream run --new-round)";
            let Some(p) = policy.select(&t)? else { bail!("the policy of round {} stops before reproducing its {} recorded attempts; {again}", rec.round, rec.calls()) };
            if Some(p) != r.parent || Some(t.nodes[p].n_exp) != r.k {
                bail!("the policy of round {} no longer reproduces its recorded attempts (it diverges at attempt {i}); {again}", rec.round);
            }
            t.nodes[p].n_exp += 1;
            let mut n = Node::pending(i, p, r.k.unwrap());
            n.sha = r.sha.clone();
            t.add(n);
        }
        Ok((t, (1..rec.nodes.len()).filter(|&j| !finished(j)).collect()))
    }

    /// Run the policy on `t` until it stops or the tree's budget is spent, with
    /// up to [`Engine::lanes`] attempts in flight. `unfinished` attempts (already
    /// in `t`, pending) are run first.
    ///
    /// This thread chooses attempts and writes every commit, ref and note; an
    /// attempt's own thread runs the agent and the evaluator in the worktree of
    /// its lane. When evaluations must run alone, attempts go in waves: all
    /// lanes are chosen together once nothing is running. Otherwise a lane is
    /// refilled as soon as it is free. After an error nothing new is started,
    /// and the attempts still running are recorded before the error is returned.
    fn search(&self, policy: &mut Policy, t: &mut Tree, mut unfinished: VecDeque<usize>) -> Result<()> {
        let lanes = self.lanes();
        for lane in 0..lanes {
            self.wt(lane)?;
        }
        let base_tree = self.mem.tree_of(&self.base()?)?;
        let gate = Gate::new(&self.cfg.explore.evaluations);
        let (tx, rx) = mpsc::channel::<(usize, Result<Outcome>)>();
        let mut free: Vec<usize> = (0..lanes).rev().collect();
        let mut flying = 0;
        let mut failed: Option<anyhow::Error> = None;
        std::thread::scope(|s| loop {
            if failed.is_none() && (flying == 0 || !gate.alone) {
                let mut batch = vec![];
                while batch.len() < free.len() {
                    match self.start(policy, t, &mut unfinished, lanes) {
                        Ok(Some(id)) => batch.push(id),
                        Ok(None) => break,
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    }
                }
                // what each of them is told: the tree with all of them started
                let view = Arc::new(t.clone());
                for id in batch {
                    let lane = free.pop().unwrap();
                    flying += 1;
                    let (tx, view, gate, base_tree) = (tx.clone(), view.clone(), &gate, &base_tree);
                    s.spawn(move || {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.attempt(gate, lane, &view, id, base_tree)));
                        let _ = tx.send((lane, r.unwrap_or_else(|_| Err(anyhow!("attempt n{id:03} panicked")))));
                    });
                }
            }
            if flying == 0 {
                break;
            }
            let (lane, outcome) = rx.recv().expect("this thread holds a sender");
            flying -= 1;
            free.push(lane);
            if let Err(e) = outcome.and_then(|o| self.record(t, o)) {
                failed.get_or_insert(e);
            }
        });
        failed.map_or(Ok(()), Err)
    }

    /// The next attempt to start, added to `t` as pending: one that never
    /// finished, else the policy's pick. None: budget spent, or the policy has
    /// nothing to continue (yet).
    fn start(&self, policy: &mut Policy, t: &mut Tree, unfinished: &mut VecDeque<usize>, lanes: usize) -> Result<Option<usize>> {
        if let Some(id) = unfinished.pop_front() {
            return Ok(Some(id));
        }
        if t.calls() >= t.budget {
            return Ok(None);
        }
        let Some(p) = policy.select(t)? else { return Ok(None) };
        anyhow::ensure!(t.nodes[p].status != PENDING, "the policy continued attempt n{p:03}, which is still running");
        let k = t.nodes[p].n_exp;
        t.nodes[p].n_exp += 1;
        let mut n = Node::pending(t.nodes.len(), p, k);
        if lanes > 1 {
            // Recorded before it runs, so git always holds every attempt that was
            // started: a killed round has no gaps and its policy can be driven through it again.
            n.sha = self.mem.commit_attempt(t, &n, &self.mem.tree_of(&t.nodes[p].sha)?, &t.nodes[p].sha, None)?;
        }
        let id = n.id;
        t.add(n);
        Ok(Some(id))
    }

    /// Commit a finished attempt and put its result into the tree.
    fn record(&self, t: &mut Tree, (mut n, tree, note): Outcome) -> Result<()> {
        let parent_sha = t.nodes[n.parent.unwrap()].sha.clone();
        // attempts chosen from now on know this result
        n.seen = t.nodes.len();
        n.sha = self.mem.commit_attempt(t, &n, &tree, &parent_sha, Some(&note))?;
        if n.status != NOOP {
            self.seen().entry(n.design.clone()).or_insert((n.score, n.status.clone(), n.sha.clone()));
        }
        if !self.quiet {
            print_attempt(t, &n);
        }
        t.resolve(n);
        Ok(())
    }

    /// One attempt, in the worktree of `lane`: run the agent on the parent,
    /// evaluate what it made. `t` is the tree as it was when the attempt started.
    fn attempt(&self, gate: &Gate, lane: usize, t: &Tree, nid: usize, base_tree: &str) -> Result<Outcome> {
        let (parent, k) = (t.nodes[nid].parent.unwrap(), t.nodes[nid].k.unwrap());
        let p = &t.nodes[parent];
        let (tree, design, change, parent_tree, agent_note) = match &self.agent {
            Agent::Space(s) => {
                let (design, change) = s.propose(&p.design, k);
                (self.mem.tree_with_files(Some(base_tree), &self.params_file(s, &design))?, design, change, None, None)
            }
            Agent::Command(c) => {
                let pt = self.mem.tree_of(&p.sha)?;
                let _working = gate.agent();
                let (tree, msg, note) = run_agent(self, c, lane, t, parent, k, &pt)?;
                (tree.clone(), tree, msg, Some(pt), Some(note))
            }
        };
        let mut n = Node::new(nid, Some(parent), Some(k), design.clone(), None, OK, change);
        let protected_changed = if parent_tree.is_some() && !self.cfg.evaluate.protected.is_empty() {
            let mut args = vec!["diff", "--name-only", base_tree, tree.as_str(), "--"];
            args.extend(self.cfg.evaluate.protected.iter().map(String::as_str));
            self.mem.git(&args)?
        } else { String::new() };
        let mut note = if !protected_changed.is_empty() {
            n.status = FAIL.into();
            json!({"status": FAIL, "exit": "protected paths changed", "stderr": protected_changed})
        } else if parent_tree.as_deref() == Some(tree.as_str()) {
            n.status = NOOP.into();
            json!({"status": NOOP})
        } else {
            let _isolated = gate.evaluation();
            let r = self.evaluate_in(lane, &tree, &design, t.round, nid, true)?;
            n.score = r.score;
            n.status = if r.score.is_none() && r.status == OK { FAIL.into() } else { r.status };
            r.note
        };
        if let Some(a) = agent_note {
            note["agent"] = a;
        }
        Ok((n, tree, note))
    }

    /// Pack the round's loose objects (and every `gc_every` rounds run git gc).
    /// The round is already recorded, so a failure here is only a warning.
    fn compact(&self, round: u32) {
        let s = &self.cfg.storage;
        let r = if s.gc_every > 0 && round.is_multiple_of(s.gc_every) {
            self.log(&format!("  r{round}: git gc"));
            self.mem.gc()
        } else if s.repack {
            self.mem.repack()
        } else {
            return;
        };
        if let Err(e) = r {
            eprintln!("git dream: warning: packing objects after round {round} failed: {e:#}");
        }
    }

    /// Pick the round's best attempt. With evaluate.confirm_top = k, the top k
    /// designs are re-measured `confirm_reps` times first (guards against
    /// promoting a lucky noisy measurement: the winner's curse).
    fn promote(&self, t: &Tree) -> Result<Node> {
        let mut best = t.best().ok_or_else(|| anyhow!("round has no valid attempt"))?.clone();
        let k = self.cfg.evaluate.confirm_top;
        if k > 0 && t.nodes.iter().filter(|n| n.score.is_some()).count() > 1 {
            let mut valid: Vec<&Node> = t.nodes.iter().filter(|n| n.score.is_some()).collect();
            valid.sort_by(|a, b| t.cost(a.score.unwrap()).total_cmp(&t.cost(b.score.unwrap())));
            let mut top: Vec<&Node> = vec![];
            for n in valid {
                if top.len() < k && !top.iter().any(|x| x.design == n.design) {
                    top.push(n);
                }
            }
            if !top.iter().any(|n| n.design == t.root().design) { top.push(t.root()); }
            // Every re-measurement is independent: they run at once when evaluations may.
            let reps = self.cfg.evaluate.confirm_reps;
            let mut jobs = vec![];
            for n in &top {
                let tree = self.mem.tree_of(&n.sha)?;
                jobs.extend((0..reps).map(|_| (*n, tree.clone())));
            }
            let lanes = if self.cfg.explore.evaluations == "parallel" { self.lanes().min(jobs.len()) } else { 1 };
            for lane in 0..lanes {
                self.wt(lane)?;
            }
            let measure = |lane: usize, (n, tree): &(&Node, String)| self.evaluate_in(lane, tree, &n.design, t.round, n.id, false);
            let mut measured = if lanes > 1 {
                crate::par::map(lanes, &jobs, measure)
            } else {
                // one at a time: stop at the first evaluation that cannot run at all
                let mut v: Vec<Result<EvalResult>> = vec![];
                for j in &jobs {
                    v.push(measure(0, j));
                    if v.last().unwrap().is_err() {
                        break;
                    }
                }
                v
            }
            .into_iter();
            let mut means = vec![];
            for n in &top {
                let mut vals = vec![];
                let mut results = vec![];
                for res in measured.by_ref().take(reps) {
                    let res = res?;
                    vals.push(res.score);
                    results.push(res.note);
                }
                let m = if vals.iter().all(Option::is_some) { Some(vals.iter().flatten().sum::<f64>() / vals.len() as f64) } else { None };
                let mut note = self.mem.read_note(&n.sha).unwrap_or(json!({}));
                note["confirm"] = json!(vals);
                note["confirmation"] = json!({"score": m, "evaluations": results});
                self.mem.note(&n.sha, &note)?;
                means.push(m);
            }
            let i = (0..top.len()).filter(|&i| means[i].is_some()).min_by(|&a, &b| t.cost(means[a].unwrap()).total_cmp(&t.cost(means[b].unwrap())))
                .ok_or_else(|| anyhow!("all confirmation candidates failed; no attempt promoted"))?;
            best = top[i].clone();
            best.score = means[i];
            self.log(&format!("  r{} confirm: {}", t.round, top.iter().zip(&means).map(|(n, m)| format!("n{:03} {}", n.id, fs(*m))).collect::<Vec<_>>().join(", ")));
        }
        self.mem.set_ref(&format!("refs/dream/{}/r{:02}/best", self.run, t.round), &best.sha)?;
        self.mem.set_ref(&format!("refs/heads/dream/{}/best", self.run), &best.sha)?;
        Ok(best)
    }

    /// A run killed mid-round (timeout, Ctrl-C, reboot) leaves recorded attempts
    /// but no promoted one, and the next round would have no parent. Close such a
    /// round by promoting from the attempts it recorded: they cost real evaluations,
    /// so they are kept. Its tree simply has fewer calls than its budget.
    pub fn resume(&self) -> Result<()> {
        let Some(&last) = self.mem.rounds(&self.run)?.last() else { return Ok(()) };
        if self.mem.rev(&format!("refs/dream/{}/r{last:02}/best", self.run)).is_some() {
            return Ok(());
        }
        let t = self.mem.load_tree(&self.run, last)?.ok_or_else(|| anyhow!("round {last} not found"))?;
        anyhow::ensure!(t.nodes.iter().any(|n| n.score.is_some()),
            "round {last} of run {:?} was interrupted before any attempt was scored; fix the evaluator, then use a new --run (or `git dream clean --run {} --yes`)", self.run, self.run);
        let best = self.promote(&t)?;
        self.log(&format!("  r{last}: interrupted after {} calls; resumed by promoting n{:03} {} from its recorded attempts", t.calls(), best.id, fs(best.score)));
        self.compact(last);
        Ok(())
    }

    pub fn rounds(&self, n: usize, dream: bool, budget: Option<usize>, candidates: Option<usize>, mode: Option<Mode>) -> Result<Vec<Tree>> {
        self.resume()?;
        let mut out = vec![];
        for _ in 0..n {
            let r = self.next_round()?;
            if dream && r > 1 {
                self.dream(Some(r), candidates, mode, budget, true)?;
            }
            out.push(self.explore(Some(r), budget)?);
        }
        Ok(out)
    }
}

fn print_attempt(t: &Tree, n: &Node) {
    let b = t.best().and_then(|b| b.score);
    let best = match (b, n.score) {
        (Some(b), Some(s)) if t.cost(s) < t.cost(b) => Some(s),
        (None, s) => s,
        (b, _) => b,
    };
    println!("  r{} n{:03} <- n{:03} {:5} {:>10}  best {:>10}  {}", t.round, n.id, n.parent.unwrap(), n.status, fs(n.score), fs(best), short(summary(&n.change), 60));
}

/// Where, inside the scratch worktree, git-dream puts the files it hands a
/// command agent (the history, the proposal to write). Inside the worktree so a
/// sandboxed agent may use them; removed before the attempt is snapshotted, so
/// they never enter a commit.
pub const AGENT_DIR: &str = ".dream-agent";
const HISTORY_FILE: &str = "history.md";
const MESSAGE_FILE: &str = "message.md";

fn run_agent(e: &Engine, c: &CommandAgent, lane: usize, t: &Tree, parent: usize, k: usize, parent_tree: &str) -> Result<(String, String, Value)> {
    let wt = e.wt(lane)?;
    e.mem.materialize(&wt, parent_tree)?;
    let dir = wt.join(AGENT_DIR);
    std::fs::create_dir_all(&dir)?;
    let ctx = dir.join(HISTORY_FILE);
    let msgf = dir.join(MESSAGE_FILE);
    // The same history dreaming replays (`[dream] history`), read from git, plus
    // this round in progress: other runs first, then this run's earlier rounds.
    let mut earlier: Vec<Tree> = e.history()?.into_iter().filter(|h| h.run != e.run || h.round < t.round).collect();
    earlier.sort_by_key(|h| h.run == e.run);
    let trees: Vec<&Tree> = earlier.iter().chain([t]).collect();
    let failed: Vec<&str> = trees.iter().flat_map(|h| &h.nodes).filter(|n| n.status != OK && n.status != NOOP && n.status != PENDING).map(|n| n.sha.as_str()).collect();
    let notes = e.mem.notes_of(&failed)?;
    std::fs::write(&ctx, history_context(&trees, parent, k, &notes))?;
    let context = agent_context(t, parent, k);
    let p = &t.nodes[parent];
    let env = vec![
        ("DREAM_RUN".to_string(), e.run.clone()),
        ("DREAM_ROUND".into(), t.round.to_string()),
        ("DREAM_PARENT".into(), p.sha.clone()),
        ("DREAM_PARENT_SCORE".into(), p.score.map_or(String::new(), |s| format!("{s:?}"))),
        ("DREAM_K".into(), k.to_string()),
        ("DREAM_GOAL".into(), t.goal.as_str().into()),
        ("DREAM_CONTEXT".into(), ctx.to_string_lossy().into_owned()),
        ("DREAM_MESSAGE_FILE".into(), msgf.to_string_lossy().into_owned()),
        ("DREAM_PROMPT".into(), format!("{}\n\n{context}", c.prompt.trim()).trim().to_string()),
        ("DREAM_WORKTREE".into(), wt.to_string_lossy().into_owned()),
        ("DREAM_LANE".into(), lane.to_string()),
    ];
    let r = sh(&c.command, &wt, &env, c.timeout)?;
    let msg = std::fs::read_to_string(&msgf).map(|s| s.trim().to_string()).unwrap_or_default();
    let msg = if msg.is_empty() { format!("agent change (k={k})") } else { msg };
    std::fs::remove_dir_all(&dir)?;
    let tree = e.mem.snapshot(&wt)?;
    Ok((tree, msg, json!({"exit": r.exit_json(), "seconds": (r.seconds * 1000.0).round() / 1000.0, "stdout": excerpt(&r.stdout, 10, 20), "stderr": excerpt(&r.stderr, 10, 20)})))
}

/// The prompt's search context for a command agent: where it is, and how to use
/// the full history (the exploration procedure of Dream-RSI, appendix B.1).
pub fn agent_context(t: &Tree, parent: usize, k: usize) -> String {
    let p = &t.nodes[parent];
    let better = if t.goal == Goal::Min { "lower" } else { "higher" };
    let mut s = format!("# git-dream: continue attempt n{:03} (continuation #{}), round {}\n\nScore: {better} is better. Current attempt score: {} ({}).\n\n## Path from the round's root to this attempt\n", p.id, k + 1, t.round, fs(p.score), p.status);
    let mut path = vec![p];
    while let Some(q) = path.last().unwrap().parent {
        path.push(&t.nodes[q]);
    }
    for a in path.iter().rev() {
        s += &format!("- n{:03} {} {}: {}\n", a.id, a.status, fs(a.score), short(summary(&a.change), 200));
    }
    // this attempt itself is in the tree already, as a pending continuation #k
    let kids: Vec<&Node> = t.children(p.id).into_iter().filter(|c| c.k != Some(k)).collect();
    let (running, tried): (Vec<&Node>, Vec<&Node>) = kids.into_iter().partition(|c| c.status == PENDING);
    if !tried.is_empty() {
        s += "\n## Already tried from this attempt\n";
        for c in tried {
            s += &format!("- n{:03} {} {}: {}\n", c.id, c.status, fs(c.score), short(summary(&c.change), 200));
        }
    }
    if !running.is_empty() {
        s += &format!(
            "\n## Being tried from this attempt right now\n\n{} other agent{} started from this same attempt at the same time as you and cannot see your work. \
             The obvious next step is the one they will most likely take: prefer a change they are unlikely to make.\n",
            running.len(),
            if running.len() == 1 { "" } else { "s" }
        );
    }
    if let Some(b) = t.best() {
        if b.id != p.id {
            s += &format!("\nBest attempt this round: n{:03} {}: {}\n", b.id, fs(b.score), short(summary(&b.change), 200));
        }
    }
    let (h, m) = (format!("{AGENT_DIR}/{HISTORY_FILE}"), format!("{AGENT_DIR}/{MESSAGE_FILE}"));
    s + &format!(
        "
## How to work

1. Before you change anything, read the whole search history in `{h}`: every attempt of every round, not only this branch. It holds each attempt's write-up, its measured result and, for failures, the evaluator's output.
2. Trust measured results over what a write-up claims. For each failure, decide whether the idea was wrong or only its implementation (a bug, a bad parameter). Don't repeat a wrong idea. Retry a badly implemented one only after you have found the fault in its code (`git show <commit>`) and have a specific fix.
3. If most attempts are small variations of one idea with shrinking gains, prefer a structurally different idea or an untried combination over another small tweak.
4. Make one change, and make it new: not a repeat or a rename of an earlier attempt.
5. Write your proposal to `{m}`: a one-line summary, a blank line, then the idea, the evidence from the history behind it, and why it is not a repeat. Don't claim results; the evaluator measures them. Leave the rest of `{AGENT_DIR}/` alone: it is removed before your change is committed.
"
    )
}

/// The complete search history handed to a command agent (`$DREAM_CONTEXT`):
/// every attempt of every round so far (`trees`, the last one in progress;
/// with `[dream] history = "all"` also the other runs' rounds, listed first),
/// with its write-up, its measured result and, for failures, the evaluator's
/// output from the attempt's note. Raw evidence, not a summary: in Dream-RSI
/// (§5.1, Fig. 5), distilled "directional insights" made the search worse.
pub fn history_context(trees: &[&Tree], parent: usize, k: usize, notes: &HashMap<String, Value>) -> String {
    let Some(cur) = trees.last() else { return String::new() };
    let better = if cur.goal == Goal::Min { "lower" } else { "higher" };
    let mut others: Vec<&str> = trees.iter().map(|t| t.run.as_str()).filter(|r| *r != cur.run).collect();
    others.dedup();
    let multi = !others.is_empty();
    // Attempt labels: "r02" within one run, "<run> r02" once other runs are included.
    let label = |t: &Tree| if multi { format!("{} r{:02}", t.run, t.round) } else { format!("r{:02}", t.round) };
    let mut s = format!(
        "# Search history: run {}\n\nScore: {better} is better; failed attempts have no score. You are continuing {} n{:03} (continuation #{}).\n\n\
         Every attempt is a git commit. `git show <commit>` shows what it changed relative to the attempt it continued; `git diff <a> <b>` compares two attempts. \
         The write-ups are the agents' own words, written before evaluation. Status, score and evaluator output are measured.\n",
        cur.run,
        label(cur),
        parent,
        k + 1
    );
    if multi {
        s += &format!(
            "\nThis history also holds the other run{} {} in this repository, listed first. Runs search separately and may have started from different code: \
             compare scores within a run, and use `git diff` to see how another run's code differs from yours.\n",
            if others.len() == 1 { "" } else { "s" },
            others.iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ")
        );
    }
    for t in trees {
        let best = t.best().map_or(String::new(), |b| format!(", best n{:03} {}", b.id, fs(b.score)));
        let run = if multi { format!("Run {}, round", t.run) } else { "Round".into() };
        // attempts without a result yet (running now, or cut off when a run was killed) have nothing to show
        let done = t.nodes.iter().skip(1).filter(|n| n.status != PENDING).count();
        // (the reader's own attempt is one of them: it is not "more")
        let own = usize::from(std::ptr::eq(*t, *cur) && t.nodes.iter().any(|n| n.status == PENDING && n.parent == Some(parent) && n.k == Some(k)));
        let running = t.calls() - done - own;
        let more = if running > 0 { format!(", {running} more without a result yet") } else { String::new() };
        s += &format!("\n## {run} {} ({done} attempt{}{best}{more})\n", t.round, if done == 1 { "" } else { "s" });
        for n in t.nodes.iter().filter(|n| n.status != PENDING) {
            let note = notes.get(&n.sha);
            let result = match n.status.as_str() {
                OK => format!("ok {}", fs(n.score)),
                NOOP => "noop (the agent changed nothing)".into(),
                st => match note {
                    Some(v) if v.get("reused").is_some() => format!("{st} (same code as {}, not re-run)", short(v["reused"].as_str().unwrap_or(""), 10)),
                    Some(v) => failure(v).map_or(st.to_string(), |f| format!("{st} ({f})")),
                    None => st.to_string(),
                },
            };
            let from = n.parent.map_or("root".to_string(), |p| format!("continues n{p:03}"));
            let here = if t.run == cur.run && t.round == cur.round && n.id == parent { " · **you are continuing this attempt**" } else { "" };
            s += &format!("\n### {} n{:03} · {from} · {result} · {}{here}\n", label(t), n.id, &n.sha[..n.sha.len().min(10)]);
            let w = n.change.trim();
            if !w.is_empty() {
                s += "\n";
                for l in w.lines() {
                    s += &format!("> {l}\n").replace("> \n", ">\n");
                }
            }
            if n.status != OK && n.status != NOOP {
                let out = note.map(note_output).unwrap_or_default();
                if !out.is_empty() {
                    s += &format!("\nEvaluator output:\n\n````text\n{out}\n````\n");
                }
            }
        }
    }
    s
}

/// The first line of an attempt's write-up.
pub fn summary(change: &str) -> &str {
    change.trim().lines().next().unwrap_or("")
}

pub fn fs(x: Option<f64>) -> String {
    match x {
        None => "-".into(),
        Some(v) => {
            let a = v.abs();
            if a != 0.0 && !(1e-4..1e7).contains(&a) {
                format!("{v:.4e}")
            } else {
                let digits = if a >= 1.0 { (6 - (a.log10().floor() as i32 + 1)).max(0) } else { 6 } as usize;
                let s = format!("{v:.digits$}");
                if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
            }
        }
    }
}

pub fn short(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
