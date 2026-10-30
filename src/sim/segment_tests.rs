//! Live segments on the Sim (app/segments.rs): the index read from the card, a ride along a synthetic segment (the
//! approach, the start, the delta against the PR, the finish), a ride that leaves it, a card with broken files, no
//! memory for segments, and a reset mid-segment.

use super::tests::run; // not the parent's `run`, which the glob would give
use super::*;
use crate::segment::tests::synth;
use crate::segment_match::{Event, tests::at};
use std::string::{String, ToString};

const ID: u32 = 4_000_000_001;
const CODES: [[u8; 2]; 5] = [*b"07", *b"05", *b"06", *b"12", *b"13"];

/// The card with `files` in `\Rustfw\Segments`.
pub(super) fn card(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    use crate::fat::{Fat, tests::Ram, write::dos_time};
    let mut d = Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let t = dos_time((2026, 10, 20, 8, 0, 0));
    let root = fat.root();
    let rustfw = fat.ensure_dir(&mut d, root, "Rustfw", t).unwrap();
    let dir = fat.ensure_dir(&mut d, rustfw, "Segments", t).unwrap();
    for (name, bytes) in files {
        let mut padded = bytes.clone();
        padded.resize(bytes.len().div_ceil(512) * 512, 0);
        let mut h = fat.create(&mut d, dir, name, t).unwrap();
        fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
        fat.set_size(&mut d, &mut h, bytes.len() as u32, t).unwrap();
    }
    d.0
}

fn good_card() -> Vec<u8> {
    card(&[
        ("4000000001.seg", synth(20, &CODES)),
        ("5.seg", vec![0x5a; 700]),                // not a segment
        ("4000000002.seg", synth(20, &CODES)),     // a segment under another id's name
        ("short.seg", vec![0; 100]),               // shorter than a header
        ("readme.txt", b"not a segment".to_vec()), // not a candidate
    ])
}

/// East along the segment at `v` m/s from 400 m before it, from 20 s (a ride started at 10 s has a fix by then).
fn east(ms: u64, v: f64) -> (i32, i32, i32) {
    let (lat, lon) = at(-400.0 + v * ms.saturating_sub(20_000) as f64 / 1000.0, 0.0);
    (lat, lon, 9_000_000)
}
fn pr_pace(ms: u64) -> (i32, i32, i32) {
    east(ms, 5.0)
}
fn faster(ms: u64) -> (i32, i32, i32) {
    east(ms, 6.0)
}
/// 5 m/s east, then due north from 300 m along (off the track at once).
fn leaves(ms: u64) -> (i32, i32, i32) {
    let s = 5.0 * ms.saturating_sub(20_000) as f64 / 1000.0;
    let (lat, lon) = if s < 700.0 { at(-400.0 + s, 0.0) } else { at(300.0, (s - 700.0) * 3.0) };
    (lat, lon, 9_000_000)
}

fn ride(route: fn(u64) -> (i32, i32, i32), seconds: u64, memory: bool) -> (Sim, App) {
    ride_on(good_card(), route, seconds, memory)
}

fn ride_on(image: Vec<u8>, route: fn(u64) -> (i32, i32, i32), seconds: u64, memory: bool) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = image;
    sim.gnss.route = Some(route);
    let mut app = App::start(&mut sim, None);
    if memory {
        app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    }
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, seconds * 1000);
    (sim, app)
}

fn events(app: &App) -> Vec<Event> {
    app.segments().unwrap().events().map(|(_, e)| e).collect()
}

/// The index holds the one good file; the broken ones are counted, the text file is not a candidate.
#[test]
fn index_skips_broken_files() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 3000);
    assert!(!app.segments().unwrap().ready(), "never at boot");
    run(&mut sim, &mut app, 6000);
    let s = app.segments().unwrap();
    assert!(s.ready());
    assert_eq!((s.indexed(), s.skipped), (1, 3));
}

/// A card with 130 segments: 128 indexed, the rest skipped, the build spread over passes (8 files a pass, a sector
/// each).
#[test]
fn index_is_capped_and_sliced() {
    let files: Vec<(std::string::String, Vec<u8>)> = (0..130u32)
        .map(|k| {
            let mut f = synth(20, &CODES);
            f[1..5].copy_from_slice(&(4_000_000_001 + k).to_le_bytes());
            (std::format!("{}.seg", 4_000_000_001 + k), f)
        })
        .collect();
    let files: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, f)| (n.as_str(), f.clone())).collect();
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = card(&files);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 5000);
    let (mut worst, mut passes) = (0, 0);
    while !app.segments().unwrap().ready() && passes < 100 {
        let before = sim.emmc.bytes_read;
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        worst = worst.max(sim.emmc.bytes_read - before);
        passes += 1;
    }
    let s = app.segments().unwrap();
    assert_eq!((s.indexed(), s.skipped), (128, 2));
    std::println!("segment index: {passes} passes, worst {worst} B read in one pass");
    assert!(passes >= 16, "sliced: {passes}");
    assert!(worst < 40_000, "{worst} B in a pass");
}

/// A ride at the PR's pace: the approach, the start, a delta near 0 on the way, the finish.
#[test]
fn ride_at_pr_pace() {
    let (mut sim, mut app) = ride(pr_pace, 35, true);
    assert_eq!(events(&app), [Event::Approach(ID)]);
    run(&mut sim, &mut app, 100_000); // 215 m along
    let n = app.segment_now().expect("started");
    assert_eq!((n.id, n.name), (ID, &b"Test climb"[..]));
    assert!(n.delta_s.unwrap().abs() <= 2, "{n:?}");
    assert_eq!(n.target, Some(crate::segment::Effort::Pr));
    assert!(n.ghost_m.unwrap().abs_diff(n.along_m) < 20, "{n:?}");
    assert!(n.left_m > 700 && n.left_m < 900, "{n:?}");
    run(&mut sim, &mut app, 160_000);
    let ev = events(&app);
    let Some(Event::Finish { id, elapsed_s, deltas }) = ev.last().copied() else { panic!("{ev:?}") };
    assert_eq!(id, ID);
    assert!(elapsed_s.abs_diff(190) <= 2, "{elapsed_s}"); // 950 m at 5 m/s
    assert!(deltas.iter().flatten().any(|d| d.abs() <= 2), "{deltas:?}");
    assert!(app.segment_now().is_none(), "back to idle");
}

/// Faster than the PR: ahead (negative), and the ghost is behind the rider.
#[test]
fn ride_faster_than_the_pr() {
    let (mut sim, mut app) = ride(faster, 110, true);
    let n = app.segment_now().expect("started");
    assert!(n.delta_s.unwrap() <= -3, "{n:?}");
    assert!(n.ghost_m.unwrap() < n.along_m, "{n:?}");
    run(&mut sim, &mut app, 140_000);
    let ev = events(&app);
    assert!(matches!(ev.last(), Some(Event::Finish { .. })), "{ev:?}");
}

/// Leaving the track: lost, and the matcher is free for the next one.
#[test]
fn ride_that_leaves_is_lost() {
    let (_, app) = ride(leaves, 220, true);
    let ev = events(&app);
    assert_eq!(ev, [Event::Approach(ID), Event::Start(ID), Event::Lost(ID)]);
    assert!(app.segment_now().is_none());
}

/// No memory for segments (or LIVE SEGMENTS off): the ride is as it was, nothing happens.
#[test]
fn without_memory_nothing_happens() {
    let (_, app) = ride(pr_pace, 220, false);
    assert!(app.segments().is_none() && app.segment_now().is_none());
}

/// A reset mid-segment: the ride resumes, the segment does not (a new run has no active segment).
#[test]
fn reset_drops_the_segment() {
    let (mut sim, app) = ride(pr_pace, 110, true);
    assert!(app.segment_now().is_some());
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 1000);
    assert!(app.segment_now().is_none());
}

/// A good header over a broken body (a flipped byte in the FIT course, or in the time table): the file is read once,
/// the segment dropped and counted, taken out of the index (no read every second in the start zone), the matcher
/// idle, and the ride goes on recording.
#[test]
fn broken_body_is_dropped_once() {
    for at in [|len: usize| len - 5, |_: usize| 0x208 + 65 + 3 * 4 * 5 + 2] {
        let mut f = synth(20, &CODES);
        let i = at(f.len());
        f[i] ^= 0x5a;
        let (sim, app) = ride_on(card(&[("4000000001.seg", f)]), pr_pace, 120, true);
        let s = app.segments().unwrap();
        assert_eq!((s.dropped, s.loads, s.indexed()), (1, 1, 0), "flip at {i}");
        assert!(app.segment_now().is_none());
        assert_eq!(app.ride.state(), ride::State::Running);
        assert!(app.ride.timer_s() > 90, "{}", app.ride.timer_s());
        drop(sim);
    }
}

/// The ride ends (paused, then saved) while a segment is active: it does not outlive the ride.
#[test]
fn segment_ends_with_the_ride() {
    let (mut sim, mut app) = ride(pr_pace, 110, true);
    assert!(app.segment_now().is_some());
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    app.end_ride();
    run(&mut sim, &mut app, 2000);
    assert!(app.segment_now().is_none());
}

fn slower(ms: u64) -> (i32, i32, i32) {
    east(ms, 4.0)
}

/// Pixels of `color` in the delta's box (the page's big line).
fn in_delta_box(sim: &Sim, color: u16) -> usize {
    let (y0, y1) = (ui::PAGE_Y + 26, ui::PAGE_Y + 90);
    (y0..y1).map(|y| (0..ui::W).filter(|&x| sim.screen[(y * ui::W + x) as usize] == color).count()).sum()
}

/// Rides `route` for `seconds`, one second at a time, and collects the banners said and the screens visited.
fn watch(route: fn(u64) -> (i32, i32, i32), seconds: u64) -> (Sim, App, Vec<String>, Vec<Screen>) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    sim.gnss.route = Some(route);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    let (mut banners, mut screens) = (Vec::<String>::new(), Vec::<Screen>::new());
    for _ in 0..seconds {
        run(&mut sim, &mut app, 1000);
        if app.notes.ms.is_some() && banners.last().map(String::as_str) != Some(app.notes.text.as_str()) {
            banners.push(app.notes.text.as_str().to_string());
        }
        if screens.last() != Some(&app.screen()) {
            screens.push(app.screen());
        }
    }
    (sim, app, banners, screens)
}

/// The approach banner with the name, the page over the ride page, the finish banner and the card, then back.
#[test]
fn page_banners_card_and_back() {
    let (mut sim, mut app, banners, screens) = watch(faster, 240);
    assert!(banners.iter().any(|b| b == "SEGMENT Test climb 300 M"), "{banners:?}");
    let fin = banners.iter().find(|b| b.starts_with("SEGMENT 2:")).unwrap_or_else(|| panic!("{banners:?}"));
    assert!(fin.contains(" -0:"), "ahead: {fin}");
    assert_eq!(screens.iter().filter(|s| **s == Screen::Segment).count(), 1, "{screens:?}");
    assert_eq!(screens.last(), Some(&Screen::SegmentResult), "{screens:?}");
    sim.save("segment_card");
    app.press(&mut sim, Press::Short(Key::Page)); // any key
    run(&mut sim, &mut app, 500);
    assert!(matches!(app.screen(), Screen::Main | Screen::Ride(_)), "{:?}", app.screen());
    // the card closes by itself after 10 s
    let (mut sim, mut app, ..) = watch(faster, 240);
    assert_eq!(app.screen(), Screen::SegmentResult);
    run(&mut sim, &mut app, 11_000);
    assert!(matches!(app.screen(), Screen::Main | Screen::Ride(_)), "{:?}", app.screen());
}

/// Ahead is green, behind is the accent, never red; the sign is in the text.
#[test]
fn delta_sign_and_colour() {
    let (sim, app, ..) = watch(faster, 120);
    assert_eq!(app.screen(), Screen::Segment);
    assert!(app.segment_now().unwrap().delta_s.unwrap() < 0);
    sim.save("segment_ahead");
    assert!(in_delta_box(&sim, ui::GREEN) > 100 && in_delta_box(&sim, ui::ACCENT) == 0);
    let (sim, app, ..) = watch(slower, 140);
    assert_eq!(app.screen(), Screen::Segment);
    assert!(app.segment_now().unwrap().delta_s.unwrap() > 0);
    sim.save("segment_behind");
    assert!(in_delta_box(&sim, ui::ACCENT) > 100 && in_delta_box(&sim, ui::GREEN) == 0);
    assert_eq!(in_delta_box(&sim, ui::RED), 0);
}

/// LIVE SEGMENTS off: no event, no page, no banner. A key dismisses the page and it does not come back.
#[test]
fn off_and_dismissed() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    sim.gnss.route = Some(pr_pace);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    app.settings.live_segments = false;
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 200_000);
    assert!(events(&app).is_empty() && !matches!(app.screen(), Screen::Segment | Screen::SegmentResult));
    let (mut sim, mut app, ..) = watch(pr_pace, 120);
    assert_eq!(app.screen(), Screen::Segment);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    assert!(matches!(app.screen(), Screen::Main | Screen::Ride(_)), "dismissed");
}

/// The page's second costs a few boxes, not a screen (docs/devloop.md: pixels a second).
#[test]
fn page_redraw_cost() {
    let (mut sim, mut app, ..) = watch(pr_pace, 110);
    assert_eq!(app.screen(), Screen::Segment);
    let (px, w0, t0) = (sim.pixels, sim.windows, sim.ms);
    run(&mut sim, &mut app, 20_000);
    let (per_s, win_s) = ((sim.pixels - px) * 1000 / (sim.ms - t0), (sim.windows - w0) * 1000 / (sim.ms - t0));
    std::println!("segment page: {per_s} px/s, {win_s} windows/s");
    assert!(per_s < 8000, "{per_s} px/s");
}

/// The SEGMENTS menu: the two settings, the segment with its distance and gain, the skipped files.
#[test]
fn segments_menu_lists_the_card() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 9000);
    app.open(Screen::Menu(Menu::Segments, 2));
    app.show(&mut sim);
    run(&mut sim, &mut app, 1500);
    let row = |i| app.menu_row(Menu::Segments, i).map(|(l, v, _)| (l.as_str().to_string(), v.as_str().to_string()));
    assert_eq!(row(0), Some(("LIVE SEGMENTS".into(), "ON".into())));
    assert_eq!(row(1), Some(("TARGET".into(), "AUTO".into())));
    let (name, value) = row(2).unwrap();
    assert_eq!(name, "Test climb");
    assert!(value.ends_with('M') && value.contains("K "), "{value}");
    assert_eq!(row(3), Some(("SKIPPED FILES".into(), "3".into())));
    assert_eq!(row(4), None);
    sim.save("segment_menu");
}
