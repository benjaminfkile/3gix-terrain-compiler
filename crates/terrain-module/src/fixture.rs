//! Fixture tiles: a small synthetic world on the real tile grid, for the
//! tests and the conformance vectors.
//!
//! The module asks for tiles of the cut dataset's pyramid ([`GRID`]) by
//! their real ids. The fixtures fill any of those tiles from one function
//! of latitude and longitude, [`elevation`], sampled at each sample's
//! center and rounded to whole meters. Most of the globe is flat seafloor,
//! which compresses to a few hundred bytes per tile; the features sit near
//! latitude 0 and longitude 0, where the frame's `x` axis meets the
//! surface:
//!
//! - an island, 1.6 degrees square, at 300 m, with a coast that falls to
//!   the seafloor over 0.02 degrees ([`COAST`]);
//! - a summit of exactly [`SUMMIT_ELEVATION`] at the center of one level 0
//!   sample ([`summit`]), a smooth bump 0.04 degrees in radius;
//! - a smooth ramp across longitude 0, which is the edge between two
//!   columns of level 0 tiles, around latitude 0.5 degrees ([`RAMP`]);
//! - a trench to [`TRENCH_ELEVATION`] at latitude 0, longitude 2.5 degrees
//!   ([`TRENCH`]).
//!
//! Nothing here is part of the module's output path; the module itself
//! never sees these values except through the tiles a vector carries.

use core::f64::consts::PI;

use terrain_tiles::{Tile, TileKey};

use crate::plan::GRID;

/// One degree, radians.
pub const DEG: f64 = PI / 180.0;

/// The seafloor everywhere away from the features, meters.
pub const SEAFLOOR: f64 = -4000.0;

/// The island's plateau, meters.
pub const ISLAND: f64 = 300.0;

/// The island's half width and the width of its coast, degrees: land to
/// `ISLAND_HALF - COAST` from the center, seafloor beyond `ISLAND_HALF`.
pub const ISLAND_HALF: f64 = 0.8;

/// See [`ISLAND_HALF`].
pub const COAST: f64 = 0.02;

/// The summit's elevation, meters.
pub const SUMMIT_ELEVATION: i16 = 8000;

/// The summit's level 0 sample: `(row, col)`. Its center is at about
/// latitude 0.0479 and longitude 0.0521 degrees.
pub const SUMMIT_SAMPLE: (u32, u32) = (345416, 691400);

/// The summit bump's radius, degrees.
pub const SUMMIT_RADIUS: f64 = 0.04;

/// The ramp: center latitude, half width (degrees), and the elevation
/// gain per degree of longitude at its center, meters.
pub const RAMP: (f64, f64, f64) = (0.5, 0.05, 2000.0);

/// The trench: center latitude and longitude, radius (degrees).
pub const TRENCH: (f64, f64, f64) = (0.0, 2.5, 0.1);

/// The trench's deepest elevation, meters.
pub const TRENCH_ELEVATION: f64 = -10_900.0;

/// The summit's latitude and longitude, radians.
pub fn summit() -> (f64, f64) {
    GRID.center(0, SUMMIT_SAMPLE.0, SUMMIT_SAMPLE.1)
}

/// `(1 - u^2)^2` for `|u| < 1`, else 0: a bump with a continuous slope.
fn bump(u: f64) -> f64 {
    if u.abs() >= 1.0 {
        0.0
    } else {
        let v = 1.0 - u * u;
        v * v
    }
}

/// The fixture world's elevation at latitude `lat` and longitude `lon`
/// (radians), meters.
pub fn elevation(lat: f64, lon: f64) -> f64 {
    let (la, lo) = (lat / DEG, lon / DEG);
    // The island.
    let d = la.abs().max(lo.abs());
    let mut e = if d <= ISLAND_HALF - COAST {
        ISLAND
    } else if d < ISLAND_HALF {
        let t = (d - (ISLAND_HALF - COAST)) / COAST;
        ISLAND + t * (SEAFLOOR - ISLAND)
    } else {
        SEAFLOOR
    };
    // The summit, exactly SUMMIT_ELEVATION at its sample's center.
    let (sl, so) = summit();
    let (dl, dn) = ((lat - sl) / DEG, (lon - so) / DEG);
    let r = (dl * dl + dn * dn).sqrt() / SUMMIT_RADIUS;
    e += (f64::from(SUMMIT_ELEVATION) - ISLAND) * bump(r);
    // The ramp across longitude 0.
    let (rl, rw, gain) = RAMP;
    e += gain * lo * bump((la - rl) / rw) * bump(lo / rw);
    // The trench.
    let (tl, to, tr) = TRENCH;
    let (dl, dn) = (la - tl, lo - to);
    let r = (dl * dl + dn * dn).sqrt() / tr;
    e += (TRENCH_ELEVATION - SEAFLOOR) * bump(r);
    e
}

/// The fixture tile with id `id`: [`elevation`] at every sample's center,
/// rounded to the nearest meter. Panics on an id that is not a tile of
/// [`GRID`].
pub fn tile(id: u64) -> Tile {
    let key = TileKey::from_id(id).expect("a tile id");
    assert!(key.level <= GRID.top_level() && key.row < GRID.tile_rows(key.level));
    assert!(key.col < GRID.tile_cols(key.level));
    let n = GRID.tile_size();
    let mut samples = Vec::with_capacity(GRID.tile_samples());
    for i in 0..n {
        for j in 0..n {
            let (lat, lon) = GRID.center(key.level, key.row * n + i, key.col * n + j);
            let v = elevation(lat.max(-PI / 2.0), lon).round();
            samples.push(v.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16);
        }
    }
    Tile::new(key, samples).expect("a full tile")
}

/// The fixture tiles with these ids, as [`crate::compile::compile`] takes
/// them.
pub fn tiles(ids: &[u64]) -> std::collections::BTreeMap<u64, Tile> {
    ids.iter().map(|&id| (id, tile(id))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features() {
        let (sl, so) = summit();
        assert_eq!(elevation(sl, so), 8000.0);
        assert_eq!(elevation(0.3 * DEG, -0.3 * DEG), ISLAND);
        assert_eq!(elevation(10.0 * DEG, 10.0 * DEG), SEAFLOOR);
        assert!((elevation(0.0, 2.5 * DEG) - TRENCH_ELEVATION).abs() < 1e-6);
        let t = tile(
            TileKey {
                level: 0,
                row: SUMMIT_SAMPLE.0 / 960,
                col: SUMMIT_SAMPLE.1 / 960,
            }
            .id(),
        );
        assert_eq!(t.get(SUMMIT_SAMPLE.0 % 960, SUMMIT_SAMPLE.1 % 960), 8000);
        // Far tiles are flat and compress to almost nothing.
        let far = tile(
            TileKey {
                level: 0,
                row: 10,
                col: 10,
            }
            .id(),
        );
        assert!(far.encode().len() < 2000);
    }
}
