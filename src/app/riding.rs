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
            self.shifts = 0;
            let mut track = map::Track::new();
            if let Some(p) = self.pvt.filter(|p| p.good()) {
                track.seed(p.lat, p.lon);
            }
            self.track = track;
            (self.map_zoom, self.map_centre) = (None, None); // a new map: fitted until "+" (review)
            self.new_ride = true;
            let now = self.now_s();
            self.alt.ride_start(now); // a calibration older than three hours is made again from the receiver
        }
        self.tones.play(if ev == ride::Event::Pause { &BEEP_PAUSE } else { &BEEP_START });
        self.say(match ev {
            ride::Event::Start => "RIDE STARTED",
            ride::Event::Pause => "PAUSED: HOLD START TO END",
            _ => "RESUMED",
        });
        if ev == ride::Event::Start {
            self.power.new_ride();
        }
        self.recorder(ev);
    }

    /// Start on a ride page or the overview (the key, a shifter's press): [`App::start_pause`]; from the overview
    /// the first ride page then, on a page that page again for a new ride (its map and graphs).
    pub(crate) fn start_key(&mut self, b: &mut impl Board) {
        self.start_pause();
        let new = core::mem::take(&mut self.new_ride);
        if self.screen == Screen::Main {
            self.screen = Screen::Ride(0);
        } else if !new {
            return;
        }
        self.show(b);
    }

    /// A lap, if the ride runs (Lap on a ride page, a shifter's press): said, recorded, and a workout goes on to
    /// its next step (an open step ends here).
    pub(crate) fn lap(&mut self) {
        let Some(ev) = self.ride.lap() else { return };
        self.say("LAP");
        self.recorder(ev);
        self.next_step();
    }

    /// The workout's next step, if one runs (Lap, or NEXT STEP in the ride menu).
    pub(crate) fn next_step(&mut self) {
        let (steps, dist) = (self.plan.steps(), self.ride.dist_m());
        if let Some(e) = self.player.as_mut().filter(|p| p.step(steps).is_some()).map(|p| p.skip(steps, dist)) {
            self.workout_event(e);
        }
    }

    /// Ends a paused ride (SAVE on the end-ride screen): the recorder saves it, the workout ends with it, and its
    /// card shows (until Back or Start).
    pub(crate) fn end_ride(&mut self) {
        let Some(ev) = self.ride.end() else { return };
        self.recorder(ev);
        self.player = None;
        (self.depth, self.screen) = (0, Screen::Summary);
    }

    /// Ends a paused ride and discards it (DISCARD, asked twice): the recorder saves both copies into
    /// `\Rustfw\Discarded` instead (recorder.rs, "A discarded ride": never erased, never in the history), the
    /// workout ends with it, back to the overview.
    pub(crate) fn discard_ride(&mut self) {
        let Some(ev) = self.ride.end() else { return };
        self.recorder(ev);
        // too late once the save has begun (a full card stopped the ride): it is in \Activities, so say so
        let took = self.recorder.discard() || !self.card_writable;
        self.full.discard();
        self.player = None;
        (self.depth, self.screen) = (0, Screen::Main);
        self.say(if took { "DISCARDING" } else { "ALREADY SAVED" });
    }

    /// The end-ride screen's numbers: the timer and the distance.
    pub(crate) fn end_stats(&self) -> Line<24> {
        let mut l = Line::new();
        push_clock(&mut l, self.ride.timer_s());
        l.push_str("  ");
        push_km(&mut l, self.ride.dist_m());
        l.push_str(" KM");
        l
    }

    /// An input on the end-ride screen or its question (ui::draw_end): Page and Lap move between the buttons, no
    /// wrap (only the two that change drawn again), Start acts on the one selected, a tap on a button acts on it,
    /// Back (or a swipe right) leaves the ride paused ("PAUSED"), or is KEEP. RESUME restarts the timer. While the
    /// screen is up the ride stays paused and its recording as it was (a reset then resumes it paused, as ever).
    pub(crate) fn end_input(&mut self, b: &mut impl Board, i: Input) {
        let (discard, sel) = match self.screen {
            Screen::End(k) => (false, k),
            Screen::Discard(k) => (true, k),
            _ => return,
        };
        if self.ride.state() != ride::State::Paused {
            self.leave_menus(); // resumed meanwhile (a shifter's Start): nothing to end
            return self.show(b);
        }
        let labels = ui::end_labels(discard);
        let k = match i {
            Input::Next | Input::Prev => {
                // the next button that way, none past the ends (as the menus: a held key stops there)
                let mut k = sel;
                let k = loop {
                    k = if i == Input::Next { k + 1 } else { k.checked_sub(1).unwrap_or(labels.len()) };
                    match labels.get(k) {
                        Some(Some(_)) => break k,
                        Some(None) => {}
                        None => return,
                    }
                };
                self.screen = if discard { Screen::Discard(k) } else { Screen::End(k) };
                let area = self.area();
                for j in [sel, k] {
                    if let Some(Some(l)) = labels.get(j) {
                        ui::draw_end_button(&mut Panel(b, area), j, l, j == k);
                    }
                }
                return;
            }
            Input::Select => sel,
            Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)) if !discard => {
                self.leave_menus();
                self.say("PAUSED"); // a manual pause never resumes by itself: say it stays
                return self.show(b);
            }
            Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)) => 2, // KEEP
            Input::Touch(Gesture::Tap(x, y)) => match ui::end_hit(&labels, x, y) {
                Some(k) => k,
                None => return,
            },
            _ => return,
        };
        self.screen = if discard { Screen::Discard(k) } else { Screen::End(k) }; // KEEP comes back to it
        match (discard, k) {
            (false, 0) => {
                self.start_pause(); // RESUME: the timer again, on the page the ride was ended from
                self.leave_menus();
            }
            (false, 1) => self.end_ride(),
            (false, _) => self.open(Screen::Discard(2)), // asked, KEEP selected
            (true, 0) => self.discard_ride(),
            (true, _) => self.close(), // KEEP: back to the end-ride screen
        }
        self.show(b);
    }

    // -----------------------------------------------------------------------------------------------------------
    // The recorder (recorder.rs): events into its RAM buffer here, its card work in step_recorder
    // -----------------------------------------------------------------------------------------------------------

    /// The ride recorder's one entry point: every [`ride::Event`] goes through here, in order. RAM only: the card
    /// work is [`App::step_recorder`]'s, at the end of the pass.
    pub(crate) fn recorder(&mut self, ev: ride::Event) {
        if !self.card_writable {
            // a build that may not write the card (emmc::Card::write refuses it): no recorder at all (it would read
            // both FATs at each try only to fail: about 2 s of a stopped loop every 5 s, the second review found),
            // said once a ride
            if ev == ride::Event::Start {
                self.alarm_notice("NOT RECORDING");
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
                    self.alarm_notice("NOT RECORDING"); // the last ride's save is still failing
                } else {
                    // no full copy of a ride the clean one does not record
                    self.full.start(t, recorder::ride_name(local), self.resume_pending || self.full_pending);
                }
            }
            ride::Event::Second => {
                let s = self.sample(t);
                self.recorder.second(&s, words);
                self.full.second(&s, words);
            }
            ride::Event::Pause => {
                self.recorder.pause(t, words);
                self.full.pause(t, words);
            }
            ride::Event::Resume => {
                self.recorder.resume(t);
                self.full.resume(t);
            }
            ride::Event::Lap => {
                self.recorder.lap(t);
                self.full.lap(t);
            }
            ride::Event::End => {
                self.recorder.end(t, words);
                self.full.end(t, words);
            }
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
        let hr = self.live.get(Metric::Hr).is_some();
        fit::Sample {
            time,
            pos: fix.map(|p| (p.lat, p.lon)),
            alt_mm: self.alt_now().0,
            speed_mm_s: self.live.get(Metric::Speed).map(|v| u32::from(v) * 1000 / 36), // km/h x 10 to mm/s
            hr: byte(Metric::Hr),
            cadence: byte(Metric::Cadence),
            power: self.live.get(Metric::Power),
            temp_c: self.baro.map(|(_, c)| (c / 100).clamp(-100, 100) as i8),
            // the strap's beats: only while it is in (hrv.rs keeps its last values)
            alpha1_milli: self.hrv.alpha1().filter(|_| hr).map(|a| (a * 1000.0 + 0.5).clamp(0.0, 65_534.0) as u16),
            rmssd_ms: self.hrv.rmssd().filter(|_| hr),
            ascent_m: Some(self.ride.ascent_m()),
        }
    }

    /// The recorder's card work when due (a checkpoint a minute, the save), the ride left open at boot, and the
    /// backup register that marks a ride open.
    pub(crate) fn step_recorder(&mut self, b: &mut impl Board) {
        if self.emmc.lent() {
            return; // a read-only build has no ride here: [`App::recorder`] never starts one, nor is one resumed
        }
        let (local, _) = self.rec_time();
        if self.recorder.phase() != recorder::Phase::Done {
            self.resume_pending = false; // a ride started: its first card work does the boot's first (one writer)
        }
        // the two copies checkpoint together: the clean one never behind (recorder.rs, "Two copies")
        self.recorder.keep_up(&self.full);
        self.full.keep_up(&self.recorder);
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
                let now = self.now_s();
                self.alt.ride_start(now);
                self.notice("RIDE RESUMED");
            }
            recorder::Outcome::Saved(_) => {
                self.notice("RIDE SAVED");
                if self.stock.status() != stock::Status::Idle {
                    self.stock.start(self.date); // the history again, with this ride
                }
            }
            recorder::Outcome::Discarded(_) => self.notice("RIDE DISCARDED"), // the history as it was: no scan
            recorder::Outcome::Kept(_) => {
                // not in \Activities: left in \Rustfw\Rides as it was (damaged, or the card kept failing)
                self.alarm_notice("SAVE FAILED");
            }
            recorder::Outcome::Stopped(e) => {
                // the ride's recording stops here (said once): what reached the card is saved next
                self.alarm_notice(if e == fat::Error::Refused { "CARD FULL" } else { "CARD ERROR" });
            }
            recorder::Outcome::Failed(_) => {
                // the first failure in a row, then every 5 minutes of them (a try every RETRY passes, 5 s)
                if self.recorder.fails() % 60 == 1 {
                    self.alarm_notice("NOT SAVING");
                }
            }
        }
        self.step_full(b, local);
        let open = self.recorder.phase() != recorder::Phase::Done || self.full.phase() != recorder::Phase::Done;
        if !self.resume_pending && open != (b.backup_read(RIDE_BKP) == RIDE_OPEN) {
            b.backup_write(RIDE_BKP, if open { RIDE_OPEN } else { 0 });
        }
    }

    /// The full copy's card work, after the clean one's (recorder.rs, "Two copies"): its ride left open is looked
    /// for once the clean one's has been, and whatever it does, it never stops the clean one; a full copy that
    /// cannot be saved is said.
    fn step_full(&mut self, b: &mut impl Board, local: recorder::Local) {
        if self.full.phase() != recorder::Phase::Done {
            self.full_pending = false;
        }
        let o = if self.full_pending && !self.resume_pending {
            let o = self.full.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            self.full_pending = matches!(o, recorder::Outcome::Failed(_)) || self.full.waiting();
            o
        } else if self.full.due() {
            self.full.io_after(&self.recorder, &mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Resumed(..) => self.full.resumed_after(&self.recorder),
            recorder::Outcome::Kept(_) => self.notice("NO FULL COPY"),
            _ => {}
        }
    }

    /// Once a second (the RTC's second changed): the ride and the history take the readings.
    pub(crate) fn second(&mut self) {
        self.alt_second();
        if self.ride.state() != ride::State::Idle {
            self.alt_follow();
        }
        let alt = self.alt_now();
        self.live.snapshot(); // the pages show this second's readings, not each broadcast's
        let input = ride::Input {
            speed_x10: self.live.get(Metric::Speed),
            hr: self.live.get(Metric::Hr),
            cadence: self.live.get(Metric::Cadence),
            power: self.live.get(Metric::Power),
            alt_mm: alt.0,
            alt_ref: alt.1,
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
        let (ftp, max_hr) = (self.settings.ftp, self.settings.max_hr);
        let (p, hr) = (&self.power, self.live.shown(Metric::Hr));
        let zones = [p.rolling(0), p.rolling(1), p.rolling(2)].map(|w| w.map(|w| ride::power_zone(w, ftp)));
        for (hold, z) in self.zones.iter_mut().zip(zones.into_iter().chain([hr.map(|v| ride::hr_zone(v, max_hr))])) {
            hold.second(z);
        }
        let wbal = self.power.wbal_j().map(|j| (j.max(0) / 10).clamp(1, i32::from(u16::MAX)) as u16);
        let dfa = self.hrv.alpha1().filter(|_| input.hr.is_some()).map(|a| (a * 100.0 + 0.5).clamp(1.0, 999.0) as u16);
        let alt = input.alt_mm.map(|mm| (mm / 1000 + ride::ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
        self.history.second([input.power, input.hr, wbal, dfa, alt]);
        self.history.dist_second(self.ride.dist_m(), alt);
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
