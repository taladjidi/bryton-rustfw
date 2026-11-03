//! The main loop's idle Stop (docs/power.md "Stop mode", phase 2a) through the unit's own pass: when it is chosen
//! (an idle pass a minute after the start, no ride, no cable but for the bench, the kill switch), that every wake
//! source ends it at once, and that the app clock keeps the RTC's time across thousands of them.

use super::loop_model::SimLink;
use super::*;
use crate::dev::{Effect, UsbWatch};
use crate::{IDLE_POLL_MS, MainLoop, POLL_MS, STOP_AFTER_BOOT_MS, STOP_FAILS_MAX};

/// A started unit that never switches itself off (AUTO OFF never), its loop, and a Stop's wake of 1.5 ms of work.
fn unit() -> (Sim, App, MainLoop<SimLink>) {
    let (mut sim, mut app) = started();
    app.settings.auto_off = 0;
    sim.main.wake_ns = 1_500_000;
    (sim, app, MainLoop::new(<Sim as Board>::CPU_HZ, 80))
}

/// Passes until the simulated clock reaches `ms`.
fn until(sim: &mut Sim, app: &mut App, main: &mut MainLoop<SimLink>, ms: u64) {
    while sim.ms < ms {
        main.pass(app, sim);
    }
}

#[test]
fn idle_passes_stop_after_the_first_minute() {
    let (mut sim, mut app, mut main) = unit();
    until(&mut sim, &mut app, &mut main, u64::from(STOP_AFTER_BOOT_MS) - 1000);
    assert!(sim.main.stops.is_empty(), "no Stop in the first minute");
    until(&mut sim, &mut app, &mut main, 120_000);
    let n = sim.main.stops.len();
    assert!(n > 200, "{n} Stops in the idle minute");
    // the whole idle pass in one Stop, ended by its timer
    assert!(sim.main.stops.iter().all(|&(ms, us)| ms <= IDLE_POLL_MS && us >= ms * 1000), "{:?}", sim.main.stops);
    let [stops, fails, ms] = main_stops(&main);
    assert_eq!((stops as usize, fails), (n, 0));
    assert!(ms >= 200 * IDLE_POLL_MS, "{ms} ms in Stop");
    assert!(app.prof.slots[crate::prof::Sec::Wake as usize].count as usize >= n, "the wake is its own section");
}

/// The loop's Stop counts, as `info` reports them.
fn main_stops(main: &MainLoop<SimLink>) -> [u32; 3] {
    main.stats().stops
}

#[test]
fn the_app_clock_keeps_the_rtc_across_stops() {
    let (mut sim, mut app, mut main) = unit();
    sim.main.jitter = true;
    until(&mut sim, &mut app, &mut main, 70_000);
    // the pass's sleep is in the clock's next step: counted here, or the lag of one pass shows
    let now = |app: &App| u64::from(app.up_ms) + u64::from(app.prof.clock.sleep.0);
    let (up0, real0, rtc0) = (now(&app), sim.ms, rtc_s(&mut sim));
    until(&mut sim, &mut app, &mut main, real0 + 600_000);
    let (up, real) = (now(&app) - up0, sim.ms - real0);
    assert!(sim.main.stops.len() > 2000, "{} Stops", sim.main.stops.len());
    assert!(real.abs_diff(up) * 5000 <= real, "app clock {up} ms against real {real} ms"); // 0.02 %
    assert_eq!(rtc_s(&mut sim) - rtc0, real / 1000, "the RTC counted the Stops too");
    assert_eq!(main_stops(&main)[1], 0);
}

/// The RTC's seconds of the day.
fn rtc_s(sim: &mut Sim) -> u64 {
    let (h, m, s) = crate::rtc_time_from_tr(sim.rtc().0);
    u64::from(h) * 3600 + u64::from(m) * 60 + u64::from(s)
}

#[test]
fn each_blocker_keeps_the_pass_out_of_stop() {
    let idle = || {
        let (mut sim, mut app, mut main) = unit();
        until(&mut sim, &mut app, &mut main, 70_000);
        assert!(app.stop_ok(&mut sim));
        (sim, app, main)
    };
    type Blocker = (&'static str, fn(&mut Sim, &mut App));
    let blockers: [Blocker; 10] = [
        ("a tone (TIM1 stops in Stop)", |_, app| app.tones.play(&crate::BEEP_ALERT)),
        ("a banner timed by the passes", |_, app| app.say("BANNER")),
        ("the first minute", |_, app| app.up_ms = STOP_AFTER_BOOT_MS - 1),
        ("a resume of the ride left open pending", |_, app| app.rec.resume_pending = true),
        ("the full copy's resume pending", |_, app| app.rec.full_pending = true),
        ("the ride marked open on the card", |sim, _| sim.backup[crate::RIDE_BKP] = crate::RIDE_OPEN),
        ("settings.bin to write", |_, app| app.settings_store.file.dirty = true),
        ("pages.bin to write", |_, app| app.store.file.dirty = true),
        ("sensors.bin to write", |_, app| app.sensors_store.dirty = true),
        ("the card lent to the computer", |sim, app| {
            app.set_usb(sim, crate::ui::UsbDot::Ready);
            assert_eq!(app.drive_on(sim), Ok(()));
            app.notes.ms = None; // its banner, a blocker of its own
        }),
    ];
    for (what, block) in blockers {
        let (mut sim, mut app, _) = idle();
        block(&mut sim, &mut app);
        assert!(!app.stop_ok(&mut sim), "{what}");
    }
    // a ride open: its passes are short and never stop (phase 2b)
    let (mut sim, mut app, mut main) = idle();
    let n = sim.main.stops.len();
    app.press(&mut sim, Press::Short(Key::Start));
    until(&mut sim, &mut app, &mut main, 140_000);
    assert!(app.busy() && !app.stop_ok(&mut sim));
    assert_eq!(sim.main.stops.len(), n, "a Stop with a ride open");
}

#[test]
fn a_wake_that_does_not_restore_the_clocks_resets() {
    let (mut sim, mut app, mut main) = unit();
    sim.main.stop_broken = true;
    until(&mut sim, &mut app, &mut main, 61_000);
    assert_eq!(sim.main.resets, 0);
    while sim.main.stops.is_empty() {
        main.pass(&mut app, &mut sim);
    }
    assert_eq!(sim.main.resets, 1, "the unit may run on the HSI with a dead PLL: reset");
    let w = sim.backup[crate::stopbench::BKP[0]];
    assert_eq!(w >> 24, crate::stopbench::TAG_A, "the report kept as `bench stop` keeps it");
    assert_eq!(w >> 12 & crate::stopbench::PLL, 0, "PLL not locked again, in register 0");
}

#[test]
fn the_kill_switch_and_failures_fall_back_to_wfi() {
    let (mut sim, mut app, mut main) = unit();
    sim.main.stop_off = true; // POWER_STEPS without bit 5
    until(&mut sim, &mut app, &mut main, 120_000);
    assert!(sim.main.stops.is_empty());
    assert_eq!(main_stops(&main), [0; 3], "not built: nothing counted");
    let (mut sim, mut app, mut main) = unit();
    sim.main.stop_fails = true; // the system never reaches Stop
    until(&mut sim, &mut app, &mut main, 120_000);
    let max = usize::from(STOP_FAILS_MAX);
    assert_eq!(sim.main.stops.len(), max, "tried until {max} in a row failed, then WFI only");
    assert_eq!(main_stops(&main)[..2], [u32::from(STOP_FAILS_MAX); 2]);
    assert_eq!(app.pass_ms(), IDLE_POLL_MS, "the loop kept its idle passes");
}

/// A Stop of the idle pass ends at once on `wake` set to start at `at` ms: the pass that crossed it ended within a
/// few ms (not the 250 of the timer).
fn wakes_at_once(wake: impl FnOnce(&mut Sim, u64)) -> (Sim, App, MainLoop<SimLink>) {
    let (mut sim, mut app, mut main) = unit();
    until(&mut sim, &mut app, &mut main, 80_000);
    let n = sim.main.stops.len();
    let at = sim.ms + 100; // inside the next Stop
    wake(&mut sim, at);
    main.pass(&mut app, &mut sim);
    assert_eq!(sim.main.stops.len(), n + 1, "the pass stopped");
    let (_, us) = *sim.main.stops.last().unwrap();
    assert!((80_000..105_000).contains(&us), "the Stop slept {us} us, woken at 100 ms in");
    assert!(sim.ms < at + 20, "the pass ended at {} ms, the wake at {at}", sim.ms);
    (sim, app, main)
}

#[test]
fn a_key_ends_a_stop_at_once() {
    let (mut sim, mut app, mut main) = wakes_at_once(|sim, at| {
        sim.keys = Box::new(move |t| if (at..at + 120).contains(&t) { Key::Page as u8 } else { 0 })
    });
    main.pass(&mut app, &mut sim); // its step reads the key
    assert_eq!(app.pass_ms(), POLL_MS, "the key was read: no longer idle");
}

#[test]
fn a_touch_ends_a_stop_at_once() {
    let mut reads = 0;
    let (sim, _, _) = wakes_at_once(|sim, at| {
        reads = sim.touch.reads;
        sim.touch.set_finger(move |t| (at..at + 200).contains(&t).then_some((120, 5)))
    });
    assert!(sim.touch.reads > reads, "the panel was read after the wake");
}

#[test]
fn the_cable_ends_a_stop_and_starts_usb() {
    let (mut sim, mut app, mut main) = wakes_at_once(|sim, at| sim.main.vbus_at = Some(at));
    main.pass(&mut app, &mut sim);
    assert_eq!(sim.main.usb_starts, 1, "USB started by the pass after the wake");
    let n = sim.main.stops.len();
    main.pass(&mut app, &mut sim);
    assert_eq!(sim.main.stops.len(), n, "no Stop once the link is up");
}

#[test]
fn the_bench_allows_stop_with_the_cable_in_for_its_seconds() {
    let (mut sim, mut app, mut main) = unit();
    let link = SimLink::default();
    (sim.main.link, sim.main.vbus) = (Some(link.clone()), true);
    until(&mut sim, &mut app, &mut main, 80_000);
    assert!(app.stop_ok(&mut sim) && sim.main.stops.is_empty(), "the USB link up: no Stop");
    link.0.borrow_mut().effects.push_back(Effect::BenchIdleStop(10));
    // a Stop takes the pass's whole sleep, so the link is polled only by the passes that do not stop: the drop the
    // watch decides at the bench's end is the bench's, not logged
    until(&mut sim, &mut app, &mut main, 85_000);
    link.0.borrow_mut().watches.push_back(UsbWatch::Drop);
    until(&mut sim, &mut app, &mut main, 100_000);
    let n = sim.main.stops.len();
    assert!((30..=41).contains(&n), "{n} Stops in the bench's 10 s");
    assert!(link.0.borrow().watches.is_empty(), "the watch ran after the bench");
    let drops = crate::USB_DROPS_BKP;
    assert_eq!(sim.backup[drops], 0, "the bench's drop counted");
    until(&mut sim, &mut app, &mut main, 120_000);
    assert_eq!(sim.main.stops.len(), n, "the bench's time ran out");
    link.0.borrow_mut().watches.push_back(UsbWatch::Drop);
    until(&mut sim, &mut app, &mut main, 121_000);
    assert_eq!(sim.backup[drops], 0x0b00_0001, "a drop well after the bench is logged");
}
