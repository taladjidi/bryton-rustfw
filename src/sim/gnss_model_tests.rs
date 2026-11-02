//! The strict GNSS receiver model (`gnss_model.rs`) against its rules, then the firmware against the model: every frame
//! `gnss.rs` builds passes the table, a refused configuration is tried once more and then given up, and a pass held
//! longer than the receiver's stream time-out loses epochs and the app recovers.

extern crate std;
use super::tests::run;
use super::*;
use crate::gnss;
use std::vec::Vec;

fn utc() -> i64 {
    crate::epoch_s((2026, 10, 23, 7, 0, 0))
}

/// A receiver powered at 2 s.
fn on() -> Gnss {
    let mut g = Gnss::default();
    g.power(true, 2000);
    g
}

/// A UBX frame.
fn ubx(class: u8, id: u8, p: &[u8]) -> Vec<u8> {
    let mut f = std::vec![0xb5, 0x62, class, id, p.len() as u8, (p.len() >> 8) as u8];
    f.extend(p);
    let (mut a, mut b) = (0u8, 0u8);
    for &x in &f[2..] {
        a = a.wrapping_add(x);
        b = b.wrapping_add(a);
    }
    f.extend([a, b]);
    f
}

/// A CFG-VALSET to `layers` with `(key, value bytes)` items.
fn valset(layers: u8, items: &[(u32, &[u8])]) -> Vec<u8> {
    let mut p = std::vec![0, layers, 0, 0];
    for (k, v) in items {
        p.extend(k.to_le_bytes());
        p.extend(*v);
    }
    ubx(0x06, 0x8a, &p)
}

/// One transfer of `out` (padded to 4 bytes with 0xff) at `now`: what came back.
fn xfer(g: &mut Gnss, now: u64, out: &[u8]) -> Vec<u8> {
    let mut buf = out.to_vec();
    while !buf.len().is_multiple_of(4) {
        buf.push(0xff);
    }
    g.transfer(now, utc(), &mut buf);
    buf
}

/// Every frame the receiver has queued, read in one go at `now`: (class, id, payload).
fn heard(g: &mut Gnss, now: u64) -> Vec<(u8, u8, Vec<u8>)> {
    let mut parser = gnss::Parser::new();
    let mut v = Vec::new();
    for b in xfer(g, now, &[0xff; 4096]) {
        if let Some((c, i, p)) = parser.push(b) {
            v.push((c, i, p.to_vec()));
        }
    }
    v
}

/// Sends `out` and returns the (class, id) of the ACK or NAK frames it drew, as `(ok, class, id)`.
fn verdict(g: &mut Gnss, now: u64, out: &[u8]) -> Vec<(bool, u8, u8)> {
    xfer(g, now, out);
    heard(g, now)
        .into_iter()
        .filter_map(|(c, i, p)| match gnss::ack(c, i, &p) {
            Some(gnss::Ack::Ack(c, i)) => Some((true, c, i)),
            Some(gnss::Ack::Nak(c, i)) => Some((false, c, i)),
            _ => None,
        })
        .collect()
}

const GPS: u32 = 0x1031_001f;
const GLO: u32 = 0x1031_0025;

#[test]
fn a_valset_is_acknowledged_and_applied() {
    let mut g = on();
    let f = valset(1, &[(0x20d0_0001, &[2]), (GLO, &[0])]);
    assert_eq!(verdict(&mut g, 2100, &f), [(true, 6, 0x8a)]);
    assert_eq!((g.pm_mode, g.acks, g.naks), (2, 1, 0));
}

#[test]
fn the_receiver_refuses_what_the_interface_description_says() {
    // (why, frame, the reason the model records)
    let many: Vec<(u32, &[u8])> = (0..65).map(|_| (GPS, &[1u8][..])).collect();
    let cases: [(&str, Vec<u8>); 6] = [
        ("unknown key", valset(1, &[(0x2000_0abc, &[1])])),
        ("no layer", valset(0, &[(GPS, &[1])])),
        ("64 pairs", valset(1, &many)),
        ("constellations", valset(1, &[(GLO, &[1])])),
        ("cut short", ubx(0x06, 0x8a, &[0, 1, 0, 0, 0x1f, 0x00, 0x31])),
        ("version", ubx(0x06, 0x8a, &[1, 1, 0, 0])),
    ];
    for (name, f) in cases {
        let mut g = on();
        g.expect = Some("");
        assert_eq!(verdict(&mut g, 2100, &f), [(false, 6, 0x8a)], "{name}");
        assert_eq!((g.pm_mode, g.signals_on, g.violations.len()), (0, [true, true, true, false], 1), "{name}");
    }
}

#[test]
fn the_flash_layer_is_not_ours_to_write() {
    let mut g = on();
    g.expect = Some("BBR or flash");
    verdict(&mut g, 2100, &valset(0b101, &[(GPS, &[1])]));
    assert_eq!(g.violations.len(), 1);
}

#[test]
#[should_panic(expected = "a key the receiver does not know")]
fn a_violation_fails_loudly_unless_expected() {
    let mut g = on();
    verdict(&mut g, 2100, &valset(1, &[(0x2000_0abc, &[1])]));
}

#[test]
fn the_constellation_limit_is_checked_for_the_ram_layer_only() {
    let mut g = on();
    g.expect = Some("");
    // the validity is checked for the RAM layer only (3.10.5.1): a BBR-only message is taken
    assert_eq!(verdict(&mut g, 2100, &valset(0b010, &[(GLO, &[1])])), [(true, 6, 0x8a)]);
    assert_eq!(g.violations, ["VALSET to the BBR or flash layer: this firmware writes the RAM layer only"]);
}

#[test]
fn a_bad_checksum_is_dropped_without_an_answer() {
    let mut g = on();
    g.expect = Some("bad checksum");
    let mut f = valset(1, &[(GPS, &[1])]);
    *f.last_mut().unwrap() ^= 1;
    assert_eq!(verdict(&mut g, 2100, &f), []);
    assert_eq!((g.acks, g.naks, g.violations.len()), (0, 0, 1));
}

#[test]
fn mga_messages_are_acknowledged_when_asked_and_ano_needs_the_time() {
    let mut g = on();
    g.expect = Some("MGA-ANO before");
    let on_ack = valset(1, &[(0x1011_0025, &[1])]);
    assert_eq!(verdict(&mut g, 2100, &on_ack), [(true, 6, 0x8a)]);
    let mut ano = std::vec![0u8; 76];
    ano[2..7].copy_from_slice(&[5, 0, 26, 10, 23]);
    xfer(&mut g, 2200, &ubx(0x13, 0x20, &ano));
    let acks: Vec<_> = heard(&mut g, 2200).into_iter().filter(|f| f.0 == 0x13).collect();
    assert_eq!(acks.len(), 1);
    assert_eq!(gnss::ack(0x13, 0x60, &acks[0].2), Some(gnss::Ack::Mga(false, 1)), "time unknown: info code 1");
    assert_eq!(g.mga_refused, 1);
    // the time first (accepted), then the same ANO is accepted
    xfer(&mut g, 2300, &gnss::mga_time(crate::from_epoch_s(utc()), 2));
    let acks: Vec<_> = heard(&mut g, 2300).into_iter().filter(|f| f.0 == 0x13).collect();
    assert_eq!(gnss::ack(0x13, 0x60, &acks[0].2), Some(gnss::Ack::Mga(true, 0)), "the time");
    xfer(&mut g, 2400, &ubx(0x13, 0x20, &ano));
    let acks: Vec<_> = heard(&mut g, 2400).into_iter().filter(|f| f.0 == 0x13).collect();
    assert_eq!(gnss::ack(0x13, 0x60, &acks[0].2), Some(gnss::Ack::Mga(true, 0)), "the ANO");
    assert_eq!(g.ano.len(), 1);
}

#[test]
fn time_aiding_off_by_more_than_its_accuracy_degrades_the_receiver() {
    let mut g = on();
    let late = crate::from_epoch_s(utc() + 3600);
    xfer(&mut g, 2100, &gnss::mga_time(late, 2));
    assert!(g.degraded);
    let mut g = on();
    xfer(&mut g, 2100, &gnss::mga_time(late, 50_400));
    assert!(!g.degraded, "a wide accuracy covers it");
}

#[test]
fn a_run_of_ff_switches_reception_off_for_the_transfer() {
    let mut g = on();
    g.expect = Some("MAXFF");
    let mut buf = std::vec![0xff; 60];
    buf.extend(valset(1, &[(GPS, &[1])]));
    xfer(&mut g, 2100, &buf);
    assert_eq!((g.acks, g.violations.len()), (0, 1), "the frame after 60 0xff was not received");
    // 40 0xff are fine, and the count starts again with each transfer
    let mut buf = std::vec![0xff; 40];
    buf.extend(valset(1, &[(GPS, &[1])]));
    xfer(&mut g, 2200, &buf);
    assert_eq!(g.acks, 1);
    // MAXFF 0 switches the mechanism off
    xfer(&mut g, 2300, &valset(1, &[(0x2064_0001, &[0])]));
    let mut buf = std::vec![0xff; 100];
    buf.extend(valset(1, &[(GPS, &[1])]));
    xfer(&mut g, 2400, &buf);
    assert_eq!((g.acks, g.violations.len()), (3, 1));
}

#[test]
fn the_stream_stops_after_the_timeout_and_restarts_at_the_next_read() {
    let mut g = on();
    g.expect = Some("stream time-out");
    xfer(&mut g, 2600, &[]);
    assert_eq!(heard(&mut g, 2600).len(), 1, "the first report");
    // read again at 5 s: the report due at 3.5 s was made, the one at 4.5 s was after the time-out (4.1 s)
    xfer(&mut g, 5000, &[]);
    assert_eq!((g.lost_epochs, g.violations.len()), (1, 1));
    assert_eq!(heard(&mut g, 5000).len(), 1, "the one made before the stream stopped");
    heard(&mut g, 6100);
    assert_eq!(heard(&mut g, 6100).len(), 0);
    xfer(&mut g, 6100, &[]);
    assert_eq!(g.lost_epochs, 1, "restarted: the next report is a second after the access");
    assert_eq!(heard(&mut g, 6100 + 1000).len(), 1);
    // a longer time-out as a parameter, and the key that disables it
    for (timeout, ext) in [(Some(3000), false), (None, true)] {
        let mut g = on();
        g.timeout_ms = timeout;
        g.extended_timeout = ext;
        xfer(&mut g, 2600, &[]);
        xfer(&mut g, 5000, &[]);
        assert_eq!(g.lost_epochs, 0, "{timeout:?} {ext}");
    }
    g.assert_reads(2640, 2);
}

#[test]
fn the_output_buffer_holds_4_kb_and_drops_the_rest() {
    let mut g = on();
    g.expect = Some("output buffer full");
    g.extended_timeout = true; // so the stream goes on unread
    for s in 1..=60u64 {
        xfer(&mut g, 2000 + 1000 * s, &[]);
    }
    assert!(g.dropped >= 15, "{} dropped", g.dropped);
    let frames = heard(&mut g, 70_000).len();
    assert!((38..=41).contains(&frames), "{frames} kept: 4096 / 100 bytes");
}

#[test]
#[should_panic(expected = "GNSS powered 700 ms after off")]
fn power_needs_the_line_low_for_a_second() {
    let mut g = on();
    g.power(false, 10_000);
    g.power(true, 10_700);
}

#[test]
fn power_after_a_second_low_starts_it_and_the_supply_loss_resets_the_ram_layer() {
    let mut g = on();
    verdict(&mut g, 2100, &valset(1, &[(0x20d0_0001, &[2])]));
    g.power(false, 10_000);
    g.power(true, 11_000);
    assert_eq!(g.pm_mode, 0);
}

/// Everything `gnss.rs` can build goes through the table: the configuration in both power modes, the four systems
/// choices (and one past the end), the time and the position.
#[test]
fn every_frame_the_firmware_builds_passes_the_table() {
    let mut g = on();
    let now = crate::from_epoch_s(utc());
    let frames = [
        gnss::config(false),
        gnss::config(true),
        gnss::systems(0),
        gnss::systems(1),
        gnss::systems(2),
        gnss::systems(3),
        gnss::systems(9),
        gnss::mga_time(now, 2),
        gnss::mga_time(now, 14 * 3600),
        gnss::mga_pos(51_234_567, 4_234_567, 10_000),
        gnss::mga_pos(-51_234_567, -4_234_567, 0),
    ];
    for (i, f) in frames.iter().enumerate() {
        xfer(&mut g, 2100 + 100 * i as u64, f);
    }
    assert!(g.violations.is_empty() && g.naks == 0, "{:?}", g.violations);
    assert_eq!(g.acks, 7);
    assert_eq!(g.signals.len(), 4 * 5);
    // and the model sees the systems as they are set
    assert!(!g.degraded);
}

/// The first test of the retry: a configuration the receiver refuses is sent once more, then given up; the ride and
/// the fixes go on. Before the change the answer was never read, and the refusal went unnoticed.
#[test]
fn a_refused_configuration_is_retried_once_then_given_up() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    sim.gnss.nak_all = true;
    sim.gnss.fix = Some(7000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 8000);
    let config = |sim: &Sim| sim.gnss.got.windows(32).filter(|w| *w == gnss::config(false)).count();
    assert_eq!(config(&sim), 2, "sent, refused, sent once more");
    assert_eq!(app.gps.answers[1], 1, "one refusal retried");
    assert!(app.gps.answers[2] >= 1, "the second refusal given up: {:?}", app.gps.answers);
    run(&mut sim, &mut app, 20_000);
    assert_eq!(config(&sim), 2, "no third try");
    assert!(app.gps.pvt.is_some_and(|p| p.good()), "the fixes go on");
}

#[test]
fn a_refusal_that_the_retry_cures_is_counted_once() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1500);
    sim.gnss.nak_next = true;
    run(&mut sim, &mut app, 2500);
    let config = sim.gnss.got.windows(32).filter(|w| *w == gnss::config(false)).count();
    assert_eq!(config, 2);
    assert_eq!((app.gps.answers[1], app.gps.answers[2]), (1, 0));
    assert_eq!(sim.gnss.pm_mode, 0);
}

/// Nothing in the start-up is refused by the strict model, and its time aiding is inside the accuracy it states,
/// with and without a learnt offset.
#[test]
fn the_start_up_is_taken_whole_and_the_aiding_is_right() {
    for offset in [None, Some(8)] {
        let mut sim = Sim::new(9, 0, 0);
        sim.classic_pages();
        sim.backup[17..19]
            .copy_from_slice(&gnss::Aid { offset_15min: offset, pos: Some((5_123_000, 4_234_000)) }.to_words());
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 6000);
        assert!(sim.gnss.violations.is_empty() && !sim.gnss.degraded, "{offset:?}");
        assert_eq!((sim.gnss.naks, sim.gnss.acks), (0, 2), "{offset:?}: the configuration and the systems");
        assert_eq!(app.gps.answers, [2, 0, 0, 0]);
    }
}

/// A pass held for 2.5 s: the receiver's stream stops after 1.5 s, the epochs after it are lost, the app notices
/// nothing worse than a missing second and has the fixes again at once.
#[test]
fn a_long_pass_loses_the_epochs_after_the_timeout_and_the_app_recovers() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    sim.gnss.fix = Some(7000);
    sim.gnss.expect = Some("stream time-out");
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 5000);
    assert!(app.gps.pvt.is_some_and(|p| p.good()));
    let at = sim.gnss.violations.len();
    sim.delay_ms(2500); // the pass (its time past MAX_PASS_MS is repaid at twice the speed: the age bound below is 2 s)
    run(&mut sim, &mut app, 2000);
    assert!(sim.gnss.lost_epochs >= 1 && sim.gnss.violations.len() > at, "{}", sim.gnss.lost_epochs);
    assert!(app.gps.pvt.is_some_and(|p| p.good()) && app.gps.pvt_ms < 2000, "fixes again: {}", app.gps.pvt_ms);
    sim.gnss.assert_reads(3631, 3);
}

/// Idle, with a fix: the firmware reads at least every 0.8 s (GNSS_PVT_WAIT_MS), inside the stream time-out (G2),
/// before and after the start-up traffic ends. Renode's GNSS run once showed 2.5 s reads from 17 s: that was its SysTick
/// at 64 MHz (3.1 times too slow), fixed in renode/s510.resc; docs/devloop.md, "Renode with the GNSS model".
#[test]
fn an_idle_minute_is_read_at_least_every_800_ms() {
    let mut sim = Sim::new(9, 0, 0);
    sim.classic_pages();
    sim.gnss.fix = Some(7000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 60_000);
    assert!(sim.gnss.violations.is_empty(), "{:?}", sim.gnss.violations);
    sim.gnss.assert_reads(880, 0);
}
