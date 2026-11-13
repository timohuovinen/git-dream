# Working on git-dream (instructions for coding agents)

git-dream is a Rust CLI for git-native experimental search: a fixed agent proposes changes, a fixed
evaluator scores them, and an exploration policy decides what to continue. Each attempt is a commit
under `refs/dream/*`. Read `README.md` (concepts) and `docs/validation.md` (current evidence) first.

You are usually running **inside git-dream itself**: an attempt in a scratch worktree, launched by a
pinned git-dream binary. Your environment provides:

- `$DREAM_PROMPT`: the task, plus where this attempt sits in the search tree.
- `$DREAM_CONTEXT`: every earlier attempt, with its write-up, score and the evaluator output for failures.
  **Read it before choosing an idea.** Don't repeat an idea that was already measured, unless you can
  say why its implementation, rather than the idea, was at fault (`git show <sha>` to check).
- `$DREAM_MESSAGE_FILE`: write your proposal here. Line 1 is a one-line summary, then a blank line,
  then the idea, the evidence from the history, why it isn't a repeat, and your own measured numbers.

## Rules (all tracks)

- One focused change per attempt. A small change that's clearly attributable beats a big rewrite.
- Never edit `meta/`, `examples/compare.rs`, `tests/`, `research/`, `docs/validation.md` or `Cargo.lock`.
  They are the evaluation contract, and attempts that touch them are rejected automatically.
- Don't add dependencies without a strong reason. They change `Cargo.lock`, and the attempt is rejected.
- `cargo test --release --locked` must pass. The evaluator runs it first.
- Policies must stay deterministic: the decision depends only on (tree, params, seed). Otherwise
  replay can't reproduce live rounds (see `src/policies.rs`, `src/rng.rs`).
- Don't special-case the three synthetic landscapes (smooth/rugged/deceptive), their optimum
  (x=23, y=9) or seed ranges. Policies only see the tree, never `compare.rs` internals. Gains that
  only come from overfitting the benchmark will be thrown away at the human's final check on
  unseen seeds and tasks.
- Local builds are slow. Prefer `cargo check` while iterating, and use
  `META_SEEDS=10 sh meta/eval-policy.sh` for quick estimates before the full evaluator.

## Policy track (`meta/policy.toml`)

Score: mean over tasks of `log(1 + mean final regret)` for `adaptive_cache` and `adaptive_strict`
at exactly 97 evaluations, on training seeds 50000+. Lower is better.

What the evidence says (docs/validation.md):

- the adaptive arms only *match* fixed `refine_best`, because the dream never deploys anything better
  than the incumbent;
- multi-start successive halving and even uniform random search beat them by a lot on rugged and
  deceptive landscapes: the local tree search gets stuck in basins.

Promising directions, roughly in order:

1. **A new policy family** in `src/policies.rs` that adds diversity: restarts from under-explored
   or old nodes, successive-halving-style allocation across subtrees, tabu or novelty on `design`,
   or a plateau detector that switches from exploiting to exploring. Give it a small grid so dreaming
   can tune it.
2. **Dream selection** (`src/sim.rs`): candidate sampling, the J objective, how misses are charged,
   tie-breaking, and the holdout gate (it currently blocks almost everything). Keep the gate's intent,
   which is to avoid deploying replay-overfit policies.
3. **Lock-in avoidance**: make policies that stop early or exploit hard keep enough data collection.

## Speed track (`meta/speed.toml`)

Score: best-of-3 wall seconds of `examples/compare`, whose output must stay **byte-identical** to
`meta/reference/compare.jsonl`. Behaviour must not change at all, including RNG draw order and
floating-point summation order.

- Profile first (`perf record -g ./target/release/examples/compare --seeds 10 >/dev/null`, or
  `cargo flamegraph` if installed). Optimise what the profile shows, not what you guess.
- Likely hot spots: replay and dream loops in `src/sim.rs`, tree queries in `src/tree.rs`
  (repeated linear scans over children and designs, cloning, string `design` keys), and policy
  scoring in `src/policies.rs`.

## Scale track (manual bootstrap, not yet a dream run)

Goal: work well with very large histories, up to millions of attempts. A scale benchmark doesn't
exist yet. When asked to work on this track, first add `examples/scale.rs`, which builds synthetic
trees of 1e4, 1e5 and 1e6 nodes and times tree rebuild, replay and one dream. Also add
`meta/eval-scale.sh`. A human then reviews and commits both, adds them to `protected`, and creates
`meta/scale.toml`. Areas to examine afterwards: rebuilding trees from commit trailers on every dream
(`src/memory.rs`) and the number of git processes spawned per attempt.
