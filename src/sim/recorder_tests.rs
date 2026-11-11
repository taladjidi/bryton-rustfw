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
    crate::fat::tests::dump("ride_resumed_saved", &sim2.emmc.image);

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
pub(super) fn record_times(img: &[u8], path: &str) -> Vec<u32> {
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
    sim.gnss.expect = Some("stream time-out"); // the test holds the pass for seconds on purpose: the receiver's stream stops after 1.5 s
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
    sim.gnss.assert_reads(4180, 3);
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
/// the 2nd, 4th, 8th... failure of the whole run). The strip's mark is a red cross while it is not saving, NOT SAVING is
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
    // the strip's red cross, right of the time (the box the strip keeps for it)
    let top = crate::ui::STRIP_H - crate::ui::LABEL.height() + 2;
    let x0 = (W - 5 * 12) / 2 + 5 * 12 + 3;
    let red = (top..top + 10)
        .flat_map(|y| (x0..x0 + 10).map(move |x| (x, y)))
        .filter(|&(x, y)| sim.screen[(y * W + x) as usize] == crate::ui::RED)
        .count();
    assert!((20..60).contains(&red), "a cross, not a dot: {red} red pixels");
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

/// The card failed at the boot, which had a ride to reopen, and answered again a little later without Start being
/// pressed (2026-10-31): the ride left open is reopened at its checkpoint as at any boot, its marker kept until then,
/// and saved whole at the end. Before the fix the marker was cleared and the ride left in `\Rustfw\Rides`.
#[test]
fn the_card_answers_late_at_boot_without_start() {
    for late_ms in [2000, 5000, 60_000] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1000);
        app.press(&mut sim, Press::Short(Key::Start));
        let name = crate::recorder::ride_name(app.rec_time().0);
        run(&mut sim, &mut app, 90_000);
        let timer = app.ride.timer_s();
        let (mut sim, mut app) = {
            sim.emmc.dead = true;
            let mut sim2 = Sim::new(9, 3, 0);
            sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
            sim2.backup = sim.backup;
            sim2.emmc.dead = true; // already at App::start
            let app = App::start(&mut sim2, None);
            (sim2, app)
        };
        run(&mut sim, &mut app, late_ms);
        assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "{late_ms} ms: the marker kept while the card is down");
        sim.emmc.dead = false;
        run(&mut sim, &mut app, 15_000);
        let notes: Vec<_> = app.notes.list.iter().map(|n| n.as_str()).collect();
        assert!(notes.iter().any(|n| n.ends_with("RIDE RESUMED")), "{late_ms} ms: {notes:?}");
        assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "{late_ms} ms: the ride open again");
        assert_eq!(app.ride.state(), ride::State::Paused, "{late_ms} ms");
        assert!(app.ride.timer_s() + 60 >= timer, "{late_ms} ms");
        // a further reset before the save: it comes back again
        let (mut sim, mut app) = reboot(&mut sim, 5);
        run(&mut sim, &mut app, 1000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{late_ms} ms: {}", app.notes.list[0].as_str());
        save_ride(&mut sim, &mut app); // end
        run(&mut sim, &mut app, 2000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{late_ms} ms: {}", app.notes.list[0].as_str());
        assert_eq!(sim.backup[RIDE_BKP], 0);
        let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
        let times = record_times(&sim.emmc.image, &path);
        assert!(times.len() + 60 >= timer as usize, "{late_ms} ms: the ride whole");
        // its full copy too, resumed after the clean one
        assert_eq!(record_times(&sim.emmc.image, &std::format!("Rustfw/{path}")), times, "{late_ms} ms");
        assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    }
}

/// The full copy's side of the same: the clean copy reopened at the boot, the full copy's record not readable (the
/// card failing in that place) for 5 or 10 s. The full copy is reopened once it reads (its pass that ends the wait
/// never drops it), and saved with the same records as the clean one.
#[test]
fn the_full_copy_resumes_when_its_record_reads_late() {
    for late_ms in [5000, 10_000] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1000);
        app.press(&mut sim, Press::Short(Key::Start));
        let name = crate::recorder::ride_name(app.rec_time().0);
        run(&mut sim, &mut app, 90_000);
        let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let rec = fat.open(&mut d, "Rustfw/full.rec").unwrap();
        let lba = fat.lba(rec.cluster);
        sim.emmc.unreadable.push(lba..lba + 6);
        let (mut sim, mut app) = reboot(&mut sim, 3);
        sim.emmc.unreadable.push(lba..lba + 6);
        run(&mut sim, &mut app, 1000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{late_ms} ms: {}", app.notes.list[0].as_str());
        run(&mut sim, &mut app, late_ms);
        assert!(app.rec.full_pending, "{late_ms} ms: still pending while its record does not read");
        sim.emmc.unreadable.clear();
        run(&mut sim, &mut app, 15_000);
        assert!(!app.rec.full_pending && app.rec.full.phase() != recorder::Phase::Done, "{late_ms} ms: reopened");
        save_ride(&mut sim, &mut app); // end
        run(&mut sim, &mut app, 3000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{late_ms} ms: {}", app.notes.list[0].as_str());
        let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
        let times = record_times(&sim.emmc.image, &path);
        assert!(times.len() >= 60, "{late_ms} ms: {} records", times.len());
        assert_eq!(record_times(&sim.emmc.image, &std::format!("Rustfw/{path}")), times, "{late_ms} ms");
        assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    }
}

/// The card fails at a boot whose marker says a ride is open but whose card has none (a cut between the record's
/// "done" and the marker's clear), and answers 6 s later (past the first retry, 5 s): the marker kept until then;
/// the try then finds no ride, and nothing stays pending or marked, the ride settled.
#[test]
fn a_late_card_with_no_ride_settles() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.emmc.dead = true;
    sim.backup[RIDE_BKP] = RIDE_OPEN;
    let mut app = App::start(&mut sim, None);
    assert!(app.rec.resume_pending, "the marker read");
    run(&mut sim, &mut app, 6000);
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "kept while the card is down");
    sim.emmc.dead = false;
    run(&mut sim, &mut app, 15_000);
    assert!(!app.rec.resume_pending && !app.rec.full_pending);
    assert!(app.ride_settled());
    assert_eq!(sim.backup[RIDE_BKP], 0);
}

/// Reliable write (emmc.rs) on a card that offers it: every write is CMD23 with bit 31 then CMD25, never a plain
/// one. So a cut tears nothing: at every sector of a ride's first 75 s, the reboot finds a clean volume and the
/// ride at its last checkpoint (the root directory's sector included, which a plain write could lose). With the
/// card's stalls (emmc_model.rs E1) too: none in these 75 s, 400 ms every 11 sectors (waited out), 1.2 s every 41
/// (past the driver's bound: `Busy`, the write retried; every 37: `a_ride_starts_through_stalls_every_37_sectors`). A
/// write given up after landing may lose a cluster (fat/write.rs, "a write that fails after landing"): counted,
/// printed, bounded; never a cross-link.
fn reliable_writes(stall: (u64, u64)) {
    let start = |cut: Option<u64>, (every, ms): (u64, u64)| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        (sim.emmc.rel_param, sim.emmc.tear, sim.emmc.cut_after) = (crate::emmc::EN_REL_WR, true, cut);
        (sim.cost.stall_every, sim.cost.stall_ms) = (every, ms);
        sim.gnss.expect = Some("stream time-out"); // a stall past the driver's bound holds the pass over 1.5 s
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1000);
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 75_000);
        (sim, app)
    };
    let (sim, app) = start(None, stall);
    let cmds = &sim.emmc.cmds;
    assert!(!cmds.iter().any(|c| c.0 == 24), "a plain single-sector write");
    for (k, _) in cmds.iter().enumerate().filter(|c| c.1.0 == 25) {
        assert!(k > 0 && cmds[k - 1].0 == 23 && cmds[k - 1].1 & 1 << 31 != 0, "CMD25 without a reliable CMD23");
    }
    assert_eq!(app.rec.clean.errors > 0, stall.1 > 1000, "{stall:?}: writes given up past the bound only");
    // clusters lost by the writes given up, never freed while named (no cross-link: fsck passes)
    let leaked = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("{stall:?}: {e}"));
    let given = app.rec.clean.errors + app.rec.full.errors; // the two copies' writes given up
    std::println!("{stall:?}: {} writes given up, {leaked} clusters lost", given);
    assert!(leaked <= given, "{stall:?}: {leaked} lost, more than one a write given up");
    let total = sim.emmc.sectors_written;
    let (mut resumed, mut first) = (0, None);
    for cut in (0..total).step_by(super::fuzz::sample_step()) {
        let (mut sim, _) = start(Some(cut), stall);
        let (mut sim, mut app) = reboot(&mut sim, 2);
        run(&mut sim, &mut app, 1000);
        let back = app.notes.list[0].as_str().ends_with("RIDE RESUMED");
        resumed += usize::from(back);
        // once the ride's first checkpoint is on the card, every later cut resumes it: a cut that does not is a
        // ride lost (a cut before it finds no ride to resume, which is right: nothing was saved yet)
        match (first, back) {
            (None, true) => first = Some(cut),
            (Some(f), false) => panic!("{stall:?} cut {cut}: the ride not resumed, though cut {f} resumed it"),
            _ => {}
        }
        let lost =
            crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("{stall:?} cut {cut}: {e}"));
        assert!(lost <= 2 + leaked, "{stall:?} cut {cut}: {lost} clusters lost");
    }
    std::println!("{stall:?}: {total} cuts, {resumed} resumed from cut {first:?}");
    assert!(resumed > 0, "{stall:?}");
}

#[test]
fn every_write_is_reliable_without_stalls() {
    reliable_writes((1024, 300));
}

#[test]
fn every_write_is_reliable_through_400_ms_stalls() {
    reliable_writes((11, 400));
}

#[test]
fn every_write_is_reliable_through_1200_ms_stalls() {
    reliable_writes((41, 1200));
}

/// The card stalls 1.2 s (past the driver's bound: `Busy`) every 37 sectors, a ride start's retry length once the
/// failed write's cluster was given back (2026-11-02: the same write then failed at every retry and the start never
/// got through). The retries move on (a cluster lost each, at worst: fat/write.rs), the ride starts, its checkpoints
/// land and it is saved whole; fsck finds no cross-link.
#[test]
fn a_ride_starts_through_stalls_every_37_sectors() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.emmc.rel_param = crate::emmc::EN_REL_WR;
    (sim.cost.stall_every, sim.cost.stall_ms) = (37, 1200);
    sim.gnss.expect = Some("stream time-out"); // past the driver's bound the pass holds over 1.5 s
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
    run(&mut sim, &mut app, 180_000);
    assert!(app.rec.clean.phase() == recorder::Phase::Recording, "started: {:?}", app.rec.clean.phase());
    let timer = app.ride.timer_s();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 10_000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    let times = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
    let dropped = app.rec.clean.dropped;
    assert!(times.len() as u32 + dropped + 2 >= timer, "{} records, {dropped} dropped, of {timer} s", times.len());
    let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    std::println!("37: {} writes given up, {lost} clusters lost", app.rec.clean.errors);
}

/// A card without reliable write whose cut leaves the torn sector's first k bytes new and the rest old (the review
/// of 2026-11-01, finding 3: a sector a reader accepts; k on a 4-byte edge, moving with the cut): at
/// every sector of a ride's first 75 s, once its first checkpoint is on the card the reboot resumes it, and the
/// volume stays clean.
#[test]
fn a_prefix_torn_sector_never_loses_the_ride() {
    let start = |cut: Option<u64>| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        (sim.emmc.tear, sim.emmc.cut_after) = (true, cut);
        sim.emmc.tear_prefix = cut.map(|c| 4 * (1 + c as usize % 127));
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1000);
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 75_000);
        sim
    };
    let total = start(None).emmc.sectors_written;
    let (mut resumed, mut first) = (0, None);
    for cut in 0..total {
        let mut sim = start(Some(cut));
        let (mut sim, mut app) = reboot(&mut sim, 2);
        run(&mut sim, &mut app, 1000);
        let back = app.notes.list[0].as_str().ends_with("RIDE RESUMED");
        resumed += usize::from(back);
        match (first, back) {
            (None, true) => first = Some(cut),
            (Some(f), false) => panic!("cut {cut}: the ride not resumed, though cut {f} resumed it"),
            _ => {}
        }
        // before the first checkpoint, while the folders are made: a folder's entry torn inside it (on cluster 0),
        // which the reader refuses as Corrupt (never the root)
        let lost = match crate::fat::write::tests::fsck_torn(&sim.emmc.image) {
            Err(e) if first.is_none() && e.contains("Corrupt") => 0,
            r => r.unwrap_or_else(|e| panic!("cut {cut}: {e}")),
        };
        assert!(first.is_none() || lost <= 2, "cut {cut}: {lost} clusters lost");
    }
    std::println!("{total} prefix-torn cuts, {resumed} resumed from cut {first:?}");
    assert!(resumed > 0);
}

/// The first cluster's sectors of the file `path` on the simulated card, and its size.
fn file_lbas(sim: &Sim, path: &str) -> (core::ops::Range<u32>, u32) {
    let mut ram = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let size = crate::fat::Fat::mount(&mut ram).unwrap().open(&mut ram, path).unwrap().size;
    (crate::sim::drive_tests::dir_sectors(&sim.emmc.image, path), size)
}

/// A ride of 70 s on the simulated card (its first checkpoint on it): the sim, the app, the clean copy's file in
/// `\Rustfw\Rides` and the sector its next checkpoint writes second (the one after the trailer's).
fn ride_70s() -> (Sim, App, std::string::String, u32) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    let name = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
    let (lbas, size) = file_lbas(&sim, &std::format!("Rustfw/Rides/{name}.fit"));
    let next = lbas.start + (size - 2) / 512 + 1;
    assert!(next + 2 < lbas.end, "the next checkpoint in the file's first cluster");
    (sim, app, name, next)
}

/// The review of 2026-11-01, finding 33: a card that stays "programming" (CMD13 state 7) after a write that landed.
/// A few polls are waited out (1 ms each); past [`crate::emmc::TRIES`] the driver gives up (`Busy`) after about a
/// second, the data on the card. The recorder counts the failed checkpoint, tries again, and the ride is saved,
/// none of its seconds lost but those the wait dropped; the volume clean.
#[test]
fn a_card_busy_past_the_drivers_second_fails_the_write_and_the_ride_goes_on() {
    let mut s = Sim::new(9, 0, 0);
    s.gnss.expect = Some("stream time-out"); // the test holds the pass for seconds on purpose: the receiver's stream stops after 1.5 s
    s.emmc.image = std::vec![0; 64 * 512];
    let card = crate::emmc::Card::init(&mut s).unwrap();
    s.emmc.prog_polls = 5;
    let t0 = s.ms;
    assert_eq!(card.write(&mut s, 3, &[7; 512]), Ok(()));
    assert!(s.ms - t0 >= 5, "the polls waited out");
    s.emmc.prog_polls = crate::emmc::TRIES;
    let t0 = s.ms;
    assert_eq!(card.write(&mut s, 4, &[8; 512]), Err(crate::emmc::Error::Busy));
    assert!(s.ms - t0 >= 1000 && s.emmc.image[4 * 512] == 8, "given up after a second, the sector landed");
    s.emmc.prog_polls = 0;
    assert_eq!(card.write(&mut s, 5, &[9; 512]), Ok(()), "the card ready again");

    let (mut sim, mut app, name, _) = ride_70s();
    sim.gnss.expect = Some("stream time-out"); // a checkpoint write the driver waits a second for blocks the pass
    let errors = app.rec.clean.errors;
    sim.emmc.prog_polls = crate::emmc::TRIES;
    run(&mut sim, &mut app, 55_000); // the checkpoint at 120 s: its first write lands, then the driver gives up
    sim.emmc.prog_polls = 0;
    assert!(app.rec.clean.errors > errors, "no write given up");
    run(&mut sim, &mut app, 120_000);
    let timer = app.ride.timer_s();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    let times = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
    assert!(times.windows(2).all(|p| p[0] < p[1]), "times in order");
    let dropped = app.rec.clean.dropped;
    assert!(times.len() as u32 + dropped + 2 >= timer, "{} records, {dropped} dropped, of {timer} s", times.len());
    crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
    super::budget_tests::print_gap_breakdown(&sim, "card busy");
    // measured 2759 ms with the app clock counting the waits (2205 before, in another phase of the read interval):
    // one write given up after the driver's second (`errors` above) plus the rest of the gap, which has 8 ms of paid
    // time (the checkpoint's other writes and the read interval around the pass). Bound: measured + 10 %.
    assert_eq!(app.rec.clean.errors - errors, 1, "one busy wait given up");
    sim.gnss.assert_reads(3040, 2);
}

/// The review of 2026-11-01, finding 33: a multi-sector checkpoint write that lands its first sector, then fails
/// (`bad_partial`), and the unit reset before the retry: the boot resumes the ride from its last whole checkpoint,
/// and it is saved, a whole FIT file; the volume clean.
#[test]
fn a_resume_after_a_partial_multi_sector_write() {
    let (mut sim, mut app, name, next) = ride_70s();
    (sim.emmc.bad, sim.emmc.bad_partial, sim.emmc.bad_left) = (std::vec![next..next + 1], true, Some(1));
    run(&mut sim, &mut app, 55_000); // the checkpoint at 120 s: its trailer's sector lands, the next fails
    assert_eq!(sim.emmc.bad_left, Some(0), "the write failed");
    let timer = app.ride.timer_s();
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    let back = app.ride.timer_s();
    assert!(back + 2 * crate::recorder::CHECKPOINT_S >= timer, "back at {back} of {timer}");
    app.press(&mut sim, Press::Short(Key::Start)); // ride on
    run(&mut sim, &mut app, 30_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    let times = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
    assert!(times.windows(2).all(|p| p[0] < p[1]) && times.len() as u32 >= back, "{} records", times.len());
    crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
}

/// The review of 2026-11-01, finding 33: a card that drops a checkpoint's sector and still answers done (`lie`).
/// Nothing reads it back until the save: its read-back (recorder/mod.rs, the save's step 2: the file's FIT CRC)
/// finds the file not whole, and the ride is kept as it is in `\Rustfw\Rides`, never a broken file in `\Activities`.
#[test]
fn the_saves_read_back_catches_a_card_that_lies() {
    let (mut sim, mut app, name, next) = ride_70s();
    // the checkpoints at 120 and 180 s land nothing but sector 0: the one at 120 s starts in the trailer's sector
    // (`next - 1`), the one at 180 s in the next, which no later checkpoint writes again
    sim.emmc.lie = Some(next - 1..next + 8);
    run(&mut sim, &mut app, 115_000);
    sim.emmc.lie = None;
    run(&mut sim, &mut app, 60_000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    let file = |dir: &str| crate::storage::trash::tests::file(&sim.emmc.image, &std::format!("{dir}/{name}.fit"));
    assert!(file("Activities").is_none(), "a file the card broke saved");
    let kept = file("Rustfw/Rides").expect("kept where it was recorded");
    assert!(crate::fit::tests::decode(&kept).is_err(), "the lie in the file");
    assert!(file("Rustfw/Activities").is_some(), "the full copy, untouched, saved");
    crate::fat::write::tests::fsck(&sim.emmc.image).unwrap();
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
    run(&mut sim, &mut app, 3500); // the ready card goes by itself (a key on it only skips it)
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

/// The ready card is up for 3 s over a ride that runs from the press: after 70 s a checkpoint exists (the ride open on
/// the card) and a reset resumes the ride at it, the card changing nothing for the recorder.
#[test]
fn the_ready_card_changes_nothing_for_the_recorder() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.settings.ready_check = true;
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.kind, Banner::Ready);
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 70_000);
    assert_eq!(app.notes.ms, None, "the card has gone");
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN, "a checkpoint: the ride is open on the card");
    let timer = app.ride.timer_s();
    assert!(timer >= 60, "the timer ran from the press: {timer}");
    let (sim2, app2) = reboot(&mut sim, 3);
    let mut app2 = app2;
    let mut sim2 = sim2;
    run(&mut sim2, &mut app2, 1000);
    assert_eq!(app2.ride.state(), ride::State::Paused, "resumed at the checkpoint");
    assert!(app2.ride.timer_s() <= timer && app2.ride.timer_s() + 60 >= timer);
}

/// A ride across the EU's October change (2026-10-25 01:00 UTC) with TIME ZONE +1 and SUMMER TIME EU, the RTC
/// on summer time as the phone left it (the sim's receiver: UTC + 2 h, learnt at the fix): it keeps the name it
/// started with (02:57 summer time), the strip goes from 02:59 to 02:00 while its FIT times go on one a second, and
/// a reset after the change resumes the same file, saved under its first name with times that never go back.
#[test]
fn a_ride_across_the_dst_change_keeps_its_name_and_times() {
    let mut sim = Sim::new(50, 57, 0); // the RTC at 2026-10-25 02:57:00, summer time: 00:57 UTC
    sim.emmc.image = crate::fat::tests::image();
    sim.gnss.fix = Some(5000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    (app.settings.tz, app.settings.dst) = (crate::TZ_UTC + 4, crate::Dst::Eu);
    app.settings_store.file.dirty = true; // as the menu does: into the file before the ride
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.gps.aid.offset_15min, Some(8), "UTC + 2 h learnt at the fix");
    assert_eq!(app.strip.time, (2, 57), "summer: UTC + 2 h");
    app.press(&mut sim, Press::Short(Key::Start));
    let name = crate::recorder::ride_name(app.rec_time().0);
    assert_eq!(&name[..8], b"26102502");
    let mut shown = Vec::new();
    for _ in 0..270 {
        run(&mut sim, &mut app, 1000);
        shown.push(app.strip.time);
    }
    assert!(shown.contains(&(2, 59)) && shown.ends_with(&[(2, 1)]), "{shown:?}");
    assert_eq!(sim.backup[RIDE_BKP], RIDE_OPEN);

    // a reset after the change: the RTC still on summer time (03:02), the shown clock on winter time (02:02)
    let mut sim2 = Sim::new(51, 2, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    sim2.gnss.fix = Some(5000);
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, 3000);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")));
    assert_eq!(app.rec.clean.name(), name, "the same file reopened");
    assert_eq!((app.settings.tz, app.strip.time), (crate::TZ_UTC + 4, (2, 2)), "the zone back from the file");
    assert_eq!(app.ride.state(), ride::State::Running, "gone on by itself at the fix's 18 km/h");
    run(&mut sim2, &mut app, 30_000);
    app.press(&mut sim2, Press::Short(Key::Start)); // pause
    save_ride(&mut sim2, &mut app);
    run(&mut sim2, &mut app, 2000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    let times = record_times(&sim2.emmc.image, &path);
    assert!(times.windows(2).all(|p| p[1] > p[0]), "monotonic: {times:?}");
    let change = crate::fit::fit_time((2026, 10, 25, 1, 0, 0));
    let at = times.iter().position(|&t| t == change).expect("a record at the change");
    assert!(times[at - 60..at + 60].windows(2).all(|p| p[1] == p[0] + 1), "one a second through the change");
    assert!(times.last().is_some_and(|&t| t >= crate::fit::fit_time((2026, 10, 25, 1, 2, 30))), "the resumed part");
    assert_eq!(crate::fat::write::tests::fsck(&sim2.emmc.image), Ok(0));
}

/// GRAVEL, CX and COMMUTE are kept as ROAD in the recovery record: after a reset the resume reads settings.bin and
/// takes the profile back, so the rules and the file's sub_sport (46, 11, 48) are the profile's.
#[test]
fn a_resumed_gravel_cx_or_commute_ride_keeps_its_profile() {
    use crate::ride::Profile::{Commute, Cx, Gravel};
    for (p, sub) in [(Gravel, 46), (Cx, 11), (Commute, 48)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 2000);
        app.settings.profile = Some(p);
        app.settings_store.file.dirty = true; // as the menu does: into the file before the ride
        sim.backup[crate::WHEEL_BKP] = app.settings.to_word(); // and into register 11: ROAD for these
        run(&mut sim, &mut app, 5000);
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.ride_profile(), p);
        run(&mut sim, &mut app, 150_000);
        let (mut sim2, mut app) = reboot(&mut sim, 3);
        run(&mut sim2, &mut app, 1000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
        assert_eq!(app.ride_profile(), p, "from settings.bin, the record says ROAD");
        assert_eq!((app.ride_profile().pause_kmh(), app.ride_profile().auto_lap()), (p.pause_kmh(), p.auto_lap()));
        app.press(&mut sim2, Press::Short(Key::Start)); // resume
        run(&mut sim2, &mut app, 5000);
        app.press(&mut sim2, Press::Short(Key::Start));
        save_ride(&mut sim2, &mut app);
        run(&mut sim2, &mut app, 2000);
        let name = std::string::String::from_utf8(app.rec.clean.name().to_vec()).unwrap();
        let mut d = crate::fat::tests::Ram(sim2.emmc.image.clone(), 0);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, &std::format!("Activities/{name}.fit")).unwrap();
        let mut v = std::vec![0u8; f.size as usize];
        fat.read_at(&mut d, &f, 0, &mut v).unwrap();
        let m = crate::fit::tests::decode(&v).unwrap();
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(crate::fit::tests::field(session, 6), Some(sub), "{p:?}: sub_sport");
    }
}

/// Without settings.bin (never written here: the register says ROAD for GRAVEL), the ride still resumes, as ROAD.
#[test]
fn a_resumed_gravel_ride_without_its_settings_file_resumes_as_road() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 2000);
    app.settings.profile = Some(crate::ride::Profile::Gravel); // not dirty: the file is not written
    sim.backup[crate::WHEEL_BKP] = app.settings.to_word();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 150_000);
    let (mut sim2, mut app) = reboot(&mut sim, 3);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert_eq!(app.ride_profile(), crate::ride::Profile::Road);
}

/// A power loss that takes the backup domain ([`Sim::power_loss`]: RAM, the backup registers with `RIDE_BKP`, and
/// the RTC, back to 2000-01-01), the card left whole or with a checkpoint torn mid-write: the boot finds no register
/// of this firmware's, reads the ride's record (`App::left_by_a_power_loss`) and resumes the ride as the marker's
/// path does, paused at its last checkpoint; the unit stays on, idle, while it is open; Start carries it on and the
/// saved ride has every second up to that checkpoint, then the new ones.
#[test]
fn a_ride_survives_a_power_loss_that_clears_the_backup_registers() {
    for torn in [false, true] {
        let (mut sim, app) = power_lost_ride(torn);
        let name = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
        let before = app.ride.timer_s();
        let mut app = App::start(&mut sim, None);
        assert!(!sim.rtc_stopped, "{torn}: the boot started the clock the loss stopped");
        run(&mut sim, &mut app, 10_000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{torn}: {}", app.notes.list[0].as_str());
        // back paused at its last checkpoint: nothing lost before it
        let (state, timer) = (app.ride.state(), app.ride.timer_s());
        assert!(state == crate::ride::State::Paused, "{torn}: {state:?}");
        assert!(timer >= if torn { 60 } else { 120 } && timer <= before, "{torn}: {timer} s of {before}");
        app.press(&mut sim, Press::Short(Key::Start)); // on
        run(&mut sim, &mut app, 70_000);
        app.press(&mut sim, Press::Short(Key::Start)); // pause
        save_ride(&mut sim, &mut app);
        run(&mut sim, &mut app, 5000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{torn}: {}", app.notes.list[0].as_str());
        let times = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
        // in order, one a second, though the RTC the loss reset reads 2000-01-01
        assert!(times.windows(2).all(|p| p[1] == p[0] + 1), "{torn}: times in order, one a second");
        assert!(
            times.len() as u32 >= timer + 60,
            "{torn}: {} records: up to the checkpoint and the new ones",
            times.len()
        );
        // the full copy too: its new seconds, though "before" those it held, are recorded (the review of
        // 2026-11-11, R1: it skipped every second earlier than its last record, to the end of the ride)
        let full = record_times(&sim.emmc.image, &std::format!("Rustfw/Activities/{name}.fit"));
        assert!(full.windows(2).all(|p| p[1] > p[0]), "{torn}: full copy in order");
        assert!(full.len() >= times.len() - 2, "{torn}: {} full records of {}", full.len(), times.len());
        // and the next ride of this boot, its clock still in 2000, has its full copy's records
        app.press(&mut sim, Press::Short(Key::Start)); // the summary closed
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 70_000);
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.ride.state(), crate::ride::State::Paused, "{torn}");
        let next = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
        save_ride(&mut sim, &mut app);
        run(&mut sim, &mut app, 5000);
        assert!(next != name, "{torn}: a new ride");
        let full = record_times(&sim.emmc.image, &std::format!("Rustfw/Activities/{next}.fit"));
        assert!(full.len() >= 60, "{torn}: the next ride's full copy, {} records", full.len());
        assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0), "{torn}");
    }
}

/// The clock lost with the backup domain and no phone (the review of 2026-11-11, R3): a fix with a valid time between
/// rides sets the RTC to UTC, the offset to 0, so the next ride is named and dated in the current year, not in 2000.
/// A ride resuming after the loss is never moved (its times must not jump), nor is a clock already right.
#[test]
fn a_fix_sets_a_clock_lost_with_the_backup_domain_between_rides() {
    let now = crate::epoch_s((2026, 11, 11, 8, 0, 0));
    let mut sim = Sim::new(0, 0, 0).power_loss();
    sim.emmc.image = crate::fat::tests::image();
    (sim.gnss.fix, sim.gnss_utc) = (Some(5000), Some(now));
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert_eq!(sim.rtc_sets, 1, "set once, at the first fix");
    assert_eq!(app.gps.aid.offset_15min, Some(0), "the RTC on UTC: the phone sets the zone later");
    assert!((crate::rtc_epoch_s(&mut sim) - now).abs() <= 11, "the RTC at the sky's time");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 70_000);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    assert!(name.starts_with("261111"), "named in the current year: {name}");
    let times = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
    let fit_now = crate::fit::fit_time(crate::from_epoch_s(now));
    assert!(times.first().is_some_and(|&t| t >= fit_now && t < fit_now + 120), "dated now: {times:?}");
    assert_eq!(sim.rtc_sets, 1, "never during the ride");

    // a ride left open by the loss: resumed on the clock as it restarted, never moved while it is open
    let (mut sim, _) = power_lost_ride(false);
    (sim.gnss.fix, sim.gnss_utc) = (Some(5000), Some(now));
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert_eq!(sim.rtc_sets, 0, "a ride resumed: its clock left alone");

    // a clock already right (2026) is never moved by a fix
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.fix = Some(5000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert_eq!((sim.rtc_sets, app.gps.aid.offset_15min), (0, Some(8)));
}

/// A ride of 125 s, its checkpoints on the card, the power lost with the backup domain at 125 s or, `torn`,
/// in the middle of its second checkpoint's first write: the unit after it, and the app that recorded it.
fn power_lost_ride(torn: bool) -> (Sim, App) {
    let (mut sim, mut app, _, _) = ride_70s();
    if torn {
        (sim.emmc.tear, sim.emmc.cut_after) = (true, Some(sim.emmc.sectors_written + 1));
    }
    run(&mut sim, &mut app, 55_000); // past the second checkpoint (120 s), cut in it if torn
    assert_eq!(sim.emmc.dead, torn);
    (sim.power_loss(), app)
}

/// AUTO OFF's own rule (app/device.rs: never off while the recorder holds anything): the unit does not switch off
/// as idle while the card holds a ride not recovered, after a power loss that cleared the backup registers too
/// (before 2026-11-02 it did: the boot had no marker and did not read the card). And a boot after such a loss with
/// no ride left reads the record once and settles: nothing pending, AUTO OFF free.
#[test]
fn a_power_loss_ride_keeps_the_unit_on() {
    let (mut sim, _) = power_lost_ride(false);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    app.settings.auto_off = 1;
    run(&mut sim, &mut app, 400_000);
    assert!(!sim.standby, "switched off as idle with a ride left on the card");
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.backup = [0; 32];
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert!(!app.rec.resume_pending && !app.rec.full_pending && app.ride_settled(), "no ride: settled");
    app.settings.auto_off = 1;
    run(&mut sim, &mut app, 400_000);
    assert!(sim.standby, "no ride: AUTO OFF as usual");
}

/// The third review of 38fb5f3, finding 2: after a power loss that reset the backup domain with a ride open, the
/// boot cannot read the ride's records (the card failing there for 3 s): what they say is unknown, so stock's
/// settings are not imported then (they would replace the ride's own); the resume then takes the ride back, and
/// nothing of stock's is imported over it.
#[test]
fn an_unread_record_after_a_power_loss_holds_stocks_import() {
    let (mut sim, _) = power_lost_ride(false);
    sim.emmc.unreadable = ["Rustfw/ride.rec", "Rustfw/full.rec"].iter().map(|p| file_lbas(&sim, p).0).collect();
    let mut app = App::start(&mut sim, None);
    assert!(app.rec.resume_pending, "unknown: pending");
    let at_boot: Vec<_> = app.notes.list.iter().map(|n| n.as_str()).collect();
    assert!(at_boot.iter().all(|n| !n.contains("IMPORTED")), "imported at the boot: {at_boot:?}");
    run(&mut sim, &mut app, 3000);
    sim.emmc.unreadable.clear();
    run(&mut sim, &mut app, 30_000);
    let notes: Vec<_> = app.notes.list.iter().map(|n| n.as_str()).collect();
    assert!(notes.iter().any(|n| n.ends_with("RIDE RESUMED")), "{notes:?}");
    assert!(notes.iter().all(|n| !n.contains("IMPORTED")), "stock's settings over the ride's: {notes:?}");
    assert!(!app.settings_store.import_pending, "settled");
}

/// The third review of 38fb5f3, finding 1: the first start's import left pending behind a resume that never
/// settles (both ride records unreadable for good after a power loss) is given up within its bound: the settings
/// file is then read, and saves work again.
#[test]
fn a_pending_import_gives_way_to_the_settings_file() {
    let (mut sim, _) = power_lost_ride(false);
    sim.emmc.unreadable = ["Rustfw/ride.rec", "Rustfw/full.rec"].iter().map(|p| file_lbas(&sim, p).0).collect();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    assert!(app.settings_store.import_pending && !app.settings_store.file.loaded, "the file waits for the import");
    run(&mut sim, &mut app, u64::from(crate::app::IMPORT_WAIT_MS) + 10_000);
    assert!(app.rec.resume_pending, "the records still unread");
    assert!(!app.settings_store.import_pending, "given up");
    assert!(app.settings_store.file.loaded, "the settings file read");
}

/// Passes of `stride` ms for `ms`, the sensors reading `speed_x10` and `power` (None: the meter silent) before each.
fn ride_passes(sim: &mut Sim, app: &mut App, ms: u64, stride: u32, speed_x10: u16, power: Option<u16>) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.live.set(Metric::Speed, speed_x10);
        match power {
            Some(p) => app.live.set(Metric::Power, p),
            None => app.live.clear(Metric::Power),
        }
        app.step(sim);
        sim.sleep_ms(stride);
    }
}

/// A ride of 70 s at 30 km/h and 200 W (its last 5 s at `speed_x10`; 0: stopped, the meter silent), then `stall_s`
/// without a pass (a loop held by a card or a radio that does not answer): the sim, the app, the timer before the
/// stall and the ride's name.
fn ride_then_stall(stall_s: u32, speed_x10: u16) -> (Sim, App, u32, [u8; 12]) {
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.expect = Some("stream time-out"); // the test holds the loop on purpose: the receiver's stream stops
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 65_000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, 5000, POLL_MS, speed_x10, (speed_x10 > 0).then_some(200));
    let (t, name) = (app.ride.timer_s(), app.rec.clean.name());
    sim.delay_ms(stall_s * 1000);
    (sim, app, t, name)
}

/// The saved file's records' times (the full copy's the same), its session's total elapsed and timer times (s) and
/// average power.
fn saved_ride(sim: &Sim, name: [u8; 12]) -> (Vec<u32>, u64, u64, Option<u64>) {
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    let times = record_times(&sim.emmc.image, &path);
    assert_eq!(record_times(&sim.emmc.image, &std::format!("Rustfw/{path}")), times, "the full copy");
    let m = crate::fit::tests::decode(&ride_file(sim, "Activities", name).unwrap()).unwrap();
    let session = m.iter().find(|x| x.0 == 18).unwrap();
    let f = |n| crate::fit::tests::field(session, n);
    (times, f(7).unwrap() / 1000, f(8).unwrap() / 1000, f(20))
}

/// The owner, 2026-11-02: no cap on a stall's seconds (10 s before). Riding, a stall of `stall_s`: every second of it
/// on the timer; records for the seconds read (the last 5 of the stall repeat the pass's readings, as before), the
/// times jumping over the rest; the session's timer is the screen's, the average power unchanged; nothing dropped.
fn every_second_of_a_stall_is_on_the_timer(stall_s: u32) {
    let (mut sim, mut app, t, name) = ride_then_stall(stall_s, 300);
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    assert!(timer.abs_diff(t + stall_s + 20) <= 1, "timer {timer}: {t} before, {stall_s} s of stall, 20 s after");
    assert_eq!(app.ride.avg_power(), Some(200));
    let (_, now) = app.rec_time();
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!((app.rec.clean.dropped, app.rec.full.dropped), (0, 0));
    let (times, elapsed, timer_s, power) = saved_ride(&sim, name);
    assert!(times.windows(2).all(|p| p[1] > p[0]), "later each");
    assert_eq!(times.last().copied(), Some(now), "the last record at the clock's time");
    let jumps: Vec<u32> = times.windows(2).map(|p| p[1] - p[0]).filter(|&d| d > 1).collect();
    let skipped = stall_s.saturating_sub(5);
    assert_eq!(jumps, if skipped > 0 { std::vec![skipped + 1] } else { std::vec![] }, "one jump over the stall");
    assert!(times.len().abs_diff((timer - skipped) as usize) <= 1, "{} records, timer {timer}", times.len());
    assert!(timer_s.abs_diff(u64::from(timer)) <= 1 && elapsed == timer_s, "session {elapsed} / {timer_s} s, {timer}");
    assert_eq!(power, Some(200));
}

#[test]
fn a_stall_of_30_s_is_all_on_the_timer() {
    every_second_of_a_stall_is_on_the_timer(30);
}

#[test]
fn a_stall_of_10_min_is_all_on_the_timer() {
    every_second_of_a_stall_is_on_the_timer(600);
}

/// A stall of 3 s (under 5): every second repeated as before, no jump in the record times.
#[test]
fn a_stall_of_3_s_repeats_the_readings() {
    every_second_of_a_stall_is_on_the_timer(3);
}

/// Auto-paused (stopped) when the loop stalls: the stall adds nothing to the timer, as a stop does not.
#[test]
fn a_stall_while_auto_paused_adds_nothing() {
    let (mut sim, mut app, t, _) = ride_then_stall(600, 0);
    assert!(!app.ride.moving());
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 0, None);
    assert_eq!(app.ride.timer_s(), t);
    assert_eq!(app.ride.avg_power(), Some(200));
    // moving again, then saved: the file's timer stopped over the stop and the stall, as the screen's
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    assert!(timer.abs_diff(t + 10) <= 1, "timer {timer}");
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    assert_eq!(timer_events(&file), [(0, None), (4, Some(1)), (0, Some(1)), (4, None)], "a stop over the stall");
}

/// The ride's timer events (event timer): (event_type, timer_trigger: None without the data field, the Start
/// key's), after checking they alternate start, stop.
pub(super) fn timer_events(file: &[u8]) -> Vec<(u64, Option<u64>)> {
    use crate::fit::tests::field;
    let m = crate::fit::tests::decode(file).unwrap();
    let ev: Vec<_> =
        m.iter().filter(|x| x.0 == 21 && field(x, 0) == Some(0)).map(|x| (field(x, 1).unwrap(), field(x, 3))).collect();
    assert!(ev.iter().enumerate().all(|(k, e)| e.0 == if k % 2 == 0 { 0 } else { 4 }), "start, stop, ...: {ev:?}");
    ev
}

/// Pauses the ride (Start), saves it, and returns its clean copy and its session's timer (s), after checking the
/// laps' timers add up to it and both copies' match.
fn saved_timer(sim: &mut Sim, app: &mut App) -> (Vec<u8>, u64) {
    use crate::fit::tests::field;
    let name = app.rec.clean.name();
    app.press(sim, Press::Short(Key::Start));
    save_ride(sim, app);
    run(sim, app, 2000);
    assert_eq!(app.ride.state(), ride::State::Idle, "saved");
    let timers = |dir| {
        let m = crate::fit::tests::decode(&ride_file(sim, dir, name).unwrap()).unwrap();
        let laps: u64 = m.iter().filter(|x| x.0 == 19).map(|l| field(l, 8).unwrap()).sum();
        let session = field(m.iter().find(|x| x.0 == 18).unwrap(), 8).unwrap();
        assert_eq!(laps, session, "{dir}: the laps add up to the session");
        session / 1000
    };
    let timer = timers("Activities");
    assert_eq!(timers("Rustfw/Activities"), timer, "the full copy");
    (ride_file(sim, "Activities", name).unwrap(), timer)
}

/// Auto-pauses as Garmin's (owner, 2026-11-02; on the unit a 4.5 min ride at 0 km/h showed TIMER 0:00 and saved
/// 241 s): stopped from the start, stops, a lap (Lap key) during a stop: each stop a timer stop_all event (trigger
/// auto) and a start when moving again; each lap's and the session's total_timer_time the screen's. The file goes to
/// `target/fit/autopause.fit` for tools/fit_check.py (`--timer`).
#[test]
fn auto_pauses_stop_the_files_timer() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 0, None);
    assert_eq!(app.ride.timer_s(), 0, "stopped from the start: TIMER 0:00");
    ride_passes(&mut sim, &mut app, 40_000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, 15_000, POLL_MS, 0, None);
    app.lap();
    let lap1 = app.ride.last_lap_s();
    ride_passes(&mut sim, &mut app, 15_000, POLL_MS, 0, None);
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, 30_000, POLL_MS, 0, None);
    let timer = app.ride.timer_s();
    assert!((105..=115).contains(&timer), "timer {timer}");
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    let m = crate::fit::tests::decode(&file).unwrap();
    let lap = m.iter().find(|x| x.0 == 19).unwrap();
    assert_eq!(crate::fit::tests::field(lap, 8), lap1.map(|s| u64::from(s) * 1000), "the first lap: its banner's");
    let auto = |k| (k, Some(1));
    // the last stop then Start and the end: no more events (the timer stopped already)
    assert_eq!(timer_events(&file), [(0, None), auto(4), auto(0), auto(4), auto(0), auto(4)]);
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("autopause.fit"), &file).unwrap();
}

/// A reset during an auto-pause (a checkpoint taken in the stop): the ride comes back paused with the timer of its
/// checkpoint, the stop a pause; Start goes on and the saved file's timer is the screen's, its events paired.
#[test]
fn a_reset_during_an_auto_pause_keeps_the_timer() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 40_000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, 90_000, POLL_MS, 0, None); // a checkpoint at 60 s and 120 s, stopped
    let timer = app.ride.timer_s();
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert_eq!((app.ride.state(), app.ride.timer_s()), (ride::State::Paused, timer), "the stop's timer");
    app.press(&mut sim, Press::Short(Key::Start)); // resume
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    // the stop ended by the resume (the Start key's start), no stop event at the reset
    assert_eq!(timer_events(&file), [(0, None), (4, Some(1)), (0, None), (4, None)]);
}

/// A reset mid-ride (owner, 2026-11-03: "Resume automatically"): the ride comes back paused with RIDE RESUMED and,
/// with nothing read (the sensors not back) or standing still, stays so; the first second at speed it goes on by
/// itself, counted. The file: the time off an auto-pause's stop (stop_all, trigger auto, at the last record) that
/// the first moving second ends (start, trigger auto); its records never repeat a time, its lap in the file kept (the
/// screen's lap count restarts after a resume, as before), each lap's and
/// the session's timer the screen's. The file goes to `target/fit/autoresume.fit` for tools/fit_check.py (`--timer`).
#[test]
fn a_reset_mid_ride_goes_on_once_riding() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200));
    app.lap();
    ride_passes(&mut sim, &mut app, 60_000, POLL_MS, 300, Some(200)); // a checkpoint at 60 s and 120 s
    let before = app.ride.timer_s();
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    let back = app.ride.timer_s();
    assert!(back > 60 && back <= before && back + 60 >= before, "the checkpoint's timer: {back} of {before}");
    assert_eq!(app.ride.state(), ride::State::Paused);
    run(&mut sim, &mut app, 10_000);
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 0, None);
    assert_eq!((app.ride.state(), app.ride.timer_s()), (ride::State::Paused, back), "nothing read, then stopped");
    sim.tones.clear();
    ride_passes(&mut sim, &mut app, 1000, POLL_MS, 300, Some(200));
    assert_eq!(app.ride.state(), ride::State::Running, "riding: gone on within the second");
    assert_eq!(app.notes.text.as_str(), "RESUMED", "said as a Start resume");
    assert!(sim.tones.windows(2).any(|w| w == [1800, 2600]), "BEEP_START: {:?}", sim.tones);
    assert!(app.ride.moving() && (back + 1..=back + 2).contains(&app.ride.timer_s()), "{}", app.ride.timer_s());
    ride_passes(&mut sim, &mut app, 30_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    let name = app.rec.clean.name();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's, the time off left out");
    let auto = |k| (k, Some(1));
    assert_eq!(timer_events(&file), [(0, None), auto(4), auto(0), (4, None)], "the time off an auto stop");
    let times = record_times(&sim.emmc.image, &std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap()));
    assert!(times.windows(2).all(|p| p[1] > p[0]), "no time twice: {times:?}");
    let m = crate::fit::tests::decode(&file).unwrap();
    assert_eq!(m.iter().filter(|x| x.0 == 19).count(), 2, "the lap before the reset and the last");
    to_fit_dir("autoresume.fit", &file);
}

/// `file` as `target/fit/<name>` for tools/fit_check.py (check.sh, `--timer`).
fn to_fit_dir(name: &str, file: &[u8]) {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), file).unwrap();
}

/// A ride at 30 km/h and 200 W for 70 s, then `stop_s` stopped (0 km/h, the meter silent), then a reset: the unit
/// after it, the RIDE RESUMED notice up and the ride paused at its checkpoint's timer.
fn resumed_after(stop_s: u64) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, stop_s * 1000, POLL_MS, 0, None);
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    assert_eq!(app.ride.state(), ride::State::Paused);
    (sim, app)
}

/// A reset during an auto-pause (its stop open at the checkpoint) goes on by itself once ridden: one auto stop (the
/// auto-pause's, none written at the reset) and one auto start; the file's timer the screen's.
#[test]
fn a_reset_during_an_auto_pause_goes_on_once_riding() {
    let (mut sim, mut app) = resumed_after(60); // a checkpoint at 120 s, stopped
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    assert_eq!(app.ride.state(), ride::State::Running);
    let timer = app.ride.timer_s();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    let auto = |k| (k, Some(1));
    assert_eq!(timer_events(&file), [(0, None), auto(4), auto(0), (4, None)]);
    to_fit_dir("autoresume_open.fit", &file);
}

/// Start during the wait goes on as ever: the reset's auto stop ended by the Start key's start.
#[test]
fn start_during_the_wait_goes_on_by_hand() {
    let (mut sim, mut app) = resumed_after(0);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    assert_eq!(timer_events(&file), [(0, None), (4, Some(1)), (0, None), (4, None)]);
    to_fit_dir("autoresume_start.fit", &file);
}

/// The end-ride screen up on a resumed ride waiting to go on: held paused (its recorders' stop the rider's, a
/// checkpoint at once), so after Back and another reset it stays paused while ridden, until Start.
#[test]
fn the_end_screen_holds_a_resumed_ride_across_a_reset() {
    let (mut sim, mut app) = resumed_after(0);
    app.press(&mut sim, Press::Long(Key::Start));
    assert!(matches!(app.screen(), Screen::End(_)), "{:?}", app.screen());
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Back));
    ride_passes(&mut sim, &mut app, 5000, POLL_MS, 300, Some(200));
    assert_eq!(app.ride.state(), ride::State::Paused, "held: not gone on");
    let (mut sim, mut app) = reboot(&mut sim, 6);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    let timer = app.ride.timer_s();
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 300, Some(200));
    assert_eq!((app.ride.state(), app.ride.timer_s()), (ride::State::Paused, timer), "held across the reset");
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    let timer = app.ride.timer_s();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    assert_eq!(timer_events(&file), [(0, None), (4, Some(1)), (0, None), (4, None)]);
    to_fit_dir("autoresume_held.fit", &file);
}

/// A record of a build before byte 30 (all zeros there) of a ride paused with Start: not known paused by hand, so it
/// goes on once ridden, the Start key's stop ended by an auto start.
#[test]
fn an_older_record_of_a_ride_paused_by_hand_goes_on_once_riding() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200));
    app.press(&mut sim, Press::Short(Key::Start)); // paused by hand (a checkpoint)
    run(&mut sim, &mut app, 5000);
    // both copies' records as a build before byte 30 writes them
    let mut older = 0;
    for c in sim.emmc.image.as_chunks_mut::<512>().0 {
        if let Some(r) = crate::recorder::rec::Rec::from_sector(c).filter(|r| r.by_hand) {
            *c = crate::recorder::rec::Rec { by_hand: false, ..r }.to_sector();
            older += 1;
        }
    }
    assert!(older >= 2, "{older} records paused by hand");
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.ride.state(), ride::State::Paused);
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    assert_eq!(app.ride.state(), ride::State::Running, "not known paused by hand: gone on");
    let timer = app.ride.timer_s();
    let (file, timer_s) = saved_timer(&mut sim, &mut app);
    assert_eq!(timer_s, u64::from(timer), "the session's timer: the screen's");
    assert_eq!(timer_events(&file), [(0, None), (4, None), (0, Some(1)), (4, None)]);
    to_fit_dir("autoresume_old.fit", &file);
}

/// A ride paused with Start before the reset stays paused after it, riding or not, until Start (the recovery record
/// says so); AUTO PAUSE OFF does not make a resumed ride go on before it is ridden, and once gone on it is never
/// auto-paused.
#[test]
fn a_ride_paused_by_hand_stays_paused_after_a_reset_and_auto_pause_off_waits_for_riding() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200));
    app.press(&mut sim, Press::Short(Key::Start)); // paused by hand (a checkpoint)
    run(&mut sim, &mut app, 5000);
    let timer = app.ride.timer_s();
    let (mut sim, mut app) = reboot(&mut sim, 3);
    run(&mut sim, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 300, Some(200));
    assert_eq!((app.ride.state(), app.ride.timer_s()), (ride::State::Paused, timer), "paused by hand: riding or not");
    app.press(&mut sim, Press::Short(Key::Start)); // resume
    ride_passes(&mut sim, &mut app, 70_000, POLL_MS, 300, Some(200)); // a checkpoint since: no longer paused by hand
    assert_eq!(app.ride.state(), ride::State::Running);

    // AUTO PAUSE OFF: back paused, stopped it stays so; ridden it goes on, then stopped it is not auto-paused
    let (mut sim, mut app) = reboot(&mut sim, 6);
    run(&mut sim, &mut app, 1000); // the resume reads BIKE's settings: OFF set after it
    app.settings.auto_pause = crate::AUTO_PAUSE_KMH.iter().position(|&k| k == 255).unwrap();
    let back = app.ride.timer_s();
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 0, None);
    assert_eq!((app.ride.state(), app.ride.timer_s()), (ride::State::Paused, back), "OFF: not gone on standing still");
    ride_passes(&mut sim, &mut app, 1000, POLL_MS, 300, Some(200));
    ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 0, None);
    assert!(app.ride.moving() && app.ride.timer_s() >= back + 10, "OFF: never auto-paused once gone on");
}

/// A card that fails during a 10 min stall and for 5 s after it: the stall's seconds go on the timer, the few
/// records read wait in the buffer, none is dropped, and they are on the card once it answers.
#[test]
fn a_card_failing_during_and_after_a_stall_drops_nothing() {
    let (mut sim, mut app, t, name) = ride_then_stall(0, 300);
    sim.emmc.bad = std::vec![0..u32::MAX];
    sim.delay_ms(600_000);
    ride_passes(&mut sim, &mut app, 5000, POLL_MS, 300, Some(200));
    sim.emmc.bad.clear();
    ride_passes(&mut sim, &mut app, 20_000, POLL_MS, 300, Some(200));
    assert!(app.ride.timer_s().abs_diff(t + 625) <= 1, "timer {}", app.ride.timer_s());
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert_eq!((app.rec.clean.dropped, app.rec.full.dropped), (0, 0));
    let (times, ..) = saved_ride(&sim, name);
    assert!(times.windows(2).all(|p| p[1] > p[0]));
}

/// A ride ended right after a 10 min stall, by Start then the end screen (keys) or by END RIDE (the ride menu):
/// the saved session's timer has the stall.
#[test]
fn a_ride_ended_after_a_stall_keeps_it() {
    for menu in [false, true] {
        let (mut sim, mut app, t, name) = ride_then_stall(600, 300);
        app.step(&mut sim);
        let timer = app.ride.timer_s();
        assert!(timer >= t + 600, "{menu}: timer {timer}");
        if menu {
            app.press(&mut sim, Press::Long(Key::Page));
            super::menu_tests::to_row(&mut sim, &mut app, crate::menu::Row::Ride(crate::menu::RideRow::End));
            app.press(&mut sim, Press::Short(Key::Start));
            app.press(&mut sim, Press::Short(Key::Start)); // SAVE, selected
        } else {
            app.press(&mut sim, Press::Short(Key::Start));
            save_ride(&mut sim, &mut app);
        }
        run(&mut sim, &mut app, 3000);
        assert_eq!(app.ride.state(), ride::State::Idle, "{menu}: saved");
        let (_, _, timer_s, _) = saved_ride(&sim, name);
        assert!(timer_s.abs_diff(u64::from(timer)) <= 1, "{menu}: session {timer_s} s, timer {timer}");
    }
}

/// The RTC stepping forward by more than an hour mid-ride (no setter of this firmware does it, the dev link's poke
/// could) is a clock step, not a stall: one second counted, as for a step back.
#[test]
fn a_clock_step_counts_one_second() {
    for step in [2 * 3600i64, -600] {
        let (mut sim, mut app, t, _) = ride_then_stall(0, 300);
        sim.start_s = (sim.start_s as i64 + step) as u64;
        ride_passes(&mut sim, &mut app, 10_000, POLL_MS, 300, Some(200));
        assert!(app.ride.timer_s().abs_diff(t + 10) <= 1, "{step}: timer {} from {t}", app.ride.timer_s());
    }
}

/// A reset with the RTC days ahead (a clock set while the unit was off): the ride resumes at its last checkpoint, and
/// the jump adds nothing to it while it stays paused.
#[test]
fn a_clock_step_across_a_reboot_resumes_at_the_checkpoint() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 150_000);
    let timer = app.ride.timer_s();
    let mut sim2 = Sim::new(9, 3, 0);
    sim2.start_s += 3 * 86400;
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, 1000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    let back = app.ride.timer_s();
    assert!(back <= timer && back + 60 >= timer, "timer {back} of {timer}");
    run(&mut sim2, &mut app, 30_000);
    assert_eq!(app.ride.timer_s(), back, "paused: the jump is not ridden");
}

/// The phone sets the RTC (the only setter: refused during a ride) 3 h forward, then 5 min back: the seconds the jump
/// skips or repeats are not the ride's, so a ride started right after counts only its own.
#[test]
fn a_phone_time_set_is_not_caught_up() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    for (h, m) in [(12, 0), (11, 55)] {
        let time = [0xea, 0x07, 10, 23, h, m, 0, 5, 0, 1]; // Current Time: 2026-10-23 h:m:00
        app.phone_time(&mut sim, Some(&time), None);
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 10_000);
        let timer = app.ride.timer_s();
        assert!(timer.abs_diff(10) <= 1, "{h}:{m}: timer {timer} after 10 s");
        app.press(&mut sim, Press::Short(Key::Start));
        save_ride(&mut sim, &mut app);
        run(&mut sim, &mut app, 3000);
    }
    assert_eq!(sim.rtc_sets, 2);
}

/// A power cut during the passes after a 10 min stall (at every sector they write, torn): the stall's seconds are on
/// the card from the checkpoint its first pass asks for (`checkpoint_soon`); a cut in that one loses the stall and
/// what came before it since the last checkpoint (the chunk the stall was), a later one at most the last minute; the
/// ride resumes, the volume clean.
#[test]
fn a_cut_after_a_stall_resumes_at_the_last_checkpoint() {
    let setup = || {
        let (mut sim, app, t, _) = ride_then_stall(600, 300);
        sim.emmc.tear = true;
        let base = sim.emmc.sectors_written;
        (sim, app, base, t)
    };
    let (mut sim, mut app, base, _) = setup();
    app.step(&mut sim);
    let first = sim.emmc.sectors_written - base;
    assert!(first > 0, "the stall's checkpoint in its first pass");
    run(&mut sim, &mut app, 65_000); // and the next one
    let total = sim.emmc.sectors_written - base;
    for cut in 0..total {
        let (mut sim, mut app, base, t) = setup();
        sim.emmc.cut_after = Some(base + cut);
        let mut timer = app.ride.timer_s();
        while !sim.emmc.dead {
            app.step(&mut sim);
            sim.sleep_ms(POLL_MS);
            timer = app.ride.timer_s();
        }
        let (mut sim, mut app) = reboot(&mut sim, 20);
        run(&mut sim, &mut app, 1000);
        assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "cut {cut}: {}", app.notes.list[0].as_str());
        let back = app.ride.timer_s();
        let lost_from = if cut < first { t } else { timer };
        assert!(back <= timer && back + 60 >= lost_from, "cut {cut}: timer {back} of {timer} ({t} before the stall)");
        crate::fat::write::tests::fsck_torn(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
    }
}

/// The review of 2026-11-11, R2: a ride started before the first fix with a stale UTC offset in the backup registers
/// (stock moved the RTC, or an earlier zone). The fix mid-ride must not learn the new offset: the FIT times are the
/// RTC less it, and they would jump by an hour. Learnt once the ride is saved, as the phone's is (`phone_time`).
#[test]
fn the_utc_offset_is_not_learnt_mid_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    // 3 h, where the simulated receiver says 2 (the RTC runs on UTC + 2 h: sim/gnss_model.rs)
    [sim.backup[17], sim.backup[18]] = crate::gnss::Aid { offset_15min: Some(12), pos: None }.to_words();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 30_000);
    sim.gnss.fix = Some(5000);
    run(&mut sim, &mut app, 90_000);
    assert_eq!(app.gps.aid.offset_15min, Some(12), "kept while the ride is open");
    app.press(&mut sim, Press::Short(Key::Start));
    let name = std::string::String::from(core::str::from_utf8(&app.rec.clean.name()).unwrap());
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    for path in [std::format!("Activities/{name}.fit"), std::format!("Rustfw/Activities/{name}.fit")] {
        let times = record_times(&sim.emmc.image, &path);
        assert!(times.len() >= 115, "{path}: {} records", times.len());
        assert!(times.windows(2).all(|p| p[1] == p[0] + 1), "{path}: one a second, no jump");
    }
    assert_eq!(app.gps.aid.offset_15min, Some(8), "learnt at a fix once the ride is saved");
}

/// The review of 5f589691: a resume with the clock a few minutes behind the ride's last record (stock moved the RTC
/// back). The full copy has every second the clean one has: it recorded the clamped seconds, then dropped 120 s once
/// the clock came within two checkpoints of its last record.
#[test]
fn a_resume_with_the_clock_behind_keeps_every_second_in_the_full_copy() {
    let (mut sim, mut app, name, _) = ride_70s();
    run(&mut sim, &mut app, 55_000); // past the second checkpoint: the last record about 09:02:06
    let mut sim2 = Sim::new(8, 58, 0); // about 4 min behind
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    let mut sim = sim2;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app.notes.list[0].as_str());
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 480_000);
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 5000);
    assert!(app.notes.list[0].as_str().ends_with("RIDE SAVED"), "{}", app.notes.list[0].as_str());
    let clean = record_times(&sim.emmc.image, &std::format!("Activities/{name}.fit"));
    let full = record_times(&sim.emmc.image, &std::format!("Rustfw/Activities/{name}.fit"));
    assert!(clean.len() >= 120 + 470, "{} clean records", clean.len());
    assert!(full.windows(2).all(|p| p[1] > p[0]), "full copy in order");
    assert!(full.len() + 2 >= clean.len(), "{} full records of {}", full.len(), clean.len());
}
