//! The chunk compiler: one chunk key and its tiles in, one validated
//! section out.
//!
//! Implements `space-model.md` section 11 item 2 for one cell, in the
//! matter format of `matter-format.md` sections 3 and 5 (matter format
//! version 3, with occupancy):
//!
//! - `registry`: the empty registry (section 5.3). The module declares no
//!   frames; the system module declares the frame this one fills.
//! - `frame-{id}`: the empty frame record (section 11.2), for the same
//!   reason.
//! - A cell: [`crate::plan::plan`] says what it needs, given the frame the
//!   layer's frame record describes ([`Frame`]). An empty plan gives
//!   an empty section (section 3.4). Otherwise the cell is sampled at
//!   [`RESOLUTION`] per axis, each sample averaging
//!   `SUBSAMPLES^3` stratified points exactly as the system module samples
//!   its balls: the cell is divided into `(SUBSAMPLES * RESOLUTION)^3` equal
//!   boxes and each box contributes its center.
//!
//! # A point
//!
//! A point is solid when its height above the ellipsoid is at or below the
//! elevation surface there, vacuum above. In order:
//!
//! 1. Its ellipsoidal radius below [`Q_SOLID`]: solid (more than 11 km
//!    deep). Above [`Q_VACUUM`]: vacuum (more than 9 km high). Neither
//!    needs the geodetic conversion.
//! 2. Otherwise its geodetic latitude, longitude, and height
//!    ([`crate::geodesy`]). Above [`H_MAX`]: vacuum; below [`H_MIN`]:
//!    solid.
//! 3. Otherwise solid when the height is at most the bicubic elevation at
//!    its latitude and longitude on the plan's level
//!    ([`Surface::clamped_elevation`]).
//!
//! Points are classified this way in the samples the surface may cross
//! (below). A solid point carries the interior density and temperature at
//! its model radius ([`crate::interior`]) and the albedo [`LAND_ALBEDO`] when its
//! height is at or above 0 (the ellipsoid, standing in for sea level), else
//! [`SEAFLOOR_ALBEDO`]. Points of an interior cell are all solid with the
//! seafloor albedo, as step 1 would find.
//!
//! # A sample
//!
//! Occupancy (`matter-format.md` section 3.3) is the share of the sample
//! below the elevation surface: a linear ramp of the height of the
//! sample's center below the surface, twice as wide as the sample reaches
//! along the vertical (the ellipsoid normal's spread over the cell axes),
//! clamped to 0 and 1. Both ends of any grid edge the surface crosses lie
//! on the ramp, so occupancy is linear in the distance to the surface
//! there and its 0.5 level, the format's one definition of the surface,
//! lies on the elevation surface between sample centers. Samples of a cell
//! with no surface listed (an interior cell) have occupancy 1.
//!
//! Density is the occupancy times the mean density the sample would have
//! filled with rock (the interior density at its `SUBSAMPLES^3` points).
//! Temperature and albedo are density-weighted means over the points
//! counted as rock. The radiating temperature (`matter-format.md` section
//! 3.3) is the temperature at the model radius of the sub-cube's farthest
//! corner: opaque matter radiates from its outer side, so a coarse sample
//! that reaches the surface shows the surface temperature, not the mean of
//! the hot rock it holds. State is solid, roughness [`ROUGHNESS`],
//! attenuation 0 (opaque). A cell whose samples are all vacuum is an empty
//! section.
//!
//! The occupancy comes first, from the center, and says how much of the
//! sample the points need:
//!
//! - **0** (the center more than half the ramp above the surface, or a
//!   share too small for the stored `f32`): vacuum. Its points are not
//!   visited.
//! - **Between 0 and 1**: each point is rock or vacuum as "A point" says.
//!   If no point is rock, the sample takes the point values at its center
//!   with the land albedo.
//! - **1** (the center more than half the ramp below the surface): every
//!   point is rock and no elevation is read. A point's height decides its
//!   albedo, and it is computed only for samples that may reach the
//!   ellipsoid; the others are land or seafloor throughout.
//!
//! # Determinism
//!
//! Points are visited in sample index order and, inside a sample, z, then
//! y, then x; sums run in that order in `f64` with plain operators. The
//! tiles are decoded once per call into a `BTreeMap`. The surface's caches
//! are hash maps that are only looked up, never iterated, and hold values
//! that do not depend on when they were computed. Nothing depends on time,
//! randomness, or the environment.

use std::collections::BTreeMap;

use gx_core::error::ValidationError;
use gx_core::frame_record::{self, FrameRecord};
use gx_core::key::{CellGeometry, CellKey, ChunkKey};
use gx_core::matter::{self, Compression, Sample, Samples, Section, State};
use gx_core::registry::{self, Registry};
use gx_core::units::{Attenuation, Density, Kelvin, Ratio};
use terrain_tiles::{Presence, Tile, TileError};

use crate::frame::{Frame, RESOLUTION, SUBSAMPLES};
use crate::geodesy::{atan2, latitude_height, A, B, H_MAX, H_MIN, Q_SOLID, Q_VACUUM};
use crate::interior::{self, SURFACE_RADIUS};
use crate::plan::{self, Plan, GRID};
use crate::surface::Surface;

/// Albedo of solid matter at or above the ellipsoid, three bands long to
/// short wavelength.
pub const LAND_ALBEDO: [f64; 3] = [0.3, 0.28, 0.25];

/// Albedo of solid matter below the ellipsoid.
pub const SEAFLOOR_ALBEDO: [f64; 3] = [0.2, 0.2, 0.22];

/// Roughness of every solid sample.
pub const ROUGHNESS: f64 = 0.9;

/// Why a key could not be compiled.
#[derive(Clone, Debug, PartialEq)]
pub enum CompileError {
    /// A tile did not decode, or one the plan listed is missing.
    Tile(TileError),
    /// The section breaks a rule of the matter format.
    Invalid(ValidationError),
}

impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CompileError::Tile(e) => write!(f, "{e}"),
            CompileError::Invalid(e) => write!(f, "section is invalid: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<TileError> for CompileError {
    fn from(e: TileError) -> Self {
        CompileError::Tile(e)
    }
}

impl From<ValidationError> for CompileError {
    fn from(e: ValidationError) -> Self {
        CompileError::Invalid(e)
    }
}

/// The source tile ids a key needs, ascending and unique: none for the
/// registry and a frame key, the plan's tiles for a cell.
pub fn sources(frame: &Frame, key: &ChunkKey) -> Vec<u64> {
    sources_with(frame, key, plan::presence())
}

/// [`sources`] over a source set whose fine tiles are `presence`.
pub fn sources_with(frame: &Frame, key: &ChunkKey, presence: &Presence) -> Vec<u64> {
    match key {
        ChunkKey::Registry | ChunkKey::Frame(_) => Vec::new(),
        ChunkKey::Cell(cell) => plan::sources_with(frame, cell, presence),
        ChunkKey::Surface(tile) => crate::surface_tile::sources_with(tile, presence),
    }
}

/// Decodes the tiles passed to the module, each checked against its id.
pub fn decode_tiles<'a>(
    tiles: impl IntoIterator<Item = (u64, &'a [u8])>,
) -> Result<BTreeMap<u64, Tile>, TileError> {
    let mut out = BTreeMap::new();
    for (id, bytes) in tiles {
        out.insert(id, Tile::decode_id(id, bytes)?);
    }
    Ok(out)
}

/// Compiles one key to its section bytes, validated with the library
/// validator. `tiles` must hold the tiles [`sources`] lists for the key.
pub fn compile(
    frame: &Frame,
    key: &ChunkKey,
    tiles: &BTreeMap<u64, Tile>,
) -> Result<Vec<u8>, CompileError> {
    compile_with(frame, key, tiles, plan::presence())
}

/// [`compile`] over a source set whose fine tiles are `presence`.
pub fn compile_with(
    frame: &Frame,
    key: &ChunkKey,
    tiles: &BTreeMap<u64, Tile>,
    presence: &Presence,
) -> Result<Vec<u8>, CompileError> {
    let bytes = match key {
        ChunkKey::Registry => registry::encode(&Registry::empty()),
        ChunkKey::Frame(id) => frame_record::encode(&FrameRecord::empty(*id)),
        ChunkKey::Cell(cell) => matter::encode(
            &section_with(frame, cell, tiles, presence)?,
            Compression::Zstd,
        ),
        ChunkKey::Surface(key) => match crate::surface_tile::tile_with(key, tiles, presence)? {
            Some(t) => gx_core::surface::encode(&t, Compression::Zstd),
            None => gx_core::surface::encode_empty(key),
        },
    };
    gx_core::validate(&key.to_string(), &bytes)?;
    Ok(bytes)
}

/// The section of one cell, before encoding. See the module documentation.
pub fn section(
    frame: &Frame,
    key: &CellKey,
    tiles: &BTreeMap<u64, Tile>,
) -> Result<Section, CompileError> {
    section_with(frame, key, tiles, plan::presence())
}

/// [`section`] over a source set whose fine tiles are `presence`.
pub fn section_with(
    frame: &Frame,
    key: &CellKey,
    tiles: &BTreeMap<u64, Tile>,
    presence: &Presence,
) -> Result<Section, CompileError> {
    let g = plan::geometry(frame, key);
    let samples = match plan::plan_with(frame, key, presence) {
        Plan::Empty => None,
        Plan::Interior => Some(sample_cell(&g, None)?),
        Plan::Surface { level, .. } => {
            let mut surface = Surface::new(GRID, level, tiles, presence);
            Some(sample_cell(&g, Some(&mut surface))?)
        }
    };
    match samples {
        Some(s) if (0..s.len()).any(|i| s.density(i).value() > 0.0) => {
            Ok(Section::new(*key, g.origin, g.edge, RESOLUTION, s)?)
        }
        _ => Ok(Section::empty(*key, g.origin, g.edge)?),
    }
}

/// The points of a cell: `SUBSAMPLES` per sample along each axis, the
/// centers of the `(SUBSAMPLES * RESOLUTION)^3` equal boxes.
struct Points {
    /// Points per sample along an axis.
    s: usize,
    /// Points per cell along an axis.
    m: usize,
    /// Frame coordinates of the points along each axis, meters.
    c: [Vec<f64>; 3],
    /// The squares of those coordinates over the ellipsoid's axes, so a
    /// point's squared ellipsoidal radius is a sum of three of them.
    q: [Vec<f64>; 3],
    /// Distance from the polar axis and longitude of every column of
    /// points, computed once the first time a column needs them.
    columns: Vec<Option<(f64, f64)>>,
}

impl Points {
    fn new(g: &CellGeometry) -> Points {
        let s = SUBSAMPLES as usize;
        let m = s * usize::from(RESOLUTION);
        let step = g.edge.value() / m as f64;
        let coords = |origin: f64| -> Vec<f64> {
            (0..m).map(|k| origin + step * (k as f64 + 0.5)).collect()
        };
        let c = [coords(g.origin.x), coords(g.origin.y), coords(g.origin.z)];
        let scaled_squares = |c: &[f64], axis: f64| -> Vec<f64> {
            c.iter()
                .map(|&v| {
                    let u = v / axis;
                    u * u
                })
                .collect()
        };
        let q = [
            scaled_squares(&c[0], A),
            scaled_squares(&c[1], A),
            scaled_squares(&c[2], B),
        ];
        Points {
            s,
            m,
            c,
            q,
            columns: vec![None; m * m],
        }
    }

    /// The squared ellipsoidal radius of point `(i, j, k)`.
    fn q2(&self, i: usize, j: usize, k: usize) -> f64 {
        self.q[0][i] + self.q[1][j] + self.q[2][k]
    }

    /// Distance from the polar axis and longitude of the column of points
    /// `(i, j)`.
    fn column(&mut self, i: usize, j: usize) -> (f64, f64) {
        let (px, py) = (self.c[0][i], self.c[1][j]);
        *self.columns[j * self.m + i]
            .get_or_insert_with(|| ((px * px + py * py).sqrt(), atan2(py, px)))
    }

    /// The point index ranges of sample `(x, y, z)`.
    fn ranges(&self, x: usize, y: usize, z: usize) -> [core::ops::Range<usize>; 3] {
        let s = self.s;
        [x * s..(x + 1) * s, y * s..(y + 1) * s, z * s..(z + 1) * s]
    }
}

/// Sums over the points of one sample, each in visiting order.
#[derive(Default)]
struct Sums {
    /// Interior density over every point: `SUBSAMPLES^3` times the mean
    /// density the sample would have filled with rock.
    full: f64,
    /// Interior density over the points counted as rock.
    rho: f64,
    /// Density times temperature over those points.
    rho_t: f64,
    /// Density times albedo over those points.
    rho_a: [f64; 3],
}

impl Sums {
    /// Counts a point of interior density `d` at model radius `r` as rock,
    /// land or seafloor.
    fn add_rock(&mut self, d: f64, r: f64, land: bool) {
        self.rho += d;
        self.rho_t += d * interior::temperature(r);
        let albedo = if land { LAND_ALBEDO } else { SEAFLOOR_ALBEDO };
        for (acc, a) in self.rho_a.iter_mut().zip(albedo) {
            *acc += d * a;
        }
    }
}

/// Which albedo the points of a sample that is all rock take.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Albedo {
    /// Every point is at or above the ellipsoid.
    Land,
    /// Every point is below the ellipsoid.
    Seafloor,
    /// Each point by its own height.
    ByHeight,
}

/// The albedo rule of an all-rock sample of edge `e` whose center is at
/// height `h` and whose ramp is `width` wide: its points' heights are
/// needed only if it may reach the ellipsoid. Its points lie within
/// `width / 4` (half the sample's reach along the vertical) of the
/// center's height; `slack` covers the ellipsoid's curvature over the
/// sample, the tilt between the normal the width is taken from and the
/// points' own, and rounding, each many times over.
fn rock_albedo(h: f64, width: f64, e: f64) -> Albedo {
    let reach = width / 4.0;
    let slack = 1.0 + 1e-4 * e + 1e-6 * e * e;
    if h - reach - slack >= 0.0 {
        Albedo::Land
    } else if h + reach + slack < 0.0 {
        Albedo::Seafloor
    } else {
        Albedo::ByHeight
    }
}

/// Sums a sample every point of which is rock: an interior cell's, or one
/// whose occupancy is 1 (no elevation is read). The height of a point is
/// computed only when `albedo` is [`Albedo::ByHeight`] and the point lies
/// in the elevation shell.
fn all_rock(pts: &mut Points, x: usize, y: usize, z: usize, albedo: Albedo) -> Sums {
    let (q_solid2, q_vacuum2) = (Q_SOLID * Q_SOLID, Q_VACUUM * Q_VACUUM);
    let mut sums = Sums::default();
    let [ri, rj, rk] = pts.ranges(x, y, z);
    for k in rk {
        for j in rj.clone() {
            for i in ri.clone() {
                let q2v = pts.q2(i, j, k);
                let r = SURFACE_RADIUS * q2v.sqrt();
                let d = interior::density(r);
                sums.full += d;
                let land = match albedo {
                    Albedo::Land => true,
                    Albedo::Seafloor => false,
                    Albedo::ByHeight if q2v < q_solid2 => false,
                    Albedo::ByHeight if q2v > q_vacuum2 => true,
                    Albedo::ByHeight => {
                        let (p, _) = pts.column(i, j);
                        latitude_height(p, pts.c[2][k]).1 >= 0.0
                    }
                };
                sums.add_rock(d, r, land);
            }
        }
    }
    sums
}

/// Sums a sample the elevation surface may cross: each point is rock or
/// vacuum as the module documentation says, its geodetic conversion done
/// once and used both for the elevation and for its albedo.
fn classified(
    pts: &mut Points,
    x: usize,
    y: usize,
    z: usize,
    sf: &mut Surface,
) -> Result<Sums, TileError> {
    let (q_solid2, q_vacuum2) = (Q_SOLID * Q_SOLID, Q_VACUUM * Q_VACUUM);
    let mut sums = Sums::default();
    let [ri, rj, rk] = pts.ranges(x, y, z);
    for k in rk {
        for j in rj.clone() {
            for i in ri.clone() {
                let q2v = pts.q2(i, j, k);
                let r = SURFACE_RADIUS * q2v.sqrt();
                let d = interior::density(r);
                sums.full += d;
                let rock = if q2v < q_solid2 {
                    Some(false)
                } else if q2v > q_vacuum2 {
                    None
                } else {
                    let (p, lon) = pts.column(i, j);
                    let (lat, h) = latitude_height(p, pts.c[2][k]);
                    if h > H_MAX {
                        None
                    } else if h < H_MIN {
                        Some(false)
                    } else if h <= sf.clamped_elevation(lat, lon)? {
                        Some(h >= 0.0)
                    } else {
                        None
                    }
                };
                if let Some(land) = rock {
                    sums.add_rock(d, r, land);
                }
            }
        }
    }
    Ok(sums)
}

/// Samples a cell. With no surface every point is solid (an interior
/// cell); with one, each sample's occupancy comes first, from its center,
/// and says how much of the sample its points need (module documentation).
fn sample_cell(g: &CellGeometry, mut surface: Option<&mut Surface>) -> Result<Samples, TileError> {
    let n = usize::from(RESOLUTION);
    let mut pts = Points::new(g);
    let total = f64::from(SUBSAMPLES * SUBSAMPLES * SUBSAMPLES);
    // Sample centers along each axis.
    let e = g.edge.value() / n as f64;
    let centers = |o: f64| -> Vec<f64> { (0..n).map(|i| o + e * (i as f64 + 0.5)).collect() };
    let (sx, sy, sz) = (
        centers(g.origin.x),
        centers(g.origin.y),
        centers(g.origin.z),
    );
    // Distance from the polar axis and longitude of every column of
    // sample centers.
    let columns: Vec<(f64, f64)> = match surface {
        None => Vec::new(),
        Some(_) => (0..n * n)
            .map(|c| {
                let (px, py) = (sx[c % n], sy[c / n]);
                ((px * px + py * py).sqrt(), atan2(py, px))
            })
            .collect(),
    };

    let mut out = Vec::with_capacity(n * n * n);
    for (z, &pz) in sz.iter().enumerate() {
        for (y, &py) in sy.iter().enumerate() {
            for (x, &px) in sx.iter().enumerate() {
                // The share of the sample below the surface: a linear ramp
                // of its center's height below the elevation surface, twice
                // as wide as the sample reaches along the vertical, so both
                // ends of any grid edge the surface crosses lie on it. A
                // surface crossing a row of samples then gives occupancies
                // linear in the distance to it, and their 0.5 level lies on
                // the surface between sample centers.
                let (fill, sums) = match surface.as_deref_mut() {
                    None => (1.0, all_rock(&mut pts, x, y, z, Albedo::Seafloor)),
                    Some(sf) => {
                        let (p, lon) = columns[y * n + x];
                        let (lat, h) = latitude_height(p, pz);
                        // The ellipsoid normal's spread over the axes.
                        let (ux, uy, uz) = (px / (A * A), py / (A * A), pz / (B * B));
                        let ul = (ux * ux + uy * uy + uz * uz).sqrt();
                        let width = 2.0 * e * (ux.abs() + uy.abs() + uz.abs()) / ul;
                        let fill = if h > H_MAX + width {
                            0.0
                        } else if h < H_MIN - width {
                            1.0
                        } else {
                            let elev = sf.clamped_elevation(lat, lon)?;
                            (0.5 + (elev - h) / width).clamp(0.0, 1.0)
                        };
                        if fill as f32 == 0.0 {
                            // More than half the ramp above the surface
                            // (or a share too small to store): vacuum,
                            // whatever its points hold.
                            out.push(Sample::VACUUM);
                            continue;
                        } else if fill < 1.0 {
                            (fill, classified(&mut pts, x, y, z, sf)?)
                        } else {
                            // More than half the ramp below the surface:
                            // all rock, no elevation read.
                            let albedo = rock_albedo(h, width, e);
                            (fill, all_rock(&mut pts, x, y, z, albedo))
                        }
                    }
                };
                let Sums {
                    full,
                    mut rho,
                    mut rho_t,
                    mut rho_a,
                } = sums;
                if rho == 0.0 {
                    // The center is within the ramp but no point counted:
                    // take the point values at the center.
                    let q = (px / A) * (px / A) + (py / A) * (py / A) + (pz / B) * (pz / B);
                    let r = SURFACE_RADIUS * q.sqrt();
                    let d = interior::density(r);
                    rho = d;
                    rho_t = d * interior::temperature(r);
                    rho_a = LAND_ALBEDO.map(|a| d * a);
                }
                // Opaque matter radiates from its outer side: the
                // temperature at the sub-cube's farthest model radius (the
                // surface value wherever the sub-cube reaches above the
                // ground).
                let far = |origin: f64, i: usize, axis: f64| -> f64 {
                    let lo = (origin + e * i as f64) / axis;
                    let hi = (origin + e * (i + 1) as f64) / axis;
                    (lo * lo).max(hi * hi)
                };
                let q_far = far(g.origin.x, x, A) + far(g.origin.y, y, A) + far(g.origin.z, z, B);
                let r_top = SURFACE_RADIUS * q_far.sqrt();
                out.push(Sample {
                    density: Density::new(fill * full / total),
                    state: State::Solid,
                    temperature: Kelvin::new(rho_t / rho),
                    radiating_temperature: Kelvin::new(interior::temperature(r_top)),
                    albedo: rho_a.map(|a| Ratio::new(a / rho)),
                    roughness: Ratio::new(ROUGHNESS),
                    attenuation: Attenuation::new(0.0),
                    occupancy: Ratio::new(fill),
                });
            }
        }
    }
    Ok(Samples::from_samples(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{frame, DEG};
    use crate::geodesy::{frame_point, Geodetic};
    use crate::plan::{cell_at, geometry};

    /// Where [`rock_albedo`] says every point of a sample is on one side of
    /// the ellipsoid, every point of the shell is, by its own height: cells
    /// across the ellipsoid at every depth from 3 to 21, at the equator,
    /// at mid latitude, and near a pole.
    #[test]
    fn rock_albedo_bound_holds_for_every_point() {
        let (mut sure, mut by_height) = (0, 0);
        for &(lat, lon) in &[(0.3, -0.3), (44.7, 120.2), (-89.2, 33.0)] {
            for depth in 3..=21u8 {
                let p = frame_point(Geodetic {
                    lat: lat * DEG,
                    lon: lon * DEG,
                    h: 0.0,
                });
                let key = cell_at(&frame(), depth, p).unwrap();
                let g = geometry(&frame(), &key);
                let mut pts = Points::new(&g);
                let n = usize::from(RESOLUTION);
                let e = g.edge.value() / n as f64;
                // Every fourth sample along each axis keeps it fast at
                // depths where the cell is mostly in the shell.
                for z in (0..n).step_by(4) {
                    for y in (0..n).step_by(4) {
                        for x in (0..n).step_by(4) {
                            let c = |o: f64, i: usize| o + e * (i as f64 + 0.5);
                            let (px, py, pz) =
                                (c(g.origin.x, x), c(g.origin.y, y), c(g.origin.z, z));
                            let (_, h) = latitude_height((px * px + py * py).sqrt(), pz);
                            let (ux, uy, uz) = (px / (A * A), py / (A * A), pz / (B * B));
                            let ul = (ux * ux + uy * uy + uz * uz).sqrt();
                            let width = 2.0 * e * (ux.abs() + uy.abs() + uz.abs()) / ul;
                            let rule = rock_albedo(h, width, e);
                            if rule == Albedo::ByHeight {
                                by_height += 1;
                                continue;
                            }
                            sure += 1;
                            let [ri, rj, rk] = pts.ranges(x, y, z);
                            for k in rk {
                                for j in rj.clone() {
                                    for i in ri.clone() {
                                        let q = pts.q2(i, j, k).sqrt();
                                        if q < Q_SOLID || q > Q_VACUUM {
                                            continue;
                                        }
                                        let (pp, _) = pts.column(i, j);
                                        let hp = latitude_height(pp, pts.c[2][k]).1;
                                        assert_eq!(
                                            hp >= 0.0,
                                            rule == Albedo::Land,
                                            "{key:?} sample {x} {y} {z} point {hp} m, center {h} m"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(sure > 1000 && by_height > 100, "{sure} {by_height}");
    }
}
