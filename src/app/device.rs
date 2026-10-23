//! The unit itself: the backlight (chosen level, time-out, the ambient light), the buzzer (blocking melodies at
//! power-on and off, alert tones played across passes), the status strip, switching off, and the accessors main.rs
//! uses (the eMMC for the dev link, the battery history).

use crate::*;

/// Tones played at power-on and power-off: (Hz, ms). Rising, then falling (stock's own melodies are not traced).
pub const BEEP_ON: [(u32, u32); 2] = [(1500, 80), (2500, 120)];

/// The power-off tones.
pub const BEEP_OFF: [(u32, u32); 2] = [(2500, 80), (1500, 120)];

/// An alert's tones: three short beeps (0 Hz: silence), played without blocking ([`App::tones`]).
pub const BEEP_ALERT: [(u32, u32); 5] = [(2500, 100), (0, 50), (2500, 100), (0, 50), (2500, 100)];

/// Plays `tones` (blocking: a few hundred ms, only at power-on and power-off).
pub(crate) fn play(b: &mut impl Board, tones: &[(u32, u32)]) {
    for &(hz, ms) in tones {
        b.beep(hz);
        b.delay_ms(ms);
    }
    b.beep(0);
}

/// Main loop period: the time and the keys are read this often.
pub const POLL_MS: u32 = 50;

/// How often the battery is read.
pub const BATTERY_MS: u32 = 10_000;

/// How often the touch panel is read while a finger is on it: a swipe of 100 ms gives ten points.
pub const TOUCH_MS: u32 = 10;

/// How often the ambient light is read while the backlight follows it (and while the menu shows it): with the
/// smoothing ([`ambient::Auto`]), a change of light reaches the backlight in about a second.
pub const LIGHT_MS: u32 = 250;

impl App {
    /// Sets the backlight to what it should be now: the chosen level, unless switched off (Back) or timed out.
    pub(crate) fn update_light(&mut self, b: &mut impl Board) {
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

    /// The eMMC, shared with the dev link so there is one view of the card's state.
    pub fn emmc(&mut self) -> &mut emmc::Emmc {
        &mut self.emmc
    }

    /// The battery history, oldest first: (mV, charging), one a minute.
    pub fn battery_history(&self) -> impl Iterator<Item = (u16, bool)> + '_ {
        self.battery.history()
    }

    /// The tones playing go on: each lasts its time, in passes of [`POLL_MS`], then the next; the buzzer stops after
    /// the last. Nothing waits: the loop goes on meanwhile.
    pub(crate) fn play_tones(&mut self, b: &mut impl Board) {
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

    /// Starts the tone asked for, or ends the one sounding once its time is up (one pass of [`POLL_MS`] at least).
    pub(crate) fn sound(&mut self, b: &mut impl Board) {
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

    /// The light sensor measures while the backlight follows the ambient light (and is on), or the sensors menu
    /// shows it, and is read every [`LIGHT_MS`]; in standby otherwise.
    pub(crate) fn poll_light(&mut self, b: &mut impl Board) {
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

    /// As stock: the radio chip off too (or it drains the battery), screen off, wait for the key to be released
    /// (or it would wake the unit at once), then standby.
    pub(crate) fn power_off(&mut self, b: &mut impl Board) {
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
        // at most 10 s: a key stuck down must not keep the unit (and the battery) on
        for _ in 0..10_000 / POLL_MS {
            if b.keys_down() & Key::Back as u8 == 0 {
                break;
            }
            b.delay_ms(POLL_MS);
        }
        b.delay_ms(50);
        b.standby();
    }

    /// Redraws the status strip if what it shows changed.
    pub(crate) fn update_strip(&mut self, b: &mut impl Board) {
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
}
