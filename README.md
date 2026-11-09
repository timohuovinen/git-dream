# git-dream

**Git-native experimental search for coding agents.** Preserve every attempt, inspect failures, reproduce evaluations, and compare exploration strategies using Git history.

Use a fixed agent to propose changes and a fixed evaluator to score them. git-dream records the search as commits, keeps your checkout separate, and lets you test policies that choose where to explore next. Adaptive search is experimental: replay scores do **not** establish better live performance.

[Dream-RSI](https://www.dream-rsi.com/) ("Recursive Self-Improvement through Evolving Worlds", Google / Google DeepMind, 2026) uses three parts:

- a fixed **discovery agent** proposes changes;
- a fixed **evaluator** scores them;
- an **exploration policy**, which is ordinary code, decides which attempt to continue next and when to stop.

After each round of real experiments, candidate policies are **dreamt**: they are replayed offline over the recorded attempts at zero evaluation cost. The training winner is checked against the incumbent on held-out rounds before deployment. Insufficient history or a failed gate keeps the incumbent. This diagnostic reduces one form of overfitting; it does not remove selection bias.

git-dream is a single Rust binary that works in **any git repository**. The agent can be a deterministic parameter-search operator, or any program that edits code, such as an LLM coding agent. Everything is stored as ordinary git objects under `refs/dream/*`. It creates `dream/*` branches and refs while leaving your checked-out branch and files alone.

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

Or download a prebuilt binary from [Releases](https://github.com/timohuovinen/git-dream/releases): Linux amd64/arm64 (static, `*-unknown-linux-musl.tar.gz`) and Windows amd64 (`*-pc-windows-msvc.zip`). Put `git-dream` (`git-dream.exe`) anywhere on your `PATH`.

```bash
gh release download -R timohuovinen/git-dream -p '*x86_64-unknown-linux-musl.tar.gz'
tar xzf git-dream-*.tar.gz && install git-dream-*/git-dream ~/.local/bin/
```

It needs git ≥ 2.31 and `sh` at run time. The examples use only `sh` and `awk`. On Windows, commands run in the `sh` that comes with [Git for Windows](https://gitforwindows.org/) (found next to `git`; override with `GIT_DREAM_SH`).

**Releasing:** bump `version` in `Cargo.toml`, commit, then `git tag v0.2.1 && git push origin v0.2.1`. [.github/workflows/release.yml](.github/workflows/release.yml) builds the three binaries and publishes the release with `SHA256SUMS`.

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

Each attempt is your base commit plus a parameter file (default `.dream/params.json`). Your evaluator reads the parameters from that file, from `$DREAM_PARAMS` (JSON) or from `$DREAM_PARAM_<NAME>`. The agent is a pure function of (salt, parent, k): each proposal moves one or two knobs by one or two grid steps. So dreaming can predict what *any* policy would have tried, and reuse every earlier measurement of that design (`mode = "cache"`). Cache replay assumes the objective and proposal operator remain stable; whether it helps must be measured prospectively.

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

- `$DREAM_PROMPT`: your prompt, where the attempt sits (the path so far, what has already been tried from this attempt, the round's best), and how to work;
- `$DREAM_CONTEXT`: the whole search history as a file: every attempt of every round of the run so far (with `[dream] history = "all"`, of every run in the repository), with its write-up, its measured result and, for failures, the evaluator's output;
- `$DREAM_MESSAGE_FILE`: a file to write the proposal into: a one-line summary (the commit subject), a blank line, then the idea, the evidence from the history and why it isn't a repeat. It's kept in full in the commit message.

Both files are in `.dream-agent/` inside the worktree, so a sandboxed agent can reach them. The directory is removed before the attempt is committed.

This follows the exploration procedure of Dream-RSI (appendix B.1 of the paper, see [papers/](papers/)). Branches keep each attempt's code apart, and knowledge is shared because every attempt reads the *raw* history of all branches and rounds. There's no distilled "lessons" file: the paper found that injecting summarised directional insights into the prompt made the search worse (§5.1, Fig. 5). For the same reason, nothing records *why* an attempt failed. The commit records how it failed (`Dream-Failure: exit 3`, `timeout`, …), the note holds the evaluator's output, and each agent is told to check the failed code (`git show`) before deciding whether the idea or its implementation was at fault.

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
protected = ["bench.sh", "tests"]  # reject attempts that change the evaluation contract

[explore]
budget = 6
min_calls = 2                     # fallback to best-node refinement if a policy stops earlier

[dream]
mode = "strict"
holdout = 1                     # last round per run; 0 explicitly disables the gate
```

An LLM agent is stochastic, so dreaming can only replay **recorded** decisions (`strict`). It can compare stopping and continuation rules on recorded edges, but it cannot establish the outcome of changes that were never made. An LLM that reads the whole search history may also propose differently when decision order changes, even at the same parent. See [Limitations](#limitations).

### The evaluator

`[evaluate] command` runs (`sh -c`) in the attempt's worktree. The **last stdout line** is the score: either a number, or a JSON object such as `{"score": 12.3, "status": "ok", ...anything else}`. A non-zero exit, a timeout or `"score": null` marks the attempt as failed. Failed attempts are recorded too, and the `repair` policy can continue from them.

`budget` counts proposals, including failures and no-ops. Initial/root evaluations and confirmation runs are **extra**; `status` reports actual evaluator executions separately. With confirmation enabled, candidates and the incumbent root are compared using fresh repetitions only. A failed repetition disqualifies that candidate, and the next round inherits the confirmed mean. Confirmation reduces measurement selection effects; it does not prove statistical significance.

`protected` checks configured paths against the base tree before evaluating a command attempt. Rejected edits remain in Git. This catches direct test/evaluator edits; it is not a sandbox or protection against malicious code running during evaluation.

Ignored build outputs (e.g. `target/`) survive between attempts in the scratch worktree, so incremental builds stay fast. Each command runs in its own process group, and the whole group is killed on timeout.

## Where everything is stored

| Dream-RSI | git |
|---|---|
| attempt | commit: its tree is the attempt's code / parameters |
| parent attempt | parent commit (a round's root has the previous round's promoted attempt as parent) |
| agent's write-up of the attempt | commit message (subject = summary, body = the rest) |
| outcome | commit trailers `Dream-Score`, `Dream-Status`, `Dream-Failure`, `Dream-Design`, `Dream-Policy`, … |
| evaluator output | git notes, `refs/notes/dream`: exit, metrics, first 20 + last 40 lines of stdout and stderr |
| discovery tree | hidden refs `refs/dream/<run>/rNN/nNNN` |
| start of a run | `refs/dream/<run>/base` |
| best attempt | branch `dream/<run>/best` (per round: `refs/dream/<run>/rNN/best`) |
| exploration policy | branch `dream/<run>/policy`: one commit per round (`policy.json` with the dream's ranking, the config, external policy sources), tags `dream/<run>/policy-rNN` |

Attempts are written with plumbing commands (`hash-object`, `commit-tree`, `update-ref`), which never trigger git's automatic `gc`. Left alone, every attempt would keep a full compressed copy of each file it changed. So after each round git-dream runs `git repack -d`, which packs that round's attempts into one pack, storing near-identical versions as deltas, and every `gc_every` rounds it runs `git gc` to merge those packs:

```toml
[storage]
repack = true      # default
gc_every = 10      # default; 0 = never
```

The trees are rebuilt from commit trailers before every dream, so the repository *is* the state. `.git/dream/` only holds scratch worktrees and can be deleted at any time. Hidden refs aren't pushed by default; `git dream push [remote]` / `git dream fetch [remote]` share runs.

## Commands

| | |
|---|---|
| `git dream init [--kind space\|command]` | write a `.git-dream.toml` template |
| `git dream run -n N [--no-dream] [--budget B]` | N rounds of dream → explore → promote (`--no-dream` = fixed-policy baseline) |
| `git dream explore` / `git dream dream [--dry-run]` | the two halves separately |
| `git dream replay ["family:k=v,..." \| JSON ...] [-v] [--holdout N]` | score policies on recorded history: the tool for developing policies |
| `git dream status` / `git dream show [ROUND]` | summary of a run / one round's tree |
| `git dream policies` | policy families and their search grids |
| `git dream push` / `fetch [REMOTE]` | share dream refs |
| `git dream clean --run NAME --yes` | delete a run |

All commands take `--run NAME` and `--config FILE`. Runs are independent by default, e.g. `--run baseline --no-dream` against `--run dream`.

A run is one discovery process, the paper's history H: what dreaming replays and what each agent reads is that run's rounds. That keeps a comparison fair, since a baseline that reads the other arm's attempts is no longer a baseline. To pool knowledge instead, set `[dream] history = "all"`. Every run's recorded rounds then feed both dreaming and the agents' history file, with each attempt labelled by its run. It's one setting because the paper has one history. Nothing is copied between runs: the history is read from the runs' refs, and `git show` works across them.

## Policies

Built-in families ([src/policies.rs](src/policies.rs)):

- `refine_best`: π0, always continue the best attempt;
- `random` (uniform valid-parent selection), `ucb`, `beam`, `softmax`;
- `repair`: continues failed attempts and merges identical designs. It came out of the [case study](docs/case-study-hashbench.md).

A dream samples `candidates` parameterisations from the families' grids. It deduplicates candidate specifications and ranks them on training rounds by

    J = mean over rounds [ relative improvement found − λ · calls / budget ]

The deployed policy is always a candidate and wins training ties. By default `[dream] holdout = 1` reserves the newest round of each run; its measurements do not enter the training cache. Only the training winner is checked on that partition. It must improve held-out J and have no higher miss rate than the incumbent. The policy commit records the candidate, deployment decision, and validation scores. With insufficient history the incumbent stays deployed; `holdout = 0` restores unguarded selection.

Repeatedly consulting this moving partition makes it a deployment diagnostic, not an untouched final test set. A miss is an unknown outcome charged as a failed call: that pessimistic surrogate can favor recorded paths or early stopping. A low miss rate is not a guarantee of unbiased policy ranking. Configuration and base-commit compatibility are checked before pooling replay histories.

To preserve data collection, set `[explore] min_calls` to a positive floor (the generated templates do this). If a policy stops before the floor, best-node refinement supplies the remaining proposals. The floor is part of the stored policy and is applied during replay too. Setting it equal to `budget` enables comparisons with equal proposal budgets. It does not guarantee diverse exploration.

```bash
git dream replay "refine_best" "ucb:c=0.05" --holdout 1
cargo run --release --locked --example compare -- --seeds 100 > results.jsonl
python3 research/validation/summarize.py results.jsonl
```

See [the validation protocol and results](docs/validation.md) for independent prospective evaluations, random search, fixed UCB, successive halving, equal allocation, costs, and paired uncertainty intervals.

### Writing new policies (in any language)

The paper's real lever is a **policy-development agent** that writes *new* policy code after reading the history. There are two ways to add a family:

1. **In Rust:** add a variant in [src/policies.rs](src/policies.rs) and rebuild.
2. **As an external program:** git-dream starts it once and asks it one JSON line per decision.

```toml
[policies.external.patient]
command = "./target/release/examples/patient"   # run from the repository root
source = "examples/patient.rs"                  # optional: snapshot into each policy commit
timeout = 10                                   # seconds per request/reply (default)
grid = { p = [3, 5, 8] }                  # non-empty grid => searched while dreaming
```

Request: `{"params": {"p": 5}, "seed": 3, "tree": {"goal": "min", "budget": 20, "calls": 4, "nodes": [{"id", "parent", "k", "design", "score", "status", "change", "n_exp"}, ...]}}`
Reply: a node id to continue, or `null` to end the round. The reply must depend only on the request, or replay can't reproduce live rounds.

A complete example in Rust, [examples/patient.rs](examples/patient.rs) (`cargo build --release --example patient`), continues the best attempt and stops after `p` calls without improvement.


```bash
git dream replay "patient:p=5" "repair:c=0.02,fp=0.03,kmax=6,kfail=2,patience=12,eps=0.005,dedup=true" -v
```

A good prompt for the meta-agent: *"Read `git dream status`, `git dream show` for the last rounds and `git dream replay -v`. Find where the deployed policy wastes calls, write a new policy, compare training and held-out replay, then freeze it and compare fresh runs against strong fixed baselines at equal total cost. Do not treat replay gains as evidence of live gains."*

## Does it work?

- **[Prospective synthetic comparison](docs/validation.md):** 100 fresh seeds on each of three landscapes, ten methods, 97 objective evaluations per fixed-budget method. Guarded adaptive search matched fixed refinement's final quality; it did not demonstrate a quality advantage. Multi-start successive halving performed better on these tasks. A separate stopping arm saved calls on some tasks. All traces and paired intervals are included.
- **[Inspectable coding example](docs/case-study-log-count.md):** a session-authored Rust optimization changed linear key lookup to hash aggregation. Fresh confirmation means were 25.16 ms before and 1.16 ms after on the included fixture. A deliberate incorrect control failed. The full Git history, notes and policy are bundled. The adapter replays supplied candidates; this is not an autonomous LLM search or evidence for adaptive policy superiority.

Earlier evidence (different prototype/configurations, not rerun under the new gate):

- **[Case study: high-load hash tables](docs/case-study-hashbench.md).** Run with the research prototype this tool came from: 722 real benchmark runs, all stored in git. With the second-generation `repair` policy, dreaming cut the remaining gap to the true optimum from **14.6% to 8.4% with 26% fewer evaluations** (100 synthetic replicates, paired difference +0.061 ± 0.022). Live dreamt rounds found the optimum in 16–17 calls, where the fixed policy spent 40. The first policy families gave **no** gain. Strict replay did **worse** than not dreaming, and replay ranked policies poorly (Spearman ≈ 0.1).
- **Toy example** (`examples/toy-space`, 10 agent salts × 6 rounds of 25 calls, `--no-dream` vs dreaming): **104 vs 150 evaluations**, gap to the optimum **10.9% vs 13.6%**, optimum reached in 5/10 vs 4/10 runs. Ten replicates is too few to call the quality difference real; the call reduction is consistent.

## Limitations

- **Replay knows only what was recorded.** It can't predict untried changes. With a stochastic agent (an LLM), only recorded decisions replay (`strict`), and the case study found strict dreaming can do harm. Use prospective evaluation before relying on an adapted policy; held-out replay does not correct behavior-policy selection or missing support.
- **Early stopping can lock in.** Once rounds stop improving, the cheapest policy on replay is one that stops early. It then records too little new history to escape a local optimum. This showed up in the toy runs. Use `min_calls`, full-budget control runs, and independent data collection. Continuing an already adapted run with `--no-dream` freezes its current policy; a fresh named run is needed for an initial-policy baseline.
- **Replay is a filter, not an oracle.** Its policy ranking correlated weakly with true performance in the case study.
- **Reproducibility has boundaries.** Git preserves source and recorded outcomes, not the machine, toolchain, dependencies, model service, or random output of an external agent. Pin these separately. Ignored build caches can affect timing.
- **External policies:** invalid replies and process failures abort replay instead of earning a stopping reward. Each request/reply has a configurable deadline (default 10 seconds); timeouts kill the policy process group and abort the operation.
- **Noisy benchmarks:** use `remeasure_root = true` and `normalize = "root"` against machine drift, and `confirm_top = k` to reduce promotion of lucky measurements.
- The evaluator runs whatever command the config names, and a command agent can edit anything in its worktree. Run untrusted configs in a sandbox.

## Repository layout

```
src/                memory.rs (git layout), engine.rs (config, rounds, live world), sim.rs (replay, dream),
                    policies.rs, agents.rs (space/command agents, evaluator), rng.rs, main.rs (CLI)
tests/e2e.rs        end-to-end tests on the examples in throwaway repositories (cargo test)
examples/           toy-space (parameter search), toy-command (code-editing agent), hashbench (case-study config)
docs/               case studies, validation protocol and limitations
research/hashbench  archived Python prototype, raw results and benchmark patch behind the case study
```

License: GPL-3.0-or-later.
