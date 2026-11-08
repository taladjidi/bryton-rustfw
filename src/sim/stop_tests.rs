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
    let [stops, fails, ms, ride, ..] = main_stops(&main);
    assert_eq!(ride, 0, "no ride open");
    assert_eq!((stops as usize, fails), (n, 0));
    assert!(ms >= 200 * IDLE_POLL_MS, "{ms} ms in Stop");
    assert!(app.prof.slots[crate::prof::Sec::Wake as usize].count as usize >= n, "the wake is its own section");
}

/// The loop's Stop counts, as `info` reports them.
fn main_stops(main: &MainLoop<SimLink>) -> [u32; 6] {
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
    // a ride open: never an idle Stop; its own (phase 2b) take at most the rest of a short pass
    let (mut sim, mut app, mut main) = idle();
    let n = sim.main.stops.len();
    app.press(&mut sim, Press::Short(Key::Start));
    until(&mut sim, &mut app, &mut main, 140_000);
    assert!(app.busy() && !app.stop_ok(&mut sim));
    assert!(sim.main.stops[n..].iter().all(|&(ms, _)| ms <= POLL_MS), "a ride's Stop longer than its pass");
    assert_eq!(sim.main.stops.len() - n, sim.main.ride_stops as usize);
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
    assert_eq!(main_stops(&main), [0; 6], "not built: nothing counted");
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

/// A unit with the cable in and the link up from the start of the run.
fn linked() -> (Sim, App, MainLoop<SimLink>, SimLink) {
    let (mut sim, app, main) = unit();
    let link = SimLink::default();
    sim.gnss.expect = Some("stream time-out"); // the stall is on purpose
    (sim.main.link, sim.main.vbus) = (Some(link.clone()), true);
    (sim, app, main, link)
}

fn recovers(link: &SimLink) -> usize {
    link.0.borrow().log.iter().filter(|l| **l == "recover").count()
}

fn bench_and_stall(link: &SimLink, bench: bool) {
    if bench {
        link.0.borrow_mut().effects.push_back(Effect::BenchIdleStop(10));
    } else {
        link.0.borrow_mut().stall_ms = 5000; // `bench stall` is not counted by the loop's clock, a long job is
    }
}

#[test]
fn a_link_up_after_the_bench_or_a_stall_is_left_alone() {
    let (mut sim, mut app, mut main, link) = linked();
    until(&mut sim, &mut app, &mut main, 80_000);
    bench_and_stall(&link, true);
    until(&mut sim, &mut app, &mut main, 110_000);
    bench_and_stall(&link, false);
    until(&mut sim, &mut app, &mut main, 140_000);
    assert_eq!((recovers(&link), main.stats().usb_recoveries), (0, 0));
}

#[test]
fn a_link_stuck_down_after_a_stall_is_reconnected_three_times_only() {
    let (mut sim, mut app, mut main, link) = linked();
    until(&mut sim, &mut app, &mut main, 80_000);
    link.0.borrow_mut().rest = Some(UsbWatch::Down);
    bench_and_stall(&link, false);
    until(&mut sim, &mut app, &mut main, 200_000);
    assert_eq!((recovers(&link), main.stats().usb_recoveries), (3, 3));
}

#[test]
fn a_link_suspended_after_the_bench_is_reconnected_but_not_after_a_stall() {
    let (mut sim, mut app, mut main, link) = linked();
    until(&mut sim, &mut app, &mut main, 80_000);
    link.0.borrow_mut().rest = Some(UsbWatch::Suspended);
    bench_and_stall(&link, false);
    until(&mut sim, &mut app, &mut main, 120_000);
    assert_eq!(recovers(&link), 0, "a host that sleeps is left alone");
    bench_and_stall(&link, true);
    until(&mut sim, &mut app, &mut main, 200_000);
    assert_eq!(recovers(&link), 3, "the port the Mac forgot (the unit, 2026-11-03)");
    assert_eq!(main.stats().usb_recoveries, 3);
}

#[test]
fn a_drop_is_not_a_recovery() {
    let (mut sim, mut app, mut main, link) = linked();
    until(&mut sim, &mut app, &mut main, 80_000);
    link.0.borrow_mut().rest = Some(UsbWatch::Drop);
    bench_and_stall(&link, true);
    until(&mut sim, &mut app, &mut main, 200_000);
    assert_eq!((recovers(&link), main.stats().usb_recoveries), (0, 0));
}

#[test]
fn a_new_bench_resets_the_tries() {
    let (mut sim, mut app, mut main, link) = linked();
    until(&mut sim, &mut app, &mut main, 80_000);
    link.0.borrow_mut().rest = Some(UsbWatch::Down);
    bench_and_stall(&link, true);
    until(&mut sim, &mut app, &mut main, 106_000);
    let first = recovers(&link);
    assert!((1..=2).contains(&first), "{first} tries so far");
    bench_and_stall(&link, true);
    until(&mut sim, &mut app, &mut main, 250_000);
    assert_eq!(recovers(&link), first + 3);
}

#[test]
fn a_charger_with_no_host_is_never_reconnected_after_a_stall() {
    let (mut sim, mut app, mut main, link) = linked();
    link.0.borrow_mut().rest = Some(UsbWatch::Down);
    until(&mut sim, &mut app, &mut main, 80_000);
    for _ in 0..3 {
        bench_and_stall(&link, false);
        let to = sim.ms + 20_000;
        until(&mut sim, &mut app, &mut main, to);
    }
    assert_eq!(recovers(&link), 0);
}

/// A heart-rate strap at 72 bpm: page 0 with the last beat's time (1/1024 s) and count moving on with the clock, so the
/// RR intervals (HRV, the full copy's `hrv` messages) come from the broadcasts' own timing, as on a strap.
fn hr_beats(t: u64) -> [u8; 8] {
    let n = t / 833;
    let at = (n * 833 * 1024 / 1000) as u16;
    [0, 0, 0, 0, at as u8, (at >> 8) as u8, n as u8, 72]
}

/// What one ride left: the clean copy's record times, its laps, its session's timer (s) and distance (cm), the full
/// copy's `hrv` messages, the auto-pauses in its events, the radio (hangs, resets, broadcasts per kind, ANT events
/// lost), the ride's Stops and its passes.
struct RideOut {
    times: Vec<u32>,
    laps: usize,
    timer_s: u64,
    dist_cm: u64,
    hrv: usize,
    auto_pauses: usize,
    radio: (u32, usize, [u32; 3], u16),
    stops: u32,
    passes: u32,
    high_water: usize,
    radio_pending: u32,
}

/// An hour's ride through the unit's own pass, the ride's Stop built or not (`ride_stop`): three sensors (heart rate
/// with beats, power, Di2) and the phone's notifications under the Nordic chip's strict limits, the GNSS model; 30 km/h
/// with an auto-pause (2 min stopped), a lap, a pause with Start (2 min), then the save. After every pass that took a
/// ride's Stop the recorders had nothing due (the step before did their card work), and the card model asserts no
/// Stop begins inside card work.
fn hour_ride(ride_stop: bool) -> RideOut {
    use super::ride_radio_tests::radio_unit;
    let image = crate::fat::tests::image();
    let (mut sim, mut app) = radio_unit(62, board_wait_us(), image, false, |s| s.nordic.sensors[0].data = hr_beats);
    app.settings.auto_off = 0;
    sim.main.wake_ns = 1_500_000;
    sim.main.ride_stop_off = !ride_stop;
    let mut main = MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    until(&mut sim, &mut app, &mut main, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    let (start, resets0, heard0, lost0) =
        (sim.ms, sim.nordic.resets.len(), app.radio_counts().1, app.ant_events_lost());
    let name = app.rec.clean.name();
    let (mut passes, mut lapped, mut paused, mut resumed) = (0, false, false, false);
    while sim.ms < start + 3_600_000 {
        let min = (sim.ms - start) / 60_000;
        if min >= 30 && !lapped {
            lapped = true;
            app.press(&mut sim, Press::Short(Key::Lap));
        }
        if min >= 40 && !paused {
            paused = true;
            app.press(&mut sim, Press::Short(Key::Start));
            assert_eq!(app.ride.state(), ride::State::Paused);
        }
        if min >= 42 && !resumed {
            resumed = true;
            app.press(&mut sim, Press::Short(Key::Start));
            assert_eq!(app.ride.state(), ride::State::Running);
        }
        app.live.set(Metric::Speed, if (20..22).contains(&min) { 0 } else { 300 });
        let n = sim.main.ride_stops;
        main.pass(&mut app, &mut sim);
        passes += 1;
        if sim.main.ride_stops > n {
            assert!(!app.rec.clean.due() && !app.rec.full.due(), "a ride's Stop with card work due");
        }
    }
    // the receiver read as often with Stops as without: no fix waits past its second (ride_radio_tests' bound, 816 ms),
    // but for a card stall landing in that gap (every 1024 sectors written, 300 ms: where it lands moves with the
    // writes, 2026-11-08 a folder's entry written twice; budget_tests.rs bounds the stalls)
    let stall = (sim.worst_gnss_gap.1[5] / 1_000_000).min(sim.cost.stall_ms); // one stall at most
    sim.gnss.assert_reads(900 + stall, 0);
    let heard = app.radio_counts().1;
    let radio = (
        sim.nordic.hangs,
        sim.nordic.resets.len() - resets0,
        [Kind::Hr, Kind::Power, Kind::Di2].map(|k| heard[k as usize] - heard0[k as usize]),
        app.ant_events_lost() - lost0,
    );
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    let end = sim.ms + 3000;
    until(&mut sim, &mut app, &mut main, end);
    assert_eq!(app.ride.state(), ride::State::Idle, "saved");
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    let times = super::recorder_tests::record_times(&sim.emmc.image, &path);
    let file = super::recorder_tests::ride_file(&sim, "Activities", name).unwrap();
    let full = super::recorder_tests::ride_file(&sim, "Rustfw/Activities", name).unwrap();
    use crate::fit::tests::{decode, field};
    let m = decode(&file).unwrap();
    let session = m.iter().find(|x| x.0 == 18).unwrap();
    let auto_pauses = super::recorder_tests::timer_events(&file).iter().filter(|e| e == &&(4, Some(1))).count();
    RideOut {
        times,
        laps: m.iter().filter(|x| x.0 == 19).count(),
        timer_s: field(session, 8).unwrap() / 1000,
        dist_cm: field(session, 9).unwrap(),
        hrv: decode(&full).unwrap().iter().filter(|x| x.0 == 78).count(),
        auto_pauses,
        radio,
        stops: sim.main.ride_stops,
        passes,
        high_water: sim.nordic.high_water,
        radio_pending: sim.main.radio_pending,
    }
}

/// The ride's Stop (phase 2b) costs the ride nothing: an hour with Stops between most of its passes records a
/// record every second of its timer, no second twice or skipped, the same laps, timer, distance and HRV as the same
/// ride without, and the radio as healthy (no hang, no reset, no broadcast or ANT event lost beyond the ride without).
#[test]
fn an_hour_ride_with_stops_records_as_one_without() {
    let (on, off) = (hour_ride(true), hour_ride(false));
    std::println!(
        "ride Stops {} of {} passes ({} not tried for the radio); chip queue high-water {} (without: {}); records {}, \
         timer {} s, laps {}, auto-pauses {}, hrv {}, radio {:?} (without: {:?})",
        on.stops,
        on.passes,
        on.radio_pending,
        on.high_water,
        off.high_water,
        on.times.len(),
        on.timer_s,
        on.laps,
        on.auto_pauses,
        on.hrv,
        on.radio,
        off.radio
    );
    assert_eq!(off.stops, 0, "not built: no ride Stop");
    // a pass whose end meets a byte of the chip's (in our FIFO, or landing in the 20 us look) sleeps in WFI: about 1 in 5
    assert!(on.stops * 10 >= on.passes * 7, "{} ride Stops in {} passes", on.stops, on.passes);
    // the chip held through the Stops: its queue a little higher, far from its 96 slots
    assert!(on.high_water <= off.high_water + 8 && on.high_water < 48, "queue high-water {}", on.high_water);
    // a record every second of the hour but the pause's 2 minutes (an auto-pause records on, its timer stopped), in
    // order, none twice, none skipped: one gap, the pause
    assert!(on.times.windows(2).all(|w| w[1] > w[0]), "a record time twice or back");
    let gaps: Vec<u32> = on.times.windows(2).map(|w| w[1] - w[0]).filter(|&d| d > 1).collect();
    assert!(gaps.len() == 1 && gaps[0].abs_diff(121) <= 1, "gaps in the records {gaps:?}: the pause only");
    assert!(on.times.len().abs_diff(3600 - 120) <= 1, "{} records in the hour less the pause", on.times.len());
    assert!(on.timer_s.abs_diff(3600 - 240) <= 1, "timer {} s: the hour less the pause and the auto-pause", on.timer_s);
    assert_eq!((on.laps, on.auto_pauses), (off.laps, off.auto_pauses));
    assert!(on.auto_pauses >= 1, "the auto-pause in the file");
    assert!(on.timer_s.abs_diff(off.timer_s) <= 1, "timer {} s against {} s", on.timer_s, off.timer_s);
    assert!(on.dist_cm.abs_diff(off.dist_cm) * 200 <= off.dist_cm, "{} cm against {} cm", on.dist_cm, off.dist_cm);
    assert!(on.times.len().abs_diff(off.times.len()) <= 1, "{} records against {}", on.times.len(), off.times.len());
    assert!(on.hrv > 0 && on.hrv.abs_diff(off.hrv) <= 1, "hrv messages {} against {}", on.hrv, off.hrv);
    let (hangs, resets, heard, lost) = on.radio;
    assert_eq!((hangs, resets), (0, 0), "the chip hung or was reset");
    assert!(lost <= off.radio.3, "ANT events lost {lost} against {}", off.radio.3);
    for (k, (a, b)) in heard.iter().zip(off.radio.2).enumerate() {
        assert!(a.abs_diff(b) * 1000 <= b, "broadcasts of kind {k}: {a} against {b}");
        assert!(u64::from(*a) >= 60 * 240 - 60, "broadcasts of kind {k}: {a} in an hour at 4 Hz");
    }
}

/// A unit 70 s into a ride (its Stops allowed), its loop.
fn riding() -> (Sim, App, MainLoop<SimLink>) {
    let (mut sim, mut app, mut main) = unit();
    sim.emmc.image = crate::fat::tests::image();
    until(&mut sim, &mut app, &mut main, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    until(&mut sim, &mut app, &mut main, 70_000);
    assert!(app.ride_stop_ok(&mut sim) && sim.main.ride_stops > 0, "a ride's Stops");
    (sim, app, main)
}

#[test]
fn each_blocker_keeps_a_ride_pass_out_of_stop() {
    type Blocker = (&'static str, fn(&mut Sim, &mut App));
    let blockers: [Blocker; 10] = [
        ("a tone (TIM1 stops in Stop)", |_, app| app.tones.play(&crate::BEEP_ALERT)),
        ("the first minute (a resumed ride's resets in a row)", |_, app| app.up_ms = STOP_AFTER_BOOT_MS - 1),
        ("a checkpoint due", |_, app| app.rec.clean.checkpoint_soon()),
        ("the full copy's checkpoint due", |_, app| app.rec.full.checkpoint_soon()),
        ("the card failing", |sim, app| {
            sim.emmc.dead = true;
            app.rec.clean.checkpoint_soon();
            app.step(sim);
            assert!(app.rec.clean.failing());
        }),
        ("the GNSS read inside a frame", |_, app| {
            let _ = app.gps.parser.push(0xb5);
        }),
        ("the end-ride screen, the save to come", |sim, app| app.press(sim, Press::Long(Key::Start))),
        ("a call", |_, app| app.phone.ringing = true),
        ("a key in the last second", |_, app| app.light.idle_ms = 0),
        ("a wake that broke, its report in register 0", |sim, _| {
            use crate::stopbench::{BKP, PASS_OK, PLL, StopReport, pack};
            sim.backup[BKP[0]] = pack(&StopReport { stop: true, flags: PASS_OK & !PLL, ..StopReport::default() })[0];
        }),
    ];
    for (what, block) in blockers {
        let (mut sim, mut app, _) = riding();
        block(&mut sim, &mut app);
        assert!(!app.ride_stop_ok(&mut sim), "{what}");
    }
    // the files whose writes wait for the ride's end do not keep a whole ride out of Stop
    let (mut sim, mut app, _) = riding();
    app.settings_store.file.dirty = true;
    app.store.file.dirty = true;
    app.sensors_store.dirty = true;
    assert!(app.ride_stop_ok(&mut sim), "a setting changed during the ride");
}

#[test]
fn the_ride_kill_switch_leaves_the_idle_stop() {
    let (mut sim, mut app, mut main) = unit();
    sim.emmc.image = crate::fat::tests::image();
    sim.main.ride_stop_off = true; // POWER_STEPS without bit 6
    until(&mut sim, &mut app, &mut main, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    until(&mut sim, &mut app, &mut main, 120_000);
    assert!(sim.main.stops.is_empty() && main_stops(&main) == [0; 6], "a ride's Stop not built");
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    until(&mut sim, &mut app, &mut main, 125_000);
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Idle, Screen::Summary));
    app.press(&mut sim, Press::Short(Key::Start)); // the summary closed
    until(&mut sim, &mut app, &mut main, 200_000);
    let [n, _, _, ride, ..] = main_stops(&main);
    assert!(n > 100 && ride == 0, "{n} idle Stops after the ride, {ride} of the ride");
}

#[test]
fn a_ride_wake_that_breaks_resets_once_and_stops_no_more() {
    let (mut sim, mut app, mut main) = riding();
    sim.main.stop_broken = true;
    let n = sim.main.ride_stops;
    while sim.main.ride_stops == n {
        main.pass(&mut app, &mut sim);
    }
    let reset_ms = sim.ms;
    assert_eq!(sim.main.resets, 1, "the clocks not back: a reset (the ride resumes from its checkpoint)");
    assert!(crate::stopbench::broke(sim.backup[crate::stopbench::BKP[0]]), "the report kept in register 0");
    // the ride resumed after the reset (the backup registers kept) never stops again while the report is there
    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    (sim2.emmc.rel_param, sim2.emmc.rel_sectors) = (sim.emmc.rel_param, sim.emmc.rel_sectors);
    sim2.backup = sim.backup;
    let mut app2 = App::start(&mut sim2, None);
    app2.settings.auto_off = 0;
    let mut main2 = MainLoop::<SimLink>::new(<Sim as Board>::CPU_HZ, 80);
    until(&mut sim2, &mut app2, &mut main2, 3_000);
    assert!(app2.notes.list[0].as_str().ends_with("RIDE RESUMED"), "{}", app2.notes.list[0].as_str());
    // it comes back paused, and moving starts it again by itself (any fault's resume: Ride::resumed)
    let t = sim2.ms + 30_000;
    while sim2.ms < t {
        app2.live.set(Metric::Speed, 300);
        main2.pass(&mut app2, &mut sim2);
    }
    assert_eq!(app2.ride.state(), ride::State::Running, "30 s moving after the resume");
    until(&mut sim2, &mut app2, &mut main2, 200_000);
    assert_eq!(app2.ride.state(), ride::State::Running);
    assert_eq!(sim2.main.ride_stops, 0, "a ride's Stop after a wake that broke");
    // the saved ride: its records reach within a checkpoint (60 s) of the reset, and none repeats
    let name = app2.rec.clean.name();
    app2.press(&mut sim2, Press::Short(Key::Start));
    save_ride(&mut sim2, &mut app2);
    until(&mut sim2, &mut app2, &mut main2, 205_000);
    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
    let times = super::recorder_tests::record_times(&sim2.emmc.image, &path);
    assert!(times.windows(2).all(|w| w[1] > w[0]), "a record time twice or back");
    let reset_at = times[0] + ((reset_ms - 2000) / 1000) as u32; // the ride started at 2 s
    let before = times.iter().copied().filter(|&t| t <= reset_at + 1).max().unwrap();
    assert!(reset_at - before <= 60, "the last record before the reset {} s before it", reset_at - before);
}

#[test]
fn a_ride_wake_with_the_sdram_changed_resets() {
    let (mut sim, mut app, mut main) = riding();
    sim.main.sdram_lost = true; // main.rs `image_marks` differ after the wake
    let n = sim.main.ride_stops;
    while sim.main.ride_stops == n {
        main.pass(&mut app, &mut sim);
    }
    assert_eq!(sim.main.resets, 1, "the SDRAM not back whole: a reset");
    assert!(crate::stopbench::broke(sim.backup[crate::stopbench::BKP[0]]));
}

#[test]
fn the_usb_diagnostics_keep_the_latch_while_a_ride_is_open() {
    let (mut sim, mut app, _) = riding();
    use crate::stopbench::{BKP, PASS_OK, PLL, StopReport, pack};
    let latch = pack(&StopReport { stop: true, flags: PASS_OK & !PLL, ..StopReport::default() })[0];
    sim.backup[BKP[0]] = latch;
    app.usb_snapshot(&mut sim, &crate::usbsnap::UsbSnap::default());
    app.usb_dropped(&mut sim);
    assert_eq!(sim.backup[BKP[0]], latch, "register 0 kept through a USB snapshot and a drop");
    assert!(!app.ride_stop_ok(&mut sim));
    // no ride open: the diagnostics write it as before
    app.press(&mut sim, Press::Short(Key::Start));
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.ride.state(), ride::State::Idle);
    app.usb_dropped(&mut sim);
    assert_eq!(sim.backup[BKP[0]] >> 24, 0xd1, "a drop's word once the ride is saved");
}

/// A key pressed and released during a ride leaves its EXTI flag up (a short pass never reads it): the ride's Stops go
/// on after it all the same (the loop clears the flag before each), and the press acted.
#[test]
fn a_key_on_a_ride_does_not_keep_its_stops_out() {
    let (mut sim, mut app, mut main) = riding();
    let at = sim.ms + 500;
    sim.keys = Box::new(move |t| if (at..at + 120).contains(&t) { Key::Page as u8 } else { 0 });
    let page = app.screen();
    until(&mut sim, &mut app, &mut main, at + 2000);
    assert_ne!(app.screen(), page, "the press acted");
    let (n, t) = (sim.main.ride_stops, sim.ms);
    until(&mut sim, &mut app, &mut main, t + 10_000);
    assert!(sim.main.ride_stops - n >= 150, "{} ride Stops in the 10 s after a key", sim.main.ride_stops - n);
}
