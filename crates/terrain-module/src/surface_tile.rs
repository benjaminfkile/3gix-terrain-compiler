//! Surface tiles of the frame (`matter-format.md` section 10).
//!
//! The same elevation surface the cells are built from, as heights above
//! the WGS84 ellipsoid on the tiles of `gx_core::surface`. Vertex `(i, j)`
//! of a tile is placed by the core library; its geodetic latitude and
//! longitude come from the ellipsoid's normal there, and its height is the
//! bicubic elevation of one pyramid level ([`level_for_tile`]): the
//! coarsest whose spacing is no larger than the tile's vertex spacing at
//! the equator. The parent heights are the same computation at the parent
//! tile's level, so they are bit for bit the parent tile's own heights at
//! those vertices. Heights below the data's spacing are the smooth
//! interpolation; nothing is invented.

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};

use gx_core::key::SurfaceKey;
use gx_core::surface::{
    ellipsoid_point, vertex_direction, SurfaceTile, HEIGHT_SIDE, PARENT_SIDE, TILE_QUADS,
    TILE_VERTICES,
};
use terrain_tiles::{Presence, Tile, TileError};

use crate::compile::{LAND_ALBEDO, ROUGHNESS, SEAFLOOR_ALBEDO};
use crate::geodesy::{atan2, A, B, H_MIN};
use crate::plan::{ANGLE_MARGIN, GRID};
use crate::surface::Surface;

/// Deepest surface level this module builds; deeper keys are empty. At
/// level 16 the vertex spacing is about 2.4 m, well below the data's.
pub const MAX_LEVEL: u8 = 16;

/// Radiating temperature of the surface, kelvin (the geotherm's surface
/// value, `crate::interior`).
pub const SURFACE_TEMPERATURE: f32 = 288.0;

/// The pyramid level a tile of `level` reads: the coarsest whose sample
/// spacing on the equator is no larger than the tile's vertex spacing
/// there (a quarter of the equator over `TILE_QUADS * 2^level`).
pub fn level_for_tile(level: u8) -> u8 {
    let spacing = FRAC_PI_2 * A / (TILE_QUADS as f64 * (1u64 << level) as f64);
    let base = A * GRID.spacing(0);
    let mut k = 0u8;
    while k < GRID.top_level() && base * (2u64 << k) as f64 <= spacing {
        k += 1;
    }
    k
}

/// Geodetic latitude and longitude (radians) of the tile vertex `(i, j)`.
fn lat_lon(key: &SurfaceKey, i: i64, j: i64) -> (f64, f64) {
    let (_, n) = ellipsoid_point(vertex_direction(key, i, j), A, B);
    let lat = atan2(n[2], (n[0] * n[0] + n[1] * n[1]).sqrt());
    (lat, atan2(n[1], n[0]))
}

/// Whether the tile holds a pole: a polar face whose tile covers the
/// face's center.
fn holds_pole(key: &SurfaceKey) -> bool {
    if key.face < 4 {
        return false;
    }
    let half = 1u32 << key.level >> 1;
    let covers = |t: u32| key.level == 0 || t == half || t + 1 == half;
    covers(key.x) && covers(key.y)
}

/// The latitude and longitude box of the tile's vertices and its parent
/// heights' vertices, borders included
/// (radians, longitudes unwrapped). A tile within `pole_margin` radians of
/// a pole reaches it and spans every longitude: the bicubic stencil there
/// reads rows across the pole, at other longitudes.
fn bounds(key: &SurfaceKey, pole_margin: f64) -> (f64, f64, f64, f64) {
    // The parent heights' border reaches one parent vertex (two of the
    // tile's) past the tile on every side.
    let last = TILE_QUADS as i64 + 2;
    let (lat0, lon0) = lat_lon(key, -2, -2);
    let (mut lat_lo, mut lat_hi, mut lon_lo, mut lon_hi) = (lat0, lat0, lon0, lon0);
    for j in -2..=last {
        for i in -2..=last {
            let (lat, mut lon) = lat_lon(key, i, j);
            if lon - lon0 > PI {
                lon -= 2.0 * PI;
            } else if lon - lon0 < -PI {
                lon += 2.0 * PI;
            }
            lat_lo = lat_lo.min(lat);
            lat_hi = lat_hi.max(lat);
            lon_lo = lon_lo.min(lon);
            lon_hi = lon_hi.max(lon);
        }
    }
    if holds_pole(key) || lat_hi + pole_margin >= FRAC_PI_2 || lat_lo - pole_margin <= -FRAC_PI_2 {
        let north = holds_pole(key) && key.face == 4 || lat_hi + pole_margin >= FRAC_PI_2;
        let south = holds_pole(key) && key.face == 5 || lat_lo - pole_margin <= -FRAC_PI_2;
        return (
            if south {
                -FRAC_PI_2
            } else {
                lat_lo - ANGLE_MARGIN
            },
            if north {
                FRAC_PI_2
            } else {
                lat_hi + ANGLE_MARGIN
            },
            -PI - 1.0,
            PI + 1.0,
        );
    }
    (
        lat_lo - ANGLE_MARGIN,
        lat_hi + ANGLE_MARGIN,
        lon_lo - ANGLE_MARGIN,
        lon_hi + ANGLE_MARGIN,
    )
}

/// The source tiles of a surface key: those of its level's pyramid level
/// and of its parent's, over the tile's box, ascending and unique. None for
/// levels past [`MAX_LEVEL`]. The caller checks the frame.
pub fn sources_with(key: &SurfaceKey, presence: &Presence) -> Vec<u64> {
    if key.level > MAX_LEVEL {
        return Vec::new();
    }
    let coarsest = level_for_tile(key.parent().map_or(key.level, |p| p.level));
    // Four samples of the coarsest level read, and of every coarser level
    // a fallback may read (missing fine tiles fall back level by level up
    // to the top level).
    let pole_margin = 4.0 * GRID.spacing(GRID.top_level().max(coarsest));
    let (la0, la1, lo0, lo1) = bounds(key, pole_margin);
    let mut ids = presence.tiles_for_region(&GRID, level_for_tile(key.level), la0, la1, lo0, lo1);
    if let Some(p) = key.parent() {
        ids.extend(presence.tiles_for_region(&GRID, level_for_tile(p.level), la0, la1, lo0, lo1));
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// One surface tile, or `None` for a tile with no surface (past
/// [`MAX_LEVEL`]). The caller checks the frame.
pub fn tile_with(
    key: &SurfaceKey,
    tiles: &BTreeMap<u64, Tile>,
    presence: &Presence,
) -> Result<Option<SurfaceTile>, TileError> {
    if key.level > MAX_LEVEL {
        return Ok(None);
    }
    let mut own = Surface::new(GRID, level_for_tile(key.level), tiles, presence);
    let mut heights = Vec::with_capacity(HEIGHT_SIDE * HEIGHT_SIDE);
    for j in -1..=(TILE_QUADS as i64 + 1) {
        for i in -1..=(TILE_QUADS as i64 + 1) {
            let (lat, lon) = lat_lon(key, i, j);
            heights.push(own.clamped_elevation(lat, lon)? as f32);
        }
    }
    let half = TILE_QUADS as i64 / 2;
    let mut parent_heights = Vec::with_capacity(PARENT_SIDE * PARENT_SIDE);
    match key.parent() {
        Some(p) => {
            let mut coarse = Surface::new(GRID, level_for_tile(p.level), tiles, presence);
            for m in -1..=(half + 1) {
                for k in -1..=(half + 1) {
                    let (lat, lon) = lat_lon(key, 2 * k, 2 * m);
                    parent_heights.push(coarse.clamped_elevation(lat, lon)? as f32);
                }
            }
        }
        None => {
            for m in -1..=(half + 1) {
                for k in -1..=(half + 1) {
                    let (lat, lon) = lat_lon(key, 2 * k, 2 * m);
                    parent_heights.push(own.clamped_elevation(lat, lon)? as f32);
                }
            }
        }
    }
    let byte = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let mut albedo = Vec::with_capacity(TILE_VERTICES * TILE_VERTICES);
    for j in 0..TILE_VERTICES {
        for i in 0..TILE_VERTICES {
            let h = heights[(i + 1) + HEIGHT_SIDE * (j + 1)];
            let a = if h >= 0.0 {
                LAND_ALBEDO
            } else {
                SEAFLOOR_ALBEDO
            };
            albedo.push(a.map(byte));
        }
    }
    let count = TILE_VERTICES * TILE_VERTICES;
    Ok(Some(SurfaceTile {
        key: *key,
        radius_equatorial: A,
        radius_polar: B,
        // Every height is clamped to [H_MIN, H_MAX]
        // (`Surface::clamped_elevation`), at every level.
        height_floor: H_MIN,
        heights,
        parent_heights,
        albedo,
        roughness: vec![byte(ROUGHNESS); count],
        temperature: vec![SURFACE_TEMPERATURE; count],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    #[test]
    fn levels_get_finer_with_the_tile_level() {
        let levels: Vec<u8> = (0..=MAX_LEVEL).map(level_for_tile).collect();
        assert_eq!(levels[0], GRID.top_level());
        assert_eq!(levels[MAX_LEVEL as usize], 0);
        assert!(levels.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn parent_heights_are_the_parents_own() {
        let all = Presence::all();
        let child = SurfaceKey::new(4, 0, 6, 33, 17).unwrap();
        let parent = child.parent().unwrap();
        let ids: Vec<u64> = sources_with(&child, &all)
            .into_iter()
            .chain(sources_with(&parent, &all))
            .collect();
        let tiles: BTreeMap<u64, Tile> = ids.iter().map(|&id| (id, fixture::tile(id))).collect();
        let c = tile_with(&child, &tiles, &all).unwrap().unwrap();
        let p = tile_with(&parent, &tiles, &all).unwrap().unwrap();
        // The child is the parent's (1, 1) quadrant: its even vertex
        // (2k, 2m) is the parent's vertex (32 + k, 32 + m).
        for m in 0..=32i64 {
            for k in 0..=32i64 {
                assert_eq!(
                    c.parent_height(k, m).to_bits(),
                    p.height(32 + k, 32 + m).to_bits(),
                    "vertex ({k}, {m})"
                );
            }
        }
    }

    #[test]
    fn tiles_declare_the_clamp_as_their_floor() {
        let all = Presence::all();
        let key = SurfaceKey::new(4, 1, 3, 2, 5).unwrap();
        let tiles: BTreeMap<u64, Tile> = sources_with(&key, &all)
            .into_iter()
            .map(|id| (id, fixture::tile(id)))
            .collect();
        let t = tile_with(&key, &tiles, &all).unwrap().unwrap();
        assert_eq!(t.height_floor, H_MIN);
        let bytes = gx_core::surface::encode(&t, gx_core::matter::Compression::Zstd);
        gx_core::surface::validate(&key, &bytes).unwrap();
    }

    #[test]
    fn neighbors_share_their_edge_heights() {
        let all = Presence::all();
        let left = SurfaceKey::new(4, 2, 5, 9, 4).unwrap();
        let right = SurfaceKey::new(4, 2, 5, 10, 4).unwrap();
        let ids: Vec<u64> = sources_with(&left, &all)
            .into_iter()
            .chain(sources_with(&right, &all))
            .collect();
        let tiles: BTreeMap<u64, Tile> = ids.iter().map(|&id| (id, fixture::tile(id))).collect();
        let l = tile_with(&left, &tiles, &all).unwrap().unwrap();
        let r = tile_with(&right, &tiles, &all).unwrap().unwrap();
        for j in -1..=65i64 {
            for d in -1..=1i64 {
                assert_eq!(l.height(64 + d, j).to_bits(), r.height(d, j).to_bits());
            }
        }
    }
}
