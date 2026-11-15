#!/bin/sh
# Local and CI check: format, lint, the WebAssembly build and its
# determinism check under Node, tests, dash check. Exits non-zero on the
# first failure.
set -eu

cd "$(dirname "$0")/.."

echo "== cargo fmt"
cargo fmt --all -- --check

echo "== cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "== zstd pin"
# Tile bytes depend on the zstd crates and the bundled C library, so they
# must be locked exactly as the core library locks them (core
# docs/determinism.md). Compared against the Cargo.lock of the core checkout
# cargo fetched for the gx-core dependency.
core=$(cargo metadata --format-version 1 --locked |
    tr ',' '\n' |
    sed -n 's/^"manifest_path":"\(.*\)\/crates\/gx-core\/Cargo.toml"$/\1/p' |
    head -n 1)
zstd_lock() {
    grep -A1 '^name = "zstd' "$1" | grep -v '^--$'
}
if [ ! -f "$core/Cargo.lock" ]; then
    echo "core library checkout not found" >&2
    exit 1
fi
if [ "$(zstd_lock Cargo.lock)" != "$(zstd_lock "$core/Cargo.lock")" ]; then
    echo "zstd crates are not locked as the core library locks them:" >&2
    zstd_lock Cargo.lock >&2
    exit 1
fi

echo "== WebAssembly build"
# The tile format and the module placeholder must build for the module
# target, without relaxed SIMD or threads (neither is enabled by default
# for wasm32-unknown-unknown).
cargo build -q --locked --release --target wasm32-unknown-unknown \
    -p terrain-tiles -p terrain-module
cargo build -q --locked --release --target wasm32-unknown-unknown \
    -p tiles-wasm-check --lib

echo "== tile determinism under Node"
# The same digest the native test pins (conformance/tiles-digest.sha256),
# computed by the WebAssembly build under Node.
node scripts/tiles-node.mjs \
    target/wasm32-unknown-unknown/release/tiles_wasm_check.wasm \
    conformance/tiles-digest.sha256

echo "== cargo test"
cargo test --workspace

echo "== dash check"
# No em or en dashes in anything this repository holds.
if grep -rnI "$(printf '\342\200\224')\|$(printf '\342\200\223')" \
    --exclude-dir=target --exclude-dir=.git --exclude=Cargo.lock . ; then
    echo "em or en dash found" >&2
    exit 1
fi

echo "ci: ok"
