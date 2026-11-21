#!/bin/sh
# Reproduce the session-authored candidate experiment in an isolated repository.
#   cargo build --locked && sh examples/log-count/evidence/run.sh --output DIR   (DIR must not exist)
set -eu
[ $# -eq 2 ] && [ "$1" = --output ] || { echo "usage: run.sh --output DIR" >&2; exit 2; }
root=$(cd "$(dirname "$0")/../../.." && pwd)
mkdir -p "$(dirname "$2")"
mkdir "$2"
output=$(cd "$2" && pwd)
binary=$root/target/debug/git-dream
[ -x "$binary" ] || binary=$binary.exe
repo=$(mktemp -d "${TMPDIR:-/tmp}/git-dream-log-count-XXXXXX")
trap 'rm -rf "$repo"' EXIT
cp -R "$root/examples/log-count/." "$repo"
rm -rf "$repo/evidence"   # the recorded run is not part of the project being explored
cd "$repo"
git init -q
git config user.name timohuovinen
git config user.email timohuovinen@users.noreply.github.com
git add -A
git commit -qm "Baseline log aggregation fixture and independent evaluator"
"$binary" run --no-dream > "$output/run.log"
{ rustc --version; git --version; } > "$output/environment.txt"
git bundle create -q "$output/history.bundle" --all
git show-ref > "$output/refs.txt"
echo "$output"
