# Recorded output of the `compare` benchmark with the `bandit` arms

The run behind the table in
[docs/validation.md](../../../../docs/validation.md#yield-model-bandit-and-extended-rounds): three synthetic
landscapes, 300 seeds (70000–70299), 13 methods, 97 evaluations each. The folder above holds the earlier run
(seeds 10000–10099, before the `bandit` arms existed).

- [results.jsonl](results.jsonl): three protocol records and 11,700 result records, each with its best-so-far trace.
- [summary.md](summary.md): paired differences against fixed refinement with 95% bootstrap intervals.
- [provenance.json](provenance.json): commands, toolchain, base revision, and SHA-256 hashes.

```sh
cargo run --release --locked --example compare -- --seeds 300 --first-seed 70000 > results.jsonl
cargo run --release --locked --example summarize -- results.jsonl > summary.md
```

The run is deterministic: the same sources give the same file, byte for byte.

The policy's parameters were chosen on seeds 50000–50999 of this harness and of `hashgrid`, so these seeds rule
out overfitting to seeds, not to these landscapes. Noiseless synthetic evaluations with a deterministic agent.
