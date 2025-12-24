#!/usr/bin/env bash
binary_name=tsp_ytdlp

set -e  # Exit on error

# Check if $bin is set
if [ -z "$bin" ]; then
    echo "Error: \$bin environment variable is not set"
    exit 1
fi

echo "Building $binary_name (release mode)..."
cargo build --release

BINARY_PATH="target/release/$binary_name"


"$BINARY_PATH" --kill

if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
fi

echo "Installing to: $bin/$binary_name"
cp "$BINARY_PATH" "$bin/$binary_name"
chmod +x "$bin/$binary_name"

echo "✓ Installation complete!"
