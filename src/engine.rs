//! Configuration, the live world (real agent + evaluator + git commits) and the
//! round loop: `[dream ->] explore -> promote`, repeated.

use crate::agents::{excerpt, failure, note_output, sh, Agent, CommandAgent, EvalResult, Evaluator, SpaceAgent};
use crate::memory::{check_run, Memory};
use crate::policies::{Param, Policy, Registry};
use crate::rng::hash;
use crate::sim::{self, DreamArgs, Mode, ReplayResult, World};
use crate::tree::{Goal, Node, Tree, FAIL, NOOP, OK};
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

#[derive(Deserialize, Debug, Clone)]
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

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct EvalCfg {
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
        EvalCfg { command: String::new(), goal: Goal::Min, timeout: 600.0, reuse: false, remeasure_root: false, confirm_top: 0, confirm_reps: 1 }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ExploreCfg {
    pub budget: usize,
}

impl Default for ExploreCfg {
    fn default() -> Self {
        ExploreCfg { budget: 20 }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct DreamCfg {
    pub lam: f64,
    pub candidates: usize,
    pub mode: String,
    /// "none" or "root": cached scores relative to each tree's root.
    pub normalize: String,
    /// "run" (this run's rounds) or "all" (every run in the repository).
    pub history: String,
}

impl Default for DreamCfg {
    fn default() -> Self {
        DreamCfg { lam: 0.02, candidates: 300, mode: "cache".into(), normalize: "none".into(), history: "run".into() }
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
        initial.insert("family".into(), toml::Value::String("refine_best".into()));
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
    seen: RefCell<Seen>,
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
        let mut registry = Registry::builtin();
        let mut external_files = vec![];
        for (name, v) in &cfg.policies.external {
            let t = v.as_table().ok_or_else(|| anyhow!("[policies.external.{name}] must be a table"))?;
            let command = t.get("command").and_then(|c| c.as_str()).ok_or_else(|| anyhow!("[policies.external.{name}] needs `command`"))?;
            let mut grid = vec![];
            if let Some(g) = t.get("grid").and_then(|g| g.as_table()) {
                for (k, vals) in g {
                    let vals = vals.as_array().ok_or_else(|| anyhow!("grid {name}.{k} must be an array"))?;
                    grid.push((k.clone(), vals.iter().map(|x| Param::from_json(&toml_to_json(x))).collect::<Result<Vec<_>>>()?));
                }
            }
            let doc = t.get("doc").and_then(|d| d.as_str()).unwrap_or("external policy");
            // run relative commands from the repository root
            registry.add_external(name, &format!("cd {} && {command}", shell_quote(&mem.top.to_string_lossy())), grid, doc)?;
            if let Some(src) = t.get("source").and_then(|s| s.as_str()) {
                if let Ok(text) = std::fs::read_to_string(mem.top.join(src)) {
                    external_files.push((Path::new(src).file_name().unwrap().to_string_lossy().into_owned(), text));
                }
            }
        }
        let goal = cfg.evaluate.goal;
        Ok(Engine { mem, cfg, raw_cfg: raw, run, agent, registry, goal, quiet: false, seen: RefCell::new(HashMap::new()), external_files })
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

    pub fn wt(&self) -> Result<PathBuf> {
        self.mem.worktree(&self.run)
    }

    pub fn mode(&self) -> Mode {
        Mode::parse(&self.cfg.dream.mode).unwrap_or(Mode::Cache)
    }

    fn normalize_root(&self) -> bool {
        self.cfg.dream.normalize == "root"
    }

    pub fn evaluate(&self, tree: &str, design: &str, round: u32, nid: usize, reuse: bool) -> Result<EvalResult> {
        if reuse && self.cfg.evaluate.reuse {
            if let Some((s, st, sha)) = self.seen.borrow().get(design) {
                return Ok(EvalResult { score: *s, status: st.clone(), note: json!({"score": s, "status": st, "reused": sha}) });
            }
        }
        let wt = self.wt()?;
        self.mem.materialize(&wt, tree)?;
        let mut env = vec![
            ("DREAM_RUN".to_string(), self.run.clone()),
            ("DREAM_ROUND".into(), round.to_string()),
            ("DREAM_NODE".into(), nid.to_string()),
            ("DREAM_DESIGN".into(), design.to_string()),
            ("DREAM_WORKTREE".into(), wt.to_string_lossy().into_owned()),
        ];
        env.extend(self.agent.env(design));
        self.evaluator()?.run(&wt, &env)
    }

    pub fn initial_policy(&self) -> Result<Policy> {
        self.registry.make(&toml_to_json(&toml::Value::Table(self.cfg.policies.initial.clone())))
    }

    pub fn deployed(&self, round: Option<u32>) -> Result<Option<Policy>> {
        match self.mem.policy_doc(&self.run, round) {
            Some(doc) => Ok(Some(self.registry.make(&doc["policy"])?)),
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
        let hist = self.history()?;
        let current = self.current_policy()?;
        if hist.is_empty() {
            self.log(&format!("dream: no history yet, keeping {}", current.repr()));
            return Ok(vec![]);
        }
        let budget = budget.unwrap_or(self.cfg.explore.budget);
        let mode = mode.unwrap_or(self.mode());
        let eff = if self.agent.space().is_some() { mode } else { Mode::Strict };
        let fams = self.registry.searchable(&self.cfg.policies.families);
        let a = DreamArgs {
            history: &hist,
            budget,
            lam: self.cfg.dream.lam,
            candidates: candidates.unwrap_or(self.cfg.dream.candidates),
            seed: hash(&format!("{}|{round}", self.run)),
            agent: self.agent.space(),
            mode: eff,
            normalize_root: self.normalize_root(),
            registry: &self.registry,
            families: &fams,
        };
        let ranking = sim::dream(&current, &[], &a)?;
        let (best, top) = (&ranking[0].0, &ranking[0].1);
        let cur = &ranking.iter().find(|(c, _)| c.repr() == current.repr()).unwrap().1;
        self.log(&format!("dream r{round}: {} policies over {} trees ({}); current J={:.4}, best J={:.4} -> {}", ranking.len(), hist.len(), eff.as_str(), cur.j, top.j, best.repr()));
        if commit {
            let doc = json!({
                "run": self.run, "round": round, "policy": best.spec(), "repr": best.repr(),
                "why": format!("dreamt over {} recorded trees, {} candidate policies, {} replay", hist.len(), ranking.len(), eff.as_str()),
                "dream": {"J": top.j, "gain": top.gain, "calls": top.calls, "miss_rate": top.miss_rate, "J_current": cur.j, "current": current.repr(),
                          "mode": eff.as_str(), "lam": self.cfg.dream.lam, "budget": budget, "trees": hist.len(), "candidates": ranking.len()},
                "top": ranking.iter().take(10).map(|(c, r)| json!({"policy": c.repr(), "spec": c.spec(), "J": r.j, "gain": r.gain, "calls": r.calls, "miss_rate": r.miss_rate})).collect::<Vec<_>>(),
            });
            self.mem.commit_policy(&self.run, round, &doc, &self.policy_files())?;
        }
        Ok(ranking)
    }

    pub fn replay(&self, policy: &Policy, mode: Option<Mode>, budget: Option<usize>) -> Result<ReplayResult> {
        let hist = self.history()?;
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
            let res = if self.cfg.evaluate.remeasure_root { None } else { Some(EvalResult { score: pn.score, status: pn.status.clone(), note: json!({"score": pn.score, "status": pn.status, "copied_from": parent}) }) };
            (parent.clone(), self.mem.tree_of(&parent)?, pn.design.clone(), format!("from r{:02} n{:03}", round - 1, pn.id), res)
        };
        let res = match res {
            Some(r) => r,
            None => self.evaluate(&tree, &design, round, 0, true)?,
        };
        let mut root = Node::new(0, None, None, design, res.score, &res.status, change);
        t.set_root(root.clone());
        root.sha = self.mem.commit_attempt(&t, &root, &tree, &parent, Some(&res.note))?;
        if root.score.is_none() {
            bail!("the root of round {round} did not evaluate ({}):\n{}", root.status, note_output(&res.note));
        }
        Ok(root)
    }

    pub fn explore(&self, round: Option<u32>, budget: Option<usize>) -> Result<Tree> {
        let round = match round {
            Some(r) => r,
            None => self.next_round()?,
        };
        let budget = budget.unwrap_or(self.cfg.explore.budget);
        let mut policy = self.policy_for(round)?;
        self.log(&format!("explore r{round}: {}, budget {budget}", policy.repr()));
        let root = self.make_root(round, budget, &policy)?;
        self.log(&format!("  r{round} n000 root {} {}", short(&root.design, 24), fs(root.score)));
        let mut world = LiveWorld { e: self, base_tree: self.mem.tree_of(&self.base()?)? };
        let quiet = self.quiet;
        let t = sim::run_tree(&mut policy, &mut world, &root, budget, &self.run, round, self.goal, round as u64, &mut |t, n| {
            if quiet {
                return;
            }
            let b = t.best().and_then(|b| b.score);
            let best = match (b, n.score) {
                (Some(b), Some(s)) if t.cost(s) < t.cost(b) => Some(s),
                (None, s) => s,
                (b, _) => b,
            };
            println!("  r{round} n{:03} <- n{:03} {:5} {:>10}  best {:>10}  {}", n.id, n.parent.unwrap(), n.status, fs(n.score), fs(best), short(summary(&n.change), 60));
        })?;
        let best = self.promote(&t)?;
        self.log(&format!("  r{round}: {} calls, promoted n{:03} {} ({:+.2}% vs root)  {}", t.calls(), best.id, fs(best.score), sim::gain(&t) * 100.0, &best.sha[..10]));
        self.compact(round);
        Ok(t)
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
            let mut means = vec![];
            for n in &top {
                let mut vals = vec![n.score.unwrap()];
                for _ in 0..self.cfg.evaluate.confirm_reps {
                    if let Some(s) = self.evaluate(&self.mem.tree_of(&n.sha)?, &n.design, t.round, n.id, false)?.score {
                        vals.push(s);
                    }
                }
                let m = vals.iter().sum::<f64>() / vals.len() as f64;
                let mut note = self.mem.read_note(&n.sha).unwrap_or(json!({}));
                note["confirm"] = json!(vals);
                self.mem.note(&n.sha, &note)?;
                means.push(m);
            }
            let i = (0..top.len()).min_by(|&a, &b| t.cost(means[a]).total_cmp(&t.cost(means[b]))).unwrap();
            best = top[i].clone();
            self.log(&format!("  r{} confirm: {}", t.round, top.iter().zip(&means).map(|(n, m)| format!("n{:03} {}", n.id, fs(Some(*m)))).collect::<Vec<_>>().join(", ")));
        }
        self.mem.set_ref(&format!("refs/dream/{}/r{:02}/best", self.run, t.round), &best.sha)?;
        self.mem.set_ref(&format!("refs/heads/dream/{}/best", self.run), &best.sha)?;
        Ok(best)
    }

    pub fn rounds(&self, n: usize, dream: bool, budget: Option<usize>, candidates: Option<usize>, mode: Option<Mode>) -> Result<Vec<Tree>> {
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

struct LiveWorld<'a> {
    e: &'a Engine,
    base_tree: String,
}

impl World for LiveWorld<'_> {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
        let e = self.e;
        let p = &t.nodes[parent];
        let nid = t.nodes.len();
        let (tree, design, change, parent_tree, agent_note) = match &e.agent {
            Agent::Space(s) => {
                let (design, change) = s.propose(&p.design, k);
                (e.mem.tree_with_files(Some(&self.base_tree), &e.params_file(s, &design))?, design, change, None, None)
            }
            Agent::Command(c) => {
                let pt = e.mem.tree_of(&p.sha)?;
                let (tree, msg, note) = run_agent(e, c, t, parent, k, &pt)?;
                (tree.clone(), tree, msg, Some(pt), Some(note))
            }
        };
        let mut n = Node::new(nid, Some(parent), Some(k), design.clone(), None, OK, change);
        let mut note = if parent_tree.as_deref() == Some(tree.as_str()) {
            n.status = NOOP.into();
            json!({"status": NOOP})
        } else {
            let r = e.evaluate(&tree, &design, t.round, nid, true)?;
            n.score = r.score;
            n.status = if r.score.is_none() && r.status == OK { FAIL.into() } else { r.status };
            r.note
        };
        if let Some(a) = agent_note {
            note["agent"] = a;
        }
        n.sha = e.mem.commit_attempt(t, &n, &tree, &p.sha, Some(&note))?;
        if n.status != NOOP {
            e.seen.borrow_mut().entry(design).or_insert((n.score, n.status.clone(), n.sha.clone()));
        }
        Ok(n)
    }
}

/// Where, inside the scratch worktree, git-dream puts the files it hands a
/// command agent (the history, the proposal to write). Inside the worktree so a
/// sandboxed agent may use them; removed before the attempt is snapshotted, so
/// they never enter a commit.
pub const AGENT_DIR: &str = ".dream-agent";
const HISTORY_FILE: &str = "history.md";
const MESSAGE_FILE: &str = "message.md";

fn run_agent(e: &Engine, c: &CommandAgent, t: &Tree, parent: usize, k: usize, parent_tree: &str) -> Result<(String, String, Value)> {
    let wt = e.wt()?;
    e.mem.materialize(&wt, parent_tree)?;
    let dir = wt.join(AGENT_DIR);
    std::fs::create_dir_all(&dir)?;
    let ctx = dir.join(HISTORY_FILE);
    let msgf = dir.join(MESSAGE_FILE);
    // Every round of this run so far: earlier ones from git, this one in progress.
    let earlier: Vec<Tree> = e.mem.history(&e.run)?.into_iter().filter(|h| h.round < t.round).collect();
    let trees: Vec<&Tree> = earlier.iter().chain([t]).collect();
    let failed: Vec<&str> = trees.iter().flat_map(|h| &h.nodes).filter(|n| n.status != OK && n.status != NOOP).map(|n| n.sha.as_str()).collect();
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
    let kids = t.children(p.id);
    if !kids.is_empty() {
        s += "\n## Already tried from this attempt\n";
        for c in kids {
            s += &format!("- n{:03} {} {}: {}\n", c.id, c.status, fs(c.score), short(summary(&c.change), 200));
        }
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
/// every attempt of every round so far (`trees`, the last one in progress),
/// with its write-up, its measured result and, for failures, the evaluator's
/// output from the attempt's note. Raw evidence, not a summary: in Dream-RSI
/// (§5.1, Fig. 5), distilled "directional insights" made the search worse.
pub fn history_context(trees: &[&Tree], parent: usize, k: usize, notes: &HashMap<String, Value>) -> String {
    let Some(cur) = trees.last() else { return String::new() };
    let better = if cur.goal == Goal::Min { "lower" } else { "higher" };
    let mut s = format!(
        "# Search history: run {}\n\nScore: {better} is better; failed attempts have no score. You are continuing r{:02} n{:03} (continuation #{}).\n\n\
         Every attempt is a git commit. `git show <commit>` shows what it changed relative to the attempt it continued; `git diff <a> <b>` compares two attempts. \
         The write-ups are the agents' own words, written before evaluation. Status, score and evaluator output are measured.\n",
        cur.run,
        cur.round,
        parent,
        k + 1
    );
    for t in trees {
        let best = t.best().map_or(String::new(), |b| format!(", best n{:03} {}", b.id, fs(b.score)));
        s += &format!("\n## Round {} ({} attempt{}{best})\n", t.round, t.calls(), if t.calls() == 1 { "" } else { "s" });
        for n in &t.nodes {
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
            let here = if t.round == cur.round && n.id == parent { " · **you are continuing this attempt**" } else { "" };
            s += &format!("\n### r{:02} n{:03} · {from} · {result} · {}{here}\n", t.round, n.id, &n.sha[..n.sha.len().min(10)]);
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
