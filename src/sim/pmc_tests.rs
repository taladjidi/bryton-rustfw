//! Training load on the Sim (app/pmc.rs, ui/load.rs): CTL, ATL and TSB from stock's synthetic rides, the TRAINING LOAD
//! chart and the MONTHS rows. The screens are saved as `target/sim/load_*.bmp`.

use super::menu_tests::to_row;
use super::tests::run;
use super::tests::*;
use super::*;
use crate::history::{Index, NONE, Ride};
use crate::load::{self, SHOWN};
use crate::menu::{Row, StockRow};
use crate::ui::{self, Field, load::PLOT};

const ID: SensorId = SensorId { device: 7, trans: 5 };

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The unit with stock's rides read, the fields CTL, ATL and TSB on the first page and a power meter at 210 W.
fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(ID);
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    let w = PageCfg { layout: 4, fields: [Field::Ctl, Field::Atl, Field::Tsb, Field::Tss, Field::Time, Field::Time] }
        .to_words();
    (sim.backup[26], sim.backup[27]) = (w[0], w[1]);
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    app.settings.ftp = 200;
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    assert!(app.stock.ready() && app.pmc.load.is_built(), "stock's rides are read and the load built");
    (sim, app)
}

fn text(app: &App, f: Field) -> std::string::String {
    app.field_text(f).as_str().into()
}

/// Pixels of `color` in the plot.
fn count(sim: &Sim, color: u16) -> usize {
    let (x0, y0, w, h) = (PLOT.0 as usize, PLOT.1 as usize, PLOT.2 as usize, PLOT.3 as usize);
    (y0..y0 + h).flat_map(|y| &sim.screen[y * 240 + x0..y * 240 + x0 + w]).filter(|&&p| p == color).count()
}

fn to_stock_row(sim: &mut Sim, app: &mut App, r: StockRow) {
    app.press(sim, Press::Long(Key::Page));
    to_row(sim, app, Row::Open(Menu::History));
    app.press(sim, Press::Short(Key::Start));
    to_row(sim, app, Row::Open(Menu::Stock));
    app.press(sim, Press::Short(Key::Start));
    to_row(sim, app, Row::Stock(r));
}

/// The fields show the scan's load; the month rows sum the same rides as the scan's totals and the TSS of each ride;
/// a ride on its way adds to fatigue; a new FTP builds the load again.
#[test]
fn the_load_from_stocks_rides() {
    let (mut sim, mut app) = unit();
    let ix = &app.stock.index;
    let tss = |ftp| {
        (0..16)
            .map_while(|i| ix.get(i))
            .map(|r| load::ride_tss(r, ftp, app.settings.lthr, app.settings.max_hr, false))
            .collect::<std::vec::Vec<_>>()
    };
    assert!(tss(200).iter().flatten().count() >= 2, "the two rides with power have a TSS: {:?}", tss(200));
    let want: u32 = tss(200).iter().flatten().sum();
    let m = app.pmc.load.months();
    assert_eq!(m.iter().map(|m| m.rides).sum::<u16>(), app.stock.out.all.rides, "every counted ride is in a month");
    assert_eq!(m.iter().map(|m| u32::from(m.tss)).sum::<u32>(), want);
    let km: u32 = (0..16).map_while(|i| ix.get(i)).map(|r| r.dist_m / 100).sum();
    assert_eq!(m.iter().map(|m| u32::from(m.dist_100m)).sum::<u32>(), km);
    assert!(m.len() >= 2 && m[0].ym > m[1].ym, "October 2026 and September: newest first");
    for f in [Field::Ctl, Field::Atl, Field::Tsb] {
        assert!(text(&app, f).parse::<i32>().is_ok(), "{f:?} reads a number: {}", text(&app, f));
    }
    let (ctl, atl) = (text(&app, Field::Ctl).parse::<i32>().unwrap(), text(&app, Field::Atl).parse::<i32>().unwrap());
    assert!(ctl > 0 && atl > ctl, "fresh rides: fatigue over fitness ({ctl} {atl})");
    // a ride: 20 minutes of 210 W on top of today's
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1_200_000);
    let after = text(&app, Field::Atl).parse::<i32>().unwrap();
    assert!(after > atl + 2, "the ride on its way adds to fatigue: {atl} -> {after}");
    // a lower FTP is a higher TSS for the same rides
    app.press(&mut sim, Press::Long(Key::Start));
    let was = app.pmc.load.months().iter().map(|m| u32::from(m.tss)).sum::<u32>();
    app.settings.ftp = 150;
    run(&mut sim, &mut app, 3000);
    assert!(app.pmc.load.months().iter().map(|m| u32::from(m.tss)).sum::<u32>() > was, "built again at the new FTP");
    assert_consistent(&sim, &app);
}

/// FITNESS > TRAINING LOAD: both lines and the bars draw, the cursor reads a day, a tap picks one, Back leaves.
#[test]
fn the_chart() {
    let (mut sim, mut app) = unit();
    to_stock_row(&mut sim, &mut app, StockRow::Load);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Load(SHOWN - 1), "on today");
    assert!(
        count(&sim, ui::ACCENT) > 50 && count(&sim, ui::YELLOW) > 50,
        "{} {}",
        count(&sim, ui::ACCENT),
        count(&sim, ui::YELLOW)
    );
    assert!(count(&sim, ui::GREEN) + count(&sim, ui::RED) > 20, "the form's bars");
    sim.save("load_dark");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Load(0), "round");
    app.input(&mut sim, Input::Touch(Gesture::Tap(PLOT.0 as u16 + 2 * 30 + 1, PLOT.1 as u16 + 20)));
    assert_eq!(app.screen(), Screen::Load(30));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_ne!(app.screen(), Screen::Load(30), "Back leaves");
    assert_consistent(&sim, &app);
    // LIGHT, for the gallery
    app.open_load();
    app.theme = ui::theme::Theme::LIGHT;
    app.repaint(&mut sim);
    sim.save("load_light");
    assert_ne!(count(&sim, ui::theme::Theme::LIGHT.ink(ui::ACCENT)), 0);
}

/// Without a scan the chart says so and the fields have no numbers.
#[test]
fn nothing_read_yet() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert_eq!(text(&app, Field::Ctl), "--");
    app.open_load();
    app.repaint(&mut sim);
    assert_consistent(&sim, &app);
}

/// FITNESS > MONTHS: a row per month; Select changes what they show.
#[test]
fn the_months() {
    let (mut sim, mut app) = unit();
    to_stock_row(&mut sim, &mut app, StockRow::Months);
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Months, _)));
    let row = |app: &App, i| {
        app.menu_row(Menu::Months, i)
            .map(|(l, v, _)| (std::string::String::from(l.as_str()), std::string::String::from(v.as_str())))
    };
    let (label, value) = row(&app, 0).unwrap();
    assert_eq!(label, "OCT 26");
    assert!(value.ends_with(" KM"), "{value}");
    sim.save("load_months");
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..5 {
        app.press(&mut sim, Press::Short(Key::Start));
        seen.insert(std::string::String::from(row(&app, 0).unwrap().1.split(' ').next_back().unwrap()));
    }
    assert_eq!(seen.len(), 5, "km, hours, rides, ascent, TSS: {seen:?}");
    assert_eq!(row(&app, 0).unwrap().1, value, "and round");
    assert_consistent(&sim, &app);
}

/// Fuzz: random keys and taps on the chart and the months never panic, the cursor stays on a day; and the build on
/// random indexes (dates before 1970 and after today, months 0 to 13, huge values, a full index) never panics and
/// keeps its sums within the rides. Both must have reached the code.
#[test]
fn fuzz_the_load() {
    let (mut sim, mut app) = unit();
    let mut reached = 0;
    for seed in 1..=200u32 {
        let mut x = seed.wrapping_mul(2_654_435_761);
        let mut next = move || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x >> 8
        };
        if seed % 2 == 0 {
            app.open_load();
        } else {
            app.open(Screen::Menu(Menu::Months, 0));
        }
        for _ in 0..6 {
            let i = match next() % 6 {
                0 => Input::Next,
                1 => Input::Prev,
                2 => Input::Select,
                3 | 4 => Input::Touch(Gesture::Tap((next() % 240) as u16, (next() % 320) as u16)),
                _ => Input::Touch(Gesture::Swipe(Dir::Up)),
            };
            app.input(&mut sim, i);
            match app.screen() {
                Screen::Load(k) => {
                    assert!(k < SHOWN);
                    reached += 1;
                }
                Screen::Menu(Menu::Months, _) => reached += 1,
                _ => break,
            }
        }
        run(&mut sim, &mut app, 100);
    }
    assert!(reached > 400, "{reached}");
    let mut built = 0;
    for seed in 1..=300u32 {
        let mut x = seed.wrapping_mul(2_246_822_519);
        let mut next = move || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x >> 4
        };
        let mut ix = Index::new();
        for _ in 0..next() % 200 {
            let start = ((next() % 3000) as u16, (next() % 15) as u8, (next() % 33) as u8, 0, 0);
            ix.add(Ride {
                start,
                timer_s: next(),
                dist_m: next(),
                ascent_m: Some(next() as u16).filter(|_| next() % 2 == 0),
                np: Some(next() as u16).filter(|_| next() % 2 == 0),
                power_s: next(),
                avg_hr: Some(next() as u8).filter(|_| next() % 2 == 0),
                ..NONE
            });
        }
        let today = (next() % 30_000) as u16;
        let mut l = load::Load::new();
        let (ftp, female) = ((next() % 400) as u16, next() % 2 == 0);
        l.build(today, &ix, |r| load::ride_tss(r, ftp, 160, 190, female));
        let d = l.days(next() % 100_000);
        assert!(d.iter().all(|d| d.ctl >= 0 && d.atl >= 0), "averages of non-negative loads");
        assert!(l.months().len() <= load::MONTHS);
        assert!(l.months().iter().map(|m| u32::from(m.rides)).sum::<u32>() <= 200);
        built += usize::from(l.months().iter().any(|m| m.rides > 0) && d.iter().any(|d| d.tss > 0));
    }
    assert!(built > 100, "the random indexes reached the sums: {built}");
}
