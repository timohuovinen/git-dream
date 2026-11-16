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

The policy's fixed parameters (prior, `m = 4`, `focus = 16`, continuation classes) were chosen on seeds 50000–50999 of these two harnesses. The numbers below are from seeds 70000 onward. The published test seeds 10000–10099 were not used at any point.

The hash-table harness was corrected after the parameters were frozen, and nothing was tuned again. Its agent used to propose moves of knobs that do nothing while a design has no levels (`L = 0`, where the search starts), so most early proposals measured the starting design again. The agent of the [case study](case-study-hashbench.md) never made such proposals, which is why fixed refinement ended 51.8% from the optimum here and 14.6% there for the same grid, noise and schedule. The harness now uses the case study's rule and gives 16.0%. `--inert-moves` restores the earlier agent, which is what a real run of [the hashbench example](../examples/hashbench/.git-dream.toml) does; both sets of numbers are below.

Recorded output: [hash-table grid](../benchmarks/evidence/hashgrid/), [synthetic landscapes](../benchmarks/evidence/compare/bandit/).

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
| 1 × 320 (extended) | 6.12% ± 0.78 | 2.31% ± 0.23 | 2.23% ± 0.24 | — |
| 8 × 40 | 15.95% ± 1.87 | 8.06% ± 1.07 | 4.00% ± 0.50 | 3.86% ± 0.44 |
| 40 × 8 | 85.72% ± 6.11 | 69.30% ± 6.19 | 68.02% ± 6.20 | 34.08% ± 4.82 |

The same with `--inert-moves` (the numbers this section reported before the correction):

| Rounds × proposals | `refine_best` | `repair` | `bandit` | `bandit_dream` |
|---|---:|---:|---:|---:|
| 1 × 320 (extended) | 9.66% ± 1.55 | 46.82% ± 7.77 | 2.38% ± 0.27 | — |
| 8 × 40 | 51.79% ± 5.97 | 50.44% ± 7.66 | 4.36% ± 0.99 | 4.85% ± 1.05 |
| 40 × 8 | 161.73% ± 7.02 | 110.88% ± 8.54 | 128.02% ± 8.24 | 89.74% ± 8.17 |

What this does and does not show:

- At equal evaluations the bandit is clearly better than fixed refinement on the rugged landscape and on the measured grid, and not distinguishable from it on the smooth one.
- On the measured grid the hand-written `repair` policy does as well as the bandit in one extended round (2.3% and 2.2%), and half as well in rounds of 40. The earlier numbers made `repair` look far worse because it does not recover from the wasted proposals at the start; the bandit's results barely depend on the agent.
- It does not help on the deceptive landscape. The proposal operator moves one or two grid steps, and no policy that only chooses which attempt to continue can cross that distance. Successive halving and uniform random search use a different operator (random points) and remain far better on all three synthetic tasks.
- Round length matters more than the policy. The same 320 evaluations end 2.2% from the optimum in one tree and 68% away in 40 rounds of 8, because a new round keeps only the promoted attempt. For a deterministic agent a new round also repeats the proposals already made from that attempt.
- Refitting the model between rounds (`bandit_dream`) made no measurable difference with rounds of 24 or 40 proposals. It helped with rounds of 8 (68% → 34%), where a round is too short to learn anything by itself. Extending the round was far better than either.
- Both harnesses use the deterministic space agent, and the parameters were chosen on the same two harnesses. Fresh seeds rule out overfitting to seeds, not to these landscapes. There is no evidence here about LLM agents or about unseen tasks.

## Attempts at once

`[explore] parallel` runs several attempts at once: the policy chooses each of them without the results of the others still in flight. `benchmarks/hashgrid.rs` measures what that costs with `sim::run_waves`, which is what a live round does when evaluations run alone: `N` attempts are chosen, then all `N` results arrive. A wave takes the time of one attempt, so the same evaluations take 1/N of the time.

No parameter was chosen for this: the policy is the one frozen above, and the seeds are the same 500 (70000 onward). The first table is part of the `hashgrid` command above; the second adds `--total 40`.

Measured hash-table grid, one round, 6% noise, 500 seeds: mean gap to the best design, 95% interval. The last column is `bandit` one attempt at a time for the same number of steps, which is the fair comparison at equal time.

320 evaluations:

| At once | Steps | `refine_best` | `bandit` | `bandit`, one at a time, same steps |
|---:|---:|---:|---:|---:|
| 1 | 320 | 6.12% ± 0.78 | 2.23% ± 0.24 | — |
| 4 | 80 | 5.88% ± 0.74 | 2.35% ± 0.27 | 8.92% ± 0.91 |
| 8 | 40 | 5.82% ± 0.74 | 2.49% ± 0.27 | 22.35% ± 2.03 |
| 16 | 20 | 6.14% ± 0.74 | 2.59% ± 0.28 | 52.43% ± 4.30 |

40 evaluations:

| At once | Steps | `refine_best` | `bandit` | `bandit`, one at a time, same steps |
|---:|---:|---:|---:|---:|
| 1 | 40 | 29.26% ± 1.99 | 22.35% ± 2.03 | — |
| 4 | 10 | 30.34% ± 2.07 | 24.84% ± 1.95 | 95.09% ± 5.50 |
| 8 | 5 | 33.69% ± 2.23 | 27.72% ± 2.24 | 136.71% ± 5.13 |
| 16 | 3 | 42.28% ± 3.01 | 33.05% ± 2.58 | 177.30% ± 5.18 |

The tables with `--inert-moves` are in the [recorded output](../benchmarks/evidence/hashgrid/); they lead to the same conclusions.

Wall-clock time of the real binary on the toy repositories, one run each (a check that attempts really overlap, not a benchmark):

| Setup | One at a time | At once |
|---|---:|---:|
| `toy-space`, 24 attempts, evaluator sleeps 0.5 s, `evaluations = "parallel"` | 13.4 s | 3.9 s (4), 2.4 s (8) |
| `toy-command`, 12 attempts, agent sleeps 1 s, evaluator 0.1 s, `evaluations = "alone"` | 14.1 s | 4.8 s (4) |

What this does and does not show:

- With a few hundred evaluations, choosing up to 16 attempts blind cost at most 0.4 points of gap, inside or near the interval, for a 16-fold cut in steps. At equal time, attempts at once were far ahead in every row.
- With 40 evaluations the cost is visible: 16 at once leaves three waves, and the gap grows from 22% to 33%. Four at once cost 2 points, eight cost 5.
- The simulated agent's k-th proposal from an attempt always differs from its earlier ones. LLM agents started together from one attempt may make the same change; they are told how many others started beside them, and whether that prevents duplicates is not measured.
- Only waves are simulated. With `evaluations = "serial"` or `"parallel"` a lane is refilled as soon as it is free; that path is covered by end-to-end tests (overlap, isolation, extension, recovery after a kill), not by a quality benchmark.
- Evaluation time is assumed equal for every attempt and the agent's time is the only thing saved when evaluations run alone: if evaluation dominates an attempt, `evaluations = "alone"` saves little.

## Can replay choose a better policy?

[`benchmarks/fidelity.rs`](../benchmarks/fidelity.rs) asks the question a replay dream depends on: is the policy that scores best on replay truly better than the one already deployed? It replaces the single-history check of the case study.

```sh
cargo run --release --locked --example fidelity -- --reps 100 --out benchmarks/evidence/fidelity/v1
```

On the measured hash-table grid it records 100 independent histories (6 rounds of 40, 6% noise) with each of five recording policies: fixed refinement, replay dreaming, and three wide ones. For each history it replays `refine_best` and 200 sampled policies, in strict and cache mode, picks the replay winner under three rules for misses, and compares that pick with `refine_best` on the noise-free grid, from the root the next round would start at. The verdict rule was written before the run: selection is trustworthy in a cell only if the 95% interval of that difference lies above zero. [Report](../benchmarks/evidence/fidelity/v1.txt), [per-policy rows](../benchmarks/evidence/fidelity/v1.jsonl).

| Recorded by | Mode | Misses | Spearman, replay J against true J | Pick − `refine_best`, same roots | Pick − `refine_best`, next root |
|---|---|---:|---:|---:|---:|
| fixed | strict | 47% | +0.03 ± 0.04 | −0.003 ± 0.009 | +0.000 ± 0.005 |
| fixed | cache | 21% | +0.18 ± 0.05 | +0.020 ± 0.014 | +0.002 ± 0.010 |
| dream | strict | 48% | +0.05 ± 0.04 | +0.001 ± 0.015 | −0.007 ± 0.018 |
| dream | cache | 22% | +0.16 ± 0.04 | +0.013 ± 0.017 | −0.016 ± 0.013 |
| wide-softmax | strict | 32% | +0.24 ± 0.05 | +0.030 ± 0.015 | −0.000 ± 0.013 |
| wide-softmax | cache | 11% | +0.37 ± 0.05 | +0.052 ± 0.016 | +0.006 ± 0.013 |
| wide-ucb | strict | 38% | +0.10 ± 0.05 | −0.007 ± 0.018 | −0.011 ± 0.017 |
| wide-ucb | cache | 17% | +0.26 ± 0.05 | +0.070 ± 0.031 | +0.003 ± 0.019 |
| wide-beam | strict | 32% | +0.13 ± 0.05 | −0.001 ± 0.020 | −0.038 ± 0.019 |
| wide-beam | cache | 14% | +0.30 ± 0.04 | +0.031 ± 0.027 | −0.041 ± 0.019 |

The rows are for the rule the tool uses (a miss is charged as a call). The best policy in each pool was 0.06 to 0.23 better than `refine_best` from the next root, so there was something to find.

What this does and does not show:

- Replay selection was trustworthy in none of the 30 cells (5 recorders × 2 modes × 3 rules). It was no better than keeping `refine_best` in 23 and worse in 7.
- Cache replay does find policies that are better on the roots it was scored on (four of five recorders). The gain does not carry to the next root. That is a fit to the recorded rounds, not a better policy.
- Ranking improves with wider recording and with the cache (Spearman up to 0.37), which is the direction the paper's wide first round points in. It was not enough to make the pick better here.
- The holdout gate was not part of this run: it tests replay's raw pick. The gate can only keep the incumbent more often.
- One landscape, a deterministic agent, policies sampled from the built-in families. The agent is the tool's own, including the inert moves described above. There is no evidence here about policies written by a policy-development agent, or about LLM agents.

## A real coding agent

Everything above uses a simulated agent that moves parameters. This section is the first comparison with an LLM agent editing code: Claude Haiku 5.5, headless, on [the codec task](../examples/codec/README.md) (a std-only compressor in one Rust file; the score is the compressed size of a fixed 121,546-byte corpus plus the size of the source, in bytes). The score is a byte count, so load does not change it.

Four strategies, 4 independent runs each, 48 attempts per run, 4 attempts at once, the same start and prompt: `fixed` (`refine_best`), `bandit` (the default), `chains` (four independent chains per round, the paper's fixed baseline, a wide record) and `paper` (`chains` in round 1, then whatever scores best on strict replay). [Runs, report and how to repeat them](../examples/codec/evidence/).

Best score after n attempts, mean and 95% interval over the 4 runs (start: 121,673; `bzip2 -9` on the corpus: 35,781):

| Strategy | 12 attempts | 24 attempts | 36 attempts | 48 attempts |
|---|---:|---:|---:|---:|
| `bandit` | 36,588 ± 1,351 | 34,942 ± 1,580 | 33,930 ± 1,008 | 33,518 ± 1,305 |
| `fixed` | 36,475 ± 1,743 | 34,711 ± 1,182 | 33,972 ± 1,342 | 33,556 ± 1,278 |
| `chains` | 36,881 ± 2,453 | 34,947 ± 880 | 34,234 ± 1,175 | 33,914 ± 1,209 |
| `paper` | 36,580 ± 1,612 | 35,233 ± 466 | 34,616 ± 629 | 34,027 ± 841 |

Differences in the final score (Welch 95% interval): `bandit` − `fixed` = −37 ± 1,476; `bandit` − `chains` = −396 ± 1,437; `paper` − `chains` = +112 ± 1,190; `paper` − `bandit` = +508 ± 1,254.

How much of what other policies would do is in the record. `refine_best` and 200 sampled policies were replayed in strict mode over the first 24 recorded attempts of each run, 24 attempts each:

| Record made by | Candidates may stop early: mean miss rate | in the record (miss ≤ 5%) | attempts the replay winner spends | Candidates spend all 24: mean miss rate | in the record |
|---|---:|---:|---:|---:|---:|
| `fixed` (narrow) | 68% | 1% | 17.0 | 80% | 0% |
| `bandit` (narrow) | 60% | 4% | 9.8 | 77% | 0% |
| `chains` (wide), `chains` arm | 45% | 16% | 7.5 | 70% | 2% |
| `chains` (wide), `paper` arm | 47% | 15% | 5.0 | 71% | 1% |

What this does and does not show:

- **No strategy is distinguishable from another.** Runs of the same strategy differ by about 1,500 bytes; the strategies' means differ by at most 500. With 4 runs per strategy, a real difference smaller than about 1,400 bytes (4% of the score) would not be seen. The default policy is neither shown to help nor shown to hurt.
- **The agent did most of the work in its first attempt.** Haiku wrote a context-mixing coder at once: the best of the first 4 attempts averaged 37,611 (range 35,887 to 39,099). The remaining 44 attempts were worth about 3,900 bytes. Which attempt to continue matters less when most attempts improve a little.
- **A wide record does give replay more support**, as the paper's design implies: about 15% of candidate policies stay inside a `chains` record, against 1 to 4% for a narrow one.
- **What replay can see in that record is where to stop.** In 13 of the 16 runs the replay winner was a policy that spends fewer than the 24 attempts (4 to 12); in the other 3 (`bandit` 4, `fixed` 2 and 4) it was a beam policy that missed 50 to 58% of its calls. Made to spend the whole budget, almost no candidate stays in the record, wide or narrow: a strict replay of one recorded tree can score doing less of what was done, not doing something else with the same attempts.
- **Deploying the replay winner did not help.** In all 4 `paper` runs the dream replaced `chains(w=4,d=6)`. In two it chose depth 1, and the rest of the run was rounds of 4 attempts. In the other two it chose a single chain (`w=1`), which the run still had to spend its 24 attempts on. The arm ended 112 bytes behind `chains` and 508 behind `bandit`, neither distinguishable from zero.
- **It found a defect no simulated agent could.** Scratch worktrees were under `.git/dream/wt`, and Claude Code refuses to write below `.git`: in the first pilot every attempt came back unchanged. They are now outside the repository ([configuration changes](compatibility.md#scratch-worktrees-are-outside-git)).
- One task, one model, 4 runs per strategy, 48 attempts. Eight runs per strategy were planned; the rest were stopped when the agents ran out of allowance. The replay objective's cost per attempt (`lam = 0.02`) and the 4-attempt floor were set once, before the runs.

## What held-out replay does and does not address

`git dream replay --holdout N` partitions each run by round number: the newest N rounds are held out. Neither partition contributes measurements to the other's cache. Candidate selection happens on training data; the deployment gate checks only that winner against the incumbent. Insufficient history keeps the incumbent. Candidate specs are deduplicated, and ties favor the incumbent.

This separates candidate fitting from a diagnostic check. It does **not** eliminate behavior-policy selection: both partitions contain attempts chosen by previous policies. Unobserved outcomes remain missing, and charging them as failures can favor conservative or short policies. Zero misses only means the queried edges/designs were available; it is not causal identification.

The default moving holdout is consulted repeatedly and eventually becomes training data. Its scores cannot be reported as an untouched final test. For a final comparison, freeze the policy, evaluator, task set, and analysis, then collect fresh independent runs. For command agents, changing search order also changes the history they see, so a recorded `(parent, k)` edge need not represent the same counterfactual action.

The new configuration/base checks prevent silently pooling different declared operators and objectives. They cannot detect changes to an external service, machine, mutable dependencies, or untracked external policy code. Pin those independently.

## Coverage and remaining evidence

| Criticism | Addressed here | Still needed for a stronger claim |
|---|---|---|
| Selection bias / replay optimism | Explicit limitations, disjoint holdout, conservative deployment gate, fresh prospective harness, replay picks checked against the truth on 500 histories | Broader prospective tasks; a holdout cannot manufacture missing counterfactuals |
| Early stopping feedback | Configurable proposal floor, full-budget comparisons, separate stopping costs | Diversity/restart strategies and long-run evaluation |
| Weak baselines / few seeds | Ten methods, 100 paired seeds per landscape, raw traces and intervals | Multiple real tasks, noise and wall-clock/model-cost budgets |
| Out-of-sample validation | Training/test salts separated; frozen policy arm; held-out replay command | Truly unseen tasks and an untouched final evaluation after development |
| Compelling coding-agent demonstration | Real measured source optimization, independent correctness tests, failed control, complete Git bundle; 16 autonomous 48-attempt LLM runs on one task with model, tokens and cost recorded | More runs per strategy, more than one task and model, a task where single attempts are not enough |
| Overstated positioning | Practical Git-native pitch; negative results prominently retained | No claim of general adaptive superiority until supported |
| Measurement and reliability defects | Fresh-only confirmation, failure rejection, root comparison, carried confirmed score, evaluator counts, protected paths, external-policy deadlines/errors | Platform isolation and reproducible machine/service environments remain caller responsibilities |

The coding bundle is described in [the log-count case study](case-study-log-count.md). Its substantial speedup establishes that a code change improved the fixture; it does not establish that adaptive search found it more efficiently than a fixed strategy.
