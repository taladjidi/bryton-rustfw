//! Live segments on the Sim (app/segments.rs): the index read from the card, a ride along a synthetic segment (the
//! approach, the start, the delta against the PR, the finish), a ride that leaves it, a card with broken files, no
//! memory for segments, and a reset mid-segment.

use super::tests::run; // not the parent's `run`, which the glob would give
use super::*;
use crate::segment::tests::{synth, synth_alt};
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

/// efforts.csv with its chain looped, listed before the segment: the index build goes on (the segment indexed), and
/// the file counts as unfinished (the FATs compared before the next write).
#[test]
fn a_broken_csv_does_not_stop_the_index() {
    let mut image = card(&[("efforts.csv", b"id,date,elapsed_s\n".to_vec()), ("4000000001.seg", synth(20, &CODES))]);
    let first = {
        use crate::fat::{Fat, tests::Ram};
        let mut d = Ram(image.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, crate::app::DIR).unwrap();
        fat.locate(&mut d, dir, "efforts.csv").unwrap().first as usize
    };
    let u =
        |img: &[u8], at: usize, n: usize| img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b));
    let (fat0, fat_len) = (u(&image, 14, 2) * 512, u(&image, 36, 4) * 512);
    for k in [0, fat_len] {
        let at = fat0 + k + 4 * first;
        image[at..at + 4].copy_from_slice(&(first as u32).to_le_bytes()); // the cluster names itself next
    }
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = image;
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 9000);
    let s = app.segments().unwrap();
    assert!(s.ready());
    assert_eq!(s.ids().collect::<Vec<_>>(), [ID]);
    assert!(app.fats_suspect);
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
    let (y0, y1) = (ui::PAGE_Y + 21, ui::PAGE_Y + 81);
    (y0..y1).map(|y| (0..ui::W).filter(|&x| sim.screen[(y * ui::W + x) as usize] == color).count()).sum()
}

/// Rides `route` for `seconds`, one second at a time, and collects the banners said and the screens visited.
fn watch(route: fn(u64) -> (i32, i32, i32), seconds: u64) -> (Sim, App, Vec<String>, Vec<Screen>) {
    watch_on(good_card(), route, seconds)
}

/// A card with the segment's file carrying altitudes (8 % over 950 m), for the profile.
fn alt_card() -> Vec<u8> {
    let up = |r: usize| Some(((600 + 4 * r) * 5) as u16);
    card(&[("4000000001.seg", synth_alt(20, &CODES, &up))])
}

/// [`watch`] on the card `image`.
fn watch_on(image: Vec<u8>, route: fn(u64) -> (i32, i32, i32), seconds: u64) -> (Sim, App, Vec<String>, Vec<Screen>) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = image;
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
    assert_eq!(screens.iter().filter(|s| **s == Screen::Ride(CLIMB)).count(), 1, "{screens:?}");
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
    let (sim, app, ..) = watch_on(alt_card(), faster, 120);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
    assert!(app.segment_now().unwrap().delta_s.unwrap() < 0);
    sim.save("segment_ahead");
    assert!(in_delta_box(&sim, ui::GREEN) > 100 && in_delta_box(&sim, ui::ACCENT) == 0);
    let (sim, app, ..) = watch_on(alt_card(), slower, 140);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
    assert!(app.segment_now().unwrap().delta_s.unwrap() > 0);
    sim.save("segment_behind");
    assert!(in_delta_box(&sim, ui::ACCENT) > 100 && in_delta_box(&sim, ui::GREEN) == 0);
    assert_eq!(in_delta_box(&sim, ui::RED), 0);
}

/// LIVE SEGMENTS off: no event, no page, no banner. The page is in the round: a key pages away from it, and paging on
/// comes back to it.
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
    assert!(events(&app).is_empty() && !matches!(app.screen(), Screen::Ride(CLIMB) | Screen::SegmentResult));
    end_until_settled(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!((app.segments().unwrap().written, csv(&sim.emmc.image)), (0, None));
    let (mut sim, mut app, ..) = watch(pr_pace, 120);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 5000);
    assert!(matches!(app.screen(), Screen::Ride(n) if n != CLIMB), "paged away, not reopened");
    assert!(app.page_on(CLIMB), "still in the round");
    let back = (0..8).any(|_| {
        app.press(&mut sim, Press::Short(Key::Page));
        app.screen() == Screen::Ride(CLIMB)
    });
    assert!(back, "paging on finds it again");
}

/// The page is in the round while the segment is ridden, opens once at its start, and is gone at the finish (the
/// card is shown) and at a loss (no card); the round goes on to the page after the one it joined.
#[test]
fn page_leaves_the_round_at_the_finish_and_at_a_loss() {
    let (mut sim, mut app, _, screens) = watch(faster, 110);
    assert!(app.page_on(CLIMB) && app.screen() == Screen::Ride(CLIMB));
    assert_eq!(screens.iter().filter(|s| **s == Screen::Ride(CLIMB)).count(), 1, "opened once: {screens:?}");
    run(&mut sim, &mut app, 130_000);
    assert_eq!(app.screen(), Screen::SegmentResult);
    assert!(!app.page_on(CLIMB));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 500);
    assert!(matches!(app.screen(), Screen::Ride(n) if n != CLIMB && app.page_on(n)), "{:?}", app.screen());
    // a loss: the page goes, no card
    let (mut sim, mut app, _, screens) = watch(leaves, 120);
    assert_eq!(screens.iter().filter(|s| **s == Screen::Ride(CLIMB)).count(), 1, "{screens:?}");
    run(&mut sim, &mut app, 100_000);
    assert!(!app.page_on(CLIMB) && app.segment_now().is_none());
    assert!(matches!(app.screen(), Screen::Ride(n) if n != CLIMB), "{:?}", app.screen());
    assert!(events(&app).contains(&Event::Lost(ID)));
}

/// A menu up at the start: not opened over it for this segment, but the page is in the round.
#[test]
fn page_not_over_a_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    sim.gnss.route = Some(faster);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.open(Screen::Menu(Menu::Segments, 0));
    app.show(&mut sim);
    run(&mut sim, &mut app, 110_000);
    assert!(app.segment_now().is_some());
    assert!(matches!(app.screen(), Screen::Menu(..)), "not over a menu: {:?}", app.screen());
    assert!(app.page_on(CLIMB), "but in the round");
}

/// Back to the overview while the segment's ride goes on: the page still opens over it at the segment's start.
#[test]
fn page_opens_over_the_overview_while_riding() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    sim.gnss.route = Some(faster);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Back));
    run(&mut sim, &mut app, 500);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main, "the overview while riding");
    let mut screens = std::vec::Vec::new();
    for _ in 0..110 {
        run(&mut sim, &mut app, 1000);
        if screens.last() != Some(&app.screen()) {
            screens.push(app.screen());
        }
    }
    assert!(screens.contains(&Screen::Ride(CLIMB)), "opened over the overview: {screens:?}");
}

/// The profile of a segment with altitudes is its own, the target's place is a grey mark on it and the rider's a
/// white one; a segment without altitudes shows its bar with both marks.
#[test]
fn profile_shows_both_marks() {
    for (image, name) in [(alt_card(), "profile"), (good_card(), "bar")] {
        let (sim, app, ..) = watch_on(image, slower, 120);
        assert_eq!(app.screen(), Screen::Ride(CLIMB));
        let (bx, p) = app
            .widgets()
            .zip(app.drawn.iter())
            .find_map(|((bx, _), d)| if let Drawn::Profile(p) = d { Some((bx, *p)) } else { None })
            .unwrap();
        let (r, g) = (p.rider().unwrap(), p.ghost().unwrap());
        let a = ui::climb_area(bx);
        let px = |k: u32, dy: u32| sim.screen[((a.1 + dy) * ui::W + a.0 + k) as usize];
        assert_eq!((px(r, 40), px(g, 40), px(g + 1, 40)), (ui::WHITE, ui::GREY, ui::GREY), "{name}");
        // the marks are where the metres are
        let n = app.segment_now().unwrap();
        let col = |m: u32| u64::from(m) * u64::from(a.2) / u64::from(n.along_m + n.left_m);
        // (the page was drawn up to a second ago: a column or two)
        let near = |a: u64, b: u64| a.abs_diff(b) <= 2;
        assert!(near(u64::from(r), col(n.along_m)) && near(u64::from(g), col(n.ghost_m.unwrap())), "{name}");
    }
}

/// A second with nothing changed paints nothing (the head, the profile, the gauge, the two values).
#[test]
fn an_update_with_nothing_changed_paints_nothing() {
    let (mut sim, mut app, ..) = watch_on(alt_card(), faster, 120);
    run(&mut sim, &mut app, 300);
    app.value_due = true;
    app.update_page(&mut sim); // the time of the pass may have moved a second on
    let px = sim.pixels;
    for _ in 0..2 {
        app.value_due = true;
        app.update_page(&mut sim);
    }
    assert_eq!(sim.pixels, px, "nothing to paint");
}

/// The page's second costs a few boxes, not a screen (docs/devloop.md: pixels a second).
#[test]
fn page_redraw_cost() {
    let (mut sim, mut app, ..) = watch(pr_pace, 110);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
    let (px, w0, t0) = (sim.pixels, sim.windows, sim.ms);
    run(&mut sim, &mut app, 20_000);
    let (per_s, win_s) = ((sim.pixels - px) * 1000 / (sim.ms - t0), (sim.windows - w0) * 1000 / (sim.ms - t0));
    std::println!("segment page: {per_s} px/s, {win_s} windows/s");
    // 4400 px/s with the old page (delta, time, distance and the bar); the CLIMB page adds the profile, the gauge and
    // two values (4650 px/s measured); the delta's colour is kept within 2 s, so a flip of the ghost's sign redraws
    // one character, not all of them
    assert!(per_s < 5000, "{per_s} px/s");
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

/// Start and Lap on the page and on the card act on the ride; a tap on the page while recording does nothing.
#[test]
fn keys_and_taps_on_the_page_and_the_card() {
    use crate::touch::Gesture;
    let (mut sim, mut app, ..) = watch(faster, 120);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
    let laps = app.ride.laps();
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, 200)));
    app.input(&mut sim, Input::NextHeld);
    assert_eq!(app.screen(), Screen::Ride(CLIMB), "a tap or a held key does not close it");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.ride.laps(), laps + 1, "Lap on the page is a lap");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Paused, "Start on the page pauses");
    // the card: Start and Lap close it and still act
    let (mut sim, mut app, ..) = watch(faster, 240);
    assert_eq!(app.screen(), Screen::SegmentResult);
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, 200)));
    assert_eq!(app.screen(), Screen::SegmentResult, "a tap does not close the card while recording");
    let laps = app.ride.laps();
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!((app.ride.laps(), app.screen() != Screen::SegmentResult), (laps + 1, true));
    let (mut sim, mut app, ..) = watch(faster, 240);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen() != Screen::SegmentResult), (ride::State::Paused, true));
}

/// The page opened over a page that is off by the end (a CLIMB whose climb ended): the finish returns to a live page.
#[test]
fn return_to_a_page_that_went() {
    let (mut sim, mut app, ..) = watch(faster, 120);
    app.seg_ui.set_prev(Screen::Ride(CLIMB));
    run(&mut sim, &mut app, 125_000);
    assert_eq!(app.screen(), Screen::SegmentResult);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(matches!(app.screen(), Screen::Ride(n) if app.page_on(n)), "{:?}", app.screen());
}

/// A name with a byte past ASCII is cut there, not emptied.
#[test]
fn utf8_names_are_cut_not_blanked() {
    let mut f = synth(20, &CODES);
    f[0x0d..0x0d + 5].copy_from_slice(&[b'A', b'B', 0xc3, 0xa9, 0]);
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = card(&[("4000000001.seg", f)]);
    sim.gnss.route = Some(pr_pace);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    let mut banners = Vec::<String>::new();
    for _ in 0..110 {
        run(&mut sim, &mut app, 1000);
        if app.notes.ms.is_some() && banners.last().map(String::as_str) != Some(app.notes.text.as_str()) {
            banners.push(app.notes.text.as_str().to_string());
        }
    }
    assert!(banners.iter().any(|b| b == "SEGMENT AB 300 M"), "{banners:?}");
}

/// The list shows the id and "--" while a ride runs (no header is read then).
#[test]
fn list_during_a_ride_shows_the_id() {
    let (mut sim, mut app, ..) = watch(pr_pace, 20);
    app.open(Screen::Menu(Menu::Segments, 2));
    app.show(&mut sim);
    run(&mut sim, &mut app, 2000);
    let (l, v, _) = app.menu_row(Menu::Segments, 2).unwrap();
    assert_eq!((l.as_str(), v.as_str()), ("4000000001", "--"));
}

/// The approach banner's header read: bytes read in its pass (mount, folder, one sector).
#[test]
fn approach_read_cost() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = good_card();
    sim.gnss.route = Some(pr_pace);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    let mut read = 0;
    for _ in 0..4000 {
        let before = sim.emmc.bytes_read;
        app.step(&mut sim);
        sim.delay_ms(POLL_MS);
        if app.notes.ms.is_some() && app.notes.text.as_str().ends_with("300 M") {
            read = sim.emmc.bytes_read - before;
            break;
        }
    }
    std::println!("approach banner: its pass reads {read} B");
    assert!(read > 0 && read < 20_000, "{read}");
}

/// Random inputs over a ride along the segment, the page and the card up at times: no panic, the ride is not ended,
/// and both screens were reached.
#[test]
fn fuzz_inputs_over_the_page_and_the_card() {
    use crate::touch::{Dir, Gesture};
    let (mut page, mut card) = (0u32, 0u32);
    for seed in 1..=60u32 {
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = good_card();
        sim.gnss.route = Some(faster);
        let mut app = App::start(&mut sim, None);
        app.set_segment_memory(Box::leak(Box::new(Segments::new())));
        run(&mut sim, &mut app, 8000);
        sim.gnss.fix = Some(8000);
        app.press(&mut sim, Press::Short(Key::Start));
        for _ in 0..330 {
            run(&mut sim, &mut app, 800);
            match app.screen() {
                Screen::Ride(CLIMB) => page += 1,
                Screen::SegmentResult => card += 1,
                _ => {}
            }
            if next() % 6 == 0 {
                let i = match next() % 12 {
                    0 => Input::Select,
                    1 => Input::Prev,
                    2 => Input::Next,
                    3 => Input::Back,
                    4 => Input::Media,
                    5 => Input::NextHeld,
                    6 => Input::PrevHeld,
                    7 => Input::Touch(Gesture::Tap(120, 160)),
                    8 => Input::Touch(Gesture::Swipe(Dir::Left)),
                    9 => Input::Touch(Gesture::Swipe(Dir::Right)),
                    10 => Input::Touch(Gesture::Hold(100, 100)),
                    _ => Input::Touch(Gesture::Drag(100, 100, 5, 5)),
                };
                app.input(&mut sim, i);
            }
        }
        assert_ne!(app.ride.state(), ride::State::Idle, "seed {seed}");
    }
    std::println!("segment fuzz: {page} page seconds, {card} card seconds");
    assert!(page > 100 && card > 20, "{page} {card}");
}

// Efforts (step S5): kept at the finish, written after the ride's save (app/segments.rs, "Efforts").

const PR: &str = "Rustfw/Segments/4000000001.pr";
const CSV: &str = "Rustfw/Segments/efforts.csv";

/// [`faster`], then again from the start 400 s later: two rides over the segment in one run.
fn faster_twice(ms: u64) -> (i32, i32, i32) {
    faster(ms % 400_000)
}

/// The newest whole copy of the PR file: (seq, total s, times).
fn pr(image: &[u8]) -> Option<(u32, u32, Vec<u16>)> {
    let f = crate::storage::trash::tests::file(image, PR)?;
    let p = crate::segment::PrCopy::newest(&f)?;
    Some((p.seq, p.total_s, p.iter().collect()))
}

fn csv(image: &[u8]) -> Option<std::string::String> {
    crate::storage::trash::tests::file(image, CSV).map(|b| std::string::String::from_utf8(b).unwrap())
}

/// A PR file of `total` s at an even pace over the synthetic segment's 20 points, as copy `seq`.
fn pr_file(total: u16, seq: u32) -> Vec<u8> {
    let times: Vec<u16> = (0..20).map(|i| (u32::from(total) * i / 19) as u16).collect();
    let copy = crate::segment::pr_copy(seq, ID, 1_100_000_000, &times).unwrap();
    let mut f = std::vec![0u8; crate::segment::PR_FILE];
    let at = (seq % 2) as usize * crate::segment::PR_COPY;
    f[at..at + crate::segment::PR_COPY].copy_from_slice(&copy);
    f
}

/// Pauses and ends the ride (SAVE), then passes until the card is free for the efforts (both copies saved).
fn end_until_settled(sim: &mut Sim, app: &mut App) -> [u8; 12] {
    app.press(sim, Press::Short(Key::Start)); // pause
    app.end_ride();
    let name = app.rec.clean.name();
    for _ in 0..2000 {
        if app.ride_settled() {
            return name;
        }
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
    panic!("the ride never saved");
}

fn finish(app: &App) -> (u32, [Option<i32>; crate::segment::KEPT]) {
    let ev = events(app);
    let Some(Event::Finish { elapsed_s, deltas, .. }) = ev.last().copied() else { panic!("{ev:?}") };
    (elapsed_s, deltas)
}

/// Faster than the file's PR: the PR file is written after the save, never before, with real splits; a csv line;
/// the volume clean. The next ride over it (same run) has the new PR as its target: a delta of about 0 at the same
/// pace, the PR file left as it is unless faster, a second csv line.
#[test]
fn a_faster_effort_is_the_next_target() {
    let (mut sim, mut app) = ride_on(card(&[("4000000001.seg", synth(20, &CODES))]), faster_twice, 260, true);
    let (elapsed, deltas) = finish(&app);
    assert!(elapsed.abs_diff(158) <= 2, "{elapsed}"); // 950 m at 6 m/s
    assert!(deltas[0].unwrap() <= -30, "{deltas:?}"); // against the file's PR, 190 s
    assert_eq!(app.segments().unwrap().efforts(), 1);
    assert_eq!(pr(&sim.emmc.image), None, "nothing written during the ride");
    let name = end_until_settled(&mut sim, &mut app);
    assert_eq!(pr(&sim.emmc.image), None, "nothing before the save is done");
    assert_eq!(super::recorder_tests::places(&sim, name)[0], ["Activities"]);
    run(&mut sim, &mut app, 2000);
    let s = app.segments().unwrap();
    assert_eq!((s.written, s.prs, s.unwritten, s.efforts()), (1, 1, 0, 0));
    let (seq, total, times) = pr(&sim.emmc.image).expect("the PR file");
    assert_eq!((seq, total), (1, elapsed));
    // the splits: 50 m a point at 6 m/s, as ridden (within the 1 s fixes' rounding)
    for (i, &t) in times.iter().enumerate() {
        assert!((f64::from(t) - i as f64 * 50.0 / 6.0).abs() <= 2.0, "point {i}: {t} s, {times:?}");
    }
    let text = csv(&sim.emmc.image).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0], "id,date,elapsed_s");
    let f: Vec<&str> = lines[1].split(',').collect();
    assert_eq!((f[0], f[2]), ("4000000001", std::format!("{elapsed}").as_str()), "{text}");
    assert!(f[1].len() == 20 && f[1].starts_with("20") && f[1].ends_with('Z'), "{text}");
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    let before = crate::storage::trash::tests::file(&sim.emmc.image, PR);

    // the second ride: from 400 s, the segment loaded again with the PR file as its PR
    let ms = 395_000 - sim.ms;
    run(&mut sim, &mut app, ms);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start));
    let ms = 665_000 - sim.ms;
    run(&mut sim, &mut app, ms);
    let s = app.segments().unwrap();
    assert_eq!(s.pr_files, 1, "the PR file read at the load");
    let (elapsed2, deltas2) = finish(&app);
    assert!(deltas2[0].unwrap().abs() <= 2, "{elapsed2} {deltas2:?}");
    end_until_settled(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let s = app.segments().unwrap();
    assert_eq!((s.written, s.prs), (2, 1 + u16::from(elapsed2 < elapsed)));
    if elapsed2 >= elapsed {
        assert_eq!(crate::storage::trash::tests::file(&sim.emmc.image, PR), before, "not replaced");
    }
    assert_eq!(csv(&sim.emmc.image).unwrap().lines().count(), 3);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// Slower than the rider's PR file: the file is the target (the delta against its 150 s), it is not replaced, and
/// the csv (made now) gets the effort.
#[test]
fn a_slower_effort_adds_a_line_only() {
    let image = card(&[("4000000001.seg", synth(20, &CODES)), ("4000000001.pr", pr_file(150, 3))]);
    let (mut sim, mut app) = ride_on(image, slower, 370, true);
    let (elapsed, deltas) = finish(&app);
    assert!(elapsed.abs_diff(238) <= 2, "{elapsed}"); // 950 m at 4 m/s
    assert!(deltas[0].unwrap().abs_diff(i32::try_from(elapsed).unwrap() - 150) <= 1, "{deltas:?}");
    end_until_settled(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let s = app.segments().unwrap();
    assert_eq!((s.pr_files, s.written, s.prs), (1, 1, 0));
    assert_eq!(crate::storage::trash::tests::file(&sim.emmc.image, PR), Some(pr_file(150, 3)));
    assert_eq!(csv(&sim.emmc.image).unwrap().lines().count(), 2);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A PR file that does not fit (its copy torn, another id, another point count, not whole) is ignored: the source's
/// PR is the target.
#[test]
fn a_pr_file_that_does_not_fit_is_ignored() {
    let mut torn = pr_file(150, 2);
    torn[30] ^= 1;
    let with = |id: u32, np: u32| {
        let times: Vec<u16> = (0..np).map(|i| (150 * i / (np - 1)) as u16).collect();
        let mut f = std::vec![0u8; crate::segment::PR_FILE];
        f[..1024].copy_from_slice(&crate::segment::pr_copy(2, id, 0, &times).unwrap());
        f
    };
    for bad in [torn, with(ID + 1, 20), with(ID, 19), pr_file(150, 2)[..1024].to_vec()] {
        let image = card(&[("4000000001.seg", synth(20, &CODES)), ("4000000001.pr", bad)]);
        let (_, app) = ride_on(image, pr_pace, 300, true);
        let (_, deltas) = finish(&app);
        assert_eq!(app.segments().unwrap().pr_files, 0);
        assert!(deltas[0].unwrap().abs() <= 2, "{deltas:?}"); // the source's PR, ridden at its pace
    }
}

/// A ride discarded: its effort is never written.
#[test]
fn a_discarded_ride_writes_nothing() {
    let (mut sim, mut app) = ride_on(card(&[("4000000001.seg", synth(20, &CODES))]), faster, 260, true);
    assert_eq!(app.segments().unwrap().efforts(), 1);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = app.rec.clean.name();
    app.discard_ride();
    run(&mut sim, &mut app, 30_000);
    assert_eq!(super::recorder_tests::places(&sim, name)[0], ["Rustfw/Discarded"]);
    let s = app.segments().unwrap();
    assert_eq!((s.written, s.unwritten, s.efforts()), (0, 0, 0));
    assert_eq!((pr(&sim.emmc.image), csv(&sim.emmc.image)), (None, None));
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A build that may not write the card keeps nothing for later either.
#[test]
fn a_read_only_build_writes_nothing() {
    let mut sim = Sim::new(9, 0, 0);
    sim.writable = false;
    sim.emmc.image = card(&[("4000000001.seg", synth(20, &CODES))]);
    sim.gnss.route = Some(faster);
    let mut app = App::start(&mut sim, None);
    app.set_segment_memory(Box::leak(Box::new(Segments::new())));
    run(&mut sim, &mut app, 8000);
    sim.gnss.fix = Some(8000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 260_000);
    finish(&app);
    let before = sim.emmc.sectors_written;
    app.press(&mut sim, Press::Short(Key::Start));
    app.end_ride();
    run(&mut sim, &mut app, 20_000);
    assert_eq!(sim.emmc.sectors_written, before);
    assert_eq!(app.segments().unwrap().efforts(), 0);
}

/// The card full when the efforts are written: the effort is counted and dropped, nothing half made, the volume
/// clean, the saved ride as it was, and the next ride starts.
#[test]
fn a_full_card_drops_the_effort() {
    let (mut sim, mut app) = ride_on(card(&[("4000000001.seg", synth(20, &CODES))]), faster, 260, true);
    let name = end_until_settled(&mut sim, &mut app);
    let ride = super::recorder_tests::ride_file(&sim, "Activities", name).unwrap();
    crate::fat::write::tests::fill(&mut sim.emmc.image, 0, true);
    run(&mut sim, &mut app, 2000);
    let s = app.segments().unwrap();
    assert_eq!((s.written, s.unwritten, s.efforts()), (0, 1, 0));
    assert_eq!((pr(&sim.emmc.image), csv(&sim.emmc.image)), (None, None));
    assert_eq!(super::recorder_tests::ride_file(&sim, "Activities", name), Some(ride));
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.state(), ride::State::Running);
}

/// A power cut after every sector written in the pass that writes the effort, on a card with no PR file yet, on one
/// with a slower PR file and a csv line, and on one whose csv ends 20 bytes before its cluster does (the line takes a
/// new cluster): the saved ride and its records as they were, the PR file read whole as
/// the old one (or none) or the new one, the csv as it was or with the new line, a restart that resumes nothing, and
/// the volume as a cut may leave it (at most the FAT sector being written apart, a cluster lost), and when the FATs
/// were left apart, the restart's index build finding the file unfinished so that the next write compares them. The
/// unit's card has the reliable write (BOARD.md, "Reliable write"), so the sector being written keeps its old data.
#[test]
fn a_cut_at_every_sector_of_the_effort() {
    let cluster = {
        let mut d = crate::fat::tests::Ram(card(&[]), 0);
        crate::fat::Fat::mount(&mut d).unwrap().cluster_bytes() as usize
    };
    let line = "4000000001,2026-10-01T08:00:00Z,190\n";
    let mut long = "id,date,elapsed_s\n".to_string();
    while long.len() + 2 * line.len() < cluster - 20 {
        long.push_str(line);
    }
    // the last line's time padded with zeros to end 20 bytes before the cluster does
    long.push_str(&std::format!("4000000001,2026-10-01T08:00:00Z,1{}\n", "0".repeat(cluster - 20 - long.len() - 34)));
    assert_eq!(long.len(), cluster - 20);
    let (mut apart_seen, mut past_seen) = (0, 0);
    for (case, old_csv) in [(0, std::string::String::new()), (1, std::format!("id,date,elapsed_s\n{line}")), (2, long)]
    {
        let had = case > 0;
        let old_csv = old_csv.as_str();
        let setup = || {
            let mut files = std::vec![("4000000001.seg", synth(20, &CODES))];
            if had {
                files.push(("4000000001.pr", pr_file(190, 1)));
                files.push(("efforts.csv", old_csv.as_bytes().to_vec()));
            }
            let mut sim = Sim::new(9, 0, 0);
            sim.emmc.image = card(&files);
            sim.emmc.rel_param = crate::emmc::EN_REL_WR;
            sim.emmc.tear = true;
            sim.gnss.route = Some(faster);
            let mut app = App::start(&mut sim, None);
            app.set_segment_memory(Box::leak(Box::new(Segments::new())));
            run(&mut sim, &mut app, 8000);
            sim.gnss.fix = Some(8000);
            app.press(&mut sim, Press::Short(Key::Start));
            run(&mut sim, &mut app, 260_000);
            let name = end_until_settled(&mut sim, &mut app);
            (sim.emmc.sectors_written, sim, app, name)
        };
        let records =
            |img: &[u8]| ["Rustfw/ride.rec", "Rustfw/full.rec"].map(|p| crate::storage::trash::tests::file(img, p));
        let (base, mut sim, mut app, name) = setup();
        let ride = super::recorder_tests::ride_file(&sim, "Activities", name).unwrap();
        let recs = records(&sim.emmc.image);
        app.step(&mut sim);
        let total = sim.emmc.sectors_written - base;
        let s = app.segments().unwrap();
        assert_eq!((s.written, s.prs), (1, 1), "case {case}");
        let new_pr = pr(&sim.emmc.image).unwrap();
        let new_csv = csv(&sim.emmc.image).unwrap();
        let old_pr = had.then(|| pr(&card(&[("4000000001.pr", pr_file(190, 1))])).unwrap());
        assert_eq!(new_pr.1, finish(&app).0);
        let (mut olds, mut news, mut worst) = (0, 0, (0, 0));
        for cut in 0..total {
            let (base, mut sim, mut app, _) = setup();
            sim.emmc.cut_after = Some(base + cut);
            app.step(&mut sim);
            run(&mut sim, &mut app, 1000);
            let img = sim.emmc.image.clone();
            let at = std::format!("case {case}, cut {cut}");
            assert_eq!(super::recorder_tests::ride_file(&sim, "Activities", name), Some(ride.clone()), "{at}");
            assert_eq!(records(&img), recs, "{at}: the ride's records");
            let got = pr(&img);
            if got == old_pr {
                olds += 1;
            } else {
                assert_eq!(got.as_ref(), Some(&new_pr), "{at}: neither the old PR nor the new");
                news += 1;
            }
            let lines = csv(&img).unwrap_or_default();
            assert!(lines == old_csv || lines == new_csv, "{at}: csv {lines:?}");
            let scan = crate::fat::write::tests::fsck_cut(&img).unwrap_or_else(|e| panic!("{at}: {e}"));
            // a cluster linked past the csv's size (a grow cut before the size): the next append writes over it
            assert!(scan.past.len() <= 1 && scan.past.iter().all(|p| p.1 == 1), "{at}: past its size {:?}", scan.past);
            assert!(scan.lost <= 1 && scan.apart <= 1, "{at}: {} lost, {} apart", scan.lost, scan.apart);
            worst = (worst.0.max(scan.lost), worst.1.max(scan.apart));
            if !scan.past.is_empty() {
                use crate::fat::{Fat, tests::Ram};
                let mut d = Ram(img.clone(), 0);
                let mut fat = Fat::mount(&mut d).unwrap();
                fat.sync_fats(&mut d).unwrap();
                let dir = fat.open(&mut d, crate::app::DIR).unwrap();
                crate::app::append_line(&mut fat, &mut d, dir, line.as_bytes(), (0, 0)).unwrap();
                assert_eq!(crate::fat::write::tests::fsck(&d.0), Ok(0), "{at}: the next append");
                assert_eq!(csv(&d.0).unwrap(), std::format!("{old_csv}{line}"), "{at}: the next append");
                past_seen += 1;
            }
            // a restart on the card: nothing to resume, the ride where it was, the PR file as read above
            let mut sim2 = Sim::new(9, 5, 0);
            sim2.emmc.image = img;
            sim2.emmc.rel_param = crate::emmc::EN_REL_WR;
            let mut app2 = App::start(&mut sim2, None);
            app2.set_segment_memory(Box::leak(Box::new(Segments::new())));
            run(&mut sim2, &mut app2, 10_000);
            assert!(app2.segments().unwrap().ready(), "{at}");
            if scan.apart > 0 {
                // a file left unfinished is seen; a cluster taken for a new file before its entry is lost, named by
                // no file, which nothing can see but a whole compare (as any FAT create's)
                assert!(app2.fats_suspect || scan.lost > 0, "{at}: the FATs apart, unseen");
                apart_seen += usize::from(app2.fats_suspect);
            }
            assert_eq!(app2.ride.state(), ride::State::Idle, "{at}");
            assert_eq!(super::recorder_tests::places(&sim2, name)[0], ["Activities"], "{at}");
            assert_eq!(pr(&sim2.emmc.image), got, "{at}");
        }
        std::println!(
            "effort write (case {case}): {total} sectors, {olds} cuts read the old, {news} the new, worst (lost, apart) {worst:?}"
        );
        assert!(total > 0 && news > 0 && (olds > 0 || !had));
    }
    assert!(apart_seen > 0, "a cut between the two FATs' writes was reached");
    assert!(past_seen > 0, "a cut leaving a cluster past the csv's size was reached");
}

/// A PR file for other points (the segment rebuilt with 19, its copy numbered 2 in slot 0): a faster effort goes over
/// the other slot numbered 3, so it is the newest, and the next ride over the segment takes it as its PR.
#[test]
fn a_pr_file_for_other_points_is_replaced() {
    let times: Vec<u16> = (0..19).map(|i| (300 * i / 18) as u16).collect();
    let mut stale = std::vec![0u8; crate::segment::PR_FILE];
    stale[..1024].copy_from_slice(&crate::segment::pr_copy(2, ID, 0, &times).unwrap());
    let image = card(&[("4000000001.seg", synth(20, &CODES)), ("4000000001.pr", stale)]);
    let (mut sim, mut app) = ride_on(image, faster_twice, 260, true);
    let (elapsed, _) = finish(&app);
    assert_eq!(app.segments().unwrap().pr_files, 0, "the stale file ignored");
    end_until_settled(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let (seq, total, times) = pr(&sim.emmc.image).unwrap();
    assert_eq!((seq, total, times.len()), (3, elapsed, 20));
    let ms = 395_000 - sim.ms;
    run(&mut sim, &mut app, ms);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Start));
    let ms = 665_000 - sim.ms;
    run(&mut sim, &mut app, ms);
    assert_eq!(app.segments().unwrap().pr_files, 1);
    assert!(finish(&app).1[0].unwrap().abs() <= 2);
}

/// [`faster`] (158 s), then in the same ride again from 400 s at 5.5 m/s (173 s) or at 7 m/s (136 s).
fn twice_then_slower(ms: u64) -> (i32, i32, i32) {
    if ms < 400_000 { faster(ms) } else { east(ms - 400_000, 5.5) }
}
fn twice_then_faster(ms: u64) -> (i32, i32, i32) {
    if ms < 400_000 { faster(ms) } else { east(ms - 400_000, 7.0) }
}

/// The result card's NEW PR follows the rule the PR file is written with: the first pass beats the file's PR (190 s);
/// a second pass in the same ride says NEW PR only if it beats the first too, and only it is written as the PR.
#[test]
fn new_pr_twice_in_a_ride() {
    for (route, second_new) in [(twice_then_slower as fn(u64) -> (i32, i32, i32), false), (twice_then_faster, true)] {
        let (mut sim, mut app) = ride_on(card(&[("4000000001.seg", synth(20, &CODES))]), route, 260, true);
        assert!(app.segments().unwrap().new_pr(), "the first pass beats the file's PR");
        run(&mut sim, &mut app, 420_000);
        let (elapsed, deltas) = finish(&app);
        assert!(deltas[0].unwrap() < 0, "{deltas:?}: faster than the file's PR either way");
        let s = app.segments().unwrap();
        assert_eq!((s.efforts(), s.new_pr()), (2, second_new), "{elapsed}");
        end_until_settled(&mut sim, &mut app);
        run(&mut sim, &mut app, 3000);
        let s = app.segments().unwrap();
        assert_eq!((s.written, s.prs), (2, 1 + u16::from(second_new)));
        let best = if second_new { elapsed } else { pr(&sim.emmc.image).unwrap().1 };
        assert_eq!(pr(&sim.emmc.image).unwrap().1, best);
        assert!(best < 160, "{best}");
    }
}

/// The climb pacer takes the profile of the segment being ridden (its file's altitudes): a plan from the rider to the
/// finish keyed by the segment's id, with a target on it; leaving the segment drops the plan (no course, no climb
/// ahead here); a segment file without altitudes gives none.
#[test]
fn the_pacer_follows_the_segment() {
    let up = |r: usize| Some(((600 + 4 * r) * 5) as u16); // 8 % over 950 m
    let with = card(&[("4000000001.seg", synth_alt(20, &CODES, &up))]);
    let (_, app) = ride_on(with.clone(), faster, 120, true);
    assert_eq!(app.segment_now().map(|n| n.id), Some(ID));
    assert_eq!(app.pace.key.map(|k| k.0), Some(ID ^ 0x8000_0000), "keyed by the segment");
    assert!(app.pace.plan.n > 0 && app.pace_now(100).step, "a target on the segment");
    let (_, app) = ride_on(with, leaves, 200, true);
    assert!(app.segment_now().is_none());
    assert!(app.pace.key.is_none() && app.pace.plan.n == 0, "off the segment: no plan");
    let (_, app) = ride(faster, 120, true);
    assert!(app.segment_now().is_some() && app.pace.key.is_none(), "no altitudes: no plan");
}
