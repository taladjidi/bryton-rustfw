//! The Nordic chip's real limits (sim `ChipLimits`: a 96-slot event queue, one frame at a time while the host reads,
//! a hang for ever on overflow) against a ride's traffic: HR, power and Di2 at 4 Hz and the phone's notifications.
//! On 2026-11-01 the chip hung about 30 s after every reset: the drain was too slow (a 20 us receive wait ended each
//! read after one frame).

use super::*;

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const DI2_KEY: [u8; 8] = [8, 7, 6, 5, 4, 3, 2, 1];

/// One ride's outcome: chip hangs, the queue's high-water mark, resets after the start, broadcasts heard per kind.
struct Outcome {
    hangs: u32,
    high_water: usize,
    resets: usize,
    heard: [u32; 3],
}

/// The ride's radio mix on a unit over `image`, paired and started up, the Nordic chip's limits on: HR, power and Di2 (and
/// with `bsc` a speed and cadence sensor), the phone linked, 12 notifications every 20 s for `minutes`, the receive
/// waiting `wait_us`; `prep` runs on the simulator before the app starts. The ANT periods are the app's channel setup.
pub(super) fn radio_unit(
    minutes: u64,
    wait_us: u32,
    image: Vec<u8>,
    bsc: bool,
    prep: impl FnOnce(&mut Sim),
) -> (Sim, App) {
    fn hr(_: u64) -> [u8; 8] {
        [0, 0, 0, 0, 0, 0, 0, 72]
    }
    fn power(_: u64) -> [u8; 8] {
        [0x10, 1, 0xff, 85, 0, 0, 210, 0]
    }
    fn di2(_: u64) -> [u8; 8] {
        [0, 0, 2, 11, 60, 0, 0, 0]
    }
    fn bsc_page(t: u64) -> [u8; 8] {
        // cumulative cadence and speed event times (1/1024 s) and revolutions, moving on with the clock
        let (e, r) = ((t * 1024 / 1000) as u16, (t / 700) as u16);
        [e as u8, (e >> 8) as u8, r as u8, (r >> 8) as u8, e as u8, (e >> 8) as u8, r as u8, (r >> 8) as u8]
    }
    let mut script = Vec::new();
    for burst in 0..minutes * 3 {
        for i in 0..12u32 {
            let uid = (burst * 12) as u32 + i;
            script.push((10_000 + burst * 20_000, Act::Notify(4, uid, "Bob", "Lunch at 1?")));
            script.push((12_000 + burst * 20_000, Act::Remove(uid)));
        }
    }
    script.sort_by_key(|s| s.0);
    let mut phone = PhoneSim::new(true);
    (phone.connect_at, phone.script) = (1000, script);
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.expect = Some("stream time-out"); // a hung chip's reset blocks the pass for seconds: the receiver's stream stops after 1.5 s
    sim.own_settings();
    sim.emmc.image = image;
    sim.backup[Kind::Hr.backup()] = Kind::Hr.to_word(SensorId { device: 9, trans: 1 });
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(SensorId { device: 7, trans: 5 });
    sim.backup[Kind::Di2.backup()] = Kind::Di2.to_word(SensorId { device: 4321, trans: 0xf1 });
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 9, data: hr, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 1, id: 4321, data: di2, network: 1, ack: &[4] });
    if bsc {
        sim.backup[Kind::SpeedCadence.backup()] = Kind::SpeedCadence.to_word(SensorId { device: 555, trans: 0xf1 });
        sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: bsc_page, network: 0, ack: &[] });
    }
    sim.nordic.phone = Some(phone);
    prep(&mut sim);
    let mut app = App::start_with(&mut sim, Some(KEY), Some(DI2_KEY));
    sim.nordic.expect_hang = Some("event queue overflow"); // the outcome counts hangs
    sim.nordic.limits = ChipLimits { wait_us, ..Default::default() };
    app.activate(&mut sim, Menu::Phone, 1); // PAIR
    (sim, app)
}

/// A ride of `minutes` with the three sensors paired, the phone linked and 12 notifications every 20 s, the main loop
/// passing every `pass_ms` and the receive waiting `wait_us`.
fn ride(minutes: u64, pass_ms: u32, wait_us: u32) -> Outcome {
    ride_with(minutes, pass_ms, wait_us, |_| {}, |_| {})
}

/// [`ride`], with `burst` run on the simulator every 30 s: the events it queues reach the chip in one SoftDevice pass.
fn ride_with(
    minutes: u64,
    pass_ms: u32,
    wait_us: u32,
    burst: impl Fn(&mut Sim),
    prep: impl FnOnce(&mut Sim),
) -> Outcome {
    let (mut sim, mut app) = radio_unit(minutes, wait_us, crate::fat::tests::image(), false, prep);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    let (start, resets0, heard0) = (sim.ms, sim.nordic.resets.len(), app.radio_counts().1);
    let mut next_burst = start + 30_000;
    while sim.ms < start + minutes * 60_000 {
        if sim.ms >= next_burst {
            next_burst += 30_000;
            burst(&mut sim);
        }
        app.step(&mut sim);
        sim.sleep_ms(pass_ms);
    }
    let heard = app.radio_counts().1;
    let d = |k: Kind| heard[k as usize] - heard0[k as usize];
    std::eprintln!(
        "ride {minutes} min, pass {pass_ms} ms, wait {wait_us} us: high-water {}, hangs {}",
        sim.nordic.high_water,
        sim.nordic.hangs
    );
    sim.gnss.assert_reads(6678, 31);
    Outcome {
        hangs: sim.nordic.hangs,
        high_water: sim.nordic.high_water,
        resets: sim.nordic.resets.len() - resets0,
        heard: [d(Kind::Hr), d(Kind::Power), d(Kind::Di2)],
    }
}

fn assert_healthy(o: &Outcome, minutes: u64) {
    assert_eq!(o.hangs, 0, "the chip's queue overflowed and it hung");
    assert!(o.high_water < 48, "queue high-water {}", o.high_water);
    assert_eq!(o.resets, 0, "the radio watchdog reset the chip");
    // a call's wait keeps the ANT events that arrive during it (Rx::keep_last), so a broadcast is lost only when the
    // ring of 16 fills: a small constant per channel, measured 0 per channel in 10 minutes; the bar is 8
    let want = (minutes * 240).saturating_sub(8) as u32;
    let lost = o.heard.map(|n| (minutes * 240).saturating_sub(u64::from(n)));
    std::println!("{minutes} min: broadcasts lost per channel {lost:?}");
    assert!(o.heard.iter().all(|&n| n >= want), "broadcasts decoded {:?}, want {want} each", o.heard);
}

/// Three sensors at 4 Hz and a phone's notifications, ten minutes: the chip never hangs, its queue stays under 48.
#[test]
fn a_ride_with_three_sensors_never_overflows_the_chip() {
    assert_healthy(&ride(10, POLL_MS, board_wait_us()), 10);
}

/// Four hours (run with `--ignored`).
#[test]
#[ignore = "soak: four simulated hours"]
fn a_four_hour_ride_never_overflows_the_chip() {
    assert_healthy(&ride(240, POLL_MS, board_wait_us()), 240);
}

/// The 20 us receive wait of 2026-10-31 reads one frame a pass: the queue runs higher (13 against 12 at the 50 ms
/// pass with the transfers' time charged; `ChipLimits::parse_us` 20 between two reads: inferred, not measured). Before
/// the differential test (2026-11-01, `resp_ahead`: events queued before a response go out before it, as the emulated
/// app does) the model said 15 against 12, and 100 ms with the 20 us wait overflowed it.
#[test]
fn the_20_us_wait_leaves_the_queue_higher() {
    let (slow, fast) = (ride(10, POLL_MS, 20), ride(10, POLL_MS, board_wait_us()));
    assert!(slow.high_water > fast.high_water, "{} against {}", slow.high_water, fast.high_water);
}

/// A 100 ms pass with the one-frame wait (the ride pass of 2026-10-31) runs the queue far higher than the production
/// wait does (70 against 12 on 2026-11-01). It no longer overflows the 96 slots in the model: that needed a response to
/// overtake the queued events, which the emulated app does not do (nrf_diff_tests.rs). A direction, not the unit's
/// numbers.
#[test]
fn a_100_ms_pass_with_the_20_us_wait_runs_the_queue_high() {
    let (slow, fast) = (ride(10, 100, 20), ride(10, 100, board_wait_us()));
    assert!(slow.high_water >= fast.high_water + 30, "{} against {}", slow.high_water, fast.high_water);
}

/// The healthy-wait case for the slower pass: the 100 ms pass is gone from the firmware (2026-11-01); this records
/// the margin the model gives it with the production wait.
#[test]
fn a_100_ms_pass_with_the_200_us_wait() {
    let o = ride(10, 100, board_wait_us());
    assert_eq!(o.hangs, 0, "high-water {}", o.high_water);
    std::eprintln!("100 ms pass, production wait: high-water {}", o.high_water);
}

/// Every open ANT channel times out its search together after a chip reset: SEARCH_TIMEOUT and CHANNEL_CLOSED for each of
/// the 8 channels (ant_parameters.h codes 1 and 7).
fn search_timeouts(sim: &mut Sim) {
    for ch in 0..8u8 {
        sim.nordic.ant_event(&[3, 0x40, ch, 1, 1], 1, ch);
        sim.nordic.ant_event(&[3, 0x40, ch, 1, 7], 7, ch);
    }
}

/// The iPhone's ANCS / AMS traffic for 12 notifications at once: per notification the attribute fetch's four GATTC
/// notifications (title, message, app id, date; sizes of the phone model, ble_sim.rs) in one pass, 48 events.
fn phone_burst(sim: &mut Sim) {
    for _ in 0..12 {
        for n in [5u16, 11, 20, 15] {
            let mut p = std::vec![2, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x25, 0, 1];
            p.extend(n.to_le_bytes());
            p.extend((0..n).map(|i| i as u8));
            sim.nordic.queue(&p);
        }
    }
}

/// The SoftDevice's ANT queue (32) held while the chip was suspended, then pulled in one pass: 32 broadcasts.
fn ant_resume(sim: &mut Sim) {
    for i in 0..32u8 {
        sim.nordic.ant_event(&[&[9, 0x4e, i % 3][..], &[i; 8]].concat(), 0x80, i % 3);
    }
}

/// An iPhone that connects with `n` notifications already on it: ANCS sends a Notification Source notification for every
/// one when the unit subscribes (Apple ANCS spec), at the phone's connection interval. Inferred (not measured): 5 per
/// 20 ms interval (the interval is 7.5 to 30 ms, 4 to 6 notifications fit one). The app fetches the attributes of each
/// as it does for any notification (the phone model answers). Production configuration.
#[test]
fn a_phone_connecting_with_many_notifications_never_hangs_the_chip() {
    for n in [32u32, 64, 128] {
        let o = ride_with(
            3,
            POLL_MS,
            board_wait_us(),
            |_| {},
            |sim| {
                let script = (0..n).map(|i| (5_000 + u64::from(i / 5) * 20, Act::Notify(4, i, "Bob", "Lunch at 1?")));
                if let Some(p) = &mut sim.nordic.phone {
                    p.script = script.collect();
                }
            },
        );
        std::eprintln!("phone connecting with {n} notifications: high-water {}, hangs {}", o.high_water, o.hangs);
        assert_eq!(o.hangs, 0, "{n} existing notifications hung the chip");
    }
}

/// The 2026-11-01 ride, honestly: the old configuration (100 ms pass, 20 us wait) with the bursts a ride really has, and
/// the production configuration under the same. Each burst arrives every 30 s on top of the ride's traffic. Which of
/// them hang the old configuration is printed; the production one must never hang and keep 36 of the 96 slots free.
#[test]
fn the_bursts_of_a_ride_against_the_old_and_the_production_configuration() {
    let bursts = [
        ("search time-outs", search_timeouts as fn(&mut Sim)),
        ("phone burst", phone_burst),
        ("ANT resume", ant_resume),
    ];
    let (mut old_hangs, mut worst) = (std::vec::Vec::new(), 0);
    for (name, burst) in bursts {
        let old = ride_with(5, 100, 20, burst, |_| {});
        let prod = ride_with(5, POLL_MS, board_wait_us(), burst, |_| {});
        std::eprintln!(
            "{name}: old config hangs {} (high-water {}), production hangs {} (high-water {})",
            old.hangs,
            old.high_water,
            prod.hangs,
            prod.high_water
        );
        if old.hangs > 0 {
            old_hangs.push(name);
        }
        assert_eq!(prod.hangs, 0, "{name}: the production configuration hung");
        assert!(prod.high_water <= 60, "{name}: production high-water {}", prod.high_water); // the burst itself is up to 48
        worst = worst.max(prod.high_water);
    }
    std::eprintln!("bursts that hang the old configuration: {old_hangs:?}; production worst high-water {worst} of 96");
    assert!(!old_hangs.is_empty(), "no realistic burst hangs the old configuration in the model: the trigger is open");
}
