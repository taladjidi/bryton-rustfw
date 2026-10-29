//! The ride's fields on the Sim: zone colours and the zone bar, live readings, laps and pauses, the power and
//! climb fields, the graph fields and the distance profile.

use super::map_tests::ride_route;
use super::menu_tests::main_row;
use super::menu_tests::row_of;
use super::menu_tests::to_row;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::{Field, Rec};

/// No zone colour is near the menu's orange (`ui::ACCENT`: selection, info banners) or the alarm red (`ui::RED`):
/// a zone must never look like a banner (UI review 2, P3).
#[test]
fn zone_colours_are_not_accent_or_alarm() {
    // the palette's brown and wine are close to the menu orange and the alarm reds: kept apart from the menu orange and the alarm
    // reds, though close (ride::ZONE_COLORS's doc)
    let rgb = |c: u16| [(c >> 11) as i32 * 255 / 31, (c >> 5 & 63) as i32 * 255 / 63, (c & 31) as i32 * 255 / 31];
    for (i, z) in crate::ride::ZONE_COLORS.iter().enumerate() {
        for (name, other) in [("ACCENT", ui::ACCENT), ("RED", ui::RED), ("the banner red", 0xf985)] {
            let d: i32 = rgb(*z).iter().zip(rgb(other)).map(|(a, b)| (a - b).abs()).sum();
            assert!(*z != other && d > 30, "zone {} is {z:04x}, near {name}: distance {d}", i + 1);
        }
    }
    let mut all = crate::ride::ZONE_COLORS.to_vec();
    all.dedup();
    assert_eq!(all.len(), 7, "seven distinct colours");
}

/// Heart rate steps over a zone's edge: 125 bpm (zone 2 of 190), 135 (zone 3) for 2 s only, 125 again, then 135 for
/// good from 20 s.
fn hr_edge(ms: u64) -> [u8; 8] {
    let s = ms / 1000;
    // 133 bpm is 1 bpm into zone 3 of 190 (70 % = 133): within the 1 % margin, so zone 2 stays; 136 is clearly in
    let bpm = if (14..16).contains(&s) {
        133
    } else if s >= 24 {
        136
    } else {
        125
    };
    [4, 0, 0, 0, 0, 0, 0, bpm]
}

/// The heart rate's zone bar follows its number at once, but a value on a zone's edge (within 1 % of max HR) does not
/// flip it, and the bar is all that is redrawn when it does change: the number, white, is redrawn on its own.
#[test]
fn the_zone_bar_follows_its_number_and_is_all_that_is_redrawn() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_edge, network: 0, ack: &[] });
    sim.gnss.fix = Some(8000); // a fix: the strip's "GPS" does not blink (720 px a second) under the count
    let mut app = App::start(&mut sim, Some(KEY));
    app.pages[0] = crate::page::Page::preset(
        1,
        [Field::Hr, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time, Field::Time],
    )
    .unwrap();
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0));
    let bx = app.pages()[0].cell_box(0).unwrap();
    let bar_at = |sim: &Sim| sim.screen[((bx.1 + 1) * W + bx.0 + 10) as usize];
    let (z2, z3) = (crate::ride::hr_color(2), crate::ride::hr_color(3));
    run(&mut sim, &mut app, 6000);
    let mut flips = std::vec::Vec::new();
    let mut last = bar_at(&sim);
    assert_eq!(last, z2);
    let t0 = sim.ms;
    for _ in 0..(30_000 / 250) {
        let px = sim.pixels;
        run(&mut sim, &mut app, 250);
        let now = bar_at(&sim);
        if now != last {
            flips.push((sim.ms - t0, now, sim.pixels - px));
            last = now;
        }
        assert_consistent(&sim, &app);
    }
    assert_eq!(flips.len(), 1, "one change, the 2 s of 133 bpm on the edge did not flip it: {flips:?}");
    let (at, colour, px) = flips[0];
    assert_eq!(colour, z3);
    // 136 bpm from 24 s on the sensor's clock, the run began about 6 s in: shown with the number, no hold
    assert!((16_000..20_000).contains(&at), "{at} ms");
    // the bar changes with its number now: the bar and the digits that changed, never the box whole
    let bar = u64::from(bx.2) * u64::from(ui::ZONE_BAR_H);
    assert!(px >= bar && px < u64::from(bx.2) * u64::from(bx.3) / 2, "the bar and the changed digits only: {px} px");
    sim.save("zone_bar");
}

/// The live readings on the pages (power, speed, cadence, heart rate) change once a second, in the second's tick,
/// though the sensors broadcast at 4 Hz and the value in hand changes at each broadcast (UI review 2).
#[test]
fn live_readings_show_at_one_hertz() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_fast, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_fast, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    let (mut changes, mut raw, mut seconds) = (0, 0, 0);
    let text = |app: &App| [Field::Power, Field::Hr].map(|f| std::string::String::from(app.field_text(f).as_str()));
    let (mut last, mut last_raw, mut last_sec) = (text(&app), app.live.get(Metric::Power), app.seconds);
    for _ in 0..80 {
        run(&mut sim, &mut app, 250);
        let (now, now_raw) = (text(&app), app.live.get(Metric::Power));
        raw += u32::from(now_raw != last_raw);
        if now != last {
            changes += 1;
            assert_ne!(app.seconds, last_sec, "a reading changed between two seconds: {last:?} to {now:?}");
        }
        seconds += u32::from(app.seconds != last_sec);
        (last, last_raw, last_sec) = (now, now_raw, app.seconds);
    }
    std::println!("20 s: {changes} changes shown, {raw} in the live value, {seconds} seconds");
    assert!(changes >= 10 && changes <= seconds, "shown: {changes} of {seconds} seconds");
    assert!(raw > changes + 15, "the sensors really change at each broadcast: {raw}");
    assert!(sim.nordic.sensors.len() == 2);
}

/// An empty list says so in a row, not in a banner that goes after 4 s: WORKOUTS with nothing planned.
#[test]
fn empty_workouts_say_so_in_a_row() {
    let (mut sim, mut app) = started();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0));
    assert_eq!(app.rows(Menu::Workouts), 2, "the row that says none, and the ramp test");
    let (label, _, color) = app.menu_row(Menu::Workouts, 0).unwrap();
    assert_eq!((label.as_str(), color), ("NONE PLANNED", ui::GREY));
    run(&mut sim, &mut app, 6000);
    assert!(app.notes.ms.is_none(), "no banner");
    assert_consistent(&sim, &app);
    sim.save("menu_workouts_empty");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0), "the row does nothing");
    app.press(&mut sim, Press::Short(Key::Back));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Main, _)));
}

/// The ride's card stays until Back or Start: another key, a swipe, a tap or a hold does not dismiss it (2026-10-25
/// review 2, P4: a raindrop threw the summary away).
#[test]
fn the_ride_card_stays_until_back_or_start() {
    use crate::touch::Dir;
    let ended = |sim: &mut Sim| {
        let mut app = App::start(sim, None);
        sim.emmc.image = crate::fat::tests::image();
        app.press(sim, Press::Short(Key::Start));
        run(sim, &mut app, 5000);
        app.press(sim, Press::Short(Key::Start));
        save_ride(sim, &mut app);
        run(sim, &mut app, 5000); // the banners go
        assert_eq!(app.screen(), Screen::Summary);
        app
    };
    let mut sim = Sim::new(9, 0, 0);
    let mut app = ended(&mut sim);
    for i in [
        Input::Next,
        Input::Prev,
        Input::Menu,
        Input::Media,
        Input::Touch(Gesture::Swipe(Dir::Left)),
        Input::Touch(Gesture::Swipe(Dir::Right)),
        Input::Touch(Gesture::Tap(120, 160)),
        Input::Touch(Gesture::Hold(120, 160)),
    ] {
        app.input(&mut sim, i);
        assert_eq!(app.screen(), Screen::Summary, "{i:?}");
    }
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "Back");
    let mut sim = Sim::new(9, 0, 0);
    let mut app = ended(&mut sim);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Main, ride::State::Idle), "Start: no new ride");
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
    sim.emmc.image = crate::fat::tests::image(); // a volume to record on (else the strip says not saving)
    sim.own_settings(); // the defaults, not stock's
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.bus.baro_alt = Some(climb); // the barometer climbs with the receiver
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_ramp, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Timer, Dist, AvgSpeed, AvgHr, AvgPower, Graph] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [Hr, Power, MaxSpeed, LapTime, LapDist, Ascent] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!((text(&app, Timer), text(&app, Dist), app.strip.rec), ("00:00".into(), "0.00".into(), Rec::Idle));
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
    // the averages of ramps settle as a long ride's do: from then on, mostly the timer, the distance, the graph (1250 px/s
    // before the UI review's step 3: the timer's seconds at the box's 4x, 560 px of ink instead of a 240 px 2x cell, and
    // the graph without its 1x axes in a half box, 112 columns instead of 85)
    let px_s = (sim.pixels - px0) / 120;
    std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), minutes 10 to 12: {px_s} px/s");
    assert!(px_s < 1800, "{px_s} px/s while riding");
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
    // the zones: a bar along the top of the heart rate and power boxes in their numbers' zones, the number white
    let (hr_bar, w_bar) = (crate::ride::hr_color(app.zones[4].shown()), crate::ride::power_color(app.zones[0].shown()));
    let (hr_box, w_box) = (colors_in(&sim, bx(0)), colors_in(&sim, bx(1)));
    assert!(hr_box.contains(&hr_bar) && w_box.contains(&w_bar), "zone bars");
    assert!(hr_box.contains(&crate::ui::WHITE) && w_box.contains(&crate::ui::WHITE), "white numbers");
    let zone_colors = |c: &[u16]| crate::ride::ZONE_COLORS.iter().filter(|z| c.contains(z)).count();
    assert!(zone_colors(&hr_box) == 1 && zone_colors(&w_box) == 1, "the bar is the only zone colour in the box");
    assert_eq!((text(&app, LapTime), text(&app, LapDist)), ("01:00".into(), "0.48".into()));
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
    save_ride(&mut sim, &mut app);
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
    sim.bus.baro_alt = Some(climb); // the barometer climbs with the receiver
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_lr, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [Power3s, Power10s, Power30s, Np, If, Tss] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [Kj, Kcal, Wkg, Wbal, MaxPower, MaxHr] }.into();
    app.pages[2] = PageCfg { layout: 6, fields: [AvgCadence, Balance, Vam, Grade, Timer, Time] }.into();
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
    assert_eq!(
        (app.field_color(Wbal), app.field_bar(Wbal)),
        (ui::WHITE, crate::ride::ZONE_COLORS[3]),
        "40 %: orange bar"
    );
    assert_eq!((app.field_color(Np), app.field_bar(Np)), (ui::WHITE, crate::ride::power_color(6)), "150 % of FTP");
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
    save_ride(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Back)); // the card, then the overview
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert!(app.power.wbal_j().unwrap() > 19_000 && app.power.kj() < 2, "a new ride");
}

/// Weight and W' from the menu, kept in RTC backup register 16 through a restart; the other settings stay in
/// register 11.
#[test]
fn weight_and_wprime_from_the_menu() {
    let (mut sim, mut app) = started();
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
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::WPrime))), "W' < 21.5 >");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(main_row(&app, row_of(menu::Row::Set(Setting::Weight))), "WEIGHT 68 KG");
    assert_consistent(&sim, &app);
    sim.save("power_settings");
    assert_eq!(sim.backup[16] >> 28, 0xa, "tagged");
    let again = App::start(&mut sim, None);
    assert_eq!((again.settings.weight_kg, again.settings.wprime_j, again.settings.ftp), (68, 21_500, 200));
}

/// Power as ridden: a two-minute swell from 140 to 320 W, +-30 W of noise from one second to the next.
pub(super) fn power_noisy(ms: u64) -> [u8; 8] {
    let s = ms / 1000;
    let swell = (s % 120).min(120 - s % 120) as u16;
    let noise = (s.wrapping_mul(2_654_435_761) >> 7) % 61;
    let [p0, p1] = (110 + 3 * swell + noise as u16).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}

/// Power and heart rate that change at every broadcast (4 Hz, the sensors' own rate): a power meter's and a strap's
/// real readings move all the time, the test sensors above only once a second.
pub(super) fn power_fast(ms: u64) -> [u8; 8] {
    let k = ms / 250;
    let swell = (k / 4 % 120).min(120 - k / 4 % 120) as u16;
    let noise = (k.wrapping_mul(2_654_435_761) >> 7) % 61;
    let [p0, p1] = (110 + 3 * swell + noise as u16).to_le_bytes();
    [0x10, 1, 0xff, 85, 0, 0, p0, p1]
}
pub(super) fn hr_fast(ms: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 120 + ((ms / 250).wrapping_mul(2_654_435_761) >> 9) as u8 % 9]
}

/// The graph field: power in its zones' colours, one pixel column per sample, drawn by differences as it
/// scrolls (the screenshot agrees). Prints the pixels a second it costs.
#[test]
fn graph_field() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 3, fields: [Graph, MaxSpeed, HrGraph, Timer, Timer, Timer] }.into();
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

/// A tap on a graph shows the next plot (power, heart rate, W' balance, DFA alpha1, elevation) until the unit is
/// switched off, the page as kept unchanged; a hold switches every graph between the last minutes and the whole
/// ride. Only while the ride is paused (or none runs): while it records, a tap or a hold on a page does nothing (UI
/// review step 7: a wet glove changed the page for good, or opened the menu over the ride).
#[test]
fn graph_plots_by_tap_and_span_by_hold() {
    let taps = [400_000u64, 401_000, 402_000, 403_000];
    let (hold, riding) = (405_000u64, [420_000u64, 422_000]);
    let mut sim = Sim::new(9, 0, 0).finger(move |ms| {
        let tap = taps.iter().chain(&riding[..1]).any(|&t| (t..t + 80).contains(&ms));
        let held = (hold..hold + 900).contains(&ms) || (riding[1]..riding[1] + 900).contains(&ms);
        (tap || held).then_some((120, 100))
    });
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_noisy, network: 0, ack: &[] });
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 2, fields: [Graph, HrGraph, Timer, Timer, Timer, Timer] }.into();
    save_page(&mut sim, 0, &app.pages[0]);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // a ride: W' balance runs
    sim.gnss.fix = Some(7000);
    let until = taps[0] - 2000 - sim.ms;
    run(&mut sim, &mut app, until);
    app.press(&mut sim, Press::Short(Key::Start)); // paused: the taps act
    let mut shots = std::vec::Vec::new();
    let shown = |app: &App| app.widgets().next().map(|w| w.1);
    for (k, want) in [HrGraph, WbalGraph, DfaGraph, AltGraph].into_iter().enumerate() {
        let until = taps[k] + 500 - sim.ms;
        run(&mut sim, &mut app, until);
        assert_eq!((shown(&app), app.pages()[0].fields[0]), (Some(Widget::Field(want)), Graph), "tap {k}");
        assert_consistent(&sim, &app);
        shots.push(std::format!("graph_{k}"));
        sim.save(&shots[k]);
    }
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]).unwrap().fields[0], Graph, "not kept");
    let until = hold + 1500 - sim.ms;
    run(&mut sim, &mut app, until);
    assert!(app.graph_whole, "the hold");
    assert_eq!(app.screen(), Screen::Ride(0), "on a graph, a hold is not the menu");
    assert_consistent(&sim, &app);
    sim.save("graph_whole");
    app.press(&mut sim, Press::Short(Key::Start)); // recording again: touch only turns pages
    let until = riding[1] + 1500 - sim.ms;
    run(&mut sim, &mut app, until);
    assert_eq!((shown(&app), app.graph_whole), (Some(Widget::Field(AltGraph)), true), "the tap and the hold ignored");
    assert_eq!(app.screen(), Screen::Ride(0), "no menu by a hold while recording");
    run(&mut sim, &mut app, 60_000);
    assert_consistent(&sim, &app);
}

/// The summary card's distance profile fills the whole width whatever the number of columns it has: more than the
/// width (128 to 256 columns went to half of it), or fewer (stretched).
#[test]
fn distance_profile_fills_the_width() {
    let cols = 232;
    for n in [40u32, 233, 256] {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        let o = crate::ride::ALT_OFFSET as u16;
        for k in 0..n {
            app.history.dist_second(k * 8, Some(o + k as u16));
        }
        assert_eq!(app.history.dist_len(), n);
        let g = app.graph_summary(cols);
        let empty: Vec<u32> = (0..cols).filter(|&i| app.graph_value(g, cols, i) == 0).collect();
        assert!(empty.is_empty(), "n = {n}: columns {empty:?} are empty");
    }
}

/// A tap on a 3 / 10 / 30 s power field, even while recording, cycles the window every such field shows (on any
/// page), keeps it through a restart, draws only those boxes, and a tap elsewhere does nothing new.
#[test]
fn a_tap_cycles_the_power_average() {
    use Field::*;
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, Some(KEY));
    app.pages[0] = PageCfg { layout: 6, fields: [Power3s, Hr, Speed, Dist, Timer, Time] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [Speed, Power30s, Hr, Dist, Timer, Time] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // recording: taps on fields do nothing else
    // 35 s at 100 W then 5 s at 400 W: 3 s 400, 10 s 250, 30 s 150
    for s in 0..40 {
        app.power.second(Some(if s < 35 { 100 } else { 400 }), &app.ride, 250, 20_000);
    }
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    let tap_field = |app: &mut App, sim: &mut Sim, f: Field| {
        let (x, y, w, h) = app.widgets().find(|&(_, wd)| wd == Widget::Field(f)).unwrap().0;
        app.input(sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16)));
    };
    let screen = app.screen();
    assert_eq!(text(&app, Power3s), "400");
    let before = sim.pixels;
    tap_field(&mut app, &mut sim, Power3s);
    assert_eq!((app.screen(), text(&app, Power3s)), (screen, "250".into()), "10 s");
    assert!(sim.pixels - before < 40_000, "only the power boxes are drawn");
    tap_field(&mut app, &mut sim, Power3s);
    assert_eq!(text(&app, Power3s), "150", "30 s");
    // the other page's field shows it too, and a restart keeps it
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(text(&app, Power30s), "150");
    tap_field(&mut app, &mut sim, Power30s);
    assert_eq!(text(&app, Power30s), "400", "3 s again");
    assert_eq!(app.avg_field(Power30s).label(), "3S W", "and its label");
    assert_eq!(text(&app, WktPower), "400", "the gauge follows");
    tap_field(&mut app, &mut sim, Power30s);
    let mut app = App::start(&mut sim, Some(KEY));
    assert_eq!(app.settings.wkt_avg, 1, "kept through a reset");
    assert_eq!(app.avg_field(Power3s).label(), "10S W");
    // the touch lock blocks the tap
    app.pages[0] = PageCfg { layout: 6, fields: [Power3s, Hr, Speed, Dist, Timer, Time] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    app.strip.locked = true;
    tap_field(&mut app, &mut sim, Power3s);
    assert_eq!(app.settings.wkt_avg, 1, "locked");
    app.strip.locked = false;
    // a MORE page (swipe up) has the field too
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert!(app.more_shown().is_some());
    tap_field(&mut app, &mut sim, Power3s);
    assert_eq!(app.settings.wkt_avg, 2, "MORE");
    app.input(&mut sim, Input::Back);
    // a tap on a field that is not a power average changes nothing
    let other = |wd: &Widget| matches!(wd, Widget::Field(f) if ![Power3s, Power10s, Power30s].contains(f));
    let (x, y, w, h) = app.widgets().find(|(_, wd)| other(wd)).unwrap().0;
    app.input(&mut sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16)));
    assert_eq!(app.settings.wkt_avg, 2);
}

/// Every profile x climbs-ahead x window survives the settings words (the window is in register 16's, the rest in 11's).
#[test]
fn settings_words_round_trip() {
    use crate::ride::Profile::*;
    for profile in [None, Some(Road), Some(Indoor), Some(Mtb)] {
        for ahead in [false, true] {
            for wkt_avg in 0..3 {
                let s = Settings { profile, ahead, wkt_avg, ..Settings::from_word(0) };
                let back = Settings::from_word(s.to_word()).with_power_word(s.power_word());
                assert_eq!((back.profile, back.ahead, back.wkt_avg), (profile, ahead, wkt_avg));
            }
        }
    }
    assert_eq!(Settings::from_word(0).with_power_word(0xa000_c000).wkt_avg, 0, "an old NOW is 3 s");
}

/// The trip, lap and slope fields on a ride page: a minute, a lap, a minute, over a climb that gains 0.2 m a second.
/// The trip time runs through the lap, the last lap is the first minute, and the climb is all uphill.
#[test]
fn trip_lap_and_slope_fields() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.bus.baro_alt = Some(climb); // the barometer climbs with the receiver
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_lr, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [TripTime, Laps, LastLap, LapAvgPower, LapNp, LastLapAvgPower] }.into();
    app.pages[1] = PageCfg { layout: 6, fields: [MaxAlt, Descent, LapAscent, UphillKm, DownhillKm, MaxCadence] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    assert_eq!(
        (text(&app, TripTime), text(&app, Laps), text(&app, LastLap)),
        ("00:00".into(), "0".into(), "--".into())
    );
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 60_000);
    app.press(&mut sim, Press::Short(Key::Lap));
    run(&mut sim, &mut app, 60_000);
    let secs = app.ride.elapsed_s();
    assert!((119..=121).contains(&secs), "trip {secs} s");
    assert_eq!(text(&app, Laps), "1");
    let last = app.ride.last_lap_s().unwrap_or(0);
    assert!((59..=61).contains(&last), "the last lap {last} s");
    assert_ne!(text(&app, LapAvgPower), "--", "power on the lap");
    assert_ne!(text(&app, LastLapAvgPower), "--", "the last lap's power");
    assert!(app.ride.uphill_m() > 300, "uphill {} m", app.ride.uphill_m());
    assert_eq!(app.ride.downhill_m(), 0, "no descent");
    assert_ne!(text(&app, MaxAlt), "--", "a highest altitude");
    sim.save("ride_trip_fields");
}

/// The heart rate and power fields against the rider's numbers, on a page: a strap ramping its heart rate and a
/// dual-sided meter at 300 W with the right pedal at 48 %, for two minutes at the default FTP (200 W) and max heart
/// rate (190). LTHR is unset until the menu sets it ("--"); the percents, zones and balance are worked out by hand here.
#[test]
fn heart_rate_and_power_fields_against_the_numbers() {
    use std::string::ToString;
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.gnss.alt = Some(climb);
    sim.bus.baro_alt = Some(climb);
    sim.gnss.fix = Some(8000);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_lr, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 6, fields: [PctMaxHr, PctLthr, HrZone, HrZoneTime, PctFtp, PowerZone] }.into();
    app.pages[1] =
        PageCfg { layout: 6, fields: [PctFtpAvg, PowerZoneTime, BalanceAvg, PctFtp10s, PctFtp30s, Power] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 120_000);
    let hr = u32::from(app.live.shown(Metric::Hr).unwrap_or(0));
    assert_eq!(text(&app, PctMaxHr), (hr * 100 / 190).to_string(), "a percent of the max heart rate");
    assert_eq!(text(&app, PctLthr), "--", "no LTHR set");
    app.settings.lthr = 150;
    assert_eq!(text(&app, PctLthr), (hr * 100 / 150).to_string(), "a percent of LTHR once it is set");
    assert_eq!(text(&app, PctFtp), "150", "3 s power 300 W at 200 W");
    assert_eq!(text(&app, PctFtpAvg), "150", "average power 300 W");
    assert_eq!(text(&app, PowerZone), "6", "150 % of the FTP is zone 6 (121 to 150 %)");
    assert_eq!(text(&app, BalanceAvg), "52/48", "the left 52 % and the right 48 %");
    assert_eq!(text(&app, Power), "300", "the big power value, live at 1 s");
    let secs = app.ride.timer_s();
    assert_eq!(app.zone_s.power_s(6), Some(secs), "every moving second in power zone 6");
    assert_ne!(text(&app, PowerZoneTime), "--", "the time in the zone of now");
    sim.save("ride_zone_fields");
}
