//! Fidelity evaluator: can replay be trusted to choose a better policy?
//!
//! The case study checked this once, on one recorded history, and the check was
//! misleading in three ways:
//!
//! 1. One history, one candidate pool: its Spearman correlations (0.01 strict,
//!    0.10 cache) were single draws with no interval.
//! 2. Rank correlation over the whole pool is not the quantity dreaming uses.
//!    A replay dream deploys ONE policy, the replay argmax. What matters is
//!    whether that policy is truly better than the one already deployed.
//! 3. "The pick is in the top 12% of true quality" was reported without the
//!    incumbent's own percentile. The incumbent was in the same top 12%.
//!
//! What this does instead, with the tool's own replay (`sim::replay`), on the
//! measured hash-table grid (examples/hashbench/evidence/oracle.jsonl):
//!
//! for each of `--reps` independent histories and each way of recording one
//! (`--recorders`), it scores the incumbent pi_0 (`refine_best`) plus `--pool`
//! sampled policies three ways:
//!
//! * replay J, strict and cache mode (what a dream sees);
//! * true J on the same roots, noise-free, averaged over `--truth-seeds` policy seeds;
//! * true J from the NEXT round's root (what deploying the policy would give);
//!
//! and reports, with 95% intervals over histories:
//!
//! * decision quality: true J of the replay pick minus true J of the incumbent,
//!   wins / ties / losses, and the share of the available headroom (best policy
//!   in the pool minus the incumbent) that the pick captures;
//! * the percentile of the pick AND of the incumbent in the pool;
//! * ranking: Spearman with average ranks for ties, over the pool and over the
//!   in-support policies (miss rate <= `--support`) only;
//! * accuracy: |replay J - true J| in and out of support, and the miss rate;
//! * three selection rules: `charged` (a miss costs a call: what `sim::replay`
//!   reports), `free` (a miss costs nothing: the paper's rule), `gated` (charged,
//!   and the pick may not miss more than the incumbent).
//!
//! Verdict rule, fixed before looking at results: replay selection is
//! "trustworthy" for a (recorder, mode, rule) cell only if the 95% interval of
//! `pick - incumbent` on the next round's root lies above zero, over at least
//! 30 histories.
//!
//! Per-policy rows go to `<out>.jsonl`, so any other analysis can be done
//! without re-running; `--summarize <out>.jsonl` re-prints the report.
//!
//! `--live REPO` scores one real history instead: a run that `git dream` recorded
//! in a checkout of the hash-table crate with examples/hashbench/.git-dream.toml.
//! Replay then sees real commits and real measurement noise, and the oracle
//! still supplies the truth. One history gives no interval and no verdict.
//!
//!   cargo run --release --example fidelity -- --smoke       # seconds: checks the pipeline only
//!   cargo run --release --example fidelity -- --reps 100 --out benchmarks/evidence/fidelity/v1
//!   cargo run --release --example fidelity -- --live ../hashbench --run main
use anyhow::{bail, Result};
use clap::Parser;
use git_dream::agents::SpaceAgent;
use git_dream::policies::{Policy, Registry};
use git_dream::rng::{hash, Rng};
use git_dream::sim::{self, DreamArgs, Mode, World};
use git_dream::tree::{Goal, Node, Tree, MISS, OK};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[path = "grid.rs"]
mod grid;

const RULES: [&str; 3] = ["charged", "free", "gated"];
/// A verdict needs an interval that means something.
const MIN_HISTORIES: usize = 30;
/// How a history is recorded: the two arms of the case study's protocol (the
/// fixed policy; replay dreams deploying their argmax), and wide behaviour
/// policies (the paper records its first round with a wide grid).
const RECORDERS: [&str; 5] = ["fixed", "dream", "wide-softmax", "wide-ucb", "wide-beam"];

#[derive(Parser, Clone)]
#[command(about = "Can replay be trusted to choose a better policy? (see the comment at the top of the source)")]
struct Args {
    /// Independent histories per recorder
    #[arg(long, default_value_t = 100)]
    reps: u64,
    /// Sampled candidate policies per history (the incumbent is added)
    #[arg(long, default_value_t = 200)]
    pool: usize,
    #[arg(long, default_value_t = 6)]
    rounds: u32,
    #[arg(long, default_value_t = 40)]
    budget: usize,
    #[arg(long, default_value_t = 0.02)]
    lam: f64,
    /// Measurement noise while recording histories
    #[arg(long, default_value_t = 0.06)]
    noise: f64,
    /// Candidates per dream when recording the dream arm
    #[arg(long, default_value_t = 300)]
    dream_cands: usize,
    /// Policy seeds averaged for the true J
    #[arg(long, default_value_t = 3)]
    truth_seeds: u64,
    /// A policy is in support if its replay miss rate is at most this
    #[arg(long, default_value_t = 0.05)]
    support: f64,
    /// Comma-separated: fixed, dream, wide-softmax, wide-ucb, wide-beam
    #[arg(long, default_value = "fixed,dream,wide-softmax,wide-ucb,wide-beam")]
    recorders: String,
    #[arg(long, default_value = grid::ORACLE)]
    oracle: String,
    /// Write per-policy rows to <out>.jsonl
    #[arg(long)]
    out: Option<String>,
    /// 2 histories, 12 candidates, 3 rounds: checks the pipeline, proves nothing
    #[arg(long)]
    smoke: bool,
    /// Re-print the report from an earlier <out>.jsonl
    #[arg(long)]
    summarize: Option<String>,
    /// Score the history of a real run recorded by `git dream` in this repository
    /// (the hashbench example's knobs), with that run's budget and lam
    #[arg(long, value_name = "REPO")]
    live: Option<std::path::PathBuf>,
    /// --live: which run (default: the one named in the repository's config)
    #[arg(long)]
    run: Option<String>,
}

/// What replay says about one policy over one history.
#[derive(Serialize, Deserialize, Clone)]
struct Replayed {
    #[serde(rename = "J")]
    j: f64,
    /// J if a miss cost nothing.
    #[serde(rename = "J_free")]
    j_free: f64,
    miss: f64,
    calls: usize,
}

#[derive(Serialize, Deserialize)]
struct Row {
    policy: String,
    true_same: f64,
    true_next: f64,
    strict: Replayed,
    cache: Replayed,
}

impl Row {
    fn replayed(&self, mode: Mode) -> &Replayed {
        if mode == Mode::Strict { &self.strict } else { &self.cache }
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    rep: u64,
    recorder: String,
    trees: usize,
    rows: Vec<Row>,
}

/// The oracle as a world. Designs are the agent's keys, as in a real run of
/// examples/hashbench, so replay's cache sees exactly what the tool would.
struct Measured<'a> {
    agent: &'a SpaceAgent,
    oracle: &'a grid::Oracle,
    noise: f64,
    rng: Rng,
}

impl Measured<'_> {
    fn node(&mut self, design: String, change: String) -> Node {
        match self.oracle[&grid::oracle_key(self.agent, &design)] {
            Some(s) => Node::new(0, None, None, design, Some(if self.noise > 0.0 { grid::noisy(s, self.noise, &mut self.rng) } else { s }), OK, change),
            None => Node::new(0, None, None, design, None, "full", change),
        }
    }
}

impl World for Measured<'_> {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
        let (design, change) = self.agent.propose(&t.nodes[parent].design, k);
        let mut n = self.node(design, change);
        n.id = t.nodes.len();
        n.parent = Some(parent);
        n.k = Some(k);
        Ok(n)
    }
}

/// Everything one replicate needs.
struct Ctx<'a> {
    args: &'a Args,
    oracle: &'a grid::Oracle,
    reg: &'a Registry,
    agent: &'a SpaceAgent,
    salt: String,
}

impl Ctx<'_> {
    fn policy(&self, spec: serde_json::Value) -> Policy {
        self.reg.make(&spec).expect("built-in policy")
    }

    /// One recorded round: noisy measurements, the policy seeded by the round as in a live run.
    fn record(&self, policy: &Policy, root: &Node, tag: &str, round: u32) -> Result<Tree> {
        let mut world = Measured { agent: self.agent, oracle: self.oracle, noise: self.args.noise, rng: Rng::new(hash(&format!("{}|{tag}|{round}", self.salt))) };
        sim::run_tree(&mut policy.clone(), &mut world, root, self.args.budget, tag, round, Goal::Min, round as u64, &mut |_, _| {})
    }

    /// The next round starts from the best attempt, with the score that was measured for it.
    fn next_root(t: &Tree) -> Node {
        let b = t.best().expect("a round has a scored root");
        Node::new(0, None, None, b.design.clone(), b.score, OK, "")
    }

    /// One history per recorder (the fixed and dream arms share round 1).
    fn histories(&self, rep: u64, want: &[String]) -> Result<Vec<(String, Vec<Tree>)>> {
        let a = self.args;
        let mut world = Measured { agent: self.agent, oracle: self.oracle, noise: 0.0, rng: Rng::new(0) };
        let start = world.node(self.agent.root(), String::new());
        let pi0 = self.policy(json!({"family": "refine_best"}));
        let mut out = vec![];
        let arms: Vec<&str> = ["fixed", "dream"].into_iter().filter(|x| want.iter().any(|w| w == x)).collect();
        if !arms.is_empty() {
            let first = self.record(&pi0, &start, "shared", 1)?;
            for arm in arms {
                let (mut hist, mut pol) = (vec![first.clone()], pi0.clone());
                for round in 2..=a.rounds {
                    if arm == "dream" {
                        let fams = self.reg.searchable(&[]);
                        let d = DreamArgs { history: &hist, budget: a.budget, lam: a.lam, candidates: a.dream_cands, min_calls: 0, seed: rep * 100 + round as u64, agent: Some(self.agent), mode: Mode::Cache, normalize_root: false, registry: self.reg, families: &fams, threads: 1 };
                        pol = sim::dream(&pol, &[], &d)?.remove(0).0;
                    }
                    let t = self.record(&pol, &Self::next_root(hist.last().unwrap()), arm, round)?;
                    hist.push(t);
                }
                out.push((arm.to_string(), hist));
            }
        }
        let wide = [
            ("wide-softmax", json!({"family": "softmax", "params": {"T": 0.3, "kmax": "inf", "patience": "inf", "eps": 0}})),
            ("wide-ucb", json!({"family": "ucb", "params": {"c": 0.5, "kmax": "inf", "patience": "inf", "eps": 0}})),
            ("wide-beam", json!({"family": "beam", "params": {"m": 8, "kmax": "inf", "patience": "inf", "eps": 0}})),
        ];
        for (name, spec) in wide {
            if !want.iter().any(|w| w == name) {
                continue;
            }
            let pol = self.policy(spec);
            let (mut hist, mut root) = (vec![], start.clone());
            for round in 1..=a.rounds {
                let t = self.record(&pol, &root, name, round)?;
                root = Self::next_root(&t);
                hist.push(t);
            }
            out.push((name.to_string(), hist));
        }
        Ok(out)
    }

    /// Noise-free J of `policy` from the designs `roots`, averaged over policy seeds.
    fn true_j(&self, policy: &Policy, roots: &[String]) -> Result<f64> {
        let a = self.args;
        let mut total = 0.0;
        for s in 0..a.truth_seeds {
            let mut trees = vec![];
            for (i, d) in roots.iter().enumerate() {
                let mut world = Measured { agent: self.agent, oracle: self.oracle, noise: 0.0, rng: Rng::new(0) };
                let root = world.node(d.clone(), String::new());
                trees.push(sim::run_tree(&mut policy.clone(), &mut world, &root, a.budget, "truth", 1, Goal::Min, 1000 * s + i as u64, &mut |_, _| {})?);
            }
            total += sim::objective(&trees, a.budget, a.lam);
        }
        Ok(total / a.truth_seeds as f64)
    }

    fn replayed(&self, policy: &Policy, hist: &[Tree], mode: Mode) -> Result<Replayed> {
        let a = self.args;
        let r = sim::replay(policy, hist, a.budget, a.lam, Some(self.agent), mode, false, true)?;
        let free = |t: &Tree| sim::gain(t) - a.lam * (t.calls() - t.nodes.iter().filter(|n| n.status == MISS).count()) as f64 / a.budget as f64;
        Ok(Replayed { j: r.j, j_free: r.trees.iter().map(free).sum::<f64>() / r.trees.len().max(1) as f64, miss: r.miss_rate, calls: r.calls })
    }

    fn score_pool(&self, hist: &[Tree], pool: &[Policy]) -> Result<Vec<Row>> {
        let roots: Vec<String> = hist.iter().map(|t| t.root().design.clone()).collect();
        let next = [hist.last().unwrap().best().unwrap().design.clone()];
        pool.iter()
            .map(|c| Ok(Row { policy: c.repr(), true_same: self.true_j(c, &roots)?, true_next: self.true_j(c, &next)?, strict: self.replayed(c, hist, Mode::Strict)?, cache: self.replayed(c, hist, Mode::Cache)? }))
            .collect()
    }
}

fn one(rep: u64, args: &Args, oracle: &grid::Oracle, reg: &Registry, want: &[String]) -> Result<Vec<Record>> {
    let salt = format!("rep{rep}");
    let agent = grid::agent(&salt)?;
    let ctx = Ctx { args, oracle, reg, agent: &agent, salt };
    let mut rng = Rng::new(9000 + rep);
    let fams = reg.searchable(&[]);
    let mut pool = vec![ctx.policy(json!({"family": "refine_best"}))];
    pool.extend((0..args.pool).map(|_| reg.sample(&mut rng, &fams)));
    let mut out = vec![];
    for (recorder, hist) in ctx.histories(rep, want)? {
        out.push(Record { rep, recorder, trees: hist.len(), rows: ctx.score_pool(&hist, &pool)? });
    }
    Ok(out)
}

/// The one history a real run recorded in git, scored like a simulated one.
fn live(repo: &std::path::Path, args: &mut Args, oracle: &grid::Oracle, reg: &Registry) -> Result<Vec<Record>> {
    let e = git_dream::engine::Engine::open(repo, None, args.run.as_deref())?;
    let Some(agent) = e.agent.space() else { bail!("--live needs a run of the parameter-space agent (examples/hashbench/.git-dream.toml)") };
    anyhow::ensure!(agent.names == ["W", "b", "L", "r", "o"], "--live needs the hashbench knobs W, b, L, r, o in that order, found {:?}", agent.names);
    let hist = e.mem.history(&e.run)?;
    anyhow::ensure!(!hist.is_empty(), "run {:?} has no recorded rounds in {}", e.run, repo.display());
    for t in &hist {
        for n in &t.nodes {
            anyhow::ensure!(n.score.is_none() || agent.indices(&n.design).is_ok_and(|_| oracle.contains_key(&grid::oracle_key(agent, &n.design))), "design {:?} of round {} is not on the oracle grid", n.design, t.round);
        }
    }
    // what a dream in that repository would be given
    (args.budget, args.lam) = (e.cfg.explore.budget, e.cfg.dream.lam);
    let ctx = Ctx { args, oracle, reg, agent, salt: String::new() };
    let mut rng = Rng::new(7);
    let fams = reg.searchable(&[]);
    let mut pool = vec![ctx.policy(json!({"family": "refine_best"}))];
    pool.extend((0..args.pool).map(|_| reg.sample(&mut rng, &fams)));
    Ok(vec![Record { rep: 0, recorder: format!("live-{}", e.run), trees: hist.len(), rows: ctx.score_pool(&hist, &pool)? }])
}

// ---------------------------------------------------------------- analysis
fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn sd(v: &[f64]) -> f64 {
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

/// Ranks, ties sharing their average rank.
fn avg_ranks(v: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..v.len()).collect();
    order.sort_by(|&a, &b| v[a].total_cmp(&v[b]));
    let mut r = vec![0.0; v.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && v[order[j + 1]] == v[order[i]] {
            j += 1;
        }
        for &o in &order[i..=j] {
            r[o] = (i + j) as f64 / 2.0;
        }
        i = j + 1;
    }
    r
}

fn spearman(x: &[f64], y: &[f64]) -> Option<f64> {
    let distinct = |v: &[f64]| v.iter().any(|a| *a != v[0]);
    if x.len() < 10 || !distinct(x) || !distinct(y) {
        return None;
    }
    let (rx, ry) = (avg_ranks(x), avg_ranks(y));
    let (mx, my) = (mean(&rx), mean(&ry));
    let cov: f64 = rx.iter().zip(&ry).map(|(a, b)| (a - mx) * (b - my)).sum();
    let var = |r: &[f64], m: f64| r.iter().map(|a| (a - m).powi(2)).sum::<f64>();
    Some(cov / (var(&rx, mx) * var(&ry, my)).sqrt())
}

/// Index of the replay argmax; the incumbent (row 0) wins ties.
fn pick(rows: &[Row], mode: Mode, rule: &str) -> usize {
    let key = |r: &Row| if rule == "free" { r.replayed(mode).j_free } else { r.replayed(mode).j };
    let mut best = 0;
    for (i, r) in rows.iter().enumerate().skip(1) {
        if rule == "gated" && r.replayed(mode).miss > rows[0].replayed(mode).miss {
            continue;
        }
        if key(r) > key(&rows[best]) {
            best = i;
        }
    }
    best
}

fn percentile(rows: &[Row], i: usize, key: impl Fn(&Row) -> f64) -> f64 {
    rows.iter().filter(|r| key(r) <= key(&rows[i])).count() as f64 / rows.len() as f64
}

/// What is known about the pool itself, for one definition of the truth.
struct PoolStats {
    incumbent_pct: f64,
    mean_minus_incumbent: f64,
    headroom: f64,
}

struct RuleStats {
    switched: bool,
    d_same: f64,
    d_next: f64,
    pct_same: f64,
}

struct ModeStats {
    miss: f64,
    in_support: f64,
    rho: Option<f64>,
    rho_in_support: Option<f64>,
    err_in: Option<f64>,
    err_out: Option<f64>,
    rules: Vec<RuleStats>,
}

struct Analysis {
    same: PoolStats,
    next: PoolStats,
    modes: Vec<ModeStats>,
}

const MODES: [Mode; 2] = [Mode::Strict, Mode::Cache];

fn analyse(rec: &Record, support: f64) -> Analysis {
    let rows = &rec.rows;
    let pool = |key: fn(&Row) -> f64| PoolStats {
        incumbent_pct: percentile(rows, 0, key),
        mean_minus_incumbent: mean(&rows.iter().map(key).collect::<Vec<_>>()) - key(&rows[0]),
        headroom: rows.iter().map(key).fold(f64::NEG_INFINITY, f64::max) - key(&rows[0]),
    };
    let modes = MODES
        .iter()
        .map(|&mode| {
            let (ins, outs): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.replayed(mode).miss <= support);
            let js = |v: &[&Row]| v.iter().map(|r| r.replayed(mode).j).collect::<Vec<_>>();
            let truth = |v: &[&Row]| v.iter().map(|r| r.true_same).collect::<Vec<_>>();
            let err = |v: &[&Row]| (!v.is_empty()).then(|| mean(&v.iter().map(|r| (r.replayed(mode).j - r.true_same).abs()).collect::<Vec<_>>()));
            let all: Vec<&Row> = rows.iter().collect();
            ModeStats {
                miss: mean(&rows.iter().map(|r| r.replayed(mode).miss).collect::<Vec<_>>()),
                in_support: ins.len() as f64 / rows.len() as f64,
                rho: spearman(&js(&all), &truth(&all)),
                rho_in_support: spearman(&js(&ins), &truth(&ins)),
                err_in: err(&ins),
                err_out: err(&outs),
                rules: RULES
                    .iter()
                    .map(|rule| {
                        let i = pick(rows, mode, rule);
                        RuleStats { switched: i != 0, d_same: rows[i].true_same - rows[0].true_same, d_next: rows[i].true_next - rows[0].true_next, pct_same: percentile(rows, i, |r| r.true_same) }
                    })
                    .collect(),
            }
        })
        .collect();
    Analysis { same: pool(|r| r.true_same), next: pool(|r| r.true_next), modes }
}

/// Mean with a 95% interval over histories.
fn ci(v: &[f64]) -> String {
    match v.len() {
        0 => "      n/a      ".into(),
        1 => format!("{:+.4} (1 value)", v[0]),
        n => format!("{:+.4} ± {:.4}", mean(v), 1.96 * sd(v) / (n as f64).sqrt()),
    }
}

fn report(records: &[Record], support: f64) {
    let mut recorders: Vec<&str> = vec![];
    for r in records {
        if !recorders.contains(&r.recorder.as_str()) {
            recorders.push(&r.recorder);
        }
    }
    for recorder in recorders {
        let a: Vec<Analysis> = records.iter().filter(|r| r.recorder == recorder).map(|r| analyse(r, support)).collect();
        let n = a.len();
        let col = |f: &dyn Fn(&Analysis) -> f64| a.iter().map(f).collect::<Vec<f64>>();
        println!("\n=== history recorded by: {recorder}  ({n} histories)");
        if n < 2 {
            println!("    one history only: no interval can be given; treat every number below as a single draw");
        }
        println!("    incumbent pi_0 percentile in the pool (true J, same roots): {:.0}%", 100.0 * mean(&col(&|x| x.same.incumbent_pct)));
        println!("    headroom, best policy in pool minus pi_0:  same roots {}   next root {}", ci(&col(&|x| x.same.headroom)), ci(&col(&|x| x.next.headroom)));
        println!("    a random policy minus pi_0:                same roots {}   next root {}", ci(&col(&|x| x.same.mean_minus_incumbent)), ci(&col(&|x| x.next.mean_minus_incumbent)));
        for (mi, mode) in MODES.iter().enumerate() {
            let opt = |f: &dyn Fn(&ModeStats) -> Option<f64>| a.iter().filter_map(|x| f(&x.modes[mi])).collect::<Vec<f64>>();
            println!(
                "  -- {} replay: miss rate {:.0}%, {:.0}% of the pool in support (miss <= {:.0}%)",
                mode.as_str(),
                100.0 * mean(&col(&|x| x.modes[mi].miss)),
                100.0 * mean(&col(&|x| x.modes[mi].in_support)),
                100.0 * support
            );
            println!("     Spearman(replay J, true J): pool {}   in support {}", ci(&opt(&|m| m.rho)), ci(&opt(&|m| m.rho_in_support)));
            println!("     |replay J - true J|:        in support {}   out of support {}", ci(&opt(&|m| m.err_in)), ci(&opt(&|m| m.err_out)));
            for (ri, rule) in RULES.iter().enumerate() {
                for (label, next) in [("same roots", false), ("next root ", true)] {
                    let d = col(&|x| if next { x.modes[mi].rules[ri].d_next } else { x.modes[mi].rules[ri].d_same });
                    let (w, l) = (d.iter().filter(|x| **x > 1e-12).count(), d.iter().filter(|x| **x < -1e-12).count());
                    let mut line = format!("     {rule:8} pick - pi_0, {label}: {}   W/T/L {w}/{}/{l}", ci(&d), n - w - l);
                    if next {
                        let head = mean(&col(&|x| x.next.headroom));
                        let m = mean(&d);
                        let half = if n > 1 { 1.96 * sd(&d) / (n as f64).sqrt() } else { f64::INFINITY };
                        let verdict = if n < MIN_HISTORIES {
                            format!("no verdict below {MIN_HISTORIES} histories")
                        } else if m - half > 0.0 {
                            "TRUSTWORTHY".into()
                        } else if m + half < 0.0 {
                            "WORSE than keeping pi_0".into()
                        } else {
                            "no better than keeping pi_0".into()
                        };
                        line += &if head > 0.0 { format!("   captures {:+.0}% of headroom -> {verdict}", 100.0 * m / head) } else { format!("   -> {verdict}") };
                    }
                    println!("{line}");
                }
                let switched = a.iter().filter(|x| x.modes[mi].rules[ri].switched).count();
                println!("              switched in {switched}/{n}; pick's percentile {:.0}%", 100.0 * mean(&col(&|x| x.modes[mi].rules[ri].pct_same)));
            }
        }
    }
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    if let Some(path) = &args.summarize {
        let mut records = vec![];
        for l in std::fs::read_to_string(path)?.lines().filter(|l| l.contains("\"recorder\"")) {
            records.push(serde_json::from_str::<Record>(l)?);
        }
        report(&records, args.support);
        return Ok(());
    }
    if args.smoke {
        (args.reps, args.pool, args.rounds, args.dream_cands, args.truth_seeds) = (2, 12, 3, 20, 1);
    }
    let want: Vec<String> = args.recorders.split(',').filter(|r| !r.is_empty()).map(String::from).collect();
    if let Some(bad) = want.iter().find(|r| !RECORDERS.contains(&r.as_str())) {
        bail!("unknown recorder {bad:?} (known: {})", RECORDERS.join(", "));
    }
    anyhow::ensure!(args.reps > 0 && args.rounds > 0 && args.budget > 0 && args.truth_seeds > 0, "reps, rounds, budget and truth-seeds must be positive");
    let oracle = grid::load(&args.oracle)?;
    let reg = Registry::builtin();
    let mut records = vec![];
    if let Some(repo) = args.live.clone() {
        records = live(&repo, &mut args, &oracle, &reg)?;
    } else {
        // Histories are independent: one per core, results kept in replicate order.
        let reps: Vec<u64> = (0..args.reps).collect();
        let done = std::sync::atomic::AtomicU64::new(0);
        for r in git_dream::par::map(git_dream::par::threads(), &reps, |_, &rep| {
            let r = one(rep, &args, &oracle, &reg, &want);
            eprintln!("history {}/{}", done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, args.reps);
            r
        }) {
            records.extend(r?);
        }
    }
    records.sort_by_key(|r| (RECORDERS.iter().position(|x| *x == r.recorder), r.rep));
    if let Some(out) = &args.out {
        let header = json!({"args": {"reps": args.reps, "pool": args.pool, "rounds": args.rounds, "budget": args.budget, "lam": args.lam, "noise": args.noise, "dream_cands": args.dream_cands, "truth_seeds": args.truth_seeds, "support": args.support, "recorders": want, "oracle": args.oracle, "smoke": args.smoke, "live": args.live, "run": args.run}});
        let mut text = header.to_string() + "\n";
        for r in &records {
            text += &(serde_json::to_string(r)? + "\n");
        }
        std::fs::write(format!("{out}.jsonl"), text)?;
    }
    if args.smoke {
        println!("SMOKE RUN: 2 tiny histories. The numbers below only show that the pipeline works.");
    }
    report(&records, args.support);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_and_picks() {
        assert_eq!(avg_ranks(&[3.0, 1.0, 3.0, 2.0]), [2.5, 0.0, 2.5, 1.0]);
        let x: Vec<f64> = (0..12).map(f64::from).collect();
        let y: Vec<f64> = x.iter().map(|v| v * v).collect();
        assert!((spearman(&x, &y).unwrap() - 1.0).abs() < 1e-12);
        assert!((spearman(&x, &y.iter().map(|v| -v).collect::<Vec<_>>()).unwrap() + 1.0).abs() < 1e-12);
        assert!(spearman(&x, &[1.0; 12]).is_none() && spearman(&x[..5], &y[..5]).is_none());
        let row = |j: f64, j_free: f64, miss: f64, truth: f64| {
            let r = Replayed { j, j_free, miss, calls: 10 };
            Row { policy: String::new(), true_same: truth, true_next: truth, strict: r.clone(), cache: r }
        };
        // incumbent first; a tie keeps it; a miss is only forgiven by the `free` rule; the gate refuses more misses
        let rows = vec![row(0.5, 0.5, 0.0, 0.1), row(0.5, 0.6, 0.2, 0.3), row(0.7, 0.4, 0.1, 0.0)];
        assert_eq!((pick(&rows, Mode::Cache, "charged"), pick(&rows, Mode::Cache, "free"), pick(&rows, Mode::Cache, "gated")), (2, 1, 0));
        assert_eq!(pick(&rows[..2], Mode::Strict, "charged"), 0);
        assert_eq!(percentile(&rows, 0, |r| r.true_same), 2.0 / 3.0);
        assert_eq!(ci(&[]).trim(), "n/a");
        assert_eq!(ci(&[0.25]), "+0.2500 (1 value)");
    }
}
