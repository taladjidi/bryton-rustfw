//! Power meter calibration over ANT+ (the Bicycle Power profile's calibration page 0x01): the zero offset the rider
//! asks for, the auto zero switch, and the meter's answers. Pure logic; the radio side is app/powercal.rs.
//!
//! Layouts (8 data bytes, byte 0 the page), as open implementations read them (the ANT+ members' document itself was
//! not available; anything beyond these sources is marked "inferred"):
//! - Nordic nRF5 SDK `ant_bpwr_page_1` (docs.nordicsemi.com, "Bicycle Power profile page 1", SDK 16.0.0): calibration
//!   ids `0xAA` manual zero request, `0xAC` manual zero successful, `0xAF` failed, and the auto zero status values
//!   `0xFF` not supported, `0x00` off, `0x01` on. The SDK also says a receiver must let the rider start and confirm a
//!   manual zero offset.
//! - GoldenCheetah `src/ANT/ANTMessage.cpp` (github.com/GoldenCheetah/GoldenCheetah, power channel, page 0x01): the id
//!   is byte 1, the auto zero status byte 2, the offset bytes 6 and 7 little endian, on both `0xAC` and `0xAF`; id
//!   `0x12` (auto zero support) carries bit 0 enabled and bit 1 supported; `0xAB` is the auto zero configuration
//!   request.
//! - The requests: manual zero `[01 AA FF FF FF FF FF FF]`; auto zero `[01 AB on FF FF FF FF FF]` with on 0 or 1
//!   (inferred: the unused bytes are 0xFF as everywhere in ANT+). Sent as acknowledged messages (inferred).
//! - The profile gives no time-out that could be checked: [`TIMEOUT_MS`] is ours (inferred).
//!
//! The Sigeyi AXO spider (the owner's meter): its retailers list auto zero and a manual zero offset (enroute.cc AXO SL
//! listing; powermetercity.com); no Sigeyi document of its page 0x01 answers was found, so whether it replies to the
//! requests, and what its offset means, is unverified (docs/status.md).

/// How long a request waits for the meter's answer, ms (inferred: the profile gives none).
pub const TIMEOUT_MS: u32 = 10_000;
/// The wait before a request is sent again, ms: the power channel's period is 125 ms; the chip queues one message.
pub const RETRY_MS: u32 = 250;
/// How long a result stays on the screen before it goes back to the instructions, ms.
pub const SHOW_MS: u32 = 8_000;

/// The auto zero state a meter reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoZero {
    /// The meter has none.
    Unsupported,
    /// Supported, off.
    Off,
    /// Supported, on.
    On,
}

/// A calibration page from the meter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// The manual zero ended: `ok`, the offset the meter reports, and its auto zero state when it says.
    Zero {
        /// 0xAC (true) or 0xAF (false).
        ok: bool,
        /// Calibration data, bytes 6 and 7 (signed, little endian).
        offset: i16,
        /// Byte 2 of the page.
        auto: Option<AutoZero>,
    },
    /// The meter's auto zero state alone (id 0x12).
    Auto(AutoZero),
}

/// The meter's reply in broadcast `d`, if it is a calibration page it answers with; any other page, or an id not
/// known, is None (never a guess).
///
/// ```
/// use bryton_rustfw::powercal::{reply, AutoZero, Reply};
/// let d = [1, 0xac, 0xff, 0xff, 0xff, 0xff, 0x2c, 0x01];
/// assert_eq!(reply(d), Some(Reply::Zero { ok: true, offset: 300, auto: Some(AutoZero::Unsupported) }));
/// assert_eq!(reply([0x10, 0xac, 0, 0, 0, 0, 0, 0]), None);
/// ```
pub fn reply(d: [u8; 8]) -> Option<Reply> {
    let [page, id, st, _, _, _, o0, o1] = d;
    let auto = |s: u8| match s {
        0 => Some(AutoZero::Off),
        1 => Some(AutoZero::On),
        0xff => Some(AutoZero::Unsupported),
        _ => None,
    };
    match (page, id) {
        (1, 0xac | 0xaf) => Some(Reply::Zero { ok: id == 0xac, offset: i16::from_le_bytes([o0, o1]), auto: auto(st) }),
        (1, 0x12) => Some(Reply::Auto(if st & 2 == 0 {
            AutoZero::Unsupported
        } else if st & 1 == 0 {
            AutoZero::Off
        } else {
            AutoZero::On
        })),
        _ => None,
    }
}

/// The manual zero request.
pub const ZERO_REQUEST: [u8; 8] = [1, 0xaa, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

/// The auto zero configuration request.
pub fn auto_request(on: bool) -> [u8; 8] {
    [1, 0xab, u8::from(on), 0xff, 0xff, 0xff, 0xff, 0xff]
}

/// What the calibration screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The instructions; nothing asked yet.
    Ready,
    /// A request is out, the answer awaited.
    Asking,
    /// Zeroed: the offset, and the one before it when known.
    Done(i16, Option<i16>),
    /// The meter said it failed.
    Failed,
    /// The meter never answered.
    NoAnswer,
    /// The auto zero state was set and confirmed (see [`Cal::auto`]).
    AutoSet,
}

/// What a request asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Zero,
    Auto(bool),
}

/// The calibration in progress.
#[derive(Clone, Copy, Debug)]
pub struct Cal {
    phase: Phase,
    op: Op,
    /// The request is to go out (first time, or after a refusal or a failed delivery).
    send: bool,
    ms: u32,
    /// Time before the next send, ms (the chip takes one acknowledged message a channel period).
    wait: u32,
    /// The last zero offset seen this session.
    last: Option<i16>,
    /// The meter's auto zero state, once it said.
    auto: Option<AutoZero>,
}

impl Default for Cal {
    fn default() -> Self {
        Cal { phase: Phase::Ready, op: Op::Zero, send: false, ms: 0, wait: 0, last: None, auto: None }
    }
}

impl Cal {
    /// Where it is.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The meter's auto zero state, if it said (a zero's answer or an auto zero answer).
    pub fn auto(&self) -> Option<AutoZero> {
        self.auto
    }

    /// A request is out.
    pub fn asking(&self) -> bool {
        self.phase == Phase::Asking
    }

    /// Back to the instructions (the screen opens, or Back).
    pub fn reset(&mut self) {
        (self.phase, self.send, self.ms) = (Phase::Ready, false, 0);
    }

    fn ask(&mut self, op: Op) {
        if !self.asking() {
            (self.phase, self.op, self.send, self.ms, self.wait) = (Phase::Asking, op, true, 0, 0);
        }
    }

    /// The rider asks for a zero offset.
    pub fn start_zero(&mut self) {
        self.ask(Op::Zero);
    }

    /// The rider switches the auto zero (only when the meter supports it).
    pub fn toggle_auto(&mut self) {
        if let Some(a @ (AutoZero::On | AutoZero::Off)) = self.auto {
            self.ask(Op::Auto(a == AutoZero::Off));
        }
    }

    /// The page to send now, if one is due (taken: it is not due again unless [`Cal::unsent`]).
    pub fn due(&mut self) -> Option<[u8; 8]> {
        if !self.asking() || self.wait > 0 || !core::mem::take(&mut self.send) {
            return None;
        }
        Some(match self.op {
            Op::Zero => ZERO_REQUEST,
            Op::Auto(on) => auto_request(on),
        })
    }

    /// The chip refused the message or the meter did not take it: sent again.
    pub fn unsent(&mut self) {
        self.send = self.asking();
        self.wait = RETRY_MS;
    }

    /// The meter's reply; true when the screen changes.
    pub fn heard(&mut self, r: Reply) -> bool {
        if let Reply::Zero { auto: Some(a), .. } | Reply::Auto(a) = r {
            self.auto = Some(a);
        }
        if !self.asking() {
            return false; // nothing asked: an old answer, or another receiver's
        }
        self.phase = match (r, self.op) {
            (Reply::Zero { ok: false, .. }, _) => Phase::Failed,
            (Reply::Zero { offset, .. }, Op::Zero) => Phase::Done(offset, self.last.replace(offset)),
            (Reply::Zero { .. } | Reply::Auto(_), Op::Auto(_)) if self.auto.is_some() => Phase::AutoSet,
            _ => return false, // an answer to the other question: still waiting
        };
        (self.ms, self.send) = (0, false);
        true
    }

    /// `dt_ms` passed; true when the screen changes.
    pub fn tick(&mut self, dt_ms: u32) -> bool {
        self.ms = self.ms.saturating_add(dt_ms);
        self.wait = self.wait.saturating_sub(dt_ms);
        if self.asking() && self.ms >= TIMEOUT_MS {
            (self.phase, self.send, self.ms) = (Phase::NoAnswer, false, 0);
            true
        } else if !matches!(self.phase, Phase::Ready | Phase::Asking) && self.ms >= SHOW_MS {
            self.reset();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_from_the_layouts() {
        let ok = [1, 0xac, 1, 0xff, 0xff, 0xff, 0xfe, 0xff];
        assert_eq!(reply(ok), Some(Reply::Zero { ok: true, offset: -2, auto: Some(AutoZero::On) }));
        let bad = [1, 0xaf, 0, 0xff, 0xff, 0xff, 0, 0];
        assert_eq!(reply(bad), Some(Reply::Zero { ok: false, offset: 0, auto: Some(AutoZero::Off) }));
        assert_eq!(reply([1, 0x12, 3, 0, 0, 0, 0, 0]), Some(Reply::Auto(AutoZero::On)));
        assert_eq!(reply([1, 0x12, 2, 0, 0, 0, 0, 0]), Some(Reply::Auto(AutoZero::Off)));
        assert_eq!(reply([1, 0x12, 0, 0, 0, 0, 0, 0]), Some(Reply::Auto(AutoZero::Unsupported)));
        assert_eq!(reply([1, 0xac, 7, 0, 0, 0, 1, 0]), Some(Reply::Zero { ok: true, offset: 1, auto: None }));
        assert_eq!(reply([1, 0xaa, 0, 0, 0, 0, 0, 0]), None, "a request is not a reply");
        assert_eq!(reply([0x10, 0xac, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(auto_request(true), [1, 0xab, 1, 0xff, 0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn zero_then_timeout() {
        let mut c = Cal::default();
        assert_eq!(c.due(), None);
        c.start_zero();
        assert_eq!(c.due(), Some(ZERO_REQUEST));
        assert_eq!(c.due(), None, "taken");
        c.unsent();
        assert_eq!(c.due(), None, "not before the retry gap");
        c.tick(RETRY_MS);
        assert_eq!(c.due(), Some(ZERO_REQUEST), "sent again");
        let r = |offset| Reply::Zero { ok: true, offset, auto: Some(AutoZero::Off) };
        assert!(c.heard(r(10)));
        assert_eq!(c.phase(), Phase::Done(10, None));
        assert!(c.tick(SHOW_MS));
        c.start_zero();
        c.heard(r(12));
        assert_eq!(c.phase(), Phase::Done(12, Some(10)));
        c.reset();
        c.start_zero();
        assert!(!c.tick(TIMEOUT_MS - 1));
        assert!(c.tick(1));
        assert_eq!(c.phase(), Phase::NoAnswer);
        c.unsent();
        assert_eq!(c.due(), None, "nothing is sent after the end");
    }

    #[test]
    fn auto_zero_and_unasked_answers() {
        let mut c = Cal::default();
        c.toggle_auto();
        assert_eq!(c.phase(), Phase::Ready, "unknown support: nothing");
        assert!(!c.heard(Reply::Auto(AutoZero::Unsupported)));
        c.toggle_auto();
        assert_eq!(c.phase(), Phase::Ready, "not supported");
        c.heard(Reply::Auto(AutoZero::Off));
        c.toggle_auto();
        assert_eq!(c.due(), Some(auto_request(true)));
        assert!(c.heard(Reply::Auto(AutoZero::On)));
        assert_eq!((c.phase(), c.auto()), (Phase::AutoSet, Some(AutoZero::On)));
        c.reset();
        c.start_zero();
        assert!(!c.heard(Reply::Auto(AutoZero::On)), "an auto zero answer is not the zero's");
        assert!(c.heard(Reply::Zero { ok: false, offset: 0, auto: None }));
        assert_eq!(c.phase(), Phase::Failed);
    }
}
