//! On wasm32, keeps the binary inside WebAssembly 1.0 plus the features the
//! module ABI allows (core `docs/modules.md`, building).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=--compress-relocations");
        println!("cargo:rustc-link-arg-cdylib=--strip-debug");
    }
}
