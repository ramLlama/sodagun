.PHONY: deps fmt fmt-check lint typecheck test audit build-debug build-release-thin build-release install uninstall clean all

_default: all

# Requires cargo-audit and cargo-deny: cargo install cargo-audit cargo-deny
deps:
	cargo fetch
	pre-commit install --hook-type pre-commit --hook-type pre-push

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

typecheck:
	cargo check --all-targets

test:
	cargo test

audit:
	cargo deny check
	# The --ignore flags below cover advisories in microsandbox's transitive dep tree
	# where no upgrade is available on our end.
	cargo audit \
		--ignore RUSTSEC-2025-0134 \
		--ignore RUSTSEC-2025-0141 \
		--ignore RUSTSEC-2026-0118 \
		--ignore RUSTSEC-2026-0119 \
		--ignore RUSTSEC-2023-0071

build-debug:
	cargo build

build-release-thin:
	cargo build --profile release-thin

build-release:
	cargo build --release

install:
	cargo install --path .

uninstall:
	cargo uninstall sodagun

clean:
	cargo clean

check-all: fmt lint typecheck test audit

all: check-all build-release-thin
