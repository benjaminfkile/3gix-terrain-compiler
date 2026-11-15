//! The geometry of the tile pyramid.
//!
//! Implements the layout half of the source set that `client-compute.md`
//! section 5.3 describes ("datasets are cut into pyramids, each level half
//! the resolution of the one below").
//!
//! # The data grid
//!
//! The dataset is an equirectangular grid in geodetic latitude and
//! longitude with `rows` rows and `cols = 2 * rows` columns covering the
//! whole globe. Samples are cell centered: data row `r` (counted from the
//! north, first row northernmost) is the cell between latitudes
//! `90 deg - r s` and `90 deg - (r + 1) s`, centered on
//! `90 deg - (r + 1/2) s`, where `s = 180 deg / rows`. Data column `c`
//! (counted eastward from 180 deg west) is centered on
//! `-180 deg + (c + 1/2) s`. No sample sits on a pole or on the
//! antimeridian. The grid of the cut dataset, [`Grid::GEBCO`], has 43200 rows
//! and 86400 columns, so `s` is 15 arcseconds.
//!
//! # Levels and tiles
//!
//! Level 0 has the data's own spacing. Level `k` has spacing `s 2^k`. Every
//! tile holds [`TILE_SIZE`] by [`TILE_SIZE`] samples, so a level 0 tile of
//! the cut dataset spans 1 degree and a level `k` tile `2^k` degrees. A level
//! has [`Grid::tile_rows`] rows and [`Grid::tile_cols`] columns of tiles, as
//! many as it takes to cover the globe, so its tiles form a lattice of
//! samples that may extend past the south pole and past 180 degrees east.
//! The top level, [`Grid::top_level`], is the first whose tiles cover the
//! globe in at most 2 by 1 tiles (level 8 for the cut dataset).
//!
//! # Values
//!
//! Lattice sample `(r, c)` of level 0 is data sample
//! `(min(r, rows - 1), c mod cols)`: past 180 degrees the lattice wraps, past
//! the south pole it repeats the last data row.
//!
//! Lattice sample `(r, c)` of level `k > 0` is the mean of the four samples
//! `(2r + i, 2c + j)`, `i, j` in `{0, 1}`, of level `k - 1`, each taken at
//! row `min(2r + i, data_rows(k - 1) - 1)` and column
//! `(2c + j) mod wrap_cols(k - 1)`, where `data_rows(k)` is the number of
//! rows of level `k` that touch the globe and `wrap_cols(k)` the number of
//! columns around it. The mean is computed in integer arithmetic and rounded
//! half away from zero ([`mean4`]). The grid requires `cols` to be divisible
//! by `2^(top - 1)`, so every level a coarser one is built from has a whole
//! number of columns around the globe and the wrap is exact.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use core::ops::RangeInclusive;

use crate::tile::TileKey;

/// Samples along each edge of a tile.
pub const TILE_SIZE: u32 = 240;

/// Samples in one tile.
pub const TILE_SAMPLES: usize = (TILE_SIZE * TILE_SIZE) as usize;

/// The largest number of rows a grid may have: tile rows and columns must
/// fit the 24 bits a tile id gives them.
pub const MAX_ROWS: u32 = 1 << 24;

/// The geometry of one dataset's pyramid. See the module documentation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Grid {
    rows: u32,
    cols: u32,
    top: u8,
}

/// A point on one level, in lattice sample coordinates: sample `(r, c)` is
/// centered on `y = r`, `x = c`. `y` grows southward from the north pole
/// and `x` eastward from 180 degrees west.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Position {
    /// The level.
    pub level: u8,
    /// Row coordinate, `-0.5` at the north pole.
    pub y: f64,
    /// Column coordinate, `-0.5` at 180 degrees west.
    pub x: f64,
}

impl Grid {
    /// The cut dataset: the GEBCO global grid, 43200 rows by 86400 columns
    /// at 15 arcseconds, nine levels (0 to 8).
    pub const GEBCO: Grid = match Grid::new(43200, 86400) {
        Some(g) => g,
        None => panic!("the dataset grid is valid"),
    };

    /// A grid of `rows` by `cols` data samples, or `None` unless `cols` is
    /// `2 * rows`, `rows` is 1 to [`MAX_ROWS`], and `cols` is divisible by
    /// `2^(top - 1)` (module documentation, values).
    pub const fn new(rows: u32, cols: u32) -> Option<Grid> {
        if rows == 0 || rows > MAX_ROWS || cols as u64 != 2 * rows as u64 {
            return None;
        }
        // With cols = 2 rows, a level fits 2 by 1 tiles exactly when one
        // tile row covers all rows.
        let mut top: u8 = 0;
        while (rows as u64) > (TILE_SIZE as u64) << top {
            top += 1;
        }
        if top > 0 && !cols.is_multiple_of(1u32 << (top - 1)) {
            return None;
        }
        Some(Grid { rows, cols, top })
    }

    /// Data rows.
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    /// Data columns.
    pub const fn cols(&self) -> u32 {
        self.cols
    }

    /// The coarsest level: the first whose tiles cover the globe in at most
    /// 2 by 1 tiles.
    pub const fn top_level(&self) -> u8 {
        self.top
    }

    /// Every level, finest first.
    pub fn levels(&self) -> RangeInclusive<u8> {
        0..=self.top
    }

    /// Rows of tiles on `level`.
    pub const fn tile_rows(&self, level: u8) -> u32 {
        div_ceil(self.rows as u64, (TILE_SIZE as u64) << level) as u32
    }

    /// Columns of tiles on `level`.
    pub const fn tile_cols(&self, level: u8) -> u32 {
        div_ceil(self.cols as u64, (TILE_SIZE as u64) << level) as u32
    }

    /// Tiles on `level`.
    pub const fn tile_count(&self, level: u8) -> u64 {
        self.tile_rows(level) as u64 * self.tile_cols(level) as u64
    }

    /// Lattice rows on `level`: tile rows times [`TILE_SIZE`].
    pub const fn lattice_rows(&self, level: u8) -> u32 {
        self.tile_rows(level) * TILE_SIZE
    }

    /// Lattice columns on `level`: tile columns times [`TILE_SIZE`].
    pub const fn lattice_cols(&self, level: u8) -> u32 {
        self.tile_cols(level) * TILE_SIZE
    }

    /// Rows of `level` that touch the globe: `ceil(rows / 2^level)`. The
    /// last one may straddle the south pole. Later rows lie past it.
    pub const fn data_rows(&self, level: u8) -> u32 {
        div_ceil(self.rows as u64, 1u64 << level) as u32
    }

    /// Whole columns of `level` around the globe: `floor(cols / 2^level)`.
    /// Exact on every level below the top. On the top level 360 degrees may
    /// not be a whole number of samples; see [`Grid::lattice_col`].
    pub const fn wrap_cols(&self, level: u8) -> u32 {
        self.cols >> level
    }

    /// Sample spacing on `level`, radians.
    pub fn spacing(&self, level: u8) -> f64 {
        PI / f64::from(self.rows) * pow2(level)
    }

    /// Samples per radian on `level`.
    pub fn samples_per_radian(&self, level: u8) -> f64 {
        f64::from(self.rows) / PI / pow2(level)
    }

    /// Where geodetic latitude `lat` and longitude `lon` (radians) fall on
    /// `level`. Latitude is clamped to the poles; longitude may be any
    /// finite value and is wrapped into `[-pi, pi)`.
    pub fn position(&self, level: u8, lat: f64, lon: f64) -> Position {
        let k = self.samples_per_radian(level);
        let lat = lat.clamp(-FRAC_PI_2, FRAC_PI_2);
        let lon = wrap_lon(lon);
        Position {
            level,
            y: (FRAC_PI_2 - lat) * k - 0.5,
            x: (lon + PI) * k - 0.5,
        }
    }

    /// The latitude and longitude (radians) of the center of lattice sample
    /// `(r, c)` on `level`. The inverse of [`Grid::position`] at integer
    /// coordinates, up to rounding.
    pub fn center(&self, level: u8, r: u32, c: u32) -> (f64, f64) {
        let s = self.spacing(level);
        (
            FRAC_PI_2 - (f64::from(r) + 0.5) * s,
            -PI + (f64::from(c) + 0.5) * s,
        )
    }

    /// The lattice row an interpolator reads for row index `r` of `level`:
    /// clamped to the rows that touch the globe, `0` to
    /// `data_rows(level) - 1`, so a stencil reaching past a pole repeats the
    /// row at the pole.
    pub fn lattice_row(&self, level: u8, r: i64) -> u32 {
        r.clamp(0, i64::from(self.data_rows(level)) - 1) as u32
    }

    /// The lattice column an interpolator reads for column index `c` of
    /// `level`: unchanged inside the lattice, else moved by
    /// [`Grid::wrap_cols`] until it is inside. On every level below the top
    /// this is exactly the column at the same longitude. On a top level
    /// whose globe is not a whole number of samples it is within half a
    /// sample of it.
    pub fn lattice_col(&self, level: u8, c: i64) -> u32 {
        let period = i64::from(self.wrap_cols(level).max(1));
        let width = i64::from(self.lattice_cols(level));
        let mut c = c;
        while c < 0 {
            c += period;
        }
        while c >= width {
            c -= period;
        }
        c as u32
    }

    /// The tile holding lattice sample `(r, c)` of `level` and the sample's
    /// index inside it (`row * TILE_SIZE + col`, north row first, west
    /// column first).
    pub fn locate(&self, level: u8, r: u32, c: u32) -> (TileKey, usize) {
        let key = TileKey {
            level,
            row: r / TILE_SIZE,
            col: c / TILE_SIZE,
        };
        let index = (r % TILE_SIZE) as usize * TILE_SIZE as usize + (c % TILE_SIZE) as usize;
        (key, index)
    }

    /// The data row lattice row `r` of level 0 holds (module documentation,
    /// values).
    pub fn data_row(&self, r: u32) -> u32 {
        r.min(self.rows - 1)
    }

    /// The data column lattice column `c` of level 0 holds.
    pub fn data_col(&self, c: u32) -> u32 {
        c % self.cols
    }

    /// The row of level `level - 1` that sample row `r` of `level` reads for
    /// `i` in `{0, 1}`. `level` must be at least 1.
    pub fn finer_row(&self, level: u8, r: u32, i: u32) -> u32 {
        (2 * r + i).min(self.data_rows(level - 1) - 1)
    }

    /// The column of level `level - 1` that sample column `c` of `level`
    /// reads for `j` in `{0, 1}`. `level` must be at least 1.
    pub fn finer_col(&self, level: u8, c: u32, j: u32) -> u32 {
        (2 * c + j) % self.wrap_cols(level - 1)
    }

    /// The ids of the tiles [`crate::bicubic`] reads at latitude `lat` and
    /// longitude `lon` (radians) on `level`, ascending and unique.
    pub fn bicubic_tiles(&self, level: u8, lat: f64, lon: f64) -> Vec<u64> {
        let p = self.position(level, lat, lon);
        let (r0, c0) = (p.y.floor() as i64, p.x.floor() as i64);
        self.tiles_for_indices(level, (r0 - 1)..=(r0 + 2), (c0 - 1)..=(c0 + 2))
    }

    /// The ids of every tile [`crate::bicubic`] or [`crate::nearest`] may
    /// read anywhere in the box from `lat_min` to `lat_max` and `lon_min` to
    /// `lon_max` (radians) on `level`, ascending and unique. The longitude
    /// range is taken as given, so a box across 180 degrees is written with
    /// `lon_max > pi` or `lon_min < -pi`; a range of `2 pi` or more is the
    /// whole globe.
    pub fn tiles_for_region(
        &self,
        level: u8,
        lat_min: f64,
        lat_max: f64,
        lon_min: f64,
        lon_max: f64,
    ) -> Vec<u64> {
        let k = self.samples_per_radian(level);
        let lat_lo = lat_min.min(lat_max).clamp(-FRAC_PI_2, FRAC_PI_2);
        let lat_hi = lat_min.max(lat_max).clamp(-FRAC_PI_2, FRAC_PI_2);
        let r_lo = ((FRAC_PI_2 - lat_hi) * k - 0.5).floor() as i64 - 1;
        let r_hi = ((FRAC_PI_2 - lat_lo) * k - 0.5).floor() as i64 + 2;
        let (lon_lo, lon_hi) = (lon_min.min(lon_max), lon_min.max(lon_max));
        let cols = if lon_hi - lon_lo >= TAU {
            -1..=i64::from(self.lattice_cols(level))
        } else {
            let start = wrap_lon(lon_lo);
            let end = start + (lon_hi - lon_lo);
            let c_lo = ((start + PI) * k - 0.5).floor() as i64 - 1;
            let c_hi = ((end + PI) * k - 0.5).floor() as i64 + 2;
            c_lo..=c_hi
        };
        self.tiles_for_indices(level, r_lo..=r_hi, cols)
    }

    fn tiles_for_indices(
        &self,
        level: u8,
        rows: RangeInclusive<i64>,
        cols: RangeInclusive<i64>,
    ) -> Vec<u64> {
        let mut tile_rows: Vec<u32> = rows
            .map(|r| self.lattice_row(level, r) / TILE_SIZE)
            .collect();
        tile_rows.sort_unstable();
        tile_rows.dedup();
        // A column past the antimeridian has two lattice columns: the one
        // [`Grid::lattice_col`] gives for this index, and the one it gives
        // for the same longitude counted from the other side (a reader that
        // starts west of the antimeridian lands in the padding columns of
        // the last tile, one that starts east of it in the first tile).
        // List both.
        let period = i64::from(self.wrap_cols(level).max(1));
        let mut tile_cols: Vec<u32> = cols
            .flat_map(|c| {
                [
                    self.lattice_col(level, c) / TILE_SIZE,
                    self.lattice_col(level, c.rem_euclid(period)) / TILE_SIZE,
                ]
            })
            .collect();
        tile_cols.sort_unstable();
        tile_cols.dedup();
        let mut ids = Vec::with_capacity(tile_rows.len() * tile_cols.len());
        for &row in &tile_rows {
            for &col in &tile_cols {
                ids.push(TileKey { level, row, col }.id());
            }
        }
        // Rows vary in bits 24 to 47, columns below them, so row-major
        // order is already ascending.
        ids
    }
}

/// The mean of four samples in integer arithmetic, rounded half away from
/// zero.
pub const fn mean4(a: i16, b: i16, c: i16, d: i16) -> i16 {
    let s = a as i32 + b as i32 + c as i32 + d as i32;
    let m = if s >= 0 { (s + 2) / 4 } else { -((-s + 2) / 4) };
    m as i16
}

/// `lon` wrapped into `[-pi, pi)`, give or take rounding at the ends.
fn wrap_lon(lon: f64) -> f64 {
    if (-PI..PI).contains(&lon) {
        lon
    } else {
        lon - TAU * ((lon + PI) / TAU).floor()
    }
}

/// `2^level` as `f64`, exactly.
fn pow2(level: u8) -> f64 {
    f64::from_bits((1023 + u64::from(level)) << 52)
}

const fn div_ceil(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_levels() {
        let g = Grid::GEBCO;
        assert_eq!(g.top_level(), 8);
        assert_eq!((g.tile_rows(0), g.tile_cols(0)), (180, 360));
        assert_eq!((g.tile_rows(3), g.tile_cols(3)), (23, 45));
        assert_eq!((g.tile_rows(7), g.tile_cols(7)), (2, 3));
        assert_eq!((g.tile_rows(8), g.tile_cols(8)), (1, 2));
        assert_eq!(g.data_rows(7), 338);
        assert_eq!(g.wrap_cols(7), 675);
        let total: u64 = g.levels().map(|l| g.tile_count(l)).sum();
        assert_eq!(total, 64800 + 16200 + 4050 + 1035 + 276 + 72 + 18 + 6 + 2);
        // One level 0 tile is one degree.
        let deg = g.spacing(0) * f64::from(TILE_SIZE) * 180.0 / PI;
        assert!((deg - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_grids() {
        assert!(Grid::new(0, 0).is_none());
        assert!(Grid::new(100, 300).is_none());
        // 541 rows: top 2, needs cols divisible by 2.
        assert!(Grid::new(541, 1082).is_some());
        // 961 rows: top 3, cols 1922 not divisible by 4.
        assert!(Grid::new(961, 1922).is_none());
    }

    #[test]
    fn rounding_half_away_from_zero() {
        assert_eq!(mean4(1, 1, 0, 0), 1);
        assert_eq!(mean4(-1, -1, 0, 0), -1);
        assert_eq!(mean4(1, 0, 0, 0), 0);
        assert_eq!(mean4(-1, 0, 0, 0), 0);
        assert_eq!(mean4(3, 0, 0, 0), 1);
        assert_eq!(mean4(-3, 0, 0, 0), -1);
        assert_eq!(mean4(i16::MAX, i16::MAX, i16::MAX, i16::MAX), i16::MAX);
        assert_eq!(mean4(i16::MIN, i16::MIN, i16::MIN, i16::MIN), i16::MIN);
    }

    #[test]
    fn positions_and_centers() {
        let g = Grid::GEBCO;
        for level in g.levels() {
            let (lat, lon) = g.center(level, 17, 33);
            let p = g.position(level, lat, lon);
            assert!((p.y - 17.0).abs() < 1e-6 && (p.x - 33.0).abs() < 1e-6);
        }
        let p = g.position(0, FRAC_PI_2, -PI);
        assert_eq!((p.y, p.x), (-0.5, -0.5));
        let p = g.position(0, 0.0, 2.0 * PI);
        assert!((p.x - 43199.5).abs() < 1e-6);
    }

    #[test]
    fn wrapping_and_clamping() {
        let g = Grid::GEBCO;
        assert_eq!(g.lattice_col(0, -1), 86399);
        assert_eq!(g.lattice_col(0, 86400), 0);
        assert_eq!(g.lattice_col(5, 2700), 2700); // inside the lattice
        assert_eq!(g.lattice_col(8, -1), 336);
        assert_eq!(g.lattice_row(0, -2), 0);
        assert_eq!(g.lattice_row(8, 200), 168);
        assert_eq!(g.finer_col(1, 43199, 1), 86399);
        assert_eq!(g.finer_col(1, 43200, 1), 1);
        assert_eq!(g.finer_row(8, 168, 1), 337);
        assert_eq!(g.finer_row(8, 200, 0), 337);
    }

    #[test]
    fn region_tiles() {
        let g = Grid::GEBCO;
        let d = PI / 180.0;
        // A box inside one level 0 tile still needs its neighbors' edges
        // only when it nears them.
        let ids = g.tiles_for_region(0, 10.2 * d, 10.8 * d, 20.2 * d, 20.8 * d);
        assert_eq!(
            ids,
            vec![TileKey {
                level: 0,
                row: 79,
                col: 200
            }
            .id()]
        );
        // Across the antimeridian.
        let ids = g.tiles_for_region(0, 0.5 * d, 0.5 * d, 179.5 * d, 180.5 * d);
        let cols: Vec<u32> = ids
            .iter()
            .map(|&i| TileKey::from_id(i).unwrap().col)
            .collect();
        assert_eq!(cols, vec![0, 359]);
        // The whole globe at the top level.
        assert_eq!(g.tiles_for_region(8, -2.0, 2.0, -4.0, 4.0).len(), 2);
        // The point stencil at a sample center away from tile edges.
        assert_eq!(g.bicubic_tiles(0, 10.5 * d, 20.5 * d).len(), 1);
    }
}
