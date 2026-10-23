//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod battery;
pub mod dev;
pub mod fonts;
pub mod gnss;
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
    /// Backlight on or off.
    fn backlight(&mut self, on: bool);
    /// The keys held down, as a mask of [`Key`] codes.
    fn keys_down(&mut self) -> u8;
    /// RTC_TR, read once the shadow registers are in sync (RSF): the time of day as BCD.
    fn rtc_tr(&mut self) -> u32;
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
/// The backup register of the wheel size.
const WHEEL_BKP: usize = 11;
/// Wheel circumferences offered (mm): 700 x 23, 25, 28, 32, 35, 38 C and 650B x 47 (ANT+ / tyre makers' tables).
pub const WHEELS: [u16; 7] = [2096, 2105, 2136, 2155, 2168, 2180, 2079];

/// The ride pages before any are set: speed big with heart rate and cadence (the time is in the strip), then four
/// fields.
const DEFAULT_PAGES: [PageCfg; PAGES] = {
    use Field::*;
    [
        PageCfg { layout: 3, fields: [Speed, Hr, Cadence, Time, Time, Time] },
        PageCfg { layout: 4, fields: [Speed, Hr, Cadence, Power, Time, Time] },
        PageCfg { layout: 0, fields: [Time; 6] },
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

/// When the GNSS receiver gets its configuration after power-up (it boots in well under a second; stock waits 3 s
/// before reading it at all).
pub const GNSS_CONFIG_MS: u32 = 1000;

/// A fix older than this is dropped (the receiver stopped answering).
pub const PVT_STALE_MS: u32 = 3000;

/// GNSS bytes read per pass: 64 per 50 ms is 1280 bytes/s, ten times NAV-PVT at 1 Hz.
const GNSS_READ: usize = 64;

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

/// Main menu rows.
const MAIN_ROWS: usize = 7;

/// The app: what is on screen, the keys, the sensors and what the Nordic chip reported.
pub struct App {
    keys: Keys,
    backlight: bool,
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
    /// Since the receiver was powered (its configuration is sent once, after [`GNSS_CONFIG_MS`]).
    gnss_ms: u32,
    wheel_mm: u16,
    pages: [PageCfg; PAGES],
    /// The ride field values on screen, to redraw only what changed.
    shown: [Line<8>; 6],
}

impl App {
    /// Panel up, backlight on, the Nordic chip started (BLE, then ANT with `ant_key` and the paired sensors), the
    /// settings read, the first ride page drawn.
    pub fn start(b: &mut impl Board, ant_key: Option<[u8; 8]>) -> Self {
        panel::init(b);
        b.backlight(true);
        b.gnss_power(true);
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
        let mut app = App {
            keys: Keys::default(),
            backlight: true,
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
            ask: 0,
            live: Live::default(),
            decoders: Decoders::default(),
            battery: battery::Battery::default(),
            battery_ms: BATTERY_MS, // read at the first step
            gnss: gnss::Parser::new(),
            pvt: None,
            pvt_ms: 0,
            gnss_ms: 0,
            wheel_mm: if wheel >> 24 == 0xa5 { wheel as u16 } else { WHEELS[1] },
            pages: core::array::from_fn(|i| PageCfg::from_word(b.backup_read(PAGE_BKP[i])).unwrap_or(DEFAULT_PAGES[i])),
            shown: Default::default(),
        };
        app.strip_shown = app.strip;
        ui::draw_strip(&mut Panel(b), &app.strip);
        app.show(b);
        app
    }

    /// What is on screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// The ride page settings.
    pub fn pages(&self) -> &[PageCfg; PAGES] {
        &self.pages
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
                5 => label.push_str("GPS"),
                6 => label.push_str("NEARBY BLE"),
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
                let (hm, s) = self.main_time();
                ui::draw_main_time(c, hm.as_str(), s.as_str());
                ui::draw_ferris(c);
            }
            Screen::Ride(n) => {
                for (bx, f) in self.boxes(n) {
                    ui::draw_field_frame(c, bx, f);
                    ui::draw_field_value(c, bx, f, self.field_text(f).as_str());
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

    /// The main screen's "HH:MM" and seconds.
    fn main_time(&self) -> (Line<8>, Line<8>) {
        let (hm, mut s) = (self.field_text(Field::Time), Line::new());
        for d in [self.seconds / 10, self.seconds % 10] {
            s.push_str(ui::DIGITS[usize::from(d % 10)]);
        }
        (hm, s)
    }

    fn draw_menu_rows(&self, c: &mut impl Canvas, menu: Menu, sel: usize) {
        for i in 0..ui::MENU_ROWS {
            match self.menu_row(menu, i) {
                Some((label, value, color)) => {
                    ui::draw_menu_row(c, i, label.as_str(), value.as_str(), color, i == sel);
                }
                None => c.fill(0, ui::LIST_Y + i as u32 * ui::LIST_ROW, ui::W, ui::LIST_ROW, ui::BLACK),
            }
        }
    }

    /// Clears the page area and draws the current screen.
    fn show(&mut self, b: &mut impl Board) {
        let mut p = Panel(b);
        ui::clear_page(&mut p);
        self.draw_screen(&mut p);
        match self.screen {
            Screen::Ride(n) => self.shown = self.pages[n].fields.map(|f| self.field_text(f)),
            Screen::Main => (self.shown[0], self.shown[1]) = self.main_time(),
            _ => {}
        }
    }

    /// Redraws the ride field values that changed.
    fn update_ride(&mut self, b: &mut impl Board, n: usize) {
        for (i, (bx, f)) in self.boxes(n).enumerate() {
            let text = self.field_text(f);
            if self.shown[i].as_str() != text.as_str() {
                ui::draw_field_value(&mut Panel(b), bx, f, text.as_str());
                self.shown[i] = text;
            }
        }
    }

    /// Opens or closes sensor `kind`'s channel to match `want`, and keeps the pairing.
    fn set_sensor(&mut self, b: &mut impl Board, kind: Kind, want: Sensor) {
        let i = kind as usize;
        if self.sensors[i] != Sensor::Off && nrf::ant_close(b, &mut self.rx, kind.channel()).is_err() {
            return; // still open: leave it as it was
        }
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
                b.backup_write(WHEEL_BKP, 0xa5 << 24 | u32::from(self.wheel_mm));
                Screen::Menu(menu, sel)
            }
            (Menu::Main, 5) => Screen::Menu(Menu::Gps, 0),
            (Menu::Gps, _) => Screen::Menu(menu, sel),
            (Menu::Main, _) => {
                if self.ble.is_some() && !self.scanning {
                    self.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
                }
                Screen::Nearby
            }
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
                b.backlight(self.backlight);
            }
            (Screen::Menu(menu, sel), Press::Short(k @ (Key::Page | Key::Lap))) => {
                let rows = (0..ui::MENU_ROWS).take_while(|&i| self.menu_row(menu, i).is_some()).count().max(1);
                let sel = if k == Key::Page { (sel + 1) % rows } else { (sel + rows - 1) % rows };
                self.screen = Screen::Menu(menu, sel);
                return self.draw_menu_rows(&mut Panel(b), menu, sel);
            }
            (Screen::Menu(menu, sel), Press::Short(Key::Start)) => {
                self.activate(b, menu, sel);
                if self.screen == before {
                    return self.draw_menu_rows(&mut Panel(b), menu, sel);
                }
            }
            (Screen::Menu(Menu::Main, _), Press::Short(Key::Back)) => self.screen = Screen::Main,
            (Screen::Menu(Menu::Sensors, _), Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, 0),
            (Screen::Menu(Menu::Gps, _), Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, 5),
            (Screen::Menu(Menu::Page(n), _), Press::Short(Key::Back)) => {
                self.screen = Screen::Menu(Menu::Main, n + 1);
            }
            (Screen::Nearby, Press::Short(Key::Back)) => self.screen = Screen::Menu(Menu::Main, MAIN_ROWS - 1),
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
        let mut buf = [0xff; 32];
        let mut send = self.gnss_ms < GNSS_CONFIG_MS && self.gnss_ms + POLL_MS >= GNSS_CONFIG_MS;
        self.gnss_ms = self.gnss_ms.saturating_add(POLL_MS);
        self.pvt_ms = self.pvt_ms.saturating_add(POLL_MS);
        for _ in 0..GNSS_READ / buf.len() {
            if core::mem::take(&mut send) {
                buf = gnss::config();
            } else {
                buf = [0xff; 32];
            }
            b.gnss_transfer(&mut buf);
            for &byte in &buf {
                if let Some(pvt) = self.gnss.push(byte).and_then(|(c, i, p)| gnss::pvt(c, i, p)) {
                    self.pvt = Some(pvt);
                    self.pvt_ms = 0;
                    let sensor =
                        [Kind::Speed, Kind::SpeedCadence].iter().any(|&k| self.sensors[k as usize] != Sensor::Off);
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
        self.strip.gps = self.pvt.map(|p| p.good());
        // blinks while searching; steady otherwise, so the strip is not redrawn every second for nothing
        self.strip.blink = self.strip.gps != Some(false) || self.seconds.is_multiple_of(2);
    }

    /// One pass of the main loop: read the time, the radio and the keys, redraw what changed. The caller sleeps
    /// [`POLL_MS`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        let (h, m, sec) = rtc_time_from_tr(b.rtc_tr());
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
        }
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
        self.redraw_ms = self.redraw_ms.saturating_add(POLL_MS);
        match self.screen {
            Screen::Main => {
                let now = self.main_time();
                if (now.0.as_str(), now.1.as_str()) != (self.shown[0].as_str(), self.shown[1].as_str()) {
                    ui::draw_main_time(&mut Panel(b), now.0.as_str(), now.1.as_str());
                    (self.shown[0], self.shown[1]) = now;
                }
            }
            Screen::Ride(n) => self.update_ride(b, n),
            Screen::Nearby if self.list_dirty && self.redraw_ms >= LIST_REDRAW_MS => {
                ui::draw_list(&mut Panel(b), &self.nearby);
                self.list_dirty = false;
                self.redraw_ms = 0;
            }
            Screen::Menu(menu @ (Menu::Sensors | Menu::Gps), sel) if self.redraw_ms >= LIST_REDRAW_MS => {
                self.draw_menu_rows(&mut Panel(b), menu, sel);
                self.redraw_ms = 0;
            }
            _ => {}
        }
        let mask = b.keys_down();
        if let Some(p) = self.keys.poll(mask, POLL_MS) {
            self.press(b, p);
        }
    }

    /// As stock: the radio chip off too (or it drains the battery), screen off, wait for the key to be released
    /// (or it would wake the unit at once), then standby.
    fn power_off(&mut self, b: &mut impl Board) {
        if self.ble.is_some() {
            nrf::system_off(b);
        }
        b.gnss_power(false);
        b.backlight(false);
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
            self.strip_shown = self.strip;
            ui::draw_strip(&mut Panel(b), &self.strip);
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
    let (h, m, _) = rtc_time_from_tr(b.rtc_tr());
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
