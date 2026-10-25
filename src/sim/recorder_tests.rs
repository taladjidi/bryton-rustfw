//! The ride recorder (recorder.rs) in the app, on the simulated card with the synthetic volume.

use super::*;

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

/// Start from a key; the unit reset mid-ride (the card and the backup registers kept): at the start the ride
/// comes back paused at its last checkpoint; Start goes on, a long Start ends it, and it is saved in
/// `\Activities` under stock's name, where the history finds it. The volume stays clean.
#[test]
fn a_ride_reset_resumed_and_saved() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 150_000);
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "a ride open on the card");
    let timer = app.ride.timer_s();
    let writes = sim.emmc.cmds.iter().filter(|c| c.0 == 24 || c.0 == 25).count();
    std::println!("{timer} s of ride: {writes} write commands, {} sectors", sim.emmc.sectors_written);

    // a reset 30 s after the second checkpoint
    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notices[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notices[0].as_str());
    assert_eq!(app.ride.state(), ride::State::Paused);
    let back = app.ride.timer_s();
    assert!(back <= timer && back + 60 >= timer, "timer {back} of {timer}");
    assert_eq!(app.ride_started, ((2026, 10, 23), (9, 0)));

    app.press(&mut sim2, Press::Short(Key::Start)); // resume
    run(&mut sim2, &mut app, 30_000);
    app.press(&mut sim2, Press::Short(Key::Start)); // pause
    save_ride(&mut sim2, &mut app); // end
    run(&mut sim2, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());
    assert_eq!(sim2.backup[RIDE_BKP], 0);
    let lost = crate::fat::write::tests::fsck(&sim2.emmc.image).unwrap();
    assert_eq!(lost, 0);

    // the history lists it first (the newest), with its time
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim2, &mut app, u64::from(POLL_MS));
    }
    let r = app.stock.index.get(0).unwrap();
    assert_eq!(r.start, (2026, 10, 23, 9, 0));
    let total = app.ride.timer_s();
    assert!(r.timer_s.abs_diff(total) <= 2, "the file's timer {} s, the ride's {total} s", r.timer_s);
}

/// A build that may not write the card (the unit's, until EMMC_WRITE=1): a ride says NOT RECORDING once and the
/// card gets no command at all during it (the recorder used to read both FATs and retry every 5 s).
#[test]
fn read_only_build_does_not_touch_the_card() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.writable = false;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    let cmds = sim.emmc.cmds.len();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    assert!(app.notices[0].as_str().ends_with("NOT RECORDING"), "{}", app.notices[0].as_str());
    assert_eq!(sim.emmc.cmds.len(), cmds, "no card work during the ride");
}

/// The record times of the ride file at `path` on the volume `img`.
fn record_times(img: &[u8], path: &str) -> Vec<u32> {
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, path).unwrap();
    let mut v = std::vec![0u8; f.size as usize];
    fat.read_at(&mut d, &f, 0, &mut v).unwrap();
    let (mut times, mut rd) = (Vec::new(), crate::fitread::Reader::new());
    rd.push_slice(&v, |m| {
        if let crate::fitread::Msg::Record(r) = m {
            times.push(r.time);
        }
    });
    assert_eq!(rd.result(), Some(Ok(())), "{path}: a whole FIT file");
    times
}

/// The unit after a reset or a power cycle: the card and the backup registers kept, the clock at `m` minutes.
fn reboot(sim: &mut Sim, m: u64) -> (Sim, App) {
    let mut sim2 = Sim::new(9, m, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    let app = App::start(&mut sim2, None);
    (sim2, app)
}

/// The second review's R4: switching off mid-ride (the second hold of Back) used to lose up to a minute, the
/// seconds since the last checkpoint. It checkpoints first: the ride comes back at the second it was switched off.
#[test]
fn off_mid_ride_keeps_every_second() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000); // a checkpoint at 60 s, then 30 s in RAM only
    let timer = app.ride.timer_s();
    app.press(&mut sim, Press::Long(Key::Back)); // warns
    app.press(&mut sim, Press::Long(Key::Back)); // off
    assert!(sim.standby);
    let (mut sim, mut app) = reboot(&mut sim, 5);
    run(&mut sim, &mut app, 1000);
    assert!(app.notices[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notices[0].as_str());
    assert!(app.ride.timer_s().abs_diff(timer) <= 1, "timer {} of {timer}", app.ride.timer_s());
}

/// The second review's R5: the seconds caught up after a long pass (here 3 s) used to all take the clock's
/// time, so the recorder pushed them later and the file ran 2 s ahead until the end. Each has its own time now.
#[test]
fn seconds_caught_up_keep_their_own_time() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = crate::recorder::ride_name(app.rec_time().0);
    run(&mut sim, &mut app, 5000);
    sim.delay_ms(3000); // a pass that took 3 s
    run(&mut sim, &mut app, 5000);
    let (_, now) = app.rec_time();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    let times = record_times(&sim.emmc.image, &path);
    assert!(times.windows(2).all(|p| p[1] == p[0] + 1), "one record a second: {times:?}");
    assert_eq!(times.last().copied(), Some(now), "the last record at the clock's time");
    // the full copy (developer fields): the same records, in \Rustfw\Activities under the same name
    assert_eq!(record_times(&sim.emmc.image, &std::format!("Rustfw/{path}")), times);
}

/// A full copy that cannot be made (here a file in the way of its folder) is said once, and the ride is recorded
/// and saved all the same: the clean copy never waits for the full one.
#[test]
fn a_full_copy_that_fails_never_stops_the_ride() {
    let mut sim = Sim::new(9, 0, 0);
    let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let dir = fat.ensure_dir(&mut d, fat.root(), "Rustfw", (0, 0)).unwrap();
    fat.create(&mut d, dir, "Full", (0, 0)).unwrap();
    sim.emmc.image = d.0;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = crate::recorder::ride_name(app.rec_time().0);
    run(&mut sim, &mut app, 90_000);
    assert!(app.notices.iter().any(|n| n.as_str().ends_with("NO FULL COPY")));
    assert_ne!(app.strip.rec, Rec::NotSaving);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());
    assert_eq!(sim.backup[RIDE_BKP], 0, "nothing left open");
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    assert!(record_times(&sim.emmc.image, &path).len() >= 89);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// The second review's R9: a recorder that fails looked like one that works (the red dot, and the warning only at
/// the 2nd, 4th, 8th... failure of the whole run). The strip's dot is grey while it is not saving, NOT SAVING is
/// said at the first failure (then every 5 minutes of them), and all is back once the card answers.
#[test]
fn a_failing_recorder_shows() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_ne!(app.strip.rec, Rec::NotSaving);
    sim.emmc.dead = true; // the card stops answering
    run(&mut sim, &mut app, 70_000); // the checkpoint at 60 s fails
    assert_eq!(app.strip.rec, Rec::NotSaving);
    assert!(app.notices[0].as_str().ends_with("NOT SAVING"), "{}", app.notices[0].as_str());
    sim.emmc.dead = false;
    run(&mut sim, &mut app, 10_000); // tried again after 5 s
    assert_ne!(app.strip.rec, Rec::NotSaving);
}

/// The second review's R2 in the app: the card failed at the boot, which had a ride to reopen, and Start was
/// pressed before it answered again. The ride left open is reopened first (in place of the new one: the marker
/// said it was open), and saved whole at the end; never two rides in one writer.
#[test]
fn start_while_the_card_failed_at_boot() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    let timer = app.ride.timer_s();
    sim.emmc.dead = true;
    let (mut sim, mut app) = reboot(&mut sim, 3);
    sim.emmc.dead = true;
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    sim.emmc.dead = false;
    run(&mut sim, &mut app, 10_000);
    assert!(app.notices.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")), "{:?}", app.notices[0].as_str());
    assert_eq!((app.ride.state(), app.ride_started), (ride::State::Paused, ((2026, 10, 23), (9, 0))));
    assert!(app.ride.timer_s() + 60 >= timer);
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// Reliable write (emmc.rs) on a card that offers it: every write is CMD23 with bit 31 then CMD25, never a plain
/// one. So a cut tears nothing: at every sector of a ride's first 75 s, the reboot finds a clean volume and the
/// ride at its last checkpoint (the root directory's sector included, which a plain write could lose).
#[test]
fn every_write_is_reliable_when_the_card_offers_it() {
    let start = |cut: Option<u64>| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        (sim.emmc.rel_param, sim.emmc.tear, sim.emmc.cut_after) = (crate::emmc::EN_REL_WR, true, cut);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1000);
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 75_000);
        (sim, app)
    };
    let (sim, _) = start(None);
    let cmds = &sim.emmc.cmds;
    assert!(!cmds.iter().any(|c| c.0 == 24), "a plain single-sector write");
    for (k, _) in cmds.iter().enumerate().filter(|c| c.1.0 == 25) {
        assert!(k > 0 && cmds[k - 1].0 == 23 && cmds[k - 1].1 & 1 << 31 != 0, "CMD25 without a reliable CMD23");
    }
    let total = sim.emmc.sectors_written;
    let (mut resumed, mut first) = (0, None);
    for cut in 0..total {
        let (mut sim, _) = start(Some(cut));
        let (mut sim, mut app) = reboot(&mut sim, 2);
        run(&mut sim, &mut app, 1000);
        let back = app.notices[0].as_str().ends_with("RIDE RESUMED");
        resumed += usize::from(back);
        // once the ride's first checkpoint is on the card, every later cut resumes it: a cut that does not is a
        // ride lost (a cut before it finds no ride to resume, which is right: nothing was saved yet)
        match (first, back) {
            (None, true) => first = Some(cut),
            (Some(f), false) => panic!("cut {cut}: the ride not resumed, though cut {f} resumed it"),
            _ => {}
        }
        let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
    }
    std::println!("{total} cuts, {resumed} resumed from cut {first:?}");
    assert!(resumed > 0);
}

// ---------------------------------------------------------------------------------------------------------------
// The end-ride screen (app/riding.rs `end_input`): RESUME, SAVE, DISCARD
// ---------------------------------------------------------------------------------------------------------------

/// A ride of 70 s (a checkpoint on the card), paused, and Start held: the end-ride screen, RESUME selected, the ride
/// paused.
fn to_end_screen(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 70_000);
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 1000);
    app.press(sim, Press::Long(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::End(0), ride::State::Paused), "the end-ride screen");
}

/// The ride's file `dir/<name>.fit` on the simulated card, if any.
pub(super) fn ride_file(sim: &Sim, dir: &str, name: [u8; 12]) -> Option<Vec<u8>> {
    let path = std::format!("{dir}/{}.fit", core::str::from_utf8(&name).unwrap());
    crate::storage::trash::tests::file(&sim.emmc.image, &path)
}

/// Where each copy of ride `name` is: (clean, full), each the folders it is in.
pub(super) fn places(sim: &Sim, name: [u8; 12]) -> [Vec<&'static str>; 2] {
    let at = |dirs: [&'static str; 4]| dirs.into_iter().filter(|d| ride_file(sim, d, name).is_some()).collect();
    [
        at(["Activities", "Rustfw/Rides", "Rustfw/Discarded", "Rustfw/Deleted"]),
        at(["Rustfw/Activities", "Rustfw/Full", "Rustfw/Discarded/Full", "Rustfw/Deleted/Full"]),
    ]
}

/// The history's scan to its end: the rides it lists and its totals.
fn scanned(sim: &mut Sim, app: &mut App) -> (usize, crate::stock::Summary) {
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(sim, app, u64::from(POLL_MS));
    }
    (app.stock.index.len(), crate::stock::Summary { bytes: 0, ..app.stock.out })
}

/// The end-ride screen by the keys: Page and Lap move (no wrap), Back and the default (RESUME) leave the ride paused as
/// it was, on its page; SAVE is today's end: both copies saved, the history lists it, and the card stays until Back
/// or Start. Screenshot: end_ride.
#[test]
fn end_ride_screen_by_keys() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    let before = scanned(&mut sim, &mut app).0;
    to_end_screen(&mut sim, &mut app);
    run(&mut sim, &mut app, 200);
    super::tests::assert_consistent(&sim, &app);
    sim.save("end_ride");
    let timer = app.ride.timer_s();
    for (k, want) in [(Key::Lap, 0), (Key::Page, 1), (Key::Page, 2), (Key::Page, 2), (Key::Lap, 1)] {
        app.press(&mut sim, Press::Short(k));
        assert_eq!(app.screen(), Screen::End(want), "{k:?}");
    }
    run(&mut sim, &mut app, 200);
    super::tests::assert_consistent(&sim, &app); // only the two buttons drawn again, the panel as the screen
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Paused), "Back: RESUME");
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.timer_s(), timer, "still paused");
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Paused), "the default: RESUME");
    // SAVE
    let name = app.recorder.name();
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Summary, ride::State::Idle), "SAVE: the card");
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE SAVED"), "{}", app.notices[0].as_str());
    assert_eq!(places(&sim, name), [["Activities"], ["Rustfw/Activities"]]);
    assert_eq!(sim.backup[RIDE_BKP], 0);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    for k in [Key::Page, Key::Lap] {
        app.press(&mut sim, Press::Short(k));
        assert_eq!(app.screen(), Screen::Summary, "the card stays: {k:?}");
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Main, "Start: the overview");
    assert_eq!(scanned(&mut sim, &mut app).0, before + 1, "in the history");
}

/// DISCARD asks once more (KEEP selected, Back keeps), then ends the ride and moves both copies into
/// `\Rustfw\Discarded`: the history, its totals and the curve as they were, nothing in `\Activities`, the volume
/// clean; the next ride is saved as ever. Screenshot: end_ride_discard.
#[test]
fn discard_by_keys_moves_both_copies() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    let before = scanned(&mut sim, &mut app);
    to_end_screen(&mut sim, &mut app);
    let name = app.recorder.name();
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Discard(2), "asked, KEEP selected");
    run(&mut sim, &mut app, 200);
    super::tests::assert_consistent(&sim, &app);
    sim.save("end_ride_discard");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::End(2), "Back: KEEP");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::End(2), ride::State::Paused), "KEEP");
    app.press(&mut sim, Press::Short(Key::Start));
    for (k, want) in [(Key::Lap, 0), (Key::Lap, 0), (Key::Page, 2), (Key::Page, 2), (Key::Lap, 0)] {
        app.press(&mut sim, Press::Short(k));
        assert_eq!(app.screen(), Screen::Discard(want), "{k:?}: the gap skipped");
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Main, ride::State::Idle), "discarded");
    run(&mut sim, &mut app, 2000);
    assert!(app.notices[0].as_str().ends_with("RIDE DISCARDED"), "{}", app.notices[0].as_str());
    assert_eq!(places(&sim, name), [["Rustfw/Discarded"], ["Rustfw/Discarded/Full"]]);
    assert_eq!(sim.backup[RIDE_BKP], 0);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    assert_eq!(scanned(&mut sim, &mut app), before, "the history and its totals as they were");
    // the next ride is recorded and saved as ever
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    let next = app.recorder.name();
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!(places(&sim, next), [["Activities"], ["Rustfw/Activities"]]);
}

/// The end-ride screen by touch: a tap acts on the button under it at once (the screen is the confirmation): RESUME,
/// KEEP, DISCARD and SAVE; a tap between the buttons does nothing; a swipe right is Back.
#[test]
fn end_ride_screen_by_touch() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    let tap = |sim: &mut Sim, app: &mut App, k: usize| {
        let (x, y, w, h) = ui::END_BUTTONS[k];
        super::tests::drag(sim, app, ((x + w / 2) as i32, (y + h / 2) as i32), (0, 0));
    };
    to_end_screen(&mut sim, &mut app);
    let name = app.recorder.name();
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::END_BUTTONS[1].1 - 6) as u16)));
    assert_eq!(app.screen(), Screen::End(0), "between two buttons: nothing");
    tap(&mut sim, &mut app, 0);
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Paused), "RESUME");
    app.press(&mut sim, Press::Long(Key::Start));
    app.input(&mut sim, Input::Touch(Gesture::Swipe(crate::touch::Dir::Right)));
    assert_eq!(app.screen(), Screen::Ride(0), "a swipe right: Back");
    app.press(&mut sim, Press::Long(Key::Start));
    tap(&mut sim, &mut app, 2);
    assert_eq!(app.screen(), Screen::Discard(2), "DISCARD asks");
    tap(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Discard(2), "the text between: nothing");
    tap(&mut sim, &mut app, 2);
    assert_eq!((app.screen(), app.ride.state()), (Screen::End(2), ride::State::Paused), "KEEP, where DISCARD was");
    tap(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Summary, "SAVE");
    run(&mut sim, &mut app, 2000);
    assert_eq!(places(&sim, name), [["Activities"], ["Rustfw/Activities"]]);
    app.press(&mut sim, Press::Short(Key::Back));
    // a second ride, discarded by two taps
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    let name = app.recorder.name();
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    tap(&mut sim, &mut app, 2);
    tap(&mut sim, &mut app, 0);
    assert_eq!((app.screen(), app.ride.state()), (Screen::Main, ride::State::Idle), "DISCARD");
    run(&mut sim, &mut app, 2000);
    assert_eq!(places(&sim, name), [["Rustfw/Discarded"], ["Rustfw/Discarded/Full"]]);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A reset while the end-ride screen (or its question) is up: the ride comes back paused at its last checkpoint, as
/// from a page, every second of it (the pause was a checkpoint), and is then saved as ever.
#[test]
fn a_reset_on_the_end_screen_resumes_the_ride() {
    for question in [false, true] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 2000);
        to_end_screen(&mut sim, &mut app);
        let (name, timer) = (app.recorder.name(), app.ride.timer_s());
        if question {
            app.press(&mut sim, Press::Short(Key::Page));
            app.press(&mut sim, Press::Short(Key::Page));
            app.press(&mut sim, Press::Short(Key::Start));
            assert_eq!(app.screen(), Screen::Discard(2));
        }
        run(&mut sim, &mut app, 3000);
        assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "the ride open");
        let (mut sim, mut app) = reboot(&mut sim, 5);
        run(&mut sim, &mut app, 1000);
        assert!(app.notices[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notices[0].as_str());
        assert_eq!(app.ride.state(), ride::State::Paused);
        assert_eq!(app.ride.timer_s(), timer, "every second: the pause was a checkpoint");
        save_ride(&mut sim, &mut app);
        run(&mut sim, &mut app, 2000);
        assert_eq!(places(&sim, name), [["Activities"], ["Rustfw/Activities"]], "question {question}");
    }
}
