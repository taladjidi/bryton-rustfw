//! The touch panel (BOARD.md 17): a Sitronix ST1xxx controller at I2C1 address 0x55, and the gestures recognised
//! from its points in software (the chip reports points only).
//!
//! Sources:
//! - Sitronix "Touch IC Protocol A" v2.7
//!   (<https://www.crystalfontz.com/products/document/4590/Sitronix_Touch_IC_Protocol_A_V2.7.pdf>): the register map
//!   (version 0x00, status 0x01 with its device status and error codes, resolution 0x04..0x06, firmware revision
//!   0x0c..0x0f, the points from 0x12 in 4-byte slots, max contacts 0x3f), the "Init" status to wait out.
//! - Linux `drivers/input/touchscreen/st1232.c`: the point and resolution decoding, the ready wait (status 0x00
//!   normal or 0x04 idle, no error).
//! - Stock (Ghidra, `Touch_Init` 0xd0243e08, `Ui_OnTouchReleaseDetectSwipe` 0xd017c894): the start (reset released,
//!   100 ms) and the swipe thresholds.

use crate::Board;

/// Status: device status in the low nibble, error code in the high one.
pub const REG_STATUS: u8 = 0x01;
/// The first point, 3 bytes (of a 4-byte slot), decoded by [`point`].
pub const REG_POINT: u8 = 0x12;

/// The status says the chip is ready: Normal (0) or Idle (4), no error (st1232.c).
///
/// ```
/// use bryton_rustfw::touch::ready;
/// assert!(ready(0x00) && ready(0x04));
/// assert!(!ready(0x01), "Init");
/// assert!(!ready(0x10), "error 1: invalid address");
/// ```
pub fn ready(status: u8) -> bool {
    status == 0x00 || status == 0x04
}

/// X and Y from the protocol's 3-byte layout: bits 6..4 of byte 0 the top of X, bits 2..0 the top of Y.
fn xy(b: [u8; 3]) -> (u16, u16) {
    (u16::from(b[0] & 0x70) << 4 | u16::from(b[1]), u16::from(b[0] & 0x07) << 8 | u16::from(b[2]))
}

/// A point slot's 3 bytes: the point if its valid bit (byte 0 bit 7) is set.
///
/// ```
/// use bryton_rustfw::touch::point;
/// assert_eq!(point([0x81, 0x2c, 0x05]), Some((44, 261)));
/// assert_eq!(point([0x80 | 0x10, 0x00, 0x40]), Some((239, 64)), "past the panel's edge: kept on it");
/// assert_eq!(point([0x11, 0x2c, 0x05]), None, "not valid: no finger");
/// ```
pub fn point(b: [u8; 3]) -> Option<(u16, u16)> {
    // the chip's 11 bits reach past the 240 x 320 panel: a point is clamped onto it, so no box test ever sees one off
    (b[0] & 0x80 != 0).then(|| xy(b)).map(|(x, y)| (x.min(239), y.min(319)))
}

/// The first two point slots, 7 bytes from [`REG_POINT`] (4-byte slots, the second at 0x16: Sitronix protocol A), as
/// points; a lone second finger is given as the first. Stock reads the same two slots; two fingers on the unit are
/// **inferred, not seen** (BOARD.md 17: 5 contacts reported by register 0x3f).
///
/// ```
/// use bryton_rustfw::touch::points;
/// let two = [0x81, 0x2c, 0x05, 0, 0x80, 0x10, 0x20];
/// assert_eq!(points(two), [Some((44, 261)), Some((16, 32))]);
/// assert_eq!(points([0, 0, 0, 0, 0x80, 0x10, 0x20]), [Some((16, 32)), None], "the first lifted");
/// ```
pub fn points(b: [u8; 7]) -> [Option<(u16, u16)>; 2] {
    let (p, q) = (point([b[0], b[1], b[2]]), point([b[4], b[5], b[6]]));
    if p.is_some() { [p, q] } else { [q, None] }
}

/// Releases the chip from reset (a low pulse first: a RAM run finds it as the last image left it) and waits for it to
/// leave Init, 150 ms at most (the protocol doc gives about 50 ms; stock waits 100). False if it never got ready.
pub fn init(b: &mut impl Board) -> bool {
    b.touch_reset(false);
    b.delay_ms(5); // ST1633 datasheet: RESET low at least 5 ms at power-on
    b.touch_reset(true);
    b.delay_ms(50);
    for _ in 0..10 {
        let mut s = [0xff];
        if b.touch_read(REG_STATUS, &mut s) && ready(s[0]) {
            return true;
        }
        b.delay_ms(10);
    }
    false
}

/// A slide farther than this, in pixels on either axis, is a drag, not a tap or a hold (stock: 10).
pub const DRAG_PX: i32 = 10;
/// A finger still for this long is a hold (stock: 600 ms).
pub const HOLD_MS: u32 = 600;
/// A drag this far right or left is a swipe (stock: 37).
pub const SWIPE_X: i32 = 37;
/// This far up or down (stock: 48 up; down chosen the same).
pub const SWIPE_Y: i32 = 48;

/// Which way the finger went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Towards x = 0.
    Left,
    /// Towards larger x.
    Right,
    /// Towards y = 0 (the top of the screen).
    Up,
    /// Towards larger y.
    Down,
}

/// What a touch meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// A short touch, at its point, given at the release.
    Tap(u16, u16),
    /// A finger kept still for [`HOLD_MS`], at its point, given while it is still down (nothing follows at the
    /// release).
    Hold(u16, u16),
    /// A drag of at least [`SWIPE_X`] or [`SWIPE_Y`], given at the release.
    Swipe(Dir),
    /// A drag that pans the zoomed map: its start and its vector. Not made here (the release of a drag is a
    /// [`Gesture::Swipe`] or nothing): the app makes it from [`Recogniser::take_drag`] where a drag pans.
    Drag(u16, u16, i16, i16),
    /// Two fingers moved apart (true) or together (false) by [`PINCH_RATIO`] since they landed or since the last
    /// pinch, given while they are still down (the release makes nothing).
    Pinch(bool),
}

/// A pinch is a change of the fingers' distance by this ratio, as 3 : 2 (1.5 times).
pub const PINCH_RATIO: (u32, u32) = (3, 2);
/// Two fingers closer than this, pixels, are not a pinch (one broad fingertip, or a glitch).
pub const PINCH_MIN_PX: u32 = 30;
/// The second finger must be there this many samples in a row before two fingers act: one sample of it (a palm, a
/// glitch) never drops a tap, a swipe or a drag.
pub const PINCH_SAMPLES: u8 = 2;

/// A finger on the panel.
#[derive(Clone, Copy)]
struct Finger {
    start: (u16, u16),
    last: (u16, u16),
    ms: u32,
    dragged: bool,
    held: bool,
}

/// Turns the points read at each poll into gestures.
#[derive(Default)]
pub struct Recogniser {
    finger: Option<Finger>,
    /// The last drag released: its start and its vector (the zoomed map pans by it).
    drag: Option<((u16, u16), (i16, i16))>,
    /// Two fingers are down: the squared distance they are compared with (from landing or the last pinch).
    span: Option<u32>,
    /// Consecutive samples with a second finger: it counts from [`PINCH_SAMPLES`].
    second: u8,
}

impl Recogniser {
    /// A finger was down at the last sample: keep reading until it is lifted (the release makes the gesture).
    pub fn down(&self) -> bool {
        self.finger.is_some()
    }

    /// The drag just released (its start point and vector, pixels), whatever gesture it made (a swipe or none),
    /// once; a new touch drops it.
    pub fn take_drag(&mut self) -> Option<((u16, u16), (i16, i16))> {
        self.drag.take()
    }

    /// [`Recogniser::feed`] with the second finger `q` too. With two fingers down the first finger's tap, hold,
    /// swipe and drag are dropped (it is a pinch), until both are lifted, and the distance between them gives
    /// [`Gesture::Pinch`] each time it changes by [`PINCH_RATIO`] from where it was.
    pub fn feed2(&mut self, p: Option<(u16, u16)>, q: Option<(u16, u16)>, dt_ms: u32) -> Option<Gesture> {
        let (Some(p), Some(q)) = (p, q.filter(|_| p.is_some())) else {
            (self.span, self.second) = (None, 0);
            return self.feed(p, dt_ms);
        };
        self.second = self.second.saturating_add(1);
        if self.second < PINCH_SAMPLES {
            return self.feed(Some(p), dt_ms); // not yet two fingers: the first one's gestures stay whole
        }
        self.feed(Some(p), dt_ms);
        if let Some(f) = &mut self.finger {
            (f.dragged, f.held) = (false, true); // no tap or swipe at the release, no pan
        }
        let (dx, dy) = (i64::from(p.0) - i64::from(q.0), i64::from(p.1) - i64::from(q.1));
        let d2 = (dx * dx + dy * dy) as u32; // panel pixels: far below 2^32
        let base = *self.span.get_or_insert(d2);
        let (n, m) = (u64::from(PINCH_RATIO.0), u64::from(PINCH_RATIO.1));
        let (d, b) = (u64::from(d2), u64::from(base));
        if b < u64::from(PINCH_MIN_PX * PINCH_MIN_PX) {
            self.span = Some(d2); // too close to start from: the base follows until they part
            return None;
        }
        let out = d * m * m >= b * n * n;
        if out || d * n * n <= b * m * m {
            self.span = Some(d2);
            return Some(Gesture::Pinch(out));
        }
        None
    }

    /// One sample, `dt_ms` after the previous one: the finger's point, or None when no finger is down.
    pub fn feed(&mut self, p: Option<(u16, u16)>, dt_ms: u32) -> Option<Gesture> {
        let (Some(f), p) = (&mut self.finger, p) else {
            if p.is_some() {
                self.drag = None;
            }
            self.finger = p.map(|p| Finger { start: p, last: p, ms: 0, dragged: false, held: false });
            return None;
        };
        let Some(p) = p else {
            let f = self.finger.take()?;
            let (dx, dy) = (i32::from(f.last.0) - i32::from(f.start.0), i32::from(f.last.1) - i32::from(f.start.1));
            if f.dragged && !f.held {
                self.drag = Some((f.start, (dx as i16, dy as i16))); // points are panel pixels: they fit
            }
            return match () {
                _ if f.held => None,
                _ if !f.dragged => Some(Gesture::Tap(f.last.0, f.last.1)),
                _ if dy <= -SWIPE_Y => Some(Gesture::Swipe(Dir::Up)),
                _ if dy >= SWIPE_Y => Some(Gesture::Swipe(Dir::Down)),
                _ if dx >= SWIPE_X => Some(Gesture::Swipe(Dir::Right)),
                _ if dx <= -SWIPE_X => Some(Gesture::Swipe(Dir::Left)),
                _ => None, // a short drag: neither
            };
        };
        f.last = p;
        f.ms = f.ms.saturating_add(dt_ms);
        let (dx, dy) = (i32::from(p.0) - i32::from(f.start.0), i32::from(p.1) - i32::from(f.start.1));
        f.dragged |= dx.abs() > DRAG_PX || dy.abs() > DRAG_PX;
        if !f.dragged && !f.held && f.ms >= HOLD_MS {
            f.held = true;
            return Some(Gesture::Hold(f.start.0, f.start.1));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// Feeds `points` one per 50 ms, then a release; every gesture given.
    fn run(points: &[(u16, u16)]) -> Vec<Gesture> {
        let mut r = Recogniser::default();
        let mut out: Vec<_> = points.iter().filter_map(|&p| r.feed(Some(p), 50)).collect();
        out.extend(r.feed(None, 50));
        assert!(!r.down());
        out
    }

    #[test]
    fn taps_holds_and_swipes() {
        assert_eq!(run(&[(100, 100), (103, 98)]), [Gesture::Tap(103, 98)]);
        let still = [(100, 100); 20]; // 950 ms
        assert_eq!(run(&still), [Gesture::Hold(100, 100)], "once, while held; nothing at the release");
        let swipe = |dx: i32, dy: i32| {
            let pts: Vec<_> = (0..=4).map(|i| ((120 + dx * i / 4) as u16, (160 + dy * i / 4) as u16)).collect();
            run(&pts)
        };
        assert_eq!(swipe(-60, 5), [Gesture::Swipe(Dir::Left)]);
        assert_eq!(swipe(60, -5), [Gesture::Swipe(Dir::Right)]);
        assert_eq!(swipe(10, -80), [Gesture::Swipe(Dir::Up)]);
        assert_eq!(swipe(-10, 80), [Gesture::Swipe(Dir::Down)]);
        assert_eq!(swipe(25, 0), [], "a drag too short to be a swipe");
    }

    /// Two fingers: apart by 1.5 times is a pinch out, together a pinch in, each from the last; the first finger's
    /// tap or swipe is dropped; too close to start gives nothing; one finger lifted leaves no tap.
    #[test]
    fn pinch_by_distance_and_no_tap_after() {
        let mut r = Recogniser::default();
        let at = |r: &mut Recogniser, d: u16| r.feed2(Some((100, 100)), Some((100 + d, 100)), 50);
        assert_eq!(at(&mut r, 60), None, "the second finger's first sample");
        assert_eq!(at(&mut r, 60), None, "landed");
        assert_eq!(at(&mut r, 80), None, "1.33 times");
        assert_eq!(at(&mut r, 90), Some(Gesture::Pinch(true)), "1.5 times");
        assert_eq!(at(&mut r, 100), None, "from the last pinch, 1.1 times");
        assert_eq!(at(&mut r, 60), Some(Gesture::Pinch(false)), "two thirds of 90");
        assert_eq!(r.feed2(Some((100, 100)), None, 50), None, "one finger left");
        assert_eq!(r.feed2(None, None, 50), None, "no tap, no swipe");
        assert_eq!(r.take_drag(), None);
        let mut r = Recogniser::default();
        assert_eq!(at(&mut r, 10), None);
        assert_eq!(at(&mut r, 10), None);
        assert_eq!(at(&mut r, 40), None, "from 10 px apart the base follows");
        assert_eq!(at(&mut r, 20), Some(Gesture::Pinch(false)), "halved from 40");
    }

    /// A one-sample blip of a second finger never drops a tap, a swipe or a drag.
    #[test]
    fn a_second_finger_blip_changes_nothing() {
        let blip = |points: &[(u16, u16)], at: usize| {
            let mut r = Recogniser::default();
            let mut out: Vec<_> = points
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| r.feed2(Some(p), (i == at).then_some((p.0 + 80, p.1)), 50))
                .collect();
            out.extend(r.feed2(None, None, 50));
            (out, r.take_drag())
        };
        assert_eq!(blip(&[(100, 100), (103, 98), (101, 99)], 1).0, [Gesture::Tap(101, 99)], "a tap still taps");
        let swipe: Vec<_> = (0..=4).map(|i| (120 - 15 * i, 160)).collect();
        assert_eq!(blip(&swipe, 2).0, [Gesture::Swipe(Dir::Left)], "a swipe still swipes");
        let drag: Vec<_> = (0..=2).map(|i| (100 + 12 * i, 100 + 5 * i)).collect();
        assert_eq!(blip(&drag, 1).1, Some(((100, 100), (24, 10))), "a drag still pans");
    }

    /// The drag released is kept once, whether it was a swipe or too short for one; a tap or a hold leaves none.
    #[test]
    fn drag_kept_at_the_release() {
        let mut r = Recogniser::default();
        for p in [(100, 100), (110, 104), (125, 112)] {
            r.feed(Some(p), 50);
        }
        assert_eq!(r.feed(None, 50), None, "too short for a swipe");
        assert_eq!(r.take_drag(), Some(((100, 100), (25, 12))));
        assert_eq!(r.take_drag(), None, "once");
        for p in [(100, 100), (40, 230)] {
            r.feed(Some(p), 50);
        }
        assert_eq!(r.feed(None, 50), Some(Gesture::Swipe(Dir::Down)));
        r.feed(Some((10, 10)), 50);
        assert_eq!(r.take_drag(), None, "a new touch drops it");
        assert_eq!(r.feed(None, 50), Some(Gesture::Tap(10, 10)));
        assert_eq!(r.take_drag(), None, "a tap is no drag");
    }
}
