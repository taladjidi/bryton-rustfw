//! The simulator tests: the whole app on [`Sim`], driven by keys, a finger, sensors and fixes as functions of
//! time, checked on what the panel shows.

use super::*;
use crate::ui::tests::Frame;
use crate::ui::{Field, Rec, Strip, UsbDot};
/// A stand-in for the ANT+ network key (the real one is never in the repository).
const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
/// The owner's strap, as the unit paired it.
const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

fn hr72(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 72]
}
fn hr80(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 80]
}
/// Speed and cadence: every second one crank turn and three wheel turns.
fn spd_cad(ms: u64) -> [u8; 8] {
    let (t, n) = (((ms / 1000) * 1024) as u16, (ms / 1000) as u16);
    let [t0, t1] = t.to_le_bytes();
    let ([c0, c1], [w0, w1]) = (n.to_le_bytes(), (3 * n).to_le_bytes());
    [t0, t1, c0, c1, t0, t1, w0, w1]
}
fn power300(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01]
}

/// Passes of the main loop for `ms`: a step, then the sleep, in which (as on the unit) the touch panel is read
/// when INT falls (here: seen within a [`TOUCH_MS`] slice) and every [`TOUCH_MS`] while a finger is down.
fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end && !sim.standby {
        app.step(sim);
        for _ in 0..POLL_MS / TOUCH_MS {
            sim.delay_ms(TOUCH_MS);
            if let Some(i) = app.touch(sim, TOUCH_MS) {
                app.input(sim, i);
            }
        }
    }
}

/// The screen drawn step by step on the panel equals the screen drawn at once from the state (the screenshot).
fn assert_consistent(sim: &Sim, app: &App) {
    for y in 0..H {
        let (drawn, panel) = (app.render_row(y).px, &sim.screen[(y * W) as usize..((y + 1) * W) as usize]);
        let bad: Vec<_> = (0..W as usize).filter(|&x| drawn[x] != panel[x]).collect();
        assert!(bad.is_empty(), "row {y}, x {bad:?}: screenshot {:04x}, panel {:04x}", drawn[bad[0]], panel[bad[0]]);
    }
}

/// Ride page `n` of `app` as the drawing code means it, with these values, under a strip at the sim's time
/// (`radio`: the Nordic chip answered).
fn ride(sim: &mut Sim, app: &App, n: usize, values: &[&str], radio: bool) -> Vec<u16> {
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    let mut f = Frame::new();
    crate::ui::draw_strip(
        &mut f,
        &Strip {
            time: (h, m),
            radio,
            gps: app.strip.gps,
            blink: app.strip.blink,
            usb: UsbDot::Off,
            battery: Some(63),
            charging: false,
            rec: app.strip.rec,
        },
    );
    let page = app.pages()[n];
    for (i, v) in values.iter().enumerate() {
        let bx = crate::ui::field_box(page.layout, crate::ui::RIDE_H, i).unwrap();
        crate::ui::draw_field_frame(&mut f, bx, page.fields[i]);
        crate::ui::draw_field_value(&mut f, bx, page.fields[i], v, zone_fg(page.fields[i], v));
    }
    f.px
}

/// Heart rate and power in their zone's colour (default max HR and FTP), the rest white.
fn zone_fg(f: Field, v: &str) -> u16 {
    match (f, v.parse::<u16>()) {
        (Field::Hr, Ok(v)) => crate::ride::hr_color(crate::ride::hr_zone(v, crate::ride::MAX_HR)),
        (Field::Power, Ok(v)) => crate::ride::power_color(crate::ride::power_zone(v, crate::ride::FTP)),
        _ => crate::ui::WHITE,
    }
}

fn hm(sim: &mut Sim) -> std::string::String {
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    format!("{h:02}:{m:02}")
}

/// The main screen as the drawing code means it, at the sim's time, under a strip with `usb`.
fn main_screen(sim: &mut Sim, app: &App, usb: UsbDot) -> Vec<u16> {
    let (h, m, s) = rtc_time_from_tr(sim.rtc().0);
    let mut f = Frame::new();
    crate::ui::draw_strip(
        &mut f,
        &Strip {
            time: (h, m),
            radio: true,
            gps: app.strip.gps,
            blink: app.strip.blink,
            usb,
            battery: Some(63),
            charging: false,
            rec: app.strip.rec,
        },
    );
    let _ = s;
    crate::ui::draw_main_time(&mut f, [h / 10, h % 10, m / 10, m % 10]);
    crate::ui::draw_date(&mut f, crate::date_text(rtc_date_from_dr(sim.rtc().1)).as_str());
    let values = [(Field::WeekKm, "--"), (Field::WeekH, "--"), (Field::TotalKm, "--"), (Field::Temp, "--")];
    for (i, (field, v)) in values.into_iter().enumerate() {
        let bx = crate::ui::overview_box(i).unwrap();
        crate::ui::draw_field_frame(&mut f, bx, field);
        crate::ui::draw_field_value(&mut f, bx, field, v, crate::ui::WHITE);
    }
    crate::ui::draw_ferris(&mut f);
    f.px
}

/// The main screen, the overview: the strip, the time, the date, four fields, Ferris (no notices yet).
#[test]
fn boots_into_the_main_screen() {
    let mut sim = Sim::new(12, 34, 56);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    app.set_usb(&mut sim, UsbDot::Ready);
    assert!(sim.visible());
    let pos = |c| sim.cmds.iter().position(|&x| x == c).unwrap();
    assert!(pos(SWRESET) < pos(SLPOUT) && pos(SLPOUT) < pos(DISPON));
    assert!(main_screen(&mut sim, &app, UsbDot::Ready) == sim.screen);
    assert_consistent(&sim, &app);
    sim.save("preview");
}

/// Stock's order: reset, sd_ble_enable, then the version; no ANT without a key, no scan before the list.
#[test]
fn starts_the_nordic_chip() {
    let mut sim = Sim::new(12, 0, 0);
    App::start(&mut sim, None);
    assert_eq!(sim.nordic.got, [vec![0, 0x60], vec![0, 0x65, 1]]);
}

/// A chip that never answers costs one timeout per command, then the pages run and the label is red.
#[test]
fn a_silent_nordic_chip_does_not_stop_the_pages() {
    let mut sim = Sim::new(12, 0, 0);
    sim.nordic.dead = true;
    let t0 = sim.ms;
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(sim.ms - t0 < 3000, "start took {} ms", sim.ms - t0);
    assert_eq!(sim.nordic.got, [vec![0, 0x60]], "gave up after the first timeout");
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], false) == sim.screen, "BLE in red");
}

#[test]
fn ticks_through_midnight() {
    let mut sim = Sim::new(23, 59, 58);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    assert_eq!(hm(&mut sim), "00:00");
    assert!(main_screen(&mut sim, &app, UsbDot::Off) == sim.screen);
}

/// Page goes from the main screen through the ride pages that are on (page 3 is the map by default), and back.
#[test]
fn page_turns_the_ride_pages() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    assert!(ride(&mut sim, &app, 0, &["--"; 3], true) == sim.screen, "full height, no Ferris");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1));
    assert_eq!(app.pages()[1].fields[..4], [Field::Speed, Field::Hr, Field::Cadence, Field::Power]);
    assert!(ride(&mut sim, &app, 1, &["--"; 4], true) == sim.screen);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(2), "the map");
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "round the ride pages, not back to the overview");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "Back: the overview");
    assert_consistent(&sim, &app);
}

/// A finger swiping left on the main screen opens the first ride page, swiping right comes back; touching wakes
/// the backlight; without a finger, the chip is never read. Then a tap on the menu's second row opens page 1's
/// settings, as Start on that row does.
#[test]
fn touch_swipes_pages_and_taps_menu_rows() {
    let swipe = |t0: u64, x0: i64, dx: i64| {
        move |ms: u64| (t0..t0 + 250).contains(&ms).then(|| ((x0 + dx * (ms - t0) as i64 / 250) as u16, 160))
    };
    let (left, right) = (swipe(5000, 200, -120), swipe(6000, 40, 120)); // after the start (a few seconds)
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| left(ms).or(right(ms)));
    let mut app = App::start(&mut sim, None);
    let left_ms = 4900 - sim.ms;
    run(&mut sim, &mut app, left_ms);
    assert_eq!(sim.touch.reads, 0, "no finger, no read");
    run(&mut sim, &mut app, 500);
    assert_eq!(app.screen(), Screen::Ride(0));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Ride(2), "swipe right: the previous ride page, round");
    assert_consistent(&sim, &app);

    let row1 = ui::LIST_Y as u16 + ui::LIST_ROW as u16 + 5;
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| (5000..5100).contains(&ms).then_some((120, row1)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    let left_ms = 5500 - sim.ms;
    run(&mut sim, &mut app, left_ms);
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(OVERVIEW), 0), "row 1: the overview's fields");
    assert_consistent(&sim, &app);
}

/// Quick touches count: a 60 ms tap and a 120 ms swipe (the panel is read on INT and every 10 ms while a finger
/// is down, not once per 50 ms pass when INT happens to be low).
#[test]
fn quick_taps_and_swipes() {
    let swipe = |ms: u64| (5000..5120).contains(&ms).then(|| ((200 - (ms - 5000) as i64 * 120 / 120) as u16, 160));
    let row1 = ui::LIST_Y as u16 + ui::LIST_ROW as u16 + 5;
    let tap = move |ms: u64| (7003..7063).contains(&ms).then_some((120, row1));
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| swipe(ms).or(tap(ms)));
    let mut app = App::start(&mut sim, None);
    let left = 5500 - sim.ms;
    run(&mut sim, &mut app, left);
    assert_eq!(app.screen(), Screen::Ride(0), "the fast swipe left");
    app.press(&mut sim, Press::Long(Key::Page));
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(OVERVIEW), 0), "the short tap on row 1");
}

/// Touch in a menu: a tap on MAX HR edits it, a swipe up adds one, a tap keeps it, a swipe right goes back.
#[test]
fn touch_edits_a_number_and_goes_back() {
    let row = |r: u16| ui::LIST_Y as u16 + r * ui::LIST_ROW as u16 + 5;
    let max_hr = row(menu::MAIN.iter().position(|&r| r == menu::Row::Set(Setting::MaxHr)).unwrap() as u16);
    let tap = move |t0: u64, y: u16| move |ms: u64| (t0..t0 + 100).contains(&ms).then_some((120, y));
    let drag = |t0: u64, (x0, y0): (i64, i64), (dx, dy): (i64, i64)| {
        move |ms: u64| {
            let k = ms.checked_sub(t0).filter(|&k| k < 250)? as i64;
            Some(((x0 + dx * k / 250) as u16, (y0 + dy * k / 250) as u16))
        }
    };
    let (a, b, c, d) =
        (tap(5000, max_hr), drag(6000, (120, 250), (0, -100)), tap(7000, max_hr), drag(8000, (40, 160), (120, 0)));
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| a(ms).or(b(ms)).or(c(ms)).or(d(ms)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    let left = 6900 - sim.ms;
    run(&mut sim, &mut app, left);
    assert!(app.editing, "the tap edits");
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::MaxHr))), "MAX HR < 191 >", "the swipe up adds one");
    run(&mut sim, &mut app, 1000);
    assert!(!app.editing, "the second tap keeps it");
    assert_eq!(Settings::from_word(sim.backup[11]).max_hr, 191);
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Main, "swipe right: back");
    assert_consistent(&sim, &app);
}

/// The overview's totals: stock's rides are read by themselves a while after start-up, then the week, year and
/// all-time distance show (with the ride running added).
#[test]
fn overview_totals_from_stocks_rides() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let w = PageCfg {
        layout: 4,
        fields: [Field::WeekKm, Field::WeekH, Field::YearKm, Field::TotalKm, Field::Time, Field::Time],
    }
    .to_words();
    (sim.backup[26], sim.backup[27]) = (w[0], w[1]);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.field_text(Field::TotalKm).as_str(), "--", "not read yet");
    run(&mut sim, &mut app, u64::from(STOCK_SCAN_MS) + 20_000);
    assert_eq!(app.stock.status(), crate::stock::Status::Done, "read by itself");
    assert_eq!(app.field_text(Field::WeekKm).as_str(), "19");
    assert_eq!(app.field_text(Field::TotalKm).as_str(), "37");
    assert_consistent(&sim, &app);
    sim.save("overview_totals");
}

/// The stock data menu over the synthetic volume (tools/fat_image.py) through the eMMC model: READ RIDES
/// reads stock's rides (a bounded amount per pass), the rows show the totals and CP, IMPORT applies stock's
/// settings. Prints what the scan cost: bytes and reads (the model takes no time to read; on the unit the
/// read speed decides).
#[test]
fn stock_data_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Stock));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Stock, 0));
    let value = |app: &App, i| std::string::String::from(app.menu_row(Menu::Stock, i).unwrap().1.as_str());
    assert_eq!((value(&app, 0), value(&app, 1)), ("GO".into(), "--".into()));
    app.press(&mut sim, Press::Short(Key::Start));
    let (t0, cmds0) = (sim.ms, sim.emmc.cmds.len());
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms - t0 < 60_000);
    }
    let reads = sim.emmc.cmds[cmds0..].iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    std::println!("scan: {} bytes, {} reads, {} passes", app.stock.out.bytes, reads, (sim.ms - t0) / 50);
    assert_eq!(app.stock.status(), crate::stock::Status::Done);
    assert_eq!([0, 4, 8].map(|i| value(&app, i)), ["4", "19", "37"].map(std::string::String::from));
    let cp = crate::power_curve::fit_cp(&app.stock.out.curve).unwrap();
    assert_eq!(value(&app, 1), std::format!("{}", cp.cp));
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("stock");

    app.press(&mut sim, Press::Short(Key::Lap)); // IMPORT, the last row
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.settings.ftp, app.settings.max_hr), (265, 188));
    let hr = crate::nrf::SensorId { device: 1234, trans: 0 };
    assert_eq!(sim.backup[Kind::Hr.backup()], hr.to_word());
    use crate::ui::Field::*;
    let p = app.pages();
    assert_eq!((ui::field_count(p[0].layout), &p[0].fields[..4]), (4, &[Power, Hr, Cadence, Speed][..]));
    assert_eq!((ui::field_count(p[1].layout), &p[1].fields[..2]), (2, &[Dist, Timer][..]), "ActBike=1: Bike2");
    assert_eq!((app.settings.weight_kg, app.settings.wprime_j), (73, 21_000), "72.5 kg rounded, W' in J");
    assert!(app.notices[0].as_str().ends_with("IMPORTED 8"), "{}", app.notices[0].as_str());
    assert_eq!(Settings::from_word(sim.backup[WHEEL_BKP]).ftp, 265, "kept");
}

/// The ride history over the synthetic volume (tools/fat_image.py): HISTORY starts the scan of stock's rides and
/// lists them newest first as they come; Select opens a ride's card (its track, profile and numbers, read from
/// its file once), Back returns to the list. Prints what opening a card read.
#[test]
fn ride_history() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 0));
    assert_eq!(app.stock.status(), crate::stock::Status::Running, "opening it starts the scan");
    let row =
        |app: &App, i| app.menu_row(Menu::History, i).map(|(l, v, _)| std::format!("{} {}", l.as_str(), v.as_str()));
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 60_000);
    }
    run(&mut sim, &mut app, 1000);
    let rows: Vec<_> = (0..5).map(|i| row(&app, i)).collect();
    let want = ["THU 22 OCT 5.0", "TUE 20 OCT 14.4", "THU 1 OCT 12.0", "SAT 15 MAR 6.0"];
    assert_eq!(rows, want.map(|r| Some(r.into())).into_iter().chain([None]).collect::<Vec<_>>(), "newest first");
    assert_consistent(&sim, &app);
    sim.save("history_list");

    app.press(&mut sim, Press::Short(Key::Page));
    let (cmds0, t0) = (sim.emmc.cmds.len(), sim.ms);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Past);
    let reads = sim.emmc.cmds[cmds0..].iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    let file = app.stock.index.get(1).unwrap().file.size;
    assert_eq!(app.past.bytes, file, "the whole file, once");
    std::println!(
        "card: {} bytes of FIT in {reads} reads, {} ms simulated; at 2.5 MB/s on the unit about {} ms",
        app.past.bytes,
        sim.ms - t0,
        app.past.bytes / 2500
    );
    // what the records give: 2 min at 400 W then 250 W, a second a record
    let mut p = crate::power::Power::new();
    (0..1800).for_each(|t| p.sample(Some(if t < 120 { 400 } else { 250 }), true));
    let np = p.np().unwrap();
    let tss = crate::power::tss(1800, np, app.settings.ftp);
    let card = app.past.card;
    assert_eq!(card.title.as_str(), "20 OCT 08:00");
    let stats: Vec<_> = card.stats.iter().map(|(l, v)| std::format!("{l} {}", v.as_str())).collect();
    let want = [
        "TIME 0:30".into(),
        "KM 14.4".into(),
        "AVG KM/H 28.8".into(),
        "ASCENT M 80".into(),
        std::format!("NP {np}"),
        "AVG W 260".into(),
        "AVG HR 140".into(),
        "MAX HR 140".into(),
        std::format!("TSS {tss}"),
        "KJ 468".into(),
    ];
    assert_eq!(stats, want);
    assert!(app.past.track.points().len() > 100, "the loop's track");
    let h = ui::card::ELEV.3;
    let bars: Vec<u32> = (0..ui::card::ELEV.2).map(|i| app.past.elev(i, h).0).collect();
    assert!(bars.iter().all(|&b| b >= 1) && bars.iter().any(|&b| b > h / 2), "two hills over the profile");
    assert!(bars[0] < h / 4 && bars[bars.len() / 4] > h / 2, "low at the start, a top a quarter in");
    run(&mut sim, &mut app, 500);
    assert_consistent(&sim, &app);
    sim.save("history_card");

    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, 1), "back to the list, on that ride");
    // the ride without a track: its card all the same, "NO FIX" on the map
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.past.card.title.as_str()), (Screen::Past, "22 OCT 10:00"));
    assert!(app.past.track.points().is_empty());
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
}

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The synthetic card's planned workouts (tools/fat_image.py, today 2026-10-23): WORKOUTS lists today's and the
/// next days' (not the past one, not the broken one), Select shows the steps (repeats written out), Start
/// starts the ride and the workout. The step clock follows the ride timer: 3-2-1 beeps and a banner per step,
/// the target coloured from the 3 s power, a pause stops it, Lap skips a step (and ends the open cool-down),
/// the last step ends the workout.
#[test]
fn workout_from_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210 });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    // the overview's field: today's planned time, the card read when the field is first shown
    app.pages[OVERVIEW].fields[3] = Workout;
    app.show(&mut sim);
    assert_eq!(text(&app, Workout), "0:01", "20 s + 2 x 45 s, the open cool-down not counted");
    app.pages[0] = PageCfg { layout: 6, fields: [StepTarget, StepTime, NextStep, Workout, Power3s, Timer] };

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    let row = |app: &App, m, i| {
        app.menu_row(m, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str())).unwrap_or_default()
    };
    assert_eq!([0, 1, 2].map(|i| row(&app, Menu::Workouts, i)), ["FRI 23 OCT 0:01", "SUN 25 OCT 1:30", ""]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("workout_list");

    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.plan.name()), (Screen::Menu(Menu::Steps, 0), "SHORT INTERVALS"));
    let steps: Vec<_> = (0..8).map(|i| row(&app, Menu::Steps, i)).collect();
    let want = ["START 0:01", "0:20 100-120W", "0:30 200-220W", "0:15 150W", "0:30 200-220W", "0:15 150W"];
    assert_eq!(steps[..6], want, "FTP 200: Bryton's 50-60 %, then % FTP, then W");
    assert_eq!(steps[6..], ["LAP OPEN", ""]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("workout_steps");

    let tones = sim.tones.len();
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Running));
    let fields = |app: &App| [StepTarget, StepTime, NextStep, Workout].map(|f| text(app, f));
    assert_eq!(fields(&app), ["100-120", "0:20", "200-220", "1/6"]);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.field_color(StepTarget), ui::RED, "210 W over 100-120");
    let (mut ms, start) = (0, sim.ms);
    while app.player.and_then(|p| p.step(app.plan.steps())) == Some(0) {
        run(&mut sim, &mut app, 50);
        ms += 50;
        assert!(ms < 30_000, "the warm-up ends");
    }
    assert!((14_000..16_500).contains(&(sim.ms - start)), "20 s from the start: {} ms after 5 s", sim.ms - start);
    assert_eq!(sim.tones[tones..], [2500, 0, 2000, 0, 2000, 0, 2000, 0, 2500], "start, 3-2-1, the next step");
    assert_eq!(app.banner.as_str(), "2/6 200-220W");
    run(&mut sim, &mut app, 5000);
    assert_eq!(fields(&app), ["200-220", "0:25", "150", "2/6"]);
    assert_eq!(app.field_color(StepTarget), ui::GREEN, "210 W in 200-220");
    assert_consistent(&sim, &app);
    sim.save("workout_ride");

    // a pause stops the step clock with the ride's timer
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(text(&app, StepTime), "0:25");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert_eq!(text(&app, StepTime), "0:23");

    // Lap: the next step; the open cool-down waits for Lap; then done
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((text(&app, Workout), app.banner.as_str()), ("3/6".into(), "3/6 150W"));
    assert_eq!(app.field_color(StepTarget), ui::RED, "210 W over 150 W");

    // one workout at a time: while it runs, another one's steps do not open, its own do
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Menu(Menu::Workouts, 1), "WORKOUT RUNNING"));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), text(&app, Workout)), (Screen::Menu(Menu::Steps, 0), "3/6".into()));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    assert_eq!(app.screen(), Screen::Ride(0));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(fields(&app), ["OPEN", "LAP", "END", "6/6"]);
    assert_eq!(app.banner.as_str(), "6/6 OPEN LAP");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((text(&app, Workout), app.banner.as_str()), ("DONE".into(), "WORKOUT DONE"));
    assert_eq!(fields(&app)[..3], ["--", "--", "--"]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    assert_eq!(app.ride.state(), ride::State::Running, "the ride goes on");
}

/// Holding Page opens the menu; Page / Lap move, Start on NEARBY BLE opens the list and scans: two devices in
/// three reports, strongest first, "2 NEARBY..." until the timeout. Back goes out level by level.
#[test]
fn menu_lists_the_devices_nearby() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
    app.press(&mut sim, Press::Short(Key::Lap));
    let last = menu::MAIN.len() - 1;
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, last), "Lap from the top wraps to the last row");
    for _ in NEARBY_ROW..last {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, NEARBY_ROW), "then up to NEARBY BLE");
    assert_consistent(&sim, &app);
    sim.save("menu");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Nearby);
    assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
    run(&mut sim, &mut app, 1500);
    let names: Vec<_> = app.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
    assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
    assert!(app.scanning);
    assert_consistent(&sim, &app);
    sim.save("nearby");
    run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
    assert!(!app.scanning);
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, NEARBY_ROW));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// Back returns to where the menu was opened, row by row: a submenu to its row, the menu to the ride page.
#[test]
fn back_returns_where_it_came_from() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1));
    app.press(&mut sim, Press::Long(Key::Page));
    for _ in 0..GPS_ROW {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, GPS_ROW), "the submenu's row");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(1), "the page the menu was opened from");
    assert_consistent(&sim, &app);
    // switching that page off from the menu: Back shows the main screen instead
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(1)));
    app.press(&mut sim, Press::Short(Key::Start)); // PAGE 2
    while app.pages()[1].layout != 0 {
        app.press(&mut sim, Press::Short(Key::Start)); // FIELDS round to OFF
    }
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// A key held in a menu moves the selection on each repeat, and its release does not move it once more.
#[test]
fn held_key_scrolls_a_menu() {
    let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..5749).contains(&ms) { Key::Page as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    assert!(sim.ms < 5000);
    app.press(&mut sim, Press::Long(Key::Page));
    let left = 7000 - sim.ms;
    run(&mut sim, &mut app, left);
    // held 750 ms: repeats at 500, 600, 700 ms, then the release
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 3));
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 4), "the next press acts");
}

/// Sensors menu: Start on a sensor searches; the first one heard is paired (kept in its backup register) and
/// its value reaches the ride page. With a key, ANT is started and every channel assigned, none opened yet.
#[test]
fn pairs_sensors_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
    sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: spd_cad });
    let mut app = App::start(&mut sim, Some(KEY));
    let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
    assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2], "enable, key, assign 5 channels");
    assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
    app.press(&mut sim, Press::Long(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // heart rate: search
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start)); // speed + cadence: search
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "strap paired and kept");
    assert_eq!(sim.backup[Kind::SpeedCadence.backup()], SensorId { device: 555, trans: 0xf1 }.to_word());
    assert_consistent(&sim, &app);
    sim.save("sensors");
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 0, &["22.7", "72", "60"], true) == sim.screen, "22.7 km/h, HR 72, 60 rpm");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 1, &["22.7", "72", "60", "--"], true) == sim.screen);
    sim.save("ride");
}

/// After a restart the paired sensor's channel opens for it only: a friend's strap is not taken. Start on it
/// in the sensors menu forgets it (channel closed, register cleared).
#[test]
fn keeps_the_paired_sensor_until_forgotten() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 999, data: hr80 }); // a friend's strap
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], true) == sim.screen, "the friend's strap is not shown");
    app.press(&mut sim, Press::Long(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(sim.nordic.got.iter().filter(|p| p[..2] == [6, 0xc5]).count(), 1, "closed");
    assert_eq!(sim.backup[Kind::Hr.backup()], 0, "forgotten");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.backup[Kind::Hr.backup()], SensorId { device: 999, trans: 0xf1 }.to_word(), "paired anew");
}

/// A paired strap that is not sending: its channel searches 30 s, closes, rests a minute, searches again; the
/// strap is found when it comes back.
#[test]
fn a_silent_sensor_searches_then_rests() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    let mut app = App::start(&mut sim, Some(KEY));
    let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
    assert_eq!(opens(&sim), 1);
    assert!(sim.nordic.got.contains(&vec![6, 0xda, 0, crate::nrf::SEARCH_TIMEOUT]));
    run(&mut sim, &mut app, 30_000 + 1000);
    assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
    run(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 2000);
    assert_eq!(opens(&sim), 1, "resting");
    run(&mut sim, &mut app, 2000);
    assert_eq!(opens(&sim), 2, "searching again");
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.live.get(Metric::Hr), Some(72), "found");
    run(&mut sim, &mut app, 60_000);
    assert!(sim.nordic.channels[0].open, "a sensor that sends keeps its channel");
}

/// The page menu: Start cycles the layout and the fields; kept in a backup register, read back at the next
/// start. A power meter's value shows in the new field.
#[test]
fn pages_are_configured_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power300 });
    let mut app = App::start(&mut sim, Some(KEY));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(0)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 0));
    while app.pages()[0].layout != 4 {
        app.press(&mut sim, Press::Short(Key::Start)); // layouts round to 2 x 2
    }
    app.press(&mut sim, Press::Short(Key::Page));
    for _ in 0..2 {
        app.press(&mut sim, Press::Short(Key::Start)); // field 1: speed -> cadence -> power
    }
    assert_consistent(&sim, &app);
    sim.save("page_menu");
    let want =
        PageCfg { layout: 4, fields: [Field::Power, Field::Hr, Field::Cadence, Field::Time, Field::Time, Field::Time] };
    assert_eq!(app.pages()[0], want);
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(want));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let t = hm(&mut sim);
    assert_consistent(&sim, &app);
    assert!(ride(&mut sim, &app, 0, &["300", "--", "85", &t], true) == sim.screen);
    let app = App::start(&mut sim, Some(KEY));
    assert_eq!(app.pages()[0], want, "read back after a restart");
}

/// When the simulated strap starts beating, ms.
const BEAT_START: u64 = 10_000;
/// The strap's interval before beat `k`, 1/1024 s: 820 plus or minus 16 (about 800 ms), scrambled (murmur3's
/// finaliser).
fn beat_ticks(k: u32) -> u64 {
    let mut x = k.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    804 + u64::from(x >> 27)
}
/// A strap beating from [`BEAT_START`]: page 4, the last beat's time, the one before, the count.
fn strap_beats(ms: u64) -> [u8; 8] {
    let now = ms * 1024 / 1000;
    let (mut t, mut prev, mut k) = (BEAT_START * 1024 / 1000, 0, 0u32);
    while t + beat_ticks(k + 1) <= now {
        (prev, k) = (t, k + 1);
        t += beat_ticks(k);
    }
    let ([p0, p1], [t0, t1]) = ((prev as u16).to_le_bytes(), (t as u16).to_le_bytes());
    [4, 0, p0, p1, t0, t1, k as u8, 70]
}

/// A strap sends a known series of beats: the HRV fields show "--" until there are enough, then the RMSSD and
/// DFA alpha1 of the series, and "--" again once the strap has no heart rate.
#[test]
fn hrv_fields_from_the_strap() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: strap_beats });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 4, fields: [Dfa, Rmssd, Hr, Time, Time, Time] };
    app.press(&mut sim, Press::Short(Key::Page));
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    run(&mut sim, &mut app, 20_000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "about 12 beats");
    run(&mut sim, &mut app, 20_000);
    assert!(text(&app, Dfa) == "--" && text(&app, Rmssd) != "--", "about 37 beats: RMSSD, not yet DFA");
    run(&mut sim, &mut app, 140_000);
    // stop where the app has the last beat or the one before: the same 5-beat update either way
    while strap_beats(sim.ms)[6] % hrv::EVERY != 2 {
        run(&mut sim, &mut app, 100);
    }
    let mut want = hrv::Hrv::default();
    for k in 1..=u32::from(strap_beats(sim.ms)[6]) {
        want.push(((beat_ticks(k) * 1000 + 512) >> 10) as u16);
    }
    let (a, ms) = (want.alpha1().unwrap(), want.rmssd().unwrap());
    assert!((0.3..0.7).contains(&a), "scrambled beats are close to white noise: {a}");
    let (a, ms) = (std::format!("{a:.2}"), std::format!("{ms}"));
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), (a.clone(), ms.clone()));
    let t = hm(&mut sim);
    assert!(ride(&mut sim, &app, 0, &[&a, &ms, "70", &t], true) == sim.screen);
    assert_consistent(&sim, &app);
    sim.save("hrv_fields");
    sim.nordic.sensors[0].data = |_| [4, 0, 0, 0, 0, 0, 0, 0]; // off the chest: no rate, no new beat
    run(&mut sim, &mut app, 7000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "no heart rate");
}

/// Start on the overview starts the ride and shows the first ride page; each key's effect on the ride is said in
/// the banner.
#[test]
fn start_from_the_overview() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Ride(0));
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 200);
    assert_eq!(app.banner.as_str(), "RIDE STARTED");
    assert_consistent(&sim, &app);
    sim.save("ride_started");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "PAUSED: HOLD = END");
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Summary, ride::State::Idle), "the ride's card");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "any key: the overview");
}

/// The card at the end of a ride: its track, its elevation profile, its numbers.
#[test]
fn ride_card_at_the_end() {
    fn hills(ms: u64) -> i32 {
        let t = (ms / 1000) as i32 % 1200;
        200_000 + if t < 600 { t * 200 } else { (1200 - t) * 200 } // up 120 m and down, in mm
    }
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(hills);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy });
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 2000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 40 * 60_000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.screen(), Screen::Summary);
    let card = app.ride_card();
    assert_eq!((card.stats[0].0, card.stats[0].1.as_str()), ("TIME", "0:40"));
    assert_eq!(card.title.as_str(), "23 OCT 09:00");
    assert_consistent(&sim, &app);
    sim.save("ride_card");
}

/// Pages kept by the builds of 2026-10-23 (from register 19, which stock reads at boot) move to 20..27, and 19 is
/// given back (stock's first-boot value, 0).
#[test]
fn pages_leave_stocks_register() {
    let mut sim = Sim::new(9, 0, 0);
    let p = PageCfg { layout: 5, fields: [Field::Map, Field::Hr, Field::Power, Field::Np, Field::Wbal, Field::Dfa] };
    let w = p.to_words().map(|w| w & 0x0fff_ffff | 0xb << 28); // as those builds wrote it
    (sim.backup[19], sim.backup[20]) = (w[0], w[1]);
    let app = App::start(&mut sim, None);
    assert_eq!(app.pages()[0], p);
    assert_eq!(sim.backup[19], 0, "given back to stock");
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(p));
    assert_eq!(App::start(&mut sim, None).pages()[0], p, "read from the new place after a restart");
}

/// Every layout draws on the unit as in the screenshot (layout_<id>.bmp, to look at).
#[test]
fn every_layout_draws() {
    use Field::*;
    let fields = [Map, Speed, Hr, Power, Cadence, Timer];
    for id in 1..ui::LAYOUTS.len() as u8 {
        let mut sim = Sim::new(9, 0, 0);
        let words = PageCfg { layout: id, fields }.to_words();
        (sim.backup[20], sim.backup[21]) = (words[0], words[1]);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 1000);
        assert_eq!(app.screen(), Screen::Ride(0));
        assert_consistent(&sim, &app);
        sim.save(&std::format!("layout_{id}"));
    }
}

/// The battery level in the strip (green while charging); an empty battery switches the unit off, but not
/// while charging.
#[test]
fn battery_in_the_strip_and_off_when_empty() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.step(&mut sim);
    assert_eq!(app.strip.battery, Some(63), "3.9 V");
    sim.charger = true;
    sim.battery_mv = Some(3100);
    run(&mut sim, &mut app, 3 * u64::from(BATTERY_MS) + 1000);
    assert!(!sim.standby && app.strip.charging, "charging: stays on");
    sim.charger = false;
    run(&mut sim, &mut app, 2 * u64::from(BATTERY_MS));
    assert!(!sim.standby, "two low readings: not yet");
    run(&mut sim, &mut app, u64::from(BATTERY_MS));
    assert!(sim.standby && !sim.display_on, "empty: switched off");
}

/// Away from the overview, a notice is a banner for a while: the page above it keeps updating; when it goes,
/// what it covered is drawn again. A key dismisses it and acts (Start on a ride must start it).
#[test]
fn notice_banner_over_a_ride_page() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    sim.gnss.fix = Some(7000);
    run(&mut sim, &mut app, 1500);
    assert!(app.banner_ms.is_some());
    assert_consistent(&sim, &app);
    sim.save("banner");
    run(&mut sim, &mut app, u64::from(BANNER_MS));
    assert!(app.banner_ms.is_none(), "gone by itself");
    assert_consistent(&sim, &app);
    app.notice("TEST");
    run(&mut sim, &mut app, 100);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1), "the key dismissed it and acted");
    assert!(app.banner_ms.is_none());
    assert_consistent(&sim, &app);
}

/// The overview: the first GPS fix becomes a notice in place of Ferris; its fields are chosen in the menu
/// (OVERVIEW: four rows, no field count) and kept.
#[test]
fn overview_notices_and_fields() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    sim.gnss.fix = Some(7000);
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.notices[0].as_str(), "09:00 GPS FIX");
    assert_consistent(&sim, &app);
    sim.save("overview_notice");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(OVERVIEW)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.rows(Menu::Page(OVERVIEW)), 4);
    app.press(&mut sim, Press::Short(Key::Start)); // WEEK KM -> WEEK H
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
    assert_eq!(App::start(&mut sim, None).pages()[OVERVIEW].fields[0], Field::WeekH, "kept");
}

/// The receiver is powered at start and gets its configuration (RAM layer) once; with a fix, "GPS" turns green
/// and the speed comes from it when no speed sensor is paired; it is powered off with the unit.
#[test]
fn gps_fix_and_speed() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert!(!sim.gnss.powered, "powered later, after its line has been low long enough");
    run(&mut sim, &mut app, 3000);
    assert!(sim.gnss.powered);
    let sent = |sim: &Sim, f: [u8; 32]| sim.gnss.got.windows(32).any(|w| w == f);
    assert!(sent(&sim, crate::gnss::config()), "configuration");
    let frames = sim.gnss.got.iter().filter(|&&b| b == 0xb5).count();
    assert_eq!(frames, 2, "the RTC time (14 h accuracy, no offset learnt yet), no position yet");
    assert_eq!(app.strip.gps, Some(0), "searching");
    sim.gnss.fix = Some(7000); // 25.2 km/h
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.strip.gps, Some(3), "9 satellites: 3 bars");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 100);
    let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
    let mut want = Frame::new();
    want.px = ride(&mut sim, &app, 0, &["25.2", "--", "--"], true);
    let strip = Strip {
        time: (h, m),
        radio: true,
        gps: Some(3),
        blink: true,
        usb: UsbDot::Off,
        battery: Some(63),
        charging: false,
        rec: Rec::Idle,
    };
    crate::ui::draw_strip(&mut want, &strip);
    assert!(want.px == sim.screen, "GPS speed, green GPS");
    sim.save("gps");
    app.press(&mut sim, Press::Long(Key::Page));
    for _ in 0..GPS_ROW {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("gps_menu");
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    // the fix taught the RTC's offset (+2 h) and the position (to 1e-4 degrees): kept for the next power-up
    let aid = crate::gnss::Aid::from_words([sim.backup[17], sim.backup[18]]);
    assert_eq!(aid, crate::gnss::Aid { offset_15min: Some(8), pos: Some((488_566_000, -22_945_000)) });
    sim.gnss.powered = false; // a power cycle: the board drives the line low at boot
    sim.gnss.off_since = sim.ms;
    let mut again = App::start(&mut sim, None);
    sim.gnss.got.clear();
    run(&mut sim, &mut again, 2500);
    let (tr, dr) = sim.rtc();
    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
    let utc = crate::from_epoch_s(crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600);
    let sent_time = (0..3).any(|back| {
        sim.gnss.got.windows(32).any(|w| w == crate::gnss::mga_time(crate::from_epoch_s(crate::epoch_s(utc) - back), 2))
    });
    assert!(sent_time, "UTC from the RTC at the next power-up");
    assert!(sent(&sim, crate::gnss::mga_pos(488_566_000, -22_945_000, 10_000)), "and the last position");
    sim.gnss.powered = false; // silent receiver: the fix ages out
    run(&mut sim, &mut app, u64::from(PVT_STALE_MS) + 200);
    assert_eq!(app.strip.gps, None);
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.gnss.powered, "off with the unit");
}

/// The backlight: level and time-out from the menu (kept with the wheel size); after the time-out any key
/// brings it back (and acts), Back only brings it back.
#[test]
fn backlight_level_and_timeout() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert_eq!(sim.light_level, 100, "100 % and never off by default");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Light));
    app.press(&mut sim, Press::Short(Key::Start)); // LIGHT %: 60
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // LIGHT OFF: 10 s
    assert_eq!(sim.light_level, 60);
    assert_consistent(&sim, &app);
    sim.save("light_menu");
    app.press(&mut sim, Press::Short(Key::Back));
    run(&mut sim, &mut app, 11_000);
    assert_eq!(sim.light_level, 0, "timed out");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!((sim.light_level, app.screen()), (60, Screen::Ride(0)), "a key brings it back and acts");
    app.press(&mut sim, Press::Short(Key::Back)); // a ride page's Back: the overview
    run(&mut sim, &mut app, 11_000);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(sim.light_level, 60, "Back after the time-out: on, not toggled off");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(sim.light_level, 0, "then Back on the overview toggles");
    let again = App::start(&mut sim, None);
    assert_eq!(sim.light_level, 60, "kept: {:x}", sim.backup[11]);
    drop(again);
}

/// The text of sensors menu row `i`: label and value.
fn sensor_row(app: &App, i: usize) -> std::string::String {
    let (label, value, _) = app.menu_row(Menu::Sensors, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// From the main menu's first row, down to `row`.
fn to_row(sim: &mut Sim, app: &mut App, row: menu::Row) {
    for _ in 0..menu::MAIN.iter().position(|&r| r == row).unwrap() {
        app.press(sim, Press::Short(Key::Page));
    }
}

/// Main menu row index of `row`.
fn row_of(row: menu::Row) -> usize {
    menu::MAIN.iter().position(|&r| r == row).unwrap()
}

/// The text of main menu row `i`: label and value.
fn main_row(app: &App, i: usize) -> std::string::String {
    let (label, value, _) = app.menu_row(Menu::Main, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// AUTO: the backlight follows the ambient light, smoothed, without flicker on a band edge, and the light
/// sensor measures only now and then, and not at all while the backlight is off.
#[test]
fn auto_backlight_follows_the_ambient_light() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    assert_eq!(sim.bus.accel_ctrl1, 0x07, "the accelerometer switched off");
    run(&mut sim, &mut app, 30_000);
    assert_eq!(sim.bus.light_reads, 0, "a fixed level and no menu: the light sensor is never used");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Light));
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Start)); // 60, 30, 10, AUTO
    }
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Light))), "LIGHT % AUTO");
    assert_eq!(Settings::from_word(sim.backup[11]).light_level, LIGHT_AUTO, "kept");
    run(&mut sim, &mut app, 4000);
    assert_eq!(sensor_row(&app, 5), "LUX 300");
    assert_eq!(sensor_row(&app, 6), "TEMP C 21.5");
    assert_eq!(sensor_row(&app, 7), "HPA 1013");
    assert_consistent(&sim, &app);
    sim.save("sensor_rows");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(sim.light_level, 1, "300 lux: past 120 lux, the minimum (stock)");

    sim.bus.lux10 = Some(5); // 0.5 lux: dark
    run(&mut sim, &mut app, 60_000);
    assert_eq!(sim.light_level, 8);
    sim.bus.lux10 = Some(300); // 30 lux
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.light_level, 50, "within a second");

    // a light hovering on the 50 lux edge: one level throughout
    let mut levels = std::collections::BTreeSet::new();
    for i in 0..40 {
        sim.bus.lux10 = Some(if i % 2 == 0 { 450 } else { 550 });
        run(&mut sim, &mut app, 3000);
        levels.insert(sim.light_level);
    }
    assert_eq!(levels.into_iter().collect::<Vec<_>>(), [50]);

    // one reading per LIGHT_MS
    let reads0 = sim.bus.light_reads;
    run(&mut sim, &mut app, 60_000);
    let reads = sim.bus.light_reads - reads0;
    assert!((230..=245).contains(&reads), "{reads} readings in 60 s");

    app.press(&mut sim, Press::Short(Key::Back)); // backlight off: no readings
    let reads = sim.bus.light_reads;
    run(&mut sim, &mut app, 30_000);
    assert_eq!(sim.bus.light_reads, reads);
    assert_eq!(sim.light_level, 0);
}

/// Without the chips the app runs as before: the rows say so, AUTO keeps a fixed level.
#[test]
fn missing_sensor_chips() {
    let mut sim = Sim::new(9, 0, 0);
    sim.bus.lux10 = None;
    sim.bus.baro = None;
    sim.backup[11] = 0xa5 << 24 | (LIGHT_AUTO as u32) << 16 | 2105;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert_eq!(sim.light_level, LIGHT_LEVELS[1]);
    assert_eq!(sensor_row(&app, 5), "LUX NONE");
    assert_eq!(sensor_row(&app, 6), "TEMP C NONE");
}

/// The barometer converts once per reading (one-shot), every 10 s.
#[test]
fn barometer_one_shot() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 60_000);
    assert!((6..=8).contains(&sim.bus.baro_conversions), "{}", sim.bus.baro_conversions);
    assert_eq!(app.baro, Some((10_132, 2150)));
    sim.bus.baro = Some((9_500, -512));
    run(&mut sim, &mut app, 20_000);
    assert_eq!(sensor_row(&app, 6), "TEMP C -5.1");
    assert_eq!(sensor_row(&app, 7), "HPA 950");
}

#[test]
fn back_short_press_toggles_the_backlight() {
    // Presses after the start (which takes about 1 s with the Nordic chip): at 3 s and at 4 s.
    let back = |ms| (3000..3200).contains(&ms) || (4000..4100).contains(&ms);
    let mut sim = Sim::new(8, 0, 0).keys(move |ms| if back(ms) { Key::Back as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    let t0 = sim.ms;
    assert!(t0 < 3000);
    run(&mut sim, &mut app, 3500 - t0);
    assert!(!sim.backlight);
    run(&mut sim, &mut app, 1000);
    assert!(sim.backlight);
}

#[test]
fn back_held_3s_powers_off_after_release() {
    let mut sim = Sim::new(8, 0, 0).keys(|ms| if (1000..6000).contains(&ms) { Key::Back as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert!(sim.standby);
    assert!(sim.ms >= 6000, "standby while the key is still down would wake the unit at once");
    assert!(sim.asleep && !sim.display_on && !sim.backlight);
    assert_eq!(sim.nordic.got.last(), Some(&vec![0, 0x32]), "the radio chip switched off too");
    assert_eq!(sim.tones, [1500, 2500, 0, 2500, 1500, 0], "rising at power-on, falling at power-off");
}

/// A synthetic ride at 8 m/s (28.8 km/h): 240 s east with a gentle wiggle, then a loop of 600 m radius
/// counterclockwise, starting heading east. Latitude, longitude (1e-7 degrees) and heading (1e-5 degrees).
fn ride_route(ms: u64) -> (i32, i32, i32) {
    route_from(ms, (FIX_LAT, FIX_LON), 1.0)
}

/// The same ride turned half a circle (west, then a loop south), from `origin`: `sign` -1.
fn route_from(ms: u64, (lat0, lon0): (i32, i32), sign: f64) -> (i32, i32, i32) {
    let at = |t: f64| {
        if t < 240.0 {
            (8.0 * t, 30.0 * (t * core::f64::consts::TAU / 60.0).sin())
        } else {
            let a = (t - 240.0) * 8.0 / 600.0;
            (1920.0 + 600.0 * a.sin(), 600.0 - 600.0 * a.cos())
        }
    };
    let t = ms as f64 / 1000.0;
    let ((x, y), (x1, y1)) = (at(t), at(t + 0.5));
    let (x, y, x1, y1) = (sign * x, sign * y, sign * x1, sign * y1);
    let heading = (x1 - x).atan2(y1 - y).to_degrees().rem_euclid(360.0);
    let m_per_unit = 0.011_132; // metres per 1e-7 degree of latitude, as map.rs
    let cos = (f64::from(lat0) * 1e-7).to_radians().cos();
    (lat0 + (y / m_per_unit) as i32, lon0 + (x / m_per_unit / cos) as i32, (heading * 1e5) as i32)
}

/// The ride west and south of 0 N 0 E, where the synthetic tiles are (tools/fat_image.py).
fn tiles_route(ms: u64) -> (i32, i32, i32) {
    route_from(ms, crate::tiles::tests::ORIGIN, -1.0)
}

/// The basemap from the synthetic tiles (on the FAT volume macOS made, through the eMMC driver): drawn at
/// the last known position before a fix, kept at the first fix, then under the ride. Between view changes the
/// arrow and the track restore it in small boxes and the eMMC is not read; the screenshot equals the panel.
#[test]
fn basemap_under_the_map() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(tiles_route);
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    run(&mut sim, &mut app, 500);
    let bg = |sim: &Sim| sim.screen.iter().filter(|&&p| p == crate::tiles::BACKGROUND).count();
    sim.save("basemap_last_known");
    assert!(bg(&sim) > 30_000, "the basemap: {}", bg(&sim));
    assert_consistent(&sim, &app);
    std::println!("basemap at the last known position: {:?}", app.basemap_stats());
    // stock's rides are read by themselves after STOCK_SCAN_MS: read first, so the reads counted below are the
    // basemap's
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
    }
    let reads = |sim: &Sim| sim.emmc.cmds.iter().filter(|c| c.0 == 17 || c.0 == 18).count();
    sim.gnss.fix = Some(8000);
    let view = app.map_view(2);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.map_view(2), view, "the first fix, inside the view: the same view, no new basemap");
    assert_consistent(&sim, &app);
    let t0 = sim.ms;
    let (mut refits, mut refit_px, mut small_px, mut small_reads, mut drawn) = (0, 0, 0, 0, 0);
    let mut seconds = 0;
    while sim.ms < t0 + 840_000 {
        let (view, px, r) = (app.map_view(0), sim.pixels, reads(&sim));
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        if app.map_view(0) != view {
            refits += 1;
            refit_px += sim.pixels - px;
            drawn += u64::from(app.basemap_stats().px);
            std::println!("view change at {} s: {:?}", (sim.ms - t0) / 1000, app.basemap_stats());
        } else {
            small_px += sim.pixels - px;
            small_reads += reads(&sim) - r;
        }
        if (sim.ms - t0) / 1000 > seconds {
            seconds = (sim.ms - t0) / 1000;
            if seconds % 60 == 0 {
                assert_consistent(&sim, &app);
            }
            if seconds == 120 {
                sim.save("basemap_ride");
            }
        }
    }
    assert_consistent(&sim, &app);
    sim.save("basemap_loop");
    let s = (sim.ms - t0) / 1000;
    std::println!(
        "basemap ride: {refits} view changes ({} panel px, {} buffer px each), {} panel px/s between them",
        refit_px / refits.max(1),
        drawn / refits.max(1),
        small_px / s
    );
    assert_eq!(small_reads, 0, "the eMMC is read on view changes only");
    assert!(small_px / s < 2000, "{} px/s between view changes", small_px / s);
    assert!(bg(&sim) > 20_000, "the basemap under the loop");
}

/// The map page (page 3 by default) through a ride: "NO FIX" until the first fix, then the track and the arrow;
/// fitted again a few times while the ride goes out, never once the loop is closed; between those, only small
/// boxes are redrawn. The panel always equals the screenshot rendered row by row.
/// Before any fix, the map shows the last known position (kept from an earlier power-up) as a grey dot instead
/// of "NO FIX"; the first fix replaces it with the arrow.
#[test]
fn map_shows_the_last_known_position_until_a_fix() {
    let mut sim = Sim::new(9, 0, 0);
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((FIX_LAT / 1000 * 1000, FIX_LON / 1000 * 1000)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2), "page 3: the map");
    run(&mut sim, &mut app, 500);
    // the middle of the map, where the dot or the arrow is (the strip and the scale bar are white too)
    let count = |sim: &Sim, c: u16| {
        let (cx, cy) = (W / 2, crate::ui::RIDE_Y + crate::ui::RIDE_H / 2);
        let mut n = 0;
        for y in cy - 20..cy + 20 {
            n += (cx - 20..cx + 20).filter(|&x| sim.screen[(y * W + x) as usize] == c).count();
        }
        n
    };
    sim.save("map_last_known");
    assert_eq!(count(&sim, crate::ui::GREY), 49, "the grey dot, 7 x 7");
    assert_eq!(count(&sim, crate::ui::WHITE), 0, "no arrow, no NO FIX text");
    assert_consistent(&sim, &app);
    sim.gnss.fix = Some(5000);
    run(&mut sim, &mut app, 3000);
    assert!(!app.track.stale() && count(&sim, crate::ui::WHITE) > 20, "the fix: the arrow");
    assert_consistent(&sim, &app);
}

#[test]
fn map_page_follows_a_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("map_nofix");
    sim.gnss.fix = Some(8000);
    let area = crate::map::area(crate::ui::field_box(1, crate::ui::RIDE_H, 0).unwrap());
    let t0 = sim.ms;
    let (mut refits, mut refit_px, mut small_px, mut late_refits) = (0, 0, 0, 0);
    let mut seconds = 0;
    while sim.ms < t0 + 840_000 {
        let (view, px) = (app.map_view(0), sim.pixels);
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        let cost = sim.pixels - px;
        if app.map_view(0) != view {
            refits += 1;
            refit_px += cost;
            late_refits += u32::from(sim.ms > t0 + 720_000);
        } else {
            small_px += cost;
        }
        if (sim.ms - t0) / 1000 > seconds {
            seconds = (sim.ms - t0) / 1000;
            if seconds % 60 == 0 {
                assert_consistent(&sim, &app);
            }
            match seconds {
                30 => sim.save("map_start"),
                240 => sim.save("map_out"),
                _ => {}
            }
        }
    }
    assert_consistent(&sim, &app);
    sim.save("map_loop");
    let s = (sim.ms - t0) / 1000;
    std::println!(
        "map: {refits} full redraws ({} px each), {} px/s otherwise, {} px/s in all, over {s} s",
        refit_px / refits.max(1),
        small_px / s,
        (refit_px + small_px) / s
    );
    assert!((2..=12).contains(&refits), "fitted again as the ride grows: {refits}");
    assert_eq!(late_refits, 0, "the loop closed: the view stays");
    assert!(small_px / s < 2000, "{} px/s between full redraws", small_px / s);
    let v = app.map_view(0).unwrap();
    let (x, y) = v.px(area, app.track.pos().unwrap());
    assert_eq!(sim.screen[(y as u32 * W + x as u32) as usize], crate::ui::WHITE, "the arrow at the position");
    let track = sim.screen.iter().filter(|&&p| p == crate::ui::ACCENT).count();
    assert!(track > 500, "the track drawn: {track} px");
    // the map as one field of three, over speed and heart rate
    app.pages[0] =
        PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] };
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "from the map, round to the first page");
    run(&mut sim, &mut app, 10_000);
    assert_consistent(&sim, &app);
    sim.save("map_field");
}

/// Heart rate rising from 90 to 209 over two minutes, then again: every zone at the default max of 190.
fn hr_ramp(ms: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 90 + (ms / 1000 % 120) as u8]
}
/// Power rising from 100 to 395 W over a minute, then again: every zone at the default FTP of 200 W.
fn power_ramp(ms: u64) -> [u8; 8] {
    let [p0, p1] = (100 + (ms / 1000 % 60) as u16 * 5).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}
/// Climbing 0.2 m/s from 100 m.
fn climb(ms: u64) -> i32 {
    100_000 + (ms / 5) as i32
}

/// The ride keys and fields: a ride on the GPS route (28.8 km/h) with a strap and a power meter. Start, a lap,
/// a stop (auto-pause), Start to pause and resume, a long Start to end. The fields' values follow, heart rate and
/// power in their zone colours, the graph's bars too; the strip's time is green while recording, orange when
/// paused. Few pixels a second while riding.
#[test]
fn a_ride_with_laps_and_pauses() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_ramp });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Timer, Dist, AvgSpeed, AvgHr, AvgPower, Graph] };
    app.pages[1] = PageCfg { layout: 6, fields: [Hr, Power, MaxSpeed, LapTime, LapDist, Ascent] };
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!((text(&app, Timer), text(&app, Dist), app.strip.rec), ("0:00:00".into(), "0.00".into(), Rec::Idle));
    app.press(&mut sim, Press::Short(Key::Start));
    let (t0, mut px0) = (sim.ms, sim.pixels);
    let mut seconds = 0;
    while sim.ms < t0 + 720_000 {
        run(&mut sim, &mut app, 1000);
        seconds += 1;
        if seconds % 30 == 0 {
            assert_consistent(&sim, &app);
        }
        if seconds == 60 {
            let px_s = (sim.pixels - px0) / 60;
            std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), first minute: {px_s} px/s");
        }
        if seconds == 600 {
            px0 = sim.pixels; // the averages have settled
        }
    }
    // the averages of ramps settle as a long ride's do: from then on, mostly the timer, the distance, the graph
    let px_s = (sim.pixels - px0) / 120;
    std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), minutes 10 to 12: {px_s} px/s");
    assert!(px_s < 1500, "{px_s} px/s while riding");
    assert_eq!(app.strip.rec, Rec::Running);
    let timer = app.ride.timer_s();
    assert!((719..=721).contains(&timer), "timer {timer}");
    assert_eq!(text(&app, Dist), "5.76", "8 m/s for 720 s");
    assert_eq!(text(&app, AvgSpeed), "28.8");
    let bx = |i| crate::ui::field_box(6, crate::ui::RIDE_H, i).unwrap();
    let colors_in = |sim: &Sim, (x, y, w, h): (u32, u32, u32, u32)| {
        let mut c: Vec<u16> = (y..y + h)
            .flat_map(|y| (x..x + w).map(move |x| (x, y)))
            .map(|(x, y)| sim.screen[(y * W + x) as usize])
            .collect();
        c.sort();
        c.dedup();
        c
    };
    let graph = colors_in(&sim, bx(5));
    let zones = crate::ride::ZONE_COLORS.iter().filter(|c| graph.contains(c)).count();
    assert!(zones >= 5, "the power bars in their zones' colours: {zones} colours");
    sim.save("ride_page");
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Page));
    let (t0, px0) = (sim.ms, sim.pixels);
    run(&mut sim, &mut app, 60_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - t0);
    std::println!("ride page (HR, power changing every second, max km/h, lap time, lap km, ascent): {px_s} px/s");
    // the value colours: the zone of the heart rate and power on screen
    let (hr, w) = (app.live.get(Metric::Hr).unwrap(), app.live.get(Metric::Power).unwrap());
    let hr_fg = crate::ride::hr_color(crate::ride::hr_zone(hr, 190));
    let w_fg = crate::ride::power_color(crate::ride::power_zone(w, 200));
    assert!(colors_in(&sim, bx(0)).contains(&hr_fg) && colors_in(&sim, bx(1)).contains(&w_fg), "zone colours");
    assert_eq!((text(&app, LapTime), text(&app, LapDist)), ("0:01:00".into(), "0.48".into()));
    assert_eq!(text(&app, MaxSpeed), "28.8");
    let ascent = app.ride.ascent_m();
    assert!((150..=157).contains(&ascent), "0.2 m/s for 780 s: {ascent} m");
    assert_consistent(&sim, &app);
    sim.save("ride_page2");
    // stopped: auto-pause, the timer stops
    sim.gnss.fix = Some(0);
    run(&mut sim, &mut app, 3000);
    let timer = app.ride.timer_s();
    run(&mut sim, &mut app, 30_000);
    assert_eq!((app.ride.timer_s(), app.strip.rec), (timer, Rec::Paused), "auto-paused");
    assert_consistent(&sim, &app);
    sim.save("ride_autopause");
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 10_000);
    assert!(app.ride.timer_s() >= timer + 8 && app.strip.rec == Rec::Running, "moving again");
    app.press(&mut sim, Press::Short(Key::Start));
    let (timer, dist) = (app.ride.timer_s(), app.ride.dist_m());
    run(&mut sim, &mut app, 10_000);
    assert_eq!((app.ride.timer_s(), app.ride.dist_m(), app.strip.rec), (timer, dist, Rec::Paused), "paused");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert!(app.ride.timer_s() > timer, "resumed");
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.ride.state(), crate::ride::State::Running, "a long Start does not end a running ride");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert_eq!((app.ride.state(), app.strip.rec), (crate::ride::State::Idle, Rec::Idle), "ended");
    assert!(app.ride.dist_m() > 2000, "the totals stay on screen");
    assert_consistent(&sim, &app);
    sim.save("ride_ended");
}

/// A dual-sided power meter: 300 W, 85 rpm, the right pedal 48 % of the power.
fn power_lr(_: u64) -> [u8; 8] {
    [0x10, 1, 0x80 | 48, 85, 0, 0, 0x2c, 0x01]
}

/// The derived fields (power.rs, ride.rs): a ride at 300 W on the GPS route (8 m/s, climbing 0.2 m/s) with a
/// strap and a dual-sided power meter, at the default FTP (200 W), weight (75 kg) and W' (20 kJ). After two
/// minutes the fields show the values worked out by hand, W' balance in its draining colour, and the screen
/// agrees with the screenshot.
#[test]
fn derived_power_and_climb_fields() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_lr });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Power3s, Power10s, Power30s, Np, If, Tss] };
    app.pages[1] = PageCfg { layout: 6, fields: [Kj, Kcal, Wkg, Wbal, MaxPower, MaxHr] };
    app.pages[2] = PageCfg { layout: 6, fields: [AvgCadence, Balance, Vam, Grade, Timer, Time] };
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!((text(&app, Np), text(&app, Wbal), text(&app, Kj)), ("--".into(), "--".into(), "0".into()));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 120_000);
    let t = app.ride.timer_s();
    assert!((119..=121).contains(&t), "timer {t}");
    // 300 W steady: every average 300, NP 300, IF 300 / 200, TSS t x 300^2 x 100 / (200^2 x 3600) = 7.5 at 120 s
    for f in [Power3s, Power10s, Power30s, Np, MaxPower] {
        assert_eq!(text(&app, f), "300", "{f:?}");
    }
    assert_eq!((text(&app, If), text(&app, Tss)), ("1.50".into(), "7".into()));
    // 300 W for t s: 0.3 t kJ, and kcal = kJ / (4.184 x 0.24); 300 / 75 = 4.0 W/kg
    let kj = 3 * t / 10;
    assert_eq!((text(&app, Kj), text(&app, Kcal)), (format!("{kj}"), format!("{}", kj * 1000 * 100 / 100_416)));
    assert_eq!(text(&app, Wkg), "4.0");
    // 100 W over CP for every second since Start (the first seconds count before the GPS speed moves the timer)
    let wbal = app.power.wbal_j().unwrap();
    assert!((7900..=8100).contains(&wbal), "W' bal {wbal} J: 20 kJ - 120 s x 100 W");
    assert_eq!(text(&app, Wbal), format!("{}.{}", wbal / 1000, wbal % 1000 / 100));
    assert_eq!(app.field_color(Wbal), crate::ride::ZONE_COLORS[4], "40 %: orange");
    assert_eq!(app.field_color(Np), crate::ride::power_color(6), "150 % of FTP");
    assert_eq!((text(&app, AvgCadence), text(&app, Balance)), ("85".into(), "52/48".into()));
    let max_hr = app.ride.max_hr().unwrap();
    assert_eq!(text(&app, MaxHr), format!("{max_hr}"));
    assert!(max_hr >= 200, "the strap's ramp, 90 bpm up");
    // 0.2 m/s up at 8 m/s: 720 m/h, 2.5 % (GPS altitude in whole fixes: a little either way)
    let (vam, grade) = (app.ride.vam().unwrap(), app.ride.grade_x10().unwrap());
    assert!((700..=740).contains(&vam) && (24..=26).contains(&grade), "VAM {vam}, grade {grade}");
    assert_eq!((text(&app, Vam), text(&app, Grade)), (format!("{vam}"), format!("2.{}", grade % 10)));
    assert_consistent(&sim, &app);
    sim.save("power_fields");
    for name in ["power_fields2", "power_fields3"] {
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 3000);
        assert_consistent(&sim, &app);
        sim.save(name);
    }
    // a new ride: the totals and W' start again
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Back)); // the card, then the overview
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert!(app.power.wbal_j().unwrap() > 19_000 && app.power.kj() < 2, "a new ride");
}

/// Weight and W' from the menu, kept in RTC backup register 16 through a restart; the other settings stay in
/// register 11.
#[test]
fn weight_and_wprime_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Weight));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..7 {
        app.press(&mut sim, Press::Short(Key::Page)); // 75 -> 68 kg
    }
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page)); // W' row
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Lap)); // 20.0 -> 21.5 kJ
    }
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::WPrime))), "W' KJ < 21.5 >");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Weight))), "WEIGHT KG 68");
    assert_consistent(&sim, &app);
    sim.save("power_settings");
    assert_eq!(sim.backup[16] >> 24, 0xc1, "tagged");
    let again = App::start(&mut sim, None);
    assert_eq!((again.settings.weight_kg, again.settings.wprime_j, again.settings.ftp), (68, 21_500, 200));
}

/// Power as ridden: a two-minute swell from 140 to 320 W, +-30 W of noise from one second to the next.
fn power_noisy(ms: u64) -> [u8; 8] {
    let s = ms / 1000;
    let swell = (s % 120).min(120 - s % 120) as u16;
    let noise = (s.wrapping_mul(2_654_435_761) >> 7) % 61;
    let [p0, p1] = (110 + 3 * swell + noise as u16).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}

/// The graph field: power in its zones' colours, one pixel column per sample, drawn by differences as it
/// scrolls (the screenshot agrees). Prints the pixels a second it costs.
#[test]
fn graph_field() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 3, fields: [Graph, MaxSpeed, HrGraph, Timer, Timer, Timer] };
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600_000);
    assert_consistent(&sim, &app);
    let (px0, win0) = (sim.pixels, sim.windows);
    run(&mut sim, &mut app, 120_000);
    let (px_s, win_s) = ((sim.pixels - px0) / 120, (sim.windows - win0) / 120);
    std::println!("graph field (240 x 135), noisy power: {px_s} px/s in {win_s} windows/s");
    assert_consistent(&sim, &app);
    let (x, y, cols, h) = crate::ui::graph_area(crate::ui::field_box(3, crate::ui::RIDE_H, 0).unwrap());
    let bottom = &sim.screen[((y + h - 1) * W + x) as usize..][..cols as usize];
    assert!(bottom.iter().all(|&c| crate::ride::ZONE_COLORS.contains(&c)), "a filled area, no gaps");
    sim.save("graph");
}

/// A tap on a graph shows the next plot (power, heart rate, W' balance, DFA alpha1, elevation), kept with the page;
/// a hold switches every graph between the last minutes and the whole ride.
#[test]
fn graph_plots_by_tap_and_span_by_hold() {
    let taps = [400_000u64, 401_000, 402_000, 403_000];
    let hold = 405_000u64;
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| {
        let tap = taps.iter().any(|&t| (t..t + 80).contains(&ms));
        let held = (hold..hold + 900).contains(&ms);
        (tap || held).then_some((120, 100))
    });
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 2, fields: [Graph, HrGraph, Timer, Timer, Timer, Timer] };
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // a ride: W' balance runs
    sim.gnss.fix = Some(7000);
    let mut shots = std::vec::Vec::new();
    for (k, want) in [HrGraph, WbalGraph, DfaGraph, AltGraph].into_iter().enumerate() {
        let until = taps[k] + 500 - sim.ms;
        run(&mut sim, &mut app, until);
        assert_eq!(app.pages()[0].fields[0], want, "tap {k}");
        assert_consistent(&sim, &app);
        shots.push(std::format!("graph_{k}"));
        sim.save(&shots[k]);
    }
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]).unwrap().fields[0], AltGraph, "kept");
    let until = hold + 1500 - sim.ms;
    run(&mut sim, &mut app, until);
    assert!(app.graph_whole, "the hold");
    assert_eq!(app.screen(), Screen::Ride(0), "on a graph, a hold is not the menu");
    assert_consistent(&sim, &app);
    sim.save("graph_whole");
    run(&mut sim, &mut app, 60_000);
    assert_consistent(&sim, &app);
}

/// After a fault (hw.rs marks it in backup register 29 and resets): a notice at start, and the count cleared after
/// a minute of running.
#[test]
fn restart_after_a_fault() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[FAULT_BKP] = 0xfa00_0001;
    let mut app = App::start(&mut sim, None);
    assert_eq!(app.notices[0].as_str(), "09:00 FAULT RESET 1");
    run(&mut sim, &mut app, u64::from(FAULT_CLEAR_MS) - 2000);
    assert_eq!(sim.backup[FAULT_BKP], 0xfa00_0001, "kept a while");
    run(&mut sim, &mut app, 4000);
    assert_eq!(sim.backup[FAULT_BKP], 0, "a minute of running: cleared");
}

/// A speed sensor paired but silent (a dead battery) does not leave the ride without speed: the GPS's is used.
#[test]
fn silent_speed_sensor_falls_back_to_gps() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Speed.backup()] = SensorId { device: 99, trans: 1 }.to_word(); // paired, never heard
    sim.gnss.route = Some(ride_route);
    let mut app = App::start(&mut sim, Some(KEY));
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.live.get(Metric::Speed), Some(288), "8 m/s from the GPS");
}

/// During a ride, holding Back once only warns (switching off would lose it); a second hold within 10 s
/// switches off. With no ride, the first hold does.
#[test]
fn off_during_a_ride_asks_twice() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.standby && app.banner.as_str() == "HOLD AGAIN: OFF", "warned");
    run(&mut sim, &mut app, 12_000);
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(!sim.standby, "the warning ran out: warned again");
    app.press(&mut sim, Press::Long(Key::Back));
    assert!(sim.standby, "the second hold");
    assert!(app.busy() || app.ride.state() != crate::ride::State::Idle);
}

/// A pass that takes longer than a second (an eMMC or radio wait) loses no ride time: the ride counts every
/// second gone, up to MAX_CATCH_UP (more is the clock being set).
#[test]
fn long_pass_loses_no_ride_time() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let t = app.ride.timer_s();
    sim.delay_ms(3000); // a 3 s pass
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.ride.timer_s(), t + 4, "the 3 s of the long pass and the next second");
    sim.delay_ms(30_000); // the clock jumps (set from the GPS): counted up to MAX_CATCH_UP only
    run(&mut sim, &mut app, 100);
    assert!(app.ride.timer_s() <= t + 4 + u32::from(MAX_CATCH_UP) + 1);
}

/// Idle is free, and no pass stalls the loop: on the overview with nothing changing, nearly every pass sends
/// nothing to the panel (the clock's minute aside), and on a ride page while riding no pass costs the panel more
/// than PASS_BUS_MS at the panel's speed (about 2 us a pixel, 20 us a window at 8 MHz: a model, measured on the
/// unit as one full map of 72 K pixels in about 146 ms).
#[test]
fn idle_is_free_and_no_pass_stalls() {
    const PASS_BUS_MS: u64 = 60;
    let bus_us = |px: u64, win: u64| px * 2 + win * 20;
    // the overview, idle
    let mut sim = Sim::new(9, 0, 30);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    let (mut busy, mut passes) = (0, 0);
    for _ in 0..1200 {
        let (px, win) = (sim.pixels, sim.windows);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        passes += 1;
        busy += u32::from(sim.pixels != px || sim.windows != win);
    }
    // the strip's GPS label blinks while it searches (once a second), nothing else: a pass in 20 at most, plus
    // a few for the minute's digits and the temperature
    assert!(busy <= passes / 20 + 3, "{busy} of {passes} passes drew on an idle overview");
    // a ride page while riding (values change every second): no pass over PASS_BUS_MS of panel time
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy });
    let mut app = App::start(&mut sim, Some(KEY));
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 10_000);
    let mut worst = 0;
    for _ in 0..1200 {
        let (px, win) = (sim.pixels, sim.windows);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        worst = worst.max(bus_us(sim.pixels - px, sim.windows - win));
    }
    std::println!("idle overview: {busy} of {passes} passes drew; ride page: worst pass {} ms of panel", worst / 1000);
    assert!(worst / 1000 <= PASS_BUS_MS, "a pass took {} ms of panel time", worst / 1000);
}

/// Reminders: EAT after 5 minutes of riding, said in a banner with three beeps that do not stop the loop;
/// nothing while paused; set from the ALERTS menu and kept.
#[test]
fn eat_reminder_on_a_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Alerts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Alerts, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // EAT MIN: edit
    app.press(&mut sim, Press::Short(Key::Lap)); // 5
    app.press(&mut sim, Press::Short(Key::Start)); // kept
    assert_eq!(Settings::from_word(0).with_alerts_word(sim.backup[28]).eat_min, 5);
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Back));
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    let tones0 = sim.tones.len();
    let steps0 = sim.ms;
    run(&mut sim, &mut app, 299_000);
    assert_eq!(sim.tones.len(), tones0, "not yet");
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.banner.as_str(), "EAT");
    let beeps: std::vec::Vec<u32> = sim.tones[tones0..].to_vec();
    assert_eq!(beeps, [2500, 0, 2500, 0, 2500, 0], "three beeps, then off");
    assert!(sim.ms - steps0 >= 300_000, "{}", sim.ms - steps0);
    assert_consistent(&sim, &app);
    sim.save("eat_reminder");
    // paused on the minute: nothing more
    app.press(&mut sim, Press::Short(Key::Start));
    let n = sim.tones.len();
    run(&mut sim, &mut app, 120_000);
    assert_eq!(sim.tones.len(), n, "paused: no reminder");
}

/// A workout's step tone and an alert in the same pass: one tone player, so both play whole, one after the other
/// (review 2: two players cut each other off).
#[test]
fn tones_do_not_cut_each_other() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    let t0 = sim.tones.len();
    app.workout_event(workout::Event::Done);
    app.alert = Some(alerts::Alert::Eat);
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.tones[t0..], [2500, 0, 2500, 0, 2500, 0, 2500, 0], "the step's tone, then the alert's three");
}

/// Performance budgets: in steady state, each kind of screen sends at most so many pixels and panel windows a
/// second (every pixel is a SPI transfer and CPU time, AGENTS.md "redraw only what changes"). A change that
/// redraws more than it should fails here before it costs battery on the unit. Measured 2026-10-24, budgets with
/// a margin; the numbers are printed (`cargo test-host perf_budgets -- --nocapture`).
#[test]
fn perf_budgets() {
    use Field::*;
    // a ride on the GPS route, with a power meter and a strap, from start-up
    let ride = |pages: [PageCfg; 3], screen: usize| {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        let course = pages.iter().any(|p| p.fields.contains(&Profile));
        if course {
            sim.emmc.image = crate::fat::tests::image();
            sim.gnss.route = Some(course_route);
        }
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy });
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
        let mut app = App::start(&mut sim, Some(KEY));
        app.pages[..3].copy_from_slice(&pages);
        if course {
            app.set_course_memory(crate::storage::course::tests::memory());
            app.open_courses(&mut sim);
            app.follow_course(&mut sim, hill(&app));
        }
        sim.gnss.fix = Some(8000);
        if screen < PAGES {
            app.press(&mut sim, Press::Short(Key::Start));
            for _ in 0..screen {
                app.press(&mut sim, Press::Short(Key::Page));
            }
        }
        run(&mut sim, &mut app, 120_000); // settled: the map fitted, the graph full
        let (px, win, t0) = (sim.pixels, sim.windows, sim.ms);
        run(&mut sim, &mut app, 300_000);
        let s = (sim.ms - t0) / 1000;
        assert_consistent(&sim, &app);
        ((sim.pixels - px) / s, (sim.windows - win) / s)
    };
    let p = |layout, f: [Field; 6]| PageCfg { layout, fields: f };
    let values = p(6, [Speed, Hr, Power, Power3s, Dist, Timer]);
    let graph = p(2, [Graph, HrGraph, Time, Time, Time, Time]);
    let map = p(1, [Map, Time, Time, Time, Time, Time]);
    let profile = p(4, [Profile, ToGo, Climb, Time, Time, Time]);
    let cases: [(&str, [PageCfg; 3], usize, (u64, u64)); 5] = [
        // measured 53 px/s, 0 windows/s (the clock's digits once a minute)
        ("overview", [values, graph, map], PAGES, (150, 5)),
        // 6794, 8: the changed digits only, but large (a glyph of a 6-field page is ~960 px)
        ("six values", [values, graph, map], 0, (9_000, 20)),
        // 646, 17: the changed columns of two graphs, a step of 5 bars every 10 s
        ("two graphs", [graph, values, map], 0, (1_000, 30)),
        // 1112, 33: the track's new segments and the arrow, refits included
        ("map", [map, values, graph], 0, (1_800, 50)),
        // 456, 3: on the course, the profile's rider column (a whole profile at each eighth), two values
        ("course profile", [profile, values, map], 0, (600, 10)),
    ];
    let mut over = std::vec::Vec::new();
    for (name, pages, screen, (px_max, win_max)) in cases {
        let (px, win) = ride(pages, screen);
        std::println!("{name}: {px} px/s, {win} windows/s (budget {px_max}, {win_max})");
        if px > px_max || win > win_max {
            over.push(std::format!("{name}: {px} px/s, {win} windows/s over {px_max}, {win_max}"));
        }
    }
    assert!(over.is_empty(), "{over:?}");
}

/// The synthetic hill (tools/fat_image.py: 2 km east on the flat at 100 m, 2 km north up to 220 m, 1 km east)
/// ridden at 8 m/s from its start, with a detour 150 m south from 60 s to 95 s.
fn course_route(ms: u64) -> (i32, i32, i32) {
    let (s, t) = (ms as f64 * 0.008, ms / 1000);
    let (x, mut y, heading) = match s {
        s if s < 2000.0 => (-2000.0 + s, -1500.0, 90.0),
        s if s < 4000.0 => (0.0, -3500.0 + s, 0.0),
        s => (s - 4000.0, 500.0, 90.0),
    };
    if (60..95).contains(&t) {
        y -= 150.0;
    }
    let m_per_unit = 0.011_132; // metres per 1e-7 degree, as map.rs (at 0 N the same east and north)
    ((y / m_per_unit) as i32, (x / m_per_unit) as i32, (heading * 1e5) as i32)
}

/// The list row of stock's hill (in \PlanTrip: the first HILL listed).
fn hill(app: &App) -> usize {
    let list = &app.courses.as_deref().unwrap().list;
    (0..list.len()).find(|&i| list.get(i).unwrap().name() == "HILL").unwrap()
}

/// Following a course (tools/fat_image.py's hill, in stock's \PlanTrip): COURSES lists the card's courses
/// with their lengths, Select follows one (the first ride page). On the ride the fields say what is left, the
/// climb ahead and the distance off the course; a detour gives "OFF COURSE" (once, with the alert's beeps) and
/// "BACK ON COURSE"; the climb's start a banner and a beep; then the fields follow the climb. The map draws the
/// course under the track; the profile moves by a column at a time. Following costs a few dozen segments a
/// second.
#[test]
fn course_ride() {
    use Field::*;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((0, 0)) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(course_route);
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    app.set_course_memory(crate::storage::course::tests::memory());
    app.pages[0] = PageCfg { layout: 6, fields: [ToGo, OffCourse, Climb, ClimbGain, UpToGo, Timer] };
    app.pages[1] = PageCfg { layout: 3, fields: [Profile, Climb, ClimbGain, Time, Time, Time] };
    app.pages[2] = PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] };
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    let fields = |app: &App| [ToGo, OffCourse, Climb, ClimbGain, UpToGo].map(|f| text(app, f));

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Courses, 0));
    let row = |app: &App, i| {
        app.menu_row(Menu::Courses, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str())).unwrap_or_default()
    };
    let mut rows: Vec<_> = (0..5).map(|i| row(&app, i)).collect();
    rows.sort();
    assert_eq!(rows, ["", "BROKEN --", "HILL 5.0 KM", "HILL 5.0 KM", "HILL GARMIN 5.0 KM"]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("courses_menu");

    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.banner.as_str()), (Screen::Ride(0), "COURSE ON"));
    assert_eq!(fields(&app), ["--", "--", "--", "--", "--"], "no fix yet");
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    run(&mut sim, &mut app, 30_000);
    let s = sim.ms as f64 * 0.008;
    let to_go: f64 = text(&app, ToGo).parse().unwrap();
    assert!((to_go - (5000.0 - s) / 1000.0).abs() <= 0.15, "{to_go} km to go after {s} m");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap() <= 2, "on the course");
    assert_eq!(fields(&app)[2..], ["2.0K 6%", "120", "120"], "the climb ahead: 2 km at 6 %, 120 m");
    assert_consistent(&sim, &app);

    // the detour: off course after 5 s beyond 50 m, said once; back
    let tones = sim.tones.len();
    while sim.ms < 63_500 {
        run(&mut sim, &mut app, 500);
        assert_ne!(app.banner.as_str(), "OFF COURSE", "not before 5 s off: {}", sim.ms);
    }
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.banner.as_str(), "OFF COURSE");
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.tones[tones..tones + 3], [2500, 0, 2500], "the alert's beeps");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap().abs_diff(150) <= 2 && app.field_color(OffCourse) == ui::RED);
    assert_consistent(&sim, &app);
    while sim.ms < 95_500 {
        run(&mut sim, &mut app, 500);
    }
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.banner.as_str(), "BACK ON COURSE");
    assert!(text(&app, OffCourse).parse::<u32>().unwrap() <= 2 && app.field_color(OffCourse) == ui::WHITE);

    // the climb: a banner at its start, then what is left of it
    while sim.ms < 249_000 {
        run(&mut sim, &mut app, 1000);
        assert!(!app.banner.as_str().starts_with("CLIMB"), "not before the climb: {}", sim.ms);
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.banner.as_str(), "CLIMB 2.0 KM 6%");
    run(&mut sim, &mut app, 47_000); // 400 m up
    let f = fields(&app);
    assert_eq!(f[2], "1.6K 6%", "1.6 km left: {f:?}");
    let m = |i: usize| f[i].parse::<u32>().unwrap();
    assert!(m(3).abs_diff(96) <= 2 && m(4).abs_diff(96) <= 6, "96 m left to climb: {f:?}");
    assert_eq!(app.field_color(Climb), ui::YELLOW, "6 %");
    assert_consistent(&sim, &app);
    sim.save("course_fields");

    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("course_profile");
    let (px, t0) = (sim.pixels, sim.ms);
    run(&mut sim, &mut app, 20_000);
    assert_consistent(&sim, &app);
    std::println!("profile page: {} px/s", (sim.pixels - px) * 1000 / (sim.ms - t0));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_consistent(&sim, &app);
    sim.save("course_map");
    let course = sim.screen.iter().filter(|&&p| p == crate::map::COURSE).count();
    assert!(course > 150, "the course on the map (under the track where ridden): {course} px");

    // the cost: segments and boxes looked at a second, detour included
    let at = app.courses.as_deref().unwrap().at;
    let per_s = at.examined / (sim.ms / 1000) as u32;
    std::println!("course following: {per_s} segments a second ({} in all)", at.examined);
    assert!(per_s <= 60, "{per_s} segments a second");

    // Select on the course followed stops following it
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.banner.as_str(), "COURSE OFF");
    assert_eq!(fields(&app), ["--", "--", "--", "--", "--"]);
}

/// Max HR and FTP from the menu: Start edits the value in place, Lap steps it up (1 bpm, 5 W), Page down,
/// holding a key repeats, Start keeps it; kept in the wheel's register beside the other settings.
#[test]
fn max_hr_and_ftp_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Set(Setting::Wheel));
    app.press(&mut sim, Press::Short(Key::Start)); // wheel: next size
    for _ in row_of(menu::Row::Set(Setting::Wheel))..row_of(menu::Row::Set(Setting::MaxHr)) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start)); // edit MAX HR
    for _ in 0..5 {
        app.press(&mut sim, Press::Short(Key::Lap)); // 190 -> 195
    }
    app.press(&mut sim, Press::Short(Key::Page)); // 194
    sim.save("max_hr_edit");
    app.press(&mut sim, Press::Short(Key::Start)); // kept
    app.press(&mut sim, Press::Short(Key::Page)); // FTP row
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..200 {
        app.press(&mut sim, Press::Repeat(Key::Page)); // held: 200 W down to the floor
    }
    app.press(&mut sim, Press::Short(Key::Page)); // the release does not step again
    app.press(&mut sim, Press::Short(Key::Lap)); // 55
    app.press(&mut sim, Press::Short(Key::Back)); // kept, still in the menu
    assert!(matches!(app.screen, Screen::Menu(Menu::Main, r) if r == row_of(menu::Row::Set(Setting::Ftp))));
    assert_consistent(&sim, &app);
    sim.save("zones_menu");
    let again = App::start(&mut sim, None);
    assert_eq!((again.settings.max_hr, again.settings.ftp, again.settings.wheel_mm), (194, 55, WHEELS[2]));
    assert_eq!((again.settings.light_level, again.settings.light_timeout), (0, 0), "the backlight settings untouched");
}

/// Continuous capture: 4 s with a Page press, one BMP per changed frame in target/sim/run/.
#[test]
fn record_a_run() {
    let mut sim = Sim::new(9, 59, 57).keys(|ms| if ms / 100 == 25 { Key::Page as u8 } else { 0 }).record("run");
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 4000);
    let frames = std::fs::read_dir(out_dir().join("run")).unwrap().count();
    assert!(frames >= 2, "the first page, then the second, got {frames}");
}
