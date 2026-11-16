# Hashbench case study: recorded results

Results of the original experiment behind [docs/case-study-hashbench.md](../../../docs/case-study-hashbench.md),
which was run with a Python prototype inside a private hash-table benchmark crate.

- `oracle.jsonl`: the measured score of all 982 designs. The benchmarks
  [hashgrid.rs](../../../benchmarks/hashgrid.rs) and [fidelity.rs](../../../benchmarks/fidelity.rs)
  and the numbers in [docs/validation.md](../../../docs/validation.md) are built on it.
- `oracle-repeats.jsonl`: repeat measurements of the same grid.

The evaluator source and the benchmark patch are one folder up, with the config that uses them.

Not in the working tree any more, kept in git history up to commit `7d0167c`
(under `research/hashbench/`): the prototype scripts (`dream.py`, `gitmem.py`,
`policies.py`, `sim.py`, `space.py`, `tree.py`) and their raw outputs
(`results/experiment.log`, `results/syn-*.out`, `results/synthetic-*.json`,
`results/fidelity-dream-r10.json`). To look at one:
`git show 7d0167c:research/hashbench/dream.py`.

The prototype did not implement the Rust tool's held-out deployment gate,
exploration floor, or fresh-only confirmation: its results are not measurements
of those safeguards.

The prototype's replay-fidelity check (one history, no interval, the pick's
percentile without the incumbent's) is replaced by `benchmarks/fidelity.rs`,
which scores many independent histories with the tool's own replay:

    cargo run --release --example fidelity -- --smoke
    cargo run --release --example fidelity -- --reps 100 --out benchmarks/evidence/fidelity/v1

The full evaluation (the second command) has not been run yet.

For the current comparisons against random search, fixed UCB, successive halving,
and equal allocation, see the [validation protocol](../../../docs/validation.md)
and its [recorded output](../../../benchmarks/evidence/compare/). The
[coding case study](../../../docs/case-study-log-count.md) includes a portable Git bundle.
