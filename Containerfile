FROM docker.io/library/debian:bullseye AS build

RUN --mount=type=cache,target=/var/cache/apt,id=apt-cache-bullseye \
    --mount=type=cache,target=/var/lib/apt/lists,id=apt-lists-bullseye \
    apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    gcc-mingw-w64-x86-64 \
    gcc-x86-64-linux-gnu \
    gcc-aarch64-linux-gnu

ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path

# Add cross-compilation targets
RUN rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu

ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc

WORKDIR /app

# Cargo.lock is only committed on release commits, hence the glob
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git \
    --mount=type=cache,target=/app/target,id=cargo-target \
    set -eu; \
    for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu; do \
        cargo build --release --target "$target" --bin fs_cli; \
    done; \
    mkdir /out; \
    cp target/x86_64-unknown-linux-gnu/release/fs_cli /out/fs_cli.amd64; \
    cp target/aarch64-unknown-linux-gnu/release/fs_cli /out/fs_cli.arm64; \
    cp target/x86_64-pc-windows-gnu/release/fs_cli.exe /out/fs_cli.exe

FROM scratch
COPY --from=build /out/ /
