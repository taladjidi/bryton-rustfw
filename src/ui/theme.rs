//! Colour themes (docs/theming.md, phase 1). The code keeps drawing with its named colour constants (the keys,
//! [`KEYS`]); a [`Theme`] says what each is drawn as, and the canvas translates ([`crate::Canvas::ink`]). DARK is the
//! identity, so every screen and test is as it was. A theme file is an INI (`\Rustfw\Themes\<NAME>.ini`) read by
//! [`parse`], then checked as a whole ([`check`]): readable text, red only for alarms; a theme that fails is refused
//! with a [`Refused`] reason and the caller uses DARK.
//!
//! Contrast is WCAG 2's ratio, (L1 + 0.05) / (L2 + 0.05) with L the relative luminance
//! (<https://www.w3.org/TR/WCAG21/#dfn-contrast-ratio>), in integers: an RGB565 channel has 32 or 64 levels, so the
//! sRGB-to-linear step is two small tables (x 10000, computed from the sRGB transfer function).

use super::*;
use crate::ride::ZONE_COLORS;

/// How many roles a theme colours.
pub const N: usize = 20;

/// The built-in colours a theme translates, in the order of [`NAMES`] and [`Theme::ink`].
pub const KEYS: [u16; N] = [
    BLACK,
    WHITE,
    DARK,
    GREY,
    ACCENT,
    ON_ACCENT,
    RED,
    ON_ALARM,
    GREEN,
    YELLOW,
    RUST,
    COG_COLOR,
    map::COURSE,
    ZONE_COLORS[0],
    ZONE_COLORS[1],
    ZONE_COLORS[2],
    ZONE_COLORS[3],
    ZONE_COLORS[4],
    ZONE_COLORS[5],
    ZONE_COLORS[6],
];

/// The keys' names in a theme file's `[colors]` section, same order as [`KEYS`].
pub const NAMES: [&str; N] = [
    "background",
    "text",
    "line",
    "dim",
    "accent",
    "on_accent",
    "alarm",
    "on_alarm",
    "ok",
    "warn",
    "ferris",
    "cog",
    "course",
    "zone1",
    "zone2",
    "zone3",
    "zone4",
    "zone5",
    "zone6",
    "zone7",
];

// positions in KEYS
const BG: usize = 0;
const TEXT: usize = 1;
const ACC: usize = 4;
const ON_ACC: usize = 5;
const ALARM: usize = 6;
const ON_AL: usize = 7;
const ZONE0: usize = 13;

/// RGB565 from 8-bit channels, each rounded.
pub const fn rgb(r: u32, g: u32, b: u32) -> u16 {
    ((((r * 31 + 127) / 255) << 11) | (((g * 63 + 127) / 255) << 5) | ((b * 31 + 127) / 255)) as u16
}

/// Why a theme file was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Over 4 KB, or no `[theme]` section.
    NotATheme,
    /// A `version` above 1.
    TooNew,
    /// `text` on `background`, `on_accent` on `accent` or `on_alarm` on `alarm` under 4.5:1.
    Contrast,
    /// `alarm` is also the accent, ok, warn, Ferris, the course or a zone: red is for alarms only.
    RedNotAlarm,
    /// Two zones have the same colour.
    SameZones,
}

impl Refused {
    /// The reason as a notice says it.
    pub fn text(self) -> &'static str {
        match self {
            Refused::NotATheme => "NOT A THEME",
            Refused::TooNew => "THEME TOO NEW",
            Refused::Contrast => "LOW CONTRAST",
            Refused::RedNotAlarm => "RED ONLY FOR ALARMS",
            Refused::SameZones => "ZONES ALIKE",
        }
    }
}

/// A theme, resolved: the colour each key is drawn in (and the font, from the FONT setting). Copy, no heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// What each of [`KEYS`] is drawn as.
    pub ink: [u16; N],
    /// The FONT setting ([`crate::Canvas::smooth`]); not part of a theme file.
    pub smooth: bool,
}

impl Theme {
    /// DARK: today's colours exactly (the text colours on the accent and the alarm are black and white).
    pub const DARK: Theme = {
        let mut ink = KEYS;
        ink[ON_ACC] = BLACK;
        ink[ON_AL] = WHITE;
        Theme { ink, smooth: false }
    };

    /// OUTDOOR: DARK with a lighter grey for the notes, brighter lines and zones (Intervals.icu's palette x 2).
    pub const OUTDOOR: Theme = {
        let mut ink = Theme::DARK.ink;
        ink[2] = rgb(112, 112, 112);
        ink[3] = rgb(192, 192, 192);
        ink[13] = rgb(94, 196, 178);
        ink[14] = rgb(94, 196, 70);
        ink[15] = rgb(255, 250, 98);
        ink[16] = rgb(255, 174, 84);
        ink[17] = rgb(248, 64, 118);
        ink[18] = rgb(136, 90, 248);
        ink[19] = rgb(120, 110, 144);
        Theme { ink, smooth: false }
    };

    /// LIGHT: black on white for the sun (the panel is transflective). Every colour is darkened to hold 4.5:1 (text)
    /// or 3:1 (lines, zones, the course) on white; the zones are Intervals.icu's own values, made for white pages.
    pub const LIGHT: Theme = {
        let mut ink = Theme::DARK.ink;
        ink[0] = WHITE;
        ink[1] = BLACK;
        ink[2] = rgb(140, 140, 140);
        ink[3] = rgb(96, 96, 96);
        ink[4] = rgb(176, 64, 0);
        ink[5] = WHITE;
        ink[6] = rgb(208, 0, 0);
        ink[7] = WHITE;
        ink[8] = rgb(0, 112, 0);
        ink[9] = rgb(138, 100, 0);
        ink[10] = rgb(192, 72, 0);
        ink[11] = rgb(112, 112, 112);
        ink[12] = rgb(112, 0, 192);
        ink[13] = rgb(47, 98, 89);
        ink[14] = rgb(47, 98, 35);
        ink[15] = rgb(148, 125, 49);
        ink[16] = rgb(143, 87, 42);
        ink[17] = rgb(124, 32, 59);
        ink[18] = rgb(68, 45, 124);
        ink[19] = rgb(60, 55, 72);
        Theme { ink, smooth: false }
    };

    /// The built-in theme called `name` (ASCII case ignored).
    pub fn builtin(name: &str) -> Option<Theme> {
        [("DARK", Theme::DARK), ("OUTDOOR", Theme::OUTDOOR), ("LIGHT", Theme::LIGHT)]
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, t)| t)
    }

    /// What the key `c` is drawn as; a colour that is no key (a picture's, a blend) as it is.
    pub fn ink(&self, c: u16) -> u16 {
        match KEYS.iter().position(|&k| k == c) {
            Some(i) => self.ink.get(i).copied().unwrap_or(c),
            None => c,
        }
    }
}

/// `#RRGGBB` or `RRGGBB` as RGB565.
///
/// ```
/// use bryton_rustfw::ui::theme::hex;
/// assert_eq!(hex(b"#FFA500"), Some(0xfd20));
/// assert_eq!(hex(b"000000"), Some(0));
/// assert_eq!([hex(b"#FFF"), hex(b"#GG0000"), hex(b"")], [None; 3]);
/// ```
pub fn hex(v: &[u8]) -> Option<u16> {
    let v = v.strip_prefix(b"#").unwrap_or(v);
    if v.len() != 6 {
        return None;
    }
    let mut n = 0u32;
    for &b in v {
        n = n << 4 | char::from(b).to_digit(16)?;
    }
    Some(rgb(n >> 16, n >> 8 & 255, n & 255))
}

/// sRGB-to-linear of a 5-bit channel, x 10000.
const LIN5: [u16; 32] = [
    0, 24, 52, 97, 152, 222, 307, 423, 545, 685, 844, 1022, 1248, 1470, 1714, 1981, 2307, 2623, 2961, 3325, 3763, 4179,
    4621, 5089, 5583, 6172, 6724, 7305, 7913, 8632, 9301, 10000,
];
/// The same for a 6-bit channel.
const LIN6: [u16; 64] = [
    0, 12, 24, 37, 52, 70, 91, 116, 144, 176, 212, 262, 307, 356, 409, 467, 529, 595, 666, 742, 823, 908, 999, 1095,
    1195, 1301, 1413, 1529, 1651, 1779, 1912, 2051, 2232, 2384, 2542, 2705, 2874, 3050, 3231, 3419, 3613, 3813, 4020,
    4233, 4452, 4678, 4910, 5149, 5395, 5647, 5906, 6172, 6445, 6795, 7084, 7379, 7682, 7991, 8308, 8632, 8963, 9301,
    9647, 10000,
];

/// Relative luminance of an RGB565 colour, x 10^8.
fn luminance(c: u16) -> u64 {
    let at = |t: &[u16], i: u16| u64::from(t.get(usize::from(i)).copied().unwrap_or(0));
    2126 * at(&LIN5, c >> 11) + 7152 * at(&LIN6, c >> 5 & 63) + 722 * at(&LIN5, c & 31)
}

/// Whether `a` and `b` have a contrast ratio of at least `min` / 10 (the ratio is that of WCAG 2).
fn contrast(a: u16, b: u16, min: u64) -> bool {
    let (hi, lo) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
    // (hi + 0.05) / (lo + 0.05) >= min / 10, with 0.05 as 5 * 10^6 in the unit above
    10 * (hi + 5_000_000) >= min * (lo + 5_000_000)
}

/// Whether `a` on `b` has a contrast ratio of at least 4.5:1 (WCAG 2 AA for text).
///
/// ```
/// use bryton_rustfw::ui::theme::readable;
/// assert!(readable(0xffff, 0x0000) && readable(0x0000, 0xfd20));
/// assert!(!readable(0x8410, 0x0000 + 0x39e7), "grey on dark grey");
/// ```
pub fn readable(a: u16, b: u16) -> bool {
    contrast(a, b, 45)
}

/// The pairs [`check`] tests: a foreground role (a position in [`KEYS`]), the role it is drawn on, and the least
/// contrast ratio x 10. Text (the text, notes, accent and alarm drawn as text on the background, and the text on the
/// accent, the alarm, ok and warn fills) needs 4.5:1, WCAG 2 success criterion 1.4.3; what is not text (lines, Ferris,
/// the cog, the course, the zones) needs 3:1, the non-text minimum of criterion 1.4.11
/// (<https://www.w3.org/TR/WCAG21/#non-text-contrast>).
pub const PAIRS: [(usize, usize, u64); 21] = [
    (TEXT, BG, 45),
    (3, BG, 45),
    (ACC, BG, 45),
    (ALARM, BG, 45),
    (8, BG, 45),
    (9, BG, 45),
    (ON_ACC, ACC, 45),
    (ON_ACC, 8, 45),
    (ON_ACC, 9, 45),
    (ON_AL, ALARM, 45),
    (2, BG, 30),
    (10, BG, 30),
    (11, BG, 30),
    (12, BG, 30),
    (13, BG, 30),
    (14, BG, 30),
    (15, BG, 30),
    (16, BG, 30),
    (17, BG, 30),
    (18, BG, 30),
    (19, BG, 30),
];

/// The whole-theme checks (module doc) of `t`, a theme file's values over `base`: red only for alarms (no other role
/// has the alarm's colour), then every pair of [`PAIRS`], then no two zones alike. A pair whose two colours are both
/// the base's is not checked: the built-ins are trusted, and DARK's white on pure red is 4.0:1 and its lines 1.9:1
/// (today's look, kept as it is), so a file that leaves a pair alone must not be refused for it.
pub fn check(t: &Theme, base: &Theme) -> Result<(), Refused> {
    let k = &t.ink;
    let at = |i: usize| k.get(i).copied().unwrap_or(0);
    if k.iter().enumerate().any(|(i, &c)| i != ALARM && c == at(ALARM)) {
        return Err(Refused::RedNotAlarm);
    }
    let same = |a: usize, b: usize| k.get(a) == base.ink.get(a) && k.get(b) == base.ink.get(b);
    if PAIRS.iter().any(|&(a, b, min)| !same(a, b) && !contrast(at(a), at(b), min)) {
        return Err(Refused::Contrast);
    }
    let zones = k.get(ZONE0..).unwrap_or(&[]);
    if zones.iter().enumerate().any(|(i, z)| zones.iter().skip(i + 1).any(|o| o == z)) {
        return Err(Refused::SameZones);
    }
    Ok(())
}

/// A theme file's text (an INI, see features.md for the keys) as a theme: the colours of `[colors]` over those of
/// `[theme] base` (dark, outdoor or light, else dark); a value that is not `#RRGGBB` or a key not in the table is ignored.
/// Refused when it is not a theme, is too new, or fails [`check`].
///
/// ```
/// use bryton_rustfw::ui::theme::{parse, Refused};
/// let t = parse(b"[theme]\nversion=1\nbase=outdoor\n[colors]\naccent=#00AAFF ; blue\nzone1=nope\n").unwrap();
/// assert_eq!(t.ink[4], 0x055f);
/// assert_eq!(parse(b"[theme]\nversion=2\n"), Err(Refused::TooNew));
/// assert_eq!(parse(b"[colors]\ntext=#FFFFFF\n"), Err(Refused::NotATheme));
/// ```
pub fn parse(text: &[u8]) -> Result<Theme, Refused> {
    use crate::ini::get;
    if text.len() > 4096 {
        return Err(Refused::NotATheme);
    }
    let value =
        |sec: &str, key: &str| get(text, sec, key).map(|v| v.split(|&b| b == b';').next().unwrap_or(&[]).trim_ascii());
    if value("theme", "version").is_none() && value("theme", "name").is_none() {
        return Err(Refused::NotATheme);
    }
    if value("theme", "version").and_then(|v| crate::ini::decimal(v, 0)).is_some_and(|v| v > 1) {
        return Err(Refused::TooNew);
    }
    let base = match value("theme", "base") {
        Some(b) if b.eq_ignore_ascii_case(b"outdoor") => Theme::OUTDOOR,
        Some(b) if b.eq_ignore_ascii_case(b"light") => Theme::LIGHT,
        _ => Theme::DARK,
    };
    let mut t = base;
    for (slot, name) in t.ink.iter_mut().zip(NAMES) {
        if let Some(c) = value("colors", name).and_then(hex) {
            *slot = c;
        }
    }
    check(&t, &base).map(|()| t)
}

/// Lux (tenths) under which AUTO's night theme comes: the top of stock's dusk band (`ambient::AUTO_BANDS`, 20 lux,
/// where its backlight is at 20 %, then 30 % up to 50 lux).
pub const NIGHT_BELOW: u32 = 200;
/// Lux (tenths) over which AUTO's day theme comes back: the top of stock's 30 % band (50 lux), 2.5 times
/// [`NIGHT_BELOW`], so a dusk that flickers stays on the side it is on.
pub const DAY_ABOVE: u32 = 500;
/// Seconds a light reading must stay on the other side before the theme switches (the owner's 30 s).
pub const HOLD_S: u32 = 30;
/// Seconds at least between two switches (at most one a minute).
pub const GAP_S: u32 = 60;

/// AUTO (THEME): day or night from the ambient light. The light is the mean of each second's readings, checked once a
/// second with integers only; a side is kept until the mean is over the other threshold for [`HOLD_S`] seconds in a
/// row, and never switched again within [`GAP_S`] of the last switch. The first checked mean decides at once (night
/// under [`NIGHT_BELOW`], else day), with no hold; no reading, no theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayNight {
    /// The night theme is in use.
    pub night: bool,
    /// The readings of this second, summed, and their count.
    sum: u32,
    n: u32,
    /// Milliseconds since the last check.
    ms: u32,
    /// Seconds the mean has been on the other side (up to [`HOLD_S`]).
    held: u32,
    /// Seconds since the last switch (up to [`GAP_S`]).
    since: u32,
    /// No check yet: the first one decides at once.
    first: bool,
}

impl Default for DayNight {
    fn default() -> Self {
        DayNight { night: false, sum: 0, n: 0, ms: 0, held: 0, since: GAP_S, first: true }
    }
}

impl DayNight {
    /// Takes one reading (tenths of a lux) taken `dt_ms` after the last. Once a second it checks the mean, and returns
    /// the new night flag when the theme switches.
    ///
    /// ```
    /// use bryton_rustfw::theme::DayNight;
    /// let mut d = DayNight::default();
    /// assert_eq!(d.update(10, 1000), Some(true), "the first reading, 1 lux: night at once");
    /// for _ in 0..59 {
    ///     assert_eq!(d.update(1000, 1000), None, "100 lux: day, but not within a minute of the switch");
    /// }
    /// assert_eq!(d.update(1000, 1000), Some(false), "a minute on, and held 30 s: day");
    /// ```
    pub fn update(&mut self, lux10: u32, dt_ms: u32) -> Option<bool> {
        self.sum = self.sum.saturating_add(lux10);
        self.n = self.n.saturating_add(1);
        self.ms = self.ms.saturating_add(dt_ms);
        if self.ms < 1000 {
            return None;
        }
        let lux = self.sum / self.n.max(1);
        (self.sum, self.n, self.ms) = (0, 0, 0);
        if self.first {
            self.first = false;
            let night = lux < NIGHT_BELOW;
            if night == self.night {
                return None;
            }
            (self.night, self.since, self.held) = (night, 0, 0);
            return Some(night);
        }
        self.since = self.since.saturating_add(1).min(GAP_S);
        let other = if self.night { lux > DAY_ABOVE } else { lux < NIGHT_BELOW };
        if !other {
            self.held = 0;
            return None;
        }
        self.held = self.held.saturating_add(1).min(HOLD_S);
        if self.held < HOLD_S || self.since < GAP_S {
            return None;
        }
        self.night = !self.night;
        (self.held, self.since) = (0, 0);
        Some(self.night)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn keys_are_distinct_and_dark_is_the_identity() {
        for (i, k) in KEYS.iter().enumerate() {
            assert!(!KEYS[i + 1..].contains(k), "key {i} repeated");
        }
        let none = Theme { ink: [0; N], smooth: false };
        assert_eq!(check(&Theme::LIGHT, &none), Ok(()), "LIGHT passes every check on its own, no base to excuse it");
        for t in [Theme::DARK, Theme::OUTDOOR, Theme::LIGHT] {
            assert_eq!(check(&t, &t), Ok(()));
        }
        let identity = |c: u16| Theme::DARK.ink(c);
        assert!(KEYS.iter().all(|&k| identity(k) == k || k == ON_ACCENT || k == ON_ALARM));
        assert_eq!((identity(ON_ACCENT), identity(ON_ALARM)), (BLACK, WHITE));
        assert_eq!(identity(0x1234), 0x1234);
    }

    /// Which pairs of the built-ins fail on their own (no base to excuse them), for the commit message.
    #[test]
    fn builtin_pairs() {
        let none = Theme { ink: [0; N], smooth: false };
        for (name, t) in [("DARK", Theme::DARK), ("OUTDOOR", Theme::OUTDOOR)] {
            let failing: std::vec::Vec<_> = PAIRS
                .iter()
                .filter(|&&(a, b, m)| !contrast(t.ink[a], t.ink[b], m))
                .map(|&(a, b, _)| (NAMES[a], NAMES[b]))
                .collect();
            std::println!("{name} fails: {failing:?}");
            if name == "OUTDOOR" {
                assert_eq!(failing, [("on_alarm", "alarm")], "OUTDOOR inherits DARK white on red");
            }
        }
        assert_eq!(check(&Theme::DARK, &none), Err(Refused::Contrast), "DARK is excused by the base only");
    }

    #[test]
    fn contrast_ratios() {
        assert!(readable(WHITE, BLACK) && readable(BLACK, ACCENT) && readable(BLACK, GREEN));
        assert!(!readable(GREY, DARK) && !readable(WHITE, YELLOW));
        assert!(!readable(WHITE, RED), "DARK's white on red is 4.0:1");
        // WCAG's own examples: #767676 on white 4.54, #777777 4.48 (RGB565 rounds the grey a little)
        assert!(readable(rgb(0x70, 0x70, 0x70), WHITE) && !readable(rgb(0x80, 0x80, 0x80), WHITE));
    }

    #[test]
    fn parser_keys_and_fallbacks() {
        let t =
            parse(b"[THEME]\r\nName = X\r\n[colors]\r\nText = #00ff00\r\nline = 12\r\nbogus = #123456\r\n").unwrap();
        assert_eq!(t.ink[TEXT], GREEN, "key and value read, any case, CRLF");
        assert_eq!(t.ink[2], DARK, "a bad value keeps the base's");
        assert_eq!(t.ink[3], GREY, "a missing key keeps the base's");
        assert_eq!(parse(b"[theme]\nversion=3\n"), Err(Refused::TooNew));
        assert_eq!(parse(&[b'#'; 5000]), Err(Refused::NotATheme));
        assert_eq!(parse(b"[theme]\nversion=abc\n").map(|t| t.ink), Ok(Theme::DARK.ink), "unreadable version: 1");
    }

    #[test]
    fn whole_theme_checks() {
        let f = |extra: &str| {
            let mut s = std::string::String::from("[theme]\nversion=1\n[colors]\n");
            s.push_str(extra);
            parse(s.as_bytes())
        };
        assert_eq!(f("text=#303030"), Err(Refused::Contrast));
        assert_eq!(f("on_accent=#FFA500"), Err(Refused::Contrast));
        assert_eq!(f("on_alarm=#F04040"), Err(Refused::Contrast));
        assert_eq!(f("accent=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("zone3=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("ferris=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("background=#FF0000"), Err(Refused::RedNotAlarm), "a red background");
        assert_eq!(f("text=#FF0000"), Err(Refused::RedNotAlarm), "red text");
        assert_eq!(f("dim=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("cog=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("ok=#101010"), Err(Refused::Contrast), "ok on black");
        assert_eq!(f("ok=#006000"), Err(Refused::Contrast), "black text on a dark ok");
        assert_eq!(f("line=#101010"), Err(Refused::Contrast), "a line under 3:1");
        assert_eq!(f("zone4=#202020"), Err(Refused::Contrast), "a zone under 3:1");
        assert_eq!(f("zone2=#4488CC\nzone1=#4488CC"), Err(Refused::SameZones));
        assert!(f("accent=#00AAFF").is_ok());
    }
}

#[cfg(test)]
mod day_night_tests {
    use super::*;

    /// A DayNight past its first check (the hold rules apply from the start).
    fn settled() -> DayNight {
        DayNight { first: false, ..DayNight::default() }
    }

    #[test]
    fn the_first_reading_decides_at_once() {
        let mut night = DayNight::default();
        assert_eq!(night.update(50, 1000), Some(true), "5 lux at the first reading: night");
        let mut day = DayNight::default();
        assert_eq!(day.update(300, 1000), None, "30 lux: between the thresholds, day");
        assert!(!day.night);
    }

    /// `secs` seconds of readings of `lux10` (four 250 ms readings each); the first switch, as (second, night).
    fn feed(d: &mut DayNight, lux10: u32, secs: u32) -> Option<(u32, bool)> {
        let mut first = None;
        for s in 1..=secs {
            for _ in 0..4 {
                if let Some(night) = d.update(lux10, 250) {
                    first.get_or_insert((s, night));
                }
            }
        }
        first
    }

    #[test]
    fn the_hold_of_30_seconds_then_night() {
        let mut d = settled();
        assert_eq!(feed(&mut d, 50, 29), None, "29 s dark: not yet");
        assert_eq!(feed(&mut d, 50, 1), Some((1, true)), "the 30th second: night");
        assert!(d.night);
    }

    #[test]
    fn between_the_thresholds_the_side_is_kept() {
        let mut d = settled();
        assert_eq!(feed(&mut d, 300, 120), None, "30 lux by day: day stays");
        assert_eq!(feed(&mut d, 50, 30), Some((30, true)));
        assert_eq!(feed(&mut d, 300, 120), None, "30 lux by night: night stays (no flicker back)");
        assert!(d.night);
    }

    #[test]
    fn a_break_restarts_the_hold() {
        let mut d = settled();
        assert_eq!(feed(&mut d, 50, 20), None);
        assert_eq!(feed(&mut d, 300, 1), None, "one second in the middle");
        assert_eq!(feed(&mut d, 50, 20), None, "20 more seconds: 20, not 40");
    }

    #[test]
    fn the_day_comes_back_above_50_lux_and_one_switch_a_minute() {
        let mut d = settled();
        assert_eq!(feed(&mut d, 50, 30), Some((30, true)));
        assert_eq!(feed(&mut d, 600, 59), None, "the hold is met at once, the minute from the switch is not");
        assert_eq!(feed(&mut d, 600, 1), Some((1, false)), "the 60th second after the switch: day");
        assert!(!d.night);
    }

    #[test]
    fn a_bright_start_stays_day() {
        let mut d = settled();
        assert_eq!(feed(&mut d, 1000, 120), None);
        assert!(!d.night);
    }
}
