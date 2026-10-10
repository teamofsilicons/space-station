#!/bin/sh
# macOS: six Rust targets, Zig/cargo-zigbuild, LLVM/lld and cargo-xwin.
set -eu
cd "$(dirname "$0")/.."
cargo build -p space-station-cli --release --locked --target aarch64-apple-darwin --target x86_64-apple-darwin
cargo zigbuild -p space-station-cli --release --locked --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-musl
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo xwin build --cross-compiler clang -p space-station-cli --release --locked --target aarch64-pc-windows-msvc --target x86_64-pc-windows-msvc
python3 scripts/package-cli-release.py
