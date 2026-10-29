//! Colour themes on the Sim (ui/theme.rs, app/themes.rs): the scramble theme shows which roles each screen uses (a
//! draw that skips `Canvas::ink` leaves a built-in colour on screen), DARK is today's look, OUTDOOR in shots for the
//! gallery, a theme file picked from the card and kept in settings.bin.

extern crate std;
use super::tests::run;
use super::*;
use crate::fat::tests::Ram;
use crate::menu::Setting;
use crate::theme::{KEYS, Theme};
use crate::{PAGES_LOAD_MS, Settings, SettingsCopy};
use std::string::{String, ToString};
use std::vec::Vec;

const SUNNY: &str =
    "[theme]\nversion = 1\nname = SUNNY\nbase = outdoor\n[colors]\naccent = #00AAFF ; blue\nzone1 = #4488CC\n";
/// Red for a zone: refused (red only for alarms).
const REDZONE: &str = "[theme]\nversion = 1\n[colors]\nzone3 = #FF0000\n";

/// Every key drawn as a colour no key has.
fn scramble() -> Theme {
    let mut t = Theme::DARK;
    for (i, c) in t.ink.iter_mut().enumerate() {
        *c = 0x2000 + 0x0111 * i as u16;
        assert!(!KEYS.contains(c));
    }
    t
}

fn unit(image: Option<Vec<u8>>) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = image.unwrap_or_else(crate::fat::tests::image);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 200);
    (sim, app)
}

/// The volume with `\Rustfw\Themes\<name>` files.
fn with_themes(files: &[(&str, &str)]) -> Vec<u8> {
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let t = crate::fat::write::dos_time((2026, 10, 20, 8, 0, 0));
    let dir = fat.mkdir(&mut d, fat.root(), crate::recorder::DIR, t).unwrap();
    let dir = fat.mkdir(&mut d, dir, "Themes", t).unwrap();
    for (name, text) in files {
        let mut h = fat.create(&mut d, dir, name, t).unwrap();
        let mut sector = std::vec![0u8; 512];
        sector[..text.len()].copy_from_slice(text.as_bytes());
        fat.write_sectors(&mut d, &mut h, 0, &sector).unwrap();
        fat.set_size(&mut d, &mut h, text.len() as u32, t).unwrap();
    }
    d.0
}

fn set_theme(sim: &mut Sim, app: &mut App, t: Theme) {
    app.theme = t;
    app.repaint(sim);
}

/// The screens: the overview, the ride pages, the menu and DISPLAY.
fn scenes(sim: &mut Sim, app: &mut App, mut each: impl FnMut(&mut Sim, &mut App, &str)) {
    each(sim, app, "overview");
    for k in 1..=3 {
        app.press(sim, Press::Short(Key::Page));
        run(sim, app, 1100);
        each(sim, app, &std::format!("page{k}"));
    }
    app.press(sim, Press::Long(Key::Page));
    run(sim, app, 200);
    each(sim, app, "menu");
    super::tests::to_row(sim, app, menu::Row::Set(Setting::Theme));
    run(sim, app, 200);
    each(sim, app, "display");
}

#[test]
fn scramble_shows_every_role_translated() {
    let (mut sim, mut app) = unit(None);
    set_theme(&mut sim, &mut app, scramble());
    let mut left = Vec::new();
    scenes(&mut sim, &mut app, |sim, app, name| {
        super::tests::run(sim, app, 50);
        for (i, &k) in KEYS.iter().enumerate() {
            let n = sim.screen.iter().filter(|&&p| p == k).count();
            if n > 0 {
                let rows: Vec<usize> = (0..sim.screen.len()).filter(|&i| sim.screen[i] == k).map(|i| i / 240).collect();
                let cols: Vec<usize> = (0..sim.screen.len()).filter(|&i| sim.screen[i] == k).map(|i| i % 240).collect();
                left.push(std::format!(
                    "rows {:?} cols {:?}",
                    (rows.iter().min(), rows.iter().max()),
                    (cols.iter().min(), cols.iter().max())
                ));
                left.push(std::format!("{name}: key {i} ({k:#06x}) x{n}"));
            }
        }
        let used = scramble().ink.iter().filter(|c| sim.screen.contains(c)).count();
        assert!(used >= 2, "{name}: the theme reaches the screen");
    });
    assert!(left.is_empty(), "built-in colours drawn untranslated:\n{}", left.join("\n"));
}

#[test]
fn dark_and_outdoor_shots() {
    let (mut sim, mut app) = unit(None);
    let mut dark = Vec::new();
    scenes(&mut sim, &mut app, |sim, _, name| {
        dark.push(sim.screen.clone());
        sim.save(&std::format!("theme_dark_{name}"));
    });
    // DARK is the identity: the same pixels as the first run, a second time through the theme
    let (mut sim, mut app) = unit(None);
    set_theme(&mut sim, &mut app, Theme::DARK);
    let mut i = 0;
    scenes(&mut sim, &mut app, |sim, _, _| {
        assert!(sim.screen == dark[i], "scene {i} differs in DARK");
        i += 1;
    });
    assert!(dark.iter().all(|s| !s.contains(&crate::ui::ON_ACCENT) && !s.contains(&crate::ui::ON_ALARM)));
    let (mut sim, mut app) = unit(None);
    set_theme(&mut sim, &mut app, Theme::OUTDOOR);
    let mut i = 0;
    scenes(&mut sim, &mut app, |sim, _, name| {
        assert!(sim.screen != dark[i], "{name}: OUTDOOR differs");
        i += 1;
        sim.save(&std::format!("theme_outdoor_{name}"));
    });
}

/// The settings file's newest copy.
fn kept(sim: &Sim) -> Option<SettingsCopy> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = crate::fat::tests::read_all(&mut fat, &mut d, "Rustfw/settings.bin").ok()?;
    SettingsCopy::newest(&v.try_into().ok()?)
}

/// THEME cycles DARK, OUTDOOR, the card's files; a refused file gives DARK and a notice; the name is kept in
/// settings.bin and applied after a restart; an older file (31 bytes of settings) reads with DARK.
#[test]
fn theme_from_the_card_is_picked_and_kept() {
    let (mut sim, mut app) =
        unit(Some(with_themes(&[("SUNNY.INI", SUNNY), ("REDZONE.INI", REDZONE), ("NOTES.TXT", "x")])));
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, menu::Row::Set(Setting::Theme));
    let mut seen: Vec<(String, Theme)> = Vec::new();
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 100);
        super::tests::assert_consistent(&sim, &app);
        seen.push((app.settings.theme_name().to_string(), app.theme));
    }
    let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["OUTDOOR", "SUNNY", "REDZONE", "DARK"], "NOTES.TXT is no theme");
    assert_eq!(seen[0].1, Theme::OUTDOOR);
    let sunny = crate::theme::parse(SUNNY.as_bytes()).unwrap();
    assert_eq!(seen[1].1, sunny);
    assert_eq!(seen[2].1, Theme::DARK, "refused: DARK drawn");
    // on to SUNNY (two more presses), kept, and applied after a restart
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.settings.theme_name(), "SUNNY");
    for _ in 0..200 {
        run(&mut sim, &mut app, 200);
        if !app.settings_store.dirty {
            break;
        }
    }
    assert_eq!(kept(&sim).map(|c| c.settings.theme_name().to_string()).as_deref(), Some("SUNNY"));
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.own_settings();
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    let mut app2 = App::start(&mut sim2, None);
    assert_eq!(app2.theme, Theme::DARK, "until the file is read");
    run(&mut sim2, &mut app2, u64::from(PAGES_LOAD_MS) + 300);
    assert_eq!((app2.settings.theme_name(), app2.theme), ("SUNNY", sunny));
    sim2.save("theme_sunny_overview");
}

#[test]
fn an_older_settings_file_reads_with_dark() {
    let s = Settings { theme: *b"OUTDOOR\0\0\0\0\0", ..Settings::from_word(0) };
    let c = SettingsCopy { seq: 1, words: [0; 4], settings: s };
    let mut sec = c.to_sector();
    assert_eq!(SettingsCopy::from_sector(&sec).map(|c| c.settings.theme_name().len()), Some(7));
    // an earlier build: 31 bytes of settings
    sec[6..8].copy_from_slice(&31u16.to_le_bytes());
    let crc = crate::inflate::crc32(sec[..508].iter().copied());
    sec[508..].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(SettingsCopy::from_sector(&sec).map(|c| c.settings.theme_name().to_string()).as_deref(), Some("DARK"));
    // a damaged name is DARK too
    let mut sec = c.to_sector();
    sec[28 + 31] = b'/';
    let crc = crate::inflate::crc32(sec[..508].iter().copied());
    sec[508..].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(SettingsCopy::from_sector(&sec).map(|c| c.settings.theme_name().to_string()).as_deref(), Some("DARK"));
}

/// The screens the scenes above do not reach: a workout's gauge, the trainer, a graph, the call overlay and the end
/// screen, drawn in the scramble theme: no built-in colour is left.
#[test]
fn scramble_reaches_the_gauge_trainer_graph_call_and_end_screens() {
    use crate::app::GaugeShown;
    use crate::storage::workout_file::Kind;
    use crate::ui::{self, Canvas};
    let look = scramble();
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.settings.ftp = 200;
    let bx = (0, 24, 240, 118);
    let mut left = Vec::new();
    let mut check = |sim: &Sim, what: &str| {
        for &k in KEYS.iter() {
            let n = sim.screen.iter().filter(|&&p| p == k).count();
            if n > 0 {
                left.push(std::format!("{what}: {k:#06x} x{n}"));
            }
        }
    };
    let fresh = |sim: &mut Sim| Panel(sim, (0, ui::H), look).fill(0, 0, ui::W, ui::H, ui::BLACK);
    // the gauge: the zones, the band's outline, the marker in the state's colour
    fresh(&mut sim);
    let mut value = crate::text::Line::new();
    value.push_u32(140);
    let band = (160, 180);
    let scale = crate::workout::gauge_scale(Kind::Power, band, 200, 190);
    let marker = Some(crate::workout::gauge_x(140, scale, ui::gauge_cols(bx)));
    let shown = GaugeShown {
        kind: Kind::Power,
        avg: 0,
        band: Some(band),
        step: true,
        pace: None,
        value,
        color: crate::workout::gauge_color(140, band),
        marker,
    };
    app.draw_gauge(&mut Panel(&mut sim, (0, ui::H), look), bx, shown);
    check(&sim, "gauge");
    // the trainer
    fresh(&mut sim);
    let now = ui::trainer::Now {
        mode: "ERG",
        source: "WORKOUT",
        target: "250 W",
        power: Some(240),
        cadence: Some(90),
        status: "CALIBRATE",
        status_color: ui::YELLOW,
        bottom: ui::trainer::Bottom::Cancel,
        timer: "12:34",
    };
    ui::trainer::draw(&mut Panel(&mut sim, (0, ui::H), look), &now);
    check(&sim, "trainer");
    // a graph with a bar in each zone
    fresh(&mut sim);
    ui::draw_graph(&mut Panel(&mut sim, (0, ui::H), look), (0, 40, 240, 120), &|i| {
        (10 + i % 100, crate::ride::power_color(1 + (i / 8) as u8 % 7))
    });
    check(&sim, "graph");
    // the call, then the end screen with each button state
    fresh(&mut sim);
    ui::draw_call(&mut Panel(&mut sim, (0, ui::H), look), "Work");
    check(&sim, "call");
    fresh(&mut sim);
    let mut p = Panel(&mut sim, (0, ui::H), look);
    ui::draw_end(&mut p, false, "10.5 KM", ui::END_SAVE);
    ui::draw_end(&mut p, true, "10.5 KM", 2);
    check(&sim, "end");
    assert!(left.is_empty(), "built-in colours drawn untranslated:\n{}", left.join("\n"));
}

/// THEME picked before settings.bin is read (no menu opened first, the file already there) is kept, OUTDOOR and
/// DARK alike (DARK over a file that says OUTDOOR): written, and read back after a restart.
#[test]
fn a_theme_picked_before_the_file_is_read_is_kept() {
    for first in ["OUTDOOR", "DARK"] {
        // a card with settings.bin written by a run that kept `first`
        let (mut sim, mut app) = unit(None);
        app.settings.set_theme_name(first);
        app.save_misc(&mut sim);
        for _ in 0..200 {
            run(&mut sim, &mut app, 200);
            if !app.settings_store.dirty {
                break;
            }
        }
        assert_eq!(kept(&sim).map(|c| c.settings.theme_name().to_string()).as_deref(), Some(first));
        // a start; THEME picked at once with the file unread
        let mut sim2 = Sim::new(9, 5, 0);
        sim2.own_settings();
        sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
        let mut app2 = App::start(&mut sim2, None);
        app2.settings.set_theme_name(first);
        app2.next_theme(&mut sim2); // the other of the two
        let picked = app2.settings.theme_name().to_string();
        assert_ne!(picked, first);
        for _ in 0..200 {
            run(&mut sim2, &mut app2, 200);
            if app2.settings_store.loaded && !app2.settings_store.dirty {
                break;
            }
        }
        assert_eq!(app2.settings.theme_name(), picked, "kept over the file's {first}");
        assert_eq!(kept(&sim2).map(|c| c.settings.theme_name().to_string()), Some(picked.clone()));
        let mut sim3 = Sim::new(9, 6, 0);
        sim3.own_settings();
        sim3.emmc.image = core::mem::take(&mut sim2.emmc.image);
        let mut app3 = App::start(&mut sim3, None);
        run(&mut sim3, &mut app3, u64::from(PAGES_LOAD_MS) + 300);
        assert_eq!(app3.settings.theme_name(), picked, "and after a restart");
    }
}

/// The Themes folder's listing (`App::theme_files`) from generated names: long (LFN), one character, non-ASCII,
/// `.INI` and `.ini`, other extensions, a built-in's name, sub-folders, spaces, and more than 14: exactly the valid
/// stems in directory order, at most 14; the seeds reach both the listed and the skipped.
#[test]
fn the_themes_folder_listing() {
    let (mut listed, mut skipped, mut capped) = (0usize, 0usize, 0u32);
    for seed in 1..60u32 {
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut d = Ram(crate::fat::tests::image(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let t = crate::fat::write::dos_time((2026, 10, 20, 8, 0, 0));
        let dir = fat.mkdir(&mut d, fat.root(), crate::recorder::DIR, t).unwrap();
        let dir = fat.mkdir(&mut d, dir, "Themes", t).unwrap();
        let mut want: Vec<String> = Vec::new();
        for i in 0..(rnd() % 60) {
            let (name, ok, folder) = match rnd() % 9 {
                0 => (std::format!("T{i}.INI"), true, false),
                1 => (std::format!("t{i}.ini"), true, false),
                2 => (std::format!("A{i}.ini"), true, false),
                3 => (std::format!("LongThemeName{i}.ini"), false, false),
                4 => (std::format!("Th\u{e9}m{i}.ini"), false, false),
                5 => (std::format!("N{i}.txt"), false, false),
                6 => ("dark.ini".to_string(), false, false),
                7 => (std::format!("SUB{i}.INI"), false, true),
                _ => (std::format!("my theme{i}.ini"), false, false),
            };
            let made = if folder {
                fat.mkdir(&mut d, dir, &name, t).is_ok()
            } else {
                fat.create(&mut d, dir, &name, t).is_ok()
            };
            if made && ok {
                want.push(name[..name.len() - 4].to_string());
            } else if made {
                skipped += 1;
            }
        }
        let mut sim = Sim::new(9, 0, 0);
        sim.own_settings();
        sim.emmc.image = d.0;
        let mut app = App::start(&mut sim, None);
        let (files, n) = app.theme_files(&mut sim);
        let got: Vec<String> = files[..n]
            .iter()
            .map(|f| String::from_utf8(f.iter().copied().take_while(|&c| c != 0).collect()).unwrap())
            .collect();
        capped += u32::from(want.len() > 14);
        want.truncate(14);
        assert_eq!(got, want, "seed {seed}");
        listed += got.len();
    }
    assert!(listed > 100 && skipped > 100 && capped > 0, "{listed} listed, {skipped} skipped, {capped} capped");
}
