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
    /// Start the calibration, or cancel the one running.
    Calibrate,
    /// Start the FTP ramp test.
    Ramp,
}

/// The bottom row of buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bottom {
    /// MODE, the whole width.
    #[default]
    Mode,
    /// MODE and CALIBRATE side by side (the trainer asks for a calibration, or a result is shown).
    Both,
    /// MODE and RAMP TEST side by side (no ride yet, nothing to calibrate).
    ModeRamp,
    /// CANCEL, the whole width (a calibration runs).
    Cancel,
}

// The text styles are 20 px (LABEL) and 40 px (VALUE) high: each box under the one above.
const TITLE_Y: u32 = PAGE_Y + 4;
/// Left edge of the timer's box in the title line: past the title "TRAINER" (it ends at 162).
const TIMER_X: u32 = 168;
const MODE_Y: u32 = TITLE_Y + 22;
const SOURCE_Y: u32 = MODE_Y + 42;
const TARGET_Y: u32 = SOURCE_Y + 22;
const NOW_Y: u32 = TARGET_Y + 44;
const STATUS_Y: u32 = NOW_Y + 24;
const Y1: u32 = STATUS_Y + 26;
const BOTTOM_Y: u32 = Y1 + 62;
const BOTTOM_H: u32 = 48;
/// The target buttons as (button, x, y, w, h).
pub const BUTTONS: [(Button, u32, u32, u32, u32); 2] =
    [(Button::Minus, 8, Y1, 108, 56), (Button::Plus, 124, Y1, 108, 56)];

type BottomButton = (Button, u32, u32, &'static str, u16, u16);

/// The bottom row's buttons: (button, x, w, label, background, text colour).
fn bottom_buttons(b: Bottom) -> [Option<BottomButton>; 2] {
    let mode = (Button::Mode, 8, 224, "MODE", ACCENT, super::ON_ACCENT);
    match b {
        Bottom::Mode => [Some(mode), None],
        Bottom::Both => [
            Some((Button::Mode, 8, 104, "MODE", ACCENT, super::ON_ACCENT)),
            Some((Button::Calibrate, 116, 116, "CALIBRATE", YELLOW, super::ON_ACCENT)),
        ],
        Bottom::ModeRamp => [
            Some((Button::Mode, 8, 104, "MODE", ACCENT, super::ON_ACCENT)),
            Some((Button::Ramp, 116, 116, "RAMP TEST", GREEN, super::ON_ACCENT)),
        ],
        Bottom::Cancel => [Some((Button::Calibrate, 8, 224, "CANCEL", RED, super::ON_ALARM)), None],
    }
}

/// The boxes of the two readings: each value right-aligned in a box of its own, its unit drawn once beside it (the
/// screen's left half power, the right half cadence): `(box x, box width, unit x, unit)`.
fn reading_boxes() -> [(u32, u32, u32, &'static str); 2] {
    let half = (W - 16) / 2;
    let one = |end: u32, widest: &str, unit: &'static str| {
        let unit_x = end - LABEL.width(unit);
        let (bw, right) = (LABEL.width(widest), unit_x - LABEL.width(" "));
        (right - bw, bw, unit_x, unit)
    };
    [one(8 + half - 4, "8888", "W"), one(8 + 2 * half - 4, "888", "RPM")]
}

/// The button under a touch at (`x`, `y`), if any.
pub fn hit(x: u16, y: u16, bottom: Bottom) -> Option<Button> {
    let row = bottom_buttons(bottom).into_iter().flatten().map(|(b, bx, w, ..)| (b, bx, BOTTOM_Y, w, BOTTOM_H));
    BUTTONS.iter().copied().chain(row).find(|&(_, bx, by, w, h)| super::Rect(bx, by, w, h).contains(x, y)).map(|b| b.0)
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
    /// The power and cadence shown (the second's snapshot, [`crate::sensors::Live::shown`]); None: not heard.
    pub power: Option<u16>,
    /// See `power`.
    pub cadence: Option<u8>,
    /// A line for the rider ("NO TRAINER", "CALIBRATE", "SPEED TOO LOW"...), and its colour.
    pub status: &'a str,
    /// See `status`.
    pub status_color: u16,
    /// The bottom row.
    pub bottom: Bottom,
    /// The ride's timer ("12:34", "1:05" from an hour) in the title line while a ride runs, else empty.
    pub timer: &'a str,
}

/// What is on screen: one fingerprint per box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown([u32; 8]);

fn fnv(h: u32, s: &str) -> u32 {
    s.bytes().chain([0]).fold(h, |h, x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// The whole screen: the title, the buttons, then every box.
pub fn draw(c: &mut impl Canvas, now: &Now) -> Shown {
    super::draw_title(c, "TRAINER");
    for &(b, x, y, w, h) in &BUTTONS {
        c.fill(x, y, w, h, DARK);
        let st = Style { fg: WHITE, bg: DARK, ..VALUE };
        let label = if b == Button::Minus { "-" } else { "+" };
        text::draw(c, &st, x + w / 2, y + (h - st.height()) / 2, Align::Centre, label);
    }
    for (_, _, x, unit) in reading_boxes() {
        text::draw(c, &Style { fg: GREY, ..LABEL }, x, NOW_Y, Align::Left, unit);
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
    // power and cadence: the value alone in a box of its own, the unit beside it drawn once
    let (mut power, mut cadence) = (Line::<12>::new(), Line::<12>::new());
    match now.power {
        Some(p) => power.push_u32(u32::from(p)),
        None => power.push_str("--"),
    }
    match now.cadence {
        Some(r) => cadence.push_u32(u32::from(r)),
        None => cadence.push_str("--"),
    }
    let prints = [
        fnv(1, now.mode),
        fnv(2, now.source),
        fnv(3, now.target),
        fnv(4, power.as_str()),
        fnv(5, cadence.as_str()),
        fnv(u32::from(now.status_color) + 6, now.status),
        fnv(7, ["MODE", "BOTH", "RAMP", "CANCEL"][now.bottom as usize]),
        fnv(8, now.timer),
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
            3 | 4 => {
                let (x, bw, ..) = reading_boxes()[i - 3];
                text::draw_in(
                    c,
                    &LABEL,
                    x,
                    NOW_Y,
                    bw,
                    Align::Right,
                    if i == 3 { power.as_str() } else { cadence.as_str() },
                );
            }
            5 => {
                text::draw_in(c, &Style { fg: now.status_color, ..LABEL }, 8, STATUS_Y, w, Align::Centre, now.status);
            }
            7 => {
                // right of the centred title: five characters ("59:59"), which is also "1:05" and "12:05"
                text::draw_in(c, &LABEL, TIMER_X, TITLE_Y, W - 8 - TIMER_X, Align::Right, now.timer);
            }
            _ => {
                // the whole row: it changes only when the buttons do
                c.fill(8, BOTTOM_Y, w, BOTTOM_H, super::BLACK);
                for (_, x, bw, label, bg, fg) in bottom_buttons(now.bottom).into_iter().flatten() {
                    c.fill(x, BOTTOM_Y, bw, BOTTOM_H, bg);
                    let big = VALUE.width(label) + 8 <= bw;
                    let st = Style { fg, bg, ..if big { VALUE } else { LABEL } };
                    // centre on the ink (7 font rows of the 10 the box has), but keep the whole box inside the button
                    let y = BOTTOM_Y + ((BOTTOM_H - 7 * st.scale) / 2).min(BOTTOM_H - st.height());
                    text::draw(c, &st, x + bw / 2, y, Align::Centre, label);
                }
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

/// The middle of `button` in the bottom row `bottom`, for the tests' taps.
#[cfg(test)]
pub fn bottom_centre(bottom: Bottom, button: Button) -> Option<(u16, u16)> {
    let (_, x, w, ..) = bottom_buttons(bottom).into_iter().flatten().find(|b| b.0 == button)?;
    Some(((x + w / 2) as u16, (BOTTOM_Y + BOTTOM_H / 2) as u16))
}
