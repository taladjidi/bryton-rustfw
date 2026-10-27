//! Time budgets on the Sim: long reads spread over the passes, the cost of a pass, a menu, the overview and the
//! idle loop.

use super::course_tests::course_route;
use super::course_tests::hill;
use super::map_tests::ride_route;
use super::menu_tests::to_row;
use super::ride_tests::hr_fast;
use super::ride_tests::power_fast;
use super::ride_tests::power_noisy;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::Field;

/// The synthetic card plus big files: a 2 MB GPX course, a ride of about 1.5 MB, and 120 days of planned workouts
/// of 9 KB each (too long to be workouts, but a reader that does not look first reads 8 KB of each).
fn big_card() -> Vec<u8> {
    use crate::fat::{Fat, tests::Ram, write::dos_time};
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let t = dos_time((2026, 10, 20, 8, 0, 0));
    let put = |fat: &mut Fat, d: &mut Ram, dir: crate::fat::File, name: &str, bytes: &[u8]| {
        let mut padded = bytes.to_vec();
        padded.resize(bytes.len().div_ceil(512) * 512, 0);
        let mut h = fat.create(d, dir, name, t).unwrap();
        fat.write_sectors(d, &mut h, 0, &padded).unwrap();
        fat.set_size(d, &mut h, bytes.len() as u32, t).unwrap();
    };
    let mut gpx = b"<gpx><trk><trkseg>\n".to_vec();
    for k in 0.. {
        if gpx.len() > 2 << 20 {
            break;
        }
        let line = format!("<trkpt lat=\"{:.6}\" lon=\"0.001\"><ele>{}</ele></trkpt>\n", f64::from(k) * 1e-4, k % 50);
        gpx.extend_from_slice(line.as_bytes());
    }
    gpx.extend_from_slice(b"</trkseg></trk></gpx>\n");
    let dir = fat.open(&mut d, "Courses").unwrap();
    put(&mut fat, &mut d, dir, "Big.gpx", &gpx);
    let t0 = crate::fit::fit_time((2026, 10, 21, 8, 0, 0));
    let mut ride = Vec::new();
    let mut w = crate::fit::Writer::begin(&mut ride, t0).unwrap();
    for k in 0..50_000u32 {
        let s = crate::fit::Sample {
            time: t0 + k,
            pos: Some((k as i32 * 10, 1000)),
            alt_mm: Some(100_000),
            speed_mm_s: Some(8000),
            hr: Some(140),
            cadence: Some(90),
            power: Some(200),
            temp_c: Some(20),
            ..Default::default()
        };
        w.record(&mut ride, &s).unwrap();
    }
    w.finish(&mut ride, t0 + 50_000).unwrap();
    let dir = fat.open(&mut d, "Activities").unwrap();
    put(&mut fat, &mut d, dir, "261021080000.fit", &ride);
    let cal = fat.open(&mut d, "Workout/calendar").unwrap();
    for k in 0..120u32 {
        let name = format!("2027{:02}{:02}", 1 + k / 28, 1 + k % 28);
        let day = fat.mkdir(&mut d, cal, &name, t).unwrap();
        put(&mut fat, &mut d, day, "plan.fit", &[0x55; 9 * 1024]);
    }
    d.0
}

/// The bytes read by `f`, the most of a pass so far kept in `worst`.
fn pass(
    sim: &mut Sim,
    app: &mut App,
    worst: &mut (u64, &'static str),
    what: &'static str,
    f: impl FnOnce(&mut Sim, &mut App),
) {
    let before = sim.emmc.bytes_read;
    f(sim, app);
    let n = sim.emmc.bytes_read - before;
    if n > worst.0 {
        *worst = (n, what);
    }
}

/// Passes of the main loop for `ms`, each measured as [`pass`].
fn passes(sim: &mut Sim, app: &mut App, worst: &mut (u64, &'static str), what: &'static str, ms: u64) {
    for _ in 0..ms / u64::from(POLL_MS) {
        pass(sim, app, worst, what, |sim, app| app.step(sim));
        sim.delay_ms(POLL_MS);
    }
}

/// Opening COURSES, following a course, a ride's card in HISTORY and WORKOUTS on a card with big files: no pass of
/// the main loop reads more than 300 ms of the unit's measured 2.5 MB/s (read and parsed, [`App::SCAN_IDLE`]); the
/// long reads go on over the passes, and their results come (review 2: each was one pass, up to 4 MB).
#[test]
fn long_reads_spread_over_passes() {
    const MAX: u64 = 2_500_000 * 300 / 1000;
    let card = big_card();
    let mut worst = (0, "");
    let w = &mut worst;
    let fresh = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = card.clone();
        let mut app = App::start(&mut sim, None);
        app.set_course_memory(crate::storage::course_file::tests::memory());
        app.press(&mut sim, Press::Long(Key::Page));
        (sim, app)
    };
    // COURSES: the list at once, the lengths over the passes
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Courses));
    pass(&mut sim, &mut app, w, "courses", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "course lengths", 5000);
    let list = &app.courses.as_deref().unwrap().list;
    let big = (0..list.len()).find(|&i| list.get(i).unwrap().name() == "BIG").unwrap();
    let (_, value, _) = app.menu_row(Menu::Courses, big).unwrap();
    let listed = std::string::String::from(value.as_str());
    // following the big course
    for _ in 0..big {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    pass(&mut sim, &mut app, w, "course load", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "course load", 5000);
    assert_eq!(app.screen(), Screen::Ride(crate::COURSE), "followed: the course page");
    let len = app.courses.as_deref().unwrap().course.len_m();
    std::println!("big course: {listed} listed, {len} m followed");
    let km: f64 = listed.trim_end_matches(" KM").parse().unwrap();
    assert!(len > 300_000 && (km * 1000.0 - f64::from(len)).abs() < 100.0, "{listed}: {len} m");
    // HISTORY: the scan, then the big ride's card
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    while app.stock.status() == crate::stock::Status::Running {
        passes(&mut sim, &mut app, w, "scan", 1000);
        assert!(sim.ms < 300_000);
    }
    let n = (0..app.stock.index.len()).find(|&i| app.stock.index.get(i).unwrap().start.2 == 21).unwrap();
    for _ in 0..n + 1 {
        // FITNESS first
        app.press(&mut sim, Press::Short(Key::Page));
    }
    pass(&mut sim, &mut app, w, "ride card", |sim, app| app.press(sim, Press::Short(Key::Start)));
    passes(&mut sim, &mut app, w, "ride card", 5000);
    assert_eq!((app.screen(), app.past.card.title.as_str()), (Screen::Past, "21 OCT 08:00"));
    assert!(app.past.bytes > 1 << 20, "{}", app.past.bytes);
    // WORKOUTS
    let (mut sim, mut app) = fresh();
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    pass(&mut sim, &mut app, w, "workouts", |sim, app| app.press(sim, Press::Short(Key::Start)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    assert_eq!(app.menu_row(Menu::Workouts, 0).unwrap().0.as_str(), "FRI 23 OCT", "today's still first");
    std::println!("the most read in a pass: {} KB ({})", worst.0 / 1024, worst.1);
    assert!(worst.0 <= MAX, "{} B in one pass ({})", worst.0, worst.1);
}

/// Performance budgets: in steady state, each kind of screen sends at most so many pixels and panel windows a
/// second (every pixel is a SPI transfer and CPU time, AGENTS.md "redraw only what changes"). A change that
/// redraws more than it should fails here before it costs battery on the unit. Measured 2026-10-24, budgets with
/// a margin; the numbers are printed (`cargo test-host perf_budgets -- --nocapture`).
#[test]
fn perf_budgets() {
    use Field::*;
    // a ride on the GPS route, with a power meter and a strap, from start-up
    let ride = |pages: [crate::page::Page; 3], screen: usize, zoom: Option<u32>, fast: bool| {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        let course = screen == crate::COURSE || pages.iter().any(|p| p.fields.contains(&Profile));
        if course {
            sim.emmc.image = crate::fat::tests::image();
            sim.gnss.route = Some(course_route);
        }
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        type Broadcast = fn(u64) -> [u8; 8]; // a sensor's page at a time
        let (power_fn, hr_fn): (Broadcast, Broadcast) = if fast { (power_fast, hr_fast) } else { (power_noisy, hr72) };
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_fn, network: 0, ack: &[] });
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_fn, network: 0, ack: &[] });
        let mut app = App::start(&mut sim, Some(KEY));
        app.pages[..3].copy_from_slice(&pages);
        if course {
            app.set_course_memory(crate::storage::course_file::tests::memory());
            app.open_courses(&mut sim);
            app.follow_course(hill(&app));
            for _ in 0..100 {
                app.step(&mut sim); // read over a few passes
            }
            assert!(app.courses.as_deref().unwrap().on.is_some());
        }
        sim.gnss.fix = Some(8000);
        if screen != PAGES {
            app.press(&mut sim, Press::Short(Key::Start));
            app.screen = Screen::Ride(screen); // the page measured (a course loaded opens the course page)
            app.map_zoom = zoom;
            app.show(&mut sim);
        }
        run(&mut sim, &mut app, 120_000); // settled: the map fitted, the graph full
        let (px, win, t0) = (sim.pixels, sim.windows, sim.ms);
        run(&mut sim, &mut app, 300_000);
        let s = (sim.ms - t0) / 1000;
        assert_consistent(&sim, &app);
        ((sim.pixels - px) / s, (sim.windows - win) / s)
    };
    let p = |layout, f: [Field; 6]| crate::page::Page::from(PageCfg { layout, fields: f });
    // pages made in the editor: eight values in four rows of two; a graph over two rows of two values
    let eight = crate::page::Page::preset(14, [Speed, Hr, Power, Cadence, Dist, Timer, Power3s, Np]).unwrap();
    let mut made = crate::page::Page::preset(1, [Graph, Time, Time, Time, Time, Time, Time, Time]).unwrap();
    made = made.split_down(0, Power3s).and_then(|p| p.resize(0, true)).and_then(|p| p.split_across(1, Hr)).unwrap();
    made = made.split_down(1, Cadence).and_then(|p| p.split_across(3, Timer)).and_then(|p| p.resize(3, true)).unwrap();
    let values = p(6, [Speed, Hr, Power, Power3s, Dist, Timer]);
    let graph = p(2, [Graph, HrGraph, Time, Time, Time, Time]);
    let map = p(1, [Map, Time, Time, Time, Time, Time]);
    let profile = p(4, [Profile, ToGo, Climb, Time, Time, Time]);
    type Case = (&'static str, [crate::page::Page; 3], usize, Option<u32>, bool, (u64, u64));
    let cases: [Case; 11] = [
        // measured 4 px/s, 0 windows/s (the strip; was 53 with the big clock's digits once a minute)
        ("overview", [values, graph, map], PAGES, None, false, (30, 3)),
        // made in the editor: 2162, 6 (7101, 10 with the zone fills, 8959 at VALUE_MARGIN 2): eight values at 3x in
        // 120 x 74 boxes, the readings once a second, the zone a bar
        ("eight values", [eight, graph, map], 0, None, false, (3_500, 12)),
        // 1537, 20 (3581, 22 with the zone fills): a graph over four values (the second row of values taller)
        ("graph over four", [made, values, map], 0, None, false, (2_500, 30)),
        // 2037, 6: the changed digits only, once a second, and a zone bar (about 1,000 px) at a zone change, held
        // three seconds. 8070, 10 with the zone fills (a box of ~12,000 px at each change, the noisy power's 3 s
        // average crossing an edge about every 4 s), 6538 before them
        ("six values", [values, graph, map], 0, None, false, (3_000, 12)),
        // the same page with a power meter and a strap that change at every broadcast (4 Hz, like the real ones):
        // 2198, 6 with the readings shown once a second (taken in the second's tick); 11339, 20 when each broadcast
        // was drawn and the zones filled the boxes
        ("six values, 4 Hz sensors", [values, graph, map], 0, None, true, (3_000, 12)),
        // 646, 17: the changed columns of two graphs, a step of 5 bars every 10 s
        ("two graphs", [graph, values, map], 0, None, false, (1_000, 30)),
        // 1494, 33: the track's new segments and the arrow, refits included (1260 before the fit kept the arrow
        // clear of the zoom buttons, 1211 without them)
        ("map", [map, values, graph], 0, None, false, (1_800, 50)),
        // 584, 3: on the course, the profile's rider column (a whole profile at each eighth), two values, and the
        // hill's left turn said twice (2026-10-25: 456 before the cues; a banner and the page under it again, about
        // 19 000 px each)
        ("course profile", [profile, values, map], 0, None, false, (800, 10)),
        // 1999, 9: the course page (COURSE_PAGE): the profile's rider column, the distances, the timer's seconds,
        // NEXT TURN's last digit (1548 before it), the turn's two banners
        ("course page", [values, graph, map], crate::COURSE, None, false, (2_600, 20)),
        // 4563, 44: zoomed in at 1.5 m a pixel (map.rs MIN_LEVEL), following the rider: a whole map at each
        // re-centre (about every 26 s at 8 m/s)
        ("map zoomed 1.5 m", [map, values, graph], 0, Some(crate::map::MIN_LEVEL), false, (6_000, 60)),
        // 3470, 42: at 2 m a pixel, a re-centre about every 35 s
        ("map zoomed 2 m", [map, values, graph], 0, Some(2), false, (4_500, 60)),
    ];
    let mut over = std::vec::Vec::new();
    for (name, pages, screen, zoom, fast, (px_max, win_max)) in cases {
        let (px, win) = ride(pages, screen, zoom, fast);
        std::println!("{name}: {px} px/s, {win} windows/s (budget {px_max}, {win_max})");
        if px > px_max || win > win_max {
            over.push(std::format!("{name}: {px} px/s, {win} windows/s over {px_max}, {win_max}"));
        }
    }
    assert!(over.is_empty(), "{over:?}");
}

/// A menu's budgets in pixels (about 1 us each on the unit, SPI1 at 16 MHz): a move redraws the two rows whose
/// selection changed, a turn to the next screen of rows only the rows (the title stays), and opening a menu clears
/// only what its rows do not paint. Measured 2026-10-26: open 75 360 (138 720 when the page was cleared under the
/// rows: the profiler's 161 ms longest input on the unit), a move 21 120, a turn 63 360 (138 720 as a whole show).
#[test]
fn menu_budgets() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    let cost = |sim: &mut Sim, app: &mut App, p: Press| {
        let px = sim.pixels;
        app.press(sim, p);
        assert_consistent(sim, app);
        sim.pixels - px
    };
    let open = cost(&mut sim, &mut app, Press::Long(Key::Page));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Main, 0)));
    assert!(open <= 80_000, "open: {open} px");
    app.open(Screen::Menu(Menu::Sensors, 0));
    app.show(&mut sim);
    assert!(app.rows(Menu::Sensors) > ui::MENU_ROWS, "a menu that turns");
    for k in 1..=ui::MENU_ROWS {
        let px = cost(&mut sim, &mut app, Press::Short(Key::Page));
        let turn = k == ui::MENU_ROWS;
        std::println!("{} to row {k}: {px} px", if turn { "turn" } else { "move" });
        assert!(px <= if turn { 64_000 } else { 22_000 }, "row {k}: {px} px");
    }
    let back = cost(&mut sim, &mut app, Press::Short(Key::Lap));
    assert!(back <= 64_000, "turn back: {back} px");
}

/// The overview shown whole (Back to it, the page round): the page cleared but for the TODAY card, which paints its
/// own box. Measured 2026-10-26: 109 457 px (134 745 when the card's box was cleared twice: about 25 ms on the unit).
#[test]
fn overview_show_budget() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000); // the calendar read: the card as it stays
    app.press(&mut sim, Press::Long(Key::Page));
    let px = sim.pixels;
    back(&mut sim, &mut app);
    let px = sim.pixels - px;
    assert_eq!(app.screen(), Screen::Main);
    std::println!("overview shown: {px} px");
    assert!(px <= 112_000, "{px} px");
    assert_consistent(&sim, &app);
}

/// The overview left alone with no fix (indoors), as the profiler's idle window on the unit: 2013 px/s there
/// (2026-10-26) was a one-off and the GPS search, nothing steady. The TODAY card is drawn again once the calendar is
/// read (CALENDAR_MS, 8 s, after the start: 32 690 px, 36 034 with a card; inside a window that starts a few
/// seconds after a RAM run), then the strip's "GPS" blinks, three glyphs a second, 720 px, and its clock's minute.
/// 36 034 / 30 + 724 = 1925 px/s.
#[test]
fn idle_overview_searching() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    let px = sim.pixels;
    run(&mut sim, &mut app, 5000);
    let card = sim.pixels - px;
    assert!(card <= 40_000 + 5 * 720, "the TODAY card once: {card} px");
    let (px, win) = (sim.pixels, sim.windows);
    run(&mut sim, &mut app, 60_000);
    let (px, win) = ((sim.pixels - px) / 60, (sim.windows - win) / 60);
    std::println!("idle overview, no fix: {card} px once, then {px} px/s, {win} windows/s");
    assert!(px <= 760 && win <= 3, "{px} px/s, {win} windows/s: the GPS blink and the minute only");
    assert_consistent(&sim, &app);
}

/// The buses the CPU polls through on every pass, idle on the overview with the automatic backlight: the GNSS
/// receiver read every pass only while it sends (else every [`crate::GNSS_IDLE_PASSES`]), the light sensor one
/// transaction a reading (its data, not its status first). Measured 2026-10-26: 8.8 SPI4 transfers a second (23.0
/// when read every pass), 4.3 I2C4 transactions (8.3 with the status read).
#[test]
fn idle_bus_budgets() {
    for fix in [None, Some(8000)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.fix = fix;
        let mut app = App::start(&mut sim, None);
        app.settings.light_level = crate::LIGHT_AUTO;
        run(&mut sim, &mut app, 20_000);
        let (g, l, i2c) = (sim.gnss.got.len(), sim.bus.light_reads, sim.bus.transactions);
        run(&mut sim, &mut app, 60_000);
        let spi = (sim.gnss.got.len() - g) as f64 / 32.0 / 60.0;
        let (reads, i2c) = (f64::from(sim.bus.light_reads - l) / 60.0, f64::from(sim.bus.transactions - i2c) / 60.0);
        std::println!(
            "fix {fix:?}: {spi:.1} GNSS transfers/s, {reads:.1} light readings/s, {i2c:.1} I2C4 transactions/s"
        );
        assert!(spi <= 10.0, "{spi:.1} GNSS transfers/s");
        assert!((reads - 4.0).abs() < 0.1, "the light four times a second: {reads:.1}");
        assert!(i2c <= 4.5, "{i2c:.1} I2C4 transactions/s");
        assert_eq!(app.gps.pvt.is_some_and(|p| p.good()), fix.is_some(), "the fix read all the same");
    }
}
