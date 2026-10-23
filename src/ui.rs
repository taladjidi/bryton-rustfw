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
/// RGB565 mid grey.
pub const GREY: u16 = 0x8410;

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

/// The bar at the top, on every page.
pub fn draw_bar(c: &mut impl Canvas) {
    c.fill(20, 40, W - 40, 6, ACCENT);
}

/// The parts of the clock page that never change: the bar, the colon, Ferris.
pub fn draw_static(c: &mut impl Canvas) {
    text::draw(c, &TIME, TIME_X + 2 * CELL * TIME.scale, TIME_Y, Align::Left, ":");
    draw_bar(c);
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

/// The Nordic chip's state, top left: "BLE" and its firmware image number in green once it answered, "BLE ?" in
/// red if it did not.
pub fn draw_ble(c: &mut impl Canvas, v: Option<crate::nrf::Version>) {
    let mut line = text::Line::<8>::new();
    line.push_str("BLE ");
    let color = match v {
        Some(v) => {
            line.push_u32(u32::from(v.image));
            GREEN
        }
        None => {
            line.push_str("?");
            RED
        }
    };
    text::draw_in(c, &Style { fg: color, ..LABEL }, BLE_LABEL.0, BLE_LABEL.1, BLE_LABEL.2, Align::Left, line.as_str());
}

/// The list page's title, under the bar: "3 NEARBY", with "..." while the scan runs.
pub fn draw_nearby(c: &mut impl Canvas, count: usize, scanning: bool) {
    let mut line = text::Line::<16>::new();
    line.push_u32(count as u32);
    line.push_str(if scanning { " NEARBY..." } else { " NEARBY" });
    text::draw_in(c, &LABEL, NEARBY.0, NEARBY.1, NEARBY.2, Align::Centre, line.as_str());
}

/// The line under the bar: left, top, width (between the bar and the time). The heart rate on the clock page, the
/// device count on the list page.
pub const NEARBY: (u32, u32, u32) = (20, 58, W - 40);

/// Heart rate from the ANT+ strap, under the bar: "HR 72", "HR --" while there is no reading.
pub fn draw_hr(c: &mut impl Canvas, bpm: Option<u8>) {
    let mut line = text::Line::<8>::new();
    line.push_str("HR ");
    match bpm {
        Some(v) if v > 0 => line.push_u32(u32::from(v)),
        _ => line.push_str("--"),
    }
    text::draw_in(c, &Style { fg: RED, ..LABEL }, NEARBY.0, NEARBY.1, NEARBY.2, Align::Centre, line.as_str());
}

/// Top of the page area, under the bar: what changes with Page.
pub const PAGE_Y: u32 = 50;
/// First row of the device list.
pub const LIST_Y: u32 = 84;
/// Device list row pitch (the font's line height at 2x).
pub const LIST_ROW: u32 = 20;
/// Characters of a device's name shown (or its address, 12 hex digits); its RSSI goes on the right.
const LIST_NAME: u32 = 12;

/// Clears the page area (a page switch).
pub fn clear_page(c: &mut impl Canvas) {
    c.fill(0, PAGE_Y, W, H - PAGE_Y, BLACK);
}

/// The devices heard, strongest first, one per row: the name (white); for one that sent none (phones, laptops),
/// its maker from the manufacturer data, else its address (grey); and the RSSI in dBm on the right. As many as fit; the rows left are cleared.
pub fn draw_list(c: &mut impl Canvas, nearby: &crate::nrf::Nearby) {
    let rows = (H - LIST_Y) / LIST_ROW;
    let mut devices = nearby.strongest();
    let name_w = LIST_NAME * CELL * LABEL.scale;
    for r in 0..rows {
        let y = LIST_Y + r * LIST_ROW;
        let Some(d) = devices.next() else {
            c.fill(NEARBY.0, y, NEARBY.2, LIST_ROW, BLACK);
            continue;
        };
        let mut name = text::Line::<16>::new();
        let maker = d.company.and_then(crate::nrf::company_name);
        let color = if !d.name().is_empty() {
            name.push_str(d.name());
            WHITE
        } else if let Some(maker) = maker {
            name.push_str(maker);
            GREY
        } else {
            for b in d.addr.iter().rev() {
                for n in [usize::from(b >> 4), usize::from(b & 0xf)] {
                    name.push_str("0123456789ABCDEF".get(n..n + 1).unwrap_or("?"));
                }
            }
            GREY
        };
        text::draw_in(c, &Style { fg: color, ..LABEL }, NEARBY.0, y, name_w, Align::Left, name.as_str());
        let mut rssi = text::Line::<8>::new();
        if d.rssi < 0 {
            rssi.push_str("-");
        }
        rssi.push_u32(u32::from(d.rssi.unsigned_abs()));
        text::draw_in(c, &LABEL, NEARBY.0 + name_w, y, NEARBY.2 - name_w, Align::Right, rssi.as_str());
    }
}

/// The BLE label's box: left, top (level with the USB label), width.
pub const BLE_LABEL: (u32, u32, u32) = (20, USB_LABEL.1, 7 * CELL * LABEL.scale);

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
        boxes.push((BLE_LABEL.0, BLE_LABEL.1, BLE_LABEL.2, LABEL.height()));
        boxes.push((NEARBY.0, NEARBY.1, NEARBY.2, LABEL.height()));
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
