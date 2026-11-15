// Runs the terrain-tiles determinism digest compiled to WebAssembly under
// Node (V8, standing in for the browser) and compares it with the native
// digest pinned in a file.
//
// Usage: node scripts/tiles-node.mjs CHECK.wasm EXPECTED.sha256
//
// The binary is instantiated with an empty import object, as a module is
// (client-compute.md section 4). Exits 1 on any difference.

import { readFileSync } from "node:fs";

const [wasmPath, expectedPath] = process.argv.slice(2);
if (!wasmPath || !expectedPath) {
    console.error("usage: node scripts/tiles-node.mjs CHECK.wasm EXPECTED.sha256");
    process.exit(2);
}

const module = await WebAssembly.compile(readFileSync(wasmPath));
if (WebAssembly.Module.imports(module).length !== 0) {
    console.error("node: the check binary has imports");
    process.exit(1);
}
const instance = await WebAssembly.instantiate(module, {});
const ptr = instance.exports.tiles_check_digest() >>> 0;
const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, 32);
const got = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const want = readFileSync(expectedPath, "utf8").trim();
if (got !== want) {
    console.error(`node: digest ${got}, expected ${want}`);
    process.exit(1);
}
console.log(`node: digest ${got} ok`);
