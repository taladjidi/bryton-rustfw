//! The start reminder and the ready card (app/start_cues.rs): nothing here starts or stops a ride by itself.

use super::page_tests::started;
use super::tests::run;
use super::*;

/// The bike moves for `s` seconds: power on, refreshed each second.
fn pedal(sim: &mut Sim, app: &mut App, s: u32) {
    for _ in 0..s {
        app.live.set(Metric::Power, 150);
        run(sim, app, 1000);
    }
}

fn banner(app: &App) -> Option<std::string::String> {
    app.notes.ms.map(|_| std::string::String::from(app.notes.text.as_str()))
}

/// Moving for 10 s with no ride: the banner and a beep, once per 5 minutes; Start then starts the ride.
#[test]
fn moving_without_a_ride_asks_to_start_once_per_five_minutes() {
    let (mut sim, mut app) = started();
    let t0 = sim.tones.len();
    pedal(&mut sim, &mut app, 9);
    assert_eq!(banner(&app), None, "not yet");
    pedal(&mut sim, &mut app, 2);
    assert_eq!(banner(&app).as_deref(), Some("START THE RIDE?"));
    assert!(sim.tones.len() > t0, "a beep");
    sim.save("start_reminder");
    pedal(&mut sim, &mut app, 280);
    let n = sim.tones.len();
    assert_eq!(app.ride.state(), ride::State::Idle, "never started by itself");
    pedal(&mut sim, &mut app, 21);
    assert!(sim.tones.len() > n, "again after 5 minutes");
    assert_eq!(banner(&app).as_deref(), Some("START THE RIDE?"));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running, "Start starts it as always");
}

/// Not while riding, paused, on USB power, in a menu, when switched off; SOUND off: the banner, no beep.
#[test]
fn no_reminder_while_riding_paused_charging_in_a_menu_or_off() {
    let (mut sim, mut app) = started();
    sim.charger = true;
    pedal(&mut sim, &mut app, 20);
    assert_eq!(banner(&app), None, "cable in");
    sim.charger = false;
    app.press(&mut sim, Press::Long(Key::Page));
    pedal(&mut sim, &mut app, 20);
    assert_eq!(banner(&app), None, "a menu is open");
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.settings.start_reminder = false;
    pedal(&mut sim, &mut app, 20);
    assert_eq!(banner(&app), None, "switched off");
    app.settings.start_reminder = true;
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    pedal(&mut sim, &mut app, 20);
    assert_eq!(banner(&app), None, "riding");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.state(), ride::State::Paused);
    pedal(&mut sim, &mut app, 20);
    assert_eq!(banner(&app), None, "paused");
    let (mut sim, mut app) = started();
    app.settings.sound = false;
    let quiet = sim.tones.len();
    pedal(&mut sim, &mut app, 12);
    assert_eq!(banner(&app).as_deref(), Some("START THE RIDE?"));
    assert!(sim.tones[quiet..].iter().all(|&hz| hz == 0), "SOUND off: silent");
}

/// The ready card: GPS, battery, the missing sensor and a low battery; the ride runs from the press; a key skips it,
/// Start on it does not pause; READY CHECK off: RIDE STARTED as before.
#[test]
fn the_ready_card_lists_gps_sensors_and_battery_and_never_delays_the_ride() {
    let (mut sim, mut app) = started();
    app.radio.sensors[Kind::Hr as usize] = Sensor::Paired(SensorId { device: 7, trans: 5 });
    app.radio.sensors[Kind::Power as usize] = Sensor::Paired(SensorId { device: 8, trans: 11 });
    app.radio.heard_ms[Kind::Hr as usize] = 100;
    app.strip.battery = Some(12);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running, "recording from the press");
    assert_eq!(app.notes.kind, Banner::Ready);
    assert_eq!(app.notes.text.as_str(), "GPS NO  BAT 12%\nLOW BATT NO POWER");
    run(&mut sim, &mut app, 500);
    sim.save("ready_card");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.ms, None, "skipped");
    assert_eq!(app.ride.state(), ride::State::Running, "Start on the card does not pause");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Paused, "the next one does");
    // all well: the sensors found, GPS, a full battery; the card goes by itself after 3 s
    let (mut sim, mut app) = started();
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 3000);
    app.radio.sensors[Kind::Hr as usize] = Sensor::Paired(SensorId { device: 7, trans: 5 });
    app.radio.heard_ms[Kind::Hr as usize] = 100;
    app.strip.battery = Some(80);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "GPS YES  BAT 80%\nHR OK");
    run(&mut sim, &mut app, READY_BANNER_MS as u64 + 500);
    assert_eq!(app.notes.ms, None);
    // off: the usual banner
    let (mut sim, mut app) = started();
    app.settings.ready_check = false;
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "RIDE STARTED");
}
