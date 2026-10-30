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

    /// Nothing playing or queued: the pass's length does not time a tone.
    fn quiet(&self) -> bool {
        !self.on && self.ms == 0 && self.left.is_empty() && self.queue[0].is_empty()
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

/// The stack canary's pattern (main.rs writes it at start into link.x `_stack_canary`, the word just above `.bss`,
/// and checks it every pass): any other value means the stack ran down into it, a fault (hw.rs `fault`: the marker,
/// a reset, the ride resumes from its checkpoint). Not 0 nor a likely pointer or small count, so a frame's own
/// words are unlikely to leave it as it was.
pub const STACK_CANARY: u32 = 0x5aa5_c3c3;

/// The stack canary's word `word` still holds [`STACK_CANARY`].
///
/// ```
/// use bryton_rustfw::{STACK_CANARY, stack_intact};
/// assert!(stack_intact(STACK_CANARY));
/// assert!(!stack_intact(0) && !stack_intact(0x2001_ff00) && !stack_intact(STACK_CANARY ^ 1));
/// ```
pub const fn stack_intact(word: u32) -> bool {
    word == STACK_CANARY
}
/// Main loop period while idle ([`App::pass_ms`]): no ride, no key for [`IDLE_AFTER_MS`]. A key, a touch and a byte
/// from the Nordic chip end main.rs's sleep at once, so this bounds only what nothing announces: the clock shown (a
/// second at most this late), the GNSS read, the timers' resolution.
pub const IDLE_POLL_MS: u32 = 250;
/// No key for this long (the backlight's default OFF AFTER), and the loop may slow to [`IDLE_POLL_MS`].
pub const IDLE_AFTER_MS: u32 = 30_000;
/// A pass's work counted at most ([`Board::awake_us`]): the first pass, or a stall, is not a key held for seconds.
pub const MAX_PASS_MS: u32 = 1000;

/// How often the battery is read.
pub const BATTERY_MS: u32 = 10_000;

/// BATTERY LOW is said at this charge, percent, or below (not while charging).
pub(crate) const LOW_BATTERY_PERCENT: u8 = 15;

/// How often the barometer is read while riding (or averaging for a calibration); app/barometer.rs.
pub const BARO_RIDE_MS: u32 = 1000;

/// How often the touch panel is read while a finger is on it: a swipe of 100 ms gives ten points.
pub const TOUCH_MS: u32 = 10;

/// How often the ambient light is read while the backlight follows it (and while the menu shows it): with the
/// band chosen once a second ([`ambient::Auto`], stock's timing), a change of light reaches the
/// backlight within 2 s (a window that straddles it, then a whole one).
pub const LIGHT_MS: u32 = 250;

/// How often the light is read when only THEME AUTO wants it (the backlight not following it, the menu not showing it):
/// one reading a second, the mean the switch checks ([`ui::theme::DayNight`]). The switch is decided and drawn while the
/// screen is off too: it costs one reading a second, and the theme on the panel is the one for when the screen returns.
pub const THEME_LIGHT_MS: u32 = 1000;

impl App {
    /// The battery, every [`BATTERY_MS`]: the charge and the charger for the strip, "BATTERY LOW" once at
    /// [`LOW_BATTERY_PERCENT`]. True when the cell is empty and the unit was switched off.
    pub(crate) fn poll_battery(&mut self, b: &mut impl Board, pass_ms: u32) -> bool {
        self.battery_ms = self.battery_ms.saturating_add(pass_ms);
        if self.battery_ms < BATTERY_MS {
            return false;
        }
        self.battery_ms = 0;
        let charging = b.charger_present();
        let reading = b.battery_adc().and_then(|(raw, vref, cal)| battery::millivolts(raw, vref, cal));
        if let Some(mv) = reading
            && self.battery.update(mv, charging)
        {
            self.power_off(b); // empty: off before the cell is drawn too low
            return true;
        }
        self.strip.battery = self.battery.percent();
        self.strip.charging = charging;
        let low = self.strip.battery.is_some_and(|p| p <= LOW_BATTERY_PERCENT) && !charging;
        if low && !self.told_low {
            self.alarm_notice("BATTERY LOW");
        }
        self.told_low = low;
        false
    }

    /// The computer dropped the USB device and the board reconnected it: counted (read it with `tools/dev.py peek`) and
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

    /// How long main.rs sleeps before the next pass: [`IDLE_POLL_MS`] when nothing is going on (no ride or its card,
    /// no key for [`IDLE_AFTER_MS`], no finger, no tone or banner timed by the passes, the card not lent to the
    /// computer), else [`POLL_MS`]. Measured 2026-10-28 overnight on the overview: 20 passes/s of 0.4 ms each.
    pub fn pass_ms(&self) -> u32 {
        let idle = !self.busy()
            && !self.touching()
            && self.light.idle_ms >= IDLE_AFTER_MS
            && self.tones.quiet()
            && self.notes.ms.is_none()
            && !self.emmc.lent();
        if idle { IDLE_POLL_MS } else { POLL_MS }
    }

    /// Sets the backlight to what it should be now: the chosen level, unless timed out.
    pub(crate) fn update_light(&mut self, b: &mut impl Board) {
        let timeout_ms = LIGHT_TIMEOUTS_S.get(self.settings.light_timeout).copied().unwrap_or(0) * 1000;
        let on = timeout_ms == 0 || self.light.idle_ms < timeout_ms || self.light.wake;
        let level = match LIGHT_LEVELS.get(self.settings.light_level) {
            _ if !on => 0,
            Some(&l) => l,
            None => self.light.auto_level,
        };
        if level != self.light.shown {
            self.light.shown = level;
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
        if !self.settings.sound {
            // SOUND OFF: what was asked is dropped, the tone sounding is cut (alerts included)
            (t.queue, t.left, t.ms) = ([&[][..]; TONES_QUEUED], &[][..], 0);
        }
        if t.ms > 0 {
            t.ms = t.ms.saturating_sub(dt);
            if t.ms > 0 {
                return;
            }
        }
        if t.left.is_empty() && !t.on {
            t.left = t.queue[0];
            t.queue.rotate_left(1);
            t.queue[TONES_QUEUED - 1] = &[];
        }
        match t.left.split_first() {
            Some((&(hz, len), rest)) => {
                b.beep(hz);
                // its whole length from now: the pass before (an idle one is 250 ms) was not part of it
                (t.left, t.ms, t.on) = (rest, len, true);
            }
            None if t.on => {
                b.beep(0);
                t.on = false;
            }
            None => {}
        }
    }

    /// The light sensor measures while the backlight follows the ambient light (and is on), or the sensors menu
    /// shows it, read every [`LIGHT_MS`]; while THEME AUTO alone wants it, read every [`THEME_LIGHT_MS`]; in standby
    /// otherwise.
    pub(crate) fn poll_light(&mut self, b: &mut impl Board) {
        if !self.chips.light {
            return;
        }
        self.light.ms = self.light.ms.saturating_add(self.dt_ms);
        let follows = (self.settings.light_level == LIGHT_AUTO && self.light.shown != 0)
            || matches!(self.screen, Screen::Menu(Menu::BuiltIn, _));
        let wanted = follows || self.settings.auto_theme;
        let every = if follows { LIGHT_MS } else { THEME_LIGHT_MS };
        match (wanted, self.light.measuring) {
            (true, false) => {
                self.light.measuring = ambient::light_start(b);
                self.light.ms = 0;
            }
            (true, true) if self.light.ms >= every => {
                if let Some(lux10) = ambient::light_read(b) {
                    self.light.auto_level = self.light.auto.update(lux10);
                    let dt = self.light.ms;
                    self.light.ms = 0;
                    self.follow_light(b, lux10, dt);
                }
            }
            (false, true) => {
                ambient::light_stop(b);
                self.light.measuring = false;
            }
            _ => {}
        }
    }

    /// As stock: the radio chip off too (or it drains the battery), screen off, wait for the key to be released
    /// (or it would wake the unit at once), then standby.
    pub(crate) fn power_off(&mut self, b: &mut impl Board) {
        // a ride open: what is in RAM goes on the card first, one checkpoint (a few ms, one try); the next start
        // resumes it at this second (the second review, R4)
        self.rec.clean.checkpoint_now();
        self.rec.full.checkpoint_now();
        self.step_recorder(b);
        self.hold_pages(b); // pages changed and not in the file yet: the registers keep them (app/editor.rs)
        if self.ble.is_some() {
            self.release_trainer(b); // not left at the last target until its own time-out
            nrf::system_off(b);
        }
        b.gnss_power(false);
        play(b, &BEEP_OFF);
        if self.light.measuring {
            ambient::light_stop(b);
            self.light.measuring = false; // the rest of this pass (standby returns on the Sim) must not read it
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

    /// What kept AUTO OFF from counting at its last second: one bit per reason, in `step_auto_off`'s order
    /// (`tools/dev.py info`).
    pub fn auto_off_held(&self) -> u32 {
        self.auto_off.why
    }

    /// AUTO OFF, once a second: counts the time with nothing keeping the unit on (no ride open or on its card, no
    /// cable, the drive not lent, no key or touch, no call, no write waiting, no movement) and switches off at the
    /// time-out, with a warning 10 s and 5 s before. True when it switched off.
    pub(crate) fn step_auto_off(&mut self, b: &mut impl Board) -> bool {
        if self.auto_off.seen == self.seconds {
            return false;
        }
        self.auto_off.seen = self.seconds;
        // what keeps the unit on, one bit each in this order (`tools/dev.py info` prints it: "auto off held 0x...")
        let held = [
            self.light.idle_ms < 1000, // a key or touch in the last second
            self.busy(),
            matches!(self.screen, Screen::End(_) | Screen::Discard(_)),
            self.emmc.lent(),
            self.phone.ringing,
            self.touching(),
            self.store.dirty || self.settings_store.dirty, // pages.bin or settings.bin waits to be written
            self.stock.status() == stock::Status::Running, // the scan and its cache
            self.sensors_store.dirty,                      // sensors.bin waits
            self.alt.is_dirty(),                           // so does the altitude calibration
            // the golden rule: never off while the recorder holds anything (a resume that keeps failing, a save
            // retried after the card closed, the full copy still saving)
            self.rec.resume_pending,
            self.rec.full_pending,
            self.rec.clean.phase() != recorder::Phase::Done,
            self.rec.full.phase() != recorder::Phase::Done,
            self.card_writable && b.backup_read(RIDE_BKP) == RIDE_OPEN,
            b.charger_present(),
            // movement keeps it on, for 60 s after the last: a speed over 2 km/h, cadence or power, from a sensor,
            // the GPS or a trainer (app/trainer.rs `fold_trainer`); a trainer merely heard or connected is not
            // movement: one left powered broadcasts all day (2026-10-28)
            self.live.get(Metric::Speed).is_some_and(|v| v > 20),
            self.live.get(Metric::Cadence).is_some_and(|v| v > 0),
            self.live.get(Metric::Power).is_some_and(|v| v > 0),
        ];
        const MOVING: u32 = 16; // the first movement bit
        let why = held.iter().enumerate().fold(0u32, |w, (i, &h)| w | u32::from(h) << i);
        self.auto_off.hold = if why >> MOVING != 0 { 60 } else { self.auto_off.hold.saturating_sub(1) };
        self.auto_off.why = why;
        let quiet = why == 0 && self.auto_off.hold == 0;
        if !quiet {
            self.light.wake = false;
        }
        let min = AUTO_OFF_MIN.get(self.settings.auto_off).copied().unwrap_or(0);
        match self.auto_off.second(min, quiet) {
            OffStep::Wait => false,
            OffStep::Warn => {
                self.alarm("SWITCHING OFF");
                self.light.wake = true; // seen and cancelled even with the light timed out; idle_ms keeps counting
                self.tones.play(&BEEP_ALERT);
                false
            }
            OffStep::Off => {
                self.power_off(b);
                true
            }
        }
    }

    /// Redraws the status strip if what it shows changed.
    pub(crate) fn update_strip(&mut self, b: &mut impl Board) {
        let want = self.ahead();
        if want != self.plan.top && !self.hint_shown && self.area().0 == 0 {
            // the next-step strip takes the status strip's place for the last seconds of a step: whole when it
            // appears, its countdown's changed cells each second, the status strip back when it goes
            let mut p = Panel(b, (0, ui::H), self.look());
            match want {
                Some(a) => {
                    let old = self.plan.top.filter(|o| o.next == a.next && o.metres == a.metres).map(|o| o.count());
                    ui::draw_next_strip(
                        &mut p,
                        self.ahead_head(&a).as_str(),
                        a.count().as_str(),
                        old.as_ref().map(|o| o.as_str()),
                    );
                }
                None => {
                    ui::draw_strip_whole(&mut p, &self.strip);
                    self.strip_shown = self.strip;
                }
            }
            self.plan.top = want;
        }
        if self.strip != self.strip_shown && !self.hint_shown && self.plan.top.is_none() {
            // under a banner or a call: not drawn, nor under the next-step strip, and drawn whole when it goes
            // (unbanner, end_ring); nor under the editor's hint, and drawn whole when the editor is left (App::show)
            ui::draw_strip_changes(
                &mut Panel(b, (self.area().0, ui::H), self.look()),
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
