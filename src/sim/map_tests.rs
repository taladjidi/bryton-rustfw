//! The map field on the Sim: the basemap and IGN tiles, the last position, following a ride, the zoom buttons and
//! the pan.

use super::course_tests::course_route;
use super::course_tests::hill;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::Field;

/// A synthetic ride at 8 m/s (28.8 km/h): 240 s east with a gentle wiggle, then a loop of 600 m radius
/// counterclockwise, starting heading east. Latitude, longitude (1e-7 degrees) and heading (1e-5 degrees).
pub(super) fn ride_route(ms: u64) -> (i32, i32, i32) {
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
pub(super) fn tiles_route(ms: u64) -> (i32, i32, i32) {
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
    sim.classic_pages();
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

/// IGN map packs on the volume (the synthetic ones of test-data/ign, through the eMMC driver): the map page draws
/// them under the track, with "(c) IGN" in the map's bottom right corner; between view changes the eMMC is not read.
#[test]
fn ign_map_under_the_map() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::tiles::tests::image_with_packs();
    let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
    [sim.backup[17], sim.backup[18]] = aid.to_words();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 60_000);
    let stats = app.basemap_stats();
    std::println!("IGN map: {stats:?}");
    assert!(stats.ign, "{stats:?}");
    sim.save("ign_map");
    assert_consistent(&sim, &app);
    // the attribution: the map's bottom right corner (the page's last 28 x 11 pixels) black and white
    let screen = &sim.screen;
    let corner = (H - 11..H).flat_map(|y| (W - 28..W).map(move |x| screen[(y * W + x) as usize]));
    let (black, white) =
        corner.fold((0, 0), |(b, w), p| (b + usize::from(p == ui::BLACK), w + usize::from(p == ui::WHITE)));
    assert!(black > 20 && white > 100, "(c) IGN: {black} black, {white} white");

    // a ride's card (the one just ended; a past ride's goes the same way) has the map under its track too
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.screen(), Screen::Summary);
    assert!(app.basemap_stats().ign, "the card's basemap: {:?}", app.basemap_stats());
    sim.save("card_ign");
    assert_consistent(&sim, &app);
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
    sim.classic_pages();
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
    sim.classic_pages();
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
    assert!(late_refits > 0, "the loop closed: the view still follows the rider round it");
    assert!(small_px / s < 2000, "{} px/s between full redraws", small_px / s);
    let v = app.map_view(0).unwrap();
    let (x, y) = v.px(area, app.track.pos().unwrap());
    assert_eq!(sim.screen[(y as u32 * W + x as u32) as usize], crate::ui::WHITE, "the arrow at the position");
    let track = sim.screen.iter().filter(|&&p| p == crate::ui::ACCENT).count();
    assert!(track > 500, "the track drawn: {track} px");
    // the map as one field of three, over speed and heart rate
    app.pages[0] =
        PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] }
            .into();
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "from the map, round to the first page");
    run(&mut sim, &mut app, 10_000);
    assert_consistent(&sim, &app);
    sim.save("map_field");
}

/// The map's "+" and "-" (map.rs, manual zoom) on the full-page map over the basemap: each tap one level, the view
/// following the rider across re-centres (the arrow always on the map), the panel equal to the screenshot at each
/// step, back to the fit at its level; a hold on the map still opens the menu; no buttons in a map of half a page.
#[test]
fn map_zoom_buttons() {
    use crate::map::{self, MIN_LEVEL, mpp16};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(2), "the map page");
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000); // out and onto the loop: a fit coarser than a few levels
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, minus] = map::buttons(area).unwrap();
    let tap = |sim: &mut Sim, app: &mut App, b: map::Rect| {
        app.input(sim, Input::Touch(Gesture::Tap((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16)));
        assert_consistent(sim, app);
    };
    let fit = app.map_view(0).unwrap();
    assert_eq!(app.map_zoom, None);
    sim.save("map_zoom_fit");
    tap(&mut sim, &mut app, plus);
    let first = app.map_zoom.expect("zoomed in");
    assert_eq!(mpp16(first + 1), fit.mpp16, "one level finer than the fit");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(first));
    for _ in 0..20 {
        tap(&mut sim, &mut app, plus);
    }
    assert_eq!(app.map_zoom, Some(MIN_LEVEL), "no finer than 1.5 m a pixel");
    assert_eq!(app.map_view(0).unwrap().mpp16, 24);
    sim.save("map_zoom_in");
    // riding on, zoomed in: the view follows, centred again only when the arrow leaves the middle part
    let (t0, px0) = (sim.ms, sim.pixels);
    let mut recentres = 0;
    while sim.ms < t0 + 120_000 {
        let view = app.map_view(0);
        run(&mut sim, &mut app, u64::from(POLL_MS));
        let v = app.map_view(0).unwrap();
        recentres += u32::from(Some(v) != view);
        assert_eq!(v.mpp16, 24, "the zoom kept");
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert!(x >= area.2 / 6 && x < area.2 * 5 / 6, "the arrow on the map: {x}");
        assert!(y >= area.1 + area.3 / 6 && y < area.1 + area.3 * 5 / 6, "the arrow on the map: {y}");
        if (sim.ms - t0) % 30_000 < u64::from(POLL_MS) {
            assert_consistent(&sim, &app);
        }
    }
    let s = (sim.ms - t0) / 1000;
    std::println!("map zoomed in at 1.5 m a pixel: {recentres} re-centres in {s} s, {} px/s", (sim.pixels - px0) / s);
    assert!((2..=12).contains(&recentres), "followed, not redrawn every second: {recentres}");
    sim.save("map_zoom_follow");
    // out again, a level at a time, past the fit: still by hand, coarser than the fit
    let fit_level = (MIN_LEVEL..map::MAX_LEVEL).find(|&l| mpp16(l) == fit.mpp16).unwrap();
    for _ in MIN_LEVEL..fit_level + 2 {
        tap(&mut sim, &mut app, minus);
    }
    assert_eq!(app.map_zoom, Some(fit_level + 2), "coarser than the fit, by hand");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(fit_level + 2));
    // the crosshair: back to the fit
    let centre = (minus.0, minus.1 + minus.3 + 4, minus.2, minus.3);
    sim.save("map_zoom_out");
    tap(&mut sim, &mut app, centre);
    assert_eq!(app.map_zoom, None, "back to the fit");
    let shown = map::Shown::of(&app.track, area, None, None, true);
    assert_eq!(app.map_view(0), shown.view, "the default view");
    // a hold on the map opens the page editor (no ride recording), even on a button
    app.input(&mut sim, Input::Touch(Gesture::Hold((plus.0 + 4) as u16, (plus.1 + 4) as u16)));
    assert_eq!(app.screen(), Screen::Edit(2, 0));
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Ride(2));
    // half a page: no buttons, a tap there does not zoom
    app.pages[2] =
        PageCfg { layout: 2, fields: [Field::Map, Field::Speed, Field::Time, Field::Time, Field::Time, Field::Time] }
            .into();
    app.show(&mut sim);
    let half = map::area(ui::field_box(2, ui::RIDE_H, 0).unwrap());
    assert_eq!(map::buttons(half), None);
    app.map_zoom = Some(MIN_LEVEL + 1); // as if zoomed in on the full page: "+" or "-" there would change it
    for b in [plus, minus] {
        tap(&mut sim, &mut app, b);
        assert_eq!(app.map_zoom, Some(MIN_LEVEL + 1), "no buttons, no zoom");
    }
    sim.save("map_zoom_half");
}

/// With a course followed, the crosshair toggles between the default view and the whole route (the fit), and back.
#[test]
fn map_crosshair_toggles_the_route() {
    use crate::map::{self, View};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(course_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.settings.light_level = 0;
    app.set_course_memory(crate::storage::course_file::tests::memory());
    app.open_courses(&mut sim);
    app.follow_course(hill(&app));
    for _ in 0..100 {
        app.step(&mut sim);
    }
    sim.gnss.fix = Some(500);
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 20_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [_, minus] = map::buttons(area).unwrap();
    let centre = (minus.0 + minus.2 / 2, minus.1 + minus.3 + 8 + minus.3 / 2);
    let tap = |sim: &mut Sim, app: &mut App| {
        app.input(sim, Input::Touch(Gesture::Tap(centre.0 as u16, centre.1 as u16)));
        assert_consistent(sim, app);
    };
    let default = map::default_mpp16(area);
    assert_eq!(app.map_view(0).unwrap().mpp16, default, "the default view first");
    tap(&mut sim, &mut app);
    let whole = View::fit(&app.track, area, app.route().as_ref()).unwrap();
    assert_eq!(app.map_view(0), Some(whole), "the whole route");
    assert_ne!(whole.mpp16, default);
    assert_eq!(app.map_zoom, None, "not a zoom by hand");
    tap(&mut sim, &mut app);
    assert_eq!(app.map_view(0).unwrap().mpp16, default, "the default view again");
}

/// No position, no view (the fit of nothing: "NO FIX"); with one, the default view is about 1 km across, and "+" and
/// "-" work from it.
#[test]
fn map_default_view_without_a_fix() {
    use crate::map::{self, mpp16};
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.map_view(0), None, "no fix: no view");
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 30_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = map::buttons(area).unwrap();
    let v = app.map_view(0).unwrap();
    assert_eq!(v.mpp16, map::default_mpp16(area), "the default level");
    let span = v.mpp16 * i64::from(area.2) / 16;
    assert!((700..=1400).contains(&span), "the default view {span} m across");
    app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    assert_consistent(&sim, &app);
    let level = app.map_zoom.expect("zoomed in by hand");
    assert!(mpp16(level) < v.mpp16, "\"+\" goes finer than the default");
    assert_eq!(app.map_view(0).unwrap().mpp16, mpp16(level));
}

/// A zoom set in one ride is not the next ride's (review: unused while the new fit was finer, it came back by
/// itself as the fit grew); a course followed or dropped fits the map again too.
#[test]
fn map_zoom_ends_with_the_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.route = Some(ride_route);
    sim.emmc.image = crate::fat::tests::image();
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    sim.gnss.fix = Some(500);
    app.press(&mut sim, Press::Short(Key::Start));
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 200_000);
    let area = crate::map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = crate::map::buttons(area).unwrap();
    app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    app.input(&mut sim, Input::Touch(Gesture::Drag(100, 200, 30, 0)));
    assert!(app.map_zoom.is_some() && app.map_centre.is_some(), "zoomed and panned");
    // the ride ended, a new one started
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    back(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.map_zoom, app.map_centre), (None, None), "the new ride's map fitted");
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 200_000);
    assert!(matches!(app.drawn[0], crate::Drawn::Map(m) if m.view.is_some() && !m.zoomed()), "still fitted");
    assert_consistent(&sim, &app);
    // zoomed again, then a course followed: fitted to it
    app.input(&mut sim, Input::Touch(Gesture::Tap((plus.0 + 20) as u16, (plus.1 + 20) as u16)));
    assert!(app.map_zoom.is_some());
    app.set_course_memory(crate::storage::course_file::tests::memory());
    app.open_courses(&mut sim);
    app.follow_course(hill(&app));
    assert_eq!((app.map_zoom, app.map_centre), (None, None), "a course: the map fitted");
}

/// A course followed, the map zoomed in: the course drawn under the track as at the fit, the view following the
/// rider along it.
#[test]
fn map_zoom_with_a_course() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(course_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    // steps without simulated time pass below: a fixed level, so the light sensor is not read before its conversion
    app.settings.light_level = 0;
    app.set_course_memory(crate::storage::course_file::tests::memory());
    app.open_courses(&mut sim);
    app.follow_course(hill(&app));
    for _ in 0..100 {
        app.step(&mut sim); // read over a few passes
    }
    sim.gnss.fix = Some(500);
    app.screen = Screen::Ride(2);
    app.show(&mut sim);
    run(&mut sim, &mut app, 20_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [_, minus] = map::buttons(area).unwrap();
    let v0 = app.map_view(0).unwrap();
    let span = v0.mpp16 * i64::from(area.2) / 16;
    assert!((700..=1400).contains(&span), "the default view {span} m across, not the route");
    // the default view is about 1 km across: five levels out to show the course
    for _ in 0..5 {
        app.input(&mut sim, Input::Touch(Gesture::Tap((minus.0 + 20) as u16, (minus.1 + 20) as u16)));
    }
    let level = app.map_zoom.expect("zoomed in");
    let mut views = 0;
    for _ in 0..12 {
        let view = app.map_view(0);
        run(&mut sim, &mut app, 60_000);
        let v = app.map_view(0).unwrap();
        views += u32::from(Some(v) != view);
        assert_eq!(v.mpp16, map::mpp16(level));
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert!(x >= area.0 && x < area.0 + area.2 && y >= area.1 && y < area.1 + area.3, "the arrow on the map");
        let course = sim.screen.iter().filter(|&&p| p == map::COURSE).count();
        // presence only: the count moved from 83 to 281 px between runs of this test (the track covers part of it)
        assert!(course > 0, "the course drawn in view: {course} px");
        assert_consistent(&sim, &app);
    }
    assert!(views > 0, "the view follows the rider");
    sim.save("map_zoom_course");
}

/// A finger dragged from `from` by `by` pixels on the touch panel, through the touch path as on the unit (sampled every
/// [`TOUCH_MS`]), then lifted; the passes run until it is handled.
pub(super) fn drag(sim: &mut Sim, app: &mut App, from: (i32, i32), by: (i32, i32)) {
    let t0 = sim.ms;
    sim.touch.finger = Box::new(move |ms| {
        let t = ms.checked_sub(t0)? as i32;
        let k = (t - 50).clamp(0, 200);
        (t < 400).then_some(((from.0 + by.0 * k / 200) as u16, (from.1 + by.1 * k / 200) as u16))
    });
    run(sim, app, 800);
    sim.touch.finger = Box::new(|_| None);
}

/// Panning the map (map.rs): fitted, a swipe still changes the page; zoomed in, a drag on the map moves the map with
/// the finger at the release, the view stops following, "+" keeps the centre, "re-centre" follows again; the pan is
/// kept near the track; the keys still change pages and a hold still opens the menu. The panel equals the screenshot
/// at each step.
#[test]
fn map_pan() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, minus] = map::buttons(area).unwrap();
    let centre_button = (minus.0, minus.1 + minus.3 + 4, minus.2, minus.3);
    let tap = |sim: &mut Sim, app: &mut App, b: map::Rect| {
        app.input(sim, Input::Touch(Gesture::Tap((b.0 + b.2 / 2) as u16, (b.1 + b.3 / 2) as u16)));
        assert_consistent(sim, app);
    };
    // fitted: a swipe on the map changes the page as before, and back
    drag(&mut sim, &mut app, (160, 200), (-80, 0));
    assert_eq!(app.screen(), Screen::Ride(0), "swipe left: the next page");
    drag(&mut sim, &mut app, (80, 200), (80, 0));
    assert_eq!(app.screen(), Screen::Ride(2), "swipe right: the map again");
    assert_eq!(app.map_centre, None);
    tap(&mut sim, &mut app, plus);
    tap(&mut sim, &mut app, plus);
    let v0 = app.map_view(0).unwrap();
    let m = |px: i32| px * v0.mpp16 as i32 / 16;
    // zoomed: a drag pans, the map moving with the finger (right and up: the view goes west and south)
    let (px, t0) = (sim.pixels, sim.ms);
    drag(&mut sim, &mut app, (100, 200), (60, -40));
    assert_eq!(app.screen(), Screen::Ride(2), "the drag panned, no page change");
    let v = app.map_view(0).unwrap();
    assert_eq!((v.cx, v.cy), (v0.cx - m(60), v0.cy - m(40)), "moved by the finger's vector");
    assert_eq!(app.map_centre, Some((v.cx, v.cy)));
    assert_consistent(&sim, &app);
    sim.save("map_pan");
    std::println!("a pan: {} px drawn in {} ms", sim.pixels - px, sim.ms - t0);
    // a short drag (not a swipe) pans too
    drag(&mut sim, &mut app, (100, 200), (-20, 0));
    assert_eq!(app.map_view(0).unwrap().cx, v.cx + m(20));
    // the view stays where it was put while the rider goes on
    let (kept, px, t0) = (app.map_view(0), sim.pixels, sim.ms);
    run(&mut sim, &mut app, 60_000);
    assert_eq!(app.map_view(0), kept, "panned: no following");
    std::println!("panned, riding: {} px/s", (sim.pixels - px) / ((sim.ms - t0) / 1000));
    assert_consistent(&sim, &app);
    // "+" while panned: finer around the same centre
    tap(&mut sim, &mut app, plus);
    let z = app.map_view(0).unwrap();
    assert!(z.mpp16 < v0.mpp16 && (z.cx, z.cy) == (kept.unwrap().cx, kept.unwrap().cy), "{z:?}");
    // a hold on the zoomed map opens the page editor (no ride recording)
    app.input(&mut sim, Input::Touch(Gesture::Hold(100, 200)));
    assert!(matches!(app.screen(), Screen::Edit(..)));
    app.input(&mut sim, Input::Back);
    // the pan is kept within PAN_SCREENS map widths of the track
    for _ in 0..30 {
        drag(&mut sim, &mut app, (200, 150), (-180, 0));
    }
    let far = app.map_view(0).unwrap();
    let east = app.track.points().iter().map(|p| p.0).max().unwrap();
    assert!(far.cx <= east + area.2 * map::PAN_SCREENS * far.mpp16 as i32 / 16 + 1, "{far:?}");
    assert!(far.cx > east, "far east: {far:?}");
    assert_consistent(&sim, &app);
    sim.save("map_pan_far");
    // re-centre: following the rider again, the button gone
    tap(&mut sim, &mut app, centre_button);
    assert_eq!(app.map_centre, None);
    let v = app.map_view(0).unwrap();
    let (x, y) = v.px(area, app.track.pos().unwrap());
    assert!(x >= area.2 / 5 && x < area.2 * 4 / 5 && y >= area.1 + area.3 / 5, "the rider in the middle: {x},{y}");
    assert!(app.zoom_at(centre_button.0 as u16 + 20, centre_button.1 as u16 + 20).is_none(), "no re-centre button");
    sim.save("map_pan_recentred");
    run(&mut sim, &mut app, 30_000);
    assert_consistent(&sim, &app);
    // zoomed, the keys still change pages
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "Page: the next page");
}

/// After "+" the pan's range is smaller: a centre panned to the old edge is outside it, and a small drag must still
/// move the map with the finger, not jump back across (review of 6de15ed: 960 m the wrong way). A drag that starts
/// on a button does not pan.
#[test]
fn pan_after_zoom_in_moves_with_the_finger() {
    use crate::map;
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.route = Some(tiles_route);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 200_000);
    let area = map::area(ui::field_box(1, ui::RIDE_H, 0).unwrap());
    let [plus, _] = map::buttons(area).unwrap();
    let (px, py) = ((plus.0 + 20) as u16, (plus.1 + 20) as u16);
    app.input(&mut sim, Input::Touch(Gesture::Tap(px, py)));
    for _ in 0..30 {
        drag(&mut sim, &mut app, (200, 150), (-180, 0));
    }
    app.input(&mut sim, Input::Touch(Gesture::Tap(px, py)));
    let before = app.map_view(0).unwrap();
    drag(&mut sim, &mut app, (150, 150), (-15, 0)); // the finger left: the view goes east, or stays at the edge
    let after = app.map_view(0).unwrap();
    assert!(after.cx >= before.cx, "a drag left moved the view west by {} m", before.cx - after.cx);
    assert_consistent(&sim, &app);
    // a gloved tap sliding 12 px on "-" is "-": a level out, the panned centre kept, not a pan
    let [_, minus] = map::buttons(area).unwrap();
    let (zoom, centre) = (app.map_zoom, app.map_centre);
    drag(&mut sim, &mut app, (minus.0 + 14, minus.1 + 20), (12, 0));
    assert_eq!(app.map_zoom.map(|z| z + 1), zoom.map(|z| z + 2), "one level out");
    assert_eq!(app.map_centre, centre, "not panned");
    assert_consistent(&sim, &app);
}
