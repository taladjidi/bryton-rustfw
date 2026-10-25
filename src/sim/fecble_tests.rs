//! A trainer without FTMS over Bluetooth, FE-C over BLE (drivers/fecble.rs, app/ftms.rs) on the Sim: the owner's Van
//! Rysel D500 as seen from a computer (`FtmsSim::new_fec`: the 128-bit service in the scan response, its GATT
//! table, pages 16 and 25 notified, the pages written applied and answered by a page 71). The FTMS twin of these
//! tests is ftms_tests.rs.

use super::ble_sim::FC_NOTIFY;
use super::ftms_tests::{KEY, bt_file, trainer, unit_with};
use super::*;
use crate::fecble::{self, Proto};
use crate::menu::Row;
use crate::ui::Field;

/// SENSORS, BT TRAINER: the search lists the D500 (its name and service in the scan response, the first report
/// only the 16-bit services); Select pairs it.
fn pair(sim: &mut Sim, app: &mut App) {
    run(sim, app, 4000); // the sensors file read
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Sensors));
    app.press(sim, Press::Short(Key::Start));
    super::tests::to_row(sim, app, Row::BtTrainer);
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 1100);
    assert_eq!(app.ftms.n_found, 1, "listed once, from the scan response");
    let (label, value, _) = app.menu_row(Menu::BtTrainers, 0).expect("a trainer heard");
    assert_eq!((label.as_str(), value.as_str()), ("SIM D500", "FE-C -56"));
    sim.save("fecble_search");
    app.press(sim, Press::Short(Key::Start));
    app.leave_menus();
}

/// The pairing with its protocol in `\Rustfw\sensors.bin`.
fn file_proto(sim: &Sim) -> Option<([u8; 7], bool)> {
    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = crate::fat::tests::read_all(&mut fat, &mut d, "Rustfw/sensors.bin").ok()?;
    crate::sensors::bt_from_sector(&v.try_into().ok()?)
}

/// Paired from the list: connected, the vendor base registered, the service and its characteristics found among the
/// others, the notifications on, the capabilities asked for (a write with response, one per 250 ms), the readings on
/// the fields; the protocol kept in the sensors file.
#[test]
fn the_d500_is_paired_and_read() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
    let vs = sim.nordic.server.vs.len();
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let t = trainer(&sim);
    assert!(t.connected && t.on(FC_NOTIFY, 1), "connected and subscribed");
    assert_eq!(sim.nordic.server.vs.len(), vs + 1);
    assert_eq!(sim.nordic.server.vs[vs], fecble::BASE, "the base registered with the chip");
    assert_eq!(t.got.first(), Some(&vec![0x46, 255, 255, 255, 255, 1, 54, 1]), "the capabilities asked for");
    assert_eq!((t.violations, t.rejected), (0, 0));
    assert_eq!(file_proto(&sim), Some((t.peer(), true)));
    assert_eq!(bt_file(&sim), Some(t.peer()));
    assert!(app.ftms.conn == Some(1) && app.ftms.control);
    assert_eq!(app.trainer.reading.modes, Some(7), "page 54 came back");
    assert_eq!(
        [Metric::Power, Metric::Cadence, Metric::Speed].map(|m| app.live.get(m)),
        [Some(150), Some(90), Some(226)]
    );
    let row = app.menu_row(Menu::Sensors, Kind::ALL.len()).unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str()), ("BT TRAINER", "LINKED"));
    // the trainer screen, taken over: 105 W, as a page 49
    app.open(Screen::Trainer);
    app.show(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!((trainer(&sim).mode, trainer(&sim).target_w), (5, 105));
    assert_eq!(app.field_text(Field::Power).as_str(), "105");
    assert_eq!(app.trainer.reading.last_cmd, Some((0x31, 0)), "its page 71");
    assert_eq!(trainer(&sim).violations, 0, "one message per slot");
    super::tests::assert_consistent(&sim, &app);
}

/// A trainer whose frames carry the sum checksum is answered in kind; one with the XOR is answered with the XOR.
#[test]
fn the_checksum_is_the_trainers() {
    for sum in [false, true] {
        let mut t = FtmsSim::new_fec();
        t.sum = sum;
        let (mut sim, mut app) = unit_with(t, None);
        pair(&mut sim, &mut app);
        run(&mut sim, &mut app, 3000);
        let t = trainer(&sim);
        assert!(!t.got.is_empty() && t.rejected == 0);
        let want = fecble::frame(t.got.last().unwrap().clone().try_into().unwrap(), sum);
        assert_eq!(t.raw, want, "sum {sum}");
    }
}

/// A workout in ERG: each step's target as page 49, held by the trainer, recorded in the ride.
#[test]
fn erg_workout_over_fec_ble_is_recorded() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
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
    assert_eq!((trainer(&sim).violations, trainer(&sim).rejected), (0, 0));
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A course: SIM, the grade as page 51.
#[test]
fn sim_follows_the_course_over_fec_ble() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
    app.set_course_memory(crate::storage::course::tests::memory());
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
    assert!(t.got.iter().any(|p| p[0] == 0x33), "page 51 written");
    assert!(t.grade_x100 > 0, "on the hill: {}", t.grade_x100);
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "SIM");
    assert_eq!(t.violations, 0);
}

/// The link drops: the readings stop, the unit connects again (the base is not registered twice), subscribes, and
/// the target in force is written again. Out of range, it gives up after the attempts.
#[test]
fn a_lost_fec_link_is_made_again() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let vs = sim.nordic.server.vs.len();
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!(trainer(&sim).target_w, 105);
    sim.nordic.ftms.as_mut().unwrap().drop_at = Some(sim.ms);
    sim.nordic.ftms.as_mut().unwrap().target_w = 0;
    run(&mut sim, &mut app, 300);
    assert_eq!(app.ftms.conn, None);
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "--", "not heard any more");
    run(&mut sim, &mut app, 5000);
    let t = trainer(&sim);
    assert!(t.connected && t.on(FC_NOTIFY, 1), "back");
    assert_eq!(sim.nordic.connects, 2);
    assert_eq!(t.target_w, 105, "the target in force written again");
    assert_eq!(sim.nordic.server.vs.len(), vs, "the base registered once");
    let t = sim.nordic.ftms.as_mut().unwrap();
    (t.drop_at, t.connectable) = (Some(sim.ms), false);
    run(&mut sim, &mut app, 120_000);
    assert_eq!(sim.nordic.connects - 2, 5, "then left alone");
    sim.nordic.ftms.as_mut().unwrap().connectable = true;
    app.open(Screen::Trainer);
    run(&mut sim, &mut app, 5000);
    assert!(trainer(&sim).connected, "back once in range and asked for");
}

/// A restart on the same card: the pairing and its protocol are read back, the unit connects without a search; a
/// chip reset registers the base again.
#[test]
fn the_protocol_is_kept_across_restarts() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
    pair(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.nordic.ftms = Some(FtmsSim::new_fec());
    let mut app2 = App::start(&mut sim2, Some(KEY));
    run(&mut sim2, &mut app2, 8000);
    assert!(app2.sensors_store.bt_fec);
    assert!(trainer(&sim2).connected && trainer(&sim2).on(FC_NOTIFY, 1), "connected again from the file");
    let vs = sim2.nordic.server.vs.len();
    app2.radio_restart(&mut sim2); // the chip restarted: its UUID table is empty
    run(&mut sim2, &mut app2, 8000);
    assert!(trainer(&sim2).connected, "and back");
    assert_eq!(sim2.nordic.server.vs.len(), vs, "the base added once to the new table");
}

/// What a report says: the first report of the D500 (16-bit services) is not listed, its scan response is; a
/// trainer offering both is listed as FTMS whichever report comes first.
#[test]
fn both_protocols_prefer_ftms() {
    let (mut sim, mut app) = unit_with(FtmsSim::new_fec(), None);
    run(&mut sim, &mut app, 100);
    app.ftms.searching = true;
    let mut fec_list = vec![17, 0x07];
    fec_list.extend(fecble::BASE);
    let ftms_data = [2, 0x01, 0x06, 6, 0x16, 0x26, 0x18, 0x01, 0x20, 0x00];
    fn report(data: &[u8], n: u8) -> crate::nrf::AdvReport<'_> {
        crate::nrf::AdvReport { addr_type: 0, addr: [n, 1, 2, 3, 4, 5], rssi: -50, data }
    }
    assert!(!app.ftms.found(&report(&[5, 0x03, 0x18, 0x18, 0x16, 0x18], 1)), "1818 and 1816 only");
    assert!(app.ftms.found(&report(&fec_list, 1)));
    assert!(app.ftms.found(&report(&ftms_data, 2)));
    assert!(app.ftms.found(&report(&fec_list, 2)));
    assert!(app.ftms.found(&report(&ftms_data, 1)), "the FE-C one offers FTMS too");
    let protos: Vec<Proto> = app.ftms.found.iter().take(app.ftms.n_found).map(|d| d.proto).collect();
    assert_eq!(protos, [Proto::Ftms, Proto::Ftms]);
}
