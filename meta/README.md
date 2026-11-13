# meta/: let a slow, unlimited local agent improve git-dream using git-dream

The idea: tokens are free and time is cheap, so run git-dream **on its own repository**. opencode is
the discovery agent, and a fixed evaluator measures whether each attempt made git-dream better.
Every attempt, failed ones included, ends up in git.

| track  | config               | evaluator              | what "better" means |
|--------|----------------------|------------------------|---------------------|
| policy | `meta/policy.toml`   | `meta/eval-policy.sh`  | lower regret of the adaptive arms in `examples/compare.rs` at an equal budget |
| speed  | `meta/speed.toml`    | `meta/eval-speed.sh`   | same output, byte-identical, in less wall time |
| scale  | not yet: see `AGENTS.md` | | bootstrap interactively first |

The agent's instructions live in `AGENTS.md`, which opencode reads automatically.

## Setup

The loop, builds and evaluations run locally. Only the model runs elsewhere: opencode's configured
provider (e.g. an OpenAI-compatible `baseURL` on another machine) does the thinking.

Needs Rust (stable), git ≥ 2.31, python3, `gh`, and opencode with `--auto` working against your local model.
The clone is only needed because git-dream's own source is what gets improved. The harness itself is the
release binary. To *use* git-dream on other projects, no clone is needed at all:

```sh
gh release download -R timohuovinen/git-dream -p '*x86_64-unknown-linux-musl.tar.gz'
tar xzf git-dream-*.tar.gz && install git-dream-*/git-dream ~/.local/bin/   # then: git dream init
```

```sh
git clone git@github.com:timohuovinen/git-dream.git git-dream-meta   # dedicated clone
cd git-dream-meta
sh meta/setup.sh                              # pinned binary from this commit + speed reference
git add meta && git commit -m "meta: speed reference"
export OPENCODE_MODEL=strata/<your-model>     # optional; otherwise opencode's default
G=~/.local/git-dream-pinned/bin/git-dream     # NOT the code under edit
```

Smoke-test both evaluators on the base commit before spending days of agent time:

```sh
sh meta/eval-policy.sh    # last line: {"score": ..., "status": "ok", ...}
sh meta/eval-speed.sh
```

## Run (e.g. in tmux; it takes hours per attempt)

```sh
$G run -n 10 --config meta/policy.toml
$G status --run meta-policy
$G show --run meta-policy
git diff HEAD dream/meta-policy/best
```

Run tracks **one at a time** in a clone. They share `target/` contention and the CPU, and the CPU
noise ruins speed timings. For parallel tracks, use separate clones on idle cores.

Tuning knobs:
- `[agent] timeout`: per attempt, 6 h by default.
- `[explore] budget`: attempts per round.
- `META_SEEDS`: evaluator seeds, 30 by default. More seeds give a less noisy score but a slower evaluation.

## Taking a winner (human step, don't skip)

1. Read the diff and the commit message (`git show dream/meta-policy/best`).
2. **Final check on held-out seeds that the search never saw**:
   ```sh
   cargo run --release --locked --example compare -- --seeds 100 --first-seed 10000 > final.jsonl
   python3 research/validation/summarize.py final.jsonl
   ```
   Compare that against `research/validation/summary.md`. Training-seed gains that disappear here
   are overfitting. Better still, also try a landscape the agent has never seen.
3. Cherry-pick into a branch, then rerun `meta/setup.sh` so the speed reference follows the new behaviour.

## Caveats

- The policy track optimises a synthetic 32×32 benchmark. That's a proxy, and the rule in
  `AGENTS.md` against special-casing it is a request, not an enforcement. The held-out check is what
  catches overfitting.
- `protected` stops edits to the evaluator. It is not a sandbox: an attempt's code still runs with
  your user's permissions during `cargo test`. Use a dedicated user or container if that matters.
- With a stochastic LLM agent, dreaming can only replay recorded edges (`mode = "strict"`).
  The policy that picks *which git-dream attempt to continue* gains little early on. The
  value is mostly the preserved, readable history the agent reasons over.
