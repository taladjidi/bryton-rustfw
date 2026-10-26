//! The Bluetooth trainer on the Sim (drivers/ftms.rs, app/ftms.rs) against the simulated chip's central side and an
//! FTMS trainer (ble_sim.rs `FtmsSim`), next to the scripted phone: pairing from SENSORS, the two links at once,
//! ERG from a workout recorded in the ride, SIM on a course, a lost link and a lost control taken back, a forgotten
//! subscription, split records, and a chip that refuses the two links' configuration (the phone still works).

use super::ble_sim::{FT_CP, FT_DATA, FT_STATUS};
use super::*;
use crate::menu::Row;
use crate::ui::Field;

pub(super) const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The unit with the synthetic volume and `t` in range (and `phone`, if any).
pub(super) fn unit_with(t: FtmsSim, phone: Option<PhoneSim>) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.nordic.ftms = Some(t);
    sim.nordic.phone = phone;
    let app = App::start(&mut sim, Some(KEY));
    (sim, app)
}

pub(super) fn trainer(sim: &Sim) -> &FtmsSim {
    sim.nordic.ftms.as_ref().expect("the trainer")
}

/// SENSORS, BT TRAINER: the search lists the trainer (its name from the scan response); Select pairs it.
fn pair(sim: &mut Sim, app: &mut App) {
    run(sim, app, 4000); // the sensors file read
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Sensors));
    app.press(sim, Press::Short(Key::Start));
    super::tests::to_row(sim, app, Row::BtTrainer);
    app.press(sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::BtTrainers, 0));
    run(sim, app, 1100);
    let (label, value, _) = app.menu_row(Menu::BtTrainers, 0).expect("a trainer heard");
    assert_eq!((label.as_str(), value.as_str()), ("SIM BIKE", "FTMS -56"));
    sim.save("ftms_search");
    app.press(sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, Kind::ALL.len()), "back to SENSORS");
    app.leave_menus();
}

/// The trainer in `\Rustfw\sensors.bin`.
pub(super) fn bt_file(sim: &Sim) -> Option<[u8; 7]> {
    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = crate::fat::tests::read_all(&mut fat, &mut d, "Rustfw/sensors.bin").ok()?;
    crate::sensors::bt_from_sector(&v.try_into().ok()?).map(|b| b.0)
}

/// The phone paired, then the trainer: both links up at once (the trainer on connection 1 as our peripheral, the
/// phone on 0 as our central), the trainer discovered, subscribed (CCCDs before any Control Point write), its
/// Feature and ranges read, the control asked for; its readings on the fields; nothing commanded until taken over;
/// the pairing kept in the sensors file; advertising and the connection on tag 1.
#[test]
fn the_trainer_and_the_phone_at_once() {
    let mut phone = PhoneSim::new(true);
    phone.connect_at = 1000;
    let (mut sim, mut app) = unit_with(FtmsSim::new(), Some(phone));
    assert_eq!(sim.nordic.cfg.len(), 2, "the two links configured");
    assert!(app.ble.is_some_and(|v| v.central));
    app.activate(&mut sim, Menu::Phone, 1); // PAIR
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.phone.link(), Link::Secure);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.phone.link(), Link::Secure, "the phone still linked");
    let t = trainer(&sim);
    assert!(t.connected && t.control, "connected and under control");
    assert!(t.on(FT_DATA, 1) && t.on(FT_STATUS, 1) && t.on(FT_CP, 2), "subscribed");
    assert_eq!(t.got, [vec![0x00], vec![0x04, 0]], "Request Control, the release (resistance 0), no target");
    assert_eq!(t.violations, 0);
    assert_eq!(app.ftms.conn, Some(1));
    assert_eq!(bt_file(&sim), Some(t.peer()), "kept in the sensors file");
    let live = |app: &App, m| app.live.get(m);
    assert_eq!(
        [Metric::Power, Metric::Cadence, Metric::Speed].map(|m| live(&app, m)),
        [Some(150), Some(90), Some(226)]
    );
    assert_eq!(app.trainer.reading.modes, Some(7), "its Feature: resistance, power, simulation");
    let row = app.menu_row(Menu::Sensors, Kind::ALL.len()).unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str()), ("BT TRAINER", "LINKED"));
    // the phone's media keys and the trainer's data together: a notification still reaches the phone's code
    app.media(&mut sim, crate::phone::Media::PlayPause, false);
    run(&mut sim, &mut app, 500);
    assert_eq!(sim.nordic.phone.as_ref().unwrap().commands, [2], "AMS toggle play / pause");
    // the trainer screen, taken over: 105 W
    app.open(Screen::Trainer);
    app.show(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!((trainer(&sim).mode, trainer(&sim).target_w), (5, 105));
    assert_eq!(app.field_text(Field::Power).as_str(), "105");
    sim.save("ftms_trainer_screen");
    super::tests::assert_consistent(&sim, &app);
    assert_eq!(trainer(&sim).violations, 0, "one procedure at a time");
}

/// A workout in ERG over Bluetooth: each step's target written as Set Target Power, the trainer holds it, the ride
/// records its power and its speed's distance.
#[test]
fn erg_workout_over_ble_is_recorded() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // SHORT INTERVALS
    let name = crate::recorder::ride_name(app.rec_time().0);
    app.press(&mut sim, Press::Short(Key::Start)); // START
    run(&mut sim, &mut app, 8000);
    assert_eq!((trainer(&sim).mode, trainer(&sim).target_w), (5, 110), "100-120 W");
    run(&mut sim, &mut app, 14_000);
    assert_eq!(trainer(&sim).target_w, 210, "the second step");
    let writes = trainer(&sim).got.len();
    run(&mut sim, &mut app, 9000);
    assert!(trainer(&sim).got.len() > writes, "sent again while it stands");
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let rec = super::trainer_tests::records(&sim, name);
    let powers: Vec<u16> = rec.iter().filter_map(|r| r.0).collect();
    assert!(powers.contains(&110) && powers.contains(&210), "the targets held: {powers:?}");
    let dist = rec.iter().filter_map(|r| r.1).max().unwrap_or(0);
    assert!(dist > 12_000, "the trainer's speed gives the distance: {dist} cm");
    assert_eq!(trainer(&sim).violations, 0);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A course over Bluetooth: SIM, the course's grade written as the indoor bike simulation parameters.
#[test]
fn sim_follows_the_course_over_ble() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    app.set_course_memory(crate::storage::course_file::tests::memory());
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..super::tests::hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 200_000);
    let t = trainer(&sim);
    assert_eq!(t.mode, 0x11);
    let last = t.got.iter().rev().find(|w| w[0] == 0x11).expect("simulation written");
    assert_eq!((last.len(), last[5], last[6]), (7, 40, crate::ftms::CW), "Crr 0.004, Cw 0.51");
    assert!(t.grade_x100 > 0, "on the hill: {}", t.grade_x100);
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "SIM");
    assert_eq!(t.violations, 0);
}

/// The link drops: the readings stop at once, the unit connects again (after a short wait), discovers, subscribes and
/// takes the control again, and the target in force is written again.
#[test]
fn a_lost_link_is_made_again() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!(trainer(&sim).target_w, 105);
    sim.nordic.ftms.as_mut().unwrap().drop_at = Some(sim.ms);
    sim.nordic.ftms.as_mut().unwrap().target_w = 0; // a trainer that forgot it with the link
    run(&mut sim, &mut app, 300);
    assert_eq!(app.ftms.conn, None);
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "--", "not heard any more");
    run(&mut sim, &mut app, 5000);
    let t = trainer(&sim);
    assert!(t.connected && t.control, "back");
    assert_eq!(sim.nordic.connects, 2);
    assert_eq!(t.target_w, 105, "the target in force written again");
    // out of range: tried again with a wait that grows, the attempts cancelled
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.drop_at, t.connectable) = (Some(sim.ms), false);
    run(&mut sim, &mut app, 120_000);
    let tries = sim.nordic.connects - 2;
    assert!((3..=8).contains(&tries), "{tries} attempts in 2 min");
    assert!(sim.nordic.got.iter().any(|p| p[..2] == [0, 0x8d]), "a connection not made is cancelled");
    assert_eq!(tries, 5, "then left alone");
    sim.nordic.ftms.as_mut().unwrap().connectable = true;
    app.open(Screen::Trainer);
    run(&mut sim, &mut app, 5000);
    assert!(trainer(&sim).control, "back once in range and asked for");
}

/// The trainer takes the control back (Machine Status 0xff): Request Control again, then the target again.
#[test]
fn control_lost_is_asked_again() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.lose_control_at, t.mode, t.target_w) = (Some(sim.ms), 0, 0);
    run(&mut sim, &mut app, 1000);
    let t = trainer(&sim);
    assert_eq!(t.got.iter().filter(|w| w[..] == [0x00]).count(), 2, "{:?}", t.got);
    assert!(t.control);
    assert_eq!((t.mode, t.target_w), (5, 105), "the target again");
    assert_eq!(t.violations, 0);
}

/// A trainer that forgot the Control Point's subscription answers its first write 0xFD: subscribed again, then the
/// control asked again. Records split over two notifications (More Data) give both halves.
#[test]
fn a_forgotten_subscription_and_split_records() {
    let mut t = FtmsSim::new();
    (t.forget_cccd, t.split) = (true, true);
    let (mut sim, mut app) = unit_with(t, None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let t = trainer(&sim);
    assert!(t.control && t.on(FT_CP, 2), "subscribed again and in control");
    assert_eq!(t.got, [vec![0x00], vec![0x04, 0]], "the refused write is not a procedure; then the release");
    assert_eq!(app.live.get(Metric::Cadence), Some(90), "from the first half");
    assert_eq!((app.live.get(Metric::Power), app.live.get(Metric::Speed)), (Some(150), Some(226)), "the second");
}

/// A trainer that refuses the control is read without it, and nothing is written to its Control Point after.
#[test]
fn no_control_reads_on() {
    let mut t = FtmsSim::new();
    t.refuse_control = true;
    let (mut sim, mut app) = unit_with(t, None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_eq!(trainer(&sim).got, [vec![0x00]]);
    assert_eq!(app.live.get(Metric::Power), Some(150));
    let row = app.menu_row(Menu::Sensors, Kind::ALL.len()).unwrap();
    assert_eq!(row.1.as_str(), "NO CONTROL");
}

/// A chip that cannot hold the two links: `sd_ble_enable` NO_MEM after the configuration (or the configuration
/// refused): reset and started on its defaults, the phone pairs and links as before, the trainer row says so and
/// nothing is connected.
#[test]
fn a_chip_without_room_for_two_links_keeps_the_phone() {
    for refuse_cfg in [false, true] {
        let mut phone = PhoneSim::new(true);
        phone.connect_at = 1000;
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        if refuse_cfg {
            sim.nordic.refuse = Some(0x69);
        } else {
            sim.nordic.no_mem = true;
        }
        sim.nordic.phone = Some(phone);
        sim.nordic.ftms = Some(FtmsSim::new());
        let mut app = App::start(&mut sim, Some(KEY));
        assert_eq!(sim.nordic.resets.len(), 2, "started again on the defaults");
        assert!(app.ble.is_some_and(|v| !v.central));
        app.activate(&mut sim, Menu::Phone, 1);
        run(&mut sim, &mut app, 2000);
        assert_eq!(app.phone.link(), Link::Secure, "the phone works");
        assert_eq!(sim.nordic.got.iter().rfind(|p| p[..2] == [0, 0x73]).map(|p| p[11]), Some(0), "tag 0");
        let row = app.menu_row(Menu::Sensors, Kind::ALL.len()).unwrap();
        assert_eq!(row.1.as_str(), "NO LINK");
        app.activate(&mut sim, Menu::Sensors, Kind::ALL.len());
        run(&mut sim, &mut app, 3000);
        assert_eq!(sim.nordic.connects, 0);
        assert_eq!(app.radio_resets(), 0, "and the link watchdog is content");
    }
}

/// An ANT+ trainer paired too: ANT+ first, the Bluetooth one is not connected; pairing the Bluetooth one forgets the
/// ANT+ one. A restart keeps the pairing and connects without a search.
#[test]
fn one_trainer_at_a_time_and_kept_across_restarts() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    let ant = SensorId { device: 5555, trans: 0xf1 };
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert!(trainer(&sim).control);
    app.set_sensor(&mut sim, Kind::Trainer, Sensor::Paired(ant));
    run(&mut sim, &mut app, 1000);
    assert!(!trainer(&sim).connected, "ANT+ first");
    let connects = sim.nordic.connects;
    run(&mut sim, &mut app, 20_000);
    assert_eq!(sim.nordic.connects, connects);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.sensors[Kind::Trainer as usize], Sensor::Off, "the ANT+ one forgotten");
    assert!(trainer(&sim).control);
    assert_eq!(super::trainer_tests::sensors_file(&sim), Some(None));
    // a restart on the same card
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.nordic.ftms = Some(FtmsSim::new());
    let mut app2 = App::start(&mut sim2, Some(KEY));
    run(&mut sim2, &mut app2, 8000);
    assert!(trainer(&sim2).control, "connected again from the file");
    // forgotten: disconnected, and not again
    app2.activate(&mut sim2, Menu::Sensors, Kind::ALL.len());
    app2.activate(&mut sim2, Menu::Sensors, Kind::ALL.len());
    run(&mut sim2, &mut app2, 10_000);
    assert!(!trainer(&sim2).connected);
    assert_eq!(bt_file(&sim2), None);
}

/// The link's end lost among dropped bytes (the trainer gone, no event reached the unit): its data stop, and after
/// [`crate::QUIET_MS`] the unit drops the link and makes it again.
#[test]
fn a_silent_link_is_made_again() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    sim.nordic.ftms.as_mut().unwrap().hang_up(); // no event
    run(&mut sim, &mut app, u64::from(crate::QUIET_MS) - 1000);
    assert_eq!(sim.nordic.connects, 1, "not before the wait");
    run(&mut sim, &mut app, 6000);
    assert_eq!(sim.nordic.connects, 2);
    assert!(trainer(&sim).control, "back");
}

/// The BLE event packet of a GATT client answer `id` on `conn`, `body` after the status and the error handle.
fn gattc(id: u8, conn: u8, status: u16, body: &[u8]) -> Vec<u8> {
    let mut p = vec![2, id, 0, conn, 0];
    p.extend(status.to_le_bytes());
    p.extend([0, 0]);
    p.extend(body);
    p
}

/// Review: the trainer's DISCONNECTED lost (a resync), the phone then connects on the freed handle: the handle is the
/// phone's, not the trainer's (no discovery on the phone, which would drop it).
#[test]
fn a_phone_on_the_trainers_stale_handle_is_the_phone() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ftms.conn, Some(1));
    // BLE_EVT, id 0x10, conn 1, peer 7 bytes, role 1 (we are the peripheral: the phone)
    let ev = [2u8, 0x10, 0, 1, 0, 0, 1, 2, 3, 4, 5, 6, 1, 6, 0, 12, 0, 0, 0, 0x90, 1];
    app.phone_event(&mut sim, &ev);
    assert_eq!(app.phone.conn, Some(1), "the phone's link");
    assert_eq!(app.ftms.conn, None, "the trainer's link is over");
    run(&mut sim, &mut app, 500);
    assert_eq!(app.phone.conn, Some(1), "not dropped by a trainer discovery");
}

/// A resync that dropped the trainer's DISCONNECTED: the chip is asked about the link and it is over.
#[test]
fn a_lost_trainer_disconnection_is_found_after_a_resync() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ftms.conn, Some(1));
    app.ftms_recheck(&mut sim);
    assert_eq!(app.ftms.conn, Some(1), "a live link stays");
    sim.nordic.ftms.as_mut().unwrap().hang_up(); // no event
    app.ftms_recheck(&mut sim);
    assert_eq!(app.ftms.conn, None);
}

/// A trainer out of range is tried five times, then left alone until the rider asks for it (TRAINER CONTROL).
#[test]
fn an_absent_trainer_is_not_scanned_for_all_day() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.drop_at, t.connectable) = (Some(sim.ms), false);
    run(&mut sim, &mut app, 600_000);
    assert_eq!(sim.nordic.connects, 1 + 5, "the first, then five attempts");
    app.open(Screen::Trainer);
    run(&mut sim, &mut app, 600_000);
    assert_eq!(sim.nordic.connects, 1 + 5 + 5, "five more once asked");
    app.close();
    sim.nordic.ftms.as_mut().unwrap().connectable = true;
    app.open(Screen::Trainer);
    run(&mut sim, &mut app, 3000);
    assert!(trainer(&sim).control, "back");
}

/// The connection is made scanning at half duty, with the slower parameters (30 to 60 ms).
#[test]
fn the_connection_is_slow_and_scans_at_half_duty() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let c = sim.nordic.got.iter().find(|p| p[..2] == [0, 0x8c]).expect("connect");
    let u16_at = |i: usize| u16::from_le_bytes([c[i], c[i + 1]]);
    // [0, op], present, peer 7, active, selective, interval, window, time-out; present, min, max, latency, supervision
    assert_eq!((u16_at(12), u16_at(14)), (0x60, 0x30), "30 ms of every 60: {c:?}");
    assert_eq!((u16_at(19), u16_at(21), u16_at(23), u16_at(25)), (24, 48, 0, 400));
}

/// Review: the control lost (Machine Status 0xff) while a target's write is in flight: Request Control waits for the
/// write's answer (the chip allows one procedure; the answer is not Request Control's), then goes out, and the link stays.
#[test]
fn control_lost_with_a_write_in_flight_is_asked_again() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert!(trainer(&sim).control && app.ftms.control);
    app.ftms_command(&mut sim, crate::trainer::Command::Power(400)); // 100 W, answer not read yet
    let status = [2u8, 0x39, 0, 1, 0, 0, 0, 0, 0, FT_STATUS as u8, 0, 1, 1, 0, 0xff];
    app.phone_event(&mut sim, &status);
    assert!(!app.ftms.control);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ftms.conn, Some(1), "the link stays");
    assert_eq!(sim.nordic.connects, 1);
    let t = trainer(&sim);
    assert_eq!(t.got.iter().filter(|w| w[..] == [0x00]).count(), 2, "control asked again: {:?}", t.got);
    assert!(app.ftms.control);
    assert_eq!(t.violations, 0);
}

/// Review: an answer that is not the discovery's (a late write answer, another procedure's) leaves the request out:
/// the real answer still advances it.
#[test]
fn a_stray_answer_does_not_stall_the_discovery() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    let mut at = 0;
    while app.ftms.conn.is_none() && at < 20_000 {
        run(&mut sim, &mut app, 1);
        at += 1;
    }
    assert_eq!(app.ftms.conn, Some(1));
    app.phone_event(&mut sim, &gattc(0x38, 1, 0, &[0x50, 0, 1, 0, 0, 0, 0])); // Written, any handle
    app.phone_event(&mut sim, &gattc(0x36, 1, 0, &[0x50, 0, 0, 0, 0, 0])); // Read, empty
    run(&mut sim, &mut app, 3000);
    let t = trainer(&sim);
    assert!(t.control && t.on(FT_DATA, 1) && t.on(FT_STATUS, 1) && t.on(FT_CP, 2));
    assert_eq!(sim.nordic.connects, 1);
    assert_eq!(t.violations, 0);
}

/// Leaving the trainer list without pairing ends the search: the trainer is not listed from a later scan.
#[test]
fn leaving_the_list_ends_the_search() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    run(&mut sim, &mut app, 4000);
    app.ftms_search(&mut sim);
    run(&mut sim, &mut app, 1100);
    assert!(app.ftms.n_found > 0 && app.ftms.searching);
    app.close();
    run(&mut sim, &mut app, 500);
    assert!(!app.ftms.searching);
}

/// BLE NEARBY while the trainer is being connected: the connection is given up first (the chip refuses a scan during
/// one), the scan runs, and the trainer's attempts go on after it.
#[test]
fn a_nearby_scan_gives_up_the_connection_being_made() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.drop_at, t.connectable) = (Some(sim.ms), false);
    run(&mut sim, &mut app, 4000); // dropped, then connecting to nobody
    assert!(app.ftms.conn.is_none());
    let mark = sim.nordic.got.len();
    app.activate(&mut sim, Menu::System, 3); // BLE NEARBY
    let ops: Vec<u8> = sim.nordic.got[mark..].iter().map(|p| p[1]).collect();
    assert_eq!(ops.first(), Some(&0x8d), "connect cancel first: {ops:x?}");
    assert!(ops.contains(&0x8a) && app.scanning, "the scan runs: {ops:x?}");
    sim.nordic.ftms.as_mut().unwrap().connectable = true;
    run(&mut sim, &mut app, 80_000);
    assert!(trainer(&sim).control, "the trainer's attempts went on after the scan");
}

fn erg(sim: &Sim) -> bool {
    trainer(sim).mode == 5
}

fn free(sim: &Sim) -> bool {
    (trainer(sim).mode, trainer(sim).resistance) == (4, 0)
}

/// A stopped workout, or its ride ended: the FTMS trainer is let go (Set Target Resistance Level 0).
#[test]
fn a_stopped_workout_releases_the_ftms_trainer() {
    for how in [super::trainer_tests::Ends::Stop, super::trainer_tests::Ends::EndRide] {
        let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
        pair(&mut sim, &mut app);
        run(&mut sim, &mut app, 2000);
        super::trainer_tests::release_after(&mut sim, &mut app, how, erg, free);
    }
}

/// The ride paused: the FTMS trainer is let go; resumed, the target again.
#[test]
fn a_paused_ride_releases_the_ftms_trainer() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    super::trainer_tests::pause_releases(&mut sim, &mut app, erg, free);
}

/// The switch-off lets the FTMS trainer go.
#[test]
fn the_switch_off_releases_the_ftms_trainer() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    super::trainer_tests::off_releases(&mut sim, &mut app, erg, free);
}

/// A command's write answer lost (among bytes a resync dropped): it no longer holds Request Control up when the
/// control is lost, so the target is set again (review: the link's control was wedged for the connection).
#[test]
fn a_lost_write_answer_does_not_wedge_the_control() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    sim.nordic.drop_gattc = Some((1, 0x38));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.drop_gattc, None, "the write's answer was lost");
    assert_eq!((trainer(&sim).mode, trainer(&sim).target_w), (5, 105));
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.lose_control_at, t.mode, t.target_w) = (Some(sim.ms), 0, 0);
    run(&mut sim, &mut app, 6000);
    let t = trainer(&sim);
    assert_eq!(t.got.iter().filter(|w| w[..] == [0x00]).count(), 2, "control asked again: {:?}", t.got);
    assert_eq!((t.control, t.mode, t.target_w), (true, 5, 105), "and the target set again");
}

/// A trainer that refused the control is asked again later while the rider drives it (review: never again for the
/// link), and the target goes out once it is given.
#[test]
fn a_refused_control_is_asked_again() {
    let mut t = FtmsSim::new();
    t.refuse_control = true;
    let (mut sim, mut app) = unit_with(t, None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert!(!trainer(&sim).control && !app.ftms.control);
    sim.nordic.ftms.as_mut().unwrap().refuse_control = false;
    run(&mut sim, &mut app, 25_000);
    let t = trainer(&sim);
    assert!(t.got.iter().filter(|w| w[..] == [0x00]).count() >= 2, "asked again: {:?}", t.got);
    assert_eq!((t.control, t.mode, t.target_w), (true, 5, 105), "given: the target");
    assert_eq!(t.violations, 0);
}

/// The switch-off right after a command's write (the 4 s resend): its answer is waited for, then the release goes.
#[test]
fn the_switch_off_with_a_write_in_flight_releases_the_ftms_trainer() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert!(erg(&sim));
    app.ftms_command(&mut sim, crate::trainer::Command::Power(420)); // its answer not read yet
    app.power_off(&mut sim);
    assert!(free(&sim), "released: {:?}", trainer(&sim).got);
    assert_eq!(trainer(&sim).violations, 0);
}

/// A release the trainer never answers is sent again, a few times at most (and once when answered).
#[test]
fn the_ftms_release_is_resent_a_few_times_at_most() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    super::trainer_tests::start_workout(&mut sim, &mut app, erg);
    let release = |sim: &Sim| trainer(sim).got.iter().filter(|w| w[..] == [0x04, 0]).count();
    let before = release(&sim);
    sim.nordic.ftms.as_mut().unwrap().mute_cp = true;
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, crate::menu::Row::Ride(crate::menu::RideRow::StopWorkout));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 60_000);
    assert_eq!(release(&sim) - before, usize::from(crate::trainer::RELEASE_TRIES));
}

/// The link lost and made again while the ride is paused: the trainer stays free; the target comes back on resume.
#[test]
fn a_reconnect_during_a_pause_keeps_the_trainer_free() {
    let (mut sim, mut app) = unit_with(FtmsSim::new(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    super::trainer_tests::start_workout(&mut sim, &mut app, erg);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    run(&mut sim, &mut app, 2000);
    assert!(free(&sim));
    let t = sim.nordic.ftms.as_mut().unwrap();
    t.drop_at = Some(sim.ms);
    run(&mut sim, &mut app, 15_000);
    assert!(trainer(&sim).connected && trainer(&sim).control, "linked again");
    assert!(sim.nordic.connects >= 2);
    assert!(!erg(&sim), "still free while paused");
    app.press(&mut sim, Press::Short(Key::Start)); // resume
    run(&mut sim, &mut app, 3000);
    assert!(erg(&sim), "the target on resume");
}
