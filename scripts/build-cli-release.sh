#!/bin/sh
# Run on macOS with both Rust Darwin targets, Linux musl targets, Zig and cargo-zigbuild.
set -eu
cd "$(dirname "$0")/.."
cargo build -p space-station-cli --release --locked --target aarch64-apple-darwin --target x86_64-apple-darwin
cargo zigbuild -p space-station-cli --release --locked --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-musl
python3 scripts/package-cli-release.py
