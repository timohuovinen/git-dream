# Validation protocol and current evidence

The current prospective synthetic comparison does **not** establish an adaptive quality advantage. At equal evaluation budgets, guarded cache replay and guarded strict replay matched fixed refinement's final scores. Multi-start successive halving did better on all three included landscapes. The stopping variant used fewer calls on some landscapes without changing final quality in this sample.

This is an exploratory benchmark, not a preregistered experiment or a demonstration of generalization to real repositories. The tasks and controls were implemented before inspecting these results; no tasks or seeds were removed after seeing outcomes.

## Reproduce

From the repository root (Rust, Python 3):

```sh
cargo run --release --locked --example compare -- --seeds 100 --first-seed 10000 > results.jsonl
python3 research/validation/summarize.py results.jsonl
```

[Source](../examples/compare.rs), [source hashes and run provenance](../research/validation/provenance.json), [raw traces](../research/validation/results.jsonl), [all paired comparisons](../research/validation/summary.md).

## What was compared

Three deterministic, finite 32 × 32 parameter landscapes: smooth, rugged, and deceptive. Each has 100 paired test seeds (10000–10099). A seed changes the proposal operator's salt. Every arm gets the same initial design and objective, and its own history. Every proposed point is evaluated against the actual objective, including points absent from its history. No replay score is substituted for a prospective measurement.

Methods:

- Fixed `refine_best`, fixed UCB (`c = 0.05`), and uniform random valid-parent selection use the same local proposal operator.
- Uniform random search samples the whole parameter grid. It is distinct from random tree exploration.
- Successive halving allocates evaluations among eight local-search trajectories initialized at random points, retaining the best half at successive stages. Its control distributes resources equally over the same eight starting trajectories. This is an adaptation of [successive halving](https://proceedings.mlr.press/v51/jamieson16.html) to iterative search trajectories: resource means objective evaluations, and arm quality means best score so far. It is not a claim about low-fidelity model-training performance.
- Guarded adaptive cache and strict replay select among 60 sampled policy specifications per dream. The newest round is held out. The winner must beat the incumbent's held-out J without a higher miss rate. A minimum-call floor equal to the round budget enforces equal evaluation budgets.
- A frozen policy is selected before testing from 20 independent training rounds (seeds 0–19), alternating refinement and random-parent behavior. It uses strict replay because training salts differ. The newest training round is the deployment check. It receives no updates from test outcomes.
- Adaptive stopping uses a six-call floor and is reported separately; it is **not** an equal-cost quality comparison.

Local tree methods run four rounds of 24 proposals, carrying forward the incumbent. All fixed-budget methods spend exactly 97 objective evaluations: one root plus 96 proposals, or one common root plus eight starts and 88 subsequent trajectory evaluations. Repeated points are charged again. The synthetic evaluator is noiseless: there are no confirmation costs or cache hits masquerading as free evaluations. Both evaluation counts and complete best-so-far traces are emitted for every arm.

Frozen-policy selection additionally costs **500 training evaluations per task**, reported in protocol records. Its 97-evaluation test result must not be described as equal total lifetime cost to the untrained baselines. Replay computation, Git overhead, wall time, and model/token costs are not measured by this synthetic harness.

## Results

Mean final regret (score minus the enumerated optimum; lower is better):

| Method | Smooth | Rugged | Deceptive |
|---|---:|---:|---:|
| Fixed refinement | 4.840 | 54.369 | 21.330 |
| Fixed UCB | 111.260 | 236.347 | 21.440 |
| Random tree | 209.230 | 240.433 | 22.110 |
| Guarded adaptive cache | 4.840 | 54.369 | 21.330 |
| Guarded adaptive strict | 4.840 | 54.369 | 21.330 |
| Frozen policy | 4.840 | 54.369 | 21.330 |
| Uniform random search | 3.220 | 12.918 | 3.220 |
| Equal allocation over starts | 8.880 | 23.274 | 6.600 |
| Successive halving over starts | 0.040 | 5.486 | 1.630 |

The [full summary](../research/validation/summary.md) reports paired mean regret differences against refinement with percentile bootstrap intervals (5,000 seed resamples, fixed bootstrap seed). These are descriptive, unadjusted 95% intervals across proposal seeds on each fixed landscape, not uncertainty across unseen tasks. We do not pool unlike objective scales or select a winner by significance tests. Enumeration of the optimum is used only for reporting regret; no search method can access it.

## What held-out replay does and does not address

`git dream replay --holdout N` partitions each run by round number: the newest N rounds are held out. Neither partition contributes measurements to the other's cache. Candidate selection happens on training data; the deployment gate checks only that winner against the incumbent. Insufficient history keeps the incumbent. Candidate specs are deduplicated, and ties favor the incumbent.

This separates candidate fitting from a diagnostic check. It does **not** eliminate behavior-policy selection: both partitions contain attempts chosen by previous policies. Unobserved outcomes remain missing, and charging them as failures can favor conservative or short policies. Zero misses only means the queried edges/designs were available; it is not causal identification.

The default moving holdout is consulted repeatedly and eventually becomes training data. Its scores cannot be reported as an untouched final test. For a final comparison, freeze the policy, evaluator, task set, and analysis, then collect fresh independent runs. For command agents, changing search order also changes the history they see, so a recorded `(parent, k)` edge need not represent the same counterfactual action.

The new configuration/base checks prevent silently pooling different declared operators and objectives. They cannot detect changes to an external service, machine, mutable dependencies, or untracked external policy code. Pin those independently.

## Coverage and remaining evidence

| Criticism | Addressed here | Still needed for a stronger claim |
|---|---|---|
| Selection bias / replay optimism | Explicit limitations, disjoint holdout, conservative deployment gate, fresh prospective harness | Broader prospective tasks; a holdout cannot manufacture missing counterfactuals |
| Early stopping feedback | Configurable proposal floor, full-budget comparisons, separate stopping costs | Diversity/restart strategies and long-run evaluation |
| Weak baselines / few seeds | Ten methods, 100 paired seeds per landscape, raw traces and intervals | Multiple real tasks, noise and wall-clock/model-cost budgets |
| Out-of-sample validation | Training/test salts separated; frozen policy arm; held-out replay command | Truly unseen tasks and an untouched final evaluation after development |
| Compelling coding-agent demonstration | Real measured source optimization, independent correctness tests, failed control, complete Git bundle | Autonomous multi-attempt LLM search on a real project with pinned model and token costs |
| Overstated positioning | Practical Git-native pitch; negative results prominently retained | No claim of general adaptive superiority until supported |
| Measurement and reliability defects | Fresh-only confirmation, failure rejection, root comparison, carried confirmed score, evaluator counts, protected paths, external-policy deadlines/errors | Platform isolation and reproducible machine/service environments remain caller responsibilities |

The coding bundle is described in [the log-count case study](case-study-log-count.md). Its substantial speedup establishes that a code change improved the fixture; it does not establish that adaptive search found it more efficiently than a fixed strategy.
