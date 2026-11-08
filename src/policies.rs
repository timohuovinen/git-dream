//! Exploration policies: code that decides which attempt to continue next and
//! when to stop. A policy never runs anything itself, so the same policy runs
//! live (real evaluations) or dreamt (replayed over recorded history).
//!
//! A policy is `{"family": ..., "params": {...}}` (its *spec*), versioned in git
//! for every round. Built-in families are implemented below; any program can be
//! a policy family too ([`External`]), so new policies can be written in any
//! language without rebuilding git-dream.

use crate::rng::Rng;
use crate::tree::{repairable, Tree};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::rc::Rc;

pub const INF: f64 = f64::INFINITY;

#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    Num(f64),
    Bool(bool),
}

impl Param {
    pub fn f(&self) -> f64 {
        match self {
            Param::Num(x) => *x,
            Param::Bool(b) => *b as u8 as f64,
        }
    }
    pub fn b(&self) -> bool {
        match self {
            Param::Num(x) => *x != 0.0,
            Param::Bool(b) => *b,
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            Param::Num(x) if x.is_infinite() => json!("inf"),
            Param::Num(x) if x.fract() == 0.0 && x.abs() < 1e15 => json!(*x as i64),
            Param::Num(x) => json!(x),
            Param::Bool(b) => json!(b),
        }
    }
    pub fn from_json(v: &Value) -> Result<Param> {
        Ok(match v {
            Value::Bool(b) => Param::Bool(*b),
            Value::Number(n) => Param::Num(n.as_f64().unwrap_or(0.0)),
            Value::String(s) if s.eq_ignore_ascii_case("inf") => Param::Num(INF),
            Value::String(s) => Param::Num(s.parse().map_err(|_| anyhow!("bad policy parameter {s:?}"))?),
            _ => bail!("bad policy parameter {v}"),
        })
    }
    pub fn show(&self) -> String {
        match self {
            Param::Num(x) if x.is_infinite() => "inf".into(),
            Param::Num(x) => format!("{x}"),
            Param::Bool(b) => format!("{b}"),
        }
    }
}

pub type Grid = Vec<(String, Vec<Param>)>;

#[derive(Clone, Debug)]
enum Kind {
    RefineBest,
    Ucb,
    Beam,
    Softmax,
    Repair,
    External(Rc<RefCell<External>>),
}

#[derive(Clone, Debug)]
pub struct Family {
    pub name: String,
    pub doc: String,
    pub grid: Grid,
    kind: Kind,
}

fn nums(v: &[f64]) -> Vec<Param> {
    v.iter().map(|x| Param::Num(*x)).collect()
}

fn grid(entries: &[(&str, Vec<Param>)]) -> Grid {
    entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

pub struct Registry {
    pub families: Vec<Family>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Registry {
    pub fn builtin() -> Registry {
        let kmax = nums(&[1.0, 2.0, 3.0, 4.0, 6.0, 8.0, INF]);
        let patience = nums(&[4.0, 6.0, 8.0, 12.0, 16.0, 24.0, INF]);
        let eps = nums(&[0.0, 0.005, 0.01, 0.02, 0.05]);
        let families = vec![
            Family { name: "refine_best".into(), doc: "pi_0, hand-written: always continue the best attempt so far and spend the whole budget.".into(), grid: vec![], kind: Kind::RefineBest },
            Family {
                name: "ucb".into(),
                doc: "Tree-search bandit: relative quality + c * exploration bonus; at most kmax continuations per node; stop after `patience` calls without an improvement > eps.".into(),
                grid: grid(&[("c", nums(&[0.0, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5])), ("kmax", kmax.clone()), ("patience", patience.clone()), ("eps", eps.clone())]),
                kind: Kind::Ucb,
            },
            Family {
                name: "beam".into(),
                doc: "Keep the top-m attempts; continue the least-continued of them.".into(),
                grid: grid(&[("m", nums(&[1.0, 2.0, 3.0, 4.0, 6.0, 8.0])), ("kmax", kmax.clone()), ("patience", patience.clone()), ("eps", eps.clone())]),
                kind: Kind::Beam,
            },
            Family {
                name: "softmax".into(),
                doc: "Sample an attempt with probability ~ exp(relative quality / T).".into(),
                grid: grid(&[("T", nums(&[0.005, 0.01, 0.02, 0.05, 0.1, 0.3])), ("kmax", kmax), ("patience", patience), ("eps", eps)]),
                kind: Kind::Softmax,
            },
            Family {
                name: "repair".into(),
                doc: "Continue failed attempts too (valued as their nearest valid ancestor minus fp, at most kfail times); with dedup, identical designs act as one (mean score, furthest-explored copy). Plus UCB bonus c, kmax, patience.".into(),
                grid: grid(&[
                    ("c", nums(&[0.0, 0.01, 0.02, 0.05, 0.1])),
                    ("fp", nums(&[0.0, 0.01, 0.03, 0.1, 0.3])),
                    ("kmax", nums(&[2.0, 3.0, 4.0, 6.0, 8.0, INF])),
                    ("kfail", nums(&[0.0, 1.0, 2.0, 3.0, 4.0])),
                    ("patience", nums(&[6.0, 8.0, 12.0, 16.0, 24.0, INF])),
                    ("eps", nums(&[0.0, 0.005, 0.01, 0.02])),
                    ("dedup", vec![Param::Bool(false), Param::Bool(true)]),
                ]),
                kind: Kind::Repair,
            },
        ];
        Registry { families }
    }

    /// Register an external policy family: `command` speaks the JSON-lines protocol of [`External`].
    pub fn add_external(&mut self, name: &str, command: &str, grid: Grid, doc: &str) -> Result<()> {
        if self.get(name).is_some() {
            bail!("policy family {name:?} already exists");
        }
        let ext = Rc::new(RefCell::new(External { command: command.into(), proc: None, failed: false }));
        self.families.push(Family { name: name.into(), doc: doc.into(), grid, kind: Kind::External(ext) });
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.name == name)
    }

    /// Families sampled while dreaming: those with a grid, optionally filtered by name.
    pub fn searchable(&self, only: &[String]) -> Vec<&Family> {
        self.families.iter().filter(|f| !f.grid.is_empty() && (only.is_empty() || only.contains(&f.name))).collect()
    }

    pub fn make(&self, spec: &Value) -> Result<Policy> {
        let name = spec.get("family").and_then(Value::as_str).ok_or_else(|| anyhow!("policy spec without family: {spec}"))?;
        let fam = self.get(name).ok_or_else(|| anyhow!("unknown policy family {name:?} (known: {})", self.families.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        let mut params: Vec<(String, Param)> = vec![];
        if let Some(obj) = spec.get("params").and_then(Value::as_object) {
            // grid order first, then anything else, so the repr is stable
            for (k, _) in &fam.grid {
                if let Some(v) = obj.get(k) {
                    params.push((k.clone(), Param::from_json(v)?));
                }
            }
            for (k, v) in obj {
                if !fam.grid.iter().any(|(g, _)| g == k) {
                    params.push((k.clone(), Param::from_json(v)?));
                }
            }
        }
        Ok(Policy::new(fam, params))
    }

    pub fn sample(&self, rng: &mut Rng, fams: &[&Family]) -> Policy {
        let fam = *rng.choose(fams);
        let params = fam.grid.iter().map(|(k, vals)| (k.clone(), rng.choose(vals).clone())).collect();
        Policy::new(fam, params)
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub family: String,
    pub params: Vec<(String, Param)>,
    kind: Kind,
    rng: Rng,
    seed: u64,
}

impl Policy {
    fn new(fam: &Family, params: Vec<(String, Param)>) -> Policy {
        Policy { family: fam.name.clone(), params, kind: fam.kind.clone(), rng: Rng::new(0), seed: 0 }
    }

    pub fn reset(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
        self.seed = seed;
    }

    pub fn spec(&self) -> Value {
        let p: serde_json::Map<String, Value> = self.params.iter().map(|(k, v)| (k.clone(), v.to_json())).collect();
        json!({"family": self.family, "params": p})
    }

    pub fn repr(&self) -> String {
        let p: Vec<String> = self.params.iter().map(|(k, v)| format!("{k}={}", v.show())).collect();
        format!("{}({})", self.family, p.join(","))
    }

    fn get(&self, k: &str) -> Option<&Param> {
        self.params.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }

    fn p(&self, k: &str, default: f64) -> f64 {
        self.get(k).map_or(default, Param::f)
    }

    fn candidates<'t>(&self, t: &'t Tree) -> Vec<&'t crate::tree::Node> {
        let kmax = self.p("kmax", INF);
        t.nodes.iter().filter(|n| n.score.is_some() && (n.n_exp as f64) < kmax).collect()
    }

    fn should_stop(&self, t: &Tree) -> bool {
        t.stalled(self.p("eps", 0.0)) as f64 >= self.p("patience", INF)
    }

    /// Which node to continue next, or None to end the round.
    pub fn select(&mut self, t: &Tree) -> Option<usize> {
        let sc = |n: &crate::tree::Node| n.score.unwrap();
        match &self.kind {
            Kind::RefineBest => {
                let c = self.candidates(t);
                // best score; ties -> newest
                c.into_iter().min_by(|a, b| t.cost(sc(a)).total_cmp(&t.cost(sc(b))).then(b.id.cmp(&a.id))).map(|n| n.id)
            }
            Kind::Ucb => {
                if self.should_stop(t) {
                    return None;
                }
                let best = t.best()?.score?;
                let ln_n = ((t.calls() + 2) as f64).ln();
                let cc = self.p("c", 0.0);
                let val = |n: &crate::tree::Node| t.rel(sc(n), best) + cc * (ln_n / (1 + n.n_exp) as f64).sqrt();
                self.candidates(t).into_iter().max_by(|a, b| val(a).total_cmp(&val(b)).then(a.id.cmp(&b.id))).map(|n| n.id)
            }
            Kind::Beam => {
                if self.should_stop(t) {
                    return None;
                }
                let mut c = self.candidates(t);
                c.sort_by(|a, b| t.cost(sc(a)).total_cmp(&t.cost(sc(b))));
                c.truncate(self.p("m", 1.0) as usize);
                c.into_iter().min_by(|a, b| a.n_exp.cmp(&b.n_exp).then(t.cost(sc(a)).total_cmp(&t.cost(sc(b))))).map(|n| n.id)
            }
            Kind::Softmax => {
                if self.should_stop(t) {
                    return None;
                }
                let c = self.candidates(t);
                let best = c.iter().map(|n| sc(n)).min_by(|a, b| t.cost(*a).total_cmp(&t.cost(*b)))?;
                let temp = self.p("T", 0.05);
                let w: Vec<f64> = c.iter().map(|n| (t.rel(sc(n), best) / temp).exp()).collect();
                Some(c[self.rng.weighted(&w)].id)
            }
            Kind::Repair => self.select_repair(t),
            Kind::External(ext) => {
                let ext = ext.clone();
                let mut e = ext.borrow_mut();
                e.select(&self.spec(), self.seed, t)
            }
        }
    }

    fn select_repair(&self, t: &Tree) -> Option<usize> {
        if self.should_stop(t) {
            return None;
        }
        let best = t.best()?.score?;
        let dedup = self.get("dedup").is_some_and(Param::b);
        let (c, fp, kmax, kfail) = (self.p("c", 0.0), self.p("fp", 0.0), self.p("kmax", INF), self.p("kfail", 0.0));
        let ln_n = ((t.calls() + 2) as f64).ln();
        let mut order: Vec<String> = vec![];
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for n in &t.nodes {
            let key = if dedup { n.design.clone() } else { n.id.to_string() };
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(n.id);
        }
        let mut top: Option<(f64, usize)> = None;
        for key in &order {
            let g: Vec<&crate::tree::Node> = groups[key].iter().map(|&i| &t.nodes[i]).collect();
            // furthest-explored copy; ties -> oldest
            let n = *g.iter().max_by(|a, b| a.n_exp.cmp(&b.n_exp).then(b.id.cmp(&a.id))).unwrap();
            let ok: Vec<f64> = g.iter().filter_map(|x| x.score).collect();
            let (v, pen, lim) = if !ok.is_empty() {
                (ok.iter().sum::<f64>() / ok.len() as f64, 0.0, kmax)
            } else if g.iter().any(|x| repairable(&x.status)) {
                let mut a = n;
                while a.score.is_none() {
                    match a.parent {
                        Some(p) => a = &t.nodes[p],
                        None => break,
                    }
                }
                match a.score {
                    Some(s) => (s, fp, kfail),
                    None => continue,
                }
            } else {
                continue;
            };
            if n.n_exp as f64 >= lim {
                continue;
            }
            let val = t.rel(v, best) - pen + c * (ln_n / (1 + n.n_exp) as f64).sqrt();
            if top.is_none_or(|(tv, _)| val >= tv) {
                top = Some((val, n.id)); // ties -> newest attempt
            }
        }
        top.map(|(_, id)| id)
    }
}

/// A policy family implemented by any program. git-dream starts `command` once
/// (via `sh -c`) and, for every decision, writes one JSON line to its stdin:
///
/// ```json
/// {"params": {...}, "seed": 7, "tree": {"goal": "min", "budget": 20, "calls": 3,
///   "nodes": [{"id": 0, "parent": null, "k": null, "design": "...", "score": 1.5,
///              "status": "ok", "change": "...", "n_exp": 2}, ...]}}
/// ```
///
/// and reads one line back: the node id to continue, or `null` to end the round
/// (a bare value or `{"select": ...}`). It must be deterministic given its
/// input, or replay cannot reproduce live rounds.
#[derive(Debug)]
pub struct External {
    command: String,
    proc: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
    failed: bool,
}

impl External {
    fn start(&mut self) -> Result<()> {
        let mut child = crate::agents::sh_command(&self.command).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().with_context(|| format!("starting policy {:?}", self.command))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        self.proc = Some((child, stdin, stdout));
        Ok(())
    }

    fn ask(&mut self, req: &Value) -> Result<Option<usize>> {
        if self.proc.is_none() {
            self.start()?;
        }
        let (_, stdin, stdout) = self.proc.as_mut().unwrap();
        writeln!(stdin, "{req}")?;
        stdin.flush()?;
        let mut line = String::new();
        if stdout.read_line(&mut line)? == 0 {
            bail!("policy {:?} closed its output", self.command);
        }
        let v: Value = serde_json::from_str(line.trim()).with_context(|| format!("policy {:?} replied {line:?}", self.command))?;
        let v = v.get("select").cloned().unwrap_or(v);
        Ok(v.as_u64().map(|x| x as usize))
    }

    fn select(&mut self, spec: &Value, seed: u64, t: &Tree) -> Option<usize> {
        if self.failed {
            return None;
        }
        let nodes: Vec<Value> = t
            .nodes
            .iter()
            .map(|n| json!({"id": n.id, "parent": n.parent, "k": n.k, "design": n.design, "score": n.score, "status": n.status, "change": n.change, "n_exp": n.n_exp}))
            .collect();
        let req = json!({"params": spec["params"], "seed": seed, "tree": {"run": t.run, "round": t.round, "goal": t.goal.as_str(), "budget": t.budget, "calls": t.calls(), "nodes": nodes}});
        match self.ask(&req) {
            Ok(Some(id)) if id < t.nodes.len() => Some(id),
            Ok(Some(id)) => {
                eprintln!("git dream: policy {:?} selected unknown node {id}; ending the round", self.command);
                None
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("git dream: {e:#}; policy disabled");
                self.failed = true;
                None
            }
        }
    }
}

impl Drop for External {
    fn drop(&mut self) {
        if let Some((mut child, stdin, _)) = self.proc.take() {
            drop(stdin);
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip() {
        let reg = Registry::builtin();
        let fams = reg.searchable(&[]);
        let mut rng = Rng::new(1);
        for _ in 0..100 {
            let p = reg.sample(&mut rng, &fams);
            let q = reg.make(&serde_json::from_str(&p.spec().to_string()).unwrap()).unwrap();
            assert_eq!(p.repr(), q.repr());
        }
    }
}
