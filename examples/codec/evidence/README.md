# codec: four exploration strategies with a real coding agent

Recorded runs of [the codec task](../README.md) with Claude Haiku 5.5 (`claude-haiku-5-5`, Claude Code 2.1.325,
headless) as the agent. Interpretation: [docs/validation.md](../../../docs/validation.md#a-real-coding-agent).

Each run starts from the same commit and spends 48 attempts, 4 at once. The score is a byte count, so
evaluations and whole runs overlapped without affecting it.

| arm | strategy |
|---|---|
| `fixed` | `refine_best`: always continue the best attempt; one tree of 48 |
| `bandit` | git-dream's default policy; one tree of 48 |
| `chains` | four independent chains, six deep, per round; two rounds (the Dream-RSI paper's fixed baseline) |
| `paper` | `chains` in round 1, then the policy that scores best on strict replay of the recorded rounds; rounds until 48 attempts are spent |

## What is here

- [run.sh](run.sh): one run. `sh run.sh WORK ARM REPEAT [TOTAL] [STEP]`.
- [haiku/](haiku/): 16 runs, 4 per arm (768 attempts, $15.83 of agent calls).
  - `<arm>-<repeat>.bundle`: the run's whole repository: every attempt as a commit, its write-up, score and
    evaluator output.
  - [haiku/report.md](haiku/report.md): the comparison, printed by [benchmarks/runs.rs](../../../benchmarks/runs.rs).
  - [haiku/runs.jsonl](haiku/runs.jsonl): one record per run (best score after each attempt, rounds, tokens,
    cost, replay support).

Look at a run:

```sh
git init /tmp/codec-run && cd /tmp/codec-run
git fetch /path/to/git-dream/examples/codec/evidence/haiku/bandit-1.bundle 'refs/*:refs/*'
git log --graph --oneline refs/dream/main/r01/n048     # the path to one attempt
git show dream/main/best:codec.rs                      # the best codec of the run
git notes --ref=dream show dream/main/best             # what the evaluator printed for it
```

Repeat the comparison (about 25 minutes per run; runs can overlap):

```sh
cargo build --release --examples          # the chains policy and the `runs` report
for arm in fixed bandit chains paper; do sh examples/codec/evidence/run.sh /tmp/codec-work $arm 1; done
cargo run --release --example runs -- /tmp/codec-work
```

## What is not here

Eight repeats per arm were planned. Repeats 5 to 8 were stopped: the Claude runs when their token allowance
ran low (8 runs, 23 to 46 attempts in), and their replacements with Codex (`gpt-6-luna`) when Codex stopped
answering after 171 calls (8 runs, about 16 real attempts each). Neither set is complete, so neither is
reported. `agent.sh` runs either agent (`CODEC_AGENT=codex`).

The recorded configurations name the experiment's own paths for the `chains` policy; build it and pass
`CHAINS=` to rerun.
