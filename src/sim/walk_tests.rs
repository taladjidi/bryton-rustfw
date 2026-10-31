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
const KNOWN: &[(&str, &str, &str)] = &[];

struct Walk {
    n: usize,
    /// The keys from the overview to where the walk is.
    keys: Vec<&'static str>,
    /// The screen being exercised (its path), to name the cuts.
    cur: String,
    /// (screen path, keys from the overview).
    paths: Vec<(String, Vec<&'static str>)>,
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
        "lap" => Press::Short(Key::Lap),
        "start" => Press::Short(Key::Start),
        "back" => Press::Short(Key::Back),
        _ => Press::Long(Key::Page),
    };
    w.keys.push(k);
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
            key(sim, app, w, "start");
            match app.screen() {
                Screen::Menu(m2, 0) if m2 != m && !w.seen.contains(&m2) => {
                    visit(sim, app, w, m2, &format!("{name}/{}", menu_title(m2)));
                }
                Screen::Menu(..) => {}
                _ => {
                    w.cur = format!("{name}/{label}");
                    run(sim, app, 300);
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
    app.settings.ftp = 200;
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    // a sprinter's curve: 4-digit watts on the axis and in the readout
    for (n, w) in app.stock.out.curve.watts.iter_mut().enumerate() {
        *w = 1450u16.saturating_sub(n as u16 * 90).max(180);
    }
    (sim, app)
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
    key(&mut sim, &mut app, w, "long_page");
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
    key(&mut sim, &mut app, w, "long_page");
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
    std::fs::create_dir_all(&dir).unwrap();
    pass(&mut w, false);
    pass(&mut w, true);

    let json: Vec<String> = w
        .paths
        .iter()
        .enumerate()
        .map(|(i, (n, k))| {
            let ks: Vec<String> = k.iter().map(|s| format!("\"{s}\"")).collect();
            format!("{{\"n\":{},\"name\":\"{n}\",\"keys\":[{}]}}", i + 1, ks.join(","))
        })
        .collect();
    std::fs::write(dir.join("paths.json"), format!("[\n{}\n]\n", json.join(",\n"))).unwrap();
    let bad: Vec<_> =
        w.found.iter().filter(|(p, c)| !KNOWN.iter().any(|k| p.contains(k.0) && c.1.starts_with(k.1))).collect();
    std::println!("walk: {} screens, {} clips, {:?}", w.n, w.found.len(), t0.elapsed());
    assert!(
        bad.is_empty(),
        "text cut by its box:\n{}",
        bad.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>().join("\n")
    );
}
