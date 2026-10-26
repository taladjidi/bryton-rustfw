//! The barometric altitude (altitude.rs, app/barometer.rs) on the simulated bus: a climb with noise on both
//! sources, the calibrations, the file, the fallback when the chip is silent.

use super::*;
use crate::altitude::{By, Cal, P_STD};

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
    assert!(r != 0 && alt.unwrap() < 20_000, "the standard day's level still, let go slowly: {alt:?}");
    run(&mut sim, &mut app, 600_000); // 0.5 m a second: the 294 m to the receiver's level in 10 min
    assert_eq!(app.ride.ascent_m(), 0, "a correction, not a climb");
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

/// A typed altitude in SETTINGS > BIKE, then the ride takes the barometer's altitude although the receiver says
/// otherwise; FROM GPS averages 30 s and says so.
#[test]
fn typed_and_from_gps() {
    let mut sim = unit(0);
    sim.gnss.alt = Some(flat_300);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    let row = |app: &App, i| {
        let (l, v, _) = app.menu_row(Menu::Bike, i).unwrap();
        std::format!("{} {}", l.as_str(), v.as_str())
    };
    assert_eq!(row(&app, 2), "ALTITUDE 0 M", "the standard day's sea-level pressure until calibrated");
    app.press(&mut sim, Press::Short(Key::Page)); // WHEEL, BIKE, then ALTITUDE
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // edit
    for _ in 0..24 {
        app.press(&mut sim, Press::Short(Key::Lap)); // up, 5 m a press
    }
    assert_eq!(row(&app, 2), "ALTITUDE < 120 >");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(row(&app, 2), "ALTITUDE 120 M");
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Typed));
    assert_ne!(app.alt_now().1, 0, "the barometer, though the receiver says 300 m");
    assert!(app.alt_now().0.unwrap().abs_diff(120_000) < 1500);

    // FROM GPS: the average runs, the row shows it
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let v = row(&app, 3);
    assert!(v.starts_with("FROM GPS ") && v.contains('/'), "{v}");
    run(&mut sim, &mut app, 30_000);
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Gps));
    assert!(app.alt.alt_mm().unwrap().abs_diff(300_000) < 1000);
    assert_eq!(row(&app, 3), "FROM GPS ");
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
    assert_eq!(app.menu_row(Menu::Bike, 2).unwrap().1.as_str(), "NONE");

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

/// The receiver reads 40 m above the truth (the barometer, once calibrated by a typed altitude, does not).
fn gps_high(ms: u64) -> i32 {
    truth(ms) + 40_000
}

/// The biggest step between two seconds of the ride's altitude (the value the records take), over `secs`.
fn biggest_step(sim: &mut Sim, app: &mut App, secs: u32) -> u32 {
    let mut last = app.alt_now().0;
    let mut worst = 0;
    for _ in 0..secs {
        run(sim, app, 1000);
        let now = app.alt_now().0;
        if let (Some(a), Some(b)) = (last, now) {
            worst = worst.max(a.abs_diff(b));
        }
        last = now;
    }
    worst
}

/// Review: a ride started before the fix took the barometer on the standard day, then the receiver (40 m apart),
/// then the barometer again once calibrated: the records stepped each time. The source change continues from the
/// last value now; the climb is 0.5 m/s.
#[test]
fn a_ride_started_before_a_fix_has_no_step() {
    let mut sim = unit(0);
    sim.gnss.fix = None; // the fix comes 20 s into the ride
    sim.gnss.alt = Some(gps_high);
    sim.bus.baro_alt = Some(truth);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 6000);
    app.start_pause();
    let mut worst = biggest_step(&mut sim, &mut app, 20);
    sim.gnss.fix = Some(5000);
    worst = worst.max(biggest_step(&mut sim, &mut app, 120));
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Auto), "the average ran");
    assert!(worst < 3000, "a step of {worst} mm");
}

/// A resumed ride: the calibration kept on the card comes back (a read, during the ride), so the altitude stays on
/// the barometer instead of going to the receiver, 40 m apart.
#[test]
fn a_resumed_ride_has_no_step() {
    let mut sim = unit(0);
    sim.gnss.alt = Some(gps_high);
    sim.bus.baro_alt = Some(truth);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    let now = app.now_s();
    assert!(app.alt.calibrate(truth(sim.ms), now));
    run(&mut sim, &mut app, 5000);
    assert_eq!(stored(&sim).map(|c| c.by), Some(By::Typed), "written between rides");
    app.start_pause();
    let before = biggest_step(&mut sim, &mut app, 40);
    assert!(before < 3000);
    let last = app.alt_now().0.unwrap();
    app.press(&mut sim, Press::Long(Key::Back));
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(sim.standby);
    let mut sim2 = unit(2);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    sim2.ms = sim.ms + 90_000;
    sim2.gnss.alt = Some(gps_high);
    sim2.bus.baro_alt = Some(truth);
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, u64::from(ALT_LOAD_MS) + 6000);
    assert!(app.notices[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notices[0].as_str());
    app.start_pause();
    let a = app.alt_now().0.unwrap();
    let climbed = 90_000 / 2 + 2000; // mm
    assert!(a.abs_diff(last) < climbed + 10_000, "{last} then {a}");
    assert!(biggest_step(&mut sim2, &mut app, 60) < 3000);
}

/// Review: Back out of the ALTITUDE edit calibrated and trusted the number; only Select (or a tap) on a changed value does.
#[test]
fn back_leaves_the_altitude_as_it_was() {
    let mut sim = unit(0);
    sim.gnss.alt = Some(flat_300);
    sim.bus.baro_alt = Some(truth);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page)); // WHEEL, BIKE, then ALTITUDE
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // edit
    for _ in 0..5 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.alt.cal(), None, "Back: not calibrated");
    app.press(&mut sim, Press::Short(Key::Start)); // edit again
    app.press(&mut sim, Press::Short(Key::Start)); // Select with nothing changed
    assert_eq!(app.alt.cal(), None, "unchanged: not calibrated");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Typed), "Select on a changed value");
}
