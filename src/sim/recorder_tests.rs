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
