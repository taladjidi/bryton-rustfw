use super::*;
use crate::demo::{hill_ride, profile_ride, synthetic_ride};

const T0: u32 = 1_100_000_000;

#[test]
fn the_hill_is_the_one_climb() {
    // 600 s flat at 6 m/s, then 600 s at 5 m/s up 0.25 m a second (3000 m, 150 m, 5 %) at 280 W
    let r = ride_insight(hill_ride(T0), 70, 250, 20_000).unwrap();
    assert_eq!(r.climbs.len(), 1, "{:?}", r.climbs);
    let c = &r.climbs[0];
    assert!((3500..3700).contains(&c.start_m), "{c:?}");
    assert!((2800..3200).contains(&c.len_m) && (140..=160).contains(&c.gain_m), "{c:?}");
    assert!((45..=55).contains(&c.avg_grade_x10) && c.max_grade_x10 >= c.avg_grade_x10, "{c:?}");
    assert!((540..=660).contains(&c.time_s), "{c:?}");
    // VAM = gain / time: about 150 m in 600 s
    assert!(c.vam.is_some_and(|v| (800..=1000).contains(&v)), "{c:?}");
    assert!(c.avg_power.is_some_and(|p| (270..=290).contains(&p)), "{c:?}");
    assert!(c.w_per_kg_x100.is_some_and(|w| (380..=410).contains(&w)), "{c:?}");
    // 5 % is the second band of the unit's grade colours
    assert_eq!(c.color, rgb(grade_color(50)));
    assert!(c.start_lat.is_some() && c.end_lon.is_some());
    // without a weight, no W/kg
    let r0 = ride_insight(hill_ride(T0), 0, 250, 20_000).unwrap();
    assert_eq!(r0.climbs[0].w_per_kg_x100, None);
}

#[test]
fn a_flat_ride_has_no_climb_and_no_beats() {
    let r = ride_insight(synthetic_ride(9, 1500), 70, 250, 20_000).unwrap();
    assert!(r.climbs.is_empty() && r.dfa.is_empty(), "{:?}", r.climbs);
}

#[test]
fn w_prime_balance_by_hand() {
    // 100 s at 300 W over a CP of 200 W: 100 J a second off a full 20 kJ, none given back above CP: 10 000 J.
    // Then one second at 100 W: back (20 000 - 10 000) x 100 / 20 000 = 50 J.
    let f = |i: u32| (100.0, 6.0, if i < 100 { 300 } else { 100 }, 140u8);
    let r = ride_insight(profile_ride(T0, 101, false, f), 0, 200, 20_000).unwrap();
    assert_eq!(r.wbal.get(99).map(|p| p.wbal_j), Some(10_000), "{:?}", r.wbal.get(95..));
    assert_eq!(r.wbal.last().map(|p| p.wbal_j), Some(10_050));
    assert_eq!(r.wbal_min_j, Some(10_000));
    // no W' given, no graph
    assert!(ride_insight(profile_ride(T0, 101, false, f), 0, 200, 0).unwrap().wbal.is_empty());
}

#[test]
fn decoupling_of_a_constructed_ride() {
    // 200 W all along; 140 bpm for the first 20 minutes, 147 for the second: EF 200/140 then 200/147, 1 - 140/147
    let f = |i: u32| (100.0, 6.0, 200, if i < 1200 { 140u8 } else { 147 });
    let d = ride_insight(profile_ride(T0, 2400, false, f), 0, 0, 0).unwrap().decoupling.unwrap();
    assert!((d.ef_first - 200.0 / 140.0).abs() < 0.01 && (d.ef_second - 200.0 / 147.0).abs() < 0.01, "{d:?}");
    assert!((d.percent - 100.0 * (1.0 - 140.0 / 147.0)).abs() < 0.2, "{d:?}");
    // a ride too short for it
    let short = ride_insight(profile_ride(T0, 900, false, f), 0, 0, 0).unwrap();
    assert_eq!(short.decoupling, None);
}

#[test]
fn dfa_follows_the_beats_of_the_file() {
    let r = ride_insight(hill_ride(T0), 70, 250, 20_000).unwrap();
    assert!(r.dfa.len() > 100, "{}", r.dfa.len());
    assert!(r.dfa.windows(2).all(|w| w[0].t_s <= w[1].t_s) && r.dfa.iter().all(|p| p.a1.is_finite() && p.t_s < 3200));
    assert!((r.vt1 - 0.75).abs() < 1e-6 && (r.vt2 - 0.5).abs() < 1e-6);
}

#[test]
fn the_same_climb_is_found_in_the_other_ride() {
    let a = ride_insight(hill_ride(T0), 70, 250, 20_000).unwrap().climbs;
    let b = ride_insight(hill_ride(T0 + 86_400), 70, 250, 20_000).unwrap().climbs;
    assert_eq!(match_climbs(a.clone(), b), vec![ClimbPair { first: 0, second: 0 }]);
    // 0.01 degrees of latitude (1.1 km) away is another hill
    let far: Vec<ClimbStat> = a
        .iter()
        .map(|c| ClimbStat {
            start_lat: c.start_lat.map(|l| l + 0.01),
            end_lat: c.end_lat.map(|l| l + 0.01),
            ..c.clone()
        })
        .collect();
    assert!(match_climbs(a.clone(), far).is_empty());
    // a climb with no position matches nothing
    let blind = vec![ClimbStat { start_lat: None, ..a[0].clone() }];
    assert!(match_climbs(a, blind).is_empty());
}

#[test]
fn bad_files_are_refused() {
    assert_eq!(ride_insight(vec![1, 2, 3], 70, 250, 20_000), Err(RideError::Unreadable));
}
