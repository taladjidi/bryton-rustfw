//! The barometric altitude: pressure to height, the smoothing, the calibration and the choice between the barometer
//! and the receiver. Pure logic, one call a second while riding ([`Altimeter::second`]).
//!
//! **Formula.** The international barometric formula (the troposphere of the ICAO standard atmosphere, as the US
//! National Weather Service's pressure altitude uses it): `h = 44307.694 m * (1 - (p / p0)^0.190284)`, with `p0` the
//! pressure at sea level (1013.25 hPa for the standard day). Here `p0` is the *calibration*: the sea-level pressure
//! that makes the altitude right where the rider is (it changes with the weather, a hPa being about 8 m). As an
//! integer table (no `powf` in `core`, no float needed): the formula at `p / p0` in 64ths, from 0.25 to 1.25, linear
//! in between (65 values; worst error against the formula 1.8 m at 9000 m, 2.6 m at 10 km, under 0.3 m below 2000 m, and the slope
//! error under 1 %). Checked in the tests against the ISA table (898.746 hPa at 1000 m, 794.952 at 2000 m, 701.085 at
//! 3000 m, 540.48 at 5000 m: ICAO Doc 7488, as tabulated in the Wikipedia article "International Standard
//! Atmosphere"). Not re-fetched in this session: the NWS PDF could not be read; the check against the ISA table is
//! the evidence.
//!
//! **Pressure** is the LPS22HH's own unit, 1/4096 hPa (0.24 Pa): [`crate::ambient::baro_read_raw`].
//!
//! **Smoothing**: the median of the last [`MEDIAN`] readings, then an exponential average of 1/[`SMOOTH`] per reading
//! (about 7 s of lag at the ride's 1 Hz). The chip's noise is 0.65 Pa (5 cm), so this is for the air at the case's
//! vent, not the chip. Measured on a real 2 h ride (2026-10-26, 1 Hz, the records' altitude turned back into
//! readings): still, the readings sit within about 0.5 m; riding at 30 km/h they spread +-2 m (5 % to 95 %), and 149
//! single readings stood 4 to 15 m off their neighbours, either sign, often a pair of opposite signs (gusts, the case
//! pressed: inferred). The median takes those out whole, which no average does: with the old average of 1/4 alone
//! the altitude moved over 1 m in a second 121 times, over 0.5 m 917 times; with this filter none over 1 m, 44 over
//! 0.5 m (descents at 40 km/h among them). Strava does the same in its way: it drops outliers and smooths before
//! counting a gain, less for barometric data (support.strava.com/hc/en-us/articles/216919447, "Elevation").
//!
//! **Calibration** ([`Cal`]): found by bisection on `p0` (the altitude rises with `p0`), so no inverse table.
//! Set by a typed altitude ([`Altimeter::calibrate`]), by the receiver's altitude averaged over [`WINDOW_S`] seconds
//! of 3D fix with six satellites or more, together with the pressure over the same seconds ([`Altimeter::from_gps`],
//! and automatically at a ride's start, [`Altimeter::ride_start`]). Stock keeps the same two things in
//! `\System\DeviceData.ini` `[DeviceSys]`: `Alt_P0` (the sea-level pressure in hPa, default 1020.0) and
//! `calAltitude_km` / `calAltitude_mi` (the typed altitude, defaults 200 and 218.7: metres and yards, inferred), and
//! its Baro_CheckAltCalibrationAtLocation sets the altitude from a saved place (ghidra/notes agent_a2.md, agent_b1.md).
//!
//! **Which altitude** ([`Altimeter::source`]): the barometer's when it reads and its calibration is *trusted* (made
//! in the last [`FRESH_S`], or this power-up, or being made at the ride's start); the receiver's otherwise, while it
//! has a fix; with no fix the barometer on whatever calibration it has (the standard day if none): a ride's ascent
//! wants only differences. While the ride's start averages a calibration, the barometer on its old one is used, not
//! the receiver: the receiver's altitude moved 10 to 17 m a second in the first 28 s of the 2026-10-26 ride, and
//! counted 26 m of ascent there. Each calibration, and each change of source, is a new *reference* (a number the
//! ride sees). Trust ends with the calibration's age ([`FRESH_S`], checked at each ride's start, also within one
//! power-up). The ride's altitude never steps: while a ride is open [`Altimeter::follow`] shifts a new reference
//! (a calibration, or the barometer to the receiver or back) to continue from the last value, then lets the shift go
//! by [`SLEW_MM`] a second at most, so the records move to the right level with no step; the part of a second's
//! change that is that slew is [`Altimeter::corr`], which the ride's ascent and climb window take out (ride.rs), so a
//! correction of 10 m never counts as 10 m of climbing. A resumed ride reads `altcal.bin` (a read, allowed during a
//! ride) so it keeps its source.
//!
//! **No drift correction during a ride.** Weather moves the pressure about 1 hPa in 3 hours at most on a normal day
//! (8 m in 3 h, 3 m an hour): over a ride the ascent counts differences over minutes, so it is untouched, and only
//! the absolute altitude is off by a few metres at the end. A slow pull toward the receiver would bring its noise
//! (several metres) back in; the owner's remedy is FROM GPS in the menu, and the automatic one at the next start.
//!
//! **Kept** in `\Rustfw\altcal.bin` (one sector, a CRC; app/altitude.rs), written between rides only. A damaged or
//! missing file means "not calibrated", which the next ride's start repairs by itself: nothing is lost with it.

/// The standard sea-level pressure, 1013.25 hPa, in the chip's unit.
pub const P_STD: u32 = 4_150_272;

/// Pressures the chip measures (260 to 1260 hPa), and the sea-level pressures a calibration may find (850 to 1100 hPa).
const P_RANGE: core::ops::RangeInclusive<u32> = 260 * 4096..=1260 * 4096;
const P0_RANGE: core::ops::RangeInclusive<u32> = 850 * 4096..=1100 * 4096;

/// `44307694 mm * (1 - r^0.190284)` for r = 16/64 to 80/64.
const TABLE: [i32; 65] = [
    10273399, 9878509, 9502002, 9142068, 8797161, 8465947, 8147267, 7840108, 7543579, 7256892, 6979345, 6710311,
    6449228, 6195589, 5948936, 5708853, 5474961, 5246915, 5024397, 4807117, 4594807, 4387220, 4184127, 3985316,
    3790591, 3599769, 3412680, 3229163, 3049069, 2872260, 2698605, 2527980, 2360269, 2195364, 2033162, 1873566,
    1716484, 1561829, 1409519, 1259477, 1111627, 965900, 822228, 680548, 540800, 402925, 266868, 132576, 0, -130910,
    -260198, -387910, -514088, -638773, -762002, -883815, -1004246, -1123330, -1241100, -1357589, -1472827, -1586844,
    -1699667, -1811326, -1921846,
];

/// Height above the level where the pressure is `p0`, mm, for the pressure `p` (both 1/4096 hPa). None for a ratio
/// outside 0.25 to 1.25 or a zero `p0`.
///
/// ```
/// use bryton_rustfw::altitude::{alt_mm, P_STD};
/// assert_eq!(alt_mm(P_STD, P_STD), Some(0));
/// let h = alt_mm(898_746 * 4096 / 1000, P_STD).unwrap(); // ISA: 898.746 hPa at 1000 m
/// assert!((h - 1_000_000).abs() < 1000);
/// ```
pub fn alt_mm(p: u32, p0: u32) -> Option<i32> {
    if p0 == 0 {
        return None;
    }
    let r = (u64::from(p) << 24) / u64::from(p0); // Q24: Q16 would be 0.13 m a step
    let (i, f) = ((r >> 18) as usize, (r & 0x3_ffff) as i64);
    let (a, b) = (i64::from(*TABLE.get(i.checked_sub(16)?)?), i64::from(*TABLE.get(i.checked_sub(15)?)?));
    Some((a + (b - a) * f / 0x4_0000) as i32)
}

/// The sea-level pressure that makes pressure `p` read `alt_mm_target` (bisection: the altitude rises with `p0`).
/// None if no sea-level pressure of 850 to 1100 hPa does.
///
/// ```
/// use bryton_rustfw::altitude::{alt_mm, p0_for, P_STD};
/// let p0 = p0_for(P_STD, 0).unwrap();
/// assert!(p0.abs_diff(P_STD) < 4, "at sea level on the standard day");
/// assert!(alt_mm(P_STD, p0_for(P_STD, 120_000).unwrap()).unwrap().abs_diff(120_000) < 100);
/// ```
pub fn p0_for(p: u32, alt_mm_target: i32) -> Option<u32> {
    // below 0.8 p the table's ratio passes 1.25 (a Dead Sea day, 1065 hPa, wants 852): start there if higher
    let (mut lo, mut hi) = ((*P0_RANGE.start()).max(p / 5 * 4 + 64), *P0_RANGE.end());
    if lo > hi || alt_mm(p, lo)? > alt_mm_target || alt_mm(p, hi)? < alt_mm_target {
        return None;
    }
    for _ in 0..24 {
        let mid = lo + (hi - lo) / 2;
        if alt_mm(p, mid)? < alt_mm_target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}

/// How a calibration was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// An altitude typed in.
    Typed = 1,
    /// FROM GPS in the menu.
    Gps = 2,
    /// At a ride's start.
    Auto = 3,
    /// Stock's `Alt_P0` (its age unknown).
    Stock = 4,
}

/// A calibration: the sea-level pressure, and when it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cal {
    /// Sea-level pressure, 1/4096 hPa.
    pub p0: u32,
    /// Seconds since 1970 on the unit's clock when made; 0: unknown (stock's).
    pub when_s: u32,
    /// How.
    pub by: By,
}

/// A calibration is trusted for this long (3 h, about 3 hPa of weather at the very most).
pub const FRESH_S: u32 = 3 * 3600;

/// Seconds of fix (and pressure) averaged for a calibration from the receiver.
pub const WINDOW_S: u32 = 30;

const MAGIC: &[u8; 4] = b"ALT1";

impl Cal {
    /// The file's sector.
    ///
    /// ```
    /// use bryton_rustfw::altitude::{By, Cal};
    /// let c = Cal { p0: 4_150_272, when_s: 1_800_000_000, by: By::Gps };
    /// let mut s = c.to_sector();
    /// assert_eq!(Cal::from_sector(&s), Some(c));
    /// s[5] ^= 1;
    /// assert_eq!(Cal::from_sector(&s), None);
    /// ```
    pub fn to_sector(&self) -> [u8; 512] {
        let mut s = [0; 512];
        s[..4].copy_from_slice(MAGIC);
        s[4..8].copy_from_slice(&self.p0.to_le_bytes());
        s[8..12].copy_from_slice(&self.when_s.to_le_bytes());
        s[12] = self.by as u8;
        let crc = crate::fit::crc(0, &s[..13]);
        s[13..15].copy_from_slice(&crc.to_le_bytes());
        s
    }

    /// From a sector [`Cal::to_sector`] wrote; None if it is not one, or its pressure is out of range.
    pub fn from_sector(s: &[u8; 512]) -> Option<Cal> {
        if &s[..4] != MAGIC || s[13..15] != crate::fit::crc(0, &s[..13]).to_le_bytes() {
            return None;
        }
        let by = match s[12] {
            1 => By::Typed,
            2 => By::Gps,
            3 => By::Auto,
            4 => By::Stock,
            _ => return None,
        };
        let p0 = u32::from_le_bytes([s[4], s[5], s[6], s[7]]);
        P0_RANGE.contains(&p0).then(|| Cal { p0, when_s: u32::from_le_bytes([s[8], s[9], s[10], s[11]]), by })
    }

    /// Made within [`FRESH_S`] of `now_s` (a clock that went back, or an unknown age, is not fresh).
    pub fn fresh(&self, now_s: u32) -> bool {
        self.when_s != 0 && now_s.checked_sub(self.when_s).is_some_and(|age| age < FRESH_S)
    }
}

/// What a running average is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    None,
    /// FROM GPS.
    Manual,
    /// A ride's start.
    Auto,
}

/// Readings missed in a row after which the barometer counts as silent.
const MISSES: u8 = 3;

/// Readings the median is taken over (module doc: a reading 15 m off, or two in a row, is dropped).
pub const MEDIAN: usize = 5;

/// The exponential average's divisor, per reading.
pub const SMOOTH: i64 = 6;

/// How fast a new reference's shift is let go, mm a second (module doc): 0.5 m/s, a 50 m correction in under 2 min.
pub const SLEW_MM: i32 = 500;

/// The barometer's smoothed pressure, the calibration and the average toward a new one.
#[derive(Clone, Copy, Debug)]
pub struct Altimeter {
    /// Smoothed pressure x 16 (1/65536 hPa).
    smooth: Option<u32>,
    /// The last readings, the newest last, and how many of them are valid (from the end).
    recent: [u32; MEDIAN],
    n_recent: u8,
    misses: u8,
    cal: Option<Cal>,
    /// The calibration may be used for the ride's altitude.
    trusted: bool,
    /// The reference number the ride sees (never 0: that is the receiver's).
    epoch: u8,
    want: Want,
    n: u32,
    alt_sum: i64,
    p_sum: u64,
    /// The calibration changed and is not in the file yet.
    dirty: bool,
    /// The last second of a ride: the reference the ride saw and the altitude given (with `off`).
    last: Option<(u8, i32)>,
    /// Added to a source's altitude so a change of source continues from the last value (the records show no step).
    off: i32,
    /// The part of the last second's change that was the shift let go ([`Altimeter::corr`]).
    corr: i32,
}

impl Default for Altimeter {
    fn default() -> Self {
        Altimeter::new()
    }
}

impl Altimeter {
    /// Nothing read, not calibrated.
    pub const fn new() -> Self {
        Altimeter {
            smooth: None,
            recent: [0; MEDIAN],
            n_recent: 0,
            misses: 0,
            cal: None,
            trusted: false,
            epoch: 1,
            want: Want::None,
            n: 0,
            alt_sum: 0,
            p_sum: 0,
            dirty: false,
            last: None,
            off: 0,
            corr: 0,
        }
    }

    /// A reading of the chip (1/4096 hPa), or None when it did not answer or was not ready. Out of the chip's range
    /// counts as no answer.
    pub fn feed(&mut self, raw: Option<u32>) {
        let Some(p) = raw.filter(|p| P_RANGE.contains(p)) else {
            self.misses = self.misses.saturating_add(1);
            if self.misses >= MISSES {
                (self.smooth, self.n_recent) = (None, 0);
            }
            return;
        };
        self.misses = 0;
        // a jump of 5 hPa is not weather (the first reading, a gap): start again from it
        if self.smooth.is_some_and(|s| s.abs_diff(p * 16) >= 5 * 4096 * 16) {
            (self.smooth, self.n_recent) = (None, 0);
        }
        self.recent.rotate_left(1);
        self.recent[MEDIAN - 1] = p;
        self.n_recent = (self.n_recent + 1).min(MEDIAN as u8);
        let mut w = self.recent;
        let w = &mut w[MEDIAN - usize::from(self.n_recent)..];
        w.sort_unstable();
        let x = w[w.len() / 2] * 16;
        self.smooth = Some(match self.smooth {
            Some(s) => (i64::from(s) + (i64::from(x) - i64::from(s)) / SMOOTH) as u32,
            None => x,
        });
    }

    /// The smoothed pressure, 1/4096 hPa; None while the barometer is silent.
    pub fn pressure(&self) -> Option<u32> {
        self.smooth.map(|s| s / 16)
    }

    /// The barometric altitude, mm (on the standard day's sea-level pressure when not calibrated).
    pub fn alt_mm(&self) -> Option<i32> {
        alt_mm(self.pressure()?, self.cal.map_or(P_STD, |c| c.p0))
    }

    /// The calibration.
    pub fn cal(&self) -> Option<Cal> {
        self.cal
    }

    /// The calibration read from the file, at start-up; trusted if fresh. Not over one made since.
    pub fn load(&mut self, cal: Cal, now_s: u32) {
        if self.cal.is_none_or(|c| c.by == By::Stock) {
            self.cal = Some(cal);
            self.trusted = cal.fresh(now_s);
            self.bump();
            (self.last, self.off, self.corr) = (None, 0, 0); // start-up (or a resume): what was followed so far is not the ride's
            if self.trusted && self.want == Want::Auto {
                (self.want, self.n, self.alt_sum, self.p_sum) = (Want::None, 0, 0, 0); // a ride's start waited for it
            }
        }
    }

    /// Stock's sea-level pressure (hPa x 10), when nothing better is known; marked for writing.
    pub fn import_stock(&mut self, hpa10: u32) -> bool {
        let p0 = hpa10 * 4096 / 10;
        if self.cal.is_some_and(|c| c.by != By::Stock) || !P0_RANGE.contains(&p0) {
            return false;
        }
        self.cal = Some(Cal { p0, when_s: 0, by: By::Stock });
        self.dirty = true;
        self.bump(); // the altitude moved: a new reference for the ride (it jumped 56 m with none)
        true
    }

    /// A new reference for the ride.
    fn bump(&mut self) {
        self.epoch = self.epoch.checked_add(1).unwrap_or(1);
    }

    fn set(&mut self, p0: u32, now_s: u32, by: By) {
        self.cal = Some(Cal { p0, when_s: now_s, by });
        self.trusted = true;
        self.dirty = true;
        self.bump();
        self.want = Want::None;
    }

    /// Calibrates so that the barometer reads `alt_mm` now. False if it is silent or no sea-level pressure fits.
    pub fn calibrate(&mut self, alt_mm: i32, now_s: u32) -> bool {
        let Some(p0) = self.pressure().and_then(|p| p0_for(p, alt_mm)) else { return false };
        self.set(p0, now_s, By::Typed);
        true
    }

    /// Starts averaging the receiver's altitude for a calibration (FROM GPS).
    pub fn from_gps(&mut self) {
        (self.want, self.n, self.alt_sum, self.p_sum) = (Want::Manual, 0, 0, 0);
    }

    /// A ride starts (or is resumed): if the calibration is not fresh at `now_s` (3 h, also within one power-up),
    /// average the receiver's altitude for one (module doc). The ride's altitude starts from the source's own.
    pub fn ride_start(&mut self, now_s: u32) {
        (self.last, self.off, self.corr) = (None, 0, 0);
        if !self.cal.is_some_and(|c| c.fresh(now_s)) {
            self.trusted = false;
        }
        if !self.trusted && self.want == Want::None {
            (self.want, self.n, self.alt_sum, self.p_sum) = (Want::Auto, 0, 0, 0);
        }
    }

    /// Seconds averaged so far for a calibration from the receiver; None when none is wanted.
    pub fn progress(&self) -> Option<u32> {
        (self.want != Want::None).then_some(self.n)
    }

    /// A calibration is being averaged for by hand (FROM GPS), not at a ride's start.
    pub fn manual(&self) -> bool {
        self.want == Want::Manual
    }

    /// The barometer reads (a calibration needs it).
    pub fn reads(&self) -> bool {
        self.smooth.is_some()
    }

    /// One second: `gps` is the receiver's altitude (mm) when it has a 3D fix with six satellites or more. A second
    /// without either, the average starts again (it wants [`WINDOW_S`] in a row).
    // ponytail: a plain mean; a median would resist a single wild altitude. Add if the owner sees one.
    pub fn second(&mut self, gps: Option<i32>, now_s: u32) {
        if self.want == Want::None {
            return;
        }
        let (Some(alt), Some(p)) = (gps.filter(|a| crate::ride::ALT_RANGE_MM.contains(a)), self.pressure()) else {
            (self.n, self.alt_sum, self.p_sum) = (0, 0, 0);
            return;
        };
        self.n += 1;
        self.alt_sum += i64::from(alt);
        self.p_sum += u64::from(p);
        if self.n < WINDOW_S {
            return;
        }
        let by = if self.want == Want::Auto { By::Auto } else { By::Gps };
        let (alt, p) = ((self.alt_sum / i64::from(WINDOW_S)) as i32, (self.p_sum / u64::from(WINDOW_S)) as u32);
        match p0_for(p, alt) {
            Some(p0) => self.set(p0, now_s, by),
            // no sea-level pressure fits: given up
            None => (self.want, self.n, self.alt_sum, self.p_sum) = (Want::None, 0, 0, 0),
        }
    }

    /// The altitude for the ride and its reference number: the barometer's when it reads and the calibration is
    /// trusted (or the receiver has nothing), else the receiver's (reference 0).
    pub fn source(&self, gps: Option<i32>) -> (Option<i32>, u8) {
        let (a, r, off) = self.pick(gps);
        (a.map(|a| a.saturating_add(off)), r)
    }

    /// The source chosen, its reference, and the shift to add to it: the one kept, or the one a change of source
    /// since the last [`Altimeter::follow`] calls for (so a reading between two seconds is shifted too).
    fn pick(&self, gps: Option<i32>) -> (Option<i32>, u8, i32) {
        let (a, r) = match self.alt_mm() {
            // the ride's start averaging a calibration: the barometer on its old one (module doc)
            Some(a) if self.trusted || gps.is_none() || self.want == Want::Auto => (Some(a), self.epoch),
            _ => (gps, 0),
        };
        let off = match (a, self.last) {
            (Some(a), Some((lr, last))) if lr != r => last.saturating_sub(a),
            _ => self.off,
        };
        (a, r, off)
    }

    /// Once a second while a ride is open, before [`Altimeter::source`]: a new reference (a calibration, the
    /// barometer to the receiver or back) is shifted to continue from the last value, and the shift is let go by
    /// [`SLEW_MM`] a second (module doc).
    pub fn follow(&mut self, gps: Option<i32>) {
        self.corr = 0;
        if let (Some(a), r, off) = self.pick(gps) {
            self.corr = -off.clamp(-SLEW_MM, SLEW_MM);
            self.off = off + self.corr;
            self.last = Some((r, a.saturating_add(self.off)));
        }
    }

    /// How much of the ride's altitude change in the last [`Altimeter::follow`] was a correction let go, not a climb
    /// (mm, signed): the ride takes it out of its ascent.
    pub fn corr(&self) -> i32 {
        self.corr
    }

    /// The calibration to write, once (None when the file has it).
    pub fn take_dirty(&mut self) -> Option<Cal> {
        let c = self.cal.filter(|_| self.dirty);
        self.dirty = false;
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    fn exact(p: u32, p0: u32) -> f64 {
        44307.694 * (1.0 - (f64::from(p) / f64::from(p0)).powf(0.190284))
    }

    #[test]
    fn table_matches_the_formula() {
        for (k, &v) in TABLE.iter().enumerate() {
            let r = (k as f64 + 16.0) / 64.0;
            assert!((f64::from(v) - 44307694.0 * (1.0 - r.powf(0.190284))).abs() < 1.0, "entry {k}");
        }
        for p in (260 * 4096..1260 * 4096).step_by(997) {
            if let Some(h) = alt_mm(p, P_STD) {
                let e = (f64::from(h) / 1000.0 - exact(p, P_STD)).abs();
                let limit = if h < 2_000_000 { 0.35 } else { 3.0 };
                assert!(e < limit, "{p}: {e}");
            }
        }
    }

    #[test]
    fn isa_reference_values() {
        // ICAO standard atmosphere: hPa at the altitude (m)
        for (hpa, m) in [(1013.25, 0.0), (898.746, 1000.0), (794.952, 2000.0), (701.085, 3000.0), (540.48, 5000.0)] {
            let h = f64::from(alt_mm((hpa * 4096.0) as u32, P_STD).unwrap()) / 1000.0;
            let tol = if m < 3500.0 { 1.5 } else { 6.0 }; // the formula is a fit of the ISA, a few m apart higher up
            assert!((h - m).abs() < tol, "{hpa} hPa: {h} m, ISA {m}");
        }
    }

    #[test]
    fn out_of_range_is_none() {
        assert_eq!(alt_mm(P_STD, 0), None);
        assert_eq!(alt_mm(100 * 4096, P_STD), None, "ratio 0.1");
        assert_eq!(alt_mm(2000 * 4096, P_STD), None);
        assert_eq!(p0_for(P_STD, 20_000_000), None);
        assert_eq!(p0_for(P_STD, -20_000_000), None);
    }

    #[test]
    fn calibration_by_a_typed_altitude() {
        let mut a = Altimeter::new();
        assert!(!a.calibrate(100_000, 5), "silent: refused");
        for _ in 0..4 {
            a.feed(Some(900 * 4096));
        }
        assert!(a.calibrate(1_200_000, 5));
        assert!(a.alt_mm().unwrap().abs_diff(1_200_000) < 100);
        for _ in 0..3 {
            a.feed(Some(899 * 4096)); // 1 hPa lower: about 9 m higher, once the median has it, smoothed a sixth
        }
        let h = a.alt_mm().unwrap();
        assert!((1_200_000..1_205_000).contains(&h), "{h}");
        assert_eq!(a.take_dirty().map(|c| c.by), Some(By::Typed));
        assert_eq!(a.take_dirty(), None, "once");
        assert!(!a.calibrate(i32::MAX, 5));
    }

    #[test]
    fn calibration_from_the_receiver() {
        let mut a = Altimeter::new();
        a.feed(Some(950 * 4096));
        a.from_gps();
        for s in 0..WINDOW_S {
            assert_eq!(a.progress(), Some(s));
            // a receiver 3 m noisy around 540 m
            a.second(Some(540_000 + [-3000, 2000, 1000, -1000, 3000][s as usize % 5]), 100);
        }
        assert_eq!(a.progress(), None, "done");
        assert!(a.alt_mm().unwrap().abs_diff(540_000) < 700);
        // a gap restarts it
        a.from_gps();
        a.second(Some(540_000), 1);
        a.second(None, 2);
        assert_eq!(a.progress(), Some(0));
    }

    #[test]
    fn trust_and_the_ride_start() {
        let mut a = Altimeter::new();
        a.feed(Some(950 * 4096));
        let day = 1_800_000_000;
        a.load(Cal { p0: P_STD, when_s: day - 4 * 3600, by: By::Typed }, day);
        a.ride_start(day);
        assert_eq!(a.progress(), Some(0), "four hours old: a new one");
        assert_ne!(a.source(Some(500_000)).1, 0, "the barometer meanwhile, on its old calibration");
        assert!(a.source(None).0.is_some() && a.source(None).1 != 0, "no fix: the barometer, as it is");
        for _ in 0..WINDOW_S {
            a.second(Some(500_000), day);
        }
        let (alt, r) = a.source(Some(500_000));
        assert!(alt.is_some() && r > 1, "a new reference");
        let mut b = Altimeter::new();
        b.load(Cal { p0: P_STD, when_s: day - 3600, by: By::Gps }, day);
        b.ride_start(day);
        assert_eq!(b.progress(), None, "an hour old: kept");
        let mut c = Altimeter::new();
        c.load(Cal { p0: P_STD, when_s: day + 5, by: By::Gps }, day);
        assert!(!c.cal().unwrap().fresh(day), "a clock gone back");
        assert!(!Cal { p0: P_STD, when_s: 0, by: By::Stock }.fresh(day));
    }

    #[test]
    fn silent_barometer_and_stock_import() {
        let mut a = Altimeter::new();
        a.feed(Some(900 * 4096));
        a.feed(None);
        a.feed(Some(5)); // out of the chip's range
        assert!(a.alt_mm().is_some(), "two misses");
        a.feed(None);
        assert_eq!((a.alt_mm(), a.source(Some(7)).1), (None, 0));
        assert!(a.import_stock(10200));
        assert_eq!(a.cal().map(|c| c.by), Some(By::Stock));
        assert!(!a.import_stock(5000), "out of range");
        let mut b = Altimeter::new();
        b.load(Cal { p0: P_STD, when_s: 9, by: By::Typed }, 9);
        assert!(!b.import_stock(10200), "not over one made here");
    }

    #[test]
    fn a_climb_is_smooth() {
        // 1 m a second up with a few counts of noise: the smoothed altitude follows within the filter's lag
        let mut a = Altimeter::new();
        let mut x = 12345u32;
        for s in 0..200u32 {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (x >> 16) as i32 % 5 - 2;
            let p = (f64::from(P_STD) * (1.0 - f64::from(s) / 8434.0)) as i32 + noise * 3;
            a.feed(Some(p as u32));
            if s > 20 {
                let h = f64::from(a.alt_mm().unwrap()) / 1000.0;
                assert!((h - f64::from(s)).abs() < 8.5, "{s}: {h}"); // lag of the filter: 2 + 5 s of climb
            }
        }
    }

    #[test]
    fn a_calibration_expires_in_a_power_up() {
        let mut a = Altimeter::new();
        for _ in 0..10 {
            a.feed(Some(P_STD));
        }
        assert!(a.calibrate(0, 1_000_000));
        a.ride_start(1_000_000 + 600);
        assert_eq!(a.progress(), None, "ten minutes: kept");
        a.ride_start(1_000_000 + 6 * 3600);
        assert_eq!(a.progress(), Some(0), "6 h later in the same power-up: made again");
        assert_ne!(a.source(Some(5)).1, 0, "and the barometer meanwhile");
    }

    #[test]
    fn importing_stock_is_a_new_reference() {
        let mut a = Altimeter::new();
        for _ in 0..10 {
            a.feed(Some(P_STD));
        }
        let (h0, e0) = a.source(None);
        assert!(a.import_stock(10200));
        let (h1, e1) = a.source(None);
        assert_ne!(e0, e1, "the altitude moved {} mm", h1.unwrap() - h0.unwrap());
    }

    #[test]
    fn low_places_calibrate() {
        // the Dead Sea's shore, -430 m, on a 1013 hPa day: about 1065 hPa; and -500 m
        for (hpa, m) in [(1065, -430), (1070, -500), (1080, -500)] {
            let p = hpa * 4096;
            let p0 = p0_for(p, m * 1000).unwrap_or_else(|| panic!("{hpa} hPa at {m} m"));
            assert!(alt_mm(p, p0).unwrap().abs_diff(m * 1000) < 100);
        }
    }

    /// The pressure at `m` metres on the standard day, 1/4096 hPa.
    fn p_at(m: f64) -> u32 {
        (f64::from(P_STD) * (1.0 - m / 44307.694).powf(1.0 / 0.190284)) as u32
    }

    /// A trusted barometer at `at_m` and a ride started at `now`.
    fn riding(at_m: f64, now: u32) -> Altimeter {
        let mut a = Altimeter::new();
        for _ in 0..MEDIAN {
            a.feed(Some(p_at(at_m)));
        }
        assert!(a.calibrate((at_m * 1000.0) as i32, now));
        a.ride_start(now);
        assert_eq!(a.progress(), None);
        a
    }

    #[test]
    fn a_change_of_source_continues_then_moves_over_slowly() {
        let mut a = riding(100.0, 1000);
        a.follow(None);
        let (v0, r0) = a.source(None);
        let v0 = v0.unwrap();
        assert_ne!(r0, 0);
        // the barometer goes silent, a fix 40 m higher: the receiver's, shifted to the last value, then let go
        for _ in 0..MISSES {
            a.feed(None);
        }
        let (mut last, mut corr) = (v0, 0);
        for s in 0..120 {
            a.follow(Some(v0 + 40_000));
            let (v, r) = a.source(Some(v0 + 40_000));
            assert_eq!(r, 0);
            let v = v.unwrap();
            assert!((v - last).abs() <= SLEW_MM, "{s}: a step of {} mm", v - last);
            assert_eq!(v - last, a.corr(), "{s}: all of the change is the correction");
            (last, corr) = (v, corr + a.corr());
        }
        assert_eq!((last, corr), (v0 + 40_000, 40_000), "at the receiver's level, the 40 m all correction");
        // the receiver climbs 1 m: 1 m in the records, no correction
        a.follow(Some(v0 + 41_000));
        assert_eq!((a.source(Some(v0 + 41_000)).0, a.corr()), (Some(v0 + 41_000), 0));
        // a new ride starts from the source's own
        a.ride_start(0);
        assert_eq!(a.source(Some(7)).0, Some(7));
    }

    /// A ride through the barometer: `alt(s)` the true altitude (m) at second s, `extra(s)` the pressure added at
    /// the vent (Pa), and a calibration typed `cal_m` off at second `cal_at`. The ride's ascent (m) and the largest
    /// step of its altitude in a second (mm).
    fn ride_through(
        secs: u32,
        alt: impl Fn(u32) -> f64,
        extra: impl Fn(u32) -> f64,
        cal_at: u32,
        cal_m: f64,
    ) -> (u32, i32) {
        let mut a = riding(alt(0), 1000);
        let mut r = crate::ride::Ride::new();
        r.start_pause();
        let (mut prev, mut step): (Option<i32>, i32) = (None, 0);
        for s in 0..secs {
            let p = f64::from(p_at(alt(s))) + extra(s) * 40.96; // 1 Pa is 40.96 of the chip's counts
            a.feed(Some(p as u32));
            if s == cal_at {
                let now = a.alt_mm().unwrap();
                assert!(a.calibrate(now + (cal_m * 1000.0) as i32, 1000 + s));
            }
            a.follow(None);
            let (h, alt_ref) = a.source(None);
            if let (Some(h), Some(p)) = (h, prev) {
                step = step.max((h - p).abs());
            }
            prev = h;
            let i = Input { speed_x10: Some(300), alt_mm: h, alt_ref, alt_corr_mm: a.corr(), ..Input::default() };
            r.second(&i);
        }
        (r.ascent_m(), step)
    }

    use crate::ride::Input;

    /// Pseudo-random in -1..1, seeded by the second.
    fn noise(s: u32) -> f64 {
        let x = s.wrapping_mul(2_654_435_761).wrapping_add(12345);
        let x = x ^ (x >> 15);
        f64::from(x % 2001) / 1000.0 - 1.0
    }

    /// The air at the vent as the 2026-10-26 ride showed it: about +-25 Pa of gusts riding (+-2 m), speed changes
    /// of 10 km/h every 40 s, and a reading 150 Pa off every 50 s, every other one followed by one of the opposite
    /// sign. The vent sees 0.3 of the dynamic pressure 0.5 rho v^2 (30 to 40 km/h: 32 Pa, 10 Pa of it): the ride's
    /// altitude against its speed changes gave a slope of 0.28 of it, an upper bound since descents speed the bike up
    /// too (inferred; three stops on that ride rose 1 to 3 m). The full 32 Pa would count 2.7 m each change: if a
    /// ride shows that, compensate for the speed.
    fn vent(s: u32) -> f64 {
        let q = |kmh: f64| 0.5 * 1.2 * (kmh / 3.6) * (kmh / 3.6);
        let speed = if (s / 40).is_multiple_of(2) { 30.0 } else { 40.0 };
        let spike = match s % 100 {
            7 | 57 => 150.0,
            8 => -150.0,
            _ => 0.0,
        };
        25.0 * noise(s) + 0.3 * q(speed) + spike
    }

    #[test]
    fn a_flat_road_with_the_air_at_the_vent_climbs_nothing() {
        let (ascent, step) = ride_through(3600, |_| 250.0, vent, u32::MAX, 0.0);
        assert!(ascent <= 3, "{ascent} m on a flat road");
        assert!(step < 1000, "the altitude moved {step} mm in a second");
    }

    #[test]
    fn a_calibration_mid_ride_climbs_nothing() {
        for cal_m in [30.0, -30.0] {
            let (ascent, step) = ride_through(1200, |_| 250.0, |_| 0.0, 300, cal_m);
            assert_eq!(ascent, 0, "a calibration of {cal_m} m");
            assert!(step <= SLEW_MM + 50, "no step: {step} mm");
        }
    }

    #[test]
    fn a_known_climb_counts_within_a_few_percent() {
        // flat, 200 m up at 0.4 m/s (8 % at 18 km/h), flat, 50 m down, 60 m up: 260 m, with the air at the vent
        let alt = |s: u32| {
            let s = f64::from(s);
            match s {
                _ if s < 300.0 => 100.0,
                _ if s < 800.0 => 100.0 + (s - 300.0) * 0.4,
                _ if s < 1000.0 => 300.0,
                _ if s < 1100.0 => 300.0 - (s - 1000.0) * 0.5,
                _ if s < 1400.0 => 250.0 + (s - 1100.0) * 0.2,
                _ => 310.0,
            }
        };
        let (ascent, _) = ride_through(1700, alt, vent, u32::MAX, 0.0);
        assert!(ascent.abs_diff(260) <= 8, "{ascent} m for 260");
    }

    #[test]
    fn the_start_rides_on_the_barometer_while_it_calibrates() {
        // a calibration four hours old: the start averages a new one, the barometer carries the ride meanwhile, and
        // the receiver's metres of noise never reach the ride; the new level comes in with no step and no ascent
        let mut a = riding(250.0, 1000);
        let day = 1000 + 4 * 3600;
        a.ride_start(day);
        assert_eq!(a.progress(), Some(0));
        let mut r = crate::ride::Ride::new();
        r.start_pause();
        let mut prev: Option<i32> = None;
        for s in 0..300 {
            a.feed(Some(p_at(250.0)));
            let gps = 262_000 + (noise(s) * 15_000.0) as i32; // 12 m higher, +-15 m
            a.second(Some(gps), day + s);
            a.follow(Some(gps));
            let (h, alt_ref) = a.source(Some(gps));
            let h = h.unwrap();
            assert_ne!(alt_ref, 0, "{s}: the barometer");
            if let Some(p) = prev {
                assert!((h - p).abs() <= SLEW_MM, "{s}: a step of {} mm", h - p);
            }
            prev = Some(h);
            r.second(&Input {
                speed_x10: Some(300),
                alt_mm: Some(h),
                alt_ref,
                alt_corr_mm: a.corr(),
                ..Input::default()
            });
        }
        assert_eq!(a.progress(), None, "calibrated");
        assert_eq!(r.ascent_m(), 0);
        assert!(prev.unwrap().abs_diff(262_000) < 3000, "at the receiver's level now: {prev:?}");
    }
}
