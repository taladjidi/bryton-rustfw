//! Power meter calibration on the Sim (powercal.rs, app/powercal.rs): the zero offset asked from SENSORS, the meter
//! answering success, failure or nothing, the request sent again when the air loses it, the auto zero switch, the
//! refusal while riding. The screens are saved as `target/sim/powercal_*.bmp`.

use super::*;
use crate::menu::{RideRow, Row};
use crate::powercal::{AutoZero, Phase};

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const ID: SensorId = SensorId { device: 7, trans: 5 };

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The unit with a power meter in range, paired (register 10) and heard, the answer it gives set.
fn unit(answer: MeterAnswer) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(ID);
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    sim.nordic.meter.answer = answer;
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    assert!(app.power_heard(), "the meter is paired and heard");
    (sim, app)
}

/// SENSORS, the CALIBRATE POWER row, Start: the screen is up.
fn open(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Sensors));
    app.press(sim, Press::Short(Key::Start));
    super::tests::to_row(sim, app, Row::Ride(RideRow::CalPower));
    app.press(sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::PowerCal);
    run(sim, app, 100);
}

fn requests(sim: &Sim) -> Vec<[u8; 8]> {
    sim.nordic.meter.got.clone()
}

/// Success: the request goes out as an acknowledged message, the answer shows the offset, a second zero shows the
/// one before it; the power on the fields is not disturbed by the calibration pages.
#[test]
fn zero_offset_succeeds() {
    let (mut sim, mut app) = unit(MeterAnswer::Zero(300));
    sim.nordic.meter.auto = Some(0);
    open(&mut sim, &mut app);
    sim.save("powercal_ready");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 100);
    sim.save("powercal_asking");
    run(&mut sim, &mut app, 1500);
    assert_eq!(requests(&sim), vec![crate::powercal::ZERO_REQUEST], "one request, taken");
    assert_eq!(app.powercal.phase(), Phase::Done(300, None));
    assert_eq!(app.powercal.auto(), Some(AutoZero::Off));
    assert_eq!(app.live.get(Metric::Power), Some(210), "power is still read between the calibration pages");
    sim.save("powercal_done");
    // again: the offset before it
    sim.nordic.meter.answer = MeterAnswer::Zero(-12);
    run(&mut sim, &mut app, crate::powercal::SHOW_MS as u64);
    assert_eq!(app.powercal.phase(), Phase::Ready, "the result goes after a while");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.powercal.phase(), Phase::Done(-12, Some(300)));
    sim.save("powercal_done_again");
    // AUTO ZERO: Page switches it, the meter's confirmed state is what shows
    run(&mut sim, &mut app, crate::powercal::SHOW_MS as u64);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1500);
    assert_eq!(sim.nordic.meter.got.last(), Some(&[1, 0xab, 1, 0xff, 0xff, 0xff, 0xff, 0xff]));
    assert_eq!((app.powercal.phase(), app.powercal.auto()), (Phase::AutoSet, Some(AutoZero::On)));
    sim.save("powercal_auto_on");
    run(&mut sim, &mut app, crate::powercal::SHOW_MS as u64);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.powercal.auto(), Some(AutoZero::Off));
    super::tests::assert_consistent(&sim, &app);
    // Back leaves
    app.press(&mut sim, Press::Short(Key::Back));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Sensors, _)));
}

/// The meter says it failed.
#[test]
fn zero_offset_fails() {
    let (mut sim, mut app) = unit(MeterAnswer::Fail);
    open(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.powercal.phase(), Phase::Failed);
    assert_eq!(app.powercal.auto(), Some(AutoZero::Unsupported), "no auto zero: the switch is not offered");
    sim.save("powercal_failed");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 500);
    assert_eq!(requests(&sim).len(), 1, "no auto zero request when it is not supported");
}

/// The meter never answers: NO ANSWER after 10 s; Start again asks again, Start while asking cancels.
#[test]
fn zero_offset_unanswered() {
    let (mut sim, mut app) = unit(MeterAnswer::Silent);
    open(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert!(app.powercal.asking());
    run(&mut sim, &mut app, 5500);
    assert_eq!(app.powercal.phase(), Phase::NoAnswer);
    sim.save("powercal_no_answer");
    assert_eq!(requests(&sim).len(), 1, "taken on air once: the silence is the meter's");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 500);
    assert!(app.powercal.asking());
    app.press(&mut sim, Press::Short(Key::Start)); // CANCEL
    assert_eq!(app.powercal.phase(), Phase::Ready);
}

/// The air loses the first two messages (TX_FAILED): the request is sent again, and the answer comes.
#[test]
fn request_sent_again() {
    let (mut sim, mut app) = unit(MeterAnswer::Zero(5));
    sim.nordic.ack_fail = 2;
    open(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.powercal.phase(), Phase::Done(5, None));
}

/// Not while a ride runs: the screen does not open and says so; paused it does.
#[test]
fn refused_while_riding() {
    let (mut sim, mut app) = unit(MeterAnswer::Zero(5));
    app.settings.ready_check = false; // these tests are about what the start does, not the card
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    app.open_powercal();
    assert_ne!(app.screen(), Screen::PowerCal);
    assert_eq!(app.notes.text.as_str(), "NO CALIBRATION RIDING");
    assert!(requests(&sim).is_empty());
    app.press(&mut sim, Press::Short(Key::Start)); // paused
    app.open_powercal();
    assert_eq!(app.screen(), Screen::PowerCal, "paused: allowed");
}

/// The row is offered only while the meter is paired and heard.
#[test]
fn row_needs_a_meter() {
    let (mut sim, mut app) = unit(MeterAnswer::Silent);
    let has = |app: &App| (0..20).any(|i| app.row(Menu::Sensors, i) == Some(Row::Ride(RideRow::CalPower)));
    assert!(has(&app));
    sim.nordic.sensors.clear();
    run(&mut sim, &mut app, 8000);
    assert!(!has(&app), "silent: no row");
}
