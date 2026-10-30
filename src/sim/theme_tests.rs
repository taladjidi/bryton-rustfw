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

/// THEME cycles AUTO, DARK, OUTDOOR, the card's files; a refused file gives DARK and a notice; the name is kept in
/// settings.bin and applied after a restart; an older file (31 bytes of settings) reads with DARK.
#[test]
fn theme_from_the_card_is_picked_and_kept() {
    let (mut sim, mut app) =
        unit(Some(with_themes(&[("SUNNY.INI", SUNNY), ("REDZONE.INI", REDZONE), ("NOTES.TXT", "x")])));
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, menu::Row::Set(Setting::Theme));
    let mut seen: Vec<(String, Theme)> = Vec::new();
    for _ in 0..5 {
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 100);
        super::tests::assert_consistent(&sim, &app);
        let name = if app.settings.auto_theme { "AUTO" } else { app.settings.theme_name() };
        seen.push((name.to_string(), app.theme));
    }
    let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["OUTDOOR", "LIGHT", "SUNNY", "REDZONE", "AUTO"], "NOTES.TXT is no theme");
    assert_eq!(seen[4].1, Theme::OUTDOOR, "AUTO by day: the day theme");
    assert_eq!(seen[0].1, Theme::OUTDOOR);
    assert_eq!(seen[1].1, Theme::LIGHT);
    let sunny = crate::theme::parse(SUNNY.as_bytes()).unwrap();
    assert_eq!(seen[2].1, sunny);
    assert_eq!(seen[3].1, Theme::DARK, "refused: DARK drawn");
    // on to SUNNY (four more presses: DARK, OUTDOOR, LIGHT, SUNNY), kept, and applied after a restart
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Start));
    }
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
    let (mut sim, mut app) = started();
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
        pm: "PM+6%",
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
        app2.next_theme(&mut sim2, Setting::Theme); // the other of the two
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

/// LIGHT with its text one step off black, so a pixel of exactly BLACK can only be a background, a blend or an
/// outline that skipped the theme.
fn light_probe() -> Theme {
    let mut t = Theme::LIGHT;
    t.ink[1] = 0x0008;
    t
}

/// LIGHT on the main screens, the menus, banners, a graph, the gauge, the profile bars, the call and the end screen:
/// no pixel is the built-in black, the background around text and the cog is the theme's; shots for the gallery
/// (`theme_light_*`).
#[test]
fn light_assumes_no_black() {
    use crate::app::GaugeShown;
    use crate::storage::workout_file::Kind;
    use crate::ui::{self, Canvas};
    let white = Theme::LIGHT.ink[0];
    let no_black = |sim: &Sim, what: &str| {
        let n = sim.screen.iter().filter(|&&p| p == ui::BLACK).count();
        assert_eq!(n, 0, "{what}: {n} px still the built-in black");
    };
    let (mut sim, mut app) = unit(None);
    set_theme(&mut sim, &mut app, light_probe());
    scenes(&mut sim, &mut app, |sim, app, name| {
        super::tests::assert_consistent(sim, app);
        no_black(sim, name);
        if name == "overview" {
            let g = ui::GEAR_AT;
            assert_eq!(sim.screen[g.1 as usize * 240 + g.0 as usize], white, "the cog's corner is the background");
            assert_eq!(sim.screen[(ui::H as usize - 1) * 240 + 1], white, "the notices' area");
        }
    });
    // the real LIGHT: shots
    let (mut sim, mut app) = unit(None);
    set_theme(&mut sim, &mut app, Theme::LIGHT);
    scenes(&mut sim, &mut app, |sim, _, name| sim.save(&std::format!("theme_light_{name}")));
    // the screens the scenes do not reach, on the probe
    let look = light_probe();
    let (mut sim, mut app) = started();
    app.settings.ftp = 200;
    let bx = (0, 24, 240, 118);
    let fresh = |sim: &mut Sim| Panel(sim, (0, ui::H), look).fill(0, 0, ui::W, ui::H, ui::BLACK);
    fresh(&mut sim);
    ui::draw_banner(&mut Panel(&mut sim, (0, ui::H), look), "BATTERY LOW", true);
    no_black(&sim, "alarm banner");
    ui::draw_banner(&mut Panel(&mut sim, (0, ui::H), look), "CLIMB 2 KM", false);
    no_black(&sim, "info banner");
    fresh(&mut sim);
    ui::draw_graph(&mut Panel(&mut sim, (0, ui::H), look), (0, 40, 240, 120), &|i| {
        (10 + i % 100, crate::ride::power_color(1 + (i / 8) as u8 % 7))
    });
    no_black(&sim, "graph");
    fresh(&mut sim);
    ui::draw_profile_bars(&mut Panel(&mut sim, (0, ui::H), look), (30, 100, 80), 4, &[10, 30, 60, 80, 50, 20, 70]);
    no_black(&sim, "profile bars");
    fresh(&mut sim);
    let mut value = crate::text::Line::new();
    value.push_u32(140);
    let band = (160, 180);
    let scale = crate::workout::gauge_scale(Kind::Power, band, 200, 190);
    let shown = GaugeShown {
        kind: Kind::Power,
        avg: 0,
        band: Some(band),
        step: true,
        pace: None,
        value,
        color: crate::workout::gauge_color(140, band),
        marker: Some(crate::workout::gauge_x(140, scale, ui::gauge_cols(bx))),
    };
    app.draw_gauge(&mut Panel(&mut sim, (0, ui::H), look), bx, shown);
    no_black(&sim, "gauge");
    fresh(&mut sim);
    ui::draw_call(&mut Panel(&mut sim, (0, ui::H), look), "Work");
    no_black(&sim, "call");
    fresh(&mut sim);
    ui::draw_end(&mut Panel(&mut sim, (0, ui::H), look), false, "10.5 KM", ui::END_SAVE);
    no_black(&sim, "end");
}

/// The map page in LIGHT (the basemap under the arrow and the track): the basemap is a picture and stays as it is,
/// the arrow, its outline and the track go through the theme; a shot for the gallery.
#[test]
fn light_map_page() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(super::map_tests::tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    set_theme(&mut sim, &mut app, Theme::LIGHT);
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 60_000);
    super::tests::assert_consistent(&sim, &app);
    sim.save("theme_light_map");
    let count = |c: u16| sim.screen.iter().filter(|&&p| p == c).count();
    let ink = Theme::LIGHT.ink;
    assert!(count(ink[4]) > 20, "the track in LIGHT's accent: {}", count(ink[4]));
    assert!(count(ink[1]) > 40, "the arrow and the buttons in LIGHT's text: {}", count(ink[1]));
    assert_eq!(count(crate::ui::ACCENT), 0, "no built-in accent left");
}

/// AUTO (THEME) follows the ambient light: the first reading decides at once (bright: day); dark for 30 s goes to the
/// night theme (DARK), redrawn with its background; light for 30 s is held by the minute from the switch; then day.
#[test]
fn auto_theme_follows_the_light_with_a_hold_and_a_gap() {
    let (mut sim, mut app) = unit(None);
    app.settings.auto_theme = true;
    app.apply_theme(&mut sim, false); // the theme in use, as the settings file read does
    sim.bus.lux10 = Some(2000); // 200 lux: the first reading is day, at once
    run(&mut sim, &mut app, 3_000);
    assert_eq!(app.theme, Theme::OUTDOOR, "the first reading: day");
    sim.bus.lux10 = Some(50); // 5 lux: dark
    run(&mut sim, &mut app, 20_000);
    assert_eq!(app.theme, Theme::OUTDOOR, "20 s of dark: the hold is not over");
    run(&mut sim, &mut app, 15_000);
    assert_eq!(app.theme, Theme::DARK, "after the hold: the night theme");
    assert_eq!(sim.screen[0], Theme::DARK.ink(crate::ui::BLACK), "a background pixel in the night theme");
    sim.bus.lux10 = Some(2000);
    run(&mut sim, &mut app, 30_000);
    assert_eq!(app.theme, Theme::DARK, "30 s of light: the hold is met, the minute from the switch is not");
    run(&mut sim, &mut app, 40_000);
    assert_eq!(app.theme, Theme::OUTDOOR, "back to the day theme");
    assert_eq!(sim.screen[0], Theme::OUTDOOR.ink(crate::ui::BLACK), "a background pixel in the day theme");
}

/// THEME AUTO alone reads the light once a second; with AUTO off (and no backlight following the light) it stops.
#[test]
fn auto_theme_alone_reads_once_a_second_and_stops_when_off() {
    let (mut sim, mut app) = unit(None);
    app.settings.light_level = 0; // a fixed backlight: the light is read for the theme only
    app.settings.auto_theme = true;
    sim.bus.lux10 = Some(2000);
    run(&mut sim, &mut app, 2_000);
    let r0 = sim.bus.light_reads;
    run(&mut sim, &mut app, 20_000);
    let reads = sim.bus.light_reads - r0;
    assert!((18..=22).contains(&reads), "{reads} readings in 20 s, one a second");
    app.settings.auto_theme = false;
    run(&mut sim, &mut app, 3_000);
    let r1 = sim.bus.light_reads;
    run(&mut sim, &mut app, 10_000);
    assert_eq!(sim.bus.light_reads, r1, "AUTO off: no readings");
}

/// A DAY THEME file missing at load: its side falls back to OUTDOOR, with a notice.
#[test]
fn a_missing_day_file_falls_back_at_load_with_a_notice() {
    let (mut sim, mut app) = unit(None);
    app.settings.day_theme = Settings::name_bytes("GONE");
    app.load_day_night(&mut sim);
    assert_eq!(app.auto_themes, [Theme::OUTDOOR, Theme::DARK]);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("THEME NOT FOUND")), "the notice is listed");
}

/// THEME picked by hand while AUTO is on: AUTO is left, and the picked theme is drawn.
#[test]
fn a_manual_theme_pick_leaves_auto() {
    let (mut sim, mut app) = unit(None);
    app.settings.auto_theme = true;
    app.apply_theme(&mut sim, false);
    app.next_theme(&mut sim, Setting::Theme); // AUTO's next: DARK
    assert!(!app.settings.auto_theme);
    assert_eq!(app.settings.theme_name(), "DARK");
    assert_eq!(app.theme, Theme::DARK);
}

/// Day and night the same theme: the switch changes the state but the theme drawn stays (no redraw).
#[test]
fn a_switch_between_equal_themes_keeps_the_theme() {
    let (mut sim, mut app) = unit(None);
    app.settings.auto_theme = true;
    app.settings.night_theme = Settings::name_bytes("OUTDOOR");
    app.load_day_night(&mut sim);
    app.apply_theme(&mut sim, false);
    sim.bus.lux10 = Some(2000);
    run(&mut sim, &mut app, 3_000);
    sim.bus.lux10 = Some(50);
    run(&mut sim, &mut app, 40_000);
    assert!(app.light.day_night.night, "the switch is made");
    assert_eq!(app.theme, Theme::OUTDOOR, "the same theme on both sides");
}

/// A theme with `menu_row` and `zone_bar` / `frame` set (the rest DARK).
fn metrics(menu_row: u32, zone_bar: u32, frame: u32) -> Theme {
    Theme { menu_row, zone_bar, frame, ..Theme::DARK }
}

/// `menu_row` 44 and 56 (theme metrics): the main menu draws with the pitch, and a tap on the first and last pixel of
/// each row's band lands on that row (as the row drawn), never on its neighbour; shots for the gallery.
#[test]
fn a_tap_lands_on_the_row_drawn_at_44_and_56() {
    for row in [44, 56] {
        let (mut sim, mut app) = unit(None);
        set_theme(&mut sim, &mut app, metrics(row, 8, 3));
        app.press(&mut sim, Press::Long(Key::Page));
        run(&mut sim, &mut app, 200);
        assert_eq!(app.menu_rows(), if row == 44 { 6 } else { 4 });
        sim.save(&std::format!("metrics_menu_{row}"));
        let mut opened = 0;
        for slot in 0..app.menu_rows() {
            let y0 = crate::ui::LIST_Y + slot as u32 * row;
            // the rule under the row is its last pixel row
            assert_eq!(sim.screen[((y0 + row - 1) * crate::ui::W + 100) as usize], crate::ui::DARK, "row {row}");
            for y in [y0, y0 + row / 2, y0 + row - 1] {
                // the menu turns as the selection moves: the row drawn in this slot is first + slot
                let Screen::Menu(Menu::Main, sel) = app.screen() else { panic!("{:?}", app.screen()) };
                let want = menu::first_shown(sel, app.menu_rows(), app.rows(Menu::Main)) + slot;
                let Some(menu::Row::Open(m)) = app.row(Menu::Main, want) else { continue };
                app.input(&mut sim, Input::Touch(Gesture::Tap(120, y as u16)));
                // the row tapped is selected or opened (a tap acts, unless the ride moves): never its neighbour
                let on = app.screen();
                assert!(
                    on == Screen::Menu(m, 0) || on == Screen::Menu(Menu::Main, want),
                    "{row} px, slot {slot}, y {y}: {on:?}"
                );
                super::tests::assert_consistent(&sim, &app);
                if on != Screen::Menu(Menu::Main, want) {
                    app.press(&mut sim, Press::Short(Key::Back));
                    assert_eq!(app.screen(), Screen::Menu(Menu::Main, want), "back to the row");
                }
                opened += 1;
            }
        }
        assert!(opened >= 9, "{opened} taps checked at {row} px");
    }
}

/// A menu longer than a screen turns by the screen's rows at the theme's pitch (4 at 56 px, 6 at 44): the last row of
/// a screen is reached by a tap on the bottom pixel, and the first of the next is the row after it.
#[test]
fn a_long_menu_turns_by_the_rows_of_the_pitch() {
    for row in [44u32, 56] {
        let (mut sim, mut app) = unit(None);
        set_theme(&mut sim, &mut app, metrics(row, 8, 3));
        app.press(&mut sim, Press::Long(Key::Page));
        super::tests::to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
        app.press(&mut sim, Press::Short(Key::Start));
        let n = app.menu_rows();
        assert!(app.rows(Menu::Sensors) > n);
        let end = crate::ui::LIST_Y + n as u32 * row;
        app.input(&mut sim, Input::Touch(Gesture::Tap(120, (end - 1) as u16)));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, n - 1), "{row} px: the bottom edge is the last slot");
        // the blank band under the rows (past 4 px) is no row: nothing is selected or opened
        app.input(&mut sim, Input::Touch(Gesture::Tap(120, (end + 5) as u16)));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, n - 1), "{row} px: the blank band is no row");
        super::tests::assert_consistent(&sim, &app);
    }
}

/// FNV-1a of the panel's pixels.
fn hash(screen: &[u16]) -> u64 {
    screen.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &p| (h ^ u64::from(p)).wrapping_mul(0x100_0000_01b3))
}

/// The built-in themes draw as they did before the metrics: the main menu and a ride page in DARK hash to the values
/// taken from the same test on the commit before (main, 2026-10-30).
#[test]
fn dark_draws_as_before_the_metrics() {
    let (mut sim, mut app) = unit(None);
    app.press(&mut sim, Press::Long(Key::Page));
    run(&mut sim, &mut app, 200);
    let menu = hash(&sim.screen);
    app.press(&mut sim, Press::Short(Key::Back));
    run(&mut sim, &mut app, 200);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1100);
    let ride = hash(&sim.screen);
    assert_eq!((menu, ride), (0x61c6_3905_740b_d4e1, 0xa6ce_182a_d71d_10ae));
}
