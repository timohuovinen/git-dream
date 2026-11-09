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

The archived scripts retain their original selection and measurement rules. They
do not implement the Rust tool's held-out deployment gate, exploration floor,
or fresh-only confirmation. The historical results are not measurements of those
new safeguards.

For the current comparisons against random search, fixed UCB, successive halving,
and equal allocation, see the [validation protocol](../../docs/validation.md),
[raw prospective traces](../validation/results.jsonl), and
[paired comparisons](../validation/summary.md). The independent
[coding case study](../../docs/case-study-log-count.md) includes a portable Git bundle.
