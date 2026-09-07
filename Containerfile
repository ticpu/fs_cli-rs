FROM docker.io/library/debian:bookworm AS build

RUN --mount=type=cache,target=/var/cache/apt,id=apt-cache-bookworm \
    --mount=type=cache,target=/var/lib/apt/lists,id=apt-lists-bookworm \
    apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    build-essential \
    pkg-config \
    gcc-mingw-w64-x86-64

ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path

# musl targets link static-pie through rust-lld against the libc rustup ships,
# so neither arch needs a cross gcc and the base image stops deciding who can
# run the binary.
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu

ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=+crt-static -C linker=rust-lld" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=+crt-static -C linker=rust-lld" \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc

WORKDIR /app

# Cargo.lock is only committed on release commits, hence the glob
COPY Cargo.toml Cargo.lock* ./
COPY src ./src

# The Linux binaries carry no interpreter and no DT_NEEDED, which is what lets
# one build serve every suite including generic. Assert it rather than trust it:
# a dependency pulling in a C library would silently reintroduce a glibc floor.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git \
    --mount=type=cache,target=/app/target,id=cargo-target \
    set -eu; \
    for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu; do \
        cargo build --release --target "$target" --bin fs_cli; \
    done; \
    mkdir /out; \
    cp target/x86_64-unknown-linux-musl/release/fs_cli /out/fs_cli.amd64; \
    cp target/aarch64-unknown-linux-musl/release/fs_cli /out/fs_cli.arm64; \
    cp target/x86_64-pc-windows-gnu/release/fs_cli.exe /out/fs_cli.exe; \
    for arch in amd64 arm64; do \
        if readelf -d "/out/fs_cli.$arch" | grep -q NEEDED; then \
            echo "ERROR: fs_cli.$arch links a shared library; it can no longer serve every suite" >&2; \
            readelf -d "/out/fs_cli.$arch" | grep NEEDED >&2; \
            exit 1; \
        fi; \
    done

FROM scratch
COPY --from=build /out/ /
