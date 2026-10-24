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

/// Firmware version (1 byte).
pub const REG_VERSION: u8 = 0x00;
/// Status: device status in the low nibble, error code in the high one.
pub const REG_STATUS: u8 = 0x01;
/// Resolution, 3 bytes, decoded by [`resolution`].
pub const REG_RESOLUTION: u8 = 0x04;
/// Firmware revision, 4 bytes (FW_Rev_3 first).
pub const REG_REVISION: u8 = 0x0c;
/// The first point, 3 bytes (of a 4-byte slot), decoded by [`point`].
pub const REG_POINT: u8 = 0x12;
/// Max number of contacts supported (1 byte).
pub const REG_MAX_CONTACTS: u8 = 0x3f;

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

/// The resolution (width, height) from register 0x04's 3 bytes.
///
/// ```
/// assert_eq!(bryton_rustfw::touch::resolution([0x01, 0xf0, 0x40]), (240, 320));
/// ```
pub fn resolution(b: [u8; 3]) -> (u16, u16) {
    xy(b)
}

/// A point slot's 3 bytes: the point if its valid bit (byte 0 bit 7) is set.
///
/// ```
/// use bryton_rustfw::touch::point;
/// assert_eq!(point([0x81, 0x2c, 0x05]), Some((44, 261)));
/// assert_eq!(point([0x80 | 0x10, 0x00, 0x40]), Some((256, 64)));
/// assert_eq!(point([0x11, 0x2c, 0x05]), None, "not valid: no finger");
/// ```
pub fn point(b: [u8; 3]) -> Option<(u16, u16)> {
    (b[0] & 0x80 != 0).then(|| xy(b))
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
}

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
    delta: (i16, i16),
}

impl Recogniser {
    /// A finger was down at the last sample: keep reading until it is lifted (the release makes the gesture).
    pub fn down(&self) -> bool {
        self.finger.is_some()
    }

    /// How far the finger moved since the previous sample (for scrolling a list by hand).
    pub fn delta(&self) -> (i16, i16) {
        self.delta
    }

    /// One sample, `dt_ms` after the previous one: the finger's point, or None when no finger is down.
    pub fn feed(&mut self, p: Option<(u16, u16)>, dt_ms: u32) -> Option<Gesture> {
        let (Some(f), p) = (&mut self.finger, p) else {
            self.delta = (0, 0);
            self.finger = p.map(|p| Finger { start: p, last: p, ms: 0, dragged: false, held: false });
            return None;
        };
        let Some(p) = p else {
            let f = self.finger.take()?;
            self.delta = (0, 0);
            let (dx, dy) = (i32::from(f.last.0) - i32::from(f.start.0), i32::from(f.last.1) - i32::from(f.start.1));
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
        self.delta = ((i32::from(p.0) - i32::from(f.last.0)) as i16, (i32::from(p.1) - i32::from(f.last.1)) as i16);
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

    #[test]
    fn delta_follows_the_finger() {
        let mut r = Recogniser::default();
        r.feed(Some((50, 200)), 50);
        r.feed(Some((50, 190)), 50);
        assert_eq!(r.delta(), (0, -10));
        r.feed(None, 50);
        assert_eq!(r.delta(), (0, 0));
    }
}
