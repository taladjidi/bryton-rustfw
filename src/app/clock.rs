//! The RTC's registers decoded, and dates: BCD fields, civil dates from day numbers and back, seconds since 1970;
//! the shown clock (TIME ZONE, SUMMER TIME). Pure functions, so the doc tests run them on the host.
//!
//! **What the RTC holds, and who sets it.** Local time, never UTC (stock runs it that way, and its ride names are
//! local). This firmware sets it from one source only: the phone link's Current Time Service (app/phone.rs
//! `phone_time`: the phone's local time, between rides only). The GNSS receiver never sets it: at a fix with a valid
//! time, `learn_aid` (app/gps.rs) learns the RTC's offset from UTC (RTC minus UTC, to the nearest 15 minutes) into
//! the backup registers, and the phone's Local Time Information replaces it. Stock firmware sets the RTC too: from the
//! GPS with its own time zone and DST (`Clk_ApplyGpsTimeZone`) and from the Bryton app's UTC plus its configured
//! offset (`Clk_SetRtcFromHostTime`) (the bryton repo's ghidra/notes/agent_b1.md; inferred from the decompiled names
//! and strings). Its INI files hold no time zone. So without the phone or stock, nothing moves the RTC at a DST
//! change: under TIME ZONE CLOCK (the default) the shown time stays as the RTC holds it, as before.
//!
//! **TIME ZONE set: the RTC is kept as is, the shown clock is converted.** UTC is the RTC minus the learnt offset
//! (which each fix keeps right), and the shown time is [`local_time`] of it ([`App::show_clock`]). Everything that
//! reads a local time (the strip, the dates, the ride file's stock-style name, the FAT times, the sun's times) reads
//! the shown one; the FIT times are the shown time minus the shown offset, which is the RTC minus the learnt offset
//! whatever the zone: they stay monotonic across a DST change mid-ride and across a resume, and no RTC write can be
//! cut apart from its offset's (setting the RTC at the change instead would need both, a backup register and the
//! RTC, written together, and is refused during a ride anyway). A ride keeps the name it started with (its recovery
//! record holds it), so a resume after the change reopens the same file. Until an offset is learnt (no fix, no phone
//! yet) the RTC is shown as is; so it is until the settings file is read, 2 s after a start ([`PAGES_LOAD_MS`]).
//!
//! The DST rules: the EU's, from the last Sunday of March to the last Sunday of October, 01:00 UTC both (Directive
//! 2000/84/EC, articles 2 and 3, <https://eur-lex.europa.eu/eli/dir/2000/84/oj>); the US's, from the second Sunday of
//! March to the first Sunday of November, 02:00 local time both (15 U.S.C. 260a,
//! <https://www.law.cornell.edu/uscode/text/15/260a>).

use crate::*;

/// Two BCD digits to binary.
///
/// ```
/// assert_eq!(bryton_rustfw::bcd(0x59), 59);
/// ```
pub fn bcd(v: u32) -> u8 {
    ((v >> 4) * 10 + (v & 0xf)) as u8
}

/// RTC_TR (RM0399: BCD hours in bits 16..21, minutes 8..14, seconds 0..6; PM at bit 22 is ignored, the
/// stock firmware runs the RTC in 24 h format) to (hours, minutes, seconds).
///
/// ```
/// assert_eq!(bryton_rustfw::rtc_time_from_tr(0x0023_5959), (23, 59, 59));
/// ```
pub fn rtc_time_from_tr(tr: u32) -> (u8, u8, u8) {
    (bcd((tr >> 16) & 0x3f), bcd((tr >> 8) & 0x7f), bcd(tr & 0x7f))
}

/// RTC_DR (RM0399: BCD year 00-99 in bits 16..23, month 8..12, day 0..5) to (year, month, day), years 2000-2099.
///
/// ```
/// assert_eq!(bryton_rustfw::rtc_date_from_dr(0x0026_5023), (2026, 10, 23)); // weekday bits 13..15 ignored
/// ```
pub fn rtc_date_from_dr(dr: u32) -> (u16, u8, u8) {
    (2000 + u16::from(bcd((dr >> 16) & 0xff)), bcd((dr >> 8) & 0x1f), bcd(dr & 0x3f))
}

/// Days since 1970-01-01 of a date (proleptic Gregorian; H. Hinnant's days_from_civil).
pub fn days_from_civil(y: u16, m: u8, d: u8) -> i32 {
    let y = i32::from(y) - i32::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i32::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i32::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The date `days` after 1970-01-01 (the inverse of [`days_from_civil`]).
///
/// ```
/// use bryton_rustfw::{civil_from_days, days_from_civil};
/// assert_eq!(days_from_civil(2026, 10, 23), 20_749);
/// assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
/// ```
pub fn civil_from_days(days: i32) -> (u16, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((yoe + era * 400 + i32::from(m <= 2)) as u16, m, d)
}

/// A date and time as seconds since 1970 (no leap seconds).
pub fn epoch_s((y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8)) -> i64 {
    i64::from(days_from_civil(y, mo, d)) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s)
}

/// Seconds since 1970 back to a date and time.
///
/// ```
/// let t = (2026, 1, 1, 0, 30, 0);
/// assert_eq!(bryton_rustfw::from_epoch_s(bryton_rustfw::epoch_s(t) - 3600), (2025, 12, 31, 23, 30, 0));
/// ```
pub fn from_epoch_s(t: i64) -> (u16, u8, u8, u8, u8, u8) {
    let (days, s) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days as i32);
    (y, mo, d, (s / 3600) as u8, (s / 60 % 60) as u8, (s % 60) as u8)
}

/// RTC_TR and RTC_DR for a date and time (RM0399: BCD as [`rtc_time_from_tr`] and [`rtc_date_from_dr`] read
/// them, the weekday in DR bits 13..15, Monday 1 to Sunday 7: 0 is forbidden).
///
/// ```
/// use bryton_rustfw::*;
/// let (tr, dr) = rtc_words((2026, 10, 24, 15, 30, 5));
/// assert_eq!((rtc_time_from_tr(tr), rtc_date_from_dr(dr)), ((15, 30, 5), (2026, 10, 24)));
/// assert_eq!(dr >> 13 & 7, 6, "a Saturday");
/// ```
pub fn rtc_words((y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8)) -> (u32, u32) {
    let bcd = |v: u8| u32::from(((v / 10) << 4) | (v % 10));
    let weekday = (days_from_civil(y, mo, d) + 3).rem_euclid(7) as u32 + 1; // 1970-01-01 was a Thursday (4)
    let tr = bcd(h) << 16 | bcd(mi) << 8 | bcd(s);
    (tr, bcd((y % 100) as u8) << 16 | weekday << 13 | bcd(mo) << 8 | bcd(d))
}

/// The time of day, hours and minutes.
pub(crate) fn rtc_hm(b: &mut impl Board) -> (u8, u8) {
    let (h, m, _) = rtc_time_from_tr(b.rtc().0);
    (h, m)
}

/// SUMMER TIME (daylight saving): the rule that moves the shown clock an hour forward in summer (module doc).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dst {
    /// No summer time: the default.
    #[default]
    Off,
    /// The EU's: last Sunday of March to last Sunday of October, 01:00 UTC.
    Eu,
    /// The US's: second Sunday of March to first Sunday of November, 02:00 local.
    Us,
}

impl Dst {
    /// The next choice (the list wraps).
    pub fn next(self) -> Self {
        match self {
            Dst::Off => Dst::Eu,
            Dst::Eu => Dst::Us,
            Dst::Us => Dst::Off,
        }
    }

    /// The choice a settings byte names (its position in the list), None for any other byte.
    pub fn from_byte(b: u8) -> Option<Self> {
        [Dst::Off, Dst::Eu, Dst::Us].get(usize::from(b)).copied()
    }
}

/// Days since 1970 of the `n`th Sunday of month `m` of year `y`; `n` 0 is the last.
///
/// ```
/// use bryton_rustfw::{civil_from_days, nth_sunday};
/// assert_eq!(civil_from_days(nth_sunday(2026, 3, 0)), (2026, 3, 29));
/// assert_eq!(civil_from_days(nth_sunday(2026, 3, 2)), (2026, 3, 8), "March 1st 2026 is a Sunday");
/// assert_eq!(civil_from_days(nth_sunday(2026, 11, 1)), (2026, 11, 1));
/// assert_eq!(civil_from_days(nth_sunday(2026, 12, 0)), (2026, 12, 27), "over the year's end and back");
/// ```
pub fn nth_sunday(y: u16, m: u8, n: u8) -> i32 {
    let sunday_on_or_before = |d: i32| d - (d + 4).rem_euclid(7); // 1970-01-01 was a Thursday (4), Sunday is 0
    if n == 0 {
        let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        sunday_on_or_before(days_from_civil(ny, nm, 1) - 1)
    } else {
        sunday_on_or_before(days_from_civil(y, m, 1) + 6) + 7 * (i32::from(n) - 1)
    }
}

/// The local time (seconds since 1970) of `utc` in the zone `zone_min` minutes east of UTC, an hour more in summer
/// by the rule `dst` (module doc).
///
/// ```
/// use bryton_rustfw::{Dst, epoch_s, local_time};
/// let paris = |t| local_time(epoch_s(t), 60, Dst::Eu);
/// assert_eq!(paris((2026, 7, 1, 10, 0, 0)), epoch_s((2026, 7, 1, 12, 0, 0)), "summer: UTC + 2 h");
/// assert_eq!(paris((2026, 10, 25, 0, 59, 59)), epoch_s((2026, 10, 25, 2, 59, 59)));
/// assert_eq!(paris((2026, 10, 25, 1, 0, 0)), epoch_s((2026, 10, 25, 2, 0, 0)), "back an hour at 01:00 UTC");
/// let new_york = |t| local_time(epoch_s(t), -300, Dst::Us);
/// assert_eq!(new_york((2026, 3, 8, 7, 0, 0)), epoch_s((2026, 3, 8, 3, 0, 0)), "02:00 EST is 03:00 EDT");
/// assert_eq!(local_time(1000, 330, Dst::Off), 1000 + 330 * 60);
/// ```
pub fn local_time(utc: i64, zone_min: i16, dst: Dst) -> i64 {
    let standard = utc + i64::from(zone_min) * 60;
    let at = |day: i32, h: i64| i64::from(day) * 86_400 + h * 3600;
    let summer = match dst {
        Dst::Off => false,
        // in UTC: 01:00 UTC at both ends
        Dst::Eu => {
            let y = from_epoch_s(utc).0;
            (at(nth_sunday(y, 3, 0), 1)..at(nth_sunday(y, 10, 0), 1)).contains(&utc)
        }
        // in standard time: 02:00 standard at the start, 02:00 daylight (01:00 standard) at the end
        Dst::Us => {
            let y = from_epoch_s(standard).0;
            (at(nth_sunday(y, 3, 2), 2)..at(nth_sunday(y, 11, 1), 1)).contains(&standard)
        }
    };
    standard + if summer { 3600 } else { 0 }
}

impl App {
    /// Reads the RTC into the shown clock ([`App::show_clock`]); its seconds, which the zone never changes.
    pub(crate) fn read_clock(&mut self, b: &mut impl Board) -> u8 {
        let (tr, dr) = b.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
        self.show_clock((y, mo, d, h, m, s));
        s
    }

    /// The shown date and time (the strip's hours and minutes, the date) from the RTC's `rtc`: as is under TIME ZONE
    /// CLOCK or before an offset from UTC is learnt, else [`local_time`] of its UTC (module doc). The difference is
    /// kept for the FIT times ([`App::rec_time`]) and the sun's.
    pub(crate) fn show_clock(&mut self, rtc: recorder::Local) {
        let rtc_s = epoch_s(rtc);
        let delta = match (self.settings.zone_min(), self.gps.aid.offset_15min) {
            (Some(zone), Some(o)) => local_time(rtc_s - i64::from(o) * 900, zone, self.settings.dst) - rtc_s,
            _ => 0,
        };
        // whole quarter hours, under a day and a half: the seconds stay the RTC's
        self.clock_delta_s = delta as i32;
        let (y, mo, d, h, m, _) = from_epoch_s(rtc_s + delta);
        (self.date, self.strip.time) = ((y, mo, d), (h, m));
    }

    /// The shown clock's offset from UTC, seconds: the learnt offset (0 before one) moved by TIME ZONE.
    pub(crate) fn shown_offset_s(&self) -> i64 {
        i64::from(self.gps.aid.offset_15min.unwrap_or(0)) * 900 + i64::from(self.clock_delta_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Time = (u16, u8, u8, u8, u8, u8);

    fn paris(t: Time) -> Time {
        from_epoch_s(local_time(epoch_s(t), 60, Dst::Eu))
    }

    fn new_york(t: Time) -> Time {
        from_epoch_s(local_time(epoch_s(t), -300, Dst::Us))
    }

    /// The EU's two changes, 01:00 UTC on the last Sundays of March and October (2026: the 29th and the 25th; 2027:
    /// the 28th and the 31st), a second either side, and the same instant in every EU zone.
    #[test]
    fn eu_changes_at_one_utc() {
        assert_eq!(paris((2026, 3, 29, 0, 59, 59)), (2026, 3, 29, 1, 59, 59));
        assert_eq!(paris((2026, 3, 29, 1, 0, 0)), (2026, 3, 29, 3, 0, 0));
        assert_eq!(paris((2026, 10, 25, 0, 59, 59)), (2026, 10, 25, 2, 59, 59));
        assert_eq!(paris((2026, 10, 25, 1, 0, 0)), (2026, 10, 25, 2, 0, 0));
        assert_eq!(paris((2027, 3, 28, 1, 0, 0)), (2027, 3, 28, 3, 0, 0));
        assert_eq!(paris((2027, 10, 31, 0, 59, 59)), (2027, 10, 31, 2, 59, 59));
        assert_eq!(paris((2027, 10, 31, 1, 0, 0)), (2027, 10, 31, 2, 0, 0));
        let at = |zone, t| from_epoch_s(local_time(epoch_s(t), zone, Dst::Eu));
        assert_eq!(at(0, (2026, 10, 25, 1, 0, 0)), (2026, 10, 25, 1, 0, 0), "Lisbon");
        assert_eq!(at(120, (2026, 10, 25, 0, 59, 59)), (2026, 10, 25, 3, 59, 59), "Helsinki");
        assert_eq!(at(120, (2026, 10, 25, 1, 0, 0)), (2026, 10, 25, 3, 0, 0));
    }

    /// The US's two changes, 02:00 local on the second Sunday of March and the first of November (2026: the 8th and
    /// the 1st), in New York (UTC - 5) and Los Angeles (UTC - 8).
    #[test]
    fn us_changes_at_two_local() {
        assert_eq!(new_york((2026, 3, 8, 6, 59, 59)), (2026, 3, 8, 1, 59, 59));
        assert_eq!(new_york((2026, 3, 8, 7, 0, 0)), (2026, 3, 8, 3, 0, 0));
        assert_eq!(new_york((2026, 11, 1, 5, 59, 59)), (2026, 11, 1, 1, 59, 59), "01:59:59 EDT");
        assert_eq!(new_york((2026, 11, 1, 6, 0, 0)), (2026, 11, 1, 1, 0, 0), "back to 01:00 EST");
        let la = |t| from_epoch_s(local_time(epoch_s(t), -480, Dst::Us));
        assert_eq!(la((2026, 3, 8, 9, 59, 59)), (2026, 3, 8, 1, 59, 59));
        assert_eq!(la((2026, 3, 8, 10, 0, 0)), (2026, 3, 8, 3, 0, 0));
        assert_eq!(la((2026, 11, 1, 8, 59, 59)), (2026, 11, 1, 1, 59, 59));
        assert_eq!(la((2026, 11, 1, 9, 0, 0)), (2026, 11, 1, 1, 0, 0));
    }

    /// The ambiguous hour (autumn) is shown twice, from two UTC instants an hour apart; the skipped one (spring) never.
    /// The shown time is a function of UTC, so nothing that counts in UTC (the FIT times) repeats or jumps.
    #[test]
    fn ambiguous_and_skipped_local_hours() {
        let shown = |t0: Time, f: fn(Time) -> Time| (0..3 * 3600).map(move |k| f(from_epoch_s(epoch_s(t0) + k)));
        let twice = shown((2026, 10, 24, 23, 30, 0), paris).filter(|t| (t.3, t.4, t.5) == (2, 30, 0)).count();
        assert_eq!(twice, 2, "02:30 in Paris on 2026-10-25");
        assert!(shown((2026, 3, 29, 0, 0, 0), paris).all(|t| t.3 != 2), "no 02:xx in Paris on 2026-03-29");
        let twice = shown((2026, 11, 1, 4, 30, 0), new_york).filter(|t| (t.3, t.4, t.5) == (1, 30, 0)).count();
        assert_eq!(twice, 2, "01:30 in New York on 2026-11-01");
        assert!(shown((2026, 3, 8, 6, 0, 0), new_york).all(|t| t.3 != 2), "no 02:xx in New York on 2026-03-08");
    }

    /// Midnight and the year's end: the zone moves the date; neither rule is in force in winter.
    #[test]
    fn midnight_and_year_end() {
        assert_eq!(paris((2026, 12, 31, 23, 30, 0)), (2027, 1, 1, 0, 30, 0));
        assert_eq!(new_york((2027, 1, 1, 3, 0, 0)), (2026, 12, 31, 22, 0, 0));
        assert_eq!(paris((2026, 7, 31, 22, 0, 0)), (2026, 8, 1, 0, 0, 0), "summer: + 2 h over midnight");
        assert_eq!(new_york((2026, 6, 30, 3, 59, 0)), (2026, 6, 29, 23, 59, 0), "summer: - 4 h");
        let fixed = |zone, t| from_epoch_s(local_time(epoch_s(t), zone, Dst::Off));
        assert_eq!(fixed(540, (2026, 12, 31, 15, 0, 0)), (2027, 1, 1, 0, 0, 0), "Tokyo");
        assert_eq!(fixed(14 * 60, (2026, 2, 28, 10, 0, 0)), (2026, 3, 1, 0, 0, 0), "UTC + 14");
        assert_eq!(fixed(-12 * 60, (2028, 3, 1, 11, 0, 0)), (2028, 2, 29, 23, 0, 0), "UTC - 12, a leap day");
    }
}
