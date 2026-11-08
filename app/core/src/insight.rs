//! The deeper analysis of one ride, worked out once per ride and cached by the app: the climbs, the W' balance, the
//! aerobic decoupling and DFA alpha1. It reads the file again (all the records, not the viewer's 2000 samples).
//!
//! The firmware's code does the maths, so the phone and the unit agree:
//! - **Climbs:** `course::Course::climbs_of`, the firmware's one definition (Garmin's ClimbPro rule: at least 500 m at
//!   3 % on the profile taken in 50 m steps; the road ahead and the courses use it too), run on the ride's altitude
//!   profile resampled every 10 m and smoothed by a 100 m moving average, since a recorded altitude is noisy and the
//!   finder was made for the smooth profile of a planned course (its own note). Per climb: the length and gain the
//!   finder gives, the average grade (gain / length), the maximum grade over 100 m of the smoothed profile, the time
//!   from the first to the last record in it (stops included), VAM (gain / time, m/h; Ferrari's measure of climbing
//!   speed), the average power over those seconds and W/kg of the weight in the profile. The colour is
//!   `ui::grade_color` of the average grade, as the unit colours its climbs.
//! - **W' balance:** `power::wbal_step`, the differential model of the unit's `Power::second` (Froncioni and Clarke,
//!   Golden Cheetah's "Differential"; see power.rs), second by second from a full W', at most 3 days of it. CP = FTP is
//!   the unit's own shortcut, not the rider's critical power: a W' from Intervals.icu belongs to its CP, so the two
//!   differ when CP and FTP do. A record counts for the seconds to the next one up to 5 s, then the power is 0 (a pause
//!   recovers).
//! - **Aerobic decoupling (Pa:HR, here from power):** the moving seconds with power and a heart rate above 0 cut in two
//!   halves of equal time; per half the efficiency EF = average power / average heart rate; decoupling = (EF1 - EF2) /
//!   EF1 x 100, positive when the second half costs more heart rate per watt. Friel's Pw:Hr / Pa:Hr; Intervals.icu's
//!   own cut is **unverified (one forum user's reply, <https://forum.intervals.icu/t/21505>)**: it is taken here to use
//!   the average power of each half, not NP, for consistency with Intervals.icu, which the owner compares against.
//!   Under 20 minutes of moving time with both, no value is given (inferred: too short for drift).
//! - **DFA alpha1:** the file's `hrv` messages (global 78, field `time`, the strap's raw RR intervals, as the
//!   firmware's `fit.rs` writes them) pushed beat by beat through the firmware's `hrv::Hrv` (5 % artifact rule, 2
//!   minute window, boxes of 4 to 16 beats). The messages carry no time: the beats are laid end to end from the
//!   first record, as the firmware's module doc says readers do. The thresholds are `hrvt::A1_VT1` and `A1_VT2`.
//!   A ride without the beats (a clean copy, a strap without RR) has an empty list.

use crate::ride::{RideError, TrackSample, parse_msgs, sample_of};
use crate::summary::MOVING_MS;
use bryton_rustfw::{
    course::{CLIMBS, Course},
    hrv::{self, Hrv},
    hrvt::{A1_VT1, A1_VT2},
    power::wbal_step,
    ui::grade_color,
};
use fitparser::{Value, profile::MesgNum};

/// The profile's step, m.
const STEP_M: usize = 10;
/// The moving average's half width, in steps (so 100 m wide).
const SMOOTH: usize = 5;
/// The window of the maximum grade, in steps (100 m).
const GRADE_STEPS: usize = 10;
/// The longest profile kept, in steps (over 1000 km is not a ride).
const MAX_STEPS: usize = 100_000;
/// A record counts for the time to the next, but at most this long, s (as `analyze_ride`).
const HOLD_S: u32 = 5;
/// The W' balance runs over at most this many seconds (3 days): a damaged timestamp cannot make it long.
const WBAL_MAX_S: usize = 3 * 86_400;
/// The largest distance offset of a profile taken as real, m.
const BASE_MAX_M: f64 = 1.0e9;
/// Points the W' balance and DFA lists keep.
const MAX_POINTS: usize = 400;
/// The least moving time with power and heart rate for a decoupling, s.
const DECOUPLING_MIN_S: u32 = 1200;
/// Two climbs are the same if both ends are within this of each other, m (the road ahead's snap distance).
const SAME_M: f64 = bryton_rustfw::climbs_ahead::SNAP_M as f64;

/// One climb of the ride.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct ClimbStat {
    /// From the start of the ride, m.
    pub start_m: u32,
    /// Length, m.
    pub len_m: u32,
    /// Gain, m.
    pub gain_m: u32,
    /// Average grade, % x 10.
    pub avg_grade_x10: u32,
    /// Maximum grade over 100 m, % x 10.
    pub max_grade_x10: u32,
    /// Seconds from the start of the ride at the foot, and at the top.
    pub start_s: u32,
    /// See `start_s`.
    pub end_s: u32,
    /// Time from foot to top, s.
    pub time_s: u32,
    /// Vertical speed, m/h; None without time.
    pub vam: Option<u32>,
    /// Average power over the climb, W.
    pub avg_power: Option<u16>,
    /// Watts per kg x 100; None without power or weight.
    pub w_per_kg_x100: Option<u32>,
    /// The grade colour, 0xRRGGBB (the unit's `grade_color`).
    pub color: u32,
    /// Where it starts, degrees.
    pub start_lat: Option<f64>,
    /// See `start_lat`.
    pub start_lon: Option<f64>,
    /// Where it ends, degrees.
    pub end_lat: Option<f64>,
    /// See `end_lat`.
    pub end_lon: Option<f64>,
}

/// The W' balance at a second of the ride.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WbalPoint {
    /// Seconds from the start.
    pub t_s: u32,
    /// W' balance, J (the lowest of the seconds the point stands for).
    pub wbal_j: i32,
}

/// Aerobic decoupling, Pa:HR (average power over average heart rate).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct Decoupling {
    /// Efficiency factor of the first half, W per bpm.
    pub ef_first: f64,
    /// Efficiency factor of the second half, W per bpm.
    pub ef_second: f64,
    /// (first - second) / first, percent.
    pub percent: f64,
}

/// DFA alpha1 at a second of the ride.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct DfaPoint {
    /// Seconds from the start.
    pub t_s: u32,
    /// Alpha1.
    pub a1: f64,
}

/// Everything `ride_insight` works out.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideInsight {
    /// The climbs, in order.
    pub climbs: Vec<ClimbStat>,
    /// The W' balance (empty without power, CP or W').
    pub wbal: Vec<WbalPoint>,
    /// The lowest W' balance of the ride, J.
    pub wbal_min_j: Option<i32>,
    /// Pa:HR (average power over average heart rate).
    pub decoupling: Option<Decoupling>,
    /// DFA alpha1.
    pub dfa: Vec<DfaPoint>,
    /// Alpha1 at the aerobic threshold (about 0.75), and at the anaerobic one (0.5).
    pub vt1: f64,
    /// See `vt1`.
    pub vt2: f64,
}

/// The distance of each record as a running maximum: a record without one, or after a reset, stands at the farthest
/// point so far, so the list is sorted for searching.
fn reach(all: &[TrackSample]) -> Vec<f64> {
    let mut far = 0.0f64;
    all.iter()
        .map(|s| {
            if s.dist_m.is_finite() {
                far = far.max(s.dist_m.min(BASE_MAX_M));
            }
            far
        })
        .collect()
}

/// The altitude, dm, every [`STEP_M`] m from the first altitude's distance (returned first), smoothed; empty without
/// a usable altitude and distance.
fn profile(all: &[TrackSample], reach: &[f64]) -> (f64, Vec<i32>) {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut far = f64::MIN;
    for (s, &d) in all.iter().zip(reach) {
        if let Some(a) = s.alt_m.filter(|a| a.is_finite() && a.abs() < 1.0e5)
            && d > far
        {
            far = d;
            pts.push((d, a));
        }
    }
    let (Some(&(d0, _)), Some(&(d1, _))) = (pts.first(), pts.last()) else { return (0.0, Vec::new()) };
    let n = (((d1 - d0) / STEP_M as f64) as usize).min(MAX_STEPS);
    // linear interpolation at d0 + k x STEP_M; the points are increasing in distance
    let mut raw = Vec::with_capacity(n + 1);
    let mut j = 0;
    for k in 0..=n {
        let d = d0 + (k * STEP_M) as f64;
        while pts.get(j + 1).is_some_and(|p| p.0 < d) {
            j += 1;
        }
        let (a, b) = (pts.get(j).copied().unwrap_or_default(), pts.get(j + 1).copied().unwrap_or_default());
        raw.push(if b.0 > a.0 { a.1 + (b.1 - a.1) * ((d - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { a.1 });
    }
    let smooth = (0..raw.len()).map(|i| {
        let w = raw.get(i.saturating_sub(SMOOTH)..(i + SMOOTH + 1).min(raw.len())).unwrap_or(&[]);
        (w.iter().sum::<f64>() / w.len().max(1) as f64 * 10.0).round() as i32
    });
    (d0, smooth.collect())
}

fn rgb(c565: u16) -> u32 {
    let (r, g, b) = (u32::from(c565 >> 11 & 31), u32::from(c565 >> 5 & 63), u32::from(c565 & 31));
    (r << 3 | r >> 2) << 16 | (g << 2 | g >> 4) << 8 | (b << 3 | b >> 2)
}

fn climbs(all: &[TrackSample], weight_kg: u16) -> Vec<ClimbStat> {
    let reach = reach(all);
    let (base, prof) = profile(all, &reach);
    let total = u32::try_from(prof.len().saturating_sub(1) * STEP_M).unwrap_or(0);
    let alt_at = |d: u32| {
        let i = (d as usize / STEP_M).min(prof.len().saturating_sub(1));
        prof.get(i).copied()
    };
    let (found, n) = Course::climbs_of(total, alt_at);
    let at = |m: f64| reach.partition_point(|&d| d < m).min(all.len().saturating_sub(1));
    found
        .iter()
        .take(n.min(CLIMBS))
        .filter_map(|c| {
            let (a, b) = (at(base + f64::from(c.start_m)), at(base + f64::from(c.end_m())));
            let start_m = c.start_m.saturating_add(base as u32);
            let (sa, sb) = (all.get(a)?, all.get(b)?);
            let time_s = sb.t_s.saturating_sub(sa.t_s);
            let powers: Vec<u32> = all.get(a..=b.max(a))?.iter().filter_map(|s| s.power.map(u32::from)).collect();
            let avg_power = (!powers.is_empty()).then(|| (powers.iter().sum::<u32>() / powers.len() as u32) as u16);
            let step = |d: u32| alt_at(d);
            let span = (GRADE_STEPS * STEP_M) as u32;
            let max_grade = (c.start_m..=c.end_m().saturating_sub(span))
                .step_by(STEP_M)
                .filter_map(|d| Some(step(d.saturating_add(span))? - step(d)?))
                .max()
                .unwrap_or(0)
                .max(0) as u32;
            Some(ClimbStat {
                start_m,
                len_m: c.len_m,
                gain_m: c.gain_m,
                avg_grade_x10: c.grade_x10(),
                max_grade_x10: max_grade.max(c.grade_x10()),
                start_s: sa.t_s,
                end_s: sb.t_s,
                time_s,
                vam: (time_s > 0).then(|| (u64::from(c.gain_m) * 3600 / u64::from(time_s)) as u32),
                avg_power,
                w_per_kg_x100: avg_power
                    .filter(|_| weight_kg > 0)
                    .map(|p| u32::from(p) * 100 * 10 / (u32::from(weight_kg) * 10)),
                color: rgb(grade_color(c.grade_x10() as i32)),
                start_lat: sa.lat,
                start_lon: sa.lon,
                end_lat: sb.lat,
                end_lon: sb.lon,
            })
        })
        .collect()
}

/// Seconds each record stands for (to the next, 1 to [`HOLD_S`]).
fn dt(all: &[TrackSample], i: usize) -> u32 {
    let Some(s) = all.get(i) else { return 0 };
    all.get(i + 1).map_or(1, |n| n.t_s.saturating_sub(s.t_s).clamp(1, HOLD_S))
}

/// W' balance (J) per second of the ride, as the lowest in each of at most [`MAX_POINTS`] buckets.
fn wbal(all: &[TrackSample], cp: u16, wprime_j: u16) -> Vec<WbalPoint> {
    let (Some(last), true) = (all.last(), cp > 0 && wprime_j > 0 && all.iter().any(|s| s.power.is_some())) else {
        return Vec::new();
    };
    let secs = (last.t_s as usize + 1).min(WBAL_MAX_S);
    let bucket = secs.div_ceil(MAX_POINTS);
    let mut w = i64::from(wprime_j) * 1000;
    let (mut out, mut i) = (Vec::new(), 0usize);
    let mut low = i64::MAX;
    for t in 0..secs {
        while all.get(i + 1).is_some_and(|n| n.t_s as usize <= t) {
            i += 1;
        }
        let p = all.get(i).filter(|s| (t as u32).saturating_sub(s.t_s) < dt(all, i)).and_then(|s| s.power).unwrap_or(0);
        w = wbal_step(w, p, cp, wprime_j);
        low = low.min(w);
        if t % bucket == bucket - 1 || t + 1 == secs {
            out.push(WbalPoint { t_s: t as u32, wbal_j: (low / 1000) as i32 });
            low = i64::MAX;
        }
    }
    out
}

fn decoupling(all: &[TrackSample]) -> Option<Decoupling> {
    let has_speed = all.iter().any(|s| s.speed_ms.is_some());
    // (power, heart rate, seconds) of the moving records with both; a heart rate of 0 is a dropout, not a value
    let mv: Vec<(u64, u64, u32)> = all
        .iter()
        .enumerate()
        .filter(|(_, s)| !has_speed || s.speed_ms.unwrap_or(0.0) >= MOVING_MS)
        .filter_map(|(i, s)| Some((u64::from(s.power?), u64::from(s.hr.filter(|h| *h > 0)?), dt(all, i))))
        .collect();
    let total = mv.iter().fold(0u32, |a, x| a.saturating_add(x.2));
    if total < DECOUPLING_MIN_S {
        return None;
    }
    // per half: power x s, heart rate x s, s
    let (mut half, mut done) = ([(0u64, 0u64, 0u64); 2], 0u32);
    for (p, h, d) in mv {
        let slot = half.get_mut(usize::from(u64::from(done) * 2 >= u64::from(total)))?;
        done = done.saturating_add(d);
        *slot = (slot.0 + p * u64::from(d), slot.1 + h * u64::from(d), slot.2 + u64::from(d));
    }
    let ef = |h: usize| {
        let (p, hr, n) = half.get(h)?;
        (*hr > 0).then(|| (*p as f64 / *n as f64) / (*hr as f64 / *n as f64))
    };
    let (a, b) = (ef(0)?, ef(1)?);
    (a > 0.0).then_some(Decoupling { ef_first: a, ef_second: b, percent: (a - b) / a * 100.0 })
}

/// One `hrv` value as ms: the file holds seconds (scale 1000), and a reader that returns the raw ms is accepted too;
/// a value outside the strap's range (250 to 2000 ms) or not finite is dropped.
fn rr_ms(x: f64) -> Option<u16> {
    let ms = if x < 20.0 { x * 1000.0 } else { x };
    (250.0..=2000.0).contains(&ms).then(|| ms.round() as u16)
}

/// The RR intervals of the `hrv` messages, ms, in order; a value outside the strap's range is dropped.
fn beats(msgs: &[fitparser::FitDataRecord]) -> Vec<u16> {
    let mut out = Vec::new();
    for m in msgs.iter().filter(|m| m.kind() == MesgNum::Hrv) {
        for f in m.fields().iter().filter(|f| f.name() == "time") {
            let vals: Vec<&Value> = match f.value() {
                Value::Array(a) => a.iter().collect(),
                v => vec![v],
            };
            for v in vals {
                let Ok(x): Result<f64, _> = v.clone().try_into() else { continue };
                out.extend(rr_ms(x));
            }
        }
    }
    out
}

fn dfa(rr: &[u16]) -> Vec<DfaPoint> {
    let (mut h, mut t_ms, mut pts) = (Hrv::default(), 0u64, Vec::new());
    for (i, &ms) in rr.iter().enumerate() {
        h.push(ms);
        t_ms += u64::from(ms);
        // the value is recomputed every EVERY beats
        if (i + 1) % usize::from(hrv::EVERY) == 0
            && let Some(a1) = h.alpha1()
        {
            pts.push(DfaPoint { t_s: (t_ms / 1000) as u32, a1: f64::from(a1) });
        }
    }
    let stride = pts.len().div_ceil(MAX_POINTS).max(1);
    pts.into_iter().step_by(stride).collect()
}

/// Works out the climbs, W' balance, decoupling and DFA alpha1 of a FIT file. `weight_kg` (0: unknown) gives W/kg;
/// `cp` (W, the FTP as the unit does) and `wprime_j` (J) the W' balance, 0 leaves it out.
#[uniffi::export]
pub fn ride_insight(bytes: Vec<u8>, weight_kg: u16, cp: u16, wprime_j: u16) -> Result<RideInsight, RideError> {
    let msgs = parse_msgs(&bytes)?;
    let mut t0 = None;
    let all: Vec<TrackSample> =
        msgs.iter().filter(|m| m.kind() == MesgNum::Record).filter_map(|m| sample_of(m.fields(), &mut t0)).collect();
    if all.is_empty() {
        return Err(RideError::Empty);
    }
    let wbal = wbal(&all, cp, wprime_j);
    Ok(RideInsight {
        climbs: climbs(&all, weight_kg),
        wbal_min_j: wbal.iter().map(|p| p.wbal_j).min(),
        wbal,
        decoupling: decoupling(&all),
        dfa: dfa(&beats(&msgs)),
        vt1: f64::from(A1_VT1),
        vt2: f64::from(A1_VT2),
    })
}

/// A climb of one ride and the same climb in another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ClimbPair {
    /// Index in the first ride's list.
    pub first: u32,
    /// Index in the second ride's.
    pub second: u32,
}

fn apart_m(a: (Option<f64>, Option<f64>), b: (Option<f64>, Option<f64>)) -> Option<f64> {
    let ((la, oa), (lb, ob)) = ((a.0?, a.1?), (b.0?, b.1?));
    // equirectangular: right to a few metres over the hundreds of metres that matter here
    let (dy, dx) = ((lb - la) * 111_320.0, (ob - oa) * 111_320.0 * ((la + lb) / 2.0).to_radians().cos());
    Some(dx.hypot(dy))
}

/// The climbs two rides share: both ends within max([`SAME_M`], a quarter of the shorter climb's length) of the other's
/// (the same road in the same direction). Pairs are taken by the least summed distance of their ends, each climb once.
/// (A shared-length test, 70 % of the length in common, is not done: it needs the tracks, not the climbs.)
#[uniffi::export]
pub fn match_climbs(first: Vec<ClimbStat>, second: Vec<ClimbStat>) -> Vec<ClimbPair> {
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for (i, a) in first.iter().enumerate() {
        for (j, b) in second.iter().enumerate() {
            let tol = SAME_M.max(f64::from(a.len_m.min(b.len_m)) / 4.0);
            let near = |x: Option<f64>| x.filter(|d| *d <= tol);
            let ends = (
                near(apart_m((a.start_lat, a.start_lon), (b.start_lat, b.start_lon))),
                near(apart_m((a.end_lat, a.end_lon), (b.end_lat, b.end_lon))),
            );
            if let (Some(s), Some(e)) = ends {
                cand.push((s + e, i, j));
            }
        }
    }
    cand.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (mut used_a, mut used_b) = (vec![false; first.len()], vec![false; second.len()]);
    let mut out = Vec::new();
    for (_, i, j) in cand {
        if used_a.get(i) == Some(&false) && used_b.get(j) == Some(&false) {
            for (used, k) in [(&mut used_a, i), (&mut used_b, j)] {
                if let Some(u) = used.get_mut(k) {
                    *u = true;
                }
            }
            out.push(ClimbPair { first: i as u32, second: j as u32 });
        }
    }
    out.sort_by_key(|p| p.first);
    out
}

#[cfg(test)]
#[path = "insight_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "insight_more_tests.rs"]
mod more_tests;
