//! The interior below the elevation surface: density from the Preliminary
//! Reference Earth Model and temperature from a simple geotherm.
//!
//! Implements the "radial interior density profile below" of
//! `space-model.md` section 11 item 2.
//!
//! # Radius
//!
//! The reference model is spherical with surface radius [`SURFACE_RADIUS`]
//! (6371 km). The frame's matter is laid on the WGS84 ellipsoid, so the
//! module evaluates the model at `SURFACE_RADIUS * q`, where `q` is the
//! point's ellipsoidal radius ([`crate::geodesy::q2`], 1 on the
//! ellipsoid). Every model shell becomes an ellipsoid with the reference
//! ellipsoid's flattening; the ellipsoid holds the volume of a sphere of
//! radius 6371.0008 km, so the mass is the model's to a part in a million.
//!
//! # Density
//!
//! The piecewise polynomials of Dziewonski and Anderson (1981), Physics of
//! the Earth and Planetary Interiors 25, 297 to 356, table 1: density in
//! grams per cubic centimeter as a cubic in `x = r / 6371 km`, here
//! multiplied by 1000 to give kilograms per cubic meter. The model's
//! 3 km ocean layer (1.02 g/cm^3 from 6368 km) is replaced by its upper
//! crust (2.6 g/cm^3), which also continues above 6371 km: the module
//! builds rock up to the elevation surface wherever it is, and no water.
//! Each layer holds from its lower radius (inclusive) to its upper radius
//! (exclusive).
//!
//! # Temperature
//!
//! Piecewise linear in depth `d = SURFACE_RADIUS - r` between the points of
//! [`GEOTHERM`], held at the end values beyond them:
//!
//! | Depth | Temperature | Source |
//! |---|---|---|
//! | 0 km | 288 K | mean surface temperature |
//! | 100 km | 1600 K | base of the lithosphere, at the mantle potential temperature |
//! | 410 km | 1830 K | Katsura et al. (2010), Physics of the Earth and Planetary Interiors 183, 212 to 218 |
//! | 660 km | 1960 K | Katsura et al. (2010) |
//! | 2700 km | 2600 K | the lower mantle adiabat of Katsura et al. (2010), rounded |
//! | 2891 km | 4000 K | the core boundary of the reference model; Lay, Hernlund, and Buffett (2008), Nature Geoscience 1, 25 to 32 |
//! | 5150 km | 6230 K | the inner core boundary; Anzellini et al. (2013), Science 340, 464 to 466 |
//!
//! Below 100 km the values are rounded from the cited studies, whose own
//! uncertainties are tens to hundreds of kelvin. Above the reference
//! surface (land above the ellipsoid) the temperature is the surface value.
//! The steep rise from 2700 to 2891 km is the thermal boundary layer at the
//! base of the mantle; below the inner core boundary the temperature is
//! held.
//!
//! Only `+`, `-`, `*`, and `/` are used.

/// Surface radius of the reference model, meters.
pub const SURFACE_RADIUS: f64 = 6_371_000.0;

/// Radius of the reference model's core boundary, meters.
pub const CORE_BOUNDARY_RADIUS: f64 = 3_480_000.0;

/// One layer of the reference model: its upper radius in meters and its
/// density polynomial in grams per cubic centimeter, constant term first.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Layer {
    /// The layer holds radii below this one (and at or above the layer
    /// before), meters.
    pub top: f64,
    /// `a0 + a1 x + a2 x^2 + a3 x^3`, `x = r / 6371 km`, g/cm^3.
    pub density: [f64; 4],
}

/// The reference model's layers, innermost first, with the ocean replaced
/// by upper crust (module documentation). The last layer has no top.
pub const LAYERS: [Layer; 9] = [
    // Inner core.
    layer(1_221_500.0, [13.0885, 0.0, -8.8381, 0.0]),
    // Outer core.
    layer(3_480_000.0, [12.5815, -1.2638, -3.6426, -5.5281]),
    // Lower mantle (including D'').
    layer(5_701_000.0, [7.9565, -6.4761, 5.5283, -3.0807]),
    // Transition zone, 5701 to 5771 km.
    layer(5_771_000.0, [5.3197, -1.4836, 0.0, 0.0]),
    // Transition zone, 5771 to 5971 km.
    layer(5_971_000.0, [11.2494, -8.0298, 0.0, 0.0]),
    // Transition zone, 5971 to 6151 km.
    layer(6_151_000.0, [7.1089, -3.8045, 0.0, 0.0]),
    // Low velocity zone and lid, 6151 to 6346.6 km: one polynomial.
    layer(6_346_600.0, [2.6910, 0.6924, 0.0, 0.0]),
    // Lower crust.
    layer(6_356_000.0, [2.900, 0.0, 0.0, 0.0]),
    // Upper crust, standing in for the ocean and continuing upward.
    layer(f64::INFINITY, [2.600, 0.0, 0.0, 0.0]),
];

const fn layer(top: f64, density: [f64; 4]) -> Layer {
    Layer { top, density }
}

/// The geotherm: `(depth in meters, temperature in kelvin)`, ascending in
/// depth. See the module documentation for the sources.
pub const GEOTHERM: [(f64, f64); 7] = [
    (0.0, 288.0),
    (100_000.0, 1600.0),
    (410_000.0, 1830.0),
    (660_000.0, 1960.0),
    (2_700_000.0, 2600.0),
    (2_891_000.0, 4000.0),
    (5_150_000.0, 6230.0),
];

/// Density at model radius `r` (meters), kilograms per cubic meter.
pub fn density(r: f64) -> f64 {
    let x = r / SURFACE_RADIUS;
    let mut i = 0;
    while r >= LAYERS[i].top {
        i += 1;
    }
    let [a0, a1, a2, a3] = LAYERS[i].density;
    1000.0 * (a0 + x * (a1 + x * (a2 + x * a3)))
}

/// Temperature at model radius `r` (meters), kelvin.
pub fn temperature(r: f64) -> f64 {
    let d = SURFACE_RADIUS - r;
    let (d0, t0) = GEOTHERM[0];
    if d <= d0 {
        return t0;
    }
    for w in GEOTHERM.windows(2) {
        let ((da, ta), (db, tb)) = (w[0], w[1]);
        if d < db {
            return ta + (tb - ta) * ((d - da) / (db - da));
        }
    }
    GEOTHERM[GEOTHERM.len() - 1].1
}

/// The mass inside model radius `r_max` (meters), kilograms, by the
/// midpoint rule with `steps` shells: `sum 4 pi r^2 rho(r) dr`. For the
/// tests.
pub fn mass_within(r_max: f64, steps: u32) -> f64 {
    let dr = r_max / f64::from(steps);
    let mut m = 0.0;
    for i in 0..steps {
        let r = (f64::from(i) + 0.5) * dr;
        m += 4.0 * core::f64::consts::PI * r * r * density(r) * dr;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_values() {
        // Center, both sides of the core boundary, the crust.
        assert!((density(0.0) - 13088.5).abs() < 1e-9);
        assert!((density(3_479_999.0) - 9903.0).abs() < 1.0);
        assert!((density(3_480_000.0) - 5566.0).abs() < 1.0);
        assert_eq!(density(6_350_000.0), 2900.0);
        assert_eq!(density(6_370_000.0), 2600.0);
        assert_eq!(density(6_380_000.0), 2600.0);
    }

    #[test]
    fn mass_matches_the_model() {
        // The model's own mass is 5.974e24 kg with its ocean; with crust
        // in place of the ocean it is about 2.4e21 kg more.
        let m = mass_within(SURFACE_RADIUS, 200_000);
        assert!((m - 5.976e24).abs() < 0.003e24, "{m}");
    }

    #[test]
    fn geotherm_rises() {
        assert_eq!(temperature(SURFACE_RADIUS + 5000.0), 288.0);
        assert_eq!(temperature(SURFACE_RADIUS), 288.0);
        assert_eq!(temperature(SURFACE_RADIUS - 50_000.0), 944.0);
        assert_eq!(temperature(CORE_BOUNDARY_RADIUS), 4000.0);
        assert_eq!(temperature(0.0), 6230.0);
        let mut last = 0.0;
        for i in 0..=6371 {
            let t = temperature(SURFACE_RADIUS - f64::from(i) * 1000.0);
            assert!(t >= last);
            last = t;
        }
    }
}
