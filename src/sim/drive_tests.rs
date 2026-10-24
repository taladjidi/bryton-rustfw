//! The USB drive (msc.rs, app/drive.rs) in the app, with a computer's side of the bulk endpoints
//! (`msc::tests::Host`) and the simulated card holding the synthetic FAT volume: when the card may be lent, that
//! nothing else touches it meanwhile, and what the computer reads and writes.

use super::*;
use crate::msc::tests::{Host, cbw, rw10};
use crate::ui::UsbDot;

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

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

/// Refused without the dev link up, during a ride and while its save is pending, each with the banner saying why;
/// then lent: the card gets no command from the app for a minute (past the start of the scan of stock's rides),
/// Start starts no ride, the dev link's card commands are refused; the computer reads the volume, writes two
/// sectors and ejects: the card is the app's again, and a restart is asked for since the volume changed under the
/// app's caches.
#[test]
fn lent_to_the_computer_one_owner_at_a_time() {
    let (mut sim, mut app) = started(true);
    app.set_usb(&mut sim, UsbDot::Powered);
    assert_eq!(app.drive_on(), Err("CONNECT USB"));
    app.set_usb(&mut sim, UsbDot::Ready);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.drive_on(), Err("END THE RIDE FIRST"));
    assert_eq!(app.banner.as_str(), "END THE RIDE FIRST");
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    app.press(&mut sim, Press::Long(Key::Start)); // end: saved at the next pass
    assert_eq!(app.drive_on(), Err("RIDE NOT SAVED YET"));
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());

    assert_eq!(app.drive_on(), Ok(()));
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

/// A build that may not write the card (EMMC_WRITE unset): the drive is write protected (MODE SENSE) and a write
/// fails with nothing sent to the card; having only read, the card is taken back from the unit without a restart.
#[test]
fn read_only_build_offers_a_write_protected_drive() {
    let (mut sim, mut app) = started(false);
    app.drive_on().unwrap();
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

/// USB DRIVE in the DEVICE menu: Select lends the card (the row says ON), Select again takes it back.
#[test]
fn the_device_menu_row() {
    let (mut sim, mut app) = started(true);
    let row = menu::DEVICE.iter().position(|&r| r == menu::Row::Drive).unwrap();
    app.open(Screen::Menu(Menu::Device, row));
    app.show(&mut sim);
    app.input(&mut sim, Input::Select);
    assert!(app.drive_lent());
    assert_eq!(app.menu_row(Menu::Device, row).unwrap().1.as_str(), "ON");
    app.input(&mut sim, Input::Select);
    assert!(!app.drive_lent());
}
