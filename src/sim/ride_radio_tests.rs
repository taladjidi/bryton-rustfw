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
    let (mut sim, mut app) = radio_unit(minutes, wait_us, crate::fat::tests::image(), false, |_| {});
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    let (start, resets0, heard0) = (sim.ms, sim.nordic.resets.len(), app.radio_counts().1);
    while sim.ms < start + minutes * 60_000 {
        app.step(&mut sim);
        sim.delay_ms(pass_ms);
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

/// The 20 us receive wait of 2026-10-31 reads one frame a pass: the queue runs higher (16 against 12 at the 50 ms
/// pass, `ChipLimits::parse_us` 20 between two reads: inferred, not measured); with the 100 ms pass (below) the
/// ride's traffic overflows it.
#[test]
fn the_20_us_wait_leaves_the_queue_higher() {
    let (slow, fast) = (ride(10, POLL_MS, 20), ride(10, POLL_MS, board_wait_us()));
    assert!(slow.high_water >= fast.high_water + 4, "{} against {}", slow.high_water, fast.high_water);
}

/// A 100 ms pass with the one-frame wait (the ride pass of 2026-10-31) is no better. Worst case for a short wait
/// (ChipLimits): a direction, not the unit's exact numbers.
#[test]
fn a_100_ms_pass_with_the_20_us_wait_hangs_the_chip() {
    let o = ride(10, 100, 20);
    assert!(o.hangs > 0, "high-water {}", o.high_water);
}

/// The healthy-wait case for the slower pass: the 100 ms pass is gone from the firmware (2026-11-01); this records
/// the margin the model gives it with the production wait.
#[test]
fn a_100_ms_pass_with_the_200_us_wait() {
    let o = ride(10, 100, board_wait_us());
    assert_eq!(o.hangs, 0, "high-water {}", o.high_water);
    std::eprintln!("100 ms pass, production wait: high-water {}", o.high_water);
}
