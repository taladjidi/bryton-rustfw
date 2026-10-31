//! BATTERY SAVER (SYSTEM): the knobs it applies, that OFF gives the settings back, AUTO's threshold and the recording
//! left alone.

use super::tests::run;
use super::*;

fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.settings.light_level = 0; // 100 %
    app.settings.light_timeout = 0; // never off
    app.settings.gps_power = GpsPower::Full;
    run(&mut sim, &mut app, 3000);
    (sim, app)
}

#[test]
fn on_applies_each_knob_and_off_restores_the_settings() {
    let (mut sim, mut app) = unit();
    let before = app.settings;
    assert_eq!((sim.light_level, sim.gnss.pm_mode, app.strip.saver), (100, 0, false));
    app.settings.saver = SaverMode::On;
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.light_level, 30, "brightness capped");
    assert_eq!(sim.gnss.pm_mode, 2, "cyclic tracking");
    assert!(app.strip.saver, "the tag");
    run(&mut sim, &mut app, 12_000);
    assert!(!sim.backlight, "never off became 10 s");
    app.settings.saver = SaverMode::Off;
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_eq!((sim.light_level, sim.gnss.pm_mode, app.strip.saver), (100, 0, false));
    run(&mut sim, &mut app, 12_000);
    assert!(sim.backlight, "never off again");
    assert_eq!(app.settings, before);
}

#[test]
fn auto_turns_on_under_the_threshold_once_and_off_when_charging() {
    let (mut sim, mut app) = unit();
    app.settings.saver = SaverMode::Auto;
    run(&mut sim, &mut app, 15_000);
    assert!(!app.saver_on(), "{:?} %", app.strip.battery);
    sim.battery_mv = Some(3715);
    run(&mut sim, &mut app, 150_000); // the reading is smoothed
    assert!(app.strip.battery.is_some_and(|p| p <= BATTERY_SAVER_PERCENT), "{:?} %", app.strip.battery);
    assert!(app.saver_on());
    assert!(app.notes.list[0].as_str().ends_with("BATTERY SAVER ON"), "{}", app.notes.list[0].as_str());
    let told = app.notes.list.iter().filter(|n| n.as_str().ends_with("BATTERY SAVER ON")).count();
    run(&mut sim, &mut app, 60_000);
    let again = app.notes.list.iter().filter(|n| n.as_str().ends_with("BATTERY SAVER ON")).count();
    assert_eq!((told, again), (1, 1), "said once");
    assert_eq!(sim.light_level, 0, "the 10 s time-out");
    sim.charger = true;
    run(&mut sim, &mut app, 15_000);
    assert!(!app.saver_on());
    assert!(app.notes.list[0].as_str().ends_with("BATTERY SAVER OFF"), "{}", app.notes.list[0].as_str());
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.light_level, 100);
}

/// The ride is recorded the same with the saver on: the same timer and the same card commands.
#[test]
fn recording_is_unchanged_while_on() {
    let ride = |saver| {
        let (mut sim, mut app) = unit();
        app.settings.saver = saver;
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 150_000);
        let writes = sim.emmc.cmds.iter().filter(|c| c.0 == 24 || c.0 == 25).count();
        (app.ride.timer_s(), writes, sim.emmc.sectors_written)
    };
    let (off, on) = (ride(SaverMode::Off), ride(SaverMode::On));
    assert!(off.0 >= 140 && off.1 > 0, "{off:?}");
    assert_eq!(off, on);
}

/// The strip tag, drawn in the simulator's screenshot.
#[test]
fn the_strip_tag_is_drawn() {
    let (mut sim, mut app) = unit();
    app.settings.saver = SaverMode::On;
    run(&mut sim, &mut app, 3000);
    sim.save("saver_strip");
}

/// A battery voltage the charge estimate calls `p` percent.
fn mv_for(p: u8) -> u16 {
    (3000..4400).find(|&mv| crate::battery::percent(mv) == p).unwrap()
}

/// Under a running ride the saver leaves the receiver's mode and the rider's time-out alone (the distance comes from
/// the GPS speed; a rider's NEVER stays) and caps the level; paused, the receiver goes cyclic.
#[test]
fn a_running_ride_keeps_its_gps_mode_and_timeout() {
    let (mut sim, mut app) = unit();
    app.settings.saver = SaverMode::On;
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(sim.gnss.pm_mode, 0, "continuous under a running ride");
    assert!(sim.backlight && sim.light_level == 30, "never off stays, level capped");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ride.state(), ride::State::Paused);
    assert_eq!(sim.gnss.pm_mode, 2, "paused: cyclic");
}

#[test]
fn auto_chosen_while_low_says_so_and_the_threshold_edges_hold() {
    let (mut sim, mut app) = unit();
    sim.battery_mv = Some(mv_for(19));
    run(&mut sim, &mut app, 150_000);
    assert!(!app.saver_on());
    app.settings.saver = SaverMode::Auto;
    run(&mut sim, &mut app, 15_000);
    assert!(app.notes.list[0].as_str().ends_with("BATTERY SAVER ON"), "{}", app.notes.list[0].as_str());
    // a rise to 22 % stays on: only the charger ends it
    sim.battery_mv = Some(mv_for(22));
    run(&mut sim, &mut app, 150_000);
    assert!(app.saver_on(), "{:?} %", app.strip.battery);
    // the charger at 21 %, then removed: off, and not on again above 20 %
    sim.charger = true;
    sim.battery_mv = Some(mv_for(21));
    run(&mut sim, &mut app, 150_000);
    assert!(!app.saver_on());
    sim.charger = false;
    run(&mut sim, &mut app, 150_000);
    assert!(!app.saver_on(), "{:?} %", app.strip.battery);
    // exactly 20 % is on
    sim.battery_mv = Some(mv_for(20));
    run(&mut sim, &mut app, 150_000);
    assert!(app.saver_on(), "{:?} %", app.strip.battery);
}

/// The map keeps its rate: the panel gets as many commands over 20 s of a map page with the saver on as without.
#[test]
fn the_map_redraws_as_often_on() {
    let commands = |saver| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.gnss.route = Some(super::tests::tiles_route);
        let mut app = App::start(&mut sim, None);
        app.settings.saver = saver;
        app.pages[0] =
            PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] }
                .into();
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 10_000);
        let n = sim.cmds.len();
        run(&mut sim, &mut app, 20_000);
        sim.cmds.len() - n
    };
    let (off, on) = (commands(SaverMode::Off), commands(SaverMode::On));
    assert!(off > 20, "{off}");
    assert_eq!(off, on);
}

/// The tag with the USB symbol and 100 %: a pixel clear of the battery text.
#[test]
fn the_tag_clears_usb_and_a_full_battery() {
    let (mut sim, mut app) = unit();
    sim.charger = true;
    sim.battery_mv = Some(4250);
    app.settings.saver = SaverMode::On;
    run(&mut sim, &mut app, 12_000);
    sim.save("saver_strip_usb");
}
