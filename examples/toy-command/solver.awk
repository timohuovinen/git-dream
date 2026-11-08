# The "code" being improved. The toy agent (agent.sh) edits these constants;
# an LLM agent would edit real code instead.
BEGIN {
STEP = 1.0
DAMPING = 0.1
ITERATIONS = 10
}

function solve(target,    x, v, i) {
  x = 0; v = 0
  for (i = 0; i < ITERATIONS; i++) { v = (1 - DAMPING) * v + STEP * (target - x) * 0.1; x += v }
  return x
}
