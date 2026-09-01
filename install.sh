#!/usr/bin/env bash
# ccline installer: builds from source in a temp dir, no manual clone needed.
#   curl -fsSL https://raw.githubusercontent.com/eng1n88r/ccline/master/install.sh | bash
# Requires: git, cargo (rustup), python3. Also removes any ccstatusline install
# and points Claude Code's statusLine at ccline (see Makefile).
set -euo pipefail

for dep in git cargo python3; do
    command -v "$dep" >/dev/null 2>&1 || {
        echo "error: '$dep' is required (install it and re-run)" >&2
        exit 1
    }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "cloning ccline..."
git clone --quiet --depth 1 https://github.com/eng1n88r/ccline "$tmp/ccline"
make -C "$tmp/ccline" install
