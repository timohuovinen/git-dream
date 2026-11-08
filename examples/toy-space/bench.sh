#!/bin/sh
# Toy benchmark for git-dream: a made-up "runtime" (lower is better) of a kernel
# with four tuning knobs. It has interactions, a narrow optimum and a region that
# fails (exit 1, like running out of memory), so naive hill-climbing wastes calls.
# TOY_NOISE=0.05 adds ~5% measurement noise.
exec awk -v t="$DREAM_PARAM_THREADS" -v b="$DREAM_PARAM_BLOCK" -v u="$DREAM_PARAM_UNROLL" \
         -v c="$DREAM_PARAM_CACHE_KB" -v noise="${TOY_NOISE:-0}" -v seed="$$" 'BEGIN {
  if (b * u * t > 8192 || b * u > 2 * c * 16) { print "{\"score\": null, \"status\": \"fail\"}"; exit 1 }
  l2 = log(2)
  work = 1000 / (t < 6 ? t : 6) + 40 * (t > 6 ? t - 6 : 0)   # scales to 6 threads, then contention
  x = log(b) / l2 - log(c / 2) / l2; blocking = 30 * x * x    # best block ~ cache/2
  du = log(u) / l2 - 2; if (du < 0) du = -du
  unrolling = 25 * du + (b >= 32 ? 0 : 15 * u)
  s = work + blocking + unrolling + 3 * log(c) / l2
  if (noise > 0) { srand(seed); s *= 1 + noise * 2 * (rand() + rand() + rand() - 1.5) }
  printf "{\"score\": %.4f}\n", s
}'
