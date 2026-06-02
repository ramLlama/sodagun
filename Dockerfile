FROM alpine:3.23

# Runtime profile: ulimit for file descriptors and redirect cargo builds to tmpfs
RUN printf '#!/bin/sh\nulimit -n 65536\nexport CARGO_TARGET_DIR=/tmp/target\n' \
    > /etc/profile.d/sodagun-setup.sh

# System dependencies
RUN apk add --no-cache \
    build-base cmake pkgconf \
    git curl ca-certificates make bash \
    openssl-dev openssl-libs-static

# Rust toolchain — no default toolchain; version pinned by rust-toolchain.toml
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain none --no-modify-path && \
    /root/.cargo/bin/rustup set auto-self-update disable && \
    echo '. /root/.cargo/env' >> /root/.profile

# Pre-fetch all crate dependencies using the pinned lockfile
COPY rust-toolchain.toml Cargo.toml Cargo.lock /tmp/rust-setup/
RUN cd /tmp/rust-setup && \
    /root/.cargo/bin/rustup toolchain install && \
    /root/.cargo/bin/cargo fetch --locked && \
    /root/.cargo/bin/cargo install cargo-deny cargo-audit && \
    rm -rf /tmp/rust-setup

# Claude Code
RUN curl -fsSL https://claude.ai/install.sh | bash
