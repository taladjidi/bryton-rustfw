//! SYSTEM > SCREENSHOT (app/screenshot.rs) and the fault log (app/faultlog.rs) on the Sim's card: the file equals the
//! panel, a full card or a write error is a notice and leaves no file, a power cut at every sector leaves the volume
//! clean and the rides as they were; the fault log written once after a fault reset, never before a resumed ride is
//! saved, appended, capped.

use super::menu_tests::to_row;
use super::tests::run;
use super::*;
use crate::fat::Fat;
use crate::fat::tests::Ram;
use crate::fat::write::tests::{fill, fsck, fsck_cut};
use crate::storage::faultlog::{self, CAP, Kind, LOGGED, cfsr_detail, marker};
use crate::storage::screenshot::{self, SIZE};
use crate::ui::menu::Row;
use std::string::String;

/// The files of folder `path` (names and sizes), None when the folder is not there.
fn files(img: &[u8], path: &str) -> Option<Vec<(String, u32)>> {
    let mut d = Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let dir = fat.open(&mut d, path).ok()?;
    let mut c = fat.cursor(dir).unwrap();
    let mut out = Vec::new();
    while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
        if !e.file.dir {
            out.push((String::from_utf8_lossy(e.name()).into_owned(), e.file.size));
        }
    }
    Some(out)
}

/// Every ride on the card, both copies: (path, bytes).
fn rides(img: &[u8]) -> Vec<(String, Vec<u8>)> {
    ["Activities", "Rustfw/Activities"]
        .iter()
        .flat_map(|dir| files(img, dir).unwrap_or_default().into_iter().map(move |(n, _)| std::format!("{dir}/{n}")))
        .map(|p| {
            let bytes = crate::storage::trash::tests::file(img, &p).unwrap();
            (p, bytes)
        })
        .collect()
}

fn newest_notice(app: &App) -> &str {
    app.notes.list[0].as_str()
}

/// The menus' SCREENSHOT: the menus left, the screen under them saved at the next pass as
/// `\Screenshot\YYYYMMDDhhmmss.bmp`, which decodes to the panel's pixels; the volume clean.
#[test]
fn screenshot_saves_the_panel() {
    let (mut sim, mut app) = started();
    sim.emmc.image = crate::storage::trash::tests::volume();
    run(&mut sim, &mut app, 3000);
    let rides_before = rides(&sim.emmc.image);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, Row::Screenshot);
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(matches!(app.screen(), Screen::Main | Screen::Ride(_)), "the menus left: {:?}", app.screen());
    app.step(&mut sim);
    let shots = files(&sim.emmc.image, "Screenshot").expect("the folder made");
    assert_eq!(shots.len(), 1, "{shots:?}");
    let (name, size) = &shots[0];
    assert!(name.starts_with("20261023090") && name.ends_with(".bmp") && name.len() == 18, "{name}");
    assert_eq!(*size, SIZE);
    let f = crate::storage::trash::tests::file(&sim.emmc.image, &std::format!("Screenshot/{name}")).unwrap();
    let px = screenshot::tests::decode(&f).expect("a BMP of ours");
    assert!(px == sim.screen, "the file equals the panel");
    assert!(newest_notice(&app).ends_with("SCREENSHOT SAVED"), "{}", newest_notice(&app));
    assert_eq!(fsck(&sim.emmc.image), Ok(0));
    assert_eq!(rides(&sim.emmc.image), rides_before);
}

/// A full card (FSInfo right): a notice, nothing written, no folder. FSInfo stale (a computer's write), so the room
/// check passes and the card fills up mid-write: a notice, no file, the chain freed (no cluster lost). The same stale
/// count after a failed write (the FAT copies suspect): the FAT counted first, so nothing written. During a ride:
/// refused, nothing written.
#[test]
fn screenshot_on_a_full_card_or_during_a_ride_writes_nothing() {
    for (tell, suspect) in [(true, false), (false, false), (false, true)] {
        let (mut sim, mut app) = started();
        let mut img = crate::fat::tests::image();
        fill(&mut img, 5, tell);
        sim.emmc.image = img;
        run(&mut sim, &mut app, 3000);
        app.fats_suspect = suspect;
        app.shot_due = true;
        app.step(&mut sim);
        let case = std::format!("tell {tell}, suspect {suspect}");
        assert!(newest_notice(&app).ends_with("CARD FULL: NO SCREENSHOT"), "{case}: {}", newest_notice(&app));
        let shots = files(&sim.emmc.image, "Screenshot");
        if tell || suspect {
            assert_eq!(shots, None, "{case}: nothing written");
        } else {
            assert_eq!(shots, Some(Vec::new()), "{case}: the folder, no file");
        }
        assert_eq!(fsck(&sim.emmc.image), Ok(0), "{case}: no cluster lost");
    }

    let (mut sim, mut app) = started();
    sim.emmc.image = crate::fat::tests::image();
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    app.screenshot();
    assert!(!app.shot_due, "refused while a ride is open");
    run(&mut sim, &mut app, 1000);
    assert_eq!(files(&sim.emmc.image, "Screenshot"), None);
}

/// A power cut after every sector of a screenshot: the volume as fsck takes it (at most the shot's chain lost, the
/// FAT copies apart in one sector at most, no chain past its size), either no file or the whole picture, and every
/// ride byte for byte as it was.
#[test]
fn screenshot_cut_at_every_sector() {
    let base = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::storage::trash::tests::volume();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 3000);
        (sim, app)
    };
    let (mut sim, mut app) = base();
    let rides_before = rides(&sim.emmc.image);
    let w0 = sim.emmc.sectors_written;
    app.shot_due = true;
    app.step(&mut sim);
    let total = sim.emmc.sectors_written - w0;
    assert!(total >= u64::from(SIZE.div_ceil(512)), "{total} sectors");
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let chain = SIZE.div_ceil(Fat::mount(&mut d).unwrap().cluster_bytes());
    let mut whole = 0;
    for cut in 0..=total {
        let (mut sim, mut app) = base();
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + cut);
        app.shot_due = true;
        app.step(&mut sim);
        let img = &sim.emmc.image;
        let scan = fsck_cut(img).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(scan.lost <= chain, "cut {cut}: {} clusters lost", scan.lost);
        assert!(scan.apart <= 1 && scan.past.is_empty(), "cut {cut}: {} apart, {:?} past", scan.apart, scan.past);
        for (name, _) in files(img, "Screenshot").unwrap_or_default() {
            let f = crate::storage::trash::tests::file(img, &std::format!("Screenshot/{name}")).unwrap();
            assert!(screenshot::tests::decode(&f).is_some(), "cut {cut}: a partial file");
            whole += 1;
        }
        assert!(rides(img) == rides_before, "cut {cut}: a ride changed");
    }
    std::println!("{total} cuts, {whole} with the picture whole");
}

/// The Sim's card with a fault marker `word` in its backup registers, started.
fn faulted(img: Vec<u8>, backup: [u32; 32], word: u32) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = img;
    sim.backup = backup;
    sim.backup[FAULT_BKP] = word;
    let app = App::start(&mut sim, None);
    (sim, app)
}

fn log(img: &[u8]) -> Option<String> {
    crate::storage::trash::tests::file(img, "Rustfw/Log/faults.log").map(|b| String::from_utf8(b).unwrap())
}

/// No fault: no log. A panic: its line written once the card is quiet (not in the first seconds), the marker marked
/// logged, so a reset before its minute is up does not log it again; a second fault in a row appended under it.
#[test]
fn fault_log_written_once_and_appended() {
    let (mut sim, mut app) = started();
    sim.emmc.image = crate::fat::tests::image();
    run(&mut sim, &mut app, 10_000);
    assert_eq!(files(&sim.emmc.image, "Rustfw/Log"), None, "no fault, no log");

    let (mut sim, mut app) = faulted(crate::fat::tests::image(), [0; 32], marker(0, Kind::Panic, 412));
    run(&mut sim, &mut app, 1000);
    assert_eq!(log(&sim.emmc.image), None, "not in the first seconds");
    run(&mut sim, &mut app, 3000);
    // the time of the start (the Sim's start-up takes a second)
    let first = log(&sim.emmc.image).expect("the log");
    let tail = std::format!(" panic at line 412, 1 in a row, build {}\n", env!("GIT_DESCRIBE"));
    assert!(first.starts_with("2026-10-23 09:00:0") && first.ends_with(&tail) && first.len() == 19 + tail.len());
    assert_ne!(sim.backup[FAULT_BKP] & LOGGED, 0, "marked logged");

    // a reset that was not a fault, within the minute: the marker kept, not logged again
    let (mut sim, mut app) = faulted(core::mem::take(&mut sim.emmc.image), sim.backup, sim.backup[FAULT_BKP]);
    run(&mut sim, &mut app, 5000);
    assert_eq!(log(&sim.emmc.image).as_deref(), Some(first.as_str()));

    // a second fault in a row
    let word = marker(sim.backup[FAULT_BKP], Kind::Cpu, cfsr_detail(1 << 16));
    let (mut sim, mut app) = faulted(core::mem::take(&mut sim.emmc.image), sim.backup, word);
    run(&mut sim, &mut app, 5000);
    let text = log(&sim.emmc.image).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[1].contains(" CPU fault UNDEFINSTR, 2 in a row"), "{}", lines[1]);
    assert_eq!(fsck(&sim.emmc.image), Ok(0));
}

/// A fault mid-ride: the ride resumed at the start, and the log waits for it to be saved (the recorder the card's one
/// writer while a ride is open). A build that may not write the card never writes it.
#[test]
fn fault_log_waits_for_the_resumed_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    let word = marker(0, Kind::Stack, 0);
    let (mut sim, mut app) = faulted(core::mem::take(&mut sim.emmc.image), sim.backup, word);
    run(&mut sim, &mut app, 5000);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")));
    assert_eq!(log(&sim.emmc.image), None, "not while the ride is open");
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    assert!(log(&sim.emmc.image).unwrap().contains(" stack overflow (canary), 1 in a row"));
    assert_eq!(fsck(&sim.emmc.image), Ok(0));

    let (mut sim, mut app) = {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.writable = false;
        sim.backup[FAULT_BKP] = word;
        let app = App::start(&mut sim, None);
        (sim, app)
    };
    run(&mut sim, &mut app, 10_000);
    assert_eq!(files(&sim.emmc.image, "Rustfw/Log"), None, "read-only: never written");
}

/// A log of `size` bytes of `x` lines on the test card.
fn with_log(size: u32) -> Vec<u8> {
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let t = (0, 0);
    let top = fat.ensure_dir(&mut d, fat.root(), crate::recorder::DIR, t).unwrap();
    let dir = fat.ensure_dir(&mut d, top, faultlog::LOG_DIR, t).unwrap();
    let mut h = fat.create(&mut d, dir, faultlog::FILE, t).unwrap();
    let mut bytes: Vec<u8> = (0..size.div_ceil(512) * 512).map(|i| if i % 50 == 49 { b'\n' } else { b'x' }).collect();
    bytes.resize(bytes.len().max(512), 0);
    fat.write_sectors(&mut d, &mut h, 0, &bytes).unwrap();
    fat.set_size(&mut d, &mut h, size, t).unwrap();
    d.0
}

/// A line that would take the log past its cap starts it again: the log is that line alone.
#[test]
fn fault_log_capped() {
    let (mut sim, mut app) = faulted(with_log(CAP - 30), [0; 32], marker(0, Kind::Panic, 7));
    run(&mut sim, &mut app, 5000);
    let text = log(&sim.emmc.image).unwrap();
    assert!(
        text.starts_with("2026-10-23 09:00:0") && text.contains(" panic at line 7,") && text.lines().count() == 1,
        "{text}"
    );
    assert_eq!(fsck(&sim.emmc.image), Ok(0));
}

/// A power cut after every sector of an append that crosses a sector boundary (the log 500 bytes long), and of one that
/// starts the log again (at the cap): the log is the old one, the old one and the line, or (started again) empty or the
/// line alone, never a mix; the volume as fsck takes it after a cut, the rides as they were.
#[test]
fn fault_log_cut_at_every_sector() {
    for size in [500, CAP - 30] {
        let base = || faulted(with_log(size), [0; 32], marker(0, Kind::Panic, 9));
        let (mut sim, mut app) = base();
        let old = log(&sim.emmc.image).unwrap();
        let rides_before = rides(&sim.emmc.image);
        run(&mut sim, &mut app, 5000);
        let new = log(&sim.emmc.image).unwrap();
        let again = size > 500;
        assert!(if again { new.lines().count() == 1 } else { new.len() > old.len() && new.starts_with(&old) });
        let total = sim.emmc.sectors_written;
        for cut in 0..=total {
            let (mut sim, mut app) = base();
            sim.emmc.cut_after = Some(cut);
            run(&mut sim, &mut app, 5000);
            let img = &sim.emmc.image;
            let got = log(img).unwrap_or_else(|| panic!("size {size}, cut {cut}: the log gone"));
            assert!(
                got == old || got == new || (again && got.is_empty()),
                "size {size}, cut {cut}: {} bytes",
                got.len()
            );
            let scan = fsck_cut(img).unwrap_or_else(|e| panic!("size {size}, cut {cut}: {e}"));
            // a cut while the tail is freed leaves the rest of it lost (a chain no entry names), at most the log's
            let most = if again { CAP / (16 << 10) + 1 } else { 1 };
            assert!(
                scan.lost <= most && scan.apart <= 1 && scan.past.len() <= 1,
                "size {size}, cut {cut}: {} lost, {} apart, {:?} past",
                scan.lost,
                scan.apart,
                scan.past
            );
            assert!(rides(img) == rides_before, "size {size}, cut {cut}: a ride changed");
        }
    }
}

/// An append that fails (every write refused by the card) is said once and tried again a minute later: once the card
/// writes again, the line is in the log, once.
#[test]
fn fault_log_retried_after_a_failure() {
    let (mut sim, mut app) = faulted(crate::fat::tests::image(), [0; 32], marker(0, Kind::Panic, 3));
    sim.emmc.bad = std::vec![0..u32::MAX];
    run(&mut sim, &mut app, 5000);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("FAULT LOG NOT SAVED")));
    assert_eq!(log(&sim.emmc.image), None);
    sim.emmc.bad.clear();
    run(&mut sim, &mut app, 30_000);
    assert_eq!(log(&sim.emmc.image), None, "not before its minute");
    run(&mut sim, &mut app, 40_000);
    let text = log(&sim.emmc.image).expect("written on the retry");
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains(" panic at line 3, 1 in a row"));
    run(&mut sim, &mut app, 70_000);
    assert_eq!(log(&sim.emmc.image), Some(text), "once");
    let notes = app.notes.list.iter().filter(|n| n.as_str().ends_with("FAULT LOG NOT SAVED")).count();
    assert_eq!(notes, 1, "said once");
}

/// Two screenshots in one second: the second finds the name taken, says so, and writes nothing.
#[test]
fn second_screenshot_in_the_same_second() {
    let (mut sim, mut app) = started();
    sim.emmc.image = crate::fat::tests::image();
    run(&mut sim, &mut app, 3000);
    app.shot_due = true;
    app.step(&mut sim);
    assert!(newest_notice(&app).ends_with("SCREENSHOT SAVED"), "{}", newest_notice(&app));
    let first = files(&sim.emmc.image, "Screenshot").unwrap();
    let local = app.rec_time().0;
    assert_eq!(
        app.write_shot(&mut sim, screenshot::name(local).as_str(), crate::fat::write::dos_time(local)),
        Ok(false)
    );
    app.shot_due = true;
    let name = screenshot::name(app.rec_time().0);
    app.step(&mut sim);
    if first.iter().any(|(n, _)| n.as_str() == name.as_str()) {
        assert!(newest_notice(&app).ends_with("SCREENSHOT: TRY AGAIN"), "{}", newest_notice(&app));
        assert_eq!(files(&sim.emmc.image, "Screenshot").unwrap(), first, "nothing written");
    }
    assert_eq!(fsck(&sim.emmc.image), Ok(0));
}
