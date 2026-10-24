//! The RTC's registers decoded, and dates: BCD fields, civil dates from day numbers and back, seconds since 1970.
//! Pure functions, so the doc tests run them on the host.

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
