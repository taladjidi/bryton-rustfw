//! What is on screen: colours, text styles, Ferris, the layout and the clock. Draws
//! through [`Canvas`] only, so it knows nothing about the panel protocol (panel.rs) or the board.

use crate::fonts::PIXEL;
use crate::text::{self, Align, Style};

/// Panel width in pixels (portrait).
pub const W: u32 = 240;
/// Panel height in pixels.
pub const H: u32 = 320;

/// RGB565 black.
pub const BLACK: u16 = 0x0000;
/// RGB565 white.
pub const WHITE: u16 = 0xffff;
/// RGB565 orange, for the seconds and the top bar.
pub const ACCENT: u16 = 0xfd20;
/// Ferris orange, #f74c00 in RGB565.
pub const RUST: u16 = 0xf260;
/// RGB565 red.
pub const RED: u16 = 0xf800;
/// RGB565 green.
pub const GREEN: u16 = 0x07e0;

/// Small text: labels. The pixel font at 2x, so its dots match Ferris' look (fonts.rs also has smooth fonts).
pub const LABEL: Style = Style { font: &PIXEL, scale: 2, fg: WHITE, bg: BLACK };
/// Large text: values, the pixel font at 4x like Ferris.
pub const VALUE: Style = Style { font: &PIXEL, scale: 4, fg: WHITE, bg: BLACK };

/// Something that can fill a rectangle with RGB565 pixels: the panel on the unit, a framebuffer in tests.
pub trait Canvas {
    /// Fills `w` x `h` pixels from (`x`, `y`) with one colour. Callers keep the rectangle on screen.
    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16);
    /// Fills `w` x `h` pixels from (`x`, `y`) with `px(dx, dy)`, `dx` / `dy` relative to (`x`, `y`). Called in row
    /// order on the panel (one window, one RAMWR), so `px` should be cheap. Callers keep the rectangle on screen.
    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16);
}

// ---------------------------------------------------------------------------------------------------------------
// Ferris
// ---------------------------------------------------------------------------------------------------------------

/// Ferris the crab, 16 x 11, bit 15 = leftmost pixel; the gaps in row 6 are the eyes.
pub const CRAB: [u16; 11] = [0x6006, 0x9009, 0xa005, 0x6006, 0x2004, 0x3ffc, 0x77ee, 0xffff, 0x7ffe, 0xa815, 0x9009];

/// Draws [`CRAB`] at (`x`, `y`), each bitmap pixel `scale` x `scale`, one fill per horizontal run.
pub fn draw_crab(c: &mut impl Canvas, x: u32, y: u32, scale: u32, color: u16) {
    for (r, &bits) in CRAB.iter().enumerate() {
        let mut col = 0;
        while col < 16 {
            let run = (bits << col).leading_ones();
            if run > 0 {
                c.fill(x + col * scale, y + r as u32 * scale, run * scale, scale, color);
                col += run;
            } else {
                col += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Screen layout
// ---------------------------------------------------------------------------------------------------------------

/// Hours and minutes: the pixel font at 7x, white.
pub const TIME: Style = Style { font: &PIXEL, scale: 7, fg: WHITE, bg: BLACK };
/// Seconds: the pixel font at 5x, orange.
pub const SECONDS: Style = Style { font: &PIXEL, scale: 5, fg: ACCENT, bg: BLACK };
/// Width of one character cell of the monospaced pixel font, in font pixels.
const CELL: u32 = 6;
/// Left edge of "HH:MM", its ink centred (each cell ends with a blank column, the last one does not count).
pub const TIME_X: u32 = (W - (5 * CELL - 1) * TIME.scale) / 2;
/// Top of the hours and minutes.
pub const TIME_Y: u32 = 96;
/// Left edge of the two seconds digits, their ink centred.
pub const SECONDS_X: u32 = (W - (2 * CELL - 1) * SECONDS.scale) / 2;
/// Top of the seconds.
pub const SECONDS_Y: u32 = 196;
/// Ferris: scale and top left corner (centred, under the seconds).
pub const CRAB_SCALE: u32 = 4;
/// Ferris left edge.
pub const CRAB_X: u32 = (W - 16 * CRAB_SCALE) / 2;
/// Ferris top edge.
pub const CRAB_Y: u32 = 266;

/// The parts of the screen that never change: the colon, the bar at the top, Ferris.
pub fn draw_static(c: &mut impl Canvas) {
    text::draw(c, &TIME, TIME_X + 2 * CELL * TIME.scale, TIME_Y, Align::Left, ":");
    c.fill(20, 40, W - 40, 6, ACCENT);
    draw_crab(c, CRAB_X, CRAB_Y, CRAB_SCALE, RUST);
}

/// USB dev link state, shown as a dot in the top right corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbDot {
    /// Not plugged in: no dot.
    Off,
    /// Plugged in, but the PHY or the core did not come up.
    Failed,
    /// Plugged in, link up, not yet configured by the computer.
    Powered,
    /// Configured: the dev link is usable.
    Ready,
}

/// The USB dot's box: (x, y, size).
pub const USB_DOT: (u32, u32, u32) = (W - 20, 12, 8);
/// The "USB" label: right edge and top, so its capitals (7 font rows at 2x) are centred on the dot.
pub const USB_LABEL: (u32, u32) = (USB_DOT.0 - 6, USB_DOT.1 + USB_DOT.2 / 2 - 7);

/// Draws the USB state dot and the "USB" label beside it, both in the state's colour (black when unplugged).
pub fn draw_usb_dot(c: &mut impl Canvas, s: UsbDot) {
    let color = match s {
        UsbDot::Off => BLACK,
        UsbDot::Failed => RED,
        UsbDot::Powered => ACCENT,
        UsbDot::Ready => GREEN,
    };
    c.fill(USB_DOT.0, USB_DOT.1, USB_DOT.2, USB_DOT.2, color);
    text::draw(c, &Style { fg: color, ..LABEL }, USB_LABEL.0, USB_LABEL.1, Align::Right, "USB");
}

/// One screen row as a [`Canvas`]: keeps only what falls on row `y`. Rendering the screen row by row takes
/// 480 bytes instead of a 150 KB framebuffer (screenshots over USB).
pub struct Row {
    /// The row drawn.
    pub y: u32,
    /// Its pixels, RGB565.
    pub px: [u16; W as usize],
}

impl Row {
    /// Row `y`, all black.
    pub fn new(y: u32) -> Self {
        Row { y, px: [BLACK; W as usize] }
    }
}

impl Canvas for Row {
    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
        self.blit(x, y, w, h, &mut |_, _| color);
    }

    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
        if y <= self.y && self.y < y + h {
            let end = (x + w).min(W) as usize;
            for (i, p) in self.px[(x as usize).min(end)..end].iter_mut().enumerate() {
                *p = px(i as u32, self.y - y);
            }
        }
    }
}

/// Clock digit `i` (0..4: H H M M, 4..6: the seconds) showing `d`, in its fixed cell of the monospaced font.
fn draw_clock_digit(c: &mut impl Canvas, i: usize, d: u8) {
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    let s = DIGITS[usize::from(d % 10)];
    if i < 4 {
        let col = if i < 2 { i } else { i + 1 } as u32; // the colon is column 2
        text::draw(c, &TIME, TIME_X + col * CELL * TIME.scale, TIME_Y, Align::Left, s);
    } else {
        text::draw(c, &SECONDS, SECONDS_X + (i as u32 - 4) * CELL * SECONDS.scale, SECONDS_Y, Align::Left, s);
    }
}

/// The time on screen; redraws only the digits that changed, since each glyph costs SPI time.
pub struct Clock {
    shown: [u8; 6],
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    /// A clock with nothing drawn yet: the first [`Clock::show`] draws every digit.
    pub const fn new() -> Self {
        Self { shown: [0xff; 6] }
    }

    /// Draws again every digit on screen (for screenshots, into a [`Row`]).
    pub fn redraw(&self, c: &mut impl Canvas) {
        for (i, &d) in self.shown.iter().enumerate() {
            if d != 0xff {
                draw_clock_digit(c, i, d);
            }
        }
    }

    /// Draws HH:MM in white and the seconds in orange below.
    pub fn show(&mut self, c: &mut impl Canvas, h: u8, m: u8, s: u8) {
        let digits = [h / 10, h % 10, m / 10, m % 10, s / 10, s % 10];
        for (i, &d) in digits.iter().enumerate() {
            if d != self.shown[i] {
                self.shown[i] = d;
                draw_clock_digit(c, i, d);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// A framebuffer that panics on any fill off screen.
    pub struct Frame {
        pub px: Vec<u16>,
        fills: usize,
    }

    impl Frame {
        pub fn new() -> Self {
            Frame { px: vec![BLACK; (W * H) as usize], fills: 0 }
        }
        pub fn at(&self, x: u32, y: u32) -> u16 {
            self.px[(y * W + x) as usize]
        }
        fn count(&self, color: u16) -> usize {
            self.px.iter().filter(|&&p| p == color).count()
        }
    }

    impl Canvas for Frame {
        fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
            self.blit(x, y, w, h, &mut |_, _| color);
        }

        fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
            assert!(x + w <= W && y + h <= H, "fill ({x},{y}) {w}x{h} off screen");
            self.fills += 1;
            for dy in 0..h {
                for dx in 0..w {
                    self.px[((y + dy) * W + x + dx) as usize] = px(dx, dy);
                }
            }
        }
    }

    #[test]
    fn crab_matches_bitmap() {
        let mut f = Frame::new();
        draw_crab(&mut f, 0, 0, 1, RUST);
        for (r, &bits) in CRAB.iter().enumerate() {
            for col in 0..16 {
                assert_eq!(f.at(col, r as u32) == RUST, bits & (0x8000 >> col) != 0, "row {r} col {col}");
            }
        }
        let mut f = Frame::new();
        draw_crab(&mut f, CRAB_X, CRAB_Y, CRAB_SCALE, RUST);
        let ones: u32 = CRAB.iter().map(|b| b.count_ones()).sum();
        assert_eq!(f.count(RUST) as u32, ones * CRAB_SCALE * CRAB_SCALE);
    }

    /// Every element in its own box: the clock cells, the bar, Ferris and the USB dot and label never
    /// overwrite each other, and everything stays on screen (Frame panics otherwise).
    #[test]
    fn layout_does_not_overlap() {
        let mut boxes = Vec::new();
        boxes.push((TIME_X, TIME_Y, TIME.width("88:88"), TIME.height()));
        boxes.push((SECONDS_X, SECONDS_Y, SECONDS.width("88"), SECONDS.height()));
        boxes.push((20, 40, W - 40, 6)); // bar
        boxes.push((CRAB_X, CRAB_Y, 16 * CRAB_SCALE, CRAB.len() as u32 * CRAB_SCALE));
        boxes.push((USB_DOT.0, USB_DOT.1, USB_DOT.2, USB_DOT.2));
        let usb = LABEL.width("USB");
        boxes.push((USB_LABEL.0 - usb, USB_LABEL.1, usb, LABEL.height()));
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.0 + a.2 <= W && a.1 + a.3 <= H, "box {i} off screen");
            for b in &boxes[i + 1..] {
                let apart = a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
        let mut f = Frame::new();
        draw_static(&mut f);
        Clock::new().show(&mut f, 88, 88, 88);
    }

    #[test]
    fn rows_match_the_framebuffer() {
        let mut f = Frame::new();
        draw_static(&mut f);
        draw_usb_dot(&mut f, UsbDot::Ready);
        for y in [0, USB_DOT.1, 43, CRAB_Y + 20, H - 1] {
            let mut row = Row::new(y);
            draw_static(&mut row);
            draw_usb_dot(&mut row, UsbDot::Ready);
            assert!(row.px[..] == f.px[(y * W) as usize..((y + 1) * W) as usize], "row {y}");
        }
    }

    #[test]
    fn clock_redraws_only_changes() {
        let mut f = Frame::new();
        let mut clock = Clock::new();
        clock.show(&mut f, 12, 34, 56);
        assert_eq!(f.fills, 6);
        clock.show(&mut f, 12, 34, 56);
        assert_eq!(f.fills, 6);
        clock.show(&mut f, 12, 34, 57);
        assert_eq!(f.fills, 7);
        clock.show(&mut f, 12, 35, 0);
        assert_eq!(f.fills, 10); // minutes units and both seconds digits
    }
}
