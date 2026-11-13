---
name: git-dream
description: Use git-dream to search for the best solution to a measurable coding problem by trying many variants as git commits, scoring each with a fixed evaluator, and letting an exploration policy choose what to try next. Use when the user wants to optimize a benchmark, tune parameters, or have an agent iterate many attempts and keep the best; when a repo has a .git-dream.toml; or when asked to run, inspect, or improve git-dream itself.
---

# git-dream

git-dream (`git dream`, or the `git-dream` binary) runs an experimental search inside any git repo:

- a fixed **agent** proposes a change;
- a fixed **evaluator** scores it;
- an **exploration policy** picks which attempt to continue next and when to stop.

Each attempt becomes a commit under `refs/dream/<run>/rNN/nNNN`. Scores are kept in commit trailers
and evaluator output in git notes. The user's checkout and branch are never touched. Between
rounds, git-dream "dreams": it replays candidate policies over the recorded history and deploys a
better one only if it passes a held-out check.

## Install (no clone needed)

```sh
gh release download -R timohuovinen/git-dream -p '*x86_64-unknown-linux-musl.tar.gz'   # aarch64 also exists
tar xzf git-dream-*.tar.gz && install git-dream-*/git-dream ~/.local/bin/
git dream --version
```

## Set up a project

1. **Define success before anything else.** The evaluator is the whole contract. It must:
   - be deterministic, or repeated enough to be stable;
   - print the score as the **last stdout line**, either a number or `{"score": N, "status": "ok", ...}`;
   - exit non-zero, or print `"score": null`, when the attempt is invalid. Run the correctness tests
     *before* measuring, e.g. `cargo test -q >/dev/null 2>&1 && ./bench.sh`.
2. Run `git dream init --kind space` (parameter search) or `git dream init --kind command` (an agent
   edits code). Then edit `.git-dream.toml` and commit it. Runs start from HEAD, so anything
   uncommitted is invisible to them.
3. List the evaluation contract under `[evaluate] protected` (tests, the bench script, fixtures) so
   that attempts which edit it are rejected.
4. Run the evaluator once by hand on HEAD before starting a long run.

A minimal command-agent config, using opencode as the agent:

```toml
run = "main"
[agent]
kind = "command"
command = 'opencode run --auto --archived --title "dream $DREAM_RUN" "$DREAM_PROMPT"'
prompt = "Make ./bench.sh faster. Keep `cargo test` passing. Make one focused change."
timeout = 21600           # seconds per attempt; generous for slow local models
[evaluate]
command = "cargo test -q >/dev/null 2>&1 && ./bench.sh"
goal = "min"              # or "max"
timeout = 3600
remeasure_root = true     # noisy timings: re-measure the incumbent
confirm_top = 2           # re-run the top candidates before promoting
protected = ["bench.sh", "tests"]
[explore]
budget = 4                # proposals per round
min_calls = 2             # stops policies quitting too early
[dream]
mode = "strict"           # an LLM agent is stochastic: only recorded edges replay
holdout = 1
```

For parameter search (`kind = "space"`), set `[agent] start` and `[agent.knobs]` grids. The
evaluator reads `$DREAM_PARAMS` (JSON) or `$DREAM_PARAM_<NAME>`. There, `[dream] mode = "cache"` is valid.

## Run and inspect

```sh
git dream run -n 3                 # rounds of dream -> explore -> promote
git dream run -n 3 --run base --no-dream   # fixed-policy baseline for comparison
git dream status [--run NAME]      # scores per round, deployed policy, best attempt
git dream show [ROUND] [--run NAME]  # one round's discovery tree
git show dream/<run>/best          # the best attempt is a normal commit
git diff HEAD dream/<run>/best     # what it changed; cherry-pick or merge it deliberately
git dream replay "refine_best" "ucb:c=0.05" -v --holdout 1   # compare policies offline
git dream policies                 # built-in families and grids
```

Runs can take hours. Start them in tmux or in the background, and check `status` instead of
waiting. `git dream clean --run NAME --yes` deletes a run, which is irreversible: ask the user first.

## When you ARE the agent inside a run

The environment gives you three things:

- `$DREAM_PROMPT`: the task and your position in the tree;
- `$DREAM_CONTEXT`: a file with every earlier attempt, its write-up, its score and the evaluator
  output for failures;
- `$DREAM_MESSAGE_FILE`: where your proposal goes.

1. Read `$DREAM_CONTEXT` first. Don't repeat a measured idea unless you can show that its
   implementation, rather than the idea, failed (`git show <sha>`).
2. Make one focused change. Never edit protected paths.
3. Write `$DREAM_MESSAGE_FILE`: a one-line summary, a blank line, then the idea, the evidence from the
   history, why it isn't a repeat, and any numbers you measured.

## Honest interpretation

- Replay gains are not live gains. Before adopting a winner, re-measure it fresh and compare it
  against a `--no-dream` baseline at equal cost.
- The best attempt is selected over many noisy measurements, so it is optimistically biased. Use
  `confirm_top` and verify it on inputs the search never saw.
- Policies that stop early can lock the search into a local optimum; keep `min_calls` above 0.

## Improving git-dream itself

The git-dream repo has a self-improvement kit. See `meta/README.md` and `AGENTS.md`:

- `meta/policy.toml`: better search quality;
- `meta/speed.toml`: same output, faster.

Run them with the pinned binary (`~/.local/git-dream-pinned/bin/git-dream`, installed by
`meta/setup.sh`), never with a binary built from the code under edit.
