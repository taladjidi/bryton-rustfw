//! The simulated touch controller (Sitronix ST1633 class, I2C1 0x55, BOARD.md 17), strict about what the part's
//! documents say. Sources (not re-read for this change: the pages timed out; "inferred" marks what no text settles):
//! - T1 RESET low at least 5 ms, then Init for about 50 ms: ST1633 datasheet v1.6 (https://www.orientdisplay.com/pdf/ST1633.pdf);
//!   a shorter low pulse leaves the chip dead until the next proper reset (the consequence is **inferred**).
//! - T2 Timeout to Idle (register 0x03, 2 s on the unit): Sitronix Touch IC Protocol A v2.7
//!   (https://www.crystalfontz.com/products/document/4590/Sitronix_Touch_IC_Protocol_A_V2.7.pdf). That the first
//!   report after the chip idled is lost, status Idle (4), is **inferred**; `idle_ms` None switches it off.
//! - T3 up to 5 contacts, one 3-byte slot each from 0x12, 4 bytes apart: same protocol document (slots past the
//!   second are **inferred** to continue the pattern); the driver reads the first two only.

extern crate std;

use std::boxed::Box;
use std::vec::Vec;

/// A finger as a function of the simulated time in ms (None: lifted).
pub(super) type FingerScript = Box<dyn Fn(u64) -> Option<(u16, u16)>>;

/// Init lasts this long after the reset is released, ms.
const INIT_MS: u64 = 50;
/// RESET must be low this long, ms (T1).
const RESET_LOW_MS: u64 = 5;

/// The touch chip: Init after its reset is released, Normal, Idle after `idle_ms` without a touch; INT low while a
/// finger is down.
pub struct TouchChip {
    /// When the reset was released (None: held in reset, or never released).
    pub(super) run_since: Option<u64>,
    /// When the reset went low, while it is.
    low_since: Option<u64>,
    /// The last reset was released after too short a low pulse: the chip never leaves Init (T1).
    pub dead: bool,
    /// The chip stops answering I2C (a wedged state machine) until a proper reset pulse; INT still follows the
    /// finger.
    pub wedged: bool,
    /// As `wedged`, but a reset does not help.
    pub wedged_hard: bool,
    /// Resets released so far.
    pub resets: u32,
    pub(super) finger: FingerScript,
    /// A second finger, for a pinch.
    pub(super) finger2: FingerScript,
    /// Contacts 3 to 5 (T3).
    pub extra: Vec<FingerScript>,
    /// Point reads so far (the app reads only while a finger is down).
    pub reads: u32,
    /// Idle after this many ms without a touch (T2); None: never.
    pub idle_ms: Option<u64>,
    /// The last time a touch was seen, ms.
    last_seen: u64,
    /// First reports lost to the chip waking from Idle.
    pub lost: u32,
}

impl TouchChip {
    pub(super) fn new() -> Self {
        TouchChip {
            run_since: None,
            low_since: None,
            dead: false,
            wedged: false,
            wedged_hard: false,
            resets: 0,
            finger: Box::new(|_| None),
            finger2: Box::new(|_| None),
            extra: Vec::new(),
            reads: 0,
            idle_ms: Some(2000),
            last_seen: 0,
            lost: 0,
        }
    }

    pub(super) fn set_finger(&mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) {
        self.finger = Box::new(f);
    }

    pub(super) fn set_finger2(&mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) {
        self.finger2 = Box::new(f);
    }

    /// The reset line (PH3): `run` releases it. A release after less than 5 ms low kills the chip (T1).
    pub(super) fn reset(&mut self, run: bool, ms: u64) {
        if !run {
            self.low_since.get_or_insert(ms);
            self.run_since = None;
        } else if self.run_since.is_none() {
            self.dead = self.low_since.is_some_and(|t| ms - t < RESET_LOW_MS);
            self.wedged &= self.dead || self.wedged_hard; // a proper pulse frees it
            self.resets += 1;
            self.low_since = None;
            self.run_since = Some(ms);
            self.last_seen = ms;
        }
    }

    fn ready(&self, ms: u64) -> bool {
        !self.dead && self.run_since.is_some_and(|t| ms >= t + INIT_MS)
    }

    fn touched(&self, ms: u64) -> bool {
        self.ready(ms) && (self.finger)(ms).is_some()
    }

    /// INT (PC6): low while a finger is down.
    pub(super) fn int(&self, ms: u64) -> bool {
        self.touched(ms)
    }

    fn slot(regs: &mut [u8], at: usize, p: (u16, u16)) {
        let (x, y) = p;
        regs[at..at + 3].copy_from_slice(&[0x80 | (x >> 4 & 0x70) as u8 | (y >> 8 & 7) as u8, x as u8, y as u8]);
    }

    pub(super) fn read(&mut self, ms: u64, reg: u8, buf: &mut [u8]) -> bool {
        let Some(t) = self.run_since else { return false }; // in reset: no acknowledge
        if self.wedged || self.wedged_hard {
            return false;
        }
        let mut regs = [0u8; 0x40];
        regs[0x00] = 0x05; // version
        regs[0x01] = if self.dead || ms < t + INIT_MS { 0x01 } else { 0x00 }; // Normal, else Init
        regs[0x04..0x07].copy_from_slice(&[0x01, 0xf0, 0x40]); // 240 x 320
        regs[0x0c..0x10].copy_from_slice(&[0, 0, 1, 2]);
        regs[0x3f] = 2;
        let p1 = if self.ready(ms) { (self.finger)(ms) } else { None };
        let down = p1.is_some();
        let asleep = self.idle_ms.is_some_and(|n| ms >= self.last_seen + n);
        if down && asleep {
            regs[0x01] = 0x04; // Idle: this report is the wake-up, no point in it (T2)
            self.lost += 1;
        } else if down {
            let others = [&self.finger2].into_iter().chain(self.extra.iter().take(3));
            for (i, p) in [p1].into_iter().chain(others.map(|f| f(ms))).enumerate() {
                if let Some(p) = p {
                    Self::slot(&mut regs, 0x12 + 4 * i, p);
                }
            }
        }
        if down {
            self.last_seen = ms;
        }
        if reg == 0x12 {
            self.reads += 1;
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = regs.get(usize::from(reg) + i).copied().unwrap_or(0);
        }
        true
    }
}
