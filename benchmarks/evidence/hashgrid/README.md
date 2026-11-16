# Recorded output of the `hashgrid` benchmark

Policies compared at equal evaluations on the measured hash-table grid
([oracle.jsonl](../../../examples/hashbench/evidence/oracle.jsonl): 982 designs, about 4 in 10 fail),
6% measurement noise, seeds 70000–70499. One recorded run of [hashgrid.rs](../../hashgrid.rs) per file.

- [Protocol and interpretation](../../../docs/validation.md#yield-model-bandit-and-extended-rounds): what the arms are, how the policy's parameters were chosen, and what the numbers do and do not show.
- `results.jsonl` / `results.txt`: 320 evaluations per arm. One row per seed and arm; the `.txt` is the table the run printed (mean gap to the best design, 95% interval).
- `results-total40.jsonl` / `.txt`: the same with `--total 40`.
- `inert-moves.jsonl` / `.txt` and `inert-moves-total40.jsonl` / `.txt`: the same two runs with `--inert-moves`, the tool's agent as it is. These are the numbers docs/validation.md reported before the harness was corrected.
- [provenance.json](provenance.json): commands, toolchain, base revision, and SHA-256 hashes of the sources and outputs.

```sh
cargo run --release --locked --example hashgrid -- --seeds 500 --first-seed 70000 > results.jsonl
cargo run --release --locked --example hashgrid -- --seeds 500 --first-seed 70000 --total 40 > results-total40.jsonl
```

Add `--inert-moves` for the other two. The runs are deterministic: the same sources give the same files, byte for byte.

## The two agents

With no levels (`L = 0`, where the search starts) the knobs `b`, `r` and `o` do nothing. The tool's parameter
agent does not know that, so three of its five knobs propose the design it started from. The agent of the
[case study](../../../docs/case-study-hashbench.md) never proposed the design it was continuing, and the
default here follows it (`grid::propose` in [grid.rs](../../grid.rs)). With levels the two agents are identical.

Fixed refinement, 8 rounds of 40: 14.6% from the optimum in the case study (100 replicates), 15.95% ± 1.87 here,
51.79% ± 5.97 with `--inert-moves`.

A real run of [examples/hashbench/.git-dream.toml](../../../examples/hashbench/.git-dream.toml) uses the tool's
agent, so `inert-moves` is what it would do.

These are simulated searches on one measured landscape with a deterministic agent, not LLM coding runs.
