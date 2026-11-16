#!/bin/sh
# Score codec.rs: bytes of corpus.txt after compress() + bytes of codec.rs. Lower is better.
#
# The score depends only on codec.rs and corpus.txt, never on timing or machine load: compress and
# decompress run in separate processes, in an empty directory, under a CPU-time limit (not a
# wall-clock one). The last stdout line is the result as JSON. Needs rustc and a POSIX shell.
set -eu
cd "$(dirname "$0")"
CPU_SECONDS=20
fail() { echo "$*" >&2; exit 1; }

# A codec may compute. It may not read files, the environment or the network, run programs, or
# pull in code that the score does not count.
banned='\b(unsafe|extern|asm|global_asm|include|include_bytes|include_str)\b'
banned="$banned"'|\b(fs|env|process|net|io|path|thread|os|ffi)[[:space:]]*::'
banned="$banned"'|::[[:space:]]*(fs|env|process|net|io|path|thread|os|ffi)\b'
banned="$banned"'|[{,][[:space:]]*(fs|env|process|net|io|path|thread|os|ffi)[[:space:]]*[,}]'
banned="$banned"'|#!?[[:space:]]*\[[[:space:]]*path|\bmod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*;'
if hits=$(grep -n -E "$banned" codec.rs); then
  fail "codec.rs uses something that is not allowed (std only, one file, no unsafe, no file/env/process/net/io/thread access, no include macros):
$hits"
fi

mkdir -p build
rustc --edition=2021 -O -o build/codec harness.rs 2> build/rustc.log || { tail -n 40 build/rustc.log >&2; fail "codec.rs does not compile"; }

bin=$(pwd)/build/codec
here=$(pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# compress, then decompress in another process; $1 = input file
round_trip() {
  for step in compress decompress; do
    if [ $step = compress ]; then in=$1 out=$tmp/blob; else in=$tmp/blob out=$tmp/back; fi
    status=0
    (cd "$tmp" && ulimit -t $CPU_SECONDS && ulimit -v 2097152 && "$bin" $step < "$in" > "$out" 2> "$tmp/err") || status=$?
    if [ $status -ne 0 ]; then
      case $status in 137|152) why="more than $CPU_SECONDS s of CPU time" ;; *) why="exit $status" ;; esac
      tail -n 20 "$tmp/err" >&2
      fail "$step of $(basename "$1"): $why"
    fi
  done
  cmp -s "$1" "$tmp/back" || fail "round trip failed for $(basename "$1"): decompress(compress(x)) is $(wc -c < "$tmp/back") bytes, x is $(wc -c < "$1")"
}

# any input must survive, not only the corpus
: > "$tmp/empty"
printf 'a' > "$tmp/one-byte"
awk 'BEGIN { for (r = 0; r < 4; r++) for (i = 0; i < 256; i++) printf "%c", (i * 73 + r * 31) % 256 }' > "$tmp/all-bytes" 2>/dev/null || true
for f in "$tmp/empty" "$tmp/one-byte" "$tmp/all-bytes"; do round_trip "$f"; done
round_trip "$here/corpus.txt"

compressed=$(wc -c < "$tmp/blob" | tr -d ' ')
code=$(wc -c < codec.rs | tr -d ' ')
echo "{\"score\": $((compressed + code)), \"status\": \"ok\", \"compressed\": $compressed, \"code\": $code, \"corpus\": $(wc -c < corpus.txt | tr -d ' ')}"
