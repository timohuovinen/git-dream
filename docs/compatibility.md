# Configuration and behavior changes

These changes apply to the current source checkout. The archived hashbench experiments retain their original protocol; their reported results have not been regenerated with these safeguards.

## Default policy, yield model and round extension

`[policies] initial` now defaults to `{ family = "bandit" }` when a configuration does not set it. Configurations that name a family (the earlier templates wrote `refine_best`) behave as before, and so do runs that already deployed a policy.

For a `bandit` policy, dreaming fits the yield model over recorded attempts instead of replaying candidates: `holdout`, `lam`, `candidates`, `mode` and `normalize` are not used, and the policy commit holds a `model` table instead of a replay ranking. `[dream] method = "replay"` restores replay-based selection for every family.

`[explore] extend` defaults to `false`; newly generated templates set it to `true`. With it, `git dream run` adds `budget` proposals to the latest round's tree instead of starting a round, also when that round was killed before promotion, and `--new-round` starts a round. `--extend` does the same for one invocation. The round's policy is first driven through its recorded attempts, so an extended round equals one that ran uninterrupted with the larger budget. If the policy cannot reproduce the recorded attempts (an external policy that is not deterministic), extension fails and nothing is recorded. Root re-measurement happens only when a round starts; confirmation runs at the end of every step.

## Scratch worktrees are outside `.git`

Attempts are built and evaluated in scratch worktrees. These were in `.git/dream/wt/`. They are now in `$GIT_DREAM_WORKTREES/<repository>/` if that variable is set, otherwise in the user's cache directory: `$XDG_CACHE_HOME/git-dream/<repository>/`, `%LOCALAPPDATA%\git-dream\<repository>\` or `~/.cache/git-dream/<repository>/`. `<repository>` is the directory's name and a hash of its path.

The reason: Claude Code refuses to write any file below a `.git` directory, so an agent started in `.git/dream/wt` could not edit the code. In the first run with a real agent every attempt ended unchanged. Commits, refs and notes are where they were.

After upgrading, the old worktrees are unused: delete `.git/dream/wt` and run `git worktree prune`. Ignored build outputs in them are rebuilt once in the new place. `git dream clean` removes a run's worktrees there.

## Trailers are read with git's parser

A recorded outcome is the last paragraph of the attempt's commit message, read with `%(trailers)`. Earlier, any line of the message that began with `Dream-` was read as an outcome, so an agent's write-up that quoted such a line (they read them in the history) could change what was loaded and stop the round from being extended. No recorded history changes meaning unless a write-up contained such a line.

## Attempts at once

`[explore] parallel` (default 1) and `[explore] evaluations` (default `"alone"`) are new; a configuration without them behaves as before, one attempt at a time, and writes the same commits. `git dream run` and `git dream explore` accept `--parallel N`.

With `parallel` above 1:

- Each attempt in flight uses its own scratch worktree: `<run>` for lane 0, `<run>@<lane>` for the others, in the repository's worktree directory (see below). Agents and evaluators receive `$DREAM_LANE`.
- An attempt is committed when it starts, with `Dream-Status: pending` and no score, and its ref is moved to the real commit when it finishes. A round killed mid-flight therefore keeps `pending` attempts; extending the round runs them again, and a new round ignores them. `pending` is a new status: tools that read the trailers, and external policies, will see it.
- A result that arrived after later attempts had been chosen carries `Dream-Seen: <id>`, the first attempt chosen with that result known. Extension uses it to show the round's policy exactly what it saw at each recorded choice.
- With `evaluations = "alone"` (the default) nothing else runs during an evaluation: attempts go in waves. For a parameter-space agent this leaves nothing to overlap, so `parallel` is ignored unless `evaluations = "parallel"`.
- An extended round no longer equals an uninterrupted one in general: a step's budget ends a wave early, so later attempts are chosen with different information. With `parallel = 1` the equality holds as before.
- `evaluate.reuse` does not stop two identical designs that are in flight together from both being measured.
- Confirmation re-measurements run at once only with `evaluations = "parallel"`.

While dreaming, replay candidates are scored on all cores. The ranking is identical to the single-threaded one.

An external policy that exits without reading its request is now reported with what it printed (`policy stopped reading requests …; it replied "…"`) instead of, depending on timing, a broken-pipe error.

## Policy selection and replay

`[dream] holdout` now defaults to `1`: the newest round of each run is reserved for a deployment check, with a separate measurement cache. The training winner must improve held-out J without increasing the miss rate. Insufficient history keeps the incumbent. Set `holdout = 0` explicitly to use the earlier unguarded selection rule.

`git dream replay --holdout N` reports training and held-out diagnostics for the supplied policies. It does not deploy a policy. `git dream dream --dry-run` reports the training ranking and deployment decision without writing a policy commit. Repeated use of the moving holdout does not provide an unbiased estimate of performance on unseen tasks; see [validation](validation.md).

`[explore] min_calls` defaults to `0` for existing configurations. Newly generated space/command templates set it to `5`/`2`, respectively. If a policy stops before the floor, best-node refinement supplies more proposals. The floor must not exceed the configured or command-line budget. A floor equal to the budget enforces equal proposal counts, not equal wall time or total evaluator cost.

Replay and continued exploration check recorded agent/evaluator configurations and base commits. Changing the objective, operator, or evaluation settings requires a separate run. For independent comparisons, use a fresh run name and `history = "run"`. `--no-dream` freezes the current policy of a run; it does not reset an already adapted run to its initial policy.

## Measurement and evaluator accounting

`budget` counts proposals, including failures and no-ops. Root evaluations and confirmation measurements are additional work. `status` reports evaluator executions separately, excluding reused scores and no-ops.

With `confirm_top > 0`, confirmation includes the incumbent root as well as the selected candidate designs. Selection uses fresh repetitions only; the original selection measurement is excluded. A failed repetition disqualifies the candidate. If every finalist fails, promotion returns an error. Confirmation output and means are preserved in Git notes, and the next root inherits the confirmed mean unless remeasurement is enabled.

`evaluate.reuse` survives reopening a run. `remeasure_root = true` takes precedence over reuse for root measurements. Changed evaluation settings cannot be mixed into an existing run.

For command agents, `evaluate.protected = ["bench.sh", "tests"]` rejects attempts that change the listed paths relative to the base commit. The rejected attempt is still recorded. This catches direct edits to the evaluation contract; it does not isolate code execution.

## Interrupted runs

A run killed mid-round (timeout, Ctrl-C, reboot) can be continued: `run` and `explore` first close the interrupted round by promoting the best of the attempts it recorded, then start the next round. That round's tree has fewer calls than its budget, and replay treats the missing proposals as missing data. If no attempt of the interrupted round was scored, the run stops with an error instead; start a new run after fixing the evaluator. Earlier versions failed with "round N has no promoted attempt". Two processes must still not run the same run at the same time.

## External policies and invalid inputs

External policies have a per-request/reply deadline, configured by `timeout` in their `[policies.external.NAME]` table (default: 10 seconds). Invalid replies, unknown node IDs, closed output, and deadline failures return an error instead of being scored as an inexpensive stopping decision. Timeout cleanup terminates the policy process group. Replies are limited to 1 MiB.

Zero exploration budgets, invalid timeout/probability values, empty external-policy grids, and unsupported history/normalization settings are rejected. Scripts that relied on silently accepted invalid values must correct their configuration.

Existing attempts are retained when an operation fails. A failed or interrupted round can remain incomplete; automatic recovery of incomplete rounds is outside these changes. Inspect its refs and notes before deciding how to continue.
