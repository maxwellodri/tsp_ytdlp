#!/usr/bin/env bash

set -e  # Exit on error

# Check if $bin is set
if [ -z "$bin" ]; then
    echo "Error: \$bin environment variable is not set"
    exit 1
fi

echo "Building tsp_ytdlp (release mode)..."
cargo build --release

BINARY_PATH="target/release/tsp_ytdlp"


"$BINARY_PATH" --kill

if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
fi

echo "Installing to: $bin/tsp_ytdlp"
cp "$BINARY_PATH" "$bin/tsp_ytdlp"
chmod +x "$bin/tsp_ytdlp"

echo "✓ Installation complete!"
