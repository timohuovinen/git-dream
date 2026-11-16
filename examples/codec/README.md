# codec: a compressor in one Rust file, improved by a coding agent

The task for git-dream's runs with a real agent. `codec.rs` starts as a copy (`compress` returns its input) and
an LLM coding agent is asked to make it compress better.

- `codec.rs`: `pub fn compress(&[u8]) -> Vec<u8>` and `pub fn decompress(&[u8]) -> Vec<u8>`, std only, one file.
- `eval.sh`: the score is the size of `corpus.txt` after `compress` plus the size of `codec.rs`, in bytes. Lower
  is better. Counting the source keeps the corpus out of the program.
- `corpus.txt`: 121,546 bytes of this repository's documentation and source, frozen.
- `harness.rs`: runs one of the two functions on stdin.
- `agent.sh`: one attempt by Claude Code, headless (`CODEC_MODEL`, default `claude-haiku-5-5`).

Why this task:

- **The score does not depend on load.** It is a byte count. `compress` and `decompress` run in separate
  processes under a CPU-time limit (20 s each), not a wall-clock one. So attempts and whole runs can overlap
  (`evaluations = "parallel"`) without changing any score.
- **It cannot be gamed cheaply.** `eval.sh` rejects file, environment, process and network access, `unsafe` and
  `include` macros, and checks the round trip on the corpus and on three small inputs.
- **There is a long way down.** For scale: `gzip -9` gives 41,091 bytes on the corpus, `bzip2 -9` 35,781, `xz -9e` 37,240.

```sh
sh eval.sh        # {"score": 121673, "status": "ok", "compressed": 121546, "code": 127, "corpus": 121546}
```

To run it yourself, copy this folder without `evidence/` into a new repository and write a `.git-dream.toml`;
[evidence/run.sh](evidence/run.sh) does both for one of four strategies. It needs `rustc`, `claude` and
`git-dream` on the `PATH`.

## The comparison

[evidence/](evidence/) holds a comparison of four exploration strategies on this task and how to repeat it.
