//! The reference ellipsoid and the conversion from frame coordinates to
//! geodetic latitude, longitude, and height.
//!
//! Implements the "solid rock up to the elevation surface on the reference
//! ellipsoid" half of `space-model.md` section 11 item 2: the module needs,
//! for every point it samples, where the point is relative to the
//! ellipsoid the elevation grid is laid on.
//!
//! # Axes
//!
//! The frame's axes are the body axes the system module declares for the
//! frame (its registry orientation): `z` points at the north pole and `x` at
//! the prime meridian, `y` completes a right-handed set, so it points at 90
//! degrees east. Longitude is `atan2(y, x)`, positive east.
//!
//! # Ellipsoid
//!
//! WGS84: semi-major axis [`A`] = 6378137 m and flattening [`F`] =
//! 1 / 298.257223563 (NIMA TR8350.2, third edition, 2000). Heights are
//! along the ellipsoid normal.
//!
//! # Determinism
//!
//! The geodetic conversion is Bowring's method with a fixed number of
//! iterations ([`BOWRING_ITERATIONS`]), written with `+`, `-`, `*`, `/`, and
//! `sqrt` only. `gx_core::detmath` has no inverse tangent, so [`atan`] and
//! [`atan2`] are written out here the same way: two exact half-angle
//! reductions and a fixed odd polynomial. Every operation is correctly
//! rounded in IEEE 754 and none is fused, so the bits are the same natively
//! and in WebAssembly.

use core::f64::consts::{FRAC_PI_2, PI};

/// WGS84 semi-major (equatorial) axis, meters.
pub const A: f64 = 6_378_137.0;

/// WGS84 flattening.
pub const F: f64 = 1.0 / 298.257_223_563;

/// WGS84 semi-minor (polar) axis, meters: `A (1 - F)`.
pub const B: f64 = A * (1.0 - F);

/// First eccentricity squared: `F (2 - F)`.
pub const E2: f64 = F * (2.0 - F);

/// Second eccentricity squared: `E2 / (1 - E2)`.
pub const EP2: f64 = E2 / (1.0 - E2);

/// Iterations of Bowring's method. One gives millimeters for points within
/// tens of kilometers of the ellipsoid; three leave only rounding.
pub const BOWRING_ITERATIONS: usize = 3;

/// The lowest height the elevation surface can have, meters: the deepest
/// elevation in the grid is above it. Points below are always solid.
pub const H_MIN: f64 = -11_000.0;

/// The highest height the elevation surface can have, meters: the highest
/// elevation in the grid is below it. Points above are always vacuum.
pub const H_MAX: f64 = 9_000.0;

/// A bound on the ellipsoidal radius [`q2`] below which every point is
/// more than `-H_MIN` below the ellipsoid. On the surface of height
/// [`H_MIN`] the ellipsoidal radius is smallest at the poles,
/// `1 + H_MIN / B`; the bound keeps a further `1e-6` (about 6 m) for
/// curvature and rounding. The tests check it over every latitude.
pub const Q_SOLID: f64 = 1.0 + H_MIN / B - 1e-6;

/// A bound on the ellipsoidal radius above which every point is more than
/// [`H_MAX`] above the ellipsoid: `1 + H_MAX / B` (largest at the poles)
/// plus `1e-6`.
pub const Q_VACUUM: f64 = 1.0 + H_MAX / B + 1e-6;

/// The bounds on `tan(geodetic latitude) / tan(geocentric latitude)` for
/// points with height from [`H_MIN`] to [`H_MAX`]: the ratio is
/// `(N + h) / (N (1 - E2) + h)`, from about 1.006730 to 1.006751 over that
/// shell. Used to bound the geodetic latitudes a cell can hold.
pub const TAN_RATIO: (f64, f64) = (1.0067, 1.0068);

/// A point in geodetic coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Geodetic {
    /// Geodetic latitude, radians, `-pi/2` to `pi/2`.
    pub lat: f64,
    /// Longitude, radians, `-pi` to `pi`, positive east.
    pub lon: f64,
    /// Height above the ellipsoid along its normal, meters.
    pub h: f64,
}

/// The squared ellipsoidal radius of a point: `(x^2 + y^2) / A^2 + z^2 /
/// B^2`. It is 1 on the ellipsoid, below 1 inside it.
pub fn q2(x: f64, y: f64, z: f64) -> f64 {
    let (u, v, w) = (x / A, y / A, z / B);
    u * u + v * v + w * w
}

/// Geodetic latitude and height of a point given its distance `p` from the
/// polar axis and its height `z` above the equatorial plane, by Bowring's
/// method with [`BOWRING_ITERATIONS`] iterations.
///
/// Valid for points outside the ellipsoid's evolute (more than about 43 km
/// from the center); the module only calls it for points within about 11
/// km of the surface.
pub fn latitude_height(p: f64, z: f64) -> (f64, f64) {
    // The parametric latitude beta, as its cosine and sine; Bowring's
    // starting value is tan(beta) = (A z) / (B p).
    let (mut cb, mut sb) = unit(p * B, z * A);
    let (mut num, mut den) = (z, p);
    for _ in 0..BOWRING_ITERATIONS {
        // tan(lat) = (z + EP2 B sin^3 beta) / (p - E2 A cos^3 beta).
        num = z + EP2 * B * (sb * sb * sb);
        den = p - E2 * A * (cb * cb * cb);
        // tan(beta) = (B / A) tan(lat).
        let (c, s) = unit(A * den, B * num);
        cb = c;
        sb = s;
    }
    let (cl, sl) = unit(den, num);
    let h = p * cl + z * sl - A * (1.0 - E2 * (sl * sl)).sqrt();
    (atan2(num, den), h)
}

/// Geodetic coordinates of the frame point `(x, y, z)`. See
/// [`latitude_height`] for where it is valid.
pub fn geodetic(x: f64, y: f64, z: f64) -> Geodetic {
    let p = (x * x + y * y).sqrt();
    let (lat, h) = latitude_height(p, z);
    Geodetic {
        lat,
        lon: atan2(y, x),
        h,
    }
}

/// Frame coordinates of a geodetic point, the inverse of [`geodetic`],
/// with `gx_core::detmath` sine and cosine.
pub fn frame_point(g: Geodetic) -> [f64; 3] {
    use gx_core::detmath::{cos, sin};
    let (sl, cl) = (sin(g.lat), cos(g.lat));
    let n = A / (1.0 - E2 * (sl * sl)).sqrt();
    let r = (n + g.h) * cl;
    [r * cos(g.lon), r * sin(g.lon), (n * (1.0 - E2) + g.h) * sl]
}

/// `(u, v) / |(u, v)|`. Both 0 gives `(1, 0)`.
fn unit(u: f64, v: f64) -> (f64, f64) {
    let r = (u * u + v * v).sqrt();
    if r == 0.0 {
        (1.0, 0.0)
    } else {
        (u / r, v / r)
    }
}

/// The inverse tangent, radians, from `+`, `-`, `*`, `/`, and `sqrt` only.
/// Odd; within a few units in the last place of the true value.
pub fn atan(x: f64) -> f64 {
    let a = x.abs();
    let r = if a <= 1.0 {
        atan_unit(a)
    } else {
        FRAC_PI_2 - atan_unit(1.0 / a)
    };
    if x < 0.0 {
        -r
    } else {
        r
    }
}

/// The angle of the vector `(x, y)` from the positive `x` axis, radians,
/// `-pi` to `pi`, from `+`, `-`, `*`, `/`, and `sqrt` only. `(0, 0)` gives
/// 0. A negative zero `y` is treated as positive.
pub fn atan2(y: f64, x: f64) -> f64 {
    let (ax, ay) = (x.abs(), y.abs());
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let a = if ay <= ax {
        atan_unit(ay / ax)
    } else {
        FRAC_PI_2 - atan_unit(ax / ay)
    };
    let a = if x < 0.0 { PI - a } else { a };
    if y < 0.0 {
        -a
    } else {
        a
    }
}

/// `atan(t)` for `t` from 0 to 1.
///
/// Two half-angle steps, `atan(t) = 2 atan(t / (1 + sqrt(1 + t^2)))`, bring
/// the argument below `tan(pi/16)` (about 0.199). There the Taylor series
/// to the `t^27` term is exact to below a part in `10^17`.
fn atan_unit(t: f64) -> f64 {
    let t = t / (1.0 + (1.0 + t * t).sqrt());
    let t = t / (1.0 + (1.0 + t * t).sqrt());
    let t2 = t * t;
    // Horner over the coefficients (-1)^k / (2k + 1), k = 13 down to 0.
    let mut s = 0.0f64;
    for k in (0..14u32).rev() {
        let c = 1.0 / f64::from(2 * k + 1);
        let c = if k % 2 == 1 { -c } else { c };
        s = c + t2 * s;
    }
    4.0 * (t * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan_matches_the_platform_closely() {
        let mut worst = 0.0f64;
        for i in -4000..=4000 {
            let x = f64::from(i) * 0.0037;
            let d = (atan(x) - x.atan()).abs();
            worst = worst.max(d / x.atan().abs().max(1e-300));
        }
        assert!(worst < 1e-15, "{worst}");
        for &x in &[1e-300, 1e-8, 0.5, 1.0, 2.0, 1e8, 1e300, f64::INFINITY] {
            assert!((atan(x) - x.atan()).abs() <= 4.0 * f64::EPSILON * x.atan().abs());
        }
        assert_eq!(atan(0.0), 0.0);
    }

    #[test]
    fn atan2_quadrants() {
        for i in 0..360 {
            let a = (f64::from(i) + 0.5) * PI / 180.0 - PI;
            let (y, x) = (a.sin() * 3.0, a.cos() * 3.0);
            assert!((atan2(y, x) - y.atan2(x)).abs() < 1e-15, "{i}");
        }
        assert_eq!(atan2(0.0, 0.0), 0.0);
        assert_eq!(atan2(0.0, -1.0), PI);
        assert_eq!(atan2(1.0, 0.0), FRAC_PI_2);
        assert_eq!(atan2(-1.0, 0.0), -FRAC_PI_2);
    }

    #[test]
    fn geodetic_round_trip() {
        for i in -90..=90 {
            for &h in &[-11_000.0, -100.0, 0.0, 4321.5, 9000.0] {
                let g = Geodetic {
                    lat: f64::from(i) * PI / 180.0 * 0.999,
                    lon: f64::from(i) * 0.033,
                    h,
                };
                let [x, y, z] = frame_point(g);
                let back = geodetic(x, y, z);
                assert!((back.lat - g.lat).abs() < 1e-13, "{i} {h}");
                assert!((back.lon - g.lon).abs() < 1e-13, "{i} {h}");
                assert!((back.h - g.h).abs() < 1e-6, "{i} {h}: {}", back.h);
            }
        }
        // On the axis.
        let g = geodetic(0.0, 0.0, B + 10.0);
        assert_eq!(g.lat, FRAC_PI_2);
        assert!((g.h - 10.0).abs() < 1e-6);
    }

    /// The constants that bound the height band hold at every latitude:
    /// the ellipsoidal radius of the surfaces of height `H_MIN` and `H_MAX`
    /// lies strictly between `Q_SOLID` and `Q_VACUUM`, and the tangent
    /// ratio stays inside `TAN_RATIO`.
    #[test]
    fn band_bounds_hold() {
        for i in 0..=9000 {
            let lat = f64::from(i) * FRAC_PI_2 / 9000.0;
            for &h in &[H_MIN, H_MAX] {
                let [x, y, z] = frame_point(Geodetic { lat, lon: 0.3, h });
                let q = q2(x, y, z).sqrt();
                assert!(q > Q_SOLID && q < Q_VACUUM, "lat {lat} h {h}: {q}");
                if i > 0 && i < 9000 {
                    let p = (x * x + y * y).sqrt();
                    let ratio = lat.tan() / (z / p);
                    assert!(ratio > TAN_RATIO.0 && ratio < TAN_RATIO.1, "{ratio}");
                }
            }
        }
    }
}
