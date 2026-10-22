//! Everything the firmware does, written against [`Board`]: the few operations that touch hardware.
//!
//! `main.rs` implements [`Board`] with registers on the unit; `sim.rs` implements it on the host, with a model
//! of the panel, so the whole app (power-up sequence, drawing protocol, clock, keys, power off) runs in
//! `cargo test-host`. Rule: logic goes here, `main.rs` only moves bytes and reads pins (AGENTS.md).

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod dev;
pub mod panel;
#[cfg(test)]
mod sim;
pub mod ui;

use panel::Panel;
use ui::{Clock, Row, UsbDot, draw_static, draw_usb_dot};

/// The hardware the app needs, as primitive operations. Each one is a few register writes on the unit.
// ponytail: one trait while the board is small; split per peripheral (Panel, Rtc, Keys...) when it grows.
pub trait Board {
    /// Panel power on and reset pulse, with the waits the panel needs before its first command.
    fn panel_power_on(&mut self);
    /// One panel command and its parameters (D/C low for the command byte, high for the parameters).
    fn panel_cmd(&mut self, cmd: u8, params: &[u8]);
    /// RAMWR (0x2c) then `count` pixels of `color`, into the window set by CASET / RASET.
    fn panel_ram_write(&mut self, color: u16, count: u32);
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
    /// Page was just pressed.
    ToggleBacklight,
    /// Back has been held for [`Keys::POWER_OFF_MS`].
    PowerOff,
}

/// Key state between polls: Page acts on the press (not while held), Back acts after a long hold.
#[derive(Default)]
pub struct Keys {
    page_was_down: bool,
    back_held_ms: u32,
}

impl Keys {
    /// How long Back must be held to switch off.
    pub const POWER_OFF_MS: u32 = 3000;

    /// One poll, `dt_ms` after the previous one, with the current key levels (true = pressed).
    pub fn poll(&mut self, page_down: bool, back_down: bool, dt_ms: u32) -> Action {
        let page_pressed = page_down && !self.page_was_down;
        self.page_was_down = page_down;
        self.back_held_ms = if back_down { self.back_held_ms + dt_ms } else { 0 };
        if self.back_held_ms >= Self::POWER_OFF_MS {
            Action::PowerOff
        } else if page_pressed {
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

/// The clock app: what is on screen and the key state.
pub struct App {
    clock: Clock,
    keys: Keys,
    backlight: bool,
    usb: UsbDot,
}

impl App {
    /// Panel up, the fixed parts drawn, backlight on.
    pub fn start(b: &mut impl Board) -> Self {
        panel::init(b);
        draw_static(&mut Panel(b));
        b.backlight(true);
        App { clock: Clock::new(), keys: Keys::default(), backlight: true, usb: UsbDot::Off }
    }

    /// One pass of the main loop: read the time and the keys, redraw what changed. The caller sleeps
    /// [`POLL_MS`] between passes (servicing USB meanwhile on the unit).
    pub fn step(&mut self, b: &mut impl Board) {
        let (h, m, s) = rtc_time_from_tr(b.rtc_tr());
        self.clock.show(&mut Panel(b), h, m, s);
        let (page, back) = b.keys_down();
        match self.keys.poll(page, back, POLL_MS) {
            Action::ToggleBacklight => {
                self.backlight = !self.backlight;
                b.backlight(self.backlight);
            }
            Action::PowerOff => power_off(b),
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
        draw_static(&mut row);
        self.clock.redraw(&mut row);
        draw_usb_dot(&mut row, self.usb);
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
    fn page_toggles_once_per_press() {
        let mut k = Keys::default();
        assert_eq!(k.poll(true, false, 50), Action::ToggleBacklight);
        assert_eq!(k.poll(true, false, 50), Action::None);
        assert_eq!(k.poll(false, false, 50), Action::None);
        assert_eq!(k.poll(true, false, 50), Action::ToggleBacklight);
    }

    #[test]
    fn back_held_3s_powers_off() {
        let mut k = Keys::default();
        for _ in 0..59 {
            assert_eq!(k.poll(false, true, 50), Action::None);
        }
        assert_eq!(k.poll(false, false, 50), Action::None); // released at 2.95 s: start again
        for _ in 0..59 {
            assert_eq!(k.poll(false, true, 50), Action::None);
        }
        assert_eq!(k.poll(false, true, 50), Action::PowerOff);
    }
}
