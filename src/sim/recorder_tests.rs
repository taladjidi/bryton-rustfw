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
    app.press(&mut sim2, Press::Long(Key::Start)); // end
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
    run(&mut sim, &mut app, u64::from(BOND_LOAD_MS) + 1000); // past the phone's bond read (a read-only build reads it)
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
    app.press(&mut sim, Press::Long(Key::Start)); // end
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
    app.press(&mut sim, Press::Long(Key::Start)); // end
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
    app.press(&mut sim, Press::Long(Key::Start)); // end
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
    for (k, c) in cmds.iter().enumerate().filter(|c| c.1.0 == 25) {
        assert!(k > 0 && cmds[k - 1].0 == 23 && cmds[k - 1].1 & 1 << 31 != 0, "CMD25 without a reliable CMD23");
    }
    let total = sim.emmc.sectors_written;
    let mut resumed = 0;
    for cut in 0..total {
        let (mut sim, _) = start(Some(cut));
        let (mut sim, mut app) = reboot(&mut sim, 2);
        run(&mut sim, &mut app, 1000);
        resumed += usize::from(app.notices[0].as_str().ends_with("RIDE RESUMED"));
        let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
    }
    std::println!("{total} cuts, {resumed} resumed");
    assert!(resumed > 0);
}
