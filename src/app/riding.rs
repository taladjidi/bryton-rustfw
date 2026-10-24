//! The ride from the app's side: Start / pause from a key, the second tick that feeds the ride, the alerts, the
//! workout, power, the history and the course, and the glue to the recorder (each ride event, the samples, the
//! checkpoints and the resume at the first step).

use crate::*;

/// The ride catches up at most this many seconds after a long pass ([`App::step`]).
pub const MAX_CATCH_UP: u8 = 10;

impl App {
    /// Start, pause or resume the ride from a key, saying so; a new ride also resets the power totals.
    pub(crate) fn start_pause(&mut self) {
        if self.emmc.lent() {
            return self.say("USB DRIVE ON"); // no ride while the computer has the card (drive_on refuses the reverse)
        }
        let ev = self.ride.start_pause();
        if ev == ride::Event::Start {
            // the ride's own track and graphs, from here (the last position kept, so the map is not empty)
            self.ride_started = (self.date, self.strip.time);
            self.history = ride::History::default();
            self.alerts = alerts::Alerts::new();
            let mut track = map::Track::new();
            if let Some(p) = self.pvt.filter(|p| p.good()) {
                track.seed(p.lat, p.lon);
            }
            self.track = track;
            self.new_ride = true;
        }
        self.say(match ev {
            ride::Event::Start => "RIDE STARTED",
            ride::Event::Pause => "PAUSED: HOLD = END",
            _ => "RESUMED",
        });
        if ev == ride::Event::Start {
            self.power.new_ride();
        }
        self.recorder(ev);
    }

    // -----------------------------------------------------------------------------------------------------------
    // The recorder (recorder.rs): events into its RAM buffer here, its card work in step_recorder
    // -----------------------------------------------------------------------------------------------------------

    /// The ride recorder's one entry point: every [`ride::Event`] goes through here, in order. RAM only: the card
    /// work is [`App::step_recorder`]'s, at the end of the pass.
    pub(crate) fn recorder(&mut self, ev: ride::Event) {
        if !self.card_writable {
            // a build that may not write the card: no recorder at all (it would read both FATs at each try only to
            // fail: about 2 s of a stopped loop every 5 s, the second review found), said once a ride
            if ev == ride::Event::Start {
                self.notice("NOT RECORDING");
            }
            return;
        }
        let (local, t) = self.rec_time();
        let words = self.ride.to_words();
        match ev {
            ride::Event::Start => {
                // a ride the boot has not reopened yet (the card failed then) is resumed in this one's place
                self.recorder.start(t, recorder::ride_name(local), self.resume_pending);
                if self.recorder.phase() != recorder::Phase::Starting {
                    self.notice("NOT RECORDING"); // the last ride's save is still failing
                }
            }
            ride::Event::Second => {
                let s = self.sample(t);
                self.recorder.second(&s, words);
            }
            ride::Event::Pause => self.recorder.pause(t, words),
            ride::Event::Resume => self.recorder.resume(t),
            ride::Event::Lap => self.recorder.lap(t),
            ride::Event::End => self.recorder.end(t, words),
        }
    }

    /// Local time now (the RTC as last read) and its FIT time (UTC from the offset learnt at a fix; the RTC as is
    /// before one), less the seconds being caught up ([`App::step`]).
    pub(crate) fn rec_time(&self) -> (recorder::Local, u32) {
        let ((y, mo, d), (h, mi)) = (self.date, self.strip.time);
        let local = (y, mo, d, h, mi, self.seconds);
        let offset = i64::from(self.aid.offset_15min.unwrap_or(0)) * 900 + i64::from(self.lag_s);
        (local, fit::fit_time(from_epoch_s(epoch_s(local) - offset)))
    }

    /// This second's record: the fix, the sensors, the barometer's temperature.
    pub(crate) fn sample(&self, time: u32) -> fit::Sample {
        let fix = self.pvt.filter(|p| p.good());
        let byte = |m| self.live.get(m).map(|v| v.min(254) as u8);
        fit::Sample {
            time,
            pos: fix.map(|p| (p.lat, p.lon)),
            alt_mm: fix.map(|p| p.alt_mm),
            speed_mm_s: self.live.get(Metric::Speed).map(|v| u32::from(v) * 1000 / 36), // km/h x 10 to mm/s
            hr: byte(Metric::Hr),
            cadence: byte(Metric::Cadence),
            power: self.live.get(Metric::Power),
            temp_c: self.baro.map(|(_, c)| (c / 100).clamp(-100, 100) as i8),
        }
    }

    /// The recorder's card work when due (a checkpoint a minute, the save), the ride left open at boot, and the
    /// backup register that marks a ride open.
    pub(crate) fn step_recorder(&mut self, b: &mut impl Board) {
        if !self.card_writable || self.emmc.lent() {
            return;
        }
        let (local, _) = self.rec_time();
        if self.recorder.phase() != recorder::Phase::Done {
            self.resume_pending = false; // a ride started: its first card work does the boot's first (one writer)
        }
        let o = if self.resume_pending {
            let o = self.recorder.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            // a card that failed at boot is tried again (the marker stays until the ride is found)
            self.resume_pending = matches!(o, recorder::Outcome::Failed(_)) || self.recorder.waiting();
            o
        } else if self.recorder.due() {
            self.recorder.io(&mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Nothing => {}
            recorder::Outcome::Resumed(words, name) => {
                // paused where the last checkpoint left it: Start goes on, a long Start ends and saves it
                self.ride = ride::Ride::resumed(&words);
                let two = |i: usize| name[i].wrapping_sub(b'0') % 10 * 10 + name[i + 1].wrapping_sub(b'0') % 10;
                self.ride_started = ((2000 + u16::from(two(0)), two(2), two(4)), (two(6), two(8)));
                self.history = ride::History::default();
                self.power.new_ride();
                self.notice("RIDE RESUMED");
            }
            recorder::Outcome::Saved(_) => {
                self.notice("RIDE SAVED");
                if self.stock.status() != stock::Status::Idle {
                    self.stock.start(self.date); // the history again, with this ride
                }
            }
            recorder::Outcome::Kept(_) => {
                // not in \Activities: left in \Rustfw\Rides as it was (damaged, or the card kept failing)
                self.notice("SAVE FAILED");
            }
            recorder::Outcome::Failed(_) => {
                // the first failure in a row, then every 5 minutes of them (a try every RETRY passes, 5 s)
                if self.recorder.fails() % 60 == 1 {
                    self.notice("NOT SAVING");
                }
            }
        }
        let open = self.recorder.phase() != recorder::Phase::Done;
        if !self.resume_pending && open != (b.backup_read(RIDE_BKP) == RIDE_OPEN) {
            b.backup_write(RIDE_BKP, if open { RIDE_OPEN } else { 0 });
        }
    }

    /// Once a second (the RTC's second changed): the ride and the history take the readings.
    pub(crate) fn second(&mut self) {
        let input = ride::Input {
            speed_x10: self.live.get(Metric::Speed),
            hr: self.live.get(Metric::Hr),
            cadence: self.live.get(Metric::Cadence),
            power: self.live.get(Metric::Power),
            alt_mm: self.pvt.filter(|p| p.good()).map(|p| p.alt_mm),
        };
        let timer = self.ride.timer_s();
        if let Some(ev) = self.ride.second(&input) {
            self.recorder(ev);
        }
        if self.ride.state() == ride::State::Running {
            let s = &self.settings;
            let due = self.alerts.second(self.ride.timer_s(), input.hr, s.eat_min, s.drink_min, s.hr_alert);
            self.alert = self.alert.or(due);
        }
        // the workout's step clock moves with the ride's timer
        let (ticked, dist, steps) = (self.ride.timer_s() != timer, self.ride.dist_m(), self.plan.steps());
        if let Some(e) = self.player.as_mut().and_then(|p| p.second(steps, ticked, dist)) {
            self.workout_event(e);
        }
        self.power.second(input.power, &self.ride, self.settings.ftp, self.settings.wprime_j);
        let wbal = self.power.wbal_j().map(|j| (j.max(0) / 10).clamp(1, i32::from(u16::MAX)) as u16);
        let dfa = self.hrv.alpha1().filter(|_| input.hr.is_some()).map(|a| (a * 100.0 + 0.5).clamp(1.0, 999.0) as u16);
        let alt = input.alt_mm.map(|mm| (mm / 1000 + ride::ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
        self.history.second([input.power, input.hr, wbal, dfa, alt]);
        // the course followed: where the rider is on it, and what to say
        let fix = self.pvt.filter(|p| p.good());
        let ev = match (self.courses.as_deref_mut(), fix) {
            (Some(c), Some(p)) if c.on.is_some() => c.course.follow(&mut c.at, p.lat, p.lon),
            _ => None,
        };
        if let Some(ev) = ev {
            self.course_event(ev);
        }
    }
}
