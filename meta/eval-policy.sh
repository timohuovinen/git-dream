#!/bin/sh
# Policy track evaluator: does the attempt make git-dream's adaptive search find
# better solutions at the same evaluation budget? Last stdout line = JSON score (min).
#
# Uses TRAINING seeds only (META_FIRST_SEED, default 50000). Never use 0-19 (frozen
# policy training) or 10000-10099 (the published test set): those are for the human's
# final, frozen comparison. See meta/README.md.
set -eu
seeds=${META_SEEDS:-30}
first=${META_FIRST_SEED:-50000}
out=.git-dream-eval
mkdir -p "$out"

cargo build --release --locked -q --examples >&2
cargo test --release --locked -q >&2 || { echo "tests failed" >&2; exit 2; }
./target/release/examples/compare --seeds "$seeds" --first-seed "$first" > "$out/compare.jsonl"
python3 meta/score.py "$out/compare.jsonl"
