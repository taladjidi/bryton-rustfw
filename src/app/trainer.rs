//! The home trainer in the app (trainer.rs is the model, drivers/fec.rs the ANT+ FE-C bytes, app/ftms.rs the
//! Bluetooth link, which feeds the same reading and takes the same commands): what the trainer
//! reports feeds the live values, what drives it (the workout's power step, the course's grade, the rider's keys on
//! the trainer screen) becomes the commands sent on its channel, and the trainer screen and fields show it.
//!
//! Which source feeds the ride: a power meter, a speed or cadence sensor and a heart-rate strap, when heard, win over
//! the trainer's own readings; the trainer fills in what is not heard ([`App::fold_trainer`]). Indoors there is no GPS,
//! so the ride's speed and distance come from the trainer's speed when no speed sensor is paired (and the GPS's speed,
//! a fix through a window at 0 km/h, does not replace it: gps.rs).
//!
//! **One acknowledged message a channel period.** The chip takes one acknowledged message per period of the
//! channel (250 ms) and refuses the next while the last is queued, so [`step_trainer`](App::step_trainer) sends at
//! most one per [`ACK_GAP_MS`]: the set-up, the commands and the course's grade changes all wait their turn. A message
//! the chip refuses, or the trainer does not acknowledge (`EVENT_TRANSFER_TX_FAILED`, [`nrf::ant_tx_failed`]), is due
//! again. A chip that does not answer at all (the message's layout is inferred, not seen on the unit) turns the
//! control off for the power-up, with a notice, instead of letting the link watchdog reset the chip over it.
//!
//! **The pairing is in `\Rustfw\sensors.bin`** (sensors.rs `sector`), not in a backup register: 14 is the
//! shifting systems', 15 is stock's. Read a few seconds after the start (like the barometer's file), written between
//! rides.

use crate::*;
use trainer::{Command, Drive, Mode, Setup};
use ui::trainer::Now;

/// The least time between two acknowledged messages on the trainer's channel, ms: its period (8192 / 32768 s).
pub const ACK_GAP_MS: u32 = 250;

/// The sensors file is read this long after the start (the GNSS clock, as the barometer's file).
pub const SENSORS_LOAD_MS: u32 = 3500;

/// The pairing file's state.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SensorStore {
    /// The file was read (or tried).
    pub(crate) loaded: bool,
    /// The paired trainer was opened after the read.
    pub(crate) applied: bool,
    /// The trainer pairing changed: to be written.
    pub(crate) dirty: bool,
    /// The paired trainer.
    pub(crate) trainer: Option<SensorId>,
    /// The paired Bluetooth trainer (app/ftms.rs): its address byte and address.
    pub(crate) bt: Option<[u8; 7]>,
}

/// The bike's weight told to the trainer (its SIM mode wants it), kg.
// ponytail: fixed; a setting when someone's bike is not about 10 kg
const BIKE_KG: u16 = 10;

impl App {
    /// A trainer is paired or searched for, over ANT+ or Bluetooth (its menu row and screen are offered).
    pub(crate) fn trainer_on(&self) -> bool {
        self.sensors.get(Kind::Trainer as usize).is_some_and(|s| *s != Sensor::Off) || self.sensors_store.bt.is_some()
    }

    /// The trainer sent something lately.
    pub(crate) fn trainer_heard(&self) -> bool {
        self.heard_ms.get(Kind::Trainer as usize).is_some_and(|&ms| ms < sensors::STALE_MS)
    }

    /// Keeps `kind`'s paired sensor: in its backup register, the trainer's in the sensors file (written between
    /// rides, [`App::sensors_storage`]).
    pub(crate) fn keep_pairing(&mut self, b: &mut impl Board, kind: Kind, id: SensorId) {
        if kind == Kind::Trainer {
            self.sensors_store.dirty |= self.sensors_store.trainer != Some(id);
            self.sensors_store.trainer = Some(id);
        } else {
            b.backup_write(kind.backup(), kind.to_word(id));
        }
    }

    /// The trainer is forgotten (SENSORS: off).
    pub(crate) fn forget_trainer(&mut self) {
        self.sensors_store.dirty |= self.sensors_store.trainer.is_some();
        self.sensors_store.trainer = None;
    }

    /// What drives the trainer now: the workout's power step (the midpoint of its range), the course's grade at the
    /// distance ridden (the trainer's speed advances the ride; no GPS indoors).
    pub(crate) fn trainer_drive(&self) -> Drive {
        let steps = self.plan.steps();
        let step = self.player.and_then(|p| p.step(steps));
        let erg_w = step
            .and_then(|i| steps.get(i))
            .filter(|s| s.target.kind == storage::workout::Kind::Power)
            .and_then(|s| s.target.range(self.settings.ftp, self.settings.max_hr))
            .map(|(lo, hi)| lo.midpoint(hi));
        // % x 10 to % x 100
        // not before the ride starts: the course's first grade would be put on the trainer while the rider sits
        let grade_x100 = self
            .courses
            .as_deref()
            .filter(|c| c.on.is_some() && self.ride.state() != ride::State::Idle)
            .and_then(|c| c.course.grade_ahead(self.ride.dist_m()))
            .map(|g| (g * 10).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        Drive { erg_w, step: step.unwrap_or(0), grade_x100 }
    }

    /// The trainer's readings into the live values, where nothing better is heard: power from a power meter, cadence
    /// from the meter or a cadence sensor, speed from a speed sensor, heart rate from a strap.
    pub(crate) fn fold_trainer(&mut self) {
        let heard = |k: Kind| self.heard_ms.get(k as usize).is_some_and(|&ms| ms < sensors::STALE_MS);
        let r = self.trainer.reading;
        if let (Some(p), false) = (r.power, heard(Kind::Power)) {
            self.live.set(Metric::Power, p);
        }
        if let (Some(c), false) = (r.cadence, heard(Kind::Power) || heard(Kind::Cadence) || heard(Kind::SpeedCadence)) {
            self.live.set(Metric::Cadence, u16::from(c));
        }
        if let (Some(v), false) = (r.speed_x10, heard(Kind::Speed) || heard(Kind::SpeedCadence)) {
            self.live.set(Metric::Speed, v);
        }
        if let (Some(h), false) = (r.hr, heard(Kind::Hr)) {
            self.live.set(Metric::Hr, u16::from(h));
        }
    }

    /// Once a pass: the pairing file, the trainer's messages, and the screen on the second's tick.
    pub(crate) fn step_trainer(&mut self, b: &mut impl Board) {
        self.sensors_storage(b);
        if self.ble.is_some() {
            self.step_ftms(b);
        }
        self.send_trainer(b);
        if core::mem::take(&mut self.trainer_second) && self.screen == Screen::Trainer {
            self.update_trainer(b); // power and cadence are the second's snapshot: nothing new between two ticks
        }
    }

    /// The trainer's set-up when it is first heard, and the command that is due: one acknowledged message at most
    /// per [`ACK_GAP_MS`]. Nothing here waits on the card or the recorder.
    fn send_trainer(&mut self, b: &mut impl Board) {
        let bt = self.ftms_on();
        let heard = self.trainer_heard() && (self.ant || bt);
        if !heard {
            if core::mem::take(&mut self.trainer_up) {
                self.trainer.lost();
            }
            return;
        }
        self.trainer_up = true;
        let d = self.trainer_drive();
        if bt {
            // FTMS: the set-up is the link's (app/ftms.rs); a command once the last one's response came
            let ready = self.ftms.ready();
            if let Some(c) = self.trainer.due(self.dt_ms, ready, &d) {
                self.ftms_command(b, c);
            }
            return;
        }
        self.ack_gap_ms = self.ack_gap_ms.saturating_add(self.dt_ms);
        let page = if self.trainer_no_control || self.ack_gap_ms < ACK_GAP_MS {
            self.trainer.due(self.dt_ms, false, &d); // the clocks run, nothing goes out
            None
        } else {
            match self.trainer.setup(self.settings.weight_kg) {
                Some(Setup::Capabilities) => Some(fec::request(54)),
                Some(Setup::User) => {
                    let s = &self.settings;
                    Some(fec::encode(Command::User {
                        weight_cg: s.weight_kg.saturating_mul(100),
                        bike_50g: BIKE_KG * 20,
                        wheel_mm: (u32::from(s.wheel_mm) * 1000 / 3142) as u16,
                    }))
                }
                None => self.trainer.due(self.dt_ms, true, &d).map(fec::encode),
            }
        };
        let Some(page) = page else { return };
        self.ack_gap_ms = 0;
        match nrf::ant_ack(b, &mut self.rx, Kind::Trainer.channel(), page) {
            Ok(()) => {}
            Err(nrf::Error::Timeout) => {
                // no answer at all: the message's layout is a guess. Not tried again this power-up, and the chip
                // is not reset over it (ant_ack keeps the watchdog's count)
                self.trainer.unsent();
                self.trainer_no_control = true;
                self.notice("TRAINER CONTROL FAILED");
            }
            Err(nrf::Error::Code(_)) => self.trainer.unsent(), // refused: due again at the next free slot
        }
    }

    /// The trainer's pairing file: read once after the start, written when it changed, between rides. A pairing an
    /// earlier build kept in backup register 14 is moved here, and 14 is left to the shifting system.
    #[inline(never)]
    fn sensors_storage(&mut self, b: &mut impl Board) {
        if !self.card_writable || self.emmc.lent() || self.gnss_ms < SENSORS_LOAD_MS {
            return;
        }
        if !self.sensors_store.loaded {
            self.sensors_store.loaded = true;
            if !self.sensors_store.dirty {
                let mut s = [0u8; 512];
                let mut d = emmc::Volume(&mut self.emmc, b);
                let read = fat::Fat::mount(&mut d).and_then(|mut fat| {
                    let dir = fat.open(&mut d, recorder::DIR)?;
                    let f = fat.open_in(&mut d, dir, sensors::SENSORS_FILE)?;
                    fat.read_at(&mut d, &f, 0, &mut s)
                });
                if let (Ok(512), Some(t)) = (read, sensors::from_sector(&s)) {
                    (self.sensors_store.trainer, self.sensors_store.bt) = (t, sensors::bt_from_sector(&s));
                }
            }
            if let Some(id) = Kind::Trainer.from_word(b.backup_read(Kind::Trainer.backup())) {
                self.keep_pairing(b, Kind::Trainer, id); // moved to the file below
            }
        }
        // opened once ANT is up (it may come later than the file), unless the rider already did something with it
        if !self.sensors_store.applied && self.ant {
            self.sensors_store.applied = true;
            if let Some(id) = self.sensors_store.trainer
                && self.sensors[Kind::Trainer as usize] == Sensor::Off
            {
                self.set_sensor(b, Kind::Trainer, Sensor::Paired(id));
            }
        }
        if !self.sensors_store.dirty || !self.ride_settled() {
            return;
        }
        let (local, _) = self.rec_time();
        let sector = sensors::sector(self.sensors_store.trainer, self.sensors_store.bt);
        let mut d = emmc::Volume(&mut self.emmc, b);
        let r = fat::Fat::mount(&mut d).and_then(|mut fat| {
            let t = fat::write::dos_time(local);
            let dir = fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)?;
            let mut h = match fat.locate(&mut d, dir, sensors::SENSORS_FILE) {
                Err(fat::Error::NotFound) => fat.create(&mut d, dir, sensors::SENSORS_FILE, t)?,
                other => other?,
            };
            fat.write_sectors(&mut d, &mut h, 0, &sector)?;
            if h.size != 512 {
                fat.set_size(&mut d, &mut h, 512, t)?;
            }
            Ok(())
        });
        if r.is_ok() {
            self.sensors_store.dirty = false;
            // the file holds it: the register goes back to the shifting system (an earlier build's word only)
            if Kind::Trainer.from_word(b.backup_read(Kind::Trainer.backup())).is_some() {
                b.backup_write(Kind::Trainer.backup(), 0);
            }
        } else {
            self.sensors_store.dirty = false; // not tried again until the next change, as the barometer's file
            self.notice("PAIRING NOT SAVED");
        }
    }

    /// Runs `f` on what the trainer screen shows now.
    pub(crate) fn with_trainer_now<R>(&self, f: impl FnOnce(&Now) -> R) -> R {
        let d = self.trainer_drive();
        let (t, r) = (&self.trainer, &self.trainer.reading);
        let mut target = Line::<12>::new();
        self.push_trainer_target(&mut target, true);
        let engaged = t.engaged(&d);
        let (status, status_color) = if !self.trainer_heard() {
            ("NO TRAINER", ui::trainer::BAD)
        } else if r.needs_calibration() {
            ("CALIBRATE", ui::trainer::WARN)
        } else if !engaged {
            ("PAGE / LAP: SET", ui::GREY)
        } else if matches!(r.last_cmd, Some((page, 1..=3)) if page == fec::encode(t.command(&d))[0]) {
            // the status of the command in force (an old refusal of another page is not this one's)
            ("NOT ACCEPTED", ui::trainer::BAD)
        } else if t.mode(&d) == Mode::Erg && r.limits == 1 {
            ("SPEED TOO LOW", ui::trainer::WARN)
        } else if t.mode(&d) == Mode::Erg && r.limits == 2 {
            ("SPEED TOO HIGH", ui::trainer::WARN)
        } else {
            ("OK", ui::trainer::GOOD)
        };
        let source = match (d.erg_w, d.grade_x100) {
            (Some(_), _) => "WORKOUT",
            (None, Some(_)) => "COURSE",
            _ => "MANUAL",
        };
        f(&Now {
            mode: t.mode(&d).name(),
            source,
            target: target.as_str(),
            power: self.live.shown(Metric::Power),
            cadence: self.live.shown(Metric::Cadence).and_then(|c| u8::try_from(c).ok()),
            status,
            status_color,
        })
    }

    /// The target as text: "250 W", "40 %", "-2.5 %" (`unit` with a space; the page field without).
    fn push_trainer_target(&self, l: &mut Line<12>, spaced: bool) {
        let d = self.trainer_drive();
        let t = &self.trainer;
        let unit = |l: &mut Line<12>, u: &str| {
            if spaced {
                l.push_str(" ");
            }
            l.push_str(u);
        };
        match t.mode(&d) {
            Mode::Erg => {
                l.push_u32(u32::from(t.target_w(&d)));
                unit(l, "W");
            }
            Mode::Resistance => {
                l.push_u32(u32::from(t.resistance_pct()));
                unit(l, "%");
            }
            Mode::Sim => {
                let g = i32::from(t.grade(&d));
                if g < 0 {
                    l.push_str("-");
                }
                let g = g.unsigned_abs();
                l.push_u32(g / 100);
                l.push_str(".");
                l.push_str(ui::DIGITS[(g / 10 % 10) as usize]);
                unit(l, "%");
            }
        }
    }

    /// [`Field::TrainerMode`] and [`Field::TrainerTarget`]'s text ("--" without a trainer).
    pub(crate) fn trainer_field(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        if !self.trainer_heard() {
            l.push_str("--");
            return l;
        }
        if f == Field::TrainerMode {
            l.push_str(self.trainer.mode(&self.trainer_drive()).name());
        } else {
            let mut t = Line::<12>::new();
            self.push_trainer_target(&mut t, false);
            l.push_str(t.as_str());
        }
        l
    }

    /// The whole trainer screen.
    pub(crate) fn draw_trainer(&self, c: &mut impl Canvas) {
        self.with_trainer_now(|n| {
            ui::trainer::draw(c, n);
        });
    }

    /// What the trainer screen shows, as drawn (set when the screen is shown).
    pub(crate) fn trainer_printed(&mut self) {
        self.trainer_shown = self.with_trainer_now(ui::trainer::shown);
    }

    /// Redraws the trainer screen's boxes that changed.
    pub(crate) fn update_trainer(&mut self, b: &mut impl Board) {
        let mut shown = self.trainer_shown;
        let area = self.area();
        self.with_trainer_now(|n| ui::trainer::update(&mut Panel(b, area), n, &mut shown));
        self.trainer_shown = shown;
    }

    /// An input on the trainer screen: Page and Lap (held: twice the step) change the target, Start the mode, Back
    /// leaves; the touch buttons do the same. During a ride Start and Lap keep their ride meaning (input.rs): the
    /// mode changes by the MODE button only, the target down by "-".
    pub(crate) fn trainer_input(&mut self, b: &mut impl Board, i: Input) {
        use ui::trainer::Button;
        let d = self.trainer_drive();
        match i {
            Input::Next => self.trainer.step_by(&d, true, false),
            Input::Prev => self.trainer.step_by(&d, false, false),
            Input::NextHeld | Input::PrevHeld => {
                self.repeated = true;
                self.trainer.step_by(&d, i == Input::NextHeld, true);
            }
            Input::Select => self.trainer.cycle_mode(&d),
            Input::Touch(Gesture::Tap(x, y)) => match ui::trainer::hit(x, y) {
                Some(Button::Plus) => self.trainer.step_by(&d, true, false),
                Some(Button::Minus) => self.trainer.step_by(&d, false, false),
                Some(Button::Mode) => self.trainer.cycle_mode(&d),
                None => return,
            },
            _ => return,
        }
        self.update_trainer(b);
    }
}
