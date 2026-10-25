//! The home trainer in the app (trainer.rs is the model, drivers/fec.rs the ANT+ FE-C bytes): what the trainer
//! reports feeds the live values, what drives it (the workout's power step, the course's grade, the rider's keys on
//! the trainer screen) becomes the commands sent on its channel, and the trainer screen and fields show it.
//!
//! Which source feeds the ride: a power meter, a speed or cadence sensor and a heart-rate strap, when heard, win over
//! the trainer's own readings; the trainer fills in what is not heard ([`App::fold_trainer`]). Indoors there is no GPS,
//! so the ride's speed and distance come from the trainer's speed when no speed sensor is paired.

use crate::*;
use trainer::{Command, Drive, Mode, Setup};
use ui::trainer::Now;

/// The bike's weight told to the trainer (its SIM mode wants it), kg.
// ponytail: fixed; a setting when someone's bike is not about 10 kg
const BIKE_KG: u16 = 10;

impl App {
    /// A trainer is paired or searched for (its menu row and screen are offered).
    pub(crate) fn trainer_on(&self) -> bool {
        self.sensors.get(Kind::Trainer as usize).is_some_and(|s| *s != Sensor::Off)
    }

    /// The trainer sent something lately.
    pub(crate) fn trainer_heard(&self) -> bool {
        self.heard_ms.get(Kind::Trainer as usize).is_some_and(|&ms| ms < sensors::STALE_MS)
    }

    /// Keeps `kind`'s paired sensor in its backup register. The trainer shares register 14 with the shifting
    /// systems (no register is free): it is kept there only while no shifting system is, else it is paired for this
    /// power-up only.
    pub(crate) fn keep_pairing(&mut self, b: &mut impl Board, kind: Kind, id: SensorId) {
        let shifting = [Kind::Shifting, Kind::Di2].iter().any(|k| k.from_word(b.backup_read(k.backup())).is_some());
        if kind != Kind::Trainer || !shifting {
            b.backup_write(kind.backup(), kind.to_word(id));
        }
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
        let grade_x100 = self
            .courses
            .as_deref()
            .filter(|c| c.on.is_some())
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

    /// Once a pass: the trainer's set-up when it is first heard, and the command that is due. Nothing here waits on
    /// the card or the recorder, and a refusal by the chip is a command sent again next pass.
    pub(crate) fn step_trainer(&mut self, b: &mut impl Board) {
        let heard = self.trainer_heard() && self.ant;
        if !heard {
            if core::mem::take(&mut self.trainer_up) {
                self.trainer.lost();
            }
            return;
        }
        self.trainer_up = true;
        let ch = Kind::Trainer.channel();
        let d = self.trainer_drive();
        let page = match self.trainer.setup() {
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
        };
        if let Some(page) = page
            && nrf::ant_ack(b, &mut self.rx, ch, page).is_err()
        {
            self.trainer.unsent(); // asked for again next pass
        }
        if self.screen == Screen::Trainer {
            self.update_trainer(b);
        }
    }

    /// Runs `f` on what the trainer screen shows now.
    fn with_trainer_now<R>(&self, f: impl FnOnce(&Now) -> R) -> R {
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
        } else if matches!(r.last_cmd, Some((_, 1..=3))) {
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
            power: r.power,
            cadence: r.cadence,
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
    /// leaves; the touch buttons do the same.
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
