//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dev;
pub mod fonts;
pub mod nrf;
pub mod panel;
#[cfg(test)]
mod sim;
pub mod text;
pub mod ui;

use panel::Panel;
use ui::{
    Clock, Row, UsbDot, clear_page, draw_bar, draw_ble, draw_hr, draw_list, draw_nearby, draw_static, draw_usb_dot,
};

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
    /// Key levels, true = pressed: (page, back).
    fn keys_down(&mut self) -> (bool, bool);
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

/// What the main loop should do after a key poll.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    None,
    /// Page was pressed and released before [`Keys::PAIR_MS`]: the next page.
    NextPage,
    /// Page has been held for [`Keys::PAIR_MS`]: forget the heart-rate strap and pair again.
    Repair,
    /// Back was pressed and released before [`Keys::POWER_OFF_MS`].
    ToggleBacklight,
    /// Back has been held for [`Keys::POWER_OFF_MS`].
    PowerOff,
}

/// Key state between polls: each key acts on its release (short press), or once held long enough.
#[derive(Default)]
pub struct Keys {
    page_held_ms: u32,
    back_held_ms: u32,
}

impl Keys {
    /// How long Back must be held to switch off.
    pub const POWER_OFF_MS: u32 = 3000;
    /// How long Page must be held to pair a new strap.
    pub const PAIR_MS: u32 = 2000;

    /// One poll, `dt_ms` after the previous one, with the current key levels (true = pressed).
    pub fn poll(&mut self, page_down: bool, back_down: bool, dt_ms: u32) -> Action {
        let held = self.page_held_ms;
        let page_short = !page_down && (1..Self::PAIR_MS).contains(&held);
        let page_long = page_down && held < Self::PAIR_MS && held + dt_ms >= Self::PAIR_MS;
        self.page_held_ms = if page_down { held + dt_ms } else { 0 };
        let back_released = !back_down && self.back_held_ms > 0;
        self.back_held_ms = if back_down { self.back_held_ms + dt_ms } else { 0 };
        if self.back_held_ms >= Self::POWER_OFF_MS {
            Action::PowerOff
        } else if page_long {
            Action::Repair
        } else if page_short {
            Action::NextPage
        } else if back_released {
            Action::ToggleBacklight
        } else {
            Action::None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------------------------------------------

/// Main loop period: the time and the keys are read this often.
pub const POLL_MS: u32 = 50;

/// How long a BLE scan runs, started when the device list opens. Scanning keeps the Nordic radio on, so it is short.
pub const SCAN_S: u16 = 10;

/// The device list is redrawn at most this often while reports come in (each row costs SPI time).
pub const LIST_REDRAW_MS: u32 = 1000;

/// A heart rate older than this is shown as "--" (the strap is off or out of range).
pub const HR_STALE_MS: u32 = 5000;

/// The RTC backup register that keeps the paired strap ([`nrf::Strap::to_word`]); stock does not use it (BOARD.md 8).
pub const STRAP_BKP: usize = 5;

/// What the screen shows under the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The time, and the heart rate if an ANT+ key was built in.
    Clock,
    /// The BLE devices heard, strongest first, with their RSSI.
    Nearby,
}

/// The app: what is on screen, the key state and what the Nordic chip reported.
pub struct App {
    clock: Clock,
    keys: Keys,
    backlight: bool,
    usb: UsbDot,
    ble: Option<nrf::Version>,
    rx: nrf::Rx,
    nearby: nrf::Nearby,
    scanning: bool,
    /// The ANT+ network key, if built in.
    ant_key: Option<[u8; 8]>,
    /// The ANT+ heart-rate channel is open.
    ant: bool,
    /// The strap the channel is for; None: pairing with the first one heard.
    strap: Option<nrf::Strap>,
    /// A heart rate came in while pairing: ask the chip which strap sent it.
    ask_strap: bool,
    hr: Option<u8>,
    hr_age_ms: u32,
    page: Page,
    list_dirty: bool,
    list_ms: u32,
}

impl App {
    /// Panel up, the clock page drawn, backlight on; then the Nordic chip started, its state shown, and with an
    /// ANT+ network key (`ant_key`, see build.rs) the heart-rate channel opened.
    pub fn start(b: &mut impl Board, ant_key: Option<[u8; 8]>) -> Self {
        panel::init(b);
        draw_static(&mut Panel(b));
        b.backlight(true);
        let ble = nrf::start(b).ok();
        draw_ble(&mut Panel(b), ble);
        let mut app = App {
            clock: Clock::new(),
            keys: Keys::default(),
            backlight: true,
            usb: UsbDot::Off,
            ble,
            rx: nrf::Rx::new(),
            nearby: nrf::Nearby::new(),
            scanning: false,
            ant_key,
            ant: false,
            strap: nrf::Strap::from_word(b.backup_read(STRAP_BKP)),
            ask_strap: false,
            hr: None,
            hr_age_ms: 0,
            page: Page::Clock,
            list_dirty: false,
            list_ms: 0,
        };
        app.open_hr(b);
        app
    }

    /// Opens the heart-rate channel for the paired strap (or to pair) when BLE runs and a key is built in.
    fn open_hr(&mut self, b: &mut impl Board) {
        if let (Some(_), Some(key)) = (self.ble, self.ant_key) {
            self.ant = nrf::ant_hr_open(b, &mut self.rx, key, self.strap).is_ok();
        }
        self.draw_line(&mut Panel(b));
    }

    /// Forgets the paired strap and pairs with the next one heard. The chip is restarted: a channel's ID can only
    /// change while it is closed, and a restart is the simplest way to a known state.
    pub fn repair(&mut self, b: &mut impl Board) {
        b.backup_write(STRAP_BKP, 0);
        self.strap = None;
        self.hr = None;
        self.scanning = false;
        self.rx = nrf::Rx::new();
        self.ble = nrf::start(b).ok();
        draw_ble(&mut Panel(b), self.ble);
        self.open_hr(b);
    }

    /// The page shown.
    pub fn page(&self) -> Page {
        self.page
    }

    /// The line under the bar: the heart rate on the clock page, the device count on the list.
    fn draw_line(&self, c: &mut impl ui::Canvas) {
        match self.page {
            Page::Clock if self.ant => draw_hr(c, self.hr, self.strap.is_some()),
            Page::Nearby if self.ble.is_some() => draw_nearby(c, self.nearby.count(), self.scanning),
            _ => {}
        }
    }

    /// The page area: fixed parts, what is known so far, and the line under the bar.
    fn draw_page(&self, c: &mut impl ui::Canvas) {
        match self.page {
            Page::Clock => {
                draw_static(c);
                self.clock.redraw(c);
            }
            Page::Nearby => draw_list(c, &self.nearby),
        }
        self.draw_line(c);
    }

    /// Page: the clock and the device list in turn. Opening the list starts a [`SCAN_S`] scan.
    pub fn next_page(&mut self, b: &mut impl Board) {
        self.page = match self.page {
            Page::Clock => Page::Nearby,
            Page::Nearby => Page::Clock,
        };
        if self.page == Page::Nearby && self.ble.is_some() && !self.scanning {
            self.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
        }
        self.clock = Clock::new(); // the next step draws every digit
        let mut p = Panel(b);
        clear_page(&mut p);
        self.draw_page(&mut p);
    }

    /// Takes what the Nordic chip sent since the last pass: advertisers, heart rate, the scan's end.
    fn poll_nrf(&mut self, b: &mut impl Board) {
        let mut line = false;
        let mut buf = [0; 64];
        for _ in 0..64 {
            let n = b.nrf_recv(&mut buf);
            for &byte in buf.get(..n).unwrap_or(&[]) {
                let Some(p) = self.rx.push(byte) else { continue };
                if let Some(r) = nrf::adv_report(p) {
                    line |= self.nearby.add(&r);
                    self.list_dirty = true;
                } else if let Some(bpm) = nrf::ant_hr(p) {
                    line |= self.hr != Some(bpm);
                    self.hr = Some(bpm);
                    self.hr_age_ms = 0;
                    self.ask_strap |= self.strap.is_none();
                } else if nrf::event_id(p) == Some(nrf::EVT_TIMEOUT) {
                    self.scanning = false;
                    line = true;
                }
            }
            if n == 0 {
                break;
            }
        }
        if line {
            self.draw_line(&mut Panel(b));
        }
    }

    /// One pass of the main loop: read the time and the keys, redraw what changed. The caller sleeps
    /// [`POLL_MS`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        if self.page == Page::Clock {
            let (h, m, s) = rtc_time_from_tr(b.rtc_tr());
            self.clock.show(&mut Panel(b), h, m, s);
        }
        if self.ble.is_some() {
            self.poll_nrf(b);
        }
        if core::mem::take(&mut self.ask_strap)
            && let Ok(Some(strap)) = nrf::ant_hr_strap(b, &mut self.rx)
        {
            self.strap = Some(strap);
            b.backup_write(STRAP_BKP, strap.to_word());
            self.draw_line(&mut Panel(b));
        }
        self.hr_age_ms = self.hr_age_ms.saturating_add(POLL_MS);
        if self.hr.is_some() && self.hr_age_ms > HR_STALE_MS {
            self.hr = None;
            self.draw_line(&mut Panel(b));
        }
        self.list_ms = self.list_ms.saturating_add(POLL_MS);
        if self.page == Page::Nearby && self.list_dirty && self.list_ms >= LIST_REDRAW_MS {
            draw_list(&mut Panel(b), &self.nearby);
            self.list_dirty = false;
            self.list_ms = 0;
        }
        let (page, back) = b.keys_down();
        match self.keys.poll(page, back, POLL_MS) {
            Action::NextPage => self.next_page(b),
            Action::Repair => self.repair(b),
            Action::ToggleBacklight => {
                self.backlight = !self.backlight;
                b.backlight(self.backlight);
            }
            Action::PowerOff => {
                if self.ble.is_some() {
                    nrf::system_off(b); // as stock: the radio chip off too, or it drains the battery
                }
                power_off(b)
            }
            Action::None => {}
        }
    }

    /// Shows the USB link state (redraws the dot only when it changes).
    pub fn set_usb(&mut self, b: &mut impl Board, s: UsbDot) {
        if s != self.usb {
            self.usb = s;
            draw_usb_dot(&mut Panel(b), s);
        }
    }

    /// Row `y` of the screen as the app drew it: the screenshot source.
    pub fn render_row(&self, y: u32) -> Row {
        let mut row = Row::new(y);
        draw_bar(&mut row);
        self.draw_page(&mut row);
        draw_usb_dot(&mut row, self.usb);
        draw_ble(&mut row, self.ble);
        row
    }
}

/// As stock: screen off, wait for the key to be released (or it would wake the unit at once), then standby.
fn power_off(b: &mut impl Board) {
    b.backlight(false);
    panel::off(b);
    while b.keys_down().1 {
        b.delay_ms(POLL_MS);
    }
    b.delay_ms(50);
    b.standby();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtc_decoding() {
        assert_eq!(rtc_time_from_tr(0), (0, 0, 0));
        assert_eq!(rtc_time_from_tr(0x0012_3456), (12, 34, 56));
        assert_eq!(rtc_time_from_tr(0x0040_0000 | 0x0009_0000), (9, 0, 0)); // PM bit ignored
    }

    #[test]
    fn page_short_press_on_release_long_hold_once() {
        let mut k = Keys::default();
        assert_eq!(k.poll(true, false, 50), Action::None);
        assert_eq!(k.poll(false, false, 50), Action::NextPage);
        assert_eq!(k.poll(false, false, 50), Action::None);
        for _ in 0..39 {
            assert_eq!(k.poll(true, false, 50), Action::None);
        }
        assert_eq!(k.poll(true, false, 50), Action::Repair, "at 2 s");
        assert_eq!(k.poll(true, false, 50), Action::None, "once");
        assert_eq!(k.poll(false, false, 50), Action::None, "no page change after a long hold");
    }

    #[test]
    fn back_short_press_toggles_the_backlight_on_release() {
        let mut k = Keys::default();
        assert_eq!(k.poll(false, true, 50), Action::None);
        assert_eq!(k.poll(false, false, 50), Action::ToggleBacklight);
        assert_eq!(k.poll(false, false, 50), Action::None);
    }

    #[test]
    fn back_held_3s_powers_off() {
        let mut k = Keys::default();
        for _ in 0..59 {
            assert_eq!(k.poll(false, true, 50), Action::None);
        }
        assert_eq!(k.poll(false, false, 50), Action::ToggleBacklight); // released at 2.95 s: a short press
        for _ in 0..59 {
            assert_eq!(k.poll(false, true, 50), Action::None);
        }
        assert_eq!(k.poll(false, true, 50), Action::PowerOff);
    }
}
