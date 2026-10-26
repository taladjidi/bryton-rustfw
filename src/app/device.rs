//! The unit itself: the backlight (chosen level, time-out, the ambient light), the buzzer (blocking melodies at
//! power-on and off, alert tones played across passes), the status strip, switching off, and the accessors main.rs
//! uses (the eMMC for the dev link, the battery history).

use crate::*;

/// Tones played at power-on and power-off: (Hz, ms). Rising, then falling (stock's own melodies are not traced).
pub const BEEP_ON: [(u32, u32); 2] = [(1500, 80), (2500, 120)];

/// The power-off tones.
pub const BEEP_OFF: [(u32, u32); 2] = [(2500, 80), (1500, 120)];

/// A ride started or resumed: rising, short (not blocking: the tone player).
pub const BEEP_START: [(u32, u32); 2] = [(1800, 60), (2600, 90)];

/// A ride paused: falling.
pub const BEEP_PAUSE: [(u32, u32); 2] = [(2600, 60), (1800, 90)];

/// An alert's tones: three short beeps (0 Hz: silence), played without blocking (`App::tones`).
pub const BEEP_ALERT: [(u32, u32); 5] = [(2500, 100), (0, 50), (2500, 100), (0, 50), (2500, 100)];

/// Melodies waiting to play after the one playing.
const TONES_QUEUED: usize = 3;

/// The one tone player (alerts, course events, a workout's steps): the melody playing, what is left of it, how long
/// the current tone has left, whether the buzzer is on, and the melodies queued after it. One player, so two sounds
/// asked in the same pass both play whole, one after the other.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Tones {
    left: &'static [(u32, u32)],
    ms: u32,
    on: bool,
    queue: [&'static [(u32, u32)]; TONES_QUEUED],
}

impl Tones {
    /// Queues `melody` (dropped if [`TONES_QUEUED`] are already waiting).
    pub(crate) fn play(&mut self, melody: &'static [(u32, u32)]) {
        if let Some(slot) = self.queue.iter_mut().find(|q| q.is_empty()) {
            *slot = melody;
        }
    }
}

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
/// A pass's work counted at most ([`Board::awake_us`]): the first pass, or a stall, is not a key held for seconds.
pub const MAX_PASS_MS: u32 = 1000;

/// How often the battery is read.
pub const BATTERY_MS: u32 = 10_000;

/// How often the barometer is read while riding (or averaging for a calibration); app/barometer.rs.
pub const BARO_RIDE_MS: u32 = 1000;

/// How often the touch panel is read while a finger is on it: a swipe of 100 ms gives ten points.
pub const TOUCH_MS: u32 = 10;

/// How often the ambient light is read while the backlight follows it (and while the menu shows it): with the
/// smoothing ([`ambient::Auto`]), a change of light reaches the backlight in about a second.
pub const LIGHT_MS: u32 = 250;

impl App {
    /// The computer dropped the USB device and the board reconnected it: counted (read it with `dev.py peek`) and
    /// said in a notice.
    pub fn usb_dropped(&mut self, b: &mut impl Board) {
        let n = b.backup_read(USB_DROPS_BKP);
        let n = if n >> 24 == 0x0b { n & 0xffff } else { 0 };
        b.backup_write(USB_DROPS_BKP, 0x0b00_0000 | (n + 1).min(0xffff));
        // what the drive saw last, kept through the replug a dead link needs: tag 0xd1, the last command, its
        // status, the bus resets (low 4 bits) and Bulk-Only resets (high 4) among the last 32 entries
        let log = self.drive_log();
        let (op, st) = log.iter().last().map_or((0, 0), |e| (e.0, e.3));
        let count = |code: u8| log.iter().filter(|e| e.0 == code).count().min(15) as u32;
        let w = 0xd100_0000 | u32::from(op) << 16 | u32::from(st) << 8 | count(0xfe) << 4 | count(0xff);
        b.backup_write(USB_LAST_BKP, w);
        self.notice("USB RECONNECT");
    }

    /// Sets the backlight to what it should be now: the chosen level, unless timed out.
    pub(crate) fn update_light(&mut self, b: &mut impl Board) {
        let timeout_ms = LIGHT_TIMEOUTS_S[self.settings.light_timeout] * 1000;
        let on = timeout_ms == 0 || self.idle_ms < timeout_ms;
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

    /// The tones playing go on: each lasts its time, by the pass's real length, then the next; the buzzer stops after
    /// the last of a melody for a pass, then the next melody queued starts. Nothing waits: the loop goes on meanwhile.
    pub(crate) fn play_tones(&mut self, b: &mut impl Board) {
        let (t, dt) = (&mut self.tones, self.dt_ms);
        if t.ms > 0 {
            t.ms = t.ms.saturating_sub(dt);
            return;
        }
        if t.left.is_empty() && !t.on {
            t.left = t.queue[0];
            t.queue.rotate_left(1);
            t.queue[TONES_QUEUED - 1] = &[];
        }
        match t.left.split_first() {
            Some((&(hz, len), rest)) => {
                b.beep(hz);
                (t.left, t.ms, t.on) = (rest, len.saturating_sub(dt), true);
            }
            None if t.on => {
                b.beep(0);
                t.on = false;
            }
            None => {}
        }
    }

    /// The light sensor measures while the backlight follows the ambient light (and is on), or the sensors menu
    /// shows it, and is read every [`LIGHT_MS`]; in standby otherwise.
    pub(crate) fn poll_light(&mut self, b: &mut impl Board) {
        if !self.chips.light {
            return;
        }
        self.light_ms = self.light_ms.saturating_add(self.dt_ms);
        let wanted = (self.settings.light_level == LIGHT_AUTO && self.light_shown != 0)
            || matches!(self.screen, Screen::Menu(Menu::BuiltIn, _));
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
        // a ride open: what is in RAM goes on the card first, one checkpoint (a few ms, one try); the next start
        // resumes it at this second (the second review, R4)
        self.recorder.checkpoint_now();
        self.full.checkpoint_now();
        self.step_recorder(b);
        self.hold_pages(b); // pages changed and not in the file yet: the registers keep them (app/editor.rs)
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
        b.touch_reset(false); // the touch chip held in reset: no INT to keep the core out of standby
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
        if self.strip != self.strip_shown && !self.hint_shown {
            // under a banner or a call: not drawn, and drawn whole when it goes (unbanner, end_ring); nor under the
            // editor's hint, and drawn whole when the editor is left (App::show)
            ui::draw_strip_changes(
                &mut Panel(b, (self.area().0, ui::H), self.settings.smooth),
                &self.strip,
                Some(&self.strip_shown),
            );
            self.strip_shown = self.strip;
        }
    }

    /// Shows the USB link state in the strip.
    pub fn set_usb(&mut self, b: &mut impl Board, s: UsbDot) {
        self.strip.usb = s;
        self.update_strip(b);
    }
}
