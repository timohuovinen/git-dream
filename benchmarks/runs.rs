//! Compare runs that `git dream` recorded with a real agent: what each strategy
//! found per attempt, and how much of what other policies would do is in its record.
//!
//! WORK holds one repository per run, named `<arm>-<repeat>`, as
//! examples/codec/evidence/run.sh leaves them, and optionally `usage/<arm>-<repeat>/*.json`
//! (one usage record per agent call: the last line of each file is a JSON object with
//! `duration_ms`, `usage` and, from Claude Code, `total_cost_usd`; examples/codec/agent.sh
//! writes them).
//!
//! For every run it reads the recorded rounds from git and reports per arm:
//!
//! * the best score after 1/4, 1/2, 3/4 and all of the attempts, in the order the
//!   attempts were chosen (mean and 95% interval over repeats, Student's t);
//! * the difference of each arm's final score from the first arm's (Welch interval);
//! * failed and unchanged attempts, agent cost and time;
//! * replay support: `refine_best` and `--pool` sampled policies are replayed in
//!   strict mode over the first `--replay-budget` attempts of the run's first
//!   round, with that many attempts each. A miss is an attempt the record does
//!   not contain. Reported: the pool's mean miss rate, the share of the pool that
//!   stays in the record (miss rate <= 5%), and what the replay winner is (the
//!   policy a replay dream would deploy), how many attempts it spends and how
//!   often it misses. Twice: with candidates free to stop after `--floor`
//!   attempts, and with every candidate made to spend the whole budget.
//!
//!   cargo run --release --example runs -- WORK [--out runs.jsonl]
use anyhow::{Context, Result};
use clap::Parser;
use git_dream::engine::Engine;
use git_dream::rng::Rng;
use git_dream::sim::{self, Mode};
use git_dream::tree::{Tree, NOOP, OK};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about = "Compare real-agent runs recorded by git dream (see the comment at the top of the source)")]
struct Args {
    /// Directory with one repository per run, named <arm>-<repeat>
    work: PathBuf,
    /// Sampled candidate policies per run for the replay-support check (refine_best is added)
    #[arg(long, default_value_t = 200)]
    pool: usize,
    /// Attempts a replayed candidate may spend
    #[arg(long, default_value_t = 24)]
    replay_budget: usize,
    /// Cost per attempt in the replay objective
    #[arg(long, default_value_t = 0.02)]
    lam: f64,
    /// Attempts a candidate must spend before it may stop (`[explore] min_calls`)
    #[arg(long, default_value_t = 4)]
    floor: usize,
    /// A policy is in support if its replay miss rate is at most this
    #[arg(long, default_value_t = 0.05)]
    support: f64,
    /// Write one JSON record per run here
    #[arg(long)]
    out: Option<PathBuf>,
}

struct Run {
    arm: String,
    rep: String,
    /// Best score after each attempt, in the order attempts were chosen (None: nothing scored yet).
    curve: Vec<Option<f64>>,
    start: f64,
    failed: usize,
    unchanged: usize,
    policies: Vec<(String, usize)>,
    cost: f64,
    agent_seconds: f64,
    calls: usize,
    /// Tokens read (cached ones included) and written by the agent.
    tokens: (f64, f64),
    /// Candidates may stop after `--floor` attempts; candidates spend the whole budget.
    support: [Support; 2],
}

struct Support {
    mean_miss: f64,
    in_support: f64,
    pick: String,
    pick_miss: f64,
    pick_calls: usize,
    incumbent: String,
}

/// Two-sided 95% critical value of Student's t.
fn t95(df: f64) -> f64 {
    const T: [(f64, f64); 14] = [(1.0, 12.706), (2.0, 4.303), (3.0, 3.182), (4.0, 2.776), (5.0, 2.571), (6.0, 2.447), (7.0, 2.365), (8.0, 2.306), (9.0, 2.262), (10.0, 2.228), (12.0, 2.179), (15.0, 2.131), (20.0, 2.086), (30.0, 2.042)];
    // the largest tabulated df not above `df`: never narrower than the truth
    T.iter().rev().find(|(d, _)| *d <= df).map_or(12.706, |(d, t)| if *d >= 30.0 && df >= 120.0 { 1.96 } else { *t })
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn var(v: &[f64]) -> f64 {
    let m = mean(v);
    v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() as f64 - 1.0).max(1.0)
}

/// Mean and half-width of its 95% interval.
fn interval(v: &[f64]) -> (f64, f64) {
    if v.len() < 2 {
        return (mean(v), f64::NAN);
    }
    (mean(v), t95(v.len() as f64 - 1.0) * (var(v) / v.len() as f64).sqrt())
}

/// Difference of means `a - b` and the half-width of its 95% interval (Welch).
fn welch(a: &[f64], b: &[f64]) -> (f64, f64) {
    if a.len() < 2 || b.len() < 2 {
        return (mean(a) - mean(b), f64::NAN);
    }
    let (va, vb) = (var(a) / a.len() as f64, var(b) / b.len() as f64);
    let se2 = va + vb;
    if se2 == 0.0 {
        return (mean(a) - mean(b), 0.0);
    }
    let df = se2 * se2 / (va * va / (a.len() as f64 - 1.0) + vb * vb / (b.len() as f64 - 1.0));
    (mean(a) - mean(b), t95(df.floor().max(1.0)) * se2.sqrt())
}

fn usage(dir: &Path) -> (f64, f64, usize, (f64, f64)) {
    let (mut cost, mut secs, mut calls, mut tokens) = (0.0, 0.0, 0, (0.0, 0.0));
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Some(v) = text.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok()) else { continue };
        cost += v["total_cost_usd"].as_f64().unwrap_or(0.0);
        secs += v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
        // Codex counts cached tokens inside input_tokens; Claude Code reports them beside it
        let u = |k: &str| v["usage"][k].as_f64().unwrap_or(0.0);
        tokens.0 += u("input_tokens") + u("cache_creation_input_tokens") + u("cache_read_input_tokens");
        tokens.1 += u("output_tokens");
        calls += 1;
    }
    (cost, secs, calls, tokens)
}

fn support(e: &Engine, first: &Tree, a: &Args, seed: u64, floor: usize) -> Result<Support> {
    let incumbent = e.registry.make(&json!({"family": "refine_best"}))?;
    let fams = e.registry.searchable(&[]);
    let mut rng = Rng::new(seed);
    let mut pool = vec![incumbent];
    pool.extend((0..a.pool).map(|_| e.registry.sample(&mut rng, &fams)));
    for p in &mut pool {
        p.set_min_calls(floor);
    }
    // the same amount of record for every arm: the first `replay_budget` attempts
    let mut record = first.clone();
    record.nodes.truncate(a.replay_budget + 1);
    record.best_trace.truncate(a.replay_budget + 1);
    let hist = std::slice::from_ref(&record);
    let scored = git_dream::par::map(git_dream::par::threads(), &pool, |_, p| sim::replay(p, hist, a.replay_budget, a.lam, None, Mode::Strict, false, false));
    let mut rows = vec![];
    for (p, r) in pool.iter().zip(scored) {
        rows.push((p.repr(), r?));
    }
    let n = rows.len() as f64;
    // the replay winner; the incumbent (index 0) wins ties, as in a dream
    let pick = rows.iter().enumerate().max_by(|(i, x), (j, y)| x.1.j.total_cmp(&y.1.j).then(j.cmp(i))).unwrap().1;
    Ok(Support {
        mean_miss: rows.iter().map(|r| r.1.miss_rate).sum::<f64>() / n,
        in_support: rows.iter().filter(|r| r.1.miss_rate <= a.support).count() as f64 / n,
        pick: pick.0.clone(),
        pick_miss: pick.1.miss_rate,
        pick_calls: pick.1.calls,
        incumbent: rows[0].0.clone(),
    })
}

fn load(dir: &Path, arm: &str, rep: &str, a: &Args) -> Result<Run> {
    let e = Engine::open(dir, None, None).with_context(|| format!("opening {}", dir.display()))?;
    let hist = e.mem.history(&e.run)?;
    anyhow::ensure!(!hist.is_empty(), "{} has no recorded rounds", dir.display());
    let start = hist[0].root().score.context("the first root has no score")?;
    let (mut curve, mut best, mut failed, mut unchanged) = (vec![], None::<f64>, 0, 0);
    for t in &hist {
        for n in &t.nodes[1..] {
            match (n.status.as_str(), n.score) {
                (OK, Some(s)) => {
                    if best.is_none_or(|b| t.cost(s) < t.cost(b)) {
                        best = Some(s);
                    }
                }
                (NOOP, _) => unchanged += 1,
                _ => failed += 1,
            }
            curve.push(best);
        }
    }
    let (cost, agent_seconds, calls, tokens) = usage(&a.work.join("usage").join(format!("{arm}-{rep}")));
    let seed = git_dream::rng::hash(&format!("{arm}|{rep}"));
    Ok(Run { arm: arm.into(), rep: rep.into(), curve, start, failed, unchanged, policies: hist.iter().map(|t| (t.policy.clone(), t.calls())).collect(), cost, agent_seconds, calls, tokens, support: [support(&e, &hist[0], a, seed, a.floor)?, support(&e, &hist[0], a, seed, a.replay_budget)?] })
}

fn main() -> Result<()> {
    let a = Args::parse();
    let mut runs = vec![];
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&a.work)?.flatten().map(|e| e.path()).filter(|p| p.join(".git-dream.toml").exists()).collect();
    dirs.sort();
    for d in dirs {
        let name = d.file_name().unwrap().to_string_lossy().into_owned();
        let Some((arm, rep)) = name.rsplit_once('-') else { continue };
        match load(&d, arm, rep, &a) {
            Ok(r) => runs.push(r),
            Err(e) => eprintln!("skipping {name}: {e:#}"),
        }
    }
    anyhow::ensure!(!runs.is_empty(), "no runs in {}", a.work.display());
    if let Some(out) = &a.out {
        let lines: Vec<String> = runs
            .iter()
            .map(|r| {
                let s: Vec<Value> = r.support.iter().zip(["free_to_stop", "whole_budget"]).map(|(s, k)| json!({"candidates": k, "mean_miss": s.mean_miss, "in_support": s.in_support, "pick": s.pick, "pick_miss": s.pick_miss, "pick_calls": s.pick_calls, "incumbent": s.incumbent})).collect();
                json!({"arm": r.arm, "repeat": r.rep, "start": r.start, "best_after_attempt": r.curve, "failed": r.failed, "unchanged": r.unchanged, "rounds": r.policies.iter().map(|(p, c)| json!({"policy": p, "attempts": c})).collect::<Vec<_>>(), "agent_calls": r.calls, "agent_cost_usd": r.cost, "agent_seconds": r.agent_seconds, "agent_tokens_in": r.tokens.0, "agent_tokens_out": r.tokens.1, "replay_support": s}).to_string()
            })
            .collect();
        std::fs::write(out, lines.join("\n") + "\n")?;
    }

    let mut arms: BTreeMap<String, Vec<&Run>> = BTreeMap::new();
    for r in &runs {
        arms.entry(r.arm.clone()).or_default().push(r);
    }
    let total = runs.iter().map(|r| r.curve.len()).min().unwrap();
    let marks: Vec<usize> = [4, 2].iter().map(|d| total / d).chain([total * 3 / 4, total]).filter(|m| *m > 0).collect();
    let at = |r: &Run, n: usize| r.curve[n - 1].unwrap_or(r.start);
    let pm = |(m, h): (f64, f64)| format!("{m:.0} ± {h:.0}");

    println!("best score after n attempts (mean ± 95% interval over repeats; start {:.0}; every run has at least {total} attempts)", runs[0].start);
    println!("| arm | repeats | {} |", marks.iter().map(|m| format!("{m} attempts")).collect::<Vec<_>>().join(" | "));
    println!("|---|---:|{}", "---:|".repeat(marks.len()));
    for (arm, rs) in &arms {
        let cols: Vec<String> = marks.iter().map(|&m| pm(interval(&rs.iter().map(|r| at(r, m)).collect::<Vec<_>>()))).collect();
        println!("| {arm} | {} | {} |", rs.len(), cols.join(" | "));
    }

    println!("\nfinal score against each other arm (row minus column, mean difference ± 95% Welch interval; below zero: the row is better)");
    let names: Vec<&String> = arms.keys().collect();
    println!("| | {} |", names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(" | "));
    println!("|---|{}", "---:|".repeat(names.len()));
    let finals = |arm: &str| arms[arm].iter().map(|r| at(r, total)).collect::<Vec<f64>>();
    for x in &names {
        let cols: Vec<String> = names.iter().map(|y| if x == y { "—".into() } else { let (d, h) = welch(&finals(x), &finals(y)); format!("{d:+.0} ± {h:.0}") }).collect();
        println!("| {x} | {} |", cols.join(" | "));
    }

    println!("\nper run");
    println!("| arm | final scores, sorted | failed | unchanged | agent cost | agent minutes | tokens in (M) | tokens out (M) |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|");
    for (arm, rs) in &arms {
        let mut f = finals(arm);
        f.sort_by(f64::total_cmp);
        let n: f64 = rs.iter().map(|r| r.curve.len() as f64).sum();
        println!(
            "| {arm} | {} | {:.0}% | {:.0}% | ${:.2} | {:.0} | {:.1} | {:.2} |",
            f.iter().map(|x| format!("{x:.0}")).collect::<Vec<_>>().join(" "),
            100.0 * rs.iter().map(|r| r.failed as f64).sum::<f64>() / n,
            100.0 * rs.iter().map(|r| r.unchanged as f64).sum::<f64>() / n,
            rs.iter().map(|r| r.cost).sum::<f64>(),
            rs.iter().map(|r| r.agent_seconds).sum::<f64>() / 60.0,
            rs.iter().map(|r| r.tokens.0).sum::<f64>() / 1e6,
            rs.iter().map(|r| r.tokens.1).sum::<f64>() / 1e6
        );
    }

    for (i, what) in [format!("candidates may stop after {} attempts", a.floor), "candidates spend the whole budget".to_string()].iter().enumerate() {
        println!("\nreplay support of the first {} recorded attempts of each run (strict replay, {} attempts per candidate, refine_best + {} sampled policies; {what})", a.replay_budget, a.replay_budget, a.pool);
        println!("| arm | recorded by | pool's mean miss rate | pool in support | replay winner's miss rate | replay winner's attempts | winner is not refine_best |");
        println!("|---|---|---:|---:|---:|---:|---:|");
        for (arm, rs) in &arms {
            let s: Vec<&Support> = rs.iter().map(|r| &r.support[i]).collect();
            let col = |f: &dyn Fn(&Support) -> f64| mean(&s.iter().map(|x| f(x)).collect::<Vec<_>>());
            println!(
                "| {arm} | {} | {:.0}% | {:.0}% | {:.0}% | {:.1} | {}/{} |",
                rs[0].policies[0].0,
                100.0 * col(&|x| x.mean_miss),
                100.0 * col(&|x| x.in_support),
                100.0 * col(&|x| x.pick_miss),
                col(&|x| x.pick_calls as f64),
                s.iter().filter(|x| x.pick != x.incumbent).count(),
                s.len()
            );
        }
    }

    println!("\nrounds (policy deployed, attempts spent)");
    for (arm, rs) in &arms {
        for r in rs {
            println!("  {arm}-{}: {}", r.rep, r.policies.iter().map(|(p, c)| format!("{p} {c}")).collect::<Vec<_>>().join("; "));
        }
    }
    Ok(())
}
