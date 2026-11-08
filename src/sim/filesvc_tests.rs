//! The file service on the phone bond (app/filesvc.rs, link/filesvc.rs) against the simulated chip and a scripted
//! phone: LIST, READ whole and from an offset, BUSY, the encrypted bond, names outside `\Activities\`, a link
//! dropped mid-read and resumed.

use super::*;
use crate::fat::tests::{Ram, put_file};
use crate::filesvc::{Command, Folder, Frame, Key, Name, Status};

/// The rides on the test card: (name, bytes). The first spans several sectors.
pub(super) fn rides() -> [(Name, Vec<u8>); 2] {
    let big: Vec<u8> = (0..3000u32).map(|i| (i * 7 + i / 251) as u8).collect();
    [(*b"251026093000", big), (*b"251027101500", (0..10u8).collect())]
}

/// A unit with the rides (and what must stay hidden) on the card, paired and connected with a phone that has
/// enabled Data; the handles of Control and Data.
pub(super) fn paired() -> (Sim, App, u16, u16) {
    let (mut sim, mut app) = bonded(0);
    let (control, data) = app.phone.files.handles.expect("the service was added");
    sim.nordic.phone_subscribe(data + 1, true);
    run(&mut sim, &mut app, 200);
    (sim, app, control, data)
}

/// PAIR from the menu and run until the phone is linked and secure.
fn pair_phone(sim: &mut Sim, app: &mut App) {
    app.activate(sim, Menu::Phone, 1);
    run(sim, app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
}

/// The unit with the rides on a chip holding `vs_max` vendor bases (0: the default), bonded to the phone.
fn bonded(vs_max: usize) -> (Sim, App) {
    let mut phone = PhoneSim::new(true);
    phone.connect_at = 1000;
    let (mut sim, mut app) = super::tests::unit_with(None, |sim| {
        sim.nordic.phone = Some(phone);
        sim.nordic.vs_max = vs_max;
        let mut d = Ram(sim.emmc.image.clone(), 0);
        for (name, data) in rides() {
            put_file(&mut d, "Activities", &std::format!("{}.fit", std::str::from_utf8(&name).unwrap()), &data);
        }
        put_file(&mut d, "Activities", "tmp.fit", b"stock's ride in progress");
        put_file(&mut d, "Activities", "notes.txt", b"not a ride");
        sim.emmc.image = d.0;
    });
    pair_phone(&mut sim, &mut app);
    (sim, app)
}

/// Nothing was answered and nothing is under way, after 500 ms.
fn stays_silent(sim: &mut Sim, app: &mut App, data: u16) {
    run(sim, app, 500);
    assert!(!app.phone.files.active() && drain(sim, data).is_empty());
}

/// The Data frames notified so far, drained.
pub(super) fn drain(sim: &mut Sim, data: u16) -> Vec<Vec<u8>> {
    let sent = std::mem::take(&mut sim.nordic.server.sent);
    let (mine, rest): (Vec<_>, Vec<_>) = sent.into_iter().partition(|s| s.0 == data);
    sim.nordic.server.sent = rest;
    mine.into_iter().map(|s| s.1).collect()
}

/// The phone's link drops (the chip's DISCONNECTED too) and it connects again at `back_at` (never: `u64::MAX`).
pub(super) fn drop_link(sim: &mut Sim, back_at: u64) {
    let ph = sim.nordic.phone.as_mut().unwrap();
    ph.hang_up();
    ph.connect_at = back_at;
    sim.nordic.evt(0x11, &[0, 0, 0x08]);
}

/// The phone enables Data again on a new link and the frames of the wait are dropped.
pub(super) fn resubscribe(sim: &mut Sim, app: &mut App, data: u16) {
    sim.nordic.phone_subscribe(data + 1, true);
    run(sim, app, 200);
    drain(sim, data);
}

/// [`run`] with the loop's real pass: the short one while a READ runs on the fast link ([`App::pass_ms`]).
fn pass(sim: &mut Sim, app: &mut App, ms: u64) {
    pass_with(sim, app, ms, true);
}

/// [`pass`]; `wake`: the chip's bytes end the sleep for a radio pass (`LoopPolicy::begin_sleep`, since 2026-11-07),
/// else the events wait for the next step (the loop of 20989f2a, which measured 16.6 KB/s on the unit).
fn pass_with(sim: &mut Sim, app: &mut App, ms: u64, wake: bool) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        if !app.phone.files.fast() {
            sim.sleep_ms(super::POLL_MS);
            continue;
        }
        for slept in 1..=app.pass_ms() {
            sim.sleep_ms(1);
            if wake && crate::LoopBoard::byte_waiting(sim) {
                app.radio_pass(sim, slept);
            }
        }
    }
}

/// Writes a READ of `name` from 0 to Control, without waiting for the answer.
fn start_read(sim: &mut Sim, control: u16, seq: u8, name: Name) {
    let mut f = [0; 20];
    let n = Command::Read { seq, offset: 0, name }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
}

/// Runs `passes` of 100 ms and gathers the Data frames: the file bytes and the END (status, count).
fn collect(sim: &mut Sim, app: &mut App, data: u16, passes: u32) -> (Vec<u8>, Option<(Status, u32)>) {
    let (mut kept, mut end): (Vec<u8>, _) = (Vec::new(), None);
    for _ in 0..passes {
        pass(sim, app, 100);
        for p in drain(sim, data) {
            match Frame::parse(&p) {
                Some(Frame::Chunk { data, .. }) => kept.extend(data),
                Some(Frame::End { status, count, .. }) => end = Some((status, count)),
                _ => {}
            }
        }
    }
    (kept, end)
}

/// What an answer held.
#[derive(Default, Debug)]
pub(super) struct Answer {
    pub entries: Vec<(Name, u32)>,
    pub bytes: Vec<u8>,
    pub end: Option<(Status, u32)>,
    /// Frames that were not for this `seq`, or did not parse.
    pub stray: usize,
    /// Simulated ms until the END.
    pub ms: u64,
}

/// Sends `cmd` and runs until its END (`max_ms` at most).
pub(super) fn ask(sim: &mut Sim, app: &mut App, control: u16, data: u16, cmd: Command, max_ms: u64) -> Answer {
    let mut f = [0; 20];
    let n = cmd.encode(&mut f);
    ask_raw(sim, app, control, data, &f[..n], max_ms)
}

/// [`ask`] with raw bytes.
pub(super) fn ask_raw(sim: &mut Sim, app: &mut App, control: u16, data: u16, raw: &[u8], max_ms: u64) -> Answer {
    let seq = raw.get(1).copied().unwrap_or(0);
    let mut a = Answer::default();
    let start = sim.ms;
    sim.nordic.phone_write(control, raw);
    while a.end.is_none() && sim.ms < start + max_ms {
        pass(sim, app, 50);
        for p in drain(sim, data) {
            match Frame::parse(&p) {
                Some(Frame::Entry { seq: s, name, size }) if s == seq => a.entries.push((name, size)),
                Some(Frame::Chunk { seq: s, data }) if s == seq => a.bytes.extend(data),
                Some(Frame::End { seq: s, status, count }) if s == seq => a.end = Some((status, count)),
                _ => a.stray += 1,
            }
        }
    }
    a.ms = sim.ms - start;
    a
}

#[test]
fn list_gives_the_rides_and_only_the_rides() {
    let (mut sim, mut app, control, data) = paired();
    let a = ask(&mut sim, &mut app, control, data, Command::List { seq: 1 }, 5000);
    // the test card holds rides of its own besides these two; tmp.fit and notes.txt are not among them
    assert_eq!(a.end, Some((Status::Ok, a.entries.len() as u32)));
    for r in rides() {
        assert!(a.entries.contains(&(r.0, r.1.len() as u32)));
    }
    assert!(a.entries.len() >= 2 && a.entries.iter().all(|e| crate::filesvc::is_name(&e.0)));
    assert_eq!(a.stray, 0);
}

#[test]
fn read_a_ride_whole_and_report_the_speed() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, bytes), _] = rides();
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 9, offset: 0, name }, 30_000);
    assert_eq!(a.end, Some((Status::Ok, bytes.len() as u32)));
    assert_eq!(a.bytes, bytes, "the bytes equal the file on the card");
    assert_eq!(a.stray, 0);
    // one 18-byte notification a 50 ms pass (the stack queues one): about 360 B/s
    let rate = bytes.len() as u64 * 1000 / a.ms;
    std::println!("file service: {} B in {} ms = {} B/s", bytes.len(), a.ms, rate);
    assert!((250..=400).contains(&rate), "{rate} B/s");
}

#[test]
fn read_from_an_offset_resumes() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, bytes), _] = rides();
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 2, offset: 1234, name }, 30_000);
    assert_eq!(a.bytes, bytes[1234..]);
    assert_eq!(a.end, Some((Status::Ok, bytes.len() as u32 - 1234)));
    let end = bytes.len() as u32;
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 3, offset: end, name }, 5000);
    assert_eq!((a.bytes.len(), a.end), (0, Some((Status::Ok, 0))), "at the end: nothing left");
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 4, offset: end + 1, name }, 5000);
    assert_eq!(a.end, Some((Status::BadRequest, 0)));
}

#[test]
fn a_ride_or_the_lent_card_makes_it_busy() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, _), _] = rides();
    app.emmc.lend(true);
    let a = ask(&mut sim, &mut app, control, data, Command::List { seq: 1 }, 5000);
    assert_eq!(a.end, Some((Status::Busy, 0)), "lent over USB");
    app.emmc.lend(false);
    // a transfer under way when the ride begins ends with BUSY, and the ride records on
    start_read(&mut sim, control, 5, name);
    run(&mut sim, &mut app, 300);
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    let (_, end) = collect(&mut sim, &mut app, data, 40);
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(end.map(|e| e.0), Some(Status::Busy), "the transfer was cut");
    for cmd in [Command::List { seq: 6 }, Command::Read { seq: 7, offset: 0, name }] {
        let a = ask(&mut sim, &mut app, control, data, cmd, 5000);
        assert_eq!(a.end, Some((Status::Busy, 0)), "{cmd:?} during a ride");
        assert!(a.entries.is_empty() && a.bytes.is_empty());
    }
}

#[test]
fn nothing_is_served_without_the_encrypted_bond() {
    let (mut sim, mut app, control, data) = paired();
    assert!(sim.nordic.server.locked.contains(&control), "Control needs an encrypted link");
    sim.nordic.unsecure_phone();
    assert!(!sim.nordic.phone_write(control, &[1, 1]), "the stack refuses the write");
    // and the app itself ignores one that reaches it on a link it does not hold secure
    app.phone.secure = false;
    app.files_command(&mut sim, &[1, 1]);
    stays_silent(&mut sim, &mut app, data);
}

#[test]
fn only_activities_rides_are_reachable() {
    let (mut sim, mut app, control, data) = paired();
    for raw in [
        &b"\x02\x01\0\0\0\0../Rustfw/ble.bin"[..],
        b"\x02\x01\0\0\0\0..\\Rustfw\\ble",
        b"\x02\x01\0\0\0\0tmp.fit\0\0\0\0\0\0",
        b"\x02\x01\0\0\0\x00251026093000.fit",
        b"\x02\x01\0\0\0\0",
        b"\x05\x01",
        b"",
    ] {
        let a = ask_raw(&mut sim, &mut app, control, data, raw, 2000);
        assert_eq!(a.end.map(|e| e.0), Some(Status::BadRequest), "{raw:?}");
        assert!(a.bytes.is_empty());
    }
    let a = ask_raw(&mut sim, &mut app, control, data, b"\x02\x01\0\0\0\x00999999999999", 2000);
    assert_eq!(a.end.map(|e| e.0), Some(Status::NotFound), "digits, but not a ride on the card");
}

#[test]
fn a_dropped_link_resumes_from_the_bytes_kept() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, bytes), _] = rides();
    start_read(&mut sim, control, 1, name);
    let (mut kept, _) = collect(&mut sim, &mut app, data, 15);
    assert!(!kept.is_empty() && kept.len() < bytes.len(), "{} kept", kept.len());
    // the phone walks away and comes back
    let back = sim.ms + 500;
    drop_link(&mut sim, back);
    run(&mut sim, &mut app, 300);
    assert!(!app.phone.files.active(), "the link's end cleared the transfer");
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    resubscribe(&mut sim, &mut app, data);
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 2, offset: kept.len() as u32, name }, 30_000);
    kept.extend(&a.bytes);
    assert_eq!(kept, bytes);
    assert_eq!(a.end, Some((Status::Ok, a.bytes.len() as u32)));
}

#[test]
fn a_phone_that_never_subscribes_does_not_hold_the_service() {
    let (mut sim, mut app, control, data) = paired();
    sim.nordic.phone_subscribe(data + 1, false);
    sim.nordic.phone_write(control, &[1, 1]);
    run(&mut sim, &mut app, 500);
    assert!(app.phone.files.active());
    run(&mut sim, &mut app, 6000);
    assert!(!app.phone.files.active(), "dropped after the stall time-out");
}

#[test]
fn a_ride_starting_mid_read_counts_only_the_bytes_the_phone_got() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, bytes), _] = rides();
    start_read(&mut sim, control, 1, name);
    run(&mut sim, &mut app, 1000);
    assert!(app.phone.files.has_pending(), "a chunk waits for the stack");
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    let (kept, end) = collect(&mut sim, &mut app, data, 40);
    assert_eq!(end, Some((Status::Busy, kept.len() as u32)), "END counts what was taken");
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(kept, bytes[..kept.len()]);
}

#[test]
fn a_folder_with_a_ride_name_is_not_a_ride() {
    let (mut sim, mut app, control, data) = paired();
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let act = fat.open(&mut d, "Activities").unwrap();
    fat.ensure_dir(&mut d, act, "251028080000", (0, 0)).unwrap();
    sim.emmc.image = d.0;
    let a = ask(&mut sim, &mut app, control, data, Command::List { seq: 1 }, 5000);
    assert!(!a.entries.iter().any(|e| &e.0 == b"251028080000"));
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 2, offset: 0, name: *b"251028080000" }, 5000);
    assert_eq!(a.end.map(|e| e.0), Some(Status::NotFound));
}

#[test]
fn an_encrypted_link_without_the_bond_is_refused() {
    let (mut sim, mut app, control, data) = paired();
    app.phone.stored.bond = None;
    sim.nordic.phone_write(control, &[1, 1]);
    stays_silent(&mut sim, &mut app, data);
}

#[test]
fn deleting_a_ride_ends_a_transfer_under_way() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, _), _] = rides();
    start_read(&mut sim, control, 1, name);
    run(&mut sim, &mut app, 500);
    app.phone.files.abort(Status::Busy); // what delete_past does first
    run(&mut sim, &mut app, 500);
    let ends = drain(&mut sim, data)
        .iter()
        .filter(|p| matches!(Frame::parse(p), Some(Frame::End { status: Status::Busy, .. })))
        .count();
    assert_eq!(ends, 1);
    assert!(!app.phone.files.active());
}

#[test]
fn a_chip_with_nine_bases_costs_the_file_service_not_the_trainer_or_the_phone() {
    let (sim, app) = bonded(9);
    assert_eq!(app.phone.link(), Link::Secure, "the phone link works");
    assert!(app.phone.files.handles.is_none(), "no file service");
    assert_eq!(sim.nordic.server.vs.len(), 9);
    assert_eq!(sim.nordic.server.vs[8], crate::fecble::BASE, "the ninth base is FE-C's");
    assert_ne!(app.ftms.vs, 0, "the trainer's base is held");
}

// ---------------------------------------------------------------------------------------------------------------
// The fast link (docs/roadmap.md 3c, reopened 2026-11-07): MTU 247, a queue of notifications, quick parameters
// during a READ. The UART's time is charged to the clock (`Sim::charge_wire`), so the rates are honest to the model.
// ---------------------------------------------------------------------------------------------------------------

/// The name of the big ride on the fast tests' card.
pub(super) const BIG: Name = *b"251028120000";

/// A pseudo-random body of `n` bytes.
pub(super) fn body(n: usize) -> Vec<u8> {
    (0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
}

/// A unit on the fast link's configuration, bonded to a phone that asks for `client_mtu`, with a ride of `size`
/// bytes (`BIG`) on the card and Data enabled; `tweak` runs on the sim before the unit starts. The UART is charged to
/// the clock from here on.
pub(super) fn fast_unit(client_mtu: u16, size: usize, tweak: impl FnOnce(&mut Sim)) -> (Sim, App, u16, u16, Vec<u8>) {
    let mut phone = PhoneSim::new(true);
    (phone.connect_at, phone.client_mtu) = (1000, client_mtu);
    let ride = body(size);
    let (mut sim, mut app) = super::tests::unit_with(None, |sim| {
        sim.fast_link = true;
        sim.nordic.phone = Some(phone);
        let mut d = Ram(sim.emmc.image.clone(), 0);
        put_file(&mut d, "Activities", "251028120000.fit", &ride);
        sim.emmc.image = d.0;
        tweak(sim);
    });
    pair_phone(&mut sim, &mut app);
    let (control, data) = app.phone.files.handles.expect("the service was added");
    sim.nordic.phone_subscribe(data + 1, true);
    run(&mut sim, &mut app, 200);
    sim.charge_wire = true;
    (sim, app, control, data, ride)
}

/// The radio as the phone link left it: back to the normal parameters, 1M PHY.
fn radio_normal(sim: &Sim) -> bool {
    let r = &sim.nordic.server.radio;
    !r.phy2 && r.interval_ms == super::ble_sim::HVN_MS && r.latency == 0
}

#[test]
fn a_400_kb_ride_downloads_in_seconds_on_the_fast_link() {
    let (mut sim, mut app, control, data, ride) = fast_unit(247, 400_000, |_| {});
    assert_eq!(app.ble.map(|v| v.mtu), Some(247));
    assert_eq!((app.phone.files.mtu, sim.nordic.server.radio.mtu), (247, 247), "the exchange answered with our max");
    assert_eq!(sim.nordic.server.radio.bad_replies, 0);
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 5, offset: 0, name: BIG }, 60_000);
    assert_eq!(a.end, Some((Status::Ok, ride.len() as u32)));
    assert!(a.bytes == ride, "the bytes equal the file on the card");
    assert_eq!(a.stray, 0);
    let rate = ride.len() as u64 * 1000 / a.ms;
    std::println!("fast link: {} B in {} ms = {} B/s ({} s)", ride.len(), a.ms, rate, a.ms / 1000);
    assert!(a.ms < 15_000 && rate >= 30_000, "{rate} B/s");
    run(&mut sim, &mut app, 300);
    assert!(radio_normal(&sim) && !app.phone.files.boosted, "the parameters came back with the END");
}

#[test]
fn without_the_fast_link_or_with_it_off_a_read_runs_as_before() {
    // the slow rate is the one the module measured (360 B/s)
    for off in [false, true] {
        let (mut sim, mut app, control, data, ride) = fast_unit(if off { 247 } else { 0 }, 3000, |s| s.fast_link = off);
        if off {
            app.phone.stored.fast = false; // the PHONE menu's row
        }
        let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 1, offset: 0, name: BIG }, 30_000);
        assert!(a.bytes == ride && a.end == Some((Status::Ok, 3000)), "off {off}");
        // the row switched off after the exchange: the MTU stays, the requests stop
        assert_eq!(app.phone.files.mtu, if off { 247 } else { 23 });
        let r = &sim.nordic.server.radio;
        assert!(r.dle == 27 && !r.phy2 && r.interval_ms == super::ble_sim::HVN_MS, "no request without the fast link");
        let rate = 3000 * 1000 / a.ms;
        std::println!("fast link off ({off}): {rate} B/s");
        assert!(a.stray == 0 && (if off { 1000..=60_000 } else { 250..=400 }).contains(&rate), "{rate} B/s");
    }
}

#[test]
fn a_chip_that_refuses_the_fast_configuration_keeps_the_phone_link_and_the_files() {
    let (mut sim, mut app, control, data, ride) = fast_unit(247, 3000, |s| s.nordic.refuse_fast_cfg = true);
    let v = app.ble.expect("the chip started on a smaller configuration");
    assert_eq!((v.mtu, v.central), (23, true), "the two links, the default MTU");
    assert_eq!((sim.nordic.server.radio.mtu, sim.nordic.server.radio.bad_replies), (23, 0), "answered 23, never more");
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 1, offset: 0, name: BIG }, 30_000);
    assert!(a.bytes == ride && a.end == Some((Status::Ok, 3000)));
    // refused outright (no configuration at all): the defaults, still no radio lost
    let (sim, app, ..) = fast_unit(247, 100, |s| s.nordic.refuse = Some(0x69));
    assert!(app.ble.is_some_and(|v| !v.central && v.mtu == 23) && sim.nordic.cfg.is_empty());
}

#[test]
fn the_link_parameters_are_asked_during_a_read_and_given_back_however_it_ends() {
    for how in ["cancel", "stall", "link drop", "new command"] {
        let (mut sim, mut app, control, data, _) = fast_unit(247, 400_000, |_| {});
        start_read(&mut sim, control, 1, BIG);
        pass(&mut sim, &mut app, 400);
        let r = &sim.nordic.server.radio;
        assert!(r.dle == 251 && r.phy2 && r.interval_ms == 15 && r.latency == 0, "{how}: asked for");
        assert!(app.phone.files.boosted && app.pass_ms() == 10, "{how}");
        match how {
            "cancel" => {
                sim.nordic.phone_write(control, &[3, 9]);
            }
            "stall" => sim.nordic.phone_subscribe(data + 1, false),
            "link drop" => {
                drop_link(&mut sim, u64::MAX);
            }
            _ => {
                sim.nordic.phone_write(control, &[1, 2]); // a LIST replaces the READ
            }
        }
        run(&mut sim, &mut app, if how == "stall" { 7000 } else { 1500 });
        if how == "link drop" {
            assert!(!app.phone.files.boosted && app.phone.files.mtu == 23, "{how}: forgotten with the link");
            continue;
        }
        let _ = drain(&mut sim, data);
        assert!(radio_normal(&sim) && !app.phone.files.boosted, "{how}: given back");
    }
}

#[test]
fn a_dropped_fast_transfer_resumes_byte_identical() {
    let (mut sim, mut app, control, data, ride) = fast_unit(247, 100_000, |_| {});
    start_read(&mut sim, control, 1, BIG);
    let (mut kept, _) = collect(&mut sim, &mut app, data, 10);
    assert!(!kept.is_empty() && kept.len() < ride.len(), "{} kept", kept.len());
    let back = sim.ms + 500;
    drop_link(&mut sim, back);
    run(&mut sim, &mut app, 300);
    assert!(!app.phone.files.active() && app.phone.files.mtu == 23);
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    assert_eq!(app.phone.files.mtu, 247, "the new link exchanged again");
    resubscribe(&mut sim, &mut app, data);
    let off = kept.len() as u32;
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 2, offset: off, name: BIG }, 30_000);
    kept.extend(&a.bytes);
    assert!(kept == ride, "{} of {} bytes", kept.len(), ride.len());
}

#[test]
fn the_media_keys_behave_on_the_fast_link_and_ask_nothing_of_the_radio() {
    let (mut sim, mut app, ..) = fast_unit(247, 100, |_| {});
    app.media(&mut sim, crate::phone::Media::Next, false);
    run(&mut sim, &mut app, 500);
    let ph = sim.nordic.phone.as_ref().unwrap();
    assert!(!ph.commands.is_empty() || !ph.reports.is_empty(), "the key reached the phone");
    let r = &sim.nordic.server.radio;
    assert!(r.dle == 27 && r.interval_ms == super::ble_sim::HVN_MS && !r.phy2, "no parameters outside a transfer");
}

/// `dev.py info`'s phone link numbers after a READ: what the stack reported and what the READ moved.
#[test]
fn the_link_info_says_what_the_link_has_and_what_the_last_read_moved() {
    let (mut sim, mut app, control, data, ride) = fast_unit(247, 100_000, |_| {});
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 5, offset: 0, name: BIG }, 60_000);
    assert_eq!(a.end, Some((Status::Ok, ride.len() as u32)));
    let (mtu, ext, l) = app.phone_link();
    assert!(ext && mtu == 247, "the extension was accepted, the MTU is the phone's");
    assert_eq!((l.dle, l.boost), (251, [0, 0, 0]), "data length in force, the three boost calls accepted");
    assert_eq!(l.bytes, ride.len() as u32, "the READ's file bytes");
    assert!(l.ms * 2 > a.ms as u32 && l.ms < a.ms as u32 * 2, "{} ms against {}", l.ms, a.ms);
    assert!(
        l.sent > 0 && l.hvn > 0 && l.hvn <= l.sent,
        "{} frames, {} confirmed, sim completed {}",
        l.sent,
        l.hvn,
        sim.nordic.server.radio.completed
    );
    assert!(l.interval > 0, "the interval came from the stack's update event");
    // a lost link clears what the link had, not the last READ
    app.phone.files.link_down();
    let (_, _, l2) = app.phone_link();
    assert_eq!((l2.interval, l2.phy, l2.dle, l2.bytes), (0, 0, 0, l.bytes));
}

/// The simulator's radio is no longer optimistic: at data length 27 a 244-byte notification is ten packets (over 6 ms
/// at 1M); at 251 on the 2M PHY it is one packet of about 1.4 ms.
#[test]
fn the_radio_model_counts_packets() {
    use super::ble_sim::Radio;
    let mut r = Radio { mtu: 247, ..Radio::default() };
    let (us, n) = r.pdu_us(244);
    assert_eq!(n, 27);
    assert!(251u64.div_ceil(27) * us > 6_000, "ten packets: {us} us each");
    (r.dle, r.phy2) = (251, true);
    let (us, n) = r.pdu_us(244);
    assert_eq!(n, 251);
    assert!((1_300..1_500).contains(&us), "{us} us");
}

#[test]
fn a_fuzz_of_the_new_events_leaves_the_link_whole() {
    let mut reached = 0;
    for seed in 1..=12u32 {
        let (mut sim, mut app, control, data, ride) = fast_unit(247, 20_000, |_| {});
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        start_read(&mut sim, control, 1, BIG);
        let mut got = Vec::new();
        for _ in 0..200 {
            // MTU requests (any size, cut short), TX completes (any count), PHY and data length results, junk
            let id = [0x55, 0x57, 0x21, 0x23, 0x22, 0x24][(next() % 6) as usize];
            let mut junk: Vec<u8> = (0..10 + next() % 6).map(|_| next() as u8).collect();
            junk.splice(0..0, [u8::from(next() % 4 == 0), 0]);
            if next() % 3 > 0 {
                sim.nordic.evt(id, &junk);
                reached += 1;
            }
            pass(&mut sim, &mut app, 50);
            for p in drain(&mut sim, data) {
                if let Some(Frame::Chunk { data, .. }) = Frame::parse(&p) {
                    got.extend(data);
                }
            }
            let m = app.phone.files.mtu;
            assert!((23..=247).contains(&m), "seed {seed}: mtu {m}");
        }
        assert!(ride.starts_with(&got) || got.len() > ride.len(), "seed {seed}: bytes in order, {} got", got.len());
    }
    assert!(reached > 1000, "the fuzz reached the events: {reached}");
}

#[test]
fn a_restore_the_chip_refuses_is_tried_again_and_the_phones_own_parameters_come_back() {
    let (mut sim, mut app, control, _, _) = fast_unit(247, 400_000, |_| {});
    // the phone changed its parameters before the READ (to 50 ms): those are what comes back
    sim.nordic.evt(0x12, &[0, 0, 40, 0, 40, 0, 0, 0, 0x90, 1]);
    run(&mut sim, &mut app, 100);
    start_read(&mut sim, control, 1, BIG);
    pass(&mut sim, &mut app, 400);
    assert!(app.phone.files.boosted && sim.nordic.server.radio.interval_ms == 15);
    sim.nordic.refuse = Some(0x75); // the restore meets a BUSY
    sim.nordic.phone_write(control, &[3, 9]);
    run(&mut sim, &mut app, 400);
    assert!(app.phone.files.boosted && sim.nordic.server.radio.interval_ms == 15, "kept trying");
    sim.nordic.refuse = None;
    run(&mut sim, &mut app, 200);
    assert!(!app.phone.files.boosted, "done once the chip took it");
    let r = &sim.nordic.server.radio;
    assert!(
        !r.phy2 && r.interval_ms == 50 && r.latency == 0,
        "the phone's 50 ms, 1M PHY: {r:?}",
        r = (r.phy2, r.interval_ms)
    );
}

#[test]
fn a_restore_that_never_works_is_dropped_after_a_bounded_number_of_passes() {
    let (mut sim, mut app, control, _, _) = fast_unit(247, 400_000, |_| {});
    start_read(&mut sim, control, 1, BIG);
    pass(&mut sim, &mut app, 400);
    sim.nordic.refuse = Some(0x8f); // the PHY update is always refused
    sim.nordic.phone_write(control, &[3, 9]);
    run(&mut sim, &mut app, 3000);
    assert!(!app.phone.files.boosted, "20 tries, then given up");
}

#[test]
fn with_no_known_parameters_no_update_is_sent_back() {
    let (mut sim, mut app, control, _, _) = fast_unit(247, 400_000, |_| {});
    app.phone.files.link = None; // the connect event was lost
    start_read(&mut sim, control, 1, BIG);
    pass(&mut sim, &mut app, 400);
    let before = sim.nordic.param_updates.len();
    sim.nordic.phone_write(control, &[3, 9]);
    run(&mut sim, &mut app, 400);
    assert!(!app.phone.files.boosted && !sim.nordic.server.radio.phy2);
    assert_eq!(sim.nordic.param_updates.len(), before, "the link keeps whatever the phone set");
}

#[test]
fn a_table_that_refuses_the_big_data_value_gets_the_small_one_and_small_frames() {
    let (mut sim, mut app, control, data, ride) = fast_unit(247, 3000, |s| s.nordic.max_value = 20);
    assert_eq!(app.phone.files.max_frame, 20);
    let a = ask(&mut sim, &mut app, control, data, Command::Read { seq: 1, offset: 0, name: BIG }, 30_000);
    assert!(a.bytes == ride && a.end == Some((Status::Ok, 3000)));
    assert!(sim.nordic.server.sent.iter().all(|s| s.1.len() <= 20));
}

/// The READ's rate with the chip's bytes waking the sleep and without (the pacing before 2026-11-07). The simulator's
/// chip sends up to the queue's depth each event, so it cannot show the unit's 16.6 KB/s (1.6 notifications an event);
/// what it checks is that the refill at once never costs rate and keeps the stack's queue fuller.
#[test]
fn a_byte_from_the_chip_ends_the_sleep_so_the_queue_is_refilled_at_once() {
    let rate = |wake: bool| {
        let (mut sim, mut app, control, data, _) = fast_unit(247, 400_000, |_| {});
        start_read(&mut sim, control, 5, BIG);
        let (t0, mut done) = (sim.ms, false);
        while !done && sim.ms < t0 + 60_000 {
            pass_with(&mut sim, &mut app, 50, wake);
            done = drain(&mut sim, data).iter().any(|p| matches!(Frame::parse(p), Some(Frame::End { .. })));
        }
        assert!(done, "the READ ended");
        let l = app.phone_link().2;
        (u64::from(l.bytes) * 1000 / (sim.ms - t0), l)
    };
    let ((old, lo), (new, ln)) = (rate(false), rate(true));
    std::println!(
        "READ, events read at the pass: {old} B/s (full {}, turns {}); woken by the chip: {new} B/s (full {}, turns {})",
        lo.full,
        lo.turns,
        ln.full,
        ln.turns
    );
    assert!(new >= old && new >= 30_000, "{new} against {old} B/s");
}

#[test]
fn during_a_fast_read_the_keys_stay_off_and_the_chips_bytes_wake_the_loop() {
    let (mut sim, mut app, control, _data, _) = fast_unit(247, 400_000, |_| {});
    start_read(&mut sim, control, 5, BIG);
    let mut main = crate::MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    let (end, mut radio) = (sim.ms + 3_000, 0);
    while sim.ms < end && !sim.standby && sim.main.resets == 0 {
        radio += main.pass(&mut app, &mut sim);
    }
    let wakes = &sim.main.wakes;
    assert!(!wakes.is_empty(), "the loop ran");
    assert!(wakes.iter().all(|&(keys, _)| !keys), "a key line armed during a READ");
    assert!(wakes.iter().any(|&(_, bytes)| bytes), "the READ's sleep never armed the chip's bytes");
    assert!(radio > 0, "no radio pass woken by a byte during the READ");
}

pub(super) type End = Option<(Status, u32)>;

/// LIST_IN of the workout folder: (key, size) of every file, and the answer's END.
pub(super) fn list_workouts(sim: &mut Sim, app: &mut App, control: u16, data: u16, seq: u8) -> (Vec<(Key, u32)>, End) {
    let mut f = [0; 20];
    let n = Command::ListIn { seq, folder: Folder::Workout }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    let (mut entries, mut end) = (Vec::new(), None);
    for _ in 0..100 {
        pass(sim, app, 50);
        for p in drain(sim, data) {
            match Frame::parse(&p) {
                Some(Frame::EntryIn { key, size, .. }) => entries.push((key, size)),
                Some(Frame::End { status, count, .. }) => end = Some((status, count)),
                _ => {}
            }
        }
        if end.is_some() {
            break;
        }
    }
    (entries, end)
}

#[test]
fn workouts_are_listed_and_read_whole_or_from_an_offset() {
    let (mut sim, mut app, control, data) = paired();
    let (entries, end) = list_workouts(&mut sim, &mut app, control, data, 3);
    // the synthetic card has four planned files, one per day folder, the past and the broken one too
    let keys: Vec<_> = entries.iter().map(|e| std::string::String::from_utf8(e.0.bytes().to_vec()).unwrap()).collect();
    assert_eq!(end, Some((Status::Ok, 4)));
    assert_eq!(keys, ["2026102000", "2026102300", "2026102400", "2026102500"]);
    let mut parsed = 0;
    for (key, size) in entries {
        let a = ask(
            &mut sim,
            &mut app,
            control,
            data,
            Command::ReadIn { seq: 4, folder: Folder::Workout, offset: 0, key },
            20_000,
        );
        assert_eq!(a.end, Some((Status::Ok, size)), "{key:?}");
        assert_eq!(a.bytes.len() as u32, size);
        parsed += usize::from(crate::storage::workout_file::parse(&a.bytes).is_ok());
        let a2 = ask(
            &mut sim,
            &mut app,
            control,
            data,
            Command::ReadIn { seq: 5, folder: Folder::Workout, offset: 10, key },
            20_000,
        );
        assert_eq!(a2.bytes, a.bytes[10..]);
    }
    assert_eq!(parsed, 3, "all but the broken one decode with the firmware's own parser");
}

#[test]
fn only_the_table_of_folders_is_reachable() {
    let (mut sim, mut app, control, data) = paired();
    for raw in [
        &b"\x04\x01\x09"[..],
        b"\x04\x01",
        b"\x05\x01\x01\0\0\0\0\x0a../../ble.b",
        b"\x05\x01\x01\0\0\0\0\x0a2026102x00",
        b"\x05\x01\x01\0\0\0\0\x0c251026093000",
        b"\x05\x01\x07\0\0\0\0\x0a2026102300",
        b"\x05\x01\x01\0\0\0\0\x0c2026102300",
    ] {
        let a = ask_raw(&mut sim, &mut app, control, data, raw, 2000);
        assert_eq!(a.end.map(|e| e.0), Some(Status::BadRequest), "{raw:?}");
        assert!(a.bytes.is_empty() && a.entries.is_empty());
    }
    let (listed, _) = list_workouts(&mut sim, &mut app, control, data, 9);
    let key = |k: &[u8]| Key::new(Folder::Workout, k).unwrap();
    for (k, want) in [(&b"2026102301"[..], Status::NotFound), (b"2027010100", Status::NotFound)] {
        let a = ask(
            &mut sim,
            &mut app,
            control,
            data,
            Command::ReadIn { seq: 2, folder: Folder::Workout, offset: 0, key: key(k) },
            2000,
        );
        assert_eq!(a.end.map(|e| e.0), Some(want), "{k:?}");
    }
    let a = ask(
        &mut sim,
        &mut app,
        control,
        data,
        Command::ReadIn { seq: 2, folder: Folder::Workout, offset: 99_999, key: listed[1].0 },
        2000,
    );
    assert_eq!(a.end.map(|e| e.0), Some(Status::BadRequest), "an offset past the end");
}

#[test]
fn workouts_are_busy_during_a_ride() {
    let (mut sim, mut app, control, data) = paired();
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    run(&mut sim, &mut app, 300);
    let key = Key::new(Folder::Workout, b"2026102300").unwrap();
    for cmd in [
        Command::ListIn { seq: 6, folder: Folder::Workout },
        Command::ReadIn { seq: 7, folder: Folder::Workout, offset: 0, key },
    ] {
        let a = ask(&mut sim, &mut app, control, data, cmd, 5000);
        assert_eq!(a.end, Some((Status::Busy, 0)), "{cmd:?} during a ride");
        assert!(a.bytes.is_empty());
    }
}

/// The image with "Short Intervals.fit" of 2026-10-23 removed (if it is there), and `new` (if any) written in its place as NEW.FIT.
pub(super) fn replace_workout(image: &[u8], new: Option<&[u8]>) -> Vec<u8> {
    let mut d = Ram(image.to_vec(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let dir = fat.open(&mut d, "Workout/calendar/20261023").unwrap();
    if let Ok(old) = fat.locate(&mut d, dir, "Short Intervals.fit") {
        fat.remove(&mut d, &old).unwrap();
    }
    if let Some(data) = new {
        let mut h = fat.create(&mut d, dir, "NEW.FIT", (0, 0)).unwrap();
        let mut padded = data.to_vec();
        padded.resize(data.len().div_ceil(512).max(1) * 512, 0);
        fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
        fat.set_size(&mut d, &mut h, data.len() as u32, (0, 0)).unwrap();
    }
    d.0
}

#[test]
fn a_folder_changed_between_list_and_read_never_serves_another_file() {
    let (mut sim, mut app, control, data) = paired();
    let (listed, _) = list_workouts(&mut sim, &mut app, control, data, 3);
    let (key, size) = listed[1]; // 2026102300: "Short Intervals.fit"
    let read = |sim: &mut Sim, app: &mut App, key| {
        ask(sim, app, control, data, Command::ReadIn { seq: 4, folder: Folder::Workout, offset: 0, key }, 20_000)
    };
    assert_eq!(read(&mut sim, &mut app, key).end, Some((Status::Ok, size)), "unchanged");
    // the file is removed: nothing is at its place
    sim.emmc.image = replace_workout(&sim.emmc.image, None);
    let a = read(&mut sim, &mut app, key);
    assert_eq!((a.end.map(|e| e.0), a.bytes.len()), (Some(Status::NotFound), 0), "removed");
    // another file of another size takes the place: still NOT FOUND, its bytes never sent under the old key
    sim.emmc.image = replace_workout(&sim.emmc.image, Some(&[7; 40]));
    let a = read(&mut sim, &mut app, key);
    assert_eq!((a.end.map(|e| e.0), a.bytes.len()), (Some(Status::NotFound), 0), "replaced");
    // a new listing gives the new check, and the file reads again
    let (relisted, _) = list_workouts(&mut sim, &mut app, control, data, 5);
    let (k2, s2) = relisted.into_iter().find(|e| e.0.bytes() == key.bytes()).expect("listed again");
    assert_ne!(k2.check(), key.check());
    assert_eq!(read(&mut sim, &mut app, k2).end, Some((Status::Ok, s2)));
}

/// The day the workouts are sent for.
pub(super) const DAY: [u8; 8] = *b"20261108";

/// A workout of 40 steps, over 1 KB: its data crosses sectors.
pub(super) fn long_workout() -> Vec<u8> {
    let steps: Vec<_> = (0..40u32).map(|i| (0, 60_000 + i, 4, 0, 100 + i, 110 + i, (i % 2) as u8)).collect();
    crate::storage::workout_file::tests::file(5, "Long one", &steps)
}

/// The END of a command whose answer is one frame.
fn answer(sim: &mut Sim, app: &mut App, control: u16, data: u16, raw: &[u8]) -> End {
    ask_raw(sim, app, control, data, raw, 2000).end
}

/// Sends `file` for [`DAY`] with `crc`, in WRITE_DATA frames of `chunk` bytes: WRITE_BEGIN's END, then the first
/// refusal of a WRITE_DATA or WRITE_END's END.
fn send_workout(sim: &mut Sim, app: &mut App, ends: (u16, u16), file: &[u8], crc: u32, chunk: usize) -> (End, End) {
    let (control, data) = ends;
    let mut f = [0; crate::filesvc::FRAME_BIG];
    let size = file.len() as u32;
    let n = Command::WriteBegin { seq: 1, folder: Folder::Workout, day: DAY, size, crc }.encode(&mut f);
    let begun = answer(sim, app, control, data, &f[..n]);
    if begun != Some((Status::Ok, 0)) {
        return (begun, None);
    }
    for (k, c) in file.chunks(chunk).enumerate() {
        let n = Command::WriteData { seq: 2, offset: (k * chunk) as u32, data: c }.encode(&mut f);
        assert_eq!(n, 6 + c.len());
        sim.nordic.phone_write(control, &f[..n]);
        pass(sim, app, 20);
        if let Some(Frame::End { status, count, .. }) = drain(sim, data).iter().find_map(|p| Frame::parse(p)) {
            return (begun, Some((status, count)));
        }
    }
    (begun, answer(sim, app, control, data, &[8, 3]))
}

/// The workouts of [`DAY`] on the card: their bytes.
pub(super) fn day_files(sim: &Sim) -> Vec<Vec<u8>> {
    crate::storage::workout_write::tests::day_files(&sim.emmc.image, "20261108").into_iter().map(|f| f.1).collect()
}

fn crc(b: &[u8]) -> u32 {
    crate::inflate::crc32(b.iter().copied())
}

#[test]
fn a_workout_sent_at_either_mtu_is_listed_read_back_whole_and_deleted() {
    let (mut sim, mut app, control, data) = paired();
    let lost0 = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    let file = long_workout();
    assert!(file.len() > 1024, "{} bytes", file.len());
    for (chunk, seq) in [(14, 20), (crate::filesvc::FRAME_BIG - 6, 30)] {
        let (begun, ended) = send_workout(&mut sim, &mut app, (control, data), &file, crc(&file), chunk);
        assert_eq!((begun, ended), (Some((Status::Ok, 0)), Some((Status::Ok, file.len() as u32))), "{chunk}");
        assert_eq!(day_files(&sim), core::slice::from_ref(&file), "{chunk}: once, the second the same file sent again");
        let (listed, end) = list_workouts(&mut sim, &mut app, control, data, seq);
        assert_eq!(end.map(|e| e.0), Some(Status::Ok));
        let (key, size) = *listed.iter().find(|e| e.0.bytes().starts_with(b"20261108")).expect("listed");
        assert_eq!(size, file.len() as u32);
        let read = Command::ReadIn { seq: seq + 1, folder: Folder::Workout, offset: 0, key };
        let a = ask(&mut sim, &mut app, control, data, read, 120_000); // 17.6 KB at 360 B/s
        assert_eq!(a.bytes, file, "byte-identical");
        assert_eq!(crate::storage::workout_file::parse(&a.bytes).unwrap().name(), "LONG ONE");
    }
    assert_eq!(
        crate::fat::write::tests::fsck(&sim.emmc.image),
        Ok(lost0),
        "as before: put_file over stock tmp.fit left one"
    );
    let (listed, _) = list_workouts(&mut sim, &mut app, control, data, 40);
    let key = listed.iter().find(|e| e.0.bytes().starts_with(b"20261108")).unwrap().0;
    // a stale check (another file there now) is NOT FOUND; the listed one deletes it
    let stale = Key::new(Folder::Workout, key.bytes()).unwrap().with_check(key.check() ^ 1);
    let mut f = [0; 20];
    let n = Command::Delete { seq: 41, folder: Folder::Workout, key: stale }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::NotFound, 0)));
    assert_eq!(day_files(&sim).len(), 1);
    let n = Command::Delete { seq: 42, folder: Folder::Workout, key }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Ok, 1)));
    assert!(day_files(&sim).is_empty());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(lost0), "its clusters freed");
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::NotFound, 0)), "gone");
}

#[test]
fn every_risky_write_is_refused_and_the_calendar_stays_as_it_was() {
    let (mut sim, mut app, control, data) = paired();
    let lost0 = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    let file = long_workout();
    let send = |sim: &mut Sim, app: &mut App, f: &[u8], c: u32| send_workout(sim, app, (control, data), f, c, 14);
    let before = sim.emmc.image.clone();
    // a build that may not write the card: READ ONLY, nothing written
    app.card_writable = false;
    assert_eq!(send(&mut sim, &mut app, &file, crc(&file)).0, Some((Status::ReadOnly, 0)));
    app.card_writable = true;
    assert!(sim.emmc.image == before, "nothing written");
    app.emmc.lend(true);
    assert_eq!(send(&mut sim, &mut app, &file, crc(&file)).0, Some((Status::Busy, 0)), "lent over USB");
    app.emmc.lend(false);
    // a CRC that is not the file's, a file that is no workout, one too big
    assert_eq!(send(&mut sim, &mut app, &file, crc(&file) ^ 1).1, Some((Status::Mismatch, file.len() as u32)));
    let junk = std::vec![0x55u8; 700];
    assert_eq!(send(&mut sim, &mut app, &junk, crc(&junk)).1, Some((Status::BadRequest, 700)));
    let big = std::vec![0u8; crate::storage::workout_file::FILE_MAX + 1];
    assert_eq!(send(&mut sim, &mut app, &big, crc(&big)).0, Some((Status::BadRequest, 0)));
    // the rides' folder, a day that is no date, data with no write begun, WRITE_END with none
    let raw: [&[u8]; 4] = [
        b"\x06\x05\x0020261108\x10\0\0\0\0\0\0\0",
        b"\x06\x05\x0120261399\x10\0\0\0\0\0\0\0",
        b"\x07\x05\0\0\0\0\x01",
        b"\x08\x05",
    ];
    for r in raw {
        assert_eq!(answer(&mut sim, &mut app, control, data, r), Some((Status::BadRequest, 0)), "{r:?}");
    }
    // after 10 bytes: out of order, past the size, a second WRITE_BEGIN: refused, the write dropped
    let mut f = [0; 64];
    let n = Command::WriteBegin { seq: 1, folder: Folder::Workout, day: DAY, size: 40, crc: 0 }.encode(&mut f);
    let begin = f[..n].to_vec();
    for bad in [
        Command::WriteData { seq: 6, offset: 11, data: &[1; 10] },
        Command::WriteData { seq: 6, offset: 10, data: &[1; 31] },
        Command::WriteBegin { seq: 6, folder: Folder::Workout, day: DAY, size: 40, crc: 0 },
    ] {
        assert_eq!(answer(&mut sim, &mut app, control, data, &begin), Some((Status::Ok, 0)));
        let n = Command::WriteData { seq: 6, offset: 0, data: &[1; 10] }.encode(&mut f);
        sim.nordic.phone_write(control, &f[..n]);
        let n = bad.encode(&mut f);
        assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::BadRequest, 10)), "{bad:?}");
        assert_eq!(answer(&mut sim, &mut app, control, data, b"\x08\x07"), Some((Status::BadRequest, 0)), "dropped");
    }
    // a write the phone stops feeding is dropped
    assert_eq!(answer(&mut sim, &mut app, control, data, &begin), Some((Status::Ok, 0)));
    run(&mut sim, &mut app, 6000);
    assert!(!app.phone.files.active(), "dropped after the stall");
    assert!(day_files(&sim).is_empty());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(lost0));
}

/// After a failed write (the FAT copies suspect), a DELETE makes the copies equal before it frees anything.
#[test]
fn a_delete_after_a_failed_write_syncs_the_fat_copies_first() {
    let (mut sim, mut app, control, data) = paired();
    let file = long_workout();
    let sent = send_workout(&mut sim, &mut app, (control, data), &file, crc(&file), 238);
    assert_eq!(sent.1, Some((Status::Ok, file.len() as u32)));
    let (listed, _) = list_workouts(&mut sim, &mut app, control, data, 7);
    let key = listed.iter().find(|e| e.0.bytes().starts_with(b"20261108")).unwrap().0;
    // the second FAT's last sector differs (a write cut between the copies), and the app knows a write failed
    let img = &mut sim.emmc.image;
    let u16_at = |i: usize| u32::from(u16::from_le_bytes([img[i], img[i + 1]]));
    let (reserved, size) = (u16_at(14), u32::from_le_bytes(img[36..40].try_into().unwrap()));
    let end = ((reserved + 2 * size) * 512) as usize;
    img[end - 1] ^= 0x01;
    assert_eq!(crate::fat::write::tests::fsck_cut(img).unwrap().apart, 1);
    app.fats_unsure();
    let mut f = [0; 20];
    let n = Command::Delete { seq: 8, folder: Folder::Workout, key }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Ok, 1)));
    assert_eq!(crate::fat::write::tests::fsck_cut(&sim.emmc.image).unwrap().apart, 0);
    assert!(day_files(&sim).is_empty());
}

/// A WRITE_BEGIN during a READ ends the READ with its END (BUSY, the bytes sent) before its own answer.
#[test]
fn a_write_begun_during_a_read_ends_the_read_first() {
    let (mut sim, mut app, control, data) = paired();
    let [(name, _), _] = rides();
    start_read(&mut sim, control, 5, name);
    pass(&mut sim, &mut app, 60);
    drain(&mut sim, data);
    let mut f = [0; 40];
    let n = Command::WriteBegin { seq: 6, folder: Folder::Workout, day: DAY, size: 40, crc: 0 }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    pass(&mut sim, &mut app, 200);
    let ends: Vec<_> = drain(&mut sim, data)
        .iter()
        .filter_map(|p| match Frame::parse(p) {
            Some(Frame::End { seq, status, .. }) => Some((seq, status)),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [(5, Status::Busy), (6, Status::Ok)]);
}

/// A refused write command during a LIST (a build without card writes) ends the LIST first too, and the refusal's
/// count is its own (0), never the list's.
#[test]
fn a_refused_write_during_a_list_ends_the_list_first() {
    let (mut sim, mut app, control, data) = paired();
    app.card_writable = false;
    sim.nordic.phone_write(control, &[1, 5]); // LIST
    let mut f = [0; 40];
    let n = Command::WriteBegin { seq: 6, folder: Folder::Workout, day: DAY, size: 40, crc: 0 }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    pass(&mut sim, &mut app, 200);
    let ends: Vec<_> = drain(&mut sim, data)
        .iter()
        .filter_map(|p| match Frame::parse(p) {
            Some(Frame::End { seq, status, count }) => Some((seq, status, count)),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 2, "{ends:?}");
    assert_eq!((ends[0].0, ends[0].1), (5, Status::Busy));
    assert_eq!(ends[1], (6, Status::ReadOnly, 0));
}

#[test]
fn a_ride_starting_mid_write_drops_it_and_nothing_shows() {
    let (mut sim, mut app, control, data) = paired();
    let file = long_workout();
    let mut f = [0; 40];
    let (size, c) = (file.len() as u32, crc(&file));
    let n = Command::WriteBegin { seq: 1, folder: Folder::Workout, day: DAY, size, crc: c }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Ok, 0)));
    let n = Command::WriteData { seq: 2, offset: 0, data: &file[..14] }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    run(&mut sim, &mut app, 100);
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    let (_, end) = collect(&mut sim, &mut app, data, 10);
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(end, Some((Status::Busy, 14)), "dropped, the bytes received said");
    assert_eq!(answer(&mut sim, &mut app, control, data, b"\x08\x03"), Some((Status::Busy, 0)));
    let n = Command::WriteBegin { seq: 1, folder: Folder::Workout, day: DAY, size: 9, crc: 0 }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Busy, 0)), "during the ride");
    assert!(day_files(&sim).is_empty());
}

/// The power cut after every sector a sent workout writes, on the simulated card: the calendar as it was or the whole
/// workout, fsck passes (a lost chain at most), and with the power back the phone sends it again, nothing more lost.
#[test]
fn a_power_cut_at_every_write_of_a_sent_workout_leaves_the_calendar_whole() {
    let file = long_workout();
    let (mut sim, mut app, control, data) = paired();
    let lost0 = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    let w0 = sim.emmc.sectors_written;
    let sent = send_workout(&mut sim, &mut app, (control, data), &file, crc(&file), 238);
    assert_eq!(sent.1, Some((Status::Ok, file.len() as u32)));
    let writes = sim.emmc.sectors_written - w0;
    let mut none = 0;
    for cut in 0..writes {
        let (mut sim, mut app, ..) = paired();
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + cut);
        let _ = send_workout(&mut sim, &mut app, (control, data), &file, crc(&file), 238);
        let scan = crate::fat::write::tests::fsck_cut(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(scan.lost <= lost0 + 1 && scan.past.is_empty(), "cut {cut}: {} lost {:?}", scan.lost, scan.past);
        match day_files(&sim).as_slice() {
            [] => none += 1,
            [f] => assert!(*f == file, "cut {cut}: a part of it"),
            other => panic!("cut {cut}: {} files", other.len()),
        }
        // the unit started again on that card, the phone sends it again
        let image = core::mem::take(&mut sim.emmc.image);
        let (mut sim, mut app, ..) = paired();
        sim.emmc.image = image;
        let again = send_workout(&mut sim, &mut app, (control, data), &file, crc(&file), 238);
        assert_eq!(again.1, Some((Status::Ok, file.len() as u32)), "cut {cut}: {again:?}");
        assert_eq!(day_files(&sim), core::slice::from_ref(&file), "cut {cut}");
        assert_eq!(crate::fat::write::tests::fsck_cut(&sim.emmc.image).map(|s| s.lost), Ok(scan.lost), "cut {cut}");
    }
    std::println!("{writes} sectors: {none} cuts left the calendar as it was");
    assert!(none > 0 && writes > 10);
}

/// The zigzag of the synthetic card, a FIT course with course points (several sectors).
pub(super) fn turns() -> Vec<u8> {
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    crate::fat::tests::read_all(&mut fat, &mut d, "Courses/Turns.fit").unwrap()
}

/// Sends course `file` as `name` with `crc`, in WRITE_DATA frames of `chunk` bytes: WRITE_BEGIN's END, then the
/// first refusal of a WRITE_DATA or WRITE_END's END.
pub(super) fn send_course(
    sim: &mut Sim,
    app: &mut App,
    ends: (u16, u16),
    (name, file, crc): (&[u8], &[u8], u32),
    chunk: usize,
) -> (End, End) {
    let (control, data) = ends;
    let mut f = [0; crate::filesvc::FRAME_BIG];
    let n = match crate::filesvc::CourseName::new(name) {
        Some(name) => Command::WriteCourse { seq: 1, size: file.len() as u32, crc, name }.encode(&mut f),
        None => {
            // a name the codec refuses, as a phone could still write it
            f[..12].copy_from_slice(&[6, 1, 2, 1, 0, 0, 0, 0, 0, 0, 0, name.len() as u8]);
            f[12..12 + name.len()].copy_from_slice(name);
            12 + name.len()
        }
    };
    let begun = answer(sim, app, control, data, &f[..n]);
    if begun != Some((Status::Ok, 0)) {
        return (begun, None);
    }
    for (k, c) in file.chunks(chunk).enumerate() {
        let n = Command::WriteData { seq: 2, offset: (k * chunk) as u32, data: c }.encode(&mut f);
        sim.nordic.phone_write(control, &f[..n]);
        pass(sim, app, 20);
        if let Some(Frame::End { status, count, .. }) = drain(sim, data).iter().find_map(|p| Frame::parse(p)) {
            return (begun, Some((status, count)));
        }
    }
    (begun, answer(sim, app, control, data, &[8, 3]))
}

/// LIST_IN of `\Courses`: (key, size) of every file, and the END.
pub(super) fn list_courses(sim: &mut Sim, app: &mut App, ends: (u16, u16), seq: u8) -> (Vec<(Key, u32)>, End) {
    let (control, data) = ends;
    sim.nordic.phone_write(control, &[4, seq, Folder::Courses as u8]);
    let (mut entries, mut end) = (Vec::new(), None);
    for _ in 0..400 {
        pass(sim, app, 50);
        for p in drain(sim, data) {
            match Frame::parse(&p) {
                Some(Frame::EntryIn { key, size, .. }) => entries.push((key, size)),
                Some(Frame::End { status, count, .. }) => end = Some((status, count)),
                _ => {}
            }
        }
        if end.is_some() {
            break;
        }
    }
    (entries, end)
}

/// The listed key of course file `file_name` of `size` bytes: the one whose check is its name's.
fn course_key(listed: &[(Key, u32)], file_name: &str, size: u32) -> Option<Key> {
    let check = crate::filesvc::name_check(file_name.as_bytes(), size);
    listed.iter().find(|e| e.0.check() == check && e.1 == size).map(|e| e.0)
}

/// `\Courses\<file_name>` on the card, if it is there.
fn course_on_card(sim: &Sim, file_name: &str) -> Option<Vec<u8>> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    crate::fat::tests::read_all(&mut fat, &mut d, &std::format!("Courses/{file_name}")).ok()
}

/// A route sent from the phone at either MTU (an 8-character name in 20-byte writes, a 20-character one in 244) is
/// written as `<name>.fit`, listed by LIST_IN 2 and by the unit's COURSES, read back byte-identical and followed;
/// while followed (or being read to follow) its DELETE is BUSY, after that it is deleted and its clusters freed.
#[test]
fn a_course_sent_at_either_mtu_is_listed_read_back_followed_and_deleted() {
    let (mut sim, mut app, control, data) = paired();
    app.set_course_memory(crate::storage::course_file::tests::memory());
    let lost0 = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    let file = turns();
    let size = file.len() as u32;
    assert!(file.len() > 1024, "{size} bytes");
    for (name, chunk) in [(&b"Zig zag2"[..], 14), (b"Zig zag from phone", crate::filesvc::FRAME_BIG - 6)] {
        let fname = std::format!("{}.fit", std::str::from_utf8(name).unwrap());
        for _ in 0..2 {
            // the second: the same course sent again (its answer lost), written once
            let sent = send_course(&mut sim, &mut app, (control, data), (name, &file, crc(&file)), chunk);
            assert_eq!(sent, (Some((Status::Ok, 0)), Some((Status::Ok, size))), "{fname}");
        }
        assert_eq!(course_on_card(&sim, &fname).as_deref(), Some(&file[..]), "{fname}");
        let (listed, end) = list_courses(&mut sim, &mut app, (control, data), 20);
        assert_eq!(end, Some((Status::Ok, listed.len() as u32)));
        let key = course_key(&listed, &fname, size).expect("listed with its name's check");
        let read = Command::ReadIn { seq: 21, folder: Folder::Courses, offset: 0, key };
        let a = ask(&mut sim, &mut app, control, data, read, 120_000); // 17.6 KB at 360 B/s
        assert_eq!((a.end, a.bytes.len()), (Some((Status::Ok, size)), file.len()), "{fname}");
        assert!(a.bytes == file, "byte-identical");
    }
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(lost0));
    // the unit lists it in COURSES and follows it
    app.open_courses(&mut sim);
    run(&mut sim, &mut app, 1000);
    let list = &app.courses.as_deref().unwrap().list;
    let k = (0..list.len()).find(|&i| list.get(i).unwrap().name() == "ZIG ZAG2").expect("in COURSES");
    app.follow_course(k);
    assert!(app.courses.as_deref().unwrap().loading());
    let (listed, _) = list_courses(&mut sim, &mut app, (control, data), 30);
    let key = course_key(&listed, "Zig zag2.fit", size).unwrap();
    let mut f = [0; 20];
    let n = Command::Delete { seq: 31, folder: Folder::Courses, key }.encode(&mut f);
    // a DELETE in the same pass as the read to follow begins: BUSY
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Busy, 0)), "being read");
    run(&mut sim, &mut app, 500);
    assert!(app.courses.as_deref().unwrap().on.is_some(), "followed");
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Busy, 0)), "followed");
    assert!(course_on_card(&sim, "Zig zag2.fit").is_some());
    // the other one goes, and an open COURSES lists the folder again
    let other = course_key(&listed, "Zig zag from phone.fit", size).unwrap();
    app.open_courses(&mut sim);
    let mut g = [0; 20];
    let n2 = Command::Delete { seq: 32, folder: Folder::Courses, key: other }.encode(&mut g);
    assert_eq!(answer(&mut sim, &mut app, control, data, &g[..n2]), Some((Status::Ok, 1)));
    let list = &app.courses.as_deref().unwrap().list;
    assert!((0..list.len()).all(|i| list.get(i).unwrap().name() != "ZIG ZAG FROM PHONE"), "listed again");
    app.stop_course();
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Ok, 1)), "no longer followed");
    assert!(course_on_card(&sim, "Zig zag2.fit").is_none() && course_on_card(&sim, "Zig zag from phone.fit").is_none());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(lost0), "their clusters freed");
}

/// Every refusal of a course write leaves `\Courses` as it was: READ ONLY, BUSY (lent, a ride), a name that leaves
/// the folder or is no ASCII, too big, no FIT course, a CRC that lies, a name taken by another file, no `\Courses`.
#[test]
fn every_risky_course_write_is_refused_and_the_folder_stays_as_it_was() {
    let (mut sim, mut app, control, data) = paired();
    let lost0 = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    let file = turns();
    let c = crc(&file);
    let send = |sim: &mut Sim, app: &mut App, n: &[u8], f: &[u8], c: u32| {
        send_course(sim, app, (control, data), (n, f, c), 238)
    };
    let before = sim.emmc.image.clone();
    app.card_writable = false;
    assert_eq!(send(&mut sim, &mut app, b"New", &file, c).0, Some((Status::ReadOnly, 0)));
    app.card_writable = true;
    app.emmc.lend(true);
    assert_eq!(send(&mut sim, &mut app, b"New", &file, c).0, Some((Status::Busy, 0)), "lent over USB");
    app.emmc.lend(false);
    for bad in [&b"../New"[..], b"New.gpx", b"N\0w", b"Caf\xc3\xa9", b"CON", b" New", b"123456789012345678901"] {
        assert_eq!(send(&mut sim, &mut app, bad, &file, c).0, Some((Status::BadRequest, 0)), "{bad:?}");
    }
    let big = std::vec![0u8; crate::storage::workout_write::COURSE_MAX as usize + 1];
    assert_eq!(send(&mut sim, &mut app, b"Big", &big, crc(&big)).0, Some((Status::BadRequest, 0)));
    assert!(sim.emmc.image == before, "nothing written");
    let size = file.len() as u32;
    assert_eq!(send(&mut sim, &mut app, b"New", &file, c ^ 1).1, Some((Status::Mismatch, size)));
    let workout = long_workout();
    let wsize = workout.len() as u32;
    assert_eq!(send(&mut sim, &mut app, b"New", &workout, crc(&workout)).1, Some((Status::BadRequest, wsize)));
    // a name the card has for another course (any case), or one COURSES shows for another: refused at WRITE_BEGIN
    for n in [&b"TURNS"[..], b"zigzag", b"Hill Garmin"] {
        assert_eq!(send(&mut sim, &mut app, n, &workout, crc(&workout)).0, Some((Status::BadRequest, 0)), "{n:?}");
    }
    assert_eq!(course_on_card(&sim, "Turns.fit"), Some(file.clone()), "kept");
    assert_eq!(course_on_card(&sim, "New.fit"), None);
    // a ride: BUSY
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    run(&mut sim, &mut app, 300);
    assert_eq!(send(&mut sim, &mut app, b"New", &file, c).0, Some((Status::Busy, 0)), "during a ride");
    let (listed, end) = list_courses(&mut sim, &mut app, (control, data), 9);
    assert_eq!((listed.len(), end), (0, Some((Status::Busy, 0))));
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(lost0));
    // a card with no \Courses: an empty listing, and the first course sent makes the folder
    let (mut sim, mut app, control, data) = paired();
    let at = sim.emmc.image.windows(11).position(|w| w == b"COURSES    ").expect("the folder's 8.3 entry");
    sim.emmc.image[at + 6] = b'X';
    assert_eq!(list_courses(&mut sim, &mut app, (control, data), 4), (Vec::new(), Some((Status::Ok, 0))));
    let sent = send_course(&mut sim, &mut app, (control, data), (b"New", &file, c), 238);
    assert_eq!(sent, (Some((Status::Ok, 0)), Some((Status::Ok, size))));
    assert_eq!(course_on_card(&sim, "New.fit"), Some(file));
    assert_eq!(list_courses(&mut sim, &mut app, (control, data), 5).0.len(), 1);
}

/// Sends a profile command; the PROFILE frame and the END that answered.
fn profile_ask(sim: &mut Sim, app: &mut App, ends: (u16, u16), cmd: Command) -> (Option<crate::filesvc::Profile>, End) {
    let mut f = [0; 20];
    let n = cmd.encode(&mut f);
    let (seq, mut got, mut end) = (f[1], None, None);
    sim.nordic.phone_write(ends.0, &f[..n]);
    for _ in 0..20 {
        pass(sim, app, 50);
        for p in drain(sim, ends.1) {
            match Frame::parse(&p) {
                Some(Frame::Profile { seq: s, profile }) if s == seq => got = Some(profile),
                Some(Frame::End { seq: s, status, count }) if s == seq => end = Some((status, count)),
                _ => {}
            }
        }
    }
    (got, end)
}

#[test]
fn the_profile_is_read_set_and_kept_after_a_restart() {
    let (mut sim, mut app, control, data) = paired();
    let ends = (control, data);
    let (p, _) = profile_ask(&mut sim, &mut app, ends, Command::ProfileGet { seq: 1 });
    let mut p = p.expect("a PROFILE frame");
    let s = app.settings;
    assert_eq!((u16::from(p.max_hr), p.ftp, u16::from(p.weight_kg)), (s.max_hr, s.ftp, s.weight_kg));
    // the values held, sent back: OK, nothing written
    let before = sim.emmc.image.clone();
    let set = |p| Command::ProfileSet { seq: 2, profile: p };
    assert_eq!(profile_ask(&mut sim, &mut app, ends, set(p)).1, Some((Status::Ok, 0)));
    assert!(sim.emmc.image == before && !app.settings_store.file.dirty, "as held: no write");
    (p.ftp, p.max_hr, p.weight_kg, p.zones_lthr) = (283, 181, 71, true);
    assert_eq!(profile_ask(&mut sim, &mut app, ends, set(p)).1, Some((Status::Ok, 1)));
    assert_eq!((app.settings.ftp, app.settings.max_hr, app.settings.weight_kg), (283, 181, 71));
    assert!(!app.settings.max_hr_age);
    assert_eq!(profile_ask(&mut sim, &mut app, ends, Command::ProfileGet { seq: 3 }).0, Some(p), "read back");
    for _ in 0..200 {
        if !app.settings_store.file.dirty {
            break;
        }
        run(&mut sim, &mut app, 200);
    }
    let (_, app2) = super::page_tests::reboot(&mut sim);
    assert_eq!((app2.settings.ftp, app2.settings.max_hr, app2.settings.weight_kg), (283, 181, 71), "after a restart");
    assert!(app2.settings.zones_lthr);
}

#[test]
fn every_risky_profile_set_is_refused_and_a_get_always_works() {
    use crate::filesvc::Profile;
    let (mut sim, mut app, control, data) = paired();
    let ends = (control, data);
    let ok = profile_ask(&mut sim, &mut app, ends, Command::ProfileGet { seq: 1 }).0.unwrap();
    let ftp = app.settings.ftp;
    let set =
        |sim: &mut Sim, app: &mut App, profile| profile_ask(sim, app, ends, Command::ProfileSet { seq: 2, profile }).1;
    for p in [
        Profile { ftp: 49, ..ok },
        Profile { ftp: 601, ..ok },
        Profile { max_hr: 99, ..ok },
        Profile { weight_kg: 29, ..ok },
        Profile { wprime_j: 4000, ..ok },
        Profile { lthr: 231, ..ok },
    ] {
        assert_eq!(set(&mut sim, &mut app, p), Some((Status::BadRequest, 0)), "{p:?}");
    }
    let want = Profile { ftp: 300, ..ok };
    app.card_writable = false;
    assert_eq!(set(&mut sim, &mut app, want), Some((Status::ReadOnly, 0)));
    app.card_writable = true;
    app.emmc.lend(true);
    assert_eq!(set(&mut sim, &mut app, want), Some((Status::Busy, 0)), "lent");
    assert!(profile_ask(&mut sim, &mut app, ends, Command::ProfileGet { seq: 3 }).0.is_some(), "a GET while lent");
    app.emmc.lend(false);
    app.menu.editing = true;
    assert_eq!(set(&mut sim, &mut app, want), Some((Status::Busy, 0)), "a setting is being edited");
    app.menu.editing = false;
    app.settings.lthr = 160;
    assert_eq!(set(&mut sim, &mut app, Profile { lthr: 0, ..ok }), Some((Status::Ok, 1)));
    assert_eq!(app.settings.lthr, 0, "LTHR 0 is unset");
    app.settings.lthr = ok.lthr.into();
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    run(&mut sim, &mut app, 300);
    assert_eq!(set(&mut sim, &mut app, ok), Some((Status::Ok, 0)), "the held values in a ride: no error");
    assert_eq!(set(&mut sim, &mut app, want), Some((Status::Busy, 0)), "during a ride");
    assert_eq!(profile_ask(&mut sim, &mut app, ends, Command::ProfileGet { seq: 4 }).0, Some(ok), "a GET in a ride");
    assert_eq!(app.settings.ftp, ftp, "nothing changed");
}

/// A full `\Courses`: only its `.fit` and `.gpx` files are listed (a folder and a text file left out), 100 of them,
/// numbered from 00 to 99 in directory order; a key past 9 reads and deletes its own file.
#[test]
fn courses_are_numbered_in_two_digits_and_only_course_files_count() {
    let (mut sim, mut app, control, data) = paired();
    let mut d = Ram(sim.emmc.image.clone(), 0);
    {
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, "Courses").unwrap();
        fat.mkdir(&mut d, dir, "Sub.fit", (0, 0)).unwrap();
    }
    put_file(&mut d, "Courses", "notes.txt", b"not a course");
    for i in 0..110usize {
        put_file(&mut d, "Courses", &std::format!("C{i:03}.{}", ["fit", "gpx"][i % 2]), &std::vec![i as u8; 1 + i]);
    }
    sim.emmc.image = d.0;
    let (listed, end) = list_courses(&mut sim, &mut app, (control, data), 3);
    assert_eq!((listed.len(), end), (100, Some((Status::Ok, 100))));
    let keys: Vec<_> = listed.iter().map(|e| std::string::String::from_utf8(e.0.bytes().to_vec()).unwrap()).collect();
    assert_eq!(keys, (0..100).map(|i| std::format!("{i:02}")).collect::<Vec<_>>());
    // the card's four courses first, then C000...: C006 is the eleventh
    let key = course_key(&listed, "C006.fit", 7).expect("C006 listed");
    assert_eq!(key.bytes(), b"10");
    let read = Command::ReadIn { seq: 4, folder: Folder::Courses, offset: 0, key };
    assert_eq!(ask(&mut sim, &mut app, control, data, read, 5000).bytes, [6; 7]);
    let mut f = [0; 20];
    let n = Command::Delete { seq: 5, folder: Folder::Courses, key }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..n]), Some((Status::Ok, 1)));
    assert!(course_on_card(&sim, "C006.fit").is_none() && course_on_card(&sim, "C007.gpx").is_some());
}

/// LIST_IN 2 numbers only what a READ_IN or DELETE can reach: a course past the first 200 entries of `\Courses` is
/// not listed (and the commit would answer FULL).
#[test]
fn courses_past_the_walk_are_not_listed() {
    let (mut sim, mut app, control, data) = paired();
    let (before, _) = list_courses(&mut sim, &mut app, (control, data), 2);
    let mut d = Ram(sim.emmc.image.clone(), 0);
    for i in 0..crate::storage::workout_write::WALK {
        put_file(&mut d, "Courses", &std::format!("n{i:03}.txt"), b"x");
    }
    put_file(&mut d, "Courses", "Late.fit", b"far");
    sim.emmc.image = d.0;
    let (listed, end) = list_courses(&mut sim, &mut app, (control, data), 3);
    assert_eq!((listed, end), (before.clone(), Some((Status::Ok, before.len() as u32))));
}

/// The COURSES menu after the phone deleted or sent a course: listed again at once when it shows (its cursor kept
/// within the shorter list); while a course is read to follow, once that read ends.
#[test]
fn the_courses_menu_follows_what_the_phone_does() {
    let (mut sim, mut app, control, data) = paired();
    app.set_course_memory(crate::storage::course_file::tests::memory());
    let file = turns();
    let ends = (control, data);
    let sent = send_course(&mut sim, &mut app, ends, (b"Late", &file, crc(&file)), 238);
    assert_eq!(sent.1, Some((Status::Ok, file.len() as u32)));
    app.open_courses(&mut sim);
    run(&mut sim, &mut app, 1000);
    let names = |app: &App| {
        let l = &app.courses.as_deref().unwrap().list;
        (0..l.len()).map(|i| std::string::String::from(l.get(i).unwrap().name())).collect::<Vec<_>>()
    };
    let n = names(&app).len();
    assert_eq!(names(&app).last().map(|s| s.as_str()), Some("LATE"));
    app.screen = Screen::Menu(Menu::Courses, n - 1); // the cursor on the last course
    let (listed, _) = list_courses(&mut sim, &mut app, ends, 4);
    let key = course_key(&listed, "Late.fit", file.len() as u32).unwrap();
    let mut f = [0; 20];
    let k = Command::Delete { seq: 5, folder: Folder::Courses, key }.encode(&mut f);
    assert_eq!(answer(&mut sim, &mut app, control, data, &f[..k]), Some((Status::Ok, 1)));
    assert!(!names(&app).contains(&"LATE".into()), "listed again");
    assert_eq!(app.screen(), Screen::Menu(Menu::Courses, n - 2), "the cursor on the new last row");
    // while a course is read to follow: the list stays as it was until the read ends
    app.follow_course(0);
    let sent = send_course(&mut sim, &mut app, ends, (b"Later", &file, crc(&file)), 238);
    assert_eq!(sent.1, Some((Status::Ok, file.len() as u32)));
    run(&mut sim, &mut app, 1000);
    assert!(!app.courses.as_deref().unwrap().loading() && !app.courses.as_deref().unwrap().stale);
    assert!(names(&app).contains(&"LATER".into()), "listed once the read ended");
}
