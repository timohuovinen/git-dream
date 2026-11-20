# git-dream

**Git-native experimental search for coding agents.** Preserve every attempt, inspect failures, reproduce evaluations, and compare exploration strategies using Git history.

Use a fixed agent to propose changes and a fixed evaluator to score them. git-dream records the search as commits, keeps your checkout separate, and decides which attempt to continue next. That decision is the *exploration policy*, and it is the part that improves.

> **Goal:** a search that gets better at *how it searches*, without paying for more evaluations (the goal of the Dream-RSI paper, below), while using existing tooling from git instead of re-implementing it.

[Dream-RSI](https://www.dream-rsi.com/) ("Recursive Self-Improvement through Evolving Worlds", Google / Google DeepMind, 2026) uses three parts:

- a fixed **discovery agent** proposes changes;
- a fixed **evaluator** scores them;
- an **exploration policy**, which is ordinary code, decides which attempt to continue next and when to stop.

In the paper the policy improves between rounds, and no evaluation is spent on it. Candidate policies are **dreamt**: replayed offline over the recorded attempts, and the best one is deployed. A **policy-development agent** writes new policy code from the same record.

git-dream builds each of those parts from something git already does:

| Dream-RSI | git-dream | git |
|---|---|---|
| the record of every attempt | one commit per attempt, the outcome in its trailers, the evaluator's output in a note | commits, trailers, notes, hidden refs |
| where an attempt is made | a scratch checkout per attempt in flight | `git worktree` |
| the policy, as versioned code | a branch whose tree *is* the policy: one commit per version, one tag per round | branch, tags |
| the policy-development agent | a second search, of the policy's tree: each candidate policy is a commit, scored by replay | the same commits, refs and worktrees |
| deploying a policy | a commit on the policy branch with the winning candidate as its second parent | a merge commit |
| looking at a search | `git dream show` is `git log --graph`; attempts are compared with `git show` and `git diff` | |
| sharing and storing a search | the refs are pushed, fetched and bundled; git packs them | `git push`, `git fetch`, `git bundle`, `git maintenance` |

Three things improve the policy, none of which evaluates anything (see [Policies](#policies)):

- **The default `bandit` policy learns which kind of move pays.** Every attempt is a move of some type: continue the best attempt, a runner-up, a lower-ranked or a failed one, for the first or the n-th time. The policy keeps the measured rate at which each type produced a new best, and continues the attempt whose type currently looks most productive.
- **Dreaming fits that table from history** (the *yield model*). Every number in it is an average over attempts that were really evaluated, so nothing is extrapolated. `git dream dream --dry-run` prints it with intervals. With `[dream] history = "all"` it is fitted over every run in the repository, so a new run starts from what the earlier ones measured.
- **A policy-development agent rewrites the policy** (`[develop]`, off until you name an agent). It edits the policy's parameters or writes a program; each candidate is scored by replay over the recorded attempts, and the winner is deployed only if it also does better on rounds it was not scored on.

One caution, from git-dream's own evidence: replay cannot tell whether a different policy would have done better. History only answers that where the candidate repeats what was recorded. That is why the default policy learns from measured outcomes instead of replay, and why a replay winner has to pass a held-out check. See [Does it work?](#does-it-work).

A round can also be **extended** instead of restarted, so runners-up and failed attempts stay continuable. On the two benchmarks here this matters more than anything else.

git-dream is a single Rust binary that works in **any git repository**. The agent can be a deterministic parameter-search operator, or any program that edits code, such as an LLM coding agent. Everything is stored as ordinary git objects under `refs/dream/*`. It creates `dream/*` branches and refs while leaving your checked-out branch and files alone.

```
           ┌──────────────────────── round r ────────────────────────┐
 history ─▶│ dream (0 evaluations): fit the yield model,             │─▶ policy r ─▶ explore: agent + evaluator
  (git)    │ and let the policy-development agent rewrite the policy │   (a commit)  (one commit per attempt)
    ▲      └─────────────────────────────────────────────────────────┘                        │
    └───────────────────────── remember: refs/dream/<run>/rNN/nNNN ◀──────────────────────────┘
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

**Coding-agent skill:** [skills/git-dream](skills/git-dream/SKILL.md) lets opencode, Claude Code, Codex or Gemini handle "dream up X" in any repo. It designs an evaluator with you, then launches a detached run that uses the same agent for attempts (`skills/git-dream/scripts/dream`). Symlink the directory into the agent's skills folder, e.g. `~/.claude/skills/git-dream`, which opencode reads too.

Existing users: see [configuration and behavior changes](docs/compatibility.md), especially the default held-out gate and confirmation accounting.

## Quick start (seconds)

```bash
cp -r git-dream/examples/toy-space /tmp/toy && cd /tmp/toy
git init -q && git add -A && git commit -qm init
git dream run -n 5          # 5 rounds of: dream -> explore -> promote
git dream run -n 2 --extend # 2 more steps in the last round's tree (every attempt stays continuable)
git dream dream --dry-run   # what was learned: the measured yield of each kind of move
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

With `[explore] parallel = N`, N agents work at once, each in its own worktree (`$DREAM_LANE` is 0 to N−1, for anything that must not collide, such as a port). See [Running attempts at once](#running-attempts-at-once).

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
parallel = 4                      # four agents at once; results are still measured one by one (see below)

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

### Running attempts at once

An attempt is an agent run followed by an evaluation. Attempts are independent once the policy has chosen them, so they can overlap:

```toml
[explore]
parallel = 4             # attempts in flight, each in its own worktree (default 1)
evaluations = "alone"    # what may run while an evaluation runs (default "alone")
```

`evaluations` says how much isolation the score needs. git-dream cannot know this, so you state it:

| `evaluations` | while an evaluation runs | use it when |
|---|---|---|
| `"alone"` | nothing else runs. Attempts go in waves: N agents work at once, then their results are measured one by one, then the next N are chosen | the score is a timing, or anything else that machine load changes |
| `"serial"` | agents may be working, no other evaluation | the evaluator needs an exclusive resource (a port, a device) but load does not change the score |
| `"parallel"` | anything | the score does not depend on load: a test count, an accuracy, a size |

A parameter search (`kind = "space"`) proposes instantly, so `parallel` only does something there with `evaluations = "parallel"`. `git dream run --parallel N` overrides the setting for one invocation.

What changes when attempts overlap:

- **The policy chooses with less information.** It picks an attempt without the results of those still running. It sees them as `pending`: they count as calls, and the `bandit` policy counts each as a move that has not paid yet, which steers the others towards different kinds of move. On the measured grid this cost little per evaluation ([numbers](docs/validation.md#attempts-at-once)), but a wave can only build on earlier waves: keep `parallel` well below the number of attempts.
- **Agents started together cannot see each other's work.** Each is told how many others started from the same attempt and asked to prefer a change they are unlikely to make. Whether LLM agents then avoid duplicates has not been measured.
- **Everything is still recorded.** An attempt is committed as `pending` when it starts and replaced by its result. Each result records which later choices were made without it (`Dream-Seen`), so a round can be extended exactly, and a killed round has no gaps: its unfinished attempts are run again by the next `git dream run` that extends it.
- **Build caches are per worktree.** The first build in each lane is cold.

Work that never depends on your setup is always done on every core: scoring replay candidates while dreaming gives the same ranking on one thread or sixteen. Confirmation re-measurements run at once only with `evaluations = "parallel"`.

## Where everything is stored

| Dream-RSI | git |
|---|---|
| attempt | commit: its tree is the attempt's code / parameters |
| parent attempt | parent commit (a round's root has the previous round's promoted attempt as parent) |
| agent's write-up of the attempt | commit message (subject = summary, body = the rest) |
| outcome | commit trailers `Dream-Score`, `Dream-Status`, `Dream-Failure`, `Dream-Design`, `Dream-Policy`, …; `Dream-Seen` when the result arrived after later attempts had been chosen |
| evaluator output | git notes, `refs/notes/dream`: exit, metrics, first 20 + last 40 lines of stdout and stderr |
| discovery tree | hidden refs `refs/dream/<run>/rNN/nNNN` |
| start of a run | `refs/dream/<run>/base` |
| best attempt | branch `dream/<run>/best` (per round: `refs/dream/<run>/rNN/best`) |
| exploration policy | branch `dream/<run>/policy`: one commit per version, and its tree is the policy. `policy.json` names a built-in family and its parameters, or the command that starts a program kept next to it. `dream.json` says how the version was chosen (yield table, replay ranking, held-out check), `git-dream.toml` is the configuration it ran under. Tags `dream/<run>/policy-rNN` mark the version each round used |
| candidate policies | the run `<run>.policy`, with the same layout as any run: `refs/dream/<run>.policy/rNN/nNNN`, one commit per candidate, its replay score in the trailers. A deployed candidate is the second parent of its policy commit |

Attempts are written with plumbing commands (`hash-object`, `commit-tree`, `update-ref`), which never trigger git's housekeeping. So after each round git-dream runs `git maintenance run --auto`, and git packs the attempts when its own thresholds are reached, as it does after ordinary commits. Tune it with git's settings (`gc.auto`, `maintenance.*`), not git-dream's.

Because all of it is commits and refs, git's own commands read a search:

```bash
git dream show                                             # = git log --graph over the round's refs
git log --first-parent -p dream/main/policy -- policy.json # how the policy changed, version by version
git diff dream/main/policy-r02 dream/main/policy-r03       # two rounds' policies
git show refs/dream/main/r01/n007                          # one attempt: write-up, outcome, change
git notes --ref=dream show refs/dream/main/r01/n007        # what the evaluator printed for it
```

The trees are rebuilt from commit trailers before every dream, so the repository *is* the state. The scratch worktrees are outside the repository, in `~/.cache/git-dream/<repository>/` (or under `$GIT_DREAM_WORKTREES`, `$XDG_CACHE_HOME`, `%LOCALAPPDATA%`), and can be deleted at any time; `.git/dream/` holds only temporary files. A policy that is a program runs from one more scratch worktree, checked out from its policy commit. The worktrees are not under `.git` because coding agents refuse to edit files there: Claude Code treats every path below `.git` as a sensitive file, so an agent placed in `.git/dream/wt` could read the code and not change it. Hidden refs aren't pushed by default; `git dream push [remote]` / `git dream fetch [remote]` share runs.

## Commands

| | |
|---|---|
| `git dream init [--kind space\|command]` | write a `.git-dream.toml` template |
| `git dream run -n N [--no-dream] [--budget B] [--parallel P]` | N rounds of dream → explore → promote (`--no-dream` = fixed-policy baseline; `--parallel` = attempts in flight at once) |
| `git dream run -n N --extend` | N steps of B more attempts in the current round's tree; the default with `[explore] extend = true` (new templates), where `--new-round` starts a round |
| `git dream explore [--extend]` / `git dream dream [--dry-run]` | the two halves separately. `dream` learns from the record and deploys the next policy version; with `[develop]` it also runs the policy-development agent |
| `git dream replay ["family:k=v,..." \| JSON ...] [-v] [--holdout N]` | score policies on recorded history |
| `git dream replay --dir DIR` | score the policy in a directory (`policy.json` and its program), exactly as a candidate policy is scored |
| `git dream status` / `git dream show [ROUND]` | summary of a run / one round's tree, drawn by `git log --graph` (`--run <run>.policy`: the candidate policies) |
| `git dream policies` | policy families and their search grids |
| `git dream push` / `fetch [REMOTE]` | share dream refs |
| `git dream clean --run NAME --yes` | delete a run |

Run-oriented commands take `--run NAME` and `--config FILE`; `git dream -C DIR ...` runs in another directory, like `git -C`. Runs are independent by default, e.g. `--run baseline --no-dream` against `--run dream`.

A run is one discovery process, the paper's history H: what dreaming replays and what each agent reads is that run's rounds. That keeps a comparison fair, since a baseline that reads the other arm's attempts is no longer a baseline. To pool knowledge instead, set `[dream] history = "all"`. Recorded rounds then feed both dreaming and the agents' history file, with each attempt labelled by its run, and a new run dreams before its first round. The yield model learns from any recorded run, because it only counts what each attempt did in its own tree. Replay (and so policy development) requires matching agent/evaluator configurations and base commits. It's one setting because the paper has one history. Nothing is copied between runs: the history is read from the runs' refs, and `git show` works across them.

## Policies

Built-in families ([src/policies.rs](src/policies.rs)):

- `bandit` (the default): learns from measured outcomes which kind of move pays. See below;
- `refine_best`: π0, always continue the best attempt;
- `random` (uniform valid-parent selection), `ucb`, `beam`, `softmax`;
- `repair`: continues failed attempts and merges identical designs. It came out of the [case study](docs/case-study-hashbench.md).

### The default: `bandit` and the yield model

A move is "continue node n for the k-th time". Its type is who n is (the `best` attempt, a runner-up `next`, any other scored attempt `rest`, or a `fail`ed one) and how often n was continued before (0, 1–2, 3–6, 7–14, 15+ times). Its outcome is measured: did the attempt become the new best? ([src/model.rs](src/model.rs))

- Within a round, the policy keeps a Beta posterior per move type, updates it after every attempt and continues the node whose type draws the highest yield. Identical designs act as one node. A best attempt that keeps failing loses to its runners-up without any hand-set limit.
- When a new round starts, dreaming fits the same table over all recorded rounds (old attempts count less: `halflife`) and stores it in the policy's spec, so each policy commit shows what was learned and from how many attempts.
- The decision is a pure function of (tree, parameters, seed). A recorded round therefore replays exactly, and `git dream run --extend` continues it as if it had never stopped. It also finishes a round that was killed.

Parameters (`initial = { family = "bandit", params = { ... } }`): `m` prior weight in attempts (4), `focus` how strongly the most productive type is preferred (16; 1 = Thompson sampling, "inf" = greedy), `gamma` credit for the moves that led to a new best (0.5), `eps` minimum relative improvement that counts (0; raise it for noisy evaluators), `halflife` (48 attempts), `stop` end the round when no type is expected to pay at least this often (0 = never).

### Replay-tuned families

With any other initial policy (or `[dream] method = "replay"`), dreaming works as in the paper. A dream samples `candidates` parameterisations from the families' grids. It deduplicates candidate specifications and ranks them on training rounds by

    J = mean over rounds [ relative improvement found − λ · calls / budget ]

The deployed policy is always a candidate and wins training ties. By default `[dream] holdout = 1` reserves the newest round of each run; its measurements do not enter the training cache. Only the training winner is checked on that partition. It must improve held-out J and have no higher miss rate than the incumbent. The policy commit records the candidate, deployment decision, and validation scores. With insufficient history the incumbent stays deployed; `holdout = 0` restores unguarded selection.

Repeatedly consulting this moving partition makes it a deployment diagnostic, not an untouched final test set. A miss is an unknown outcome charged as a failed call: that pessimistic surrogate can favor recorded paths or early stopping. A low miss rate is not a guarantee of unbiased policy ranking. Configuration and base-commit compatibility are checked before pooling replay histories.

To preserve data collection, set `[explore] min_calls` to a positive floor (the generated templates do this). If a policy stops before the floor, best-node refinement supplies the remaining proposals. The floor is part of the stored policy and is applied during replay too. Setting it equal to `budget` enables comparisons with equal proposal budgets. It does not guarantee diverse exploration.

```bash
git dream replay "refine_best" "ucb:c=0.05" --holdout 1
cargo run --release --locked --example compare -- --seeds 100 > results.jsonl
cargo run --release --locked --example summarize -- results.jsonl
```

See [the validation protocol and results](docs/validation.md) for independent prospective evaluations, random search, fixed UCB, successive halving, equal allocation, costs, and paired uncertainty intervals.

### Policy development: an agent that writes the policy

The paper's stronger lever is a **policy-development agent** that writes *new* policy code after reading the history. In git-dream that agent is configured like the discovery agent, and its work is recorded the same way:

```toml
[develop]
command = 'claude -p "$DREAM_PROMPT" --permission-mode acceptEdits'   # any program that edits files
attempts = 4          # candidate policies per dream (default 4)
parallel = 1          # candidates written at once
timeout = 1800
prompt = ""           # added to the built-in instructions
```

While dreaming (before a new round, or `git dream dream`), git-dream runs a second search in the run `<run>.policy`:

1. **The start is the deployed policy**: the tree of the newest commit on `dream/<run>/policy`, checked out in a scratch worktree.
2. **Each attempt is a candidate policy.** The agent edits `policy.json` (other parameters, another built-in family) or writes a program and names the command that starts it: `{"command": "python3 policy.py", "params": {...}}`. It is told the protocol below, given every earlier candidate with its score and failure output, and can score its own work before it finishes.
3. **The score is replay**, J over the recorded rounds, so no evaluation is spent. A policy that cannot be started, or answers with something that is not a node, is a failed attempt with the reason recorded.
4. **The best candidate is deployed if it passes the held-out check**: it must also beat the deployed policy on the newest round of each run, which it was not scored on (`[dream] holdout`, default 1; with fewer than two recorded rounds development waits). Deployment is a commit on the policy branch whose tree is the candidate's and whose second parent is the candidate.

The next round is then searched by that policy. If it is a program, git-dream checks it out from its commit and starts it there.

```bash
git dream show --run main.policy          # the candidates of the latest dream, as a tree
git log --graph --oneline dream/main/policy   # policy versions and the candidates they came from
git dream replay --dir path/to/a/policy   # score a policy directory by hand
```

With a `bandit` policy deployed, the yield model is fitted first and the agent starts from the fitted policy. With any other policy the agent replaces sampling parameters from the grids, unless `[dream] method = "replay"` asks for both.

The agent's command needs whatever permission it takes to run the scoring command it is given (`$DREAM_CHECK`, also spelled out in its prompt); without it the agent writes candidates blind and git-dream still scores them. No policy written by an LLM has been measured here yet: what replay rewards is in [Limitations](#limitations).

### The policy protocol (any language)

A policy that is not a built-in family is a program. git-dream starts it once and asks it one JSON line per decision.

Request: `{"params": {"p": 5}, "seed": 3, "tree": {"goal": "min", "budget": 20, "calls": 4, "nodes": [{"id", "parent", "k", "design", "score", "status", "change", "n_exp"}, ...]}}`
Reply: a node id to continue, or `null` to end the round. The reply must depend only on the request, or replay can't reproduce live rounds.

With `[explore] parallel`, attempts still running appear as nodes with `"status": "pending"` and no score. They count as calls and in their parent's `n_exp`, and they can't be continued. After `null`, git-dream asks again when a running attempt finishes, and the round ends when nothing is running.

There are three places a policy can live:

1. **In the policy tree**, as above: written by the policy-development agent or by you, versioned on the policy branch, started from its checkout.
2. **In your repository**, as a named family that dreaming can search by its grid:

   ```toml
   [policies.external.patient]
   command = "./target/release/examples/patient"   # run from the repository root
   source = "policies/patient.rs"                  # optional: snapshot into each policy commit
   timeout = 10                                   # seconds per request/reply (default)
   grid = { p = [3, 5, 8] }                  # non-empty grid => searched while dreaming
   ```

3. **In Rust:** add a variant in [src/policies.rs](src/policies.rs) and rebuild.

A complete example in Rust, [policies/patient.rs](policies/patient.rs) (`cargo build --release --example patient`), continues the best attempt and stops after `p` calls without improvement.

```bash
git dream replay "patient:p=5" "repair:c=0.02,fp=0.03,kmax=6,kfail=2,patience=12,eps=0.005,dedup=true" -v
```

Replay gains are not live gains: after developing a policy, freeze it and compare fresh runs against strong fixed baselines at equal total cost.

## Does it work?

- **[Yield-model bandit](docs/validation.md#yield-model-bandit-and-extended-rounds):** on seeds never used in development, at equal evaluations. Synthetic rugged landscape: mean regret 57.4 with `refine_best`, 21.8 with `bandit`, 12.3 when the round is extended instead of restarted. Measured hash-table grid (982 designs, 4 in 10 fail, 6% noise): gap to the best design 16.0% → 4.0% in 8 rounds of 40, and 6.1% → 2.2% in one extended round, where the hand-written `repair` policy does as well (2.3%). No gain on the deceptive landscape, where successive halving with random restarts stays far ahead. Refitting the model between rounds only helped when rounds were very short. Deterministic agents only; for an LLM agent see the next point.
- **[A real coding agent](docs/validation.md#a-real-coding-agent):** Claude Haiku 5.5 improving a Rust compressor, 4 strategies × 4 runs × 48 attempts. No strategy was distinguishable from another: `bandit` ended 37 ± 1,476 bytes from fixed refinement on a score near 33,500. The agent did most of the work in its first attempt. A wide record (the paper's independent chains) kept 15% of candidate policies inside the record against 1 to 4% for a narrow one, but in 13 of 16 runs the replay winner was a policy that spends less, and deploying it did not help. Every attempt is in the bundled repositories.
- **[Attempts at once](docs/validation.md#attempts-at-once):** on the same measured grid, 320 evaluations run 16 at a time (20 waves instead of 320 steps) ended 2.6% from the best design, against 2.2% one at a time. The same 20 steps spent one attempt at a time ended 52% away. With only 40 evaluations the cost of choosing blind is larger (22% → 33% at 16 at once). Simulated with a deterministic agent; on a real toy repository the wall-clock time for 24 attempts fell from 13.4 s to 2.4 s with 8 at once.
- **[Replay selection](docs/validation.md#can-replay-choose-a-better-policy):** on 100 independent histories for each of five ways of recording them, the policy that scored best on replay was never reliably better than `refine_best` once deployed: no better in 23 of 30 settings, worse in 7. Cache replay found policies that were better on the recorded rounds, and the gain did not carry over. This is why replay is no longer the default.
- **[Prospective synthetic comparison](docs/validation.md):** 100 fresh seeds on each of three landscapes, ten methods, 97 objective evaluations per fixed-budget method. Guarded adaptive search matched fixed refinement's final quality; it did not demonstrate a quality advantage. Multi-start successive halving performed better on these tasks. A separate stopping arm saved calls on some tasks. All traces and paired intervals are included.
- **[Inspectable coding example](docs/case-study-log-count.md):** a supplied Rust optimization changed linear key lookup to hash aggregation. Fresh confirmation means were 25.16 ms before and 1.16 ms after on the included fixture. A deliberate incorrect control failed. The full Git history, notes and policy are bundled. The adapter replays supplied candidates; this is not an autonomous LLM search or evidence for adaptive policy superiority.

Earlier evidence (different prototype/configurations, not rerun under the new gate):

- **[Case study: high-load hash tables](docs/case-study-hashbench.md).** Run with the research prototype this tool came from: 722 real benchmark runs, all stored in git. With the second-generation `repair` policy, dreaming cut the remaining gap to the true optimum from **14.6% to 8.4% with 26% fewer evaluations** (100 synthetic replicates, paired difference +0.061 ± 0.022). Live dreamt rounds found the optimum in 16–17 calls, where the fixed policy spent 40. The first policy families gave **no** gain. Strict replay did **worse** than not dreaming, and replay ranked policies poorly (Spearman ≈ 0.1).
- **Toy example** (`examples/toy-space`, 10 agent salts × 6 rounds of 25 calls, `--no-dream` vs dreaming): **104 vs 150 evaluations**, gap to the optimum **10.9% vs 13.6%**, optimum reached in 5/10 vs 4/10 runs. Ten replicates is too few to call the quality difference real; the call reduction is consistent.

## Limitations

- **The yield model is coarse.** It knows 20 kinds of move, not your problem. It cannot plan a detour through worse attempts, and with fewer than a few dozen attempts it is mostly its prior. Its parameters were chosen on the two benchmarks it is reported on.
- **Short rounds discard the search.** A new round keeps only the promoted attempt. Prefer `extend = true`; start a new round when scores from earlier attempts stop being comparable (machine drift, changed evaluator inputs).
- **Replay knows only what was recorded.** It can't predict untried changes. With a stochastic agent (an LLM), only recorded decisions replay (`strict`), and the case study found strict dreaming can do harm. Use prospective evaluation before relying on an adapted policy; held-out replay does not correct behavior-policy selection or missing support.
- **Policy development is scored by replay, with replay's limits.** Replay can score a policy that does less of what was recorded, not one that does something else with the same attempts, so the cheapest way to win is to stop early. The held-out check, `min_calls` and the instructions given to the agent push against that; they do not remove it. The check needs two recorded rounds, so a run that only ever extends one round never develops. The mechanism is tested with a scripted developer; no LLM-written policy has been measured here.
- **A developed policy is a program that git-dream runs.** It is started with your permissions, from a checkout of its commit, on every decision. Read `git show dream/<run>/policy` before trusting a run someone else pushed.
- **Early stopping can lock in.** Once rounds stop improving, the cheapest policy on replay is one that stops early. It then records too little new history to escape a local optimum. This showed up in the toy runs. Use `min_calls`, full-budget control runs, and independent data collection. Continuing an already adapted run with `--no-dream` freezes its current policy; a fresh named run is needed for an initial-policy baseline.
- **Replay is not an oracle, and here not a filter either.** Its ranking of policies correlated weakly with their true performance (Spearman 0.03 to 0.37), and its pick was not better than the deployed policy ([numbers](docs/validation.md#can-replay-choose-a-better-policy)).
- **Attempts at once trade information for time.** Each choice is made without the results still in flight. With `evaluations = "alone"` and a deterministic agent the round is still reproducible; otherwise which results a choice knew depends on timing (it is recorded, so the round can be extended, but a second run may differ). Replay scores rounds one attempt at a time, so it does not reproduce a round recorded with several at once. Identical designs proposed at the same time are each measured, even with `reuse = true`. Nothing here has been measured with LLM agents.
- **Reproducibility has boundaries.** Git preserves source and recorded outcomes, not the machine, toolchain, dependencies, model service, or random output of an external agent. Pin these separately. Ignored build caches can affect timing.
- **External policies:** invalid replies and process failures abort replay instead of earning a stopping reward. Each request/reply has a configurable deadline (default 10 seconds); timeouts kill the policy process group and abort the operation.
- **Noisy benchmarks:** use `remeasure_root = true` and `normalize = "root"` against machine drift, and `confirm_top = k` to reduce promotion of lucky measurements.
- The evaluator runs whatever command the config names, and a command agent can edit anything in its worktree. Run untrusted configs in a sandbox.

## Repository layout

```
src/                the tool: memory.rs (git layout), engine.rs (config, rounds, running attempts), model.rs (yield
                    model, bandit), sim.rs (replay), policies.rs, agents.rs (space/command agents, evaluator),
                    par.rs (jobs at once), rng.rs, main.rs (CLI)
tests/e2e.rs        end-to-end tests on the examples in throwaway repositories (cargo test)
examples/           projects to run `git dream` on, one folder each: toy-space (parameter search), toy-command
                    (code-editing agent), log-count (a Rust optimization), codec (a Rust compressor improved by an
                    LLM agent), hashbench (config, evaluator source and patch for the hash-table crate). `<project>/evidence/` holds what a recorded run of it produced
benchmarks/         programs that measure the tool itself: compare.rs, hashgrid.rs and fidelity.rs (prospective
                    comparisons), summarize.rs and score.rs (reports), runs.rs (compares runs recorded with a real
                    agent). Run as `cargo run --release --example <name>`.
                    benchmarks/evidence/<benchmark>/ holds a benchmark's recorded output (the numbers in
                    docs/validation.md)
policies/           external policies, written as standalone programs: patient.rs (a sample), chains.rs (the
                    paper's fixed strategy: independent chains from the root)
docs/               case studies, validation protocol and limitations
meta/               git-dream applied to its own source (configs and evaluators for self-improvement runs)
skills/             the git-dream agent skill
```
