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
    let mut phone = PhoneSim::new(true);
    phone.connect_at = 1000;
    let (mut sim, mut app) = super::tests::unit_with(None, |sim| {
        sim.nordic.phone = Some(phone);
        let mut d = Ram(sim.emmc.image.clone(), 0);
        for (name, data) in rides() {
            put_file(&mut d, "Activities", &std::format!("{}.fit", std::str::from_utf8(&name).unwrap()), &data);
        }
        put_file(&mut d, "Activities", "tmp.fit", b"stock's ride in progress");
        put_file(&mut d, "Activities", "notes.txt", b"not a ride");
        sim.emmc.image = d.0;
    });
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    let (control, data) = app.phone.files.handles.expect("the service was added");
    sim.nordic.phone_subscribe(data + 1, true);
    run(&mut sim, &mut app, 200);
    (sim, app, control, data)
}

/// The Data frames notified so far, drained.
pub(super) fn drain(sim: &mut Sim, data: u16) -> Vec<Vec<u8>> {
    let sent = std::mem::take(&mut sim.nordic.server.sent);
    let (mine, rest): (Vec<_>, Vec<_>) = sent.into_iter().partition(|s| s.0 == data);
    sim.nordic.server.sent = rest;
    mine.into_iter().map(|s| s.1).collect()
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
        run(sim, app, 50);
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
    let mut f = [0; 20];
    let n = Command::Read { seq: 5, offset: 0, name }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    run(&mut sim, &mut app, 300);
    app.leave_menus();
    app.input(&mut sim, Input::Select); // Start
    let mut end = None;
    for _ in 0..40 {
        run(&mut sim, &mut app, 100);
        for p in drain(&mut sim, data) {
            if let Some(Frame::End { status, .. }) = Frame::parse(&p) {
                end = Some(status);
            }
        }
    }
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(end, Some(Status::Busy), "the transfer was cut");
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
    run(&mut sim, &mut app, 500);
    assert!(!app.phone.files.active() && drain(&mut sim, data).is_empty());
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
    let mut f = [0; 20];
    let n = Command::Read { seq: 1, offset: 0, name }.encode(&mut f);
    sim.nordic.phone_write(control, &f[..n]);
    run(&mut sim, &mut app, 1500);
    let mut kept: Vec<u8> = Vec::new();
    for p in drain(&mut sim, data) {
        if let Some(Frame::Chunk { data, .. }) = Frame::parse(&p) {
            kept.extend(data);
        }
    }
    assert!(!kept.is_empty() && kept.len() < bytes.len(), "{} kept", kept.len());
    // the phone walks away and comes back
    let ph = sim.nordic.phone.as_mut().unwrap();
    ph.hang_up();
    ph.connect_at = sim.ms + 500;
    sim.nordic.evt(0x11, &[0, 0, 0x08]);
    run(&mut sim, &mut app, 300);
    assert!(!app.phone.files.active(), "the link's end cleared the transfer");
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    sim.nordic.phone_subscribe(data + 1, true);
    run(&mut sim, &mut app, 200);
    drain(&mut sim, data);
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
