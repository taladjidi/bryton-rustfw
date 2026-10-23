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
/// RGB565 dark grey.
pub const DARK: u16 = 0x39e7;

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
    /// The part of the screen this canvas keeps (x, y, w, h): drawing code may skip what falls outside it.
    fn clip(&self) -> (u32, u32, u32, u32) {
        (0, 0, W, H)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Ferris
// ---------------------------------------------------------------------------------------------------------------

/// Ferris the crab, 16 x 11, bit 15 = leftmost pixel; the gaps in row 6 are the eyes.
pub const CRAB: [u16; 11] = [0x6006, 0x9009, 0xa005, 0x6006, 0x2004, 0x3ffc, 0x77ee, 0xffff, 0x7ffe, 0xa815, 0x9009];

/// Draws [`CRAB`] at (`x`, `y`), each bitmap pixel `scale` x `scale`, one fill per horizontal run.
pub fn draw_crab(c: &mut impl Canvas, x: u32, y: u32, scale: u32, color: u16) {
    draw_bitmap(c, &CRAB, x, y, scale, color);
}

/// Draws a 16-pixel-wide bitmap (bit 15 = leftmost) at (`x`, `y`), each pixel `scale` x `scale`, one fill per
/// horizontal run; the 0 bits are left as they are.
pub fn draw_bitmap(c: &mut impl Canvas, bitmap: &[u16], x: u32, y: u32, scale: u32, color: u16) {
    for (r, &bits) in bitmap.iter().enumerate() {
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
/// Ferris: scale and top left corner (centred, at the bottom of the overview).
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
    /// GNSS: None when the receiver is not answering; 0 while it searches; with a fix, signal bars 1 to 4.
    pub gps: Option<u8>,
    /// The blinking "GPS" while searching is shown (it alternates each second).
    pub blink: bool,
    /// USB state.
    pub usb: UsbDot,
    /// Battery charge in percent, if known.
    pub battery: Option<u8>,
    /// USB power present: the battery is charging (shown in green).
    pub charging: bool,
    /// The ride: the time is white with no ride, green while it records, orange while paused or auto-paused.
    pub rec: Rec,
}

/// The ride's state as the strip shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rec {
    /// No ride running.
    Idle,
    /// Recording.
    Running,
    /// Paused, or auto-paused (stopped).
    Paused,
}

/// The strip's boxes (x, width): BLE on the left, the time centred, then USB and the battery on the right.
const STRIP_BLE: (u32, u32) = (4, 3 * CELL * 2);
const STRIP_GPS: (u32, u32) = (STRIP_BLE.0 + STRIP_BLE.1 + 6, 3 * CELL * 2);
const STRIP_TIME: (u32, u32) = ((W - 5 * CELL * 2) / 2, 5 * CELL * 2);
const STRIP_BATTERY: (u32, u32) = (W - 4 - 4 * CELL * 2, 4 * CELL * 2);
/// The USB symbol, 13 x 14 (bit 15 = leftmost): a trident, the plug's arrow on top.
pub const USB_ICON: [u16; 14] = [
    0b0000001000000000,
    0b0000011100000000,
    0b0000111110000000,
    0b0000001000000000,
    0b0100001000111000,
    0b1110001000111000,
    0b0100001000010000,
    0b0010001000100000,
    0b0001001001000000,
    0b0000101010000000,
    0b0000011100000000,
    0b0000001000000000,
    0b0000011100000000,
    0b0000011100000000,
];
/// The USB symbol's left edge and width, between the time and the battery.
const STRIP_USB: (u32, u32) = (STRIP_BATTERY.0 - 21, 13);

/// Draws the status strip: "BLE" on the left (green, red if the chip did not answer), "GPS" blinking grey while
/// the receiver searches, signal bars once it has a fix (nothing if it is silent), the time in the middle; on the right the USB
/// symbol in the USB state's colour (nothing when unplugged) and the battery (green while charging, red at 15 % and
/// under); then the orange line.
pub fn draw_strip(c: &mut impl Canvas, s: &Strip) {
    draw_strip_changes(c, s, None);
    c.fill(0, STRIP_H, W, 2, ACCENT);
}

/// Redraws only the parts of the strip that differ from `old` (all of them, without it): each part has its own
/// box, the time its own character cells.
pub fn draw_strip_changes(c: &mut impl Canvas, s: &Strip, old: Option<&Strip>) {
    let changed = |f: fn(&Strip) -> u32| old.is_none_or(|o| f(o) != f(s));
    let at = |c: &mut _, (x, w): (u32, u32), fg, align, t: &str| {
        text::draw_in(c, &Style { fg, ..LABEL }, x, STRIP_H - LABEL.height(), w, align, t); // ends at the line
    };
    let hm = |s: &Strip| {
        let mut l = text::Line::<8>::new();
        for d in [s.time.0 / 10, s.time.0 % 10, 10, s.time.1 / 10, s.time.1 % 10] {
            l.push_str(if d == 10 { ":" } else { DIGITS[usize::from(d % 10)] });
        }
        l
    };
    let fg = match s.rec {
        Rec::Idle => WHITE,
        Rec::Running => GREEN,
        Rec::Paused => ACCENT,
    };
    let style = Style { fg, ..LABEL };
    let top = STRIP_H - LABEL.height();
    match old {
        Some(o) if o.rec == s.rec => draw_changed_chars(c, &style, STRIP_TIME.0, top, hm(o).as_str(), hm(s).as_str()),
        _ => at(c, STRIP_TIME, fg, Align::Left, hm(s).as_str()),
    }
    if changed(|s| u32::from(s.radio)) {
        at(c, STRIP_BLE, if s.radio { GREEN } else { RED }, Align::Left, "BLE");
    }
    if changed(|s| s.gps.map_or(0x100, u32::from) | u32::from(s.blink) << 9) {
        match s.gps {
            Some(bars @ 1..) => draw_bars(c, bars),
            Some(0) if s.blink => at(c, STRIP_GPS, GREY, Align::Left, "GPS"),
            _ => at(c, STRIP_GPS, BLACK, Align::Left, "GPS"),
        }
    }
    if changed(|s| s.usb as u32) {
        let usb = match s.usb {
            UsbDot::Off => BLACK,
            UsbDot::Failed => RED,
            UsbDot::Powered => ACCENT,
            UsbDot::Ready => GREEN,
        };
        // level with the capitals next to it: 7 font rows at 2x = 14 px, the symbol's height, from the text's top
        draw_bitmap(c, &USB_ICON, STRIP_USB.0, top, 1, usb);
    }
    if changed(|s| s.battery.map_or(0x100, u32::from) | u32::from(s.charging) << 9) {
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
    }
}

/// Redraws, of monospaced text at (`x`, `y`) that was `old` and is now `new`, only the characters that differ, each
/// in its own cell; the whole text when the lengths differ (its cells move). Callers clear what a shorter text
/// leaves, or keep lengths equal.
pub fn draw_changed_chars(c: &mut impl Canvas, style: &Style, x: u32, y: u32, old: &str, new: &str) {
    let cell = CELL * style.scale;
    if old.len() != new.len() {
        text::draw(c, style, x, y, Align::Left, new);
        return;
    }
    for (i, (a, b)) in old.bytes().zip(new.bytes()).enumerate() {
        if a != b {
            let ch = new.get(i..i + 1).unwrap_or(" ");
            text::draw_in(c, style, x + i as u32 * cell, y, cell, Align::Left, ch);
        }
    }
}

/// GNSS signal bars in the "GPS" box, level with the text: four bars rising left to right, the first `n` green,
/// the others dark grey.
fn draw_bars(c: &mut impl Canvas, n: u8) {
    let top = STRIP_H - LABEL.height(); // the capitals' top row; they are 14 px high
    c.fill(STRIP_GPS.0, top, STRIP_GPS.1, LABEL.height(), BLACK);
    for i in 0..4u32 {
        let h = 5 + 3 * i; // 5, 8, 11, 14 px
        let color = if i < u32::from(n) { GREEN } else { DARK };
        c.fill(STRIP_GPS.0 + 2 + 8 * i, top + 14 - h, 6, h, color);
    }
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

    fn clip(&self) -> (u32, u32, u32, u32) {
        (0, self.y, W, 1)
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
    /// The map of the ride (map.rs): its box holds the map instead of a value.
    Map,
    /// The ride's moving time, h:mm:ss.
    Timer,
    /// The ride's distance, km (2 decimals under 100 km, then 1).
    Dist,
    /// Average moving speed, km/h.
    AvgSpeed,
    /// Top speed, km/h.
    MaxSpeed,
    /// Average heart rate.
    AvgHr,
    /// Average power.
    AvgPower,
    /// This lap's moving time.
    LapTime,
    /// This lap's distance, km.
    LapDist,
    /// Altitude gained, m (GPS).
    Ascent,
    /// Bars of the last minutes of power (with a power meter paired), else of heart rate, coloured by zone.
    Graph,
    /// The barometer's temperature, C.
    Temp,
}

impl Field {
    /// Every field, in the order the page menu cycles through them; the index is kept in a byte
    /// ([`crate::PageCfg`]), so new ones go at the end.
    pub const ALL: [Field; 17] = {
        use Field::*;
        [
            Time, Hr, Speed, Cadence, Power, Map, Timer, Dist, AvgSpeed, MaxSpeed, AvgHr, AvgPower, LapTime, LapDist,
            Ascent, Graph, Temp,
        ]
    };

    /// Its label, top left of its box.
    pub fn label(self) -> &'static str {
        [
            "TIME", "HR", "KM/H", "RPM", "WATTS", "MAP", "TIMER", "KM", "AVG KM/H", "MAX KM/H", "AVG HR", "AVG W",
            "LAP TIME", "LAP KM", "ASCENT M", "GRAPH", "TEMP C",
        ][self as usize]
    }

    /// The widest value it shows: the value's scale is chosen so this fits.
    fn widest(self) -> &'static str {
        [
            "88:88", "888", "88.8", "888", "8888", "", "88:88:88", "88.88", "88.8", "88.8", "888", "8888", "88:88:88",
            "88.88", "8888", "", "-88.8",
        ][self as usize]
    }

    /// Its box holds a picture (map, graph), not a value.
    pub fn is_picture(self) -> bool {
        matches!(self, Field::Map | Field::Graph)
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

/// A field's frame: the lines between boxes (right and bottom edges, inside the box) and its label (none on a map).
pub fn draw_field_frame(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field) {
    if x + w < W {
        c.fill(x + w - 1, y, 1, h, GREY);
    }
    if y + h < H - 2 {
        c.fill(x, y + h - 1, w, 1, GREY);
    }
    if field != Field::Map {
        text::draw(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, Align::Left, field.label());
    }
}

/// A field's value in `fg`, centred under its label, in a fixed box (what was there before is cleared).
pub fn draw_field_value(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field, value: &str, fg: u16) {
    let style = Style { fg, ..value_style(field, w, h) };
    let top = y + FIELD_LABEL_H + (h - 1 - FIELD_LABEL_H - style.height()) / 2;
    text::draw_in(c, &style, x + 1, top, w - 2, Align::Centre, value);
}

/// A field's value that was `old`: only the characters that changed when the length is the same (they keep their
/// cells), else the whole value box.
pub fn draw_field_value_changes(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    field: Field,
    old: &str,
    new: &str,
    fg: u16,
) {
    let (x, y, w, h) = bx;
    let style = Style { fg, ..value_style(field, w, h) };
    if old.len() != new.len() || style.width(new) + 2 > w {
        return draw_field_value(c, bx, field, new, fg);
    }
    let top = y + FIELD_LABEL_H + (h - 1 - FIELD_LABEL_H - style.height()) / 2;
    draw_changed_chars(c, &style, x + 1 + (w - 2 - style.width(new)) / 2, top, old, new);
}

/// The graph's plot in field box `bx`: left, top, columns, height. One bar per pixel column, side by side, so the
/// bars make a filled area as stock's graph does; the newest bar is the rightmost column.
pub fn graph_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x + 4, y + FIELD_LABEL_H, w - 8, h - FIELD_LABEL_H - 4)
}

/// Graph column `i` that showed a bar `old` (height, colour) now showing `new`: only the band between the two
/// tops when the colour stays (most columns as the chart scrolls: neighbouring bars are close once smoothed), else
/// the new bar whole.
pub fn draw_graph_column(c: &mut impl Canvas, area: (u32, u32, u32, u32), i: u32, old: (u32, u16), new: (u32, u16)) {
    let (x, bottom) = (area.0 + i, area.1 + area.3);
    let ((h0, c0), (h1, c1)) = ((old.0.min(area.3), old.1), (new.0.min(area.3), new.1));
    if c0 == c1 || h0 == 0 {
        if h1 > h0 {
            c.fill(x, bottom - h1, 1, h1 - h0, c1);
        }
    } else {
        c.fill(x, bottom - h1, 1, h1, c1);
    }
    if h0 > h1 {
        c.fill(x, bottom - h0, 1, h0 - h1, BLACK);
    }
}

/// The whole graph: column `i` gets `bar(i)` (height, colour), the rest of the column black.
pub fn draw_graph(c: &mut impl Canvas, area: (u32, u32, u32, u32), bar: &dyn Fn(u32) -> (u32, u16)) {
    for i in 0..area.2 {
        draw_graph_column(c, area, i, (area.3, BLACK), bar(i));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The overview (the main screen): the time, the date, four fields of the user's choice, the notices (Ferris when
// there are none)
// ---------------------------------------------------------------------------------------------------------------

/// Hours and minutes: the pixel font at 7x, white. No seconds: the overview changes once a minute.
pub const TIME: Style = Style { font: &PIXEL, scale: 7, fg: WHITE, bg: BLACK };
/// Top of the hours and minutes.
pub const TIME_Y: u32 = PAGE_Y + 6;
/// Top of the date line.
pub const DATE_Y: u32 = TIME_Y + PIXEL.height as u32 * TIME.scale; // the text box, not just the ink
/// The overview's fields: top and height (a 2 x 2 grid of [`field_box`] boxes).
pub const OVERVIEW_Y: u32 = DATE_Y + PIXEL.height as u32 * LABEL.scale + 2;
/// See [`OVERVIEW_Y`].
pub const OVERVIEW_H: u32 = NOTICES_Y - 4 - OVERVIEW_Y;
/// Top of the notices: [`NOTICES`] rows of [`LIST_ROW`].
pub const NOTICES_Y: u32 = CRAB_Y - 4;
/// Notices shown at most.
pub const NOTICES: usize = 2;

/// Left edge of "HH:MM", its ink centred (each cell ends with a blank column, the last one does not count).
const TIME_X: u32 = (W - (5 * CELL - 1) * TIME.scale) / 2;

/// Field box `i` of the overview's four.
pub fn overview_box(i: usize) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = field_box(4, OVERVIEW_H, i)?;
    Some((x, y - RIDE_Y + OVERVIEW_Y, w, h))
}

/// Overview digit `i` (H H M M) showing `d`, in its fixed cell of the monospaced font: only the digits that change
/// are drawn, each one a SPI transfer.
pub fn draw_main_digit(c: &mut impl Canvas, i: usize, d: u8) {
    let col = if i < 2 { i } else { i + 1 } as u32; // the colon is column 2
    let s = DIGITS[usize::from(d % 10)];
    text::draw_in(c, &TIME, TIME_X + col * CELL * TIME.scale, TIME_Y, CELL * TIME.scale, Align::Left, s);
}

/// The overview's time: the colon and the digits of HH MM.
pub fn draw_main_time(c: &mut impl Canvas, digits: [u8; 4]) {
    text::draw(c, &TIME, TIME_X + 2 * CELL * TIME.scale, TIME_Y, Align::Left, ":");
    for (i, d) in digits.into_iter().enumerate() {
        draw_main_digit(c, i, d);
    }
}

/// The date line, centred, grey.
pub fn draw_date(c: &mut impl Canvas, date: &str) {
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, 0, DATE_Y, W, Align::Centre, date);
}

/// The notices, newest first; with none, Ferris.
pub fn draw_notices(c: &mut impl Canvas, notices: &[&str]) {
    c.fill(0, NOTICES_Y, W, H - NOTICES_Y, BLACK);
    if notices.is_empty() {
        return draw_ferris(c);
    }
    for (r, n) in notices.iter().take(NOTICES).enumerate() {
        let style = Style { fg: if r == 0 { WHITE } else { GREY }, ..LABEL };
        text::draw_in(c, &style, NEARBY.0, NOTICES_Y + 4 + r as u32 * LIST_ROW, W - 2 * NEARBY.0, Align::Left, n);
    }
}

/// Top of a banner: a notice over the bottom of any screen other than the overview.
pub const BANNER_Y: u32 = H - 40;

/// A banner: `text` in black on orange over the bottom of the screen.
pub fn draw_banner(c: &mut impl Canvas, text: &str) {
    c.fill(0, BANNER_Y, W, 4, BLACK);
    let style = Style { fg: BLACK, bg: ACCENT, ..LABEL };
    c.fill(0, BANNER_Y + 4, W, 8, ACCENT);
    text::draw_in(c, &style, 0, BANNER_Y + 12, W, Align::Centre, text);
    c.fill(0, BANNER_Y + 12 + style.height(), W, H - BANNER_Y - 12 - style.height(), ACCENT);
}

/// Ferris, at the bottom of the overview.
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
    // the label's own length (at most 10 characters), the value takes the rest of the row
    let name_w = (label.len().min(9) as u32 + 1) * CELL * LABEL.scale;
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
                    draw_field_value(&mut f, bx, *field, field.widest(), WHITE);
                    assert!(value_style(*field, bx.2, bx.3).scale >= 2, "{n} fields: {field:?} readable");
                }
            }
        }
        assert!(RIDE_Y + RIDE_H == H && RIDE_Y >= STRIP_H + 2);
        let strip = [STRIP_BLE, STRIP_GPS, STRIP_TIME, STRIP_USB, STRIP_BATTERY];
        assert!(strip.windows(2).all(|p| p[0].0 + p[0].1 <= p[1].0) && STRIP_BATTERY.0 + STRIP_BATTERY.1 <= W);
        assert!(TIME_Y + TIME.height() <= DATE_Y && OVERVIEW_Y + OVERVIEW_H < NOTICES_Y && TIME_Y > STRIP_H);
        assert!(
            NOTICES_Y + 4 + NOTICES as u32 * LIST_ROW <= H && overview_box(3).is_some_and(|b| b.1 + b.3 <= NOTICES_Y)
        );
    }

    /// Updating only the changed characters gives the same pixels as drawing the new value whole, and draws less.
    #[test]
    fn changed_characters_equal_a_full_redraw() {
        let bx = field_box(4, RIDE_H, 1).unwrap();
        for (old, new) in [("22.7", "22.8"), ("22.7", "9.8"), ("--", "135"), ("99", "00")] {
            let (mut a, mut b) = (Frame::new(), Frame::new());
            draw_field_value(&mut a, bx, Field::Speed, old, WHITE);
            let before = a.fills;
            draw_field_value_changes(&mut a, bx, Field::Speed, old, new, WHITE);
            draw_field_value(&mut b, bx, Field::Speed, new, WHITE);
            assert!(a.px == b.px, "{old} -> {new}");
            if old.len() == new.len() && old != "99" {
                assert!(a.fills - before < 4, "{old} -> {new}: {} fills", a.fills - before);
            }
        }
        let strip = Strip {
            time: (9, 59),
            radio: true,
            gps: Some(0),
            blink: true,
            usb: UsbDot::Off,
            battery: Some(80),
            charging: false,
            rec: Rec::Idle,
        };
        let next = Strip { time: (10, 0), blink: false, rec: Rec::Running, ..strip };
        let (mut a, mut b) = (Frame::new(), Frame::new());
        draw_strip(&mut a, &strip);
        draw_strip_changes(&mut a, &next, Some(&strip));
        draw_strip(&mut b, &next);
        assert!(a.px == b.px, "strip");
    }

    #[test]
    fn rows_match_the_framebuffer() {
        let strip = Strip {
            time: (12, 34),
            radio: true,
            gps: Some(4),
            blink: true,
            usb: UsbDot::Ready,
            battery: Some(87),
            charging: true,
            rec: Rec::Paused,
        };
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
