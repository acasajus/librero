# Justfile for Librero Service Daemon

# Default recipe: check compilation
default: check

# Run the Librero daemon directly with embedded Arti Tor
run:
    cargo run -- --mode embedded

# Live reload: watch source files and automatically restart on change
watch:
    cargo watch -x "run --mode embedded"

# Check project compilation
check:
    cargo check

# Run unit tests
test:
    cargo test

# Build debug binary
build:
    cargo build
