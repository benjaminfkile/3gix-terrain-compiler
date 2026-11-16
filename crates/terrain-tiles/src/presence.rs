//! Which fine tiles exist.
//!
//! The GLO-30 pyramid ([`Grid::GLO30`]) has data on land only. Its fine
//! levels, `0` to [`FINE_LEVELS`]` - 1`, hold a tile only where some sample
//! of the tile came from land data; under open ocean there is nothing finer
//! than level [`FINE_LEVELS`], which covers the globe from GEBCO. The
//! module may only ask for tiles the source manifest lists
//! (`client-compute.md` section 5.2) and cannot see the manifest, so the
//! tiler writes this map and the module carries it.
//!
//! A level `k > 0` fine tile exists exactly when one of its four children
//! on level `k - 1` does, so the map is the level 0 bits; the coarser fine
//! levels are derived from them. Every tile of a level at or above
//! [`FINE_LEVELS`] exists.
//!
//! # Bytes
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `3GTP` |
//! | 4 | 2 | version, `u16` LE, 1 |
//! | 6 | 2 | reserved, 0 |
//! | 8 | 4 | level 0 tile rows, `u32` LE |
//! | 12 | 4 | level 0 tile columns, `u32` LE |
//! | 16 | ceil(rows cols / 8) | level 0 bits, row after row from the north, west first, least significant bit first |

use std::collections::BTreeSet;

use crate::grid::{Grid, FINE_LEVELS};
use crate::tile::TileKey;

/// The four magic bytes of a presence map: `3GTP` in ASCII.
pub const MAGIC: [u8; 4] = *b"3GTP";

/// The presence map version this crate reads and writes.
pub const VERSION: u16 = 1;

/// Length of the header in bytes.
pub const HEADER_LEN: usize = 16;

/// Which fine tiles of a pyramid exist (module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    /// Tile rows and columns of each fine level, finest first.
    shape: Vec<(u32, u32)>,
    /// One bit set per fine level, finest first, row major.
    bits: Vec<Vec<u64>>,
    /// Every tile exists: a pyramid without sparse levels.
    all: bool,
}

impl Presence {
    /// Every tile of every level exists (a pyramid without sparse fine
    /// levels, such as the GEBCO one).
    pub fn all() -> Presence {
        Presence {
            shape: Vec::new(),
            bits: Vec::new(),
            all: true,
        }
    }

    /// No fine tile of `grid` exists: everything is read from level
    /// [`FINE_LEVELS`] and coarser.
    pub fn none(grid: &Grid) -> Presence {
        Presence::from_level0(grid, &BTreeSet::new())
    }

    /// The map of `grid` whose level 0 tiles are exactly `present`, given as
    /// `(row, col)`; the coarser fine levels follow from them.
    pub fn from_level0(grid: &Grid, present: &BTreeSet<(u32, u32)>) -> Presence {
        let levels = FINE_LEVELS.min(grid.top_level() + 1);
        let mut shape = Vec::new();
        let mut bits = Vec::new();
        for level in 0..levels {
            let (rows, cols) = (grid.tile_rows(level), grid.tile_cols(level));
            shape.push((rows, cols));
            bits.push(vec![0u64; (rows as usize * cols as usize).div_ceil(64)]);
        }
        let mut p = Presence {
            shape,
            bits,
            all: false,
        };
        for &(row, col) in present {
            for level in 0..levels {
                let (r, c) = (row >> level, col >> level);
                if let Some(i) = p.index(level, r, c) {
                    p.bits[level as usize][i / 64] |= 1 << (i % 64);
                }
            }
        }
        p
    }

    fn index(&self, level: u8, row: u32, col: u32) -> Option<usize> {
        let (rows, cols) = *self.shape.get(level as usize)?;
        (row < rows && col < cols).then(|| row as usize * cols as usize + col as usize)
    }

    /// Whether the tile `key` exists.
    pub fn contains(&self, key: TileKey) -> bool {
        if self.all || key.level as usize >= self.shape.len() {
            return true;
        }
        match self.index(key.level, key.row, key.col) {
            Some(i) => self.bits[key.level as usize][i / 64] >> (i % 64) & 1 == 1,
            None => false,
        }
    }

    /// Level 0 tiles that exist.
    pub fn level0_count(&self) -> u64 {
        self.bits
            .first()
            .map_or(0, |b| b.iter().map(|w| u64::from(w.count_ones())).sum())
    }

    /// The map's bytes (module documentation). A map of [`Presence::all`]
    /// has no bytes and encodes to an empty vector.
    pub fn encode(&self) -> Vec<u8> {
        let Some(&(rows, cols)) = self.shape.first() else {
            return Vec::new();
        };
        let n = rows as usize * cols as usize;
        let mut out = Vec::with_capacity(HEADER_LEN + n.div_ceil(8));
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&rows.to_le_bytes());
        out.extend_from_slice(&cols.to_le_bytes());
        let words = &self.bits[0];
        for i in 0..n.div_ceil(8) {
            out.push((words[i / 8] >> ((i % 8) * 8)) as u8);
        }
        out
    }

    /// Decodes a map of `grid` from its bytes, or `None` if they are not one
    /// (wrong magic, version, shape, or length, or bits past the last
    /// tile).
    pub fn decode(grid: &Grid, bytes: &[u8]) -> Option<Presence> {
        if bytes.len() < HEADER_LEN || bytes[0..4] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION || bytes[6] != 0 || bytes[7] != 0 {
            return None;
        }
        let rows = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        let cols = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
        if (rows, cols) != (grid.tile_rows(0), grid.tile_cols(0)) {
            return None;
        }
        let n = rows as usize * cols as usize;
        let body = &bytes[HEADER_LEN..];
        if body.len() != n.div_ceil(8) {
            return None;
        }
        let mut present = BTreeSet::new();
        for i in 0..n {
            if body[i / 8] >> (i % 8) & 1 == 1 {
                present.insert(((i / cols as usize) as u32, (i % cols as usize) as u32));
            }
        }
        if !n.is_multiple_of(8) && body[n / 8] >> (n % 8) != 0 {
            return None;
        }
        Some(Presence::from_level0(grid, &present))
    }

    /// The ids of the tiles to read the box from `lat_min` to `lat_max` and
    /// `lon_min` to `lon_max` (radians) on `level` with the fallback of
    /// [`crate::interp::bicubic_present`]: the box's tiles on `level` that
    /// exist and, while some of them do not, the box's existing tiles on
    /// each coarser level, ascending and unique.
    pub fn tiles_for_region(
        &self,
        grid: &Grid,
        level: u8,
        lat_min: f64,
        lat_max: f64,
        lon_min: f64,
        lon_max: f64,
    ) -> Vec<u64> {
        let mut out = BTreeSet::new();
        let mut level = level;
        let (mut lat_min, mut lat_max) = (lat_min.min(lat_max), lat_min.max(lat_max));
        let (mut lon_min, mut lon_max) = (lon_min.min(lon_max), lon_min.max(lon_max));
        loop {
            let mut missing = false;
            for id in grid.tiles_for_region(level, lat_min, lat_max, lon_min, lon_max) {
                let key = TileKey::from_id(id).expect("a grid tile id");
                if self.contains(key) {
                    out.insert(id);
                } else {
                    missing = true;
                }
            }
            if !missing || level >= grid.top_level() {
                break;
            }
            // A missing sample read on `level` lies up to two of its
            // spacings and a half outside the box, and its own stencil on
            // the next level reaches out from there: widen the box by three
            // spacings of `level` before listing the next one.
            let reach = 3.0 * grid.spacing(level);
            lat_min -= reach;
            lat_max += reach;
            lon_min -= reach;
            lon_max += reach;
            level += 1;
        }
        out.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_level0_and_round_trip() {
        let g = Grid::GLO30;
        let present: BTreeSet<(u32, u32)> = [(0, 0), (5, 9), (719, 1439)].into();
        let p = Presence::from_level0(&g, &present);
        assert!(p.contains(TileKey {
            level: 0,
            row: 5,
            col: 9
        }));
        assert!(!p.contains(TileKey {
            level: 0,
            row: 5,
            col: 8
        }));
        assert!(p.contains(TileKey {
            level: 1,
            row: 2,
            col: 4
        }));
        assert!(p.contains(TileKey {
            level: 3,
            row: 0,
            col: 1
        }));
        assert!(!p.contains(TileKey {
            level: 3,
            row: 1,
            col: 1
        }));
        assert!(p.contains(TileKey {
            level: 4,
            row: 40,
            col: 40
        }));
        assert_eq!(p.level0_count(), 3);
        let bytes = p.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 720 * 1440 / 8);
        assert_eq!(Presence::decode(&g, &bytes), Some(p));
        assert_eq!(Presence::decode(&Grid::GEBCO, &bytes), None);
        assert!(Presence::all().contains(TileKey {
            level: 0,
            row: 1,
            col: 1
        }));
    }

    #[test]
    fn regions_fall_back_until_covered() {
        let g = Grid::GLO30;
        let d = core::f64::consts::PI / 180.0;
        // Nothing fine: only level 4 tiles.
        let none = Presence::none(&g);
        let ids = none.tiles_for_region(&g, 0, 0.1 * d, 0.2 * d, 0.1 * d, 0.2 * d);
        assert!(!ids.is_empty());
        assert!(ids
            .iter()
            .all(|&id| TileKey::from_id(id).unwrap().level == 4));
        // Everything fine there: only level 0.
        let keys: BTreeSet<(u32, u32)> = g
            .tiles_for_region(0, 0.1 * d, 0.2 * d, 0.1 * d, 0.2 * d)
            .into_iter()
            .map(|id| {
                let k = TileKey::from_id(id).unwrap();
                (k.row, k.col)
            })
            .collect();
        let full = Presence::from_level0(&g, &keys);
        let ids = full.tiles_for_region(&g, 0, 0.1 * d, 0.2 * d, 0.1 * d, 0.2 * d);
        assert!(ids
            .iter()
            .all(|&id| TileKey::from_id(id).unwrap().level == 0));
    }
}
