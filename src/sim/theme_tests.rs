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
    "[theme]\nversion = 1\nname = SUNNY\nbase = outdoor\n[colors]\naccent = #00AAFF ; blue\nzone1 = #123456\n";
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
