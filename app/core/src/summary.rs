//! What the ride list and the weekly panel need from a ride, and the weekly sums.
//!
//! **Definitions, as close to Intervals.icu as could be checked** (looked up 2026-11-08). Only the week start is
//! stated by a cited page; the rest is a forum summary or inferred, and so marked. The numbers can differ from
//! Intervals.icu's for the same file.
//! - **Week:** starts on Monday. A forum reply says Monday "is very much hardcoded" in Intervals.icu's calendar and a
//!   Sunday option is requested (<https://forum.intervals.icu/t/changing-start-of-the-week/3235>,
//!   <https://forum.intervals.icu/t/option-to-start-week-on-sunday/47887>); `weekly_totals` still takes the day. A ride
//!   belongs to the week of its local start day, taken from the unit's file name (`YYMMDDhhmmss` is the unit's local
//!   time); inferred: Intervals shows an activity on its local start date.
//! - **Moving time:** Intervals.icu derives it from the speed stream (or distance, or GPS) and counts a whole indoor
//!   ride without one (forum.intervals.icu/t/moving-time-is-different/108707, <https://forum.intervals.icu/t/intervals-icu-faq/123707/5>;
//!   a staff post is reported to give 0.05 m/s, **inferred, not confirmed by the cited page**). Here a second is moving
//!   at or above the unit's own auto-pause speed (2 km/h, `ride::AUTO_PAUSE_X10`), so the phone and the unit agree;
//!   each record counts for the time to the next, at most 5 s, so a longer gap is a pause (inferred). The file's
//!   total_timer_time is not used.
//! - **Distance:** the session's total_distance, else the last record's (inferred).
//! - **Load with power:** TSS from NP over the moving seconds, NP from moving seconds only (the firmware's
//!   `power::tss`). A forum answer says Intervals' load from power is TrainingPeaks' "except that not moving time is
//!   excluded" (<https://forum.intervals.icu/t/how-is-calculated-training-load/1661>); its NP method is not confirmed
//!   to be the same (inferred).
//! - **Load without power:** Intervals' default HR load is a regression on time in HR zones (staff reply in
//!   <https://forum.intervals.icu/t/how-training-load-estimation-from-hr-works/280>); this app uses Banister TRIMP as an
//!   approximation (inferred): rest 60 bpm, 0.64 e^(1.92 x reserve), against an hour at 85 % of the max HR, times 100.
//!   It is not a TSS: the app marks it ("HR load", "TSS*").

use bryton_rustfw::ride::AUTO_PAUSE_X10;

/// Speed under which a second is not moving, m/s: the unit's auto-pause speed (km/h x 10 to m/s).
pub const MOVING_MS: f64 = AUTO_PAUSE_X10 as f64 / 36.0;

/// Seconds at one heart rate while moving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct HrBin {
    /// Heart rate, bpm.
    pub bpm: u8,
    /// Moving seconds at it.
    pub secs: u32,
}

/// A ride reduced to what the list caches: enough to recompute the load for another FTP or max HR.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideBrief {
    /// Start, Unix seconds.
    pub start_unix: i64,
    /// Start to end, s.
    pub elapsed_s: u32,
    /// Moving time, s.
    pub moving_s: u32,
    /// Distance, m.
    pub distance_m: f64,
    /// Normalized power over the moving seconds, W; 0: no power.
    pub np: u16,
    /// Moving seconds counted for the power (from the first reading).
    pub power_s: u32,
    /// Moving seconds per heart rate.
    pub hr_bins: Vec<HrBin>,
}

const REST_HR: f64 = 60.0;

fn trimp(bpm: f64, secs: f64, max_hr: f64) -> f64 {
    let r = ((bpm - REST_HR) / (max_hr - REST_HR)).clamp(0.0, 1.0);
    secs / 60.0 * r * 0.64 * (1.92 * r).exp()
}

/// The load of a ride against `ftp` (W) and `max_hr` (bpm), and whether it came from heart rate. Power when there is
/// power and an FTP, else heart rate when there is a max HR.
pub fn brief_load(b: &RideBrief, ftp: u16, max_hr: u16) -> (Option<u32>, bool) {
    if b.np > 0 && ftp > 0 {
        return (Some(bryton_rustfw::power::tss(b.power_s, b.np, ftp)), false);
    }
    let m = f64::from(max_hr);
    if b.hr_bins.is_empty() || m <= REST_HR + 10.0 {
        return (None, false);
    }
    let sum: f64 = b.hr_bins.iter().map(|x| trimp(f64::from(x.bpm), f64::from(x.secs), m)).sum();
    let hour = trimp(0.85 * m, 3600.0, m);
    (Some((sum / hour * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32), true)
}

/// Most heart rate bins a cached brief may carry.
const HR_BINS_MAX: usize = 256;

/// The brief of a FIT file (`None`: not a ride).
#[uniffi::export]
pub fn ride_brief(bytes: Vec<u8>) -> Option<RideBrief> {
    crate::ride::analyze_ride(bytes, 0, 0).ok().map(|v| v.summary.brief)
}

/// The brief as one line of text, for the app's cache.
#[uniffi::export]
pub fn brief_encode(b: RideBrief) -> String {
    let bins: Vec<String> = b.hr_bins.iter().map(|x| format!("{}:{}", x.bpm, x.secs)).collect();
    format!(
        "1 {} {} {} {} {} {} {}",
        b.start_unix,
        b.elapsed_s,
        b.moving_s,
        b.distance_m,
        b.np,
        b.power_s,
        bins.join(",")
    )
}

/// The brief back from [`brief_encode`]'s text (`None`: not that format).
#[uniffi::export]
pub fn brief_decode(text: String) -> Option<RideBrief> {
    let mut it = text.split(' ');
    (it.next()? == "1").then_some(())?;
    let b = RideBrief {
        start_unix: it.next()?.parse().ok()?,
        elapsed_s: it.next()?.parse().ok()?,
        moving_s: it.next()?.parse().ok()?,
        distance_m: it.next()?.parse().ok().filter(|d: &f64| d.is_finite() && *d >= 0.0)?,
        np: it.next()?.parse().ok()?,
        power_s: it.next()?.parse().ok()?,
        hr_bins: it
            .next()?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| {
                let (a, c) = s.split_once(':')?;
                Some(HrBin { bpm: a.parse().ok()?, secs: c.parse().ok()? })
            })
            .take(HR_BINS_MAX + 1)
            .collect::<Option<_>>()?,
    };
    let mut seen = [false; 256];
    let bins_ok = b.hr_bins.len() <= HR_BINS_MAX
        && b.hr_bins.iter().all(|x| !std::mem::replace(&mut seen[usize::from(x.bpm)], true));
    (it.next().is_none() && bins_ok).then_some(b)
}

/// One ride as the weekly totals see it.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideStat {
    /// Start, Unix seconds.
    pub start_unix: i64,
    /// The local day of the unit's file name (days since 1970-01-01); the week is taken from it when there is one.
    pub local_day: Option<i64>,
    /// The load is a heart rate load, not a TSS.
    pub load_from_hr: bool,
    /// The phone's offset from UTC at that moment, s (so a daylight saving change is the caller's).
    pub tz_offset_s: i32,
    /// Moving time, s.
    pub moving_s: u32,
    /// Distance, m.
    pub distance_m: f64,
    /// Load, if there is one.
    pub load: Option<u32>,
}

/// A brief as a stat for the phone's offset, with the load against `ftp` and `max_hr`.
#[uniffi::export]
pub fn ride_stat(b: RideBrief, name: String, ftp: u16, max_hr: u16, tz_offset_s: i32) -> RideStat {
    let (load, load_from_hr) = brief_load(&b, ftp, max_hr);
    RideStat {
        start_unix: b.start_unix,
        local_day: name_day(&name),
        load_from_hr,
        tz_offset_s,
        moving_s: b.moving_s,
        distance_m: b.distance_m,
        load,
    }
}

/// Days since 1970-01-01 of a calendar date (Hinnant's days_from_civil, public domain).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The local day of a unit file name `YYMMDDhhmmss` (`None`: not a date).
pub fn name_day(name: &str) -> Option<i64> {
    let b = name.as_bytes();
    if b.len() != 12 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v = |i: usize| i64::from(b[i] - b'0') * 10 + i64::from(b[i + 1] - b'0');
    let (y, m, d) = (2000 + v(0), v(2), v(4));
    let dim = match m {
        2 => 28 + i64::from(y % 4 == 0), // 2000 to 2099
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    ((1..=12).contains(&m) && (1..=dim).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// One week's sums.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct WeekTotal {
    /// The week's first local day, as days since 1970-01-01.
    pub first_day: i64,
    /// Moving time, s.
    pub moving_s: u64,
    /// Distance, m.
    pub distance_m: f64,
    /// Load.
    pub load: u64,
    /// Rides.
    pub rides: u32,
    /// The load holds heart rate loads (not TSS).
    pub has_hr_load: bool,
}

const WEEKS_MAX: u32 = 520;

/// The `weeks` weeks up to and including the one holding `now`, oldest first (empty weeks are there, at zero). A week
/// starts on `week_start` (0 Monday to 6 Sunday); a ride belongs to the week of its local start.
#[uniffi::export]
pub fn weekly_totals(
    stats: Vec<RideStat>,
    week_start: u8,
    now_unix: i64,
    now_tz_offset_s: i32,
    weeks: u32,
) -> Vec<WeekTotal> {
    let weeks = weeks.min(WEEKS_MAX);
    // 1970-01-01 was a Thursday: Monday-based weekday of day d is (d + 3) mod 7
    let first_of = |d: i64| d - (d + 3 - i64::from(week_start % 7)).rem_euclid(7);
    let first = |t: i64, off: i32| first_of(t.saturating_add(i64::from(off)).div_euclid(86_400));
    let cur = first(now_unix, now_tz_offset_s);
    let mut out: Vec<WeekTotal> = (0..weeks)
        .map(|i| WeekTotal { first_day: cur - 7 * i64::from(weeks - 1 - i), ..WeekTotal::default() })
        .collect();
    for s in stats {
        let wk = s.local_day.map_or_else(|| first(s.start_unix, s.tz_offset_s), first_of);
        let back = (cur - wk) / 7;
        let Some(w) =
            u32::try_from(back).ok().and_then(|b| weeks.checked_sub(1 + b)).and_then(|i| out.get_mut(i as usize))
        else {
            continue;
        };
        w.moving_s += u64::from(s.moving_s);
        w.distance_m += s.distance_m;
        w.load += u64::from(s.load.unwrap_or(0));
        w.rides += 1;
        w.has_hr_load |= s.load_from_hr && s.load.is_some();
    }
    out
}
