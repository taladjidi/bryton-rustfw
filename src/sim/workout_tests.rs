//! Workouts on the Sim: from the calendar, the page that shows one, the graph and gauge fields.

use super::menu_tests::to_row;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::Field;

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The synthetic card's planned workouts (tools/fat_image.py, today 2026-10-23): WORKOUTS lists today's and the
/// next days' (not the past one, not the broken one), Select shows the steps (repeats written out), Start
/// starts the ride and the workout. The step clock follows the ride timer: beep, beep, beeeep and a banner per step,
/// the target coloured from the 3 s power, a pause stops it, Lap skips a step (and ends the open cool-down),
/// the last step ends the workout.
#[test]
fn workout_from_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings(); // the defaults, not stock's
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    // the overview's field: today's planned time, the card read when the field is first shown
    app.pages[OVERVIEW].fields[3] = Workout;
    app.show(&mut sim);
    assert_eq!(text(&app, Workout), "0:01", "20 s + 2 x 45 s, the open cool-down not counted");
    app.pages[0] = PageCfg { layout: 6, fields: [StepTarget, StepTime, NextStep, Workout, Power3s, Timer] }.into();

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    let row = |app: &App, m, i| {
        app.menu_row(m, i).map(|(l, v, _)| format!("{} {}", l.as_str(), v.as_str())).unwrap_or_default()
    };
    assert_eq!(
        [0, 1, 2].map(|i| row(&app, Menu::Workouts, i)),
        ["FRI 23 OCT 0:01", "SUN 25 OCT 1:30", "RAMP TEST FTP 200"]
    );
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    sim.save("workout_list");

    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.plan.workout.name()), (Screen::Menu(Menu::Steps, 0), "SHORT INTERVALS"));
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
    assert_eq!(fields(&app), ["110|+-10", "0:20", "210|0:30", "1/6"]);
    assert_eq!(text(&app, WorkoutLeft), "6:50", "20 + 30 + 15 + 30 + 15 s and the open cool-down (graph width 5 min)");
    assert_eq!(text(&app, ErgTarget), "--", "no trainer");
    run(&mut sim, &mut app, 5000);
    assert_eq!(
        (app.field_color(StepTarget), app.field_bar(StepTarget)),
        (ui::WHITE, crate::ride::ZONE_COLORS[4]),
        "210 W over 100-120: white digits, the bar says over"
    );
    let (mut ms, start) = (0, sim.ms);
    let mut was_top = false;
    while app.plan.player.and_then(|p| p.step(app.plan.steps())) == Some(0) {
        was_top = app.plan.top.is_some();
        run(&mut sim, &mut app, 50);
        ms += 50;
        assert!(ms < 30_000, "the warm-up ends");
    }
    assert!((14_000..16_500).contains(&(sim.ms - start)), "20 s from the start: {} ms after 5 s", sim.ms - start);
    assert_eq!(
        sim.tones[tones..],
        [1800, 2600, 0, 2500, 0, 2000, 0, 2000, 0, 2500],
        "the ride's start, the step's start, beep at 2 and 1 s left, the next step"
    );
    assert!(was_top, "the strip was up just before the step changed");
    assert_ne!(app.notes.text.as_str(), "2/6 200-220W", "the next-step strip told it already: no banner");
    run(&mut sim, &mut app, 5000);
    assert_eq!(fields(&app), ["210|+-10", "0:25", "150|0:15", "2/6"]);
    assert_eq!(
        (app.field_color(StepTarget), app.field_bar(StepTarget)),
        (ui::WHITE, crate::ride::ZONE_COLORS[1]),
        "210 W in 200-220"
    );
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
    assert_eq!((text(&app, Workout), app.notes.text.as_str()), ("3/6".into(), "3/6 150W"));
    assert_eq!(
        (app.field_color(StepTarget), app.field_bar(StepTarget)),
        (ui::WHITE, crate::ride::ZONE_COLORS[4]),
        "210 W over 150 W"
    );

    // one workout at a time: while it runs, another one's steps do not open, its own do
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.notes.text.as_str()), (Screen::Menu(Menu::Workouts, 1), "WORKOUT RUNNING"));
    app.press(&mut sim, Press::Short(Key::Lap));
    app.press(&mut sim, Press::Short(Key::Start));
    // running: no START row (it would restart the workout, and Start is the ride's pause key), the step playing
    // selected
    assert_eq!((app.screen(), text(&app, Workout)), (Screen::Menu(Menu::Steps, 2), "3/6".into()));
    assert_eq!(row(&app, Menu::Steps, 0), "0:20 100-120W");
    assert_eq!(row(&app, Menu::Steps, 5), "LAP OPEN");
    assert_eq!(row(&app, Menu::Steps, 6), "");
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Back)); // WORKOUTS, ALL SETTINGS, the ride menu, out
    }
    assert_eq!(app.screen(), Screen::Ride(0));
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(fields(&app), ["OPEN", "LAP", "END", "6/6"]);
    assert_eq!(app.notes.text.as_str(), "6/6 OPEN LAP");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((text(&app, Workout), app.notes.text.as_str()), ("DONE".into(), "WORKOUT DONE"));
    assert_eq!(fields(&app)[..3], ["--", "--", "--"]);
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
    assert_eq!(app.ride.state(), ride::State::Running, "the ride goes on");
}

/// WORKOUTS > START opens the first page that holds a workout field, page 1 when none does.
#[test]
fn a_workout_opens_the_page_that_shows_it() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    let mut app = App::start(&mut sim, Some(KEY));
    app.page_count = 3;
    let start = |sim: &mut Sim, app: &mut App| {
        app.press(sim, Press::Long(Key::Page));
        to_row(sim, app, menu::Row::Open(Menu::Workouts));
        app.press(sim, Press::Short(Key::Start));
        app.press(sim, Press::Short(Key::Start)); // the first workout's steps
        app.press(sim, Press::Short(Key::Start)); // START
    };
    assert!(!app.pages[0].fields.iter().any(|f| f.is_workout()), "the default page has no workout field");
    start(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Ride(0), "no page shows it: page 1");
    assert!(app.plan.player.is_some());
    // a new ride and workout, with the step on page 3
    let (mut sim, mut app) = {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        let mut app = App::start(&mut sim, Some(KEY));
        app.page_count = 3;
        (sim, app)
    };
    app.pages[2] = PageCfg {
        layout: 6,
        fields: [Field::StepTarget, Field::StepTime, Field::NextStep, Field::Workout, Field::Power3s, Field::Timer],
    }
    .into();
    start(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Ride(2), "the page with the step");
    run(&mut sim, &mut app, 1000);
    assert_consistent(&sim, &app);
}

/// The workout graph field: the synthetic card's SHORT INTERVALS over the whole box, each step as high as its target
/// in its zone colour; once started, the rider's column moves right with the step clock, the part done dark, and
/// only the columns it leaves and reaches are sent (not the whole graph each second).
#[test]
fn workout_graph_field() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, Some(KEY));
    let fields = [Field::WorkoutGraph, Field::StepTarget, Field::StepTime, Field::Time, Field::Time, Field::Time];
    app.pages[0] = PageCfg { layout: 9, fields }.into(); // with the step's target and time left
    let rider = |app: &App| match app.drawn.first() {
        Some(Drawn::Workout(w)) => w.rider,
        _ => panic!("the graph's box first"),
    };
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start)); // today's: SHORT INTERVALS
    app.press(&mut sim, Press::Short(Key::Start)); // START: the ride and the workout
    assert_eq!(app.screen(), Screen::Ride(0));
    run(&mut sim, &mut app, 1000);
    let first = rider(&app).expect("the rider's column");
    assert_consistent(&sim, &app);

    run(&mut sim, &mut app, 25_000);
    let now = rider(&app).unwrap();
    assert!(now > first + 5, "the marker moves: column {first}, then {now}");
    assert_consistent(&sim, &app);
    sim.save("workout_graph");

    app.pages[0] = PageCfg { layout: 1, fields }.into(); // the graph alone: what it sends
    app.show(&mut sim);

    // Lap to the open cool-down (its 300 s, the last 73 % of the graph), the step's banner gone: the cost alone
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 10_000); // the step's banner, then the one that waited for it
    let (at, px0, s0) = (rider(&app).unwrap(), sim.pixels, sim.ms);
    assert!(at > now + 30, "Lap jumps it: column {now}, then {at}");
    run(&mut sim, &mut app, 30_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - s0);
    let end = rider(&app).unwrap();
    assert_consistent(&sim, &app);
    std::println!("workout graph: rider column {at} to {end} in 30 s, {px_s} panel px/s");
    // the strip and the rest send about 730 px/s with a still field in this box (measured); the graph adds two
    // columns of its height a move, about 0.6 moves a second (232 columns over the 410 s drawn): about 300 px/s,
    // against 60 000 for the whole graph
    assert!(px_s < 1200, "{px_s} px/s: only the columns that change");
}

/// The power the scripted meter sends ([`scripted_power`]); 0, 3, 6 or 9 W are added from one second to the next so
/// the number changes each second as a real meter's does.
static SCRIPTED_W: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
fn scripted_power(ms: u64) -> [u8; 8] {
    let w = SCRIPTED_W.load(core::sync::atomic::Ordering::Relaxed) + (ms / 1000 % 4) as u16 * 3;
    let [lo, hi] = w.to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, lo, hi]
}

/// The workout gauge field (WKT POWER) through the synthetic card's SHORT INTERVALS: the 3 s power big, the step's
/// band under a bar with a marker at the power, coloured by how far it is (red over the warm-up's 100-120 W, blue
/// under the 200-220 W interval, then green in it); the menu's window in its title; only the number's characters and
/// the marker's columns sent each second, and what is sent equals the gauge drawn whole.
#[test]
fn workout_gauge_field() {
    use core::sync::atomic::Ordering::Relaxed;
    let z = crate::ride::ZONE_COLORS;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings(); // the defaults, not stock's
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: scripted_power, network: 0, ack: &[] });
    SCRIPTED_W.store(210, Relaxed);
    let mut app = App::start(&mut sim, Some(KEY));
    let fields = [Field::WktPower, Field::StepTime, Field::NextStep, Field::Time, Field::Time, Field::Time];
    app.pages[0] = PageCfg { layout: 3, fields }.into(); // the gauge over the step's time left and the next step
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start)); // today's: SHORT INTERVALS
    app.press(&mut sim, Press::Short(Key::Start)); // START: the ride and the workout
    let shown = |app: &App| match app.drawn.first() {
        Some(Drawn::Gauge(g)) => (std::string::String::from(g.value.as_str()), g.color),
        _ => panic!("the gauge's box first"),
    };
    run(&mut sim, &mut app, 6000);
    assert_eq!(shown(&app).1, z[4], "about 214 W over the warm-up's 100-120 W: red");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge_over");

    let mut ms = 0;
    while app.plan.player.and_then(|p| p.step(app.plan.steps())) == Some(0) {
        run(&mut sim, &mut app, 100);
        ms += 100;
        assert!(ms < 30_000, "the warm-up ends");
    }
    SCRIPTED_W.store(150, Relaxed);
    run(&mut sim, &mut app, 5000);
    assert_eq!(shown(&app).1, z[0], "about 154 W under 200-220 W: blue");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge_under");

    SCRIPTED_W.store(206, Relaxed);
    run(&mut sim, &mut app, 5000);
    let (w, color) = shown(&app);
    assert_eq!(color, z[1], "{w} W in 200-220 W: green");
    assert_consistent(&sim, &app);
    sim.save("workout_gauge");

    // what a second costs: the strip, the step's time left, the number's changed characters, the marker
    let (px0, s0) = (sim.pixels, sim.ms);
    run(&mut sim, &mut app, 10_000);
    let px_s = (sim.pixels - px0) * 1000 / (sim.ms - s0);
    assert_consistent(&sim, &app);
    std::println!("workout gauge: {px_s} panel px/s, the number and the marker moving each second");
    // measured 6468 px/s: a digit of the number at scale 7 is 42 x 70 px and one or two change a second, the marker
    // two 3-column strips of 18 px, the strip and the time left the rest; the gauge's box whole would be 35 520 px
    assert!(px_s < 9000, "{px_s} px/s: only what changes");

    // the menu's window: 30 s, in the title
    app.settings.wkt_avg = 2;
    run(&mut sim, &mut app, 1000);
    let want = app.power.rolling(2).map(u32::from).unwrap_or_default();
    assert_eq!(app.field_text(Field::WktPower).as_str(), std::format!("{want}"));
    assert_consistent(&sim, &app);
}

/// The gauge's bar, 3 times as thick, with the rider's power zones behind the target band (white outline) and the
/// reading as a white tick: full width and half width (the pacer's cell), a target in zone 3 and in zone 5 of an FTP
/// of 200 W, the reading under it and over it. The zones follow the FTP; the marker's move repaints the zones under
/// the old tick (what is sent equals the gauge drawn whole). Screenshots: `gauge_zones_full`, `gauge_zones_half`.
#[test]
fn gauge_shows_the_zones() {
    use crate::app::GaugeShown;
    use crate::storage::workout_file::Kind;
    let (mut sim, mut app) = started();
    app.settings.ftp = 200;
    let shown = |band: (u16, u16), w: u16, bx: (u32, u32, u32, u32), pace: Option<u16>| {
        let mut value = crate::text::Line::new();
        value.push_u32(u32::from(w));
        let color = crate::workout::gauge_color(w, band);
        let scale = crate::workout::gauge_scale(Kind::Power, band, 200, 190);
        let marker = Some(crate::workout::gauge_x(w, scale, ui::gauge_cols(bx)));
        GaugeShown { kind: Kind::Power, avg: 0, band: Some(band), step: true, second: None, pace, value, color, marker }
    };
    let cases = [((160, 180), 140), ((160, 180), 210), ((215, 235), 190), ((215, 235), 260)];
    // full width: two gauges on top of each other, then the half-width ones side by side
    let mut p = Panel(&mut sim, (0, ui::H), ui::Theme::DARK);
    p.fill(0, 0, ui::W, ui::H, ui::BLACK);
    let full = [(0, 24, 240, 118), (0, 160, 240, 118)];
    let half = [(0, 24, 120, 100), (120, 24, 120, 100), (0, 160, 120, 100), (120, 160, 120, 100)];
    for (bx, (band, w)) in full.into_iter().zip([cases[0], cases[2]]) {
        app.draw_gauge(&mut p, bx, shown(band, w, bx, None));
    }
    sim.save("gauge_zones_full_a");
    let mut p = Panel(&mut sim, (0, ui::H), ui::Theme::DARK);
    p.fill(0, 0, ui::W, ui::H, ui::BLACK);
    for (bx, (band, w)) in full.into_iter().zip([cases[1], cases[3]]) {
        app.draw_gauge(&mut p, bx, shown(band, w, bx, None));
    }
    sim.save("gauge_zones_full_b");
    let mut p = Panel(&mut sim, (0, ui::H), ui::Theme::DARK);
    p.fill(0, 0, ui::W, ui::H, ui::BLACK);
    for (bx, (band, w)) in half.into_iter().zip(cases) {
        app.draw_gauge(&mut p, bx, shown(band, w, bx, Some(754)));
    }
    sim.save("gauge_zones_half");

    // every zone on the gauge, and they follow the FTP: 130 W is zone 2 of 200 W, under the scale (zone 1) at 400 W
    let bx = full[0];
    let z = crate::ride::power_color;
    let screen_of = |sim: &mut Sim, app: &App| {
        Panel(&mut *sim, (0, ui::H), ui::Theme::DARK).fill(0, 0, ui::W, ui::H, ui::BLACK);
        app.draw_gauge(&mut Panel(&mut *sim, (0, ui::H), ui::Theme::DARK), bx, shown((160, 180), 140, bx, None));
        sim.screen.clone()
    };
    let column = |screen: &[u16], ftp: u16| {
        let scale = crate::workout::gauge_scale(Kind::Power, (160, 180), ftp, 190);
        let x = 8 + crate::workout::gauge_x(130, scale, ui::gauge_cols(bx));
        (0..bx.3).map(|y| screen[((bx.1 + y) * ui::W + x) as usize]).collect::<Vec<_>>()
    };
    let s = screen_of(&mut sim, &app);
    assert!((1..=7).all(|k| s.contains(&z(k))), "FTP 200: all seven zones on the gauge");
    assert!(column(&s, 200).contains(&z(2)), "FTP 200: 130 W in zone 2");
    app.settings.ftp = 400;
    let s = screen_of(&mut sim, &app);
    assert!(column(&s, 400).contains(&z(1)) && !column(&s, 400).contains(&z(2)), "FTP 400: zone 1");
}

/// The next-step strip (patch P3): in the status strip's place for the last 9 s of a timed step, the next step's
/// target and length at the left and the countdown at the right; across a step change, with the last step ("LAST
/// STEP"), and not for an open step. At a step change the strip replaces the banner (the beeps stay); a Lap skip,
/// which the strip did not announce, still says the step. A second changes one cell of the countdown.
#[test]
fn next_step_strip() {
    use crate::storage::workout_file::{Kind as TKind, Length, Step, Target, Workout};
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, None);
    let pw = |lo, hi| Target { kind: TKind::Power, lo, hi, pct: false, zone: 0 };
    let st = |len, target| Step { len, target, target2: Target::OPEN, intensity: 0 };
    app.plan.workout = Workout::built(
        "STRIP",
        &[st(Length::Time(15), pw(200, 200)), st(Length::Time(12), pw(150, 170)), st(Length::Time(10), pw(100, 120))],
    );
    app.start_workout(0);
    app.show(&mut sim);
    assert_eq!(app.screen(), Screen::Ride(0));
    let mut seen: Vec<(usize, u32, std::string::String)> = Vec::new();
    for _ in 0..400 {
        run(&mut sim, &mut app, 100);
        assert_consistent(&sim, &app);
        if let Some(a) = app.ahead() {
            let row = (a.next, a.left, std::string::String::from(app.ahead_head(&a).as_str()));
            if seen.last() != Some(&row) {
                seen.push(row);
            }
        }
    }
    let head = |next, left, h: &str| (next, left, std::string::String::from(h));
    let mut want: Vec<_> = (1..=9).rev().map(|s| head(1, s, "NEXT 160W 0:12")).collect();
    want.extend((1..=9).rev().map(|s| head(2, s, "NEXT 110W 0:10")));
    want.extend((1..=9).rev().map(|s| head(3, s, "LAST STEP")));
    assert_eq!(seen, want, "a strip per step, counting 9 to 1, none after the last step");
    run(&mut sim, &mut app, 10_000); // the DONE banner goes
    assert_consistent(&sim, &app);
    assert!(app.plan.top.is_none() && !app.plan.running(), "the status strip is back once the workout is done");

    // the strip is on screen while it counts: screenshot, and what a second costs
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app2 = App::start(&mut sim, None);
    app2.plan.workout =
        Workout::built("STRIP", &[st(Length::Time(15), pw(200, 200)), st(Length::Time(60), pw(250, 270))]);
    app2.start_workout(0);
    app2.show(&mut sim);
    while app2.ahead().is_none_or(|a| a.left != 6) {
        run(&mut sim, &mut app2, 100);
    }
    run(&mut sim, &mut app2, 200);
    assert_consistent(&sim, &app2);
    sim.save("workout_next_strip");
    let (look, a) = (app2.look(), app2.ahead().unwrap());
    let head = app2.ahead_head(&a);
    let (n, o) = (a.count(), {
        let mut b = a;
        b.left += 1;
        b.count()
    });
    let px0 = sim.pixels;
    ui::draw_next_strip(&mut Panel(&mut sim, (0, ui::H), look), head.as_str(), n.as_str(), Some(o.as_str()));
    let second = sim.pixels - px0;
    let px0 = sim.pixels;
    ui::draw_next_strip(&mut Panel(&mut sim, (0, ui::H), look), head.as_str(), n.as_str(), None);
    std::println!("next-step strip: {second} px for a second, {} px whole", sim.pixels - px0);
    assert!(second <= 2 * 24 * 14, "{second} px: one cell of the countdown");
}

fn power150(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 150, 0]
}

/// A step with two targets (FIT secondary target, fields 19 to 22): 200-250 W and 90-100 RPM. The gauge shows the
/// secondary after the primary under its bar; its marker says the primary's state, and the secondary's only once
/// the primary is in range (85 rpm is under 90: not green; at 150 W the primary's blue stays). Screenshot:
/// `workout_two_targets`.
#[test]
fn two_targets_shown() {
    use crate::storage::workout_file::{Kind as TKind, parse, tests::two_targets};
    let z = crate::ride::ZONE_COLORS;
    for (data, want) in [(power210 as fn(u64) -> [u8; 8], z[2]), (power150, z[0])] {
        let mut sim = Sim::new(9, 0, 0);
        sim.own_settings();
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data, network: 0, ack: &[] });
        let mut app = App::start(&mut sim, Some(KEY));
        let fields = [Field::WktPower, Field::StepTarget, Field::NextStep, Field::Time, Field::Time, Field::Time];
        app.pages[0] = PageCfg { layout: 3, fields }.into();
        app.plan.workout = parse(&two_targets()).unwrap();
        app.start_workout(0);
        app.show(&mut sim);
        run(&mut sim, &mut app, 8000);
        let g = match app.drawn.first() {
            Some(Drawn::Gauge(g)) => *g,
            _ => panic!("the gauge's box first"),
        };
        assert_eq!(g.second, Some((TKind::Cadence, (90, 100))));
        assert_eq!(g.band, Some((200, 250)));
        assert_eq!(g.color, want, "the primary's state, or the secondary's with the primary in range");
        assert_consistent(&sim, &app);
        if want == z[2] {
            sim.save("workout_two_targets");
        }
    }
}

/// The strip's left part keeps every number: "NEXT " goes first, then the unit; a 150 bpm 10:00 step, a 1 h step, a
/// 12.5 km step. The status strip is whole again once the strip goes.
#[test]
fn next_step_strip_text_and_return() {
    use crate::storage::workout_file::{Kind as TKind, Length, Step, Target, Workout};
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, None);
    let t = |kind, lo, len| Step {
        len,
        target: Target { kind, lo, hi: lo, pct: false, zone: 0 },
        target2: Target::OPEN,
        intensity: 0,
    };
    app.plan.workout = Workout::built(
        "T",
        &[
            t(TKind::Hr, 150, Length::Time(600)),
            t(TKind::Power, 275, Length::Time(3600)),
            t(TKind::Hr, 150, Length::Distance(12_500)),
            t(TKind::Power, 275, Length::Time(600)),
        ],
    );
    let head = |app: &App, next| {
        let a = crate::app::Ahead { next, left: 5, metres: false };
        std::string::String::from(app.ahead_head(&a).as_str())
    };
    assert_eq!(head(&app, 0), "150BPM 10:00", "NEXT dropped, no number cut");
    assert_eq!(head(&app, 1), "275W 1:00:00");
    assert_eq!(head(&app, 2), "150BPM 12.50K");
    assert_eq!(head(&app, 3), "NEXT 275W 10:00");
}
