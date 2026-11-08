//! What the ride list and the weekly panel need from a ride, and the weekly sums.
//!
//! **Definitions, matched to Intervals.icu so the two apps agree** (looked up 2026-11-08; the phone shows the same
//! numbers, within rounding, for the same FIT file, FTP and max HR):
//! - **Week:** starts on Monday. Intervals.icu's calendar has no first-day-of-week option (a forum reply: Monday "is
//!   very much hardcoded", a Sunday option is requested), <https://forum.intervals.icu/t/changing-start-of-the-week/3235>
//!   and <https://forum.intervals.icu/t/option-to-start-week-on-sunday/47887>. `weekly_totals` still takes the day.
//!   A ride belongs to the week of its local start (inferred: Intervals shows an activity on its start date).
//! - **Moving time:** from the speed stream, a speed of 0.05 m/s or more is moving; with no speed or GPS (an indoor
//!   ride) the whole ride is moving; short gaps count as moving (<https://forum.intervals.icu/t/moving-time-is-different/108707>,
//!   <https://forum.intervals.icu/t/intervals-icu-faq/123707/5>). Here each record counts for the time to the next
//!   one, at most 5 s (`ride.rs`), so a longer gap is a pause. Not read from the file's total_timer_time: Intervals
//!   does not use it (it says moving time is not a file field).
//! - **Distance:** the session's total_distance, else the last record's (inferred: Intervals reads the file's value).
//! - **Load with power:** TSS with NP, "calculated in the same way as Training Peaks except that not moving time is
//!   excluded" (<https://forum.intervals.icu/t/how-is-calculated-training-load/1661>): seconds x NP^2 x 100 /
//!   (FTP^2 x 3600) over the moving seconds, NP from moving seconds only (the firmware's `power::tss`). Intervals' own
//!   NP differs from TrainingPeaks' by small details (its forum threads), so a few percent of difference is possible.
//! - **Load without power:** Intervals' default HR model is HRSS, "normalised TRIMP" from Elevate (staff reply in
//!   <https://forum.intervals.icu/t/how-training-load-estimation-from-hr-works/280>). Inferred here: Banister TRIMP
//!   per heart rate (rest 60 bpm, exponent 0.64 e^(1.92 x reserve), male coefficients) against one hour at 85 % of
//!   the max HR, times 100. Intervals uses the rider's resting and threshold HR, which the app does not have, so an
//!   HR load can differ more than a power one.

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
        distance_m: it.next()?.parse().ok().filter(|d: &f64| d.is_finite())?,
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
            .collect::<Option<_>>()?,
    };
    it.next().is_none().then_some(b)
}

/// One ride as the weekly totals see it.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideStat {
    /// Start, Unix seconds.
    pub start_unix: i64,
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
pub fn ride_stat(b: RideBrief, ftp: u16, max_hr: u16, tz_offset_s: i32) -> RideStat {
    RideStat {
        start_unix: b.start_unix,
        tz_offset_s,
        moving_s: b.moving_s,
        distance_m: b.distance_m,
        load: brief_load(&b, ftp, max_hr).0,
    }
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
    let first = |t: i64, off: i32| {
        let d = t.saturating_add(i64::from(off)).div_euclid(86_400);
        d - (d + 3 - i64::from(week_start % 7)).rem_euclid(7)
    };
    let cur = first(now_unix, now_tz_offset_s);
    let mut out: Vec<WeekTotal> = (0..weeks)
        .map(|i| WeekTotal { first_day: cur - 7 * i64::from(weeks - 1 - i), ..WeekTotal::default() })
        .collect();
    for s in stats {
        let back = (cur - first(s.start_unix, s.tz_offset_s)) / 7;
        let Some(w) =
            u32::try_from(back).ok().and_then(|b| weeks.checked_sub(1 + b)).and_then(|i| out.get_mut(i as usize))
        else {
            continue;
        };
        w.moving_s += u64::from(s.moving_s);
        w.distance_m += s.distance_m;
        w.load += u64::from(s.load.unwrap_or(0));
        w.rides += 1;
    }
    out
}
