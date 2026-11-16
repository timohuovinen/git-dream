#!/bin/sh
# Replay adapter for candidates authored in the implementation session.
# This is deliberately not presented as an autonomous LLM invocation.
set -eu
if grep -q HashMap count.rs; then
  message="No further candidate supplied in this session"
elif [ "$DREAM_K" = 0 ]; then
  cp candidates/control.rs count.rs
  message="Negative control: overwrite duplicate keys (must fail correctness)"
else
  cp candidates/optimized.rs count.rs
  message="Aggregate borrowed keys in a hash map before sorting

The baseline scans all distinct keys per input row. Replace the linear lookup with HashMap::entry, then allocate the sorted result once per key. Keep parsing and duplicate aggregation semantics unchanged."
fi
printf '%s\n' "$message" > "$DREAM_MESSAGE_FILE"
