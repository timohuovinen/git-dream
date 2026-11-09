# Papers

These are the papers git-dream is based on. The PDFs are **not** in this repository, because their licenses don't allow redistribution. Each paper's entry below states its license.

Download local copies once with:

```bash
papers/fetch.sh
```

This saves the PDFs in this folder. They are gitignored, so they stay on your machine. If a downloaded file's checksum differs from the one listed below, the script warns you: a new version of the paper may have been published.

## Dream-RSI: Recursive Self-Improvement through Evolving Worlds

Tong Zheng, Xidong Wu, Zheng Zhang, Zhankui He, Chaoyi Zhang, Benjamin Coleman, Ruoqiao Wei, Di Bai, Haolin Liu, Rui Liu, Xue Wang, Yue Zhuan, Wang-Cheng Kang, Renkai Xiang, Heng Huang, Xinwu Cheng, Yunsong Guo (Google, Google DeepMind, University of Maryland, University of Virginia), 2026.

- Paper: <https://www.dream-rsi.com/assets/dream-rsi.pdf> (36 pages; version dated 2026-09-12)
- Project page: <https://www.dream-rsi.com/>
- Code: <https://github.com/zhengkid/Dream-RSI>
- License: "© 2026 Google. All rights reserved." Do not commit the PDF.
- Local file: `papers/dream-rsi.pdf`
- sha256: `b2403013a14c588d772fc6b66a2ae556e5e8e2bcc58402dd4b582d9ba95a793f`

Sections that matter most for git-dream:

| Section | Topic |
|---|---|
| §3 | discovery trees, `CONTINUE(v)`, replay as a simulator |
| §5.1, Fig. 5 | injecting summarised "directional insights" into the prompt does worse than giving no guidance |
| App. B.1 | exploration prompt: the agent reads *every* sibling and historical `proposal.md`, score and error before it proposes anything |
| App. B.2 | policy-development prompt and the replay API, which only sees the part of the tree revealed so far |
