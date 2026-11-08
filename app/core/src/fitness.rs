//! The fitness trend (CTL fitness, ATL fatigue, TSB form), eFTP and the 90-day power curve.
//!
//! Sources (the OpenAPI spec <https://intervals.icu/api/v1/docs>, looked up 2026-11-08):
//! - `GET /api/v1/athlete/{id}/wellness{ext}` ("List wellness records for date range"), query `oldest` and `newest`
//!   (ISO dates), answers an array of `Wellness`: `id` (the date, a string), `ctl`, `atl` (numbers), `sportInfo`: an
//!   array of `{type, eftp, wPrime, pMax}` (numbers; `type` an activity type such as "Ride"). The ext is `.json`
//!   (inferred: the spec shows `.csv` for CSV). Units of `eftp` (W) are inferred from the Intervals.icu UI.
//! - `GET /api/v1/athlete/{id}/power-curves{ext}`, query `curves` (an array; "90d" for the last 90 days is inferred
//!   from the Intervals.icu UI's curve names) and `type=Ride`, answers `DataCurveSetPowerCurve`: `list` of
//!   `DataCurve` with `secs` and `values` (arrays of integers, W) and `days`.
//!
//! Every field is optional in the spec: a day lacking `id`, `ctl` or `atl` is skipped, a number that is not finite
//! or is outside 0 to 1000 is dropped, and nothing here reaches the network (Swift fetches).
//!
//! **Without a key** ([`fitness_from_loads`]): the firmware's model (`load.rs`: Coggan's Performance Management
//! Chart), CTL and ATL the exponentially weighted averages of the daily load with time constants 42 and 7 days,
//! `x[d] = x[d-1] + (load[d] - x[d-1]) / N`, days without a ride counting 0, the series starting at 0 (the rides
//! the phone holds are all there is) and TSB the form the day starts with: yesterday's CTL minus yesterday's ATL. The
//! same TSB rule is applied to Intervals.icu's days, so the two sources draw alike (inferred: Intervals.icu's
//! own "form" may be the same day's difference).

use crate::ride::CurvePoint;

/// Days of history asked and kept.
pub const DAYS: usize = 90;
/// CTL's and ATL's time constants, days.
const CTL_D: f64 = 42.0;
const ATL_D: f64 = 7.0;
/// The curve's durations kept, s.
const CURVE_S: [u32; 11] = [1, 5, 15, 30, 60, 120, 300, 600, 1200, 1800, 3600];
/// The largest CTL or ATL taken.
const CAP: f64 = 1000.0;
/// The largest power taken, W: a sprint is clamped to it, not dropped.
const WATTS_CAP: f64 = 3000.0;

/// One day of the trend.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct FitnessDay {
    /// Days since 1970-01-01.
    pub day: i64,
    /// Fitness at the end of the day.
    pub ctl: f64,
    /// Fatigue at the end of the day.
    pub atl: f64,
    /// Form the day started with: yesterday's CTL minus yesterday's ATL.
    pub tsb: f64,
}

/// What the wellness answer holds.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct Wellness {
    /// The days, oldest first, at most [`DAYS`] (the last ones).
    pub days: Vec<FitnessDay>,
    /// The latest eFTP for Ride, W.
    pub eftp: Option<f64>,
}

/// Why an answer could not be used.
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum FitnessError {
    /// Not the JSON of the expected shape.
    NotWellness,
}

impl core::fmt::Display for FitnessError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Intervals.icu answered with something unreadable.")
    }
}

/// A load of one day, for the fitness without a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DayLoad {
    /// Days since 1970-01-01.
    pub day: i64,
    /// That ride's load.
    pub load: u32,
}

/// Days since 1970-01-01 of a civil date (Hinnant's days_from_civil, public domain).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// "YYYY-MM-DD" (a time after it is ignored) to a day number.
fn day_of(id: &str) -> Option<i64> {
    let mut it = id.get(..10)?.split('-');
    let (y, m, d) = (it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?);
    let len = match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    ((1900..=2200).contains(&y) && (1..=12).contains(&m) && (1..=len).contains(&d)).then(|| days_from_civil(y, m, d))
}

fn val(v: Option<&serde_json::Value>) -> Option<f64> {
    v?.as_f64().filter(|x| (0.0..=CAP).contains(x))
}

/// A power: finite and not negative, clamped to [`WATTS_CAP`].
fn watts(v: Option<&serde_json::Value>) -> Option<f64> {
    v?.as_f64().filter(|x| x.is_finite() && *x >= 0.0).map(|x| x.min(WATTS_CAP))
}

/// Sets each day's TSB from the day before's CTL and ATL (the first, and a day after a gap, from its own) and keeps the last [`DAYS`].
fn finish(mut days: Vec<(i64, f64, f64)>) -> Vec<FitnessDay> {
    days.sort_by_key(|d| d.0);
    days.dedup_by_key(|d| d.0);
    let out: Vec<FitnessDay> = days
        .iter()
        .enumerate()
        .map(|(i, &(day, ctl, atl))| {
            // yesterday's values only when the record before is yesterday's; after a gap the day's own
            let prev = i.checked_sub(1).and_then(|k| days.get(k)).filter(|p| p.0 + 1 == day);
            let (pc, pa) = prev.map_or((ctl, atl), |p| (p.1, p.2));
            FitnessDay { day, ctl, atl, tsb: pc - pa }
        })
        .collect();
    out.get(out.len().saturating_sub(DAYS)..).unwrap_or(&[]).to_vec()
}

/// The wellness records: CTL, ATL and the latest Ride eFTP.
#[uniffi::export]
pub fn wellness_parse(json: Vec<u8>) -> Result<Wellness, FitnessError> {
    let v: serde_json::Value = serde_json::from_slice(&json).map_err(|_| FitnessError::NotWellness)?;
    let list = v.as_array().ok_or(FitnessError::NotWellness)?;
    let mut days = Vec::new();
    let mut eftp: Option<(i64, f64)> = None;
    for w in list {
        let Some(day) = w.get("id").and_then(|i| i.as_str()).and_then(day_of) else { continue };
        if let Some(e) = w
            .get("sportInfo")
            .and_then(|s| s.as_array())
            .and_then(|a| a.iter().find(|s| s.get("type").and_then(|t| t.as_str()) == Some("Ride")))
            .and_then(|s| watts(s.get("eftp")))
            .filter(|e| *e > 0.0 && eftp.is_none_or(|(d, _)| day >= d))
        {
            eftp = Some((day, e));
        }
        if let (Some(c), Some(a)) = (val(w.get("ctl")), val(w.get("atl"))) {
            days.push((day, c, a));
        }
    }
    Ok(Wellness { days: finish(days), eftp: eftp.map(|e| e.1) })
}

/// The 90-day best power curve: the first curve of the answer, at the durations in [`CURVE_S`] it holds.
#[uniffi::export]
pub fn power_curve_parse(json: Vec<u8>) -> Result<Vec<CurvePoint>, FitnessError> {
    let v: serde_json::Value = serde_json::from_slice(&json).map_err(|_| FitnessError::NotWellness)?;
    let c = v.get("list").and_then(|l| l.as_array()).and_then(|l| l.first()).ok_or(FitnessError::NotWellness)?;
    let arr = |k: &str| c.get(k).and_then(|a| a.as_array());
    let (secs, vals) = (arr("secs").ok_or(FitnessError::NotWellness)?, arr("values").ok_or(FitnessError::NotWellness)?);
    Ok(secs
        .iter()
        .zip(vals)
        .filter_map(|(s, w)| {
            let (s, w) = (u32::try_from(s.as_u64()?).ok()?, watts(Some(w))?);
            (CURVE_S.contains(&s) && w > 0.0).then_some(CurvePoint { secs: s, watts: w.round() as u16 })
        })
        .collect())
}

/// CTL, ATL and TSB for the [`DAYS`] days ending on `today` (days since 1970) from the rides' loads (the model in
/// the module doc). The series starts at the first load, at 0.
#[uniffi::export]
pub fn fitness_from_loads(loads: Vec<DayLoad>, today: i64) -> Vec<FitnessDay> {
    let Some(first) = loads.iter().map(|l| l.day).filter(|d| *d <= today).min() else { return Vec::new() };
    let first = first.max(today - 3650); // ten years at most: a damaged date cannot make a long loop
    let (mut c, mut a) = (0.0, 0.0);
    let mut days = Vec::new();
    for day in first..=today {
        let load: f64 = loads.iter().filter(|l| l.day == day).map(|l| f64::from(l.load)).sum();
        (c, a) = (c + (load - c) / CTL_D, a + (load - a) / ATL_D);
        days.push((day, c, a));
    }
    finish(days)
}

#[cfg(test)]
#[path = "fitness_tests.rs"]
mod tests;
