# Inspectable coding experiment: log aggregation

A Rust log aggregation fixture improved from a fresh confirmation mean of **25.16 ms to 1.16 ms** on the recorded machine (about **21.8×**). The change replaces a linear scan of previously seen keys on every input row with hash aggregation over borrowed keys, then constructs the sorted result once per distinct key.

The command adapter installs a supplied optimization candidate. It does not call an LLM, and this is **not** evidence of autonomous LLM exploration or of adaptive exploration beating a fixed strategy. A deliberately incorrect duplicate-overwrite implementation is an explicitly labeled negative control, not an unsuccessful discovery attributed to a model.

## Evidence

[Full Git bundle](../research/log-count/history.bundle), [run log](../research/log-count/run.log), [toolchain versions](../research/log-count/environment.txt), [refs](../research/log-count/refs.txt), [fixture and adapter](../examples/log-count).

One fixed-policy round recorded:

1. The baseline root.
2. The negative control, rejected by the independent correctness tests.
3. Hash aggregation, accepted after testing.
4. A no-op when the supplied candidate list was exhausted.

The evaluator compiles with `rustc`, checks duplicate keys, Unicode/space-containing/empty keys, CRLF, malformed rows, and a generated 10,000-row oracle case. Its benchmark aggregates 40,000 rows with 2,048 distinct keys. Each evaluator execution includes a warmup and seven timed measurements; the score is the median. Three fresh evaluator executions per finalist confirm the optimized candidate and incumbent root. Scores are wall-clock milliseconds. Compilation and correctness tests are outside the timing interval but still incur real execution cost.

There were nine evaluator executions: root, failed control, successful candidate, and six confirmations. The no-op did not invoke the evaluator. Raw samples, test failure output, and all confirmation results are in Git notes. The evaluation contract and adapter/candidates are protected against direct edits by the command agent.

This fixture has a deliberately simple algorithmic bottleneck. Timing depends on the machine and workload, and the reported ratio is descriptive, not a population confidence interval. The fixture does not assess many-key memory pressure, adversarial hash inputs, other parser semantics, or a real service's end-to-end performance.

## Inspect

Use a fresh directory; fetch explicitly because ordinary clones omit hidden refs:

```sh
git init /tmp/log-count-evidence
git -C /tmp/log-count-evidence fetch /absolute/path/to/git-dream/research/log-count/history.bundle \
  'refs/heads/*:refs/heads/*' 'refs/dream/*:refs/dream/*' \
  'refs/tags/*:refs/tags/*' 'refs/notes/*:refs/notes/*'
git -C /tmp/log-count-evidence log --all --graph --oneline
git -C /tmp/log-count-evidence show --notes=dream refs/dream/main/r01/n001
git -C /tmp/log-count-evidence diff refs/dream/main/base dream/main/best -- count.rs
git -C /tmp/log-count-evidence notes --ref=dream show dream/main/best
```

## Rerun

```sh
cargo build --locked
python3 research/log-count/run.py --output /tmp/log-count-new-results
```

The runner creates a temporary repository, records all attempts and confirmations, and exports a new bundle and logs. It requires Rust, Git, and Python 3. To perform a new autonomous agent experiment, replace `[agent] command` with the actual agent invocation, retain the independent evaluator and protected paths, record model/version/token costs, and collect fixed-policy controls. The existing candidate-replay run must not be relabeled as that experiment.
