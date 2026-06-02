FROM rust:1.95-alpine3.23

# Additional system dependencies needed for git2 (vendored libgit2) and sodagun
RUN apk add --no-cache \
    cmake pkgconf \
    curl ca-certificates \
    git make bash \
    openssl-dev openssl-libs-static

# Pre-fetch all crate dependencies using the pinned lockfile.
# The official rust image installs rustfmt/clippy/rust-analyzer binaries but
# omits them from rustup's components tracking file, breaking `cargo fmt` etc.
# via the rustup proxy. Append the entries so rustup recognises them.
COPY rust-toolchain.toml Cargo.toml Cargo.lock /tmp/rust-setup/
RUN cd /tmp/rust-setup && \
    target=$(rustc -vV | awk '/^host:/{print $2}') && \
    toolchain=$(rustup show active-toolchain | awk '{print $1}') && \
    printf "clippy-%s\nrust-analyzer-%s\nrustfmt-%s\n" "$target" "$target" "$target" \
        >> "/usr/local/rustup/toolchains/$toolchain/lib/rustlib/components" && \
    curl -L --proto '=https' --tlsv1.2 -sSf \
        https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash && \
    cargo binstall --no-confirm cargo-deny cargo-audit && \
    cargo fetch --locked && \
    rm -rf /tmp/rust-setup

# Claude Code
RUN curl -fsSL https://claude.ai/install.sh | bash

# microsandbox replaces /etc/profile and ignores image ENV vars, so PATH and
# build settings must be set via /root/.profile (which login shells do source).
RUN printf 'export PATH="/usr/local/cargo/bin:$PATH"\nexport CARGO_TARGET_DIR=/tmp/target\nulimit -n 65536\n' \
    >> /root/.profile
