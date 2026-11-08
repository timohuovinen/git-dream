#!/bin/sh
# Score = error of solve() on a few targets + a cost per iteration (lower is
# better). Exit 1 (a failed attempt) if the solver blows up.
exec awk -f solver.awk -f /dev/stdin <<'AWK'
BEGIN {
  n = split("1 5 -3 10", ts, " ")
  for (i = 1; i <= n; i++) { d = solve(ts[i]) - ts[i]; err += d < 0 ? -d : d }
  if (err != err || err > 1e6) { print "diverged" > "/dev/stderr"; exit 1 }
  printf "{\"score\": %.6f, \"error\": %.6f}\n", err + 0.05 * ITERATIONS, err
}
AWK
