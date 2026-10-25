//! The trainer screen: the mode and the target big, what the trainer reports under them, the buttons that change the
//! target (touch; the keys are Page up, Lap down, Start the mode). Pure drawing against [`Canvas`], each value in its
//! own fixed box so a second's update redraws only the boxes whose text changed.

use super::{ACCENT, Canvas, DARK, GREEN, GREY, LABEL, PAGE_Y, RED, VALUE, W, WHITE, YELLOW};
use crate::text::{self, Align, Line, Style};

/// A button of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The target down.
    Minus,
    /// The target up.
    Plus,
    /// The next mode.
    Mode,
}

// The text styles are 20 px (LABEL) and 40 px (VALUE) high: each box under the one above.
const TITLE_Y: u32 = PAGE_Y + 4;
const MODE_Y: u32 = TITLE_Y + 22;
const SOURCE_Y: u32 = MODE_Y + 42;
const TARGET_Y: u32 = SOURCE_Y + 22;
const NOW_Y: u32 = TARGET_Y + 44;
const STATUS_Y: u32 = NOW_Y + 24;
/// The buttons as (button, x, y, w, h).
pub const BUTTONS: [(Button, u32, u32, u32, u32); 3] = {
    const Y1: u32 = STATUS_Y + 26;
    [(Button::Minus, 8, Y1, 108, 56), (Button::Plus, 124, Y1, 108, 56), (Button::Mode, 8, Y1 + 62, 224, 40)]
};

/// The button under a touch at (`x`, `y`), if any.
pub fn hit(x: u16, y: u16) -> Option<Button> {
    let (x, y) = (u32::from(x), u32::from(y));
    BUTTONS.iter().find(|&&(_, bx, by, w, h)| x >= bx && x < bx + w && y >= by && y < by + h).map(|b| b.0)
}

/// What to show.
#[derive(Clone, Copy, Debug, Default)]
pub struct Now<'a> {
    /// The mode in force ("ERG", "RES", "SIM"), or none yet.
    pub mode: &'a str,
    /// What drives it: "WORKOUT", "COURSE" or nothing (by hand).
    pub source: &'a str,
    /// The target with its unit ("250 W", "40 %", "2.5 %").
    pub target: &'a str,
    /// The trainer's power and cadence; None: not heard.
    pub power: Option<u16>,
    /// See `power`.
    pub cadence: Option<u8>,
    /// A line for the rider ("NO TRAINER", "CALIBRATE", "SPEED TOO LOW"...), and its colour.
    pub status: &'a str,
    /// See `status`.
    pub status_color: u16,
}

/// What is on screen: one fingerprint per box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown([u32; 5]);

fn fnv(h: u32, s: &str) -> u32 {
    s.bytes().chain([0]).fold(h, |h, x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// The whole screen: the title, the buttons, then every box.
pub fn draw(c: &mut impl Canvas, now: &Now) -> Shown {
    text::draw_in(c, &LABEL, 8, TITLE_Y, W - 16, Align::Left, "TRAINER");
    for &(b, x, y, w, h) in &BUTTONS {
        c.fill(x, y, w, h, if b == Button::Mode { ACCENT } else { DARK });
        let (label, fg) = match b {
            Button::Minus => ("-", WHITE),
            Button::Plus => ("+", WHITE),
            Button::Mode => ("MODE", super::BLACK),
        };
        let st = Style { fg, bg: if b == Button::Mode { ACCENT } else { DARK }, ..VALUE };
        text::draw(c, &st, x + w / 2, y + (h - st.height()) / 2, Align::Centre, label);
    }
    let mut shown = Shown::default();
    update(c, now, &mut shown);
    shown
}

/// What [`draw`] leaves on screen, without drawing (a screenshot row).
pub fn shown(now: &Now) -> Shown {
    struct Nothing;
    impl Canvas for Nothing {
        fn fill(&mut self, _: u32, _: u32, _: u32, _: u32, _: u16) {}
        fn blit(&mut self, _: u32, _: u32, _: u32, _: u32, _: &mut dyn FnMut(u32, u32) -> u16) {}
    }
    let mut s = Shown::default();
    update(&mut Nothing, now, &mut s);
    s
}

/// Redraws the boxes whose text changed since `shown` (all of them when it is the default: a fingerprint is never 0).
pub fn update(c: &mut impl Canvas, now: &Now, shown: &mut Shown) {
    let mut reading = Line::<24>::new();
    match now.power {
        Some(p) => {
            reading.push_u32(u32::from(p));
            reading.push_str(" W");
        }
        None => reading.push_str("-- W"),
    }
    reading.push_str("  ");
    match now.cadence {
        Some(r) => {
            reading.push_u32(u32::from(r));
            reading.push_str(" RPM");
        }
        None => reading.push_str("-- RPM"),
    }
    let prints = [
        fnv(1, now.mode),
        fnv(2, now.source),
        fnv(3, now.target),
        fnv(4, reading.as_str()),
        fnv(u32::from(now.status_color) + 5, now.status),
    ];
    let bold = Style { fg: ACCENT, ..VALUE };
    let grey = Style { fg: GREY, ..LABEL };
    let w = W - 16;
    for (i, (&p, old)) in prints.iter().zip(&mut shown.0).enumerate() {
        if p == *old {
            continue;
        }
        *old = p;
        match i {
            0 => {
                text::draw_in(c, &bold, 8, MODE_Y, w, Align::Centre, now.mode);
            }
            1 => {
                text::draw_in(c, &grey, 8, SOURCE_Y, w, Align::Centre, now.source);
            }
            2 => {
                text::draw_in(c, &VALUE, 8, TARGET_Y, w, Align::Centre, now.target);
            }
            3 => {
                text::draw_in(c, &LABEL, 8, NOW_Y, w, Align::Centre, reading.as_str());
            }
            _ => {
                text::draw_in(c, &Style { fg: now.status_color, ..LABEL }, 8, STATUS_Y, w, Align::Centre, now.status);
            }
        }
    }
}

/// The status line's colours: a problem, a warning, all well.
pub const BAD: u16 = RED;
/// See [`BAD`].
pub const WARN: u16 = YELLOW;
/// See [`BAD`].
pub const GOOD: u16 = GREEN;
