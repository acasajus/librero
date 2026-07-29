#!/usr/bin/env bash
set -e

echo "=== Librero Live Reload Watcher ==="

if ! command -v cargo-watch &> /dev/null && ! cargo watch --version &> /dev/null; then
    echo "📦 cargo-watch not detected. Installing cargo-watch..."
    cargo install cargo-watch
fi

echo "🚀 Starting cargo watch for Librero daemon (recompiles & restarts cargo run on file changes)..."
cargo watch -x "run --mode embedded"
