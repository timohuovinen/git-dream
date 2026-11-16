#!/bin/sh
# Correctness first, then the benchmark: its last stdout line is the score.
set -eu
mkdir -p build
case $(uname -s) in MINGW*|MSYS*|CYGWIN*) exe=.exe ;; *) exe= ;; esac
rustc --edition=2021 tests.rs --test -o "build/tests$exe"
"./build/tests$exe"
rustc --edition=2021 bench.rs -O -o "build/bench$exe"
"./build/bench$exe"
