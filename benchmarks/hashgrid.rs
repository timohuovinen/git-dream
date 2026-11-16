//! Prospective comparison on a measured landscape: the 982 hash-table designs of
//! the hashbench case study (examples/hashbench/evidence/oracle.jsonl), of which
//! about 4 in 10 fail ("full"). Every arm spends the same number of evaluations
//! from the same start; each evaluation gets fresh multiplicative noise.
//! Reported: how far the true (noise-free) score of the design each arm ends on
//! is from the best design on the grid.
//!
//! A second table runs several attempts at once (`[explore] parallel`): the same
//! evaluations in fewer waves, each wave chosen without its own results.
//!
//!   cargo run --release --example hashgrid -- --seeds 500 > hashgrid.jsonl
use anyhow::Result;
use clap::Parser;
use git_dream::agents::SpaceAgent;
use git_dream::policies::Registry;
use git_dream::rng::Rng;
use git_dream::sim::{self, World};
use git_dream::tree::{Goal, Node, Tree, OK};
use serde_json::{json, Value};

#[path = "grid.rs"]
mod grid;
use grid::{agent_key, oracle_key};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 100)]
    seeds: u64,
    #[arg(long, default_value_t = 50000)]
    first_seed: u64,
    #[arg(long, default_value = grid::ORACLE)]
    oracle: String,
    /// Relative standard deviation of the measurement noise.
    #[arg(long, default_value_t = 0.06)]
    noise: f64,
    /// Evaluations per arm (the root is extra).
    #[arg(long, default_value_t = 320)]
    total: usize,
}

struct Grid<'a> {
    agent: &'a SpaceAgent,
    oracle: &'a grid::Oracle,
    noise: f64,
    rng: Rng,
}

impl Grid<'_> {
    fn node(&mut self, design: String) -> Node {
        let key = oracle_key(self.agent, &design);
        match self.oracle[&key] {
            Some(s) => Node::new(0, None, None, key, Some(grid::noisy(s, self.noise, &mut self.rng)), OK, ""),
            None => Node::new(0, None, None, key, None, "full", ""),
        }
    }
}

impl World for Grid<'_> {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
        let (d, change) = self.agent.propose(&agent_key(self.agent, &t.nodes[parent].design), k);
        let mut n = self.node(d);
        n.id = t.nodes.len();
        n.parent = Some(parent);
        n.k = Some(k);
        n.change = change;
        Ok(n)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let oracle = grid::load(&args.oracle)?;
    let optimum = oracle.values().flatten().cloned().min_by(f64::total_cmp).unwrap();
    let reg = Registry::builtin();
    let repair = json!({"family": "repair", "params": {"c": 0, "fp": 0.01, "kmax": 6, "kfail": 2, "patience": "inf", "eps": 0, "dedup": true}});
    // (method, policy, refit the yield model before each round)
    let arms = [
        ("refine_best", json!({"family": "refine_best"}), false),
        ("repair", repair, false),
        ("bandit", json!({"family": "bandit"}), false),
        ("bandit_dream", json!({"family": "bandit"}), true),
    ];
    let mut summary: Vec<(String, Vec<f64>)> = vec![];
    let seeds: Vec<u64> = (args.first_seed..args.first_seed + args.seeds).collect();
    // One seed's run: `rounds` rounds of `budget` evaluations, `lanes` attempts in
    // flight at once (waves: the policy picks `lanes` attempts before seeing any
    // of their results). Seeds are independent, so they run on every core;
    // results are kept in seed order.
    let run = |method: &str, spec: &Value, dream: bool, rounds: u32, budget: usize, lanes: usize| -> Result<Vec<f64>> {
        let rows = git_dream::par::map(git_dream::par::threads(), &seeds, |_, &seed| -> Result<(Value, f64)> {
            let a = grid::agent(&seed.to_string())?;
            let mut world = Grid { agent: &a, oracle: &oracle, noise: args.noise, rng: Rng::new(seed) };
            let mut root = world.node(a.root());
            let mut p = reg.make(spec)?;
            let mut hist: Vec<Tree> = vec![];
            let (mut evaluations, mut waves) = (0, 0);
            for round in 1..=rounds {
                if dream {
                    p = git_dream::model::learn(&p, &hist).unwrap().0;
                }
                let t = if lanes == 1 {
                    sim::run_tree(&mut p, &mut world, &root, budget, "hashgrid", round, Goal::Min, round as u64, &mut |_, _| {})?
                } else {
                    sim::run_waves(&mut p, &mut world, &root, budget, "hashgrid", round, Goal::Min, round as u64, lanes)?
                };
                evaluations += t.calls();
                waves += t.calls().div_ceil(lanes);
                root = t.best().unwrap().clone();
                hist.push(t);
            }
            let gap = oracle[&root.design].unwrap() / optimum - 1.0;
            let mut row = json!({"type": "result", "rounds": rounds, "budget": budget, "seed": seed, "method": method, "evaluations": evaluations, "design": root.design, "gap": gap});
            if lanes > 1 || budget * rounds as usize != args.total {
                row["lanes"] = json!(lanes);
                row["waves"] = json!(waves);
            }
            Ok((row, gap))
        });
        let mut gaps = vec![];
        for r in rows {
            let (row, gap) = r?;
            println!("{row}");
            gaps.push(gap);
        }
        Ok(gaps)
    };
    // rounds per run: one tree of the whole budget is what `[explore] extend` produces
    for rounds in [1u32, 8, 40] {
        let budget = args.total / rounds as usize;
        for (method, spec, dream) in &arms {
            if rounds == 1 && *dream {
                continue;
            }
            summary.push((format!("{rounds:2} x {budget:3}  {method}"), run(method, spec, *dream, rounds, budget, 1)?));
        }
    }
    // Attempts at once (`[explore] parallel`), one round. A wave takes the time of
    // one attempt, so `lanes` at once spend the same evaluations in 1/lanes of the
    // time, choosing each wave without the results of the attempts in it.
    let lanes_arms = [("refine_best", json!({"family": "refine_best"})), ("bandit", json!({"family": "bandit"}))];
    let mut timed: Vec<(String, Vec<f64>)> = vec![];
    for lanes in [4usize, 8, 16] {
        for (method, spec) in &lanes_arms {
            timed.push((format!("{:3} waves of {lanes:2}  {method}", args.total.div_ceil(lanes)), run(&format!("{method}_x{lanes}"), spec, false, 1, args.total, lanes)?));
        }
        // the same time spent one attempt at a time
        let short = args.total / lanes;
        timed.push((format!("{short:3} waves of  1  bandit"), run(&format!("bandit_{short}"), &lanes_arms[1].1, false, 1, short, 1)?));
    }
    let print = |rows: &[(String, Vec<f64>)]| {
        for (name, g) in rows {
            let n = g.len() as f64;
            let m = g.iter().sum::<f64>() / n;
            let se = (g.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0).max(1.0) / n).sqrt();
            eprintln!("  {name:28} {:7.2}% ± {:.2}", 100.0 * m, 196.0 * se);
        }
    };
    eprintln!("gap to the best design after {} evaluations (mean, 95% interval over {} seeds, noise {}):", args.total, args.seeds, args.noise);
    print(&summary);
    eprintln!("attempts at once, one round (a wave takes the time of one attempt; {} evaluations unless the row says fewer waves of 1):", args.total);
    print(&timed);
    Ok(())
}
