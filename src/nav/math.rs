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
//! - `cos_q16`:
//!   a one degree table of the cosine and linear interpolation (Bhaskara I's formula was 0.2 % off, more than the
//!   rest of the projection).
//! - `project`, `plane_mm`: the length of a degree of latitude and of longitude at the latitude in use, from the
//!   WGS84 series in `mm_per_degree`; the longitude difference wraps at 180 degrees (`dlon`). Cost: a few integer
//!   multiplies per point.

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

/// Natural logarithm (NaN for a negative or NaN `x`, -inf for 0, +inf for +inf): the exponent from the bits, then 2 atanh((m - 1) / (m + 1)) for the mantissa.
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
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

/// cos(1 degree steps) x 65536, 0 to 90 degrees (rounded from the exact cosine).
const COS_TAB: [i32; 91] = [
    65536, 65526, 65496, 65446, 65376, 65287, 65177, 65048, 64898, 64729, 64540, 64332, 64104, 63856, 63589, 63303,
    62997, 62672, 62328, 61966, 61584, 61183, 60764, 60326, 59870, 59396, 58903, 58393, 57865, 57319, 56756, 56175,
    55578, 54963, 54332, 53684, 53020, 52339, 51643, 50931, 50203, 49461, 48703, 47930, 47143, 46341, 45525, 44695,
    43852, 42995, 42126, 41243, 40348, 39441, 38521, 37590, 36647, 35693, 34729, 33754, 32768, 31772, 30767, 29753,
    28729, 27697, 26656, 25607, 24550, 23486, 22415, 21336, 20252, 19161, 18064, 16962, 15855, 14742, 13626, 12505,
    11380, 10252, 9121, 7987, 6850, 5712, 4572, 3430, 2287, 1144, 0,
];

/// cos(latitude) x 65536, latitude in degrees x 1e-7: the table above, linear between its degrees (error under
/// 4e-5 of 1, from the second derivative: h^2 / 8 with h = 1 degree).
///
/// ```
/// use bryton_rustfw::math::cos_q16;
/// assert_eq!(cos_q16(0), 65536);
/// assert!((cos_q16(600_000_000) - 32768).abs() < 2); // cos 60 = 0.5
/// assert_eq!(cos_q16(-900_000_000), 0);
/// ```
pub fn cos_q16(lat: i32) -> i64 {
    let a = i64::from(lat).abs().min(900_000_000);
    let (i, f) = ((a / 10_000_000) as usize, a % 10_000_000);
    let at = |k: usize| i64::from(COS_TAB.get(k).copied().unwrap_or(0));
    at(i) + (at(i + 1) - at(i)) * f / 10_000_000
}

/// The 1e-7 degree longitude difference from `from` to `to`, the short way round: wrapped to +-180 degrees, so
/// 179.9999 to -179.9999 is 0.0002 degrees, not 359.9998.
pub fn dlon(from: i32, to: i32) -> i64 {
    (i64::from(to) - i64::from(from) + 1_800_000_000).rem_euclid(3_600_000_000) - 1_800_000_000
}

/// Millimetres per degree of (meridian, parallel) at the latitude whose cosine x 65536 is `cos`. The WGS84 series
/// for the length of a degree (<https://en.wikipedia.org/wiki/Latitude#Length_of_a_degree_of_latitude>, from
/// Snyder / NIMA TR8350.2):
/// meridian 111132.954 - 559.822 cos 2phi + 1.175 cos 4phi m, parallel 111412.84 cos phi - 93.5 cos 3phi m
/// (+ 0.118 cos 5phi, under 1 cm, left out). cos 2phi, 4phi and 3phi come from cos phi by the double and triple
/// angle formulas, so one table lookup serves all.
pub fn mm_per_degree(cos: i64) -> (i64, i64) {
    let c2 = ((2 * cos * cos) >> 16) - 65_536;
    let c4 = ((2 * c2 * c2) >> 16) - 65_536;
    let c3 = ((4 * cos * cos * cos) >> 32) - 3 * cos;
    let meridian = 111_132_954 + ((-559_822 * c2 + 1175 * c4) >> 16);
    let parallel = (111_412_840 * cos - 93_500 * c3) >> 16;
    (meridian, parallel)
}

/// Where (`lat`, `lon`) is from `o` (its latitude, longitude, [`cos_q16`] of its latitude), all degrees x 1e-7:
/// millimetres east and north on the local plane, unclamped (at most 3.6e9 x 1.2e8 before the division: fits i64).
pub fn plane_mm(o: (i32, i32, i64), lat: i32, lon: i32) -> (i64, i64) {
    let (north, east) = mm_per_degree(o.2);
    let dy = (i64::from(lat) - i64::from(o.0)) * north / 10_000_000;
    let dx = dlon(o.1, lon) * east / 10_000_000;
    (dx, dy)
}

/// Where (`lat`, `lon`) (degrees x 1e-7) is on the plane of `origin` (its latitude, longitude and [`cos_q16`] of its
/// latitude): metres east and north.
///
/// ```
/// use bryton_rustfw::math::{cos_q16, project};
/// assert_eq!(project((0, 0, cos_q16(0)), 9043, 0), (0, 99)); // 100 m of latitude at the equator is 9043e-7 degrees
/// ```
pub fn project(o: (i32, i32, i64), lat: i32, lon: i32) -> (i32, i32) {
    let (x, y) = plane_mm(o, lat, lon);
    ((x / 1000) as i32, (y / 1000) as i32)
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

    /// Expected lengths from GeographicLib's WGS84 inverse geodesic (Karney; `geographiclib` 2.x, Python
    /// `Geodesic.WGS84.Inverse(lat1, lon1, lat2, lon2)['s12']`), one-off run, values pasted. The plane is local, so
    /// 100 km legs are allowed 0.25 % (a plane at the first point, not the midpoint).
    #[test]
    fn lengths_against_a_geodesic() {
        let e7 = |d: f64| (d * 1e7).round() as i32;
        let cases = [
            ((45.0, 10.0), (45.9, 10.0), 100_026.514),
            ((45.0, 10.0), (45.0, 11.27), 100_134.456),
            ((45.0, 10.0), (45.45, 10.6), 68_713.965),
            ((0.0, 0.0), (0.9, 0.0), 99_516.930),
            ((0.0, 0.0), (0.0, 0.9), 100_187.542),
            ((60.0, 5.0), (60.2, 5.5), 35_640.256),
        ];
        for (a, b, want) in cases {
            let (lat, lon) = (e7(a.0), e7(a.1));
            let (x, y) = plane_mm((lat, lon, cos_q16(lat)), e7(b.0), e7(b.1));
            let got = sqrt((x * x + y * y) as f64) / 1000.0;
            assert!((got - want).abs() <= want * 0.0025, "{a:?} to {b:?}: {got} m, geodesic {want}");
        }
    }

    /// The table's cosine against the exact one: under 4e-5 over the quarter turn (the old Bhaskara was 2e-3).
    #[test]
    fn cos_table_error() {
        for lat in (0..=900_000_000).step_by(1_234_567) {
            let want = F::cos(f64::from(lat) * 1e-7 * core::f64::consts::PI / 180.0) * 65_536.0;
            assert!((cos_q16(lat) as f64 - want).abs() <= 4.0, "{lat}");
            assert_eq!(cos_q16(-lat), cos_q16(lat));
        }
    }

    /// 179.9999 to -179.9999 is 0.0002 degrees (22.26 m at the equator by GeographicLib), not 359.9998.
    #[test]
    fn the_antimeridian() {
        assert_eq!(dlon(1_799_999_000, -1_799_999_000), 2000);
        assert_eq!(dlon(-1_799_999_000, 1_799_999_000), -2000);
        let (x, _) = plane_mm((0, 1_799_999_000, cos_q16(0)), 0, -1_799_999_000);
        assert!((x - 22_264).abs() <= 5, "{x} mm");
        let (x, _) = plane_mm((450_000_000, 1_799_999_000, cos_q16(450_000_000)), 450_000_000, -1_799_999_000);
        assert!((x - 15_769).abs() <= 20, "{x} mm"); // GeographicLib: 15.769 m at 45 N
    }

    /// Across the equator and the prime meridian the signs follow the axes.
    #[test]
    fn project_across_zero() {
        let o = (0, 0, cos_q16(0));
        assert_eq!(project(o, 9043, 8983), (99, 99));
        assert_eq!(project(o, -9043, -8983), (-99, -99));
        let o = (-4521, -4491, cos_q16(-4521));
        assert_eq!(project(o, 4522, 4492), (99, 99));
    }

    #[test]
    fn logarithm_edges() {
        assert!(ln(-2.0).is_nan() && ln(f64::NAN).is_nan());
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
        assert_eq!(ln(f64::INFINITY), f64::INFINITY);
        assert_eq!(atanh(1.0), f64::INFINITY); // the pole's Mercator ordinate
        assert_eq!(atanh(-1.0), f64::NEG_INFINITY);
    }
}
