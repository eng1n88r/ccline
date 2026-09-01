# ccline

A fast, single-binary statusline for [Claude Code](https://code.claude.com), written in
Rust as a replacement for the npm `ccstatusline` (~535 ms per render → ~5–20 ms, no Node).

```
Fable 5 │ v2.1.252 │ src-09 │ 96.3k 10% │ 5h 63% │ wk 22% │ Fable 26% │  main │ +5 -2
```

Widgets, left to right: model name, Claude Code version, session name, context usage
(tokens + % of window), 5-hour rate limit, weekly rate limit, Fable weekly limit,
git branch, and uncommitted diff stats. Widgets whose data is unavailable are simply
omitted.

Colors are 16-color ANSI only, so the line always follows the active terminal theme
(e.g. omarchy theme switches restyle it with no config).

## Install

One-liner — downloads the prebuilt static binary for your platform (Linux
x86_64/aarch64, macOS arm64/x86_64) from the latest release, falling back to a
cargo source build:

```sh
curl -fsSL https://raw.githubusercontent.com/eng1n88r/ccline/master/install.sh | bash
```

Or from a checkout (needs cargo):

```sh
make install
```

Either way the install is idempotent and does three things:

1. Removes any existing ccstatusline install (npm global package, `~/.config/ccstatusline`,
   `~/.cache/ccstatusline`, stale mise shims).
2. Builds and installs the binary to `~/.local/bin/ccline` (override with `PREFIX=`).
3. Points `statusLine.command` in `~/.claude/settings.json` at it, preserving all other
   settings (new sessions pick it up immediately; running ones after a restart).

`make uninstall` removes the binary; `make check` runs fmt + clippy.

## Design

The render path never touches the network and never spawns anything slower than git.
Slow data sources are refreshed by detached background re-invocations of the binary,
so a render always returns immediately with last-known values:

- `ccline --refresh-usage` — fetches `https://api.anthropic.com/api/oauth/usage`
  with the OAuth token from `~/.claude/.credentials.json` (passed to curl via a
  0600 config file, never argv). Cached at `~/.cache/ccline/usage.json`, 60 s TTL,
  with a lock file to prevent refresh stampedes. This is the only source for the
  Fable weekly-scoped limit; the 5 h/weekly numbers prefer the `rate_limits` object
  Claude Code pipes in on stdin and use the cache as fallback.
- `ccline --refresh-session-name <sid>` — resolves the session's auto-assigned name
  via `claude agents --json`, cached at `$TMPDIR/cc-session-name-<sid>.txt`, 60 s TTL.

Context tokens come from the stdin payload's `context_window.current_usage`, falling
back to scanning the tail of the transcript JSONL for the last usage entry.

### Constraints worth knowing

Claude Code post-processes statusline output with
`stdout.trim().split("\n").flatMap(l => l.trim() || [])`: every line is trimmed and
whitespace-only lines are dropped. Leading-space alignment therefore cannot work, and
the ~2-character indent of the footer area is Claude Code chrome that no statusline
command can change.

There is deliberately no config file — widget set, order, and colors live in
`src/main.rs` (`render()` is the place to reorder or drop widgets).
