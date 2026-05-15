.PHONY: deps fmt lint typecheck test audit build install uninstall clean all

# Requires cargo-audit and cargo-deny: cargo install cargo-audit cargo-deny
deps:
	cargo fetch
	pre-commit install --hook-type pre-commit --hook-type pre-push

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets --all-features -- -D warnings

typecheck:
	cargo check --all-targets

test:
	cargo test

audit:
	cargo deny check
	cargo audit

build:
	cargo build --release

install:
	cargo install --path .

uninstall:
	cargo uninstall sodagun

clean:
	cargo clean

all: fmt lint typecheck test audit
