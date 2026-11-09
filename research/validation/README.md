# Prospective synthetic comparison artifacts

- [Protocol and interpretation](../../docs/validation.md): methods, budgets, training/test separation, limitations, and results.
- [results.jsonl](results.jsonl): three protocol records and 3,000 result records, each including the best-so-far evaluation trace.
- [summary.md](summary.md): paired differences against fixed refinement and descriptive 95% bootstrap intervals.
- [provenance.json](provenance.json): command, toolchain, base revision, and source/artifact SHA-256 hashes at the experiment checkpoint.
- [summarize.py](summarize.py): regenerate the summary with `python3 research/validation/summarize.py research/validation/results.jsonl` from the repository root.

The recorded source checkpoint preceded its containing Git commit, so provenance identifies it by content hashes as well as its base revision. Documentation-only follow-ups do not change the recorded experiment.

These are noiseless synthetic evaluations with fresh proposal seeds, not LLM coding runs or evidence of general adaptive-search superiority. The moving held-out check is a deployment diagnostic; the separate frozen-policy arm uses training seeds distinct from its test seeds. Frozen-policy training costs are additional to its reported test budget.
