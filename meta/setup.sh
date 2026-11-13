#!/bin/sh
# One-time setup on the machine that runs opencode. Run from the repo root of a
# DEDICATED clone (not your working checkout):
#   sh meta/setup.sh                            # pinned binary built from this commit (default)
#   GIT_DREAM_VERSION=v0.2.2 sh meta/setup.sh   # pinned binary = that GitHub release instead
# Releases <= v0.2.1 predate `[evaluate] protected` and cannot read meta/*.toml.
# 1. installs a pinned git-dream binary outside PATH, so attempts that break
#    git-dream can't break the harness running them;
# 2. records the speed-track reference output at this commit.
set -eu
repo=timohuovinen/git-dream
pin=${GIT_DREAM_PIN:-$HOME/.local/git-dream-pinned}
mkdir -p "$pin/bin"

if [ -z "${GIT_DREAM_VERSION:-}" ]; then
  cargo install --locked --path . --root "$pin" --force
else
  case $(uname -m) in
    x86_64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;;
    *) echo "no release for $(uname -m); unset GIT_DREAM_VERSION to build from source" >&2; exit 1 ;;
  esac
  tag=$GIT_DREAM_VERSION
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  gh release download "$tag" -R "$repo" -D "$tmp" -p "*-$arch-unknown-linux-musl.tar.gz" -p SHA256SUMS
  (cd "$tmp" && grep -- "-$arch-unknown-linux-musl.tar.gz" SHA256SUMS | sha256sum -c -)
  tar xzf "$tmp"/*.tar.gz -C "$tmp"
  install "$tmp"/git-dream-*/git-dream "$pin/bin/git-dream"
fi
"$pin/bin/git-dream" --version || true

cargo build --release --locked -q --examples
mkdir -p meta/reference
./target/release/examples/compare --seeds "${META_SEEDS:-30}" --first-seed "${META_FIRST_SEED:-50000}" \
  > meta/reference/compare.jsonl
python3 meta/score.py meta/reference/compare.jsonl
echo
echo "pinned binary: $pin/bin/git-dream"
echo "now: git add meta && git commit -m 'meta: reference output' && $pin/bin/git-dream run -n 3 --config meta/policy.toml"
