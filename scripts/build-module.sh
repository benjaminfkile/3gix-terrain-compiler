#!/bin/sh
# Builds the terrain module: the library's cdylib for
# wasm32-unknown-unknown, copied to target/module/terrain.wasm, and prints
# its size and SHA-256 (the module hash the hub stores it under,
# client-compute.md section 2.1).
set -eu

cd "$(dirname "$0")/.."

cargo build -q --locked --release -p terrain-module --lib --target wasm32-unknown-unknown
mkdir -p target/module
cp target/wasm32-unknown-unknown/release/terrain_module.wasm target/module/terrain.wasm
size=$(wc -c < target/module/terrain.wasm | tr -d ' ')
hash=$(sha256sum target/module/terrain.wasm | cut -d ' ' -f 1)
echo "target/module/terrain.wasm $size bytes sha256 $hash"
