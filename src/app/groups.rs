//! Groups of the app's fields, one struct per concern, so `App` (lib.rs) stays a list of parts rather than of
//! flags. Each has its own `new` and keeps the small pure logic that belongs to it alone, testable without an `App`
//! (the same way as `Phone` and `Ftms`). Fields are `pub(crate)`: the other `impl App` blocks read them as
//! `self.notes.ms`.

use crate::text::Line;
use crate::*;

/// The notices and the banner over the screen (app/screens.rs).
pub(crate) struct Notices {
    /// The latest notices, newest first (empty lines: none); the overview shows the first [`ui::NOTICES`].
    pub(crate) list: [Line<40>; 2],
    /// How long the banner (the latest notice, over the screen) has been up; None: none. Nothing under it is
    /// redrawn meanwhile; when it goes, the screen is drawn again.
    pub(crate) ms: Option<u32>,
    /// What the banner says (two lines at most).
    pub(crate) text: Line<40>,
    /// The banner's kind (an alarm white on red).
    pub(crate) kind: Banner,
    /// The one waiting for the banner to go.
    pub(crate) next: Option<(Line<40>, Banner)>,
    /// The banner is drawn yet.
    pub(crate) drawn: bool,
}

impl Notices {
    /// No notice, no banner.
    pub(crate) fn new() -> Self {
        Notices { list: Default::default(), ms: None, text: Line::new(), kind: Banner::Info, next: None, drawn: false }
    }

    /// `line` is the newest notice.
    pub(crate) fn push(&mut self, line: Line<40>) {
        self.list.rotate_right(1);
        self.list[0] = line;
    }

    /// Shows `text` as a banner of `kind`. One up of a higher kind stays and this one waits; one up of a lower kind
    /// waits for this one; one of the same kind is replaced (2026-10-25 review: a turn said again after the next
    /// one, LAP twice shown 8 s). One waits at most (the higher kind kept, else the newer) and shows when the
    /// banner up goes ([`Notices::show_waiting`]): an alarm over a workout step, the step after it. A cue is good
    /// for its moment only: one waiting, or one up pushed off, goes when another cue is said.
    pub(crate) fn say(&mut self, text: &str, kind: Banner) {
        let mut l = Line::<40>::new();
        l.push_str(text);
        if kind.is_cue() && self.next.is_some_and(|n| n.1.is_cue()) {
            self.next = None;
        }
        let up = self.ms.is_some().then_some((self.text, self.kind));
        let (show, wait) = match up {
            Some(up) if up.1 > kind => (up, Some((l, kind))),
            Some(up) if up.1 < kind && !(up.1.is_cue() && kind.is_cue()) => ((l, kind), Some(up)),
            _ => ((l, kind), None),
        };
        if let Some(w) = wait
            && self.next.is_none_or(|n| n.1 <= w.1)
        {
            self.next = Some(w);
        }
        if up != Some(show) {
            (self.text, self.kind) = show;
            (self.ms, self.drawn) = (Some(0), false);
        }
    }

    /// The banner is gone: the one that waited, if any, is up next.
    pub(crate) fn show_waiting(&mut self) {
        if let Some((text, kind)) = self.next.take() {
            (self.text, self.kind) = (text, kind);
            (self.ms, self.drawn) = (Some(0), false);
        }
    }
}

/// The backlight (app/device.rs): what it shows, the automatic level and the measurement behind it.
pub(crate) struct Light {
    /// The level the backlight is at (0: off).
    pub(crate) shown: u8,
    /// Since the last key press.
    pub(crate) idle_ms: u32,
    /// The AUTO OFF warning is up: the light stays on though timed out.
    pub(crate) wake: bool,
    /// The automatic backlight's state (ambient.rs).
    pub(crate) auto: ambient::Auto,
    /// The level it asks for.
    pub(crate) auto_level: u8,
    /// Since the last light measurement started.
    pub(crate) ms: u32,
    /// The measurement is still running.
    pub(crate) measuring: bool,
}

impl Light {
    /// Off, a measurement due at the first step, the automatic level at its middle until the first reading.
    pub(crate) fn new() -> Self {
        Light {
            shown: 0,
            idle_ms: 0,
            wake: false,
            auto: ambient::Auto::default(),
            auto_level: LIGHT_LEVELS[1],
            ms: LIGHT_MS,
            measuring: false,
        }
    }

    /// A key was held (or not) for a pass of `pass_ms`: the idle time restarts or grows.
    pub(crate) fn idle(&mut self, held: bool, pass_ms: u32) {
        self.idle_ms = if held { 0 } else { self.idle_ms.saturating_add(pass_ms) };
    }
}

/// The GNSS receiver's state (app/gps.rs): the parser, the last fix, the start-up sequence and the aiding.
pub(crate) struct Gps {
    /// The receiver's UBX framing.
    pub(crate) parser: gnss::Parser,
    /// The last fix, and its age.
    pub(crate) pvt: Option<gnss::Pvt>,
    /// Since the last fix, ms.
    pub(crate) pvt_ms: u32,
    /// Since the app's first step: the receiver is powered at [`GNSS_ON_MS`], its configuration and aiding sent
    /// once from [`GNSS_CONFIG_MS`].
    pub(crate) ms: u32,
    /// Messages sent to the receiver so far: configuration, time, position.
    pub(crate) sent: u8,
    /// The configuration goes again at the next message (the GPS POWER setting changed).
    pub(crate) reconfig: bool,
    /// The power mode the last configuration sent (saver: cyclic tracking).
    pub(crate) saver: bool,
    /// Passes left before the receiver is read again while it has nothing ([`GNSS_IDLE_PASSES`]).
    pub(crate) wait: u8,
    /// The receiver is switched off for an indoor ride (app/gps.rs `gnss_switch`).
    pub(crate) off: bool,
    /// The RTC's offset from UTC and the last position, as kept in backup registers.
    pub(crate) aid: gnss::Aid,
    /// The GPS had a good fix since power-on.
    pub(crate) had_fix: bool,
    /// GPS POWER AUTO's choice ([`gnss::Adaptive`]).
    pub(crate) auto: gnss::Adaptive,
    /// Between rides (app/gps.rs `gnss_idle`): the time since the receiver was switched on or a screen needed it, a
    /// good fix since it was switched on, and asleep (off until needed or a ride starts).
    pub(crate) quiet_ms: u32,
    pub(crate) fixed: bool,
    pub(crate) slept: bool,
}

impl Gps {
    /// Nothing heard yet, with the aid read from the backup registers.
    pub(crate) fn new(aid: gnss::Aid) -> Self {
        Gps {
            parser: gnss::Parser::new(),
            pvt: None,
            pvt_ms: 0,
            ms: 0,
            sent: 0,
            reconfig: false,
            saver: false,
            wait: 0,
            off: false,
            aid,
            had_fix: false,
            auto: gnss::Adaptive::default(),
            quiet_ms: 0,
            fixed: false,
            slept: false,
        }
    }

    /// Remembers that a good fix was seen (no notice: the strip's GPS says it, and a notice stayed after the fix was
    /// lost).
    pub(crate) fn note_fix(&mut self) {
        self.had_fix |= self.pvt.is_some_and(|p| p.good());
    }

    /// The strip's bars: None without a fix, 0 for a poor one.
    pub(crate) fn bars(&self) -> Option<u8> {
        self.pvt.map(|p| if p.good() { gnss::bars(p.sats) } else { 0 })
    }
}

/// The ANT+ side of the radio (app/radio.rs): the paired sensors, how long since each was heard, the search and the
/// shifting system's counts.
pub(crate) struct Radio {
    /// The sensors, by kind: off or paired with a device.
    pub(crate) sensors: [Sensor; Kind::ALL.len()],
    /// Since each sensor's last broadcast.
    pub(crate) heard_ms: [u32; Kind::ALL.len()],
    /// Since each sensor's channel was closed by its search time-out (`u32::MAX`: not resting).
    pub(crate) resting_ms: [u32; Kind::ALL.len()],
    /// Each sensor's searches in a row that found nothing: its rest grows with them ([`crate::search_rest_ms`]).
    pub(crate) misses: [u8; Kind::ALL.len()],
    /// Channels that sent data while searching: ask the chip which sensor it found.
    pub(crate) ask: u8,
    /// The sensors heard in the NEARBY search.
    pub(crate) nearby: nrf::Nearby,
    /// The NEARBY search is running.
    pub(crate) scanning: bool,
    /// The gear last heard (front, rear: [`Metric::Gear`]), to count the shifts and record them.
    pub(crate) gear: Option<u16>,
    /// Gear changes in this ride.
    pub(crate) shifts: u16,
}

impl Radio {
    /// None heard, with the sensors that were paired at start-up.
    pub(crate) fn new(sensors: [Sensor; Kind::ALL.len()]) -> Self {
        Radio {
            sensors,
            heard_ms: [u32::MAX; Kind::ALL.len()],
            resting_ms: [u32::MAX; Kind::ALL.len()],
            misses: [0; Kind::ALL.len()],
            ask: 0,
            nearby: nrf::Nearby::new(),
            scanning: false,
            gear: None,
            shifts: 0,
        }
    }

    /// A ride starts: the channels resting search again at the next pass, from the first rest (a strap put on just
    /// before Start is found at once, not after a long rest).
    pub(crate) fn wake_rested(&mut self) {
        for (r, m) in self.resting_ms.iter_mut().zip(&mut self.misses) {
            if *r != u32::MAX {
                (*r, *m) = (u32::MAX - 1, 0);
            }
        }
    }

    /// A pass of `pass_ms` has gone by: every sensor's silence grows.
    pub(crate) fn tick(&mut self, pass_ms: u32) {
        for h in &mut self.heard_ms {
            *h = h.saturating_add(pass_ms);
        }
    }
}

/// The ANT+ home trainer link's bookkeeping (app/trainer.rs): what was heard and acknowledged, and the screen's state.
pub(crate) struct TrainerLink {
    /// The trainer screen as drawn.
    pub(crate) shown: ui::trainer::Shown,
    /// The trainer was heard last pass.
    pub(crate) up: bool,
    /// The time since the last acknowledged message, ms.
    pub(crate) ack_gap_ms: u32,
    /// The second ticked since the screen was drawn.
    pub(crate) second: bool,
    /// The chip never answered the first message (control off until it starts again).
    pub(crate) no_control: bool,
    /// The chip answered a message since it started.
    pub(crate) acked: bool,
    /// The seconds the trainer has been silent while the profile is indoor and automatic.
    pub(crate) quiet_s: u16,
    /// The control pages sent ([`fit::cmd_bit`] each) and the TX_FAILED events on its ANT channel since the last
    /// record (the full copy's `cmd_sent` and `ack_fail`).
    pub(crate) cmds: u8,
    /// See `cmds`.
    pub(crate) fails: u8,
}

impl TrainerLink {
    /// Nothing heard, nothing sent.
    pub(crate) fn new() -> Self {
        TrainerLink {
            shown: ui::trainer::Shown::default(),
            up: false,
            ack_gap_ms: 0,
            second: false,
            no_control: false,
            acked: false,
            quiet_s: 0,
            cmds: 0,
            fails: 0,
        }
    }
}

/// The planned workouts (app/workouts.rs): the calendar read from the card, the workout opened or running, and the
/// player that walks its steps.
pub(crate) struct Plan {
    /// The workouts planned from the day it was read on.
    pub(crate) calendar: storage::workout_file::Calendar,
    /// The day the calendar was read for.
    pub(crate) day: Option<(u16, u8, u8)>,
    /// The FTP and maximum heart rate the calendar's profiles were coloured with: another pair reads it again.
    pub(crate) zones: (u16, u16),
    /// The workout opened (its steps menu) or running.
    pub(crate) workout: storage::workout_file::Workout,
    /// Its file.
    pub(crate) file: Option<fat::File>,
    /// Where it is (workout.rs); None: not running.
    pub(crate) player: Option<workout::Player>,
    /// The workout played to its end (its file; counts for its day only).
    pub(crate) played: Option<((u16, u8, u8), fat::File)>,
    /// The workout opened is a calendar one (not the ramp test, which plays through the same player).
    pub(crate) planned: bool,
    /// Seconds the player ran since START: a Lap skipped through to the end is not a workout done.
    pub(crate) ran_s: u32,
    /// Bit k: calendar entry k has a ride of its day matching it ([`App::refresh_done`]); kept while a scan runs.
    pub(crate) done: u8,
    /// What `done` was computed for (the date, the calendar, the history), None: not yet.
    pub(crate) done_key: Option<u32>,
}

impl Plan {
    /// Nothing planned, nothing running.
    pub(crate) fn new() -> Self {
        Plan {
            calendar: Default::default(),
            day: None,
            zones: (0, 0),
            workout: storage::workout_file::Workout::new(),
            file: None,
            player: None,
            played: None,
            planned: false,
            ran_s: 0,
            done: 0,
            done_key: None,
        }
    }

    /// The opened workout's steps.
    pub(crate) fn steps(&self) -> &[storage::workout_file::Step] {
        self.workout.steps()
    }

    /// The player is on a step of the opened workout.
    pub(crate) fn running(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.step(self.steps()).is_some())
    }
}

/// The ramp test (app/ramp.rs): the one being ridden, its last result, and the result screen due.
#[derive(Default)]
pub(crate) struct RampState {
    /// The test being ridden.
    pub(crate) run: Option<ramp::Ramp>,
    /// The last result.
    pub(crate) result: Option<ramp::Outcome>,
    /// The result screen is due (shown at the next pass).
    pub(crate) show: bool,
}

/// The ride recorders and what goes with the ride being recorded (app/riding.rs, storage/recorder/mod.rs).
pub(crate) struct Recording {
    /// The ride recorder (FIT file, checkpoints, save); a ride to look for on the card at the first step is
    /// `resume_pending`.
    pub(crate) clean: recorder::Recorder,
    /// A ride is left open on the card for `clean`.
    pub(crate) resume_pending: bool,
    /// The ride's full copy (developer fields: storage/recorder/mod.rs, "Two copies"), driven after `clean`.
    pub(crate) full: recorder::Recorder,
    /// A ride of its own to look for on the card once the clean one's is found.
    pub(crate) full_pending: bool,
    /// While seconds are caught up after a long pass: how many seconds ago the one being ticked was (its time).
    pub(crate) lag_s: u8,
    /// The CP (W) and W' (J) the ride in progress took at its start, and whether from the fit (app/fitness.rs): kept
    /// for the whole ride.
    pub(crate) model: Option<(u16, u16, bool)>,
    /// Both copies' card errors as [`App::step_recorder`] last saw them.
    pub(crate) errors_seen: u32,
}

impl Recording {
    /// Nothing recorded, nothing to resume.
    pub(crate) fn new() -> Self {
        Recording {
            clean: recorder::Recorder::new(),
            resume_pending: false,
            full: recorder::Recorder::of(recorder::Kind::Full),
            full_pending: false,
            lag_s: 0,
            model: None,
            errors_seen: 0,
        }
    }

    /// A ride was left open on the card (`open`): both copies look for theirs at the first step.
    pub(crate) fn resume_if(&mut self, open: bool) {
        self.resume_pending = open;
        self.full_pending = open;
    }

    /// The strip's recording mark: whether the ride is saving, running or paused (`writable`: the card may be written).
    pub(crate) fn mark(&self, ride: &ride::Ride, writable: bool) -> Rec {
        match ride.state() {
            ride::State::Idle => Rec::Idle,
            _ if !writable
                || self.clean.failing()
                || !matches!(self.clean.phase(), recorder::Phase::Starting | recorder::Phase::Recording) =>
            {
                Rec::NotSaving
            }
            _ if ride.moving() => Rec::Running,
            _ => Rec::Paused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_banner_stays_and_the_lower_waits() {
        let mut n = Notices::new();
        n.say("TURN LEFT", Banner::Info);
        n.say("HEART RATE", Banner::Alarm);
        assert_eq!((n.text.as_str(), n.kind), ("HEART RATE", Banner::Alarm), "the alarm takes over");
        assert_eq!(n.next.map(|w| w.0.as_str() == "TURN LEFT"), Some(true), "and the lower waits");
        n.say("LAP", Banner::Info);
        assert_eq!(n.text.as_str(), "HEART RATE", "a lower kind does not push an alarm off");
        n.ms = None;
        n.show_waiting();
        assert_eq!(n.ms, Some(0), "the one that waited comes up");
    }

    #[test]
    fn notices_come_newest_first() {
        let mut n = Notices::new();
        for t in ["a", "b", "c"] {
            let mut l = Line::new();
            l.push_str(t);
            n.push(l);
        }
        assert_eq!((n.list[0].as_str(), n.list[1].as_str()), ("c", "b"));
    }

    #[test]
    fn silence_grows_and_a_resume_sets_both_copies() {
        let mut r = Radio::new([Sensor::Off; Kind::ALL.len()]);
        r.heard_ms[0] = 10;
        r.tick(5);
        assert_eq!((r.heard_ms[0], r.heard_ms[1]), (15, u32::MAX), "saturating");
        let mut rec = Recording::new();
        rec.resume_if(true);
        assert!(rec.resume_pending && rec.full_pending);
    }
}

/// The AUTO OFF timer (app/device.rs `step_auto_off`): seconds of quiet counted, pure logic.
pub(crate) struct AutoOff {
    /// The RTC second last counted (255: none yet).
    pub(crate) seen: u8,
    /// Seconds left of the 60 s that movement keeps the unit on.
    pub(crate) hold: u8,
    /// What kept the unit on at the last second counted (app/device.rs `step_auto_off`: one bit per reason).
    pub(crate) why: u32,
    /// Seconds in a row with nothing keeping the unit on.
    quiet_s: u32,
}

/// What a quiet second asks of [`AutoOff::second`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OffStep {
    /// Nothing yet.
    Wait,
    /// 10 s and 5 s before: the warning banner and beep.
    Warn,
    /// The time-out is up: switch off.
    Off,
}

impl AutoOff {
    /// Not counting.
    pub(crate) const fn new() -> Self {
        AutoOff { seen: 255, hold: 0, why: 0, quiet_s: 0 }
    }

    /// One second gone: `quiet` when nothing keeps the unit on (any input or reason restarts the count); `min`
    /// minutes is the time-out (0: never).
    pub(crate) fn second(&mut self, min: u32, quiet: bool) -> OffStep {
        if !quiet || min == 0 {
            self.quiet_s = 0;
            return OffStep::Wait;
        }
        self.quiet_s += 1;
        match (min * 60).saturating_sub(self.quiet_s) {
            0 => OffStep::Off,
            left if left <= 10 && left % 5 == 0 => OffStep::Warn,
            _ => OffStep::Wait,
        }
    }
}

#[cfg(test)]
mod auto_off_tests {
    extern crate std;
    use super::*;

    #[test]
    fn counts_warns_and_restarts() {
        let mut a = AutoOff::new();
        let steps: std::vec::Vec<_> = (0..300).map(|_| a.second(5, true)).collect();
        assert_eq!(steps[289], OffStep::Warn);
        assert_eq!(steps[294], OffStep::Warn);
        assert_eq!(steps[299], OffStep::Off);
        assert_eq!(steps.iter().filter(|s| **s != OffStep::Wait).count(), 3);
        for _ in 0..299 {
            a.second(5, true);
        }
        assert_eq!(a.second(5, false), OffStep::Wait, "any activity restarts the count");
        assert_eq!(a.second(5, true), OffStep::Wait);
        assert!((0..5000).all(|_| a.second(0, true) == OffStep::Wait), "0: never");
    }
}
