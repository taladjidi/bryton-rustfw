//! Differential tests of the fast chip model (`chip_link.rs`, `chip_table.rs`) against Bryton's real nRF app in its
//! emulator (`tools/nrf_emu.py`): the same scripted traffic goes through the library's real radio code to both, and what
//! the host can see is compared: the frames out (type, op, length, order), whether the chip holds CTS at each
//! checkpoint, and the hang verdict with its step. `#[ignore]`d like `nrf_emu_tests.rs` (python3, `unicorn`, the binary);
//! without them each test prints `skip:` and passes. Run: `cargo test-host -- --ignored nrf_diff`.
//!
//! Times are not compared (the emulator counts one instruction per cycle and a stub SoftDevice costs nothing): a frame
//! arrives when the host looks, in the same order, or not.

use super::nrf_emu_tests::{Pipe, Shared};
use super::*;
use crate::nrf;
use std::cell::RefCell;
use std::rc::Rc;
use std::{format, string::String, vec, vec::Vec};

/// One side: a simulator whose chip is the fast model, or the emulated app.
struct Dut {
    sim: Sim,
    emu: Option<Shared>,
    rx: nrf::Rx,
    /// What the host saw, one line per observation.
    log: Vec<String>,
    /// The host holds RTS: it does not read.
    held: bool,
    hung: bool,
    /// Soak: count frames, keep no log.
    quiet: bool,
    /// Scripted operations so far: a hang is logged with the one it showed in.
    step: u32,
    /// The step the hang was seen in. Printed on a disagreement, not compared: the emulator finds a hang when time runs
    /// (the `advance` after the bytes), the model when it takes them, so the steps differ by one.
    hung_step: u32,
    frames: u64,
    ant_frames: u64,
    /// Bytes the chip's side accepted from the host so far.
    sent: usize,
    /// What `offer` could not hand over (the fast model's TX FIFO is 16), sent when RTS is released.
    rest: Vec<u8>,
    /// The hang's reason as that side words it (printed on a disagreement, never compared).
    reason: String,
}

impl Dut {
    fn new(emu: Option<Shared>) -> Dut {
        let mut sim = Sim::new(8, 0, 0);
        if let Some(e) = &emu {
            e.0.borrow_mut().soft = true;
            e.0.borrow_mut().req(&[b'F', 16, 0], false); // our USART2 RX FIFO: RTS drops when 16 bytes are unread
            sim.nrf_ext = Some(Box::new(e.clone()));
        }
        sim.nordic.expect_hang = Some("");
        let v = nrf::start(&mut sim).expect("start");
        let mut d = Dut {
            sim,
            emu,
            rx: nrf::Rx::new(),
            log: Vec::new(),
            held: false,
            hung: false,
            quiet: false,
            step: 0,
            hung_step: 0,
            frames: 0,
            ant_frames: 0,
            sent: 0,
            rest: Vec::new(),
            reason: String::new(),
        };
        d.log.push(format!("start central={}", v.central));
        d
    }

    fn check_hung(&mut self) {
        if self.hung {
            return;
        }
        let (h, why) = match &self.emu {
            Some(e) => {
                let p = e.0.borrow();
                (p.hung.is_some() || p.flags & 4 != 0, p.hung.clone().unwrap_or_default())
            }
            None => (self.sim.nordic.hung, self.sim.nordic.hang_reason.clone().unwrap_or_default()),
        };
        if h {
            (self.hung, self.reason) = (true, why);
            self.log.push("HUNG".into());
            self.hung_step = self.step;
        }
    }

    fn cts(&self) -> bool {
        match &self.emu {
            Some(e) => e.0.borrow().flags & 1 != 0,
            None => self.sim.nordic.blocked(),
        }
    }

    fn checkpoint(&mut self, name: &str) {
        let c = self.cts();
        self.log.push(format!("{name}: cts held {c}"));
    }

    /// Time passes (nothing is read).
    fn advance(&mut self, ms: u32) {
        self.step += 1;
        self.tick(ms);
    }

    fn tick(&mut self, ms: u32) {
        if self.hung && self.emu.is_some() {
            return;
        }
        self.sim.delay_ms(ms);
        self.check_hung();
    }

    fn hold(&mut self, held: bool) {
        self.held = held;
        if let Some(e) = &self.emu {
            e.0.borrow_mut().req(&[b'H', u8::from(held)], false);
        }
    }

    /// The host reads what is there, as the ride pass does, and logs the frames.
    fn poll(&mut self) {
        self.step += 1;
        if self.held || self.hung {
            return;
        }
        if !self.rest.is_empty() {
            let rest = std::mem::take(&mut self.rest);
            let n = self.sim.nrf_send(&rest);
            (self.sent, self.rest) = (self.sent + n, rest[n..].to_vec());
        }
        let mut buf = [0u8; 64];
        for _ in 0..2000 {
            let n = self.sim.nrf_recv(&mut buf);
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if let Some(p) = self.rx.push(b) {
                    self.frames += 1;
                    self.ant_frames += u64::from(p[0] == 8);
                    if !self.quiet {
                        let line = format!("frame {:02x} {:02x} len {}", p[0], p.get(1).copied().unwrap_or(0), p.len());
                        self.log.push(line);
                    }
                    self.rx.keep_last();
                }
            }
        }
        self.check_hung();
    }

    /// Pass of the ride loop: read, then sleep `ms`.
    fn pass(&mut self, ms: u32) {
        self.poll();
        self.advance(ms);
    }

    fn ant(&mut self, ch: u8, data: [u8; 8]) {
        self.step += 1;
        if self.hung {
            return;
        }
        match &self.emu {
            Some(e) => {
                e.0.borrow_mut().req(&[&[b'A', ch, 0x80, 8][..], &data].concat(), false);
            }
            None => self.sim.nordic.ant_event(&[&[9, 0x4e, ch][..], &data].concat(), 0x80, ch),
        }
        self.check_hung();
    }

    /// A BLE GATTC notification of `n` data bytes (a phone service or a BLE sensor).
    fn hvx(&mut self, n: u16) {
        self.step += 1;
        if self.hung {
            return;
        }
        match &self.emu {
            Some(e) => {
                e.0.borrow_mut().req(&[b'V', n as u8, (n >> 8) as u8], false);
            }
            None => {
                let mut p = vec![2, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x0f, 0, 1];
                p.extend(n.to_le_bytes());
                p.extend((0..n).map(|i| i as u8));
                self.sim.nordic.queue(&p);
            }
        }
        self.check_hung();
    }

    /// Raw bytes to the chip, as the library's `nrf::send` would put them (it sleeps while the FIFO is full).
    fn send(&mut self, bytes: &[u8]) {
        if self.hung {
            return;
        }
        let mut rest = bytes;
        for _ in 0..200 {
            if rest.is_empty() {
                break;
            }
            let n = self.sim.nrf_send(rest);
            self.sent += n;
            rest = &rest[n..];
            if !rest.is_empty() {
                self.tick(1);
            }
        }
        self.check_hung();
    }

    /// Bytes offered to the chip once, then `ms` of waiting: how many bytes in all left the host (the chip's UART took them, its
    /// 4-byte FIFO included). `nrf_send` takes 16 at most while the chip holds CTS.
    fn offer(&mut self, bytes: &[u8], ms: u32) -> usize {
        self.step += 1;
        if self.hung {
            return 0;
        }
        let taken = self.sim.nrf_send(bytes);
        self.sent += taken;
        self.rest = bytes[taken..].to_vec();
        self.advance(ms);
        let pending = match &self.emu {
            Some(e) => {
                let r = e.0.borrow_mut().req(b"N", true);
                usize::from(u16::from_le_bytes([r[0], r[1]]))
            }
            None => self.sim.nordic.txq.len(),
        };
        self.sent - pending
    }

    fn command(&mut self, payload: &[u8]) {
        self.step += 1;
        self.send(&[&(payload.len() as u16).to_le_bytes()[..], payload].concat());
    }
}

/// Runs `script` against both and fails with both traces when they differ. Skips when the emulator is not there.
fn both(name: &str, script: impl Fn(&mut Dut)) {
    let Some(pipe) = Pipe::spawn() else { return };
    let mut emu = Dut::new(Some(Shared(Rc::new(RefCell::new(pipe)))));
    let mut fast = Dut::new(None);
    script(&mut emu);
    script(&mut fast);
    if emu.log != fast.log {
        let n = emu.log.iter().zip(&fast.log).take_while(|(a, b)| a == b).count();
        panic!(
            "{name}: the fast model and the real app disagree at line {n}\n emulated ({}, hang at step {}): {:#?}\n fast ({}, hang at step {}): {:#?}",
            emu.reason, emu.hung_step, &emu.log, fast.reason, fast.hung_step, &fast.log
        );
    }
    std::println!(
        "agree: {name} ({} lines, hung {}: emulated '{}' / fast '{}')",
        fast.log.len(),
        fast.hung,
        emu.reason,
        fast.reason
    );
}

const VERSION_GET: [u8; 3] = [0, 0x65, 1];

fn drain(d: &mut Dut) {
    for _ in 0..40 {
        d.pass(5);
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_start_and_version() {
    both("start and a version request", |d| {
        d.command(&VERSION_GET);
        drain(d);
        d.checkpoint("after");
    });
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_ant_four_hz_three_channels() {
    // drained every pass, then every 20 passes (a backlog builds, a request each second meets a held CTS)
    for every in [1, 20] {
        both(&format!("ANT 4 Hz on 3 channels, drained every {every} passes"), move |d| {
            for (i, ms) in (0..6000u32).step_by(10).enumerate() {
                for ch in 0..3u8 {
                    if ms % 250 == u32::from(ch) * 80 {
                        d.ant(ch, [ch, 1, 2, 3, 4, 5, 6, (ms / 250) as u8]);
                    }
                }
                if ms % 1000 == 500 {
                    d.command(&VERSION_GET);
                }
                if i % every == every - 1 {
                    d.poll();
                }
                d.advance(10);
                if ms % 1000 == 990 {
                    d.checkpoint(&format!("{ms} ms"));
                }
            }
            d.checkpoint("end");
        });
    }
}

fn held_for(ms: u32) -> impl Fn(&mut Dut) {
    move |d| {
        d.hold(true);
        for i in 0..30u32 {
            d.ant((i % 3) as u8, [i as u8; 8]);
            d.advance(ms / 30);
        }
        d.checkpoint("held");
        d.hold(false);
        drain(d);
        d.checkpoint("released");
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_rts_held() {
    both("RTS held 200 ms with 30 events", held_for(200));
    both("RTS held 2 s with 30 events", held_for(2000));
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_event_burst() {
    for (n, name) in [(96, "96 events in one pass"), (97, "97 events in one pass")] {
        both(name, move |d| {
            for _ in 0..n {
                d.hvx(3); // 3 bytes: a lone byte is encoded with a zero length by the emulated app (an artefact of its inferred input)
            }
            d.advance(1);
            drain(d);
            d.checkpoint("end");
        });
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_fatal_rules() {
    // Not here: an empty payload (a length of 0). The emulator does not model a MAXCNT of 0, so it takes the header and
    // goes on; nordic-app.md says the real app hangs, and the fast model follows that.
    let cases: [(&str, Vec<u8>); 6] = [
        ("unknown opcode", vec![0, 0xfe]),
        ("unknown ANT opcode", vec![6, 0xfe]),
        ("wrong length (version_get with an extra byte)", vec![0, 0x65, 1, 0, 0]),
        ("type only", vec![0]),
        ("type 7", vec![7, 0x65, 1]),
        ("oversize", vec![0; 400]),
    ];
    for (name, p) in cases {
        both(name, move |d| {
            d.command(&VERSION_GET);
            drain(d);
            d.command(&p);
            d.advance(50);
            d.poll(); // no CTS checkpoint: the emulator stops stepping at the hang, so its flag is stale from there
        });
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_two_commands_behind_a_stalled_event() {
    both("two commands behind a stalled event frame", |d| {
        d.hold(true);
        d.ant(0, [7; 8]);
        d.advance(5); // the event frame is in flight and waits on RTS
        d.command(&VERSION_GET);
        d.advance(2);
        d.checkpoint("first command sent");
        d.command(&VERSION_GET);
        d.advance(2);
        d.checkpoint("second command sent");
        // a longer one: the bytes the chip takes before it holds CTS (header, then its UART's 4-byte FIFO)
        let mut name = vec![0, 0x72, 20, 1];
        name.extend([b'x'; 20]);
        name.extend([0, 0]);
        let frame = [&(name.len() as u16).to_le_bytes()[..], &name].concat();
        let took = d.offer(&frame, 5);
        d.log.push(format!("bytes taken in all behind the stall: {took}"));
        d.checkpoint("third command offered");
        d.hold(false);
        drain(d);
        d.checkpoint("released");
    });
}

/// Seconds of emulated ride traffic for [`nrf_soak`] (`NRF_SOAK_S`, default 3600).
fn soak_seconds() -> u32 {
    std::env::var("NRF_SOAK_S").ok().and_then(|v| v.parse().ok()).unwrap_or(3600)
}

/// A ride's radio traffic through the library's radio code against the emulated app: three ANT channels at about 4 Hz
/// (a power meter, a heart-rate strap, a cadence sensor, each at its own period), a BLE trainer's notifications at 10 Hz,
/// the phone's ANCS / AMS burst of 12 notifications every 30 s, a version request every 10 s through the real command
/// path, and the host's ride pass every 50 ms (`POLL_MS`) that now and then stalls 200 ms (a screen redraw, a save).
/// Asserts that the app never hangs, that every ANT broadcast and notification came out, and prints the deepest backlog
/// the host saw (events injected and not yet read). `NRF_SOAK_S` sets the length; an hour of emulated time takes about
/// 10 minutes (docs/devloop.md).
#[test]
#[ignore = "soak: needs python3 + unicorn + Bryton's nordic.bin; NRF_SOAK_S=60 cargo test-host -- --ignored nrf_soak --nocapture"]
fn nrf_soak() {
    let Some(pipe) = Pipe::spawn() else { return };
    let mut d = Dut::new(Some(Shared(Rc::new(RefCell::new(pipe)))));
    d.quiet = true;
    let secs = soak_seconds();
    let periods = [250u32, 246, 252]; // ms, the sensors drift against each other
    let (mut injected, mut ants, mut backlog_max) = (0u64, 0u64, 0u64);
    let started = std::time::Instant::now();
    for pass in 0..secs * 20 {
        let ms = pass * 50;
        for (ch, per) in periods.iter().enumerate() {
            // at most one event per sensor per pass: the periods are 5 passes apart
            if (ms / per) != ((ms + 50) / per) {
                d.ant(ch as u8, [ch as u8, 1, 2, 3, 4, 5, 6, (ms / per) as u8]);
                (injected, ants) = (injected + 1, ants + 1);
            }
        }
        if pass % 2 == 0 {
            d.hvx(13); // the trainer, 10 Hz
            injected += 1;
        }
        if ms % 30_000 == 15_000 {
            for i in 0..12 {
                d.hvx(20 + i); // ANCS / AMS
                injected += 1;
            }
        }
        if ms % 10_000 == 0 {
            d.command(&VERSION_GET);
            injected += 1;
        }
        backlog_max = backlog_max.max(injected.saturating_sub(d.frames));
        if pass % 400 == 399 {
            d.advance(200); // the host is busy: no read for 200 ms, the chip waits on a full FIFO
        }
        d.pass(50);
        assert!(!d.hung, "the app hung at {ms} ms: {}", d.reason);
    }
    for _ in 0..200 {
        d.pass(50);
    }
    std::println!(
        "soak: {secs} s emulated in {:.0} s; {injected} events and requests in, {} frames out ({} ANT of {ants}); deepest backlog {backlog_max}",
        started.elapsed().as_secs_f32(),
        d.frames,
        d.ant_frames,
    );
    assert!(!d.hung, "{}", d.reason);
    assert_eq!(d.frames, injected, "every event and response came out");
    assert_eq!(d.ant_frames, ants, "no broadcast lost");
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_diff"]
fn nrf_diff_stall_by_payload_length() {
    // A command whose header is read behind a stalled event frame: the chip's UART takes 4 more bytes, so a payload of
    // 3 lets the next header's first byte in, 4 fills the FIFO exactly, 5 holds one byte of the payload back.
    for (name, cmd2) in [
        ("3-byte payload", VERSION_GET.to_vec()),
        ("4-byte payload", vec![6, 0xda, 0, 10]),
        ("5-byte payload", vec![6, 0xc4, 0, 0, 0]),
    ] {
        both(&format!("a {name} behind a stalled event, then another command"), move |d| {
            d.hold(true);
            d.ant(0, [7; 8]);
            d.advance(5);
            d.command(&VERSION_GET);
            d.advance(2);
            let f2 = [&(cmd2.len() as u16).to_le_bytes()[..], &cmd2].concat();
            let took = d.offer(&f2, 2);
            d.log.push(format!("bytes taken in all after the second: {took}"));
            d.checkpoint("second offered");
            let f3 = [&5u16.to_le_bytes()[..], &VERSION_GET[..]].concat();
            let took = d.offer(&f3, 2);
            d.log.push(format!("bytes taken in all after the third: {took}"));
            d.checkpoint("third offered");
            d.hold(false);
            drain(d);
            d.checkpoint("released");
        });
    }
}
