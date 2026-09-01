PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin

.PHONY: build install uninstall check clean

build:
	cargo build --release

install: build
	install -Dm755 target/release/ccline $(BINDIR)/ccline

uninstall:
	rm -f $(BINDIR)/ccline

check:
	cargo fmt --check
	cargo clippy --release -- -D warnings

clean:
	cargo clean
