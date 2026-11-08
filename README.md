# git-dream

**Dream-RSI on git.** git-dream improves *how* a search explores a repository, not just *what* it finds. Git is its only memory: every attempt is a commit, and dreaming replays recorded history.

[Dream-RSI](https://www.dream-rsi.com/) ("Recursive Self-Improvement through Evolving Worlds", Google / Google DeepMind, 2026) uses three parts:

- a fixed **discovery agent** proposes changes;
- a fixed **evaluator** scores them;
- an **exploration policy**, which is ordinary code, decides which attempt to continue next and when to stop.

After each round of real experiments, candidate policies are **dreamt**: they are replayed offline over the recorded attempts at zero evaluation cost. The best one is deployed for the next round.

git-dream is a single Rust binary that works in **any git repository**. The agent can be a deterministic parameter-search operator, or any program that edits code, such as an LLM coding agent. Everything is stored as ordinary git objects under `refs/dream/*`. It never touches your branches or working tree.

```
           ┌─────────────────── round r ───────────────────┐
 history ─▶│ dream: replay N candidate policies (0 evals)  │─▶ policy r ─▶ explore: agent + evaluator
  (git)    └───────────────────────────────────────────────┘               (one commit per attempt)
    ▲                                                                                 │
    └───────────────────── remember: refs/dream/<run>/rNN/nNNN ◀──────────────────────┘
```

## Install

```bash
cargo install --git https://github.com/timohuovinen/git-dream   # installs `git-dream`; git runs it as `git dream`
# private repo / SSH:  CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git ssh://git@github.com/timohuovinen/git-dream
# from a checkout:     cargo install --path .
```

It needs git ≥ 2.31 and `sh` at run time. The examples use only `sh` and `awk`.

## Quick start (seconds)

```bash
cp -r git-dream/examples/toy-space /tmp/toy && cd /tmp/toy
git init -q && git add -A && git commit -qm init
git dream run -n 5          # 5 rounds of: dream -> explore -> promote
git dream status            # scores per round, deployed policy, best attempt
git dream show              # the last round's discovery tree
git show dream/main/best    # the best attempt is a real commit
```

## Use it on your own repository

```bash
cd your-repo
git dream init                 # or: git dream init --kind command
$EDITOR .git-dream.toml        # describe the agent and the evaluator
git add .git-dream.toml && git commit -m "git-dream config"
git dream run -n 3
git diff HEAD dream/main/best  # what the best attempt changed; merge or cherry-pick it
```

### 1. Parameter search (`kind = "space"`)

Each attempt is your base commit plus a parameter file (default `.dream/params.json`). Your evaluator reads the parameters from that file, from `$DREAM_PARAMS` (JSON) or from `$DREAM_PARAM_<NAME>`. The agent is a pure function of (salt, parent, k): each proposal moves one or two knobs by one or two grid steps. So dreaming can predict what *any* policy would have tried, and reuse every earlier measurement of that design (`mode = "cache"`). This is where dreaming helps most.

```toml
[agent]
kind = "space"
start = { threads = 4, block = 64 }
[agent.knobs]
threads = [1, 2, 4, 8, 16]
block = [16, 32, 64, 128, 256]

[evaluate]
command = "cargo build --release -q && ./target/release/bench"   # last stdout line = score
goal = "min"
```

### 2. Code changes by an agent (`kind = "command"`)

The command runs in a scratch worktree checked out at the parent attempt. Whatever it leaves behind, respecting `.gitignore`, becomes the child attempt's commit. The child's parent commit is the attempt it continued.

The command receives:

- `$DREAM_PROMPT`: your prompt plus search context (the path so far, and what has already been tried from this attempt);
- `$DREAM_CONTEXT`: the same context as a file;
- `$DREAM_MESSAGE_FILE`: a file to write a one-line description of the change into.

```toml
[agent]
kind = "command"
command = 'claude -p "$DREAM_PROMPT" --permission-mode acceptEdits'
prompt = "Make `cargo bench` faster. Keep `cargo test` passing. Make one focused change."
timeout = 1800

[evaluate]
command = "cargo test -q >/dev/null 2>&1 && ./bench.sh"
goal = "min"
remeasure_root = true
confirm_top = 2

[explore]
budget = 6

[dream]
mode = "strict"
```

An LLM agent is stochastic, so dreaming can only replay **recorded** decisions (`strict`). It still tunes stopping rules and which attempts are worth continuing, but it can't predict changes that were never made. See [Limitations](#limitations).

### The evaluator

`[evaluate] command` runs (`sh -c`) in the attempt's worktree. The **last stdout line** is the score: either a number, or a JSON object such as `{"score": 12.3, "status": "ok", ...anything else}`. A non-zero exit, a timeout or `"score": null` marks the attempt as failed. Failed attempts are recorded too, and the `repair` policy can continue from them.

Ignored build outputs (e.g. `target/`) survive between attempts in the scratch worktree, so incremental builds stay fast. Each command runs in its own process group, and the whole group is killed on timeout.

## Where everything is stored

| Dream-RSI | git |
|---|---|
| attempt | commit: its tree is the attempt's code / parameters |
| parent attempt | parent commit (a round's root has the previous round's promoted attempt as parent) |
| outcome | commit trailers `Dream-Score`, `Dream-Status`, `Dream-Design`, `Dream-Policy`, … |
| full evaluator output | git notes, `refs/notes/dream` |
| discovery tree | hidden refs `refs/dream/<run>/rNN/nNNN` |
| start of a run | `refs/dream/<run>/base` |
| best attempt | branch `dream/<run>/best` (per round: `refs/dream/<run>/rNN/best`) |
| exploration policy | branch `dream/<run>/policy`: one commit per round (`policy.json` with the dream's ranking, the config, external policy sources), tags `dream/<run>/policy-rNN` |

The trees are rebuilt from commit trailers before every dream, so the repository *is* the state. `.git/dream/` only holds scratch worktrees and can be deleted at any time. Hidden refs aren't pushed by default; `git dream push [remote]` / `git dream fetch [remote]` share runs.

## Commands

| | |
|---|---|
| `git dream init [--kind space\|command]` | write a `.git-dream.toml` template |
| `git dream run -n N [--no-dream] [--budget B]` | N rounds of dream → explore → promote (`--no-dream` = fixed-policy baseline) |
| `git dream explore` / `git dream dream [--dry-run]` | the two halves separately |
| `git dream replay ["family:k=v,..." \| JSON ...] [-v]` | score policies on recorded history: the tool for developing policies |
| `git dream status` / `git dream show [ROUND]` | summary of a run / one round's tree |
| `git dream policies` | policy families and their search grids |
| `git dream push` / `fetch [REMOTE]` | share dream refs |
| `git dream clean --run NAME --yes` | delete a run |

All commands take `--run NAME` and `--config FILE`. Runs are independent, e.g. `--run baseline --no-dream` against `--run dream`.

## Policies

Built-in families ([src/policies.rs](src/policies.rs)):

- `refine_best`: π0, always continue the best attempt;
- `ucb`, `beam`, `softmax`;
- `repair`: continues failed attempts and merges identical designs. It came out of the [case study](docs/case-study-hashbench.md).

A dream samples `candidates` parameterisations from the families' grids. It replays each one over every recorded round and deploys the one with the best

    J = mean over rounds [ relative improvement found − λ · calls / budget ]

The deployed policy is always a candidate and wins ties, so on replay a dream never picks something worse than what's running.

### Writing new policies (in any language)

The paper's real lever is a **policy-development agent** that writes *new* policy code after reading the history. There are two ways to add a family:

1. **In Rust:** add a variant in [src/policies.rs](src/policies.rs) and rebuild.
2. **As an external program:** git-dream starts it once and asks it one JSON line per decision.

```toml
[policies.external.patient]
command = "./target/release/examples/patient"   # run from the repository root
source = "examples/patient.rs"                  # optional: snapshot into each policy commit
grid = { p = [3, 5, 8] }                  # non-empty grid => searched while dreaming
```

Request: `{"params": {"p": 5}, "seed": 3, "tree": {"goal": "min", "budget": 20, "calls": 4, "nodes": [{"id", "parent", "k", "design", "score", "status", "change", "n_exp"}, ...]}}`
Reply: a node id to continue, or `null` to end the round. The reply must depend only on the request, or replay can't reproduce live rounds.

A complete example in Rust, [examples/patient.rs](examples/patient.rs) (`cargo build --release --example patient`), continues the best attempt and stops after `p` calls without improvement.


```bash
git dream replay "patient:p=5" "repair:c=0.02,fp=0.03,kmax=6,kfail=2,patience=12,eps=0.005,dedup=true" -v
```

A good prompt for the meta-agent: *"Read `git dream status`, `git dream show` for the last rounds and `git dream replay -v`. Find where the deployed policy wastes calls, write a new policy, and check with `git dream replay` that it beats the deployed one."*

## Does it work?

- **[Case study: high-load hash tables](docs/case-study-hashbench.md).** Run with the research prototype this tool came from: 722 real benchmark runs, all stored in git. With the second-generation `repair` policy, dreaming cut the remaining gap to the true optimum from **14.6% to 8.4% with 26% fewer evaluations** (100 synthetic replicates, paired difference +0.061 ± 0.022). Live dreamt rounds found the optimum in 16–17 calls, where the fixed policy spent 40. The first policy families gave **no** gain. Strict replay did **worse** than not dreaming, and replay ranked policies poorly (Spearman ≈ 0.1).
- **Toy example** (`examples/toy-space`, 10 agent salts × 6 rounds of 25 calls, `--no-dream` vs dreaming): **104 vs 150 evaluations**, gap to the optimum **10.9% vs 13.6%**, optimum reached in 5/10 vs 4/10 runs. Ten replicates is too few to call the quality difference real; the call reduction is consistent.

## Limitations

- **Replay knows only what was recorded.** It can't predict untried changes. With a stochastic agent (an LLM), only recorded decisions replay (`strict`), and the case study found strict dreaming can do harm. Keep `lam` small and budgets modest, and treat it as a stopping/allocation optimiser.
- **Early stopping can lock in.** Once rounds stop improving, the cheapest policy on replay is one that stops early. It then records too little new history to escape a local optimum. This showed up in the toy runs. Mix in `--no-dream` rounds now and then, or lower `lam`.
- **Replay is a filter, not an oracle.** Its policy ranking correlated weakly with true performance in the case study.
- **Noisy benchmarks:** use `remeasure_root = true` and `normalize = "root"` against machine drift, and `confirm_top = k` against the winner's curse (promoting a lucky measurement).
- The evaluator runs whatever command the config names, and a command agent can edit anything in its worktree. Run untrusted configs in a sandbox.

## Repository layout

```
src/                memory.rs (git layout), engine.rs (config, rounds, live world), sim.rs (replay, dream),
                    policies.rs, agents.rs (space/command agents, evaluator), rng.rs, main.rs (CLI)
tests/e2e.rs        end-to-end tests on the examples in throwaway repositories (cargo test)
examples/           toy-space (parameter search), toy-command (code-editing agent), hashbench (case-study config)
docs/               case-study-hashbench.md
research/hashbench  archived Python prototype, raw results and benchmark patch behind the case study
```

License: GPL-3.0-or-later.
