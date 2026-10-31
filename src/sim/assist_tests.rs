//! GNSS SYSTEMS and AssistNow Offline on the Sim (drivers/gnss.rs `systems`, drivers/mga.rs, app/gps.rs): the
//! receiver is told its systems at power-up and when the setting changes, and gets the day's MGA-ANO messages from
//! `\Rustfw\mga.ubx`, and nothing when the file is missing, stale or damaged.

extern crate std;
use super::tests::{run, to_row};
use super::*;
use crate::PAGES_LOAD_MS;
use crate::fat::tests::Ram;
use crate::menu::Setting;
use crate::mga::ano;
use std::vec::Vec;

/// The volume with `mga.ubx` holding `bytes`.
fn with_file(bytes: &[u8]) -> Vec<u8> {
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let t = crate::fat::write::dos_time((2026, 10, 20, 8, 0, 0));
    let dir = fat.mkdir(&mut d, fat.root(), crate::recorder::DIR, t).unwrap();
    let mut h = fat.create(&mut d, dir, "mga.ubx", t).unwrap();
    let mut padded = bytes.to_vec();
    padded.resize(bytes.len().div_ceil(512).max(1) * 512, 0);
    fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
    fat.set_size(&mut d, &mut h, bytes.len() as u32, t).unwrap();
    d.0
}

/// A unit started on `image`, run well past the receiver's start-up; today in UTC (the RTC runs on UTC + 2 h) as
/// year since 2000, month, day.
fn unit(image: Vec<u8>, systems: usize) -> (Sim, App, (u8, u8, u8)) {
    unit_with(image, systems, Some(8)) // the RTC's offset from UTC learnt at an earlier fix
}

fn unit_with(image: Vec<u8>, systems: usize, offset: Option<i8>) -> (Sim, App, (u8, u8, u8)) {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    [sim.backup[17], sim.backup[18]] = crate::gnss::Aid { offset_15min: offset, pos: None }.to_words();
    sim.emmc.image = image;
    let mut app = App::start(&mut sim, None);
    app.settings.gnss_systems = systems;
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 6000);
    let (tr, dr) = sim.rtc();
    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
    let (y, mo, d, ..) = from_epoch_s(epoch_s((y, mo, d, h, m, s)) - 2 * 3600);
    (sim, app, ((y - 2000) as u8, mo, d))
}

/// Which of GPS, GAL, BDS, GLO the receiver was last told to track.
fn on(sim: &Sim) -> [u8; 4] {
    let mut v = [9; 4];
    for &(key, val) in &sim.gnss.signals {
        let at = [0x1031_001f, 0x1031_0021, 0x1031_0022, 0x1031_0025].iter().position(|&k| k == key).unwrap();
        v[at] = val;
    }
    v
}

#[test]
fn the_receiver_is_told_its_systems_at_start_and_on_a_change() {
    let (mut sim, mut app, _) = unit(crate::fat::tests::image(), 0);
    assert_eq!(on(&sim), [1, 1, 1, 0], "the default: GPS, Galileo, BeiDou");
    sim.gnss.signals.clear();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Settings));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::GnssSystems));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.settings.gnss_systems, 1);
    assert_eq!(on(&sim), [1, 0, 0, 0], "GPS alone, sent once");
    assert_eq!(sim.gnss.signals.len(), 4);
    // a unit started with the setting sends it at power-up
    let (sim2, _, _) = unit(crate::fat::tests::image(), 3);
    assert_eq!(on(&sim2), [1, 1, 0, 1]);
}

#[test]
fn todays_offline_messages_go_to_the_receiver_paced() {
    let probe = unit(crate::fat::tests::image(), 0).2;
    let mut file = Vec::new();
    for day in [probe.2.wrapping_sub(1), probe.2, probe.2 + 1] {
        for sv in 1..=32u8 {
            file.extend(ano(0, sv, (probe.0, probe.1, day)));
        }
        file.extend(ano(2, 7, (probe.0, probe.1, day))); // the file is in date order
    }
    let (sim, app, today) = unit(with_file(&file), 0);
    assert_eq!(today, probe);
    let got = &sim.gnss.ano;
    assert_eq!(got.len(), 33, "today's, none of the other days");
    assert!(got.iter().all(|a| (a.3, a.4, a.5) == today), "the date");
    assert!(got.iter().take(32).enumerate().all(|(i, a)| a.1 as usize == i + 1), "in the file's order");
    assert!(got.windows(2).all(|w| w[1].0 >= w[0].0 + 100), "one message every 100 ms or more");
    assert!(app.gps.ano.done() && app.gps.ano.found == 33);
    assert!(app.gps.pvt.is_none() || app.gps.pvt.is_some_and(|p| p.fix <= 3), "the fixes are still read meanwhile");
}

#[test]
fn a_missing_stale_or_damaged_file_sends_nothing_and_is_not_a_problem() {
    let probe = unit(crate::fat::tests::image(), 0).2;
    let other = (probe.0, probe.1, if probe.2 > 10 { probe.2 - 5 } else { probe.2 + 5 });
    let mut stale = Vec::new();
    (1..=32u8).for_each(|sv| stale.extend(ano(0, sv, other)));
    let mut damaged = Vec::new();
    for sv in 1..=32u8 {
        let mut f = ano(0, sv, (probe.0, probe.1, probe.2));
        f[30] ^= 0x55;
        damaged.extend(f);
    }
    for (name, image) in [
        ("missing", crate::fat::tests::image()),
        ("stale", with_file(&stale)),
        ("damaged", with_file(&damaged)),
        ("empty", with_file(&[])),
        ("noise", with_file(&[0xb5; 3000])),
    ] {
        let (sim, app, _) = unit(image, 0);
        assert!(sim.gnss.ano.is_empty(), "{name}");
        assert!(app.gps.ano.done(), "{name}: given up");
    }
}

/// GPS POWER and GNSS change in the same pass: both frames go out; a GNSS change during a ride waits for its end.
#[test]
fn both_changes_go_out_and_the_systems_wait_for_the_ride_to_end() {
    let (mut sim, mut app, _) = unit(crate::fat::tests::image(), 0);
    sim.gnss.signals.clear();
    app.settings.gnss_systems = 2;
    app.settings.gps_power = GpsPower::Saver;
    app.gps.resys = true;
    app.gps.reconfig = true;
    run(&mut sim, &mut app, 500);
    assert_eq!(on(&sim), [1, 1, 0, 0], "the systems");
    assert_eq!(sim.gnss.pm_mode, 2, "and the power mode");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    sim.gnss.signals.clear();
    app.settings.gnss_systems = 1;
    app.gps.resys = true;
    run(&mut sim, &mut app, 2000);
    assert!(sim.gnss.signals.is_empty(), "not while recording");
    app.press(&mut sim, Press::Short(Key::Start)); // paused
    app.end_ride();
    run(&mut sim, &mut app, 2000);
    assert_eq!(on(&sim), [1, 0, 0, 0], "at the ride's end");
}

/// Without a learnt offset the RTC's date may be a day off UTC: tomorrow's messages are taken too.
#[test]
fn without_an_offset_a_day_of_slack() {
    let probe = unit(crate::fat::tests::image(), 0).2;
    let mut file = Vec::new();
    for day in [probe.2, probe.2 + 1] {
        file.extend(ano(0, 1, (probe.0, probe.1, day)));
    }
    let days = |offset| unit_with(with_file(&file), 0, offset).0.gnss.ano.iter().map(|a| a.5).collect::<Vec<_>>();
    assert_eq!(days(Some(8)), [probe.2]);
    assert_eq!(days(None), [probe.2 + 1]);
}
