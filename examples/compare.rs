//! Prospective synthetic evaluation. See docs/validation.md for the protocol.
use anyhow::Result;
use clap::Parser;
use git_dream::agents::SpaceAgent;
use git_dream::policies::{Policy, Registry};
use git_dream::rng::{hash, Rng};
use git_dream::sim::{self, DreamArgs, Mode, World};
use git_dream::tree::{Goal, Node, Tree, OK};
use serde_json::json;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 100)]
    seeds: u64,
    #[arg(long, default_value_t = 10000)]
    first_seed: u64,
}

const BUDGET: usize = 24;
const ROUNDS: u32 = 4;
const TOTAL: usize = 1 + BUDGET * ROUNDS as usize;

fn agent(seed: u64) -> SpaceAgent {
    let vals = (0..32).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let knobs = toml::from_str(&format!("x=[{vals}]\ny=[{vals}]")).unwrap();
    let start = toml::from_str("x=0\ny=0").unwrap();
    SpaceAgent::new(&knobs, &start, &seed.to_string(), "params.json", 0.3, 0.2).unwrap()
}

fn score(task: &str, x: f64, y: f64) -> f64 {
    let bowl = (x - 23.0).powi(2) + (y - 9.0).powi(2);
    match task {
        "smooth" => 1.0 + bowl,
        "rugged" => 1.0 + bowl + 35.0 * ((x * 1.7).sin().powi(2) + (y * 1.3).sin().powi(2)),
        "deceptive" => 1.0 + (20.0 + (x - 5.0).powi(2) + (y - 5.0).powi(2)).min(bowl),
        _ => unreachable!(),
    }
}

struct Surface<'a> {
    agent: &'a SpaceAgent,
    task: &'a str,
}
impl Surface<'_> {
    fn node(&self, design: String) -> Node {
        let q = self.agent.indices(&design).unwrap();
        Node::new(
            0,
            None,
            None,
            design,
            Some(score(self.task, q[0] as f64, q[1] as f64)),
            OK,
            "",
        )
    }
}
impl World for Surface<'_> {
    fn step(&mut self, t: &Tree, parent: usize, k: usize) -> Result<Node> {
        let (d, change) = self.agent.propose(&t.nodes[parent].design, k);
        let mut n = self.node(d);
        n.id = t.nodes.len();
        n.parent = Some(parent);
        n.k = Some(k);
        n.change = change;
        Ok(n)
    }
}

fn select(
    reg: &Registry,
    a: &SpaceAgent,
    hist: &[Tree],
    current: &Policy,
    mode: Mode,
    floor: usize,
    seed: u64,
) -> Result<Policy> {
    let Ok((train, test)) = sim::split_history(hist, 1) else {
        return Ok(current.clone());
    };
    let fams = reg.searchable(&[]);
    let ranking = sim::dream(
        current,
        &[],
        &DreamArgs {
            history: &train,
            budget: BUDGET,
            lam: 0.02,
            candidates: 60,
            min_calls: floor,
            seed,
            agent: Some(a),
            mode,
            normalize_root: false,
            registry: reg,
            families: &fams,
        },
    )?;
    let candidate = &ranking[0].0;
    let eval = |p| sim::replay(p, &test, BUDGET, 0.02, Some(a), mode, false, false);
    let (p, c) = (eval(candidate)?, eval(current)?);
    Ok(if p.j > c.j && p.miss_rate <= c.miss_rate {
        candidate
    } else {
        current
    }
    .clone())
}

fn search(
    reg: &Registry,
    a: &SpaceAgent,
    task: &str,
    mut p: Policy,
    adaptive: Option<Mode>,
    floor: usize,
) -> Result<Vec<f64>> {
    let mut world = Surface { agent: a, task };
    let mut root = world.node(a.root());
    let mut trace = vec![root.score.unwrap()];
    let mut hist = vec![];
    p.set_min_calls(floor);
    for round in 1..=ROUNDS {
        if let Some(mode) = adaptive {
            p = select(
                reg,
                a,
                &hist,
                &p,
                mode,
                floor,
                hash(&format!("{}|{round}", a.salt)),
            )?;
        }
        let t = sim::run_tree(
            &mut p,
            &mut world,
            &root,
            BUDGET,
            "prospective",
            round,
            Goal::Min,
            round as u64,
            &mut |_, _| {},
        )?;
        trace.extend(t.best_trace.iter().skip(1).map(|s| s.unwrap()));
        root = t.best().unwrap().clone();
        hist.push(t);
    }
    Ok(trace)
}

// Successive halving over eight independently initialized local-search trajectories.
// An arm's resource is actual objective evaluations, not a synthetic fidelity score.
fn allocation(a: &SpaceAgent, task: &str, seed: u64, halve: bool) -> Vec<f64> {
    let mut rng = Rng::new(seed);
    let mut world = Surface { agent: a, task };
    let mut trace = vec![world.node(a.root()).score.unwrap()];
    let mut arms = vec![];
    for i in 0..8 {
        let n = world.node(a.key(&[rng.below(32), rng.below(32)]));
        trace.push(trace.last().unwrap().min(n.score.unwrap()));
        let mut t = Tree::new("arm", i, TOTAL, Goal::Min, "refine_best");
        t.set_root(n);
        arms.push(t);
    }
    while trace.len() < TOTAL {
        let stages = if halve {
            arms.len().ilog2() as usize + 1
        } else {
            1
        };
        let per_arm = ((TOTAL - trace.len()) / (stages * arms.len())).max(1);
        for t in &mut arms {
            for _ in 0..per_arm {
                if trace.len() == TOTAL {
                    break;
                }
                let id = t.best().unwrap().id;
                let k = t.nodes[id].n_exp;
                t.nodes[id].n_exp += 1;
                let n = world.step(t, id, k).unwrap();
                t.add(n);
                trace.push(trace.last().unwrap().min(t.best().unwrap().score.unwrap()));
            }
        }
        if halve && arms.len() > 1 {
            arms.sort_by(|a, b| {
                a.best()
                    .unwrap()
                    .score
                    .unwrap()
                    .total_cmp(&b.best().unwrap().score.unwrap())
            });
            arms.truncate(arms.len().div_ceil(2));
        }
    }
    trace
}

fn main() -> Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.seeds > 0 && args.first_seed >= 100,
        "use positive seeds and first-seed >= 100 to separate training seeds 0..20"
    );
    let reg = Registry::builtin();
    let refine = reg.make(&json!({"family":"refine_best"}))?;
    for task in ["smooth", "rugged", "deceptive"] {
        // Train once on independent rounds. Strict replay avoids using the wrong
        // salt for cached proposals across training seeds. Freeze before testing.
        let mut train = vec![];
        for seed in 0..20 {
            let a = agent(seed);
            let mut world = Surface { agent: &a, task };
            let root = world.node(a.root());
            let mut p =
                reg.make(&json!({"family": if seed % 2 == 0 {"random"} else {"refine_best"}}))?;
            train.push(sim::run_tree(
                &mut p,
                &mut world,
                &root,
                BUDGET,
                "training",
                seed as u32 + 1,
                Goal::Min,
                seed + 1,
                &mut |_, _| {},
            )?);
        }
        let frozen = select(&reg, &agent(0), &train, &refine, Mode::Strict, BUDGET, 42)?;
        println!(
            "{}",
            json!({"type":"protocol", "task":task, "frozen":frozen.spec(), "training_evaluations":20*(BUDGET+1), "test_seeds":args.seeds, "first_seed":args.first_seed, "evaluation_budget":TOTAL})
        );
        let optimum = (0..32)
            .flat_map(|x| (0..32).map(move |y| score(task, x as f64, y as f64)))
            .min_by(f64::total_cmp)
            .unwrap();
        for seed in args.first_seed..args.first_seed + args.seeds {
            let a = agent(seed);
            let mut rows = vec![];
            for (name, family) in [
                ("refine_best", "refine_best"),
                ("fixed_ucb", "ucb"),
                ("random_tree", "random"),
            ] {
                let p = reg.make(&json!({"family":family, "params":{"c":0.05}}))?;
                rows.push((name, search(&reg, &a, task, p, None, BUDGET)?));
            }
            rows.push((
                "frozen",
                search(&reg, &a, task, frozen.clone(), None, BUDGET)?,
            ));
            for (name, mode, floor) in [
                ("adaptive_cache", Mode::Cache, BUDGET),
                ("adaptive_strict", Mode::Strict, BUDGET),
                ("adaptive_stopping", Mode::Cache, 6),
            ] {
                rows.push((
                    name,
                    search(&reg, &a, task, refine.clone(), Some(mode), floor)?,
                ));
            }
            let mut rng = Rng::new(seed);
            let mut uniform = vec![Surface { agent: &a, task }.node(a.root()).score.unwrap()];
            while uniform.len() < TOTAL {
                uniform.push(uniform.last().unwrap().min(score(
                    task,
                    rng.below(32) as f64,
                    rng.below(32) as f64,
                )));
            }
            rows.push(("uniform_random", uniform));
            rows.push(("successive_halving", allocation(&a, task, seed, true)));
            rows.push(("equal_allocation", allocation(&a, task, seed, false)));
            for (name, trace) in rows {
                anyhow::ensure!(
                    name == "adaptive_stopping" || trace.len() == TOTAL,
                    "unequal budget for {name}"
                );
                println!(
                    "{}",
                    json!({"type":"result", "task":task, "seed":seed, "method":name, "evaluations":trace.len(), "best":trace.last(), "regret":trace.last().unwrap()-optimum, "trace":trace})
                );
            }
        }
    }
    Ok(())
}
