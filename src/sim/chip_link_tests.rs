//! The simulated Nordic link and queues (chip_link.rs): each fatal condition hangs the model, the RTS and CTS holds
//! happen when the reversal says, and `nrf::send` never cuts a frame short.

use super::*;
use crate::nrf;

/// sd_ble_gattc_write of 20 bytes (the table's longest accepted write): a payload of 34 bytes.
const WRITE20: [u8; 34] = {
    let mut p = [0u8; 34];
    p[1] = 0x9c;
    p[4] = 1;
    p[5] = 1;
    p[7] = 0x0d;
    p[11] = 20;
    p[13] = 1;
    p
};

/// A reset simulator whose chip answers every valid command (`table_only`).
fn chip() -> Sim {
    let mut s = Sim::new(8, 0, 0);
    s.nordic.table_only = true;
    s.nrf_reset();
    s
}

fn ant(s: &mut Sim, n: usize) {
    for i in 0..n {
        s.nordic.ant_event(&[1, 0x40, 0, 1, 1], 1, i as u8);
    }
}

fn idle(s: &Sim) -> bool {
    let n = &s.nordic;
    n.fifo.is_empty() && n.out.is_empty() && n.events.is_empty() && n.resp.is_empty() && n.sd_ble.is_empty()
}

/// Reads everything the chip has, however long it takes, as frames.
fn drain(s: &mut Sim) -> Vec<Vec<u8>> {
    let (mut rx, mut got, mut buf) = (nrf::Rx::new(), Vec::new(), [0u8; 64]);
    for _ in 0..10_000 {
        let n = s.nrf_recv(&mut buf);
        got.extend(buf[..n].iter().filter_map(|&b| rx.push(b).map(<[u8]>::to_vec)));
        if n == 0 {
            if idle(s) {
                break;
            }
            s.delay_ms(1);
        }
    }
    got
}

/// The RX FIFO holds 16 bytes and RTS stalls the chip; reading resumes the frame exactly where it stopped.
#[test]
fn a_full_fifo_stalls_the_chip_and_the_frame_resumes_whole() {
    let mut s = chip();
    ant(&mut s, 2);
    s.delay_ms(5); // the host does not read for 5 ms: far more than a frame's wire time
    let mut one = [0u8; 1];
    assert_eq!(s.nrf_recv(&mut one), 1);
    assert_eq!(s.nordic.fifo.len(), 15, "the FIFO holds 16 bytes at most (one was read)");
    let mut rest: Vec<u8> = Vec::new();
    for _ in 0..4000 {
        let mut b = [0u8; 8];
        let n = s.nrf_recv(&mut b);
        rest.extend(&b[..n]);
        if n == 0 {
            if idle(&s) {
                break;
            }
            s.delay_ms(1);
        }
    }
    assert_eq!(rest.len(), 2 * 46 - 1, "two whole frames, no byte lost or added");
    assert_eq!(&rest[45..47], &[44, 0], "the second frame's header follows the first frame's last byte");
}

/// Frame end in microseconds: the next frame's first byte comes 15 us of gap and 10 us of byte after the last byte.
#[test]
fn the_gap_between_frames_is_15_us_and_the_burst_ends_with_the_wait() {
    for (wait_us, frames) in [(20, 1), (26, 2)] {
        let mut s = chip();
        s.nordic.limits.wait_us = wait_us;
        ant(&mut s, 2);
        s.delay_ms(1);
        let mut buf = [0u8; 256];
        let n = s.nrf_recv(&mut buf);
        assert_eq!(n, 46 * frames, "wait {wait_us} us (the next frame's first byte is 25 us after the last)");
    }
}

/// One pass with more events than free slots hangs; a pass that just fits does not, and stops the pull.
#[test]
fn an_overfull_pass_hangs_and_an_exact_one_suspends_the_pull() {
    let ble = |s: &mut Sim, n: u8| (0..n).for_each(|i| s.nordic.queue(&[2, 0x1b, 0, 0xff, 0xff, i]));
    let mut s = chip();
    s.nordic.expect_hang = Some("event queue overflow");
    ant(&mut s, 32);
    ble(&mut s, 65);
    s.nordic.sync(1);
    s.nordic.run();
    assert!(s.nordic.hung, "97 events in one pass");
    assert!(s.nordic.hang_reason.as_deref().unwrap_or("").contains("97"));

    let mut s = chip();
    ant(&mut s, 32);
    ble(&mut s, 64);
    s.nordic.sync(1);
    s.nordic.run();
    assert!(!s.nordic.hung, "96 events fit exactly");
    assert!(s.nordic.suspended, "0 free: the pull stops");
    ble(&mut s, 1);
    s.nordic.run();
    assert_eq!(s.nordic.sd_ble.len(), 1, "held in the SoftDevice while suspended");
    let frames = drain(&mut s);
    assert!(!s.nordic.hung);
    assert_eq!(frames.len(), 97, "the pull resumed and nothing was lost");
}

/// The SoftDevice's ANT queue of 32 loses what does not fit while the chip is suspended.
#[test]
fn the_ant_queue_of_32_overflows_while_suspended() {
    let mut s = chip();
    s.nordic.suspended = true;
    ant(&mut s, 40);
    assert_eq!((s.nordic.sd_ant.len(), s.nordic.ant_dropped), (32, 8));
}

/// A 9th security context without AUTH_STATUS or DISCONNECTED hangs the chip; those free one.
#[test]
fn a_ninth_security_context_hangs() {
    let mut s = chip();
    s.nordic.expect_hang = Some("security contexts");
    let mut rx = nrf::Rx::new();
    for _ in 0..8 {
        crate::ble::sec_params_reply(&mut s, &mut rx, 0, true).expect("accepted");
    }
    assert!(!s.nordic.hung);
    s.nordic.queue(&[2, 0x11, 0, 0, 0, 0x13]); // DISCONNECTED frees one
    crate::ble::sec_params_reply(&mut s, &mut rx, 0, true).expect("a context was free");
    assert!(!s.nordic.hung);
    assert_eq!(crate::ble::sec_params_reply(&mut s, &mut rx, 0, true), Err(nrf::Error::Timeout));
    assert!(s.nordic.hung);
}

/// A frame shorter than its header says eats the next frame's start: the chip hangs on the mismatch.
#[test]
fn a_truncated_frame_hangs_the_chip() {
    let mut s = chip();
    s.nordic.expect_hang = Some("");
    s.nrf_send(&(WRITE20.len() as u16).to_le_bytes());
    s.nrf_send(&WRITE20[..10]);
    s.nrf_send(&[3, 0, 0, 0x65, 1]); // the next, whole command
    s.nrf_send(&WRITE20[10..]);
    assert!(s.nordic.hung, "{:?}", s.nordic.got);
}

/// The chip holds CTS after a command's header exactly while the previous response waits for an event frame that
/// waits for our RTS; it takes the header and our FIFO's 16 bytes, no more.
#[test]
fn cts_is_held_while_an_event_frame_keeps_the_response_waiting() {
    let mut s = chip();
    ant(&mut s, 3);
    s.delay_ms(1); // the first event frame stalls on our full FIFO
    // cmd 1 is taken whole (the chip had read no header); its response waits behind the event frame in flight
    assert_eq!(s.nrf_send(&[3, 0, 0, 0x65, 1]), 5);
    assert_eq!(s.nordic.resp.len(), 1, "queued, not out");
    // cmd 2: the header goes in, then CTS is held: 2 bytes to the chip and 16 in our FIFO
    let two = [(WRITE20.len() as u16).to_le_bytes().as_slice(), &WRITE20].concat();
    assert_eq!(s.nrf_send(&two), 18);
    assert!(s.nordic.blocked());
    // reading the event frame releases the response, and CTS: cmd 2 goes through whole
    drain(&mut s);
    assert!(!s.nordic.blocked());
    assert_eq!(s.nrf_send(&two[18..]), two.len() - 18);
    assert_eq!(s.nordic.got.len(), 2);
    // no hold when no response waits
    let mut s = chip();
    assert_eq!(s.nrf_send(&two), two.len());
    assert_eq!(s.nordic.got.len(), 1);
}

/// A hung chip takes 4 bytes more, then holds CTS for ever.
#[test]
fn a_hung_chip_takes_a_few_bytes_then_holds_cts() {
    let mut s = chip();
    s.nordic.hung = true;
    assert_eq!(s.nrf_send(&[0; 40]), 4 + 16);
    assert!(s.nordic.blocked());
    s.delay_ms(1000);
    let mut b = [0u8; 8];
    assert_eq!(s.nrf_recv(&mut b), 0);
    assert_eq!(s.nrf_send(&[0; 8]), 0);
}

/// The deadlock of 2026-11-01: two commands back to back while an event frame is in flight and our FIFO is full. The
/// chip cannot queue the first response (the TX buffer is busy on our RTS), so it holds CTS on the second command,
/// which is longer than our TX FIFO. A send that gave up there cut the frame short and hung the chip for ever.
#[test]
fn two_commands_back_to_back_behind_a_stalled_event_frame() {
    let mut s = chip();
    ant(&mut s, 3);
    s.delay_ms(1);
    let mut rx = nrf::Rx::new();
    assert_eq!(nrf::send(&mut s, &mut rx, &[0, 0x65, 1]), Ok(()));
    assert_eq!(nrf::send(&mut s, &mut rx, &WRITE20), Ok(()));
    assert_eq!(nrf::call(&mut s, &mut rx, &[0, 0x65, 1], &mut [0; 8]), Ok(5), "and the chip still answers");
    assert!(!s.nordic.hung, "{:?}", s.nordic.hang_reason);
    assert_eq!(s.nordic.got.len(), 3, "all three commands arrived whole");
}

/// A chip that never lets go: the send gives up after the command time-out and reports it (the radio watchdog then
/// resets the chip, and a truncated frame no longer matters).
#[test]
fn a_chip_that_holds_cts_for_ever_fails_the_send_in_bounded_time() {
    let mut s = chip();
    s.nordic.hung = true;
    let mut rx = nrf::Rx::new();
    let t0 = s.ms;
    assert_eq!(nrf::send(&mut s, &mut rx, &WRITE20), Err(nrf::Error::Timeout));
    assert!(s.ms - t0 <= u64::from(nrf::TIMEOUT_MS) + 5, "{} ms", s.ms - t0);
    assert_eq!(nrf::call(&mut s, &mut rx, &[0, 0x65, 1], &mut [0; 8]), Err(nrf::Error::Timeout));
    assert_eq!(rx.timeouts, 0, "a broken link counts no time-out (the reset is the watchdog's)");
}

/// Nine pairings in a row in the app's real flow (the phone asks, the app accepts, the chip ends the pairing with
/// AUTH_STATUS or DISCONNECTED) never hold more than one context: the app does not leak them.
#[test]
fn nine_pairings_in_a_row_never_exceed_8_live_contexts() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    let mut most = 0;
    for i in 0..9 {
        app.phone_event(&mut s, &[2, 0x13, 0, 0, 0]); // SEC_PARAMS_REQUEST, connection 0
        most = most.max(s.nordic.sec);
        assert!(!s.nordic.hung, "pairing {i}: {:?}", s.nordic.hang_reason);
        s.nordic.queue(&[2, 0x11, 0, 0, 0, 0x13]); // the pairing ends
    }
    assert!((1..=8).contains(&most), "{most} live contexts at most");
}

/// A call timed out; its answer arrives while the next call's send is held by CTS (the in-flight event frame keeps
/// the response waiting). The drain in `send` swallows it, so the next call gets its own answer.
#[test]
fn a_late_answer_arriving_while_the_send_is_held_is_swallowed() {
    let mut s = chip();
    ant(&mut s, 3);
    s.nordic.has_bytes(s.ms); // the first event frame is in flight, stalled on our full FIFO
    s.nordic.queue(&[1, 0x7f, 5, 0, 0, 0]); // the late answer: waits behind the event frame in flight
    let mut rx = nrf::Rx::new();
    rx.late = Some(0x7f);
    // sd_ble_gap_sec_params_reply: a command longer than the TX FIFO, so its send is held and drains
    assert_eq!(crate::ble::sec_params_reply(&mut s, &mut rx, 0, true), Ok(()), "its own answer, not the late one");
    assert_eq!(rx.late, None);
}

/// A send that fails leaves the swallow armed: the late answer may still come.
#[test]
fn a_failed_send_keeps_the_late_answer_armed() {
    let mut s = chip();
    s.nordic.hung = true;
    let mut rx = nrf::Rx::new();
    rx.late = Some(0x61);
    // longer than the TX FIFO and the 4 bytes a hung chip takes: the send fails
    assert_eq!(nrf::call(&mut s, &mut rx, &WRITE20, &mut [0; 8]), Err(nrf::Error::Timeout));
    assert_eq!(rx.late, Some(0x61));
}

/// A frame cut short (the chip holds CTS for ever mid-payload) leaves the chip mis-framed: no later frame is sent to
/// it, and the watchdog resets it on the next pass instead of after two time-outs.
#[test]
fn a_truncated_frame_has_the_watchdog_reset_the_chip_at_once() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    s.nordic.hung = true;
    let resets = s.nordic.resets.len();
    assert_eq!(nrf::send(&mut s, &mut app.rx, &WRITE20), Err(nrf::Error::Timeout));
    assert!(app.rx.broken);
    let (got, t0) = (s.nordic.got.len(), s.ms);
    assert_eq!(nrf::send(&mut s, &mut app.rx, &[0, 0x65, 1]), Err(nrf::Error::Timeout));
    assert_eq!((s.nordic.got.len(), s.ms), (got, t0), "nothing sent, no time spent");
    app.step(&mut s);
    assert_eq!(format!("{:?}", app.watch.reset), "Held", "PA6 goes low on the next pass");
    run(&mut s, &mut app, 3000);
    assert_eq!(s.nordic.resets.len(), resets + 1, "and the chip is reset, once");
    assert!(!app.rx.broken && !s.nordic.hung && app.radio_up());
}

/// The most a dead chip costs: one call, and a whole pass of the main loop (a probe, then the restart), ms.
#[test]
fn a_dead_chip_costs_a_bounded_time() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    s.nordic.hung = true;
    let t0 = s.ms;
    let _ = nrf::call(&mut s, &mut app.rx, &[0, 0x65, 1], &mut [0; 8]);
    let call = s.ms - t0;
    let mut pass = 0;
    for _ in 0..4000 {
        let t = s.ms;
        app.step(&mut s);
        pass = pass.max(s.ms - t);
        s.delay_ms(50);
        s.nordic.hung = true; // dead for ever, whatever a reset does
    }
    std::println!("dead chip: one call {call} ms, worst pass {pass} ms");
    assert!(call <= u64::from(nrf::TIMEOUT_MS) + 20, "{call}");
    assert!(pass <= 3 * u64::from(nrf::TIMEOUT_MS) + 1000, "{pass}");
}

/// nordic-app.md section 2, "One event in flight": the response is queued behind the event frame in flight, and the
/// chip holds CTS exactly until that frame has left its TX buffer, not before and not after.
#[test]
fn cts_is_released_when_the_event_frame_in_flight_ends() {
    let mut s = chip();
    ant(&mut s, 3);
    s.delay_ms(1);
    assert_eq!(s.nrf_send(&[3, 0, 0, 0x65, 1]), 5);
    assert_eq!(s.nrf_send(&[34, 0]), 2); // the next header: CTS is held from here
    assert!(s.nordic.blocked());
    let on_chip = s.nordic.out.len(); // what of the event frame is still in the chip's TX buffer
    let mut one = [0u8; 1];
    for read in 1..=on_chip {
        assert_eq!(s.nrf_recv(&mut one), 1);
        s.nordic.has_bytes(s.ms); // the byte that took the freed place has had its 10 us on the wire
        assert_eq!(s.nordic.blocked(), read < on_chip, "after {read} of {on_chip} bytes read");
    }
}

/// The ANT search time-out (30 s) after a chip reset: every channel times out together, and one SoftDevice pass brings
/// SEARCH_TIMEOUT and CHANNEL_CLOSED for each, on top of a queue already at the ride's high-water mark.
#[test]
fn the_search_timeouts_of_all_channels_together_stay_under_the_hang_limit() {
    let mut s = chip();
    ant(&mut s, 22); // the ride's high-water mark with the 100 ms pass (ride_radio_tests)
    s.nordic.has_bytes(s.ms); // the pass
    for ch in 0..8u8 {
        s.nordic.ant_event(&[1, 0x40, ch, 1, 1], 1, ch);
        s.nordic.ant_event(&[1, 0x40, ch, 1, 7], 7, ch);
    }
    s.nordic.has_bytes(s.ms);
    std::println!("search timeout burst: high-water {}", s.nordic.high_water);
    assert!(!s.nordic.hung, "{:?}", s.nordic.hang_reason);
    assert!(s.nordic.high_water < 48, "{}", s.nordic.high_water);
}

/// A broadcast frame (44 bytes) for channel `ch` with `data`.
fn broadcast(ch: u8, data: [u8; 8]) -> Vec<u8> {
    let mut p = vec![8, 9, 0x4e, ch];
    p.extend(data);
    p.resize(42, 0);
    p.extend([0x80, ch]);
    p
}

/// Feeds a frame into the app's reader as a command's wait does: framed, then kept.
fn keep_in(app: &mut App, payload: &[u8]) {
    for &b in (payload.len() as u16).to_le_bytes().iter().chain(payload) {
        if app.rx.push(b).is_some() {
            app.rx.keep_last();
        }
    }
}

/// Review 2, finding 1: a truncated frame is certain proof of a mis-framed chip, and it resets at once even inside
/// the wait after a resync (the first break since the last calm period).
#[test]
fn a_truncated_frame_resets_inside_the_wait_after_a_resync() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    app.watch.wait_ms = 5_000; // a resync just happened
    s.nordic.hung = true;
    let resets = s.nordic.resets.len();
    assert_eq!(nrf::send(&mut s, &mut app.rx, &WRITE20), Err(nrf::Error::Timeout));
    assert!(app.rx.broken);
    run(&mut s, &mut app, 3000); // the reset, then the chip's start (non-blocking), well inside the 5 s
    assert_eq!(s.nordic.resets.len(), resets + 1, "reset inside the wait");
    assert!(!app.rx.broken && app.radio_up());
}

/// Review 2, finding 2: a late answer that arrives in the main stream read clears the swallow.
#[test]
fn a_late_answer_in_the_stream_read_clears_the_swallow() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    app.rx.late = Some(0x7f);
    s.nordic.queue(&[1, 0x7f, 5, 0, 0, 0]);
    s.delay_ms(5);
    app.poll_nrf(&mut s);
    assert_eq!(app.rx.late, None);
}

/// Review 2, finding 3: a mis-framed chip is reset before the power-off goes to it, so the command arrives whole.
#[test]
fn power_off_resets_a_broken_link_first() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    s.nordic.hung = true;
    assert_eq!(nrf::send(&mut s, &mut app.rx, &WRITE20), Err(nrf::Error::Timeout));
    assert!(app.rx.broken);
    let resets = s.nordic.resets.len();
    app.power_off(&mut s);
    assert_eq!(s.nordic.resets.len(), resets + 1, "reset before the power-off");
    assert!(!s.nordic.hung);
    assert_eq!(s.nordic.got.last().map(Vec::as_slice), Some(&[nrf::CMD, nrf::SD_POWER_SYSTEM_OFF][..]));
}

/// Review 2, finding 5: the events kept while a command waited are older than the stream's: they are handled first.
#[test]
fn kept_events_are_handled_before_the_stream_read() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    let ch = crate::sensors::Kind::Hr.channel();
    keep_in(&mut app, &broadcast(ch, [0, 0, 0, 0, 0, 0, 0, 100]));
    s.nordic.queue(&broadcast(ch, [0, 0, 0, 0, 0, 0, 0, 140]));
    s.delay_ms(5);
    app.poll_nrf(&mut s);
    assert_eq!(app.live.get(crate::Metric::Hr), Some(140), "the newer broadcast is the last decoded");
}

/// Part 2: a radio reset runs across passes. The loop keeps its period (the GNSS is read, the keys and the screen
/// served), PA6 is low for 150 ms and the chip boots for 800 ms (BOARD.md 11), and the restart still succeeds.
#[test]
fn a_radio_reset_never_blocks_the_main_loop() {
    let mut s = Sim::new(9, 0, 0);
    let mut app = App::start(&mut s, None);
    run(&mut s, &mut app, 2000);
    s.nordic.hung = true;
    assert_eq!(nrf::send(&mut s, &mut app.rx, &WRITE20), Err(nrf::Error::Timeout));
    let (resets, t0, mut worst, mut passes) = (s.nordic.resets.len(), s.ms, 0, 0);
    while s.nordic.resets.len() == resets || !app.radio_up() || format!("{:?}", app.watch.reset) != "None" {
        let t = s.ms;
        app.step(&mut s);
        worst = worst.max(s.ms - t);
        passes += 1;
        s.delay_ms(POLL_MS);
        assert!(s.ms - t0 < 5000, "the restart finishes");
    }
    std::println!("radio reset: {passes} passes over {} ms, worst pass {worst} ms", s.ms - t0);
    assert!(worst < 100, "{worst} ms");
    assert!(s.ms - t0 >= u64::from(nrf::RESET_HOLD_MS + nrf::BOOT_MS), "the boot time is kept");
    assert_eq!(s.nordic.resets.len(), resets + 1);
    assert!(!app.rx.broken && app.radio_up());
}

/// Review 2, finding 7: a call on a broken link returns at once and counts neither a time-out nor a failure.
#[test]
fn a_call_on_a_broken_link_counts_nothing() {
    let mut s = chip();
    let mut rx = nrf::Rx::new();
    rx.broken = true;
    assert_eq!(nrf::call(&mut s, &mut rx, &[0, 0x65, 1], &mut [0; 8]), Err(nrf::Error::Timeout));
    assert_eq!((rx.timeouts, rx.failed), (0, None));
}
