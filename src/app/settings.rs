//! The settings and where they are kept: [`Settings`] (wheel, backlight, heart rate, power, alerts) and the ride
//! pages' [`PageCfg`], packed into RTC backup registers (which survive standby) until the eMMC holds a settings
//! file (BOARD.md 8: none used by stock). The app's backup registers are all named here, so two features never take
//! the same one; the paired sensors' are [`sensors::Kind::backup`].

use crate::*;

/// A ride page: its layout (an id in [`ui::LAYOUTS`], 0: off) and what each field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCfg {
    /// Layout id ([`ui::layout`]).
    pub layout: u8,
    /// The fields, the first `layout` of them used.
    pub fields: [Field; 6],
}

impl PageCfg {
    /// As kept in two backup registers, behind tags 0xd and 0xe in their top nibbles: the layout in bits 24..27 of
    /// the first, then a byte per field, three in each word. (Builds of 2026-10-23 used tag 0xb in both, from register
    /// 19: [`PageCfg::from_words_b`].)
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, ui::Field};
    /// let p = PageCfg { layout: 6, fields: [Field::Power, Field::Hr, Field::Temp, Field::Map, Field::Dist, Field::Time] };
    /// assert_eq!(PageCfg::from_words(p.to_words()), Some(p));
    /// assert_eq!(PageCfg::from_words([0, 0]), None);
    /// ```
    pub fn to_words(self) -> [u32; 2] {
        let bytes = |f: &[Field]| f.iter().enumerate().fold(0, |w, (i, &f)| w | (f as u32) << (8 * i));
        let (a, b) = self.fields.split_at(3);
        [(0xd << 28) | (u32::from(self.layout) << 24) | bytes(a), (0xe << 28) | bytes(b)]
    }

    /// The page kept in `w`, if [`PageCfg::to_words`] wrote it.
    pub fn from_words(w: [u32; 2]) -> Option<Self> {
        Self::tagged(w, [0xd, 0xe])
    }

    /// A page as builds of 2026-10-23 kept it (tag 0xb in both words).
    pub fn from_words_b(w: [u32; 2]) -> Option<Self> {
        Self::tagged(w, [0xb, 0xb])
    }

    fn tagged(w: [u32; 2], tags: [u32; 2]) -> Option<Self> {
        let layout = (w[0] >> 24 & 0xf) as u8;
        if w[0] >> 28 != tags[0] || w[1] >> 28 != tags[1] || ui::layout(layout).is_none() {
            return None;
        }
        Some(PageCfg { layout, fields: core::array::from_fn(|i| Field::from_index(w[i / 3] >> (8 * (i % 3)) & 0xff)) })
    }

    /// A page as earlier builds kept it in one register: tag 0xa, the layout, 4 bits per field.
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, ui::Field};
    /// let p = PageCfg::from_word(0xa300_0021).unwrap();
    /// assert_eq!((p.layout, &p.fields[..3]), (3, &[Field::Hr, Field::Speed, Field::Time][..]));
    /// ```
    pub fn from_word(w: u32) -> Option<Self> {
        let layout = (w >> 24 & 0xf) as u8;
        if w >> 28 != 0xa || ui::layout(layout).is_none() {
            return None;
        }
        Some(PageCfg { layout, fields: core::array::from_fn(|i| Field::from_index(w >> (4 * i) & 0xf)) })
    }
}

/// Ride pages at most.
pub const PAGES: usize = 3;

/// The overview's fields are kept as page [`OVERVIEW`] (four fields, never off), after the ride pages.
pub const OVERVIEW: usize = PAGES;

/// [`Screen::Ride`]'s number for the course page (its fields fixed: the profile, the distances, time and speed), after the ride pages in their round while a
/// course is followed.
pub const COURSE: usize = PAGES + 1;

/// The backup registers of the ride pages and the overview, two each ([`PageCfg::to_words`]).
/// Registers 20 to 31 are not referenced by stock (R039; ghidra/HARDWARE.md, RTC and backup registers), 19 is: its
/// boot reads it (AGENTS.md, "Share the storage with stock").
pub(crate) const PAGE_BKP: [[usize; 2]; PAGES + 1] = [[20, 21], [22, 23], [24, 25], [26, 27]];

/// Where builds of 2026-10-23 kept them (from 19, which stock reads: moved, and 19 given back).
pub(crate) const PAGE_BKP_B: [[usize; 2]; PAGES + 1] = [[19, 20], [21, 22], [23, 24], [25, 26]];

/// The backup register stock reads at boot (with 15: whether to keep its last battery voltage); stock's own first
/// boot writes 0 there.
pub(crate) const STOCK_BKP19: usize = 19;

/// Where earlier builds kept the ride pages, one register each ([`PageCfg::from_word`]): read when the new ones
/// are empty.
pub(crate) const OLD_PAGE_BKP: [usize; PAGES] = [14, 15, 16];

/// The backup register of the wheel size, the backlight, max heart rate and FTP: see [`Settings`].
pub(crate) const WHEEL_BKP: usize = 11;

/// The backup register of the rider's weight and W': see [`Settings::power_word`]. Not 12, which looks unused but
/// holds stock's Nordic image number (BOARD.md 8, 11): 16 is ours (page 3 of earlier builds, read only to carry it
/// over; a tagged word here is not a page, [`PageCfg::from_word`] wants tag 0xa).
pub(crate) const POWER_BKP: usize = 16;

/// The backup register of the fault marker (hw.rs default_handler): 0xfa in the top byte, the faults in a row in
/// the low byte; said at start, cleared after [`FAULT_CLEAR_MS`] of running.
pub const FAULT_BKP: usize = 29;

/// Running this long after a fault clears its count (a fault after it is the first in a row again).
pub const FAULT_CLEAR_MS: u32 = 60_000;

/// The backup register of the alerts ([`Settings::alerts_word`]); not referenced by stock (BOARD.md 8).
pub(crate) const ALERTS_BKP: usize = 28;

/// The backup register that says a ride is open on the card (recorder.rs): [`RIDE_OPEN`] from its start until it
/// is saved, so the boot looks for it; not referenced by stock (BOARD.md 8). Lost with the backup domain (a flat
/// battery): the next Start then finds the ride from its record.
pub(crate) const RIDE_BKP: usize = 30;

/// See [`RIDE_BKP`].
pub(crate) const RIDE_OPEN: u32 = 0x5243_0001;

/// The small settings kept in RTC backup register 11, as one word behind a tag byte:
/// - tag 0xa7 (this layout): bits 0..2 the wheel's index in [`WHEELS`], 3..5 the backlight level (an index in
///   [`LIGHT_LEVELS`], or [`LIGHT_AUTO`]), 6..7 its time-out (an index in [`LIGHT_TIMEOUTS_S`]), 8..15 max heart
///   rate less 100, 16..23 FTP in 5 W;
/// - tag 0xa6 (earlier builds): the wheel's index in bits 0..2, max heart rate and FTP as 4-bit codes in 4..7 and
///   8..11 ([`code_hr`], [`code_ftp`]), the backlight level in 16..18 and its time-out in 20..21; read only;
/// - tag 0xa5 (written by earlier builds): the wheel in mm in bits 0..11, the backlight level in 16..19 and its
///   time-out in 20..23; read, never written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Wheel circumference, mm (one of [`WHEELS`]).
    pub wheel_mm: u16,
    /// Max heart rate, bpm (in [`menu::MAX_HR_RANGE`]).
    pub max_hr: u16,
    /// FTP, W (in [`menu::FTP_RANGE`], a multiple of 5).
    pub ftp: u16,
    /// Backlight level: an index in [`LIGHT_LEVELS`], or [`LIGHT_AUTO`].
    pub light_level: usize,
    /// Backlight time-out: an index in [`LIGHT_TIMEOUTS_S`].
    pub light_timeout: usize,
    /// Rider weight, kg (in [`menu::WEIGHT_RANGE`]); kept in register 16 ([`Settings::power_word`]).
    pub weight_kg: u16,
    /// W', J (in [`menu::WPRIME_RANGE`], a multiple of 500); kept in register 16.
    pub wprime_j: u16,
    /// Eat and drink reminders, minutes of ride time (0: off), and the heart rate alert, bpm (0: off); kept in
    /// register 28 ([`Settings::alerts_word`]).
    pub eat_min: u16,
    /// See `eat_min`.
    pub drink_min: u16,
    /// See `eat_min`.
    pub hr_alert: u16,
    /// The power the workout gauge shows ([`Field::WktPower`]): an index in [`WKT_AVGS`]; kept in register 16.
    pub wkt_avg: usize,
}

/// The power windows the workout gauge offers, as its menu row and its title show them: 3, 10 and 30 s averages
/// (power.rs [`power::Power::rolling`] 0, 1, 2), then the power each second. 3 s first: the default.
pub const WKT_AVGS: [&str; 4] = ["3S", "10S", "30S", "NOW"];

impl Settings {
    /// The settings in `w` (defaults for an empty or foreign word).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, WHEELS};
    /// let m = Settings { wheel_mm: WHEELS[3], max_hr: 187, ftp: 265, light_level: 4, light_timeout: 3, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(m.to_word()), m);
    /// // words from earlier builds: 2136 mm, AUTO, 60 s; then max HR code 7 (225), FTP code 11 (100)
    /// let old = Settings::from_word(0xa5 << 24 | 3 << 20 | 4 << 16 | 2136);
    /// assert_eq!((old.wheel_mm, old.light_level, old.light_timeout, old.max_hr), (2136, 4, 3, 190));
    /// let old = Settings::from_word(0xa6 << 24 | 11 << 8 | 7 << 4 | 3);
    /// assert_eq!((old.wheel_mm, old.max_hr, old.ftp), (WHEELS[3], 225, 100));
    /// ```
    pub fn from_word(w: u32) -> Settings {
        let (weight_kg, wprime_j) = (power::WEIGHT_KG, power::WPRIME_J);
        let wheel = |mm: u16| if WHEELS.contains(&mm) { mm } else { WHEELS[1] };
        let level = |l: u32| (l as usize) % (LIGHT_LEVELS.len() + 1);
        let index = |i: u32| WHEELS.get(i as usize).copied().unwrap_or(WHEELS[1]);
        match w >> 24 {
            0xa7 => Settings {
                wheel_mm: index(w & 7),
                max_hr: (100 + (w >> 8 & 0xff) as u16).clamp(*menu::MAX_HR_RANGE.start(), *menu::MAX_HR_RANGE.end()),
                ftp: (5 * (w >> 16 & 0xff) as u16).clamp(*menu::FTP_RANGE.start(), *menu::FTP_RANGE.end()),
                light_level: level(w >> 3 & 7),
                light_timeout: (w >> 6 & 3) as usize,
                weight_kg,
                wprime_j,
                eat_min: 0,
                drink_min: 0,
                hr_alert: 0,
                wkt_avg: 0,
            },
            0xa6 => Settings {
                wheel_mm: index(w & 7),
                max_hr: code_hr(w >> 4 & 0xf),
                ftp: code_ftp(w >> 8 & 0xf),
                light_level: level(w >> 16 & 7),
                light_timeout: (w >> 20 & 3) as usize,
                weight_kg,
                wprime_j,
                eat_min: 0,
                drink_min: 0,
                hr_alert: 0,
                wkt_avg: 0,
            },
            0xa5 => Settings {
                wheel_mm: wheel((w & 0xfff) as u16),
                max_hr: ride::MAX_HR,
                ftp: ride::FTP,
                light_level: level(w >> 16 & 0xf),
                light_timeout: (w >> 20 & 0xf) as usize % LIGHT_TIMEOUTS_S.len(),
                weight_kg,
                wprime_j,
                eat_min: 0,
                drink_min: 0,
                hr_alert: 0,
                wkt_avg: 0,
            },
            _ => Settings {
                wheel_mm: WHEELS[1],
                max_hr: ride::MAX_HR,
                ftp: ride::FTP,
                light_level: 0,
                light_timeout: 0,
                weight_kg,
                wprime_j,
                eat_min: 0,
                drink_min: 0,
                hr_alert: 0,
                wkt_avg: 0,
            },
        }
    }

    /// The alerts kept in register 28: tag 0xc2 in the top byte, eat and drink reminders in 5 min steps in bits 0..4
    /// and 8..12, the heart rate alert in 16..23 (0: off for each).
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let s = Settings { eat_min: 45, drink_min: 20, hr_alert: 175, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(0).with_alerts_word(s.alerts_word()), s);
    /// assert_eq!(Settings::from_word(0).with_alerts_word(0).eat_min, 0, "off without the tag");
    /// ```
    pub fn alerts_word(self) -> u32 {
        (0xc2 << 24)
            | u32::from(self.hr_alert.min(255)) << 16
            | (u32::from(self.drink_min / 5) & 0x1f) << 8
            | (u32::from(self.eat_min / 5) & 0x1f)
    }

    /// Self with the alerts of register 28 ([`Settings::alerts_word`]).
    pub fn with_alerts_word(self, w: u32) -> Settings {
        if w >> 24 != 0xc2 {
            return Settings { eat_min: 0, drink_min: 0, hr_alert: 0, ..self };
        }
        let min = |v: u32| ((v & 0x1f) as u16 * 5).min(120);
        Settings { eat_min: min(w), drink_min: min(w >> 8), hr_alert: (w >> 16 & 0xff) as u16, ..self }
    }

    /// These settings with the weight, W' and the workout gauge's window kept in `w` (RTC backup register 16): tag
    /// 0xc1 in bits 24..31, the weight in kg in 0..7, W' in 0.5 kJ in 8..15, the gauge's window in 16..17 (0 in words
    /// written before it: 3 s). Without the tag, the defaults ([`power::WEIGHT_KG`], [`power::WPRIME_J`], 3 s).
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let s = Settings { weight_kg: 68, wprime_j: 23_500, wkt_avg: 3, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(0).with_power_word(s.power_word()), s);
    /// assert_eq!(Settings::from_word(0).with_power_word(0).wprime_j, 20_000);
    /// ```
    pub fn with_power_word(self, w: u32) -> Settings {
        if w >> 24 != 0xc1 {
            return Settings { weight_kg: power::WEIGHT_KG, wprime_j: power::WPRIME_J, wkt_avg: 0, ..self };
        }
        let (kg, wp) = (menu::WEIGHT_RANGE, menu::WPRIME_RANGE);
        Settings {
            weight_kg: ((w & 0xff) as u16).clamp(*kg.start(), *kg.end()),
            wprime_j: (500 * (w >> 8 & 0xff) as u16).clamp(*wp.start(), *wp.end()),
            wkt_avg: (w >> 16 & 3) as usize,
            ..self
        }
    }

    /// The weight, W' and the gauge's window as kept in register 16 ([`Settings::with_power_word`]).
    pub fn power_word(self) -> u32 {
        (0xc1 << 24)
            | (self.wkt_avg as u32 & 3) << 16
            | (u32::from(self.wprime_j / 500) & 0xff) << 8
            | u32::from(self.weight_kg) & 0xff
    }

    /// As kept (tag 0xa7).
    pub fn to_word(self) -> u32 {
        let wheel = WHEELS.iter().position(|&w| w == self.wheel_mm).unwrap_or(1) as u32;
        (0xa7 << 24)
            | (u32::from(self.ftp / 5) & 0xff) << 16
            | (u32::from(self.max_hr.saturating_sub(100)) & 0xff) << 8
            | (self.light_timeout as u32 & 3) << 6
            | (self.light_level as u32 & 7) << 3
            | wheel
    }
}

impl Settings {
    /// Setting `s`'s value as its menu row shows it.
    pub fn text(&self, s: Setting, l: &mut Line<12>) {
        match s {
            Setting::Wheel => l.push_u32(u32::from(self.wheel_mm)),
            Setting::Light => match LIGHT_LEVELS.get(self.light_level) {
                Some(&v) => l.push_u32(u32::from(v)),
                None => l.push_str("AUTO"),
            },
            Setting::LightOff => match LIGHT_TIMEOUTS_S.get(self.light_timeout) {
                Some(0) | None => l.push_str("NEVER"),
                Some(&t) => {
                    l.push_u32(t);
                    l.push_str(" S");
                }
            },
            Setting::MaxHr => l.push_u32(u32::from(self.max_hr)),
            Setting::Ftp => l.push_u32(u32::from(self.ftp)),
            Setting::Weight => l.push_u32(u32::from(self.weight_kg)),
            Setting::WPrime => {
                l.push_u32(u32::from(self.wprime_j / 1000));
                l.push_str(".");
                l.push_u32(u32::from(self.wprime_j % 1000 / 100));
            }
            Setting::Eat | Setting::Drink | Setting::HrAlert => {
                let v = match s {
                    Setting::Eat => self.eat_min,
                    Setting::Drink => self.drink_min,
                    _ => self.hr_alert,
                };
                if v == 0 { l.push_str("OFF") } else { l.push_u32(u32::from(v)) }
            }
            Setting::WktAvg => l.push_str(WKT_AVGS.get(self.wkt_avg).copied().unwrap_or("")),
        }
    }

    /// A choice to its next value (round), as Select does.
    ///
    /// ```
    /// use bryton_rustfw::{Settings, LIGHT_LEVELS, menu::Setting};
    /// let mut s = Settings::from_word(0);
    /// for _ in 0..LIGHT_LEVELS.len() + 1 {
    ///     s.cycle(Setting::Light);
    /// }
    /// assert_eq!(s.light_level, 0, "round");
    /// ```
    pub fn cycle(&mut self, s: Setting) {
        match s {
            Setting::Wheel => {
                let i = WHEELS.iter().position(|&w| w == self.wheel_mm).map_or(0, |i| (i + 1) % WHEELS.len());
                self.wheel_mm = WHEELS.get(i).copied().unwrap_or(WHEELS[1]);
            }
            Setting::Light => self.light_level = (self.light_level + 1) % (LIGHT_LEVELS.len() + 1),
            Setting::LightOff => self.light_timeout = (self.light_timeout + 1) % LIGHT_TIMEOUTS_S.len(),
            Setting::WktAvg => self.wkt_avg = (self.wkt_avg + 1) % WKT_AVGS.len(),
            _ => {} // numbers: Setting::number
        }
    }

    /// A number one step up or down, held in its range ([`Setting::number`]).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, menu::Setting};
    /// let mut s = Settings::from_word(0);
    /// s.step(Setting::Ftp, true);
    /// s.step(Setting::MaxHr, false);
    /// assert_eq!((s.ftp, s.max_hr), (205, 189));
    /// ```
    pub fn step(&mut self, s: Setting, up: bool) {
        let (Some((range, step)), Some(v)) = (s.number(), self.number_mut(s)) else { return };
        *v = if up { v.saturating_add(step) } else { v.saturating_sub(step) }.clamp(*range.start(), *range.end());
    }

    fn number_mut(&mut self, s: Setting) -> Option<&mut u16> {
        match s {
            Setting::MaxHr => Some(&mut self.max_hr),
            Setting::Ftp => Some(&mut self.ftp),
            Setting::Weight => Some(&mut self.weight_kg),
            Setting::WPrime => Some(&mut self.wprime_j),
            Setting::Eat => Some(&mut self.eat_min),
            Setting::Drink => Some(&mut self.drink_min),
            Setting::HrAlert => Some(&mut self.hr_alert),
            _ => None,
        }
    }
}

/// Max heart rate from its 4-bit code in a tag 0xa6 word: 150 to 225 in steps of 5, 0 being stock's default, 190
/// ([`ride::MAX_HR`]).
///
/// ```
/// use bryton_rustfw::code_hr;
/// assert_eq!([0, 7, 8, 15].map(code_hr), [190, 225, 150, 185]);
/// ```
pub fn code_hr(bits: u32) -> u16 {
    150 + 5 * ((bits + 8) % 16) as u16
}

/// FTP from its 4-bit code in a tag 0xa6 word: 100 to 400 W in steps of 20, 0 being [`ride::FTP`] (200 W).
///
/// ```
/// use bryton_rustfw::code_ftp;
/// assert_eq!([0, 10, 11, 15].map(code_ftp), [200, 400, 100, 180]);
/// ```
pub fn code_ftp(bits: u32) -> u16 {
    100 + 20 * ((bits + 5) % 16) as u16
}

/// Backlight levels offered, percent (stock: 1 to 100). One more choice follows them: [`LIGHT_AUTO`].
pub const LIGHT_LEVELS: [u8; 4] = [100, 60, 30, 10];

/// The backlight setting that follows the ambient light ([`ambient::Auto`]): the index after [`LIGHT_LEVELS`].
pub const LIGHT_AUTO: usize = LIGHT_LEVELS.len();

/// Backlight time-outs offered, seconds without a key press (0: never off).
pub const LIGHT_TIMEOUTS_S: [u32; 4] = [0, 10, 30, 60];

/// Wheel circumferences offered (mm): 700 x 23, 25, 28, 32, 35, 38 C and 650B x 47 (ANT+ / tyre makers' tables).
pub const WHEELS: [u16; 7] = [2096, 2105, 2136, 2155, 2168, 2180, 2079];

/// The ride pages before any are set (or imported from stock), after what Garmin's, Wahoo's and Hammerhead's bike
/// computers ship (2026-10-24: Wahoo's default stack is a data page, an elevation page and a map page with an
/// elevation profile and a few fields; Garmin's map page, when navigating, distance and time to the destination):
/// the ride's essentials, the climbing, the map whole; the overview's totals last.
pub(crate) const DEFAULT_PAGES: [PageCfg; PAGES + 1] = {
    use Field::*;
    [
        PageCfg { layout: 6, fields: [Speed, Timer, Dist, AvgSpeed, Hr, Power] },
        PageCfg { layout: 5, fields: [AltGraph, Ascent, Grade, Vam, Cadence, Time] },
        PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
        PageCfg { layout: 4, fields: [WeekKm, WeekH, TotalKm, Temp, Time, Time] },
    ]
};

/// The course page, an extra page while a course is followed ([`COURSE`]; Garmin and Wahoo add theirs the same
/// way): the course's profile large, the next turn and the distance to go, the distance and the moving time (the
/// average speed gave its box to the next turn: five boxes, no smaller). Not configurable.
pub(crate) const COURSE_PAGE: PageCfg = {
    use Field::*;
    PageCfg { layout: 5, fields: [Profile, NextTurn, ToGo, Dist, Timer, Time] }
};

/// The backup registers of the GNSS aiding (the RTC's offset from UTC, the last position).
pub(crate) const AID_BKP: [usize; 2] = [17, 18];

/// Writes ride page (or overview) `n`'s configuration to its backup registers.
pub(crate) fn save_page(b: &mut impl Board, n: usize, page: &PageCfg) {
    if let Some(regs) = PAGE_BKP.get(n) {
        for (r, w) in regs.iter().zip(page.to_words()) {
            b.backup_write(*r, w);
        }
    }
}

impl App {
    /// Writes the settings to their backup registers.
    pub(crate) fn save_misc(&self, b: &mut impl Board) {
        b.backup_write(WHEEL_BKP, self.settings.to_word());
        b.backup_write(POWER_BKP, self.settings.power_word());
        b.backup_write(ALERTS_BKP, self.settings.alerts_word());
    }

    /// The ride page settings.
    pub fn pages(&self) -> &[PageCfg; PAGES + 1] {
        &self.pages
    }
}
