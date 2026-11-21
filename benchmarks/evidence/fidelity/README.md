# Recorded output of the `fidelity` benchmark

Can replay be trusted to choose a better policy? One recorded run of [fidelity.rs](../../fidelity.rs), whose
header describes the method and the verdict rule (fixed before the run).

- [Interpretation](../../../docs/validation.md#can-replay-choose-a-better-policy).
- [v1.txt](v1.txt): the report. 100 histories for each of five recording policies; for each, strict and cache
  replay and three rules for misses.
- [v1.jsonl](v1.jsonl): one record per history, with every policy's replay scores and true scores (32 MB).
  `--summarize v1.jsonl` prints the report again without re-running.
- [provenance.json](provenance.json): command, toolchain, base revision, and SHA-256 hashes.

```sh
cargo run --release --locked --example fidelity -- --reps 100 --out benchmarks/evidence/fidelity/v1
cargo run --release --locked --example fidelity -- --summarize benchmarks/evidence/fidelity/v1.jsonl
```

Result: replay selection was trustworthy in none of the 30 cells. In 23 the replay pick was no better than
keeping `refine_best`; in 7 it was worse.

The histories use the tool's parameter agent as it is, as a real run of
[examples/hashbench](../../../examples/hashbench/.git-dream.toml) would. That agent proposes moves of knobs that
do nothing while `L = 0` (see [../hashgrid/](../hashgrid/README.md)), which `hashgrid` no longer does by default.
Replay runs the agent inside the tool, so the same correction here needs a change to the tool. It has not been
made, and this result has not been checked with the case study's agent.

One measured landscape, a deterministic agent, policies sampled from the built-in families.
