#!/bin/sh
# Download the papers listed in papers/README.md into papers/ (gitignored).
# Files that are already present are skipped. Pass -f to download them again.
set -eu
cd "$(dirname "$0")"

force=0
[ "${1:-}" = "-f" ] && force=1

fetch() { # name url sha256
    if [ -f "$1" ] && [ "$force" = 0 ]; then
        echo "have  $1"
    else
        echo "fetch $1"
        curl -fsSL -o "$1.part" "$2" && mv "$1.part" "$1"
    fi
    got=$(sha256sum "$1" | cut -d' ' -f1)
    [ "$got" = "$3" ] || echo "warning: $1 sha256 is $got, not the $3 listed in README.md (new version?)" >&2
}

fetch dream-rsi.pdf https://www.dream-rsi.com/assets/dream-rsi.pdf b2403013a14c588d772fc6b66a2ae556e5e8e2bcc58402dd4b582d9ba95a793f
