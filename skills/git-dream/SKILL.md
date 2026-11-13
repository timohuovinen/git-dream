---
name: git-dream
description: Use when the user says "dream up ..." or "dream on ...", or asks to let an agent iterate many attempts on a measurable goal (faster, smaller, higher score, fewer failures) and keep the best. Sets up and launches a git-dream run in the current repo, using the same coding agent you are (opencode, Claude Code, Codex, Gemini) as the attempt-making agent, then reports progress and the best result. Also covers reading git-dream results and the agent role inside a run.
---

# git-dream: "dream up X"

git-dream runs a long experimental search in a git repo:

- a **coding agent** makes one attempt at a time, each in a scratch worktree;
- a fixed **evaluator** scores each attempt;
- an **exploration policy** picks which attempt to build on next.

Every attempt is a commit under `refs/dream/<run>/...`. The user's branch and files are never touched.

The helper `scripts/dream`, in this skill's directory, handles setup and detached launching. Below,
`dream` means `<this skill's dir>/scripts/dream`. Use it via the PATH symlink if `command -v dream`
finds it.

## When the user says "dream up X"

Work in the repo you are in. Follow these steps in order; only step 3 needs the user.

1. **Understand X and the repo.** Read enough code, tests and existing benchmarks to know what
   "better" means for X and how it can be measured.

2. **Design the evaluator.** This is the most important step, because the search optimizes exactly
   what it measures. It must:
   - be **one command** run from the repo root, whose **last stdout line** is the score: a number,
     or `{"score": N, "status": "ok", ...extra metrics}`;
   - **fail** (non-zero exit) when the attempt is broken, so run the correctness checks first,
     e.g. `cargo test -q >/dev/null 2>&1 && ./bench.sh` or `npm test --silent && node bench.js`;
   - be **stable**. For timings, use the median of several runs and set
     `remeasure_root = true` and `confirm_top = 2`;
   - be **cheap relative to an attempt**: seconds to a few minutes;
   - be hard to game. List the tests, benchmark and fixtures in `protected`.

   Prefer a benchmark or test that already exists. If you have to write one, write it **outside the
   tree**, at `<run dir>/eval.sh`, and call it by absolute path. The run starts from HEAD, so a new
   in-tree script would only count if committed, and don't commit on the user's behalf.

3. **Create the run and show it to the user before starting.**
   ```sh
   dream new <run-name>     # writes .git/dream-runs/<run>/config.toml, wired to YOUR harness
   ```
   Fill in every TODO: the `prompt` (the goal, what must keep working, "one focused change per
   attempt"), `[evaluate] command`, `goal`, `protected` and timeouts. Then run:
   ```sh
   dream check <run-name>   # runs the evaluator once on HEAD in a throwaway worktree
   ```
   Show the user the goal, the evaluator, the baseline score, the protected paths, the rounds ×
   budget, and a rough time estimate. Get an explicit OK. If the work tree has uncommitted changes,
   say that the run starts from HEAD and ignores them.

4. **Launch the run detached**, then stop working:
   ```sh
   dream start <run-name> -n <rounds>    # default 3; it keeps running after this session ends
   ```
   Tell the user the run name, and that they can ask "how is the dream going" or run
   `dream status <run>`. Don't poll in a loop: attempts take minutes to hours, and your own model
   may be the same one the run uses.

## When asked how it's going, or for results

```sh
dream status [<run>]          # running/finished, log tail, rounds, scores, best attempt
dream log <run> [lines]
git dream show --run <run> --config .git/dream-runs/<run>/config.toml   # a round's tree
git show dream/<run>/best      # the best attempt's commit; its message is the agent's write-up
git diff HEAD dream/<run>/best
```

To summarize, give the baseline versus the best score, the ideas that worked and those that failed,
and the best diff. To adopt it, offer `git cherry-pick dream/<run>/best`, or a new branch from it,
for the user to review. Never merge into their branch on your own.

Be honest about the result:
- the best of many noisy measurements is optimistically biased, so re-run the evaluator on
  `dream/<run>/best` before claiming a gain;
- "no improvement" is a valid result.

`dream stop <run>` kills a run. `dream start <run> -n N` continues it later: an interrupted round is
closed by promoting its best recorded attempt.
Deleting a run (`git dream clean --run NAME --yes`) is irreversible, so ask first.

## Harness wiring (done by `dream new`)

`dream harness` detects which agent you are, and `dream new` writes the matching headless command.
Override it with `--harness`.

| harness | attempt command |
|---|---|
| opencode | `opencode run --auto --archived --title "dream $DREAM_RUN r$DREAM_ROUND" "$DREAM_PROMPT"` |
| claude | `claude -p "$DREAM_PROMPT" --permission-mode acceptEdits --allowedTools Bash` |
| codex | `codex exec --full-auto "$DREAM_PROMPT"` |
| gemini | `gemini --yolo -p "$DREAM_PROMPT"` |

Attempt agents can run arbitrary commands inside their scratch worktree. That's needed for building
and testing, but it is not a sandbox. Slow local models need long `[agent] timeout`s; opencode gets
6 h by default.

## If you ARE the agent inside a run

You can tell because `$DREAM_PROMPT`, `$DREAM_CONTEXT` and `$DREAM_MESSAGE_FILE` are set.

1. Read `$DREAM_CONTEXT`, which is every earlier attempt with its write-up, score and failure output.
   Don't repeat a measured idea unless you can show its implementation, rather than the idea, was at
   fault (`git show <sha>`).
2. Make one focused change. Don't touch protected paths. Don't commit, because git-dream commits for you.
3. Write `$DREAM_MESSAGE_FILE`: a one-line summary, a blank line, then the idea, the evidence from the
   history, why it isn't a repeat, and any numbers you measured.

## Install and reference

- The binary must be on PATH as `git-dream`, at v0.2.2 or later (older versions reject
  `protected` and can't resume interrupted runs). Install or update it from the latest release:
  ```sh
  t=$(mktemp -d) && gh release download -R timohuovinen/git-dream -D "$t" -p '*x86_64-unknown-linux-musl.tar.gz' -p SHA256SUMS \
    && (cd "$t" && grep x86_64-unknown-linux-musl SHA256SUMS | sha256sum -c -) && tar xzf "$t"/*.tar.gz -C "$t" \
    && install "$t"/git-dream-*/git-dream ~/.local/bin/ && git dream --version
  ```
  (Use `aarch64` on ARM. Without `gh`: `cargo install --git https://github.com/timohuovinen/git-dream`.)
- Full config reference: the git-dream README (`kind = "space"` for pure parameter search,
  `[dream]` and `[policies]` for the exploration policy).
- To improve git-dream itself, use `meta/README.md` in the git-dream repo.
