//! The [`terrain_tiles::check::digest`] as a WebAssembly export.
//!
//! Built for `wasm32-unknown-unknown` with no imports, the same way a
//! compiler module is built (`client-compute.md` section 4), and run by
//! `scripts/tiles-node.mjs` under Node, which stands in for the browser.
//! The digest must equal the native one in `conformance/tiles-digest.sha256`
//! (`space-model.md` section 4 item 6).

#![warn(missing_docs)]

/// Computes the digest and returns a pointer to its 32 bytes in linear
/// memory. The bytes stay valid for the life of the instance.
#[no_mangle]
pub extern "C" fn tiles_check_digest() -> *const u8 {
    Box::leak(Box::new(terrain_tiles::check::digest())).as_ptr()
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_digest_matches_the_pinned_one() {
        let want = include_str!("../../../conformance/tiles-digest.sha256").trim();
        // SAFETY: the export returns a pointer to 32 leaked bytes.
        let got = unsafe { core::slice::from_raw_parts(super::tiles_check_digest(), 32) };
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, want);
    }
}
