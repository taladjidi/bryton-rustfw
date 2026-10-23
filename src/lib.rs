//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod alerts;
pub mod dev;
pub mod drivers;
pub mod hrv;
pub mod power;
pub mod power_curve;
pub mod ride;
#[cfg(test)]
mod sim;
pub mod storage;
pub mod ui;
pub mod workout;

// the modules by their own names too (crate::ambient, bryton_rustfw::fit): the folders only sort the files
pub use drivers::{ambient, battery, emmc, gnss, nrf, panel, sensors, touch};
pub use storage::{fat, fit, fitread, history, ini, stock};
pub use ui::{fonts, map, menu, text, tiles};

use menu::{Input, Setting};
use nrf::SensorId;
use panel::Panel;
use sensors::{Decoders, Kind, Live, Metric};
use text::Line;
use touch::{Dir, Gesture};
use ui::{Canvas, Field, Rec, Row, Strip, UsbDot};

/// The hardware the app needs, as primitive operations. Each one is a few register writes on the unit.
// ponytail: one trait while the board is small; split per peripheral (Panel, Rtc, Keys...) when it grows.
pub trait Board {
    /// Panel power on and reset pulse, with the waits the panel needs before its first command.
    fn panel_power_on(&mut self);
    /// One panel command and its parameters (D/C low for the command byte, high for the parameters).
    fn panel_cmd(&mut self, cmd: u8, params: &[u8]);
    /// RAMWR (0x2c) then `count` pixels, each from `next` (called once per pixel, in order), into the window set by
    /// CASET / RASET.
    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16);
    /// RAMWR then `count` pixels of one colour (a fill: no per-pixel callback on the unit).
    fn panel_fill(&mut self, count: u32, color: u16) {
        self.panel_ram_write(count, &mut || color);
    }
    /// Backlight at `level` percent (0: off).
    fn backlight(&mut self, level: u8);
    /// The buzzer sounding a tone at `hz` (0: silence).
    fn beep(&mut self, hz: u32);
    /// The keys held down, as a mask of [`Key`] codes.
    fn keys_down(&mut self) -> u8;
    /// RTC_TR and RTC_DR, read once the shadow registers are in sync (RSF): the time of day and the date, BCD.
    fn rtc(&mut self) -> (u32, u32);
    /// Busy wait.
    fn delay_ms(&mut self, ms: u32);
    /// Standby, woken by the power button (a reset). Never returns on the unit; the simulation records it.
    fn standby(&mut self);
    /// Nordic chip: resets it as stock does with its UART up (BOARD.md 11), and returns once it can take commands.
    fn nrf_reset(&mut self);
    /// Bytes to the Nordic chip, in order (the chip's flow control may hold them a little).
    fn nrf_send(&mut self, data: &[u8]);
    /// Bytes received from the Nordic chip, at most `buf.len()`; never waits.
    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize;
    /// The battery as ADC3 reads it (BOARD.md 12): the battery channel, VREFINT, and VREFINT_CAL (VREFINT at
    /// VDDA = 3.3 V), all 16-bit; None if the ADC did not come up. Takes about half a millisecond.
    fn battery_adc(&mut self) -> Option<(u16, u16, u16)>;
    /// GNSS receiver power (PI6, BOARD.md 13).
    fn gnss_power(&mut self, on: bool);
    /// Exchanges `buf` with the GNSS receiver over SPI, in place; `buf.len()` is a multiple of 4. The receiver
    /// sends 0xff when it has nothing.
    fn gnss_transfer(&mut self, buf: &mut [u8]);
    /// USB power (charger) present.
    fn charger_present(&mut self) -> bool;
    /// RTC backup register `n` (0..32): kept through resets and power off (BOARD.md 8).
    fn backup_read(&mut self, n: usize) -> u32;
    /// Writes RTC backup register `n`.
    fn backup_write(&mut self, n: usize, v: u32);
    /// eMMC controller (SDMMC1, BOARD.md 15) from reset: pins, kernel clock, powered, bus 1 bit at 400 kHz, and the
    /// wait the card needs before its first command. The card keeps the state it was in.
    fn emmc_power_on(&mut self);
    /// eMMC bus clock: 400 kHz (identification) or 25 MHz (`fast`); width 1 or 4 bits (`wide`).
    fn emmc_bus(&mut self, fast: bool, wide: bool);
    /// Sends eMMC command `index` with `arg` and waits for the response `resp` names: its words (R2: bits 127..96
    /// first; others: word 0), all zero for [`emmc::Resp::None`].
    fn emmc_cmd(&mut self, index: u8, arg: u32, resp: emmc::Resp) -> Result<[u32; 4], emmc::BusError>;
    /// Sends eMMC command `index` with `arg`, a read of `buf.len()` bytes (whole 512-byte blocks) whose R1 answer
    /// it returns, and reads the data into `buf`, every wait bounded.
    fn emmc_read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, emmc::BusError>;
    /// Reads `buf.len()` bytes from register `reg` on, of the chip at 7-bit address `addr` on the sensor bus (I2C4,
    /// BOARD.md 16); false if it did not answer (no acknowledge, or a time-out: it never hangs).
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool;
    /// Writes `data` to register `reg` on, of the chip at `addr`; false if it did not answer.
    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool;
    /// Reads `buf.len()` bytes from register `reg` on, of the touch controller (I2C1 address 0x55, BOARD.md 17);
    /// false if it did not answer.
    fn touch_read(&mut self, reg: u8, buf: &mut [u8]) -> bool;
    /// The touch controller's reset line (PH3): `run` releases it, false holds the chip in reset.
    fn touch_reset(&mut self, run: bool);
    /// The touch controller pulls its INT line (PC6) low: it has a point to read.
    fn touch_int(&mut self) -> bool;
}

// ---------------------------------------------------------------------------------------------------------------
// RTC
// ---------------------------------------------------------------------------------------------------------------

/// Two BCD digits to binary.
///
/// ```
/// assert_eq!(bryton_rustfw::bcd(0x59), 59);
/// ```
pub fn bcd(v: u32) -> u8 {
    ((v >> 4) * 10 + (v & 0xf)) as u8
}

/// RTC_TR (RM0399: BCD hours in bits 16..21, minutes 8..14, seconds 0..6; PM at bit 22 is ignored, the
/// stock firmware runs the RTC in 24 h format) to (hours, minutes, seconds).
///
/// ```
/// assert_eq!(bryton_rustfw::rtc_time_from_tr(0x0023_5959), (23, 59, 59));
/// ```
pub fn rtc_time_from_tr(tr: u32) -> (u8, u8, u8) {
    (bcd((tr >> 16) & 0x3f), bcd((tr >> 8) & 0x7f), bcd(tr & 0x7f))
}

/// RTC_DR (RM0399: BCD year 00-99 in bits 16..23, month 8..12, day 0..5) to (year, month, day), years 2000-2099.
///
/// ```
/// assert_eq!(bryton_rustfw::rtc_date_from_dr(0x0026_5023), (2026, 10, 23)); // weekday bits 13..15 ignored
/// ```
pub fn rtc_date_from_dr(dr: u32) -> (u16, u8, u8) {
    (2000 + u16::from(bcd((dr >> 16) & 0xff)), bcd((dr >> 8) & 0x1f), bcd(dr & 0x3f))
}

/// Days since 1970-01-01 of a date (proleptic Gregorian; H. Hinnant's days_from_civil).
pub fn days_from_civil(y: u16, m: u8, d: u8) -> i32 {
    let y = i32::from(y) - i32::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i32::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i32::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The date `days` after 1970-01-01 (the inverse of [`days_from_civil`]).
///
/// ```
/// use bryton_rustfw::{civil_from_days, days_from_civil};
/// assert_eq!(days_from_civil(2026, 10, 23), 20_749);
/// assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
/// ```
pub fn civil_from_days(days: i32) -> (u16, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((yoe + era * 400 + i32::from(m <= 2)) as u16, m, d)
}

/// A date and time as seconds since 1970 (no leap seconds).
pub fn epoch_s((y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8)) -> i64 {
    i64::from(days_from_civil(y, mo, d)) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s)
}

/// Seconds since 1970 back to a date and time.
///
/// ```
/// let t = (2026, 1, 1, 0, 30, 0);
/// assert_eq!(bryton_rustfw::from_epoch_s(bryton_rustfw::epoch_s(t) - 3600), (2025, 12, 31, 23, 30, 0));
/// ```
pub fn from_epoch_s(t: i64) -> (u16, u8, u8, u8, u8, u8) {
    let (days, s) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days as i32);
    (y, mo, d, (s / 3600) as u8, (s / 60 % 60) as u8, (s % 60) as u8)
}

// ---------------------------------------------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------------------------------------------

/// The four keys, with stock's key codes (BOARD.md 5) as their bit in [`Board::keys_down`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Back / power (PA1).
    Back = 0x01,
    /// Lap (PB2).
    Lap = 0x02,
    /// Start / stop (PB4).
    Start = 0x04,
    /// Page (PE3).
    Page = 0x08,
}

impl Key {
    const ALL: [Key; 4] = [Key::Back, Key::Lap, Key::Start, Key::Page];

    /// How long the key must be held for a long press: Back (power off) longer than the others.
    pub fn long_ms(self) -> u32 {
        if self == Key::Back { 3000 } else { 2000 }
    }

    /// From its name (the dev link's `key` command).
    pub fn from_name(name: &[u8]) -> Option<Key> {
        match name {
            b"back" => Some(Key::Back),
            b"lap" => Some(Key::Lap),
            b"start" => Some(Key::Start),
            b"page" => Some(Key::Page),
            _ => None,
        }
    }
}

/// A key press, short (acted on at the release) or long (acted on once, while still held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Pressed and released before [`Key::long_ms`].
    Short(Key),
    /// Held for [`Key::long_ms`].
    Long(Key),
    /// Still held: one every [`REPEAT_MS`] from [`REPEAT_AFTER_MS`] on (a value editor steps on it). The
    /// [`Press::Short`] or [`Press::Long`] still follows.
    Repeat(Key),
}

/// A held key repeats from then on ([`Press::Repeat`]).
pub const REPEAT_AFTER_MS: u32 = 500;
/// Then once per this.
pub const REPEAT_MS: u32 = 100;

/// How long each key has been held, between polls.
#[derive(Default)]
pub struct Keys {
    held_ms: [u32; 4],
}

impl Keys {
    /// One poll, `dt_ms` after the previous one, with the keys held down (`mask` of [`Key`] codes). One press at
    /// most per poll, the first key in [`Key::ALL`] order.
    pub fn poll(&mut self, mask: u8, dt_ms: u32) -> Option<Press> {
        let mut press = None;
        for (key, held) in Key::ALL.into_iter().zip(&mut self.held_ms) {
            let long = key.long_ms();
            let now = if mask & key as u8 != 0 { *held + dt_ms } else { 0 };
            let p = match (now, *held) {
                (0, 1..) if *held < long => Some(Press::Short(key)),
                (1.., _) if *held < long && now >= long => Some(Press::Long(key)),
                (REPEAT_AFTER_MS.., _) if now / REPEAT_MS != *held / REPEAT_MS => Some(Press::Repeat(key)),
                _ => None,
            };
            *held = now;
            press = press.or(p);
        }
        press
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Settings, kept in RTC backup registers until the eMMC holds a settings file (BOARD.md 8: none used by stock)
// ---------------------------------------------------------------------------------------------------------------

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
/// The backup registers of the ride pages and the overview, two each ([`PageCfg::to_words`]).
/// Registers 20 to 31 are not referenced by stock (R039; ghidra/HARDWARE.md, RTC and backup registers), 19 is: its
/// boot reads it (AGENTS.md, "Share the storage with stock").
const PAGE_BKP: [[usize; 2]; PAGES + 1] = [[20, 21], [22, 23], [24, 25], [26, 27]];
/// Where builds of 2026-10-23 kept them (from 19, which stock reads: moved, and 19 given back).
const PAGE_BKP_B: [[usize; 2]; PAGES + 1] = [[19, 20], [21, 22], [23, 24], [25, 26]];
/// The backup register stock reads at boot (with 15: whether to keep its last battery voltage); stock's own first
/// boot writes 0 there.
const STOCK_BKP19: usize = 19;
/// Where earlier builds kept the ride pages, one register each ([`PageCfg::from_word`]): read when the new ones
/// are empty.
const OLD_PAGE_BKP: [usize; PAGES] = [14, 15, 16];
/// The backup register of the wheel size, the backlight, max heart rate and FTP: see [`Settings`].
const WHEEL_BKP: usize = 11;
/// The backup register of the rider's weight and W': see [`Settings::power_word`]. Not 12, which looks unused but
/// holds stock's Nordic image number (BOARD.md 8, 11): 16 is ours (page 3 of earlier builds, read only to carry it
/// over; a tagged word here is not a page, [`PageCfg::from_word`] wants tag 0xa).
const POWER_BKP: usize = 16;
/// The backup register of the alerts ([`Settings::alerts_word`]); not referenced by stock (BOARD.md 8).
const ALERTS_BKP: usize = 28;

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
}

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

    /// These settings with the weight and W' kept in `w` (RTC backup register 16): tag 0xc1 in bits 24..31, the
    /// weight in kg in 0..7, W' in 0.5 kJ in 8..15. Without the tag, the defaults ([`power::WEIGHT_KG`],
    /// [`power::WPRIME_J`]).
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let s = Settings { weight_kg: 68, wprime_j: 23_500, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(0).with_power_word(s.power_word()), s);
    /// assert_eq!(Settings::from_word(0).with_power_word(0).wprime_j, 20_000);
    /// ```
    pub fn with_power_word(self, w: u32) -> Settings {
        if w >> 24 != 0xc1 {
            return Settings { weight_kg: power::WEIGHT_KG, wprime_j: power::WPRIME_J, ..self };
        }
        let (kg, wp) = (menu::WEIGHT_RANGE, menu::WPRIME_RANGE);
        Settings {
            weight_kg: ((w & 0xff) as u16).clamp(*kg.start(), *kg.end()),
            wprime_j: (500 * (w >> 8 & 0xff) as u16).clamp(*wp.start(), *wp.end()),
            ..self
        }
    }

    /// The weight and W' as kept in register 16 ([`Settings::with_power_word`]).
    pub fn power_word(self) -> u32 {
        (0xc1 << 24) | (u32::from(self.wprime_j / 500) & 0xff) << 8 | u32::from(self.weight_kg) & 0xff
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

/// The ride pages before any are set: speed big with heart rate and cadence (the time is in the strip), then four
/// fields, then the map.
const DEFAULT_PAGES: [PageCfg; PAGES + 1] = {
    use Field::*;
    [
        PageCfg { layout: 3, fields: [Speed, Hr, Cadence, Time, Time, Time] },
        PageCfg { layout: 4, fields: [Speed, Hr, Cadence, Power, Time, Time] },
        PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
        PageCfg { layout: 4, fields: [WeekKm, WeekH, TotalKm, Temp, Time, Time] },
    ]
};

// ---------------------------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------------------------

/// Tones played at power-on and power-off: (Hz, ms). Rising, then falling (stock's own melodies are not traced).
pub const BEEP_ON: [(u32, u32); 2] = [(1500, 80), (2500, 120)];
/// The power-off tones.
pub const BEEP_OFF: [(u32, u32); 2] = [(2500, 80), (1500, 120)];

/// An alert's tones: three short beeps (0 Hz: silence), played without blocking ([`App::tones`]).
pub const BEEP_ALERT: [(u32, u32); 5] = [(2500, 100), (0, 50), (2500, 100), (0, 50), (2500, 100)];

/// Plays `tones` (blocking: a few hundred ms, only at power-on and power-off).
fn play(b: &mut impl Board, tones: &[(u32, u32)]) {
    for &(hz, ms) in tones {
        b.beep(hz);
        b.delay_ms(ms);
    }
    b.beep(0);
}

/// Main loop period: the time and the keys are read this often.
pub const POLL_MS: u32 = 50;

/// How long a BLE scan runs, started when the device list opens. Scanning keeps the Nordic radio on, so it is short.
pub const SCAN_S: u16 = 10;

/// The device list and the sensors menu are redrawn at most this often while reports come in.
pub const LIST_REDRAW_MS: u32 = 1000;

/// How often the battery is read.
pub const BATTERY_MS: u32 = 10_000;

/// How often the touch panel is read while a finger is on it: a swipe of 100 ms gives ten points.
pub const TOUCH_MS: u32 = 10;

/// How often the ambient light is read while the backlight follows it (and while the menu shows it): with the
/// smoothing ([`ambient::Auto`]), a change of light reaches the backlight in about a second.
pub const LIGHT_MS: u32 = 250;

/// When the GNSS receiver is powered, counted from the app's first step: its power line (PI6) has then been
/// driven low for over a second (the board does it at boot). After a reset the line is left undriven, and powered
/// again ~100 ms after being driven low, the receiver did not start (seen on the unit, BOARD.md 13).
pub const GNSS_ON_MS: u32 = 1000;

/// When the GNSS receiver gets its configuration, counted from the app's first step (1 s after its power-up;
/// stock waits 3 s before reading it at all).
pub const GNSS_CONFIG_MS: u32 = GNSS_ON_MS + 1000;

/// The backup registers of the GNSS aiding (the RTC's offset from UTC, the last position).
const AID_BKP: [usize; 2] = [17, 18];

/// A fix older than this is dropped (the receiver stopped answering).
pub const PVT_STALE_MS: u32 = 3000;

/// GNSS bytes read per pass at most: 128 per 50 ms is 2560 bytes/s, twenty times NAV-PVT at 1 Hz. A pass stops at
/// the first 32 bytes of idle fill between frames, so most passes read 32.
const GNSS_READ: usize = 128;

/// How long a sensor's channel rests, closed, after its search timed out ([`nrf::SEARCH_TIMEOUT`]) before it
/// searches again: so a strap not worn costs the radio 30 s of listening in 90 instead of all the time.
// ponytail: fixed; during a ride a lost sensor should come back sooner (search again at once on the first drop-out)
pub const SEARCH_REST_MS: u32 = 60_000;

/// A sensor's channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    /// Closed.
    Off,
    /// Open for any sensor of its kind: the first one heard is paired.
    Search,
    /// Open for this sensor.
    Paired(SensorId),
}

/// The menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Sensors, pages, wheel, nearby.
    Main,
    /// One row per sensor kind.
    Sensors,
    /// Ride page `n`'s layout and fields.
    Page(usize),
    /// What the GNSS receiver reports.
    Gps,
    /// What stock's rides and settings give ([`menu::STOCK`]).
    Stock,
    /// Reminders and alerts ([`menu::ALERTS`]).
    Alerts,
    /// Stock's rides, newest first ([`history`]).
    History,
    /// The workouts planned from today on, one row each.
    Workouts,
    /// The steps of the workout opened from [`Menu::Workouts`].
    Steps,
}

/// What is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The main screen: the time, big, and Ferris.
    Main,
    /// Ride page `n`.
    Ride(usize),
    /// A menu, with the selected row.
    Menu(Menu, usize),
    /// The BLE devices heard.
    Nearby,
    /// The card of the ride just ended: its map, elevation profile and numbers.
    Summary,
    /// The card of a past ride ([`App::past`]); Back returns to the history.
    Past,
}

/// Stock's rides are read by themselves this long after start-up (for the totals), unless a ride runs.
pub const STOCK_SCAN_MS: u32 = 30_000;

/// During a ride, holding Back a second time within this long switches off (the first hold only warns).
pub const OFF_CONFIRM_MS: u32 = 10_000;

/// The ride catches up at most this many seconds after a long pass ([`App::step`]).
pub const MAX_CATCH_UP: u8 = 10;

/// How long a banner stays up, unless an input dismisses it first.
pub const BANNER_MS: u32 = 4000;

/// The GPS row of the main menu.
pub const GPS_ROW: usize = 13;
/// The nearby BLE devices row of the main menu.
pub const NEARBY_ROW: usize = 14;
const _: () = assert!(matches!(menu::MAIN[GPS_ROW], menu::Row::Open(Menu::Gps)));
const _: () = assert!(matches!(menu::MAIN[NEARBY_ROW], menu::Row::Nearby));

/// What a graph field shows on screen: its plot, over the last minutes or the whole ride, the bars up to `n`
/// ([`ride::History::len`] or [`ride::History::whole_len`]), the whole ride's halvings and the scale (low, high, in
/// the plot's units). A change of anything but `n` redraws the graph whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GraphShown {
    plot: ride::Plot,
    whole: bool,
    n: u32,
    halved: u32,
    scale: (u32, u32),
}

/// What a page box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Widget {
    /// The big time, each digit in its own cell.
    Clock,
    /// The date line.
    Date,
    /// The latest notices, or Ferris when there are none.
    Notices,
    /// A ride field in its box: a value, the map or the graph.
    Field(Field),
}

/// A page box as drawn: each pass compares it with what the box would show now and redraws only the difference
/// (the digits, the characters, the map's new points, the graph's columns that changed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Drawn {
    /// Nothing that changes.
    #[default]
    Fixed,
    /// The clock's digits: H H M M.
    Digits([u8; 4]),
    /// A fingerprint of what the box shows, redrawn whole when it changes (the date, the notices).
    Print(u32),
    /// A value and its colour.
    Value(Line<8>, u16),
    /// A map.
    Map(map::Shown),
    /// A graph.
    Graph(GraphShown),
}

/// Box `i` of a page with this many fields (the ride pages' and the overview's grids).
type BoxAt = fn(u8, usize) -> Option<(u32, u32, u32, u32)>;

/// Boxes on a page at most.
const BOXES: usize = 8;

/// The app: what is on screen, the keys, the sensors and what the Nordic chip reported.
pub struct App {
    keys: Keys,
    /// The backlight is wanted on (Back toggles it); it is on unless timed out.
    backlight: bool,
    /// The level the backlight is at (0: off).
    light_shown: u8,
    /// Since the last key press.
    idle_ms: u32,
    /// The chips found on the sensor bus.
    chips: ambient::Chips,
    /// The automatic backlight's state, and the level it asks for.
    auto: ambient::Auto,
    auto_level: u8,
    /// Since the last light measurement started; whether it is still running.
    light_ms: u32,
    measuring: bool,
    /// The barometer's last reading: hPa x 10, C x 100.
    baro: Option<(u32, i16)>,
    /// The RTC's seconds (the main screen shows them).
    seconds: u8,
    /// What the status strip shows, and what it showed when last drawn.
    strip: Strip,
    strip_shown: Strip,
    screen: Screen,
    ble: Option<nrf::Version>,
    rx: nrf::Rx,
    nearby: nrf::Nearby,
    scanning: bool,
    redraw_ms: u32,
    list_dirty: bool,
    /// ANT started (needs BLE and a built-in ANT+ key).
    ant: bool,
    sensors: [Sensor; 5],
    /// Since each sensor's last broadcast.
    heard_ms: [u32; 5],
    /// Since each sensor's channel was closed by its search time-out (`u32::MAX`: not resting).
    resting_ms: [u32; 5],
    /// Channels that sent data while searching: ask the chip which sensor it found.
    ask: u8,
    live: Live,
    decoders: Decoders,
    /// RMSSD and DFA alpha1 from the strap's beats.
    hrv: hrv::Hrv,
    battery: battery::Battery,
    /// Since the last battery reading.
    battery_ms: u32,
    gnss: gnss::Parser,
    /// The last fix, and its age.
    pvt: Option<gnss::Pvt>,
    pvt_ms: u32,
    /// Since the app's first step: the receiver is powered at [`GNSS_ON_MS`], its configuration and aiding sent
    /// once from [`GNSS_CONFIG_MS`].
    gnss_ms: u32,
    /// Messages sent to the receiver so far: configuration, time, position.
    gnss_sent: u8,
    /// The RTC's offset from UTC and the last position, as kept in backup registers.
    aid: gnss::Aid,
    /// Wheel, backlight, max heart rate, FTP.
    settings: Settings,
    /// The selected menu row's number is being changed (Lap up, Page down, Start or Back to leave).
    editing: bool,
    /// A held key already acted (a list scrolled, a value stepped), so its release does not act once more; cleared
    /// when no key is down.
    repeated: bool,
    /// The screens under this one, the latest last: Back returns to them, with their selection.
    under: [Screen; 4],
    depth: usize,
    /// The ride pages, then the overview's fields ([`OVERVIEW`]).
    pages: [PageCfg; PAGES + 1],
    /// Today (RTC), for the overview.
    date: (u16, u8, u8),
    /// The latest notices, newest first (empty lines: none).
    notices: [Line<20>; ui::NOTICES],
    /// The banner over the screen (the latest notice) and how long it has been up; drawn yet. Nothing under it is
    /// redrawn meanwhile; when it goes, the screen is drawn again.
    banner_ms: Option<u32>,
    /// What the banner says.
    banner: Line<20>,
    banner_drawn: bool,
    /// The GPS had a good fix since power-on; the battery was reported low.
    had_fix: bool,
    told_low: bool,
    /// A ride was just started from a key: the page is drawn again (its map and graphs start over).
    new_ride: bool,
    /// The card's map and elevation profile as drawn (set when the card is shown).
    summary: Option<(map::Shown, GraphShown)>,
    /// Reminders and alerts of the ride, the one due (said and beeped at the next pass), and the tones playing: what
    /// is left of them, how long the current one has left, and whether the buzzer is on.
    alerts: alerts::Alerts,
    alert: Option<alerts::Alert>,
    tones: (&'static [(u32, u32)], u32, bool),
    /// A first hold of Back during a ride armed switching off for this long ([`OFF_CONFIRM_MS`]).
    off_armed_ms: u32,
    /// When the ride started: date, hours and minutes (its card's title).
    ride_started: ((u16, u8, u8), (u8, u8)),
    /// The graphs show the whole ride rather than the last minutes (a hold on a graph toggles it).
    graph_whole: bool,
    /// The page's boxes as drawn ([`App::widgets`]).
    drawn: [Drawn; BOXES],
    /// The ride: timer, totals, laps.
    ride: ride::Ride,
    /// Heart rate and power over the last minutes, and the graphs on screen by ride field.
    history: ride::History,
    /// Rolling power, NP, TSS, work, W' balance.
    power: power::Power,
    /// The menu rows on screen, as fingerprints ([`App::menu_row_print`]).
    menu_shown: [u32; ui::MENU_ROWS],
    /// The ride's track, from the good fixes.
    track: map::Track,
    /// The eMMC, for the basemap and the dev link (identified at first use, never at boot).
    emmc: emmc::Emmc,
    /// Roads and water under the first map field of a page, drawn once per view.
    basemap: tiles::Basemap,
    /// The touch controller answered at the start; the gestures from its points.
    touch_ok: bool,
    touch: touch::Recogniser,
    /// The scan of stock's rides (the stock data menu), and its buffer for reading stock's settings.
    stock: stock::Scan,
    /// The card of the past ride shown ([`Screen::Past`]).
    past: history::Past,
    /// Workouts: those planned from the day it was read on; the one opened (its steps menu) or running, and its
    /// file; where it is (workout.rs); a tone to start (Hz, 0: none) and how long it sounds, ms.
    calendar: storage::workout::Calendar,
    calendar_day: Option<(u16, u8, u8)>,
    plan: storage::workout::Workout,
    plan_file: Option<fat::File>,
    player: Option<workout::Player>,
    tone: (u32, u32),
}

impl App {
    /// Panel up, backlight on, the Nordic chip started (BLE, then ANT with `ant_key` and the paired sensors), the
    /// settings read, the first ride page drawn.
    pub fn start(b: &mut impl Board, ant_key: Option<[u8; 8]>) -> Self {
        panel::init(b);
        play(b, &BEEP_ON);
        let mut rx = nrf::Rx::new();
        let ble = nrf::start(b).ok();
        let ant = match (ble, ant_key) {
            (Some(_), Some(key)) => nrf::ant_start(b, &mut rx, key, Kind::ALL.len() as u8).is_ok(),
            _ => false,
        };
        let mut sensors = [Sensor::Off; 5];
        for (kind, s) in Kind::ALL.into_iter().zip(&mut sensors) {
            if let Some(id) = SensorId::from_word(b.backup_read(kind.backup()))
                && ant
                && nrf::ant_open(b, &mut rx, kind.channel(), kind.device_type(), kind.period(), Some(id)).is_ok()
            {
                *s = Sensor::Paired(id);
            }
        }
        let misc = Settings::from_word(b.backup_read(WHEEL_BKP))
            .with_power_word(b.backup_read(POWER_BKP))
            .with_alerts_word(b.backup_read(ALERTS_BKP));
        let mut app = App {
            keys: Keys::default(),
            backlight: true,
            light_shown: 0,
            idle_ms: 0,
            chips: ambient::init(b),
            auto: ambient::Auto::default(),
            auto_level: LIGHT_LEVELS[1], // until the first reading
            light_ms: LIGHT_MS,          // a measurement at the first step
            measuring: false,
            baro: None,
            seconds: rtc_time_from_tr(b.rtc().0).2,
            strip: Strip {
                time: rtc_hm(b),
                radio: ble.is_some(),
                gps: None,
                blink: true,
                usb: UsbDot::Off,
                battery: None,
                charging: false,
                rec: Rec::Idle,
            },
            strip_shown: Strip {
                time: (0, 0),
                radio: false,
                gps: None,
                blink: true,
                usb: UsbDot::Off,
                battery: None,
                charging: false,
                rec: Rec::Idle,
            },
            screen: Screen::Main,
            ble,
            rx,
            nearby: nrf::Nearby::new(),
            scanning: false,
            redraw_ms: 0,
            list_dirty: false,
            ant,
            sensors,
            heard_ms: [u32::MAX; 5],
            resting_ms: [u32::MAX; 5],
            ask: 0,
            live: Live::default(),
            decoders: Decoders::default(),
            hrv: hrv::Hrv::default(),
            battery: battery::Battery::default(),
            battery_ms: BATTERY_MS, // read at the first step
            gnss: gnss::Parser::new(),
            pvt: None,
            pvt_ms: 0,
            gnss_ms: 0,
            gnss_sent: 0,
            aid: gnss::Aid::from_words(AID_BKP.map(|r| b.backup_read(r))),
            settings: misc,
            editing: false,
            repeated: false,
            under: [Screen::Main; 4],
            depth: 0,
            pages: core::array::from_fn(|i| {
                PageCfg::from_words(PAGE_BKP[i].map(|r| b.backup_read(r)))
                    .or_else(|| PageCfg::from_words_b(PAGE_BKP_B[i].map(|r| b.backup_read(r))))
                    .or_else(|| OLD_PAGE_BKP.get(i).and_then(|&r| PageCfg::from_word(b.backup_read(r))))
                    .unwrap_or(DEFAULT_PAGES[i])
            }),
            date: rtc_date_from_dr(b.rtc().1),
            notices: Default::default(),
            banner_ms: None,
            banner: Line::new(),
            banner_drawn: false,
            had_fix: false,
            told_low: false,
            graph_whole: false,
            ride_started: ((2000, 1, 1), (0, 0)),
            alerts: alerts::Alerts::new(),
            off_armed_ms: 0,
            alert: None,
            tones: (&[], 0, false),
            summary: None,
            new_ride: false,
            drawn: Default::default(),
            ride: ride::Ride::new(),
            history: ride::History::default(),
            power: power::Power::default(),
            menu_shown: [0; ui::MENU_ROWS],
            track: map::Track::new(),
            emmc: emmc::Emmc::new(),
            basemap: tiles::Basemap::new(),
            touch_ok: touch::init(b),
            touch: touch::Recogniser::default(),
            stock: stock::Scan::new(),
            past: history::Past::new(),
            calendar: Default::default(),
            calendar_day: None,
            plan_file: None,
            plan: storage::workout::Workout::new(),
            player: None,
            tone: (0, 0),
        };
        if app.chips.baro {
            ambient::baro_start(b); // read at the first step
        }
        if let Some((lat, lon)) = app.aid.pos {
            app.track.seed(lat, lon); // the map shows where the unit last was until the first fix
        }
        if b.backup_read(STOCK_BKP19) >> 28 == 0xb {
            // pages kept from register 19 by builds of 2026-10-23: kept where stock does not look, 19 given back
            for (n, page) in app.pages.iter().enumerate() {
                save_page(b, n, page);
            }
            b.backup_write(STOCK_BKP19, 0);
        }
        app.strip_shown = app.strip;
        ui::draw_strip(&mut Panel(b, (0, ui::H)), &app.strip);
        app.show(b);
        app.update_light(b);
        app
    }

    /// Sets the backlight to what it should be now: the chosen level, unless switched off (Back) or timed out.
    fn update_light(&mut self, b: &mut impl Board) {
        let timeout_ms = LIGHT_TIMEOUTS_S[self.settings.light_timeout] * 1000;
        let on = self.backlight && (timeout_ms == 0 || self.idle_ms < timeout_ms);
        let level = match LIGHT_LEVELS.get(self.settings.light_level) {
            _ if !on => 0,
            Some(&l) => l,
            None => self.auto_level,
        };
        if level != self.light_shown {
            self.light_shown = level;
            b.backlight(level);
        }
    }

    /// Writes the settings to their backup registers.
    fn save_misc(&self, b: &mut impl Board) {
        b.backup_write(WHEEL_BKP, self.settings.to_word());
        b.backup_write(POWER_BKP, self.settings.power_word());
        b.backup_write(ALERTS_BKP, self.settings.alerts_word());
    }

    /// Memory for the basemap (at least [`tiles::MEM_WORDS`] words); without it the map has no basemap.
    pub fn set_map_memory(&mut self, mem: &'static mut [u16]) {
        self.basemap.set_memory(mem);
    }

    /// The eMMC, shared with the dev link so there is one view of the card's state.
    pub fn emmc(&mut self) -> &mut emmc::Emmc {
        &mut self.emmc
    }

    /// The last basemap view change's cost.
    pub fn basemap_stats(&self) -> tiles::Stats {
        self.basemap.stats
    }

    /// Ride page `n`'s first map field: the one with the basemap.
    fn first_map(&self) -> Option<usize> {
        self.widgets().position(|(_, w)| w == Widget::Field(Field::Map))
    }

    /// Draws the basemap of `view` in `area` into its buffer, unless it holds it already.
    fn render_basemap(&mut self, b: &mut impl Board, area: map::Rect, view: Option<map::View>) {
        if let (Some(v), Some(o)) = (view, self.track.origin()) {
            self.basemap.render(&mut emmc::Volume(&mut self.emmc, b), area, v, o);
        }
    }

    /// What is on screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The ride page settings.
    pub fn pages(&self) -> &[PageCfg; PAGES + 1] {
        &self.pages
    }

    /// The battery history, oldest first: (mV, charging), one a minute.
    pub fn battery_history(&self) -> impl Iterator<Item = (u16, bool)> + '_ {
        self.battery.history()
    }

    /// The text of ride field `f` now ("--" without a value).
    fn field_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let (r, live) = (&self.ride, |m| self.live.get(m).map(u32::from));
        let (p, ftp) = (&self.power, self.settings.ftp);
        // the value, and whether it is in tenths
        let (v, tenths) = match f {
            Field::Time => {
                let (h, m) = self.strip.time;
                for d in [h / 10, h % 10, 10, m / 10, m % 10] {
                    l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
                }
                return l;
            }
            Field::Timer | Field::LapTime => {
                let s = if f == Field::Timer { r.timer_s() } else { r.lap_s() };
                l.push_u32(s / 3600);
                for v in [s / 60 % 60, s % 60] {
                    l.push_str(":");
                    l.push_str(ui::DIGITS[(v / 10) as usize % 10]);
                    l.push_str(ui::DIGITS[(v % 10) as usize]);
                }
                return l;
            }
            Field::Dist | Field::LapDist => {
                let m = if f == Field::Dist { r.dist_m() } else { r.lap_dist_m() };
                // as stock: 2 decimals under 100 km, then 1
                let (unit, digits) = if m < 100_000 { (10, 2) } else { (100, 1) };
                l.push_u32(m / 1000);
                l.push_str(".");
                let frac = m % 1000 / unit;
                for d in [frac / 10, frac % 10].get(2 - digits..).unwrap_or(&[]) {
                    l.push_str(ui::DIGITS[*d as usize % 10]);
                }
                return l;
            }
            Field::Map | Field::Graph | Field::HrGraph | Field::WbalGraph | Field::DfaGraph | Field::AltGraph => {
                return l;
            }
            Field::WeekKm | Field::WeekH | Field::YearKm | Field::TotalKm => {
                // the rides on the card (once read: see step_stock) and the one running
                if self.stock.status() != stock::Status::Done {
                    l.push_str("--");
                    return l;
                }
                let o = &self.stock.out;
                let (live_m, live_s) = if r.state() == ride::State::Idle { (0, 0) } else { (r.dist_m(), r.timer_s()) };
                match f {
                    Field::WeekH => push_fixed_u(&mut l, (o.week.time_s + live_s) / 360, 1),
                    Field::WeekKm => l.push_u32((o.week.dist_m + live_m) / 1000),
                    Field::YearKm => l.push_u32((o.year.dist_m + live_m) / 1000),
                    _ => l.push_u32((o.all.dist_m + live_m) / 1000),
                }
                return l;
            }
            Field::Workout | Field::StepTarget | Field::StepTime | Field::NextStep => return self.workout_text(f),
            Field::Temp => {
                let t = self.baro.map(|(_, t)| i32::from(t) / 10); // C x 100 to tenths
                if t.is_some_and(|t| t < 0) {
                    l.push_str("-");
                }
                (t.map(i32::unsigned_abs), true)
            }
            Field::Hr => (live(Metric::Hr), false),
            Field::Speed => (live(Metric::Speed), true),
            Field::Cadence => (live(Metric::Cadence), false),
            Field::Power => (live(Metric::Power), false),
            Field::AvgSpeed => (r.avg_speed_x10().map(u32::from), true),
            Field::MaxSpeed => (Some(u32::from(r.max_speed_x10())), true),
            Field::AvgHr => (r.avg_hr().map(u32::from), false),
            Field::AvgPower => (r.avg_power().map(u32::from), false),
            Field::Ascent => (Some(r.ascent_m()), false),
            // "--" once the strap is silent, else 2 decimals
            Field::Dfa => match self.hrv.alpha1().filter(|_| self.live.get(Metric::Hr).is_some()) {
                Some(a) => {
                    let v = (a * 100.0 + 0.5).clamp(0.0, 999.0) as u32;
                    l.push_u32(v / 100);
                    l.push_str(".");
                    l.push_str(ui::DIGITS[(v / 10 % 10) as usize]);
                    l.push_str(ui::DIGITS[(v % 10) as usize]);
                    return l;
                }
                None => (None, false),
            },
            Field::Rmssd => (self.hrv.rmssd().map(u32::from).filter(|_| self.live.get(Metric::Hr).is_some()), false),
            // power.rs and the ride's derived metrics
            Field::Power3s => (p.rolling(0).map(u32::from), false),
            Field::Power10s => (p.rolling(1).map(u32::from), false),
            Field::Power30s => (p.rolling(2).map(u32::from), false),
            Field::Np => (p.np().map(u32::from), false),
            Field::If => {
                match p.if_x100(ftp) {
                    Some(v) => {
                        l.push_u32(v / 100);
                        l.push_str(".");
                        l.push_str(ui::DIGITS[(v / 10 % 10) as usize]);
                        l.push_str(ui::DIGITS[(v % 10) as usize]);
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Tss => (p.tss(ftp), false),
            Field::Kj => (Some(p.kj()), false),
            Field::Kcal => (Some(p.kcal()), false),
            Field::Wkg => (p.rolling(0).map(|w| power::wkg_x10(w, self.settings.weight_kg)), true),
            Field::MaxPower => (p.max().map(u32::from), false),
            Field::MaxHr => (r.max_hr().map(u32::from), false),
            Field::AvgCadence => (r.avg_cadence().map(u32::from), false),
            Field::Balance => {
                match self.live.get(Metric::Balance) {
                    Some(right) => {
                        l.push_u32(u32::from(100u16.saturating_sub(right)));
                        l.push_str("/");
                        l.push_u32(u32::from(right));
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Wbal | Field::Vam | Field::Grade => {
                let v = match f {
                    Field::Wbal => p.wbal_j().map(|j| j / 100), // kJ x 10
                    Field::Vam => r.vam(),
                    _ => r.grade_x10(),
                };
                if v.is_some_and(|v| v < 0) {
                    l.push_str("-");
                }
                (v.map(i32::unsigned_abs), f != Field::Vam)
            }
        };
        match v {
            None => l.push_str("--"),
            Some(v) if tenths => {
                l.push_u32(v / 10);
                l.push_str(".");
                l.push_u32(v % 10);
            }
            Some(v) => l.push_u32(v),
        }
        l
    }

    /// The colour of ride field `f`'s value: heart rate and power (live, rolling, NP, max) in their zone's colour,
    /// W' balance as it drains ([`power::wbal_color`]), the rest white.
    fn field_color(&self, f: Field) -> u16 {
        if f == Field::StepTarget {
            return self.target_color();
        }
        let p = &self.power;
        let power = match f {
            Field::Power => self.live.get(Metric::Power),
            Field::Power3s => p.rolling(0),
            Field::Power10s => p.rolling(1),
            Field::Power30s => p.rolling(2),
            Field::Np => p.np(),
            Field::MaxPower => p.max(),
            _ => None,
        };
        let hr = match f {
            Field::Hr => self.live.get(Metric::Hr),
            Field::MaxHr => self.ride.max_hr(),
            _ => None,
        };
        match (power, hr, p.wbal_j()) {
            (Some(w), _, _) => ride::power_color(ride::power_zone(w, self.settings.ftp)),
            (_, Some(v), _) => ride::hr_color(ride::hr_zone(v, self.settings.max_hr)),
            (_, _, Some(j)) if f == Field::Wbal => power::wbal_color(j, self.settings.wprime_j),
            _ => ui::WHITE,
        }
    }

    /// What a graph of `plot` with `cols` columns shows now.
    fn graph_now(&self, plot: ride::Plot, cols: u32) -> GraphShown {
        self.graph_shown(plot, self.graph_whole, cols)
    }

    /// What a graph of `plot` over the last minutes or the `whole` ride, `cols` wide, shows now.
    fn graph_shown(&self, plot: ride::Plot, whole: bool, cols: u32) -> GraphShown {
        let h = &self.history;
        let (n, halved) = if whole { (h.whole_len(), h.whole_halved()) } else { (h.len(), 0) };
        let mut g = GraphShown { plot, whole, n, halved, scale: (0, 1) };
        g.scale = self.graph_scale(g, cols);
        g
    }

    /// Column `i` of `cols` of a graph showing `g`: its value (0: none). Over the whole ride, a column is the mean of
    /// the bars it covers when there are more bars than columns; over the last minutes, the newest bar is the
    /// rightmost column.
    fn graph_value(&self, g: GraphShown, cols: u32, i: u32) -> u16 {
        let h = &self.history;
        if !g.whole {
            return (g.n + i).checked_sub(cols).map_or(0, |k| h.bar(k, g.plot));
        }
        let per = g.n.div_ceil(cols.max(1)).max(1);
        let (sum, count) = (i * per..((i + 1) * per).min(g.n))
            .map(|k| u32::from(h.whole_bar(k, g.plot)))
            .filter(|&v| v != 0)
            .fold((0, 0), |(s, n), v| (s + v, n + 1));
        if count == 0 { 0 } else { (sum / count) as u16 }
    }

    /// Column `i` of a graph of `cols` columns `h` high showing `g`: the bar's height and colour (a bar of a reading
    /// is at least 1 high).
    fn graph_bar(&self, g: GraphShown, cols: u32, h: u32, i: u32) -> (u32, u16) {
        use ride::Plot;
        let v = self.graph_value(g, cols, i);
        if v == 0 {
            return (0, ui::BLACK);
        }
        let color = match g.plot {
            Plot::Power => ride::power_color(ride::power_zone(v, self.settings.ftp)),
            Plot::Hr => ride::hr_color(ride::hr_zone(v, self.settings.max_hr)),
            Plot::Wbal => power::wbal_color(i32::from(v) * 10, self.settings.wprime_j),
            // above the aerobic threshold's 0.75, between it and 0.5, below (Rogers et al.)
            Plot::Dfa if v >= 75 => ui::GREEN,
            Plot::Dfa if v >= 50 => ui::ACCENT,
            Plot::Dfa => ui::RED,
            Plot::Alt => ui::GREY,
        };
        let (low, high) = g.scale;
        let height = u32::from(v).saturating_sub(low) * h / high.saturating_sub(low).max(1);
        (height.clamp(1, h), color)
    }

    /// A graph's scale, bottom and top, in its plot's units: power from 0 to twice the FTP, heart rate from half the
    /// maximum to it, W' balance from 0 to W', DFA alpha1 from 0 to 1.5; altitude around what the graph shows (in
    /// 20 m steps, 60 m at least, so it is not redrawn for every metre).
    fn graph_scale(&self, g: GraphShown, cols: u32) -> (u32, u32) {
        use ride::Plot;
        let (max, ftp) = (u32::from(self.settings.max_hr), u32::from(self.settings.ftp));
        match g.plot {
            Plot::Power => (0, 2 * ftp),
            Plot::Hr => (max / 2, max),
            Plot::Wbal => (0, u32::from(self.settings.wprime_j) / 10),
            Plot::Dfa => (0, 150),
            Plot::Alt => ride::alt_scale((0..cols).map(|i| u32::from(self.graph_value(g, cols, i)))),
        }
    }

    /// A graph's label, scale and time span (its axes) in box `bx`.
    fn draw_graph_axes(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), g: GraphShown) {
        use ride::Plot;
        let (low, high) = g.scale;
        let label = |v: u32| {
            let mut l = Line::<8>::new();
            match g.plot {
                Plot::Wbal => l.push_u32(v / 100), // kJ
                Plot::Dfa => push_fixed_u(&mut l, v, 2),
                Plot::Alt => l.push_u32(v.saturating_sub(ride::ALT_OFFSET as u32)),
                _ => l.push_u32(v),
            }
            l
        };
        let (lo, mid, hi) = (label(low), label(low.midpoint(high)), label(high));
        let title = match g.plot {
            Plot::Power => "POWER W",
            Plot::Hr => "HR",
            Plot::Wbal => "W'BAL KJ",
            Plot::Dfa => "DFA A1",
            Plot::Alt => "ELEV M",
        };
        let mut from = Line::<12>::new();
        if g.whole {
            from.push_str("START");
        } else {
            from.push_str("-");
            from.push_u32(ui::graph_area(bx).2 * ride::BAR_S / 60);
            from.push_str(" MIN");
        }
        ui::draw_graph_axes(c, bx, title, [lo.as_str(), mid.as_str(), hi.as_str()], from.as_str());
    }

    /// The first ride page that is on from page `from` (taken round), going forward by `step` (1, or PAGES - 1 for
    /// backward): the ride pages go round among themselves; page 1 is never off.
    fn ride_page(&self, from: usize, step: usize) -> Screen {
        let on = |i: usize| self.pages.get(i % PAGES).is_some_and(|p| p.layout > 0);
        let n = (0..PAGES).map(|k| (from + k * step) % PAGES).find(|&i| on(i)).unwrap_or(0);
        Screen::Ride(n)
    }

    /// A ride runs (or its card is on screen): what a reset or switching off would lose (main.rs does not reset on a
    /// USB unplug then).
    pub fn busy(&self) -> bool {
        self.ride.state() != ride::State::Idle || self.screen == Screen::Summary
    }

    /// The card of the ride (the one running, or the last one): its numbers and title, as a past ride's
    /// ([`history::Ride::stats`]).
    fn ride_card(&self) -> ui::card::Card {
        let (r, p) = (&self.ride, &self.power);
        ui::card::Card::of(&ui::card::Stats {
            start: self.ride_started,
            timer_s: r.timer_s(),
            dist_m: r.dist_m(),
            avg_speed_x10: r.avg_speed_x10(),
            ascent_m: Some(r.ascent_m()),
            np: p.np(),
            avg_power: r.avg_power(),
            avg_hr: r.avg_hr(),
            max_hr: r.max_hr(),
            tss: p.tss(self.settings.ftp),
            kj: Some(p.kj()),
        })
    }

    /// The card screen: the track in [`ui::card::MAP`], the whole ride's elevation, the numbers, as they were when
    /// the card was shown ([`App::summary`]).
    fn draw_summary(&self, c: &mut impl Canvas) {
        let Some((m, g)) = self.summary else { return };
        let (_, _, cols, h) = ui::card::ELEV;
        ui::card::draw(c, &self.ride_card(), (&self.track, m.view), &|i| self.graph_bar(g, cols, h, i));
    }

    /// The graph field at (`x`, `y`) on the page shown, if there is one: its box index and field.
    fn graph_at(&self, x: u16, y: u16) -> Option<(usize, Field)> {
        let (x, y) = (u32::from(x), u32::from(y));
        self.widgets().enumerate().find_map(|(i, ((bx, by, bw, bh), w))| match w {
            Widget::Field(f) if graph_plot(f).is_some() && (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y) => {
                Some((i, f))
            }
            _ => None,
        })
    }

    /// Row `i` of `menu`; None past its last.
    fn row(&self, menu: Menu, i: usize) -> Option<menu::Row> {
        use menu::Row;
        match menu {
            Menu::Main => menu::MAIN.get(i).copied(),
            Menu::Sensors if i < Kind::ALL.len() => Some(Row::Sensor(i)),
            Menu::Sensors => menu::BUILT_IN.get(i - Kind::ALL.len()).map(|&r| Row::Read(r)),
            Menu::Gps => menu::GPS.get(i).filter(|_| i == 0 || self.pvt.is_some()).map(|&r| Row::Read(r)),
            Menu::Stock => menu::STOCK.get(i).map(|&r| Row::Stock(r)),
            Menu::Alerts => menu::ALERTS.get(i).copied(),
            // no rides yet: the scan's row, which shows how far it is
            Menu::History if self.stock.index.is_empty() => (i == 0).then_some(Row::Stock(menu::StockRow::Scan)),
            Menu::History => self.stock.index.get(i).map(|_| Row::Past(i)),
            Menu::Workouts => self.calendar.get(i).map(|_| Row::Workout(i)),
            Menu::Steps if i == 0 => Some(Row::WorkoutStart),
            Menu::Steps => (i <= self.plan.steps().len()).then_some(Row::Step(i - 1)),
            Menu::Page(OVERVIEW) => (i < 4).then_some(Row::Field(OVERVIEW, i)), // always four
            Menu::Page(n) => match i {
                0 => Some(Row::Layout(n)),
                _ if i <= ui::field_count(self.pages.get(n)?.layout) => Some(Row::Field(n, i - 1)),
                _ => None,
            },
        }
    }

    /// How many rows `menu` has.
    fn rows(&self, menu: Menu) -> usize {
        (0..128).take_while(|&i| self.row(menu, i).is_some()).count() // a workout's steps: up to 97
    }

    /// Menu row `i` of `menu`: label, value, value colour; None past the last row.
    fn menu_row(&self, menu: Menu, i: usize) -> Option<(Line<16>, Line<12>, u16)> {
        use menu::{Reading, Row};
        let (mut label, mut value, mut color) = (Line::new(), Line::new(), ui::GREY);
        match self.row(menu, i)? {
            Row::Open(Menu::Page(OVERVIEW)) => label.push_str("OVERVIEW"),
            Row::Open(Menu::Page(n)) => {
                label.push_str("PAGE ");
                label.push_u32(n as u32 + 1);
                layout_text(&mut value, self.pages.get(n)?.layout);
            }
            Row::Open(m) => label.push_str(menu_title(m)),
            Row::Nearby => label.push_str("NEARBY BLE"),
            Row::Set(s) => {
                label.push_str(s.label());
                let edited = self.editing && self.screen == Screen::Menu(menu, i);
                value.push_str(if edited { "< " } else { "" });
                self.settings.text(s, &mut value);
                value.push_str(if edited { " >" } else { "" });
            }
            Row::Sensor(k) => {
                let kind = *Kind::ALL.get(k)?;
                label.push_str(kind.name());
                match (self.ant, self.sensors.get(k)?) {
                    (false, _) => value.push_str(if self.ble.is_some() { "NO KEY" } else { "NO BLE" }),
                    (true, Sensor::Off) => value.push_str("OFF"),
                    (true, Sensor::Search) => value.push_str("SEARCH"),
                    (true, Sensor::Paired(id)) => value.push_u32(u32::from(id.device)),
                }
                if self.heard_ms.get(k).is_some_and(|&ms| ms < sensors::STALE_MS) {
                    color = ui::GREEN;
                }
            }
            Row::Read(r @ (Reading::Lux | Reading::TempC | Reading::Hpa)) => {
                label.push_str(match r {
                    Reading::Lux => "LUX",
                    Reading::TempC => "TEMP C",
                    _ => "HPA",
                });
                let present = if r == Reading::Lux { self.chips.light } else { self.chips.baro };
                match (r, self.auto.lux10(), self.baro) {
                    _ if !present => value.push_str("NONE"),
                    (Reading::Lux, Some(l), _) => value.push_u32(l / 10),
                    (Reading::TempC, _, Some((_, t))) => push_fixed(&mut value, i32::from(t), 2, 1),
                    (Reading::Hpa, _, Some((p, _))) => push_fixed(&mut value, p as i32, 1, 0),
                    _ => value.push_str("--"),
                }
            }
            Row::Read(r) => {
                let two = |l: &mut Line<12>, v: u8| {
                    for d in [v / 10, v % 10] {
                        l.push_str(ui::DIGITS[usize::from(d)]);
                    }
                };
                let p = self.pvt;
                match (r, p) {
                    (Reading::Fix, None) => (label.push_str("FIX"), value.push_str("NO GPS")).1,
                    (Reading::Fix, Some(p)) => {
                        label.push_str("FIX");
                        value.push_str(match p.fix {
                            _ if !p.fix_ok => "NONE",
                            2 => "2D",
                            3 => "3D",
                            _ => "OTHER",
                        });
                    }
                    (_, None) => return None,
                    (Reading::Sats, Some(p)) => (label.push_str("SATS"), value.push_u32(u32::from(p.sats))).1,
                    (Reading::Lat, Some(p)) => (label.push_str("LAT"), push_fixed(&mut value, p.lat, 7, 5)).1,
                    (Reading::Lon, Some(p)) => (label.push_str("LON"), push_fixed(&mut value, p.lon, 7, 5)).1,
                    (Reading::Alt, Some(p)) => (label.push_str("ALT M"), push_fixed(&mut value, p.alt_mm, 3, 0)).1,
                    (Reading::Utc | Reading::Date, Some(p)) if !p.time_ok => {
                        label.push_str(if r == Reading::Utc { "UTC" } else { "DATE" });
                        value.push_str("--"); // the receiver's placeholder until it knows the time
                    }
                    (Reading::Utc, Some(p)) => {
                        label.push_str("UTC");
                        let (_, _, _, h, m, sec) = p.utc;
                        for (k, v) in [h, m, sec].into_iter().enumerate() {
                            if k > 0 {
                                value.push_str(":");
                            }
                            two(&mut value, v % 100);
                        }
                    }
                    (Reading::Date, Some(p)) => {
                        label.push_str("DATE");
                        let (y, mo, d, ..) = p.utc;
                        value.push_u32(u32::from(y));
                        value.push_str("-");
                        two(&mut value, mo % 100);
                        value.push_str("-");
                        two(&mut value, d % 100);
                    }
                    _ => return None,
                }
                if p.is_some_and(|p| p.good()) {
                    color = ui::GREEN;
                }
            }
            Row::Layout(n) => {
                label.push_str("FIELDS");
                layout_text(&mut value, self.pages.get(n)?.layout);
            }
            Row::Field(n, f) => {
                label.push_str("FIELD ");
                label.push_u32(f as u32 + 1);
                value.push_str(self.pages.get(n)?.fields.get(f)?.label());
            }
            Row::Stock(r) => {
                label.push_str(r.label());
                self.stock_value(r, &mut value);
            }
            Row::Past(n) => {
                // "SAT 24 OCT", "42.1" (km: a unit does not fit beside the date)
                let r = self.stock.index.get(n)?;
                let (y, mo, d, ..) = r.start;
                label.push_str(date_text((y, mo, d)).as_str());
                ui::card::push_km(&mut value, r.dist_m);
            }
            Row::Workout(k) => {
                let p = self.calendar.get(k)?;
                label.push_str(date_text(p.day).as_str());
                push_hm(&mut value, p.total_s);
            }
            Row::WorkoutStart => {
                label.push_str("START");
                push_hm(&mut value, self.plan.total_s());
                color = ui::GREEN;
            }
            Row::Step(k) => color = self.step_row(self.plan.steps().get(k)?, &mut label, &mut value),
        }
        Some((label, value, color))
    }

    /// Draws the whole screen below the bar from the state (the panel was cleared, or a screenshot row).
    fn draw_screen(&self, c: &mut impl Canvas) {
        match self.screen {
            Screen::Main | Screen::Ride(_) => {
                for (i, (bx, w)) in self.widgets().enumerate() {
                    self.draw_widget(c, i, bx, w);
                }
            }
            Screen::Menu(menu, sel) => {
                let mut title = Line::<16>::new();
                match menu {
                    Menu::Page(OVERVIEW) => title.push_str("OVERVIEW"),
                    Menu::Steps if !self.plan.name().is_empty() => title.push_str(self.plan.name()),
                    Menu::Page(n) => {
                        title.push_str(menu_title(menu));
                        title.push_u32(n as u32 + 1);
                    }
                    _ => title.push_str(menu_title(menu)),
                }
                ui::draw_title(c, title.as_str());
                self.draw_menu_rows(c, menu, sel);
            }
            Screen::Summary => self.draw_summary(c),
            Screen::Past => {
                let (p, h) = (&self.past, ui::card::ELEV.3);
                let view = map::View::fit_whole(&p.track, map::area(ui::card::MAP));
                ui::card::draw(c, &p.card, (&p.track, view), &|i| p.elev(i, h));
            }
            Screen::Nearby => {
                if self.ble.is_some() {
                    ui::draw_nearby(c, self.nearby.count(), self.scanning);
                }
                ui::draw_list(c, &self.nearby);
            }
        }
        if self.banner_ms.is_some() && self.banner_drawn {
            ui::draw_banner(c, self.banner.as_str());
        }
    }

    /// The boxes of the page on screen and what each holds (none on a menu or a list). The clock and Ferris place
    /// themselves.
    fn widgets(&self) -> impl Iterator<Item = ((u32, u32, u32, u32), Widget)> + use<> {
        let mut all = [((0, 0, 0, 0), Widget::Clock); BOXES];
        let (page, at, fixed): (_, BoxAt, &[Widget]) = match self.screen {
            Screen::Main => (OVERVIEW, |_, i| ui::overview_box(i), &[Widget::Clock, Widget::Date, Widget::Notices]),
            Screen::Ride(n) => (n, |layout, i| ui::field_box(layout, ui::RIDE_H, i), &[]),
            _ => return all.into_iter().take(0),
        };
        let p = self.pages.get(page).copied().unwrap_or(DEFAULT_PAGES[0]);
        let mut n = 0;
        for (slot, &w) in all.iter_mut().zip(fixed) {
            slot.1 = w;
            n += 1;
        }
        for (i, &f) in p.fields.iter().enumerate() {
            let (Some(bx), Some(slot)) = (at(p.layout, i), all.get_mut(n)) else { break };
            *slot = (bx, Widget::Field(f));
            n += 1;
        }
        all.into_iter().take(n)
    }

    /// What box `bx` holding `w` would show now.
    fn now_drawn(&self, bx: (u32, u32, u32, u32), w: Widget) -> Drawn {
        match w {
            Widget::Clock => Drawn::Digits(self.main_digits()),
            Widget::Date => {
                Drawn::Print(u32::from(self.date.0) << 16 | u32::from(self.date.1) << 8 | u32::from(self.date.2))
            }
            Widget::Notices => {
                Drawn::Print(self.notices.iter().fold(0x811c_9dc5, |h, n| fnv(h, n.as_str().as_bytes())))
            }
            Widget::Field(Field::Map) => Drawn::Map(map::Shown::of(&self.track, map::area(bx))),
            Widget::Field(f) => match graph_plot(f) {
                Some(plot) => Drawn::Graph(self.graph_now(plot, ui::graph_area(bx).2)),
                None => Drawn::Value(self.field_text(f), self.field_color(f)),
            },
        }
    }

    /// Box `i` (`bx`, holding `w`) whole, as drawn ([`App::drawn`]): after the page was cleared, or a screenshot row.
    fn draw_widget(&self, c: &mut impl Canvas, i: usize, bx: (u32, u32, u32, u32), w: Widget) {
        let d = self.drawn.get(i).copied().unwrap_or_default();
        match (w, d) {
            (Widget::Clock, Drawn::Digits(digits)) => ui::draw_main_time(c, digits),
            (Widget::Date, _) => ui::draw_date(c, date_text(self.date).as_str()),
            (Widget::Notices, _) => {
                let mut n: [&str; ui::NOTICES] = [""; ui::NOTICES];
                for (slot, line) in n.iter_mut().zip(&self.notices) {
                    *slot = line.as_str();
                }
                ui::draw_notices(c, &n[..self.notices.iter().take_while(|l| !l.as_str().is_empty()).count()]);
            }
            (Widget::Field(f), d) => {
                ui::draw_field_frame(c, bx, f);
                match d {
                    Drawn::Map(m) => {
                        let base = (self.first_map() == Some(i)).then_some(&self.basemap);
                        map::draw(c, map::area(bx), &self.track, m.view, base);
                    }
                    Drawn::Graph(g) => {
                        self.draw_graph_axes(c, bx, g);
                        let a = ui::graph_area(bx);
                        ui::draw_graph(c, a, &|k| self.graph_bar(g, a.2, a.3, k));
                    }
                    Drawn::Value(text, fg) => ui::draw_field_value(c, bx, f, text.as_str(), fg),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The map in box `i` as drawn (tests).
    #[cfg(test)]
    fn map_view(&self, i: usize) -> Option<map::View> {
        match self.drawn.get(i) {
            Some(Drawn::Map(m)) => m.view,
            _ => None,
        }
    }

    /// The overview's digits: H H M M.
    fn main_digits(&self) -> [u8; 4] {
        let (h, m) = self.strip.time;
        [h / 10, h % 10, m / 10, m % 10]
    }

    /// How far down the page may be drawn: above the banner while one is up.
    fn bottom(&self) -> u32 {
        if self.banner_ms.is_some() { ui::BANNER_Y } else { ui::H }
    }

    /// The banner is gone: what is under it drawn again, from what the screen shows.
    fn unbanner(&mut self, b: &mut impl Board) {
        self.banner_ms = None;
        let mut p = Panel(b, (ui::BANNER_Y, ui::H));
        p.fill(0, ui::BANNER_Y, ui::W, ui::H - ui::BANNER_Y, ui::BLACK);
        self.draw_screen(&mut p);
    }

    /// Adds a notice (newest first), stamped with the time: what the unit itself has to say (a fix, a sensor
    /// paired, the battery low); phone notifications will come here too.
    fn notice(&mut self, text: &str) {
        self.notices.rotate_right(1);
        let (h, m) = self.strip.time;
        let mut l = Line::new();
        for d in [h / 10, h % 10, 10, m / 10, m % 10] {
            l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
        }
        l.push_str(" ");
        l.push_str(text);
        self.notices[0] = l;
        // over the overview the notice is in its own box already
        if self.screen != Screen::Main {
            self.say(l.as_str());
        }
    }

    /// The tones playing go on: each lasts its time, in passes of [`POLL_MS`], then the next; the buzzer stops after
    /// the last. Nothing waits: the loop goes on meanwhile.
    fn play_tones(&mut self, b: &mut impl Board) {
        let (left, ms, playing) = self.tones;
        if ms > 0 {
            self.tones.1 = ms.saturating_sub(POLL_MS);
            return;
        }
        match left.split_first() {
            Some((&(hz, len), rest)) => {
                b.beep(hz);
                self.tones = (rest, len.saturating_sub(POLL_MS), true);
            }
            None if playing => {
                b.beep(0);
                self.tones.2 = false;
            }
            None => {}
        }
    }

    /// Shows `text` in the banner for [`BANNER_MS`] (what a key just did, a notice).
    fn say(&mut self, text: &str) {
        self.banner.clear();
        self.banner.push_str(text);
        (self.banner_ms, self.banner_drawn) = (Some(0), false);
    }

    /// Start, pause or resume the ride from a key, saying so; a new ride also resets the power totals.
    fn start_pause(&mut self) {
        let ev = self.ride.start_pause();
        if ev == ride::Event::Start {
            // the ride's own track and graphs, from here (the last position kept, so the map is not empty)
            self.ride_started = (self.date, self.strip.time);
            self.history = ride::History::default();
            self.alerts = alerts::Alerts::new();
            let mut track = map::Track::new();
            if let Some(p) = self.pvt.filter(|p| p.good()) {
                track.seed(p.lat, p.lon);
            }
            self.track = track;
            self.new_ride = true;
        }
        self.say(match ev {
            ride::Event::Start => "RIDE STARTED",
            ride::Event::Pause => "PAUSED: HOLD = END",
            _ => "RESUMED",
        });
        if ev == ride::Event::Start {
            self.power.new_ride();
        }
        self.recorder(ev);
    }

    fn draw_menu_rows(&self, c: &mut impl Canvas, menu: Menu, sel: usize) {
        for i in 0..ui::MENU_ROWS {
            self.draw_menu_row(c, menu, sel, i);
        }
    }

    /// Screen row `slot` of `menu` (the menu turns a screen at a time: [`menu::first_shown`]).
    fn draw_menu_row(&self, c: &mut impl Canvas, menu: Menu, sel: usize, slot: usize) {
        let i = menu::first_shown(sel, ui::MENU_ROWS) + slot;
        match self.menu_row(menu, i) {
            Some((label, value, color)) => ui::draw_menu_row(c, slot, label.as_str(), value.as_str(), color, i == sel),
            None => c.fill(0, ui::LIST_Y + slot as u32 * ui::LIST_ROW, ui::W, ui::LIST_ROW, ui::BLACK),
        }
    }

    /// A fingerprint of screen row `slot` as it would be drawn (FNV-1a over its text, colour and selection).
    fn menu_row_print(&self, menu: Menu, sel: usize, slot: usize) -> u32 {
        let i = menu::first_shown(sel, ui::MENU_ROWS) + slot;
        let mut h = 0x811c_9dc5u32;
        let mut eat = |bytes: &[u8]| {
            for &x in bytes {
                h = (h ^ u32::from(x)).wrapping_mul(0x0100_0193);
            }
        };
        match self.menu_row(menu, i) {
            Some((label, value, color)) => {
                eat(label.as_str().as_bytes());
                eat(&[0]);
                eat(value.as_str().as_bytes());
                eat(&color.to_le_bytes());
                eat(&[u8::from(i == sel)]);
            }
            None => eat(&[1]),
        }
        h
    }

    /// Redraws the menu rows that changed since they were drawn (the selection, a value).
    fn update_menu_rows(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        for i in 0..ui::MENU_ROWS {
            let print = self.menu_row_print(menu, sel, i);
            if self.menu_shown[i] != print {
                self.draw_menu_row(&mut Panel(b, (0, self.bottom())), menu, sel, i);
                self.menu_shown[i] = print;
            }
        }
    }

    /// Clears the page area and draws the current screen.
    fn show(&mut self, b: &mut impl Board) {
        if self.calendar_day.is_none() && self.widgets().any(|(_, w)| w == Widget::Field(Field::Workout)) {
            self.load_calendar(b); // the overview's planned workout: the card read at first use
        }
        for (i, (bx, w)) in self.widgets().enumerate() {
            let now = self.now_drawn(bx, w);
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
        if self.screen == Screen::Summary {
            let (_, _, cols, _) = ui::card::ELEV;
            let m = map::Shown::of(&self.track, map::area(ui::card::MAP));
            self.summary = Some((m, self.graph_shown(ride::Plot::Alt, true, cols)));
        }
        if let Some(i) = self.first_map()
            && let (Some((bx, _)), Some(Drawn::Map(m))) = (self.widgets().nth(i), self.drawn.get(i))
        {
            let view = m.view;
            self.render_basemap(b, map::area(bx), view);
        }
        let mut p = Panel(b, (0, self.bottom()));
        ui::clear_page(&mut p);
        self.draw_screen(&mut p);
        if let Screen::Menu(menu, sel) = self.screen {
            self.menu_shown = core::array::from_fn(|i| self.menu_row_print(menu, sel, i));
        }
    }

    /// Redraws what changed in the page's boxes since they were drawn.
    fn update_page(&mut self, b: &mut impl Board) {
        let first = self.first_map();
        for (i, (bx, w)) in self.widgets().enumerate() {
            let Some(&old) = self.drawn.get(i) else { break };
            let now = match (w, old) {
                (_, Drawn::Map(mut m)) => {
                    let area = map::area(bx);
                    if first == Some(i) && map::refit(area, &self.track, &m) {
                        let view = map::Shown::of(&self.track, area).view;
                        self.render_basemap(b, area, view);
                    }
                    let base = (first == Some(i)).then_some(&self.basemap);
                    map::update(&mut Panel(b, (0, self.bottom())), area, &self.track, &mut m, base);
                    Drawn::Map(m)
                }
                (_, Drawn::Graph(g)) => self.update_graph(b, g, bx),
                (Widget::Clock, Drawn::Digits(was)) => {
                    let now = self.main_digits();
                    for (k, (d, was)) in now.into_iter().zip(was).enumerate() {
                        if d != was {
                            ui::draw_main_digit(&mut Panel(b, (0, self.bottom())), k, d);
                        }
                    }
                    Drawn::Digits(now)
                }
                (Widget::Field(f), Drawn::Value(was, was_fg)) => {
                    let (text, fg) = (self.field_text(f), self.field_color(f));
                    if (was, was_fg) != (text, fg) {
                        let old = if was_fg == fg { was.as_str() } else { "" }; // a new colour: all of it
                        ui::draw_field_value_changes(&mut Panel(b, (0, self.bottom())), bx, f, old, text.as_str(), fg);
                    }
                    Drawn::Value(text, fg)
                }
                _ => {
                    let now = self.now_drawn(bx, w);
                    if now != old {
                        if let Some(d) = self.drawn.get_mut(i) {
                            *d = now;
                        }
                        self.draw_widget(&mut Panel(b, (0, self.bottom())), i, bx, w);
                    }
                    now
                }
            };
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
    }

    /// A graph in box `bx`, drawn as `old`, after a new bar: each column changes by the difference between its old
    /// bar and its new one (the chart scrolls left by `STEP` bars), all of it if the graph changed metric.
    fn update_graph(&mut self, b: &mut impl Board, old: GraphShown, bx: (u32, u32, u32, u32)) -> Drawn {
        // Bars scroll in this many at a time (10 s): nearly every column changes on a scroll, so this sends a fifth
        // of the panel windows of a scroll per bar, and fewer pixels (sim graph_field: 1812 px/s in 33 windows/s,
        // against 1987 in 113). The whole ride grows to the right instead: a new bar changes one column.
        const STEP: u32 = 5;
        let a = ui::graph_area(bx);
        let now = self.graph_now(old.plot, a.2);
        let step = if now.whole { 1 } else { STEP };
        if now == old || (GraphShown { n: old.n, ..now } == old && now.n.wrapping_sub(old.n) < step) {
            return Drawn::Graph(old);
        }
        let mut p = Panel(b, (0, self.bottom()));
        if (GraphShown { n: old.n, ..now }) != old {
            // another scale, a halved whole ride, or another plot or span: all of it
            self.draw_graph_axes(&mut p, bx, now);
            ui::draw_graph(&mut p, a, &|k| self.graph_bar(now, a.2, a.3, k));
        } else {
            for k in 0..a.2 {
                let (was, is) = (self.graph_bar(old, a.2, a.3, k), self.graph_bar(now, a.2, a.3, k));
                ui::draw_graph_column(&mut p, a, k, was, is);
            }
        }
        Drawn::Graph(now)
    }

    /// The ride recorder's one entry point: every [`ride::Event`] goes through here, in order.
    // ponytail: nothing is stored yet (no eMMC driver). The FIT writer plugs in here and nowhere else:
    // Start: fit::Writer::start (a new file); Second: Writer::record with a fit::Sample built from self.live,
    // self.pvt and the RTC (fit::fit_time), then every 60 s Writer::checkpoint, its record kept in two copies
    // (AGENTS.md, golden rule); Pause / Resume: timer events; Lap: a lap message; End: Writer::finish. Resume on
    // boot reads the newer valid record back and puts the ride back to Paused.
    fn recorder(&mut self, ev: ride::Event) {
        let _ = ev;
    }

    /// Opens or closes sensor `kind`'s channel to match `want`, and keeps the pairing.
    fn set_sensor(&mut self, b: &mut impl Board, kind: Kind, want: Sensor) {
        let i = kind as usize;
        let resting = self.resting_ms[i] != u32::MAX;
        if self.sensors[i] != Sensor::Off && !resting && nrf::ant_close(b, &mut self.rx, kind.channel()).is_err() {
            return; // still open: leave it as it was
        }
        self.resting_ms[i] = u32::MAX;
        self.sensors[i] = Sensor::Off;
        self.heard_ms[i] = u32::MAX;
        let id = match want {
            Sensor::Off => None,
            Sensor::Search => None,
            Sensor::Paired(id) => Some(id),
        };
        b.backup_write(kind.backup(), id.map_or(0, SensorId::to_word));
        if want != Sensor::Off
            && nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok()
        {
            self.sensors[i] = want;
        }
    }

    /// Opens screen `s` over this one: Back returns here.
    fn open(&mut self, s: Screen) {
        if let Some(u) = self.under.get_mut(self.depth) {
            *u = self.screen;
            self.depth += 1;
        }
        self.screen = s;
    }

    /// Back to the screen under this one (a ride page switched off since shows the main screen).
    fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.screen = match self.under.get(self.depth) {
            Some(&Screen::Ride(n)) if self.pages.get(n).is_none_or(|p| p.layout == 0) => Screen::Main,
            Some(&s) if self.depth > 0 || matches!(s, Screen::Main | Screen::Ride(_)) => s,
            _ => Screen::Main,
        };
    }

    /// Select on row `sel` of `menu`.
    fn activate(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        use menu::Row;
        match self.row(menu, sel) {
            Some(Row::Open(Menu::Workouts)) => {
                self.load_calendar(b);
                if self.calendar.get(0).is_none() {
                    self.say("NO WORKOUT PLANNED");
                }
                self.open(Screen::Menu(Menu::Workouts, 0));
            }
            Some(Row::Open(m)) => {
                if m == Menu::History && self.stock.status() == stock::Status::Idle {
                    self.stock.start(self.date); // the history is what the scan of stock's rides finds
                }
                self.open(Screen::Menu(m, 0))
            }
            Some(Row::Past(n)) => {
                let Some(&r) = self.stock.index.get(n) else { return };
                match self.past.read(&mut emmc::Volume(&mut self.emmc, b), &r, self.settings.ftp) {
                    Ok(()) => self.open(Screen::Past),
                    Err(_) => self.notice("RIDE UNREADABLE"),
                }
            }
            Some(Row::Workout(k)) => self.open_workout(b, k),
            Some(Row::WorkoutStart) => self.start_workout(0),
            Some(Row::Step(k)) => self.start_workout(k),
            Some(Row::Nearby) => {
                if self.ble.is_some() && !self.scanning {
                    self.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
                }
                self.open(Screen::Nearby);
            }
            Some(Row::Set(s)) if s.number().is_some() => {
                self.editing = !self.editing;
                if !self.editing {
                    self.save_misc(b);
                }
            }
            Some(Row::Set(s)) => {
                self.settings.cycle(s);
                self.save_misc(b);
                self.update_light(b);
            }
            Some(Row::Sensor(k)) => {
                if let (true, Some(&kind), Some(&now)) = (self.ant, Kind::ALL.get(k), self.sensors.get(k)) {
                    let want = if now == Sensor::Off { Sensor::Search } else { Sensor::Off };
                    self.set_sensor(b, kind, want);
                }
            }
            Some(Row::Layout(n)) => {
                if let Some(page) = self.pages.get_mut(n) {
                    let order = ui::LAYOUT_ORDER;
                    let i = order.iter().position(|&l| l == page.layout).unwrap_or(0);
                    page.layout = order[(i + 1) % order.len()];
                    if n == 0 && page.layout == 0 {
                        page.layout = order[1]; // the first page is never off
                    }
                    save_page(b, n, page);
                }
            }
            Some(Row::Field(n, f)) => {
                if let Some(page) = self.pages.get_mut(n)
                    && let Some(field) = page.fields.get_mut(f)
                {
                    *field = field.next();
                    save_page(b, n, page);
                }
            }
            Some(Row::Stock(menu::StockRow::Scan)) if self.stock.status() != stock::Status::Running => {
                self.stock.start(self.date);
            }
            Some(Row::Stock(menu::StockRow::Import)) => self.import_stock(b),
            Some(Row::Read(_) | Row::Stock(_)) | None => {}
        }
    }

    // -----------------------------------------------------------------------------------------------------------
    // What stock left on the card (storage/stock.rs): the stock data menu
    // -----------------------------------------------------------------------------------------------------------

    /// Bytes of stock's rides read per pass: more while no ride runs, little during a ride. Seen on the unit
    /// (2026-10-24, `dev.py bench scan`): 2.1 to 2.5 MB/s read and parsed (the eMMC alone 10.5 MB/s), so 96 KB keeps
    /// a pass near 40 ms and the menu answering (70 rides, 12 MB: about 15 s).
    const SCAN_IDLE: u32 = 96 * 1024;
    const SCAN_RIDING: u32 = 16 * 1024;

    /// A stock data row's value.
    fn stock_value(&self, r: menu::StockRow, v: &mut Line<12>) {
        use menu::StockRow as S;
        let o = &self.stock.out;
        let cp = power_curve::fit_cp(&o.curve);
        match (r, self.stock.status()) {
            (S::Scan, stock::Status::Idle) => v.push_str("GO"),
            (S::Scan, stock::Status::Running) => v.push_u32(u32::from(self.stock.listed)),
            (S::Scan, stock::Status::Done) => v.push_u32(u32::from(o.all.rides)),
            (S::Scan, stock::Status::Failed(_)) => v.push_str("NONE"),
            (S::Import, _) => v.push_str("SETTINGS"),
            (_, s) if s != stock::Status::Done => v.push_str("--"),
            (S::Cp | S::WPrime | S::Fit, _) if cp.is_none() => v.push_str("--"),
            (S::Cp, _) => v.push_u32(cp.map_or(0, |c| u32::from(c.cp))),
            (S::WPrime, _) => push_fixed(v, cp.map_or(0, |c| c.w_prime) as i32, 3, 1),
            (S::Fit, _) => {
                let c = cp.map_or((0, 0), |c| (c.points, c.err_pct));
                v.push_u32(u32::from(c.0));
                v.push_str(" PTS ");
                v.push_u32(u32::from(c.1));
                v.push_str("%");
            }
            (S::WeekKm, _) => v.push_u32(o.week.dist_m / 1000),
            (S::WeekH, _) => push_fixed(v, (o.week.time_s / 360) as i32, 1, 1),
            (S::YearKm, _) => v.push_u32(o.year.dist_m / 1000),
            (S::YearH, _) => push_fixed(v, (o.year.time_s / 360) as i32, 1, 1),
            (S::AllKm, _) => v.push_u32(o.all.dist_m / 1000),
            (S::AllH, _) => push_fixed(v, (o.all.time_s / 360) as i32, 1, 1),
        }
    }

    /// For the dev link's `bench scan`: the scan of stock's rides (started if it is not running) goes on for about
    /// `bytes`; how many it read.
    pub fn bench_scan(&mut self, b: &mut impl Board, bytes: u32) -> u32 {
        if self.stock.status() != stock::Status::Running {
            self.stock.start(self.date);
        }
        let before = self.stock.read_bytes();
        self.stock.step(&mut emmc::Volume(&mut self.emmc, b), bytes);
        self.stock.read_bytes().wrapping_sub(before)
    }

    /// The scan of stock's rides goes on, if it runs.
    fn step_stock(&mut self, b: &mut impl Board) {
        // read once by itself, a while after start-up and not during a ride, for the totals on the overview
        if self.stock.status() == stock::Status::Idle
            && self.gnss_ms >= STOCK_SCAN_MS
            && self.ride.state() == ride::State::Idle
        {
            self.stock.start(self.date);
        }
        if self.stock.status() == stock::Status::Running {
            let budget = if self.ride.state() == ride::State::Idle { Self::SCAN_IDLE } else { Self::SCAN_RIDING };
            self.stock.step(&mut emmc::Volume(&mut self.emmc, b), budget);
        }
    }

    /// Applies stock's settings ([`stock::import`]) where this firmware has them, and says how many.
    fn import_stock(&mut self, b: &mut impl Board) {
        let Ok(i) = self.stock.import(&mut emmc::Volume(&mut self.emmc, b)) else {
            return self.notice("NO STOCK DATA");
        };
        let mut n = 0;
        let s = &mut self.settings;
        for (want, have) in [(i.ftp, &mut s.ftp), (i.max_hr, &mut s.max_hr)] {
            if let Some(v) = want {
                (*have, n) = (v, n + 1);
            }
        }
        if let Some(mm) = i.wheel_mm {
            // the nearest wheel offered
            s.wheel_mm = WHEELS.iter().copied().min_by_key(|w| w.abs_diff(mm)).unwrap_or(s.wheel_mm);
            n += 1;
        }
        if let Some(dg) = i.weight_dg {
            let (lo, hi) = (*menu::WEIGHT_RANGE.start(), *menu::WEIGHT_RANGE.end());
            (s.weight_kg, n) = (((dg + 5) / 10).clamp(lo, hi), n + 1);
        }
        if let Some(j) = i.w_prime {
            // to the menu's 0.5 kJ steps; CP is this firmware's FTP (W' balance uses it), so stock's CP is not taken
            let (lo, hi) = (*menu::WPRIME_RANGE.start(), *menu::WPRIME_RANGE.end());
            let j = ((j + 250) / 500 * 500).min(u32::from(hi)) as u16;
            (s.wprime_j, n) = (j.clamp(lo, hi), n + 1);
        }
        self.save_misc(b);
        for (kind, id) in Kind::ALL.into_iter().zip(i.sensors) {
            if let Some(id) = id {
                b.backup_write(kind.backup(), id.to_word()); // paired at the next start if ANT is not up
                if self.ant {
                    self.set_sensor(b, kind, Sensor::Paired(id));
                }
                n += 1;
            }
        }
        for (k, p) in i.pages.iter().enumerate() {
            if let (Some(p), Some(page)) = (p, self.pages.get_mut(k)) {
                *page = *p;
                save_page(b, k, p);
                n += 1;
            }
        }
        let mut l = Line::<20>::new();
        l.push_str("IMPORTED ");
        l.push_u32(n);
        self.notice(l.as_str());
    }

    // -----------------------------------------------------------------------------------------------------------
    // Planned workouts (storage/workout.rs) and the player (workout.rs)
    // -----------------------------------------------------------------------------------------------------------

    /// The buzzer for a step start and for the countdown's beeps: (Hz, ms).
    const STEP_TONE: (u32, u32) = (2500, 400);
    const COUNT_TONE: (u32, u32) = (2000, 100);

    /// Reads the workouts planned from today on (none if the card cannot be read).
    fn load_calendar(&mut self, b: &mut impl Board) {
        let vol = &mut emmc::Volume(&mut self.emmc, b);
        self.calendar = storage::workout::calendar(vol, self.date).unwrap_or_default();
        self.calendar_day = Some(self.date);
    }

    /// Reads planned workout `k` and shows its steps. One workout at a time: while one runs, only its own steps
    /// open (Select on a step then goes to it).
    #[inline(never)] // the workout read lives in this frame, not in the main loop's
    fn open_workout(&mut self, b: &mut impl Board, k: usize) {
        let Some(p) = self.calendar.get(k).copied() else { return };
        let running = self.player.is_some_and(|pl| pl.step(self.plan.steps()).is_some());
        if running && self.plan_file != Some(p.file) {
            return self.say("WORKOUT RUNNING");
        }
        if !running {
            let vol = &mut emmc::Volume(&mut self.emmc, b);
            let Some(w) = fat::Fat::mount(vol).ok().and_then(|mut fat| storage::workout::load(&mut fat, vol, &p.file))
            else {
                return self.say("UNREADABLE");
            };
            (self.plan, self.plan_file, self.player) = (w, Some(p.file), None);
        }
        self.open(Screen::Menu(Menu::Steps, 0));
    }

    /// Starts the workout opened from step `at`, and the ride if none runs; the first ride page.
    fn start_workout(&mut self, at: usize) {
        if self.ride.state() == ride::State::Idle {
            self.start_pause();
            self.new_ride = false; // the page is drawn anew anyway
        }
        let (p, ev) = workout::Player::start(self.plan.steps(), at, self.ride.dist_m());
        self.player = Some(p);
        self.workout_event(ev);
        (self.depth, self.screen) = (0, Screen::Ride(0));
    }

    /// Shows and sounds what the player said: a banner and a long tone for a new step ("3/14 210-230W"), a short
    /// beep for each of the last 3 seconds.
    fn workout_event(&mut self, ev: workout::Event) {
        let steps = self.plan.steps();
        let mut l = Line::<20>::new();
        match ev {
            workout::Event::Countdown(_) => return self.tone = Self::COUNT_TONE,
            workout::Event::Done => l.push_str("WORKOUT DONE"),
            workout::Event::Step(i) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
                l.push_str(" ");
                if let Some(s) = steps.get(i) {
                    self.push_target(&mut l, s, true);
                    if s.len == storage::workout::Length::Open {
                        l.push_str(" LAP");
                    }
                }
            }
        }
        self.say(l.as_str());
        self.tone = Self::STEP_TONE;
    }

    /// Starts the tone asked for, or ends the one sounding once its time is up (one pass of [`POLL_MS`] at least).
    fn sound(&mut self, b: &mut impl Board) {
        match self.tone {
            (0, 0) => {}
            (0, ms) => {
                self.tone.1 = ms.saturating_sub(POLL_MS);
                if self.tone.1 == 0 {
                    b.beep(0);
                }
            }
            (hz, ms) => {
                b.beep(hz);
                self.tone = (0, ms);
            }
        }
    }

    /// Step `s`'s target: "210-230", with its unit if `unit` ("W", "BPM", "RPM"); "OPEN" for none.
    fn push_target<const N: usize>(&self, l: &mut Line<N>, s: &storage::workout::Step, unit: bool) {
        use storage::workout::Kind;
        let Some((lo, hi)) = s.target.range(self.settings.ftp, self.settings.max_hr) else {
            return l.push_str("OPEN");
        };
        l.push_u32(u32::from(lo));
        if hi != lo {
            l.push_str("-");
            l.push_u32(u32::from(hi));
        }
        if unit {
            l.push_str(match s.target.kind {
                Kind::Power => "W",
                Kind::Hr => "BPM",
                _ => "RPM",
            });
        }
    }

    /// A step's menu row: what ends it as the label ("10:00", "2.50 KM", "LAP"), its target as the value, in the
    /// zone colour of the target's middle; the colour.
    fn step_row(&self, s: &storage::workout::Step, label: &mut Line<16>, value: &mut Line<12>) -> u16 {
        use storage::workout::{Kind, Length};
        match s.len {
            Length::Time(t) => push_clock(label, t),
            Length::Distance(m) => {
                push_km(label, m);
                label.push_str(" KM");
            }
            Length::Open => label.push_str("LAP"),
        }
        self.push_target(value, s, true);
        let (ftp, max) = (self.settings.ftp, self.settings.max_hr);
        match (s.target.kind, s.target.range(ftp, max)) {
            (Kind::Power, Some((lo, hi))) => ride::power_color(ride::power_zone(lo.midpoint(hi), ftp)),
            (Kind::Hr, Some((lo, hi))) => ride::hr_color(ride::hr_zone(lo.midpoint(hi), max)),
            (Kind::Open, _) => ui::GREY,
            _ => ui::WHITE,
        }
    }

    /// The workout fields' text ([`Field::Workout`], [`Field::StepTarget`], [`Field::StepTime`],
    /// [`Field::NextStep`]): "--" without a workout running.
    fn workout_text(&self, f: Field) -> Line<8> {
        use workout::Left;
        let mut l = Line::new();
        let steps = self.plan.steps();
        let now = self.player.and_then(|p| p.step(steps).map(|i| (p, i)));
        match (f, now) {
            (Field::Workout, Some((_, i))) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
            }
            (Field::Workout, None) if self.player.is_some() => l.push_str("DONE"),
            (Field::Workout, None) => match self.calendar.iter().find(|p| p.day == self.date) {
                Some(p) => push_hm(&mut l, p.total_s),
                None => l.push_str("--"),
            },
            (Field::StepTarget, Some((_, i))) => self.push_target(&mut l, steps.get(i).unwrap_or(&OPEN), false),
            (Field::NextStep, Some((_, i))) => match steps.get(i + 1) {
                Some(s) => self.push_target(&mut l, s, false),
                None => l.push_str("END"),
            },
            (Field::StepTime, Some((p, _))) => match p.left(steps, self.ride.dist_m()) {
                Left::Time(t) => push_clock(&mut l, t),
                Left::Distance(m) => push_km(&mut l, m),
                Left::Lap => l.push_str("LAP"),
                Left::Done => l.push_str("--"),
            },
            _ => l.push_str("--"),
        }
        l
    }

    /// [`Field::StepTarget`]'s colour: green in the range, orange under, red over (3 s power, heart rate, cadence);
    /// white without a reading or a target.
    fn target_color(&self) -> u16 {
        use storage::workout::Kind;
        let steps = self.plan.steps();
        let Some(s) = self.player.and_then(|p| p.step(steps)).and_then(|i| steps.get(i)) else { return ui::WHITE };
        let reading = match s.target.kind {
            Kind::Power => self.power.rolling(0),
            Kind::Hr => self.live.get(Metric::Hr),
            Kind::Cadence => self.live.get(Metric::Cadence),
            Kind::Open => None,
        };
        match (reading, s.target.range(self.settings.ftp, self.settings.max_hr)) {
            (Some(v), Some((lo, _))) if v < lo => ui::ACCENT,
            (Some(v), Some((_, hi))) if v > hi => ui::RED,
            (Some(_), Some(_)) => ui::GREEN,
            _ => ui::WHITE,
        }
    }

    /// Acts on a key press ([`Input::of`]). A key held to repeat that acted (a list scrolled, a value stepped) is
    /// not acted on again at its release.
    pub fn press(&mut self, b: &mut impl Board, p: Press) {
        if matches!(p, Press::Short(_)) && core::mem::take(&mut self.repeated) {
            return;
        }
        if let Some(i) = Input::of(p) {
            self.input(b, i);
        }
    }

    /// Acts on an input: Next goes from the main screen through the ride pages and back, Menu opens the menu, Back
    /// toggles the backlight; in the menus Next / Prev move down / up, Select opens or changes, Back goes out; a
    /// number being edited goes up with Prev, down with Next. Off switches off anywhere.
    pub fn input(&mut self, b: &mut impl Board, i: Input) {
        let before = self.screen;
        let woke = self.light_shown == 0 && self.backlight;
        self.idle_ms = 0;
        if woke {
            self.update_light(b); // an input after the time-out: the light comes back (and the input acts)
            if i == Input::Back {
                return; // Back's own job is the light: done
            }
        }
        if self.banner_ms.take().is_some() {
            self.unbanner(b); // and the input acts
        }
        let graph_hit = match (self.screen, i) {
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(x, y) | Gesture::Hold(x, y))) => {
                self.graph_at(x, y)
            }
            _ => None,
        };
        match (self.screen, i) {
            // during a ride (or on its card), a first hold only warns: switching off loses it (nothing is saved yet)
            (_, Input::Off) if self.busy() && self.off_armed_ms == 0 => {
                self.off_armed_ms = OFF_CONFIRM_MS;
                return self.say("HOLD AGAIN: OFF");
            }
            (_, Input::Off) => return self.power_off(b),
            (Screen::Menu(menu, sel), _) if self.editing => {
                let Some(menu::Row::Set(s)) = self.row(menu, sel) else {
                    self.editing = false;
                    return;
                };
                match i {
                    Input::Next | Input::Prev => self.settings.step(s, i == Input::Prev),
                    Input::Touch(Gesture::Swipe(d @ (Dir::Up | Dir::Down))) => self.settings.step(s, d == Dir::Up),
                    Input::NextHeld | Input::PrevHeld => {
                        self.repeated = true;
                        self.settings.step(s, i == Input::PrevHeld);
                    }
                    Input::Select | Input::Back | Input::Touch(Gesture::Tap(..) | Gesture::Swipe(Dir::Right)) => {
                        self.editing = false;
                        self.save_misc(b);
                    }
                    _ => {}
                }
                return self.update_menu_rows(b, menu, sel);
            }
            // the overview is apart from the ride pages, as on stock: Page goes to them, then round them; Back
            // comes back
            (Screen::Main, Input::Next | Input::Touch(Gesture::Swipe(Dir::Left))) => self.screen = self.ride_page(0, 1),
            (Screen::Ride(n), Input::Next | Input::Touch(Gesture::Swipe(Dir::Left))) => {
                self.screen = self.ride_page(n + 1, 1)
            }
            // a tap on a graph shows the next plot (kept with the page), a hold switches between the last minutes and
            // the whole ride
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(..) | Gesture::Hold(..)))
                if graph_hit.is_some() =>
            {
                let Some((k, f)) = graph_hit else { return };
                if matches!(i, Input::Touch(Gesture::Hold(..))) {
                    self.graph_whole = !self.graph_whole;
                } else if let Some(plot) = graph_plot(f) {
                    let all = ride::Plot::ALL;
                    let next = all[(all.iter().position(|&p| p == plot).unwrap_or(0) + 1) % all.len()];
                    let n = if let Screen::Ride(n) = self.screen { n } else { OVERVIEW };
                    let fixed = self.widgets().take_while(|(_, w)| !matches!(w, Widget::Field(_))).count();
                    if let Some(page) = self.pages.get_mut(n)
                        && let Some(slot) = page.fields.get_mut(k.saturating_sub(fixed))
                    {
                        *slot = graph_field(next);
                        save_page(b, n, page);
                    }
                }
                return self.show(b);
            }
            (Screen::Ride(n), Input::Touch(Gesture::Swipe(Dir::Right))) => {
                self.screen = self.ride_page(n + PAGES - 1, PAGES - 1)
            }
            (Screen::Ride(_), Input::Back) => self.screen = Screen::Main,
            (Screen::Main | Screen::Ride(_), Input::Menu | Input::Touch(Gesture::Hold(..))) => {
                self.open(Screen::Menu(Menu::Main, 0))
            }
            // the ride: Select starts, pauses, resumes; End on a paused ride ends it; Prev marks a lap
            (Screen::Ride(_), Input::Select) => {
                self.start_pause();
                if !core::mem::take(&mut self.new_ride) {
                    return;
                }
                return self.show(b); // the page again, with the new ride's map and graphs
            }
            // Start on the overview too, which then shows the first ride page
            (Screen::Main, Input::Select) => {
                self.start_pause();
                self.new_ride = false; // the page is drawn anew anyway
                self.screen = Screen::Ride(0);
            }
            (Screen::Main | Screen::Ride(_), Input::End) => {
                let Some(ev) = self.ride.end() else { return };
                self.recorder(ev);
                self.player = None; // the workout ends with the ride
                self.screen = Screen::Summary; // the ride's card; any key goes on to the overview
            }
            (Screen::Summary, _) => self.screen = Screen::Main,
            (Screen::Ride(_), Input::Prev) => {
                if let Some(ev) = self.ride.lap() {
                    self.say("LAP");
                    self.recorder(ev);
                    // a workout goes on to its next step (an open step ends here)
                    let (steps, dist) = (self.plan.steps(), self.ride.dist_m());
                    if let Some(e) =
                        self.player.as_mut().filter(|p| p.step(steps).is_some()).map(|p| p.skip(steps, dist))
                    {
                        self.workout_event(e);
                    }
                }
                return;
            }
            (Screen::Main, Input::Back) => {
                self.backlight = !self.backlight;
                self.update_light(b);
            }
            (Screen::Menu(menu, sel), Input::Next | Input::Prev | Input::NextHeld | Input::PrevHeld) => {
                self.repeated |= matches!(i, Input::NextHeld | Input::PrevHeld);
                let rows = self.rows(menu).max(1);
                let down = matches!(i, Input::Next | Input::NextHeld);
                let next = if down { (sel + 1) % rows } else { (sel + rows - 1) % rows };
                self.screen = Screen::Menu(menu, next);
                if menu::first_shown(next, ui::MENU_ROWS) == menu::first_shown(sel, ui::MENU_ROWS) {
                    return self.update_menu_rows(b, menu, next);
                }
            }
            (Screen::Menu(menu, sel), Input::Touch(Gesture::Swipe(d @ (Dir::Up | Dir::Down)))) => {
                // a screen of rows at a time: up shows the next, down the previous
                let (first, rows) = (menu::first_shown(sel, ui::MENU_ROWS), self.rows(menu));
                let next = if d == Dir::Up { first + ui::MENU_ROWS } else { first.saturating_sub(ui::MENU_ROWS) };
                if next >= rows || next == first {
                    return;
                }
                self.screen = Screen::Menu(menu, next);
            }
            (Screen::Menu(menu, sel), Input::Select | Input::Touch(Gesture::Tap(..))) => {
                let sel = match i {
                    Input::Touch(Gesture::Tap(_, y)) => {
                        let slot = (u32::from(y).checked_sub(ui::LIST_Y)).map(|d| (d / ui::LIST_ROW) as usize);
                        match slot.map(|k| menu::first_shown(sel, ui::MENU_ROWS) + k) {
                            Some(row) if self.row(menu, row).is_some() => row,
                            _ => return,
                        }
                    }
                    _ => sel,
                };
                self.screen = Screen::Menu(menu, sel);
                let shown = self.screen;
                self.activate(b, menu, sel);
                if self.screen == shown {
                    return self.update_menu_rows(b, menu, sel);
                }
            }
            (
                Screen::Menu(..) | Screen::Nearby | Screen::Past,
                Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)),
            ) => self.close(),
            _ => return,
        }
        if self.screen != before {
            self.show(b);
        }
    }

    /// Takes what the Nordic chip sent since the last pass: advertisers, sensor data, the scan's end.
    fn poll_nrf(&mut self, b: &mut impl Board) {
        let mut title = false;
        let mut buf = [0; 64];
        for _ in 0..64 {
            let n = b.nrf_recv(&mut buf);
            for &byte in buf.get(..n).unwrap_or(&[]) {
                let Some(p) = self.rx.push(byte) else { continue };
                if let Some(r) = nrf::adv_report(p) {
                    title |= self.nearby.add(&r);
                    self.list_dirty = true;
                } else if let Some((ch, data)) = nrf::ant_broadcast(p)
                    && let Some(kind) = Kind::from_channel(ch)
                {
                    if kind == Kind::Hr && self.heard_ms[kind as usize] >= sensors::STALE_MS {
                        // back after a drop-out: the beats before it are not part of this series (and the beat
                        // time may have wrapped meanwhile, which would make one false interval)
                        (self.hrv, self.decoders.beats) = Default::default();
                    }
                    self.decoders.decode(kind, data, self.settings.wheel_mm, &mut self.live);
                    for &rr in self.decoders.beats.rr() {
                        self.hrv.push(rr);
                    }
                    self.heard_ms[kind as usize] = 0;
                    if self.sensors[kind as usize] == Sensor::Search {
                        self.ask |= 1 << ch;
                    }
                } else if let Some(kind) = nrf::ant_closed(p).and_then(Kind::from_channel) {
                    self.resting_ms[kind as usize] = 0; // its search timed out: reopened after a rest
                } else if nrf::event_id(p) == Some(nrf::EVT_TIMEOUT) {
                    self.scanning = false;
                    title = true;
                }
            }
            if n == 0 {
                break;
            }
        }
        if title && self.screen == Screen::Nearby {
            ui::draw_nearby(&mut Panel(b, (0, self.bottom())), self.nearby.count(), self.scanning);
        }
    }

    /// Reads what the GNSS receiver has (sending its configuration once): the latest fix, its state in the strip,
    /// and the speed when no speed sensor is paired.
    fn poll_gnss(&mut self, b: &mut impl Board) {
        if self.gnss_ms < GNSS_ON_MS && self.gnss_ms + POLL_MS >= GNSS_ON_MS {
            b.gnss_power(true);
        }
        self.gnss_ms = self.gnss_ms.saturating_add(POLL_MS);
        self.pvt_ms = self.pvt_ms.saturating_add(POLL_MS);
        let mut send = if self.gnss_ms >= GNSS_CONFIG_MS { self.next_gnss_message(b) } else { None };
        for _ in 0..GNSS_READ / 32 {
            let mut buf = send.take().unwrap_or([0xff; 32]);
            b.gnss_transfer(&mut buf);
            if buf == [0xff; 32] && self.gnss.idle() {
                break; // nothing waiting in the receiver
            }
            for &byte in &buf {
                if let Some(pvt) = self.gnss.push(byte).and_then(|(c, i, p)| gnss::pvt(c, i, p)) {
                    self.pvt = Some(pvt);
                    self.pvt_ms = 0;
                    self.learn_aid(b, pvt);
                    let sensor =
                        [Kind::Speed, Kind::SpeedCadence].iter().any(|&k| self.sensors[k as usize] != Sensor::Off);
                    if pvt.good() {
                        self.track.add(pvt.lat, pvt.lon, pvt.heading);
                    }
                    if pvt.good() && !sensor {
                        // mm/s to km/h x 10
                        self.live.set(Metric::Speed, (pvt.speed_mm_s * 36 / 1000).min(u32::from(u16::MAX)) as u16);
                    }
                }
            }
        }
        if self.pvt_ms > PVT_STALE_MS {
            self.pvt = None;
        }
        self.strip.gps = self.pvt.map(|p| if p.good() { gnss::bars(p.sats) } else { 0 });
        // blinks while searching; steady otherwise, so the strip is not redrawn every second for nothing
        self.strip.blink = self.strip.gps != Some(0) || self.seconds.is_multiple_of(2);
    }

    /// The next message for the receiver after power-up, one per pass: the configuration, the time (UTC from the
    /// RTC and its learnt offset, or the RTC as is with a 14 h accuracy) and the last position when known. Then
    /// None.
    fn next_gnss_message(&mut self, b: &mut impl Board) -> Option<[u8; 32]> {
        loop {
            self.gnss_sent = self.gnss_sent.saturating_add(1);
            match self.gnss_sent {
                1 => return Some(gnss::config()),
                2 => {
                    // the RTC holds local time: with the offset learnt at a fix, UTC within a couple of seconds;
                    // without it, the RTC as is, within 14 h (any time zone): the date still helps
                    let (offset, acc_s) = self.aid.offset_15min.map_or((0, 14 * 3600), |o| (o, 2));
                    let (tr, dr) = b.rtc();
                    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
                    let utc = epoch_s((y, mo, d, h, m, s)) - i64::from(offset) * 900;
                    return Some(gnss::mga_time(from_epoch_s(utc), acc_s));
                }
                3 => {
                    let Some((lat, lon)) = self.aid.pos else { continue };
                    return Some(gnss::mga_pos(lat, lon, 10_000)); // where it was: within 10 km, most rides
                }
                _ => return None,
            }
        }
    }

    /// Keeps what the next power-up needs from a fix: the RTC's offset from UTC (once the receiver's time is
    /// valid), the position (a good fix); written to the backup registers when it changes.
    fn learn_aid(&mut self, b: &mut impl Board, pvt: gnss::Pvt) {
        let mut aid = self.aid;
        if pvt.time_ok {
            let (tr, dr) = b.rtc();
            let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
            let diff = epoch_s((y, mo, d, h, m, s)) - epoch_s(pvt.utc);
            let steps = (diff + diff.signum() * 450) / 900; // nearest 15 minutes
            if steps.abs() <= 14 * 4 {
                aid.offset_15min = Some(steps as i8);
            }
        }
        if pvt.good() {
            // to the 1e-4 degrees kept: moving within 11 m does not rewrite the registers each second
            aid.pos = Some((pvt.lat / 1000 * 1000, pvt.lon / 1000 * 1000));
        }
        if aid != self.aid {
            self.aid = aid;
            for (r, w) in AID_BKP.into_iter().zip(aid.to_words()) {
                b.backup_write(r, w);
            }
        }
    }

    /// One pass of the main loop: read the time, the radio and the keys, redraw what changed. The caller sleeps
    /// [`POLL_MS`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        let (tr, dr) = b.rtc();
        let (h, m, sec) = rtc_time_from_tr(tr);
        self.strip.time = (h, m);
        self.date = rtc_date_from_dr(dr);
        if sec != self.seconds {
            // every second gone, not one a pass: a pass that took longer (an eMMC or radio wait) loses no ride time;
            // more than MAX_CATCH_UP is the clock set, not riding
            let gone = (sec + 60 - self.seconds) % 60;
            for _ in 0..gone.clamp(1, MAX_CATCH_UP) {
                self.second();
            }
        }
        if let Some(a) = self.alert.take() {
            self.say(a.text());
            self.tones = (&BEEP_ALERT, 0, false);
        }
        self.play_tones(b);
        self.off_armed_ms = self.off_armed_ms.saturating_sub(POLL_MS);
        self.seconds = sec;
        self.sound(b);
        self.battery_ms = self.battery_ms.saturating_add(POLL_MS);
        if self.battery_ms >= BATTERY_MS {
            self.battery_ms = 0;
            let charging = b.charger_present();
            let reading = b.battery_adc().and_then(|(raw, vref, cal)| battery::millivolts(raw, vref, cal));
            if let Some(mv) = reading
                && self.battery.update(mv, charging)
            {
                return self.power_off(b); // empty: off before the cell is drawn too low
            }
            self.strip.battery = self.battery.percent();
            self.strip.charging = charging;
            let low = self.strip.battery.is_some_and(|p| p <= 15) && !charging;
            if low && !self.told_low {
                self.notice("BATTERY LOW");
            }
            self.told_low = low;
            if self.chips.baro {
                // the conversion started 10 s ago; the next one now (one-shot: the chip sleeps in between)
                self.baro = ambient::baro_read(b);
                ambient::baro_start(b);
            }
        }
        self.poll_light(b);
        if self.ble.is_some() {
            self.poll_nrf(b);
        }
        self.poll_gnss(b);
        if !self.had_fix && self.pvt.is_some_and(|p| p.good()) {
            self.had_fix = true;
            self.notice("GPS FIX");
        }
        self.strip.rec = match self.ride.state() {
            ride::State::Idle => Rec::Idle,
            _ if self.ride.moving() => Rec::Running,
            _ => Rec::Paused,
        };
        self.update_strip(b); // after everything that changes it (time, battery, GPS, the ride)
        for kind in Kind::ALL {
            if self.ask & 1 << kind.channel() != 0
                && let Ok(Some(id)) = nrf::ant_channel_id(b, &mut self.rx, kind.channel())
            {
                self.sensors[kind as usize] = Sensor::Paired(id);
                b.backup_write(kind.backup(), id.to_word());
                let mut l = Line::<20>::new();
                l.push_str(kind.name());
                l.push_str(" PAIRED");
                self.notice(l.as_str());
            }
        }
        self.ask = 0;
        self.live.tick(POLL_MS);
        for h in &mut self.heard_ms {
            *h = h.saturating_add(POLL_MS);
        }
        for kind in Kind::ALL {
            let i = kind as usize;
            if self.resting_ms[i] == u32::MAX {
                continue;
            }
            self.resting_ms[i] += POLL_MS;
            if self.resting_ms[i] >= SEARCH_REST_MS {
                let id = match self.sensors[i] {
                    Sensor::Paired(id) => Some(id),
                    _ => None, // still pairing: search for any
                };
                let ok = nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok();
                self.resting_ms[i] = if ok { u32::MAX } else { 0 }; // failed: another rest
            }
        }
        self.step_stock(b);
        self.redraw_ms = self.redraw_ms.saturating_add(POLL_MS);
        if let Some(ms) = self.banner_ms {
            if !core::mem::replace(&mut self.banner_drawn, true) {
                ui::draw_banner(&mut Panel(b, (0, ui::H)), self.banner.as_str());
            }
            self.banner_ms = Some(ms + POLL_MS).filter(|&ms| ms < BANNER_MS);
            if self.banner_ms.is_none() {
                self.unbanner(b);
            }
        }
        match self.screen {
            Screen::Main | Screen::Ride(_) => self.update_page(b),
            Screen::Summary => {
                // the elevation stays as it was; the map keeps up with points still coming (the unit moving on)
                let bottom = self.bottom();
                if let Some((m, _)) = &mut self.summary {
                    map::update(&mut Panel(b, (0, bottom)), map::area(ui::card::MAP), &self.track, m, None);
                }
            }
            Screen::Nearby if self.list_dirty && self.redraw_ms >= LIST_REDRAW_MS => {
                ui::draw_list(&mut Panel(b, (0, self.bottom())), &self.nearby);
                self.list_dirty = false;
                self.redraw_ms = 0;
            }
            Screen::Menu(menu @ (Menu::Main | Menu::Sensors | Menu::Gps | Menu::Stock | Menu::History), sel)
                if self.redraw_ms >= LIST_REDRAW_MS =>
            {
                self.update_menu_rows(b, menu, sel);
                self.redraw_ms = 0;
            }
            _ => {}
        }
        let mask = b.keys_down();
        self.idle_ms = if mask != 0 { 0 } else { self.idle_ms.saturating_add(POLL_MS) };
        if let Some(p) = self.keys.poll(mask, POLL_MS) {
            self.press(b, p);
        }
        if mask == 0 {
            self.repeated = false;
        }
        self.update_light(b);
    }

    /// The touch panel, called by the main loop as soon as INT falls and then every [`TOUCH_MS`] while a finger is
    /// down ([`App::touching`]); `dt_ms` since the previous call. Reads the point only if INT is low or a finger was
    /// down (the release makes the gesture). A touch keeps the backlight on like a key. The gesture, if one ended
    /// here, comes back as an input for the caller to act on once it has stopped sleeping ([`App::input`]).
    pub fn touch(&mut self, b: &mut impl Board, dt_ms: u32) -> Option<Input> {
        if !self.touch_ok || !(self.touch.down() || b.touch_int()) {
            return None;
        }
        let mut buf = [0; 3];
        let p = if b.touch_read(touch::REG_POINT, &mut buf) { touch::point(buf) } else { None };
        if p.is_some() {
            self.idle_ms = 0;
        }
        self.touch.feed(p, dt_ms).map(Input::Touch)
    }

    /// A finger is on the panel: sample it every [`TOUCH_MS`].
    pub fn touching(&self) -> bool {
        self.touch.down()
    }

    /// The light sensor measures while the backlight follows the ambient light (and is on), or the sensors menu
    /// shows it, and is read every [`LIGHT_MS`]; in standby otherwise.
    fn poll_light(&mut self, b: &mut impl Board) {
        if !self.chips.light {
            return;
        }
        self.light_ms = self.light_ms.saturating_add(POLL_MS);
        let wanted = (self.settings.light_level == LIGHT_AUTO && self.light_shown != 0)
            || matches!(self.screen, Screen::Menu(Menu::Sensors, _));
        match (wanted, self.measuring) {
            (true, false) => {
                self.measuring = ambient::light_start(b);
                self.light_ms = 0;
            }
            (true, true) if self.light_ms >= LIGHT_MS => {
                if let Some(lux10) = ambient::light_read(b) {
                    self.auto_level = self.auto.update(lux10);
                    self.light_ms = 0;
                }
            }
            (false, true) => {
                ambient::light_stop(b);
                self.measuring = false;
            }
            _ => {}
        }
    }

    /// Once a second (the RTC's second changed): the ride and the history take the readings.
    fn second(&mut self) {
        let input = ride::Input {
            speed_x10: self.live.get(Metric::Speed),
            hr: self.live.get(Metric::Hr),
            cadence: self.live.get(Metric::Cadence),
            power: self.live.get(Metric::Power),
            alt_mm: self.pvt.filter(|p| p.good()).map(|p| p.alt_mm),
        };
        let timer = self.ride.timer_s();
        if let Some(ev) = self.ride.second(&input) {
            self.recorder(ev);
        }
        if self.ride.state() == ride::State::Running {
            let s = &self.settings;
            let due = self.alerts.second(self.ride.timer_s(), input.hr, s.eat_min, s.drink_min, s.hr_alert);
            self.alert = self.alert.or(due);
        }
        // the workout's step clock moves with the ride's timer
        let (ticked, dist, steps) = (self.ride.timer_s() != timer, self.ride.dist_m(), self.plan.steps());
        if let Some(e) = self.player.as_mut().and_then(|p| p.second(steps, ticked, dist)) {
            self.workout_event(e);
        }
        self.power.second(input.power, &self.ride, self.settings.ftp, self.settings.wprime_j);
        let wbal = self.power.wbal_j().map(|j| (j.max(0) / 10).clamp(1, i32::from(u16::MAX)) as u16);
        let dfa = self.hrv.alpha1().filter(|_| input.hr.is_some()).map(|a| (a * 100.0 + 0.5).clamp(1.0, 999.0) as u16);
        let alt = input.alt_mm.map(|mm| (mm / 1000 + ride::ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
        self.history.second([input.power, input.hr, wbal, dfa, alt]);
    }

    /// As stock: the radio chip off too (or it drains the battery), screen off, wait for the key to be released
    /// (or it would wake the unit at once), then standby.
    fn power_off(&mut self, b: &mut impl Board) {
        if self.ble.is_some() {
            nrf::system_off(b);
        }
        b.gnss_power(false);
        play(b, &BEEP_OFF);
        if self.measuring {
            ambient::light_stop(b);
        }
        b.backlight(0);
        panel::off(b);
        while b.keys_down() & Key::Back as u8 != 0 {
            b.delay_ms(POLL_MS);
        }
        b.delay_ms(50);
        b.standby();
    }

    /// Redraws the status strip if what it shows changed.
    fn update_strip(&mut self, b: &mut impl Board) {
        if self.strip != self.strip_shown {
            ui::draw_strip_changes(&mut Panel(b, (0, ui::H)), &self.strip, Some(&self.strip_shown));
            self.strip_shown = self.strip;
        }
    }

    /// Shows the USB link state in the strip.
    pub fn set_usb(&mut self, b: &mut impl Board, s: UsbDot) {
        self.strip.usb = s;
        self.update_strip(b);
    }

    /// Row `y` of the screen as the app drew it: the screenshot source.
    pub fn render_row(&self, y: u32) -> Row {
        let mut row = Row::new(y);
        ui::draw_strip(&mut row, &self.strip_shown);
        self.draw_screen(&mut row);
        row
    }
}

/// `v / 10^shift` with `decimals` decimals (truncated): `push_fixed(l, 488566100, 7, 5)` gives "48.85661".
fn push_fixed(l: &mut Line<12>, v: i32, shift: u32, decimals: u32) {
    if v < 0 {
        l.push_str("-");
    }
    let v = v.unsigned_abs();
    let unit = 10u32.pow(shift);
    l.push_u32(v / unit);
    if decimals > 0 {
        l.push_str(".");
        let frac = v % unit / 10u32.pow(shift - decimals);
        let digits = 10u32.pow(decimals - 1);
        let mut d = digits;
        while d > 0 {
            l.push_str(ui::DIGITS[(frac / d % 10) as usize]);
            d /= 10;
        }
    }
}

/// The plot a graph field shows, None for a field that is no graph.
fn graph_plot(f: Field) -> Option<ride::Plot> {
    use ride::Plot;
    Some(match f {
        Field::Graph => Plot::Power,
        Field::HrGraph => Plot::Hr,
        Field::WbalGraph => Plot::Wbal,
        Field::DfaGraph => Plot::Dfa,
        Field::AltGraph => Plot::Alt,
        _ => return None,
    })
}

/// The graph field showing `plot`.
fn graph_field(plot: ride::Plot) -> Field {
    use ride::Plot;
    match plot {
        Plot::Power => Field::Graph,
        Plot::Hr => Field::HrGraph,
        Plot::Wbal => Field::WbalGraph,
        Plot::Dfa => Field::DfaGraph,
        Plot::Alt => Field::AltGraph,
    }
}

/// `v / 10^decimals` with its decimals ("0.75").
fn push_fixed_u(l: &mut Line<8>, v: u32, decimals: u32) {
    let unit = 10u32.pow(decimals);
    l.push_u32(v / unit);
    if !v.is_multiple_of(unit) {
        l.push_str(".");
        let mut d = unit / 10;
        while d > 0 {
            l.push_str(ui::DIGITS[(v / d % 10) as usize]);
            d /= 10;
        }
    }
}

/// No step: what a workout field shows past the last one.
const OPEN: storage::workout::Step = storage::workout::Step {
    len: storage::workout::Length::Open,
    target: storage::workout::Target::OPEN,
    intensity: 0,
};

/// Seconds as "h:mm".
fn push_hm<const N: usize>(l: &mut Line<N>, s: u32) {
    l.push_u32(s / 3600);
    l.push_str(":");
    l.push_str(ui::DIGITS[(s / 600 % 6) as usize]);
    l.push_str(ui::DIGITS[(s / 60 % 10) as usize]);
}

/// Seconds as "m:ss", or "h:mm:ss" from an hour.
fn push_clock<const N: usize>(l: &mut Line<N>, s: u32) {
    if s >= 3600 {
        l.push_u32(s / 3600);
        l.push_str(":");
        l.push_str(ui::DIGITS[(s / 600 % 6) as usize]);
        l.push_str(ui::DIGITS[(s / 60 % 10) as usize]);
    } else {
        l.push_u32(s / 60);
    }
    l.push_str(":");
    l.push_str(ui::DIGITS[(s / 10 % 6) as usize]);
    l.push_str(ui::DIGITS[(s % 10) as usize]);
}

/// Metres as km with 2 decimals.
fn push_km<const N: usize>(l: &mut Line<N>, m: u32) {
    l.push_u32(m / 1000);
    l.push_str(".");
    l.push_str(ui::DIGITS[(m / 100 % 10) as usize]);
    l.push_str(ui::DIGITS[(m / 10 % 10) as usize]);
}

/// Writes ride page (or overview) `n`'s configuration to its backup registers.
fn save_page(b: &mut impl Board, n: usize, page: &PageCfg) {
    if let Some(regs) = PAGE_BKP.get(n) {
        for (r, w) in regs.iter().zip(page.to_words()) {
            b.backup_write(*r, w);
        }
    }
}

/// One step of FNV-1a over `bytes` (fingerprints of what a box shows).
fn fnv(h: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(h, |h, &x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// Months as the screens write them.
const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// "FRI 23 OCT".
fn date_text((y, m, d): (u16, u8, u8)) -> Line<12> {
    const DAYS: [&str; 7] = ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"]; // 1970-01-01 was a Thursday
    let mut l = Line::new();
    l.push_str(DAYS[days_from_civil(y, m, d).rem_euclid(7) as usize]);
    l.push_str(" ");
    l.push_u32(u32::from(d));
    l.push_str(" ");
    l.push_str(MONTHS.get(usize::from(m).wrapping_sub(1)).copied().unwrap_or(""));
    l
}

/// A menu's title (a page's is followed by its number).
fn menu_title(m: Menu) -> &'static str {
    match m {
        Menu::Main => "MENU",
        Menu::Sensors => "SENSORS",
        Menu::Page(_) => "PAGE ",
        Menu::Gps => "GPS",
        Menu::Stock => "STOCK DATA",
        Menu::Alerts => "ALERTS",
        Menu::History => "HISTORY",
        Menu::Workouts => "WORKOUTS",
        Menu::Steps => "WORKOUT",
    }
}

/// A page's layout by its name ("OFF" for none).
fn layout_text(l: &mut Line<12>, layout: u8) {
    l.push_str(ui::layout(layout).map_or("?", |(_, name)| name));
}

/// The time of day, hours and minutes.
fn rtc_hm(b: &mut impl Board) -> (u8, u8) {
    let (h, m, _) = rtc_time_from_tr(b.rtc().0);
    (h, m)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn rtc_decoding() {
        assert_eq!(rtc_time_from_tr(0), (0, 0, 0));
        assert_eq!(rtc_time_from_tr(0x0012_3456), (12, 34, 56));
        assert_eq!(rtc_time_from_tr(0x0040_0000 | 0x0009_0000), (9, 0, 0)); // PM bit ignored
    }

    #[test]
    fn fixed_point() {
        let s = |v, shift, dec| {
            let mut l = Line::<12>::new();
            push_fixed(&mut l, v, shift, dec);
            std::string::String::from(l.as_str())
        };
        assert_eq!(s(488_566_100, 7, 5), "48.85661");
        assert_eq!(s(-1_224_194_200, 7, 5), "-122.41942");
        assert_eq!(s(23_500, 3, 0), "23");
        assert_eq!(s(1_000_500, 7, 5), "0.10005");
    }

    #[test]
    fn short_on_release_long_once_while_held_repeats_between() {
        let mut k = Keys::default();
        assert_eq!(k.poll(Key::Page as u8, 50), None);
        assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Page)));
        assert_eq!(k.poll(0, 50), None);
        let held: std::vec::Vec<_> = (0..42).map(|_| k.poll(Key::Page as u8, 50)).collect();
        let at = |p| held.iter().enumerate().filter(|(_, x)| **x == Some(p)).map(|(i, _)| 50 * (i + 1)).collect();
        let repeats: std::vec::Vec<usize> = at(Press::Repeat(Key::Page));
        assert_eq!(repeats.first(), Some(&500), "repeats from 0.5 s");
        assert!(repeats.windows(2).all(|w| w[1] - w[0] == 100 || w[1] - w[0] == 200), "every 0.1 s");
        assert_eq!(at(Press::Long(Key::Page)), [2000], "long once, at 2 s");
        assert_eq!(k.poll(0, 50), None, "nothing at the release after a long press");
        let back: std::vec::Vec<_> = (0..60).map(|_| k.poll(Key::Back as u8, 50)).collect();
        assert_eq!(back[59], Some(Press::Long(Key::Back)), "Back: at 3 s");
    }
}
