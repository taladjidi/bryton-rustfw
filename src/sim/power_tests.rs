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

#[test]
fn a_key_or_a_cable_restarts_the_count() {
    let (mut sim, mut app) = fresh();
    run(&mut sim, &mut app, 299_000);
    app.press(&mut sim, Press::Short(Key::Page));
    app.light.idle_ms = 0; // the main loop's key poll does this
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
