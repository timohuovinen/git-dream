//! The measured landscape of the hashbench case study, shared by hashgrid.rs
//! and fidelity.rs: 982 hash-table designs (examples/hashbench/evidence/oracle.jsonl),
//! of which about 4 in 10 fail ("full"), and the parameter-space agent that walks them.
#![allow(dead_code)]
use anyhow::Result;
use git_dream::agents::SpaceAgent;
use git_dream::rng::Rng;
use serde_json::Value;
use std::collections::HashMap;

pub const ORACLE: &str = "examples/hashbench/evidence/oracle.jsonl";

/// Design (the oracle's own key) -> measured score, None if the table overflowed.
pub type Oracle = HashMap<String, Option<f64>>;

pub fn load(path: &str) -> Result<Oracle> {
    let mut oracle = Oracle::new();
    for l in std::fs::read_to_string(path)?.lines() {
        let v: Value = serde_json::from_str(l)?;
        oracle.entry(v["design"].as_str().unwrap().to_string()).or_insert(v["score"].as_f64());
    }
    Ok(oracle)
}

/// The agent of examples/hashbench/.git-dream.toml; `salt` makes its proposals differ per replicate.
pub fn agent(salt: &str) -> Result<SpaceAgent> {
    let knobs = toml::from_str("W=[16,64]\nb=[1,2]\nL=[0,1,2,3,4,5,6,8]\nr=[0.25,0.375,0.5,0.625,0.75]\no=[0.005,0.01,0.02,0.03,0.05,0.08,0.12]")?;
    let start = toml::from_str("W=16\nb=1\nL=0\nr=0.5\no=0.05")?;
    SpaceAgent::new(&knobs, &start, salt, "params.json", 0.3, 0.2)
}

/// The agent's design key -> the oracle's (with no levels, b / r / o are inert).
pub fn oracle_key(a: &SpaceAgent, design: &str) -> String {
    let q = a.indices(design).unwrap();
    let v: Vec<String> = (0..5).map(|i| a.values[i][q[i]].show()).collect();
    if v[2] == "0" {
        format!("W{}-b1-L0-r0.5-o0.05", v[0])
    } else {
        format!("W{}-b{}-L{}-r{}-o{}", v[0], v[1], v[2], v[3], v[4])
    }
}

pub fn agent_key(a: &SpaceAgent, key: &str) -> String {
    a.names.iter().zip(key.split('-')).map(|(n, p)| format!("{n}={}", &p[1..])).collect::<Vec<_>>().join(",")
}

/// Offset of a retry in the agent's proposal index: larger than any budget, so
/// retries never collide with another proposal's first try.
const RETRY: usize = 1 << 20;

/// k-th proposal from `design` (an oracle key): (child's oracle key, change).
///
/// With no levels (`L = 0`) the knobs b, r and o do nothing, so a move of one of
/// them proposes the design it started from. The case study's agent never made
/// such a proposal, and neither does this one unless `inert_moves` is set: it
/// asks the tool's agent again (at most 32 times) until the design changes.
/// `inert_moves` gives the tool's agent as it is, which is what a real run of
/// examples/hashbench/.git-dream.toml does. With levels the two are identical.
pub fn propose(a: &SpaceAgent, design: &str, k: usize, inert_moves: bool) -> (String, String) {
    let from = agent_key(a, design);
    let mut out = (design.to_string(), "no-op".to_string());
    for j in 0..32 {
        let (d, change) = a.propose(&from, k + j * RETRY);
        out = (oracle_key(a, &d), change);
        if inert_moves || out.0 != design {
            break;
        }
    }
    out
}

/// One measurement of a true score `s` with relative noise `noise` (standard deviation).
pub fn noisy(s: f64, noise: f64, rng: &mut Rng) -> f64 {
    let u = rng.f64().max(1e-300);
    let z = (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * rng.f64()).cos();
    s * (1.0 + noise * z)
}
