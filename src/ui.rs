//! What is on screen: colours, text styles, Ferris, the layout and the clock. Draws
//! through [`Canvas`] only, so it knows nothing about the panel protocol (panel.rs) or the board.

pub mod card;
pub mod fonts;
pub mod map;
pub mod media;
pub mod menu;
pub mod text;
pub mod tiles;

use crate::fonts::PIXEL;
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

/// A climb's colour by its grade (% x 10), in the bands climbing apps use: green under 6 %, yellow under 9 %,
/// orange under 12 %, red above (Garmin's ClimbPro colours its climbs green to red the same way; the bands here are
/// this firmware's).
///
/// ```
/// use bryton_rustfw::ui::{grade_color, GREEN, RED};
/// assert_eq!([grade_color(45), grade_color(150)], [GREEN, RED]);
/// ```
pub fn grade_color(x10: i32) -> u16 {
    match x10 {
        ..60 => GREEN,
        60..90 => YELLOW,
        90..120 => ACCENT,
        _ => RED,
    }
}

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
    /// A ride, but the recorder is not saving it to the card (failing, refused, or a build that may not write):
    /// the dot grey.
    NotSaving,
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
/// The ride's state, right of the time: a red dot while it records, orange bars while paused, nothing otherwise.
const STRIP_REC: (u32, u32) = (STRIP_TIME.0 + STRIP_TIME.1 + 3, 10);
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
    /// The next step's target range (END on the last step).
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
    /// The course's next cue: its arrow and how far ("350M", "1.2K"); "--" off course (course/cues.rs).
    NextTurn,
}

impl Field {
    /// Every field, in the order the page menu cycles through them; the index is kept in a byte
    /// ([`crate::PageCfg`]), so new ones go at the end.
    pub const ALL: [Field; 57] = {
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
            Music,    // the course followed again:
            NextTurn,
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
            "NEXT TURN",
        ];
        LABELS[self as usize]
    }

    /// The widest value it shows: the value's scale is chosen so this fits.
    fn widest(self) -> &'static str {
        const WIDEST: [&str; Field::ALL.len()] = [
            "88:88", "888", "88.8", "888", "8888", "", "88:88:88", "88.88", "88.8", "88.8", "888", "8888", "88:88:88",
            "88.88", "8888", "", "-88.8", "", "8.88", "888", // power.rs and the ride's derived metrics:
            "8888", "8888", "8888", "8888", "8.88", "888", "8888", "8888", "88.8", "-88.8", "8888", "888", "888",
            "88/88", "-8888", "-88.8", "", "", "", "8888", "888.8", "88888", "888888", // the workout player:
            "88/88", "888-888", "8:88:88", "888-888", // the course followed:
            "888.8", "8888", "8.8K 88%", "8888", "8888", "", "", "8888",  // the phone:
            "88:88", // the course followed again:
            "888.8K",
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
pub const FIELD_GROUPS: [(&str, &[Field]); 9] = {
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
        ("WORKOUT", &[Workout, StepTarget, StepTime, NextStep, WorkoutGraph, WktPower]),
        ("COURSE", &[ToGo, NextTurn, OffCourse, Climb, ClimbGain, UpToGo, Profile]),
        ("TOTALS", &[WeekKm, WeekH, YearKm, TotalKm]),
        ("PHONE", &[Music]),
    ]
};

/// A page layout: rows top to bottom, each (its share of the height, boxes side by side), and its name in the menu.
pub type Layout = (&'static [(u8, u8)], &'static str);

/// The layouts by the id a page keeps ([`crate::PageCfg::layout`]); 0 is a page switched off. Ids 1, 2, 3, 4 and 6
/// are the layouts of earlier builds, whose id was their field count, so pages kept then read the same.
pub const LAYOUTS: [Layout; 13] = [
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
];

/// The order the page menu goes through the layouts: by field count.
pub const LAYOUT_ORDER: [u8; 13] = [0, 1, 2, 8, 3, 7, 9, 4, 10, 5, 11, 6, 12];

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

/// Field box `i` (x, y, w, h) of layout `id` in a field area `h` high, row by row, left to right; the last row and
/// the last box of a row take what division leaves. None past the last.
///
/// ```
/// use bryton_rustfw::ui::{field_box, RIDE_Y};
/// assert_eq!(field_box(5, 300, 0), Some((0, RIDE_Y, 240, 150)), "big over 2 x 2: the big box");
/// assert_eq!(field_box(5, 300, 4), Some((120, RIDE_Y + 225, 120, 75)));
/// assert_eq!(field_box(5, 300, 5), None);
/// ```
pub fn field_box(id: u8, h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    let (rows, _) = layout(id)?;
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
    if !field.is_picture() {
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

/// A graph's axis text: the pixel font at 1x, grey.
const AXIS: Style = Style { scale: 1, fg: GREY, ..LABEL };
/// Room left of the plot for its scale (four characters: "0.75", "2400"), and under it for the time.
const AXIS_W: u32 = 4 * CELL + 3;
const AXIS_H: u32 = 10;

/// A graph box this big has its axes drawn (smaller ones show the bars alone).
fn has_axes(w: u32, h: u32) -> bool {
    w >= 110 && h >= 80
}

/// The graph's plot in field box `bx`: left, top, columns, height. One bar per pixel column, side by side, so the
/// bars make a filled area as stock's graph does; the newest bar is the rightmost column. Room is left for the
/// axes ([`draw_graph_axes`]) when the box has it.
pub fn graph_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let (left, below) = if has_axes(w, h) { (AXIS_W, AXIS_H) } else { (0, 0) };
    (x + 4 + left, y + FIELD_LABEL_H, w - 8 - left, h - FIELD_LABEL_H - 4 - below)
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

/// Room at a graph's top right for two figures ([`draw_graph_stats`]): "AVG 1234".
const STATS_W: u32 = 8 * CELL;

/// Two figures of the ride (`lines`: "AVG 182", "NP 195") at the right of a graph's label line, one above the other,
/// each in its own box (None: left as drawn); none in a box too narrow to hold them beside `title`.
pub fn draw_graph_stats(
    c: &mut impl Canvas,
    (x, y, w, _): (u32, u32, u32, u32),
    title: &str,
    lines: [Option<&str>; 2],
) {
    if LABEL.width(title) + STATS_W + 16 > w {
        return;
    }
    let style = Style { fg: WHITE, ..AXIS };
    for (k, l) in (0..).zip(lines).filter_map(|(k, l)| Some((k, l?))) {
        text::draw_in(c, &style, x + w - 6 - STATS_W, y + 3 + k * (AXIS.height() + 2), STATS_W, Align::Right, l);
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

/// The marker's width, columns, centred on the reading's column.
const MARK_W: u32 = 3;
/// The inside of the target band: a dark green, so a green marker in it still shows.
const BAND: u16 = 0x0320;

/// A gauge's bar: the band's first and last columns, and the marker's column and colour (None: no reading).
pub type GaugeBar = ((u32, u32), Option<(u32, u16)>);

/// A gauge bar's place: left, top, columns, height.
type GaugeArea = (u32, u32, u32, u32);

/// The gauge field's parts in box `bx`: the bar (left, top, columns, height), the number's top and style, the
/// target line's top and style (the axis text in a box under 100 high, so the number keeps room). Fixed for a box,
/// so the number and the marker are redrawn in place.
fn gauge_parts(bx: (u32, u32, u32, u32)) -> (GaugeArea, u32, Style, (u32, Style)) {
    let (x, y, w, h) = bx;
    let small = if h < 100 { AXIS } else { Style { fg: GREY, ..LABEL } };
    let target_y = (y + h).saturating_sub(2 + small.height()).max(y);
    let gh = (h / 8).clamp(12, 24); // the bar's height: a quarter of it above and below the track is the marker's
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

/// Pixel (`i`, `dy`) of a gauge bar `h` high: the marker over all of it; else black in the quarters above and below
/// the track (so the marker stands out of it), the band's edges in the zone palette's green, its inside dark green, the
/// rest of the track dark grey.
fn gauge_px((band, marker): GaugeBar, h: u32, i: u32, dy: u32) -> u16 {
    match marker {
        Some((m, color)) if i + MARK_W / 2 >= m && i <= m + MARK_W / 2 => color,
        _ if dy < h / 4 || dy + h / 4 >= h => BLACK,
        _ if i == band.0 || i == band.1 => crate::ride::ZONE_COLORS[2],
        _ if band.0 < i && i < band.1 => BAND,
        _ => DARK,
    }
}

/// Columns `from` to `to` (included) of the gauge bar `bar` showing `g`.
fn draw_gauge_columns(c: &mut impl Canvas, bar: (u32, u32, u32, u32), g: GaugeBar, from: u32, to: u32) {
    let to = to.min(bar.2.saturating_sub(1));
    if from <= to {
        c.blit(bar.0 + from, bar.1, to - from + 1, bar.3, &mut |i, dy| gauge_px(g, bar.3, from + i, dy));
    }
}

/// The gauge field whole in box `bx`: `title` in its label line, `value` big in `fg`, the bar `g` (none: the bar's
/// place black) and `target` under it.
pub fn draw_gauge(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    (title, target): (&str, &str),
    value: &str,
    fg: u16,
    g: Option<GaugeBar>,
) {
    let (x, _, w, _) = bx;
    let (bar, _, _, (target_y, small)) = gauge_parts(bx);
    draw_plot_title(c, bx, title);
    draw_gauge_value(c, bx, "", value, fg);
    match g {
        Some(g) => draw_gauge_columns(c, bar, g, 0, bar.2),
        None => c.fill(bar.0, bar.1, bar.2, bar.3, BLACK),
    }
    text::draw_in(c, &small, x + 1, target_y, w.saturating_sub(2), Align::Centre, target);
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
    for (m, _) in [old, g.1].into_iter().flatten() {
        draw_gauge_columns(c, bar, g, m.saturating_sub(MARK_W / 2), m + MARK_W / 2);
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
    draw_bitmap(c, &GEAR, GEAR_AT.0, GEAR_AT.1, 2, GREY); // after the date: its box spans the row
}

/// The menu button on the overview, right of the date: a gear, 9 x 9 bitmap pixels.
pub const GEAR: [u16; 9] = [0x1400, 0x7f00, 0x6300, 0x4100, 0xc180, 0x4100, 0x6300, 0x7f00, 0x1400];
/// Where [`GEAR`] is drawn (at scale 2: 18 x 18 px), in the date's row: under the clock's digit cells, which a
/// second's redraw paints whole.
pub const GEAR_AT: (u32, u32) = (W - 30, DATE_Y);
/// Where a tap opens the menu: around the gear, wider than it, a finger's size (above the overview's fields).
pub const GEAR_TAP: (u32, u32, u32, u32) = (W - 64, DATE_Y - 24, 64, OVERVIEW_Y - (DATE_Y - 24));

/// The notices, newest first; with none, Ferris.
pub fn draw_notices(c: &mut impl Canvas, notices: &[&str]) {
    c.fill(0, NOTICES_Y, W, H - NOTICES_Y, BLACK);
    if notices.is_empty() {
        return draw_ferris(c);
    }
    for (r, n) in notices.iter().take(NOTICES).enumerate() {
        // "19:36 Edwige A...": the time small and grey, the text in the large font for every row (cut, not shrunk:
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

/// `style`, or at scale 1 if `text` would not fit in `w` ("08:41 HEART RATE PAIRED"): smaller rather than cut.
fn fit(style: Style, text: &str, w: u32) -> Style {
    if style.width(text) > w { Style { scale: 1, ..style } } else { style }
}

/// Top of a banner: a notice over the bottom of any screen other than the overview.
pub const BANNER_Y: u32 = H - 40;

/// A banner: `text` in black on orange over the bottom of the screen.
pub fn draw_banner(c: &mut impl Canvas, text: &str) {
    c.fill(0, BANNER_Y, W, 4, BLACK);
    let style = fit(Style { fg: BLACK, bg: ACCENT, ..LABEL }, text, W);
    c.fill(0, BANNER_Y + 4, W, 8, ACCENT);
    text::draw_in(c, &style, 0, BANNER_Y + 12, W, Align::Centre, text);
    c.fill(0, BANNER_Y + 12 + style.height(), W, H - BANNER_Y - 12 - style.height(), ACCENT);
}

/// Top of the incoming call's overlay: the two buttons, then the banner with the caller.
pub const CALL_Y: u32 = BANNER_Y - CALL_BUTTON_H - 8;
/// The call buttons' height.
const CALL_BUTTON_H: u32 = 56;
/// The decline (red, left) and answer (green, right) buttons: x, y, w, h.
pub const CALL_BUTTONS: [(u32, u32, u32, u32); 2] =
    [(8, CALL_Y + 4, W / 2 - 16, CALL_BUTTON_H), (W / 2 + 8, CALL_Y + 4, W / 2 - 16, CALL_BUTTON_H)];
/// A telephone handset, 16 x 16 (the earpiece top left, the mouthpiece bottom right).
const HANDSET: [u16; 16] = [
    0x3000, 0x7800, 0x7c00, 0x7c00, 0x7800, 0x3800, 0x3800, 0x1c00, 0x1c00, 0x0e00, 0x0706, 0x039f, 0x01ff, 0x00fe,
    0x007c, 0x0000,
];

/// An incoming call over the bottom of any screen: a red decline and a green answer button, each with a handset,
/// and the banner with the caller (the page above keeps updating: [`crate::App`]'s bottom is [`CALL_Y`]). Once
/// answered it goes: iOS removes the call's notification at the answer (seen on the unit, 2026-10-24), so neither
/// the call's time nor a hang-up can follow over ANCS.
pub fn draw_call(c: &mut impl Canvas, who: &str) {
    c.fill(0, CALL_Y, W, BANNER_Y - CALL_Y, BLACK);
    for (i, &(x, y, w, h)) in CALL_BUTTONS.iter().enumerate() {
        c.fill(x, y, w, h, if i == 0 { RED } else { GREEN });
        draw_bitmap(c, &HANDSET, x + w / 2 - 16, y + h / 2 - 16, 2, WHITE);
    }
    draw_banner(c, if who.is_empty() { "INCOMING CALL" } else { who });
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

/// A menu row's label box: its own length (at most 10 characters: "NEARBY BLE") and half a character's gap; the
/// value takes the rest of the row (beside "FIELD 1", 9 characters: every field's name, a test checks).
fn menu_label_w(label: &str) -> u32 {
    (label.len().min(10) as u32 * 2 + 1) * CELL / 2 * LABEL.scale
}

/// Menu row `i`: `label` on the left, `value` on the right in `value_color`; selected, the row is orange.
pub fn draw_menu_row(c: &mut impl Canvas, i: usize, label: &str, value: &str, value_color: u16, selected: bool) {
    let y = LIST_Y + i as u32 * LIST_ROW;
    let (bg, fg) = if selected { (ACCENT, BLACK) } else { (BLACK, WHITE) };
    let name_w = menu_label_w(label);
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

    #[test]
    fn every_field_in_one_group_that_fits_a_screen() {
        for f in Field::ALL {
            assert_eq!(FIELD_GROUPS.iter().filter(|(_, g)| g.contains(&f)).count(), 1, "{f:?}");
        }
        assert!(FIELD_GROUPS.iter().all(|(_, g)| g.len() <= MENU_ROWS), "a group is one screen");
        assert!(FIELD_GROUPS.len() <= MENU_ROWS);
    }

    #[test]
    fn menu_labels_are_whole() {
        assert!(LABEL.width("NEARBY BLE") <= menu_label_w("NEARBY BLE"), "a 10-letter label is not cut");
    }

    #[test]
    fn field_names_fit_their_menu_row() {
        for f in Field::ALL {
            assert!(LABEL.width(f.label()) <= NEARBY.2 - menu_label_w("FIELD 1"), "{}", f.label());
        }
    }

    #[test]
    fn long_notices_fit() {
        for t in ["08:41 RIDE RESUMED", "08:41 HEART RATE PAIRED", "08:41 SPEED+CAD PAIRED", "08:41 FAULT RESET 3"] {
            assert!(fit(LABEL, t, W - 8).width(t) <= W - 8, "{t}");
        }
        assert_eq!(fit(LABEL, "08:41 RIDE RESUMED", W - 8).scale, LABEL.scale, "the usual size when it fits");
    }

    /// A framebuffer that panics on any fill off screen.
    pub struct Frame {
        pub px: Vec<u16>,
        fills: usize,
        painted: usize,
    }

    impl Frame {
        pub fn new() -> Self {
            Frame { px: vec![BLACK; (W * H) as usize], fills: 0, painted: 0 }
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

    /// The ride boxes tile the field area exactly, every value fits its box, and Ferris and the top line stay
    /// clear (Frame panics on anything off screen).
    #[test]
    fn ride_layouts_tile_and_fit() {
        for h in [RIDE_H] {
            for n in 1..LAYOUTS.len() as u8 {
                let b: Vec<_> = (0..8).map_while(|i| field_box(n, h, i)).collect();
                assert_eq!(b.len(), field_count(n));
                assert!(b.len() <= 6, "a page keeps six fields");
                let area: u32 = b.iter().map(|&(_, _, w, h)| w * h).sum();
                assert!(area <= W * h && area + W * 2 >= W * h, "{n} fields cover the area");
                let mut f = Frame::new();
                for (&bx, field) in b.iter().flat_map(|bx| Field::ALL.iter().map(move |f| (bx, f))) {
                    draw_field_frame(&mut f, bx, *field);
                    draw_field_value(&mut f, bx, *field, field.widest(), WHITE);
                    assert!(value_style(*field, bx.2, bx.3).scale >= 2, "{n} fields: {field:?} readable");
                }
            }
        }
        const { assert!(RIDE_Y + RIDE_H == H && RIDE_Y >= STRIP_H + 2) };
        let strip = [STRIP_BLE, STRIP_GPS, STRIP_TIME, STRIP_REC, STRIP_USB, STRIP_BATTERY];
        assert!(strip.windows(2).all(|p| p[0].0 + p[0].1 <= p[1].0) && STRIP_BATTERY.0 + STRIP_BATTERY.1 <= W);
        assert!(TIME_Y + TIME.height() <= DATE_Y && OVERVIEW_Y + OVERVIEW_H < NOTICES_Y && TIME_Y > STRIP_H);
        assert!(GEAR_AT.1 >= DATE_Y && GEAR_AT.1 + 18 <= OVERVIEW_Y && GEAR_AT.0 + 18 <= W, "the gear in the date row");
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
        for bx in rides.chain((0..4).filter_map(overview_box)) {
            let cols = gauge_cols(bx);
            let band = (cols / 3, cols * 2 / 3);
            let titles = ("POWER 30S", "200-220");
            let (mut a, mut b) = (Frame::new(), Frame::new());
            draw_gauge(&mut a, bx, titles, "195", YELLOW, Some((band, Some((0, YELLOW)))));
            draw_gauge_marker(&mut a, bx, Some((0, YELLOW)), (band, Some((cols - 1, YELLOW))));
            draw_gauge(&mut b, bx, titles, "195", YELLOW, Some((band, Some((cols - 1, YELLOW)))));
            assert!(a.px == b.px, "{bx:?}");
            assert!(gauge_parts(bx).2.scale >= 2, "{bx:?}: the number readable");
        }
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
