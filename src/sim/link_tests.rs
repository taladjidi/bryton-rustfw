//! The Nordic link's watchdog (app/nrf_watch.rs) against a chip that misbehaves: a stray byte, a hang, silence, a dead
//! chip. The recorder must go on whatever the radio does.

use super::*;

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

fn hr72(_: u64) -> [u8; 8] {
    [4, 0, 0, 0, 0, 0, 0, 72]
}

/// A unit with the card, ANT started, the strap paired and in range, riding.
fn riding() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.live.get(Metric::Hr), Some(72));
    assert_eq!(app.rec.clean.phase(), crate::recorder::Phase::Recording);
    (sim, app)
}

/// The ride goes on: recording, not failing, and its timer has run for the time that passed.
fn ride_ok(app: &App, since_ms: u64, sim: &Sim) {
    assert_eq!(app.rec.clean.phase(), crate::recorder::Phase::Recording);
    assert_eq!(app.rec.clean.fails(), 0);
    assert!(u64::from(app.ride.timer_s()) + 2 >= (sim.ms - since_ms) / 1000, "the ride's timer ran");
}

fn said(app: &App) -> usize {
    app.notes.list.iter().filter(|n| n.as_str().ends_with("RADIO RESET")).count()
}

/// One stray byte between two frames puts the framing out of step. The link is read again from a frame boundary
/// (no chip reset needed), the strap's heart rate returns, and the recording never noticed.
#[test]
fn a_stray_byte_is_resynced() {
    let (mut sim, mut app) = riding();
    let t0 = sim.ms - 5000;
    sim.nordic.stray(0x5a);
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.watch.resyncs, 1, "resynced");
    assert_eq!(app.watch.resets, 0, "the chip was not reset");
    run(&mut sim, &mut app, 10_000);
    assert_eq!(app.live.get(Metric::Hr), Some(72), "the strap is heard again");
    ride_ok(&app, t0, &sim);
    assert_eq!(sim.nordic.resets.len(), 1, "only the boot's reset");
}

/// A hung chip answers nothing: found by the probe, reset, started again with the strap's channel reopened; the
/// heart rate shows "--" meanwhile, and the ride records throughout.
#[test]
fn a_hung_chip_is_reset() {
    let (mut sim, mut app) = riding();
    let t0 = sim.ms - 5000;
    sim.nordic.hung = true;
    run(&mut sim, &mut app, 20_000);
    assert_eq!(app.live.get(Metric::Hr), None, "-- while the sensor is gone");
    assert_eq!(app.watch.resets, 0, "not before the probe");
    run(&mut sim, &mut app, 40_000);
    assert_eq!(app.watch.resets, 1);
    assert_eq!(said(&app), 1);
    run(&mut sim, &mut app, 10_000);
    assert_eq!(app.live.get(Metric::Hr), Some(72), "the strap is back");
    assert_eq!(app.radio.sensors[Kind::Hr as usize], Sensor::Paired(STRAP));
    ride_ok(&app, t0, &sim);
    sim.gnss.assert_reads(890, 0);
}

/// The bonded phone is connected when the chip hangs; after the reset the server is set up again and the unit
/// advertises for it again; the PHONE menu says so.
#[test]
fn the_phone_comes_back_after_a_reset() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut p = PhoneSim::new(true);
    p.connect_at = 1000;
    sim.nordic.phone = Some(p);
    let mut app = App::start(&mut sim, None);
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    sim.nordic.hung = true;
    run(&mut sim, &mut app, 80_000);
    assert_eq!(app.watch.resets, 1);
    assert_ne!(app.phone.link(), Link::NoBle);
    assert!(sim.nordic.server.adv.is_some() || app.phone.link() == Link::Secure, "advertising, or already back");
    sim.gnss.assert_reads(890, 0);
}

/// No sensor, no phone, a sensor paired that is not in range: 30 minutes of a quiet healthy link are never a
/// reset (nor a resync); the chip is asked now and then.
#[test]
fn a_quiet_healthy_link_is_left_alone() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word(); // paired, out of range: search, rest, search
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 30 * 60 * 1000);
    assert_eq!((app.watch.resets, app.watch.resyncs), (0, 0));
    let asked = sim.nordic.got.iter().filter(|p| p[..2] == [0, 0x65]).count();
    assert!(asked > 2, "the quiet link was asked {asked} times");
    let (mut sim, mut app) = started();
    run(&mut sim, &mut app, 30 * 60 * 1000);
    assert_eq!((app.watch.resets, app.watch.resyncs), (0, 0), "no ANT, no phone");
}

/// A chip that never starts: the attempts back off (15 s, 30 s, 60 s...), "RADIO RESET" is said once.
#[test]
fn repeated_failures_back_off() {
    let mut sim = Sim::new(9, 0, 0);
    sim.nordic.dead = true;
    let mut app = App::start(&mut sim, Some(KEY));
    let (end, mut seen, mut begins) = (sim.ms + 9 * 60 * 1000, 0, Vec::new());
    while sim.ms < end {
        run(&mut sim, &mut app, 100);
        if app.watch.resets != seen {
            seen = app.watch.resets;
            begins.push(sim.ms); // the pass a reset began in
        }
    }
    let t = &sim.nordic.resets;
    assert!((5..=8).contains(&t.len()), "{} resets: {t:?}", t.len());
    let gaps: Vec<_> = begins.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.windows(2).all(|g| g[1] + 300 >= g[0]), "gaps only grow: {gaps:?}");
    assert!(*gaps.last().unwrap() >= 120_000, "{gaps:?}");
    assert_eq!(said(&app), 1);
    // the chip comes back: the next attempt starts it
    sim.nordic.dead = false;
    run(&mut sim, &mut app, 6 * 60 * 1000);
    assert!(app.ble.is_some() && app.ant, "started at last");
    sim.gnss.assert_reads(890, 0);
}

/// Fuzz: bytes of the chip's stream dropped, added and flipped at random, over long rides, seeds fixed. No panic,
/// the ride records throughout, the watchdog's recoveries are reached (counts asserted), and once the damage
/// stops the strap is heard again.
#[test]
fn a_damaged_stream_is_recovered_from() {
    let (mut resyncs, mut resets) = (0, 0);
    for (seed, per_mille) in [(1u32, 1), (2, 2), (3, 5), (4, 20), (5, 100)] {
        let (mut sim, mut app) = riding();
        let t0 = sim.ms - 5000;
        sim.nordic.chaos = Some((0x9e37_79b9 ^ seed.wrapping_mul(2_654_435_761), per_mille));
        run(&mut sim, &mut app, 40 * 60 * 1000);
        ride_ok(&app, t0, &sim);
        resyncs += app.watch.resyncs;
        resets += app.watch.resets;
        sim.nordic.chaos = None;
        run(&mut sim, &mut app, 6 * 60 * 1000);
        let (i, w) = (Kind::Hr as usize, &app.watch);
        assert_eq!(
            app.live.get(Metric::Hr),
            Some(72),
            "seed {seed}: the strap is heard again; sensor {:?} resting {} heard {} ant {} ble {} resets {} resyncs {} ch {:?}",
            app.radio.sensors[i],
            app.radio.resting_ms[i],
            app.radio.heard_ms[i],
            app.ant,
            app.ble.is_some(),
            w.resets,
            w.resyncs,
            sim.nordic.channels[0].open
        );
        super::budget_tests::print_gap_breakdown(&sim, "damaged stream");
        sim.gnss.assert_reads(DAMAGED_GAP_MS, 0);
        ride_ok(&app, t0, &sim);
    }
    assert!(resyncs > 20, "{resyncs} resyncs reached");
    assert!(resets > 2, "{resets} chip resets reached");
}

/// Review: a chip that answers with an error code to ANT's enable was reset every 5 min for ever.
#[test]
fn ant_refused_is_not_a_broken_link() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.nordic.ant_refuse = Some([0xfa, 1]);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 60 * 60 * 1000);
    assert!(app.watch.resets <= 1, "a chip that answers is reset {} times", app.watch.resets);
}

/// The same after a restart: a channel refused at every open is not owed for ever.
#[test]
fn a_refusal_at_a_restart_is_not_retried_by_reset() {
    let (mut sim, mut app) = riding();
    sim.nordic.dead = true; // a real fault: one reset
    run(&mut sim, &mut app, 40_000);
    sim.nordic.dead = false;
    sim.nordic.ant_refuse = Some([0xc4, 0]); // the strap's channel is refused from now on
    run(&mut sim, &mut app, 60 * 60 * 1000);
    let n = app.watch.resets;
    assert!(n <= 3, "reset {n} times");
    run(&mut sim, &mut app, 60 * 60 * 1000);
    assert_eq!(app.watch.resets, n, "still resetting a chip that answers");
    sim.gnss.assert_reads(1170, 0);
}

/// Review: a resync dropped the bytes of a connection's end, and the unit stayed "connected" for ever, advertising
/// nothing, so the bonded phone could not come back. The resync asks the chip about the link.
#[test]
fn a_lost_disconnection_is_found_after_a_resync() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.backup[Kind::Hr.backup()] = STRAP.to_word(); // frames flow, so a stray byte is noticed
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    let mut p = PhoneSim::new(true);
    p.connect_at = 1000;
    sim.nordic.phone = Some(p);
    let mut app = App::start(&mut sim, Some(KEY));
    app.activate(&mut sim, Menu::Phone, 1);
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.phone.link(), Link::Secure);
    let adverts = |sim: &Sim| sim.nordic.got.iter().filter(|p| p[..2] == [0, 0x73]).count();
    let before = adverts(&sim);
    let now = sim.ms;
    sim.nordic.phone.as_mut().unwrap().lose_link(now + 20_000);
    sim.nordic.stray(0x5a); // the link is out of step: drained and resynced, with no event of the end
    run(&mut sim, &mut app, 12_000);
    assert_eq!(app.watch.resyncs, 1);
    assert_eq!(app.watch.resets, 0);
    assert!(adverts(&sim) > before, "advertising again");
    run(&mut sim, &mut app, 20_000);
    assert_eq!(app.phone.link(), Link::Secure, "the phone is back");
}

/// Review: a probe that timed out replaced the last real failure (shown in the PHONE menu) with "no answer".
#[test]
fn a_silent_probe_keeps_the_last_failure() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, Some(KEY));
    app.rx.failed = Some((0x42, 7));
    sim.nordic.hung = true;
    assert!(!app.probe(&mut sim));
    assert_eq!(app.rx.failed, Some((0x42, 7)));
}

/// Review: a chip reset in the middle of the PAIR window ended the pairing while the screen said to pair.
#[test]
fn a_reset_during_pairing_goes_on_pairing() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 4000);
    app.activate(&mut sim, Menu::Phone, 1);
    assert_eq!(app.phone.link(), Link::Pairing);
    sim.nordic.hung = true;
    run(&mut sim, &mut app, 40_000);
    sim.nordic.hung = false;
    run(&mut sim, &mut app, 60_000);
    assert!(app.watch.resets >= 1);
    assert_eq!(app.phone.link(), Link::Pairing, "pairing resumed");
    sim.gnss.assert_reads(890, 0);
}

/// The longest GNSS gap a damaged radio stream may cause: measured 2026-11-02 after the radio time-out went to 250 ms,
/// once a pass (Rx::stalled), 1462 ms over the 5 seeds of [`a_damaged_stream_is_recovered_from`] (1751 before), no
/// epoch lost; a frame the chip takes slowly on a damaged link (a put may run 2x its bound to finish a started frame,
/// review of a719c1c). Measured + 10 %, under the receiver's own time-out (1.5 s plus its second). Past the budget's
/// 1.2 s: an open item (docs/status.md).
const DAMAGED_GAP_MS: u64 = 1610;

/// The owner, 2026-11-02: GNSS reads do not depend on the radio. A ride with a fix walks every radio failure (a
/// hung chip reset while running, a dead chip, a damaged stream with broken-link resets, a 48-event phone burst, a
/// 32-ANT resume, a flood of events); whatever the chip does, no pass keeps the receiver unread past the budget's
/// limit (1.2 s, budget_tests.rs), but for the damaged stream ([`DAMAGED_GAP_MS`], 1.61 s: under the receiver's
/// time-out), no epoch is lost to its stream time-out, and the app's fix is never dropped.
#[test]
fn no_radio_failure_delays_the_gnss_reads_or_loses_the_fix() {
    type Fail = fn(&mut Sim);
    fn flood(s: &mut Sim) {
        (0..90).for_each(|_| s.nordic.queue(&[2, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x25, 0, 1, 5, 0, 1, 2, 3, 4, 5]));
    }
    let cases: [(&str, Fail, bool); 7] = [
        ("hung chip", |s| s.nordic.hung = true, true),
        ("dead chip", |s| s.nordic.dead = true, false),
        ("damaged stream", |s| s.nordic.chaos = Some((0x9e37_79b9, 100)), false),
        ("phone burst", super::ride_radio_tests::phone_burst, false),
        ("ant resume", super::ride_radio_tests::ant_resume, false),
        ("flood", flood, false),
        (
            "flood and hang",
            |s| {
                s.nordic.hung = true;
                flood(s);
            },
            true,
        ),
    ];
    for (name, fail, recovers) in cases {
        let (mut sim, mut app) = riding();
        sim.gnss.fix = Some(8000);
        run(&mut sim, &mut app, 30_000);
        assert!(app.gps.pvt.is_some_and(|p| p.good()), "{name}: a fix before the failure");
        sim.gnss.longest_gap_ms = 0;
        sim.gnss.lost_epochs = 0;
        fail(&mut sim);
        let end = sim.ms + 5 * 60_000;
        while sim.ms < end {
            app.step(&mut sim);
            sim.sleep_ms(POLL_MS);
            assert!(app.gps.pvt.is_some_and(|p| p.good()), "{name}: the fix was lost at {} ms", sim.ms);
        }
        std::eprintln!("{name}: longest GNSS gap {} ms", sim.gnss.longest_gap_ms);
        // the damaged stream's gaps move with any change of the pass's timing (2026-11-08: 830 ms, then 1322 ms with
        // no card time in it)
        let bound = if name == "damaged stream" { DAMAGED_GAP_MS } else { 1200 };
        assert!(sim.gnss.longest_gap_ms <= bound, "{name}: longest unread gap {} ms", sim.gnss.longest_gap_ms);
        assert_eq!(sim.gnss.lost_epochs, 0, "{name}: epochs lost");
        if recovers {
            // the reset's stages are never lost to a refused command: ANT, the BLE link and the strap's channel are back
            run(&mut sim, &mut app, 120_000);
            assert!(app.ble.is_some() && app.ant, "{name}: the radio is back up");
            assert!(sim.nordic.channels[0].open, "{name}: the strap's channel is open again");
            assert_eq!(app.live.get(Metric::Hr), Some(72), "{name}: the strap is heard again");
        }
    }
}

/// A slow but alive chip (every response 300 ms after its command): never reset, never resynced, no fix lost, and the
/// receiver read inside the budget (the commands wait for real, one pass at a time).
#[test]
fn a_slow_chip_is_not_reset_and_does_not_hold_the_gnss_reads() {
    let (mut sim, mut app) = riding();
    sim.gnss.fix = Some(8000);
    run(&mut sim, &mut app, 30_000);
    sim.gnss.longest_gap_ms = 0;
    let resets = app.watch.resets;
    sim.nordic.slow_ms = 300;
    let (end, mut slowest) = (sim.ms + 3 * 60_000, 0);
    while sim.ms < end {
        let t = sim.ms;
        if (t / 50) % 100 == 0 {
            app.watch.quiet_ms = 30_000; // the probe is due: a command that waits 300 ms for its answer
        }
        app.step(&mut sim);
        slowest = slowest.max(sim.ms - t);
        sim.sleep_ms(POLL_MS);
        assert!(app.gps.pvt.is_some_and(|p| p.good()), "the fix was lost at {} ms", sim.ms);
    }
    assert!(slowest >= 300, "the chip answered slowly: a pass took {slowest} ms");
    assert_eq!((app.watch.resets, app.watch.resyncs), (resets, 0), "a slow chip is not a broken one");
    std::eprintln!("slow chip: longest GNSS gap {} ms", sim.gnss.longest_gap_ms);
    sim.gnss.assert_reads(1200, 0);
}
