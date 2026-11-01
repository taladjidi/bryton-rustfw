//! The Nordic link's transport and queues, as the chip and our USART2 do them (BOARD.md 11 has the facts and sources).
//!
//! Our side: USART2's RX FIFO of 16 bytes (RM0399, USART FIFO mode); our RTS drops when it is full, so the chip's
//! transmitter stalls mid-frame and resumes exactly where it stopped, with no time-out. Our TX FIFO is 16 bytes too, and
//! drains only while the chip's RTS (our CTS) allows. A byte is 10 bits at 1 Mbaud: 10 us.
//!
//! The chip (Bryton's nRF app, reversed; bryton-S510-mods ghidra/nordic-app.md): one frame in flight on a single TX
//! buffer, 15 us between two frames (measured in an emulator of the app); a command's header read, it holds our CTS (after
//! the 4 bytes its UART still takes) until the previous command's response is queued, and queuing needs the TX buffer, which an event frame in flight keeps while
//! it waits on our RTS; no new event frame starts between a command's header and its response. The events come from the
//! SoftDevice in passes (all pending BLE, then all pending ANT) into 96 slots; a pass bringing more events than free
//! slots hangs the app; after a put leaves 0 free it stops pulling until more than 48 are free. The SoftDevice's ANT queue
//! holds 32 (inferred: the newest is lost when it is full; the S332 text was not found). 8 security contexts.
//! A hung chip never sends again, takes 2 to 6 more bytes (4 here) and holds CTS.
//!
//! Time: the chip's clock `t` (us) follows the simulated ms and runs ahead of it while the host reads or writes bytes
//! (wire time); a receive that finds nothing within the board's wait ends its burst.

use super::*;

/// The USART FIFO depth, both ways (RM0399).
pub(super) const FIFO: usize = 16;
/// One byte on the wire, us (1 Mbaud, 10 bits).
pub(super) const BYTE_US: u64 = 10;
/// More than this many free slots resume the pull.
pub(super) const RESUME_FREE: usize = 48;
/// The SoftDevice's ANT event queue.
pub(super) const ANT_QUEUE: usize = 32;
/// Bytes a hung chip still takes.
const HUNG_TAKES: u8 = 4;
/// The chip's UARTE FIFO: the bytes it still takes after the header it holds CTS on (found by the differential test
/// against the emulated app, nrf_diff_tests.rs; nRF52 UARTE, 4 bytes).
const UART_FIFO: u8 = 4;

impl Nordic {
    /// The real chip's side of an event the fuzzes make up for the app: a DISCONNECTED or AUTH_STATUS frees a security
    /// context, and it asks for pairing (SEC_PARAMS_REQUEST) only after the last pairing ended, so a made-up request
    /// with all contexts live stands for an AUTH_STATUS that came before. The limit of 8 stays on.
    pub(super) fn frees_context(&mut self, event: &[u8]) {
        let full = self.sec >= self.limits.sec_contexts;
        if matches!(event, [2, 0x11 | 0x19, 0, ..]) || (full && matches!(event, [2, 0x13, 0, ..])) {
            self.sec = self.sec.saturating_sub(1);
        }
    }

    /// The clock follows the simulated time.
    pub(super) fn sync(&mut self, ms: u64) {
        self.now_ms = ms;
        self.t = self.t.max(ms * 1000);
    }

    /// The pipeline was idle: a frame that appears now cannot start long ago. The simulator makes an event when the
    /// host looks (a tick covers the last ms), so it counts as made up to 1 ms ago: ready for the host's read.
    pub(super) fn touch(&mut self) {
        if self.out.is_empty() && self.events.is_empty() && self.resp.is_empty() {
            self.tx_at = self.tx_at.max(self.t.saturating_sub(1000));
        }
    }

    /// The chip holds our CTS: it has read a command's header while the previous response still waits for the TX
    /// buffer (an event frame in flight), or it is hung and has taken what it takes.
    pub(super) fn blocked(&self) -> bool {
        if self.hung {
            return self.hung_rx >= HUNG_TAKES;
        }
        matches!(self.stall, Some(k) if k >= UART_FIFO) && !self.resp.is_empty() && !self.out.is_empty()
    }

    /// SoftDevice to queue: one pass (all pending BLE, then all pending ANT). More than the free slots: the app hangs.
    fn pull(&mut self) {
        let slots = self.limits.slots;
        if self.suspended && self.events.len() + RESUME_FREE < slots {
            self.suspended = false;
        }
        let n = self.sd_ble.len() + self.sd_ant.len();
        if self.suspended || n == 0 || self.hung {
            return;
        }
        let free = slots.saturating_sub(self.events.len());
        if n > free {
            let why = format!("event queue overflow ({n} events, {free} free)");
            return self.hang_on_why(why);
        }
        self.touch();
        self.events.extend(self.sd_ble.drain(..));
        self.events.extend(self.sd_ant.drain(..));
        self.high_water = self.high_water.max(self.events.len());
        self.suspended = self.events.len() >= slots;
    }

    /// Starts the next frame when the TX buffer is free: a response first, an event unless a command's header has been
    /// read and its response is not out yet. Chaos damages it here.
    fn start_frame(&mut self) -> bool {
        let frame = if let Some(f) = self.resp.pop_front() {
            f
        } else if self.frame.len() < 2
            && let Some(f) = self.events.pop_front()
        {
            f
        } else {
            return false;
        };
        self.tx_at = self.tx_at.max(self.frame_end + u64::from(self.limits.frame_gap_us));
        self.out = self.damage(frame);
        true
    }

    /// Everything that can happen up to the clock `t`: pulls, our queued bytes reaching the chip, the chip's bytes
    /// reaching our FIFO (until it is full: RTS).
    pub(super) fn run(&mut self) {
        loop {
            let mut moved = false;
            self.pull();
            if self.resp.is_empty() || self.out.is_empty() {
                self.stall = None;
            }
            while !self.blocked() {
                let Some(b) = self.txq.pop_front() else { break };
                self.t += BYTE_US;
                moved = true;
                if self.hung {
                    self.hung_rx += 1;
                } else {
                    self.feed(b, self.now_ms);
                    // a header read while the previous response waits for the TX buffer: the UART takes 4 more
                    match &mut self.stall {
                        Some(k) => *k = k.saturating_add(1),
                        None if self.frame.len() == 2 && !self.resp.is_empty() && !self.out.is_empty() => {
                            self.stall = Some(0);
                        }
                        None => {}
                    }
                }
            }
            while self.fifo.len() < FIFO && !self.hung {
                if self.out.is_empty() && !self.start_frame() {
                    break;
                }
                let at = self.tx_at + BYTE_US;
                if at > self.t {
                    break;
                }
                let Some(b) = self.out.pop_front() else { break };
                self.fifo.push_back(b);
                self.tx_at = at;
                moved = true;
                if self.out.is_empty() {
                    self.frame_end = at;
                }
            }
            if !moved {
                return;
            }
        }
    }

    /// When the chip's next byte reaches our FIFO, if it has one to send.
    fn next_arrival(&self) -> Option<u64> {
        if self.hung || self.fifo.len() >= FIFO {
            return None;
        }
        let start = if !self.out.is_empty() {
            self.tx_at
        } else if !self.resp.is_empty() || (self.frame.len() < 2 && !self.events.is_empty()) {
            self.tx_at.max(self.frame_end + u64::from(self.limits.frame_gap_us))
        } else {
            return None;
        };
        Some(start + BYTE_US)
    }

    /// `nrf_recv`: the bytes in the FIFO, then as long as the next byte comes within the board's wait of the last one.
    pub(super) fn recv(&mut self, buf: &mut [u8], ms: u64) -> usize {
        self.sync(ms);
        self.run();
        let (mut n, mut last) = (0, self.t);
        while n < buf.len() {
            if let Some(b) = self.fifo.pop_front() {
                if self.fifo.len() + 1 == FIFO {
                    self.tx_at = self.tx_at.max(self.t); // RTS rises: the chip goes on from here
                }
                buf[n] = b;
                n += 1;
                last = self.t;
                self.run();
            } else if let Some(at) =
                self.next_arrival().filter(|&at| n > 0 && at <= last + u64::from(self.limits.wait_us))
            {
                self.t = at;
                self.run();
            } else {
                break;
            }
        }
        if n > 0 {
            self.t += u64::from(self.limits.parse_us); // the host decodes what it got before it reads again
        }
        n
    }

    /// `nrf_send`: the bytes our TX FIFO takes. While the chip holds CTS it fills (16) and then refuses: the board
    /// returns at once with how many it took (the library sleeps and offers the rest again).
    pub(super) fn send(&mut self, data: &[u8], ms: u64) -> usize {
        self.sync(ms);
        self.run();
        for (i, &b) in data.iter().enumerate() {
            if self.txq.len() >= FIFO {
                return i;
            }
            self.txq.push_back(b);
            self.run();
        }
        data.len()
    }

    /// The chip is back (a reset in the field): nothing of the hung state, and the bytes held in our TX FIFO are gone.
    pub fn unhang(&mut self) {
        (self.hung, self.hung_rx) = (false, 0);
        self.txq.clear();
        self.frame.clear();
        self.stall = None;
    }

    /// A byte is in the FIFO now (one still on the wire is not).
    pub(super) fn has_bytes(&mut self, ms: u64) -> bool {
        self.sync(ms);
        self.run();
        !self.fifo.is_empty()
    }
}
