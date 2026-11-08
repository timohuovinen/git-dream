# Archived research prototype (hashbench case study)

The original experiment behind [docs/case-study-hashbench.md](../../docs/case-study-hashbench.md),
kept as the record of how its numbers were produced. It is **not** part of the
`git-dream` tool (the Rust crate at the repository root) and is not maintained.

- `*.py`: the prototype (Python, stdlib only). It ran inside a private hash-table
  benchmark crate and expects that crate's layout (`target/release/dream-eval`).
- `dream-eval.rs`, `hashbench-funnel-simd-groups.patch`: the evaluator binary and
  the benchmark changes it needed.
- `results/`: raw outputs: live experiment log, full oracle grid (982 designs),
  synthetic replicates and the replay-fidelity study.

To run the same task with the tool, see [examples/hashbench](../../examples/hashbench/).
