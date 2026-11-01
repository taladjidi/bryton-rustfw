//! The sensor bus, touch and key models (sensor_model.rs, touch_model.rs, key_model.rs): each rule fires, and the
//! app lives with them (survey brief 4).

use super::key_model::bounce;
use super::tests::run;
use super::*;
use crate::touch::Gesture;

fn read(sim: &mut Sim, addr: u8, reg: u8, n: usize) -> Vec<u8> {
    let mut v = vec![0; n];
    assert!(sim.i2c_read(addr, reg, &mut v));
    v
}

#[test]
fn s1_a_split_pressure_read_is_refused_under_bdu() {
    let mut sim = Sim::new(9, 0, 0);
    assert!(sim.i2c_write(crate::ambient::BARO_ADDR, 0x10, &[0x02])); // BDU
    sim.bus.expect = Some("S1");
    read(&mut sim, crate::ambient::BARO_ADDR, 0x28, 2); // stops before PRESS_OUT_H
    read(&mut sim, crate::ambient::BARO_ADDR, 0x28, 5); // whole: fine
    assert_eq!(sim.bus.violations.len(), 1);
}

#[test]
#[should_panic(expected = "S1")]
fn a_split_read_fails_loudly_unless_expected() {
    let mut sim = Sim::new(9, 0, 0);
    sim.i2c_write(crate::ambient::BARO_ADDR, 0x10, &[0x02]);
    read(&mut sim, crate::ambient::BARO_ADDR, 0x28, 1);
}

#[test]
fn the_firmware_reads_the_barometer_whole() {
    let mut sim = Sim::new(9, 0, 0);
    let chips = crate::ambient::init(&mut sim);
    assert!(chips.baro && crate::ambient::baro_start(&mut sim));
    sim.delay_ms(25);
    let (p, _) = crate::ambient::baro_read_raw(&mut sim).flatten().expect("coherent read");
    assert_eq!(p, 10_132 * 4096 / 10);
    assert!(sim.bus.violations.is_empty());
}

#[test]
fn s2_one_shot_clears_itself_and_sets_the_status() {
    let mut sim = Sim::new(9, 0, 0);
    let a = crate::ambient::BARO_ADDR;
    sim.i2c_write(a, 0x10, &[0x02]);
    sim.i2c_write(a, 0x11, &[0x11]);
    assert_eq!((read(&mut sim, a, 0x11, 1)[0] & 1, read(&mut sim, a, 0x27, 1)[0]), (1, 0), "converting");
    sim.i2c_write(a, 0x11, &[0x11]); // asked again while converting: ignored
    assert_eq!(sim.bus.baro_conversions, 1);
    sim.delay_ms(21);
    assert_eq!((read(&mut sim, a, 0x11, 1)[0] & 1, read(&mut sim, a, 0x27, 1)[0]), (0, 3), "done");
}

#[test]
fn s3_new_data_bit_clears_on_the_data_read_and_s4_early_read_is_refused() {
    let mut sim = Sim::new(9, 0, 0);
    crate::ambient::init(&mut sim);
    assert!(crate::ambient::light_start(&mut sim));
    sim.bus.expect = Some("S4");
    read(&mut sim, crate::ambient::LIGHT_ADDR, 0x0d, 3);
    assert_eq!(sim.bus.violations.len(), 1, "inside the first conversion");
    sim.delay_ms(100);
    assert_eq!(read(&mut sim, crate::ambient::LIGHT_ADDR, 0x07, 1)[0] & 8, 8);
    read(&mut sim, crate::ambient::LIGHT_ADDR, 0x0d, 3);
    assert_eq!(read(&mut sim, crate::ambient::LIGHT_ADDR, 0x07, 1)[0] & 8, 0, "cleared by the read");
    sim.delay_ms(100);
    assert_eq!(read(&mut sim, crate::ambient::LIGHT_ADDR, 0x07, 1)[0] & 8, 8, "the next conversion");
}

/// S5, at boot: a reset in the middle of a read left a slave holding SDA. Before the bus clear, the chips were
/// never found and the light sensor and barometer stayed dead until a power cycle.
#[test]
fn s5_a_stuck_bus_at_boot_is_cleared() {
    let mut sim = Sim::new(9, 0, 0);
    sim.bus.stuck_sda = true;
    let app = App::start(&mut sim, None);
    assert!(app.chips.light && app.chips.baro, "{:?}", app.chips);
    assert_eq!(sim.bus.recovers, 1);
}

/// S5, mid-run: after three failed transfers in a row the library clears the bus and the light is read again.
#[test]
fn s5_a_bus_that_sticks_mid_run_is_cleared_and_the_light_comes_back() {
    let (mut sim, mut app) = started();
    app.open(Screen::Menu(Menu::BuiltIn, 0)); // the screen that follows the light, every LIGHT_MS
    app.show(&mut sim);
    run(&mut sim, &mut app, 3000);
    let reads = sim.bus.light_reads;
    assert!(reads > 0 && sim.bus.recovers == 0);
    sim.bus.stuck_sda = true;
    run(&mut sim, &mut app, 4000);
    assert_eq!(sim.bus.recovers, 1, "one clear");
    assert!(sim.bus.nacks >= 3 && !sim.bus.stuck_sda);
    run(&mut sim, &mut app, 3000);
    assert!(sim.bus.light_reads > reads, "the light is read again");
}

/// S5: a bus the clear cannot free (a dead chip, a short) is not clocked and polled every pass: the clears and the
/// transfers back off, and the light is read again once the bus is free.
#[test]
fn s5_a_bus_the_clear_cannot_free_backs_off() {
    let (mut sim, mut app) = started();
    app.open(Screen::Menu(Menu::BuiltIn, 0));
    app.show(&mut sim);
    run(&mut sim, &mut app, 3000);
    sim.bus.stuck_sda = true;
    sim.bus.stuck_hard = true;
    let t0 = sim.bus.transactions;
    run(&mut sim, &mut app, 600_000);
    let (recovers, transfers) = (sim.bus.recovers, sim.bus.transactions - t0);
    assert!((5..=20).contains(&recovers) && transfers <= 100, "{recovers} clears, {transfers} transfers in 10 min");
    sim.bus.stuck_hard = false;
    let reads = sim.bus.light_reads;
    run(&mut sim, &mut app, 130_000);
    assert!(sim.bus.light_reads > reads, "the light comes back once the clear works");
}

/// A barometer that never answers counts toward the clear (not "not ready", "no answer").
#[test]
fn the_barometer_not_answering_counts_toward_the_clear() {
    let mut sim = Sim::new(9, 0, 0);
    sim.bus.lux10 = None; // no light sensor: its good transfers would reset the count of failures in a row
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start)); // a ride: the barometer is read each second
    run(&mut sim, &mut app, 5000);
    assert_eq!(sim.bus.recovers, 0, "a healthy barometer, never a clear");
    sim.bus.baro = None;
    run(&mut sim, &mut app, 10_000);
    assert!(sim.bus.recovers >= 1, "{}", sim.bus.recovers);
}

/// T4: the touch chip stops answering (wedged); five failed reads later the app pulses its reset and the next tap
/// works.
#[test]
fn t4_a_wedged_touch_chip_is_reset_and_answers_again() {
    let mut sim = Sim::new(9, 0, 0)
        .finger(|t| ((6000..6300).contains(&t) || (12_000..12_150).contains(&t)).then_some((120, 160)));
    sim.touch.idle_ms = None;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    let resets = sim.touch.resets;
    sim.touch.wedged = true;
    run(&mut sim, &mut app, 5000);
    assert!(!sim.touch.wedged && sim.touch.resets == resets + 1, "{} resets", sim.touch.resets - resets);
    let g = gestures(&mut sim, &mut app, 8000);
    assert!(g.iter().any(|i| matches!(i, Input::Touch(Gesture::Tap(..)))), "{g:?}");
}

/// T4: a chip no reset frees is reset a handful of times in 10 minutes, not every few reads.
#[test]
fn t4_a_chip_no_reset_frees_is_reset_with_backoff() {
    let mut sim = Sim::new(9, 0, 0).finger(|t| (t >= 6000).then_some((120, 160)));
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    let resets = sim.touch.resets;
    sim.touch.wedged_hard = true;
    run(&mut sim, &mut app, 600_000);
    let n = sim.touch.resets - resets;
    assert!((5..=20).contains(&n), "{n} resets in 10 min");
}

#[test]
fn t1_a_reset_pulse_under_5_ms_leaves_the_touch_chip_dead() {
    for (low, up) in [(2, false), (5, true)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.touch_reset(false);
        sim.delay_ms(low);
        sim.touch_reset(true);
        sim.delay_ms(100);
        let mut s = [0xff];
        assert!(sim.touch_read(0x01, &mut s));
        assert_eq!(crate::touch::ready(s[0]), up, "low {low} ms: status {s:?}");
        assert!(crate::touch::init(&mut sim), "the driver's own reset always works");
    }
}

/// Gestures the app sees from a finger, sampled as `run` does.
fn gestures(sim: &mut Sim, app: &mut App, ms: u64) -> Vec<Input> {
    let end = sim.ms + ms;
    let mut out = Vec::new();
    while sim.ms < end {
        app.step(sim);
        for _ in 0..POLL_MS / TOUCH_MS {
            sim.delay_ms(TOUCH_MS);
            if let Some(i) = app.touch(sim, TOUCH_MS) {
                out.push(i);
                app.input(sim, i);
            }
        }
    }
    out
}

/// T2: a tap on a chip that idled loses its first report, and is still one tap (not a hold, not two).
#[test]
fn t2_the_first_report_after_idle_is_lost_and_a_tap_is_still_one_tap() {
    let mut sim = Sim::new(9, 0, 0).finger(|t| (5000..5200).contains(&t).then_some((120, 20)));
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    let taps: Vec<_> = gestures(&mut sim, &mut app, 1500)
        .into_iter()
        .filter(|i| matches!(i, Input::Touch(Gesture::Tap(..))))
        .collect();
    assert_eq!(sim.touch.lost, 1, "the wake-up report");
    assert_eq!(taps.len(), 1, "{taps:?}");
    // the same tap with the idle off: nothing lost
    let mut sim = Sim::new(9, 0, 0).finger(|t| (5000..5200).contains(&t).then_some((120, 20)));
    sim.touch.idle_ms = None;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    gestures(&mut sim, &mut app, 1500);
    assert_eq!(sim.touch.lost, 0);
}

/// T2 on a ride page: a tap that wakes the chip must not act twice nor lose the ride.
#[test]
fn t2_a_tap_after_idle_on_a_running_ride_page_does_not_stop_the_ride() {
    let mut sim = Sim::new(9, 0, 0).finger(|t| (30_000..30_150).contains(&t).then_some((120, 160)));
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 40_000);
    assert_eq!(app.ride.state(), ride::State::Running, "a stray tap never ends or pauses the ride");
}

/// T3: contacts 3 to 5 are in the chip's slots; the driver reads the first two and the gesture is unchanged.
#[test]
fn t3_five_contacts_do_not_confuse_the_gesture() {
    let mut sim = Sim::new(9, 0, 0).finger(|t| (6000..6200).contains(&t).then_some((120, 20)));
    sim.touch.idle_ms = None;
    for _ in 0..3 {
        sim.touch.extra.push(Box::new(|t| (6000..6200).contains(&t).then_some((10, 300))));
    }
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 4000);
    let g = gestures(&mut sim, &mut app, 1000);
    assert!(g.iter().all(|i| !matches!(i, Input::Touch(Gesture::Pinch(..)))), "{g:?}");
    assert!(g.iter().any(|i| matches!(i, Input::Touch(Gesture::Tap(..)))), "{g:?}");
}

/// K1: a bouncing Start press and release start the ride once, whatever the phase against the key poll.
#[test]
fn k1_bounce_on_start_does_not_start_and_stop_a_ride() {
    for phase in (0..50).step_by(7) {
        let on = 3000 + phase;
        let mut sim =
            Sim::new(9, 0, 0).keys(bounce(move |t| if (on..on + 300).contains(&t) { Key::Start as u8 } else { 0 }));
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 5000);
        assert_eq!(app.ride.state(), ride::State::Running, "phase {phase}: one press, one start");
    }
}

/// K1: Start held for 5 minutes (rain, a jammed button) on a running ride never saves or discards it.
#[test]
fn k1_a_stuck_start_for_five_minutes_does_not_end_the_ride() {
    let mut sim = Sim::new(9, 0, 0).keys(|t| if t >= 20_000 { Key::Start as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 9000);
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 300_000);
    assert_ne!(app.ride.state(), ride::State::Idle, "the ride is still there");
    assert!(!matches!(app.screen(), Screen::Summary | Screen::Discard(_)), "{:?}", app.screen());
}

/// The fuzz of the sensor bus: random NACK bursts, stuck bus episodes and raw structured transactions (the three
/// chips' addresses and register ranges, lengths 1 to 8) against the app on a screen that follows the light and a
/// running ride that reads the barometer. The app never panics; the counts show it reached the recovery, the model's
/// rules and the readings.
#[test]
fn fuzz_the_sensor_bus() {
    let (mut recovers, mut violations, mut reads, mut nacks) = (0, 0, 0, 0);
    for seed in 1..=16u32 {
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let (mut sim, mut app) = started();
        app.open(Screen::Menu(Menu::BuiltIn, 0));
        app.show(&mut sim);
        app.press(&mut sim, Press::Short(Key::Start));
        for _ in 0..40 {
            match rnd() % 4 {
                0 => sim.bus.nack_left = rnd() % 7,
                1 => sim.bus.stuck_sda = true,
                2 => {
                    let addr = [0x53, 0x5c, 0x18, (rnd() % 128) as u8][(rnd() % 4) as usize];
                    // a raw write skips the light sensor's MEAS_RATE / GAIN (regs below 6): a chip that lost its set-up is not the app's rule to break
                    let mut buf = vec![0; 1 + (rnd() % 8) as usize];
                    sim.bus.expect = Some("S"); // the raw transactions' violations are recorded; the app's still fail
                    if rnd() % 2 == 0 {
                        sim.i2c_read(addr, (rnd() % 0x30) as u8, &mut buf);
                    } else {
                        sim.i2c_write(
                            addr,
                            if addr == 0x53 { 6 + (rnd() % 0x10) as u8 } else { (rnd() % 0x30) as u8 },
                            &buf,
                        );
                    }
                    sim.bus.expect = None;
                }
                _ => {}
            }
            run(&mut sim, &mut app, 500);
        }
        run(&mut sim, &mut app, 3000);
        recovers += sim.bus.recovers;
        violations += sim.bus.violations.len();
        reads += sim.bus.light_reads;
        nacks += sim.bus.nacks;
    }
    assert!(recovers > 4 && violations > 0 && reads > 100 && nacks > 20, "{recovers} {violations} {reads} {nacks}");
}
