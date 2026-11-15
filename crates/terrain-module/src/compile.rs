//! The chunk compiler: one chunk key and its tiles in, one validated
//! section out.
//!
//! Implements `space-model.md` section 11 item 2 for one cell, in the
//! matter format of `matter-format.md` sections 3 and 5:
//!
//! - `registry`: the empty registry (section 5.3). The module declares no
//!   frames; the system module declares the frame this one fills.
//! - A cell: [`crate::plan::plan`] says what it needs. An empty plan gives
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
//! A solid point carries the interior density and temperature at its model
//! radius ([`crate::interior`]) and the albedo [`LAND_ALBEDO`] when its
//! height is at or above 0 (the ellipsoid, standing in for sea level), else
//! [`SEAFLOOR_ALBEDO`]. Points of an interior cell are all solid with the
//! seafloor albedo, as step 1 would find.
//!
//! # A sample
//!
//! Density is the sum of the solid points' densities divided by
//! `SUBSAMPLES^3`: the solid fraction times the local density. Temperature
//! and albedo are density-weighted means over the solid points. The
//! radiating temperature (`matter-format.md` section 3.3) is the
//! temperature at the model radius of the sub-cube's farthest corner:
//! opaque matter radiates from its outer side, so a coarse sample that
//! reaches the surface shows the surface temperature, not the mean of the
//! hot rock it holds. State is
//! solid, roughness [`ROUGHNESS`], attenuation 0 (opaque). A sample with no
//! solid point is vacuum. A cell whose samples are all vacuum is an empty
//! section.
//!
//! # Determinism
//!
//! Points are visited in sample index order and, inside a sample, z, then
//! y, then x; sums run in that order in `f64` with plain operators. The
//! tiles are decoded into a `BTreeMap`. Nothing depends on time,
//! randomness, or the environment.

use std::collections::BTreeMap;

use gx_core::error::ValidationError;
use gx_core::key::{CellGeometry, CellKey, ChunkKey};
use gx_core::matter::{self, Compression, Sample, Samples, Section, State};
use gx_core::registry::{self, Registry};
use gx_core::units::{Attenuation, Density, Kelvin, Ratio};
use terrain_tiles::{Tile, TileError};

use crate::frame::{RESOLUTION, SUBSAMPLES};
use crate::geodesy::{atan2, latitude_height, A, B, H_MAX, H_MIN, Q_SOLID, Q_VACUUM};
use crate::interior::{self, SURFACE_RADIUS};
use crate::plan::{self, Plan, PlanError, GRID};
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
    /// The cell cannot be planned.
    Plan(PlanError),
    /// A tile did not decode, or one the plan listed is missing.
    Tile(TileError),
    /// The section breaks a rule of the matter format.
    Invalid(ValidationError),
}

impl core::fmt::Display for CompileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CompileError::Plan(e) => write!(f, "{e}"),
            CompileError::Tile(e) => write!(f, "{e}"),
            CompileError::Invalid(e) => write!(f, "section is invalid: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<PlanError> for CompileError {
    fn from(e: PlanError) -> Self {
        CompileError::Plan(e)
    }
}

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
/// registry, the plan's tiles for a cell.
pub fn sources(key: &ChunkKey) -> Vec<u64> {
    match key {
        ChunkKey::Registry => Vec::new(),
        ChunkKey::Cell(cell) => plan::sources(cell),
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
pub fn compile(key: &ChunkKey, tiles: &BTreeMap<u64, Tile>) -> Result<Vec<u8>, CompileError> {
    let bytes = match key {
        ChunkKey::Registry => registry::encode(&Registry::empty()),
        ChunkKey::Cell(cell) => matter::encode(&section(cell, tiles)?, Compression::Zstd),
    };
    gx_core::validate(&key.to_string(), &bytes)?;
    Ok(bytes)
}

/// The section of one cell, before encoding. See the module documentation.
pub fn section(key: &CellKey, tiles: &BTreeMap<u64, Tile>) -> Result<Section, CompileError> {
    let g = plan::geometry(key)?;
    let samples = match plan::plan(key)? {
        Plan::Empty => None,
        Plan::Interior => Some(sample_cell(&g, None)?),
        Plan::Surface { level, .. } => {
            let mut surface = Surface::new(GRID, level, tiles);
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

/// What one point holds.
enum Point {
    Vacuum,
    Solid { land: bool },
}

/// Samples a cell. With no surface every point is solid (an interior
/// cell); with one, points are classified as the module documentation
/// says.
fn sample_cell(g: &CellGeometry, mut surface: Option<&mut Surface>) -> Result<Samples, TileError> {
    let n = usize::from(RESOLUTION);
    let s = SUBSAMPLES as usize;
    let m = s * n;
    let step = g.edge.value() / m as f64;
    let coords =
        |origin: f64| -> Vec<f64> { (0..m).map(|k| origin + step * (k as f64 + 0.5)).collect() };
    let (cx, cy, cz) = (coords(g.origin.x), coords(g.origin.y), coords(g.origin.z));
    let scaled_squares = |c: &[f64], axis: f64| -> Vec<f64> {
        c.iter()
            .map(|&v| {
                let u = v / axis;
                u * u
            })
            .collect()
    };
    let (qx, qy, qz) = (
        scaled_squares(&cx, A),
        scaled_squares(&cy, A),
        scaled_squares(&cz, B),
    );
    // Distance from the polar axis and longitude of every column of
    // points, computed once the first time a column needs them.
    let mut columns: Vec<Option<(f64, f64)>> = vec![None; m * m];
    let (q_solid2, q_vacuum2) = (Q_SOLID * Q_SOLID, Q_VACUUM * Q_VACUUM);
    let total = f64::from(SUBSAMPLES * SUBSAMPLES * SUBSAMPLES);

    let mut out = Vec::with_capacity(n * n * n);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let (mut rho, mut rho_t) = (0.0f64, 0.0f64);
                let mut rho_a = [0.0f64; 3];
                for k in z * s..(z + 1) * s {
                    for j in y * s..(y + 1) * s {
                        for i in x * s..(x + 1) * s {
                            let q2v = qx[i] + qy[j] + qz[k];
                            let point = match surface.as_deref_mut() {
                                None => Point::Solid { land: false },
                                Some(_) if q2v < q_solid2 => Point::Solid { land: false },
                                Some(_) if q2v > q_vacuum2 => Point::Vacuum,
                                Some(sf) => {
                                    let (p, lon) = *columns[j * m + i].get_or_insert_with(|| {
                                        let (px, py) = (cx[i], cy[j]);
                                        ((px * px + py * py).sqrt(), atan2(py, px))
                                    });
                                    let (lat, h) = latitude_height(p, cz[k]);
                                    if h > H_MAX {
                                        Point::Vacuum
                                    } else if h < H_MIN {
                                        Point::Solid { land: false }
                                    } else if h <= sf.clamped_elevation(lat, lon)? {
                                        Point::Solid { land: h >= 0.0 }
                                    } else {
                                        Point::Vacuum
                                    }
                                }
                            };
                            if let Point::Solid { land } = point {
                                let r = SURFACE_RADIUS * q2v.sqrt();
                                let d = interior::density(r);
                                rho += d;
                                rho_t += d * interior::temperature(r);
                                let albedo = if land { LAND_ALBEDO } else { SEAFLOOR_ALBEDO };
                                for (acc, a) in rho_a.iter_mut().zip(albedo) {
                                    *acc += d * a;
                                }
                            }
                        }
                    }
                }
                out.push(if rho > 0.0 {
                    // Opaque matter radiates from its outer side: the
                    // temperature at the sub-cube's farthest model radius
                    // (the surface value wherever the sub-cube reaches
                    // above the ground).
                    let e = g.edge.value() / n as f64;
                    let far = |origin: f64, i: usize, axis: f64| -> f64 {
                        let lo = (origin + e * i as f64) / axis;
                        let hi = (origin + e * (i + 1) as f64) / axis;
                        (lo * lo).max(hi * hi)
                    };
                    let q_far =
                        far(g.origin.x, x, A) + far(g.origin.y, y, A) + far(g.origin.z, z, B);
                    let r_top = SURFACE_RADIUS * q_far.sqrt();
                    Sample {
                        density: Density::new(rho / total),
                        state: State::Solid,
                        temperature: Kelvin::new(rho_t / rho),
                        radiating_temperature: Kelvin::new(interior::temperature(r_top)),
                        albedo: rho_a.map(|a| Ratio::new(a / rho)),
                        roughness: Ratio::new(ROUGHNESS),
                        attenuation: Attenuation::new(0.0),
                    }
                } else {
                    Sample::VACUUM
                });
            }
        }
    }
    Ok(Samples::from_samples(out))
}
