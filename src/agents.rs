//! Discovery agents (what the policy steers but never changes) and the evaluator.
//!
//! * [`SpaceAgent`]: parameter search over a discrete grid. A pure function of
//!   (salt, parent design, k), so "what would the agent have proposed if node n
//!   had been continued once more" is always computable, and dreaming may reuse
//!   any earlier measurement of the same design.
//! * [`CommandAgent`]: any program that edits the checkout, typically an LLM
//!   coding agent. It runs in a scratch worktree at the parent attempt; what it
//!   leaves behind becomes the child attempt. It is stochastic, so dreaming can
//!   only reuse recorded decisions.

use crate::rng::{hash, Rng};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum KVal {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
}

impl KVal {
    pub fn from_toml(v: &toml::Value) -> Result<KVal> {
        Ok(match v {
            toml::Value::Integer(i) => KVal::Int(*i),
            toml::Value::Float(f) => KVal::Float(*f),
            toml::Value::String(s) => KVal::Str(s.clone()),
            toml::Value::Boolean(b) => KVal::Bool(*b),
            _ => bail!("knob values must be numbers, strings or booleans, got {v}"),
        })
    }
    pub fn show(&self) -> String {
        match self {
            KVal::Int(i) => i.to_string(),
            KVal::Float(f) => format!("{f}"),
            KVal::Str(s) => s.clone(),
            KVal::Bool(b) => b.to_string(),
        }
    }
    pub fn to_json(&self) -> Value {
        match self {
            KVal::Int(i) => json!(i),
            KVal::Float(f) => json!(f),
            KVal::Str(s) => json!(s),
            KVal::Bool(b) => json!(b),
        }
    }
}

#[derive(Debug)]
pub struct SpaceAgent {
    pub names: Vec<String>,
    pub values: Vec<Vec<KVal>>,
    pub start: Vec<usize>,
    pub salt: String,
    /// Path (in the attempt's tree) of the JSON file holding the parameters.
    pub file: String,
    pub p_two: f64,
    pub p_far: f64,
    cache: RefCell<HashMap<(String, usize), (String, String)>>,
}

impl SpaceAgent {
    pub fn new(knobs: &toml::Table, start: &toml::Table, salt: &str, file: &str, p_two: f64, p_far: f64) -> Result<SpaceAgent> {
        if knobs.is_empty() {
            bail!("[agent] kind = \"space\" needs an [agent.knobs] table");
        }
        let mut names = vec![];
        let mut values = vec![];
        for (k, v) in knobs {
            let arr = v.as_array().filter(|a| !a.is_empty());
            let Some(arr) = arr else { bail!("knob {k} must be a non-empty array of values") };
            names.push(k.clone());
            values.push(arr.iter().map(KVal::from_toml).collect::<Result<Vec<_>>>()?);
        }
        let mut st = vec![];
        for (i, k) in names.iter().enumerate() {
            st.push(match start.get(k) {
                Some(v) => {
                    let kv = KVal::from_toml(v)?;
                    match values[i].iter().position(|x| *x == kv || x.show() == kv.show()) {
                        Some(p) => p,
                        None => bail!("start value {k}={} is not one of the [agent.knobs] values", kv.show()),
                    }
                }
                None => values[i].len() / 2,
            });
        }
        for k in start.keys() {
            if !names.contains(k) {
                bail!("start value {k} is not a knob");
            }
        }
        Ok(SpaceAgent { names, values, start: st, salt: salt.into(), file: file.into(), p_two, p_far, cache: RefCell::new(HashMap::new()) })
    }

    pub fn key(&self, idx: &[usize]) -> String {
        self.names.iter().zip(idx).enumerate().map(|(i, (n, &j))| format!("{n}={}", self.values[i][j].show())).collect::<Vec<_>>().join(",")
    }

    pub fn indices(&self, key: &str) -> Result<Vec<usize>> {
        let mut out = vec![];
        let parts: Vec<&str> = key.split(',').collect();
        if parts.len() != self.names.len() {
            bail!("design {key:?} does not match the configured knobs");
        }
        for (i, part) in parts.iter().enumerate() {
            let (n, s) = part.split_once('=').unwrap_or((part, ""));
            if n != self.names[i] {
                bail!("design {key:?} does not match the configured knobs");
            }
            match self.values[i].iter().position(|v| v.show() == s) {
                Some(p) => out.push(p),
                None => bail!("design {key:?}: {n}={s} is not a configured value"),
            }
        }
        Ok(out)
    }

    pub fn root(&self) -> String {
        self.key(&self.start)
    }

    pub fn size(&self) -> usize {
        self.values.iter().map(Vec::len).product()
    }

    /// k-th proposal from `parent`: move 1 knob (2 with probability p_two) by
    /// +-1 grid step (+-2 with probability p_far). Returns (child key, change).
    pub fn propose(&self, parent: &str, k: usize) -> (String, String) {
        let ck = (parent.to_string(), k);
        if let Some(v) = self.cache.borrow().get(&ck) {
            return v.clone();
        }
        let out = self.propose_uncached(parent, k);
        self.cache.borrow_mut().insert(ck, out.clone());
        out
    }

    fn propose_uncached(&self, parent: &str, k: usize) -> (String, String) {
        let Ok(d) = self.indices(parent) else { return (parent.into(), "no-op".into()) };
        let mut rng = Rng::new(hash(&format!("{}|{parent}|{k}", self.salt)));
        let movable: Vec<usize> = (0..self.names.len()).filter(|&i| self.values[i].len() > 1).collect();
        for _ in 0..32 {
            if movable.is_empty() {
                break;
            }
            let mut c = d.clone();
            let n = if rng.f64() < self.p_two { 2 } else { 1 };
            for j in rng.sample(movable.len(), n) {
                let kn = movable[j];
                let len = self.values[kn].len() as i64;
                let dir = if rng.f64() < 0.5 { -1 } else { 1 };
                let step = dir * if rng.f64() < self.p_far { 2 } else { 1 };
                c[kn] = (c[kn] as i64 + step).clamp(0, len - 1) as usize;
            }
            if c != d {
                let change: Vec<String> = (0..self.names.len()).filter(|&i| c[i] != d[i]).map(|i| format!("{} {}->{}", self.names[i], self.values[i][d[i]].show(), self.values[i][c[i]].show())).collect();
                return (self.key(&c), change.join(", "));
            }
        }
        (parent.into(), "no-op".into())
    }

    pub fn params_json(&self, design: &str) -> Value {
        let idx = self.indices(design).unwrap_or_else(|_| self.start.clone());
        let m: Map<String, Value> = self.names.iter().enumerate().map(|(i, n)| (n.clone(), self.values[i][idx[i]].to_json())).collect();
        Value::Object(m)
    }

    pub fn env(&self, design: &str) -> Vec<(String, String)> {
        let idx = self.indices(design).unwrap_or_else(|_| self.start.clone());
        let mut e = vec![("DREAM_PARAMS".to_string(), self.params_json(design).to_string())];
        for (i, n) in self.names.iter().enumerate() {
            let var: String = n.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
            e.push((format!("DREAM_PARAM_{var}"), self.values[i][idx[i]].show()));
        }
        e
    }
}

#[derive(Debug)]
pub struct CommandAgent {
    pub command: String,
    pub prompt: String,
    pub timeout: f64,
}

#[derive(Debug)]
pub enum Agent {
    Space(SpaceAgent),
    Command(CommandAgent),
}

impl Agent {
    pub fn kind(&self) -> &'static str {
        match self {
            Agent::Space(_) => "space",
            Agent::Command(_) => "command",
        }
    }
    /// The deterministic agent, if this is one (replay may then predict its proposals).
    pub fn space(&self) -> Option<&SpaceAgent> {
        match self {
            Agent::Space(s) => Some(s),
            Agent::Command(_) => None,
        }
    }
    pub fn env(&self, design: &str) -> Vec<(String, String)> {
        self.space().map(|s| s.env(design)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
pub struct ShOut {
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub seconds: f64,
    pub stdout: String,
    pub stderr: String,
}

impl ShOut {
    pub fn exit_json(&self) -> Value {
        if self.timed_out {
            json!("timeout")
        } else {
            json!(self.exit)
        }
    }
}

/// Run `sh -c command` in its own process group; kill the whole group on timeout.
pub fn sh(command: &str, cwd: &Path, env: &[(String, String)], timeout: f64) -> Result<ShOut> {
    let t0 = Instant::now();
    let mut child = Command::new("sh").arg("-c").arg(command).current_dir(cwd).envs(env.iter().map(|(a, b)| (a, b))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn()?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let ho = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = so.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let he = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = se.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let deadline = Duration::from_secs_f64(timeout.max(0.001));
    let mut timed_out = false;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if t0.elapsed() > deadline {
            timed_out = true;
            let _ = Command::new("kill").arg("-KILL").arg(format!("-{}", child.id())).status();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(ShOut { exit: status.code(), timed_out, seconds: t0.elapsed().as_secs_f64(), stdout: ho.join().unwrap_or_default(), stderr: he.join().unwrap_or_default() })
}

pub fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The last non-empty stdout line: a number, or a JSON object with "score"
/// (and optionally "status" and anything else, kept as metrics).
pub fn parse_score(stdout: &str) -> (Option<f64>, Option<Map<String, Value>>) {
    let Some(line) = stdout.lines().rev().map(str::trim).find(|l| !l.is_empty()) else { return (None, None) };
    match serde_json::from_str::<Value>(line) {
        Ok(Value::Number(n)) => (n.as_f64(), Some(Map::new())),
        Ok(Value::Object(m)) => (m.get("score").and_then(Value::as_f64), Some(m)),
        _ => (None, None),
    }
}

#[derive(Clone, Debug)]
pub struct EvalResult {
    pub score: Option<f64>,
    pub status: String,
    /// Everything worth keeping (stored as the attempt's git note).
    pub note: Value,
}

pub struct Evaluator {
    pub command: String,
    pub timeout: f64,
}

impl Evaluator {
    pub fn run(&self, cwd: &Path, env: &[(String, String)]) -> Result<EvalResult> {
        let r = sh(&self.command, cwd, env, self.timeout)?;
        let (mut score, metrics) = parse_score(&r.stdout);
        let mstatus = metrics.as_ref().and_then(|m| m.get("status")).and_then(Value::as_str).map(String::from);
        let status = if r.timed_out || r.exit != Some(0) {
            score = None;
            if r.timed_out { "timeout".to_string() } else { crate::tree::FAIL.to_string() }
        } else if score.is_none() {
            mstatus.unwrap_or_else(|| crate::tree::FAIL.into())
        } else {
            let s = mstatus.unwrap_or_else(|| crate::tree::OK.into());
            if s != crate::tree::OK {
                score = None;
            }
            s
        };
        let status = if status == "timeout" { crate::tree::FAIL.to_string() } else { status };
        let note = json!({
            "score": score, "status": status, "metrics": metrics.map(Value::Object).unwrap_or(json!({})),
            "exit": r.exit_json(), "seconds": (r.seconds * 1000.0).round() / 1000.0,
            "stdout_tail": tail(&r.stdout, 40), "stderr_tail": tail(&r.stderr, 40),
        });
        Ok(EvalResult { score, status, note })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(salt: &str) -> SpaceAgent {
        let knobs: toml::Table = toml::from_str("a = [1, 2, 3, 4]\nb = [0.1, 0.25, 0.5]\nc = [\"x\", \"y\"]").unwrap();
        SpaceAgent::new(&knobs, &toml::Table::new(), salt, ".dream/params.json", 0.3, 0.2).unwrap()
    }

    #[test]
    fn deterministic_agent() {
        let (a, b) = (agent(""), agent(""));
        let r = a.root();
        assert_eq!(r, "a=3,b=0.25,c=y");
        let pa: Vec<_> = (0..10).map(|k| a.propose(&r, k)).collect();
        assert_eq!(pa, (0..10).map(|k| b.propose(&r, k)).collect::<Vec<_>>());
        assert_ne!(pa, (0..10).map(|k| agent("s").propose(&r, k)).collect::<Vec<_>>());
        assert!(pa.iter().all(|(d, _)| *d != r && a.indices(d).is_ok()));
        assert_eq!(a.key(&a.indices(&r).unwrap()), r);
        assert_eq!(a.params_json(&r), json!({"a": 3, "b": 0.25, "c": "y"}));
    }

    #[test]
    fn scores() {
        assert_eq!(parse_score("noise\n3.5\n\n").0, Some(3.5));
        assert_eq!(parse_score("{\"score\": 2, \"x\": 1}").0, Some(2.0));
        assert_eq!(parse_score("not a number").0, None);
        assert_eq!(parse_score("").0, None);
    }

    #[test]
    fn shell_timeout_kills_group() {
        let r = sh("sleep 5 & sleep 5; echo late", Path::new("."), &[], 0.2).unwrap();
        assert!(r.timed_out && r.seconds < 2.0 && r.stdout.is_empty());
    }
}
