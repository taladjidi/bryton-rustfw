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
            self.ride_started = RideStart::new(self.date, self.strip.time);
            self.history = ride::History::default();
            self.alerts = alerts::Alerts::new();
            self.radio.shifts = 0;
            self.radio.wake_rested();
            self.pace = PaceState::new(); // the last ride's plan: its TOP time and W' budget
            let mut track = map::Track::new();
            if let Some(p) = self.gps.pvt.filter(|p| p.good()) {
                track.seed(p.lat, p.lon);
            }
            self.track = track;
            (self.map_zoom, self.map_centre, self.map_whole) = (None, None, false); // a new map: fitted until "+" (review)
            self.new_ride = true;
            self.tlink.pm = power_match::PowerMatch::new();
            let now = self.now_s();
            self.alt.ride_start(now); // a calibration older than three hours is made again from the receiver
        }
        self.tones.play(if ev == ride::Event::Pause { &BEEP_PAUSE } else { &BEEP_START });
        // a new ride: the ready card replaces RIDE STARTED (the recording starts below, at the press, in either case)
        if ev != ride::Event::Start || !self.ready_card() {
            self.say(match ev {
                ride::Event::Start => "RIDE STARTED",
                ride::Event::Pause => "PAUSED: HOLD START TO END",
                _ => "RESUMED",
            });
        }
        if ev == ride::Event::Start {
            self.power.new_ride();
            self.zone_s = ride::ZoneTime::default();
            self.hrtss = hrtss::Hrtss::default();
            self.rec.model = Some(self.model_now()); // kept for the whole ride (app/fitness.rs)
            (self.profile_by_hand, self.profile) = (false, ride::Profile::Road);
            self.choose_profile(); // after the banner above: INDOOR RIDE replaces it
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
        self.recorder(ev);
        // a workout step (or the ramp's end) tells the rider already: no summary then
        let steps = self.plan.workout.steps();
        let told = self.ramp_running() || self.plan.player.is_some_and(|p| p.step(steps).is_some());
        self.next_step();
        if !told {
            self.lap_summary();
        }
    }

    /// An auto lap (AUTO LAP in BIKE) once this lap passed its distance or moving time: the Lap key's path to the
    /// recorder (`Ride::lap`, then `App::recorder`), without moving a workout on.
    pub(crate) fn auto_lap(&mut self) {
        let (m, s) = self.settings.auto_lap_limits();
        if self.ride.lap_reached(m, s)
            && let Some(ev) = self.ride.lap()
        {
            self.recorder(ev);
            self.lap_summary();
        }
    }

    /// The lap that just ended, in a banner of 8 s (or until a key): number, time and distance, then average power
    /// (speed without power) and heart rate.
    fn lap_summary(&mut self) {
        let r = &self.ride;
        let (s, m) = (r.last_lap_s().unwrap_or(0), r.last_lap_dist_m());
        let mut l = Line::<40>::new();
        l.push_str("LAP ");
        l.push_u32(r.laps());
        l.push_str(" ");
        push_clock(&mut l, s);
        l.push_str(" ");
        let imp = self.settings.imperial;
        push_trip(&mut l, m, imp);
        l.push_str(if imp { "MI\n" } else { "KM\n" });
        match r.last_lap_avg_power() {
            Some(w) => {
                l.push_u32(u32::from(w));
                l.push_str("W");
            }
            None => {
                let v = if s > 0 { m * 36 / s } else { 0 }; // km/h x 10
                let v = if imp { mph_x10(v) } else { v };
                l.push_u32(v / 10);
                l.push_str(".");
                l.push_u32(v % 10);
                l.push_str(if imp { "MPH" } else { "KMH" });
            }
        }
        if let Some(hr) = r.last_lap_hr() {
            l.push_str(" ");
            l.push_u32(u32::from(hr));
            l.push_str("BPM");
        }
        self.say_as(l.as_str(), Banner::Summary);
    }

    /// A lap at a workout's step change (and at its start and end), as Garmin and Wahoo record them, so each interval
    /// is a lap in the file (Intervals.icu, Domestique). None if the ride is not running, or if a lap began less than
    /// a second ago: Lap's own lap, which moves the workout on, is not doubled, nor a workout started with its ride.
    pub(crate) fn step_lap(&mut self) {
        if self.ride.lap_s() == 0 {
            return;
        }
        if let Some(ev) = self.ride.lap() {
            self.recorder(ev);
        }
    }

    /// The workout's next step, if one runs (Lap, or NEXT STEP in the ride menu).
    pub(crate) fn next_step(&mut self) {
        if self.ramp_running() {
            return self.finish_ramp(); // Lap in the ramp is its end
        }
        let (steps, dist) = (self.plan.workout.steps(), self.ride.dist_m());
        if let Some(e) = self.plan.player.as_mut().filter(|p| p.step(steps).is_some()).map(|p| p.skip(steps, dist)) {
            self.workout_event(e);
        }
    }

    /// Ends a paused ride (SAVE on the end-ride screen): the recorder saves it, the workout ends with it, and its
    /// card shows (until Back or Start).
    pub(crate) fn end_ride(&mut self) {
        let Some(ev) = self.ride.end() else { return };
        // the trips get it once this ride's save is done ([`SettingsStore::done`])
        self.settings_store.ended(self.ride.dist_m(), self.ride.timer_s());
        self.recorder(ev);
        self.plan.player = None;
        (self.depth, self.screen) = (0, Screen::Summary);
    }

    /// Ends a paused ride and discards it (DISCARD, asked twice): the recorder saves both copies into
    /// `\Rustfw\Discarded` instead (storage/recorder/mod.rs, "A discarded ride": never erased, never in the history), the
    /// workout ends with it, back to the overview.
    pub(crate) fn discard_ride(&mut self) {
        let Some(ev) = self.ride.end() else { return };
        // a ride held behind the last one's save: its end drops it (nothing recorded), and the last one, still in
        // the recorder, is never the one discarded
        let held = [self.rec.clean.pending(), self.rec.full.pending()];
        self.recorder(ev);
        // too late once the save has begun (a full card stopped the ride): it is in \Activities, so say so
        let took = held[0] || self.rec.clean.discard() || !self.card_writable;
        if !held[1] {
            self.rec.full.discard();
        }
        self.plan.player = None;
        self.plan.played = None; // a discarded ride is not a day's workout done
        (self.depth, self.screen) = (0, Screen::Main);
        self.say(if took { "DISCARDING" } else { "ALREADY SAVED" });
    }

    /// GIVE UP SAVE (SYSTEM, only while a save is stuck: storage/recorder/mod.rs, "GIVE UP SAVE"): two banners of explanation,
    /// then asked twice, each Start on the row going on and anything else (Back) dropping it; then both copies'
    /// stuck saves given up, the outcome said. Refused while the card is lent (the recorder cannot reach it), and
    /// while a ride runs that is recorded: one held behind the stuck save has nothing recorded, so it is let
    /// through and starts recording here (the rider's way to record the ride under way on a failing card).
    pub(crate) fn give_up_save(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        const ASK: [&str; 4] = [
            "THE RIDE STAYS ON\nTHE CARD, UNNAMED.",
            "COPY THE CARD FIRST\n(USB, READ ONLY).",
            "GIVE UP SAVE?\nSTART: YES BACK: NO",
            "SURE: GIVE UP SAVE?\nSTART: YES BACK: NO",
        ];
        if self.emmc.lent() {
            return self.say("USB DRIVE ON");
        }
        if self.ride.state() != ride::State::Idle && !self.rec.clean.held() {
            return self.say("END THE RIDE FIRST");
        }
        let step = if self.confirm.take() == Some((menu, sel)) { usize::from(self.give_up_step) + 1 } else { 0 };
        if let Some(text) = ASK.get(step) {
            (self.confirm, self.give_up_step) = (Some((menu, sel)), step as u8);
            return self.say(text);
        }
        self.give_up_step = 0;
        let (local, _) = self.rec_time();
        let clean = self.rec.clean.give_up_save(&mut emmc::Volume(&mut self.emmc, b), local);
        let full = self.rec.full.give_up_save(&mut emmc::Volume(&mut self.emmc, b), local);
        // the boot's recovery over too: the record read by the next ride's first card work (one writer, one ride)
        if self.rec.clean.phase() == recorder::Phase::Done {
            self.rec.resume_pending = false;
        }
        if self.rec.full.phase() == recorder::Phase::Done {
            self.rec.full_pending = false;
        }
        use recorder::{GaveUp, Outcome};
        match clean.or(full) {
            Some(GaveUp::Named(Outcome::Saved(_))) => {
                self.settings_store.done(true);
                self.notice("RIDE SAVED");
            }
            Some(GaveUp::Named(Outcome::Discarded(_))) => {
                self.settings_store.done(false);
                self.notice("RIDE DISCARDED");
            }
            Some(GaveUp::Named(_)) => {
                self.settings_store.done(false);
                self.notice("KEPT IN RUSTFW/RIDES");
            }
            Some(GaveUp::Rescue(_)) => {
                self.settings_store.done(false);
                self.notice("KEPT IN RUSTFW/RESCUE");
            }
            Some(GaveUp::Lost) => {
                self.settings_store.done(false);
                self.alarm_notice("GIVEN UP: COPY THE CARD");
            }
            None => {}
        }
    }

    /// The end-ride screen's numbers: the timer and the distance.
    pub(crate) fn end_stats(&self) -> Line<24> {
        let mut l = Line::new();
        push_clock(&mut l, self.ride.timer_s());
        l.push_str("  ");
        let imp = self.settings.imperial;
        push_trip(&mut l, self.ride.dist_m(), imp);
        l.push_str(if imp { " MI" } else { " KM" });
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
                        ui::draw_end_button(&mut Panel(b, area, self.look()), j, l, j == k);
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
    // The recorder (storage/recorder/mod.rs): events into its RAM buffer here, its card work in step_recorder
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
        self.profile_to_recorders(); // a change while paused, before the pause's or the end's messages
        match ev {
            ride::Event::Start => {
                // a ride the boot has not reopened yet (the card failed then) is resumed in this one's place
                self.rec.clean.start(t, recorder::ride_name(local), self.rec.resume_pending);
                if !self.rec.clean.starting() {
                    self.alarm_notice("NOT RECORDING");
                } else {
                    if self.rec.clean.held() {
                        // behind the last ride's save, which the card keeps failing: tried again, this one after it
                        self.alarm_notice("NOT RECORDING: CARD");
                    }
                    // (a ride started while the last one is being saved waits for that save: storage/recorder/mod.rs `start`)
                    // no full copy of a ride the clean one does not record
                    let resume = self.rec.resume_pending || self.rec.full_pending;
                    self.rec.full.start_after(&self.rec.clean, t, recorder::ride_name(local), resume);
                }
            }
            ride::Event::Second => {
                let s = self.sample(t);
                (self.tlink.cmds, self.tlink.fails) = (0, 0); // in this record
                self.rec.clean.second(&s, words);
                self.rec.full.second(&s, words);
            }
            ride::Event::Pause => {
                self.rec.clean.pause(t, words);
                self.rec.full.pause(t, words);
            }
            ride::Event::Resume => {
                self.rec.clean.resume(t);
                self.rec.full.resume(t);
            }
            ride::Event::Lap => {
                self.rec.clean.lap(t);
                self.rec.full.lap(t);
            }
            ride::Event::End => {
                self.rec.clean.end(t, words);
                self.rec.full.end(t, words);
            }
        }
    }

    /// Local time now (the shown clock as last read, app/clock.rs) and its FIT time (UTC: the RTC less the offset
    /// learnt at a fix, whatever TIME ZONE shows; the RTC as is before one), less the seconds being caught up
    /// ([`App::step`]).
    pub(crate) fn rec_time(&self) -> (recorder::Local, u32) {
        let ((y, mo, d), (h, mi)) = (self.date, self.strip.time);
        let local = (y, mo, d, h, mi, self.seconds);
        let offset = self.shown_offset_s() + i64::from(self.rec.lag_s);
        (local, fit::fit_time(from_epoch_s(epoch_s(local) - offset)))
    }

    /// This second's record: the fix, the sensors, the barometer's temperature.
    pub(crate) fn sample(&self, time: u32) -> fit::Sample {
        let fix = self.gps.pvt.filter(|p| p.good());
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
            artifacts_pct: self.hrv.artifacts().filter(|_| hr),
            ascent_m: Some(self.ride.ascent_m()),
            trainer: self.trainer_sample(),
            profile: self.ride_profile(),
            by_hand: self.profile_by_hand,
        }
    }

    /// The trainer this second for the full copy (fit.rs, "The trainer"), while it is connected (heard last pass).
    fn trainer_sample(&self) -> Option<fit::TrainerSample> {
        if !self.tlink.up {
            return None;
        }
        let (t, d) = (&self.trainer, self.trainer_drive());
        let r = &t.reading;
        let meter = self.radio.heard_ms.get(Kind::Power as usize).is_some_and(|&ms| ms < sensors::STALE_MS);
        Some(fit::TrainerSample {
            // what it should hold (cleared by no resend or loss of the link); whether it went out is `cmds`
            target_w: (t.engaged(&d) && t.mode(&d) == trainer::Mode::Erg).then(|| t.target_w(&d)),
            trainer_w: r.power,
            // a meter heard is what the live power is (app/trainer.rs `fold_trainer`)
            meter_w: self.live.get(Metric::Power).filter(|_| meter),
            fe_state: r.state,
            status: r.status << 4 | r.limits,
            ble: self.ftms_on(),
            cmds: self.tlink.cmds,
            ack_fails: self.tlink.fails,
            device: r.maker.map(|maker| fit::TrainerDevice {
                maker,
                model: r.model,
                hw: r.hw,
                serial: r.serial,
                sw_x100: r.sw_x100,
            }),
        })
    }

    /// The profile in force to both copies' writers: the samples carry it only on running seconds, so a change
    /// while paused would miss the end and the recovery record (the review of the ride profile).
    /// The ride's own profile, not [`App::indoors`]: at the End event the ride is idle already, where that is the
    /// next ride's choice. A recorder without a ride open ignores it.
    fn profile_to_recorders(&mut self) {
        let (profile, by_hand) = (self.profile, self.profile_by_hand);
        self.rec.clean.set_profile(profile, by_hand);
        self.rec.full.set_profile(profile, by_hand);
    }

    /// The recorder's card work when due (a checkpoint a minute, the save), the ride left open at boot, and the
    /// backup register that marks a ride open.
    pub(crate) fn step_recorder(&mut self, b: &mut impl Board) {
        if self.emmc.lent() {
            return; // a read-only build has no ride here: [`App::recorder`] never starts one, nor is one resumed
        }
        let (local, _) = self.rec_time();
        self.profile_to_recorders(); // before a checkpoint (a ride switched off or cut while paused)
        if self.rec.clean.phase() != recorder::Phase::Done {
            self.rec.resume_pending = false; // a ride started: its first card work does the boot's first (one writer)
        }
        // the two copies checkpoint together: the clean one never behind (storage/recorder/mod.rs, "Two copies")
        self.rec.clean.keep_up(&self.rec.full);
        self.rec.full.keep_up(&self.rec.clean);
        let was_held = self.rec.clean.held();
        let recovering = self.rec.resume_pending;
        let o = if self.rec.resume_pending {
            let o = self.rec.clean.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            // a card that failed at boot is tried again (the marker stays until the ride is found)
            self.rec.resume_pending = matches!(o, recorder::Outcome::Failed(_)) || self.rec.clean.waiting();
            o
        } else if self.rec.clean.due() {
            self.rec.clean.io(&mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Nothing => {}
            recorder::Outcome::Resumed(words, name) => {
                // paused where the last checkpoint left it: Start goes on, a long Start ends and saves it
                self.ride = ride::Ride::resumed(&words);
                // its profile from the record (an older one: ROAD, automatic), by hand if the rider set it so;
                // automatic, a trainer silent for TRAINER_QUIET_S after the restart turns it back to ROAD or MTB
                self.profile = self.rec.clean.profile();
                (self.profile_by_hand, self.tlink.quiet_s) = (self.rec.clean.by_hand(), 0);
                let two = |i: usize| name[i].wrapping_sub(b'0') % 10 * 10 + name[i + 1].wrapping_sub(b'0') % 10;
                self.ride_started = RideStart::new((2000 + u16::from(two(0)), two(2), two(4)), (two(6), two(8)));
                self.history = ride::History::default();
                // the recovery record keeps the ride's totals, not power.rs's sums: the end card's NP, TSS and kJ
                // cover the part after the restart only (the file has every second's power: the history's and a
                // computer's cover the whole ride)
                self.power.new_ride();
                // the zone times and the hrTSS restart after a resume, as the laps do
                self.zone_s = ride::ZoneTime::default();
                self.hrtss = hrtss::Hrtss::default();
                self.rec.model = Some(self.model_now()); // what the ride has now: the scan may not have read yet
                let now = self.now_s();
                self.alt.ride_start(now);
                self.notice("RIDE RESUMED");
            }
            recorder::Outcome::Saved(_) => {
                self.notice("RIDE SAVED");
                self.settings_store.done(true);
                if !recovering {
                    // this run's ride, read back whole and named in \Activities: its segment efforts may be written
                    // (app/segments.rs, once both copies are done); never an earlier run's ride the boot saved
                    self.segments_ride_saved();
                }
                if let Some(t) = self.thresholds.as_deref_mut() {
                    t.reset(); // its full copy read too, when FITNESS next shows
                }
                if self.stock.status() != stock::Status::Idle {
                    self.rescan(); // the history again, with this ride
                }
            }
            recorder::Outcome::Discarded(_) => {
                self.settings_store.done(false);
                self.notice("RIDE DISCARDED"); // the history as it was: no scan
            }
            recorder::Outcome::Kept(_) => {
                self.settings_store.done(false);
                // not in \Activities: left in \Rustfw\Rides as it was, or named there again (damaged, or the card
                // kept failing)
                self.alarm_notice("SAVE FAILED: KEPT IN RUSTFW/RIDES");
            }
            recorder::Outcome::Stopped(e) => {
                // the ride's recording stops here (said once): what reached the card is saved next
                self.alarm_notice(if e == fat::Error::Refused { "CARD FULL" } else { "CARD ERROR" });
            }
            recorder::Outcome::Failed(_) => {
                // the first failure in a row, then every 5 minutes of them (a try every RETRY passes, 5 s)
                if self.rec.clean.fails() % 60 == 1 && was_held == self.rec.clean.held() {
                    self.alarm_notice(if self.rec.clean.held() { "NOT RECORDING: CARD" } else { "NOT SAVING" });
                }
            }
        }
        if !was_held && self.rec.clean.held() {
            self.alarm_notice("NOT RECORDING: CARD"); // the ride started behind the last one's save, now stuck
        }
        self.step_full(b, local);
        // a card error of either copy (GIVE UP SAVE's too) since the last pass: a FAT sector may have reached the
        // first copy only, so the next settings.bin or pages.bin write compares the copies first
        let errors = self.rec.clean.errors.saturating_add(self.rec.full.errors);
        if errors != self.rec.errors_seen {
            self.rec.errors_seen = errors;
            self.fats_unsure();
        }
        let open = self.rec.clean.phase() != recorder::Phase::Done || self.rec.full.phase() != recorder::Phase::Done;
        // a build that cannot write the card never clears the marker of a ride left open
        if self.card_writable && !self.rec.resume_pending && open != (b.backup_read(RIDE_BKP) == RIDE_OPEN) {
            b.backup_write(RIDE_BKP, if open { RIDE_OPEN } else { 0 });
        }
    }

    /// The full copy's card work, after the clean one's (storage/recorder/mod.rs, "Two copies"): its ride left open is looked
    /// for once the clean one's has been, and whatever it does, it never stops the clean one; a full copy that
    /// cannot be saved is said.
    fn step_full(&mut self, b: &mut impl Board, local: recorder::Local) {
        if self.rec.full.phase() != recorder::Phase::Done {
            self.rec.full_pending = false;
        }
        let o = if self.rec.full_pending && !self.rec.resume_pending {
            let o = self.rec.full.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            self.rec.full_pending = matches!(o, recorder::Outcome::Failed(_)) || self.rec.full.waiting();
            o
        } else if self.rec.full.due() {
            self.rec.full.io_after(&self.rec.clean, &mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Resumed(..) => self.rec.full.resumed_after(&self.rec.clean),
            recorder::Outcome::Kept(_) => self.notice("NO FULL COPY"),
            _ => {}
        }
    }

    /// Once a second (the RTC's second changed): the ride and the history take the readings.
    pub(crate) fn second(&mut self) {
        self.alt_second();
        self.profile_second();
        self.start_reminder_second();
        if self.ride.state() != ride::State::Idle {
            self.alt_follow();
        }
        let alt = self.alt_now();
        self.live.snapshot(); // the pages show this second's readings, not each broadcast's
        self.tlink.second = true; // and the trainer screen
        let input = ride::Input {
            speed_x10: self.live.get(Metric::Speed),
            hr: self.live.get(Metric::Hr),
            cadence: self.live.get(Metric::Cadence),
            power: self.live.get(Metric::Power),
            balance: self.live.get(Metric::Balance),
            alt_mm: alt.0,
            alt_ref: alt.1,
            alt_corr_mm: self.alt.corr(),
            profile: self.ride_profile(),
            pause_kmh: AUTO_PAUSE_KMH.get(self.settings.auto_pause).copied().unwrap_or(0),
        };
        let timer = self.ride.timer_s();
        if let Some(ev) = self.ride.second(&input) {
            self.recorder(ev);
        }
        if self.ride.state() == ride::State::Running {
            self.auto_lap();
            let s = &self.settings;
            let due = self.alerts.second(self.ride.timer_s(), input.hr, s.eat_min, s.drink_min, s.hr_alert);
            self.alert.push(due);
            let r = alerts::Reading {
                power: input.power,
                cadence: input.cadence,
                speed_x10: input.speed_x10,
                hr: input.hr,
                dfa_x100: self
                    .hrv
                    .alpha1()
                    .filter(|_| input.hr.is_some())
                    .map(|a| (a * 100.0 + 0.5).clamp(0.0, 999.0) as u16),
                dist_m: self.ride.dist_m(),
                kcal: self.power.kcal(),
                kj: self.power.kj(),
            };
            self.alerts.limits(&r, &self.settings.limits, &mut self.alert);
            // ERG: the trainer holds the watts, the resistance climbs as the rider slows
            let slow = self.alerts.cadence(input.cadence, self.erg_on());
            self.alert.push(slow);
            // the sunset alert: the minutes left to today's sunset by the clock the status bar shows (no fix, no times)
            let now = u32::from(self.strip.time.0) * 60 + u32::from(self.strip.time.1);
            let left = super::fields::minutes_to_sunset(now, self.sun_times());
            let lead = crate::SUNSET_LEAD_MIN.get(self.settings.sunset_alert).copied().unwrap_or(0);
            let sun = self.alerts.sunset(left, lead);
            self.alert.push(sun);
        } else {
            self.alerts.reset_limits(); // a pause or a stop starts the holds again
        }
        // the workout's step clock moves with the ride's timer
        let (ticked, dist, steps) = (self.ride.timer_s() != timer, self.ride.dist_m(), self.plan.workout.steps());
        self.plan.ran_s += u32::from(ticked && self.plan.player.is_some_and(|p| p.step(steps).is_some()));
        if let Some(e) = self.plan.player.as_mut().and_then(|p| p.second(steps, ticked, dist)) {
            self.workout_event(e);
        }
        self.ramp_second(ticked);
        let (cp, wprime) = self.cp_model();
        self.power.second(input.power, &self.ride, cp, wprime);
        let (ftp, max_hr) = (self.settings.ftp, self.settings.max_hr);
        let zt = self.settings.zones();
        // each bar zones exactly the number its box shows: live power, the 3, 10 and 30 s averages, heart rate
        let p = &self.power;
        let watts = [self.power_now(), p.rolling(0), p.rolling(1), p.rolling(2)];
        let (pm, hm) = ((ftp / 50).max(1), (max_hr / 100).max(1)); // 2 % of FTP, 1 % of max HR
        for (hold, w) in self.zones.iter_mut().zip(watts) {
            hold.update(w, pm, |w| zt.power_zone(w));
        }
        if let Some(hold) = self.zones.get_mut(4) {
            hold.update(self.live.shown(Metric::Hr), hm, |v| zt.hr_zone(v));
        }
        // the time in each zone: the moving seconds, in the zone the bars show (a Start or a resume clears it)
        if self.ride.state() == ride::State::Running && self.ride.moving() {
            self.zone_s.second(self.zone_now(4), self.zone_now(0));
            self.hrtss.second(self.live.shown(Metric::Hr), max_hr, self.settings.female);
        }
        let wbal = self.power.wbal_j().map(|j| (j.max(0) / 10).clamp(1, i32::from(u16::MAX)) as u16);
        let dfa = self.hrv.alpha1().filter(|_| input.hr.is_some()).map(|a| (a * 100.0 + 0.5).clamp(1.0, 999.0) as u16);
        let alt = input.alt_mm.map(|mm| (mm / 1000 + ride::ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
        self.history.second([input.power, input.hr, wbal, dfa, alt]);
        self.history.dist_second(self.ride.dist_m(), alt);
        // the course followed: where the rider is on it, and what to say
        let fix = self.gps.pvt.filter(|p| p.good());
        let ev = match (self.courses.as_deref_mut(), fix) {
            (Some(c), Some(p)) if c.on.is_some() => c.course.follow(&mut c.at, p.lat, p.lon),
            _ => None,
        };
        if let Some(ev) = ev {
            self.course_event(ev);
        }
        self.segment_fix();
        self.ahead_second();
        self.pace_second(); // after the fix: the position is this second's
    }
}
