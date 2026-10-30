//! Live segments on the Sim (app/segments.rs): the index read from the card, a ride along a synthetic segment (the
//! approach, the start, the delta against the PR, the finish), a ride that leaves it, a card with broken files, no
//! memory for segments, and a reset mid-segment.

use super::tests::run; // not the parent's `run`, which the glob would give
use super::*;
use crate::segment::tests::synth;
use crate::segment_match::{Event, tests::at};

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

// Efforts (step S5): kept at the finish, written after the ride's save (app/segments.rs, "Efforts").

const PR: &str = "Rustfw/Segments/4000000001.pr";
const CSV: &str = "Rustfw/Segments/efforts.csv";

fn slower(ms: u64) -> (i32, i32, i32) {
    east(ms, 4.0)
}
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

/// A power cut after every sector written in the pass that writes the effort, on a card with no PR file yet and on
/// one with a slower PR file and a csv line: the saved ride and its records as they were, the PR file read whole as
/// the old one (or none) or the new one, the csv as it was or with the new line, a restart that resumes nothing, and
/// the volume as a cut may leave it (at most the FAT sector being written apart, a cluster lost). The unit's card
/// has the reliable write (BOARD.md, "Reliable write"), so the sector being written keeps its old data.
#[test]
fn a_cut_at_every_sector_of_the_effort() {
    let old_csv = "id,date,elapsed_s\n4000000001,2026-10-01T08:00:00Z,190\n";
    for had in [false, true] {
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
        assert_eq!((s.written, s.prs), (1, 1), "had {had}");
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
            let at = std::format!("had {had}, cut {cut}");
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
            let old = if had { old_csv } else { "" };
            assert!(lines == old || lines == new_csv, "{at}: csv {lines:?}");
            let scan = crate::fat::write::tests::fsck_cut(&img).unwrap_or_else(|e| panic!("{at}: {e}"));
            assert!(scan.past.is_empty(), "{at}: a chain past its size {:?}", scan.past);
            assert!(scan.lost <= 1 && scan.apart <= 1, "{at}: {} lost, {} apart", scan.lost, scan.apart);
            worst = (worst.0.max(scan.lost), worst.1.max(scan.apart));
            // a restart on the card: nothing to resume, the ride where it was, the PR file as read above
            let mut sim2 = Sim::new(9, 5, 0);
            sim2.emmc.image = img;
            sim2.emmc.rel_param = crate::emmc::EN_REL_WR;
            let mut app2 = App::start(&mut sim2, None);
            run(&mut sim2, &mut app2, 10_000);
            assert_eq!(app2.ride.state(), ride::State::Idle, "{at}");
            assert_eq!(super::recorder_tests::places(&sim2, name)[0], ["Activities"], "{at}");
            assert_eq!(pr(&sim2.emmc.image), got, "{at}");
        }
        std::println!(
            "effort write (a PR file before: {had}): {total} sectors, {olds} cuts read the old, {news} the new, worst (lost, apart) {worst:?}"
        );
        assert!(total > 0 && news > 0 && (olds > 0 || !had));
    }
}
