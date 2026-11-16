# Validation protocol and current evidence

The original prospective synthetic comparison did **not** establish a quality advantage for replay-based adaptation. At equal evaluation budgets, guarded cache replay and guarded strict replay matched fixed refinement's final scores. A later section reports the yield-model `bandit` policy, which does better than fixed refinement on two of four landscapes. Multi-start successive halving did better on all three included landscapes. The stopping variant used fewer calls on some landscapes without changing final quality in this sample.

This is an exploratory benchmark, not a preregistered experiment or a demonstration of generalization to real repositories. The tasks and controls were implemented before inspecting these results; no tasks or seeds were removed after seeing outcomes.

## Reproduce

From the repository root:

```sh
cargo run --release --locked --example compare -- --seeds 100 --first-seed 10000 > results.jsonl
cargo run --release --locked --example summarize -- results.jsonl
```

[Source](../benchmarks/compare.rs), [source hashes and run provenance](../benchmarks/evidence/compare/provenance.json), [raw traces](../benchmarks/evidence/compare/results.jsonl), [all paired comparisons](../benchmarks/evidence/compare/summary.md).

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

The [full summary](../benchmarks/evidence/compare/summary.md) reports paired mean regret differences against refinement with percentile bootstrap intervals (5,000 seed resamples, fixed bootstrap seed). These are descriptive, unadjusted 95% intervals across proposal seeds on each fixed landscape, not uncertainty across unseen tasks. We do not pool unlike objective scales or select a winner by significance tests. Enumeration of the optimum is used only for reporting regret; no search method can access it.

## Yield-model bandit and extended rounds

Added after the comparison above. The three `bandit` arms in `benchmarks/compare.rs` and the second harness `benchmarks/hashgrid.rs` test the default policy family and round extension (see the README's Policies section). The rows of the earlier methods are byte-identical to before.

- `bandit`: four rounds of 24, the policy learns within each round only.
- `bandit_dream`: the same, and the yield model is refitted over the arm's own recorded rounds before each round.
- `bandit_extended`: one tree of all 96 proposals, which is what `[explore] extend` produces.

The policy's fixed parameters (prior, `m = 4`, `focus = 16`, continuation classes) were chosen on seeds 50000–50999 of these two harnesses. The numbers below are from seeds 70000 onward, run once after the choice was frozen. The published test seeds 10000–10099 were not used at any point.

```sh
cargo run --release --locked --example compare -- --seeds 300 --first-seed 70000 > compare.jsonl
cargo run --release --locked --example hashgrid -- --seeds 500 --first-seed 70000 > hashgrid.jsonl
```

Synthetic landscapes, mean final regret after 97 evaluations, 300 seeds. In brackets: paired difference against fixed refinement with a 95% interval.

| Method | Smooth | Rugged | Deceptive |
|---|---:|---:|---:|
| Fixed refinement | 1.307 | 57.379 | 20.940 |
| Guarded adaptive cache / strict | 1.307 | 57.379 | 20.940 |
| `bandit` | 0.203 (−1.10 ± 1.73) | 21.825 (−35.55 ± 6.12) | 20.043 (−0.90 ± 0.58) |
| `bandit_dream` | 0.380 (−0.93 ± 1.74) | 22.796 (−34.58 ± 6.27) | 20.057 (−0.88 ± 0.58) |
| `bandit_extended` | 0.030 (−1.28 ± 1.72) | 12.325 (−45.05 ± 5.69) | 20.000 (−0.94 ± 0.58) |
| Uniform random search | 3.510 | 11.101 | 3.510 |
| Successive halving over starts | 0.093 | 5.695 | 1.477 |

Measured hash-table grid ([oracle](../examples/hashbench/evidence/oracle.jsonl): 982 designs, about 4 in 10 fail), 320 evaluations with 6% measurement noise, 500 seeds. Mean gap between the true score of the final design and the best design on the grid, with a 95% interval:

| Rounds × proposals | `refine_best` | `repair` | `bandit` | `bandit_dream` |
|---|---:|---:|---:|---:|
| 1 × 320 (extended) | 9.66% ± 1.55 | 46.82% ± 7.77 | 2.38% ± 0.27 | — |
| 8 × 40 | 51.79% ± 5.97 | 50.44% ± 7.66 | 4.36% ± 0.99 | 4.85% ± 1.05 |
| 40 × 8 | 161.73% ± 7.02 | 110.88% ± 8.54 | 128.02% ± 8.24 | 89.74% ± 8.17 |

What this does and does not show:

- At equal evaluations the bandit is clearly better than fixed refinement on the rugged landscape and on the measured grid, and not distinguishable from it on the smooth one.
- It does not help on the deceptive landscape. The proposal operator moves one or two grid steps, and no policy that only chooses which attempt to continue can cross that distance. Successive halving and uniform random search use a different operator (random points) and remain far better on all three synthetic tasks.
- Round length matters more than the policy. The same 320 evaluations end 2.4% from the optimum in one tree and 128% away in 40 rounds of 8, because a new round keeps only the promoted attempt. For a deterministic agent a new round also repeats the proposals already made from that attempt.
- Refitting the model between rounds (`bandit_dream`) made no measurable difference with rounds of 24 or 40 proposals. It helped with rounds of 8 (128% → 90%), where a round is too short to learn anything by itself. Extending the round was far better than either.
- Both harnesses use the deterministic space agent, and the parameters were chosen on the same two harnesses. Fresh seeds rule out overfitting to seeds, not to these landscapes. There is no evidence here about LLM agents or about unseen tasks.

## Attempts at once

`[explore] parallel` runs several attempts at once: the policy chooses each of them without the results of the others still in flight. `benchmarks/hashgrid.rs` measures what that costs with `sim::run_waves`, which is what a live round does when evaluations run alone: `N` attempts are chosen, then all `N` results arrive. A wave takes the time of one attempt, so the same evaluations take 1/N of the time.

No parameter was chosen for this: the policy is the one frozen above, and the seeds are the same 500 (70000 onward). The first table is part of the `hashgrid` command above; the second adds `--total 40`.

Measured hash-table grid, one round, 6% noise, 500 seeds: mean gap to the best design, 95% interval. The last column is `bandit` one attempt at a time for the same number of steps, which is the fair comparison at equal time.

320 evaluations:

| At once | Steps | `refine_best` | `bandit` | `bandit`, one at a time, same steps |
|---:|---:|---:|---:|---:|
| 1 | 320 | 9.66% ± 1.55 | 2.38% ± 0.27 | — |
| 4 | 80 | 8.40% ± 1.38 | 2.46% ± 0.29 | 9.90% ± 1.19 |
| 8 | 40 | 8.27% ± 1.51 | 2.51% ± 0.27 | 38.57% ± 4.14 |
| 16 | 20 | 8.41% ± 1.50 | 2.71% ± 0.29 | 98.28% ± 6.17 |

40 evaluations:

| At once | Steps | `refine_best` | `bandit` | `bandit`, one at a time, same steps |
|---:|---:|---:|---:|---:|
| 1 | 40 | 104.34% ± 6.21 | 38.57% ± 4.14 | — |
| 4 | 10 | 109.73% ± 6.08 | 42.89% ± 4.32 | 157.67% ± 6.16 |
| 8 | 5 | 111.67% ± 6.11 | 42.44% ± 4.11 | 192.73% ± 6.04 |
| 16 | 3 | 105.87% ± 6.11 | 53.28% ± 4.61 | 231.88% ± 4.98 |

Wall-clock time of the real binary on the toy repositories, one run each (a check that attempts really overlap, not a benchmark):

| Setup | One at a time | At once |
|---|---:|---:|
| `toy-space`, 24 attempts, evaluator sleeps 0.5 s, `evaluations = "parallel"` | 13.4 s | 3.9 s (4), 2.4 s (8) |
| `toy-command`, 12 attempts, agent sleeps 1 s, evaluator 0.1 s, `evaluations = "alone"` | 14.1 s | 4.8 s (4) |

What this does and does not show:

- With a few hundred evaluations, choosing up to 16 attempts blind cost at most 0.3 points of gap, inside or near the interval, for a 16-fold cut in steps. At equal time, attempts at once were far ahead in every row.
- With 40 evaluations the cost is visible: 16 at once leaves three waves, and the gap grows from 39% to 53%. Four or eight at once cost about 4 points.
- The simulated agent's k-th proposal from an attempt always differs from its earlier ones. LLM agents started together from one attempt may make the same change; they are told how many others started beside them, and whether that prevents duplicates is not measured.
- Only waves are simulated. With `evaluations = "serial"` or `"parallel"` a lane is refilled as soon as it is free; that path is covered by end-to-end tests (overlap, isolation, extension, recovery after a kill), not by a quality benchmark.
- Evaluation time is assumed equal for every attempt and the agent's time is the only thing saved when evaluations run alone: if evaluation dominates an attempt, `evaluations = "alone"` saves little.

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
