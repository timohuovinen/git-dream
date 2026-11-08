#!/bin/sh
# Evaluator for the hashbench case study: build dream-eval once (target/ is
# ignored, so it survives in git-dream's scratch worktree) and measure one design.
# Pinned to one core; dream-eval prints {"score": ns/op | null, "status": "ok" | "full", ...}.
set -e
cargo build --release -q --bin dream-eval
printf 'W%s-b%s-L%s-r%s-o%s\n' "$DREAM_PARAM_W" "$DREAM_PARAM_B" "$DREAM_PARAM_L" "$DREAM_PARAM_R" "$DREAM_PARAM_O" \
  | taskset -c "${DREAM_CPU:-3}" target/release/dream-eval
