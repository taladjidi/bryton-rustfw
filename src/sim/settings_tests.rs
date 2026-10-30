//! `\Rustfw\settings.bin` on the Sim (app/settings_file.rs): a setting changed in the menu is back after a restart,
//! from the registers and from the file alone; the migration from the registers, once; a power cut after every
//! sector of a write reads the old settings or the new, never the defaults.

extern crate std;
use super::tests::{run, to_row};
use super::*;
use crate::fat::tests::{Ram, read_all};
use crate::menu::Setting;
use crate::{PAGES_LOAD_MS, Settings, SettingsCopy};
use std::vec::Vec;

/// Settings unlike the defaults in every field the registers keep.
fn some() -> Settings {
    Settings {
        wheel_mm: crate::WHEELS[4],
        max_hr: 177,
        ftp: 265,
        weight_kg: 68,
        eat_min: 45,
        light_level: 1,
        smooth: true,
        gps_power: crate::GpsPower::Saver,
        ..Settings::from_word(0)
    }
}

/// The settings' registers as `s` writes them (App::save_misc).
fn set_registers(sim: &mut Sim, s: Settings) {
    let [w, p, a, off] = s.register_words();
    (sim.backup[crate::WHEEL_BKP], sim.backup[crate::POWER_BKP], sim.backup[crate::ALERTS_BKP]) = (w, p, a);
    sim.backup[crate::AID_BKP[1]] = off << 22;
}

/// A unit with `regs` in its registers on the synthetic volume, past the file's read (and its first write).
fn started(regs: Settings, reliable: bool) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    if reliable {
        sim.emmc.rel_param = crate::emmc::EN_REL_WR;
    }
    set_registers(&mut sim, regs);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 200);
    settle(&mut sim, &mut app);
    (sim, app)
}

/// Passes until the settings are written (the FAT copies compared first when the file is created: some passes).
fn settle(sim: &mut Sim, app: &mut App) {
    for _ in 0..200 {
        run(sim, app, 200);
        if !app.settings_store.dirty {
            return;
        }
    }
    panic!("the settings never written");
}

/// A restart on the same card: `lost` with the backup registers empty (a flat backup battery).
fn reboot(sim: &mut Sim, lost: bool) -> (Sim, App) {
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.emmc.rel_param = sim.emmc.rel_param;
    if !lost {
        sim2.backup = sim.backup;
    }
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, u64::from(PAGES_LOAD_MS) + 200);
    (sim2, app)
}

/// The file's newest copy, if there is one.
fn file(sim: &Sim) -> Option<SettingsCopy> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = read_all(&mut fat, &mut d, "Rustfw/settings.bin").ok()?;
    SettingsCopy::newest(&v.try_into().ok()?)
}

/// The settings kept (the altitude being edited is not).
fn kept(app: &App) -> Settings {
    Settings { alt_m: 500, ..app.settings }
}

/// GPS POWER changed in SETTINGS > BIKE: written to the file between rides; after a restart it is back, from the
/// registers and, with them lost, from the file alone (not stock's or the defaults).
#[test]
fn a_menu_change_is_back_after_a_restart() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    assert_eq!(file(&sim).map(|c| c.settings), Some(Settings::from_word(0)), "migrated");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Settings));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::GpsPower));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.settings.gps_power, crate::GpsPower::Full);
    let c = file(&sim).unwrap();
    assert_eq!((c.seq, c.settings.gps_power), (2, crate::GpsPower::Full), "written");
    let image = sim.emmc.image.clone();
    let (_, app2) = reboot(&mut sim, false);
    assert_eq!(app2.settings.gps_power, crate::GpsPower::Full, "back");
    sim.emmc.image = image;
    let (sim3, app3) = reboot(&mut sim, true);
    assert_eq!(app3.settings.gps_power, crate::GpsPower::Full, "back from the file alone");
    let regs = Settings::from_word(sim3.backup[crate::WHEEL_BKP]).with_power_word(sim3.backup[crate::POWER_BKP]);
    assert_eq!(regs.gps_power, crate::GpsPower::Full, "the registers put back from the file");
    assert_eq!(file(&sim3).unwrap().seq, 2, "not written again");
}

/// The first start of a build with the file: the registers' settings written as its first copy; the next start
/// reads it and writes nothing.
#[test]
fn migration_from_the_registers_happens_once() {
    let (mut sim, app) = started(some(), false);
    assert_eq!(kept(&app), some());
    let c = file(&sim).unwrap();
    assert_eq!((c.seq, c.settings, c.words), (1, some(), some().register_words()));
    let written = sim.emmc.sectors_written;
    let image = sim.emmc.image.clone();
    let (sim2, app2) = reboot(&mut sim, false);
    assert_eq!(kept(&app2), some());
    assert!(sim2.emmc.image == image, "nothing written");
    assert!(!app2.settings_store.dirty);
    std::println!("migration: {written} sectors");
}

/// A change made while the file had to wait (during a ride), then a reset before it was written: the registers have
/// it and win; the file gets it at the next start.
#[test]
fn registers_newer_than_the_file_win() {
    let (mut sim, mut app) = started(some(), false);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    app.settings.ftp = 300;
    app.save_misc(&mut sim);
    run(&mut sim, &mut app, 2000);
    assert_eq!(file(&sim).unwrap().settings.ftp, 265, "not written during the ride");
    let (sim2, app2) = reboot(&mut sim, false);
    assert_eq!(app2.settings.ftp, 300);
    // the ride resumed: still not written
    assert_eq!(file(&sim2).unwrap().settings.ftp, 265);
    assert!(app2.settings_store.dirty);
}

/// The test volume with `\Rustfw\` and a `pages.bin` in it (no settings file).
fn with_rustfw() -> Vec<u8> {
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let t = crate::fat::write::dos_time((2026, 10, 20, 8, 0, 0));
    let dir = fat.mkdir(&mut d, fat.root(), crate::recorder::DIR, t).unwrap();
    let mut h = fat.create(&mut d, dir, crate::page::FILE, t).unwrap();
    fat.write_sectors(&mut d, &mut h, 0, &[0x55; 1024]).unwrap();
    fat.set_size(&mut d, &mut h, 1024, t).unwrap();
    d.0
}

/// A read-only start with the registers lost (stock's settings imported into them), then a writable start: the
/// file's settings, not the import (the read-only start put the registers right from the file).
#[test]
fn a_read_only_start_with_lost_registers_keeps_the_file() {
    let (mut sim, mut app) = started(some(), false);
    app.settings.ftp = 300;
    app.save_misc(&mut sim);
    settle(&mut sim, &mut app);
    let want = kept(&app);
    let image = sim.emmc.image.clone();
    let mut ro = Sim::new(9, 5, 0);
    (ro.emmc.image, ro.writable) = (image.clone(), false);
    let mut app = App::start(&mut ro, None);
    run(&mut ro, &mut app, u64::from(PAGES_LOAD_MS) + 200);
    assert_eq!(kept(&app), want, "read-only: the file's");
    assert!(ro.emmc.image == image, "the card unchanged");
    let mut w = Sim::new(9, 10, 0);
    (w.emmc.image, w.backup) = (image, ro.backup);
    let mut app = App::start(&mut w, None);
    run(&mut w, &mut app, u64::from(PAGES_LOAD_MS) + 200);
    assert_eq!(kept(&app), want, "then writable: still the file's");
    assert_eq!(file(&w).map(|c| c.settings), Some(want));
}

/// Register 18 without AUTO OFF's bits at the start (an earlier build), and the aiding written before the file is
/// read: the bits stay as they were, so the next start still takes AUTO OFF from the file, not the default.
#[test]
fn aiding_written_before_the_read_keeps_auto_off_unknown() {
    let (mut sim, mut app) = started(some(), false);
    app.settings.auto_off = 3;
    app.save_misc(&mut sim);
    settle(&mut sim, &mut app);
    sim.backup[crate::AID_BKP[1]] = 0;
    let mut s2 = Sim::new(9, 5, 0);
    (s2.emmc.image, s2.backup) = (sim.emmc.image.clone(), sim.backup);
    let app2 = App::start(&mut s2, None);
    app2.write_aid(&mut s2); // a fix, before the file is read
    assert_eq!(s2.backup[crate::AID_BKP[1]] >> 22 & 7, 0, "no AUTO OFF written");
    let mut s3 = Sim::new(9, 10, 0);
    (s3.emmc.image, s3.backup) = (s2.emmc.image.clone(), s2.backup);
    let mut app3 = App::start(&mut s3, None);
    run(&mut s3, &mut app3, u64::from(PAGES_LOAD_MS) + 200);
    assert_eq!(app3.settings.auto_off, 3, "the file's");
}

/// With the drive lent (the file not read yet), SMOOTH turned on and off again, then a restart: the file's FONT, the
/// register put back as it was at the start rather than left with the undone change.
#[test]
fn a_change_undone_before_the_read_leaves_the_files() {
    let (sim, _) = started(Settings { smooth: false, ..some() }, false);
    assert!(!file(&sim).unwrap().settings.smooth);
    let mut s2 = Sim::new(9, 5, 0);
    (s2.emmc.image, s2.backup) = (sim.emmc.image.clone(), sim.backup);
    let mut app2 = App::start(&mut s2, None);
    app2.emmc.lend(true);
    for smooth in [true, false] {
        app2.settings.smooth = smooth;
        app2.save_misc(&mut s2);
    }
    run(&mut s2, &mut app2, u64::from(PAGES_LOAD_MS) + 200);
    assert!(!app2.settings_store.loaded, "the drive lent: not read");
    let (_, app3) = reboot(&mut s2, false);
    assert!(!app3.settings.smooth, "the file's");
}

/// Register by register: register 18 rewritten by a build without AUTO OFF (its bits gone) keeps the file's AUTO OFF;
/// register 16 changed alone gives only its fields (the weight), the others staying the file's; the file then has
/// both and the registers agree with it.
#[test]
fn each_register_on_its_own() {
    let (mut sim, mut app) = started(some(), false);
    app.settings.auto_off = 3;
    app.settings.ftp = 300;
    app.save_misc(&mut sim);
    run(&mut sim, &mut app, 500);
    let file_settings = kept(&app);
    assert_eq!(file(&sim).map(|c| c.settings), Some(file_settings));
    sim.backup[crate::AID_BKP[1]] = 0x0123_4567; // no aiding tag: AUTO OFF unknown
    sim.backup[crate::POWER_BKP] = Settings { weight_kg: 81, smooth: false, ..file_settings }.power_word();
    let (mut sim2, mut app2) = reboot(&mut sim, false);
    let want = Settings { weight_kg: 81, smooth: false, ..file_settings };
    assert_eq!(kept(&app2), want, "register 16's fields only; AUTO OFF and FTP the file's");
    settle(&mut sim2, &mut app2);
    assert_eq!(file(&sim2).map(|c| c.settings), Some(want), "the file has them");
    let back = Settings::from_word(0).with_auto_off_word(sim2.backup[crate::AID_BKP[1]]);
    assert_eq!(back.auto_off, 3, "register 18 put right");
    assert_eq!(sim2.backup[crate::POWER_BKP], want.power_word());
}

/// Both copies damaged: the registers' settings, written again as a new copy; one damaged: the other.
#[test]
fn damaged_copies() {
    let (mut sim, mut app) = started(some(), false);
    app.settings.ftp = 300;
    app.save_misc(&mut sim);
    run(&mut sim, &mut app, 500);
    assert_eq!(file(&sim).unwrap().seq, 2);
    // the file's first sector: the first of its two copies' magics
    let base =
        (0..sim.emmc.image.len() / 512).map(|k| 512 * k).find(|&a| sim.emmc.image[a..a + 4] == *b"RSET").unwrap();
    // copy 2 (slot 0) damaged: copy 1 (slot 1, FTP 265) wins, with the registers lost
    sim.emmc.image[base + 40] ^= 1;
    let image = sim.emmc.image.clone();
    let (_, app2) = reboot(&mut sim, true);
    assert_eq!(app2.settings.ftp, 265, "the other copy");
    // both damaged: the registers' (FTP 300), written as a new copy
    sim.emmc.image = image;
    sim.emmc.image[base + 512 + 40] ^= 1;
    let (sim3, app3) = reboot(&mut sim, false);
    assert_eq!(app3.settings.ftp, 300);
    assert_eq!(file(&sim3).map(|c| c.settings.ftp), Some(300), "written again");
}

/// A power cut after every sector of a write of the settings: the first (the file created, the migration) and later
/// ones (a copy over the older). The next start, with the registers as the change left them, has the new settings;
/// with them lost, the file's old or new, never the defaults once a copy was whole; the volume stays clean for fsck.
/// The sector being written at the cut is torn (half old, half new, the rest garbage) unless the card offers reliable
/// writes. On a plain card the migration is cut with `\Rustfw\` there already (pages.bin's entry beside the new
/// one): only the root directory's write is left out, since a torn root sector garbles the entries beside it, stock's
/// folders included, and the FAT layer cannot make a plain rewrite in place atomic. The unit's card has the enhanced
/// reliable write (BOARD.md, "Reliable write", read 2026-10-29), so every write there is reliable.
#[test]
fn a_cut_at_every_sector_reads_old_or_new() {
    let change = |app: &mut App, sim: &mut Sim, k: u16| {
        app.settings.ftp = 200 + 5 * k;
        app.settings.smooth = k % 2 == 1;
        app.save_misc(sim);
    };
    for (writes, reliable) in [(0, false), (1, false), (2, false), (0, true), (1, true), (2, true)] {
        // `writes` changes written before the one cut (0: the cut is in the migration)
        // what a start with the registers lost reads from a card without the file (stock's settings, imported)
        let baseline = {
            let mut sim = Sim::new(9, 0, 0);
            sim.emmc.image = crate::fat::tests::image();
            kept(&reboot(&mut sim, true).1)
        };
        let setup = || {
            let mut sim = Sim::new(9, 0, 0);
            // the migration on a plain card: `\Rustfw\` there already, with pages.bin's entry beside the new one
            sim.emmc.image = if writes == 0 && !reliable { with_rustfw() } else { crate::fat::tests::image() };
            if reliable {
                sim.emmc.rel_param = crate::emmc::EN_REL_WR;
            }
            sim.emmc.tear = true;
            set_registers(&mut sim, some());
            let mut app = App::start(&mut sim, None);
            let old = if writes == 0 { baseline } else { some() };
            if writes > 0 {
                run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 200);
                settle(&mut sim, &mut app);
            }
            for k in 1..writes {
                change(&mut app, &mut sim, k);
                settle(&mut sim, &mut app);
            }
            let old = if writes > 1 { kept(&app) } else { old };
            if writes > 0 {
                change(&mut app, &mut sim, 20);
            }
            let new = kept(&app);
            (sim.emmc.sectors_written, sim, app, old, new)
        };
        let (base, mut sim, mut app, _, _) = setup();
        run(&mut sim, &mut app, 30_000);
        let total = sim.emmc.sectors_written - base;
        assert!(total > 0);
        let new = kept(&app);
        assert_eq!(kept(&reboot(&mut sim, true).1), new, "writes {writes}: uncut, the new from the file");
        let (mut olds, mut news) = (0, 0);
        for cut in 0..total {
            let (base, mut sim, mut app, old, new) = setup();
            sim.emmc.cut_after = Some(base + cut);
            run(&mut sim, &mut app, 30_000);
            let mut lost_sim = Sim::new(9, 0, 0);
            lost_sim.emmc.image = sim.emmc.image.clone();
            lost_sim.emmc.rel_param = sim.emmc.rel_param;
            let (_, from_file) = reboot(&mut lost_sim, true);
            let got = kept(&from_file);
            if got == old {
                olds += 1;
            } else {
                assert_eq!(got, new, "writes {writes}, cut {cut}: neither old nor new");
                news += 1;
            }
            let (mut sim, mut app) = reboot(&mut sim, false);
            assert_eq!(kept(&app), new, "writes {writes}, cut {cut}: the registers' settings");
            settle(&mut sim, &mut app);
            assert_eq!(file(&sim).map(|c| c.settings), Some(new), "cut {cut}");
            let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
        }
        std::println!(
            "writes {writes} (reliable {reliable}): {total} sectors, {olds} cuts read the old, {news} the new"
        );
        assert!(olds > 0);
    }
}

/// The second FAT of the synthetic volume, and whether a command of `cmds` touched it since command `from`.
fn fat2(sim: &Sim) -> core::ops::Range<u32> {
    let img = &sim.emmc.image;
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
    let (reserved, size) = (u(14, 2), u(36, 4));
    reserved + size..reserved + 2 * size
}

fn touched(sim: &Sim, from: usize, cmds: &[u8]) -> bool {
    let fat2 = fat2(sim);
    sim.emmc.cmds[from..].iter().any(|c| cmds.contains(&c.0) && fat2.contains(&c.1))
}

/// Presses Start until a write to the second FAT (failing in it) has been made.
fn start_until_fat2_written(sim: &mut Sim, app: &mut App) {
    let from = sim.emmc.cmds.len();
    app.press(sim, Press::Short(Key::Start));
    for _ in 0..1000 {
        run(sim, app, 100);
        if touched(sim, from, &[24, 25]) {
            break;
        }
    }
    assert!(touched(sim, from, &[24, 25]), "a FAT sector written to the first copy only");
}

/// The first read of the second FAT (CMD17 / 18) at or after command `from` comes before the first write (CMD24 / 25)
/// of settings.bin's two sectors.
fn fat2_read_before_the_settings_write(sim: &Sim, from: usize) {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let cluster = fat.open(&mut d, "Rustfw/settings.bin").unwrap().cluster;
    let lba = fat.lba(cluster);
    let fat2 = fat2(sim);
    let at = |pred: &dyn Fn(&(u8, u32)) -> bool| sim.emmc.cmds[from..].iter().position(pred);
    let read = at(&|c| matches!(c.0, 17 | 18) && fat2.contains(&c.1));
    let write = at(&|c| matches!(c.0, 24 | 25) && (lba..lba + 2).contains(&c.1));
    assert!(write.is_some(), "settings.bin written");
    assert!(
        read.is_some_and(|r| r < write.unwrap_or(0)),
        "the FAT copies compared first: read {read:?}, write {write:?}"
    );
}

/// The ride paused, saved, its card closed; then GPS POWER changed: written after the FAT copies were compared
/// (the second one read), and the volume clean. The saved ride's trips are the first settings write after the
/// failure, so they do the comparing; the copies are then made doubtful again for GPS POWER's write.
fn save_then_settings_compare_the_fats(sim: &mut Sim, app: &mut App) {
    let from = sim.emmc.cmds.len();
    run(sim, app, 70_000);
    app.press(sim, Press::Short(Key::Start)); // pause
    super::save_ride(sim, app);
    run(sim, app, 10_000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    settle(sim, app);
    assert!(app.settings.trip_s[0] > 0, "the ride's trips written");
    fat2_read_before_the_settings_write(sim, from);
    app.press(sim, Press::Short(Key::Back)); // the banner
    app.press(sim, Press::Short(Key::Back)); // the summary
    app.press(sim, Press::Long(Key::Page));
    to_row(sim, app, menu::Row::Open(Menu::Settings));
    app.press(sim, Press::Short(Key::Start));
    to_row(sim, app, menu::Row::Open(Menu::Bike));
    app.press(sim, Press::Short(Key::Start));
    to_row(sim, app, menu::Row::Set(Setting::GpsPower));
    app.fats_unsure(); // a card error since: the next write compares again
    let from = sim.emmc.cmds.len();
    app.press(sim, Press::Short(Key::Start));
    settle(sim, app);
    assert_eq!(file(sim).map(|c| c.settings.gps_power), Some(crate::GpsPower::Full), "written");
    fat2_read_before_the_settings_write(sim, from);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image).map(|_| ()), Ok(()));
}

/// A ride whose card write fails in the second FAT (a FAT sector reached the first copy only): the next settings
/// write, after the ride, compares the FAT copies first.
#[test]
fn a_recorder_write_failure_has_the_next_settings_write_compare_the_fats() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    sim.emmc.bad = std::vec![fat2(&sim)];
    start_until_fat2_written(&mut sim, &mut app); // the ride's file takes its first cluster
    sim.emmc.bad.clear();
    save_then_settings_compare_the_fats(&mut sim, &mut app);
}

/// The same for an error the recorder goes on after (the review of 4dbec8d): `\Rustfw\Discarded\Full` missing at a
/// ride's start, its creation fails in the second FAT once; the ride records on, and the settings write after it
/// compares the FAT copies all the same.
#[test]
fn a_failed_folder_at_a_ride_start_has_the_settings_write_compare_the_fats() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    app.press(&mut sim, Press::Short(Key::Start)); // a first ride: every folder made
    run(&mut sim, &mut app, 70_000);
    app.press(&mut sim, Press::Short(Key::Start));
    super::save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 10_000);
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    // `Full` deleted by hand (0xe5 on its entry and its long name's), its cluster left lost
    let dir = crate::sim::drive_tests::dir_sectors(&sim.emmc.image, recorder::DISCARDED_PATHS[0]);
    let img = &mut sim.emmc.image;
    let mut entries = (dir.start as usize * 512..dir.end as usize * 512).step_by(32);
    let at = entries.find(|&a| img[a + 11] == 0x10 && img[a] != b'.'); // its one folder
    let mut at = at.expect("Discarded\\Full made by the first ride");
    img[at] = 0xe5;
    while at % 512 > 0 && img[at - 32 + 11] == 0x0f {
        at -= 32;
        img[at] = 0xe5;
    }
    let errors = app.rec.clean.errors;
    (sim.emmc.bad, sim.emmc.bad_left) = (std::vec![fat2(&sim)], Some(1));
    start_until_fat2_written(&mut sim, &mut app); // the folder made again: its cluster's FAT sector, once
    assert_eq!((app.rec.clean.errors, app.rec.clean.fails()), (errors + 1, 0), "an error gone on after");
    save_then_settings_compare_the_fats(&mut sim, &mut app);
}

/// TRIP 1, TRIP 2 and the ODOMETER grow by a saved ride's totals once it is saved (never while it runs), are written to
/// the file, and a trip resets by hand (asked twice) while the odometer stays.
#[test]
fn trips_and_the_odometer_grow_at_a_save_and_a_trip_resets_by_hand() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    assert_eq!((app.settings.trip_m, app.settings.odo_m), ([0; 2], 0), "nothing while the ride runs");
    app.press(&mut sim, Press::Short(Key::Start));
    let (m, s) = (app.ride.dist_m(), app.ride.timer_s());
    assert!(m > 300 && s > 30, "a ride: {m} m {s} s");
    super::save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 10_000);
    settle(&mut sim, &mut app);
    let got = |st: &Settings| (st.trip_m, st.trip_s, st.odo_m);
    assert_eq!(got(&app.settings), ([m; 2], [s; 2], m), "added at the save");
    assert_eq!(file(&sim).map(|c| got(&c.settings)), Some(([m; 2], [s; 2], m)), "and written");
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Settings));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Trip(0)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.settings.trip_m, [m; 2], "asked first");
    app.press(&mut sim, Press::Short(Key::Start));
    settle(&mut sim, &mut app);
    assert_eq!(got(&app.settings), ([0, m], [0, s], m), "TRIP 1 reset, TRIP 2 and the odometer kept");
    assert_eq!(file(&sim).map(|c| got(&c.settings)), Some(([0, m], [0, s], m)), "written");
}

/// SOUND OFF silences every tone (an alert's too); ON plays them.
#[test]
fn sound_off_silences_the_tones() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    for sound in [false, true] {
        app.settings.sound = sound;
        sim.tones.clear();
        app.tones.play(&crate::BEEP_ALERT);
        run(&mut sim, &mut app, 1000);
        assert_eq!(sim.tones.iter().any(|&hz| hz != 0), sound, "sound {sound}: {:?}", sim.tones);
    }
}

/// Each saved ride pays the trips once, even when ride B is ended while A's save is pending; a discarded ride pays
/// nothing.
#[test]
fn trips_pay_once_per_saved_ride_and_a_discarded_ride_pays_nothing() {
    let (mut sim, mut app) = started(Settings::from_word(0), false);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 40_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    let a = app.ride.dist_m();
    super::save_ride(&mut sim, &mut app); // A ended, its save at the next pass
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start)); // B starts, held behind A's save
    run(&mut sim, &mut app, 20_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    let b = app.ride.dist_m();
    super::save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 20_000);
    settle(&mut sim, &mut app);
    assert!(a > 100 && b > 50, "{a} {b}");
    assert_eq!((app.settings.trip_m[0], app.settings.odo_m), (a + b, a + b), "each once");
    assert_eq!(app.settings_store.unpaid(), (0, 0), "nothing left waiting");
    // C is discarded: nothing added
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    assert!(app.ride.state() == crate::ride::State::Running && app.ride.dist_m() > 50, "C is riding");
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    assert!(app.ride.dist_m() > 50 && app.ride.state() != crate::ride::State::Idle, "C is paused");
    app.discard_ride();
    run(&mut sim, &mut app, 20_000);
    settle(&mut sim, &mut app);
    assert_eq!((app.settings.trip_m[0], app.settings.odo_m), (a + b, a + b), "a discarded ride adds nothing");
    assert_eq!(app.settings_store.unpaid(), (0, 0));
}

/// The queue of ended rides (`SettingsStore::done`): saved pays, kept or discarded drops, the one behind moves up.
#[test]
fn the_ended_rides_queue_pays_saved_ones_only() {
    let mut st = crate::SettingsStore::default();
    st.ended(10, 1);
    st.ended(20, 2);
    assert_eq!(st.unpaid(), (30, 3), "both count while unpaid");
    st.done(true);
    assert_eq!((st.due, st.saving, st.unpaid()), ((10, 1), Some((20, 2)), (30, 3)));
    st.done(false); // the second was kept
    assert_eq!((st.due, st.saving, st.queued, st.unpaid()), ((10, 1), None, None, (10, 1)));
    st.done(true); // a save with nothing ended: nothing
    assert_eq!(st.due, (10, 1));
}
