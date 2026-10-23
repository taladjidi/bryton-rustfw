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

/// Width of one character cell of the monospaced pixel font, in font pixels.
const CELL: u32 = 6;
/// Ferris: scale and top left corner (centred, at the bottom of the first ride page).
pub const CRAB_SCALE: u32 = 4;
/// Ferris left edge.
pub const CRAB_X: u32 = (W - 16 * CRAB_SCALE) / 2;
/// Ferris top edge.
pub const CRAB_Y: u32 = 272;

// ---------------------------------------------------------------------------------------------------------------
// The status strip: a thin line at the top of every screen
// ---------------------------------------------------------------------------------------------------------------

/// Height of the status strip; an orange line follows it.
pub const STRIP_H: u32 = 22;

/// USB dev link state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbDot {
    /// Not plugged in: not shown.
    Off,
    /// Plugged in, but the PHY or the core did not come up.
    Failed,
    /// Plugged in, link up, not yet configured by the computer.
    Powered,
    /// Configured: the dev link is usable.
    Ready,
}

/// What the status strip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strip {
    /// Time of day, hours and minutes.
    pub time: (u8, u8),
    /// The Nordic chip answered (BLE / ANT usable).
    pub radio: bool,
    /// USB state.
    pub usb: UsbDot,
    /// Battery charge in percent, if known.
    pub battery: Option<u8>,
    /// USB power present: the battery is charging (shown in green).
    pub charging: bool,
}

/// The strip's boxes (x, width): BLE on the left, the time centred, then USB and the battery on the right.
const STRIP_BLE: (u32, u32) = (4, 3 * CELL * 2);
const STRIP_TIME: (u32, u32) = ((W - 5 * CELL * 2) / 2, 5 * CELL * 2);
const STRIP_BATTERY: (u32, u32) = (W - 4 - 4 * CELL * 2, 4 * CELL * 2);
/// The USB dot: x, size (it sits between the time and the battery, centred on the text's capitals).
const STRIP_USB: (u32, u32) = (STRIP_BATTERY.0 - 16, 8);

/// Draws the status strip: "BLE" on the left (green, red if the chip did not answer), the time in the middle; on
/// the right a dot in the USB state's colour (nothing when unplugged) and the battery (green while charging, red at 15 % and
/// under); then the orange line.
pub fn draw_strip(c: &mut impl Canvas, s: &Strip) {
    let at = |c: &mut _, (x, w): (u32, u32), fg, align, t: &str| {
        text::draw_in(c, &Style { fg, ..LABEL }, x, STRIP_H - LABEL.height(), w, align, t); // ends at the line
    };
    let mut l = text::Line::<8>::new();
    for d in [s.time.0 / 10, s.time.0 % 10, 10, s.time.1 / 10, s.time.1 % 10] {
        l.push_str(if d == 10 { ":" } else { DIGITS[usize::from(d % 10)] });
    }
    at(c, STRIP_TIME, WHITE, Align::Left, l.as_str());
    at(c, STRIP_BLE, if s.radio { GREEN } else { RED }, Align::Left, "BLE");
    let usb = match s.usb {
        UsbDot::Off => BLACK,
        UsbDot::Failed => RED,
        UsbDot::Powered => ACCENT,
        UsbDot::Ready => GREEN,
    };
    c.fill(STRIP_USB.0, STRIP_H - LABEL.height() + 3, STRIP_USB.1, STRIP_USB.1, usb);
    let mut l = text::Line::<8>::new();
    match s.battery {
        Some(p) => l.push_u32(u32::from(p)),
        None => l.push_str("--"),
    }
    l.push_str("%");
    let color = match s.battery {
        _ if s.charging => GREEN,
        Some(p) if p <= 15 => RED,
        _ => WHITE,
    };
    at(c, STRIP_BATTERY, color, Align::Right, l.as_str());
    c.fill(0, STRIP_H, W, 2, ACCENT);
}

/// One-digit strings, to build numbers without core::fmt.
pub const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The device list's title: "3 NEARBY", with "..." while the scan runs.
pub fn draw_nearby(c: &mut impl Canvas, count: usize, scanning: bool) {
    let mut line = text::Line::<16>::new();
    line.push_u32(count as u32);
    line.push_str(if scanning { " NEARBY..." } else { " NEARBY" });
    text::draw_in(c, &LABEL, NEARBY.0, NEARBY.1, NEARBY.2, Align::Centre, line.as_str());
}

/// The title line of the menus and the device list: left, top, width.
pub const NEARBY: (u32, u32, u32) = (20, PAGE_Y + 4, W - 40);

/// Top of the page area, under the strip and its line.
pub const PAGE_Y: u32 = STRIP_H + 2;
/// First row of the device list and the menus.
pub const LIST_Y: u32 = NEARBY.1 + 24;
/// Device list row pitch (the font's line height at 2x).
pub const LIST_ROW: u32 = 20;
/// Characters of a device's name shown (or its address, 12 hex digits); its RSSI goes on the right.
const LIST_NAME: u32 = 12;

/// Clears the page area (a page switch).
pub fn clear_page(c: &mut impl Canvas) {
    c.fill(0, PAGE_Y, W, H - PAGE_Y, BLACK);
}

/// The devices heard, strongest first, one per row: the name (white); for one that sent none (phones, laptops),
/// what kind of device it is from its manufacturer data, else its address (grey); and the RSSI in dBm on the right. As many as fit; the rows left are cleared.
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
        let color = if !d.name().is_empty() {
            name.push_str(d.name());
            WHITE
        } else if let Some(maker) = d.maker {
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

// ---------------------------------------------------------------------------------------------------------------
// Ride pages: a layout of 1 to 6 fields between the bar and Ferris.
// ---------------------------------------------------------------------------------------------------------------

/// What a ride page field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Time of day, HH:MM.
    Time,
    /// Heart rate.
    Hr,
    /// Speed, km/h with one decimal.
    Speed,
    /// Cadence.
    Cadence,
    /// Power.
    Power,
}

impl Field {
    /// Every field, in the order the page menu cycles through them.
    pub const ALL: [Field; 5] = [Field::Time, Field::Hr, Field::Speed, Field::Cadence, Field::Power];

    /// Its label, top left of its box.
    pub fn label(self) -> &'static str {
        ["TIME", "HR", "KM/H", "RPM", "WATTS"][self as usize]
    }

    /// The widest value it shows: the value's scale is chosen so this fits.
    fn widest(self) -> &'static str {
        ["88:88", "888", "88.8", "888", "8888"][self as usize]
    }

    /// The next one in [`Field::ALL`].
    pub fn next(self) -> Field {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }

    /// From its index in [`Field::ALL`] (anything else: time).
    pub fn from_index(i: u32) -> Field {
        Self::ALL.get(i as usize).copied().unwrap_or(Field::Time)
    }
}

/// The field counts a page can have, in the order the page menu cycles through them (0: page off).
pub const LAYOUTS: [u8; 6] = [0, 1, 2, 3, 4, 6];

/// Top of the ride fields.
pub const RIDE_Y: u32 = PAGE_Y;
/// Height of the ride fields: everything under the strip.
pub const RIDE_H: u32 = H - RIDE_Y;

/// Field box `i` (x, y, w, h) of a page with `n` fields in a field area `h` high: one, two stacked, one big over
/// two, 2 x 2, 2 x 3. None past the last.
pub fn field_box(n: u8, h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    let (c, half, third) = (W / 2, h / 2, h / 3);
    let (col, row) = ((i % 2) as u32, (i / 2) as u32);
    let b = match (n, i) {
        (1, 0) => (0, 0, W, h),
        (2, 0..=1) => (0, i as u32 * half, W, half),
        (3, 0) => (0, 0, W, half),
        (3, 1..=2) => ((i as u32 - 1) * c, half, c, half),
        (4, 0..=3) => (col * c, row * half, c, half),
        (6, 0..=5) => (col * c, row * third, c, third),
        _ => return None,
    };
    Some((b.0, RIDE_Y + b.1, b.2, b.3))
}

/// Height of a field's label line.
const FIELD_LABEL_H: u32 = 22;

/// The value style for `field` in a box `w` x `h`: the largest pixel-font scale its widest value fits.
fn value_style(field: Field, w: u32, h: u32) -> Style {
    let mut style = Style { scale: 1, ..LABEL };
    for scale in (1..=8).rev() {
        style.scale = scale;
        if style.width(field.widest()) + 8 <= w && style.height() + FIELD_LABEL_H <= h {
            break;
        }
    }
    style
}

/// A field's frame: the lines between boxes (right and bottom edges, inside the box) and its label.
pub fn draw_field_frame(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field) {
    if x + w < W {
        c.fill(x + w - 1, y, 1, h, GREY);
    }
    if y + h < H - 2 {
        c.fill(x, y + h - 1, w, 1, GREY);
    }
    text::draw(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, Align::Left, field.label());
}

/// A field's value, centred under its label, in a fixed box (what was there before is cleared).
pub fn draw_field_value(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field, value: &str) {
    let style = value_style(field, w, h);
    let top = y + FIELD_LABEL_H + (h - 1 - FIELD_LABEL_H - style.height()) / 2;
    text::draw_in(c, &style, x + 1, top, w - 2, Align::Centre, value);
}

// ---------------------------------------------------------------------------------------------------------------
// The main screen: the time, big, and Ferris
// ---------------------------------------------------------------------------------------------------------------

/// Hours and minutes on the main screen: the pixel font at 7x, white.
pub const TIME: Style = Style { font: &PIXEL, scale: 7, fg: WHITE, bg: BLACK };
/// Seconds on the main screen: the pixel font at 5x, orange.
pub const SECONDS: Style = Style { font: &PIXEL, scale: 5, fg: ACCENT, bg: BLACK };
/// Top of the hours and minutes.
pub const TIME_Y: u32 = 70;
/// Top of the seconds.
pub const SECONDS_Y: u32 = 160;

/// The main screen's time: "HH:MM" centred, the seconds under it.
pub fn draw_main_time(c: &mut impl Canvas, hm: &str, s: &str) {
    text::draw_in(c, &TIME, 0, TIME_Y, W, Align::Centre, hm);
    text::draw_in(c, &SECONDS, 0, SECONDS_Y, W, Align::Centre, s);
}

/// Ferris, at the bottom of the main screen.
pub fn draw_ferris(c: &mut impl Canvas) {
    draw_crab(c, CRAB_X, CRAB_Y, CRAB_SCALE, RUST);
}

// ---------------------------------------------------------------------------------------------------------------
// Menus: a title, then rows of a label and a value; the selected row in orange.
// ---------------------------------------------------------------------------------------------------------------

/// Rows a menu shows at most.
pub const MENU_ROWS: usize = ((H - LIST_Y) / LIST_ROW) as usize;

/// The title line of a menu or list.
pub fn draw_title(c: &mut impl Canvas, title: &str) {
    text::draw_in(c, &LABEL, NEARBY.0, NEARBY.1, NEARBY.2, Align::Centre, title);
}

/// Menu row `i`: `label` on the left, `value` on the right in `value_color`; selected, the row is orange.
pub fn draw_menu_row(c: &mut impl Canvas, i: usize, label: &str, value: &str, value_color: u16, selected: bool) {
    let y = LIST_Y + i as u32 * LIST_ROW;
    let (bg, fg) = if selected { (ACCENT, BLACK) } else { (BLACK, WHITE) };
    let name_w = 10 * CELL * LABEL.scale;
    c.fill(0, y, NEARBY.0, LIST_ROW, bg);
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0, y, name_w, Align::Left, label);
    let fg = if selected { BLACK } else { value_color };
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0 + name_w, y, NEARBY.2 - name_w, Align::Right, value);
    c.fill(NEARBY.0 + NEARBY.2, y, W - NEARBY.0 - NEARBY.2, LIST_ROW, bg);
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

    /// The ride boxes tile the field area exactly, every value fits its box, and Ferris and the top line stay
    /// clear (Frame panics on anything off screen).
    #[test]
    fn ride_layouts_tile_and_fit() {
        for h in [RIDE_H] {
            for &n in &LAYOUTS[1..] {
                let b: Vec<_> = (0..8).map_while(|i| field_box(n, h, i)).collect();
                assert_eq!(b.len(), usize::from(n));
                let area: u32 = b.iter().map(|&(_, _, w, h)| w * h).sum();
                assert!(area <= W * h && area + W * 2 >= W * h, "{n} fields cover the area");
                let mut f = Frame::new();
                for (&bx, field) in b.iter().zip(Field::ALL.iter().cycle()) {
                    draw_field_frame(&mut f, bx, *field);
                    draw_field_value(&mut f, bx, *field, field.widest());
                    assert!(value_style(*field, bx.2, bx.3).scale >= 2, "{n} fields: {field:?} readable");
                }
            }
        }
        assert!(RIDE_Y + RIDE_H == H && RIDE_Y >= STRIP_H + 2);
        let strip = [STRIP_BLE, STRIP_TIME, (STRIP_USB.0 - 8, STRIP_USB.1 + 8), STRIP_BATTERY];
        assert!(strip.windows(2).all(|p| p[0].0 + p[0].1 <= p[1].0) && STRIP_BATTERY.0 + STRIP_BATTERY.1 <= W);
        assert!(SECONDS_Y + SECONDS.height() <= CRAB_Y && TIME_Y + TIME.height() <= SECONDS_Y && TIME_Y > STRIP_H);
    }

    #[test]
    fn rows_match_the_framebuffer() {
        let strip = Strip { time: (12, 34), radio: true, usb: UsbDot::Ready, battery: Some(87), charging: true };
        let mut f = Frame::new();
        draw_strip(&mut f, &strip);
        draw_ferris(&mut f);
        for y in [0, 8, STRIP_H, CRAB_Y + 20, H - 1] {
            let mut row = Row::new(y);
            draw_strip(&mut row, &strip);
            draw_ferris(&mut row);
            assert!(row.px[..] == f.px[(y * W) as usize..((y + 1) * W) as usize], "row {y}");
        }
    }
}
