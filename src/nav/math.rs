//! The math the firmware shares, without libm (`core` has no float functions and libm costs flash): sin, cos, ln,
//! atanh and sqrt in f64, an integer square root, and the local flat projection of the map and the course (cos of the
//! latitude, degrees x 1e-7 to metres). Pure logic; one copy, so the map, the course, the climbs, the sun, the
//! tiles, HRV and the pacer agree.
//!
//! - `sin`: the Taylor series of sine, <https://en.wikipedia.org/wiki/Sine_and_cosine> (series definition), the
//!   argument reduced to [-pi, pi] first; 12 terms, error under 1e-12 over a few turns. `cos` is `sin` shifted by a
//!   quarter turn.
//! - `ln`: via atanh, ln(x) = 2 atanh((x - 1) / (x + 1)), <https://en.wikipedia.org/wiki/Logarithm#Calculation> (the
//!   inverse hyperbolic tangent series); the exponent comes from the bits, then the series for the mantissa.
//! - `atanh`: its Maclaurin series, <https://en.wikipedia.org/wiki/Inverse_hyperbolic_functions#Series_expansions>;
//!   here the Mercator ordinate of a latitude whose sine is `s`.
//! - `sqrt`: Heron's method (Newton's steps),
//!   <https://en.wikipedia.org/wiki/Methods_of_computing_square_roots#Heron's_method>, from a first guess taken from
//!   the exponent halved.
//! - `isqrt`: core's `u32::isqrt`.
//! - `cos_q16`: Bhaskara I's sine approximation,
//!   <https://en.wikipedia.org/wiki/Bh%C4%81skara_I%27s_sine_approximation_formula>, within 0.2 %.
//!   `project`: 1e-7 degrees of a great circle is 0.011132 m.

use core::f64::consts::{PI, TAU};

/// Radians reduced to [-pi, pi] (the arguments here are a few turns at most).
fn reduce(x: f64) -> f64 {
    if x.abs() <= PI {
        return x;
    }
    let x = x - TAU * f64::from(((x / TAU) + 1024.0) as i32 - 1024);
    if x > PI { x - TAU } else { x }
}

/// Sine, error under 1e-12 for arguments of a few turns.
pub fn sin(x: f64) -> f64 {
    let x = reduce(x);
    let (mut term, mut sum) = (x, x);
    for k in 1..12 {
        term *= -x * x / f64::from((2 * k) * (2 * k + 1));
        sum += term;
    }
    sum
}

/// Cosine, as [`sin`]. cos(0) is sin(pi / 2), within 1e-16 of 1, not exactly 1.
pub fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

/// Natural logarithm: the exponent from the bits, then 2 atanh((m - 1) / (m + 1)) for the mantissa.
pub fn ln(x: f64) -> f64 {
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));
    if m > core::f64::consts::SQRT_2 {
        m /= 2.0;
        e += 1;
    }
    let t = (m - 1.0) / (m + 1.0);
    let (mut term, mut sum) = (t, t);
    for k in 1..12 {
        term *= t * t;
        sum += term / f64::from(2 * k + 1);
    }
    e as f64 * core::f64::consts::LN_2 + 2.0 * sum
}

/// The Mercator ordinate of a latitude whose sine is `s`.
pub fn atanh(s: f64) -> f64 {
    0.5 * ln((1.0 + s) / (1.0 - s))
}

/// The square root (`core` has none): a first guess from the exponent halved, then Newton's steps. 0 for `s` not
/// above 0 or not finite.
pub fn sqrt(s: f64) -> f64 {
    if !(s > 0.0 && s.is_finite()) {
        return 0.0;
    }
    let mut x = f64::from_bits((s.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..6 {
        x = 0.5 * (x + s / x);
    }
    x
}

/// The integer square root of `n` (0 for less than 1), for the crosshair's ring.
pub fn isqrt(n: i32) -> i32 {
    (n.max(0) as u32).isqrt() as i32
}

/// cos(latitude) x 65536, latitude in degrees x 1e-7 (Bhaskara I's approximation, within 0.2 %).
///
/// ```
/// use bryton_rustfw::math::cos_q16;
/// assert_eq!(cos_q16(0), 65536);
/// assert!((cos_q16(600_000_000) - 32768).abs() < 100); // cos 60 = 0.5
/// assert_eq!(cos_q16(-900_000_000), 0);
/// ```
pub fn cos_q16(lat: i32) -> i64 {
    let t = i64::from(lat / 10_000); // 1e-3 degrees
    let t2 = t * t;
    ((32_400_000_000 - 4 * t2) << 16) / (32_400_000_000 + t2)
}

/// Where (`lat`, `lon`) (degrees x 1e-7) is on the plane of `origin` (its latitude, longitude and [`cos_q16`] of its
/// latitude): metres east and north.
///
/// ```
/// use bryton_rustfw::math::{cos_q16, project};
/// assert_eq!(project((0, 0, cos_q16(0)), 8983, 0), (0, 99)); // 8983e-7 degrees of latitude: 100 m
/// ```
pub fn project((lat0, lon0, cos): (i32, i32, i64), lat: i32, lon: i32) -> (i32, i32) {
    // 1e-7 degrees of a great circle is 0.011132 m (WGS84 equator; inferred close enough for a drawing)
    let x = (i64::from(lon) - i64::from(lon0)) * 11_132 * cos / 65_536 / 1_000_000;
    let y = (i64::from(lat) - i64::from(lat0)) * 11_132 / 1_000_000;
    (x as i32, y as i32)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::primitive::f64 as F;

    #[test]
    fn floating_point_without_libm() {
        for x in [-7.0, -3.2, -1.5, -1.0, -0.7, 0.0, 0.3, 0.5, 0.785, 1.2, 2.9, 3.1, 6.5, 40.0] {
            assert!((sin(x) - F::sin(x)).abs() < 1e-9, "sin {x}");
            assert!((cos(x) - F::cos(x)).abs() < 1e-9, "cos {x}");
        }
        for x in [-1.5, -0.7, 0.3, 0.785, 1.2, core::f64::consts::FRAC_PI_2] {
            assert!((sin(x) - F::sin(x)).abs() < 1e-12, "sin {x}");
            assert!((cos(x) - F::cos(x)).abs() < 1e-12, "cos {x}");
        }
        for x in [1e-9, 0.01, 0.5, 1.0, 1.5, 2.0, 10.0, 12345.678] {
            assert!((ln(x) - F::ln(x)).abs() < 1e-12, "ln {x}");
        }
    }

    #[test]
    fn square_roots() {
        for s in [1e-6, 0.25, 2.0, 100.0, 1e6, 1e12] {
            assert!((sqrt(s) - F::sqrt(s)).abs() <= 1e-9 * F::sqrt(s), "sqrt {s}");
        }
        assert_eq!([sqrt(0.0), sqrt(-4.0), sqrt(f64::NAN), sqrt(f64::INFINITY)], [0.0; 4]);
        assert_eq!([isqrt(-5), isqrt(0), isqrt(15), isqrt(16), isqrt(i32::MAX)], [0, 0, 3, 4, 46_340]);
    }
}
