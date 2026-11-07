//! The file service on the phone bond (app/filesvc.rs, link/filesvc.rs) against the simulated chip and a scripted
//! phone: LIST, READ whole and from an offset, BUSY, the encrypted bond, names outside `\Activities\`, a link
//! dropped mid-read and resumed.

use super::*;
use crate::fat::tests::{Ram, put_file};
use crate::filesvc::{Command, Frame, Name, Status};

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
fn drop_link(sim: &mut Sim, back_at: u64) {
    let ph = sim.nordic.phone.as_mut().unwrap();
    ph.hang_up();
    ph.connect_at = back_at;
    sim.nordic.evt(0x11, &[0, 0, 0x08]);
}

/// The phone enables Data again on a new link and the frames of the wait are dropped.
fn resubscribe(sim: &mut Sim, app: &mut App, data: u16) {
    sim.nordic.phone_subscribe(data + 1, true);
    run(sim, app, 200);
    drain(sim, data);
}

/// [`run`] with the loop's real pass: the short one while a READ runs on the fast link ([`App::pass_ms`]).
fn pass(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.sleep_ms(if app.phone.files.fast() { app.pass_ms() } else { super::POLL_MS });
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
    app.files_command(&[1, 1]);
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
            let id = [0x55, 0x57, 0x21, 0x23][(next() % 4) as usize];
            let mut junk: Vec<u8> = (0..4 + next() % 8).map(|_| next() as u8).collect();
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
