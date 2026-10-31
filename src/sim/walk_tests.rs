//! The screen walk: every menu screen reached by keys, depth-first, each setting cycled through its values, the ride
//! pages and the ride menu with a ride started, on synthetic state with big ids and numbers, metric and imperial. Text
//! cut by its box (`text::clips`) is a failure unless listed in [`KNOWN`]. Screens go to
//! `target/sim/walk/NNN-<path>.bmp` and the key sequences that reach them to `target/sim/walk/paths.json` (a later
//! tool replays them on the unit with tools/dev.py key).

use super::tests::{KEY, run};
use super::*;
use crate::menu::{Row, StockRow};
use crate::text::clips::{self, Clip};
use std::string::String;

/// Clippings not fixed yet: (screen path contains, string prefix, why).
const KNOWN: &[(&str, &str, &str)] = &[
    (
        "BLE NEARBY",
        "DECATH TRAINER 4",
        "the list shows 12 characters at 2x; a long device name needs a 1x font or two lines (layout decision)",
    ),
    (
        "BLE NEARBY",
        "Wahoo KICKR 5A3F",
        "the list shows 12 characters at 2x; a long device name needs a 1x font or two lines (layout decision)",
    ),
    (
        "BLE NEARBY",
        "HRM-Dual 1234567",
        "the list shows 12 characters at 2x; a long device name needs a 1x font or two lines (layout decision)",
    ),
];

/// The menus whose rows are the const tables (MAIN, SETTINGS, ME, ZONES, DISPLAY, BIKE, ALERTS, REMOTE = DI2 KEYS) or
/// depend on nothing stored (PAGES: three profiles and fixed rows): a Start there lands on the same row on the unit.
/// Every other menu (sensors, places, bikes, history, the ride menu, SYSTEM, storage...) lists data.
fn static_menu(m: Menu) -> bool {
    matches!(
        m,
        Menu::Main
            | Menu::Settings
            | Menu::Me
            | Menu::Zones
            | Menu::Display
            | Menu::Bike
            | Menu::Alerts
            | Menu::Pages
            | Menu::Remote
    )
}

struct Walk {
    n: usize,
    /// The keys from the overview to where the walk is, each with whether it is a Start pressed inside a menu whose
    /// rows depend on data (tools/walk.py refuses such a path: the unit's rows differ from the sim's).
    keys: Vec<(&'static str, bool)>,
    /// The screen being exercised (its path), to name the cuts.
    cur: String,
    /// (screen path, keys from the overview).
    paths: Vec<(String, Vec<(&'static str, bool)>)>,
    seen: Vec<Menu>,
    found: Vec<(String, Clip)>,
}

impl Walk {
    /// Notes the cuts since the last call.
    fn drain(&mut self) {
        for c in clips::take() {
            if !self.found.iter().any(|f| f.0 == self.cur && f.1 == c) {
                self.found.push((self.cur.clone(), c));
            }
        }
    }

    /// Saves the screen as this path's.
    fn shot(&mut self, sim: &Sim) {
        self.n += 1;
        let file: String =
            self.cur.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        sim.save(&format!("walk/{:03}-{file}", self.n));
        self.paths.push((self.cur.clone(), self.keys.clone()));
    }
}

fn key(sim: &mut Sim, app: &mut App, w: &mut Walk, k: &'static str) {
    let p = match k {
        "page" => Press::Short(Key::Page),
        "page --long" => Press::Long(Key::Page),
        "lap" => Press::Short(Key::Lap),
        "start" => Press::Short(Key::Start),
        "back" => Press::Short(Key::Back),
        _ => unreachable!("unknown key {k}"),
    };
    let dynamic = k == "start" && matches!(app.screen(), Screen::Menu(m, _) if !static_menu(m));
    w.keys.push((k, dynamic));
    app.press(sim, p);
    run(sim, app, 30);
    w.drain();
}

/// Backs out until the menu `m` shows again (a screen opened from a row), at most four steps.
fn back_to(sim: &mut Sim, app: &mut App, w: &mut Walk, m: Menu, i: usize) {
    for _ in 0..4 {
        if matches!(app.screen(), Screen::Menu(x, _) if x == m) {
            break;
        }
        key(sim, app, w, "back");
    }
    app.screen = Screen::Menu(m, i);
}

/// Rows worth opening: a screen, a card or a list. (Import, the drive and the like act on the card: not walked.)
fn opens(row: Row) -> bool {
    match row {
        Row::Open(m) => m != Menu::Main,
        Row::Stock(r) => r != StockRow::Import,
        Row::Nearby | Row::About | Row::OldRide(_) | Row::Place(_) | Row::Part(..) | Row::Past(_) => true,
        Row::Workout(_) | Row::Notif(_) | Row::Month(_) => true,
        _ => false,
    }
}

fn visit(sim: &mut Sim, app: &mut App, w: &mut Walk, m: Menu, name: &str) {
    w.seen.push(m);
    w.cur = name.into();
    w.drain();
    w.shot(sim);
    let base = w.keys.len();
    let mut cards = 0; // rows of a long list that open a card: the first few
    for i in 0..app.rows(m) {
        w.cur = name.into();
        if i > 0 {
            key(sim, app, w, "page");
        }
        let here = w.keys.len();
        let Some(row) = app.row(m, i) else { break };
        let label: String = app.menu_row(m, i).map_or(String::new(), |r| r.0.as_str().into());
        if let Row::Set(s) = row {
            w.cur = format!("{name}/{label}");
            if s.number().is_some() {
                key(sim, app, w, "start");
                for k in ["lap", "page"] {
                    for _ in 0..40 {
                        key(sim, app, w, k);
                    }
                }
                key(sim, app, w, "start");
            } else {
                for _ in 0..12 {
                    key(sim, app, w, "start");
                }
            }
        } else if opens(row) && !matches!(row, Row::Open(m2) if w.seen.contains(&m2)) && cards < 4 {
            cards += usize::from(!matches!(row, Row::Open(_) | Row::Stock(_)));
            rich(app);
            key(sim, app, w, "start");
            if app.screen() == Screen::Nearby {
                run(sim, app, 1200); // the scan has cleared and listed its own first
                // devices as a scan would list them: long names, a manufacturer's
                for (n, name) in ["DECATH TRAINER 44945", "Wahoo KICKR 5A3F", "HRM-Dual 123456789"].iter().enumerate() {
                    let mut ad = vec![name.len() as u8 + 1, 0x09];
                    ad.extend(name.as_bytes());
                    let r = crate::nrf::AdvReport { addr_type: 1, addr: [0x50 + n as u8; 6], rssi: -100, data: &ad };
                    app.radio.nearby.add(&r);
                }
                app.show(sim);
                run(sim, app, 1100);
            }
            match app.screen() {
                Screen::Menu(m2, 0) if m2 != m && !w.seen.contains(&m2) => {
                    visit(sim, app, w, m2, &format!("{name}/{}", menu_title(m2)));
                }
                Screen::Menu(..) => {}
                _ => {
                    w.cur = format!("{name}/{label}");
                    run(sim, app, 300);
                    if app.screen() == Screen::Past {
                        // the card of a long hard ride, whatever the file read says
                        let big = crate::ui::card::Stats {
                            start: ((2026, 10, 30), (10, 17)),
                            timer_s: 40_000,
                            dist_m: 312_400,
                            avg_speed_x10: Some(1093),
                            ascent_m: Some(2345),
                            np: Some(420),
                            avg_power: Some(380),
                            avg_hr: Some(187),
                            max_hr: Some(199),
                            tss: Some(1234),
                            kj: Some(4321),
                        };
                        app.past.card = crate::ui::card::Card::of(&big, app.settings.imperial);
                        app.show(sim);
                    }
                    w.drain();
                    w.shot(sim);
                }
            }
            back_to(sim, app, w, m, i);
        }
        w.keys.truncate(here);
        app.screen = Screen::Menu(m, i);
    }
    w.keys.truncate(base);
}

fn power1200(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 0xb0, 0x04]
}

/// The unit with stock's rides read, every sensor paired with a 5-digit id, a power meter and a fix.
fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.gnss.fix = Some(8000);
    for (n, k) in Kind::ALL.into_iter().enumerate() {
        let id = SensorId { device: 44945 - n as u16, trans: 5 };
        sim.backup[k.backup()] = k.to_word(id);
        if k == Kind::Power {
            sim.nordic.sensors.push(AntSensor {
                device_type: 0x0b,
                id: id.device,
                data: power1200,
                network: 0,
                ack: &[],
            });
        }
    }
    let mut app = App::start(&mut sim, Some(KEY));
    app.set_bike_memory(std::boxed::Box::leak(std::boxed::Box::new(crate::BikeStore::default())));
    app.settings.ftp = 200;
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    // three bikes, 5-digit km odometers, parts 13 km from a 5000 KM service
    if let Some(s) = app.bikes.as_deref_mut() {
        for b in &mut s.list.list {
            b.odo_m = 45_678_900;
            b.parts = [crate::bikes::Part { m: 4_987_000, every_km: 5000 }; crate::bikes::PARTS];
        }
    }
    rich(&mut app);
    (sim, app)
}

/// Big synthetic numbers in what the scan read (again before each screen: a scan may replace them): rides of 300 km,
/// 2000 m of ascent and 4000 kJ, a 1450 W curve, and heavy rides in the last days, so the form is far under -100.
fn rich(app: &mut App) {
    let rides: Vec<_> = (0..app.stock.index.len()).filter_map(|i| app.stock.index.get(i).cloned()).collect();
    for (i, mut r) in rides.into_iter().enumerate() {
        app.stock.index.remove(&r.file, r.start);
        (r.dist_m, r.timer_s, r.ascent_m, r.kj) = (312_400 + i as u32 * 1000, 40_000, Some(2345 + i as u16), 4321);
        (r.np, r.avg_power, r.power_s) = (Some(420), Some(380), 14_400);
        app.stock.index.add(r);
    }
    for (n, w) in app.stock.out.curve.watts.iter_mut().enumerate() {
        *w = 1450u16.saturating_sub(n as u16 * 90).max(180);
    }
}

/// One pass: the whole menu tree, the overview under each date format, then a ride started with its pages and menu.
fn pass(w: &mut Walk, imperial: bool) {
    let (mut sim, mut app) = unit();
    app.settings.imperial = imperial;
    let tag = if imperial { "IMP" } else { "MET" };
    w.seen.clear();
    w.keys.clear();
    w.cur = format!("{tag} OVERVIEW");
    w.shot(&sim);
    key(&mut sim, &mut app, w, "page --long");
    visit(&mut sim, &mut app, w, Menu::Main, &format!("{tag} MAIN"));
    app.screen = Screen::Main;
    for _ in 0..3 {
        app.settings.date_fmt = app.settings.date_fmt.next();
        run(&mut sim, &mut app, 1100);
        w.cur = format!("{tag} OVERVIEW {:?}", app.settings.date_fmt);
        w.drain();
    }
    w.keys.clear();
    w.seen.clear();
    key(&mut sim, &mut app, w, "start");
    run(&mut sim, &mut app, 5000);
    for n in 1..=app.page_count() + 1 {
        key(&mut sim, &mut app, w, "page");
        run(&mut sim, &mut app, 1500);
        w.cur = format!("{tag} RIDE PAGE {n}");
        w.drain();
        w.shot(&sim);
    }
    key(&mut sim, &mut app, w, "page --long");
    if let Screen::Menu(m, 0) = app.screen() {
        visit(&mut sim, &mut app, w, m, &format!("{tag} RIDE MENU"));
    }
}

/// The detector sees a string cut by its box, and not one inside `may_truncate`.
#[test]
fn the_detector_sees_a_cut_label() {
    use crate::text::{Align, draw_in};
    let mut f = crate::ui::tests::Frame::new();
    clips::take();
    draw_in(&mut f, &crate::ui::LABEL, 0, 0, 48, Align::Left, "CASSETTE");
    clips::may_truncate(|| draw_in(&mut f, &crate::ui::LABEL, 0, 0, 48, Align::Left, "CASSETTE"));
    assert_eq!(clips::take(), [("draw_in", "CASSETTE".into(), 48)]);
}

#[test]
fn walk_every_screen() {
    let t0 = std::time::Instant::now();
    let mut w =
        Walk { n: 0, keys: Vec::new(), cur: String::new(), paths: Vec::new(), seen: Vec::new(), found: Vec::new() };
    let dir = out_dir().join("walk");
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}");
    }
    std::fs::create_dir_all(&dir).unwrap();
    pass(&mut w, false);
    pass(&mut w, true);

    let json: Vec<String> = w
        .paths
        .iter()
        .enumerate()
        .map(|(i, (n, k))| {
            let ks: Vec<String> = k.iter().map(|s| format!("\"{}\"", s.0)).collect();
            let dynamic = k.iter().any(|s| s.1);
            format!("{{\"n\":{},\"name\":\"{n}\",\"dyn\":{dynamic},\"keys\":[{}]}}", i + 1, ks.join(","))
        })
        .collect();
    std::fs::write(dir.join("paths.json"), format!("[\n{}\n]\n", json.join(",\n"))).unwrap();
    let bad: Vec<_> =
        w.found.iter().filter(|(p, c)| !KNOWN.iter().any(|k| p.contains(k.0) && c.1.starts_with(k.1))).collect();
    for k in KNOWN {
        assert!(
            w.found.iter().any(|(p, c)| p.contains(k.0) && c.1.starts_with(k.1)),
            "stale KNOWN entry: {} {:?}",
            k.0,
            k.1
        );
    }
    std::println!("walk: {} screens, {} clips, {:?}", w.n, w.found.len(), t0.elapsed());
    assert!(
        bad.is_empty(),
        "text cut by its box:\n{}",
        bad.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>().join("\n")
    );
}
