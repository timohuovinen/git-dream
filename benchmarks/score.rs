//! Score a compare.rs JSONL output for the policy track (lower is better).
//!
//! score = mean over (task, method) of log(1 + mean regret) for the adaptive arms.
//! Fails (exit 3) if any fixed-budget arm did not spend exactly the protocol budget,
//! or if seeds are missing/duplicated: a cheaper or partial run must not look better.
//!
//!   cargo run --release --locked --example score -- compare.jsonl
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const ARMS: [&str; 2] = ["adaptive_cache", "adaptive_strict"];
const EQUAL_BUDGET: [&str; 4] = ["adaptive_cache", "adaptive_strict", "refine_best", "successive_halving"];

fn reject(why: String) -> ! {
    eprintln!("{why}");
    std::process::exit(3)
}

/// `x` rounded to `places` decimals, ties resolved on its exact binary value.
fn round(x: f64, places: usize) -> f64 {
    format!("{x:.places$}").parse().unwrap()
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: score COMPARE.jsonl")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let mut proto: BTreeMap<String, Value> = BTreeMap::new();
    let mut res: HashMap<(String, String), Vec<Value>> = HashMap::new();
    let mut seen = HashSet::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let r: Value = serde_json::from_str(line).with_context(|| format!("{path}:{}", n + 1))?;
        let task = r["task"].as_str().unwrap_or_default().to_string();
        match r["type"].as_str() {
            Some("protocol") => {
                proto.insert(task, r);
            }
            Some("result") => {
                let method = r["method"].as_str().unwrap_or_default().to_string();
                if !seen.insert((task.clone(), method.clone(), r["seed"].to_string())) {
                    reject(format!("duplicate seed {} for {task}/{method}", r["seed"]));
                }
                res.entry((task, method)).or_default().push(r);
            }
            _ => {}
        }
    }
    if !proto.keys().map(String::as_str).eq(["deceptive", "rugged", "smooth"]) {
        reject(format!("unexpected task set {:?}", proto.keys().collect::<Vec<_>>()));
    }

    let mut metrics = Map::new();
    let mut terms = Vec::new();
    for (task, p) in &proto {
        let want = p["test_seeds"].as_u64().with_context(|| format!("{task}: no test_seeds"))? as usize;
        for method in EQUAL_BUDGET {
            let got = res.get(&(task.clone(), method.to_string())).map_or(&[][..], Vec::as_slice);
            if got.len() != want {
                reject(format!("{task}/{method}: {} seeds, expected {want}", got.len()));
            }
            let budget = p["evaluation_budget"].as_f64();
            let bad: Vec<&Value> = got.iter().filter(|r| r["evaluations"].as_f64() != budget).map(|r| &r["seed"]).take(5).collect();
            if !bad.is_empty() {
                reject(format!("{task}/{method}: unequal budget on seeds {bad:?}"));
            }
            let mut sum = 0.0;
            for r in got {
                sum += r["regret"].as_f64().with_context(|| format!("{task}/{method}: seed {} has no regret", r["seed"]))?;
            }
            let mean = sum / want as f64;
            metrics.insert(format!("{task}.{method}"), json!(round(mean, 4)));
            if ARMS.contains(&method) {
                terms.push(mean.ln_1p());
            }
        }
    }

    let mut out = Map::new();
    out.insert("score".into(), json!(round(terms.iter().sum::<f64>() / terms.len() as f64, 6)));
    out.insert("status".into(), json!("ok"));
    out.extend(metrics);
    println!("{}", Value::Object(out));
    Ok(())
}
