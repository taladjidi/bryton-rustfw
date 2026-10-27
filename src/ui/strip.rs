//! The status strip: a thin line at the top of every screen (what it shows, drawn whole and as changes).

use super::*;

/// Height of the status strip; an orange line follows it.
pub const STRIP_H: u32 = 22;

/// USB dev link state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsbDot {
    /// Not plugged in: not shown.
    #[default]
    Off,
    /// Plugged in, but the PHY or the core did not come up.
    Failed,
    /// Plugged in, link up, not yet configured by the computer.
    Powered,
    /// Configured: the dev link is usable.
    Ready,
    /// Configured, then suspended by the computer (idle): the next traffic resumes it.
    Suspended,
}

/// What the status strip shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    /// The touch is locked (Back and Lap held together): a padlock left of the time.
    pub locked: bool,
    /// A ride page's MORE variant is up: "MORE" where "BLE" and the GPS bars are.
    pub more: bool,
}

/// The ride's state as the strip shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rec {
    /// No ride running.
    #[default]
    Idle,
    /// Recording.
    Running,
    /// Paused, or auto-paused (stopped).
    Paused,
    /// A ride, but the recorder is not saving it to the card (failing, refused, or a build that may not write):
    /// the dot grey.
    NotSaving,
}

/// The strip's boxes (x, width): BLE on the left, the time centred, then USB and the battery on the right.
pub(super) const STRIP_BLE: (u32, u32) = (4, 3 * CELL * 2);
pub(super) const STRIP_GPS: (u32, u32) = (STRIP_BLE.0 + STRIP_BLE.1 + 2, 3 * CELL * 2);
pub(super) const STRIP_TIME: (u32, u32) = ((W - 5 * CELL * 2) / 2, 5 * CELL * 2);
pub(super) const STRIP_BATTERY: (u32, u32) = (W - 4 - 4 * CELL * 2, 4 * CELL * 2);
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
pub(super) const STRIP_USB: (u32, u32) = (STRIP_BATTERY.0 - 21, 13);
/// The ride's state, right of the time: a red dot while it records, orange bars while paused, nothing otherwise.
pub(super) const STRIP_REC: (u32, u32) = (STRIP_TIME.0 + STRIP_TIME.1 + 3, 10);
/// The touch lock's padlock: a cell of its own between "GPS" and the time (1 px clear of the GPS box, whose last letter has a pixel of its own, 3 of the time's),
/// 7 wide, 10 high.
pub(super) const STRIP_LOCK: (u32, u32) = (STRIP_GPS.0 + STRIP_GPS.1 + 1, 8);
const PADLOCK: [u16; 10] = [0x3800, 0x4400, 0x4400, 0xfe00, 0xfe00, 0xee00, 0xee00, 0xfe00, 0xfe00, 0xfe00];
/// A dot 10 px wide (bit 15 leftmost), and two bars.
const REC_DOT: [u16; 10] = [0x1e00, 0x7f80, 0x7f80, 0xffc0, 0xffc0, 0xffc0, 0xffc0, 0x7f80, 0x7f80, 0x1e00];
const PAUSE_BARS: [u16; 10] = [0xe1c0; 10];

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
    let top = STRIP_H - LABEL.height();
    match old {
        Some(o) => draw_changed_chars(c, &LABEL, STRIP_TIME.0, top, hm(o).as_str(), hm(s).as_str()),
        None => at(c, STRIP_TIME, WHITE, Align::Left, hm(s).as_str()),
    }
    if changed(|s| s.rec as u32) {
        // level with the capitals: 10 rows in their 14
        c.fill(STRIP_REC.0, top, STRIP_REC.1, 14, BLACK);
        match s.rec {
            Rec::Idle => {}
            Rec::Running => draw_bitmap(c, &REC_DOT, STRIP_REC.0, top + 2, 1, RED),
            Rec::Paused => draw_bitmap(c, &PAUSE_BARS, STRIP_REC.0, top + 2, 1, ACCENT),
            Rec::NotSaving => draw_bitmap(c, &REC_DOT, STRIP_REC.0, top + 2, 1, GREY),
        }
    }
    if changed(|s| s.locked as u32) {
        c.fill(STRIP_LOCK.0, top + 2, STRIP_LOCK.1, 10, BLACK);
        if s.locked {
            draw_bitmap(c, &PADLOCK, STRIP_LOCK.0, top + 2, 1, WHITE);
        }
    }
    // MORE takes the place of "BLE" and the GPS bars while a page's denser variant is up
    let swapped = changed(|s| s.more as u32);
    let both = (STRIP_BLE.0, STRIP_GPS.0 + STRIP_GPS.1 - STRIP_BLE.0 - 1); // up to the padlock's column
    if s.more {
        if swapped {
            at(c, both, ACCENT, Align::Left, "MORE");
        }
    } else {
        if swapped {
            c.fill(both.0, top, both.1, LABEL.height(), BLACK);
        }
        if swapped || changed(|s| u32::from(s.radio)) {
            at(c, STRIP_BLE, if s.radio { GREEN } else { RED }, Align::Left, "BLE");
        }
        if swapped || changed(|s| s.gps.map_or(0x100, u32::from) | u32::from(s.blink) << 9) {
            match s.gps {
                Some(bars @ 1..) => draw_bars(c, bars),
                Some(0) if s.blink => at(c, STRIP_GPS, GREY, Align::Left, "GPS"),
                _ => at(c, STRIP_GPS, BLACK, Align::Left, "GPS"),
            }
        }
    }
    if changed(|s| s.usb as u32) {
        let usb = match s.usb {
            UsbDot::Off => BLACK,
            UsbDot::Failed => RED,
            UsbDot::Powered => ACCENT,
            UsbDot::Ready => GREEN,
            UsbDot::Suspended => GREY,
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

/// The strip whole again after the hint ([`draw_edit_hint`]): its background first, which [`draw_strip`] leaves.
pub fn draw_strip_whole(c: &mut impl Canvas, s: &Strip) {
    c.fill(0, 0, W, STRIP_H, BLACK);
    draw_strip(c, s);
}
