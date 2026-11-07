use super::*;
use crate::demo::synthetic_ride;
use bryton_rustfw::fit::crc;
use fitparser::{FitDataField, FitDataRecord, Value};

const ASC: u32 = 72;
const DESC: u32 = 69;

#[test]
fn demo_ride_summary() {
    let v = analyze_ride(synthetic_ride(9, 1500), 250, 190).unwrap();
    let s = &v.summary;
    assert_eq!(s.elapsed_s, 1500);
    assert!(s.distance_m > 0.0, "{s:?}");
    assert!(s.np.is_some() && s.tss.is_some() && s.if_x100.is_some(), "{s:?}");
    assert!(s.avg_power.is_some() && s.avg_hr.is_some() && s.avg_cadence.is_some());
    assert!(s.max_speed_ms.is_some() && s.avg_speed_ms.is_some());
    // the demo's altitude: 65 m +- 15 m over 600 s, +- 1.2 m wobble; both from the samples (3 m hysteresis)
    assert_eq!((s.ascent_m, s.descent_m), (Some(ASC), Some(DESC)), "{s:?}");
    assert_eq!(s.power_zone_s.iter().sum::<u32>(), 1500);
    assert_eq!(s.hr_zone_s.iter().sum::<u32>(), 1500);
    assert!(s.curve.iter().any(|c| c.secs == 300) && s.curve.iter().all(|c| c.secs != 3600), "{:?}", s.curve);
    let lat = v.samples.iter().find_map(|x| x.lat).unwrap();
    assert!((48.8..48.9).contains(&lat), "degrees: {lat}");
}

#[test]
fn no_ftp_no_intensity() {
    let s = analyze_ride(synthetic_ride(9, 600), 0, 0).unwrap().summary;
    assert!(s.np.is_some() && s.if_x100.is_none() && s.tss.is_none());
    assert_eq!(s.power_zone_s.iter().sum::<u32>(), 0);
}

#[test]
fn long_rides_are_decimated_with_peaks() {
    let v = analyze_ride(synthetic_ride(6, 9000), 250, 190).unwrap();
    assert!(v.samples.len() <= MAX_SAMPLES && v.samples.len() > 1000, "{}", v.samples.len());
    let max = v.samples.iter().filter_map(|s| s.power).max();
    assert_eq!(max, v.summary.max_power);
    // the last sample is kept, and the altitude's valleys and peaks (50 to 80 m)
    assert_eq!(v.samples.last().map(|s| s.t_s), Some(8999));
    let alts: Vec<f64> = v.samples.iter().filter_map(|s| s.alt_m).collect();
    let lo = alts.iter().copied().fold(f64::MAX, f64::min);
    let hi = alts.iter().copied().fold(f64::MIN, f64::max);
    assert!(lo < 51.0 && hi > 79.0, "{lo} {hi}");
}

/// The data CRC (and the header's, when it has one) made right again after a change, so the file reaches `view()`.
fn fix_crc(b: &mut [u8]) {
    let (Some(&h), true) = (b.first(), b.len() > 16) else { return };
    let h = usize::from(h);
    let end = b.len() - 2;
    let c = crc(0, b.get(h..end).unwrap_or(&[])).to_le_bytes();
    b[end..].copy_from_slice(&c);
    if h == 14 {
        let c = crc(0, &b[..12]).to_le_bytes();
        b[12..14].copy_from_slice(&c);
    }
}

#[test]
fn damaged_files_never_panic() {
    let good = synthetic_ride(9, 300);
    assert!(!good.is_empty());
    let mut x = 0x1234_5678u32;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    // (Ok, Empty, Unreadable): the CRCs are made right again, so the damaged content is what is tested
    let mut seen = (0, 0, 0);
    let mut count = |r: Result<RideView, RideError>| match r {
        Ok(_) => seen.0 += 1,
        Err(RideError::Empty) => seen.1 += 1,
        Err(RideError::Unreadable) => seen.2 += 1,
    };
    for n in (0..good.len()).step_by(5) {
        count(analyze_ride(good[..n].to_vec(), 250, 190));
        let mut cut = good[..n].to_vec();
        fix_crc(&mut cut);
        count(analyze_ride(cut, 250, 190));
    }
    for _ in 0..1500 {
        assert!(analyze_ride((0..rnd() % 200).map(|_| rnd() as u8).collect(), 250, 190).is_err());
        let mut bad = good.clone();
        // half of the time the 14-byte file header is left alone, so the messages are what changes
        let from = if rnd() % 2 == 0 { 14 } else { 0 };
        for _ in 0..1 + rnd() % 4 {
            let i = from + rnd() as usize % (bad.len() - from);
            bad[i] ^= 1 << (rnd() % 8);
        }
        fix_crc(&mut bad);
        count(analyze_ride(bad, 250, 190));
    }
    assert!(analyze_ride(Vec::new(), 0, 0).is_err());
    assert!(seen.0 > 100 && seen.1 + seen.2 > 100, "the fuzz reaches view() and the refusals: {seen:?}");
}

#[test]
fn a_ride_with_a_bad_crc_still_opens() {
    let mut good = synthetic_ride(9, 300);
    let last = good.len() - 1;
    good[last] ^= 0xff; // the data CRC of a ride never finalized
    let v = analyze_ride(good, 250, 190).unwrap();
    assert_eq!(v.summary.elapsed_s, 300);
}

fn rec(fields: &[(&str, f64)]) -> FitDataRecord {
    let mut r = FitDataRecord::new(MesgNum::Record);
    for (i, (name, v)) in fields.iter().enumerate() {
        r.push(FitDataField::new((*name).into(), i as u8, None, Value::Float64(*v), String::new()));
    }
    r
}

#[test]
fn view_takes_odd_records() {
    let mut session = FitDataRecord::new(MesgNum::Session);
    for (i, name) in ["total_distance", "total_ascent", "avg_power", "max_speed"].iter().enumerate() {
        let v = [f64::NAN, f64::INFINITY, -5.0, 0.0][i];
        session.push(FitDataField::new((*name).into(), i as u8, None, Value::Float64(v), String::new()));
    }
    let msgs = vec![
        rec(&[]),                                             // no timestamp: skipped
        rec(&[("timestamp", 1000.0), ("heart_rate", 140.0)]), // no power, no position
        rec(&[("timestamp", 1001.0), ("power", 200.0), ("altitude", 100.0)]),
        rec(&[("timestamp", 990.0), ("power", f64::NAN)]), // time going backwards
        rec(&[("timestamp", 1030.0), ("power", 70000.0)]), // a 29 s gap, a power over u16
        session,
    ];
    let v = view(&msgs, 250, 190).unwrap();
    assert_eq!(v.samples.len(), 4);
    assert!(v.summary.power_zone_s.iter().sum::<u32>() <= 4 * 5, "a gap counts for at most 5 s");
    assert_eq!(view(&[rec(&[])], 250, 190), Err(RideError::Empty));
}

#[test]
fn smart_recording_counts_seconds_not_records() {
    // the same 600 s recorded every 1 s and every 3 s: the zone seconds and the averages agree
    let every = |dt: u32| -> Vec<FitDataRecord> {
        (0..600 / dt)
            .map(|i| rec(&[("timestamp", f64::from(1000 + i * dt)), ("power", 200.0), ("heart_rate", 150.0)]))
            .collect()
    };
    let a = view(&every(1), 250, 190).unwrap().summary;
    let b = view(&every(3), 250, 190).unwrap().summary;
    assert_eq!(a.power_zone_s.iter().sum::<u32>(), 600);
    assert!((598..=600).contains(&b.power_zone_s.iter().sum::<u32>()), "the last record counts 1 s");
    assert_eq!((a.np, a.avg_power, a.avg_hr), (b.np, b.avg_power, b.avg_hr));
}
