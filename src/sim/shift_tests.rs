//! Electronic shifting on the Sim (sensors.rs, app/radio.rs): a Shimano Di2 on its private network and an ANT+
//! Shifting system, paired from the sensors menu, their gear and battery on the fields, the shifts counted while
//! riding, the Di2 hood buttons doing what the DI2 KEYS menu set, and a system that goes silent.

use super::*;
use crate::menu::Remote;
use crate::ui::Field;

/// Stand-ins for the network keys (the real ones are never in the repository).
const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const DI2_KEY: [u8; 8] = [8, 7, 6, 5, 4, 3, 2, 1];

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

/// Runs until simulated time `ms`.
fn run_to(sim: &mut Sim, app: &mut App, ms: u64) {
    let left = ms.saturating_sub(sim.ms);
    run(sim, app, left);
}

/// A Di2 (stock's layout, sensors.rs): page 0 with front 2, rear 11 then 10 from 20 s, battery 60 %, and every
/// fourth message the button page, acknowledged: the right button's byte 0x11 (a single press) from 8 s, the
/// same byte repeated after (one press only), then 0x22 (a long press) from 30 s.
fn di2(ms: u64) -> [u8; 8] {
    if ms / 250 % 4 == 3 {
        let right = match ms {
            ..8_000 => 0,
            ..30_000 => 0x11,
            _ => 0x22,
        };
        return [4, right, 0, 0, 0, 0, 0, 0];
    }
    [0, 0, 2, if ms < 20_000 { 11 } else { 10 }, 60, 0, 0, 0]
}

/// An ANT+ Shifting system: page 1 with front 2 of 2, rear 11 of 12 (sent from 0), and the common battery page
/// with one component low.
fn axs(ms: u64) -> [u8; 8] {
    if ms / 250 % 4 == 3 {
        return [0x52, 0xff, 0x21, 0, 0, 0, 0, 4 << 4];
    }
    [1, 7, 0xff, 1 << 5 | 10, 2 << 5 | 12, 0, 0, 0]
}

fn sensor_row(app: &App, i: usize) -> std::string::String {
    let (label, value, _) = app.menu_row(Menu::Sensors, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// The sensors menu's row `row`, then Start.
fn select_sensor(sim: &mut Sim, app: &mut App, row: usize) {
    app.press(sim, Press::Long(Key::Page));
    app.press(sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
    for _ in 0..row {
        app.press(sim, Press::Short(Key::Page));
    }
    app.press(sim, Press::Short(Key::Start));
}

/// Pairing the Di2: Shimano's key on network 1 and channel 6 assigned to it at start; Start on its row finds it,
/// keeps it in register 14; its gear and battery reach the fields; the shifts are counted only while riding.
#[test]
fn di2_pairs_and_shows_gear_and_battery() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.nordic.sensors.push(AntSensor { device_type: 1, id: 4321, data: di2, network: 1, ack: &[4] });
    let mut app = App::start_with(&mut sim, Some(KEY), Some(DI2_KEY));
    assert!(sim.nordic.got.contains(&[&[6, 0xcc, 1][..], &DI2_KEY].concat()), "Shimano's key on network 1");
    assert!(sim.nordic.got.contains(&vec![6, 0xc2, 6, 0, 1, 0]), "channel 6 on network 1");
    assert_eq!(sensor_row(&app, 6), "DI2 OFF");
    select_sensor(&mut sim, &mut app, Kind::Di2 as usize);
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 6, 0, 0, 1, 0]), "any Di2");
    run(&mut sim, &mut app, 3000);
    let id = SensorId { device: 4321, trans: 0xf1 };
    assert_eq!(sim.backup[14], Kind::Di2.to_word(id), "paired and kept");
    assert_eq!(sensor_row(&app, 6), "DI2 4321");
    assert_eq!(app.field_text(Field::Gear).as_str(), "2-11");
    assert_eq!(app.field_text(Field::ShiftBattery).as_str(), "60%");
    assert_eq!(app.field_color(Field::ShiftBattery), crate::ui::WHITE);
    // a ride: the shift at 20 s counted
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.field_text(Field::Shifts).as_str(), "0");
    run_to(&mut sim, &mut app, 21_000);
    assert_eq!(app.field_text(Field::Gear).as_str(), "2-10");
    assert_eq!(app.field_text(Field::Shifts).as_str(), "1");
    // the next start pairs it again from the register
    let mut sim2 = Sim::new(9, 0, 0);
    sim2.own_settings();
    sim2.backup[14] = sim.backup[14];
    let _app = App::start_with(&mut sim2, Some(KEY), Some(DI2_KEY));
    assert!(sim2.nordic.got.contains(&vec![6, 0xd3, 6, 0xe1, 0x10, 1, 0xf1]), "opened for that Di2");
}

/// Without Shimano's key the Di2 row says so and Start does nothing; the ANT+ channels are as before.
#[test]
fn di2_without_its_key() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(!sim.nordic.got.iter().any(|p| p.starts_with(&[6, 0xcc, 1])), "no network 1");
    assert_eq!(sensor_row(&app, 6), "DI2 NO KEY");
    select_sensor(&mut sim, &mut app, Kind::Di2 as usize);
    assert!(!sim.nordic.got.iter().any(|p| p.starts_with(&[6, 0xc4, 6])), "channel 6 never opened");
    assert_eq!(sim.backup[14], 0);
}

/// The hood buttons: R PRESS set to NEXT PAGE in the DI2 KEYS menu, R HOLD to LAP; one press (its byte repeated
/// by the Di2) turns one page, the long press marks a lap while riding.
#[test]
fn di2_buttons_do_their_actions() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.classic_pages();
    sim.backup[14] = Kind::Di2.to_word(SensorId { device: 4321, trans: 0xf1 });
    sim.nordic.sensors.push(AntSensor { device_type: 1, id: 4321, data: di2, network: 1, ack: &[4] });
    let mut app = App::start_with(&mut sim, Some(KEY), Some(DI2_KEY));
    // sensors, the DI2 KEYS row (after the kinds), R PRESS: NONE to NEXT PAGE; R HOLD: to LAP
    select_sensor(&mut sim, &mut app, Kind::ALL.len());
    assert_eq!(app.screen(), Screen::Menu(Menu::Remote, 0));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..2 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Start));
    }
    let row = |app: &App, i| app.menu_row(Menu::Remote, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str()));
    assert_eq!(row(&app, 1).unwrap(), "R PRESS NEXT PAGE");
    assert_eq!(row(&app, 3).unwrap(), "R HOLD LAP");
    let saved = Settings::from_word(0).with_power_word(sim.backup[POWER_BKP]).with_alerts_word(sim.backup[ALERTS_BKP]);
    assert_eq!(saved.remote, [Remote::None, Remote::NextPage, Remote::None, Remote::Lap], "kept");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    assert_eq!(app.screen(), Screen::Main);
    run_to(&mut sim, &mut app, 7_000);
    assert_eq!(app.screen(), Screen::Main, "no press yet");
    run(&mut sim, &mut app, 4_000);
    assert_eq!(app.screen(), Screen::Ride(0), "one press, one page, though its byte came several times");
    app.press(&mut sim, Press::Short(Key::Start)); // ride
    // the banner says LAP once, after 30 s (other notices may follow it: this Sim has no card to record on)
    let mut said = std::vec::Vec::new();
    while sim.ms < 31_500 {
        run(&mut sim, &mut app, POLL_MS.into());
        if app.banner.as_str() == "LAP" && said.last() != Some(&(sim.ms >= 30_000)) {
            said.push(sim.ms >= 30_000);
        }
    }
    assert_eq!(said, [true], "a lap at the long press");
    assert_eq!(app.screen(), Screen::Ride(0), "and no page turned");
}

/// An ANT+ Shifting system: paired from its row, its gear (sent from 0) and its battery status shown, red when
/// low.
#[test]
fn ant_plus_shifting() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.nordic.sensors.push(AntSensor { device_type: 0x22, id: 77, data: axs, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    select_sensor(&mut sim, &mut app, Kind::Shifting as usize);
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.backup[14], Kind::Shifting.to_word(SensorId { device: 77, trans: 0xf1 }));
    assert_eq!(app.field_text(Field::Gear).as_str(), "2-11");
    assert_eq!(app.field_text(Field::ShiftBattery).as_str(), "LOW");
    assert_eq!(app.field_color(Field::ShiftBattery), crate::ui::RED);
    // forgetting it clears its pairing; forgetting the Di2 (which it is not) leaves the register alone
    sim.backup[14] = Kind::Shifting.to_word(SensorId { device: 77, trans: 0xf1 });
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(sim.backup[14], 0, "forgotten");
}

/// A shifting system that goes silent: its gear and battery go to "--" once stale, as the other sensors' values.
#[test]
fn silent_shifting_system() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.backup[14] = Kind::Di2.to_word(SensorId { device: 4321, trans: 0xf1 });
    sim.nordic.sensors.push(AntSensor { device_type: 1, id: 4321, data: di2, network: 1, ack: &[4] });
    let mut app = App::start_with(&mut sim, Some(KEY), Some(DI2_KEY));
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.field_text(Field::Gear).as_str(), "2-11");
    sim.nordic.sensors.clear();
    run(&mut sim, &mut app, u64::from(sensors::STALE_MS) + 1000);
    assert_eq!(app.field_text(Field::Gear).as_str(), "--");
    assert_eq!(app.field_text(Field::ShiftBattery).as_str(), "--");
}
