//! A fixed digest over everything this crate computes.
//!
//! [`digest`] encodes a set of tiles with fixed contents, decodes them back,
//! and interpolates them at fixed points on two levels, and hashes every
//! byte and every `f64` bit pattern that comes out. The native test pins the
//! digest, and `scripts/ci.sh` runs the same function compiled to
//! WebAssembly under Node and requires the same digest
//! (`space-model.md` section 4 item 6, determinism checks). It needs no
//! input, so the check has nothing to disagree about but the code.

use std::collections::BTreeMap;

use gx_core::sha256::{Sha256, DIGEST_LEN};

use crate::grid::{Grid, TILE_SAMPLES, TILE_SIZE};
use crate::interp::{bicubic, nearest};
use crate::tile::{Tile, TileKey};

/// The grid the digest uses: 480 by 960 samples, levels 0 and 1.
pub const GRID: Grid = match Grid::new(480, 960) {
    Some(g) => g,
    None => panic!("the check grid is valid"),
};

/// The elevation of lattice sample `(r, c)` in the check's tiles: a fixed
/// integer pattern with large and small, positive and negative values.
pub fn pattern(level: u8, r: u32, c: u32) -> i16 {
    let a = (r as i64 * 7919 + c as i64 * 104_729 + level as i64 * 31) % 4001 - 2000;
    let b = ((r as i64 - 200) * (c as i64 - 500)) % 9000;
    (a + b) as i16
}

/// Every tile of both levels of [`GRID`], filled with [`pattern`].
pub fn tiles() -> BTreeMap<u64, Tile> {
    let mut out = BTreeMap::new();
    for level in GRID.levels() {
        for row in 0..GRID.tile_rows(level) {
            for col in 0..GRID.tile_cols(level) {
                let mut s = Vec::with_capacity(TILE_SAMPLES);
                for i in 0..TILE_SIZE {
                    for j in 0..TILE_SIZE {
                        s.push(pattern(level, row * TILE_SIZE + i, col * TILE_SIZE + j));
                    }
                }
                let t = Tile::new(TileKey { level, row, col }, s).expect("a full tile");
                out.insert(t.id(), t);
            }
        }
    }
    out
}

/// The digest: SHA-256 over every tile's encoded bytes in id order, then
/// over the bit patterns of [`bicubic`] and the values of [`nearest`] at
/// 1024 fixed points per level. Panics if a tile does not decode to
/// itself.
pub fn digest() -> [u8; DIGEST_LEN] {
    let tiles = tiles();
    let mut h = Sha256::new();
    let mut decoded = BTreeMap::new();
    for (&id, tile) in &tiles {
        let bytes = tile.encode();
        h.update(&id.to_le_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
        let back = Tile::decode_id(id, &bytes).expect("the tile decodes");
        assert_eq!(&back, tile, "the tile round trips");
        decoded.insert(id, back);
    }
    for level in GRID.levels() {
        for i in 0..32u32 {
            for j in 0..32u32 {
                // Latitudes from pole to pole and longitudes past both ends.
                let lat = (f64::from(i) / 31.0 - 0.5) * core::f64::consts::PI;
                let lon = (f64::from(j) / 31.0 - 0.5) * 2.2 * core::f64::consts::PI + 0.001;
                let v = bicubic(&GRID, &decoded, level, lat, lon).expect("every tile");
                let n = nearest(&GRID, &decoded, level, lat, lon).expect("every tile");
                h.update(&v.to_bits().to_le_bytes());
                h.update(&n.to_le_bytes());
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_pinned() {
        let want = include_str!("../../../conformance/tiles-digest.sha256").trim();
        assert_eq!(gx_core::sha256::to_hex(&digest()), want);
    }
}
