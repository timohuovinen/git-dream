#!/bin/sh
# Speed track evaluator: same results, less time. Output of the comparison harness
# must be byte-identical to meta/reference/compare.jsonl (made by meta/setup.sh at
# the base commit); score = best-of-3 wall seconds (min). Any behaviour change fails.
set -eu
seeds=${META_SEEDS:-30}
first=${META_FIRST_SEED:-50000}
ref=meta/reference/compare.jsonl
out=.git-dream-eval
mkdir -p "$out"

[ -f "$ref" ] || { echo "missing $ref: run meta/setup.sh first" >&2; exit 2; }
cargo build --release --locked -q --examples >&2
cargo test --release --locked -q >&2 || { echo "tests failed" >&2; exit 2; }

best=
for i in 1 2 3; do
  t0=$(date +%s.%N)
  ./target/release/examples/compare --seeds "$seeds" --first-seed "$first" > "$out/compare.jsonl"
  t1=$(date +%s.%N)
  cmp -s "$out/compare.jsonl" "$ref" || { echo "output differs from reference (behaviour changed)" >&2; exit 3; }
  t=$(echo "$t0 $t1" | awk '{printf "%.4f", $2 - $1}')
  best=$(echo "${best:-$t} $t" | awk '{print ($2 < $1) ? $2 : $1}')
done
echo "{\"score\": $best, \"status\": \"ok\"}"
