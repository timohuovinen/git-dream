# Configuration and behavior changes

These changes apply to the current source checkout. The archived hashbench experiments retain their original protocol; their reported results have not been regenerated with these safeguards.

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

## External policies and invalid inputs

External policies have a per-request/reply deadline, configured by `timeout` in their `[policies.external.NAME]` table (default: 10 seconds). Invalid replies, unknown node IDs, closed output, and deadline failures return an error instead of being scored as an inexpensive stopping decision. Timeout cleanup terminates the policy process group. Replies are limited to 1 MiB.

Zero exploration budgets, invalid timeout/probability values, empty external-policy grids, and unsupported history/normalization settings are rejected. Scripts that relied on silently accepted invalid values must correct their configuration.

Existing attempts are retained when an operation fails. A failed or interrupted round can remain incomplete; automatic recovery of incomplete rounds is outside these changes. Inspect its refs and notes before deciding how to continue.
