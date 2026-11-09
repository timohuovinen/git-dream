//! `git dream`: Dream-RSI on git. Install the binary on PATH as `git-dream`
//! and git runs it as `git dream <command>`.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use git_dream::engine::{fs, short, summary, Engine, CONFIG_NAME};
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
budget = 20                       # max evaluations per round

[dream]
lam = 0.02                        # cost of spending the whole budget, in units of relative improvement
candidates = 300                  # sampled policies per dream
mode = "cache"                    # cache: reuse any recorded measurement of a design; strict: recorded edges only
normalize = "none"                # "root": rescale cached scores by each tree's root (machine drift)

[policies]
initial = { family = "refine_best" }
families = []                     # restrict the dream search (default: all searchable families)
# Policies in any language: a program answering one JSON line per decision (see README).
# [policies.external.patient]
# command = "./target/release/patient-policy"
# grid = { p = [3, 5, 8] }

[storage]
repack = true                     # pack each round's attempts into one pack (deltas instead of full copies)
gc_every = 10                     # every N rounds also run `git gc` to merge the packs (0 = never)
"#;

const TEMPLATE_COMMAND: &str = r#"# git-dream configuration (https://github.com/timohuovinen/git-dream)
run = "main"

[agent]
kind = "command"                  # any program that edits the checkout, e.g. an LLM coding agent
# Runs in a scratch worktree at the parent attempt. Env: $DREAM_PROMPT (prompt below + where
# the attempt is + how to work), $DREAM_CONTEXT (file: every attempt of every round so far,
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

[explore]
budget = 6                        # agent calls are expensive: keep rounds small

[dream]
lam = 0.05
candidates = 300
mode = "strict"                   # stochastic agent: only recorded edges can be replayed

[policies]
initial = { family = "refine_best" }

[storage]
repack = true                     # pack each round's attempts into one pack (deltas instead of full copies)
gc_every = 10                     # every N rounds also run `git gc` to merge the packs (0 = never)
"#;

#[derive(Parser)]
#[command(name = "git dream", version, about = "Dream-RSI on git: improve how a search explores a repository, using git as memory.")]
struct Cli {
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
    /// N rounds of: dream (replay policies over history) -> explore -> promote
    Run {
        #[arg(short = 'n', long = "rounds", default_value_t = 1)]
        rounds: usize,
        #[arg(long)]
        budget: Option<usize>,
        #[arg(long)]
        cands: Option<usize>,
        #[arg(long, value_parser = ["cache", "strict"])]
        mode: Option<String>,
        /// Baseline: keep the policy fixed
        #[arg(long)]
        no_dream: bool,
        #[command(flatten)]
        c: Common,
    },
    /// One live round with the deployed policy
    Explore {
        #[arg(long)]
        budget: Option<usize>,
        #[command(flatten)]
        c: Common,
    },
    /// Dream only: choose and deploy the policy for the next round
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
    /// Print one round's discovery tree (default: the last round)
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
    let base = e.mem.base(&e.run)?;
    println!("run {:?}: base {}, goal {}, agent {}, {} rounds", e.run, &base[..10], e.goal.as_str(), e.agent.kind(), rounds.len());
    let mut cum = 0;
    for r in rounds {
        let Some(t) = e.mem.load_tree(&e.run, r)? else { continue };
        cum += t.calls();
        let promoted = e.mem.rev(&format!("refs/dream/{}/r{r:02}/best", e.run));
        let pn = t.nodes.iter().find(|n| Some(&n.sha) == promoted.as_ref());
        println!(
            "  r{r:02} {:3} calls (cum {cum:4})  root {:>10}  best {:>10}  {:10}  {}",
            t.calls(),
            fs(t.root().score),
            fs(pn.and_then(|n| n.score)),
            pn.map_or("-".to_string(), |n| n.sha[..10].to_string()),
            short(&t.policy, 70)
        );
    }
    match e.deployed(None)? {
        Some(p) => println!("deployed policy: {}", p.repr()),
        None => println!("deployed policy: (none yet)"),
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
        Cmd::Run { rounds, budget, cands, mode: m, no_dream, c } => {
            let e = engine(&c)?;
            e.rounds(rounds, !no_dream, budget, cands, mode(&m)?)?;
            status(&e)?;
        }
        Cmd::Explore { budget, c } => {
            engine(&c)?.explore(None, budget)?;
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
        Cmd::Replay { policy, budget, mode: m, verbose, c } => {
            let e = engine(&c)?;
            let pols = if policy.is_empty() { vec![e.current_policy()?] } else { policy.iter().map(|s| parse_policy(&e, s)).collect::<Result<_>>()? };
            for p in pols {
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
            let Some(t) = e.mem.load_tree(&e.run, r)? else { bail!("round {r} not found") };
            let promoted = e.mem.rev(&format!("refs/dream/{}/r{r:02}/best", e.run)).unwrap_or_default();
            println!("run {} r{r:02}: {}, {} calls", e.run, t.policy, t.calls());
            fn rec(t: &git_dream::tree::Tree, id: usize, depth: usize, promoted: &str) {
                let n = &t.nodes[id];
                let mark = if n.sha == promoted { "  <- promoted" } else { "" };
                println!("{}n{:03} {:5} {:>10} {}  {}{mark}", "  ".repeat(depth), n.id, n.status, fs(n.score), &n.sha[..9], short(summary(&n.change), 60));
                for c in t.children(id) {
                    rec(t, c.id, depth + 1, promoted);
                }
            }
            rec(&t, 0, 0, &promoted);
        }
        Cmd::Policies { c } => {
            let e = engine(&c)?;
            for f in &e.registry.families {
                println!("{}{}", f.name, if f.grid.is_empty() { "  (not searched: no grid)" } else { "" });
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
