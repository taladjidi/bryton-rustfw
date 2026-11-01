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
    assert_eq!(rx.timeouts, 1);
}
