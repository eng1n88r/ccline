PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
CLAUDE_SETTINGS := $(HOME)/.claude/settings.json

.PHONY: build install configure purge-ccstatusline uninstall check clean

build:
	cargo build --release

# Idempotent: removes any ccstatusline install, (re)installs the binary, and
# points Claude Code's statusLine at it. Safe to run repeatedly.
install: build purge-ccstatusline
	install -Dm755 target/release/ccline $(BINDIR)/ccline
	@$(MAKE) --no-print-directory configure
	@echo "ccline installed to $(BINDIR)/ccline"

# Remove any ccstatusline install (npm global, bun global, bunx cache) plus
# its config/cache.
purge-ccstatusline:
	@if command -v npm >/dev/null 2>&1 && npm ls -g ccstatusline >/dev/null 2>&1; then \
		echo "removing ccstatusline npm package"; \
		npm uninstall -g ccstatusline >/dev/null; \
	fi
	@if command -v bun >/dev/null 2>&1; then \
		if bun pm ls -g 2>/dev/null | grep -q ccstatusline; then \
			echo "removing ccstatusline bun global"; \
			bun remove -g ccstatusline >/dev/null 2>&1; \
		fi; \
		rm -rf $(HOME)/.bun/install/cache/ccstatusline*; \
	fi
	@rm -rf "$${TMPDIR:-/tmp}"/bunx-*-ccstatusline@*
	@rm -rf $(HOME)/.config/ccstatusline $(HOME)/.cache/ccstatusline
	@if command -v mise >/dev/null 2>&1; then mise reshim; fi

# Point statusLine.command at the installed binary, preserving all other
# settings. Creates settings.json if missing; no-op when already configured.
configure:
	@mkdir -p $(HOME)/.claude
	@python3 -c 'import json, os, sys; \
p = os.path.expanduser("$(CLAUDE_SETTINGS)"); \
d = json.load(open(p)) if os.path.exists(p) else {}; \
sl = d.get("statusLine") or {}; \
cmd = "$(BINDIR)/ccline"; \
done = sl.get("type") == "command" and sl.get("command") == cmd; \
sl.update({"type": "command", "command": cmd}); \
sl.setdefault("padding", 0); sl.setdefault("refreshInterval", 10); \
d["statusLine"] = sl; \
done or (json.dump(d, open(p, "w"), indent=2), open(p, "a").write("\n")); \
print("statusLine already configured" if done else "statusLine configured in " + p)'

uninstall:
	rm -f $(BINDIR)/ccline

check:
	cargo fmt --check
	cargo clippy --release -- -D warnings

clean:
	cargo clean
