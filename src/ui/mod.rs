//! What is on screen: colours, text styles, Ferris, the layout and the clock. Draws
//! through [`Canvas`] only, so it knows nothing about the panel protocol (panel.rs) or the board.

pub mod card;
pub mod fonts;
pub mod map;
pub mod media;
pub mod menu;
pub mod page;
pub mod text;
pub mod tiles;
pub mod trainer;

use crate::fonts::PIXEL;
use crate::storage::workout::{PROF_H, unpack_bar};
use text::{Align, Style};

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
    fn smooth(&self) -> bool {
        false
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
/// Ferris: scale and top left corner (centred; where the overview's notices end).
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
    /// The touch is locked (Back and Lap held together): a padlock left of the time.
    pub locked: bool,
    /// A ride page's MORE variant is up: "MORE" where "BLE" and the GPS bars are.
    pub more: bool,
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
    /// A ride, but the recorder is not saving it to the card (failing, refused, or a build that may not write):
    /// the dot grey.
    NotSaving,
}

/// The strip's boxes (x, width): BLE on the left, the time centred, then USB and the battery on the right.
const STRIP_BLE: (u32, u32) = (4, 3 * CELL * 2);
const STRIP_GPS: (u32, u32) = (STRIP_BLE.0 + STRIP_BLE.1 + 2, 3 * CELL * 2);
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
/// The ride's state, right of the time: a red dot while it records, orange bars while paused, nothing otherwise.
const STRIP_REC: (u32, u32) = (STRIP_TIME.0 + STRIP_TIME.1 + 3, 10);
/// The touch lock's padlock: a cell of its own between "GPS" and the time (1 px clear of the GPS box, whose last letter has a pixel of its own, 3 of the time's),
/// 7 wide, 10 high.
const STRIP_LOCK: (u32, u32) = (STRIP_GPS.0 + STRIP_GPS.1 + 1, 8);
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

/// The page under a menu, but for its rows ([`draw_menu_row`] paints the whole of each): the title's band and what
/// is left under the last row.
pub fn clear_menu(c: &mut impl Canvas) {
    c.fill(0, PAGE_Y, W, LIST_Y - PAGE_Y, BLACK);
    let end = LIST_Y + MENU_ROWS as u32 * MENU_ROW;
    c.fill(0, end, W, H - end, BLACK);
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
    /// Text takes the smooth font ([`Canvas::smooth`]).
    pub smooth: bool,
}

impl Row {
    /// Row `y`, all black.
    pub fn new(y: u32) -> Self {
        Row { y, px: [BLACK; W as usize], smooth: false }
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

    fn smooth(&self) -> bool {
        self.smooth
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
    /// The last minutes of power, coloured by zone.
    Graph,
    /// The barometer's temperature, C.
    Temp,
    /// The last minutes of heart rate, coloured by zone.
    HrGraph,
    /// DFA alpha1 of the last 2 minutes of beats (hrv.rs): about 0.75 at the aerobic threshold.
    Dfa,
    /// RMSSD of the last minute of beats, ms (hrv.rs).
    Rmssd,
    // power.rs and the ride's derived metrics
    /// 3 s average power.
    Power3s,
    /// 10 s average power.
    Power10s,
    /// 30 s average power.
    Power30s,
    /// Normalized power.
    Np,
    /// Intensity factor (NP / FTP).
    If,
    /// Training stress score.
    Tss,
    /// Work, kJ.
    Kj,
    /// Energy, kcal.
    Kcal,
    /// 3 s power per kg.
    Wkg,
    /// W' balance, kJ.
    Wbal,
    /// The ride's max power.
    MaxPower,
    /// The ride's max heart rate.
    MaxHr,
    /// Average cadence (zeros left out).
    AvgCadence,
    /// Left / right pedal balance, percent.
    Balance,
    /// VAM, m/h.
    Vam,
    /// Grade, %.
    Grade,
    // the graphs of the other plots (ride::Plot): a tap on a graph goes to the next one
    /// The last minutes (or the ride) of W' balance.
    WbalGraph,
    /// Of DFA alpha1.
    DfaGraph,
    /// Of altitude: the elevation profile.
    AltGraph,
    // totals of the rides on the card (stock's and, once saved, this firmware's) and the ride running
    /// This week's distance, km (from Monday).
    WeekKm,
    /// This week's riding time, h.
    WeekH,
    /// This year's distance, km.
    YearKm,
    /// Every ride's distance, km (the odometer).
    TotalKm,
    // the workout player (workout.rs)
    /// During a workout, the step running of all ("3/14"); otherwise today's planned workout, h:mm.
    Workout,
    /// The step's target range, green in it, orange under, red over (3 s power, heart rate, cadence).
    StepTarget,
    /// What is left of the step: time, km, or LAP.
    StepTime,
    /// The next step's target (its middle) and its length as a small tail ("150|0:30"; END on the last step).
    NextStep,
    // the course followed (course.rs)
    /// What is left of the course, km.
    ToGo,
    /// How far the course is, m (red once off course was said).
    OffCourse,
    /// The climb the rider is on (what is left, and the grade of the next 200 m: "0.8K 9%") or the next one (its
    /// length and average grade: "1.2K 6%"), coloured by grade.
    Climb,
    /// The climb's gain left, m (on it), or the next climb's gain.
    ClimbGain,
    /// The course's ascent left, m.
    UpToGo,
    /// The course's profile from about where the rider is to its end: the rider's place, the ridden part dark, the
    /// climbs coloured by grade.
    Profile,
    /// The workout's steps over its whole length, each as high as its target and in its zone colour; the rider's
    /// place, the part done dark.
    WorkoutGraph,
    /// During a workout, the reading the step targets (power over the window chosen in the menu, or heart rate or
    /// cadence) big, a gauge around the target band with a marker for the reading, and the band in numbers;
    /// otherwise the power alone.
    WktPower,
    /// What plays on the phone (AMS): the elapsed time of the track, m:ss (its title is on the media screen and in the
    /// banner when it changes).
    Music,
    // electronic shifting (sensors.rs)
    /// The gear, front-rear ("2-11"), from 1.
    Gear,
    /// Gear changes in this ride.
    Shifts,
    /// The shifting system's battery: percent (Di2), or its status (ANT+: NEW, GOOD, OK, LOW, CRIT).
    ShiftBattery,
    /// The course's next cue: its arrow and how far ("350M", "1.2K"); "--" off course (course/cues.rs).
    NextTurn,
    // the trainer (trainer.rs)
    /// The trainer's mode in force: ERG, RES or SIM.
    TrainerMode,
    /// Its target: watts (ERG), percent (RES) or grade (SIM).
    TrainerTarget,
    /// What the trainer is told now in ERG, watts, the rider's adjust included ("225|+15"); "--" outside ERG or without
    /// a trainer (the plan's band is [`Field::StepTarget`]).
    ErgTarget,
    /// The time left in the whole workout (the steps after the one running, and what is left of it).
    WorkoutLeft,
    /// On a climb of the followed course (or within 5 km of one), the power to ride now for the fastest climb with the
    /// W' left ([`crate::pacing`]), in the workout gauge: the 3 s power against a band around the target; "TOP" and the
    /// time to the top in its title.
    ClimbPace,
}

impl Field {
    /// Every field, in the order the page menu cycles through them; the index is kept in a byte
    /// ([`crate::PageCfg`]), so new ones go at the end.
    pub const ALL: [Field; 65] = {
        use Field::*;
        [
            Time,
            Hr,
            Speed,
            Cadence,
            Power,
            Map,
            Timer,
            Dist,
            AvgSpeed,
            MaxSpeed,
            AvgHr,
            AvgPower,
            LapTime,
            LapDist,
            Ascent,
            Graph,
            Temp,
            HrGraph,
            Dfa,
            Rmssd, // power.rs and the ride's derived metrics:
            Power3s,
            Power10s,
            Power30s,
            Np,
            If,
            Tss,
            Kj,
            Kcal,
            Wkg,
            Wbal,
            MaxPower,
            MaxHr,
            AvgCadence,
            Balance,
            Vam,
            Grade,
            WbalGraph,
            DfaGraph,
            AltGraph,
            WeekKm,
            WeekH,
            YearKm,
            TotalKm, // the workout player:
            Workout,
            StepTarget,
            StepTime,
            NextStep, // the course followed:
            ToGo,
            OffCourse,
            Climb,
            ClimbGain,
            UpToGo,
            Profile, // the workout player again:
            WorkoutGraph,
            WktPower, // the phone:
            Music,    // electronic shifting:
            Gear,
            Shifts,
            ShiftBattery, // the course followed again:
            NextTurn,     // the trainer:
            TrainerMode,
            TrainerTarget,
            ErgTarget,
            WorkoutLeft,
            ClimbPace,
        ]
    };

    /// Its label, top left of its box.
    pub fn label(self) -> &'static str {
        const LABELS: [&str; Field::ALL.len()] = [
            "TIME",
            "HR",
            "KM/H",
            "RPM",
            "WATTS",
            "MAP",
            "TIMER",
            "KM",
            "AVG KM/H",
            "MAX KM/H",
            "AVG HR",
            "AVG W",
            "LAP TIME",
            "LAP KM",
            "ASCENT M",
            "W GRAPH",
            "TEMP C",
            "HR GRAPH",
            "DFA A1",
            "RMSSD",
            // power.rs and the ride's derived metrics
            "3S W",
            "10S W",
            "30S W",
            "NP",
            "IF",
            "TSS",
            "KJ",
            "KCAL",
            "W/KG",
            "W'BAL KJ",
            "MAX W",
            "MAX HR",
            "AVG RPM",
            "L/R",
            "VAM M/H",
            "GRADE %",
            "W' GRAPH",
            "DFA GRAPH",
            "ALT GRAPH",
            "WEEK KM",
            "WEEK H",
            "YEAR KM",
            "TOTAL KM",
            // the workout player
            "WORKOUT",
            "TARGET",
            "STEP LEFT",
            "NEXT",
            // the course followed
            "TO GO KM",
            "OFF CRS M",
            "CLIMB",
            "CLIMB M",
            "UP LEFT M",
            "PROFILE",
            "WKT GRAPH",
            "WKT POWER",
            "MUSIC",
            // electronic shifting
            "GEAR",
            "SHIFTS",
            "SHIFT BAT",
            "NEXT TURN",
            // the trainer
            "MODE",
            "TRG",
            "ERG W",
            "WKT LEFT",
            "TOP",
        ];
        LABELS[self as usize]
    }

    /// The widest value it shows (a time: without its small seconds): the tests draw it in every box of every layout
    /// (the scale follows the value, not this; one longer than [`VALUE_CHARS`] gets a smaller one).
    #[cfg(test)]
    fn widest(self) -> &'static str {
        const WIDEST: [&str; Field::ALL.len()] = [
            "88:88", "888", "88.8", "888", "8888", "", "88:88", "88.88", "88.8", "88.8", "888", "8888", "88:88",
            "88.88", "8888", "", "-88.8", "", "8.88", "888", // power.rs and the ride's derived metrics:
            "8888", "8888", "8888", "8888", "8.88", "888", "8888", "8888", "88.8", "-88.8", "8888", "888", "888",
            "88/88", "-8888", "-88.8", "", "", "", "8888", "888.8", "88888", "888888", // the workout player:
            "88/88", "888-888", "88:88", "888-888", // the course followed:
            "888.8", "8888", "8.8K 88%", "8888", "8888", "", "", "8888",  // the phone:
            "88:88", // electronic shifting:
            "8-88", "8888", "GOOD",   // the course followed again:
            "888.8K", // the trainer:
            "ERG", "-10.0%", // the widest target: a grade (a power is "2000W")
            "888|+88", "88:88|88", "", // the workout and ERG again, the climb pacer
        ];
        WIDEST[self as usize]
    }

    /// Its box holds a picture (map, graph), not a value.
    pub fn is_picture(self) -> bool {
        matches!(
            self,
            Field::Map
                | Field::Graph
                | Field::HrGraph
                | Field::WbalGraph
                | Field::DfaGraph
                | Field::AltGraph
                | Field::Profile
                | Field::WorkoutGraph
                | Field::WktPower
                | Field::ClimbPace
        )
    }

    /// A field of the workout player (the step, its target, what is left, the next step, the graph, the gauge).
    pub fn is_workout(self) -> bool {
        matches!(
            self,
            Field::Workout
                | Field::StepTarget
                | Field::StepTime
                | Field::NextStep
                | Field::WorkoutLeft
                | Field::WorkoutGraph
                | Field::WktPower
        )
    }

    /// Its group in [`FIELD_GROUPS`] (every field is in one: a test checks).
    pub fn group(self) -> usize {
        FIELD_GROUPS.iter().position(|(_, g)| g.contains(&self)).unwrap_or(0)
    }

    /// From its index in [`Field::ALL`] (anything else: time).
    pub fn from_index(i: u32) -> Field {
        Self::ALL.get(i as usize).copied().unwrap_or(Field::Time)
    }
}

/// The fields by kind, for the picker: a page's box is a group then a field, a few rows each, not 55 in a row.
pub const FIELD_GROUPS: [(&str, &[Field]); 11] = {
    use Field::*;
    [
        (
            "RIDE",
            &[Time, Timer, Speed, Dist, AvgSpeed, MaxSpeed, Cadence, AvgCadence, LapTime, LapDist, Kcal, Temp, Map],
        ),
        ("POWER", &[Power, Power3s, Power10s, Power30s, AvgPower, MaxPower, Np, If, Tss, Kj, Wkg, Wbal, Balance]),
        ("HEART", &[Hr, AvgHr, MaxHr, Dfa, Rmssd]),
        ("CLIMB", &[Ascent, Vam, Grade]),
        ("GRAPHS", &[Graph, HrGraph, WbalGraph, DfaGraph, AltGraph]),
        ("WORKOUT", &[Workout, StepTarget, StepTime, WorkoutLeft, NextStep, WorkoutGraph, WktPower]),
        ("COURSE", &[ToGo, NextTurn, OffCourse, Climb, ClimbGain, UpToGo, Profile, ClimbPace]),
        ("TOTALS", &[WeekKm, WeekH, YearKm, TotalKm]),
        ("PHONE", &[Music]),
        ("SHIFTING", &[Gear, Shifts, ShiftBattery]),
        ("TRAINER", &[TrainerMode, TrainerTarget, ErgTarget]),
    ]
};

/// A page layout: rows top to bottom, each (its share of the height, boxes side by side), and its name in the menu.
pub type Layout = (&'static [(u8, u8)], &'static str);

/// The layouts by the id a page keeps ([`crate::PageCfg::layout`]); 0 is a page switched off. Ids 1, 2, 3, 4 and 6
/// are the layouts of earlier builds, whose id was their field count, so pages kept then read the same.
pub const LAYOUTS: [Layout; 15] = [
    (&[], "OFF"),
    (&[(1, 1)], "1"),
    (&[(1, 1), (1, 1)], "2"),
    (&[(1, 1), (1, 2)], "1+2"),
    (&[(1, 2), (1, 2)], "2X2"),
    (&[(2, 1), (1, 2), (1, 2)], "1+2X2"),
    (&[(1, 2), (1, 2), (1, 2)], "2X3"),
    (&[(1, 2), (1, 1)], "2+1"),
    (&[(1, 1), (1, 1), (1, 1)], "3 ROWS"),
    (&[(2, 1), (1, 1), (1, 1)], "BIG+2"),
    (&[(2, 1), (1, 1), (1, 2)], "BIG+1+2"),
    (&[(1, 2), (1, 1), (1, 2)], "2+1+2"),
    (&[(2, 1), (1, 1), (1, 2), (1, 2)], "BIG+1+2X2"),
    // ids 13 and 14 are this build's (stock's pages of 7 and 8 fields); earlier builds refuse them, so the backup
    // registers never keep them (crate::page::Page::to_cfg)
    (&[(1, 1), (1, 2), (1, 2), (1, 2)], "1+2X3"),
    (&[(1, 2), (1, 2), (1, 2), (1, 2)], "2X4"),
];

/// The order the page menu goes through the layouts: by field count (none is off: a page is deleted instead).
pub const LAYOUT_ORDER: [u8; 14] = [1, 2, 8, 3, 7, 9, 4, 10, 5, 11, 6, 12, 13, 14];

/// Layout `id`, if there is one.
pub fn layout(id: u8) -> Option<Layout> {
    LAYOUTS.get(usize::from(id)).copied()
}

/// How many fields layout `id` shows.
///
/// ```
/// use bryton_rustfw::ui::field_count;
/// assert_eq!([0, 1, 4, 5, 6, 10, 99].map(field_count), [0, 1, 4, 5, 6, 4, 0]);
/// ```
pub fn field_count(id: u8) -> usize {
    layout(id).map_or(0, |(rows, _)| rows.iter().map(|&(_, n)| usize::from(n)).sum())
}

/// Top of the ride fields.
pub const RIDE_Y: u32 = PAGE_Y;
/// Height of the ride fields: everything under the strip.
pub const RIDE_H: u32 = H - RIDE_Y;

/// Field box `i` (x, y, w, h) of layout `id` in a field area `h` high ([`rows_box`]). None past the last.
///
/// ```
/// use bryton_rustfw::ui::{field_box, RIDE_Y};
/// assert_eq!(field_box(5, 300, 0), Some((0, RIDE_Y, 240, 150)), "big over 2 x 2: the big box");
/// assert_eq!(field_box(5, 300, 4), Some((120, RIDE_Y + 225, 120, 75)));
/// assert_eq!(field_box(5, 300, 5), None);
/// ```
pub fn field_box(id: u8, h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    rows_box(layout(id)?.0, h, i)
}

/// Box `i` of rows `rows` (top to bottom, each its share of the height and its boxes side by side) in a field area
/// `h` high, row by row, left to right; the last row and the last box of a row take what division leaves. None past
/// the last. The one geometry of the ride pages, preset or edited ([`crate::page::Page`]).
pub fn rows_box(rows: &[(u8, u8)], h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    let total: u32 = rows.iter().map(|&(share, _)| u32::from(share)).sum();
    let (mut y, mut k) = (0, i);
    for (r, &(share, n)) in rows.iter().enumerate() {
        let rh = if r + 1 == rows.len() { h - y } else { h * u32::from(share) / total };
        let n = usize::from(n);
        if k < n {
            let bw = W / n as u32;
            let w = if k + 1 == n { W - bw * k as u32 } else { bw };
            return Some((bw * k as u32, RIDE_Y + y, w, rh));
        }
        (y, k) = (y + rh, k - n);
    }
    None
}

/// Height of a field's label line.
const FIELD_LABEL_H: u32 = 22;

/// The characters a value box is sized for: "88.88", "12:34", "-88.8". A box's values all have one scale, so the
/// eye finds the same digit height in every box of a size (the UI review, 2026-10-25: a timer at 2x beside a
/// distance at 4x and a heart rate at 6x).
const VALUE_CHARS: u32 = 5;

/// A value's ink stays this far from its box's edges (and its rules): two values side by side are then 2 x 3 + 1 px
/// apart, more than twice the gap between two characters of one value at 3x (UI review 2, P2: at 2 px, "12:00" and
/// "5.76" in a row read "12:005.76"). A half-width box holds five characters at 3x, no longer 4x.
const VALUE_MARGIN: u32 = 3;

/// The smallest value box an edit may leave (w, h): five characters at 3x under the label (UI review 2, 5.2 point 2;
/// at 2x a value is no longer read at a glance).
pub const VALUE_MIN: (u32, u32) =
    (ink_cells(VALUE_CHARS, 3) + 2 * VALUE_MARGIN, PIXEL.height as u32 * 3 + FIELD_LABEL_H + 1);

/// In a field's value, what follows this is drawn small and grey after the rest: the seconds of a time over an
/// hour ("1:23|45"), so the hours and minutes keep the box's size.
pub const SMALL_TAIL: char = '|';

/// The small tail's style ([`SMALL_TAIL`]).
const TAIL: Style = Style { fg: GREY, ..LABEL };

/// The width of `s` in the pixel font less its last cell's blank column: its ink for laying out, the same for every
/// text of a length (so a value's characters keep their cells as they change).
fn ink(style: &Style, s: &str) -> u32 {
    style.width(s).saturating_sub(if s.is_empty() { 0 } else { style.scale })
}

/// The ink width of `n` cells of the pixel font at `scale` (the last cell's blank column left out).
const fn ink_cells(n: u32, scale: u32) -> u32 {
    (n * CELL - 1) * scale
}

/// The value scale of a box `w` x `h`: the largest at which five characters ("88.88", "12:34") fit its width and the
/// height under the label; 2 at least.
///
/// ```
/// use bryton_rustfw::ui::{box_scale, field_box, RIDE_H};
/// let half = field_box(6, RIDE_H, 0).unwrap(); // 2 x 3: 120 x 98
/// assert_eq!((box_scale(half.2, half.3), box_scale(240, 296)), (3, 8));
/// ```
pub fn box_scale(w: u32, h: u32) -> u32 {
    let fits =
        |s: u32| ink_cells(VALUE_CHARS, s) + 2 * VALUE_MARGIN <= w && PIXEL.height as u32 * s + FIELD_LABEL_H < h;
    (2..=8).rev().find(|&s| fits(s)).unwrap_or(2)
}

/// The scale of a value whose big part is `big` in a box `w` x `h`: the box's ([`box_scale`]), or, for a value
/// longer than [`VALUE_CHARS`] that does not fit it ("1.6K 6%", "200-220"), the largest under it at which it does.
/// By its length only, so a value that keeps its length keeps its scale and its characters their cells.
fn value_scale(big: &str, w: u32, h: u32) -> u32 {
    let n = big.len() as u32;
    let mut scale = box_scale(w, h);
    while scale > 1 && ink_cells(n.max(1), scale) + 2 * VALUE_MARGIN > w {
        scale -= 1;
    }
    scale
}

/// Where a value goes in box `bx`: its style, the big part's left edge and top, the big part and the small tail
/// (empty when there is none or it would not fit: the tail goes first), and the value band (left, right).
type ValueAt<'a> = (Style, u32, u32, &'a str, &'a str, (u32, u32));

fn value_at<'a>((x, y, w, h): (u32, u32, u32, u32), value: &'a str, fg: u16) -> ValueAt<'a> {
    let (mut big, tail) = value.split_once(SMALL_TAIL).unwrap_or((value, ""));
    let style = Style { fg, scale: value_scale(big, w, h), ..LABEL };
    let top = y + FIELD_LABEL_H + h.saturating_sub(1 + FIELD_LABEL_H).saturating_sub(style.height()) / 2;
    let band = (x + 1, x + w - 1);
    let room = band.1 - band.0;
    while ink(&style, big) > room {
        big = big.get(..big.len().saturating_sub(1)).unwrap_or(""); // too long even at 1x: cut, whole characters
    }
    let with_tail = style.width(big) + ink(&TAIL, tail);
    let (tail, ink) = if tail.is_empty() || with_tail > room { ("", ink(&style, big)) } else { (tail, with_tail) };
    (style, band.0 + room.saturating_sub(ink) / 2, top, big, tail, band)
}

/// Draws `s` in `style` from `x`, character by character, only those that differ from `old` (all: None), each in its
/// cell, nothing at or right of `right`.
fn draw_cells(c: &mut impl Canvas, style: &Style, x: u32, y: u32, right: u32, s: &str, old: Option<&str>) {
    let cell = CELL * style.scale;
    for (i, ch) in s.char_indices() {
        let cx = x + i as u32 * cell;
        match old.map(|o| o.get(i..).and_then(|t| t.chars().next())) {
            _ if cx >= right => {}
            None => {
                text::draw_clipped(c, style, cx, y, right, s.get(i..i + ch.len_utf8()).unwrap_or(" "));
            }
            Some(Some(was)) if was == ch => {}
            Some(was) => text::redraw_char(c, style, (cx, y, right), was.unwrap_or(' '), ch),
        }
    }
}

/// Height of a zone bar along the top of a box (above the label, a pixel apart from it: the labels of the value fields
/// start 9 px down). 8 px: 4 was too thin to see on the bike (owner, 2026-10-26).
pub const ZONE_BAR_H: u32 = 8;

/// The zone bar of box `bx` in `color` (BLACK clears it): the box's full width less its right rule.
pub fn draw_zone_bar(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), color: u16) {
    c.fill(x, y, w - u32::from(x + w < W), ZONE_BAR_H, color);
}

/// A field's frame with its zone bar (`bar`; BLACK: none), for the plain power and heart rate fields.
pub fn draw_field_frame_bar(c: &mut impl Canvas, bx: (u32, u32, u32, u32), field: Field, bar: u16) {
    draw_field_frame(c, bx, field);
    if bar != BLACK {
        draw_zone_bar(c, bx, bar);
    }
}

/// A field's frame: the lines between boxes (right and bottom edges, inside the box) and its label (none on a map).
pub fn draw_field_frame(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field) {
    if x + w < W {
        c.fill(x + w - 1, y, 1, h, GREY);
    }
    if y + h < H - 2 {
        c.fill(x, y + h - 1, w, 1, GREY);
    }
    if !field.is_picture() {
        text::draw(c, &Style { fg: GREY, ..LABEL }, x + 6, y + ZONE_BAR_H + 1, Align::Left, field.label());
    }
}

/// Width of the page editor's frame round the selected cell (UI review 2, 5.2 point 5).
pub const FRAME: u32 = 3;

/// The inside of a field box: without its rules (right and bottom edges, [`draw_field_frame`]).
pub fn box_inside((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x, y, w - u32::from(x + w < W), h - u32::from(y + h < H - 2))
}

/// The page editor's frame round box `bx`, inside its rules, in `color`.
pub fn draw_cell_frame(c: &mut impl Canvas, bx: (u32, u32, u32, u32), color: u16) {
    let (x, y, w, h) = box_inside(bx);
    c.fill(x, y, w, FRAME, color);
    c.fill(x, y + h - FRAME, w, FRAME, color);
    c.fill(x, y + FRAME, FRAME, h - 2 * FRAME, color);
    c.fill(x + w - FRAME, y + FRAME, FRAME, h - 2 * FRAME, color);
}

/// The page editor's hint in the strip's place, over its orange line: what the keys do.
pub fn draw_edit_hint(c: &mut impl Canvas, text: &str) {
    c.fill(0, 0, W, STRIP_H, BLACK);
    text::draw_in(c, &Style { fg: ACCENT, ..LABEL }, 0, STRIP_H - LABEL.height(), W, Align::Centre, text);
    c.fill(0, STRIP_H, W, 2, ACCENT);
}

/// The strip whole again after the hint ([`draw_edit_hint`]): its background first, which [`draw_strip`] leaves.
pub fn draw_strip_whole(c: &mut impl Canvas, s: &Strip) {
    c.fill(0, 0, W, STRIP_H, BLACK);
    draw_strip(c, s);
}

/// A split's icon at the right of menu row `i` (the editor's SPLIT rows, no dash in the text): a box 24 x 16 cut
/// side by side (`stacked` false) or one over the other, in the row's ink.
pub fn draw_split_icon(c: &mut impl Canvas, i: usize, stacked: bool, selected: bool) {
    let (iw, ih) = (24, 16);
    let ink = if selected { BLACK } else { WHITE };
    let x = NEARBY.0 + NEARBY.2 - iw;
    let y = LIST_Y + i as u32 * MENU_ROW + (MENU_ROW - 1 - ih) / 2;
    c.fill(x, y, iw, 2, ink);
    c.fill(x, y + ih - 2, iw, 2, ink);
    c.fill(x, y, 2, ih, ink);
    c.fill(x + iw - 2, y, 2, ih, ink);
    if stacked {
        c.fill(x, y + ih / 2 - 1, iw, 2, ink);
    } else {
        c.fill(x + iw / 2 - 1, y, 2, ih, ink);
    }
}

/// A field's value in `fg`, centred by its ink under its label, in a fixed band (what was there before is cleared):
/// at the box's scale, a [`SMALL_TAIL`] small and grey after it, bottoms aligned.
pub fn draw_field_value(c: &mut impl Canvas, bx: (u32, u32, u32, u32), value: &str, fg: u16) {
    let (style, left, top, big, tail, (l, r)) = value_at(bx, value, fg);
    let bh = style.height();
    c.fill(l, top, left.saturating_sub(l), bh, BLACK);
    let mut end = text::draw_clipped(c, &style, left, top, r, big);
    if !tail.is_empty() {
        // the ink's bottom rows level: 7 font rows of each
        let ty = top + 7 * style.scale - 7 * TAIL.scale;
        let tw = TAIL.width(tail).min(r.saturating_sub(end));
        c.fill(end, top, tw, ty - top, BLACK);
        let t_end = text::draw_clipped(c, &TAIL, end, ty, r, tail);
        c.fill(end, ty + TAIL.height(), tw, (top + bh).saturating_sub(ty + TAIL.height()), BLACK);
        end = t_end;
    }
    c.fill(end, top, r.saturating_sub(end), bh, BLACK);
}

/// A field's value that was `old`: only the characters that changed when the length and the place of the small tail
/// are the same (they keep their cells), else the whole value band, and the rows of the old one's band outside it
/// (a value drawn smaller: "OPEN" then "200-220", 2026-10-25 review).
pub fn draw_field_value_changes(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: &str, new: &str, fg: u16) {
    let (style, left, top, big, tail, (l, r)) = value_at(bx, new, fg);
    let (old_style, _, old_top, old_big, old_tail, _) = value_at(bx, old, fg);
    if old_big.len() != big.len() || old_tail.len() != tail.len() || old.is_empty() {
        let (end, old_end) = (top + style.height(), old_top + old_style.height());
        c.fill(l, old_top, r - l, top.saturating_sub(old_top), BLACK);
        c.fill(l, end, r - l, old_end.saturating_sub(end), BLACK);
        return draw_field_value(c, bx, new, fg);
    }
    draw_cells(c, &style, left, top, r, big, Some(old_big));
    let ty = top + 7 * style.scale - 7 * TAIL.scale;
    draw_cells(c, &TAIL, left + style.width(big), ty, r, tail, Some(old_tail));
}

/// A graph's axis text and figures: the label font (2x), grey; no 1x text where a rider reads (UI review).
const AXIS: Style = Style { fg: GREY, ..LABEL };
/// Room left of the plot for its scale (four characters: "0.75", "2400"), and under it for the time.
const AXIS_W: u32 = 4 * CELL * 2 + 3;
const AXIS_H: u32 = 22;
/// The label lines of a graph with axes: the title and a figure, then the second figure.
const GRAPH_HEAD_H: u32 = 2 * 20 + 4;

/// A graph box this big has its axes and figures drawn (smaller ones show the title and the bars alone): the full
/// width, so the 2x scale leaves the plot most of the box.
fn has_axes(w: u32, h: u32) -> bool {
    w >= 200 && h >= 120
}

/// The graph's plot in field box `bx`: left, top, columns, height. One bar per pixel column, side by side, so the
/// bars make a filled area as stock's graph does; the newest bar is the rightmost column. Room is left for the
/// axes ([`draw_graph_axes`]) and the figures ([`draw_graph_stats`]) when the box has it.
pub fn graph_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let (left, head, below) = if has_axes(w, h) { (AXIS_W, GRAPH_HEAD_H, AXIS_H) } else { (0, FIELD_LABEL_H, 0) };
    (x + 4 + left, y + head, w.saturating_sub(8 + left), h.saturating_sub(head + 4 + below))
}

/// What a graph plots, in its label line (`title`: "POWER W", "HR"), and, when the box has room, its axes: the
/// values at the bottom, the middle and the top of the plot (`labels`) on its left, and the time it spans under it,
/// from `from` ("-7 MIN", "START") on the left to "NOW" on the right. Drawn with the box; nothing here changes as the
/// bars move.
pub fn draw_graph_axes(c: &mut impl Canvas, bx: (u32, u32, u32, u32), title: &str, labels: [&str; 3], from: &str) {
    let (x, y, w, h) = bx;
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, w - 12, Align::Left, title);
    if !has_axes(w, h) {
        return;
    }
    let a = graph_area(bx);
    let yw = AXIS_W - 3;
    let [low, mid, high] = labels;
    text::draw_in(c, &AXIS, x + 2, a.1, yw, Align::Right, high);
    text::draw_in(c, &AXIS, x + 2, a.1 + a.3 / 2 - AXIS.height() / 2, yw, Align::Right, mid);
    text::draw_in(c, &AXIS, x + 2, a.1 + a.3 - AXIS.height(), yw, Align::Right, low);
    let ty = a.1 + a.3 + 2;
    text::draw_in(c, &AXIS, a.0, ty, a.2 / 2, Align::Left, from);
    text::draw_in(c, &AXIS, a.0 + a.2 / 2, ty, a.2 - a.2 / 2, Align::Right, "NOW");
}

/// Room at a graph's top right for its figures ([`draw_graph_stats`]): "AVG 1234".
const STATS_W: u32 = 8 * CELL * 2;

/// Two figures of the ride (`lines`: what each was, "" when not drawn yet, and is: "AVG 182", "NP 195") at the right
/// of a graph's label lines, one above the other, each in its own box (None: left as drawn), only its changed
/// characters when its length stays; only in a box with axes, which has the two lines.
pub fn draw_graph_stats(
    c: &mut impl Canvas,
    (x, y, w, h): (u32, u32, u32, u32),
    title: &str,
    lines: [Option<(&str, &str)>; 2],
) {
    if !has_axes(w, h) || LABEL.width(title) + STATS_W + 16 > w {
        return;
    }
    let style = Style { fg: WHITE, ..AXIS };
    for (k, (was, l)) in (0..).zip(lines).filter_map(|(k, l)| Some((k, l?))) {
        let (right, ly) = (x + w - 6, y + 4 + k * AXIS.height());
        if was.len() == l.len() && style.width(l) <= STATS_W {
            draw_changed_chars(c, &style, right - style.width(l), ly, was, l);
        } else {
            text::draw_in(c, &style, right - STATS_W, ly, STATS_W, Align::Right, l);
        }
    }
}

/// A plot without axes (the course profile) in field box `bx`: left, top, columns, height.
pub fn plot_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x + 4, y + FIELD_LABEL_H, w.saturating_sub(8), h.saturating_sub(FIELD_LABEL_H + 4))
}

/// A plot's title in its label line.
pub fn draw_plot_title(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), title: &str) {
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, w.saturating_sub(12), Align::Left, title);
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
// The workout gauge field: the reading big, a bar around the step's target band with a marker at the reading, the
// band in numbers under it
// ---------------------------------------------------------------------------------------------------------------

/// The marker's white core, columns, centred on the reading's column; a black column each side outlines it.
const MARK_W: u32 = 3;
/// Rows of the bar above and below the track at most, where the marker reaches out of it.
const OVERSHOOT: u32 = 4;
/// Rows of the white outline round the target band, and columns at its two edges.
const BRACKET: u32 = 2;

/// A gauge's bar: the target band's first and last columns, the marker's column and the colour of its caps (None: no
/// reading), and the zones behind the track (each one's first column and colour, [`crate::workout::gauge_zones`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GaugeBar {
    /// The band's first and last columns.
    pub band: (u32, u32),
    /// The marker's column and the colour of its ends.
    pub marker: Option<(u32, u16)>,
    /// The zones: first column and colour, in order; `u32::MAX` for one off the scale.
    pub zones: [(u32, u16); 7],
}

/// A gauge bar's place: left, top, columns, height.
type GaugeArea = (u32, u32, u32, u32);

/// The gauge field's parts in box `bx`: the bar (left, top, columns, height), the number's top and style, the
/// target line's top and style (none in a box under 100 high: its 2x text would leave the number no room; the band shows
/// on the bar). Fixed for a box, so the number and the marker are redrawn in place. The track is three times what it was
/// (6 to 12 rows), as thick as the box leaves the number room for 2x text.
fn gauge_parts(bx: (u32, u32, u32, u32)) -> (GaugeArea, u32, Style, (u32, Style)) {
    let (x, y, w, h) = bx;
    let small = Style { fg: GREY, ..LABEL };
    let target_y = (y + h).saturating_sub(2 + if h < 100 { 0 } else { small.height() }).max(y);
    let wanted = 3 * ((h / 8).clamp(12, 24) / 2) + 2 * OVERSHOOT;
    let room_2x = target_y.saturating_sub(y + FIELD_LABEL_H + 2 + LABEL.height() + 2 + 2);
    let gh = wanted.min(room_2x).max(12);
    let bar = (x + 8, target_y.saturating_sub(2 + gh).max(y), w.saturating_sub(16), gh);
    let top = y + FIELD_LABEL_H;
    let room = bar.1.saturating_sub(top + 2);
    let mut style = Style { scale: 1, ..LABEL };
    for scale in (1..=8).rev() {
        style.scale = scale;
        if style.width("8888") + 8 <= w && style.height() <= room {
            break;
        }
    }
    (bar, top + room.saturating_sub(style.height()) / 2, style, (target_y, small))
}

/// The columns a gauge in box `bx` has.
pub fn gauge_cols(bx: (u32, u32, u32, u32)) -> u32 {
    gauge_parts(bx).0.2
}

/// Pixel (`i`, `dy`) of a gauge bar `h` high: the marker (a white tick through the track with a black column each
/// side, its ends out of the track in the state's colour) over all; else black above and below the track, the target
/// band's white outline (its inside shows the zones), and the zone's colour.
fn gauge_px(g: &GaugeBar, h: u32, i: u32, dy: u32) -> u16 {
    let over = (h / 4).min(OVERSHOOT);
    let track = dy >= over && dy + over < h;
    if let Some((m, color)) = g.marker.filter(|&(m, _)| i + MARK_W / 2 + 1 >= m && i <= m + MARK_W / 2 + 1) {
        let core = i + MARK_W / 2 >= m && i <= m + MARK_W / 2;
        return match (core, track) {
            (true, true) => WHITE,
            (true, false) => color,
            _ => BLACK,
        };
    }
    if !track {
        return BLACK;
    }
    let (a, b) = g.band;
    let edge = dy < over + BRACKET || dy + over + BRACKET >= h;
    if (a..=b).contains(&i) && (edge || i < a + BRACKET || i + BRACKET > b) {
        return WHITE;
    }
    g.zones.iter().rev().find(|z| z.0 <= i).map_or(DARK, |z| z.1)
}

/// Columns `from` to `to` (included) of the gauge bar `bar` showing `g`.
fn draw_gauge_columns(c: &mut impl Canvas, bar: (u32, u32, u32, u32), g: &GaugeBar, from: u32, to: u32) {
    let to = to.min(bar.2.saturating_sub(1));
    if from <= to {
        c.blit(bar.0 + from, bar.1, to - from + 1, bar.3, &mut |i, dy| gauge_px(g, bar.3, from + i, dy));
    }
}

/// A gauge's title row: `title` at the left in the label style, and `tag` (the pacer's target, big and green) at the
/// right end when there is one, then the title is at 1x to leave it room.
pub fn draw_gauge_title(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), title: &str, tag: &str) {
    let big = Style { fg: GREEN, ..LABEL };
    let tag_w = if tag.is_empty() { 0 } else { big.width(tag) };
    let room = w.saturating_sub(12 + if tag_w > 0 { tag_w + 6 } else { 0 });
    let (style, dy) = gauge_title_style(tag);
    if tag_w > 0 {
        c.fill(x + 6, y + 4, room, LABEL.height(), BLACK); // the 1x title is shorter than what the 2x one left
    }
    text::draw_in(c, &style, x + 6, y + dy, room, Align::Left, title);
    if tag_w > 0 {
        c.fill(x + 6 + room, y + 4, w.saturating_sub(12 + room), LABEL.height(), BLACK); // the gap and the tag's box
        text::draw_in(c, &big, (x + w).saturating_sub(6 + tag_w), y + 4, tag_w, Align::Left, tag);
    }
}

/// The title's style and its offset in the title row: the label style, or at 1x (centred in the row) beside a tag.
fn gauge_title_style(tag: &str) -> (Style, u32) {
    let small = Style { scale: 1, fg: GREY, ..LABEL };
    if tag.is_empty() {
        (Style { fg: GREY, ..LABEL }, 4)
    } else {
        (Style { ..small }, 4 + (LABEL.height() - small.height()) / 2)
    }
}

/// A gauge's title that was `old` now `new` (the tag as it was): only the characters that changed when the length is the
/// same, else the whole row.
pub fn update_gauge_title(c: &mut impl Canvas, bx: (u32, u32, u32, u32), (old, new): (&str, &str), tag: &str) {
    if old.len() == new.len() {
        let (style, dy) = gauge_title_style(tag);
        draw_changed_chars(c, &style, bx.0 + 6, bx.1 + dy, old, new);
    } else {
        draw_gauge_title(c, bx, new, tag);
    }
}

/// The gauge field whole in box `bx`: `title` (and `tag`) in its label line, `value` big in `fg`, the bar `g` (none:
/// the bar's place black) and `target` under it.
pub fn draw_gauge(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    (title, tag, target): (&str, &str, &str),
    value: &str,
    fg: u16,
    g: Option<GaugeBar>,
) {
    let (x, _, w, h) = bx;
    let (bar, _, _, (target_y, small)) = gauge_parts(bx);
    draw_gauge_title(c, bx, title, tag);
    draw_gauge_value(c, bx, "", value, fg);
    match g {
        Some(g) => draw_gauge_columns(c, bar, &g, 0, bar.2),
        None => c.fill(bar.0, bar.1, bar.2, bar.3, BLACK),
    }
    if h >= 100 {
        text::draw_in(c, &small, x + 1, target_y, w.saturating_sub(2), Align::Centre, target);
    }
}

/// The gauge's number that was `old` ("": not drawn) now `new` in `fg`: only the characters that changed when the
/// length is the same, else its whole box.
pub fn draw_gauge_value(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: &str, new: &str, fg: u16) {
    let (x, _, w, _) = bx;
    let (_, top, style, _) = gauge_parts(bx);
    let style = Style { fg, ..style };
    if old.len() == new.len() && style.width(new) + 2 <= w {
        draw_changed_chars(c, &style, x + 1 + (w - 2 - style.width(new)) / 2, top, old, new);
    } else {
        text::draw_in(c, &style, x + 1, top, w.saturating_sub(2), Align::Centre, new);
    }
}

/// The gauge's marker moved from `old` to where `g` has it: only the old marker's columns and the new one's.
pub fn draw_gauge_marker(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: Option<(u32, u16)>, g: GaugeBar) {
    let bar = gauge_parts(bx).0;
    for (m, _) in [old, g.marker].into_iter().flatten() {
        draw_gauge_columns(c, bar, &g, m.saturating_sub(MARK_W / 2 + 1), m + MARK_W / 2 + 1);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The overview (the main screen): the date, the TODAY card, four fields of the user's choice, the notices
// ---------------------------------------------------------------------------------------------------------------

/// The date: the pixel font at 3x, one size up from the labels, white. The time is in the strip.
pub const DATE: Style = Style { font: &PIXEL, scale: 3, fg: WHITE, bg: BLACK };
/// Top of the date row.
pub const DATE_Y: u32 = PAGE_Y + 1;
/// Height of the date row: the date's text box.
pub const DATE_H: u32 = PIXEL.height as u32 * DATE.scale;
/// The TODAY card (left, top, width, height): a tap anywhere in it opens the calendar. Its top 4 rows are also in
/// the cog's touch box, where the cog wins. It takes all the height the fields leave (owner, 2026-10-26: a taller
/// profile).
pub const TODAY_BOX: (u32, u32, u32, u32) = (4, DATE_Y + DATE_H + 2, W - 8, OVERVIEW_Y - 2 - (DATE_Y + DATE_H + 2));
/// The overview's fields: top and height (a 2 x 2 grid of [`field_box`] boxes), right above the notices.
pub const OVERVIEW_Y: u32 = NOTICES_Y - 4 - OVERVIEW_H;
/// See [`OVERVIEW_Y`]: two rows of 60 px, the 4x digits clear of their label.
pub const OVERVIEW_H: u32 = 120;
/// Top of the notices: [`NOTICES`] rows of [`LIST_ROW`] at the screen's foot.
pub const NOTICES_Y: u32 = H - 8 - NOTICES as u32 * LIST_ROW;
/// Notices shown at most: the newest (a tap opens them all); one, so the TODAY card has the height (owner, 2026-10-26).
pub const NOTICES: usize = 1;

/// Field box `i` of the overview's four.
pub fn overview_box(i: usize) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = field_box(4, OVERVIEW_H, i)?;
    Some((x, y - RIDE_Y + OVERVIEW_Y, w, h))
}

/// The date row: the date centred in the row left of the cog (the screen's centre would run a 10-character date at
/// 3x into it), white, the cog right.
pub fn draw_date(c: &mut impl Canvas, date: &str) {
    text::draw_in(c, &DATE, 4, DATE_Y, GEAR_AT.0 - 4 - 4, Align::Centre, date);
    draw_cog(c); // after the date: its box ends the row
}

/// The longest start of `s` that fits `w` in `style`, whole characters.
fn fit<'a>(style: &Style, s: &'a str, w: u32) -> &'a str {
    let end =
        s.char_indices().map(|(i, ch)| i + ch.len_utf8()).take_while(|&e| style.width(s.get(..e).unwrap_or("")) <= w);
    s.get(..end.last().unwrap_or(0)).unwrap_or("")
}

/// Bars side by side from `x`, `colw` px each, on the baseline `y + h`: a bar is a byte of a workout's profile
/// ([`unpack_bar`]: a height in 1/[`PROF_H`] of `h`, a colour), 0 for none; neighbours alike are one fill. The area
/// is cleared before.
pub fn draw_profile_bars(c: &mut impl Canvas, (x, y, h): (u32, u32, u32), colw: u32, bars: &[u8]) {
    let mut i = 0;
    while let Some(&bar) = bars.get(i) {
        let mut j = i + 1;
        while bars.get(j) == Some(&bar) {
            j += 1;
        }
        let (level, color) = unpack_bar(bar);
        if level > 0 {
            let bh = (u32::from(level) * h / PROF_H).clamp(1, h);
            c.fill(x + i as u32 * colw, y + h - bh, (j - i) as u32 * colw, bh, color);
        }
        i = j;
    }
}

/// What the TODAY card shows.
#[derive(Clone, Copy)]
pub enum Today<'a> {
    /// The calendar is not read yet (it is, a few seconds after the start): the title alone.
    Pending,
    /// Nothing planned: REST DAY.
    Rest,
    /// A workout: its name, its length ("1:30") and its profile ([`draw_profile_bars`]).
    Planned(&'a str, &'a str, &'a [u8]),
}

/// The TODAY card whole:"TODAY" and the length (H:MM) on the first line, the workout's name cut to the width on
/// the second, its profile (a bar per column in its zone's colour, [`draw_profile_bars`]) under them; None: REST DAY in grey.
/// Drawn once, when the overview opens and when the day or the calendar changes.
pub fn draw_today(c: &mut impl Canvas, card: Today) {
    let (x, y, w, h) = TODAY_BOX;
    c.fill(x, y, w, h, BLACK);
    for (rx, ry, rw, rh) in [(x, y, w, 1), (x, y + h - 1, w, 1), (x, y, 1, h), (x + w - 1, y, 1, h)] {
        c.fill(rx, ry, rw, rh, DARK);
    }
    let (tx, tw) = (x + 6, w - 12);
    let grey = Style { fg: GREY, ..LABEL };
    text::draw(c, &grey, tx, y + 3, Align::Left, "TODAY");
    match card {
        Today::Planned(name, length, prof) => {
            text::draw(c, &LABEL, x + w - 6, y + 3, Align::Right, length);
            text::draw(c, &LABEL, tx, y + 23, Align::Left, fit(&LABEL, name, tw));
            draw_profile_bars(c, (x + 4, y + 46, h - 51), (w - 8) / prof.len().max(1) as u32, prof);
        }
        Today::Rest => {
            text::draw(c, &grey, tx, y + 23, Align::Left, "REST DAY");
        }
        Today::Pending => {}
    }
}

/// A WORKOUTS list row in slot `slot`: the day over the length on the left, the workout's profile on the right
/// ([`draw_profile_bars`], 2 px a column, on black even when the row is selected).
pub fn draw_workout_row(c: &mut impl Canvas, slot: usize, (day, length): (&str, &str), selected: bool, prof: &[u8]) {
    let y = LIST_Y + slot as u32 * MENU_ROW;
    let (bg, fg) = if selected { (ACCENT, BLACK) } else { (BLACK, WHITE) };
    c.fill(0, y, W, MENU_ROW - 1, bg);
    c.fill(0, y + MENU_ROW - 1, W, 1, DARK);
    let style = Style { fg, bg, ..LABEL };
    text::draw(c, &style, NEARBY.0, y + 1, Align::Left, day);
    text::draw(c, &style, NEARBY.0, y + 21, Align::Left, length);
    let (bx, bw) = (W - 8 - 2 * prof.len() as u32 - 4, 2 * prof.len() as u32 + 4);
    c.fill(bx, y + 3, bw, MENU_ROW - 7, BLACK);
    draw_profile_bars(c, (bx + 2, y + 5, MENU_ROW - 11), 2, prof);
}

/// The menu button on the overview: a cog, [`COG`] px square.
pub const COG: u32 = 25;
/// Where the cog is drawn: 6 px from the right edge, in the date's row, level with the date.
pub const GEAR_AT: (u32, u32) = (W - 6 - COG, DATE_Y + (DATE_H - COG) / 2);
/// The cog's centre, the middle of its touch box.
pub const GEAR_C: (u32, u32) = (GEAR_AT.0 + COG / 2, GEAR_AT.1 + COG / 2);
/// Where a tap opens the menu: 44 x 44 px round the cog (the menu rows' rule), over the strip's bottom rows and the
/// TODAY card's top right corner, where the cog wins.
pub const GEAR_TAP: (u32, u32, u32, u32) = (W - 44, GEAR_C.1 - 22, 44, 44);

/// Coverage (0 to 9, nine samples) of the cog's metal at pixel (`i`, `j`) of its [`COG`] square, symmetric: a ring
/// (two thirds of the radius) with a round hole (3/10) and eight square teeth (7/20 of the radius wide) at the compass points, each sample taken in sixths of a pixel.
fn cog_coverage(i: u32, j: u32) -> u32 {
    const C: i32 = COG as i32 * 3; // the centre, in sixths
    const ROOT: i32 = C * 2 / 3; // the ring's outside, a third of the way in from the teeth's tips
    const HOLE: i32 = C * 3 / 10;
    const TOOTH: i32 = C - 3; // the tips, half a pixel from the edge
    const HALF: i32 = C * 7 / 40; // half a tooth's width
    let mut n = 0;
    for sy in 0..3 {
        for sx in 0..3 {
            let (dx, dy) = ((i * 6 + 1 + 2 * sx) as i32 - C, (j * 6 + 1 + 2 * sy) as i32 - C);
            let (u, v) = ((dx - dy).abs() * 5, (dx + dy).abs() * 5); // the diagonals' axes, times 5 / sqrt 2 ~ 7 / 2
            let tooth = (dy.abs() <= HALF && dx.abs() <= TOOTH)
                || (dx.abs() <= HALF && dy.abs() <= TOOTH)
                || (u <= HALF * 7 && v <= TOOTH * 7)
                || (v <= HALF * 7 && u <= TOOTH * 7);
            let r2 = dx * dx + dy * dy;
            n += u32::from((r2 <= ROOT * ROOT || tooth) && r2 > HOLE * HOLE);
        }
    }
    n
}

/// RGB565 `color` at `n` ninths over black.
fn dim(color: u16, n: u32) -> u16 {
    let ch = |shift: u32, max: u32| ((u32::from(color) >> shift & max) * n / 9) << shift;
    (ch(11, 31) | ch(5, 63) | ch(0, 31)) as u16
}

/// The cog at [`GEAR_AT`], anti-aliased over black.
fn draw_cog(c: &mut impl Canvas) {
    c.blit(GEAR_AT.0, GEAR_AT.1, COG, COG, &mut |i, j| dim(0xc618, cog_coverage(i, j)));
}

/// The notices, newest first; with none, the area stays black.
pub fn draw_notices(c: &mut impl Canvas, notices: &[&str]) {
    c.fill(0, NOTICES_Y, W, H - NOTICES_Y, BLACK);
    if notices.is_empty() {
        return; // nothing to say: nothing drawn (Ferris is in ABOUT)
    }
    for (r, n) in notices.iter().take(NOTICES).enumerate() {
        // "19:36 A CALLER": the time small and grey, the text in the large font for every row (cut, not shrunk:
        // one row smaller than the other read as inconsistent on the unit)
        let style = Style { fg: if r == 0 { WHITE } else { GREY }, ..LABEL };
        let small = Style { fg: GREY, scale: 1, ..LABEL };
        let y = NOTICES_Y + 4 + r as u32 * LIST_ROW;
        let (time, text) = n.split_at(n.find(' ').map_or(0, |i| i + 1));
        let tw = small.width("00:00 ");
        text::draw_in(c, &small, 4, y + style.height() - small.height(), tw, Align::Left, time);
        text::draw_in(c, &style, 4 + tw, y, W - 8 - tw, Align::Left, text);
    }
}

/// Text lines the notification detail shows at once.
pub const NOTIF_LINES: usize = ((H - LIST_Y - LIST_ROW) / LIST_ROW) as usize;

/// Characters of the pixel font that fit a detail line.
pub fn notif_cols() -> usize {
    ((W - 8) / LABEL.width("W").max(1)) as usize
}

/// A notification in full: `app` in the title, `time` and a MORE cue under it, then `lines` (title lines first, in
/// the accent colour: the flag is true) from the first one shown, at most [`NOTIF_LINES`].
pub fn draw_notif<'a>(
    c: &mut impl Canvas,
    app: &str,
    time: &str,
    more: bool,
    lines: impl Iterator<Item = (&'a str, bool)>,
) {
    draw_title(c, app);
    let grey = Style { fg: GREY, ..LABEL };
    // the time and MORE each in a box of their own, so neither wipes the other
    let mw = grey.width("MORE");
    text::draw_in(c, &grey, 4, LIST_Y, W - 12 - mw, Align::Left, time);
    text::draw_in(c, &grey, W - 4 - mw, LIST_Y, mw, Align::Right, if more { "MORE" } else { "" });
    for (r, (line, head)) in lines.take(NOTIF_LINES).enumerate() {
        let style = Style { fg: if head { ACCENT } else { WHITE }, ..LABEL };
        text::draw_in(c, &style, 4, LIST_Y + LIST_ROW * (r as u32 + 1), W - 8, Align::Left, line);
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
    let (fg, bg) = if alarm { (WHITE, RED) } else { (BLACK, ACCENT) };
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
        let (bg, fg, key) = if i == 0 { (RED, WHITE, "BACK") } else { (GREEN, BLACK, "START") };
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
    END_BUTTONS.iter().zip(labels).position(|(&(bx, by, w, h), l)| {
        l.is_some() && (bx..bx + w).contains(&u32::from(x)) && (by..by + h).contains(&u32::from(y))
    })
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
        (true, _) => (ACCENT, BLACK),
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
    const BIG: u32 = 8; // Ferris 128 x 88
    draw_crab(c, (W - 16 * BIG) / 2, PAGE_Y + 16, BIG, RUST);
    let mut y = PAGE_Y + 16 + 11 * BIG + 20;
    let mut line = |style: &Style, s: &str| {
        text::draw_in(c, style, 0, y, W, Align::Centre, s);
        y += style.height() + 8;
    };
    line(&LABEL, "BRYTON-RUSTFW");
    let mut v = text::Line::<24>::new();
    v.push_str("V");
    v.push_str(version);
    line(&LABEL, v.as_str());
    line(&LABEL, describe);
    let grey = Style { fg: GREY, ..LABEL };
    line(&grey, date);
    line(&grey, "SAFE RUST");
}

// ---------------------------------------------------------------------------------------------------------------
// Menus: a title, then rows of a label and a value; the selected row in orange.
// ---------------------------------------------------------------------------------------------------------------

/// Menu row pitch: 44 px, about 8 mm on this panel (0.18 mm a pixel), a fingertip's size (Apple's 44 pt, Material's
/// 48 dp); 20 px rows (3.6 mm) let a gloved finger cover two or three (UI review, 2026-10-25).
pub const MENU_ROW: u32 = 44;

/// Rows a menu shows at most (6); a longer menu turns a screen of rows at a time.
pub const MENU_ROWS: usize = ((H - LIST_Y) / MENU_ROW) as usize;

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
/// (a long course name is cut before its length; every row of the tree fits whole, a test checks).
pub(crate) fn menu_label_w(label: &str, value: &str) -> u32 {
    let own = (label.len() as u32 * 2 + 1) * CELL / 2 * LABEL.scale;
    own.min(NEARBY.2.saturating_sub(LABEL.width(value)))
}

/// Menu row `i`: `label` on the left, `value` on the right in `value_color` (white unless it says a state), centred
/// in its [`MENU_ROW`], a dark line under it; selected, the row is orange with black text (10:1 against the orange,
/// where white was 2:1).
pub fn draw_menu_row(c: &mut impl Canvas, i: usize, label: &str, value: &str, value_color: u16, selected: bool) {
    let y = LIST_Y + i as u32 * MENU_ROW;
    // a row with no value and a grey one is a note (an empty list), not a choice
    let note = value.is_empty() && value_color == GREY;
    let (bg, fg) = if selected { (ACCENT, BLACK) } else { (BLACK, if note { GREY } else { WHITE }) };
    let name_w = menu_label_w(label, value);
    let ty = y + (MENU_ROW - 1 - LABEL.height()) / 2;
    c.fill(0, y, W, ty - y, bg);
    c.fill(0, ty, NEARBY.0, LABEL.height(), bg);
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0, ty, name_w, Align::Left, label);
    let fg = if selected { BLACK } else { value_color };
    text::draw_in(c, &Style { fg, bg, ..LABEL }, NEARBY.0 + name_w, ty, NEARBY.2 - name_w, Align::Right, value);
    c.fill(NEARBY.0 + NEARBY.2, ty, W - NEARBY.0 - NEARBY.2, LABEL.height(), bg);
    let below = ty + LABEL.height();
    c.fill(0, below, W, y + MENU_ROW - 1 - below, bg);
    c.fill(0, y + MENU_ROW - 1, W, 1, DARK);
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
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

    /// A framebuffer that panics on any fill off screen.
    pub struct Frame {
        pub px: Vec<u16>,
        fills: usize,
        painted: usize,
        /// Text in the smooth font ([`Canvas::smooth`]).
        pub smooth: bool,
    }

    impl Frame {
        pub fn new() -> Self {
            Frame { px: vec![BLACK; (W * H) as usize], fills: 0, painted: 0, smooth: false }
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

        fn smooth(&self) -> bool {
            self.smooth
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
    }

    /// Two values side by side never run together: the gap between their inks is at least twice the gap between
    /// two characters of one (one blank column of the font at the value's scale), in every layout.
    #[test]
    fn neighbouring_values_keep_apart() {
        for id in 1..LAYOUTS.len() as u8 {
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

    /// The ride boxes tile the field area exactly, every value fits its box, and Ferris and the top line stay
    /// clear (Frame panics on anything off screen).
    #[test]
    fn ride_layouts_tile_and_fit() {
        for h in [RIDE_H] {
            for n in 1..LAYOUTS.len() as u8 {
                let b: Vec<_> = (0..8).map_while(|i| field_box(n, h, i)).collect();
                assert_eq!(b.len(), field_count(n));
                assert!(b.len() <= crate::page::CELLS, "a page keeps eight fields");
                let area: u32 = b.iter().map(|&(_, _, w, h)| w * h).sum();
                assert!(area <= W * h && area + W * 2 >= W * h, "{n} fields cover the area");
                let mut f = Frame::new();
                for (&bx, field) in b.iter().flat_map(|bx| Field::ALL.iter().map(move |f| (bx, f))) {
                    draw_field_frame(&mut f, bx, *field);
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
        let rides = (1..LAYOUTS.len() as u8)
            .flat_map(|n| (0..6).filter_map(move |i| field_box(n, RIDE_H, i)))
            .filter(|b| b.3 >= 64);
        // the overview's boxes are 60 px since the TODAY card grew: the same rule as a ride page's small boxes
        for bx in rides.chain((0..4).filter_map(overview_box).filter(|b| b.3 >= 64)) {
            let cols = gauge_cols(bx);
            let band = (cols / 3, cols * 2 / 3);
            let titles = ("POWER 30S", "", "200-220");
            let (mut a, mut b) = (Frame::new(), Frame::new());
            let zones = crate::workout::gauge_zones(crate::storage::workout::Kind::Power, (200, 220), cols, 200, 190);
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
        let boxes: Vec<_> = (1..LAYOUTS.len() as u8)
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
