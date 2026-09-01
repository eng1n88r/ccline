#!/usr/bin/env bash
# ccline installer:
#   curl -fsSL https://raw.githubusercontent.com/eng1n88r/ccline/master/install.sh | bash
#
# Downloads the prebuilt static binary for this platform from the latest GitHub
# release (falls back to building from source with cargo if none exists), removes
# any ccstatusline install, and points Claude Code's statusLine at ccline.
set -euo pipefail

REPO=eng1n88r/ccline
PREFIX=${PREFIX:-$HOME/.local}
BINDIR=$PREFIX/bin

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *) target="" ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$BINDIR"

installed=""
if [ -n "$target" ]; then
    url="https://github.com/$REPO/releases/latest/download/ccline-$target.tar.gz"
    echo "downloading $url"
    if curl -fsSL "$url" -o "$tmp/ccline.tar.gz"; then
        tar -xzf "$tmp/ccline.tar.gz" -C "$tmp"
        install -m755 "$tmp/ccline" "$BINDIR/ccline"
        installed="prebuilt"
    else
        echo "download failed; falling back to source build"
    fi
fi

if [ -z "$installed" ]; then
    for dep in git cargo; do
        command -v "$dep" >/dev/null 2>&1 || {
            echo "error: no prebuilt binary for this platform and '$dep' is missing" >&2
            exit 1
        }
    done
    git clone --quiet --depth 1 "https://github.com/$REPO" "$tmp/src"
    (cd "$tmp/src" && cargo build --release)
    install -m755 "$tmp/src/target/release/ccline" "$BINDIR/ccline"
    installed="source"
fi

# Remove any ccstatusline install (package, config, cache, stale mise shims).
if command -v npm >/dev/null 2>&1 && npm ls -g ccstatusline >/dev/null 2>&1; then
    echo "removing ccstatusline npm package"
    npm uninstall -g ccstatusline >/dev/null
fi
rm -rf "$HOME/.config/ccstatusline" "$HOME/.cache/ccstatusline"
if command -v mise >/dev/null 2>&1; then mise reshim; fi

# Point Claude Code's statusLine at ccline, preserving all other settings.
if command -v python3 >/dev/null 2>&1; then
    python3 - "$BINDIR/ccline" <<'EOF'
import json, os, sys
cmd = sys.argv[1]
p = os.path.expanduser("~/.claude/settings.json")
os.makedirs(os.path.dirname(p), exist_ok=True)
d = json.load(open(p)) if os.path.exists(p) else {}
sl = d.get("statusLine") or {}
if sl.get("type") == "command" and sl.get("command") == cmd:
    print("statusLine already configured")
else:
    sl.update({"type": "command", "command": cmd})
    sl.setdefault("padding", 0)
    sl.setdefault("refreshInterval", 10)
    d["statusLine"] = sl
    with open(p, "w") as f:
        json.dump(d, f, indent=2)
        f.write("\n")
    print("statusLine configured in " + p)
EOF
else
    echo "python3 not found — add this to ~/.claude/settings.json yourself:"
    printf '  "statusLine": {"type": "command", "command": "%s", "padding": 0, "refreshInterval": 10}\n' "$BINDIR/ccline"
fi

echo "ccline installed to $BINDIR/ccline ($installed)"
