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

# Build the container using git dependencies
cd /mnt/bcachefs/home/jerome/GIT/freeswitch/fs_cli-rs
$CONTAINER_CMD build -f Containerfile -t fs_cli-build .

# Create temporary container and copy binary
CONTAINER=$($CONTAINER_CMD create fs_cli-build)
$CONTAINER_CMD cp $CONTAINER:/app/target/release/fs_cli fs_cli

# Clean up
$CONTAINER_CMD rm $CONTAINER
$CONTAINER_CMD rmi fs_cli-build

echo "Binary extracted to fs_cli"