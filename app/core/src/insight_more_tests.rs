use super::*;
use crate::demo::{hill_ride, profile_ride};

const T0: u32 = 1_100_000_000;

/// Pseudo-random in [-1, 1] from the second and a channel.
fn noise(i: u32, k: u32) -> f64 {
    f64::from((i.wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(40_503)) >> 16) & 0xffff) / 32767.5 - 1.0
}

/// The hill of `hill_ride` (0.25 m a second for 600 s from second 600, 5 m/s) with `amp` metres of noise on the altitude.
fn hill_at(amp: f64, reversed: bool) -> impl Fn(u32) -> (f64, f64, u16, u8) {
    move |i| {
        let up = f64::from(i.clamp(600, 1200) - 600) * 0.25;
        let alt = if reversed { 250.0 - up } else { 100.0 + up };
        (alt + amp * noise(i, 1), if (600..1200).contains(&i) { 5.0 } else { 6.0 }, 200, 140)
    }
}

#[test]
fn a_noisy_hill_is_one_climb_and_a_noisy_flat_none() {
    let hill = ride_insight(profile_ride(T0, 2000, false, hill_at(2.0, false)), 0, 0, 0).unwrap();
    assert_eq!(hill.climbs.len(), 1, "{:?}", hill.climbs);
    assert!((130..=170).contains(&hill.climbs[0].gain_m), "{:?}", hill.climbs[0]);
    let flat = |i: u32| (100.0 + 3.0 * noise(i, 2), 6.0, 200, 140);
    let r = ride_insight(profile_ride(T0, 2400, false, flat), 0, 0, 0).unwrap();
    assert!(r.climbs.is_empty(), "{:?}", r.climbs);
}

#[test]
fn riding_the_hill_downwards_is_no_climb() {
    let r = ride_insight(profile_ride(T0, 2000, false, hill_at(0.0, true)), 0, 0, 0).unwrap();
    assert!(r.climbs.is_empty(), "{:?}", r.climbs);
}

#[test]
fn rr_values_are_seconds_or_ms_within_the_strap_range() {
    assert_eq!(rr_ms(0.8), Some(800));
    assert_eq!(rr_ms(800.0), Some(800));
    assert_eq!(rr_ms(0.25), Some(250));
    let out = [rr_ms(0.1), rr_ms(2500.0), rr_ms(65.535), rr_ms(f64::NAN), rr_ms(f64::INFINITY)];
    assert_eq!(out, [None; 5]);
    // the beats of the file: all in range, the first one the first second's heart rate (133 bpm, 451 ms)
    let b = beats(&parse_msgs(&hill_ride(T0)).unwrap());
    assert!(b.len() > 2500 && b.iter().all(|x| (250..=2000).contains(x)), "{}", b.len());
    assert!((449..=453).contains(&b[0]), "{}", b[0]);
}

fn stat(lat: f64, len: u32) -> ClimbStat {
    ClimbStat {
        len_m: len,
        start_lat: Some(lat),
        start_lon: Some(2.0),
        end_lat: Some(lat),
        end_lon: Some(2.01),
        ..Default::default()
    }
}

#[test]
fn climbs_match_within_the_larger_of_150_m_and_a_quarter_of_the_length() {
    let north = |m: f64| 48.0 + m / 111_320.0;
    // a 500 m climb: a quarter is 125 m, so 150 m rules
    assert_eq!(match_climbs(vec![stat(48.0, 500)], vec![stat(north(145.0), 500)]).len(), 1);
    assert!(match_climbs(vec![stat(48.0, 500)], vec![stat(north(155.0), 500)]).is_empty());
    // a 3 km climb: 750 m
    assert_eq!(match_climbs(vec![stat(48.0, 3000)], vec![stat(north(700.0), 3000)]).len(), 1);
    assert!(match_climbs(vec![stat(48.0, 3000)], vec![stat(north(800.0), 3000)]).is_empty());
    // two candidates: the nearer one is taken, not the first found
    let p = match_climbs(vec![stat(48.0, 3000)], vec![stat(north(500.0), 3000), stat(north(50.0), 3000)]);
    assert_eq!(p, vec![ClimbPair { first: 0, second: 1 }]);
}

/// Records made of the real structure with missing, NaN, huge, repeated and backwards values, and files with flipped
/// bytes: nothing panics or hangs, and the generator reaches climbs, W' balance, decoupling and DFA.
#[test]
fn fuzz_insight() {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut r = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    let (mut with_climbs, mut with_wbal, mut with_dec, mut with_dfa, mut ok_files) = (0, 0, 0, 0, 0);
    for seed in 0..60u64 {
        let n = [60usize, 400, 1500, 2600][r(4) as usize];
        let wild = seed % 3 != 0; // a third of the rides are clean
        let (mut dist, mut t) = (0.0f64, 0u32);
        let mut all = Vec::new();
        for i in 0..n as u32 {
            t = match (wild, r(120)) {
                (true, 0) => t, // a repeat
                (true, 1) => t.saturating_sub(30),
                (true, 2) => u32::MAX - r(1000) as u32,
                _ => t.saturating_add(1),
            };
            dist = match (wild, r(150)) {
                (true, 0) => 0.0,
                (true, 1) => dist + 1.0e7,
                (true, 2) => f64::NAN,
                (true, 3) => f64::INFINITY,
                _ => dist + 6.0,
            };
            let hill = 100.0 + f64::from(i.clamp(300, 900) - 300) * 0.25 + 2.0 * noise(i, 3);
            let mut pick = |good: f64, bad: [f64; 3]| match (wild, r(60)) {
                (true, 0) => None,
                (true, k @ 1..=3) => bad.get(k as usize - 1).copied(),
                _ => Some(good),
            };
            let (alt_m, speed_ms) = (pick(hill, [f64::NAN, 1.0e30, -1.0e30]), pick(6.0, [f64::NAN, -3.0, 1.0e9]));
            let (h, p) = if wild { (r(4) as usize, r(4) as usize) } else { (0, 0) };
            all.push(TrackSample {
                t_s: t,
                dist_m: dist,
                lat: Some(48.0),
                lon: Some(2.0 + dist / 70_000.0),
                alt_m,
                speed_ms,
                hr: [Some(120 + r(60) as u16), Some(0), None, Some(u16::MAX)][h],
                power: [Some(r(400) as u16), Some(u16::MAX), None, Some(0)][p],
                cadence: None,
            });
        }
        let w = [0u16, 70][r(2) as usize];
        with_climbs += usize::from(!climbs(&all, w).is_empty());
        with_wbal += usize::from(!wbal(&all, [0u16, 200, 250][r(3) as usize], [0u16, 20_000][r(2) as usize]).is_empty());
        with_dec += usize::from(decoupling(&all).is_some());
        // beats: a slow wander with now and then a wild one
        let mut b = 800u16;
        let mut rr = Vec::new();
        for _ in 0..r(1500) {
            b = (i32::from(b) + r(9) as i32 - 4).clamp(500, 1100) as u16;
            rr.push(if wild && r(40) == 0 { r(65_535) as u16 } else { b });
        }
        with_dfa += usize::from(!dfa(&rr).is_empty());
        // a valid file with a few bytes changed (a parse of a long file is slow in a debug build: one seed in six)
        if seed % 6 != 0 {
            continue;
        }
        let mut f = hill_ride(T0);
        for _ in 0..1 + r(3) {
            let k = r(f.len() as u64) as usize;
            if let Some(byte) = f.get_mut(k) {
                *byte ^= 1 << r(8);
            }
        }
        if let Ok(v) = ride_insight(f, 70, 250, 20_000) {
            ok_files += 1;
            with_climbs += usize::from(!v.climbs.is_empty());
        }
    }
    assert!(
        with_climbs > 15 && with_wbal > 10 && with_dec > 3 && with_dfa > 30 && ok_files > 5,
        "{with_climbs} {with_wbal} {with_dec} {with_dfa} {ok_files}"
    );
}
