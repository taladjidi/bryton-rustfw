//! The barometric altitude (altitude.rs, app/barometer.rs) on the simulated bus: a climb with noise on both
//! sources, the calibrations, the file, the fallback when the chip is silent.

use super::*;
use crate::altitude::{By, Cal, P_STD};

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

/// Noise in -1.0..1.0, a function of the time (the same on every run).
fn noise(ms: u64, salt: u64) -> f64 {
    let x = (ms / 1000).wrapping_add(salt).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    ((x >> 33) % 2001) as f64 / 1000.0 - 1.0
}

/// Climbing 0.5 m/s from 100 m, the truth.
fn truth(ms: u64) -> i32 {
    100_000 + (ms / 2) as i32
}

/// The receiver's altitude: the truth and 4 m of noise a second.
fn gps_noisy(ms: u64) -> i32 {
    truth(ms) + (noise(ms, 1) * 4000.0) as i32
}

fn flat_300(_: u64) -> i32 {
    300_000
}

/// A card to record on and the settings of this firmware.
fn unit(min: u64) -> Sim {
    let mut sim = Sim::new(9, min, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.gnss.fix = Some(5000);
    sim
}

/// `\Rustfw\altcal.bin` as the card holds it.
fn stored(sim: &Sim) -> Option<Cal> {
    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Rustfw/altcal.bin").ok()?;
    let mut s = [0u8; 512];
    assert_eq!(fat.read_at(&mut d, &f, 0, &mut s), Ok(512));
    Cal::from_sector(&s)
}

/// A 10-minute climb of 300 m with 4 m of receiver noise: the barometer's ascent is near the truth, the
/// receiver's alone is not (its noise counted past its hysteresis), and the ride's altitude is the barometer's.
#[test]
fn the_barometer_counts_the_climb() {
    let ascent = |baro: bool| {
        let mut sim = unit(0);
        sim.gnss.alt = Some(gps_noisy);
        sim.bus.baro_alt = Some(truth);
        sim.bus.baro_noise = 100; // 2.4 Pa
        sim.bus.baro = baro.then_some((10_132, 2150));
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 12_000);
        app.alt_from_gps(); // calibrated before the start (a stale one is made again at the start: module doc)
        run(&mut sim, &mut app, 35_000);
        app.start_pause();
        let t0 = sim.ms;
        run(&mut sim, &mut app, 600_000);
        let climbed = (sim.ms - t0) as i32 / 2000; // m
        let alt = app.alt_now();
        if baro {
            let a = alt.0.unwrap();
            assert!(a.abs_diff(truth(sim.ms)) < 8000, "{a} against {}", truth(sim.ms));
        }
        (app.ride.ascent_m() as i32, climbed, alt.1, sim.bus.baro_conversions)
    };
    let (up, truth_m, reference, conversions) = ascent(true);
    let (gps_up, ..) = ascent(false);
    std::println!("ascent: barometer {up} m, receiver {gps_up} m, truth {truth_m} m; {conversions} conversions");
    assert!((up - truth_m).abs() * 100 <= truth_m * 4, "the barometer: {up} m of {truth_m}");
    assert!((up - truth_m).abs() < (gps_up - truth_m).abs(), "steadier than the receiver's {gps_up} m");
    assert_ne!(reference, 0, "the barometer's reference");
    // a conversion a second while riding, one in ten idle
    assert!((595..=700).contains(&conversions), "{conversions} conversions");
}

/// The calibration at a ride's start, from the receiver: none or stale, 30 s of 3D fix; written between rides, read
/// at the next start; fresh, a ride does not make another.
#[test]
fn a_ride_start_calibrates_and_it_is_kept() {
    let mut sim = unit(0);
    sim.gnss.alt = Some(flat_300);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 8000);
    assert_eq!((app.alt.cal(), app.alt_now().1), (None, 0), "nothing yet: the receiver's altitude");
    app.start_pause();
    run(&mut sim, &mut app, 40_000);
    let cal = app.alt.cal().expect("calibrated at the start");
    assert_eq!(cal.by, By::Auto);
    let (alt, r) = app.alt_now();
    assert!(r != 0 && alt.unwrap().abs_diff(300_000) < 1000, "{alt:?} {r}");
    assert_eq!(stored(&sim), None, "nothing written during a ride");
    app.start_pause(); // paused
    app.end_ride();
    run(&mut sim, &mut app, 30_000);
    assert_eq!(stored(&sim), Some(cal), "kept between rides");

    // the next start, minutes later: read, fresh, so no new average
    let mut sim2 = unit(5);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.gnss.alt = Some(flat_300);
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, u64::from(ALT_LOAD_MS) + 2000);
    assert_eq!(app.alt.cal(), Some(cal));
    app.start_pause();
    assert_eq!(app.alt.progress(), None, "a calibration of minutes ago is kept");
    assert_ne!(app.alt_now().1, 0, "and used from the first second");
}

/// A typed altitude in SYSTEM > BUILT IN, then the ride takes the barometer's altitude although the receiver says
/// otherwise; FROM GPS averages 30 s and says so.
#[test]
fn typed_and_from_gps() {
    let mut sim = unit(0);
    sim.gnss.alt = Some(flat_300);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, menu::Row::Open(Menu::BuiltIn));
    app.press(&mut sim, Press::Short(Key::Start));
    let row = |app: &App, i| {
        let (l, v, _) = app.menu_row(Menu::BuiltIn, i).unwrap();
        std::format!("{} {}", l.as_str(), v.as_str())
    };
    assert_eq!(row(&app, 3), "ALTITUDE 0 M", "the standard day's sea-level pressure until calibrated");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start)); // edit
    for _ in 0..24 {
        app.press(&mut sim, Press::Short(Key::Lap)); // up, 5 m a press
    }
    assert_eq!(row(&app, 3), "ALTITUDE < 120 >");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(row(&app, 3), "ALTITUDE 120 M");
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Typed));
    assert_ne!(app.alt_now().1, 0, "the barometer, though the receiver says 300 m");
    assert!(app.alt_now().0.unwrap().abs_diff(120_000) < 1500);

    // FROM GPS: the average runs, the row shows it
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let v = row(&app, 4);
    assert!(v.starts_with("FROM GPS ") && v.contains('/'), "{v}");
    run(&mut sim, &mut app, 30_000);
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Gps));
    assert!(app.alt.alt_mm().unwrap().abs_diff(300_000) < 1000);
    assert_eq!(row(&app, 4), "FROM GPS ");
}

/// The barometer silent, from the start or in the middle of a ride: the receiver's altitude, and the ascent goes on.
#[test]
fn the_receiver_when_the_barometer_is_silent() {
    let mut sim = unit(0);
    sim.bus.baro = None;
    sim.gnss.alt = Some(gps_noisy);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 20_000);
    assert_eq!(app.alt_now().1, 0);
    assert!(app.alt_now().0.is_some());
    assert_eq!(app.menu_row(Menu::BuiltIn, 3).unwrap().1.as_str(), "NONE");

    let mut sim = unit(0);
    sim.gnss.alt = Some(truth);
    sim.bus.baro_alt = Some(truth);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 30_000);
    app.start_pause();
    run(&mut sim, &mut app, 60_000);
    assert_ne!(app.alt_now().1, 0, "the barometer");
    let before = app.ride.ascent_m();
    let baro = sim.bus.baro.take();
    run(&mut sim, &mut app, 120_000);
    assert_eq!(app.alt_now().1, 0, "silent: the receiver's");
    assert!(app.ride.ascent_m() >= before + 40, "the climb counted on: {} m", app.ride.ascent_m());
    sim.bus.baro = baro;
    run(&mut sim, &mut app, 20_000);
    assert_ne!(app.alt_now().1, 0, "back");
}

/// Garbage from the chip into a whole ride (altitudes of hundreds of kilometres, pressures from nothing to full
/// scale): no panic, the ride goes on, and both sources are used along the way.
#[test]
fn a_garbage_barometer_cannot_stop_a_ride() {
    fn wild(ms: u64) -> i32 {
        let x = (ms / 1000).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 20;
        match x % 5 {
            0 => (x as i32).wrapping_mul(1001),
            1 => -300_000_000,
            2 => 300_000_000,
            _ => 100_000 + (x % 5000) as i32,
        }
    }
    let mut sim = unit(0);
    sim.gnss.alt = Some(truth);
    sim.bus.baro_alt = Some(wild);
    sim.bus.baro_noise = 4000;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    app.start_pause();
    let mut seen = [0u32; 2];
    for _ in 0..300 {
        run(&mut sim, &mut app, 1000);
        seen[usize::from(app.alt_now().1 != 0)] += 1;
    }
    assert!(app.ride.timer_s() > 250, "the ride went on");
    assert!(seen[0] > 20 && seen[1] > 20, "both sources reached: {seen:?}");
}

/// Stock's `Alt_P0` is taken as a calibration that is not trusted, and not over one made here.
#[test]
fn stock_sea_level_pressure() {
    let mut sim = unit(0);
    let mut app = App::start(&mut sim, None);
    assert!(app.alt.import_stock(10_200));
    assert_eq!(app.alt.cal().map(|c| (c.by, c.p0)), Some((By::Stock, 10_200 * 4096 / 10)));
    assert_ne!(app.alt.cal().unwrap().p0, P_STD);
}
