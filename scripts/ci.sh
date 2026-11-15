#!/bin/sh
# Local and CI check: format, lint, the zstd pin, the WebAssembly builds,
# the tile determinism check under Node, the module determinism harness,
# tests, dash check. Exits non-zero on the first failure.
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
# The tile format must build for the module target, without relaxed SIMD
# or threads (neither is enabled by default for wasm32-unknown-unknown).
cargo build -q --locked --release --target wasm32-unknown-unknown \
    -p terrain-tiles
cargo build -q --locked --release --target wasm32-unknown-unknown \
    -p tiles-wasm-check --lib

echo "== module build"
sh scripts/build-module.sh

echo "== tile determinism under Node"
# The same digest the native test pins (conformance/tiles-digest.sha256),
# computed by the WebAssembly build under Node.
node scripts/tiles-node.mjs \
    target/wasm32-unknown-unknown/release/tiles_wasm_check.wasm \
    conformance/tiles-digest.sha256

echo "== module determinism harness"
# The core library's harness (core docs/modules.md): every vector in
# conformance/module through the native module host (which also runs the
# host's admission checks: no imports, no forbidden instructions) and
# through Node. It runs from the core checkout cargo fetched for the gx-core
# dependency, or from GX_CORE_DIR when set.
if [ -n "${GX_CORE_DIR:-}" ]; then
    core=$GX_CORE_DIR
fi
if [ ! -f "$core/scripts/module-node.mjs" ]; then
    echo "core library checkout not found (set GX_CORE_DIR)" >&2
    exit 1
fi
cargo build -q --release --manifest-path "$core/Cargo.toml" \
    -p gx-module-host --bin gx-module-run --target-dir target/harness
status=0
target/harness/release/gx-module-run target/module/terrain.wasm conformance/module || status=1
node "$core/scripts/module-node.mjs" target/module/terrain.wasm conformance/module || status=1
if [ "$status" -ne 0 ]; then
    echo "module determinism: FAILED" >&2
    exit 1
fi

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
