FROM debian:buster

RUN sed -i 's|http://deb.debian.org/debian|http://archive.debian.org/debian|g' /etc/apt/sources.list && \
    sed -i '/security.debian.org/d' /etc/apt/sources.list && \
    echo "Acquire::Check-Valid-Until false;" > /etc/apt/apt.conf.d/99no-check-valid-until

RUN --mount=type=cache,target=/var/cache/apt,id=apt-cache-buster \
    --mount=type=cache,target=/var/lib/apt/lists,id=apt-lists-buster \
    apt-get update && apt-get install -y \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    gcc-aarch64-linux-gnu \
    gcc-mingw-w64-x86-64

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Add cross-compilation targets
RUN rustup target add aarch64-unknown-linux-gnu x86_64-pc-windows-gnu

WORKDIR /app

# Copy dependency files first for better caching
COPY Cargo.toml ./

# Download dependencies (cached layer unless Cargo.toml changes)
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git \
    cargo fetch

# Copy source files
COPY src ./src
COPY fs_cli.yaml ./

# Build the actual binary
ARG TARGET=x86_64-unknown-linux-gnu
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git \
    if [ "$TARGET" = "aarch64-unknown-linux-gnu" ]; then \
        export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc && \
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc && \
        cargo build --release --target aarch64-unknown-linux-gnu --bin fs_cli && \
        cp target/aarch64-unknown-linux-gnu/release/fs_cli target/release/; \
    elif [ "$TARGET" = "x86_64-pc-windows-gnu" ]; then \
        export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc && \
        export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc && \
        cargo build --release --target x86_64-pc-windows-gnu --bin fs_cli && \
        cp target/x86_64-pc-windows-gnu/release/fs_cli.exe target/release/; \
    else \
        cargo build --release --bin fs_cli; \
    fi