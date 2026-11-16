#!/bin/sh
# One run of the codec experiment: a fresh repository, one exploration strategy, TOTAL attempts
# by a real coding agent (examples/codec/agent.sh).
#
#   sh examples/codec/evidence/run.sh WORK ARM REP [TOTAL] [STEP]
#
# WORK   directory for the run's repository (WORK/ARM-REP) and its usage records
# ARM    fixed   refine_best, one tree of TOTAL attempts (git-dream's fixed baseline)
#        bandit  the default policy, one tree of TOTAL attempts
#        chains  4 independent chains per round, rounds of STEP (the paper's fixed baseline)
#        paper   chains in round 1, then the policy that scores best on strict replay of the
#                recorded rounds, rounds of at most STEP until TOTAL attempts are spent
# All arms run 4 attempts at once. GIT_DREAM names the binary (default: git-dream on PATH).
set -eu
work=$1 arm=$2 rep=$3 total=${4:-48} step=${5:-24}
here=$(cd "$(dirname "$0")/.." && pwd)
G=${GIT_DREAM:-git-dream}
# the chains policy: cargo build --release --example chains
CHAINS=${CHAINS:-$here/../../target/release/examples/chains}
repo=$work/$arm-$rep
mkdir -p "$work/usage/$arm-$rep"
CODEC_USAGE_DIR=$(cd "$work/usage/$arm-$rep" && pwd); export CODEC_USAGE_DIR
rm -rf "$repo"; mkdir -p "$repo"
for f in .gitignore codec.rs harness.rs eval.sh corpus.txt agent.sh; do cp "$here/$f" "$repo/"; done

case $arm in
  fixed)  extend=true;  initial='{ family = "refine_best" }'; method=auto ;;
  bandit) extend=true;  initial='{ family = "bandit" }'; method=auto ;;
  chains|paper) extend=false; initial='{ family = "chains", params = { w = 4, d = 6 } }'; method=replay ;;
  *) echo "unknown arm $arm" >&2; exit 2 ;;
esac
cat > "$repo/.git-dream.toml" <<TOML
run = "main"

[agent]
kind = "command"
command = "./agent.sh"
prompt = """
Lower the score printed by \`sh eval.sh\`: the size in bytes of corpus.txt after compress() plus the size in bytes of codec.rs.
codec.rs must keep \`pub fn compress(data: &[u8]) -> Vec<u8>\` and \`pub fn decompress(blob: &[u8]) -> Vec<u8>\`, with decompress(compress(data)) == data for any input.
Rules, enforced by eval.sh: one file, std only; compress and decompress run in separate processes (do not rely on HashMap iteration order), each with 20 s of CPU time; no unsafe, no file, environment, process, network, io or thread access, no include macros.
Do not edit eval.sh, harness.rs or corpus.txt. Put experiments in scratch/ (it is ignored). Make one focused change to codec.rs and run \`sh eval.sh\` before you finish.
"""
timeout = 900

[evaluate]
command = "sh eval.sh"
goal = "min"
timeout = 300
protected = ["eval.sh", "harness.rs", "corpus.txt", "agent.sh", ".git-dream.toml", ".gitignore"]

[explore]
budget = $step
min_calls = 4
extend = $extend
parallel = 4
evaluations = "parallel"

[dream]
mode = "strict"
method = "$method"
holdout = 0
lam = 0.02
candidates = 300

[policies]
initial = $initial

[policies.external.chains]
command = "$CHAINS"
grid = { w = [1, 2, 4, 8], d = [1, 2, 3, 6, 12] }
TOML

cd "$repo"
git init -q
git add -A
git -c user.name=codec -c user.email=codec@localhost commit -qm "codec: start"
used() { echo $(( $(git for-each-ref --format='%(refname)' refs/dream/main | grep -c '/n[0-9]*$' || true) - $(git for-each-ref --format='%(refname)' refs/dream/main | grep -c '/n000$' || true) )); }
case $arm in
  fixed)  "$G" run -n $(( total / step )) --no-dream ;;
  bandit) "$G" run -n $(( total / step )) ;;
  chains) "$G" run -n $(( total / step )) --no-dream ;;
  paper)
    rounds=0
    while u=$(used); [ "$u" -lt "$total" ] && [ "$rounds" -lt 12 ]; do
      left=$(( total - u )); [ "$left" -gt "$step" ] && left=$step
      "$G" run -n 1 --budget "$left"
      rounds=$(( rounds + 1 ))
    done ;;
esac
"$G" status
