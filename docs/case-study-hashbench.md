# Case study: Dream-RSI on git for high-load hash tables

> This is the experiment `git-dream` grew out of. It was run with the research prototype in [`research/hashbench/`](../research/hashbench/), inside a private hash-table benchmark crate (`hashbench`). The `git-dream` tool ([`src/`](../src/), Rust) is that prototype rewritten and made repository-agnostic, and [`examples/hashbench/`](../examples/hashbench/) expresses the same task as a `.git-dream.toml`. Paths and commands below refer to the original setup. The current Rust implementation adds a held-out deployment gate, exploration floors, and fresh-only confirmation; those changes were not used to obtain the archived results. See the [current validation protocol](validation.md) and [configuration changes](compatibility.md).

This is a working, minimal re-creation of **Dream-RSI** ("Recursive Self-Improvement through Evolving Worlds", Google / Google DeepMind et al., 2026, [dream-rsi.com](https://www.dream-rsi.com/)). It uses **git as the only experimental memory**. The loop is applied to a real optimisation task: finding the fastest fixed-capacity open-addressing hash-table design at 95–98% load. The hash tables come from `hashbench/`.

Code (original layout): a worktree `dream-rsi/` on branch `dream/main` of the `hashbench` repository. Entry point: [`dream.py`](../research/hashbench/dream.py). The hashbench changes it needed are in [`hashbench-funnel-simd-groups.patch`](../research/hashbench/hashbench-funnel-simd-groups.patch) and [`dream-eval.rs`](../research/hashbench/dream-eval.rs).

## TL;DR

- **The whole loop runs on plain git.** Explore, remember, dream and improve all work with ordinary commits, commit trailers, git notes and refs. There's no database and no model training. 722 real evaluations live in the repo: 732 commits under `refs/dream/*`, which is every attempt plus each round's root, including 10 roots whose scores were carried over rather than re-measured. A discovery tree is rebuilt from git alone before every dream step.
- **Dreaming is cheap.** Replaying 1,000 candidate exploration policies over the whole recorded history takes 0.1–1 s, with zero new evaluations.
- **Dreaming with the first policy families changed nothing.** In rounds 2–6 the dream re-selected the hand-written π0 every time: nothing beat it on replay. Inspection of the recorded trees motivated a v2 policy family (`repair`, mainly its *dedup* rule), corresponding to the paper's policy-development step.
- **With v2, dreaming helped measurably.** Over 100 synthetic replicates in an oracle-backed world, the remaining gap to the true optimum after 8 rounds fell from **14.6% (fixed π0) to 8.4% (Dream-RSI)**. The paired difference was +0.061 ± 0.022, using **26% fewer evaluations** (238 vs 320).
- **Live, the dream arm reached the true optimum** (1.000×) after 273 evaluations, with rounds of 16–17 calls. The fixed arm was still 1.27× off after 400. The two arms had already diverged by luck before the policies differed, so this live run is anecdotal (n=1).
- **Honest limits:**
  - Replay ranks individual policies poorly: Spearman correlation 0.01–0.10 against the true ranking.
  - Paper-faithful *strict* replay, which only trusts recorded edges, made results **worse** (1.193× vs 1.146×). The gains come from cross-tree reuse, which is valid only because this task's discovery agent is deterministic.
  - Measurement noise (~5–9%) and machine drift (+23% mid-session) create a real winner's-curse risk.
- **Hash-table finding.** At 95%/98% load the best design found is `W16-b1-L4-r0.25-o0.01`: a 4-level funnel table with 16-wide SIMD buckets, level ratio 1/4 and 1% overflow. It averages **19.4 ns/op against 71.6 for SwissTable-style probing at the same fixed capacity (3.7×)**. Failed lookups at 98% cost 19 ns instead of 286 ns, and the worst case is 8 probes instead of 1,371.

## 1. What Dream-RSI is, and how it maps onto git

From the project page, Dream-RSI has four parts:

1. An **exploration policy** is *executable code*. Given what it has seen, it decides which attempt to continue next and when to stop. It steers a *fixed* discovery agent. Its outcomes are scored by a *fixed* evaluator.
2. **Online**, the current policy expands a *discovery tree*. Each node is one attempt with the outcome it actually produced.
3. **Dreaming**: an alternative policy never has to re-run anything. *"It walks the same recorded tree in a different order, and every outcome it asks for is already on disk."* A policy-development agent writes revisions of the policy, and each revision is scored by replaying it over the whole history.
4. **Selection**: pick the argmax. The deployed policy is itself a candidate, so the winner is never worse *on replay*. The paper's baseline is **Recursive Fixed Exploration**: same agent, evaluator and budget, but the policy never changes.

The mapping used here:

| Dream-RSI concept | This implementation |
|---|---|
| Discovery agent (LLM coding agent in the paper) | `space.agent(design, k)`: deterministic local mutation, the k-th proposal from a design ([space.py](../research/hashbench/space.py)) |
| Fixed evaluator | `target/release/dream-eval` (Rust, [dream-eval.rs](../research/hashbench/dream-eval.rs)) |
| Attempt | one **commit**; its tree = frozen world (`tag dream/world`) + `dream/attempt.json` |
| Parent attempt | the commit's **parent** (a round's root has the previous round's best as parent, so all rounds form one DAG) |
| Outcome / score | **commit-message trailers** (`Dream-Score`, `Dream-Status`, `Dream-Design`, …) |
| Full diagnostics | **git notes** `refs/notes/dream` (per-load ns, probe counts, bytes) |
| Discovery tree | hidden refs `refs/dream/<arm>/r<NN>/n<NNN>` |
| Exploration path | branch `dream/<arm>/r<NN>-best`; `git log` on it shows the lineage |
| Exploration policy (versioned code) | branch `dream/<arm>/policy`: one commit per round with `policy.json` + snapshot of `policies.py`, tags `dream/<arm>/policy-r<NN>` |
| Dreaming (offline replay) | [sim.py](../research/hashbench/sim.py): `ReplayWorld`, `replay`, `dream` |
| Policy-development agent | (a) automatic: random search over parametric policy families; (b) inspection of recorded trees followed by development of a new family |

An attempt commit, exactly as stored:

```
attempt(shared r1 n002): W 64->16

design: W16-b1-L0-r0.5-o0.05
parent: n001 W64-b1-L0-r0.5-o0.05 (49.26 ns/op)
result: ok 76.49 ns/op (+55.3% vs parent)

Dream-Arm: shared
Dream-Round: 1
Dream-Node: 2
Dream-Parent-Node: 1
Dream-Expansion: 0
Dream-Design: W16-b1-L0-r0.5-o0.05
Dream-Policy: refine_best()
Dream-Budget: 40
Dream-Status: ok
Dream-Score: 76.494
```

Commits are written with plumbing (`hash-object`, `mktree`, `commit-tree`, `update-ref`, `notes add`), about 6 git calls per attempt. The working tree is never touched. History is read back with one `for-each-ref` and one `log --no-walk --stdin` per tree, which parses trailers only. The lineage that found the optimum in the live run is a plain `git log`:

```
$ git log --first-parent --format='%h %s' dream/dream/r08-best   (oldest first, abridged)
6b70215 root(shared r1): W16-b1-L0-r0.5-o0.05          # SwissTable-style start, 81.7 ns/op
3647361 attempt(shared r1 n001): W 16->64
17c4779 attempt(shared r1 n032): L 0->2                 # the funnel breakthrough, 28.6
abc777e attempt(dream r2 n011): r 0.5->0.25
79d36b6 attempt(dream r2 n022): o 0.02->0.005           # 24.7
8ad7418 attempt(dream r5 n014): W 64->16, L 2->3        # 21.7
425b5d0 attempt(dream r7 n004): L 2->4, o 0.02->0.005
16d136b attempt(dream r8 n001): o 0.005->0.01           # true optimum (oracle 19.37)
```

## 2. The task

**Objective** (fixed evaluator, lower is better). Use a fixed-capacity table of 2²¹ slots holding u64→u64 entries (36 MB, just above the 32 MB L3 of the Ryzen 7 9700X it ran on). For each load α ∈ {0.95, 0.98}:
1. Fill to α.
2. Run 1M successful lookups in random order.
3. Run 1M failed lookups.

The score is the mean over α of (insert + hit + miss)/3 in ns/op. Every phase is the minimum of 3 repetitions, and the evaluator is pinned to one core. If any insert hits `Full`, the design is invalid at that load. One evaluation takes about 0.8 s.

**Design space.** 982 distinct designs on the existing `Funnel` engine in hashbench (Farach-Colton, Krapivin & Kuszmaul 2025). I added one knob: buckets spanning several SIMD groups.

| knob | values | meaning |
|---|---|---|
| `W` | 16, 64 | SIMD control-group width (SSE2 / AVX-512 = one cache line) |
| `b` | 1, 2 | SIMD groups per level bucket (β = b·W) |
| `L` | 0,1,2,3,4,5,6,8 | funnel levels (0 = pure SwissTable-style linear group probing) |
| `r` | .25 … .75 | geometric level-size ratio |
| `o` | .005 … .12 | overflow-region fraction |

192 of the 982 designs are invalid: they cannot hold 98% load. Most of these have 1 or 2 levels.

**Discovery agent.** `agent(design, k)` changes 1 knob (70%) or 2 knobs, by ±1 grid step (80%) or ±2. It is seeded by sha256(salt, design, k). Because it's a pure function, the agent's proposal for any "what if we had continued node n once more" is always computable. Only its *score* can be unknown. This property matters in §3.

## 3. Dreaming: how replay works

`ReplayWorld` answers one question: the agent proposed design *d* as the k-th continuation of node *n*; what score does it get?

1. **Recorded edge first.** If this exact (parent attempt, k) was recorded in the tree being replayed, return the recorded outcome. The deployed policy therefore replays its own tree bit for bit (verified).
2. Off the recorded path:
   - **strict** mode: nothing else is known. This is all you can assume with a stochastic LLM agent, and it matches the paper's "a policy can only be dreamt where history actually went".
   - **cache** mode: use any recorded measurement of design *d* from anywhere in the history. This is valid here only because the agent is deterministic and a design's score doesn't depend on how it was reached. It is **drift-corrected**: scores are stored relative to the root of the tree that measured them, then rescaled to the root of the tree being replayed.
3. Anything else is a **MISS**. The project page doesn't say how unrecorded nodes are handled, so I chose a pessimistic rule. The call is still charged and the attempt counts as a failure. This is a pessimistic surrogate for an unknown outcome; it can favor recorded paths and early stopping, so it does not remove selection bias.

**Objective.** For each recorded tree, the policy starts at that tree's root with that round's budget (40). The objective is

J = mean over trees of [ (root − best found)/root − λ·calls/budget ],  with λ = 0.02

In words: a 2% speedup is worth one full round of compute. Policies may stop early.

**Dream step.** Score the deployed policy and 1,000 sampled candidates (300 in the synthetic study) by replay over the whole history, then take the argmax. The deployed policy wins ties. The winner is committed to `dream/dream/policy` with its replay diagnostics (`top5`, J before and after, miss rate) and deployed in the next round.

## 4. Policies

All policies share one interface: `select(tree) -> node_id | None`. They live in [policies.py](../research/hashbench/policies.py) and are serialised as `{"family", "params"}`.

- **π0 `refine_best`** (hand-written start, like the paper's): always continue the best attempt, spend the whole budget.
- **v1 families**:
  - `ucb`: quality + c·exploration bonus, with `kmax` continuations per node and patience/eps stopping.
  - `beam`: top-m attempts, least-continued first.
  - `softmax`: sampling ∝ exp(−Δ/T).
- **v2 family `repair`** was developed after inspection of the recorded trees (commit `da1e0ab`). It makes two changes:
  - **Repair failures.** It may continue *failed* attempts, valued at their nearest valid ancestor's score × (1+fp), at most `kfail` times. 45% of live calls were designs that overflow, and a failed L1 design is often one step from a working L2.
  - **Dedup.** All copies of a design act as one node: continue the copy that has explored furthest, and value the design by the mean of its measurements. The agent's k-th proposal depends on (design, k), so continuing a fresh copy of an already-explored design just replays old proposals. Round 1 had only 11 distinct designs among 41 attempts.

In the end, replay mostly selected `dedup=True`, with mixed `kfail` values. The dedup rule is what carried the improvement (see the exhaustive sweep in §5.2).

## 5. Experiments and results

All live numbers are real measurements stored in git. "Oracle" means one uniform sweep of all 982 designs (`dream/results/oracle.jsonl`). The dream **never** sees the oracle. It is used only to score how good a found design truly is and to drive the synthetic replicates. Optimum: `W16-b1-L4-r0.25-o0.01`, 19.37 ns/op.

### 5.1 Live run, rounds 1–6 (v1 families): dreaming re-selected π0

Round 1 is shared by both arms and is identical by construction. Budget is 40 calls per round.

- **Round 1** found the funnel breakthrough (81.7 → 28.6 ns/op), but only on call 32. π0 spent 31 calls re-continuing one node (`W64-L0`) whose only possible mutations were duplicates (`W 64→16`) or overflowing `L1` designs.
- **Every dream step (r2–r6)** returned π0. The top 5 of 1,000 candidates tied with it exactly, and all were greedy variants. Two reasons:
  1. The recorded world was tiny where it mattered. Policies that branch elsewhere run into MISSes, and policies that stop early lose round 1's 65% gain.
  2. J is dominated by that round-1 gain, so saving compute on plateaus never paid for missing it.

  This is the same limitation the page states (dreaming is exact only where history went), observed in practice.
- With the same π0, the two arms diverged by measurement noise. In r5 the "dream" arm stumbled into `W16-L3` (oracle 20.43). The fixed arm stayed in the `W64-L2` basin (24.68).

### 5.2 Policy-development step

I read the recorded trees: the duplicate proposals, the 45% overflow failures and the late breakthrough. From that I wrote the `repair` family. An exhaustive replay sweep of its 36,000 parameter settings over the 6 recorded trees gave:

| replay mode | best `repair` J | π0 J |
|---|---|---|
| cache | **0.173** (dedup=True) → 0.209 with drift correction | 0.132 |
| strict | 0.132 (identical to π0) | 0.132 |

Without dedup, even the best `repair` setting only tied π0.

### 5.3 Live run, rounds 7–10 (v2 families available, roots re-measured)

| round | fixed arm (π0) calls → true quality | dream arm: dreamt policy | calls → true quality |
|---|---|---|---|
| 6 (state) | cum 240, 1.274× opt | (π0 so far) | cum 240, 1.055× |
| 7 | 40, 1.469×¹ | `repair(c=.02,fp=.3,kmax=3,kfail=4,patience=16,dedup)` | **16**, 1.003× |
| 8 | 40, 1.342×¹ | `repair(c=0,fp=.01,kmax=3,kfail=4,patience=16,dedup)` | **17**, **1.000×** |
| 9 | 40, 1.274× | `ucb(c=.05)` | 40, 1.000× |
| 10 | 40, 1.274× (cum 400) | `repair(c=.1,fp=.3,kfail=4,dedup)` | 40, 1.104×² (cum 353) |

¹ The incumbent is chosen by its *recorded* (noisy) score, so it can swap to a truly worse design. This is the winner's curse.
² The same effect: a lucky 22.41 reading made `W16-L5` (true 21.38) beat the root's fresh 21.75.

The dream arm behaved qualitatively the way the paper reports. When the replayed history said progress was likely cheap, it **conserved compute** (16–17 calls instead of 40). When gains plateaued, it **spent again** (40). Note what the replay diagnostics showed, though. The r7 winner's replay J fell from 0.32 to 0.10 once its own round entered history, because it had a 58% miss rate. Selection over 1,000 candidates on sparse history is optimistic.

### 5.4 Synthetic replicates (100 per configuration, oracle world + 6% noise)

This runs the full protocol (shared r1, then 7 rounds per arm) in a world backed by the oracle sweep, with multiplicative Gaussian noise matching the measured noise level. Each replicate uses a different agent salt. Values are means after 8 rounds:

| configuration | fixed π0: gap to opt / calls | Dream-RSI: gap / calls | paired Δgap (± s.e.) | dream better / tie / worse |
|---|---|---|---|---|
| v1 families, cache replay | 14.6% / 320 | 12.9% / 243 | +0.017 ± 0.016 (n.s.) | 41 / 17 / 42 |
| **v2 families, cache replay** | 14.6% / 320 | **8.4% / 238** | **+0.061 ± 0.022** | 50 / 11 / 39 |
| v2 families, λ = 0 | 14.6% / 320 | 8.8% / 282 | +0.058 ± 0.015 | 49 / 15 / 36 |
| v2 families, **strict** replay | 14.6% / 320 | 19.3% / 195 | −0.047 ± 0.014 (worse) | 29 / 21 / 50 |

What this says:
- **v1** (the families the live dream also had in r2–r6) buys 24% less compute at statistically equal quality.
- **v2** is the first configuration where dreaming improves *quality* as well as cost. The improvement doesn't depend on the compute penalty (the λ=0 row).
- **Strict** replay doesn't work in this setting. With only recorded edges, alternatives are mostly MISSes. The dream learns to stop early and settles too soon.
- Early rounds are a weak spot. After round 2 the dream arm is slightly *behind* (1.235× vs 1.200×), because a policy dreamt on a single tree overfits it. The benefit appears from round 4 on, as history accumulates.

### 5.5 Fidelity: does replay rank policies the way the world does?

I took 401 policies (π0 + 400 random v1/v2) and replayed each over the 10 live dream-arm trees. I compared that with running the same policy from the same roots in the oracle world ([fidelity-dream-r10.json](../research/hashbench/results/fidelity-dream-r10.json)):

| | strict | cache |
|---|---|---|
| Spearman ρ(replay J, true J) | 0.01 | 0.10 |
| mean miss rate | 42% | 14% |
| true percentile of the replay-selected policy | 88% | 88% |

Ranking fidelity across *all* policies is weak. Most candidates drift into unrecorded territory, where MISSes dominate their score. Even so, the argmax lands in the top ~12% of true quality, which is why selection still helps on average (§5.4). The truly best policy in that pool was also a dedup `repair` (true J 0.166). The replay-selected one scored 0.073, the same as π0. There is clear headroom for better replay.

## 6. The hash-table result

| design (fixed capacity, 36 MB) | ns/op | 95%: ins / hit / miss | 98%: ins / hit / miss | max probes @98% |
|---|---|---|---|---|
| SwissTable-style `W16-L0` (root) | 71.6 | 19.0 / 21.1 / 55.9 | 22.6 / 24.8 / **286** | 1371 |
| SwissTable-style `W64-L0` | 51.2 | 21.2 / 23.6 / 43.9 | 23.6 / 26.5 / 168 | 343 |
| fixed arm's best `W64-b1-L2-r0.25-o0.005` | 24.7 | 22.4 / 24.2 / 23.0 | 22.5 / 27.7 / 28.3 | 7 |
| **optimum `W16-b1-L4-r0.25-o0.01`** | **19.4** | 19.3 / 20.0 / 17.7 | 19.4 / 20.6 / **19.2** | **8** |
| hashbrown (reference only: grows itself, ~48% load, **71 MB**) | 17.2 | 31.8 / 15.6 / 4.3 | — | — |

The basin of good designs is consistent: **16-wide groups, level ratio r = 1/4, 3–5 levels, ≤ 3% overflow**. Many designs within ~5% of the optimum are statistically tied at this noise level. The hashbench session's independent probe sweep reached the same conclusion (L=3, r=0.25, 2–3% overflow). At half the memory, the funnel table comes within about 13% of hashbrown's op cost. Swiss-style probing at the same capacity collapses on failed lookups.

Cross-checks from the parallel hashbench session (branch `bench/main`), and what they mean for this objective:
- **Lookup inlining.** That session found `Funnel::get` compiled out of line in its harness, costing about 5 ns per DRAM-resident lookup. I rebuilt `dream-eval` with `#[inline(always)]` on `get` and measured 10 designs, interleaved with the original binary. The new/old ratio was 0.955–1.048 (mean ≈ 1.00, within noise), with rank correlation 0.99. In this harness at this size, there is no measurable handicap, and the recorded scores and rankings stand.
- **Larger tables.** At 7.3M keys (fully DRAM-resident), hashbrown at its own 87.5% load beats funnel16 L3 r0.25 o0.03 at 95% on successful lookups: 20.7 vs 27.5 ns. Failed lookups are equal (≈18.7 ns). At 99% load that design overflows (`full`), and L4 r0.5 o0.03 is needed. This objective (2²¹ slots, 95/98%) doesn't see either effect.
- **Churn.** Under delete/insert churn, greedy funnel placement drifts keys into the overflow region and needs rebuilds in random order. This objective is insert-only, so it doesn't measure that.

## 7. Lessons and limitations

1. **Git really is enough as memory.** Commits, trailers, notes and refs give you an append-only, inspectable and diffable discovery DAG, and `git log` shows the lineage for free. One caveat: `git gc --prune` must not remove `refs/dream/*`. Those are ordinary refs, so normal gc keeps them.
2. **Dreaming is bounded by coverage.** Exact replay exists only where history went. With the paper-faithful strict rule, alternatives are mostly MISSes, and in this task that made the dream *worse*. The cross-tree cache is what made it useful, and it is valid only because the agent here is deterministic. With an LLM agent, replay cannot establish outcomes for unobserved changes. Fresh prospective evaluations are needed to test policy improvements; a learned outcome model would introduce additional assumptions.
3. **The policy space matters more than the search over it.** 1,000 random v1 candidates per round never beat π0. One additional family, `repair` with dedup, based on reading the trees, did. This matches the paper's framing: the policy-development agent writes *code*, it doesn't just tune knobs.
4. **Noise and drift are first-class problems.** Measurement noise was ~5–9%. The machine slowed by about 23% mid-session when another benchmark started sharing DRAM/L3. Without the three corrections (fresh root measurements, drift-normalised cache, a single uniform oracle sweep), scores from different times aren't comparable. Incumbents chosen by recorded score suffer the winner's curse (§5.3 r10). The current Rust tool remeasures finalists and the incumbent root using fresh repetitions, rejecting candidates with failed confirmations. That protection was absent from these archived runs.
5. **Replay is optimistic about its own pick.** The replay J of the selected policy dropped sharply once its own round entered history. The current Rust tool reserves recent rounds for a deployment check and rejects a candidate with a higher held-out miss rate. These checks diagnose some overfitting; they do not eliminate behavior-policy selection or make repeatedly consulted rounds an untouched test set.
6. **Scope.** The discovery agent is a mutation operator, not an LLM. The design space is a 982-point grid, so the oracle can be measured exhaustively, which is what made validation possible. The paper's tasks are open-ended code. The machinery here (git memory, replay, selection, versioned policies) carries over unchanged. Only `agent` and `dream-eval` would be replaced: by an LLM producing a code diff per attempt, and by a build-and-benchmark of that commit.

## 8. Reproducing

```bash
cd dream-rsi
cargo build --release && cargo test --release          # incl. multi-group bucket tests
python3 dream/dream.py experiment --rounds 10           # live: resumes from git, skips recorded rounds
python3 dream/dream.py show --arm shared --round 1      # print a discovery tree rebuilt from git
python3 dream/dream.py report                           # live arms vs oracle
python3 dream/dream.py oracle                           # measure all 982 designs (~15 min)
python3 dream/dream.py synthetic --reps 100 --rounds 8 --families v2 --tag v2
python3 dream/dream.py fidelity --arm dream --upto 10

git log --oneline dream/dream/policy                    # every deployed policy, one commit per round
git show dream/dream/policy-r07:policy.json             # its replay diagnostics (top-5, J, miss rate)
git notes --ref=dream show dream/dream/r08-best         # full evaluator output of an attempt
```

Starting from scratch is just deleting the refs: `git for-each-ref --format='%(refname)' refs/dream | xargs -n1 git update-ref -d`.

Files: [space.py](../research/hashbench/space.py) (design grid and agent) · [gitmem.py](../research/hashbench/gitmem.py) (git memory) · [policies.py](../research/hashbench/policies.py) · [sim.py](../research/hashbench/sim.py) (worlds, replay, dream) · [dream.py](../research/hashbench/dream.py) (CLI and protocols) · [dream-eval.rs](../research/hashbench/dream-eval.rs) · results in [results/](../research/hashbench/results/).

**Repository notes.** All work is on branch `dream/main` (from `dream/base`, a snapshot of `hashbench/` at 11:00) in a separate worktree. `hashbench/`'s own working tree and its unborn `master` are untouched, because another session is developing there. Live evaluations were pinned to core 3, and the synthetic and fidelity runs to cores 8–15.
