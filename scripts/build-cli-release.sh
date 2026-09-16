#!/bin/sh
# macOS: six Rust targets, Zig/cargo-zigbuild, LLVM/lld and cargo-xwin.
set -eu
cd "$(dirname "$0")/.."
target_dir="${CARGO_TARGET_DIR:-$PWD/target}"
for feature in honeycomb-managed standalone; do
    set --
    if [ "$feature" = honeycomb-managed ]; then set -- --features honeycomb-managed; fi
    cargo build -p space-station-cli --release --locked --target aarch64-apple-darwin --target x86_64-apple-darwin "$@"
    cargo zigbuild -p space-station-cli --release --locked --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-musl "$@"
    RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo xwin build --cross-compiler clang -p space-station-cli --release --locked --target aarch64-pc-windows-msvc --target x86_64-pc-windows-msvc "$@"
    if [ "$feature" = honeycomb-managed ]; then
        for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl aarch64-pc-windows-msvc x86_64-pc-windows-msvc; do
            suffix=''
            case "$target" in *windows*) suffix='.exe';; esac
            cp "$target_dir/$target/release/spacestation$suffix" "$target_dir/$target/release/spacestation-honeycomb$suffix"
        done
    fi
done
python3 scripts/package-cli-release.py
