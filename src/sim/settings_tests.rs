//! `\Rustfw\settings.bin` on the Sim (app/settings_file.rs): a setting changed in the menu is back after a restart,
//! from the registers and from the file alone; the migration from the registers, once; a power cut after every
//! sector of a write reads the old settings or the new, never the defaults.

extern crate std;
use super::tests::{run, to_row};
use super::*;
use crate::fat::tests::{Ram, read_all};
use crate::menu::Setting;
use crate::{PAGES_LOAD_MS, Settings, SettingsCopy};

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
/// On a card with reliable writes, the sector being written is torn too.
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
            sim.emmc.image = crate::fat::tests::image();
            if reliable {
                sim.emmc.rel_param = crate::emmc::EN_REL_WR;
            }
            sim.emmc.tear = reliable;
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
