.PHONY: deps fmt fmt-check lint typecheck test test-unit test-integration audit build-debug build-release-thin build-release install uninstall clean all

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

test: test-unit test-integration

# In-source #[cfg(test)] tests: pure, need neither msb nor git.
test-unit:
	cargo test --bin sodagun

# Spawns the sodagun binary; needs git for repo fixtures. VM-boot tests
# additionally need hardware virtualization and skip themselves without it
# (the require-virt pre-push hook blocks pushing from such hosts).
test-integration:
	cargo test --test integration

audit:
	cargo deny check
	# The --ignore flags below cover advisories in microsandbox's transitive dep tree
	# where no upgrade is available on our end.
	cargo audit \
		--ignore RUSTSEC-2025-0134 \
		--ignore RUSTSEC-2025-0141 \
		--ignore RUSTSEC-2026-0118 \
		--ignore RUSTSEC-2026-0119 \
		--ignore RUSTSEC-2023-0071 \
		--ignore RUSTSEC-2026-0173

build-debug:
	cargo build

build-release-thin:
	cargo build --profile release-thin

build-release:
	cargo build --release

install: build-release
	cargo install --path . --profile release --locked

uninstall:
	cargo uninstall sodagun

clean:
	cargo clean

check-all: fmt lint typecheck test audit

all: check-all build-release-thin
