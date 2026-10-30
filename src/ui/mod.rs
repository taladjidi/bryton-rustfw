//! What is on screen: colours, text styles, the overview, the ride pages, menus and ABOUT's Ferris. Draws
//! through [`Canvas`] only, so it knows nothing about the panel protocol (panel.rs) or the board.

pub mod card;
pub mod fonts;
pub mod map;
pub mod media;
pub mod menu;
pub mod page;
pub mod quick;
pub mod segment;
pub mod text;
pub mod theme;
pub mod tiles;
pub mod trainer;

mod fields;
mod gauge;
mod overview;
mod strip;
pub use fields::*;
pub use gauge::*;
pub use overview::*;
pub use strip::*;
pub use theme::Theme;

use crate::fonts::PIXEL;
use crate::storage::workout_file::{PROF_H, unpack_bar};
use text::{Align, Face, Style};

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
/// RGB565 yellow.
pub const YELLOW: u16 = 0xffe0;

/// Text on the accent (a selected row, a banner, a button): black, one step off [`BLACK`] so the theme tells it from
/// the background ([`Theme`]); DARK draws it as black.
pub const ON_ACCENT: u16 = 0x0020;
/// Text on the alarm red: white, one step off [`WHITE`] for the same reason.
pub const ON_ALARM: u16 = 0xffdf;
/// The overview's menu cog.
pub const COG_COLOR: u16 = 0xc618;

/// A climb's colour by its grade (% x 10), in the bands climbing apps use: under 6 %, under 9 %, under 12 %, above
/// (Garmin's ClimbPro colours its climbs by grade the same way; the bands here are this firmware's). The zone
/// palette's zone 2 to 5 colours by severity.
///
/// ```
/// use bryton_rustfw::ride::ZONE_COLORS;
/// use bryton_rustfw::ui::grade_color;
/// assert_eq!([grade_color(45), grade_color(150)], [ZONE_COLORS[1], ZONE_COLORS[4]]);
/// ```
pub fn grade_color(x10: i32) -> u16 {
    use crate::ride::ZONE_COLORS;
    ZONE_COLORS[match x10 {
        ..60 => 1,
        60..90 => 2,
        90..120 => 3,
        _ => 4,
    }]
}

/// Small text: labels. The pixel font at 2x, so its dots match Ferris' look.
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
    /// Whether text takes the smooth font where there is one (the FONT setting, [`crate::text`]); the pixel font
    /// otherwise.
    fn face(&self) -> Face {
        Face::Pixel
    }
    /// The colour the built-in colour `c` is drawn in: the theme's ([`Theme`]); `c` itself unless a theme is on.
    /// Drawing code that makes colours itself (a blit) asks once before the blit, never per pixel.
    fn ink(&self, c: u16) -> u16 {
        c
    }
    /// The theme in use: the colours of [`Canvas::ink`] and the metrics (zone bar, frame, menu row). DARK unless a
    /// canvas carries one.
    fn theme(&self) -> &Theme {
        &Theme::DARK
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
/// Ferris: scale and top left corner for the drawing test (the overview has none; ABOUT draws its own, larger).
#[cfg(test)]
const CRAB_SCALE: u32 = 4;
/// Ferris left edge.
#[cfg(test)]
const CRAB_X: u32 = (W - 16 * CRAB_SCALE) / 2;
/// Ferris top edge.
#[cfg(test)]
const CRAB_Y: u32 = 272;

// ---------------------------------------------------------------------------------------------------------------
// Boxes (the status strip is in strip.rs)
// ---------------------------------------------------------------------------------------------------------------

/// A box on the panel: left, top, width, height. What a touch is tested against (one place for every hit-test);
/// the boxes that are only drawn stay tuples, and convert with `Rect::from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect(pub u32, pub u32, pub u32, pub u32);

impl From<(u32, u32, u32, u32)> for Rect {
    fn from((x, y, w, h): (u32, u32, u32, u32)) -> Self {
        Rect(x, y, w, h)
    }
}

impl From<Rect> for (u32, u32, u32, u32) {
    fn from(Rect(x, y, w, h): Rect) -> Self {
        (x, y, w, h)
    }
}

impl Rect {
    /// The touch at (`x`, `y`) is in the box.
    pub fn contains(self, x: u16, y: u16) -> bool {
        (self.0..self.0 + self.2).contains(&u32::from(x)) && (self.1..self.1 + self.3).contains(&u32::from(y))
    }

    /// The box with a margin of `m` px on every side (fingers are wide), clipped at the panel's top left.
    pub fn grown(self, m: u32) -> Rect {
        let (x, y) = (self.0.saturating_sub(m), self.1.saturating_sub(m));
        Rect(x, y, self.0 + self.2 + m - x, self.1 + self.3 + m - y)
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

/// The page area but box `(x, y, w, h)` (inside it), which its widget paints whole: above it, beside it, below it.
pub fn clear_page_but(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32)) {
    c.fill(0, PAGE_Y, W, y - PAGE_Y, BLACK);
    c.fill(0, y, x, h, BLACK);
    c.fill(x + w, y, W - x - w, h, BLACK);
    c.fill(0, y + h, W, H - y - h, BLACK);
}

/// The page under a menu, but for its rows ([`draw_menu_row`] paints the whole of each): the title's band and what
/// is left under the last row.
pub fn clear_menu(c: &mut impl Canvas) {
    c.fill(0, PAGE_Y, W, LIST_Y - PAGE_Y, BLACK);
    let row = c.theme().menu_row;
    let end = LIST_Y + menu_rows(row) as u32 * row;
    c.fill(0, end, W, H - end, BLACK);
}

/// How many rows [`draw_list`] has.
pub const LIST_ROWS: usize = ((H - LIST_Y) / LIST_ROW) as usize;

/// A device's row as a fingerprint (FNV-1a over what the row shows; the RSSI in buckets of 4 dBm, so a jitter does not
/// redraw); 1 for a row with no device.
fn list_print(d: Option<&crate::nrf::Device>) -> u32 {
    let Some(d) = d else { return 1 };
    let mut h = 0x811c_9dc5u32;
    let bucket = (i32::from(d.rssi) >> 2).to_le_bytes();
    let maker = d.maker.unwrap_or("").as_bytes();
    for &x in d.name().as_bytes().iter().chain(&[0]).chain(maker).chain(&d.addr).chain(&bucket) {
        h = (h ^ u32::from(x)).wrapping_mul(0x0100_0193);
    }
    h
}

/// The fingerprints of the rows [`draw_list`] would draw now.
pub fn list_prints(nearby: &crate::nrf::Nearby) -> [u32; LIST_ROWS] {
    let mut devices = nearby.strongest();
    core::array::from_fn(|_| list_print(devices.next().as_ref()))
}

/// The devices heard, strongest first, one per row: the name (white); for one that sent none (phones, laptops),
/// what kind of device it is from its manufacturer data, else its address (grey); and the RSSI in dBm on the right. As many as fit; the rows left are cleared.
/// With `shown` (the fingerprints on screen), only the rows that differ are drawn, and it is updated.
pub fn draw_list(c: &mut impl Canvas, nearby: &crate::nrf::Nearby, mut shown: Option<&mut [u32; LIST_ROWS]>) {
    let mut devices = nearby.strongest();
    let name_w = LIST_NAME * CELL * LABEL.scale;
    for r in 0..LIST_ROWS {
        let y = LIST_Y + r as u32 * LIST_ROW;
        let d = devices.next();
        if let Some(s) = shown.as_deref_mut().and_then(|s| s.get_mut(r)) {
            let print = list_print(d.as_ref());
            if *s == print {
                continue;
            }
            *s = print;
        }
        let Some(d) = d else {
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
    /// The font and colours ([`Canvas::face`], [`Canvas::ink`]).
    pub look: Theme,
}

impl Row {
    /// Row `y`, all black, in DARK.
    pub fn new(y: u32) -> Self {
        Row { y, px: [BLACK; W as usize], look: Theme::DARK }
    }
}

impl Canvas for Row {
    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
        let color = self.look.ink(color);
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

    fn face(&self) -> Face {
        self.look.face
    }

    fn ink(&self, c: u16) -> u16 {
        self.look.ink(c)
    }

    fn theme(&self) -> &Theme {
        &self.look
    }
}

/// Height of a banner: a notice over the top of any screen other than the overview (the strip and the page's
/// first 20 px), two lines of the 2x font.
pub const BANNER_H: u32 = 44;

/// `text` as at most two lines that fit `w` in `style`: cut at a newline, else at the last space that lets the first
/// line fit (else at the last character that does); the second line is what is left (cut when drawn if too long).
///
/// ```
/// use bryton_rustfw::ui::{lines, LABEL};
/// assert_eq!(lines(&LABEL, "LAP", 228), ("LAP", ""));
/// assert_eq!(lines(&LABEL, "PAUSED: HOLD START TO END", 228), ("PAUSED: HOLD START", "TO END"));
/// assert_eq!(lines(&LABEL, "INCOMING CALL\nMum", 228), ("INCOMING CALL", "Mum"));
/// ```
pub fn lines<'a>(style: &Style, text: &'a str, w: u32) -> (&'a str, &'a str) {
    if let Some((a, b)) = text.split_once('\n') {
        return (a, b);
    }
    if style.width(text) <= w {
        return (text, "");
    }
    let fits = |end: usize| text.get(..end).is_some_and(|t| style.width(t) <= w);
    match text.char_indices().rev().find(|&(i, ch)| ch == ' ' && fits(i)).map(|(i, _)| i) {
        Some(i) => (text.get(..i).unwrap_or(""), text.get(i + 1..).unwrap_or("")),
        None => {
            let i = text.char_indices().map(|(i, _)| i).take_while(|&i| fits(i)).last().unwrap_or(0);
            (text.get(..i).unwrap_or(""), text.get(i..).unwrap_or(""))
        }
    }
}

/// A banner over the top of the screen: `text` on one or two lines ([`lines`]) at 2x, black on orange, or white on
/// red for an `alarm` (heart rate high, off course, the battery low, a ride not saving).
pub fn draw_banner(c: &mut impl Canvas, text: &str, alarm: bool) {
    let (fg, bg) = if alarm { (ON_ALARM, RED) } else { (ON_ACCENT, ACCENT) };
    let style = Style { fg, bg, ..LABEL };
    let lh = style.height();
    let (a, b) = lines(&style, text, W - 12);
    let rows: &[&str] = if b.is_empty() { &[a] } else { &[a, b] };
    let mut y = (BANNER_H - lh * rows.len() as u32) / 2;
    c.fill(0, 0, W, y, bg);
    for t in rows {
        text::draw_in(c, &style, 0, y, W, Align::Centre, t);
        y += lh;
    }
    c.fill(0, y, W, BANNER_H - y, bg);
}

/// The call buttons' height: a handset and the key's name.
const CALL_BUTTON_H: u32 = 64;
/// Top of the incoming call's buttons (the banner with the caller is at the top).
pub const CALL_Y: u32 = H - CALL_BUTTON_H - 12;
/// The decline (red, left) and answer (green, right) buttons: x, y, w, h.
pub const CALL_BUTTONS: [(u32, u32, u32, u32); 2] =
    [(8, CALL_Y + 6, W / 2 - 16, CALL_BUTTON_H), (W / 2 + 8, CALL_Y + 6, W / 2 - 16, CALL_BUTTON_H)];
/// A telephone handset, 16 x 16 (the earpiece top left, the mouthpiece bottom right).
const HANDSET: [u16; 16] = [
    0x3000, 0x7800, 0x7c00, 0x7c00, 0x7800, 0x3800, 0x3800, 0x1c00, 0x1c00, 0x0e00, 0x0706, 0x039f, 0x01ff, 0x00fe,
    0x007c, 0x0000,
];

/// An incoming call over any screen: the banner at the top says it is a call and who calls; two buttons at the
/// bottom, each with a handset and the key that does the same: a red decline (BACK) and a green answer (START). The
/// page between keeps updating ([`crate::App`]'s drawing area stops at [`CALL_Y`]). Once answered it goes: iOS
/// removes the call's notification at the answer (seen on the unit, 2026-10-24), so neither the call's time nor a
/// hang-up can follow over ANCS.
pub fn draw_call(c: &mut impl Canvas, who: &str) {
    let mut l = text::Line::<48>::new();
    l.push_str("INCOMING CALL\n");
    l.push_str(who);
    draw_banner(c, l.as_str(), false);
    c.fill(0, CALL_Y, W, H - CALL_Y, BLACK);
    for (i, &(x, y, w, h)) in CALL_BUTTONS.iter().enumerate() {
        let (bg, fg, key) = if i == 0 { (RED, ON_ALARM, "BACK") } else { (GREEN, ON_ACCENT, "START") };
        c.fill(x, y, w, h, bg);
        draw_bitmap(c, &HANDSET, x + w / 2 - 16, y + 6, 2, fg);
        text::draw_in(c, &Style { fg, bg, ..LABEL }, x + 2, y + h - 4 - LABEL.height(), w - 4, Align::Centre, key);
    }
}

/// The end-ride screen's buttons (x, y, w, h), top to bottom: 64 px tall (a gloved finger, the call's buttons), 12
/// px apart, under the title and the ride's numbers.
pub const END_BUTTONS: [(u32, u32, u32, u32); 3] = [(12, 84, W - 24, 64), (12, 160, W - 24, 64), (12, 236, W - 24, 64)];

/// The button the end-ride screen opens on: SAVE (the ride is over: one Start; RESUME is a Page up, DISCARD two
/// questions away).
pub const END_SAVE: usize = 1;

/// The end-ride screen's buttons by slot: RESUME, SAVE (the default), DISCARD; or the discard's question, DISCARD
/// and KEEP (the default) apart, where a second tap of the DISCARD before cannot reach DISCARD again.
pub fn end_labels(discard: bool) -> [Option<&'static str>; 3] {
    if discard { [Some("DISCARD"), None, Some("KEEP")] } else { [Some("RESUME"), Some("SAVE"), Some("DISCARD")] }
}

/// The end-ride screen's slot at (`x`, `y`), if a button of `labels` is there.
pub fn end_hit(labels: &[Option<&str>; 3], x: u16, y: u16) -> Option<usize> {
    END_BUTTONS.iter().zip(labels).position(|(&r, l)| l.is_some() && Rect::from(r).contains(x, y))
}

/// The end-ride screen (`discard`: its question, "DISCARD RIDE?"): the title, the ride's numbers (`stats`), the
/// buttons of [`end_labels`], slot `sel` selected; between the question's two, where the ride goes.
pub fn draw_end(c: &mut impl Canvas, discard: bool, stats: &str, sel: usize) {
    let title = if discard { "DISCARD RIDE?" } else { "END RIDE" };
    text::draw_in(c, &LABEL, 0, PAGE_Y + 8, W, Align::Centre, title);
    text::draw_in(c, &LABEL, 0, PAGE_Y + 32, W, Align::Centre, stats);
    let labels = end_labels(discard);
    for (k, l) in labels.iter().enumerate() {
        match l {
            Some(l) => draw_end_button(c, k, l, k == sel),
            None => {
                let (_, y, _, h) = END_BUTTONS[1];
                let grey = Style { fg: GREY, ..LABEL };
                text::draw_in(c, &grey, 0, y + h / 2 - LABEL.height() - 2, W, Align::Centre, "KEPT IN");
                text::draw_in(c, &grey, 0, y + h / 2 + 2, W, Align::Centre, "RUSTFW/DISCARDED");
            }
        }
    }
}

/// End-ride button `k` alone (the selection moved: only the two buttons are drawn again): selected, orange with
/// black text (the menus' selected row); else a grey frame, the label white, DISCARD's red.
pub fn draw_end_button(c: &mut impl Canvas, k: usize, label: &str, selected: bool) {
    let Some(&(x, y, w, h)) = END_BUTTONS.get(k) else { return };
    let ty = y + (h - LABEL.height()) / 2;
    let (bg, fg) = match (selected, label) {
        (true, _) => (ACCENT, ON_ACCENT),
        (false, "DISCARD") => (BLACK, RED),
        _ => (BLACK, WHITE),
    };
    let frame = if selected { ACCENT } else { GREY };
    c.fill(x, y, w, 2, frame);
    c.fill(x, y + h - 2, w, 2, frame);
    c.fill(x, y + 2, 2, h - 4, frame);
    c.fill(x + w - 2, y + 2, 2, h - 4, frame);
    c.fill(x + 2, y + 2, w - 4, ty - y - 2, bg);
    text::draw_in(c, &Style { fg, bg, ..LABEL }, x + 2, ty, w - 4, Align::Centre, label);
    c.fill(x + 2, ty + LABEL.height(), w - 4, y + h - 2 - ty - LABEL.height(), bg);
}

/// ABOUT: Ferris large, the firmware's name, its version and commit (`describe`: "1a2b3c4", "-dirty" when built
/// from changes not committed) and the commit's date, and where it lives.
pub fn draw_about(c: &mut impl Canvas, version: &str, describe: &str, date: &str) {
    // Ferris 64 x 44 and the lines close: the six notice lines (scale 2, the pixel font's) fill the panel's lower part
    const BIG: u32 = 4;
    draw_crab(c, (W - 16 * BIG) / 2, PAGE_Y + 8, BIG, RUST);
    let mut y = PAGE_Y + 8 + 11 * BIG + 8;
    let mut line = |style: &Style, s: &str| {
        text::draw_in(c, style, 0, y, W, Align::Centre, s);
        y += style.height() + 2;
    };
    line(&LABEL, "BRYTON-RUSTFW");
    let mut v = text::Line::<24>::new();
    v.push_str("V");
    v.push_str(version);
    v.push_str(" SAFE RUST");
    line(&LABEL, v.as_str());
    line(&LABEL, describe);
    let grey = Style { fg: GREY, ..LABEL };
    line(&grey, date);
    // the notices at the pixel font's scale 2, a line each (about 19 characters fit the panel's width)
    for notice in ABOUT_NOTICES {
        text::draw_in(c, &grey, 0, y, W, Align::Centre, notice);
        y += grey.height();
    }
}

/// ABOUT's notices: the map data's licences (OpenStreetMap's ODbL, OpenMapTiles, IGN's Plan IGN v2 under Etalab 2.0)
/// and the font's (Barlow, SIL Open Font License 1.1).
pub const ABOUT_NOTICES: [&str; 6] = [
    "(c) OpenStreetMap",
    "contributors (ODbL)",
    "(c) OpenMapTiles",
    "(c) IGN (Plan IGN",
    "v2, Etalab 2.0)",
    "BARLOW, SIL OFL 1.1",
];

// ---------------------------------------------------------------------------------------------------------------
// Menus: a title, then rows of a label and a value; the selected row in orange.
// ---------------------------------------------------------------------------------------------------------------

/// Menu row pitch of the built-in themes (a file sets 44 to 56, `Canvas::theme().menu_row`): 44 px, about 8 mm on
/// this panel (0.18 mm a pixel), a fingertip's size (Apple's 44 pt, Material's 48 dp); 20 px rows (3.6 mm) let a gloved finger cover two or three (UI review, 2026-10-25).
pub const MENU_ROW: u32 = 44;

/// Rows a menu shows at most (6); a longer menu turns a screen of rows at a time.
pub const MENU_ROWS: usize = menu_rows(MENU_ROW);

/// Rows a menu shows at row pitch `row` (6 at 44, 4 at 56).
pub const fn menu_rows(row: u32) -> usize {
    ((H - LIST_Y) / row) as usize
}

/// Where a tap goes back (as Back): the "<" at the left of a menu's title, a finger wide, from the strip's line to
/// the first row.
pub const BACK_TAP: (u32, u32, u32, u32) = (0, PAGE_Y, 48, LIST_Y - PAGE_Y);

/// The "<" at the left of a title line ([`BACK_TAP`]).
pub fn draw_back(c: &mut impl Canvas) {
    text::draw_in(c, &LABEL, 4, NEARBY.1, NEARBY.0 - 4, Align::Left, "<");
}

/// The title line of a menu, with the "<" that goes back ([`BACK_TAP`]).
pub fn draw_title(c: &mut impl Canvas, title: &str) {
    draw_back(c);
    text::draw_in(c, &LABEL, NEARBY.0, NEARBY.1, NEARBY.2, Align::Centre, title);
}

/// A menu row's label box: its own length and half a character's gap, but never over the value, which is whole
/// (every row fits whole except a course's: its name is cut and its distance stays whole, see `course_row`).
pub(crate) fn menu_label_w(label: &str, value: &str) -> u32 {
    let own = (label.len() as u32 * 2 + 1) * CELL / 2 * LABEL.scale;
    own.min(NEARBY.2.saturating_sub(LABEL.width(value)))
}

/// Menu row `i`: `label` on the left, `value` on the right in `value_color` (white unless it says a state), centred
/// in its [`MENU_ROW`], a dark line under it; selected, the row is orange with black text (10:1 against the orange,
/// where white was 2:1).
pub fn draw_menu_row(c: &mut impl Canvas, i: usize, label: &str, value: &str, value_color: u16, selected: bool) {
    let row = c.theme().menu_row;
    let y = LIST_Y + i as u32 * row;
    // a row with no value and a grey one is a note (an empty list), not a choice
    let note = value.is_empty() && value_color == GREY;
    let (bg, fg) = if selected { (ACCENT, ON_ACCENT) } else { (BLACK, if note { GREY } else { WHITE }) };
    let name_w = menu_label_w(label, value);
    let ty = y + (row - 1 - LABEL.height()) / 2;
    c.fill(0, y, W, ty - y, bg);
    c.fill(0, ty, NEARBY.0, LABEL.height(), bg);
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0, ty, name_w, Align::Left, label);
    let fg = if selected { ON_ACCENT } else { value_color };
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0 + name_w, ty, NEARBY.2 - name_w, Align::Right, value);
    c.fill(NEARBY.0 + NEARBY.2, ty, W - NEARBY.0 - NEARBY.2, LABEL.height(), bg);
    let below = ty + LABEL.height();
    c.fill(0, below, W, y + row - 1 - below, bg);
    c.fill(0, y + row - 1, W, 1, DARK);
}

/// Only the value of [`draw_menu_row`]`(i, label, ..)`, in its own box (which `draw_in` clears): the row's label,
/// selection and colour are as drawn.
pub fn draw_menu_value(c: &mut impl Canvas, i: usize, label: &str, value: &str, value_color: u16, selected: bool) {
    let row = c.theme().menu_row;
    let y = LIST_Y + i as u32 * row;
    let (bg, fg) = if selected { (ACCENT, ON_ACCENT) } else { (BLACK, value_color) };
    let name_w = menu_label_w(label, value);
    let ty = y + (row - 1 - LABEL.height()) / 2;
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0 + name_w, ty, NEARBY.2 - name_w, Align::Right, value);
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;

    /// A menu value changed in its own box paints what a whole row would, in fewer pixels (P4).
    #[test]
    fn a_menu_value_repaints_in_its_own_box() {
        for selected in [false, true] {
            let (mut whole, mut part) = (Frame::new(), Frame::new());
            draw_menu_row(&mut whole, 2, "HEART RATE", "FOUND", GREY, selected);
            draw_menu_row(&mut part, 2, "HEART RATE", "FOUND", GREY, selected);
            let before = part.painted;
            draw_menu_row(&mut whole, 2, "HEART RATE", "PAIRED", WHITE, selected);
            draw_menu_value(&mut part, 2, "HEART RATE", "PAIRED", WHITE, selected);
            assert_eq!(whole.px, part.px, "same screen");
            let (row, value) = (240 * 43, part.painted - before);
            std::println!("menu value: {value} px for the value, {row} for the row");
            assert!(value < row / 2);
        }
    }

    /// The nearby list draws only the rows that changed: an RSSI jitter nothing, a new device one row (P5).
    #[test]
    fn the_nearby_list_draws_only_changed_rows() {
        use crate::nrf::{AdvReport, Nearby};
        let report = |a: u8, rssi: i8| AdvReport { addr_type: 0, addr: [a; 6], rssi, data: &[] };
        let mut n = Nearby::new();
        n.add(&report(1, -50));
        n.add(&report(2, -70));
        let mut f = Frame::new();
        let mut shown = list_prints(&n);
        draw_list(&mut f, &n, None);
        let all = f.painted;
        n.add(&report(1, -51)); // same bucket
        draw_list(&mut f, &n, Some(&mut shown));
        assert_eq!(f.painted, all, "a jitter draws nothing");
        n.add(&report(3, -80));
        draw_list(&mut f, &n, Some(&mut shown));
        let one = f.painted - all;
        std::println!("nearby: first draw {all} px, new device {one} px");
        assert!(one > 0 && one < all, "{one} of {all}");
        n.add(&report(1, -60)); // a new bucket: the row changes
        draw_list(&mut f, &n, Some(&mut shown));
        assert!(f.painted > all + one);
    }

    #[test]
    fn rect_contains_and_grows() {
        let r = Rect(10, 20, 30, 40);
        assert!(r.contains(10, 20) && r.contains(39, 59));
        assert!(!r.contains(9, 20) && !r.contains(40, 20) && !r.contains(10, 60) && !r.contains(10, 19));
        let g = r.grown(6);
        assert!(g.contains(4, 14) && g.contains(45, 65) && !g.contains(3, 14) && !g.contains(46, 65));
        // clipped at the top left: the same far edge
        assert_eq!(Rect(4, 2, 10, 10).grown(6), Rect(0, 0, 20, 18));
    }
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn every_field_in_one_group() {
        for f in Field::ALL {
            assert_eq!(FIELD_GROUPS.iter().filter(|(_, g)| g.contains(&f)).count(), 1, "{f:?}");
        }
    }

    #[test]
    fn menu_labels_are_whole() {
        assert!(LABEL.width("EDIT THIS PAGE") <= menu_label_w("EDIT THIS PAGE", ">"), "a long label is not cut");
        let cut = menu_label_w("A COURSE WITH A LONG NAME", "42.1 KM");
        assert_eq!(cut + LABEL.width("42.1 KM"), NEARBY.2, "the value stays whole");
    }

    #[test]
    fn field_names_fit_their_menu_row() {
        for f in Field::ALL {
            assert!(LABEL.width(f.label()) <= NEARBY.2 - menu_label_w("FIELD 1", ""), "{}", f.label());
        }
    }

    #[test]
    fn banners_fit_two_lines() {
        for t in [
            "HEART RATE PAIRED",
            "PAUSED: HOLD START TO END",
            "HOLD BACK AGAIN TO SWITCH OFF",
            "INCOMING CALL\nMum",
            "FORGET HEART RATE?\nSTART: YES BACK: NO",
            "FORGET SPEED+CAD?\nSTART: YES BACK: NO",
            "IMPORT SETTINGS?\nSTART: YES BACK: NO",
            "SEARCHING AGAIN\nSTART: FORGET",
            "FORGET THE PHONE?\nSTART: YES BACK: NO",
        ] {
            let (a, b) = lines(&LABEL, t, W - 12);
            assert!(LABEL.width(a) <= W - 12 && LABEL.width(b) <= W - 12, "{t}: {a} / {b}");
            assert_eq!(a.len() + b.len() + usize::from(!b.is_empty()), t.len(), "{t}: nothing lost");
        }
        assert!(2 * LABEL.height() <= BANNER_H && BANNER_H > PAGE_Y, "two lines, over the strip");
        let mut f = Frame::new();
        draw_banner(&mut f, "A VERY LONG BANNER THAT GOES ON AND ON PAST TWO LINES", true);
        draw_call(&mut f, "Somebody with a long name");
    }

    /// The two ends of every metric a theme file may set: all at the least, all at the most.
    pub fn metric_ends() -> [Theme; 2] {
        use theme::{FRAME, MENU_ROW, ZONE_BAR};
        [(ZONE_BAR.0, FRAME.0, MENU_ROW.0), (ZONE_BAR.2, FRAME.2, MENU_ROW.2)]
            .map(|(zone_bar, frame, menu_row)| Theme { zone_bar, frame, menu_row, ..Theme::DARK })
    }

    /// A framebuffer that panics on any fill off screen.
    pub struct Frame {
        pub px: Vec<u16>,
        fills: usize,
        painted: usize,
        /// Text in the smooth font ([`Canvas::face`]).
        pub face: Face,
        /// The theme drawn with (DARK).
        pub theme: Theme,
    }

    impl Frame {
        pub fn new() -> Self {
            Frame { px: vec![BLACK; (W * H) as usize], fills: 0, painted: 0, face: Face::Pixel, theme: Theme::DARK }
        }
        /// A frame drawn with the theme `t`.
        pub fn themed(t: Theme) -> Self {
            Frame { theme: t, ..Frame::new() }
        }
        pub fn at(&self, x: u32, y: u32) -> u16 {
            self.px[(y * W + x) as usize]
        }
        /// Pixels painted so far (a redraw's cost).
        pub fn filled(&self) -> usize {
            self.painted
        }
        fn count(&self, color: u16) -> usize {
            self.px.iter().filter(|&&p| p == color).count()
        }
    }

    impl Canvas for Frame {
        fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
            let color = self.ink(color);
            self.blit(x, y, w, h, &mut |_, _| color);
        }

        fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
            assert!(x + w <= W && y + h <= H, "fill ({x},{y}) {w}x{h} off screen");
            self.fills += 1;
            self.painted += (w * h) as usize;
            for dy in 0..h {
                for dx in 0..w {
                    self.px[((y + dy) * W + x + dx) as usize] = px(dx, dy);
                }
            }
        }

        fn face(&self) -> Face {
            self.face
        }

        /// As the panel in DARK draws it, so a test compares the same pixels.
        fn ink(&self, c: u16) -> u16 {
            self.theme.ink(c)
        }

        fn theme(&self) -> &Theme {
            &self.theme
        }
    }

    /// Menu rows at both ends of `menu_row` (44 and 56): the rows tile from LIST_Y without overlap or gap, each paints
    /// only its own band (its last line the DARK rule), the last whole row ends on screen, and the title's band and
    /// the rest under the rows are cleared by `clear_menu`.
    #[test]
    fn menu_rows_tile_at_both_ends() {
        for t in metric_ends() {
            let row = t.menu_row;
            let rows = menu_rows(row);
            assert!(rows >= 4 && LIST_Y + rows as u32 * row <= H, "{row}: {rows} rows");
            assert!(row >= 44, "a fingertip's size");
            for selected in [false, true] {
                for slot in 0..rows {
                    let mut f = Frame::themed(t);
                    draw_menu_row(&mut f, slot, "HEART RATE", "PAIRED", WHITE, selected);
                    draw_menu_value(&mut f, slot, "HEART RATE", "PAIRED", WHITE, selected);
                    draw_workout_row(&mut f, slot, ("SAT 24", "1:00"), selected, &[3; 20], true);
                    draw_split_icon(&mut f, slot, true, selected);
                    let (y0, y1) = (LIST_Y + slot as u32 * row, LIST_Y + (slot as u32 + 1) * row);
                    // an unselected row is black on black: only a selected one shows its band
                    for y in (0..H).filter(|_| selected) {
                        let painted = (0..W).any(|x| f.at(x, y) != BLACK);
                        assert_eq!(painted, (y0..y1).contains(&y), "{row} px, slot {slot}, y {y}");
                    }
                    assert!((0..W).all(|x| f.at(x, y1 - 1) == DARK), "{row} px: the rule under slot {slot}");
                }
            }
            let mut f = Frame::themed(t);
            f.fill(0, 0, W, H, WHITE);
            clear_menu(&mut f);
            assert!((0..W).all(|x| f.at(x, PAGE_Y) == BLACK && f.at(x, H - 1) == BLACK && f.at(x, LIST_Y) == WHITE));
            assert!((0..W).all(|x| f.at(x, LIST_Y + rows as u32 * row - 1) == WHITE), "rows left alone");
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

    /// `Field::ALL` lists the fields in their declaration order, so `label` and `widest` (sized by it, indexed by the
    /// field) line up: a merge that duplicated a row of `widest` once sized the power fields' values five off.
    #[test]
    fn field_tables_line_up() {
        for (i, f) in Field::ALL.iter().enumerate() {
            assert_eq!(*f as usize, i, "{f:?}");
        }
        assert_eq!((Field::Power30s.widest(), Field::If.widest(), Field::Dfa.widest()), ("8888", "8.88", "8.88"));
        assert_eq!(Field::BattHrs.widest(), "8.8H");
    }

    /// Two values side by side never run together: the gap between their inks is at least twice the gap between
    /// two characters of one (one blank column of the font at the value's scale), in every layout.
    #[test]
    fn neighbouring_values_keep_apart() {
        for id in 1..USER_LAYOUTS {
            let b: Vec<_> = (0..8).map_while(|i| field_box(id, RIDE_H, i)).collect();
            for pair in b.windows(2).filter(|p| p[0].1 == p[1].1) {
                let at = |bx| {
                    let (style, left, _, big, _, _) = value_at(bx, "88.88", WHITE);
                    (left, left + ink(&style, big), style.scale)
                };
                let ((_, right_end, s1), (next_left, _, s2)) = (at(pair[0]), at(pair[1]));
                let gap = next_left - right_end;
                assert!(gap >= 2 * s1.max(s2), "layout {id}: {gap} px between values at {s1}x");
            }
        }
    }

    /// Both overview variants are tables of boxes that tile their area (the TODAY card's, the four fields' below),
    /// touch nothing else, and hold their values at both ends of the metrics (Frame panics off screen).
    #[test]
    fn overview_variants_tile_and_fit() {
        let table: Vec<_> = (0..).map_while(table_box).collect();
        let fields: Vec<_> = (0..).map_while(overview_box).collect();
        assert_eq!((table.len(), fields.len()), (TABLE.len(), 4));
        for (boxes, (ay, ah)) in [(&table, (TODAY_BOX.1, TODAY_BOX.3)), (&fields, (OVERVIEW_Y, OVERVIEW_H))] {
            assert_eq!(boxes.iter().map(|b| b.2 * b.3).sum::<u32>(), W * ah, "the boxes tile their area");
            assert!(boxes.iter().all(|b| b.0 + b.2 <= W && b.1 >= ay && b.1 + b.3 <= ay + ah));
        }
        for theme in metric_ends() {
            let mut f = Frame::themed(theme);
            for (&bx, field) in table.iter().zip(TABLE) {
                draw_field_frame_bar(&mut f, bx, field, GREEN, (true, true)); // imperial: the longer unit labels
                draw_field_value(&mut f, bx, field.widest(), WHITE);
                assert!(value_scale(field.widest(), bx.2, bx.3) >= 2, "{field:?} readable at bar {}", theme.zone_bar);
            }
        }
    }

    /// The ride boxes tile the field area exactly, every value fits its box, and the top line stays
    /// clear (Frame panics on anything off screen).
    #[test]
    fn ride_layouts_tile_and_fit() {
        for (h, theme) in metric_ends().into_iter().map(|t| (RIDE_H, t)) {
            for n in 1..USER_LAYOUTS {
                let b: Vec<_> = (0..8).map_while(|i| field_box(n, h, i)).collect();
                assert_eq!(b.len(), field_count(n));
                assert!(b.len() <= crate::page::CELLS, "a page keeps eight fields");
                let area: u32 = b.iter().map(|&(_, _, w, h)| w * h).sum();
                assert!(area <= W * h && area + W * 2 >= W * h, "{n} fields cover the area");
                let mut f = Frame::themed(theme);
                for (&bx, field) in b.iter().flat_map(|bx| Field::ALL.iter().map(move |f| (bx, f))) {
                    draw_field_frame(&mut f, bx, *field, (false, false));
                    draw_field_frame_bar(&mut f, bx, *field, GREEN, (false, false));
                    draw_cell_frame(&mut f, bx, ACCENT);
                    // the label's ink, under the bar, ends before the value band (FIELD_LABEL_H)
                    let mut l = Frame::themed(theme);
                    draw_field_frame(&mut l, bx, *field, (false, false));
                    let last = (bx.1..bx.1 + 40).rev().find(|&y| (bx.0..bx.0 + 100).any(|x| l.at(x, y) == GREY));
                    if let Some(y) = last.filter(|_| !field.is_picture() && bx.3 > 70) {
                        assert!(
                            y - bx.1 <= FIELD_LABEL_H,
                            "{n} fields, bar {}: the label reaches the value band",
                            theme.zone_bar
                        );
                    }
                    draw_field_value(&mut f, bx, field.widest(), WHITE);
                    assert!(value_scale(field.widest(), bx.2, bx.3) >= 2, "{n} fields: {field:?} readable");
                    if field.widest().len() as u32 <= VALUE_CHARS {
                        let s = value_scale(field.widest(), bx.2, bx.3);
                        assert_eq!(s, box_scale(bx.2, bx.3), "{n} fields: {field:?} at its box's scale");
                        assert!(ink_cells(VALUE_CHARS, s) + 2 * VALUE_MARGIN <= bx.2, "{n} fields: {field:?} fits");
                    }
                }
            }
        }
        const { assert!(RIDE_Y + RIDE_H == H && RIDE_Y >= STRIP_H + 2) };
        let strip = [STRIP_BLE, STRIP_GPS, STRIP_LOCK, STRIP_TIME, STRIP_REC, STRIP_USB, STRIP_BATTERY];
        assert!(strip.windows(2).all(|p| p[0].0 + p[0].1 <= p[1].0) && STRIP_BATTERY.0 + STRIP_BATTERY.1 <= W);
        assert!(DATE_Y > STRIP_H && DATE_Y + DATE_H < TODAY_BOX.1 && OVERVIEW_Y + OVERVIEW_H < NOTICES_Y);
        assert!(GEAR_AT.1 + COG <= TODAY_BOX.1 && GEAR_AT.0 + COG < W, "the cog in the date row");
        assert!(DATE.width("WED 28 OCT") + 8 <= GEAR_AT.0 - 4, "the longest date clears the cog");
        assert!(GEAR_TAP.2 >= 44 && GEAR_TAP.3 >= 44 && GEAR_TAP.0 + GEAR_TAP.2 <= W, "a finger's size round the cog");
        assert!(
            NOTICES_Y + 4 + NOTICES as u32 * LIST_ROW <= H && overview_box(3).is_some_and(|b| b.1 + b.3 <= NOTICES_Y)
        );
    }

    /// The workout gauge fits every box of every layout and of the overview (Frame panics off screen), and moving
    /// the marker draws the same pixels as drawing the gauge whole.
    #[test]
    fn gauge_fits_and_moves_in_place() {
        // a box under 64 px (the rows of BIG+1+2X2) draws the gauge's number smaller: left out
        let rides =
            (1..USER_LAYOUTS).flat_map(|n| (0..6).filter_map(move |i| field_box(n, RIDE_H, i))).filter(|b| b.3 >= 64);
        // the overview's boxes are 60 px since the TODAY card grew: the same rule as a ride page's small boxes
        for bx in rides.chain((0..4).filter_map(overview_box).filter(|b| b.3 >= 64)) {
            let cols = gauge_cols(bx);
            let band = (cols / 3, cols * 2 / 3);
            let titles = ("POWER 30S", "", "200-220");
            let (mut a, mut b) = (Frame::new(), Frame::new());
            let zones = crate::workout::gauge_zones(
                crate::storage::workout_file::Kind::Power,
                (200, 220),
                cols,
                &crate::ride::Zones::standard(200, 190),
            );
            let bar = |m| GaugeBar { band, marker: Some((m, YELLOW)), zones };
            draw_gauge(&mut a, bx, titles, "195", YELLOW, Some(bar(0)));
            draw_gauge_marker(&mut a, bx, Some((0, YELLOW)), bar(cols - 1));
            draw_gauge(&mut b, bx, titles, "195", YELLOW, Some(bar(cols - 1)));
            assert!(a.px == b.px, "{bx:?}");
            assert!(gauge_parts(bx).2.scale >= 2, "{bx:?}: the number readable");
        }
    }

    /// Updating only the changed characters gives the same pixels as drawing the new value whole, and draws less.
    #[test]
    fn changed_characters_equal_a_full_redraw() {
        let bx = field_box(4, RIDE_H, 1).unwrap();
        for (old, new) in [
            ("22.7", "22.8"),
            ("22.7", "9.8"),
            ("--", "135"),
            ("99", "00"),
            ("12:31", "12:38"),
            ("1:23|45", "1:23|46"),
            ("59:59", "1:00|00"),
        ] {
            let (mut a, mut b) = (Frame::new(), Frame::new());
            draw_field_value(&mut a, bx, old, WHITE);
            let before = a.fills;
            draw_field_value_changes(&mut a, bx, old, new, WHITE);
            draw_field_value(&mut b, bx, new, WHITE);
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
            locked: false,
            more: false,
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
            locked: false,
            more: false,
        };
        let mut f = Frame::new();
        draw_strip(&mut f, &strip);
        for y in [0, 8, STRIP_H, CRAB_Y + 20, H - 1] {
            let mut row = Row::new(y);
            draw_strip(&mut row, &strip);
            assert!(row.px[..] == f.px[(y * W) as usize..((y + 1) * W) as usize], "row {y}");
        }
    }

    /// A value redrawn over another, whatever their lengths and scales, leaves what a fresh draw of the new one
    /// does, in every field box (2026-10-25 review: "OPEN" then "200-220", "95-105" then "100-110", any short value
    /// then "1.6K 6%" left the old glyphs' pixels above and below the smaller value).
    #[test]
    fn values_redrawn_leave_nothing_behind() {
        let vals = [
            "OPEN", "95-105", "100-110", "1.6K 10%", "0.5K -1%", "--", "9", "99", "9.9", "99.9", "999", "9999",
            "99.99", "100.0", "1:00|00", "59:59", "12:34|56", "1.6K 6%", "12K 10%", "200-220", "50/50", "-12.5",
            "CRIT", "9999K", "88/88",
        ];
        let boxes: Vec<_> = (1..USER_LAYOUTS)
            .flat_map(|n| (0..8).filter_map(move |i| field_box(n, RIDE_H, i)))
            .chain((0..4).filter_map(overview_box))
            .collect();
        let mut bad = Vec::new();
        for &bx in &boxes {
            for old in vals {
                for new in vals {
                    let mut b = Frame::new();
                    draw_field_value(&mut b, bx, new, WHITE);
                    // "" as the old text: a new colour, all of it (App::update_page)
                    for was in [old, ""] {
                        let mut a = Frame::new();
                        draw_field_value(&mut a, bx, old, if was.is_empty() { RED } else { WHITE });
                        draw_field_value_changes(&mut a, bx, was, new, WHITE);
                        if a.px != b.px {
                            bad.push(std::format!("{bx:?}: {old:?} was {was:?} -> {new}"));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{} left over, first {:?}", bad.len(), bad.first());
    }

    /// The time and MORE under a notification's title are both on screen (one used to wipe the other), and MORE goes
    /// when there is no more.
    #[test]
    fn notif_shows_time_and_more() {
        let lit = |f: &Frame, x0: u32, x1: u32| (x0..x1).any(|x| (LIST_Y..LIST_Y + 20).any(|y| f.at(x, y) != BLACK));
        let mut f = Frame::new();
        draw_notif(&mut f, "MESSAGES", "09:41", true, [("Ana", true), ("hi", false)].into_iter());
        assert!(lit(&f, 4, 80) && lit(&f, W - 60, W), "time left, MORE right");
        draw_notif(&mut f, "MESSAGES", "09:41", false, [("Ana", true)].into_iter());
        assert!(lit(&f, 4, 80) && !lit(&f, W - 60, W), "MORE gone, time kept");
        // more lines than fit: the first NOTIF_LINES are drawn, no more
        let mut f = Frame::new();
        let many = (0..NOTIF_LINES + 5).map(|_| ("line", false));
        draw_notif(&mut f, "MESSAGES", "09:41", true, many);
        let row = |r: u32| (4..60).any(|x| f.at(x, LIST_Y + LIST_ROW * r + 8) != BLACK);
        assert!(row(NOTIF_LINES as u32), "the last row is drawn; the Frame refuses a fill off screen");
    }
}
