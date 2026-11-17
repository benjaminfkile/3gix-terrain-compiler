//! The elevation surface of one pyramid level, read bicubically from the
//! tiles a cell was given.
//!
//! [`Surface::elevation`] gives exactly the bits of
//! [`terrain_tiles::bicubic`] (the same operations in the same order) and
//! keeps the last 4 x 4 stencil it read. Neighboring sub-samples of a deep
//! cell are far closer together than the data's spacing, so almost every
//! call reuses the stencil and reads no tile.
//!
//! A sample whose tile does not exist is the bicubic of the next coarser
//! level, whose samples may be missing too
//! ([`terrain_tiles::interp::sample_present`]). Evaluated directly that is
//! a tree of `16^k` reads `k` levels deep (65536 per sample for open ocean
//! read at level 0), so the surface keeps every such sample it computes:
//! the same values, each lattice point computed once per cell.
//!
//! The points of one sample fall in a few neighboring stencils and the
//! sampler visits them in turn, so the surface also keeps every stencil it
//! has read, by its lattice cell: a stencil is read from the tiles once
//! per cell, not each time the points move back into it.

use std::collections::{BTreeMap, HashMap};

use terrain_tiles::interp::catmull_rom;
#[cfg(test)]
use terrain_tiles::TileKey;
use terrain_tiles::{Grid, Presence, Tile, TileError};

use crate::geodesy::{H_MAX, H_MIN};

/// One level's surface over a set of decoded tiles.
pub struct Surface<'a> {
    grid: Grid,
    level: u8,
    tiles: &'a BTreeMap<u64, Tile>,
    presence: &'a Presence,
    stencil_at: Option<(i64, i64)>,
    stencil: [[f64; 4]; 4],
    /// Every stencil read so far, by the lattice row and column of its
    /// second sample.
    stencils: HashMap<(i64, i64), [[f64; 4]; 4]>,
    /// Samples of missing tiles already computed, by level and lattice
    /// row and column.
    fallback: HashMap<(u8, u32, u32), f64>,
}

impl<'a> Surface<'a> {
    /// The surface of `level` of `grid` over `tiles`, reading the next
    /// coarser level where a fine tile does not exist (`presence`,
    /// [`terrain_tiles::interp::sample_present`]).
    pub fn new(
        grid: Grid,
        level: u8,
        tiles: &'a BTreeMap<u64, Tile>,
        presence: &'a Presence,
    ) -> Surface<'a> {
        Surface {
            grid,
            level,
            tiles,
            presence,
            stencil_at: None,
            stencil: [[0.0; 4]; 4],
            stencils: HashMap::new(),
            fallback: HashMap::new(),
        }
    }

    /// The level.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The bicubic elevation at geodetic latitude `lat` and longitude
    /// `lon` (radians), meters, bit for bit
    /// [`terrain_tiles::bicubic_present`].
    pub fn elevation(&mut self, lat: f64, lon: f64) -> Result<f64, TileError> {
        let p = self.grid.position(self.level, lat, lon);
        let (fy, fx) = (p.y.floor(), p.x.floor());
        let (ty, tx) = (p.y - fy, p.x - fx);
        let (r0, c0) = (fy as i64, fx as i64);
        if self.stencil_at != Some((r0, c0)) {
            self.stencil = match self.stencils.get(&(r0, c0)) {
                Some(&stencil) => stencil,
                None => {
                    let mut stencil = [[0.0f64; 4]; 4];
                    for (i, row) in stencil.iter_mut().enumerate() {
                        for (j, v) in row.iter_mut().enumerate() {
                            *v = self.sample(self.level, r0 - 1 + i as i64, c0 - 1 + j as i64)?;
                        }
                    }
                    self.stencils.insert((r0, c0), stencil);
                    stencil
                }
            };
            self.stencil_at = Some((r0, c0));
        }
        let mut rows = [0.0f64; 4];
        for (row, v) in rows.iter_mut().zip(self.stencil.iter()) {
            *row = catmull_rom(*v, tx);
        }
        Ok(catmull_rom(rows, ty))
    }

    /// [`terrain_tiles::interp::sample_present`], bit for bit, keeping the
    /// samples of missing tiles.
    fn sample(&mut self, level: u8, r: i64, c: i64) -> Result<f64, TileError> {
        let (lr, lc) = (
            self.grid.lattice_row(level, r),
            self.grid.lattice_col(level, c),
        );
        let (key, index) = self.grid.locate(level, lr, lc);
        if self.presence.contains(key) || level >= self.grid.top_level() {
            let id = key.id();
            let tile = self.tiles.get(&id).ok_or(TileError::Missing(id))?;
            return Ok(f64::from(tile.samples()[index]));
        }
        if let Some(&v) = self.fallback.get(&(level, lr, lc)) {
            return Ok(v);
        }
        let (lat, lon) = self.grid.center(level, lr, lc);
        let coarser = level + 1;
        let p = self.grid.position(coarser, lat, lon);
        let (fy, fx) = (p.y.floor(), p.x.floor());
        let (ty, tx) = (p.y - fy, p.x - fx);
        let (r0, c0) = (fy as i64, fx as i64);
        let mut rows = [0.0f64; 4];
        for (i, row) in rows.iter_mut().enumerate() {
            let mut v = [0.0f64; 4];
            for (j, value) in v.iter_mut().enumerate() {
                *value = self.sample(coarser, r0 - 1 + i as i64, c0 - 1 + j as i64)?;
            }
            *row = catmull_rom(v, tx);
        }
        let v = catmull_rom(rows, ty);
        self.fallback.insert((level, lr, lc), v);
        Ok(v)
    }

    /// [`Surface::elevation`] clamped to the shell the module assumes the
    /// surface lies in, [`H_MIN`] to [`H_MAX`]. The data lies inside it;
    /// only a spline's overshoot next to the most extreme samples could
    /// reach past it.
    pub fn clamped_elevation(&mut self, lat: f64, lon: f64) -> Result<f64, TileError> {
        Ok(self.elevation(lat, lon)?.clamp(H_MIN, H_MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;
    use crate::plan::GRID;

    #[test]
    fn same_bits_as_the_tile_crate() {
        let d = core::f64::consts::PI / 180.0;
        let ids = GRID.tiles_for_region(0, -0.3 * d, 0.3 * d, -0.3 * d, 0.3 * d);
        let tiles: BTreeMap<u64, Tile> = ids.iter().map(|&id| (id, fixture::tile(id))).collect();
        let all = Presence::all();
        let mut s = Surface::new(GRID, 0, &tiles, &all);
        for i in 0..2000 {
            let lat = (f64::from(i) * 0.000_29 - 0.25) * d;
            let lon = (f64::from(i % 97) * 0.0051 - 0.24) * d;
            let a = s.elevation(lat, lon).unwrap();
            let b = terrain_tiles::bicubic(&GRID, &tiles, 0, lat, lon).unwrap();
            assert_eq!(a.to_bits(), b.to_bits(), "{i}");
        }
    }

    #[test]
    fn same_bits_as_the_tile_crate_where_fine_tiles_are_missing() {
        let d = core::f64::consts::PI / 180.0;
        // Level 0 present on one tile only: the rest falls back through
        // levels 1 to 4.
        let present = std::collections::BTreeSet::from([(359u32, 720u32)]);
        let presence = Presence::from_level0(&GRID, &present);
        let ids = presence.tiles_for_region(&GRID, 0, -0.4 * d, 0.4 * d, -0.4 * d, 0.4 * d);
        let tiles: BTreeMap<u64, Tile> = ids.iter().map(|&id| (id, fixture::tile(id))).collect();
        let mut s = Surface::new(GRID, 0, &tiles, &presence);
        for i in 0..300 {
            let lat = (f64::from(i) * 0.0021 - 0.3) * d;
            let lon = (f64::from(i % 31) * 0.017 - 0.25) * d;
            let a = s.elevation(lat, lon).unwrap();
            let b = terrain_tiles::bicubic_present(&GRID, &tiles, &presence, 0, lat, lon).unwrap();
            assert_eq!(a.to_bits(), b.to_bits(), "{i}");
        }
    }

    #[test]
    fn reads_only_listed_tiles_where_fine_tiles_are_missing() {
        // A sparse level 0 map around the north pole and around 0, 0: every
        // third tile in a band, so boxes see every mix of present and
        // missing neighbors on every fine level.
        let mut present = std::collections::BTreeSet::new();
        for r in 0..12u32 {
            for c in 0..GRID.tile_cols(0) {
                if (r * 7 + c * 13) % 3 == 0 {
                    present.insert((r, c));
                }
            }
        }
        for r in 350..370u32 {
            for c in 710..730u32 {
                if (r * 5 + c * 11) % 3 == 0 {
                    present.insert((r, c));
                }
            }
        }
        let presence = Presence::from_level0(&GRID, &present);
        let d = core::f64::consts::PI / 180.0;
        let half = 0.004 * d;
        let mut centers = Vec::new();
        for i in 0..24 {
            // Near the pole, across tile edges at every fine level.
            centers.push((
                89.98 * d - f64::from(i) * 0.37 * d,
                (f64::from(i) * 7.9 - 120.0) * d,
            ));
            // Near 0, 0.
            centers.push((
                (f64::from(i) * 0.23 - 2.5) * d,
                (f64::from(i) * 0.31 - 3.0) * d,
            ));
        }
        for (k, &(lat, lon)) in centers.iter().enumerate() {
            let (la0, la1) = (
                (lat - half).max(-0.5 * core::f64::consts::PI),
                (lat + half).min(0.5 * core::f64::consts::PI),
            );
            let ids = presence.tiles_for_region(&GRID, 0, la0, la1, lon - half, lon + half);
            let zero = |id: u64| {
                let key = TileKey::from_id(id).unwrap();
                Tile::new(key, vec![0i16; GRID.tile_samples()]).unwrap()
            };
            let tiles: BTreeMap<u64, Tile> = ids.iter().map(|&id| (id, zero(id))).collect();
            let mut s = Surface::new(GRID, 0, &tiles, &presence);
            for i in 0..=8 {
                for j in 0..=8 {
                    let a = la0 + (la1 - la0) * f64::from(i) / 8.0;
                    let o = lon - half + 2.0 * half * f64::from(j) / 8.0;
                    s.elevation(a, o)
                        .unwrap_or_else(|e| panic!("box {k} at {a} {o}: {e}"));
                }
            }
        }
    }
}
