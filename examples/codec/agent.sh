#!/bin/sh
# One attempt by a headless coding agent in git-dream's scratch worktree.
#   CODEC_AGENT      claude (default) or codex
#   CODEC_MODEL      default claude-haiku-5-5 (claude), gpt-6-luna (codex)
#   CODEC_USAGE_DIR  optional: receives each call's usage record (its last line is one JSON object)
start=$(date +%s)
case ${CODEC_AGENT:-claude} in
  claude)
    out=$(env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT claude -p "$DREAM_PROMPT" \
      --model "${CODEC_MODEL:-claude-haiku-5-5}" --permission-mode acceptEdits \
      --tools "Read,Edit,Write,Bash,Grep,Glob" \
      --allowedTools "Bash(sh eval.sh:*)" "Bash(rustc:*)" "Bash(./build/:*)" "Bash(build/:*)" "Bash(git show:*)" "Bash(git log:*)" "Bash(git diff:*)" "Bash(ls:*)" "Bash(wc:*)" "Bash(mkdir:*)" \
      --strict-mcp-config --setting-sources project --no-session-persistence \
      --max-budget-usd "${CODEC_MAX_USD:-1}" --output-format json </dev/null)
    status=$? ;;
  codex)
    # the sandbox lets it write in the worktree and nowhere else
    model=${CODEC_MODEL:-gpt-6-luna}
    out=$(codex exec -m "$model" -s workspace-write --ephemeral --json "$DREAM_PROMPT" </dev/null 2>/dev/null)
    status=$?
    usage=$(printf '%s\n' "$out" | sed -n 's/.*"type":"turn.completed".*"usage":\({[^}]*}\).*/\1/p' | tail -n 1)
    out="$out
{\"agent\":\"codex\",\"model\":\"$model\",\"exit\":$status,\"duration_ms\":$(( ($(date +%s) - start) * 1000 )),\"usage\":${usage:-null}}" ;;
  *) echo "CODEC_AGENT must be claude or codex" >&2; exit 2 ;;
esac
if [ -n "${CODEC_USAGE_DIR:-}" ]; then
  printf '%s\n' "$out" > "$CODEC_USAGE_DIR/$DREAM_RUN-r$DREAM_ROUND-l$DREAM_LANE-$$.json"
fi
printf '%s\n' "$out" | tail -c 1500
exit $status
