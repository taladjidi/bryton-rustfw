//! The ride from the app's side: Start / pause from a key, the second tick that feeds the ride, the alerts, the
//! workout, power, the history and the course, and the glue to the recorder (each ride event, the samples, the
//! checkpoints and the resume at the first step).

use crate::*;
use ui::card::Save;

/// The seconds before a pass, up to this many, that a long pass (a stall) ticks one by one with the pass's readings
/// (app/pass.rs); the older ones of a longer stall go on the timer at once, without records. 5: the FIT writer's
/// distance takes at most 5 s between two records (fit.rs `record`).
pub const REPEAT_S: u8 = 5;

/// A step of the RTC forward by more than this is a clock step, not a stall: one second counted, as for a step back.
/// No stall of a running unit lasts an hour (the review of 2026-11-02), and a ride left stopped longer is not
/// resumed either (`recorder::RESUME_S`, the same hour).
pub const CLOCK_STEP_S: u32 = 3600;

/// Seconds in a row an auto-pause or its end holds before it is said.
const AUTO_SAY_S: u8 = 3;

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
            (self.map_view.zoom, self.map_view.centre, self.map_view.whole) = (None, None, false); // a new map: fitted until "+" (review)
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
            self.reset_ride_curve();
            self.power.new_ride();
            self.climbs_done.clear();
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
        if self.ride_profile().auto_lap()
            && self.ride.lap_reached(m, s)
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
        // recorded: a ride the recorder holds (not one started and ended behind the last one's save, nothing of it kept)
        let recorded = matches!(self.rec.clean.phase(), recorder::Phase::Starting | recorder::Phase::Recording)
            && !self.rec.clean.pending();
        // the trips get it once this ride's save is done ([`SettingsStore::done`])
        self.settings_store.ended(self.ride.dist_m(), self.ride.timer_s());
        self.recorder(ev);
        self.plan.player = None;
        self.save = if !recorded {
            Save::Failed("NOT RECORDED")
        } else if !self.card_writable {
            Save::Failed("NOT SAVED: CARD")
        } else {
            Save::Saving
        };
        (self.summary_page, self.laps_wanted, self.save_drawn) = (0, false, Save::Saving);
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
        let step =
            if self.menu.confirm.take() == Some((menu, sel)) { usize::from(self.menu.give_up_step) + 1 } else { 0 };
        if let Some(text) = ASK.get(step) {
            (self.menu.confirm, self.menu.give_up_step) = (Some((menu, sel)), step as u8);
            return self.say(text);
        }
        self.menu.give_up_step = 0;
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
                self.save = ui::card::Save::Saved;
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
                self.radio.wake_rested(); // the channels resting search again, as at a Start
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
        let local = self.rec_local();
        let offset = self.shown_offset_s() + i64::from(self.rec.lag_s);
        (local, fit::fit_time(from_epoch_s(epoch_s(local) - offset)))
    }

    /// The local time alone, as [`App::rec_time`]'s: the date and time read, no division (every pass's users).
    pub(crate) fn rec_local(&self) -> recorder::Local {
        let ((y, mo, d), (h, mi)) = (self.date, self.strip.time);
        (y, mo, d, h, mi, self.seconds)
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
            auto_paused: !self.ride.moving(), // the lap's averages over the moving seconds, as its banner
        }
    }

    /// The trainer this second for the full copy (fit.rs, "The trainer"), while it is connected (heard last pass).
    fn trainer_sample(&self) -> Option<fit::TrainerSample> {
        if !self.tlink.up {
            return None;
        }
        let (t, d) = (&self.trainer, self.trainer_drive());
        let r = &t.reading;
        let meter = self.radio.heard(Kind::Power);
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

    /// At a boot without the ride marker: whether the backup domain was reset (none of this firmware's registers
    /// set: a battery run flat clears them with the marker and the RTC, RM0399) and the card's record says a ride is
    /// left unfinished: `Some(true)`; `None` when the card did not answer (unknown: the resume stays pending, as the
    /// marker's does on a failing card, but stock's settings are still imported once it answers with no ride). The
    /// golden rule's resume on boot then takes the marker's path: the ride reopened at the first step, AUTO OFF held
    /// meanwhile. One read of each record at such a boot (`record_open`, read only), none at any other; before it a
    /// ride left this way waited for Start (2026-11-02).
    #[inline(never)] // its card reading in its own frame, not begin's
    pub(crate) fn left_by_a_power_loss(&mut self, b: &mut impl Board) -> Option<bool> {
        if own_registers(b) {
            return Some(false);
        }
        let clean = self.rec.clean.record_open(&mut emmc::Volume(&mut self.emmc, b));
        if clean == Some(true) {
            return clean;
        }
        let full = self.rec.full.record_open(&mut emmc::Volume(&mut self.emmc, b));
        match (clean, full) {
            (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        }
    }

    /// The recorder's card work when due (a checkpoint a minute, the save), the ride left open at boot, and the
    /// backup register that marks a ride open.
    pub(crate) fn step_recorder(&mut self, b: &mut impl Board) {
        if self.emmc.lent() {
            return; // a read-only build has no ride here: [`App::recorder`] never starts one, nor is one resumed
        }
        let local = self.rec_local();
        self.profile_to_recorders(); // before a checkpoint (a ride switched off or cut while paused)
        // the totals after this pass's second (power.rs runs after the recorder's second): a resume goes on from them
        let more = self.ride.more_words(&self.power, &self.zone_s, &self.hrtss);
        self.rec.clean.set_more(more);
        self.rec.full.set_more(more);
        if self.rec.clean.phase() != recorder::Phase::Done {
            self.rec.resume_pending = false; // a ride started: its first card work does the boot's first (one writer)
        }
        // the two copies checkpoint together: the clean one never behind (storage/recorder/mod.rs, "Two copies")
        self.rec.clean.keep_up(&self.rec.full);
        self.rec.full.keep_up(&self.rec.clean);
        let was_held = self.rec.clean.held();
        let recovering = self.rec.resume_pending;
        let o = if self.rec.resume_pending {
            // a call while waiting after a failure tries nothing and says Nothing, the last one too (its wait then
            // 0): still pending. Cleared only by a try that found no ride (2026-10-31: the card that answered late at
            // boot lost the marker, and the ride was never resumed)
            let waited = self.rec.clean.waiting();
            let o = self.rec.clean.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            // a card that failed at boot is tried again (the marker stays until the ride is found)
            self.rec.resume_pending = waited || matches!(o, recorder::Outcome::Failed(_));
            o
        } else if self.rec.clean.due() {
            self.rec.clean.io(&mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Nothing => {}
            recorder::Outcome::Resumed(words, name) => {
                // paused where the last checkpoint left it: Start goes on, a long Start ends and saves it; unless the
                // rider had paused it with Start, it also goes on by itself once riding (an auto-pause's stop in the
                // file: storage/recorder/mod.rs `recover_from`)
                // its totals from the record (one of a build before them: zeros, counted from the resume), at the boot
                // and when the card answered late alike (both reach here)
                let more = self.rec.clean.more();
                self.ride = ride::Ride::resumed(&words, &more, self.rec.clean.paused_by_hand());
                // the record keeps GRAVEL, CX and COMMUTE as ROAD: BIKE (settings.bin) says which, read now
                if !self.settings_store.file.loaded {
                    self.load_settings(b);
                }
                // its profile from the record (an older one: ROAD, automatic), by hand if the rider set it so;
                // automatic, a trainer silent for TRAINER_QUIET_S after the restart turns it back to ROAD or MTB
                self.profile = self.rec.clean.profile();
                if self.profile == ride::Profile::Road {
                    // the record keeps GRAVEL, CX and COMMUTE as ROAD: BIKE says which
                    self.profile = self
                        .settings
                        .profile
                        .filter(|p| p.outdoors() && *p != ride::Profile::Mtb)
                        .unwrap_or(self.profile);
                }
                (self.profile_by_hand, self.tlink.quiet_s) = (self.rec.clean.by_hand(), 0);
                let digit = |i: usize| name.get(i).copied().unwrap_or(b'0').wrapping_sub(b'0') % 10;
                let two = |i: usize| digit(i) * 10 + digit(i + 1);
                self.ride_started = RideStart::new((2000 + u16::from(two(0)), two(2), two(4)), (two(6), two(8)));
                self.history = ride::History::default();
                // NP, TSS, kJ, max power and W' balance go on from the checkpoint (the NP window refills: 29 s with
                // no NP sample)
                self.power.resume(&ride::power_words(&more));
                self.reset_ride_curve();
                // the zone times and the hrTSS go on too (zeros in a record of a build before them: from the resume)
                (self.zone_s, self.hrtss) = (ride::ZoneTime::resumed(&more), ride::hrtss_resumed(&more));
                self.rec.model = Some(self.model_now()); // what the ride has now: the scan may not have read yet
                let now = self.now_s();
                self.alt.ride_start(now);
                self.notice("RIDE RESUMED");
            }
            recorder::Outcome::Saved(_) => {
                // this ride's own save (the card of one ended behind another's save said NOT RECORDED: the other's
                // Saved is not its)
                if !recovering && (self.save == Save::Saving || self.save == Save::Failed("NOT SAVED: CARD")) {
                    self.save = if core::mem::take(&mut self.save_part) { Save::Part } else { Save::Saved };
                }
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
                self.save = Save::Failed("NOT SAVED: CARD");
                // not in \Activities: left in \Rustfw\Rides as it was, or named there again (damaged, or the card
                // kept failing)
                self.alarm_notice("SAVE FAILED: KEPT IN RUSTFW/RIDES");
            }
            recorder::Outcome::Stopped(e) => {
                // the ride's recording stops here (said once): what reached the card is saved next
                if self.rec.clean.phase() == recorder::Phase::Done {
                    self.save = Save::Failed("NOT RECORDED");
                } else {
                    self.save_part = true;
                }
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
        self.save_settle();
        self.save_redraw(b);
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
            let waited = self.rec.full.waiting(); // as the clean one's (step_recorder)
            let o = self.rec.full.recover(&mut emmc::Volume(&mut self.emmc, b), local);
            self.rec.full_pending = waited || matches!(o, recorder::Outcome::Failed(_));
            o
        } else if self.rec.full.due() {
            self.rec.full.io_after(&self.rec.clean, &mut emmc::Volume(&mut self.emmc, b), local)
        } else {
            recorder::Outcome::Nothing
        };
        match o {
            recorder::Outcome::Resumed(..) => self.rec.full.resumed_after(&self.rec.clean),
            recorder::Outcome::Kept(_) => {
                if matches!(self.save, Save::Saved | Save::Copying) {
                    self.save = Save::NoFullCopy;
                }
                self.notice("NO FULL COPY");
            }
            _ => {}
        }
    }

    /// AUTO PAUSED / RESUMED, said when the ride has been auto-paused (or moving again) for [`AUTO_SAY_S`] seconds in
    /// a row, so a stop-and-go flicker says nothing; a manual pause or no ride says nothing and resets it.
    pub(crate) fn auto_pause_notice(&mut self) {
        let (said, n) = self.rec.auto_said;
        if self.ride.state() != ride::State::Running {
            self.rec.auto_said = (false, 0);
        } else if self.ride.moving() != said {
            self.rec.auto_said = (said, 0);
        } else if n + 1 < AUTO_SAY_S {
            self.rec.auto_said = (said, n + 1);
        } else {
            self.rec.auto_said = (!said, 0);
            let text = if said { "RESUMED" } else { "AUTO PAUSED" };
            self.say_as(text, Banner::Auto);
            // the tone only with its text: a banner waiting behind a summary or an alarm is silent
            if self.notes.text.as_str() == text && self.notes.kind == Banner::Auto {
                self.tones.play(if said { &BEEP_START } else { &BEEP_PAUSE });
            }
        }
    }

    /// Once a second (the RTC's second changed): the ride and the history take the readings.
    pub(crate) fn second(&mut self) {
        self.alt_second();
        self.places_second();
        self.profile_second();
        self.start_reminder_second();
        if self.ride.state() != ride::State::Idle {
            self.alt_follow();
        }
        let alt = self.alt_now();
        self.live.snapshot(); // the pages show this second's readings, not each broadcast's
        self.climbs_second();
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
            pause_kmh: match AUTO_PAUSE_KMH.get(self.settings.auto_pause).copied().unwrap_or(0) {
                0 => self.ride_profile().pause_kmh(),
                k => k,
            },
        };
        if matches!(self.screen, Screen::End(_) | Screen::Discard(_)) && self.ride.hold() {
            // the rider is ending it: a resumed ride no longer goes on by itself, after a reset either
            self.rec.clean.hold();
            self.rec.full.hold();
        }
        let (timer, was) = (self.ride.timer_s(), self.ride.state());
        if let Some(ev) = self.ride.second(&input) {
            self.recorder(ev);
        }
        if was == ride::State::Paused && self.ride.state() == ride::State::Running {
            // a resumed ride gone on by itself once ridden: said as a Start resume is
            self.tones.play(&BEEP_START);
            self.say("RESUMED");
        }
        self.auto_pause_notice();
        // only the seconds the timer ran, keyed by the wall clock: a stop longer than FILL_S ends the efforts
        let now = self.now_s();
        if let (true, Some(r)) = (self.ride.timer_s() != timer, self.ride_curve.as_deref_mut()) {
            r.sample(now, input.power);
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
            // held conditions count real seconds: the repeats of a stall (`lag_s` > 0) carry one old reading each, so a
            // stall counts only its last tick toward a hold (a deliberate cost: a hold can be a few seconds late)
            if self.rec.lag_s == 0 {
                self.alerts.limits(&r, &self.settings.limits, &mut self.alert);
                // ERG: the trainer holds the watts, the resistance climbs as the rider slows
                let slow = self.alerts.cadence(input.cadence, self.erg_on());
                self.alert.push(slow);
            }
            // the sunset alert: the minutes left to today's sunset by the clock the status bar shows (no fix, no times)
            let now = u32::from(self.strip.time.0) * 60 + u32::from(self.strip.time.1);
            let left = super::fields::minutes_to_sunset(now, self.sun_times());
            let mut lead = crate::SUNSET_LEAD_MIN.get(self.settings.sunset_alert).copied().unwrap_or(0);
            if self.profile == ride::Profile::Commute {
                lead = lead.max(30); // home before dark (a choice)
            }
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
        // the time in each zone: the moving seconds, in the zone ridden, not the held one the bars show (a Start or a resume clears it)
        if self.ride.state() == ride::State::Running && self.ride.moving() {
            self.zone_s.second(self.live.shown(Metric::Hr).map(|v| zt.hr_zone(v)), watts[0].map(|w| zt.power_zone(w)));
            self.hrtss.second(self.live.shown(Metric::Hr), max_hr, self.settings.female);
        }
        let wbal = self.power.wbal_j().map(|j| (j.max(0) / 10).clamp(1, i32::from(u16::MAX)) as u16);
        let dfa = self.hrv.alpha1().filter(|_| input.hr.is_some()).map(|a| (a * 100.0 + 0.5).clamp(1.0, 999.0) as u16);
        let alt = input.alt_mm.map(|mm| (mm / 1000 + ride::ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
        self.history.second([input.power, input.hr, wbal, dfa, alt]);
        self.history.dist_second(self.ride.dist_m(), alt);
        // the course followed: where the rider is on it, and what to say
        self.virtual_course_second();
        let fix = self.gps.pvt.filter(|p| p.good());
        let ev = match (self.courses.as_deref_mut(), fix) {
            (Some(c), Some(p)) if c.on.is_some() && c.base_m.is_none() => c.course.follow(&mut c.at, p.lat, p.lon),
            _ => None,
        };
        if let Some(ev) = ev {
            self.course_event(ev);
        }
        self.segment_fix();
        self.ahead_second();
        self.descent_second();
        self.pace_second(); // after the fix: the position is this second's
    }
}
