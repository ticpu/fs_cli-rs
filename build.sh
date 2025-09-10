#!/bin/bash

# Detect container runtime (podman or docker)
if command -v podman &> /dev/null; then
    CONTAINER_CMD="podman"
elif command -v docker &> /dev/null; then
    CONTAINER_CMD="docker"
else
    echo "Error: Neither podman nor docker found"
    exit 1
fi

echo "Using $CONTAINER_CMD as container runtime"

# Default target
ARG=${1:-x86_64}

# Map short names to full targets
case $ARG in
    x86_64)
        TARGET="x86_64-unknown-linux-gnu"
        BINARY_NAME="fs_cli.x86_64"
        SRC_BINARY="fs_cli"
        ;;
    aarch64)
        TARGET="aarch64-unknown-linux-gnu"
        BINARY_NAME="fs_cli.aarch64"
        SRC_BINARY="fs_cli"
        ;;
    windows)
        TARGET="x86_64-pc-windows-gnu"
        BINARY_NAME="fs_cli.exe"
        SRC_BINARY="fs_cli.exe"
        ;;
    # Support full target names too
    x86_64-unknown-linux-gnu)
        TARGET="x86_64-unknown-linux-gnu"
        BINARY_NAME="fs_cli.x86_64"
        SRC_BINARY="fs_cli"
        ;;
    aarch64-unknown-linux-gnu)
        TARGET="aarch64-unknown-linux-gnu"
        BINARY_NAME="fs_cli.aarch64"
        SRC_BINARY="fs_cli"
        ;;
    x86_64-pc-windows-gnu)
        TARGET="x86_64-pc-windows-gnu"
        BINARY_NAME="fs_cli.exe"
        SRC_BINARY="fs_cli.exe"
        ;;
    *)
        echo "Error: Unsupported target $ARG"
        echo "Usage: $0 [x86_64|aarch64|windows]"
        exit 1
        ;;
esac

echo "Building for target: $TARGET"

# Build the container using git dependencies
cd /mnt/bcachefs/home/jerome/GIT/freeswitch/fs_cli-rs
$CONTAINER_CMD build -f Containerfile -t fs_cli-build --build-arg TARGET=$TARGET . -q

# Create temporary container and copy binary
CONTAINER=$($CONTAINER_CMD create fs_cli-build)
$CONTAINER_CMD cp $CONTAINER:/app/target/release/$SRC_BINARY $BINARY_NAME

# Clean up
$CONTAINER_CMD rm $CONTAINER > /dev/null 2>&1
$CONTAINER_CMD rmi fs_cli-build -f > /dev/null 2>&1

echo "Binary extracted to $BINARY_NAME"