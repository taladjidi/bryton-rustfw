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
            // a notice of what a key did replaces a lap summary (dropped, not queued)
            Some(up) if up.1 == Banner::Summary && kind == Banner::Info => ((l, kind), None),
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
    /// Sensor bus transfers failed in a row (app/device.rs `sensor_ok`).
    pub(crate) fails: u8,
    /// The sensor bus is left alone for this long, ms (after a clear: `sensor_ok`).
    pub(crate) hold_ms: u32,
    /// The last such wait, ms; 0 once a transfer succeeded.
    pub(crate) backoff_ms: u32,
    /// AUTO (THEME): the day or night theme from the light (ui/theme.rs).
    pub(crate) day_night: crate::theme::DayNight,
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
            fails: 0,
            hold_ms: 0,
            backoff_ms: 0,
            day_night: Default::default(),
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
    /// The last speed taken (mm/s) and its age (ms): what the next fix's speed is checked against
    /// ([`gnss::Pvt::speed_x10`]).
    pub(crate) ok_speed: Option<(u32, u32)>,
    /// Since the app's first step: the receiver is powered at [`GNSS_ON_MS`], its configuration and aiding sent
    /// once from [`GNSS_CONFIG_MS`].
    pub(crate) ms: u32,
    /// Messages sent to the receiver so far: configuration, time, position.
    pub(crate) sent: u8,
    /// The configuration goes again at the next message (the GPS POWER setting changed).
    pub(crate) reconfig: bool,
    /// The systems go again at the next message (the GNSS SYSTEMS setting changed).
    pub(crate) resys: bool,
    /// AssistNow Offline: the card's file being read and sent ([`mga::Ano`]), and the passes to wait between messages.
    pub(crate) ano: mga::Ano,
    pub(crate) ano_gap: u8,
    /// The open `mga.ubx` and where the last read ended (one open, then reads from there: no walk of the FAT chain
    /// from the start for each sector).
    pub(crate) ano_file: Option<(fat::File, fat::Seek)>,
    /// The power mode the last configuration sent (saver: cyclic tracking).
    pub(crate) saver: bool,
    /// Time left before the receiver is read again while it has nothing, ms ([`GNSS_FILL_MS`], [`GNSS_PVT_WAIT_MS`]).
    pub(crate) wait: u32,
    /// The receiver is switched off for an indoor ride (app/gps.rs `gnss_switch`).
    pub(crate) off: bool,
    /// The RTC's offset from UTC and the last position, as kept in backup registers.
    pub(crate) aid: gnss::Aid,
    /// GPS POWER AUTO's choice ([`gnss::Adaptive`]).
    pub(crate) auto: gnss::Adaptive,
    /// Between rides (app/gps.rs `gnss_idle`): the time since the receiver was switched on or a screen needed it, a
    /// good fix since it was switched on, and asleep (off until needed or a ride starts).
    pub(crate) quiet_ms: u32,
    pub(crate) fixed: bool,
    pub(crate) slept: bool,
    /// What the receiver answered (app/gps.rs `note_ack`): CFG messages taken, refused (UBX-ACK-NAK), configurations
    /// given up after their one retry, and assistance messages it did not use. Counters for the debug info; nothing
    /// reads them on the ride path.
    pub(crate) answers: [u16; 4],
    /// The one retry of a refused configuration is spent (until the receiver is switched on again).
    pub(crate) retried: bool,
    /// Whether the screen shown holds a map, with the screen and MORE page it was worked out for (a page's boxes
    /// are built to find out: once per screen, not each pass).
    pub(crate) map_on: Option<(MapKey, bool)>,
}

/// What `gps.map_on` was worked out for: the screen, the MORE page, the page set, the ride's state, the page count.
pub(crate) type MapKey = (Screen, Option<usize>, ride::Profile, ride::State, usize);

impl Gps {
    /// Nothing heard yet, with the aid read from the backup registers.
    pub(crate) fn new(aid: gnss::Aid) -> Self {
        Gps {
            parser: gnss::Parser::new(),
            pvt: None,
            pvt_ms: 0,
            ok_speed: None,
            ms: 0,
            sent: 0,
            reconfig: false,
            resys: false,
            ano: mga::Ano::new(),
            ano_gap: 0,
            ano_file: None,
            saver: false,
            wait: 0,
            off: false,
            aid,
            auto: gnss::Adaptive::default(),
            quiet_ms: 0,
            fixed: false,
            slept: false,
            answers: [0; 4],
            retried: false,
            map_on: None,
        }
    }

    /// The strip's bars: None without a fix, 0 for a poor one.
    pub(crate) fn bars(&self) -> Option<u8> {
        self.pvt.map(|p| if p.good() { gnss::bars(p.sats) } else { 0 })
    }
}

/// A per-kind array read and written by [`Kind`], the one place that indexes it.
pub(crate) trait ByKind<T: Copy> {
    /// The entry of `k`.
    fn at(&self, k: Kind) -> T;
    /// The entry of `k`, to write.
    fn at_mut(&mut self, k: Kind) -> &mut T;
}

impl<T: Copy> ByKind<T> for [T; Kind::ALL.len()] {
    #[allow(clippy::indexing_slicing)] // Kind's discriminants are 0..Kind::ALL.len() (channel order)
    fn at(&self, k: Kind) -> T {
        self[k as usize]
    }

    #[allow(clippy::indexing_slicing)] // as above
    fn at_mut(&mut self, k: Kind) -> &mut T {
        &mut self[k as usize]
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
    /// The device a search heard, until the rider confirms it on the sensor's row (nothing of it is used before).
    pub(crate) found: [Option<nrf::SensorId>; Kind::ALL.len()],
    /// The sensors heard in the NEARBY search.
    pub(crate) nearby: nrf::Nearby,
    /// The NEARBY search is running.
    pub(crate) scanning: bool,
    /// The gear last heard (front, rear: [`Metric::Gear`]), to count the shifts and record them.
    pub(crate) gear: Option<u16>,
    /// Gear changes in this ride.
    pub(crate) shifts: u16,
    /// The sensors announced as lost in this ride (bit per kind), until they are heard again.
    pub(crate) lost: u8,
    /// The sensors heard in this ride (bit per kind): only those can be lost.
    pub(crate) heard: u8,
    /// ANT broadcasts heard per kind since the start (the dev link's `info`: against about 4 Hz per sensor).
    pub(crate) broadcasts: [u32; Kind::ALL.len()],
}

/// A paired sensor silent this long while a ride records is announced as lost.
pub(crate) const LOST_MS: u32 = 10_000;

impl Radio {
    /// `kind` was heard in the last [`sensors::STALE_MS`].
    pub(crate) fn heard(&self, kind: Kind) -> bool {
        self.heard_ms.get(kind as usize).is_some_and(|&ms| ms < sensors::STALE_MS)
    }

    /// The sensor of `kind` is paired and was heard lately.
    pub(crate) fn paired_heard(&self, kind: Kind) -> bool {
        matches!(self.sensors.get(kind as usize), Some(Sensor::Paired(_))) && self.heard(kind)
    }

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
            lost: 0,
            heard: 0,
            broadcasts: [0; Kind::ALL.len()],
            found: [None; Kind::ALL.len()],
        }
    }

    /// The sensors to announce while a ride records, as masks of kinds: `(lost, back)`. A sensor is lost once it was
    /// heard in this ride and has been silent [`LOST_MS`] (a chip restart included); back when it is heard again.
    /// Without a ride the marks are cleared, silently.
    pub(crate) fn lost_event(&mut self, recording: bool) -> (u8, u8) {
        if !recording {
            (self.heard, self.lost) = (0, 0);
            return (0, 0);
        }
        let (mut lost, mut back) = (0, 0);
        for k in [Kind::Hr, Kind::Speed, Kind::Cadence, Kind::SpeedCadence, Kind::Power] {
            let (bit, h) = (1u8 << (k as usize), self.heard_ms.at(k));
            if !matches!(self.sensors.at(k), Sensor::Paired(_)) {
                (self.heard, self.lost) = (self.heard & !bit, self.lost & !bit);
            } else if h < sensors::STALE_MS {
                self.heard |= bit;
                if self.lost & bit != 0 {
                    self.lost &= !bit;
                    back |= bit;
                }
            } else if h >= LOST_MS && self.heard & bit != 0 && self.lost & bit == 0 {
                self.lost |= bit;
                lost |= bit;
            }
        }
        (lost, back)
    }

    /// A ride starts or resumes: the channels resting search again at the next pass, from the first rest (a strap put
    /// on just before Start is found at once, not after a long rest; a power meter out of range during a pause, too).
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
    /// Power match (training/power_match.rs) and the ms since its last second.
    pub(crate) pm: power_match::PowerMatch,
    /// See `pm`.
    pub(crate) pm_ms: u32,
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
            pm: power_match::PowerMatch::new(),
            pm_ms: 0,
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
    pub(crate) zones: ride::Zones,
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
    /// The next-step strip as drawn in the status strip's place ([`App::update_strip`]); None: the status strip.
    pub(crate) top: Option<super::workouts::Ahead>,
}

impl Plan {
    /// Nothing planned, nothing running.
    pub(crate) fn new() -> Self {
        Plan {
            calendar: Default::default(),
            day: None,
            zones: ride::Zones::standard(0, 0),
            workout: storage::workout_file::Workout::new(),
            file: None,
            player: None,
            played: None,
            planned: false,
            ran_s: 0,
            done: 0,
            done_key: None,
            top: None,
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
    /// An APPLY made during the ride: the FTP to set once the ride is over (the ride's TSS and zone times keep one FTP).
    pub(crate) held: Option<u16>,
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
    /// The RTC's second (since 1970) the ride's seconds are counted up to; a clock set moves it ([`App::phone_time`]).
    pub(crate) counted_s: i64,
    /// The CP (W) and W' (J) the ride in progress took at its start, and whether from the fit (app/fitness.rs): kept
    /// for the whole ride.
    pub(crate) model: Option<(u16, u16, bool)>,
    /// Both copies' card errors as [`App::step_recorder`] last saw them.
    pub(crate) errors_seen: u32,
    /// Whether AUTO PAUSED was said and RESUMED not yet, and the seconds in a row the ride has differed from that.
    pub(crate) auto_said: (bool, u8),
}

impl Recording {
    /// Nothing recorded, nothing to resume; the seconds counted from the RTC's `rtc_s`.
    pub(crate) fn new(rtc_s: i64) -> Self {
        Recording {
            clean: recorder::Recorder::new(),
            resume_pending: false,
            full: recorder::Recorder::of(recorder::Kind::Full),
            full_pending: false,
            auto_said: (false, 0),
            lag_s: 0,
            counted_s: rtc_s,
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

/// The battery reading and BATTERY SAVER AUTO's state (app/device.rs).
pub(crate) struct Saver {
    /// Since the last battery reading.
    pub(crate) battery_ms: u32,
    /// The battery was reported low.
    pub(crate) told_low: bool,
    /// BATTERY SAVER AUTO has turned the saver on ([`App::saver_on`]): from [`BATTERY_SAVER_PERCENT`] until the charger.
    pub(crate) saver_auto: bool,
    /// A USB snapshot was just written to the backup registers ([`App::usb_snapshot`]): the drop that follows it
    /// leaves registers 0 and 31 to it.
    pub(crate) usb_snapped: bool,
    /// BATTERY SAVER AUTO's state as the rider was last told.
    pub(crate) saver_said: bool,
    /// Battery polls in a row with no reading (BATTERY UNKNOWN said at the first; the ADC brought up again).
    pub(crate) adc_misses: u16,
}

impl Saver {
    /// The state at start.
    pub(crate) fn new() -> Self {
        Saver {
            battery_ms: BATTERY_MS, // read at the first step
            told_low: false,
            saver_auto: false,
            usb_snapped: false,
            saver_said: false,
            adc_misses: 0,
        }
    }
}

/// The quick panel's drawn state (app/quick.rs).
pub(crate) struct Quick {
    /// What the quick panel shows, box by box, and its shape (app/quick.rs).
    pub(crate) shown: [u32; app::QUICK_BOXES],
    pub(crate) shape: u8,
    /// The title and artist scroll: the track's fingerprint and the steps since it began (app/quick.rs).
    pub(crate) scroll: (u32, u32),
    /// ZERO armed until, and its result shown until (epoch seconds, app/quick.rs).
    pub(crate) zero: (Option<u32>, Option<u32>),
    /// Each sensor line's value as drawn, and its colour: a changed value redraws only the characters that differ.
    pub(crate) vals: [(crate::text::Line<16>, u16); ui::quick::ROWS],
}

impl Quick {
    /// The state at start.
    pub(crate) fn new() -> Self {
        Quick {
            shown: [0; app::QUICK_BOXES],
            shape: 0,
            scroll: (0, 0),
            zero: (None, None),
            vals: [(crate::text::Line::new(), 0); ui::quick::ROWS],
        }
    }
}

/// The menus' input state and what they have drawn (app/menus.rs, app/input.rs).
pub(crate) struct MenuInput {
    /// The selected menu row's number is being changed (Lap up, Page down, Start or Back to leave).
    pub(crate) editing: bool,
    /// MAX HR when its editing began: leaving it unchanged keeps "from age" ([`App::setting_edited`]).
    pub(crate) max_hr_start: u16,
    /// The selected menu row was chosen (by a key or a tap) since the menu opened: a tap on it acts. A tap on another
    /// row, or on the row a menu opens on, only selects it (one stray touch never acts).
    pub(crate) chosen: bool,
    /// A row that cannot be undone (forget a sensor or the phone, import stock's settings) asked to be confirmed: Start
    /// on it again acts; any other input clears it (App::confirmed). A past ride's DELETE, asked on its card, is
    /// [`PAST_DELETE`].
    pub(crate) confirm: Option<(Menu, usize)>,
    /// GIVE UP SAVE's step while its row waits in [`MenuInput::confirm`]: two banners of explanation, then two questions.
    pub(crate) give_up_step: u8,
    /// A held key already acted (a list scrolled, a value stepped), so its release does not act once more; cleared
    /// when no key is down.
    pub(crate) repeated: bool,
    /// The menu rows on screen, as fingerprints ([`App::menu_row_print`]).
    pub(crate) shown: [u32; ui::MENU_ROWS],
    /// The same without the value ([`App::menu_row_print`]): a row whose head is unchanged repaints its value only.
    pub(crate) head: [u32; ui::MENU_ROWS],
    /// The nearby list's rows on screen, as fingerprints ([`ui::list_prints`]).
    pub(crate) list_shown: [u32; ui::LIST_ROWS],
}

impl MenuInput {
    /// The state at start.
    pub(crate) fn new() -> Self {
        MenuInput {
            editing: false,
            max_hr_start: 0,
            chosen: false,
            confirm: None,
            give_up_step: 0,
            repeated: false,
            shown: [0; ui::MENU_ROWS],
            head: [0; ui::MENU_ROWS],
            list_shown: [0; ui::LIST_ROWS],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sensor_lost_is_said_once_and_back_once() {
        let mut sensors = [Sensor::Off; Kind::ALL.len()];
        sensors[Kind::Hr as usize] = Sensor::Paired(SensorId { device: 1, trans: 1 });
        sensors[Kind::Power as usize] = Sensor::Paired(SensorId { device: 2, trans: 1 });
        let mut r = Radio::new(sensors);
        let (hr, pw) = (1u8 << Kind::Hr as u8, 1u8 << Kind::Power as u8);
        r.heard_ms[Kind::Hr as usize] = LOST_MS + 1; // silent before the ride: not lost
        assert_eq!(r.lost_event(true), (0, 0));
        r.heard_ms = [0; Kind::ALL.len()];
        assert_eq!(r.lost_event(true), (0, 0), "heard");
        r.heard_ms = [5_000; Kind::ALL.len()];
        assert_eq!(r.lost_event(true), (0, 0), "5 s");
        r.heard_ms = [LOST_MS; Kind::ALL.len()];
        assert_eq!(r.lost_event(true), (hr | pw, 0), "10 s: both, in one event");
        assert_eq!(r.lost_event(true), (0, 0), "once");
        r.heard_ms[Kind::Hr as usize] = 0;
        assert_eq!(r.lost_event(true), (0, hr), "back");
        assert_eq!(r.lost_event(true), (0, 0), "back once");
        r.heard_ms[Kind::Hr as usize] = u32::MAX; // a chip restart, then silence
        assert_eq!(r.lost_event(true), (hr, 0));
        assert_eq!((r.lost_event(false), r.lost, r.heard), ((0, 0), 0, 0), "no ride: cleared silently");
    }

    #[test]
    fn a_higher_banner_stays_and_the_lower_waits() {
        let mut n = Notices::new();
        n.say("LAP 1", Banner::Summary);
        n.say("TOUCH LOCKED", Banner::Info);
        assert_eq!((n.text.as_str(), n.next), ("TOUCH LOCKED", None), "a notice replaces a lap summary at once");
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
    fn a_limit_alert_and_a_workout_step_in_one_second_both_show() {
        for first_step in [true, false] {
            let mut n = Notices::new();
            let (step, alert) = (("STEP 2", Banner::Ride), ("POWER HIGH", Banner::Warn));
            for (t, k) in if first_step { [step, alert] } else { [alert, step] } {
                n.say(t, k);
            }
            assert_eq!(n.text.as_str(), "POWER HIGH", "the alert is up");
            n.ms = None;
            n.show_waiting();
            assert_eq!(n.text.as_str(), "STEP 2", "and the step after it");
        }
    }

    #[test]
    fn auto_paused_waits_behind_a_lap_summary() {
        let mut n = Notices::new();
        n.say("LAP 1", Banner::Summary);
        n.say("AUTO PAUSED", Banner::Auto);
        assert_eq!(n.text.as_str(), "LAP 1", "the summary stays");
        n.ms = None;
        n.show_waiting();
        assert_eq!(n.text.as_str(), "AUTO PAUSED");
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
        let mut rec = Recording::new(0);
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

/// The climb page's place in the round (app/profile.rs).
pub(crate) struct ClimbPage {
    /// The climb page is in the round.
    pub(crate) on: bool,
    /// The ride page it follows.
    pub(crate) after: usize,
    /// The climb handled: its start (m) and whether the page opened, or was skipped, for it.
    pub(crate) handled: Option<(u32, bool)>,
    /// A segment started and the page has not opened for it yet (a finger or key down: the next pass tries again).
    pub(crate) seg: bool,
    /// The ride's timer (s) when the rider reached the climb's start (the time on the climb).
    pub(crate) t0: Option<u32>,
}

impl ClimbPage {
    /// Not in the round, after page 0, no climb handled.
    pub(crate) fn new() -> Self {
        ClimbPage { on: false, after: 0, handled: None, seg: false, t0: None }
    }
}

/// CPU time of the road ahead's tile loads, in microseconds ([`App::ahead_tile_us`]).
#[derive(Clone, Copy, Default)]
pub(crate) struct AheadUs {
    /// The last tile, all its slices.
    pub(crate) last: u32,
    /// The longest tile so far.
    pub(crate) max: u32,
    /// The longest single call on a tile.
    pub(crate) slice: u32,
}

/// When the ride started: its card's title.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct RideStart {
    /// Year, month, day.
    pub(crate) date: (u16, u8, u8),
    /// Hours and minutes.
    pub(crate) time: (u8, u8),
}

impl RideStart {
    /// A date and a time of day.
    pub(crate) fn new(date: (u16, u8, u8), time: (u8, u8)) -> Self {
        RideStart { date, time }
    }
}

/// The touch panel: the controller's answer at start, the gesture recogniser and what a drive command left over.
pub(crate) struct TouchState {
    /// The touch controller answered at the start.
    pub(crate) ok: bool,
    /// The gestures from the panel's points.
    pub(crate) rec: touch::Recogniser,
    /// A gesture that ended during a USB drive command, acted on after it ([`App::touch`]).
    pub(crate) pending: Option<Input>,
    /// The panel was sampled during a drive command: the main loop's next `dt_ms` (its sleep since its own last
    /// read) is time those samples already counted.
    pub(crate) sampled: bool,
    /// Touch reads failed in a row (`App::touch_health`).
    pub(crate) fails: u8,
    /// No controller reset for this long, ms.
    pub(crate) wait_ms: u32,
    /// The last such wait, ms; 0 once a read succeeded.
    pub(crate) backoff_ms: u32,
}

impl TouchState {
    /// With the controller's answer (`ok`), nothing pending.
    pub(crate) fn new(ok: bool) -> Self {
        TouchState {
            ok,
            rec: touch::Recogniser::default(),
            pending: None,
            sampled: false,
            fails: 0,
            wait_ms: 0,
            backoff_ms: 0,
        }
    }
}

/// The ride page's map view, set by hand (app/screens.rs).
pub(crate) struct MapView {
    /// The zoom level set by "+" and "-" ([`map::mpp16`]); None: fitted to the track (per power-up).
    pub(crate) zoom: Option<u32>,
    /// Zoomed in and panned by hand: the centre (metres on the track's plane); None: it follows the rider.
    pub(crate) centre: Option<(i32, i32)>,
    /// The crosshair with a course followed showed the whole route ([`map::View::fit`]) instead of the default view.
    pub(crate) whole: bool,
}

impl MapView {
    /// Fitted to the track.
    pub(crate) const fn new() -> Self {
        MapView { zoom: None, centre: None, whole: false }
    }
}

/// The road ahead (app/ahead.rs): climbs found with no course followed.
pub(crate) struct AheadState {
    /// Its memory from main.rs ([`App::set_ahead_memory`]); None until given.
    pub(crate) road: Option<(&'static mut climbs_ahead::Ahead, &'static mut [u16])>,
    /// The last and the longest tile load, and the longest slice ([`App::ahead_tile_us`]).
    pub(crate) us: AheadUs,
    /// The time spent so far on the tile being loaded, us.
    pub(crate) acc: u32,
    /// The steep descent said last: its place on the ride's odometer, m ([`App::descent_second`]).
    pub(crate) descent_said: Option<i32>,
}

impl AheadState {
    /// No road yet.
    pub(crate) fn new() -> Self {
        AheadState { road: None, us: AheadUs::default(), acc: 0, descent_said: None }
    }
}
