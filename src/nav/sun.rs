//! The sun's rise and set at a place on a date, for the ride's sun fields. The equations are the ones NOAA's solar
//! calculator uses (<https://gml.noaa.gov/grad/solcalc/calcdetails.html>: "based on equations from Astronomical
//! Algorithms, by Jean Meeus", ch. 25 and 28; its calculation spreadsheets write the same steps): the sun's mean
//! longitude and anomaly, the equation of the centre, the apparent longitude, the obliquity corrected for nutation,
//! the declination, the equation of time, then the hour angle at the zenith 90.833 degrees (the disc's upper limb and
//! 0.833 degrees of refraction). The calculator states an accuracy of about a minute between 72 degrees north and
//! south. Floating point without libm: a 12-term Taylor series for sin and cos, and bisection for asin and acos (50
//! halvings, error under 1e-12). Pure logic: the app works it out once a minute.

use core::f64::consts::{PI, TAU};

const DEG: f64 = PI / 180.0;

/// Radians reduced to [-pi, pi] (the arguments here are a few turns at most).
fn reduce(x: f64) -> f64 {
    let x = x - TAU * f64::from(((x / TAU) + 1024.0) as i32 - 1024);
    if x > PI { x - TAU } else { x }
}

fn sin(x: f64) -> f64 {
    let x = reduce(x);
    let (mut term, mut sum) = (x, x);
    for k in 1..12 {
        term *= -x * x / f64::from((2 * k) * (2 * k + 1));
        sum += term;
    }
    sum
}

fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

/// asin of `s` (in [-1, 1]): bisection on sin, which rises on [-pi/2, pi/2].
fn asin(s: f64) -> f64 {
    let (mut lo, mut hi) = (-PI / 2.0, PI / 2.0);
    for _ in 0..50 {
        let mid = (lo + hi) / 2.0;
        if sin(mid) < s { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

/// acos of `c` (in [-1, 1]): bisection on cos, which falls on [0, pi].
fn acos(c: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, PI);
    for _ in 0..50 {
        let mid = (lo + hi) / 2.0;
        if cos(mid) > c { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

/// The sun's declination (radians) and the equation of time (minutes) at a Julian day (Meeus 25, 28).
fn solar(jd: f64) -> (f64, f64) {
    let t = (jd - 2_451_545.0) / 36_525.0;
    let l0 = 280.46646 + t * (36_000.769_83 + t * 0.000_303_2); // geometric mean longitude, degrees
    let m = (357.52911 + t * (35_999.050_29 - 0.000_153_7 * t)) * DEG; // mean anomaly
    let e = 0.016_708_634 - t * (0.000_042_037 + 0.000_000_126_7 * t); // eccentricity of the orbit
    let c = sin(m) * (1.914_602 - t * (0.004_817 + 0.000_014 * t))
        + sin(2.0 * m) * (0.019_993 - 0.000_101 * t)
        + sin(3.0 * m) * 0.000_289; // equation of the centre, degrees
    let node = (125.04 - 1934.136 * t) * DEG;
    let omega = (l0 + c - 0.005_69 - 0.004_78 * sin(node)) * DEG; // apparent longitude
    let ob0 = 23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.000_59 - t * 0.001_813))) / 60.0) / 60.0;
    let ob = (ob0 + 0.002_56 * cos(node)) * DEG; // obliquity, corrected for nutation
    let decl = asin(sin(ob) * sin(omega));
    let u = sin(ob) / (1.0 + cos(ob)); // tan(obliquity / 2)
    let y = u * u;
    let l0r = l0 * DEG;
    let eq = 4.0 / DEG
        * (y * sin(2.0 * l0r) - 2.0 * e * sin(m) + 4.0 * e * y * sin(m) * cos(2.0 * l0r)
            - 0.5 * y * y * sin(4.0 * l0r)
            - 1.25 * e * e * sin(2.0 * m));
    (decl, eq)
}

/// Minute of the day, [0, 1440), rounded to the nearest minute.
fn minute_of_day(m: f64) -> u32 {
    ((((m + 0.5) + 14_400.0) as i32 - 14_400).rem_euclid(1440)) as u32
}

/// Sunrise and sunset on a date at a place, as local minutes of the day (rounded): `lat_e7` and `lon_e7` in degrees
/// x 1e-7 (east positive), `days` since 1970-01-01 of the local date, `offset_min` the local clock's offset from UTC.
/// None in polar day or night (the sun does not cross the zenith 90.833 degrees).
///
/// ```
/// // the equator at the equinox: about 06:04 and 18:11 (solar noon at 12:00 minus the equation of time)
/// assert_eq!(bryton_rustfw::sun::rise_set(0, 0, 20532, 0), Some((364, 1091)));
/// // 70 degrees north at the June solstice: the sun stays up
/// assert_eq!(bryton_rustfw::sun::rise_set(700_000_000, 0, 20625, 0), None);
/// ```
pub fn rise_set(lat_e7: i32, lon_e7: i32, days: i32, offset_min: i32) -> Option<(u32, u32)> {
    let (lat, lon) = (f64::from(lat_e7) * 1e-7, f64::from(lon_e7) * 1e-7);
    let jd0 = f64::from(days) + 2_440_587.5; // 0 h UTC of the date
    // the declination and the equation of time, first at noon UTC, then at the solar noon found there
    let (_, eq) = solar(jd0 + 0.5);
    let noon = 720.0 - 4.0 * lon - eq;
    let (decl, eq) = solar(jd0 + noon / 1440.0);
    let noon = 720.0 - 4.0 * lon - eq;
    let (lat_r, decl_c) = (lat * DEG, sin(decl));
    let x = (cos(90.833 * DEG) - sin(lat_r) * decl_c) / (cos(lat_r) * cos(decl));
    if !(-1.0..=1.0).contains(&x) {
        return None;
    }
    let half = acos(x) / DEG * 4.0; // minutes from solar noon: 4 minutes a degree of hour angle
    let off = f64::from(offset_min);
    Some((minute_of_day(noon - half + off), minute_of_day(noon + half + off)))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// Local minutes of the day within a minute of the expected ones (the expected values are from the same
    /// equations written in double precision, checked by hand at the equinox; NOAA's calculator was not
    /// reachable for its numbers).
    fn near(got: Option<(u32, u32)>, want: (u32, u32)) -> bool {
        got.is_some_and(|(r, s)| r.abs_diff(want.0) <= 1 && s.abs_diff(want.1) <= 1)
    }

    #[test]
    fn sun_times_match_the_published_equations() {
        // equator, equinox (2026-03-20, days 20532): 06:04 and 18:11 UTC
        assert!(near(rise_set(0, 0, 20532, 0), (364, 1091)));
        // 60 N at the June solstice (2026-06-21): 02:36 and 21:28 UTC, a day of 18 h 52 min
        assert!(near(rise_set(600_000_000, 0, 20625, 0), (156, 1288)));
        // 60 N at the December solstice: 09:02 and 14:54, 5 h 52 min of light
        assert!(near(rise_set(600_000_000, 0, 20808, 0), (542, 894)));
        // 40 S, 150 E, December (20808), local clock at UTC+10: 04:27 and 19:28
        assert!(near(rise_set(-400_000_000, 1_500_000_000, 20808, 600), (267, 1168)));
        // 15 E in September (20718), local clock at UTC+1: 05:49 and 17:56
        assert!(near(rise_set(0, 150_000_000, 20718, 60), (349, 1076)));
    }

    #[test]
    fn polar_day_and_night_have_no_times() {
        assert_eq!(rise_set(700_000_000, 0, 20625, 0), None); // midnight sun
        assert_eq!(rise_set(700_000_000, 0, 20808, 0), None); // polar night
        assert_eq!(rise_set(-700_000_000, 0, 20625, 0), None); // the southern winter's polar night
    }

    #[test]
    fn southern_summer_and_the_clock_offset() {
        // the offset moves both times, and the day is the same length
        let a = rise_set(-340_000_000, 1_510_000_000, 20808, 0).unwrap();
        let b = rise_set(-340_000_000, 1_510_000_000, 20808, 600).unwrap();
        assert_eq!([(b.0 + 1440 - a.0) % 1440, (b.1 + 1440 - a.1) % 1440], [600, 600]);
    }

    #[test]
    fn trig_without_libm() {
        use std::primitive::f64 as F;
        for x in [-7.0, -3.2, -1.0, 0.0, 0.5, 2.9, 3.1, 6.5, 40.0] {
            assert!((sin(x) - F::sin(x)).abs() < 1e-9, "sin {x}");
            assert!((cos(x) - F::cos(x)).abs() < 1e-9, "cos {x}");
        }
        assert!((asin(0.3) - F::asin(0.3)).abs() < 1e-9);
        assert!((acos(-0.6) - F::acos(-0.6)).abs() < 1e-9);
    }
}
