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
pub mod fit;
pub mod fonts;
pub mod gnss;
pub mod map;
pub mod nrf;
pub mod panel;
pub mod sensors;
#[cfg(test)]
mod sim;
pub mod text;
pub mod ui;

use nrf::SensorId;
use panel::Panel;
use sensors::{Decoders, Kind, Live, Metric};
use text::Line;
use ui::{Canvas, Field, Row, Strip, UsbDot};

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
    /// Reads `buf.len()` bytes from register `reg` on, of the chip at 7-bit address `addr` on the sensor bus (I2C4,
    /// BOARD.md 14); false if it did not answer (no acknowledge, or a time-out: it never hangs).
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
}

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
/// The backup register of the wheel size (bits 0..15) and the backlight settings (16..19 level or [`LIGHT_AUTO`], 20..23 time-out:
/// indexes into [`LIGHT_LEVELS`] and [`LIGHT_TIMEOUTS_S`]), behind the tag 0xa5.
const WHEEL_BKP: usize = 11;
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
const GPS_ROW: usize = 7;
/// The nearby BLE devices row of the main menu.
const NEARBY_ROW: usize = 8;

/// The app: what is on screen, the keys, the sensors and what the Nordic chip reported.
pub struct App {
    keys: Keys,
    /// The backlight is wanted on (Back toggles it); it is on unless timed out.
    backlight: bool,
    /// The level the backlight is at (0: off).
    light_shown: u8,
    /// Indexes into [`LIGHT_LEVELS`] and [`LIGHT_TIMEOUTS_S`].
    light_level: usize,
    light_timeout: usize,
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
    wheel_mm: u16,
    pages: [PageCfg; PAGES],
    /// The ride field values on screen, to redraw only what changed.
    shown: [Line<8>; 6],
    /// The main screen's digits on screen.
    main_shown: [u8; 6],
    /// The menu rows on screen, as fingerprints ([`App::menu_row_print`]).
    menu_shown: [u32; ui::MENU_ROWS],
    /// The ride's track, from the good fixes.
    track: map::Track,
    /// The maps on screen, by ride field.
    map_shown: [map::Shown; 6],
}

impl App {
    /// Panel up, backlight on, the Nordic chip started (BLE, then ANT with `ant_key` and the paired sensors), the
    /// settings read, the first ride page drawn.
    pub fn start(b: &mut impl Board, ant_key: Option<[u8; 8]>) -> Self {
        panel::init(b);
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
        let wheel = b.backup_read(WHEEL_BKP);
        let wheel = if wheel >> 24 == 0xa5 { wheel } else { 0 };
        let mut app = App {
            keys: Keys::default(),
            backlight: true,
            light_shown: 0,
            light_level: (wheel >> 16 & 0xf) as usize % (LIGHT_LEVELS.len() + 1),
            light_timeout: (wheel >> 20 & 0xf) as usize % LIGHT_TIMEOUTS_S.len(),
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
            },
            strip_shown: Strip {
                time: (0, 0),
                radio: false,
                gps: None,
                blink: true,
                usb: UsbDot::Off,
                battery: None,
                charging: false,
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
            wheel_mm: if wheel != 0 { wheel as u16 } else { WHEELS[1] },
            pages: core::array::from_fn(|i| PageCfg::from_word(b.backup_read(PAGE_BKP[i])).unwrap_or(DEFAULT_PAGES[i])),
            shown: Default::default(),
            main_shown: [0; 6],
            menu_shown: [0; ui::MENU_ROWS],
            track: map::Track::new(),
            map_shown: Default::default(),
        };
        if app.chips.baro {
            ambient::baro_start(b); // read at the first step
        }
        app.strip_shown = app.strip;
        ui::draw_strip(&mut Panel(b), &app.strip);
        app.show(b);
        app.update_light(b);
        app
    }

    /// Sets the backlight to what it should be now: the chosen level, unless switched off (Back) or timed out.
    fn update_light(&mut self, b: &mut impl Board) {
        let timeout_ms = LIGHT_TIMEOUTS_S[self.light_timeout] * 1000;
        let on = self.backlight && (timeout_ms == 0 || self.idle_ms < timeout_ms);
        let level = match LIGHT_LEVELS.get(self.light_level) {
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
        let w = (0xa5 << 24) | ((self.light_timeout as u32) << 20) | ((self.light_level as u32) << 16);
        b.backup_write(WHEEL_BKP, w | u32::from(self.wheel_mm));
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
        let metric = match f {
            Field::Time => {
                let (h, m) = self.strip.time;
                for d in [h / 10, h % 10, 10, m / 10, m % 10] {
                    l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
                }
                return l;
            }
            Field::Hr => Metric::Hr,
            Field::Speed => Metric::Speed,
            Field::Cadence => Metric::Cadence,
            Field::Power => Metric::Power,
            Field::Map => return l,
        };
        match (self.live.get(metric), metric) {
            (None, _) => l.push_str("--"),
            (Some(v), Metric::Speed) => {
                l.push_u32(u32::from(v / 10));
                l.push_str(".");
                l.push_u32(u32::from(v % 10));
            }
            (Some(v), _) => l.push_u32(u32::from(v)),
        }
        l
    }

    /// Menu row `i` of `menu`: label, value, value colour; None past the last row.
    fn menu_row(&self, menu: Menu, i: usize) -> Option<(Line<16>, Line<12>, u16)> {
        let (mut label, mut value, mut color) = (Line::new(), Line::new(), ui::GREY);
        match menu {
            Menu::Main => match i {
                0 => label.push_str("SENSORS"),
                1..=3 => {
                    label.push_str("PAGE ");
                    label.push_u32(i as u32);
                    layout_text(&mut value, self.pages.get(i - 1)?.layout);
                }
                4 => {
                    label.push_str("WHEEL MM");
                    value.push_u32(u32::from(self.wheel_mm));
                }
                5 => {
                    label.push_str("LIGHT %");
                    match LIGHT_LEVELS.get(self.light_level) {
                        Some(&l) => value.push_u32(u32::from(l)),
                        None => value.push_str("AUTO"),
                    }
                }
                6 => {
                    label.push_str("LIGHT OFF");
                    match LIGHT_TIMEOUTS_S[self.light_timeout] {
                        0 => value.push_str("NEVER"),
                        t => {
                            value.push_u32(t);
                            value.push_str(" S");
                        }
                    }
                }
                7 => label.push_str("GPS"),
                8 => label.push_str("NEARBY BLE"),
                9 => {
                    label.push_str("LUX");
                    match self.auto.lux10() {
                        _ if !self.chips.light => value.push_str("NONE"),
                        None => value.push_str("--"),
                        Some(l) => value.push_u32(l / 10),
                    }
                }
                10 | 11 => {
                    label.push_str(if i == 10 { "TEMP C" } else { "HPA" });
                    match self.baro {
                        _ if !self.chips.baro => value.push_str("NONE"),
                        None => value.push_str("--"),
                        Some((_, t)) if i == 10 => push_fixed(&mut value, i32::from(t), 2, 1),
                        Some((p, _)) => push_fixed(&mut value, p as i32, 1, 0),
                    }
                }
                _ => return None,
            },
            Menu::Sensors => {
                let kind = *Kind::ALL.get(i)?;
                label.push_str(kind.name());
                match (self.ant, self.sensors[i]) {
                    (false, _) => value.push_str(if self.ble.is_some() { "NO KEY" } else { "NO BLE" }),
                    (true, Sensor::Off) => value.push_str("OFF"),
                    (true, Sensor::Search) => value.push_str("SEARCH"),
                    (true, Sensor::Paired(id)) => value.push_u32(u32::from(id.device)),
                }
                if self.heard_ms[i] < sensors::STALE_MS {
                    color = ui::GREEN;
                }
            }
            Menu::Gps => {
                let p = self.pvt;
                let two = |l: &mut Line<12>, v: u8| {
                    for d in [v / 10, v % 10] {
                        l.push_str(ui::DIGITS[usize::from(d)]);
                    }
                };
                match (i, p) {
                    (0, None) => (label.push_str("FIX"), value.push_str("NO GPS")).1,
                    (0, Some(p)) => {
                        label.push_str("FIX");
                        value.push_str(match p.fix {
                            _ if !p.fix_ok => "NONE",
                            2 => "2D",
                            3 => "3D",
                            _ => "OTHER",
                        });
                    }
                    (_, None) => return None,
                    (1, Some(p)) => (label.push_str("SATS"), value.push_u32(u32::from(p.sats))).1,
                    (2, Some(p)) => (label.push_str("LAT"), push_fixed(&mut value, p.lat, 7, 5)).1,
                    (3, Some(p)) => (label.push_str("LON"), push_fixed(&mut value, p.lon, 7, 5)).1,
                    (4, Some(p)) => (label.push_str("ALT M"), push_fixed(&mut value, p.alt_mm, 3, 0)).1,
                    (5 | 6, Some(p)) if !p.time_ok => {
                        label.push_str(if i == 5 { "UTC" } else { "DATE" });
                        value.push_str("--"); // the receiver's placeholder until it knows the time
                    }
                    (5, Some(p)) => {
                        label.push_str("UTC");
                        let (_, _, _, h, m, sec) = p.utc;
                        for (k, v) in [h, m, sec].into_iter().enumerate() {
                            if k > 0 {
                                value.push_str(":");
                            }
                            two(&mut value, v % 100);
                        }
                    }
                    (6, Some(p)) => {
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
            Menu::Page(n) => {
                let page = self.pages.get(n)?;
                if i == 0 {
                    label.push_str("FIELDS");
                    layout_text(&mut value, page.layout);
                } else if i <= usize::from(page.layout) {
                    label.push_str("FIELD ");
                    label.push_u32(i as u32);
                    value.push_str(page.fields.get(i - 1)?.label());
                } else {
                    return None;
                }
            }
        }
        Some((label, value, color))
    }

    /// Draws the whole screen below the bar from the state (the panel was cleared, or a screenshot row).
    fn draw_screen(&self, c: &mut impl Canvas) {
        match self.screen {
            Screen::Main => {
                ui::draw_main_time(c, self.main_digits());
                ui::draw_ferris(c);
            }
            Screen::Ride(n) => {
                for (i, (bx, f)) in self.boxes(n).enumerate() {
                    ui::draw_field_frame(c, bx, f);
                    match (f, self.map_shown.get(i)) {
                        (Field::Map, Some(m)) => map::draw(c, map::area(bx), &self.track, m.view),
                        _ => ui::draw_field_value(c, bx, f, self.field_text(f).as_str()),
                    }
                }
            }
            Screen::Menu(menu, sel) => {
                let mut title = Line::<12>::new();
                match menu {
                    Menu::Main => title.push_str("MENU"),
                    Menu::Gps => title.push_str("GPS"),
                    Menu::Sensors => title.push_str("SENSORS"),
                    Menu::Page(n) => {
                        title.push_str("PAGE ");
                        title.push_u32(n as u32 + 1);
                    }
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

    /// Ride page `n`'s field boxes and fields.
    fn boxes(&self, n: usize) -> impl Iterator<Item = ((u32, u32, u32, u32), Field)> + use<> {
        let page = self.pages[n];
        (0..page.fields.len()).map_while(move |i| Some((ui::field_box(page.layout, ui::RIDE_H, i)?, page.fields[i])))
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

    fn draw_menu_row(&self, c: &mut impl Canvas, menu: Menu, sel: usize, i: usize) {
        match self.menu_row(menu, i) {
            Some((label, value, color)) => ui::draw_menu_row(c, i, label.as_str(), value.as_str(), color, i == sel),
            None => c.fill(0, ui::LIST_Y + i as u32 * ui::LIST_ROW, ui::W, ui::LIST_ROW, ui::BLACK),
        }
    }

    /// A fingerprint of menu row `i` as it would be drawn (FNV-1a over its text, colour and selection).
    fn menu_row_print(&self, menu: Menu, sel: usize, i: usize) -> u32 {
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
        if let Screen::Ride(n) = self.screen {
            for ((bx, f), m) in self.boxes(n).zip(&mut self.map_shown) {
                if f == Field::Map {
                    *m = map::Shown::of(&self.track, map::area(bx));
                }
            }
        }
        let mut p = Panel(b);
        ui::clear_page(&mut p);
        self.draw_screen(&mut p);
        match self.screen {
            Screen::Ride(n) => self.shown = self.pages[n].fields.map(|f| self.field_text(f)),
            Screen::Main => self.main_shown = self.main_digits(),
            Screen::Menu(menu, sel) => self.menu_shown = core::array::from_fn(|i| self.menu_row_print(menu, sel, i)),
            Screen::Nearby => {}
        }
    }

    /// Redraws the ride field values that changed, and what changed on a map.
    fn update_ride(&mut self, b: &mut impl Board, n: usize) {
        for (i, (bx, f)) in self.boxes(n).enumerate() {
            if f == Field::Map {
                if let Some(m) = self.map_shown.get_mut(i) {
                    map::update(&mut Panel(b), map::area(bx), &self.track, m);
                }
                continue;
            }
            let text = self.field_text(f);
            if self.shown[i].as_str() != text.as_str() {
                ui::draw_field_value_changes(&mut Panel(b), bx, f, self.shown[i].as_str(), text.as_str());
                self.shown[i] = text;
            }
        }
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

    /// Start on menu row `sel`.
    fn activate(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        self.screen = match (menu, sel) {
            (Menu::Main, 0) => Screen::Menu(Menu::Sensors, 0),
            (Menu::Main, 1..=3) => Screen::Menu(Menu::Page(sel - 1), 0),
            (Menu::Main, 4) => {
                let i = WHEELS.iter().position(|&w| w == self.wheel_mm).map_or(0, |i| (i + 1) % WHEELS.len());
                self.wheel_mm = WHEELS[i];
                self.save_misc(b);
                Screen::Menu(menu, sel)
            }
            (Menu::Main, 5 | 6) => {
                if sel == 5 {
                    self.light_level = (self.light_level + 1) % (LIGHT_LEVELS.len() + 1);
                } else {
                    self.light_timeout = (self.light_timeout + 1) % LIGHT_TIMEOUTS_S.len();
                }
                self.save_misc(b);
                self.update_light(b);
                Screen::Menu(menu, sel)
            }
            (Menu::Main, GPS_ROW) => Screen::Menu(Menu::Gps, 0),
            (Menu::Gps, _) => Screen::Menu(menu, sel),
            (Menu::Main, NEARBY_ROW) => {
                if self.ble.is_some() && !self.scanning {
                    self.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
                }
                Screen::Nearby
            }
            (Menu::Main, _) => Screen::Menu(menu, sel), // readings only
            (Menu::Sensors, _) => {
                if let (true, Some(&kind)) = (self.ant, Kind::ALL.get(sel)) {
                    let want = if self.sensors[sel] == Sensor::Off { Sensor::Search } else { Sensor::Off };
                    self.set_sensor(b, kind, want);
                }
                Screen::Menu(menu, sel)
            }
            (Menu::Page(n), _) => {
                if let Some(page) = self.pages.get_mut(n) {
                    if sel == 0 {
                        let i = ui::LAYOUTS.iter().position(|&l| l == page.layout).unwrap_or(0);
                        page.layout = ui::LAYOUTS[(i + 1) % ui::LAYOUTS.len()];
                        if n == 0 && page.layout == 0 {
                            page.layout = ui::LAYOUTS[1]; // the first page is never off
                        }
                    } else if let Some(f) = page.fields.get_mut(sel - 1) {
                        *f = f.next();
                    }
                    b.backup_write(PAGE_BKP[n], page.to_word());
                }
                Screen::Menu(menu, sel)
            }
        };
    }

    /// Acts on a key press: Page goes from the main screen through the ride pages and back, holding it opens the
    /// menu, Back toggles the backlight; in the menus Page / Lap move down / up, Start selects or changes, Back goes out. Holding Back
    /// switches off anywhere.
    pub fn press(&mut self, b: &mut impl Board, p: Press) {
        let before = self.screen;
        let woke = self.light_shown == 0 && self.backlight;
        self.idle_ms = 0;
        if woke {
            self.update_light(b); // a key press after the time-out: the light comes back (and the key acts)
            if p == Press::Short(Key::Back) {
                return; // Back's own job is the light: done
            }
        }
        match (self.screen, p) {
            (_, Press::Long(Key::Back)) => return self.power_off(b),
            (Screen::Main, Press::Short(Key::Page)) => self.screen = Screen::Ride(0),
            (Screen::Ride(n), Press::Short(Key::Page)) => {
                let next = (n + 1..PAGES).find(|&i| self.pages[i].layout > 0);
                self.screen = next.map_or(Screen::Main, Screen::Ride);
            }
            (Screen::Main | Screen::Ride(_), Press::Long(Key::Page)) => self.screen = Screen::Menu(Menu::Main, 0),
            (Screen::Main | Screen::Ride(_), Press::Short(Key::Back)) => {
                self.backlight = !self.backlight;
                self.update_light(b);
            }
            (Screen::Menu(menu, sel), Press::Short(k @ (Key::Page | Key::Lap))) => {
                let rows = (0..ui::MENU_ROWS).take_while(|&i| self.menu_row(menu, i).is_some()).count().max(1);
                let sel = if k == Key::Page { (sel + 1) % rows } else { (sel + rows - 1) % rows };
                self.screen = Screen::Menu(menu, sel);
                return self.update_menu_rows(b, menu, sel);
            }
            (Screen::Menu(menu, sel), Press::Short(Key::Start)) => {
                self.activate(b, menu, sel);
                if self.screen == before {
                    return self.update_menu_rows(b, menu, sel);
                }
            }
            (Screen::Menu(Menu::Main, _), Press::Short(Key::Back)) => self.screen = Screen::Main,
            (Screen::Menu(Menu::Sensors, _), Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, 0),
            (Screen::Menu(Menu::Gps, _), Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, GPS_ROW),
            (Screen::Menu(Menu::Page(n), _), Press::Short(Key::Back)) => {
                self.screen = Screen::Menu(Menu::Main, n + 1);
            }
            (Screen::Nearby, Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, NEARBY_ROW),
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
                    self.decoders.decode(kind, data, self.wheel_mm, &mut self.live);
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
        self.update_strip(b);
        if self.ble.is_some() {
            self.poll_nrf(b);
        }
        self.poll_gnss(b);
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
            Screen::Main => {
                let now = self.main_digits();
                for (i, (&d, shown)) in now.iter().zip(&mut self.main_shown).enumerate() {
                    if d != *shown {
                        ui::draw_main_digit(&mut Panel(b), i, d);
                        *shown = d;
                    }
                }
            }
            Screen::Ride(n) => self.update_ride(b, n),
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
        self.update_light(b);
    }

    /// One light measurement every [`LIGHT_MS`] while the backlight follows the ambient light (and is on), or the
    /// main menu shows it: the sensor is started, read [`LIGHT_WAIT_MS`] later, and put back in standby.
    fn poll_light(&mut self, b: &mut impl Board) {
        if !self.chips.light {
            return;
        }
        self.light_ms = self.light_ms.saturating_add(POLL_MS);
        let wanted = (self.light_level == LIGHT_AUTO && self.light_shown != 0)
            || matches!(self.screen, Screen::Menu(Menu::Main, _));
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

    /// As stock: the radio chip off too (or it drains the battery), screen off, wait for the key to be released
    /// (or it would wake the unit at once), then standby.
    fn power_off(&mut self, b: &mut impl Board) {
        if self.ble.is_some() {
            nrf::system_off(b);
        }
        b.gnss_power(false);
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
    fn short_on_release_long_once_while_held() {
        let mut k = Keys::default();
        assert_eq!(k.poll(Key::Page as u8, 50), None);
        assert_eq!(k.poll(0, 50), Some(Press::Short(Key::Page)));
        assert_eq!(k.poll(0, 50), None);
        for _ in 0..39 {
            assert_eq!(k.poll(Key::Page as u8, 50), None);
        }
        assert_eq!(k.poll(Key::Page as u8, 50), Some(Press::Long(Key::Page)), "at 2 s");
        assert_eq!(k.poll(Key::Page as u8, 50), None, "once");
        assert_eq!(k.poll(0, 50), None, "nothing at the release after a long press");
        for _ in 0..59 {
            assert_eq!(k.poll(Key::Back as u8, 50), None);
        }
        assert_eq!(k.poll(Key::Back as u8, 50), Some(Press::Long(Key::Back)), "Back: at 3 s");
    }
}
