//! The elevation surface of one pyramid level, read bicubically from the
//! tiles a cell was given.
//!
//! [`Surface::elevation`] gives exactly the bits of
//! [`terrain_tiles::bicubic`] (the same operations in the same order) and
//! keeps the last 4 x 4 stencil it read. Neighboring sub-samples of a deep
//! cell are far closer together than the data's spacing, so almost every
//! call reuses the stencil and reads no tile.

use std::collections::BTreeMap;

use terrain_tiles::interp::{catmull_rom, sample_at};
use terrain_tiles::{Grid, Tile, TileError};

use crate::geodesy::{H_MAX, H_MIN};

/// One level's surface over a set of decoded tiles.
pub struct Surface<'a> {
    grid: Grid,
    level: u8,
    tiles: &'a BTreeMap<u64, Tile>,
    stencil_at: Option<(i64, i64)>,
    stencil: [[f64; 4]; 4],
}

impl<'a> Surface<'a> {
    /// The surface of `level` of `grid` over `tiles`.
    pub fn new(grid: Grid, level: u8, tiles: &'a BTreeMap<u64, Tile>) -> Surface<'a> {
        Surface {
            grid,
            level,
            tiles,
            stencil_at: None,
            stencil: [[0.0; 4]; 4],
        }
    }

    /// The level.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The bicubic elevation at geodetic latitude `lat` and longitude
    /// `lon` (radians), meters, bit for bit [`terrain_tiles::bicubic`].
    pub fn elevation(&mut self, lat: f64, lon: f64) -> Result<f64, TileError> {
        let p = self.grid.position(self.level, lat, lon);
        let (fy, fx) = (p.y.floor(), p.x.floor());
        let (ty, tx) = (p.y - fy, p.x - fx);
        let (r0, c0) = (fy as i64, fx as i64);
        if self.stencil_at != Some((r0, c0)) {
            for (i, row) in self.stencil.iter_mut().enumerate() {
                for (j, v) in row.iter_mut().enumerate() {
                    let s = sample_at(
                        &self.grid,
                        self.tiles,
                        self.level,
                        r0 - 1 + i as i64,
                        c0 - 1 + j as i64,
                    )?;
                    *v = f64::from(s);
                }
            }
            self.stencil_at = Some((r0, c0));
        }
        let mut rows = [0.0f64; 4];
        for (row, v) in rows.iter_mut().zip(self.stencil.iter()) {
            *row = catmull_rom(*v, tx);
        }
        Ok(catmull_rom(rows, ty))
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
        let mut s = Surface::new(GRID, 0, &tiles);
        for i in 0..2000 {
            let lat = (f64::from(i) * 0.000_29 - 0.25) * d;
            let lon = (f64::from(i % 97) * 0.0051 - 0.24) * d;
            let a = s.elevation(lat, lon).unwrap();
            let b = terrain_tiles::bicubic(&GRID, &tiles, 0, lat, lon).unwrap();
            assert_eq!(a.to_bits(), b.to_bits(), "{i}");
        }
    }
}
