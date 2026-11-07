use super::*;
use crate::demo::synthetic_ride;

#[test]
fn demo_ride_summary() {
    let v = analyze_ride(synthetic_ride(9, 1500), 250, 190).unwrap();
    let s = &v.summary;
    assert_eq!(s.elapsed_s, 1500);
    assert!(s.distance_m > 0.0, "{s:?}");
    assert!(s.np.is_some() && s.tss.is_some() && s.if_x100.is_some(), "{s:?}");
    assert!(s.avg_power.is_some() && s.avg_hr.is_some() && s.avg_cadence.is_some());
    assert!(s.max_speed_ms.is_some() && s.avg_speed_ms.is_some());
    assert!(s.ascent_m.is_some());
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
}

#[test]
fn damaged_files_never_panic() {
    let good = synthetic_ride(9, 300);
    assert!(!good.is_empty());
    let mut ok = 0;
    for n in (0..good.len()).step_by(7) {
        ok += usize::from(analyze_ride(good[..n].to_vec(), 250, 190).is_ok());
    }
    assert!(ok < good.len() / 7, "a truncation is refused");
    // random bytes, and bit flips in a good file
    let mut x = 0x1234_5678u32;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for _ in 0..300 {
        let junk: Vec<u8> = (0..rnd() % 200).map(|_| rnd() as u8).collect();
        assert!(analyze_ride(junk, 250, 190).is_err());
        let mut bad = good.clone();
        for _ in 0..3 {
            let i = rnd() as usize % bad.len();
            bad[i] ^= 1 << (rnd() % 8);
        }
        let _ = analyze_ride(bad, 250, 190);
    }
    assert!(analyze_ride(Vec::new(), 0, 0).is_err());
}
