//! The conformance vectors in `conformance/module/` are what the module
//! computes.
//!
//! The vectors pin the module's output bytes (`space-model.md` section 4
//! item 6). This test regenerates them natively through the module's own
//! entry points through module ABI v2 (`gx_module_guest::run_inputs`,
//! `run_sources_v2`, and `run_compile_v2`, with the system module's record of
//! the frame) and
//! requires every file to match the checked-in copy byte for byte;
//! `scripts/ci.sh` then runs the same vectors through the built WebAssembly
//! module natively and under Node with the core determinism harness. To
//! refresh the vectors after an intended change, run
//! `cargo run -p terrain-module --release -- conformance conformance/module`
//! and review the diff.

use std::path::Path;

use gx_core::key::ChunkKey;
use terrain_module::conformance::{files, VECTORS};
use terrain_module::{compile, fixture, sources};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../conformance/module");

#[test]
fn checked_in_vectors_match() {
    let dir = Path::new(DIR);
    let generated = files().unwrap();
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    on_disk.sort();
    let names: Vec<&String> = generated.iter().map(|(n, _)| n).collect();
    assert_eq!(on_disk.iter().collect::<Vec<_>>(), names);
    for (name, bytes) in &generated {
        let disk = std::fs::read(dir.join(name)).unwrap();
        assert!(
            &disk == bytes,
            "conformance/module/{name} differs from what the module computes"
        );
    }
}

#[test]
fn twelve_vectors_and_the_module_is_the_library() {
    assert_eq!(VECTORS.len(), 12);
    for v in VECTORS.iter() {
        let key: ChunkKey = v.key.parse().unwrap();
        let ids = sources(&fixture::frame(), &key);
        assert_eq!(v.sources().unwrap(), ids, "{}", v.name);
        let direct = compile(&fixture::frame(), &key, &fixture::tiles(&ids)).unwrap();
        assert_eq!(v.section().unwrap(), direct, "{}", v.name);
    }
}
