//! A downloaded ride, decoded for the ride viewer: the summary, the laps, the zones, the power curve and the samples
//! for the graphs and the map.
//!
//! **Reader:** `fitparser` (MIT, github.com/stadelmanma/fitparse-rs), not the firmware's `fitread`: that one streams
//! with no heap and gives only the fields the unit shows (no speed, cadence, calories, descent, max speed, lap
//! start), so using it would mean widening a firmware module for the phone. The training maths is the firmware's own
//! (`power::Power` for NP / IF / TSS / kcal, `power_curve::Rolling` for the curve, `ride::{power_zone, hr_zone}`),
//! so the app and the unit agree on a ride.
//! Fields (FIT profile, Profile.xlsx of the Garmin SDK): record 253 timestamp, 0/1 position, 2/78 altitude,
//! 3 heart_rate, 4 cadence, 5 distance, 6/73 speed, 7 power; session 18 and lap 19 as in the firmware's writer.

use bryton_rustfw::{
    power::Power,
    power_curve::{DURATIONS, Rolling},
    ride::{hr_zone, power_zone},
};
use fitparser::{FitDataField, FitDataRecord, profile::MesgNum};

/// The most samples handed to the graphs and the map.
pub const MAX_SAMPLES: usize = 2000;
/// The curve's durations, seconds (all in the firmware's `DURATIONS`).
const CURVE_S: [u32; 5] = [5, 60, 300, 1200, 3600];

/// Why a file is not shown.
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum RideError {
    /// Not a FIT file, or damaged.
    Unreadable,
    /// A FIT file with no `record` in it.
    Empty,
}

impl core::fmt::Display for RideError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            RideError::Unreadable => "not a readable FIT file",
            RideError::Empty => "the ride has no records",
        })
    }
}

/// One second of the ride.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct TrackSample {
    /// Seconds since the first record.
    pub t_s: u32,
    /// Distance from the start, m.
    pub dist_m: f64,
    /// Latitude, degrees.
    pub lat: Option<f64>,
    /// Longitude, degrees.
    pub lon: Option<f64>,
    /// Altitude, m.
    pub alt_m: Option<f64>,
    /// Speed, m/s.
    pub speed_ms: Option<f64>,
    /// Heart rate, bpm.
    pub hr: Option<u16>,
    /// Power, W.
    pub power: Option<u16>,
    /// Cadence, rpm.
    pub cadence: Option<u16>,
}

/// A lap.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RideLap {
    /// Moving time, s.
    pub time_s: u32,
    /// Distance, m.
    pub dist_m: f64,
    /// Average speed, m/s.
    pub avg_speed_ms: Option<f64>,
    /// Average power, W.
    pub avg_power: Option<u16>,
    /// Average heart rate, bpm.
    pub avg_hr: Option<u16>,
}

/// The ride's best average power over one duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CurvePoint {
    /// The duration, s.
    pub secs: u32,
    /// The best average, W.
    pub watts: u16,
}

/// The ride's numbers. Metric units; `None`: the file does not say and it cannot be worked out.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideSummary {
    /// Start, Unix seconds.
    pub start_unix: i64,
    /// Start to end, s.
    pub elapsed_s: u32,
    /// Moving (timer) time, s.
    pub moving_s: u32,
    /// Distance, m.
    pub distance_m: f64,
    /// Total ascent, m.
    pub ascent_m: Option<u32>,
    /// Total descent, m.
    pub descent_m: Option<u32>,
    /// Average speed over the moving time, m/s.
    pub avg_speed_ms: Option<f64>,
    /// Maximum speed, m/s.
    pub max_speed_ms: Option<f64>,
    /// Average heart rate, bpm.
    pub avg_hr: Option<u16>,
    /// Maximum heart rate, bpm.
    pub max_hr: Option<u16>,
    /// Average power, W.
    pub avg_power: Option<u16>,
    /// Maximum power, W.
    pub max_power: Option<u16>,
    /// Average cadence, rpm.
    pub avg_cadence: Option<u16>,
    /// Normalized power, W.
    pub np: Option<u16>,
    /// Intensity factor x 100 (against the FTP given).
    pub if_x100: Option<u32>,
    /// Training stress score (against the FTP given).
    pub tss: Option<u32>,
    /// Calories: the file's, else worked out from the power.
    pub kcal: Option<u32>,
    /// The laps.
    pub laps: Vec<RideLap>,
    /// Seconds in heart rate zones 1 to 5 (against the max HR given).
    pub hr_zone_s: Vec<u32>,
    /// Seconds in power zones 1 to 7 (against the FTP given).
    pub power_zone_s: Vec<u32>,
    /// The power curve; durations the ride did not hold are left out.
    pub curve: Vec<CurvePoint>,
}

/// A ride for the viewer.
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct RideView {
    /// The numbers.
    pub summary: RideSummary,
    /// The samples, at most [`MAX_SAMPLES`], peaks kept.
    pub samples: Vec<TrackSample>,
}

fn num(f: &[FitDataField], name: &str) -> Option<f64> {
    let v: f64 = f.iter().find(|x| x.name() == name)?.value().clone().try_into().ok()?;
    v.is_finite().then_some(v)
}

/// First of the names that is there (the enhanced field wins).
fn first(f: &[FitDataField], names: &[&str]) -> Option<f64> {
    names.iter().find_map(|n| num(f, n))
}

/// FIT semicircles (2^31 per 180 degrees) to degrees; fitparser leaves the raw value.
fn degrees(sc: f64) -> f64 {
    sc * 180.0 / 2_147_483_648.0
}

fn u16_of(v: Option<f64>) -> Option<u16> {
    v.filter(|v| *v >= 0.0).map(|v| v.round().min(f64::from(u16::MAX)) as u16)
}

fn u32_of(v: Option<f64>) -> Option<u32> {
    v.filter(|v| *v >= 0.0).map(|v| v.round().min(f64::from(u32::MAX)) as u32)
}

/// Keeps at most [`MAX_SAMPLES`]: per bucket the first sample and the one with the highest power (else heart rate,
/// else speed), so the graphs keep their peaks.
fn decimate(all: Vec<TrackSample>) -> Vec<TrackSample> {
    if all.len() <= MAX_SAMPLES {
        return all;
    }
    let bucket = all.len().div_ceil(MAX_SAMPLES / 2);
    let key = |s: &TrackSample| {
        f64::from(s.power.unwrap_or(0)) * 1000.0 + f64::from(s.hr.unwrap_or(0)) + s.speed_ms.unwrap_or(0.0)
    };
    let mut out = Vec::with_capacity(MAX_SAMPLES);
    for chunk in all.chunks(bucket) {
        let peak = (0..chunk.len())
            .max_by(|&a, &b| match (chunk.get(a), chunk.get(b)) {
                (Some(x), Some(y)) => key(x).total_cmp(&key(y)),
                _ => core::cmp::Ordering::Equal,
            })
            .unwrap_or(0);
        out.extend(chunk.first().cloned());
        if let Some(p) = chunk.get(peak).filter(|_| peak > 0) {
            out.push(p.clone());
        }
    }
    out
}

/// Decodes a FIT file's bytes. `ftp` (W) and `max_hr` (bpm) set the intensity and the zones; 0 leaves them out.
#[uniffi::export]
pub fn analyze_ride(bytes: Vec<u8>, ftp: u16, max_hr: u16) -> Result<RideView, RideError> {
    // the crate is not fuzzed by us: a panic inside it is an unreadable file, not a crash of the app
    let msgs = std::panic::catch_unwind(|| fitparser::from_bytes(&bytes))
        .map_err(|_| RideError::Unreadable)?
        .map_err(|_| RideError::Unreadable)?;
    view(&msgs, ftp, max_hr)
}

fn view(msgs: &[FitDataRecord], ftp: u16, max_hr: u16) -> Result<RideView, RideError> {
    let mut samples = Vec::new();
    let (mut session, mut laps): (Option<&[FitDataField]>, Vec<RideLap>) = (None, Vec::new());
    let mut t0 = None;
    for m in msgs {
        let f = m.fields();
        match m.kind() {
            MesgNum::Record => {
                let Some(ts) = num(f, "timestamp") else { continue };
                let t0 = *t0.get_or_insert(ts);
                samples.push(TrackSample {
                    t_s: u32_of(Some(ts - t0)).unwrap_or(0),
                    dist_m: num(f, "distance").unwrap_or(0.0),
                    lat: num(f, "position_lat").map(degrees),
                    lon: num(f, "position_long").map(degrees),
                    alt_m: first(f, &["enhanced_altitude", "altitude"]),
                    speed_ms: first(f, &["enhanced_speed", "speed"]),
                    hr: u16_of(num(f, "heart_rate")),
                    power: u16_of(num(f, "power")),
                    cadence: u16_of(num(f, "cadence")),
                });
            }
            MesgNum::Session if session.is_none() => session = Some(f),
            MesgNum::Lap => laps.push(RideLap {
                time_s: u32_of(num(f, "total_timer_time")).unwrap_or(0),
                dist_m: num(f, "total_distance").unwrap_or(0.0),
                avg_speed_ms: first(f, &["enhanced_avg_speed", "avg_speed"]),
                avg_power: u16_of(num(f, "avg_power")),
                avg_hr: u16_of(num(f, "avg_heart_rate")),
            }),
            _ => {}
        }
    }
    let (Some(first_s), Some(last_s)) = (samples.first(), samples.last()) else { return Err(RideError::Empty) };
    let ses: &[FitDataField] = session.unwrap_or(&[]);

    // Per-second pass over the records, as the unit counts a ride read back: one record is one second.
    let (mut power, mut rolling) = (Power::new(), Rolling::new());
    let (mut hr_z, mut p_z) = (vec![0u32; 5], vec![0u32; 7]);
    let (mut hr_sum, mut hr_n, mut cad_sum, mut cad_n) = (0u64, 0u64, 0u64, 0u64);
    let (mut hr_max, mut v_max, mut moving) = (0u16, 0f64, 0u32);
    let (mut up, mut down, mut anchor) = (0f64, 0f64, None::<f64>);
    for s in &samples {
        power.sample(s.power, true);
        rolling.sample(s.t_s, s.power);
        if let Some(p) = s.power.filter(|_| ftp > 0)
            && let Some(z) = p_z.get_mut(usize::from(power_zone(p, ftp)).wrapping_sub(1))
        {
            *z += 1;
        }
        if let Some(h) = s.hr {
            hr_sum += u64::from(h);
            hr_n += 1;
            hr_max = hr_max.max(h);
            if max_hr > 0
                && let Some(z) = hr_z.get_mut(usize::from(hr_zone(h, max_hr)).wrapping_sub(1))
            {
                *z += 1;
            }
        }
        if let Some(c) = s.cadence.filter(|c| *c > 0) {
            cad_sum += u64::from(c);
            cad_n += 1;
        }
        let v = s.speed_ms.unwrap_or(0.0);
        v_max = v_max.max(v);
        moving += u32::from(v > 0.5);
        // 3 m of hysteresis against the altitude's noise (a common ascent rule)
        if let Some(a) = s.alt_m {
            let base = *anchor.get_or_insert(a);
            if a - base >= 3.0 {
                (up, anchor) = (up + a - base, Some(a));
            } else if base - a >= 3.0 {
                (down, anchor) = (down + base - a, Some(a));
            }
        }
    }
    let elapsed_s = last_s.t_s;
    let distance_m = num(ses, "total_distance").unwrap_or(last_s.dist_m);
    let moving_s = u32_of(num(ses, "total_timer_time")).unwrap_or(if moving > 0 { moving } else { elapsed_s });
    let avg = |sum: u64, n: u64| (n > 0).then(|| (sum / n) as u16);
    let curve = CURVE_S
        .iter()
        .filter_map(|&secs| {
            let i = DURATIONS.iter().position(|&d| u32::from(d) == secs)?;
            rolling.best.get(i).copied().filter(|w| *w > 0).map(|watts| CurvePoint { secs, watts })
        })
        .collect();
    let has_alt = first_s.alt_m.is_some();
    let summary = RideSummary {
        start_unix: num(ses, "start_time").map_or_else(|| t0.unwrap_or(0.0) as i64, |v| v as i64),
        elapsed_s: u32_of(num(ses, "total_elapsed_time")).unwrap_or(elapsed_s).max(elapsed_s),
        moving_s,
        distance_m,
        ascent_m: u32_of(num(ses, "total_ascent")).filter(|a| *a > 0).or(has_alt.then_some(up.round() as u32)),
        descent_m: u32_of(num(ses, "total_descent")).filter(|a| *a > 0).or(has_alt.then_some(down.round() as u32)),
        avg_speed_ms: first(ses, &["enhanced_avg_speed", "avg_speed"])
            .or_else(|| (moving_s > 0).then(|| distance_m / f64::from(moving_s))),
        max_speed_ms: first(ses, &["enhanced_max_speed", "max_speed"]).or((v_max > 0.0).then_some(v_max)),
        avg_hr: u16_of(num(ses, "avg_heart_rate")).or(avg(hr_sum, hr_n)),
        max_hr: u16_of(num(ses, "max_heart_rate")).or((hr_n > 0).then_some(hr_max)),
        avg_power: u16_of(num(ses, "avg_power")).or(power.avg()),
        max_power: u16_of(num(ses, "max_power")).or(power.max()),
        avg_cadence: u16_of(num(ses, "avg_cadence")).filter(|c| *c > 0).or(avg(cad_sum, cad_n)),
        np: power.np(),
        if_x100: power.if_x100(ftp),
        tss: power.tss(ftp),
        kcal: u32_of(num(ses, "total_calories")).filter(|k| *k > 0).or_else(|| power.np().map(|_| power.kcal())),
        laps,
        hr_zone_s: hr_z,
        power_zone_s: p_z,
        curve,
    };
    Ok(RideView { summary, samples: decimate(samples) })
}

#[cfg(test)]
#[path = "ride_tests.rs"]
mod tests;
