//! Reading elevations off a level: the nearest sample and bicubic
//! (Catmull-Rom) interpolation, across tile edges.
//!
//! The terrain module reads its source tiles through these functions
//! (`client-compute.md` section 4, `gx_compile` receives the tiles
//! `gx_sources` asked for). The caller supplies the tiles, normally the ones
//! [`Grid::bicubic_tiles`] or [`Grid::tiles_for_region`] listed. Stencils
//! that reach past 180 degrees wrap and stencils that reach past a pole are
//! clamped, as [`Grid::lattice_col`] and [`Grid::lattice_row`] describe.
//!
//! The arithmetic is `+`, `-`, `*`, `/`, and `floor` on `f64`, written as
//! separate operations (Rust never fuses them), so the result is bitwise the
//! same natively and in WebAssembly.

use std::collections::BTreeMap;

use crate::grid::Grid;
use crate::presence::Presence;
use crate::tile::{Tile, TileError};

/// Where an interpolator gets tiles from.
pub trait TileSource {
    /// The tile with this id, or `None` if the caller did not supply it.
    fn tile(&self, id: u64) -> Option<&Tile>;
}

impl TileSource for BTreeMap<u64, Tile> {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.get(&id)
    }
}

impl TileSource for [Tile] {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.iter().find(|t| t.id() == id)
    }
}

impl TileSource for Vec<Tile> {
    fn tile(&self, id: u64) -> Option<&Tile> {
        self.as_slice().tile(id)
    }
}

/// The elevation in meters of lattice sample index `(r, c)` on `level`,
/// after wrapping and clamping.
pub fn sample_at<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    level: u8,
    r: i64,
    c: i64,
) -> Result<i16, TileError> {
    let (key, index) = grid.locate(
        level,
        grid.lattice_row(level, r),
        grid.lattice_col(level, c),
    );
    let id = key.id();
    let tile = tiles.tile(id).ok_or(TileError::Missing(id))?;
    Ok(tile.samples()[index])
}

/// The elevation in meters of the sample whose cell holds latitude `lat`
/// and longitude `lon` (radians) on `level`.
pub fn nearest<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    level: u8,
    lat: f64,
    lon: f64,
) -> Result<i16, TileError> {
    let p = grid.position(level, lat, lon);
    let r = (p.y + 0.5).floor() as i64;
    let c = (p.x + 0.5).floor() as i64;
    sample_at(grid, tiles, level, r, c)
}

/// The elevation in meters at latitude `lat` and longitude `lon` (radians)
/// on `level`, interpolated bicubically (Catmull-Rom, separable, columns
/// first) from the 4 x 4 samples around the point. Passes through every
/// sample exactly; between samples it is smooth with a continuous slope.
pub fn bicubic<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    level: u8,
    lat: f64,
    lon: f64,
) -> Result<f64, TileError> {
    let p = grid.position(level, lat, lon);
    let (fy, fx) = (p.y.floor(), p.x.floor());
    let (ty, tx) = (p.y - fy, p.x - fx);
    let (r0, c0) = (fy as i64, fx as i64);
    let mut rows = [0.0f64; 4];
    for (i, row) in rows.iter_mut().enumerate() {
        let r = r0 - 1 + i as i64;
        let mut v = [0.0f64; 4];
        for (j, value) in v.iter_mut().enumerate() {
            *value = f64::from(sample_at(grid, tiles, level, r, c0 - 1 + j as i64)?);
        }
        *row = catmull_rom(v, tx);
    }
    Ok(catmull_rom(rows, ty))
}

/// The elevation of lattice sample `(r, c)` on `level` like [`sample_at`],
/// except where its tile does not exist (`presence`): there it is the
/// [`bicubic_present`] elevation at the sample's center on the next
/// coarser level. A fine level missing under the ocean thus reads as the
/// coarser data interpolated, and a stencil across a coast mixes the two
/// smoothly.
pub fn sample_present<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    presence: &Presence,
    level: u8,
    r: i64,
    c: i64,
) -> Result<f64, TileError> {
    let (lr, lc) = (grid.lattice_row(level, r), grid.lattice_col(level, c));
    let (key, index) = grid.locate(level, lr, lc);
    if presence.contains(key) || level >= grid.top_level() {
        let id = key.id();
        let tile = tiles.tile(id).ok_or(TileError::Missing(id))?;
        return Ok(f64::from(tile.samples()[index]));
    }
    let (lat, lon) = grid.center(level, lr, lc);
    bicubic_present(grid, tiles, presence, level + 1, lat, lon)
}

/// [`bicubic`] over the tiles that exist (`presence`), each sample read with
/// [`sample_present`]. With [`Presence::all`] it is exactly [`bicubic`].
pub fn bicubic_present<S: TileSource + ?Sized>(
    grid: &Grid,
    tiles: &S,
    presence: &Presence,
    level: u8,
    lat: f64,
    lon: f64,
) -> Result<f64, TileError> {
    let p = grid.position(level, lat, lon);
    let (fy, fx) = (p.y.floor(), p.x.floor());
    let (ty, tx) = (p.y - fy, p.x - fx);
    let (r0, c0) = (fy as i64, fx as i64);
    let mut rows = [0.0f64; 4];
    for (i, row) in rows.iter_mut().enumerate() {
        let r = r0 - 1 + i as i64;
        let mut v = [0.0f64; 4];
        for (j, value) in v.iter_mut().enumerate() {
            *value = sample_present(grid, tiles, presence, level, r, c0 - 1 + j as i64)?;
        }
        *row = catmull_rom(v, tx);
    }
    Ok(catmull_rom(rows, ty))
}

/// The Catmull-Rom spline through `p[1]` (at `t = 0`) and `p[2]` (at
/// `t = 1`), with tangents from the outer points.
pub fn catmull_rom(p: [f64; 4], t: f64) -> f64 {
    let [p0, p1, p2, p3] = p;
    let a = p2 - p0;
    let b = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let c = 3.0 * (p1 - p2) + p3 - p0;
    p1 + 0.5 * t * (a + t * (b + t * c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{TILE_SAMPLES, TILE_SIZE};
    use crate::tile::TileKey;
    use core::f64::consts::PI;

    /// Every tile of level 0 of a small grid, with sample value
    /// `r + 2 c` (global lattice indices), a plane in index space.
    fn plane(grid: &Grid) -> BTreeMap<u64, Tile> {
        let mut tiles = BTreeMap::new();
        for row in 0..grid.tile_rows(0) {
            for col in 0..grid.tile_cols(0) {
                let mut s = Vec::with_capacity(TILE_SAMPLES);
                for i in 0..TILE_SIZE {
                    for j in 0..TILE_SIZE {
                        let r = row * TILE_SIZE + i;
                        let c = col * TILE_SIZE + j;
                        s.push((grid.data_row(r) + 2 * grid.data_col(c)) as i16);
                    }
                }
                let t = Tile::new(TileKey { level: 0, row, col }, s).unwrap();
                tiles.insert(t.id(), t);
            }
        }
        tiles
    }

    #[test]
    fn spline_reproduces_lines_and_samples() {
        assert_eq!(catmull_rom([1.0, 2.0, 3.0, 4.0], 0.25), 2.25);
        assert_eq!(catmull_rom([5.0, -3.0, 8.0, 1.0], 0.0), -3.0);
        assert_eq!(catmull_rom([5.0, -3.0, 8.0, 1.0], 1.0), 8.0);
    }

    #[test]
    fn plane_is_exact_inside_and_across_tile_edges() {
        let grid = Grid::new(480, 960).unwrap();
        let tiles = plane(&grid);
        let k = grid.samples_per_radian(0);
        for &(y, x) in &[
            (10.25, 20.75),
            (239.5, 239.5),
            (120.0, 479.9),
            (300.3, 700.1),
        ] {
            let lat = PI / 2.0 - (y + 0.5) / k;
            let lon = -PI + (x + 0.5) / k;
            let v = bicubic(&grid, &tiles, 0, lat, lon).unwrap();
            assert!((v - (y + 2.0 * x)).abs() < 1e-6, "{y} {x}: {v}");
            let n = nearest(&grid, &tiles, 0, lat, lon).unwrap();
            assert_eq!(n as f64, (y + 0.5).floor() + 2.0 * (x + 0.5).floor());
        }
    }

    #[test]
    fn wraps_and_clamps() {
        let grid = Grid::new(480, 960).unwrap();
        let tiles = plane(&grid);
        // West of the first column reads the last column.
        assert_eq!(sample_at(&grid, &tiles, 0, 0, -1).unwrap(), 2 * 959);
        assert_eq!(sample_at(&grid, &tiles, 0, -3, 960).unwrap(), 0);
        assert_eq!(sample_at(&grid, &tiles, 0, 999, 0).unwrap(), 479);
        // At the pole the stencil rows are 0, 0, 0, 1, half way between
        // the second and the third: 959 + (1/4) (1/2) (-1 + 1/2).
        let v = bicubic(&grid, &tiles, 0, PI / 2.0, 0.0).unwrap();
        assert_eq!(v, 958.9375);
    }

    #[test]
    fn missing_tile_is_an_error() {
        let grid = Grid::new(480, 960).unwrap();
        let tiles: Vec<Tile> = Vec::new();
        assert_eq!(
            nearest(&grid, tiles.as_slice(), 0, 0.0, 0.0),
            Err(TileError::Missing(
                TileKey {
                    level: 0,
                    row: 1,
                    col: 2
                }
                .id()
            ))
        );
    }

    #[test]
    fn bicubic_tiles_cover_the_stencil() {
        let grid = Grid::new(480, 960).unwrap();
        let all = plane(&grid);
        let k = grid.samples_per_radian(0);
        for &(y, x) in &[(239.5, 239.5), (0.1, 0.2), (479.0, 959.4), (240.0, 0.0)] {
            let lat = PI / 2.0 - (y + 0.5) / k;
            let lon = -PI + (x + 0.5) / k;
            let ids = grid.bicubic_tiles(0, lat, lon);
            let some: Vec<Tile> = ids.iter().map(|id| all[id].clone()).collect();
            assert_eq!(
                bicubic(&grid, some.as_slice(), 0, lat, lon),
                bicubic(&grid, &all, 0, lat, lon)
            );
        }
    }
}
