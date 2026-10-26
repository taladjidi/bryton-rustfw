//! The USB drive (msc.rs, app/drive.rs) in the app, with a computer's side of the bulk endpoints
//! (`msc::tests::Host`) and the simulated card holding the synthetic FAT volume: when the card may be lent, that
//! nothing else touches it meanwhile, and what the computer reads and writes.

use super::*;
use crate::msc::tests::{Host, cbw, rw10};
use crate::ui::UsbDot;

/// One command from the computer: its data and status (the CSW checked).
fn command(sim: &mut Sim, app: &mut App, h: &mut Host, tag: u32, len: u32, dir_in: bool, cb: &[u8]) -> (Vec<u8>, u8) {
    app.drive_packet(sim, &cbw(tag, len, dir_in, cb), h);
    let (data, status, _) = h.take(tag);
    (data, status)
}

fn started(writable: bool) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.writable = writable;
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    (sim, app)
}

/// A workout started while the card is lent: the ride refuses, so no player runs and no workout page shows.
#[test]
fn no_workout_while_the_card_is_lent() {
    let (mut sim, mut app) = started(true);
    assert_eq!(app.drive_on(&mut sim), Ok(()));
    let screen = app.screen();
    app.start_workout(0);
    assert_eq!(app.banner.as_str(), "USB DRIVE ON");
    assert!(app.player.is_none() && app.ride.state() == ride::State::Idle);
    assert_eq!(app.screen(), screen);
}

/// Refused without the dev link up, during a ride and while its save is pending, each with the banner saying why;
/// then lent: the card gets no command from the app for a minute (past the start of the scan of stock's rides),
/// Start starts no ride, the dev link's card commands are refused; the computer reads the volume, writes two
/// sectors and ejects: the card is the app's again, and a restart is asked for since the volume changed under the
/// app's caches.
#[test]
fn lent_to_the_computer_one_owner_at_a_time() {
    let (mut sim, mut app) = started(true);
    app.set_usb(&mut sim, UsbDot::Powered);
    assert_eq!(app.drive_on(&mut sim), Err("CONNECT USB"));
    app.set_usb(&mut sim, UsbDot::Ready);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.drive_on(&mut sim), Err("END THE RIDE FIRST"));
    assert_eq!(app.banner.as_str(), "END THE RIDE FIRST");
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // end: saved at the next pass
    assert_eq!(app.drive_on(&mut sim), Err("RIDE NOT SAVED YET"));
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());

    assert_eq!(app.drive_on(&mut sim), Ok(()));
    sim.emmc.cmds.clear();
    app.press(&mut sim, Press::Short(Key::Back)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 60_000);
    assert_eq!(app.ride.state(), ride::State::Idle, "no ride while lent");
    assert!(app.gnss_ms >= STOCK_SCAN_MS && app.stock.status() == crate::stock::Status::Idle, "the scan waits");
    assert_eq!(app.emmc().card(&mut sim), Err(crate::emmc::Error::Lent), "the dev link refused");
    assert!(sim.emmc.cmds.is_empty(), "the app sent the card nothing: {:?}", sim.emmc.cmds);

    let mut h = Host::default();
    assert_eq!(command(&mut sim, &mut app, &mut h, 1, 0, false, &[0; 6]).1, 1, "UNIT ATTENTION: medium changed");
    assert_eq!(command(&mut sim, &mut app, &mut h, 2, 0, false, &[0; 6]).1, 0, "ready");
    let (cap, _) = command(&mut sim, &mut app, &mut h, 3, 8, true, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let b = EMMC_SECTORS - crate::emmc::RESERVED;
    assert_eq!(cap[..4], (b - 1).to_be_bytes(), "the FAT volume only: sectors 0 .. B-1");
    let (boot, st) = command(&mut sim, &mut app, &mut h, 4, 64 * 512, true, &rw10(0x28, 0, 64));
    assert_eq!(st, 0);
    assert!(boot[..] == sim.emmc.image[..64 * 512], "the volume's first 64 sectors");
    assert_eq!(boot[510..512], [0x55, 0xaa]);
    assert!(sim.emmc.cmds.iter().any(|c| c.0 == 18), "multi-block reads");
    assert_eq!(command(&mut sim, &mut app, &mut h, 5, 512, true, &rw10(0x28, b, 1)).1, 1, "past B: out of range");
    let at = (sim.emmc.image.len() / 512) as u32 - 2;
    h.out = [vec![0xa5; 512], vec![0x5a; 512]].into();
    assert_eq!(command(&mut sim, &mut app, &mut h, 6, 1024, false, &rw10(0x2a, at, 2)).1, 0, "written");
    let img = &sim.emmc.image[at as usize * 512..];
    assert!(img[..512].iter().all(|&x| x == 0xa5) && img[512..1024].iter().all(|&x| x == 0x5a));
    assert!(!app.restart_wanted());
    command(&mut sim, &mut app, &mut h, 7, 0, false, &[0x1e, 0, 0, 0, 1, 0]); // mounted: removal prevented
    app.drive_off();
    assert!(app.drive_lent(), "the unit cannot take it back while the computer holds it");
    assert_eq!(app.banner.as_str(), "EJECT ON COMPUTER");
    command(&mut sim, &mut app, &mut h, 8, 0, false, &[0x1e, 0, 0, 0, 0, 0]);
    assert_eq!(command(&mut sim, &mut app, &mut h, 9, 0, false, &[0x1b, 0, 0, 0, 2, 0]).1, 0, "ejected");
    assert!(!app.drive_lent() && app.restart_wanted(), "the card back; a restart, since the volume changed");
    assert_eq!(command(&mut sim, &mut app, &mut h, 10, 0, false, &[0; 6]).1, 1, "no medium any more");
}

/// A full card (the review of 2026-10-25): the ride's recording stops at once, "CARD FULL" said once, and the ride
/// ended, the card is lent to the computer that would free it (it was refused for good: the ride never settled).
#[test]
fn a_full_card_never_holds_the_drive() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    crate::fat::write::tests::fill(&mut sim.emmc.image, 0, true);
    sim.writable = true;
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let full = app.notices.iter().filter(|n| n.as_str().ends_with("CARD FULL")).count();
    assert_eq!(full, 1, "{:?}", app.notices.iter().map(|n| n.as_str()).collect::<Vec<_>>());
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.drive_on(&mut sim), Ok(()));
}

/// A build that may not write the card (EMMC_WRITE unset): the drive is write protected (MODE SENSE) and a write
/// fails with nothing sent to the card; having only read, the card is taken back from the unit without a restart.
#[test]
fn read_only_build_offers_a_write_protected_drive() {
    let (mut sim, mut app) = started(false);
    app.drive_on(&mut sim).unwrap();
    let mut h = Host::default();
    command(&mut sim, &mut app, &mut h, 1, 0, false, &[0; 6]);
    let (ms, _) = command(&mut sim, &mut app, &mut h, 2, 192, true, &[0x1a, 0, 0x3f, 0, 192, 0]);
    assert_eq!(ms[2], 0x80, "write protected");
    sim.emmc.cmds.clear();
    h.out = [vec![0; 512]].into();
    assert_eq!(command(&mut sim, &mut app, &mut h, 3, 512, false, &rw10(0x2a, 100, 1)).1, 1);
    assert!(sim.emmc.cmds.iter().all(|c| c.0 != 24 && c.0 != 25), "nothing written");
    app.drive_off();
    assert!(!app.drive_lent() && !app.restart_wanted());
    assert_eq!(app.banner.as_str(), "USB DRIVE OFF");
}

/// USB DRIVE in the SYSTEM menu: Select lends the card (the row says ON), Select again takes it back.
#[test]
fn the_device_menu_row() {
    let (mut sim, mut app) = started(true);
    let row = menu::SYSTEM.iter().position(|&r| r == menu::Row::Drive).unwrap();
    app.open(Screen::Menu(Menu::System, row));
    app.show(&mut sim);
    app.input(&mut sim, Input::Select);
    assert!(app.drive_lent());
    assert_eq!(app.menu_row(Menu::System, row).unwrap().1.as_str(), "ON");
    app.input(&mut sim, Input::Select);
    assert!(!app.drive_lent());
}

/// A swipe during a long drive command (a second for 1 MB at full speed, when the main loop reads no gesture):
/// the panel is read after each chunk, and the swipe acts once the command is done.
#[test]
fn a_swipe_during_a_long_read() {
    use std::{cell::Cell, rc::Rc};
    let samples = Rc::new(Cell::new(0u32));
    let n = samples.clone();
    // the finger moves 16 px down per look at the panel (the simulated card takes no time), then lifts
    let mut sim = Sim::new(9, 0, 0).finger(move |_| {
        let k = n.get();
        n.set(k + 1);
        (k < 6).then_some((120, 40 + 16 * k as u16))
    });
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    samples.set(0);
    app.drive_on(&mut sim).unwrap();
    let mut h = Host::default();
    assert_eq!(command(&mut sim, &mut app, &mut h, 1, 0, false, &[0; 6]).1, 1, "UNIT ATTENTION: medium changed");
    samples.set(0);
    let (data, st) = command(&mut sim, &mut app, &mut h, 2, 128 * 512, true, &rw10(0x28, 0, 128));
    assert_eq!((data.len(), st), (128 * 512, 0), "8 chunks");
    assert!(app.touching(), "a gesture waits");
    assert_eq!(app.touch(&mut sim, 0), Some(Input::Touch(Gesture::Swipe(Dir::Down))));
}

/// A tap still down when a drive command ends stays a tap: the main loop's first read after the command does not
/// charge its sleep again on top of the samples taken in the command (review: it was a hold, the menu opened).
#[test]
fn a_tap_across_the_end_of_a_command() {
    use std::{cell::Cell, rc::Rc};
    let samples = Rc::new(Cell::new(0u32));
    let n = samples.clone();
    // a still finger from the 5th look at the panel, lifted after the command
    let mut sim = Sim::new(9, 0, 0).finger(move |_| {
        let k = n.get();
        n.set(k + 1);
        (4..10).contains(&k).then_some((120, 150))
    });
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    app.drive_on(&mut sim).unwrap();
    let mut h = Host::default();
    command(&mut sim, &mut app, &mut h, 1, 0, false, &[0; 6]);
    samples.set(0);
    assert_eq!(command(&mut sim, &mut app, &mut h, 2, 128 * 512, true, &rw10(0x28, 0, 128)).1, 0);
    assert!(app.touching(), "the finger down");
    assert_eq!(app.touch(&mut sim, 2000), None, "2 s slept since the main loop's last read: not a hold");
    let g = (0..8).find_map(|_| app.touch(&mut sim, 20));
    assert!(matches!(g, Some(Input::Touch(Gesture::Tap(..)))), "{g:?}");
}

/// A delete a cut left halfway (the ride named by no entry, trash.rs) is finished before the card is lent, even
/// before the scan that finishes it at start-up: lent first, the computer would see the ride nowhere and its fsck
/// could free or reconnect the chain the move then links (the review of 2026-10-25).
#[test]
fn a_delete_cut_halfway_is_finished_before_lending() {
    use crate::storage::trash::tests::{cut_midway, file};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = cut_midway();
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261020080000.fit").is_none(), "not finished yet");
    assert_eq!(app.drive_on(&mut sim), Ok(()));
    assert!(file(&sim.emmc.image, "Rustfw/Deleted/261020080000.fit").is_some());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A save failing after its step 4 (recorder.rs: the ride's entry removed, the new one not written) and unable to
/// name the ride again in `\Rustfw\Rides` (its directory failing too): stuck. The marker stays and the drive is
/// lent read only (MODE SENSE write protected: the owner copies what is on the card, no computer's fsck writes the
/// nameless chain); a Start says the new ride waits ("NOT RECORDING: CARD"); once the card is well the old ride is
/// saved, the new one recorded, and when it is saved the drive is lent as ever.
#[test]
fn a_save_stuck_after_its_step_4_lends_the_drive_read_only() {
    let (mut sim, mut app) = started(true);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 30_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    let dirs = ["Activities", "Rustfw/Rides"].map(|p| dir_sectors(&sim.emmc.image, p));
    sim.emmc.bad = std::vec![dirs[0].clone()]; // \Activities' directory cannot be written
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 1000); // its end on the card, step 4 done, 5 failed
    sim.emmc.bad = dirs.to_vec(); // nor \Rustfw\Rides' from here
    run(&mut sim, &mut app, 300_000); // past GIVE_UP tries
    assert_eq!(app.recorder.phase(), recorder::Phase::Saving, "never done while nameless");
    assert!(app.recorder.stuck());
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "the marker kept");
    assert_eq!(app.drive_on(&mut sim), Ok(()));
    assert_eq!(app.banner.as_str(), "USB DRIVE READ ONLY");
    let mut h = Host::default();
    command(&mut sim, &mut app, &mut h, 1, 0, false, &[0; 6]);
    let (ms, _) = command(&mut sim, &mut app, &mut h, 2, 192, true, &[0x1a, 0, 0x3f, 0, 192, 0]);
    assert_eq!(ms[2], 0x80, "write protected");
    h.out = [vec![0; 512]].into();
    assert_eq!(command(&mut sim, &mut app, &mut h, 3, 512, false, &rw10(0x2a, 100, 1)).1, 1, "a write refused");
    app.drive_off();
    assert!(!app.drive_lent() && !app.restart_wanted());
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    assert!(app.notices[0].as_str().ends_with("NOT RECORDING: CARD"), "{}", app.notices[0].as_str());
    sim.emmc.bad.clear();
    run(&mut sim, &mut app, 300_000);
    assert!(app.notices.iter().any(|n| n.as_str().ends_with("RIDE SAVED")));
    assert_eq!(app.recorder.phase(), recorder::Phase::Recording, "the new ride recorded");
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.drive_on(&mut sim), Ok(()));
    assert_eq!(app.banner.as_str(), "USB DRIVE ON");
}

/// The sectors of folder `path`'s first cluster on the volume `img`.
fn dir_sectors(img: &[u8], path: &str) -> core::ops::Range<u32> {
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
    let (spc, data) = (u(13, 1), u(14, 2) + u(16, 1) * u(36, 4));
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let c = crate::fat::Fat::mount(&mut ram).unwrap().open(&mut ram, path).unwrap().cluster;
    let lba = data + (c - 2) * spc;
    lba..lba + spc
}

/// The `.fit` files in folder `path` of the volume `img`.
fn fits_in(img: &[u8], path: &str) -> usize {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = crate::fat::Fat::mount(&mut ram).unwrap();
    let Ok(dir) = fat.open(&mut ram, path) else { return 0 };
    let mut c = fat.cursor(dir).unwrap();
    let mut n = 0;
    while let Some(e) = fat.list(&mut ram, &mut c).unwrap() {
        n += usize::from(e.name().ends_with(b".fit"));
    }
    n
}

/// The re-review of 4748752: DISCARD on a ride held behind the last one's save discarded the last one (the end took
/// the held start, the discard then acted on the ride still in the recorder). The held ride is dropped, the last
/// one saved.
#[test]
fn discarding_a_held_ride_saves_the_last_one() {
    let (mut sim, mut app) = started(true);
    let before = [fits_in(&sim.emmc.image, "Activities"), fits_in(&sim.emmc.image, "Rustfw/Activities")];
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 30_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // ended, its save at the next pass
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start)); // a new ride, held
    assert!(app.recorder.pending() && app.full.pending());
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    app.discard_ride();
    assert!(!app.recorder.pending() && !app.full.pending());
    run(&mut sim, &mut app, 10_000);
    let after = [fits_in(&sim.emmc.image, "Activities"), fits_in(&sim.emmc.image, "Rustfw/Activities")];
    assert_eq!(after, before.map(|n| n + 1), "the last ride saved, both copies");
    assert_eq!(fits_in(&sim.emmc.image, "Rustfw/Discarded") + fits_in(&sim.emmc.image, "Rustfw/Discarded/Full"), 0);
}

/// "NOT RECORDING: CARD" as soon as a ride held behind the last one's save is known not to be recorded (that save
/// stuck), not at the next of the every-5-minutes "NOT SAVING" notices.
#[test]
fn a_held_ride_is_said_once_the_save_is_stuck() {
    let (mut sim, mut app) = started(true);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 30_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    let dirs = ["Activities", "Rustfw/Rides"].map(|p| dir_sectors(&sim.emmc.image, p));
    sim.emmc.bad = std::vec![dirs[0].clone()];
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 1000); // step 4 done, 5 failed
    sim.emmc.bad = dirs.to_vec();
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start)); // a new ride, held
    assert!(app.recorder.pending() && !app.recorder.held());
    let said = |app: &App| app.notices.iter().filter(|n| n.as_str().ends_with("NOT RECORDING: CARD")).count();
    assert_eq!(said(&app), 0);
    let mut ms = 0;
    while !app.recorder.held() && ms < 300_000 {
        run(&mut sim, &mut app, 100);
        ms += 100;
    }
    assert!(app.recorder.held());
    assert_eq!(said(&app), 1, "said when it became so");
}
