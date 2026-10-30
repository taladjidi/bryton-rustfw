//! The ride recorder (storage/recorder/mod.rs) in the app, on the simulated card with the synthetic volume.

use super::*;

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
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert_eq!(app.ride.state(), ride::State::Paused);
    let back = app.ride.timer_s();
    assert!(back <= timer && back + 60 >= timer, "timer {back} of {timer}");
    assert_eq!(app.ride_started, RideStart::new((2026, 10, 23), (9, 0)));

    app.press(&mut sim2, Press::Short(Key::Start)); // resume
    run(&mut sim2, &mut app, 30_000);
    app.press(&mut sim2, Press::Short(Key::Start)); // pause
    save_ride(&mut sim2, &mut app); // end
    run(&mut sim2, &mut app, 2000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
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
/// card gets no command at all during it (the recorder used to read both FATs and retry every 5 s), once the
/// rider's files are read after the start (pages, pairing, bond, altitude: read, never written).
#[test]
fn read_only_build_does_not_touch_the_card() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.writable = false;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    let cmds = sim.emmc.cmds.len();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 20_000);
    assert!(app.notes.list[0].as_str().ends_with("NOT RECORDING"), "{}", app.notes.list[0].as_str());
    assert_eq!(sim.emmc.cmds.len(), cmds, "no card work during the ride");
    assert!(sim.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
}

/// A read-only build (a RAM run) that boots with a ride left open never clears its marker; a writable build then
/// resumes the ride.
#[test]
fn a_read_only_build_leaves_the_open_ride_marker() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN);
    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    sim2.writable = false;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, 60_000);
    assert_eq!(sim2.backup[RIDE_BKP], RIDE_OPEN, "the marker kept");
    let (mut sim3, mut app) = reboot(&mut sim2, 5);
    run(&mut sim3, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
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

/// An indoor ride (set by hand, no trainer: nothing but the recovery record can say so after the reset) comes back
/// indoor after a reset, and is saved with sub_sport indoor_cycling (6) in its laps and session; its full copy too.
#[test]
fn an_indoor_ride_resumes_indoor_and_saves_as_indoor_cycling() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(!app.indoors(), "no trainer: a road ride");
    app.toggle_indoor();
    run(&mut sim, &mut app, 150_000);
    let (mut sim2, mut app) = reboot(&mut sim, 3);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert!(app.indoors(), "indoor again, from the record");
    app.press(&mut sim2, Press::Short(Key::Start)); // resume
    run(&mut sim2, &mut app, 30_000); // under TRAINER_QUIET_S: still indoor
    app.press(&mut sim2, Press::Short(Key::Start));
    save_ride(&mut sim2, &mut app);
    run(&mut sim2, &mut app, 2000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    let name = std::string::String::from_utf8(app.rec.clean.name().to_vec()).unwrap();
    let field = crate::fit::tests::field;
    for path in [std::format!("Activities/{name}.fit"), std::format!("Rustfw/Activities/{name}.fit")] {
        let mut d = crate::fat::tests::Ram(sim2.emmc.image.clone(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, &path).unwrap();
        let mut v = std::vec![0u8; f.size as usize];
        fat.read_at(&mut d, &f, 0, &mut v).unwrap();
        let m = crate::fit::tests::decode(&v).unwrap();
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!((field(session, 5), field(session, 6)), (Some(2), Some(6)), "{path}: cycling, indoor_cycling");
        assert!(m.iter().filter(|x| x.0 == 19).all(|l| field(l, 39) == Some(6)), "{path}: the laps");
    }
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
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
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
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("NO FULL COPY")));
    assert_ne!(app.strip.rec, Rec::NotSaving);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
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
    assert!(app.notes.list[0].as_str().ends_with("NOT SAVING"), "{}", app.notes.list[0].as_str());
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
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")), "{:?}", app.notes.list[0].as_str());
    assert_eq!((app.ride.state(), app.ride_started), (ride::State::Paused, RideStart::new((2026, 10, 23), (9, 0))));
    assert!(app.ride.timer_s() + 60 >= timer);
    save_ride(&mut sim, &mut app); // end
    run(&mut sim, &mut app, 2000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
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
        let back = app.notes.list[0].as_str().ends_with("RIDE RESUMED");
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

/// A ride of 70 s (a checkpoint on the card), paused, and Start held: the end-ride screen, SAVE selected, the ride
/// paused.
fn to_end_screen(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 70_000);
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 1000);
    app.press(sim, Press::Long(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::End(1), ride::State::Paused), "the end-ride screen");
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

/// The end-ride screen by the keys: Page and Lap move (no wrap), Back leaves the ride paused on its page, the default
/// (RESUME) runs it again; SAVE is today's end: both copies saved, the history lists it, and the card stays until Back
/// or Start. Screenshots: end_ride, summary.
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
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Paused), "Back: paused still");
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.timer_s(), timer, "still paused");
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Lap)); // SAVE is the default; RESUME is one up
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Running), "RESUME");
    app.press(&mut sim, Press::Short(Key::Start)); // paused again
    // SAVE
    let name = app.rec.clean.name();
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.screen(), app.ride.state()), (Screen::Summary, ride::State::Idle), "SAVE: the card, 2 keys, was 3");
    run(&mut sim, &mut app, 2000);
    sim.save("summary");
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
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
    let name = app.rec.clean.name();
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
    assert!(app.notes.list[0].as_str().ends_with("RIDE DISCARDED"), "{}", app.notes.list[0].as_str());
    assert_eq!(places(&sim, name), [["Rustfw/Discarded"], ["Rustfw/Discarded/Full"]]);
    assert_eq!(sim.backup[RIDE_BKP], 0);
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    assert_eq!(scanned(&mut sim, &mut app), before, "the history and its totals as they were");
    // the next ride is recorded and saved as ever
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    let next = app.rec.clean.name();
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
    let name = app.rec.clean.name();
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, (ui::END_BUTTONS[1].1 - 6) as u16)));
    assert_eq!(app.screen(), Screen::End(1), "between two buttons: nothing");
    tap(&mut sim, &mut app, 0);
    assert_eq!((app.screen(), app.ride.state()), (Screen::Ride(0), ride::State::Running), "RESUME: the timer again");
    app.press(&mut sim, Press::Short(Key::Start)); // paused again
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
    super::tests::back(&mut sim, &mut app);
    // a second ride, discarded by two taps
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    let name = app.rec.clean.name();
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
        let (name, timer) = (app.rec.clean.name(), app.ride.timer_s());
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
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
        assert_eq!(app.ride.state(), ride::State::Paused);
        assert_eq!(app.ride.timer_s(), timer, "every second: the pause was a checkpoint");
        save_ride(&mut sim, &mut app);
        run(&mut sim, &mut app, 2000);
        assert_eq!(places(&sim, name), [["Activities"], ["Rustfw/Activities"]], "question {question}");
    }
}

/// Review of the ride profile: a ride set INDOOR by hand (no trainer heard: a dumb trainer, rollers) stays INDOOR
/// after a reset. The recovery record keeps "by hand": without it the resumed ride, automatic again, went back to
/// ROAD after TRAINER_QUIET_S without a trainer.
#[test]
fn an_indoor_ride_by_hand_stays_indoor_after_a_reset() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.toggle_indoor();
    run(&mut sim, &mut app, 300_000);
    assert!(app.indoors(), "by hand, before the reset: stays indoor");
    let (mut sim2, mut app) = reboot(&mut sim, 3);
    run(&mut sim2, &mut app, 1000);
    assert!(app.indoors());
    app.press(&mut sim2, Press::Short(Key::Start));
    run(&mut sim2, &mut app, 150_000);
    assert!(app.indoors(), "after the reset, 150 s on: {}", app.notes.list[0].as_str());
}

/// Review of the ride profile: samples come only on running seconds, so INDOOR set while paused, then SAVE, left
/// the session with the last running second's profile. The profile in force goes to both copies at the end.
#[test]
fn a_profile_set_while_paused_is_in_the_saved_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    run(&mut sim, &mut app, 2000);
    app.toggle_indoor();
    assert!(app.indoors());
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let name = std::string::String::from_utf8(app.rec.clean.name().to_vec()).unwrap();
    for path in [std::format!("Activities/{name}.fit"), std::format!("Rustfw/Activities/{name}.fit")] {
        let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, &path).unwrap();
        let mut v = std::vec![0u8; f.size as usize];
        fat.read_at(&mut d, &f, 0, &mut v).unwrap();
        let m = crate::fit::tests::decode(&v).unwrap();
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(crate::fit::tests::field(session, 6), Some(6), "{path}: indoor at the end, set while paused");
    }
}

/// The same for a ride switched off while paused: INDOOR set by hand during the pause comes back after the restart
/// (the checkpoint at the switch off carries the profile in force, not the last running second's).
#[test]
fn a_profile_set_while_paused_survives_a_switch_off() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    run(&mut sim, &mut app, 2000);
    app.toggle_indoor();
    app.power_off(&mut sim);
    let (mut sim2, mut app) = reboot(&mut sim, 3);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert!(app.indoors(), "indoor, set while paused, from the record");
    app.press(&mut sim2, Press::Short(Key::Start));
    run(&mut sim2, &mut app, 150_000);
    assert!(app.indoors(), "and by hand: no trainer, still indoor");
}

/// A ride resumed after a restart takes the CP and W' available at its resume (the backup registers have no room
/// for the pair it had, app/fitness.rs): here the settings', since this start has no scan yet, though the ride began
/// on the fit's; and keeps them to its end, the scan finishing meanwhile.
#[test]
fn a_resumed_ride_takes_the_model_available_at_resume() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 60_000);
    }
    app.settings.ftp = 250; // the fit's CP is 250 W: under stock's 265 W it would not be taken
    app.save_misc(&mut sim);
    let settings = (app.settings.ftp, app.settings.wprime_j);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.rec.model.map(|m| m.2), Some(true), "this ride on the fit");
    assert_ne!(app.cp_model(), settings);
    run(&mut sim, &mut app, 150_000);

    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")));
    assert_eq!(app.rec.model, Some((settings.0, settings.1, false)), "what this start had at the resume");
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim2, &mut app, u64::from(POLL_MS));
        assert!(sim2.ms < 120_000);
    }
    assert!(app.cp_fit().is_some_and(|c| c.trusted()));
    assert_eq!(app.cp_model(), settings, "kept to the ride's end");
}

/// A strap beating through a ride (reset mid-ride, resumed, saved): the full copy holds its beats as the strap sent
/// them, in `hrv` messages, each run of them unbroken and in the strap's order; the clean copy none.
#[test]
fn the_strap_beats_go_into_the_full_copy() {
    use super::tests::{KEY, STRAP, beat_ticks, strap_beats};
    let strap = |sim: &mut Sim| {
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor {
            device_type: 0x78,
            id: STRAP.device,
            data: strap_beats,
            network: 0,
            ack: &[],
        });
    };
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    strap(&mut sim);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 20_000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 150_000);
    assert_eq!(app.rec.full.beats_dropped, 0);
    let name = app.rec.clean.name();

    let mut sim2 = Sim::new(9, 3, 0); // a reset
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    strap(&mut sim2);
    let mut app = App::start(&mut sim2, Some(KEY));
    run(&mut sim2, &mut app, 1000);
    app.press(&mut sim2, Press::Short(Key::Start));
    run(&mut sim2, &mut app, 30_000);
    app.press(&mut sim2, Press::Short(Key::Start));
    save_ride(&mut sim2, &mut app);
    run(&mut sim2, &mut app, 3000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());

    let rr = |dir| {
        let f = ride_file(&sim2, dir, name).unwrap();
        let m = crate::fit::tests::decode(&f).unwrap();
        m.iter().filter(|x| x.0 == 78).flat_map(|x| x.1.iter().map(|f| f.1 as u16)).collect::<Vec<_>>()
    };
    assert!(rr("Activities").is_empty(), "the clean copy has no beats");
    let got = rr("Rustfw/Activities");
    // the strap's beats in ms (the simulated strap starts again from its first after the reset); the file's are runs
    // of them, unbroken and in order: the longest run from each point, two at most (before and after the reset)
    let sent: Vec<u16> = (1..2000).map(|k| ((beat_ticks(k) * 1000 + 512) >> 10) as u16).collect();
    let got: Vec<u16> = got.into_iter().filter(|&v| v != 0xffff).collect();
    let (mut p, mut runs) = (0, Vec::new());
    while p < got.len() {
        let run = |s: usize| sent[s..].iter().zip(&got[p..]).take_while(|(a, b)| a == b).count();
        let longest = (0..sent.len()).map(run).max().unwrap();
        assert!(longest > 0, "beat {p} not the strap's");
        runs.push(longest);
        p += longest;
    }
    assert!(runs.len() <= 2 && runs.iter().all(|&r| r >= 20), "{runs:?}"); // the strap beats from 10 s
    // about 150 s before the reset (less the minute at most after its last checkpoint) and 30 s after, at 0.8 s
    assert!((100..250).contains(&got.len()), "{} beats", got.len());
    std::println!("{} beats in runs of {runs:?}", got.len());
}

/// The lap messages (FIT global 19) in a ride file: a walk of its records (definitions, data, developer fields).
fn lap_count(file: &[u8]) -> usize {
    let (mut i, mut laps) = (usize::from(file[0]), 0);
    let mut defs: [(u16, usize); 16] = [(0, 0); 16];
    while i + 2 < file.len() {
        let h = file[i];
        let local = usize::from(h & 15);
        if h & 0x40 != 0 {
            let global = u16::from_le_bytes([file[i + 3], file[i + 4]]);
            let n = usize::from(file[i + 5]);
            let mut size: usize = (0..n).map(|k| usize::from(file[i + 6 + 3 * k + 1])).sum();
            let mut next = i + 6 + 3 * n;
            if h & 0x20 != 0 {
                let d = usize::from(file[next]);
                size += (0..d).map(|k| usize::from(file[next + 1 + 3 * k + 1])).sum::<usize>();
                next += 1 + 3 * d;
            }
            defs[local] = (global, size);
            i = next;
        } else {
            let (global, size) = defs[local];
            laps += usize::from(global == 19);
            i += 1 + size;
        }
    }
    laps
}

/// A workout started during a ride: a lap at its start (the ride before it), one at each step change and at its end,
/// as Garmin and Wahoo record them; Lap during the workout moves it on with a single lap, not two.
#[test]
fn a_workout_records_a_lap_per_step() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride, before any workout
    run(&mut sim, &mut app, 10_000);
    app.load_calendar(&mut sim);
    app.open_workout(&mut sim, 0); // 20, 30, 15, 30, 15 s, then an open cool-down
    app.start_workout(0);
    run(&mut sim, &mut app, 115_000); // through the five timed steps, into the cool-down
    app.press(&mut sim, Press::Short(Key::Lap)); // ends the open cool-down: the workout done, one lap
    run(&mut sim, &mut app, 5000);
    let name = app.rec.clean.name();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let file = ride_file(&sim, "Activities", name).expect("saved");
    // before the workout, the five timed steps, the cool-down up to Lap, after it to the end
    assert_eq!(lap_count(&file), 8);
}

/// AUTO OFF never switches off while the recorder holds a ride: a save that keeps failing holds it (the recorder
/// itself does not retry after a failed end: the rider's GIVE UP SAVE closes it).
#[test]
fn auto_off_waits_for_the_recorder() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.settings.ready_check = false; // these tests are about what the start does, not the card
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN);
    app.settings.auto_off = 1;
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    // the rides folders fail to write (as drive_tests.rs's GIVE UP SAVE): the save is stuck after its retries
    let dirs = ["Activities", "Rustfw/Rides"].map(|p| crate::sim::drive_tests::dir_sectors(&sim.emmc.image, p));
    sim.emmc.bad = std::vec![dirs[0].clone()];
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 1000); // the first folder's write fails, the next step too
    sim.emmc.bad = dirs.to_vec();
    run(&mut sim, &mut app, 300_000);
    assert!(app.rec.clean.stuck());
    app.press(&mut sim, Press::Short(Key::Start)); // the End card closes
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "a save stuck after its screen closed");
    // the card comes back; a new ride is held behind the stuck save, then the rider gives up the save
    // (SYSTEM > GIVE UP SAVE: asked, then sure, then YES), as in drive_tests.rs
    sim.emmc.bad.clear();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert!(app.rec.clean.stuck() && app.rec.clean.held());
    app.open(Screen::Menu(Menu::System, 0));
    app.show(&mut sim);
    for _ in 0..5 {
        app.input(&mut sim, Input::Select);
    }
    run(&mut sim, &mut app, 30_000);
    crate::sim::tests::back(&mut sim, &mut app); // out of SYSTEM
    crate::sim::tests::back(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start)); // the held ride, pause, then saved
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 30_000);
    assert_eq!(sim.backup[RIDE_BKP], 0, "no ride open once the held one is saved");
    app.press(&mut sim, Press::Short(Key::Back)); // the banner
    app.press(&mut sim, Press::Short(Key::Back)); // the summary closes: the overview, idle
    // 5 min of idle (AUTO OFF 5) plus a margin for the second-by-second count
    run(&mut sim, &mut app, 360_000);
    assert!(sim.standby, "AUTO OFF 5 did not switch off within its time");
    assert_eq!(sim.backup[RIDE_BKP], 0);
}

/// Lap (19) and session (18) data messages of a ride file as (global, [(field number, value)]), little endian.
fn msg_fields(file: &[u8]) -> Vec<(u16, Vec<(u8, u32)>)> {
    let (mut i, mut out) = (usize::from(file[0]), Vec::new());
    #[allow(clippy::type_complexity)]
    let mut defs: [(u16, Vec<(u8, usize)>, usize); 16] = Default::default();
    while i + 2 < file.len() {
        let h = file[i];
        let local = usize::from(h & 15);
        if h & 0x40 != 0 {
            let global = u16::from_le_bytes([file[i + 3], file[i + 4]]);
            let n = usize::from(file[i + 5]);
            let f: Vec<(u8, usize)> =
                (0..n).map(|k| (file[i + 6 + 3 * k], usize::from(file[i + 6 + 3 * k + 1]))).collect();
            let mut size: usize = f.iter().map(|x| x.1).sum();
            let mut next = i + 6 + 3 * n;
            if h & 0x20 != 0 {
                let d = usize::from(file[next]);
                size += (0..d).map(|k| usize::from(file[next + 1 + 3 * k + 1])).sum::<usize>();
                next += 1 + 3 * d;
            }
            defs[local] = (global, f, size);
            i = next;
        } else {
            let (global, f, size) = &defs[local];
            if matches!(global, 18 | 19) {
                let mut at = i + 1;
                let v = f
                    .iter()
                    .map(|&(num, len)| {
                        let mut b = [0u8; 4];
                        b[..len.min(4)].copy_from_slice(&file[at..at + len.min(4)]);
                        at += len;
                        (num, u32::from_le_bytes(b))
                    })
                    .collect();
                out.push((*global, v));
            }
            i += 1 + size;
        }
    }
    out
}

/// AUTO LAP BY DISTANCE 5 km on a ride of 11 km at 28.8 km/h: laps fire at 5 and 10 km exactly as the Lap key does
/// (the same recorder event), so the FIT file holds the two laps and the one that closes the ride; each lap's
/// summary is in the banner (screenshot `auto_lap`).
#[test]
fn auto_lap_by_distance_writes_a_lap_message_per_5_km() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.gnss.route = Some(super::map_tests::ride_route);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.settings.auto_lap = 1;
    app.press(&mut sim, Press::Short(Key::Start));
    let mut shot = false;
    while app.ride.dist_m() < 11_000 {
        run(&mut sim, &mut app, 1000);
        if app.ride.laps() == 1 && !shot {
            shot = true;
            run(&mut sim, &mut app, 1000);
            assert!(app.notes.text.as_str().starts_with("LAP 1 "), "{}", app.notes.text.as_str());
            assert_eq!(app.notes.kind, Banner::Summary);
            sim.save("auto_lap");
        }
    }
    assert_eq!(app.ride.laps(), 2, "laps at 5 and 10 km");
    let name = app.rec.clean.name();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    let file = ride_file(&sim, "Activities", name).expect("saved");
    assert_eq!(lap_count(&file), 3, "the two auto laps and the last, closed at the save");
    // each lap's total_timer_time (8, ms) and total_distance (9, cm): 5 km each at 8 m/s, and the three make the session
    let msgs = msg_fields(&file);
    let get = |m: &(u16, Vec<(u8, u32)>), n: u8| m.1.iter().find(|f| f.0 == n).map(|f| f.1).unwrap();
    let laps: Vec<_> = msgs.iter().filter(|m| m.0 == 19).collect();
    let session = msgs.iter().find(|m| m.0 == 18).expect("a session");
    for l in &laps[..2] {
        assert!((490_000..=510_000).contains(&get(l, 9)), "lap distance {} cm", get(l, 9));
        assert!((600_000..650_000).contains(&get(l, 8)), "lap time {} ms", get(l, 8));
    }
    for n in [8u8, 9] {
        let sum: u32 = laps.iter().map(|l| get(l, n)).sum();
        assert!(sum.abs_diff(get(session, n)) <= 2000, "field {n}: laps {sum}, session {}", get(session, n));
    }
}
