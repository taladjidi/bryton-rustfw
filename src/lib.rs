//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ambient;
pub mod battery;
pub mod dev;
pub mod emmc;
pub mod fat;
pub mod fit;
pub mod fonts;
pub mod gnss;
pub mod map;
pub mod menu;
pub mod nrf;
pub mod panel;
pub mod ride;
pub mod sensors;
#[cfg(test)]
mod sim;
pub mod text;
pub mod tiles;
pub mod ui;

use menu::{Input, Setting};
use nrf::SensorId;
use panel::Panel;
use sensors::{Decoders, Kind, Live, Metric};
use text::Line;
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

/// A ride page: how many fields (one of [`ui::LAYOUTS`], 0: off) and what each shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCfg {
    /// Field count.
    pub layout: u8,
    /// The fields, the first `layout` of them used.
    pub fields: [Field; 6],
}

impl PageCfg {
    /// As kept in a backup register: tag 0xa in the top nibble, the layout, then 4 bits per field.
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, ui::Field};
    /// let p = PageCfg { layout: 3, fields: [Field::Power, Field::Hr, Field::Time, Field::Time, Field::Time, Field::Time] };
    /// assert_eq!(PageCfg::from_word(p.to_word()), Some(p));
    /// assert_eq!(PageCfg::from_word(0), None);
    /// ```
    pub fn to_word(self) -> u32 {
        let fields = self.fields.iter().enumerate().fold(0, |w, (i, f)| w | (*f as u32) << (4 * i));
        (0xa << 28) | (u32::from(self.layout) << 24) | fields
    }

    /// The page kept in `w`, if [`PageCfg::to_word`] wrote it.
    pub fn from_word(w: u32) -> Option<Self> {
        let layout = (w >> 24 & 0xf) as u8;
        if w >> 28 != 0xa || !ui::LAYOUTS.contains(&layout) {
            return None;
        }
        Some(PageCfg { layout, fields: core::array::from_fn(|i| Field::from_index(w >> (4 * i) & 0xf)) })
    }
}

/// Ride pages at most.
pub const PAGES: usize = 3;
/// The backup registers of the ride pages.
const PAGE_BKP: [usize; PAGES] = [14, 15, 16];
/// The backup register of the wheel size, the backlight, max heart rate and FTP: see [`Settings`].
const WHEEL_BKP: usize = 11;

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
}

impl Settings {
    /// The settings in `w` (defaults for an empty or foreign word).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, WHEELS};
    /// let m = Settings { wheel_mm: WHEELS[3], max_hr: 187, ftp: 265, light_level: 4, light_timeout: 3 };
    /// assert_eq!(Settings::from_word(m.to_word()), m);
    /// // words from earlier builds: 2136 mm, AUTO, 60 s; then max HR code 7 (225), FTP code 11 (100)
    /// let old = Settings::from_word(0xa5 << 24 | 3 << 20 | 4 << 16 | 2136);
    /// assert_eq!((old.wheel_mm, old.light_level, old.light_timeout, old.max_hr), (2136, 4, 3, 190));
    /// let old = Settings::from_word(0xa6 << 24 | 11 << 8 | 7 << 4 | 3);
    /// assert_eq!((old.wheel_mm, old.max_hr, old.ftp), (WHEELS[3], 225, 100));
    /// ```
    pub fn from_word(w: u32) -> Settings {
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
            },
            0xa6 => Settings {
                wheel_mm: index(w & 7),
                max_hr: code_hr(w >> 4 & 0xf),
                ftp: code_ftp(w >> 8 & 0xf),
                light_level: level(w >> 16 & 7),
                light_timeout: (w >> 20 & 3) as usize,
            },
            0xa5 => Settings {
                wheel_mm: wheel((w & 0xfff) as u16),
                max_hr: ride::MAX_HR,
                ftp: ride::FTP,
                light_level: level(w >> 16 & 0xf),
                light_timeout: (w >> 20 & 0xf) as usize % LIGHT_TIMEOUTS_S.len(),
            },
            _ => {
                Settings { wheel_mm: WHEELS[1], max_hr: ride::MAX_HR, ftp: ride::FTP, light_level: 0, light_timeout: 0 }
            }
        }
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
            Setting::MaxHr | Setting::Ftp => {}
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
const DEFAULT_PAGES: [PageCfg; PAGES] = {
    use Field::*;
    [
        PageCfg { layout: 3, fields: [Speed, Hr, Cadence, Time, Time, Time] },
        PageCfg { layout: 4, fields: [Speed, Hr, Cadence, Power, Time, Time] },
        PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
    ]
};

// ---------------------------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------------------------

/// Tones played at power-on and power-off: (Hz, ms). Rising, then falling (stock's own melodies are not traced).
pub const BEEP_ON: [(u32, u32); 2] = [(1500, 80), (2500, 120)];
/// The power-off tones.
pub const BEEP_OFF: [(u32, u32); 2] = [(2500, 80), (1500, 120)];

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

/// How often the ambient light is measured while the backlight follows it (and while the menu shows it).
pub const LIGHT_MS: u32 = 3000;

/// From the start of a light measurement to its reading: one 400 ms conversion and a margin.
pub const LIGHT_WAIT_MS: u32 = 600;

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
}

/// The GPS row of the main menu.
pub const GPS_ROW: usize = 9;
/// The nearby BLE devices row of the main menu.
pub const NEARBY_ROW: usize = 10;
const _: () = assert!(matches!(menu::MAIN[GPS_ROW], menu::Row::Open(Menu::Gps)));
const _: () = assert!(matches!(menu::MAIN[NEARBY_ROW], menu::Row::Nearby));

/// What a graph field shows on screen: the bars up to `n` ([`ride::History::len`]), of power or heart rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GraphShown {
    n: u32,
    power: bool,
}

/// What a page box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Widget {
    /// The big time with its seconds, each digit in its own cell.
    Clock,
    /// Ferris the crab (drawn once).
    Ferris,
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
    /// The clock's digits: H H M M S S.
    Digits([u8; 6]),
    /// A value and its colour.
    Value(Line<8>, u16),
    /// A map.
    Map(map::Shown),
    /// A graph.
    Graph(GraphShown),
}

/// Boxes on a page at most.
const BOXES: usize = 6;

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
    pages: [PageCfg; PAGES],
    /// The page's boxes as drawn ([`App::widgets`]).
    drawn: [Drawn; BOXES],
    /// The ride: timer, totals, laps.
    ride: ride::Ride,
    /// Heart rate and power over the last minutes, and the graphs on screen by ride field.
    history: ride::History,
    /// The menu rows on screen, as fingerprints ([`App::menu_row_print`]).
    menu_shown: [u32; ui::MENU_ROWS],
    /// The ride's track, from the good fixes.
    track: map::Track,
    /// The eMMC, for the basemap and the dev link (identified at first use, never at boot).
    emmc: emmc::Emmc,
    /// Roads and water under the first map field of a page, drawn once per view.
    basemap: tiles::Basemap,
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
        let misc = Settings::from_word(b.backup_read(WHEEL_BKP));
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
            seconds: 0,
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
            pages: core::array::from_fn(|i| PageCfg::from_word(b.backup_read(PAGE_BKP[i])).unwrap_or(DEFAULT_PAGES[i])),
            drawn: Default::default(),
            ride: ride::Ride::new(),
            history: ride::History::default(),
            menu_shown: [0; ui::MENU_ROWS],
            track: map::Track::new(),
            emmc: emmc::Emmc::new(),
            basemap: tiles::Basemap::new(),
        };
        if app.chips.baro {
            ambient::baro_start(b); // read at the first step
        }
        if let Some((lat, lon)) = app.aid.pos {
            app.track.seed(lat, lon); // the map shows where the unit last was until the first fix
        }
        app.strip_shown = app.strip;
        ui::draw_strip(&mut Panel(b), &app.strip);
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

    /// Writes the wheel size and the backlight settings to their backup register.
    fn save_misc(&self, b: &mut impl Board) {
        b.backup_write(WHEEL_BKP, self.settings.to_word());
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
    pub fn pages(&self) -> &[PageCfg; PAGES] {
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
            Field::Map | Field::Graph => return l,
            Field::Hr => (live(Metric::Hr), false),
            Field::Speed => (live(Metric::Speed), true),
            Field::Cadence => (live(Metric::Cadence), false),
            Field::Power => (live(Metric::Power), false),
            Field::AvgSpeed => (r.avg_speed_x10().map(u32::from), true),
            Field::MaxSpeed => (Some(u32::from(r.max_speed_x10())), true),
            Field::AvgHr => (r.avg_hr().map(u32::from), false),
            Field::AvgPower => (r.avg_power().map(u32::from), false),
            Field::Ascent => (Some(r.ascent_m()), false),
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

    /// The colour of ride field `f`'s value: heart rate and power in their zone's colour, the rest white.
    fn field_color(&self, f: Field) -> u16 {
        match (f, self.live.get(Metric::Hr), self.live.get(Metric::Power)) {
            (Field::Hr, Some(v), _) => ride::hr_color(ride::hr_zone(v, self.settings.max_hr)),
            (Field::Power, _, Some(v)) => ride::power_color(ride::power_zone(v, self.settings.ftp)),
            _ => ui::WHITE,
        }
    }

    /// The graph shows power when a power meter is paired, else heart rate.
    fn graph_now(&self) -> GraphShown {
        GraphShown { n: self.history.len(), power: self.sensors[Kind::Power as usize] != Sensor::Off }
    }

    /// Column `i` of a graph of `cols` columns `h` high showing `g`: the bar's height and colour. Power is drawn
    /// up to twice the FTP, heart rate from half the maximum to the maximum (a bar of a reading is at least 1 high).
    fn graph_bar(&self, g: GraphShown, cols: u32, h: u32, i: u32) -> (u32, u16) {
        let Some(k) = (g.n + i).checked_sub(cols) else { return (0, ui::BLACK) };
        let v = self.history.bar(k, g.power);
        if v == 0 {
            return (0, ui::BLACK);
        }
        let (max, ftp) = (self.settings.max_hr, self.settings.ftp);
        let (low, high, color) = if g.power {
            (0, 2 * u32::from(ftp), ride::power_color(ride::power_zone(v, ftp)))
        } else {
            (u32::from(max / 2), u32::from(max), ride::hr_color(ride::hr_zone(v, max)))
        };
        let height = u32::from(v).saturating_sub(low) * h / (high - low).max(1);
        (height.clamp(1, h), color)
    }

    /// Row `i` of `menu`; None past its last.
    fn row(&self, menu: Menu, i: usize) -> Option<menu::Row> {
        use menu::Row;
        match menu {
            Menu::Main => menu::MAIN.get(i).copied(),
            Menu::Sensors if i < Kind::ALL.len() => Some(Row::Sensor(i)),
            Menu::Sensors => menu::BUILT_IN.get(i - Kind::ALL.len()).map(|&r| Row::Read(r)),
            Menu::Gps => menu::GPS.get(i).filter(|_| i == 0 || self.pvt.is_some()).map(|&r| Row::Read(r)),
            Menu::Page(n) => match i {
                0 => Some(Row::Layout(n)),
                _ if i <= usize::from(self.pages.get(n)?.layout) => Some(Row::Field(n, i - 1)),
                _ => None,
            },
        }
    }

    /// How many rows `menu` has.
    fn rows(&self, menu: Menu) -> usize {
        (0..64).take_while(|&i| self.row(menu, i).is_some()).count()
    }

    /// Menu row `i` of `menu`: label, value, value colour; None past the last row.
    fn menu_row(&self, menu: Menu, i: usize) -> Option<(Line<16>, Line<12>, u16)> {
        use menu::{Reading, Row};
        let (mut label, mut value, mut color) = (Line::new(), Line::new(), ui::GREY);
        match self.row(menu, i)? {
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
                let mut title = Line::<12>::new();
                title.push_str(menu_title(menu));
                if let Menu::Page(n) = menu {
                    title.push_u32(n as u32 + 1);
                }
                ui::draw_title(c, title.as_str());
                self.draw_menu_rows(c, menu, sel);
            }
            Screen::Nearby => {
                if self.ble.is_some() {
                    ui::draw_nearby(c, self.nearby.count(), self.scanning);
                }
                ui::draw_list(c, &self.nearby);
            }
        }
    }

    /// The boxes of the page on screen and what each holds (none on a menu or a list). The clock and Ferris place
    /// themselves.
    fn widgets(&self) -> impl Iterator<Item = ((u32, u32, u32, u32), Widget)> + use<> {
        let mut all = [((0, 0, 0, 0), Widget::Ferris); BOXES];
        let n = match self.screen {
            Screen::Main => {
                all[0].1 = Widget::Clock;
                2
            }
            Screen::Ride(n) => {
                let p = self.pages.get(n).copied().unwrap_or(DEFAULT_PAGES[0]);
                let mut k = 0;
                for (slot, (i, &f)) in all.iter_mut().zip(p.fields.iter().enumerate()) {
                    let Some(bx) = ui::field_box(p.layout, ui::RIDE_H, i) else { break };
                    *slot = (bx, Widget::Field(f));
                    k += 1;
                }
                k
            }
            _ => 0,
        };
        all.into_iter().take(n)
    }

    /// What box `bx` holding `w` would show now.
    fn now_drawn(&self, bx: (u32, u32, u32, u32), w: Widget) -> Drawn {
        match w {
            Widget::Clock => Drawn::Digits(self.main_digits()),
            Widget::Ferris => Drawn::Fixed,
            Widget::Field(Field::Map) => Drawn::Map(map::Shown::of(&self.track, map::area(bx))),
            Widget::Field(Field::Graph) => Drawn::Graph(self.graph_now()),
            Widget::Field(f) => Drawn::Value(self.field_text(f), self.field_color(f)),
        }
    }

    /// Box `i` (`bx`, holding `w`) whole, as drawn ([`App::drawn`]): after the page was cleared, or a screenshot row.
    fn draw_widget(&self, c: &mut impl Canvas, i: usize, bx: (u32, u32, u32, u32), w: Widget) {
        let d = self.drawn.get(i).copied().unwrap_or_default();
        match (w, d) {
            (Widget::Clock, Drawn::Digits(digits)) => ui::draw_main_time(c, digits),
            (Widget::Ferris, _) => ui::draw_ferris(c),
            (Widget::Field(f), d) => {
                ui::draw_field_frame(c, bx, f);
                match d {
                    Drawn::Map(m) => {
                        let base = (self.first_map() == Some(i)).then_some(&self.basemap);
                        map::draw(c, map::area(bx), &self.track, m.view, base);
                    }
                    Drawn::Graph(g) => {
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

    /// The main screen's digits: H H M M S S.
    fn main_digits(&self) -> [u8; 6] {
        let ((h, m), s) = (self.strip.time, self.seconds);
        [h / 10, h % 10, m / 10, m % 10, s / 10, s % 10]
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
                self.draw_menu_row(&mut Panel(b), menu, sel, i);
                self.menu_shown[i] = print;
            }
        }
    }

    /// Clears the page area and draws the current screen.
    fn show(&mut self, b: &mut impl Board) {
        for (i, (bx, w)) in self.widgets().enumerate() {
            let now = self.now_drawn(bx, w);
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
        if let Some(i) = self.first_map()
            && let (Some((bx, _)), Some(Drawn::Map(m))) = (self.widgets().nth(i), self.drawn.get(i))
        {
            let view = m.view;
            self.render_basemap(b, map::area(bx), view);
        }
        let mut p = Panel(b);
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
                    map::update(&mut Panel(b), area, &self.track, &mut m, base);
                    Drawn::Map(m)
                }
                (_, Drawn::Graph(g)) => self.update_graph(b, g, bx),
                (Widget::Clock, Drawn::Digits(was)) => {
                    let now = self.main_digits();
                    for (k, (d, was)) in now.into_iter().zip(was).enumerate() {
                        if d != was {
                            ui::draw_main_digit(&mut Panel(b), k, d);
                        }
                    }
                    Drawn::Digits(now)
                }
                (Widget::Field(f), Drawn::Value(was, was_fg)) => {
                    let (text, fg) = (self.field_text(f), self.field_color(f));
                    if (was, was_fg) != (text, fg) {
                        let old = if was_fg == fg { was.as_str() } else { "" }; // a new colour: all of it
                        ui::draw_field_value_changes(&mut Panel(b), bx, f, old, text.as_str(), fg);
                    }
                    Drawn::Value(text, fg)
                }
                _ => old,
            };
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
    }

    /// A graph in box `bx`, drawn as `old`, after a new bar: each column changes by the difference between its old
    /// bar and its new one (the chart scrolls left by one), all of it if the graph changed metric.
    fn update_graph(&mut self, b: &mut impl Board, old: GraphShown, bx: (u32, u32, u32, u32)) -> Drawn {
        let now = self.graph_now();
        if old == now {
            return Drawn::Graph(old);
        }
        let a = ui::graph_area(bx);
        if old.power != now.power {
            ui::draw_graph(&mut Panel(b), a, &|k| self.graph_bar(now, a.2, a.3, k));
        } else {
            for k in 0..a.2 {
                let (was, is) = (self.graph_bar(old, a.2, a.3, k), self.graph_bar(now, a.2, a.3, k));
                ui::draw_graph_column(&mut Panel(b), a, k, was, is);
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
            Some(Row::Open(m)) => self.open(Screen::Menu(m, 0)),
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
                    let i = ui::LAYOUTS.iter().position(|&l| l == page.layout).unwrap_or(0);
                    page.layout = ui::LAYOUTS[(i + 1) % ui::LAYOUTS.len()];
                    if n == 0 && page.layout == 0 {
                        page.layout = ui::LAYOUTS[1]; // the first page is never off
                    }
                    b.backup_write(PAGE_BKP[n], page.to_word());
                }
            }
            Some(Row::Field(n, f)) => {
                if let Some(page) = self.pages.get_mut(n)
                    && let Some(field) = page.fields.get_mut(f)
                {
                    *field = field.next();
                    b.backup_write(PAGE_BKP[n], page.to_word());
                }
            }
            Some(Row::Read(_)) | None => {}
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
        match (self.screen, i) {
            (_, Input::Off) => return self.power_off(b),
            (Screen::Menu(menu, sel), _) if self.editing => {
                let Some(menu::Row::Set(s)) = self.row(menu, sel) else {
                    self.editing = false;
                    return;
                };
                match i {
                    Input::Next | Input::Prev => self.settings.step(s, i == Input::Prev),
                    Input::NextHeld | Input::PrevHeld => {
                        self.repeated = true;
                        self.settings.step(s, i == Input::PrevHeld);
                    }
                    Input::Select | Input::Back => {
                        self.editing = false;
                        self.save_misc(b);
                    }
                    _ => {}
                }
                return self.update_menu_rows(b, menu, sel);
            }
            (Screen::Main, Input::Next) => self.screen = Screen::Ride(0),
            (Screen::Ride(n), Input::Next) => {
                let next = (n + 1..PAGES).find(|&i| self.pages[i].layout > 0);
                self.screen = next.map_or(Screen::Main, Screen::Ride);
            }
            (Screen::Main | Screen::Ride(_), Input::Menu) => self.open(Screen::Menu(Menu::Main, 0)),
            // the ride: Select starts, pauses, resumes; End on a paused ride ends it; Prev marks a lap
            (Screen::Ride(_), Input::Select) => {
                let ev = self.ride.start_pause();
                return self.recorder(ev);
            }
            (Screen::Ride(_), Input::End) => {
                if let Some(ev) = self.ride.end() {
                    self.recorder(ev);
                }
                return;
            }
            (Screen::Ride(_), Input::Prev) => {
                if let Some(ev) = self.ride.lap() {
                    self.recorder(ev);
                }
                return;
            }
            (Screen::Main | Screen::Ride(_), Input::Back) => {
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
            (Screen::Menu(menu, sel), Input::Select) => {
                self.activate(b, menu, sel);
                if self.screen == before {
                    return self.update_menu_rows(b, menu, sel);
                }
            }
            (Screen::Menu(..) | Screen::Nearby, Input::Back) => self.close(),
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
                    self.decoders.decode(kind, data, self.settings.wheel_mm, &mut self.live);
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
            ui::draw_nearby(&mut Panel(b), self.nearby.count(), self.scanning);
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
        let (h, m, sec) = rtc_time_from_tr(b.rtc().0);
        self.strip.time = (h, m);
        if sec != self.seconds {
            self.second();
        }
        self.seconds = sec;
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
        self.redraw_ms = self.redraw_ms.saturating_add(POLL_MS);
        match self.screen {
            Screen::Main | Screen::Ride(_) => self.update_page(b),
            Screen::Nearby if self.list_dirty && self.redraw_ms >= LIST_REDRAW_MS => {
                ui::draw_list(&mut Panel(b), &self.nearby);
                self.list_dirty = false;
                self.redraw_ms = 0;
            }
            Screen::Menu(menu @ (Menu::Main | Menu::Sensors | Menu::Gps), sel) if self.redraw_ms >= LIST_REDRAW_MS => {
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

    /// One light measurement every [`LIGHT_MS`] while the backlight follows the ambient light (and is on), or the
    /// sensors menu shows it: the sensor is started, read [`LIGHT_WAIT_MS`] later, and put back in standby.
    fn poll_light(&mut self, b: &mut impl Board) {
        if !self.chips.light {
            return;
        }
        self.light_ms = self.light_ms.saturating_add(POLL_MS);
        let wanted = (self.settings.light_level == LIGHT_AUTO && self.light_shown != 0)
            || matches!(self.screen, Screen::Menu(Menu::Sensors, _));
        if self.measuring && self.light_ms >= LIGHT_WAIT_MS {
            match ambient::light_read(b) {
                Some(lux10) => self.auto_level = self.auto.update(lux10),
                None if self.light_ms < LIGHT_MS => return, // not ready yet: again at the next pass
                None => {}
            }
            ambient::light_stop(b);
            self.measuring = false;
        } else if !self.measuring && wanted && self.light_ms >= LIGHT_MS {
            self.measuring = ambient::light_start(b);
            self.light_ms = 0;
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
        if let Some(ev) = self.ride.second(&input) {
            self.recorder(ev);
        }
        self.history.second(input.hr, input.power);
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
            ui::draw_strip_changes(&mut Panel(b), &self.strip, Some(&self.strip_shown));
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

/// A menu's title (a page's is followed by its number).
fn menu_title(m: Menu) -> &'static str {
    match m {
        Menu::Main => "MENU",
        Menu::Sensors => "SENSORS",
        Menu::Page(_) => "PAGE ",
        Menu::Gps => "GPS",
    }
}

/// A page's field count, "OFF" for none.
fn layout_text(l: &mut Line<12>, layout: u8) {
    match layout {
        0 => l.push_str("OFF"),
        n => l.push_u32(u32::from(n)),
    }
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
