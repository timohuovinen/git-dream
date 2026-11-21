//! `git dream`: Dream-RSI on git. Install the binary on PATH as `git-dream`
//! and git runs it as `git dream <command>`.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use git_dream::engine::{fs, short, Engine, CONFIG_NAME};
use git_dream::memory::{check_run, Memory, REFSPECS};
use git_dream::policies::{Param, Policy, INF};
use git_dream::sim::Mode;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const TEMPLATE_SPACE: &str = r#"# git-dream configuration (https://github.com/timohuovinen/git-dream)
run = "main"                      # name of the run (refs/dream/<run>/...)

[agent]
kind = "space"                    # deterministic parameter search
file = ".dream/params.json"       # each attempt commit = base tree + this file holding its parameters
salt = ""                         # change for a different (still reproducible) agent
start = { threads = 4, block = 64 }

[agent.knobs]                     # ordered candidate values; the agent moves 1-2 knobs by 1-2 steps
threads = [1, 2, 4, 8, 16]
block = [16, 32, 64, 128, 256, 512]

[evaluate]
# Runs in a scratch worktree checked out at the attempt. Parameters arrive as
# $DREAM_PARAMS (JSON), $DREAM_PARAM_<NAME> and in the file above.
# Last stdout line = the score: a number or {"score": x, ...}. Non-zero exit = failed attempt.
command = "./bench.sh"
goal = "min"                      # "min" or "max"
timeout = 600
reuse = false                     # true: never re-measure a design already measured in this run
remeasure_root = false            # true for noisy benchmarks: re-measure each round's root
confirm_top = 0                   # re-measure the top-k designs before promoting (noisy benchmarks)

[explore]
budget = 20                       # proposals per `git dream run` step (root and confirmation evaluations are extra)
extend = true                     # each step adds to the current round's tree, so every attempt stays
                                  # continuable; `git dream run --new-round` starts over from the best attempt
parallel = 1                      # attempts evaluated at once, each in its own worktree. Only takes effect with
evaluations = "alone"             # evaluations = "parallel": say so when the score does not depend on machine
                                  # load (a test count, an accuracy, a size). Never for a timing benchmark.

[dream]
history = "run"                   # "all": learn from, and show agents, every run in this repository
# A bandit policy is refitted from measured outcomes when a new round starts (the yield
# model). The settings below only apply to families tuned by replay (method = "replay"):
holdout = 1                       # reserve last round per run; keep incumbent until enough history
lam = 0.02                        # cost of spending the whole budget, in units of relative improvement
candidates = 300                  # sampled policies per dream
mode = "cache"                    # cache: reuse any recorded measurement of a design; strict: recorded edges only
normalize = "none"                # "root": rescale cached scores by each tree's root (machine drift)

[policies]
initial = { family = "bandit" }   # learns which kind of move pays; see `git dream policies`
families = []                     # restrict the replay search (default: all searchable families)
# Policies in any language: a program answering one JSON line per decision (see README).
# [policies.external.patient]
# command = "./target/release/patient-policy"
# grid = { p = [3, 5, 8] }

# An agent that rewrites the exploration policy itself while dreaming (new rounds, `git dream dream`).
# Each candidate is a commit holding the policy (policy.json: a built-in family and its parameters, or a
# program it wrote), scored by replay over the recorded attempts: no evaluations. See README, "Policies".
# [develop]
# command = 'claude -p "$DREAM_PROMPT" --permission-mode acceptEdits'
# attempts = 4                    # candidate policies per dream
"#;

const TEMPLATE_COMMAND: &str = r#"# git-dream configuration (https://github.com/timohuovinen/git-dream)
run = "main"

[agent]
kind = "command"                  # any program that edits the checkout, e.g. an LLM coding agent
# Runs in a scratch worktree at the parent attempt. Env: $DREAM_PROMPT (prompt below + where
# the attempt is + how to work), $DREAM_CONTEXT (file: every attempt so far, see [dream] history,
# with write-ups, results and failure output), $DREAM_MESSAGE_FILE (write the proposal there).
command = 'claude -p "$DREAM_PROMPT" --permission-mode acceptEdits'
prompt = """
Make `cargo bench` faster. Keep `cargo test` passing. Make one focused change.
"""
timeout = 1800

[evaluate]
command = "cargo test -q >/dev/null 2>&1 && ./bench.sh"   # last stdout line = score
goal = "min"
timeout = 900
remeasure_root = true
confirm_top = 2
protected = ["bench.sh", "tests"] # reject direct edits to the evaluation contract

[explore]
budget = 6                        # proposals per `git dream run` step; root and confirmation evaluations are extra
extend = true                     # each step adds to the current round's tree, so every attempt stays
                                  # continuable; `git dream run --new-round` starts over from the best attempt
parallel = 4                      # agents working at once, each in its own worktree ($DREAM_LANE = 0..3)
evaluations = "alone"             # what may run during an evaluation. "alone": nothing (timing benchmarks:
                                  # the agents finish, then each result is measured by itself); "serial": agents,
                                  # but no other evaluation; "parallel": anything (scores independent of load)

[dream]
history = "run"                   # "all": learn from, and show agents, every run in this repository
mode = "strict"                   # replay-tuned families only: a stochastic agent replays recorded edges only

[policies]
initial = { family = "bandit" }   # learns which kind of move pays; see `git dream policies`

# An agent that rewrites the exploration policy itself while dreaming (new rounds, `git dream dream`).
# Each candidate is a commit holding the policy (policy.json: a built-in family and its parameters, or a
# program it wrote), scored by replay over the recorded attempts: no evaluations. See README, "Policies".
# [develop]
# command = 'claude -p "$DREAM_PROMPT" --permission-mode acceptEdits'
# attempts = 4                    # candidate policies per dream
"#;

#[derive(Parser)]
#[command(name = "git dream", version, about = "Git-native experimental search: preserve attempts, reproduce evaluations, compare exploration policies.")]
struct Cli {
    /// Run as if started in this directory (like `git -C`)
    #[arg(short = 'C', value_name = "DIR")]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct Common {
    /// Config file (default: <repo>/.git-dream.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Run name (default: from the config)
    #[arg(long, global = true)]
    run: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a .git-dream.toml template
    Init {
        #[arg(long, default_value = "space", value_parser = ["space", "command"])]
        kind: String,
        #[arg(long)]
        force: bool,
    },
    /// N steps of search. Each step is a new round (dream -> explore -> promote) or,
    /// with `[explore] extend` / --extend, more attempts in the current round's tree
    Run {
        #[arg(short = 'n', long = "rounds", default_value_t = 1)]
        rounds: usize,
        /// Add to the current round's tree instead of starting a new round
        #[arg(long, conflicts_with = "new_round")]
        extend: bool,
        /// Start a new round from the best attempt (after dreaming), even with `[explore] extend`
        #[arg(long)]
        new_round: bool,
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        cands: Option<usize>,
        #[arg(long, value_parser = ["cache", "strict"])]
        mode: Option<String>,
        /// Baseline: keep the policy fixed
        #[arg(long)]
        no_dream: bool,
        /// Attempts in flight at once (default: `[explore] parallel`)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=64))]
        parallel: Option<u64>,
        #[command(flatten)]
        c: Common,
    },
    /// One live round with the deployed policy (--extend: more attempts in the current round)
    Explore {
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        extend: bool,
        /// Attempts in flight at once (default: `[explore] parallel`)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=64))]
        parallel: Option<u64>,
        #[command(flatten)]
        c: Common,
    },
    /// Dream only: learn from recorded history and deploy the policy for the next round
    Dream {
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        cands: Option<usize>,
        #[arg(long, value_parser = ["cache", "strict"])]
        mode: Option<String>,
        #[arg(long, default_value_t = 10)]
        top: usize,
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        c: Common,
    },
    /// Score policies on recorded history ("family:k=v,..." or JSON; default: the deployed policy)
    Replay {
        policy: Vec<String>,
        /// Score the policy in this directory (its policy.json and program), as a candidate policy is scored
        #[arg(long, value_name = "DIR", conflicts_with_all = ["policy", "holdout"])]
        dir: Option<PathBuf>,
        /// Report disjoint training / held-out replay (last N rounds of each run)
        #[arg(long)]
        holdout: Option<usize>,
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long, value_parser = ["cache", "strict"])]
        mode: Option<String>,
        #[arg(short, long)]
        verbose: bool,
        #[command(flatten)]
        c: Common,
    },
    /// Rounds, scores, deployed policy and best attempt of a run
    Status {
        #[command(flatten)]
        c: Common,
    },
    /// One round's discovery tree (default: the last round), drawn by `git log --graph`
    Show {
        round: Option<u32>,
        #[command(flatten)]
        c: Common,
    },
    /// List policy families and their search grids
    Policies {
        #[command(flatten)]
        c: Common,
    },
    /// Push dream refs (refs/dream/*, dream/* branches and tags, notes)
    Push {
        #[arg(default_value = "origin")]
        remote: String,
    },
    /// Fetch dream refs
    Fetch {
        #[arg(default_value = "origin")]
        remote: String,
    },
    /// Delete every ref and the scratch worktree of a run
    Clean {
        #[arg(long)]
        run: String,
        #[arg(long)]
        yes: bool,
    },
}

fn engine(c: &Common) -> Result<Engine> {
    Engine::open(Path::new("."), c.config.as_deref(), c.run.as_deref())
}

fn mode(m: &Option<String>) -> Result<Option<Mode>> {
    m.as_deref().map(Mode::parse).transpose()
}

fn parse_policy(e: &Engine, s: &str) -> Result<Policy> {
    let s = s.trim();
    if s.starts_with('{') {
        return e.registry.make(&serde_json::from_str(s)?);
    }
    let (fam, rest) = s.split_once(':').unwrap_or((s, ""));
    let mut params = Map::new();
    for kv in rest.split(',').filter(|x| !x.is_empty()) {
        let Some((k, v)) = kv.split_once('=') else { bail!("bad parameter {kv:?} (want k=v)") };
        let p = if v.eq_ignore_ascii_case("inf") { Param::Num(INF) } else { Param::from_json(&serde_json::from_str(v).unwrap_or(Value::String(v.into())))? };
        params.insert(k.into(), p.to_json());
    }
    e.registry.make(&serde_json::json!({"family": fam, "params": params}))
}

fn status(e: &Engine) -> Result<()> {
    let rounds = e.mem.rounds(&e.run)?;
    let base = e.mem.rev(&format!("refs/dream/{}/base", e.run)).or_else(|| e.mem.rev("HEAD^{commit}")).ok_or_else(|| anyhow::anyhow!("repository has no commits"))?;
    println!("run {:?}: base {}, goal {}, agent {}, {} rounds", e.run, &base[..10], e.goal.as_str(), e.agent.kind(), rounds.len());
    let mut cum = 0;
    for r in rounds {
        let Some(t) = e.mem.load_tree(&e.run, r)? else { continue };
        cum += t.calls();
        let promoted = e.mem.rev(&format!("refs/dream/{}/r{r:02}/best", e.run));
        let pn = t.nodes.iter().find(|n| Some(&n.sha) == promoted.as_ref());
        let notes = e.mem.notes_of(&t.nodes.iter().map(|n| n.sha.as_str()).collect::<Vec<_>>())?;
        let evaluations: usize = notes.values().map(|n| usize::from(n.get("seconds").is_some()) + n["confirmation"]["evaluations"].as_array().map_or(0, Vec::len)).sum();
        println!(
            "  r{r:02} {:3} calls (cum {cum:4}), {evaluations} evaluator runs  root {:>10}  best {:>10}  {:10}  {}",
            t.calls(),
            fs(t.root().score),
            fs(pn.and_then(|n| notes.get(&n.sha).and_then(|note| note["confirmation"]["score"].as_f64()).or(n.score))),
            pn.map_or("-".to_string(), |n| n.sha[..10].to_string()),
            short(&t.policy, 70)
        );
    }
    match e.deployed(None)? {
        Some(p) => println!("deployed policy: {}   (git log --first-parent -p dream/{}/policy -- policy.json)", p.repr(), e.run),
        None => println!("deployed policy: (none yet)"),
    }
    let candidates = git_dream::memory::policy_run(&e.run);
    let developed = e.mem.rounds(&candidates)?;
    if !developed.is_empty() {
        println!("policy development: {} dream{} with candidate policies   (git dream show --run {candidates})", developed.len(), if developed.len() == 1 { "" } else { "s" });
    }
    if let Some(b) = e.mem.rev(&format!("refs/heads/dream/{}/best", e.run)) {
        println!("best attempt: dream/{}/best = {}   (git diff {} dream/{}/best)", e.run, &b[..10], &base[..10], e.run);
    }
    Ok(())
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("git dream: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let started_in = std::env::current_dir()?;
    if let Some(dir) = &cli.dir {
        std::env::set_current_dir(dir).map_err(|e| anyhow::anyhow!("-C {}: {e}", dir.display()))?;
    }
    match cli.cmd {
        Cmd::Init { kind, force } => {
            let mem = Memory::open(Path::new("."))?;
            let p = mem.top.join(CONFIG_NAME);
            if p.exists() && !force {
                bail!("{} exists (use --force to overwrite)", p.display());
            }
            std::fs::write(&p, if kind == "space" { TEMPLATE_SPACE } else { TEMPLATE_COMMAND })?;
            println!("wrote {}\nedit [agent] and [evaluate] and commit it (a run starts from the commit you are on), then: git dream run -n 3", p.display());
        }
        Cmd::Run { rounds, extend, new_round, budget, cands, mode: m, no_dream, parallel, c } => {
            let mut e = engine(&c)?;
            e.cfg.explore.parallel = parallel.map_or(e.cfg.explore.parallel, |p| p as usize);
            e.run(rounds, !no_dream, budget, cands, mode(&m)?, extend || new_round || e.cfg.explore.extend, new_round)?;
            status(&e)?;
        }
        Cmd::Explore { budget, extend, parallel, c } => {
            let mut e = engine(&c)?;
            e.cfg.explore.parallel = parallel.map_or(e.cfg.explore.parallel, |p| p as usize);
            if extend { e.extend(budget)? } else { e.explore(None, budget)? };
        }
        Cmd::Dream { budget, cands, mode: m, top, dry_run, c } => {
            let e = engine(&c)?;
            let ranking = e.dream(None, cands, mode(&m)?, budget, !dry_run)?;
            for (p, r) in ranking.iter().take(top) {
                println!("  J={:+.4} gain={:+.2}% calls={:4} miss={:.2}  {}", r.j, r.gain * 100.0, r.calls, r.miss_rate, p.repr());
            }
            if dry_run {
                println!("(dry run: nothing deployed)");
            }
        }
        Cmd::Replay { policy, dir, holdout, budget, mode: m, verbose, c } => {
            let e = engine(&c)?;
            if let Some(dir) = dir {
                let (rounds, budget, mode) = e.scoring(mode(&m)?, budget)?;
                let r = e.score_policy(&started_in.join(dir), &rounds, budget, mode);
                let Some(replay) = r.note.get("replay") else { bail!("{}: {}", r.note["exit"].as_str().unwrap_or("failed"), r.note["stderr"].as_str().unwrap_or("")) };
                let f = |k: &str| replay[k].as_f64().unwrap_or(0.0);
                println!("J={:+.4} gain={:+.2}% calls={:4} miss={:.2}  {}", f("J"), f("gain") * 100.0, replay["calls"], f("miss_rate"), r.note["policy"].as_str().unwrap_or(""));
                println!("(over {} recorded round{} of run {:?}, {} attempts each, {} replay; J is the score of a candidate policy)", rounds.len(), if rounds.len() == 1 { "" } else { "s" }, e.run, budget, mode.as_str());
                return Ok(());
            }
            let pols = if policy.is_empty() { vec![e.current_policy()?] } else { policy.iter().map(|s| parse_policy(&e, s)).collect::<Result<_>>()? };
            for p in pols {
                if let Some(n) = holdout {
                    let (train, test) = git_dream::sim::split_history(&e.replay_history()?, n)?;
                    for (label, hist) in [("training", train), ("held-out", test)] {
                        let r = git_dream::sim::replay(&p, &hist, budget.unwrap_or(e.cfg.explore.budget), e.cfg.dream.lam, e.agent.space(), mode(&m)?.unwrap_or(e.mode()), e.cfg.dream.normalize == "root", false)?;
                        println!("{label}: J={:+.4} gain={:+.2}% calls={} miss={:.2} rounds={} {}", r.j, r.gain * 100.0, r.calls, r.miss_rate, hist.len(), p.repr());
                    }
                    println!("Held-out replay diagnoses overfitting; it does not estimate unobserved outcomes or establish live improvement.");
                    continue;
                }
                let r = e.replay(&p, mode(&m)?, budget)?;
                println!("J={:+.4} gain={:+.2}% calls={:4} miss={:.2}  {}", r.j, r.gain * 100.0, r.calls, r.miss_rate, p.repr());
                if verbose {
                    for t in &r.trees {
                        let misses = t.nodes.iter().filter(|n| n.status == "miss").count();
                        println!("    r{:02}: {:3} calls, best {} ({misses} misses)", t.round, t.calls(), fs(t.best().and_then(|b| b.score)));
                    }
                }
            }
        }
        Cmd::Status { c } => {
            let e = engine(&c)?;
            status(&e)?;
            let others: Vec<String> = e.mem.runs()?.into_iter().filter(|r| *r != e.run).collect();
            if !others.is_empty() {
                println!("other runs: {}", others.join(", "));
            }
        }
        Cmd::Show { round, c } => {
            let e = engine(&c)?;
            let rounds = e.mem.rounds(&e.run)?;
            let Some(&last) = rounds.last() else { bail!("no rounds yet") };
            let r = round.unwrap_or(last);
            let refs = format!("refs/dream/{}/r{r:02}", e.run);
            let Some(root) = e.mem.rev(&format!("{refs}/n000")) else { bail!("round {r} not found") };
            // The tree is commits: git draws it. Each line is an attempt (status, score,
            // subject); the promoted one carries the round's `best` ref.
            let format = "--format=%C(auto)%h%C(reset) %(trailers:key=Dream-Status,valueonly,separator=) %(trailers:key=Dream-Score,valueonly,separator=) %s%C(auto)%d";
            let status = std::process::Command::new("git")
                .args(["log", "--graph", format, &format!("--decorate-refs={refs}/best"), &format!("--glob={refs}/n*"), "--not", &format!("{root}^@")])
                .status()?;
            if !status.success() {
                bail!("git log failed");
            }
        }
        Cmd::Policies { c } => {
            let e = engine(&c)?;
            for f in &e.registry.families {
                println!("{}{}", f.name, if f.name == git_dream::model::BANDIT { "  (learns from measured outcomes; not tuned by replay)" } else if f.grid.is_empty() { "  (not searched: no grid)" } else { "" });
                println!("    {}", f.doc);
                for (k, v) in &f.grid {
                    println!("    {k}: [{}]", v.iter().map(Param::show).collect::<Vec<_>>().join(", "));
                }
            }
        }
        Cmd::Push { remote } => {
            let mem = Memory::open(Path::new("."))?;
            let mut args = vec!["push", remote.as_str()];
            args.extend(REFSPECS);
            println!("{}", mem.git(&args)?);
        }
        Cmd::Fetch { remote } => {
            let mem = Memory::open(Path::new("."))?;
            let mut args = vec!["fetch", remote.as_str()];
            args.extend(REFSPECS);
            println!("{}", mem.git(&args)?);
        }
        Cmd::Clean { run, yes } => {
            check_run(&run)?;
            if !yes {
                bail!("this deletes every ref of run {run:?}; re-run with --yes");
            }
            let n = Memory::open(Path::new("."))?.delete_run(&run)?;
            println!("deleted {n} refs of run {run:?}");
        }
    }
    Ok(())
}
