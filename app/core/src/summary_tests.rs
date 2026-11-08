use crate::demo::synthetic_ride;
use crate::ride::analyze_ride;
use crate::summary::*;

fn stat(t: i64, off: i32, load: u32) -> RideStat {
    RideStat {
        start_unix: t,
        local_day: None,
        load_from_hr: false,
        tz_offset_s: off,
        moving_s: 100,
        distance_m: 1000.0,
        load: Some(load),
    }
}

#[test]
fn the_brief_and_the_viewer_agree() {
    let v = analyze_ride(synthetic_ride(9, 1500), 250, 190).unwrap();
    let s = &v.summary;
    assert_eq!(s.load, s.tss, "{s:?}");
    assert!(!s.load_from_hr && s.load.is_some());
    assert_eq!(brief_load(&s.brief, 250, 190), (s.load, false));
    // the demo coasts at speed 0 for 20 s of every 450, so moving is less than elapsed
    assert!(s.moving_s < s.elapsed_s && s.moving_s > s.elapsed_s * 9 / 10, "{} of {}", s.moving_s, s.elapsed_s);
    let b = ride_brief(synthetic_ride(9, 1500)).unwrap();
    assert_eq!(b, s.brief);
    assert_eq!(brief_decode(brief_encode(b.clone())), Some(b));
}

#[test]
fn without_power_the_load_is_heart_rate() {
    let b = RideBrief {
        moving_s: 3600,
        hr_bins: vec![HrBin { bpm: 161, secs: 3600 }], // 85 % of 190 for an hour: 100 by construction
        ..RideBrief::default()
    };
    let (l, hr) = brief_load(&b, 250, 190);
    assert!(hr && (99..=101).contains(&l.unwrap()), "{l:?}");
    assert_eq!(brief_load(&b, 250, 0), (None, false));
    assert_eq!(brief_load(&RideBrief::default(), 250, 190), (None, false));
    // power with no FTP falls back to heart rate
    let p = RideBrief { np: 200, power_s: 3600, ..b };
    assert!(brief_load(&p, 0, 190).1 && brief_load(&p, 200, 190) == (Some(100), false));
}

#[test]
fn decode_refuses_other_text() {
    for t in ["", "2 1 1 1 1 1 1 ", "1 a", "1 1 1 1 NaN 0 0 ", "1 1 1 1 1 0 0 5:x", "1 1 1 1 1 0 0  extra"] {
        assert_eq!(brief_decode(t.into()), None, "{t}");
    }
}

#[test]
fn weeks_start_on_monday_in_local_time() {
    // 2026-11-08 is a Sunday. Sunday 22:30 UTC is 23:30 in Paris (UTC+1); an hour later it is Monday 00:30 there.
    let sun = 1_794_177_000; // 2026-11-08 22:30 UTC
    let mon = sun + 3600;
    let r = weekly_totals(vec![stat(sun, 3600, 10), stat(mon, 3600, 20)], 0, mon, 3600, 2);
    assert_eq!((r[0].rides, r[0].load, r[1].rides, r[1].load), (1, 10, 1, 20), "{r:?}");
    assert_eq!(r[1].first_day - r[0].first_day, 7);
    // the same instants in UTC are both Sunday
    let r = weekly_totals(vec![stat(sun, 0, 10), stat(mon, 0, 20)], 0, mon, 0, 2);
    assert_eq!((r[0].rides, r[1].rides), (0, 2));
    // a Sunday start
    let r = weekly_totals(vec![stat(sun, 0, 10)], 6, sun, 0, 1);
    assert_eq!(r[0].rides, 1);
    // DST: each ride keeps the offset it had
    let r = weekly_totals(vec![stat(mon - 86_400 * 7, 7200, 5), stat(mon, 3600, 5)], 0, mon, 3600, 3);
    assert_eq!((r[0].rides, r[1].rides, r[2].rides), (0, 1, 1));
}

#[test]
fn weekly_sums_hold_over_random_rides() {
    let mut x = 0x9e37_79b9u64;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut kept = 0;
    for _ in 0..300 {
        let now = (rnd() % 4_000_000_000) as i64;
        let stats: Vec<RideStat> = (0..rnd() % 40)
            .map(|_| {
                let t = if rnd() % 5 == 0 { rnd() as i64 } else { now - (rnd() % (30 * 86_400)) as i64 };
                RideStat {
                    start_unix: t,
                    local_day: None,
                    load_from_hr: false,
                    tz_offset_s: (rnd() % 100_000) as i32 - 50_000,
                    moving_s: (rnd() % 20_000) as u32,
                    distance_m: (rnd() % 200_000) as f64,
                    load: (rnd() % 2 == 0).then(|| (rnd() % 500) as u32),
                }
            })
            .collect();
        let w = weekly_totals(stats.clone(), (rnd() % 9) as u8, now, 3600, (rnd() % 12) as u32);
        let n: u32 = w.iter().map(|w| w.rides).sum();
        let mov: u64 = w.iter().map(|w| w.moving_s).sum();
        assert!(n as usize <= stats.len());
        assert!(mov <= stats.iter().map(|s| u64::from(s.moving_s)).sum());
        assert!(w.windows(2).all(|p| p[1].first_day - p[0].first_day == 7));
        kept += n;
    }
    assert!(kept > 500, "the fuzz reaches the buckets: {kept}");
    assert!(weekly_totals(vec![], 0, i64::MAX, i32::MAX, u32::MAX).len() <= 520);
}

#[test]
fn decode_fuzz_round_trips_what_it_accepts() {
    let mut x = 0x2545_f491u64;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let odd = ["NaN", "inf", "-5", "-0.5", "99999999999999999999", "", "1e400", "x", "300", "70000"];
    let (mut some, mut none) = (0, 0);
    for _ in 0..2000 {
        let mut tok = |m: u64| {
            let (valid, coin, pick) = (rnd() % m, rnd() % 6, (rnd() % 10) as usize);
            if coin == 0 { odd[pick].to_string() } else { valid.to_string() }
        };
        let nbins = tok(8).parse::<u64>().unwrap_or(0);
        let bins: Vec<String> = (0..nbins)
            .map(|_| {
                let (a, b) = (tok(256), tok(5000));
                format!("{a}:{b}")
            })
            .collect();
        let line = format!(
            "1 {} {} {} {} {} {} {}",
            tok(2_000_000_000),
            tok(90_000),
            tok(90_000),
            tok(200_000),
            tok(500),
            tok(90_000),
            bins.join(",")
        );
        match brief_decode(line) {
            Some(b) => {
                some += 1;
                assert!(b.distance_m >= 0.0 && b.distance_m.is_finite());
                assert_eq!(brief_decode(brief_encode(b.clone())), Some(b));
            }
            None => none += 1,
        }
    }
    assert!(some > 100 && none > 100, "the fuzz reaches both sides: {some} / {none}");
    // a repeated heart rate, and too many bins
    assert_eq!(brief_decode("1 1 1 1 1 0 0 70:1,70:2".into()), None);
    let many: Vec<String> = (0..=256).map(|i| format!("{}:1", i % 256)).collect();
    assert_eq!(brief_decode(format!("1 1 1 1 1 0 0 {}", many.join(","))), None);
    assert_eq!(brief_decode("1 1 1 -1 1 0 0 ".into()), None);
}

#[test]
fn ride_brief_of_damaged_rides_never_panics() {
    let good = synthetic_ride(9, 300);
    let mut x = 0x1357_9bdfu32;
    let mut rnd = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let mut some = 0;
    for _ in 0..600 {
        let mut bad = good.clone();
        for _ in 0..1 + rnd() % 4 {
            let i = rnd() as usize % bad.len();
            bad[i] ^= 1 << (rnd() % 8);
        }
        if rnd() % 4 == 0 {
            bad.truncate(rnd() as usize % bad.len());
        }
        some += usize::from(ride_brief(bad).is_some());
    }
    assert!(some > 100 && some < 600, "{some}");
}

#[test]
fn the_week_comes_from_the_file_name() {
    // 2026-11-09 is a Monday: this instant is still Sunday in UTC, the name says Monday
    let b = RideBrief { start_unix: 1_794_182_400 - 60, moving_s: 60, ..RideBrief::default() };
    let mon = ride_stat(b.clone(), "261109000100".into(), 200, 190, 0);
    assert_eq!(mon.local_day, name_day("261109000000"));
    let sun = ride_stat(b.clone(), "261108235900".into(), 200, 190, 0);
    let r = weekly_totals(vec![mon, sun], 0, 1_794_182_400 + 3600, 0, 2);
    assert_eq!((r[0].rides, r[1].rides), (1, 1));
    // a name that is not a date falls back to the instant
    assert_eq!(name_day("261301000000"), None);
    assert_eq!(name_day("260230000000"), None);
    assert_eq!(name_day("26x109000100"), None);
    assert_eq!(ride_stat(b, "tmp".into(), 200, 190, 0).local_day, None);
}
