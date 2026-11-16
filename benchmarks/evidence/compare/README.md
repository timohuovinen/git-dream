# Recorded output of the `compare` benchmark

The prospective synthetic comparison: one recorded run of [compare.rs](../../compare.rs).

- [Protocol and interpretation](../../../docs/validation.md): methods, budgets, training/test separation, limitations, and results.
- [results.jsonl](results.jsonl): three protocol records and 3,000 result records, each including the best-so-far evaluation trace.
- [summary.md](summary.md): paired differences against fixed refinement and descriptive 95% bootstrap intervals.
- [provenance.json](provenance.json): command, toolchain, base revision, and source/artifact SHA-256 hashes at the experiment checkpoint.
- [summarize.rs](../../summarize.rs): regenerate the summary with `cargo run --release --locked --example summarize -- benchmarks/evidence/compare/results.jsonl` from the repository root. It replaces the Python `summarize.py` listed in provenance.json and prints the same table from the same input.

provenance.json is a record and is not edited: its paths are those of the layout at the time. `examples/compare.rs` is now `benchmarks/compare.rs`, and `research/validation/` is this folder. It describes this one run of the `compare` benchmark, not any of the projects in `examples/`.

The recorded source checkpoint preceded its containing Git commit, so provenance identifies it by content hashes as well as its base revision. Documentation-only follow-ups do not change the recorded experiment.

These are noiseless synthetic evaluations with fresh proposal seeds, not LLM coding runs or evidence of general adaptive-search superiority. The moving held-out check is a deployment diagnostic; the separate frozen-policy arm uses training seeds distinct from its test seeds. Frozen-policy training costs are additional to its reported test budget.
