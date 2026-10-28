//! AUTO OFF: the unit switches itself off after its time-out of quiet, and only then.

use super::tests::run;
use super::*;

fn fresh() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[18] = 0; // unset: the default
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    (sim, app)
}

#[test]
fn idle_five_minutes_switches_off_with_a_warning() {
    let (mut sim, mut app) = fresh();
    assert_eq!(app.settings.auto_off, 1, "5 min by default");
    run(&mut sim, &mut app, 285_000);
    assert!(!sim.standby);
    run(&mut sim, &mut app, 6000);
    assert!(app.notes.ms.is_some() && app.notes.text.as_str() == "SWITCHING OFF", "warned 10 s before");
    run(&mut sim, &mut app, 20_000);
    assert!(sim.standby && !sim.display_on);
}

/// A Page key held for 120 ms from `at` ms (the sim's clock), read by the main loop as on the unit.
fn page_at(sim: &mut Sim, at: u64) {
    sim.keys = Box::new(move |t| if (at..at + 120).contains(&t) { Key::Page as u8 } else { 0 });
}

#[test]
fn a_key_or_a_cable_restarts_the_count() {
    let (mut sim, mut app) = fresh();
    page_at(&mut sim, 299_000);
    run(&mut sim, &mut app, 299_500);
    run(&mut sim, &mut app, 290_000);
    assert!(!sim.standby, "a key at 4:59 restarted it");
    sim.charger = true;
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "cable in: never");
    sim.charger = false;
    run(&mut sim, &mut app, 290_000);
    assert!(!sim.standby, "the count started again at the unplug");
    run(&mut sim, &mut app, 20_000);
    assert!(sim.standby);
}

#[test]
fn the_warning_lights_the_screen_and_a_key_cancels() {
    let (mut sim, mut app) = fresh();
    app.settings.light_timeout = 1; // 10 s
    run(&mut sim, &mut app, 296_000);
    assert!(app.notes.text.as_str() == "SWITCHING OFF" && app.light.shown > 0, "warned, light on");
    let at = sim.ms + 100;
    page_at(&mut sim, at);
    run(&mut sim, &mut app, 250_000);
    assert!(!sim.standby, "a key cancelled it");
}

/// Movement of any kind keeps it on, and it goes 60 s after the last, then 5 min of quiet.
#[test]
fn movement_keeps_it_on() {
    use crate::sensors::Metric;
    for which in 0..4 {
        let (mut sim, mut app) = fresh();
        for _ in 0..400 {
            match which {
                0 => app.live.set(Metric::Speed, 250), // riding without Start: GPS or sensor speed
                1 => app.live.set(Metric::Power, 150),
                2 => app.live.set(Metric::Cadence, 80),
                _ => app.radio.heard_ms[crate::sensors::Kind::Trainer as usize] = 0, // a trainer heard
            }
            run(&mut sim, &mut app, 1000);
        }
        assert!(!sim.standby, "moving {which}: never");
        run(&mut sim, &mut app, 300_000);
        assert!(!sim.standby, "60 s of grace after the last movement");
        run(&mut sim, &mut app, 80_000);
        assert!(sim.standby, "still for 5 min: off");
    }
}

/// Pairing, the altitude calibration and the pages wait to be written: never off meanwhile.
#[test]
fn files_waiting_keep_it_on() {
    let (mut sim, mut app) = fresh();
    app.card_writable = false;
    app.sensors_store.dirty = true;
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "pairing pending");
    app.sensors_store.dirty = false;
    run(&mut sim, &mut app, 310_000);
    assert!(sim.standby);
}

#[test]
fn off_a_ride_or_a_pending_write_never() {
    let (mut sim, mut app) = fresh();
    app.settings.auto_off = 0;
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "setting OFF");
    app.settings.auto_off = 1;
    (app.store.dirty, app.card_writable) = (true, false);
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "pages.bin waits to be written");
    app.store.dirty = false;
    app.press(&mut sim, Press::Short(Key::Start));
    assert_ne!(app.ride.state(), ride::State::Idle);
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "running");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "paused");
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "end-ride screen");
}

#[test]
fn the_setting_round_trips_in_register_18() {
    let (mut sim, mut app) = fresh();
    for i in 0..AUTO_OFF_MIN.len() {
        app.settings.auto_off = i;
        app.save_misc(&mut sim);
        let back = Settings::from_word(0).with_auto_off_word(sim.backup[AID_BKP[1]]);
        assert_eq!(back.auto_off, i);
        let aid = gnss::Aid::from_words(AID_BKP.map(|r| sim.backup[r]));
        assert_eq!(aid, app.gps.aid, "the aiding reads as before");
    }
    assert_eq!(Settings::from_word(0).with_auto_off_word(0).auto_off, 1, "an older word: 5 min");
}

/// The recorder's holds, each alone: never off, and off within 310 s once cleared.
#[test]
fn the_recorders_holds_keep_it_on() {
    for which in 0..2 {
        let (mut sim, mut app) = fresh();
        let hold = |_: &mut Sim, app: &mut App, on: bool| match which {
            0 => app.rec.resume_pending = on,
            _ => app.rec.full_pending = on,
        };
        for _ in 0..400 {
            hold(&mut sim, &mut app, true); // the recorder clears its own flags: held each second
            run(&mut sim, &mut app, 1000);
        }
        assert!(!sim.standby, "hold {which}: never");
        hold(&mut sim, &mut app, false);
        run(&mut sim, &mut app, 310_000);
        assert!(sim.standby, "hold {which} cleared: off");
    }
}
