#!/bin/sh
# A stand-in for an LLM coding agent: one truly random edit (seeded from
# /dev/urandom, like an LLM's sampling) to a constant in solver.awk, described
# in $DREAM_MESSAGE_FILE. It receives the same $DREAM_CONTEXT an LLM would.
set -e
seed=$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')
awk -v seed="$seed" -v msg="${DREAM_MESSAGE_FILE:-/dev/null}" '
BEGIN { srand(seed); split("STEP DAMPING ITERATIONS", names, " "); pick = names[int(rand() * 3) + 1] }
$1 == pick && $2 == "=" {
  old = $3
  if (pick == "ITERATIONS") { split("-4 -2 -1 1 2 4 8", d, " "); new = old + d[int(rand() * 7) + 1]; if (new < 1) new = 1 }
  else { split("0.5 0.7 0.9 1.1 1.4 2.0", f, " "); new = sprintf("%.4g", old * f[int(rand() * 6) + 1]) }
  $0 = pick " = " new
  print pick " " old " -> " new > msg
}
{ print }' solver.awk > solver.awk.tmp
mv solver.awk.tmp solver.awk
