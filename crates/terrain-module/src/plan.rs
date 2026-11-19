//! What one cell needs: nothing, the interior profile alone, or the
//! elevation surface at one level of the tile pyramid and the tiles that
//! level holds for the cell.
//!
//! This is the `gx_sources` half of the module (`client-compute.md` section
//! 4.1): [`plan`] is a function of the chunk key and the frame's record
//! ([`Frame`]) alone, and the compiler
//! ([`crate::compile`]) follows the same plan, so the tiles it reads are
//! always among the ones the plan listed.
//!
//! # Classes
//!
//! The elevation surface lies between the heights
//! [`H_MIN`](crate::geodesy::H_MIN) and [`H_MAX`](crate::geodesy::H_MAX).
//! The cell cube is tested against that shell with its range of
//! ellipsoidal radius ([`crate::geodesy::q2`]), which is exact for a box:
//!
//! - [`Plan::Empty`]: the cube is entirely above `H_MAX`, or deeper than
//!   [`MAX_DEPTH`], or in another frame than the record's. No sources, an empty section.
//! - [`Plan::Interior`]: the cube is entirely below `H_MIN`. No sources;
//!   every sample is solid with the interior profile.
//! - [`Plan::Surface`]: the cube meets the shell. The level and the tiles
//!   below.
//!
//! # Level
//!
//! [`level_for_depth`]: the coarsest level whose sample spacing at the
//! equator is no larger than the cell's sample spacing, so a level never
//! has detail the samples cannot show by more than a factor of two, and
//! level 0 (the data's own spacing, about 29 m) from depth 15 on. Below
//! the data's spacing the surface is the smooth bicubic interpolation of
//! level 0: nothing is invented. Where a fine tile does not exist (open
//! ocean, `terrain_tiles::presence`) the surface reads the next coarser
//! level that does, down to level 4, GEBCO's own spacing.
//!
//! # Tiles
//!
//! The tiles the bicubic stencil may read anywhere in the cell's range of
//! geodetic latitude and longitude, from
//! [`terrain_tiles::Grid::tiles_for_region`]. The range is a bound computed
//! from the cube's corners:
//!
//! - Longitude: the angles of the four corners of the cube's projection on
//!   the equatorial plane, or every longitude if that rectangle holds the
//!   polar axis.
//! - Latitude: the geocentric latitude of a point of the cube is largest at
//!   the corner of `(p, z)` (distance from the axis, height above the
//!   equator) with the largest `z` and, for positive `z`, the smallest `p`;
//!   symmetrically for the smallest. Geodetic latitude has a tangent
//!   between [`TAN_RATIO`] times the geocentric one for every point of the
//!   shell, so the bound takes the wider of the two ratios on each side.
//!
//! Both are widened by [`ANGLE_MARGIN`] for rounding.

use core::f64::consts::{FRAC_PI_2, PI, TAU};

use gx_core::key::{CellGeometry, CellKey};
use gx_core::units::Meters;
use std::sync::OnceLock;

use terrain_tiles::{Grid, Presence};

use crate::frame::{Frame, MAX_DEPTH, RESOLUTION};
use crate::geodesy::{atan2, A, B, Q_SOLID, Q_VACUUM, TAN_RATIO};

/// The elevation grid the module reads: GLO-30 on land, GEBCO under the
/// oceans ([`Grid::GLO30`]).
pub const GRID: Grid = Grid::GLO30;

/// The presence map of the source set this module version reads
/// (`terrain_tiles::presence`), written by `terrain-tiler glo30-finish`.
/// It must be the map of the manifest the layer uses: the module asks only
/// for fine tiles it lists.
const PRESENCE_BYTES: &[u8] = include_bytes!("../data/glo30-presence.bin");

/// The decoded [`PRESENCE_BYTES`].
pub fn presence() -> &'static Presence {
    static P: OnceLock<Presence> = OnceLock::new();
    P.get_or_init(|| {
        Presence::decode(&GRID, PRESENCE_BYTES).expect("the embedded presence map is valid")
    })
}

/// Radians added on every side of a cell's latitude and longitude range,
/// about 6 mm on the surface: far more than the rounding of the bound and
/// of the per-point conversion, far less than a sample.
pub const ANGLE_MARGIN: f64 = 1e-9;

/// What one cell needs. See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No matter: an empty section, no sources.
    Empty,
    /// Every point is below the elevation surface: the interior profile
    /// alone, no sources.
    Interior,
    /// The cell meets the elevation shell.
    Surface {
        /// The pyramid level the surface is read from.
        level: u8,
        /// The tiles of that level the cell may read, ascending and
        /// unique.
        tiles: Vec<u64>,
    },
}

/// The cube of a cell in a frame of the record's `root_extent`: the cell's
/// own cube for a cell of the record's frame (see [`crate::frame`] for
/// other frames).
pub fn geometry(frame: &Frame, key: &CellKey) -> CellGeometry {
    key.geometry(Meters::new(frame.root_extent))
}

/// The plan of one cell, with the module's own presence map.
pub fn plan(frame: &Frame, key: &CellKey) -> Plan {
    plan_with(frame, key, presence())
}

/// The plan of one cell over a source set whose fine tiles are `presence`.
pub fn plan_with(frame: &Frame, key: &CellKey, presence: &Presence) -> Plan {
    if key.frame_id != frame.frame_id || key.depth > MAX_DEPTH {
        return Plan::Empty;
    }
    let b = Bounds::of(&geometry(frame, key));
    let (q2_min, q2_max) = b.q2_range();
    if q2_min > Q_VACUUM * Q_VACUUM {
        return Plan::Empty;
    }
    if q2_max < Q_SOLID * Q_SOLID {
        return Plan::Interior;
    }
    let level = level_for_depth(frame, key.depth);
    let (lat_lo, lat_hi) = b.latitude_range();
    let (lon_lo, lon_hi) = b.longitude_range();
    let tiles = presence.tiles_for_region(&GRID, level, lat_lo, lat_hi, lon_lo, lon_hi);
    Plan::Surface { level, tiles }
}

/// The tiles a cell needs: the plan's tiles, or none.
pub fn sources(frame: &Frame, key: &CellKey) -> Vec<u64> {
    sources_with(frame, key, presence())
}

/// [`sources`] over a source set whose fine tiles are `presence`.
pub fn sources_with(frame: &Frame, key: &CellKey, presence: &Presence) -> Vec<u64> {
    match plan_with(frame, key, presence) {
        Plan::Surface { tiles, .. } => tiles,
        _ => Vec::new(),
    }
}

/// The pyramid level a cell of `depth` reads (module documentation): the
/// largest level `k` with `A * spacing(0) * 2^k` at most the cell's sample
/// spacing `root_extent / 2^depth / RESOLUTION`, and 0 if none is.
pub fn level_for_depth(frame: &Frame, depth: u8) -> u8 {
    let spacing = frame.root_extent / (1u64 << depth) as f64 / f64::from(RESOLUTION);
    let base = A * GRID.spacing(0);
    let mut level = 0u8;
    while level < GRID.top_level() && base * (2u64 << level) as f64 <= spacing {
        level += 1;
    }
    level
}

/// The cell of the record's frame at `depth` that holds the frame point
/// `p` (meters), or `None` if `p` is outside the frame's cube.
pub fn cell_at(frame: &Frame, depth: u8, p: [f64; 3]) -> Option<CellKey> {
    let cells = 1u64 << depth;
    let edge = frame.root_extent / cells as f64;
    let index = |v: f64| -> Option<u32> {
        let i = ((v + frame.root_extent / 2.0) / edge).floor();
        (i >= 0.0 && i < cells as f64).then_some(i as u32)
    };
    Some(CellKey {
        frame_id: frame.frame_id,
        depth,
        x: index(p[0])?,
        y: index(p[1])?,
        z: index(p[2])?,
    })
}

/// The cube of a cell as per-axis intervals, meters.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Bounds {
    /// `[x0, x1]`.
    pub x: (f64, f64),
    /// `[y0, y1]`.
    pub y: (f64, f64),
    /// `[z0, z1]`.
    pub z: (f64, f64),
}

impl Bounds {
    /// The cube of a cell.
    pub fn of(g: &CellGeometry) -> Bounds {
        let e = g.edge.value();
        let o = g.origin;
        Bounds {
            x: (o.x, o.x + e),
            y: (o.y, o.y + e),
            z: (o.z, o.z + e),
        }
    }

    /// The smallest and largest squared ellipsoidal radius over the cube:
    /// the point of the scaled box nearest the center and its farthest
    /// corner.
    pub fn q2_range(&self) -> (f64, f64) {
        let axes = [(self.x, A), (self.y, A), (self.z, B)];
        let (mut lo, mut hi) = (0.0, 0.0);
        for ((a, b), s) in axes {
            let (u, v) = (a / s, b / s);
            let near = nearest_to_zero(u, v);
            let far = if u.abs() > v.abs() { u } else { v };
            lo += near * near;
            hi += far * far;
        }
        (lo, hi)
    }

    /// The smallest and largest distance from the polar axis over the
    /// cube.
    pub fn p_range(&self) -> (f64, f64) {
        let nx = nearest_to_zero(self.x.0, self.x.1);
        let ny = nearest_to_zero(self.y.0, self.y.1);
        let fx = self.x.0.abs().max(self.x.1.abs());
        let fy = self.y.0.abs().max(self.y.1.abs());
        ((nx * nx + ny * ny).sqrt(), (fx * fx + fy * fy).sqrt())
    }

    /// A range holding the geodetic latitude of every point of the cube
    /// within the elevation shell, radians (module documentation).
    pub fn latitude_range(&self) -> (f64, f64) {
        let (p_min, p_max) = self.p_range();
        let (z0, z1) = self.z;
        let (lo_ratio, hi_ratio) = TAN_RATIO;
        let hi = if z1 > 0.0 {
            if p_min == 0.0 {
                FRAC_PI_2
            } else {
                atan2(hi_ratio * z1, p_min)
            }
        } else {
            atan2(lo_ratio * z1, p_max)
        };
        let lo = if z0 < 0.0 {
            if p_min == 0.0 {
                -FRAC_PI_2
            } else {
                atan2(hi_ratio * z0, p_min)
            }
        } else {
            atan2(lo_ratio * z0, p_max)
        };
        (lo - ANGLE_MARGIN, hi + ANGLE_MARGIN)
    }

    /// A range holding the longitude of every point of the cube, radians,
    /// possibly reaching past `pi` or `-pi`; wider than `2 pi` when the
    /// cube holds part of the polar axis.
    pub fn longitude_range(&self) -> (f64, f64) {
        let (x0, x1) = self.x;
        let (y0, y1) = self.y;
        if x0 <= 0.0 && 0.0 <= x1 && y0 <= 0.0 && 0.0 <= y1 {
            return (-PI - 1.0, PI + 1.0);
        }
        // The rectangle does not hold the axis, so it spans less than pi
        // as seen from it and its extreme directions are corners.
        let a0 = atan2(y0, x0);
        let (mut lo, mut hi) = (a0, a0);
        for (x, y) in [(x1, y0), (x0, y1), (x1, y1)] {
            let mut a = atan2(y, x);
            if a - a0 > PI {
                a -= TAU;
            } else if a - a0 < -PI {
                a += TAU;
            }
            lo = lo.min(a);
            hi = hi.max(a);
        }
        (lo - ANGLE_MARGIN, hi + ANGLE_MARGIN)
    }
}

/// The value of `[a, b]` nearest zero.
fn nearest_to_zero(a: f64, b: f64) -> f64 {
    if b < 0.0 {
        b
    } else if a > 0.0 {
        a
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::frame;

    #[test]
    fn levels_by_depth() {
        let levels: Vec<u8> = (0..=21).map(|d| level_for_depth(&frame(), d)).collect();
        assert_eq!(
            levels,
            [10, 10, 10, 10, 10, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn classes() {
        let key = |d, x, y, z| CellKey {
            frame_id: 4,
            depth: d,
            x,
            y,
            z,
        };
        let plan = |k: &CellKey| plan(&frame(), k);
        // The cell holding the center at depth 8 is deep inside.
        assert_eq!(plan(&key(8, 128, 128, 128)), Plan::Interior);
        // A corner of the frame is empty.
        assert_eq!(plan(&key(4, 0, 0, 0)), Plan::Empty);
        // Too deep.
        assert_eq!(plan(&key(22, 0, 0, 0)), Plan::Empty);
        // Other frames, declared or not.
        for frame_id in [3, 99] {
            assert_eq!(
                plan(&CellKey {
                    frame_id,
                    ..key(2, 1, 1, 1)
                }),
                Plan::Empty
            );
        }
        // The whole frame needs the top level, both tiles.
        match plan(&key(0, 0, 0, 0)) {
            Plan::Surface { level: 10, tiles } => assert_eq!(tiles.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn longitude_across_the_antimeridian() {
        let b = Bounds {
            x: (-6.4e6, -6.3e6),
            y: (-1e4, 1e4),
            z: (0.0, 1e4),
        };
        let (lo, hi) = b.longitude_range();
        let crosses = (lo < -PI && hi > -PI) || (lo < PI && hi > PI);
        assert!(crosses && hi - lo < 0.01, "{lo} {hi}");
        let (la, lb) = b.latitude_range();
        assert!(
            la < 0.0 && la > -1e-8 && lb > 0.0 && lb < 0.002,
            "{la} {lb}"
        );
    }
}
