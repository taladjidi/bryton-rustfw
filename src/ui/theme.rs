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

    /// The built-in theme called `name` (ASCII case ignored).
    pub fn builtin(name: &str) -> Option<Theme> {
        [("DARK", Theme::DARK), ("OUTDOOR", Theme::OUTDOOR)]
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

/// Whether `a` on `b` has a contrast ratio of at least 4.5:1 (WCAG 2 AA for text).
///
/// ```
/// use bryton_rustfw::ui::theme::readable;
/// assert!(readable(0xffff, 0x0000) && readable(0x0000, 0xfd20));
/// assert!(!readable(0x8410, 0x0000 + 0x39e7), "grey on dark grey");
/// ```
pub fn readable(a: u16, b: u16) -> bool {
    let (hi, lo) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));
    // (hi + 0.05) / (lo + 0.05) >= 4.5, with 0.05 as 5 * 10^6 in the unit above
    10 * (hi + 5_000_000) >= 45 * (lo + 5_000_000)
}

/// The whole-theme checks (module doc) of `t`, a theme file's values over `base`. A text and background pair that
/// is the base's is not checked: the built-ins are trusted, and DARK's white on pure red is 4.0:1 (it is today's
/// look, kept as it is), so a file that leaves the alarm pair alone must not be refused for it.
pub fn check(t: &Theme, base: &Theme) -> Result<(), Refused> {
    let k = &t.ink;
    let at = |i: usize| k.get(i).copied().unwrap_or(0);
    let same = |a: usize, b: usize| k.get(a) == base.ink.get(a) && k.get(b) == base.ink.get(b);
    let pair = |a: usize, b: usize| same(a, b) || readable(at(a), at(b));
    if !(pair(TEXT, BG) && pair(ON_ACC, ACC) && pair(ON_AL, ALARM)) {
        return Err(Refused::Contrast);
    }
    // alarm against the accent, ok, warn, Ferris, the course and the zones (positions 4, and 8 to 19 but the cog's)
    if k.iter().enumerate().any(|(i, &c)| matches!(i, ACC | 8..=10 | 12..) && c == at(ALARM)) {
        return Err(Refused::RedNotAlarm);
    }
    let zones = k.get(ZONE0..).unwrap_or(&[]);
    if zones.iter().enumerate().any(|(i, z)| zones.iter().skip(i + 1).any(|o| o == z)) {
        return Err(Refused::SameZones);
    }
    Ok(())
}

/// A theme file's text (an INI, see features.md for the keys) as a theme: the colours of `[colors]` over those of
/// `[theme] base` (dark or outdoor, else dark); a value that is not `#RRGGBB` or a key not in the table is ignored.
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

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn keys_are_distinct_and_dark_is_the_identity() {
        for (i, k) in KEYS.iter().enumerate() {
            assert!(!KEYS[i + 1..].contains(k), "key {i} repeated");
        }
        for t in [Theme::DARK, Theme::OUTDOOR] {
            assert_eq!(check(&t, &t), Ok(()));
        }
        let identity = |c: u16| Theme::DARK.ink(c);
        assert!(KEYS.iter().all(|&k| identity(k) == k || k == ON_ACCENT || k == ON_ALARM));
        assert_eq!((identity(ON_ACCENT), identity(ON_ALARM)), (BLACK, WHITE));
        assert_eq!(identity(0x1234), 0x1234);
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
        assert_eq!(f("on_alarm=#FF0000"), Err(Refused::Contrast));
        assert_eq!(f("accent=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("zone3=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("ferris=#FF0000"), Err(Refused::RedNotAlarm));
        assert_eq!(f("zone2=#123456\nzone1=#123456"), Err(Refused::SameZones));
        assert!(f("accent=#00AAFF").is_ok());
    }
}
