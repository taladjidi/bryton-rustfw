//! Stock's planned workouts: the calendar read from the card, a workout opened and started, its player fed by the
//! ride's seconds, the step fields and their target colours, the workout gauge field ([`Field::WktPower`]), and the
//! tones at a step's start and count-down.

use crate::*;

/// The overview's calendar is read this long after the start, not at boot (the card is not touched then).
/// It was 8 s until 2026-10-27: an empty TODAY card that long after the start.
pub(crate) const CALENDAR_MS: u32 = 1_000;

/// No step: what a workout field shows past the last one.
pub(crate) const OPEN: storage::workout_file::Step = storage::workout_file::Step {
    len: storage::workout_file::Length::Open,
    target: storage::workout_file::Target::OPEN,
    intensity: 0,
};

/// What the workout gauge field shows ([`Field::WktPower`]): what it measures and over which window, the step's
/// band (None: no gauge) and whether a step runs, which change it whole; then the number, its colour and the marker's
/// column, which are redrawn alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GaugeShown {
    pub(crate) kind: storage::workout_file::Kind,
    pub(crate) avg: usize,
    pub(crate) band: Option<(u16, u16)>,
    pub(crate) step: bool,
    /// The climb pacer's gauge ([`Field::ClimbPace`]): the predicted time to the top, s (0: no plan).
    pub(crate) pace: Option<u16>,
    pub(crate) value: Line<8>,
    pub(crate) color: u16,
    pub(crate) marker: Option<u32>,
}

impl GaugeShown {
    /// The same field with only the number or the marker moved.
    fn same_frame(&self, o: &GaugeShown) -> bool {
        (self.kind, self.avg, self.band, self.step, self.pace.is_some())
            == (o.kind, o.avg, o.band, o.step, o.pace.is_some())
    }

    /// Its bar for a gauge `cols` wide: the band's columns, the marker with the number's colour, and the zones of
    /// the reading's kind behind it (for this FTP and maximum heart rate, so they follow a change of either).
    fn bar(&self, cols: u32, (ftp, max_hr): (u16, u16)) -> Option<ui::GaugeBar> {
        let band = self.band?;
        Some(ui::GaugeBar {
            band: {
                let scale = workout::gauge_scale(self.kind, band, ftp, max_hr);
                (workout::gauge_x(band.0, scale, cols), workout::gauge_x(band.1, scale, cols))
            },
            marker: self.marker.map(|m| (m, self.color)),
            zones: workout::gauge_zones(self.kind, band, cols, ftp, max_hr),
        })
    }
}

impl App {
    // -----------------------------------------------------------------------------------------------------------
    // Planned workouts (storage/workout_file.rs) and the player (workout.rs)
    // -----------------------------------------------------------------------------------------------------------

    /// The buzzer for a step start and for the countdown's beeps: (Hz, ms). Beep, beep, beeeep: 500 ms at 2 and 1 s
    /// left, then 1 s as the target changes (the owner's choice).
    pub(crate) const STEP_TONE: [(u32, u32); 1] = [(2500, 1000)];

    /// See [`App::STEP_TONE`].
    pub(crate) const COUNT_TONE: [(u32, u32); 1] = [(2000, 500)];

    /// Reads the workouts planned from today on (none if the card cannot be read).
    pub(crate) fn load_calendar(&mut self, b: &mut impl Board) {
        let vol = &mut emmc::Volume(&mut self.emmc, b);
        let zones = (self.settings.ftp, self.settings.max_hr);
        self.plan.calendar = storage::workout_file::calendar(vol, self.date, zones).unwrap_or_default();
        (self.plan.day, self.plan.zones) = (Some(self.date), zones);
        self.plan.done_key = None;
    }

    /// Reads the calendar again when it is not the one for today and the rider's zones (a new day, a new FTP or
    /// maximum heart rate recolours the profiles); not while the computer holds the card. Not in the first
    /// [`CALENDAR_MS`] after the start (the card is not touched then) unless `asked`.
    pub(crate) fn refresh_calendar(&mut self, b: &mut impl Board, asked: bool) {
        let zones = (self.settings.ftp, self.settings.max_hr);
        let stale = self.plan.day != Some(self.date) || self.plan.zones != zones;
        if stale && !self.emmc.lent() && (asked || self.gps.ms >= CALENDAR_MS) {
            self.load_calendar(b);
        }
    }

    /// Today's planned workout, if any.
    pub(crate) fn today(&self) -> Option<&storage::workout_file::Planned> {
        self.plan.calendar.iter().find(|p| p.day == self.date)
    }

    fn today_index(&self) -> usize {
        self.plan.calendar.iter().position(|p| p.day == self.date).unwrap_or(0)
    }

    /// Planned workout `k` is done: played to its end on its day, or a ride of its day matched it
    /// ([`App::refresh_done`]).
    pub(crate) fn workout_done(&self, k: usize) -> bool {
        let Some(p) = self.plan.calendar.get(k) else { return false };
        self.plan.played == Some((p.day, p.file)) || self.plan.done >> k & 1 != 0
    }

    /// Works out `plan.done` when the date, the calendar or the history changed ([`history::done_mask`]). Not while
    /// the scan has not the rides (no wrong mark), and the last answer stays through a rescan (no flicker).
    pub(crate) fn refresh_done(&mut self) {
        if !self.stock.ready() {
            return;
        }
        let n = self.plan.calendar.iter().count();
        let key = fnv(fnv(0x811c_9dc5, &[self.date.1, self.date.2, n as u8]), &self.stock.index.rev.to_le_bytes());
        if self.plan.done_key == Some(key) {
            return;
        }
        self.plan.done_key = Some(key);
        self.plan.done = (0..n.min(8)).fold(0, |m, k| m | u8::from(self.ride_done(k)) << k);
    }

    /// Whether a saved ride matches calendar entry `k` once its day's workouts each took a ride.
    fn ride_done(&self, k: usize) -> bool {
        let Some(p) = self.plan.calendar.get(k) else { return false };
        let (mut want, mut rides) = ([0u32; storage::workout_file::DAYS], [0u32; 8]);
        let (mut at, mut n) = (None, 0);
        for (i, q) in self.plan.calendar.iter().enumerate().filter(|(_, q)| q.day == p.day) {
            let Some(w) = want.get_mut(n) else { break };
            *w = q.total_s;
            if i == k {
                at = Some(n);
            }
            n += 1;
        }
        // a day with more than 8 rides: only the 8 newest count (a workout needs one of them)
        let mut m = 0;
        for r in (0..history::RIDES).map_while(|i| self.stock.index.get(i)) {
            if (r.start.0, r.start.1, r.start.2) == p.day
                && let Some(s) = rides.get_mut(m)
            {
                *s = r.timer_s;
                m += 1;
            }
        }
        at.is_some_and(|at| history::done_mask(&want[..n], &rides[..m]) >> at & 1 != 0)
    }

    /// The overview's TODAY card whole ([`ui::draw_today`]).
    pub(crate) fn draw_today(&self, c: &mut impl Canvas) {
        let mut hm = Line::<8>::new();
        match (self.today(), self.plan.day) {
            (Some(p), _) => {
                push_hm(&mut hm, p.total_s);
                let name = if p.name.as_str().is_empty() { "WORKOUT" } else { p.name.as_str() };
                ui::draw_today(
                    c,
                    ui::Today::Planned(name, hm.as_str(), &p.prof, self.workout_done(self.today_index())),
                );
            }
            (None, Some(_)) => ui::draw_today(c, ui::Today::Rest),
            (None, None) => ui::draw_today(c, ui::Today::Pending),
        }
    }

    /// A fingerprint of what the TODAY card shows (the date, the workout's name, length and profile).
    pub(crate) fn today_print(&self) -> u32 {
        let h = fnv(0x811c_9dc5, &[self.date.1, self.date.2, u8::from(self.plan.day.is_some())]);
        let Some(p) = self.today() else { return h };
        let h = fnv(fnv(fnv(h, p.name.as_str().as_bytes()), &p.total_s.to_le_bytes()), &p.prof);
        fnv(h, &[u8::from(self.workout_done(self.today_index()))])
    }

    /// Reads planned workout `k` and shows its steps. One workout at a time: while one runs, only its own steps
    /// open (Select on a step then goes to it).
    #[inline(never)] // the workout read lives in this frame, not in the main loop's
    pub(crate) fn open_workout(&mut self, b: &mut impl Board, k: usize) {
        let Some(p) = self.plan.calendar.get(k).copied() else { return };
        let running = self.plan.running();
        if running && self.plan.file != Some(p.file) {
            return self.say("WORKOUT RUNNING");
        }
        if !running {
            let vol = &mut emmc::Volume(&mut self.emmc, b);
            let Some(w) =
                fat::Fat::mount(vol).ok().and_then(|mut fat| storage::workout_file::load(&mut fat, vol, &p.file))
            else {
                return self.say("UNREADABLE");
            };
            (self.plan.workout, self.plan.file, self.plan.player) = (w, Some(p.file), None);
            self.plan.planned = true;
        }
        // running: the step playing is selected (row 0 is START only when it is not)
        let at = self.plan.player.and_then(|pl| pl.step(self.plan.steps())).unwrap_or(0);
        self.open(Screen::Menu(Menu::Steps, at));
    }

    /// The TODAY card's tap and Start held on the overview: today's workout, its steps with START selected; on a
    /// rest day WORKOUTS (the other days).
    pub(crate) fn open_today(&mut self, b: &mut impl Board) {
        self.refresh_calendar(b, true);
        let today = self.plan.calendar.iter().position(|p| p.day == self.date);
        match today {
            Some(k) => self.open_workout(b, k),
            None => self.open(Screen::Menu(Menu::Workouts, 0)),
        }
    }

    /// A workout is playing a step (not done): the ride menu shows WORKOUT.
    pub(crate) fn workout_running(&self) -> bool {
        self.plan.running()
    }

    /// Starts the workout opened from step `at`, and the ride if none runs; the workout page when a trainer is heard
    /// (the bike is on it), else the first ride page that holds a workout field, else page 1.
    pub(crate) fn start_workout(&mut self, at: usize) {
        self.ftms_retry(); // a Bluetooth trainer given up is tried again
        if self.ride.state() == ride::State::Idle {
            self.start_pause();
            if self.ride.state() == ride::State::Idle {
                return; // refused (USB DRIVE ON): no player on a ride that never ticks
            }
            self.new_ride = false; // the page is drawn anew anyway
        }
        self.choose_profile(); // a ride that began before the trainer was heard goes indoor here
        let (p, ev) = workout::Player::start(self.plan.steps(), at, self.ride.dist_m());
        (self.plan.player, self.plan.ran_s) = (Some(p), 0);
        self.workout_event(ev);
        // the first page that shows the workout (the default page 1 has no field for it), else page 1
        let page = if self.indoors() || self.trainer_heard() {
            WORKOUT
        } else {
            (0..self.ride_count())
                .find(|&n| {
                    self.ride_page_ref(n).is_some_and(|p| p.fields.iter().take(p.cells()).any(|f| f.is_workout()))
                })
                .unwrap_or(0)
        };
        (self.depth, self.screen) = (0, Screen::Ride(page));
    }

    /// Shows and sounds what the player said: a banner and a long tone for a new step ("3/14 210-230W"), a short
    /// beep for each of the last 2 seconds.
    pub(crate) fn workout_event(&mut self, ev: workout::Event) {
        if matches!(ev, workout::Event::Step(_) | workout::Event::Done) {
            self.step_lap(); // each interval a lap in the file
        }
        let steps = self.plan.steps();
        let mut l = Line::<20>::new();
        match ev {
            workout::Event::Countdown(_) => return self.tones.play(&Self::COUNT_TONE),
            workout::Event::Done => {
                // the day's mark ([`App::workout_done`]; not kept over a restart): a calendar workout (not the ramp
                // test), run for 80 % of its time at least (Lap through every step is not doing it)
                let (total, file) = (self.plan.workout.total_s(), self.plan.file);
                if self.plan.planned && 5 * u64::from(self.plan.ran_s) >= 4 * u64::from(total) {
                    self.plan.played = file.map(|f| (self.date, f));
                }
                l.push_str("WORKOUT DONE")
            }
            workout::Event::Step(i) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
                l.push_str(" ");
                if let Some(s) = steps.get(i) {
                    self.push_target(&mut l, s, true);
                    if s.len == storage::workout_file::Length::Open {
                        l.push_str(" LAP");
                    }
                }
            }
        }
        self.say_as(l.as_str(), Banner::Ride);
        self.tones.play(&Self::STEP_TONE);
    }

    /// Step `s`'s target: "210-230", with its unit if `unit` ("W", "BPM", "RPM"); "OPEN" for none.
    pub(crate) fn push_target<const N: usize>(&self, l: &mut Line<N>, s: &storage::workout_file::Step, unit: bool) {
        use storage::workout_file::Kind;
        let Some((lo, hi)) = s.target.range(self.settings.ftp, self.settings.max_hr) else {
            return l.push_str("OPEN");
        };
        l.push_u32(u32::from(lo));
        if hi != lo {
            l.push_str("-");
            l.push_u32(u32::from(hi));
        }
        if unit {
            l.push_str(match s.target.kind {
                Kind::Power => "W",
                Kind::Hr => "BPM",
                _ => "RPM",
            });
        }
    }

    /// A target as a field shows it: the middle big and the half-width as a small tail ("210|+-10" for 200-220), so the
    /// value keeps the box's scale (a "200-220" dropped to 2x, the smallest text on the page); one number alone ("150"),
    /// or OPEN, as [`App::push_target`].
    fn push_target_field(&self, l: &mut Line<8>, s: &storage::workout_file::Step) {
        match s.target.range(self.settings.ftp, self.settings.max_hr) {
            Some((lo, hi)) if hi > lo => {
                let (lo, hi) = (u32::from(lo), u32::from(hi));
                l.push_u32((lo + hi) / 2);
                // "1000|+-50" is 9 characters, and the line cuts the 50 to 5: without the sign it is whole
                let half = (hi - lo).div_ceil(2);
                l.push_str(if l.as_str().len() + 3 + if half >= 10 { 2 } else { 1 } > 8 { "|" } else { "|+-" });
                l.push_u32(half);
            }
            _ => self.push_target(l, s, false),
        }
    }

    /// The next step as [`Field::NextStep`] shows it: the middle of its target big, its length as a small tail ("150|0:30",
    /// "210|10M", "150|2.5K", "OPEN|LAP"), in 8 characters.
    fn push_next(&self, l: &mut Line<8>, s: &storage::workout_file::Step) {
        use storage::workout_file::Length;
        let mut head = Line::<8>::new();
        match s.target.range(self.settings.ftp, self.settings.max_hr) {
            Some((lo, hi)) => head.push_u32(u32::from(lo).midpoint(u32::from(hi))),
            None => head.push_str("OPEN"),
        }
        let mut tail = Line::<8>::new();
        match s.len {
            Length::Time(t) if t < 600 => push_clock(&mut tail, t),
            Length::Time(t) if t < 3600 => {
                tail.push_u32(t / 60);
                tail.push_str("M");
            }
            Length::Time(t) => {
                tail.push_u32(t / 3600);
                tail.push_str("H");
            }
            Length::Distance(m) => {
                tail.push_u32(m / 1000);
                tail.push_str(".");
                tail.push_str(ui::DIGITS[(m / 100 % 10) as usize]);
                tail.push_str("K");
            }
            Length::Open => tail.push_str("LAP"),
        }
        l.push_str(head.as_str());
        if head.as_str().len() + 1 + tail.as_str().len() <= 8 {
            l.push_str("|");
            l.push_str(tail.as_str());
        }
    }

    /// A step's menu row: what ends it as the label ("10:00", "2.50 KM", "LAP"), its target as the value, in the
    /// zone colour of the target's middle; the colour.
    pub(crate) fn step_row(&self, s: &storage::workout_file::Step, label: &mut Line<16>, value: &mut Line<12>) -> u16 {
        use storage::workout_file::Length;
        match s.len {
            Length::Time(t) => push_clock(label, t),
            Length::Distance(m) => {
                push_km(label, m);
                label.push_str(" KM");
            }
            Length::Open => label.push_str("LAP"),
        }
        self.push_target(value, s, true);
        workout::level(s, self.settings.ftp, self.settings.max_hr).1
    }

    /// The workout fields' text ([`Field::Workout`], [`Field::StepTarget`], [`Field::StepTime`],
    /// [`Field::NextStep`]): "--" without a workout running.
    pub(crate) fn workout_text(&self, f: Field) -> Line<8> {
        use workout::Left;
        let mut l = Line::new();
        let steps = self.plan.steps();
        let now = self.plan.player.and_then(|p| p.step(steps).map(|i| (p, i)));
        match (f, now) {
            (Field::Workout, Some((_, i))) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
            }
            (Field::Workout, None) if self.plan.player.is_some() => l.push_str("DONE"),
            (Field::Workout, None) => match self.today() {
                Some(p) => push_hm(&mut l, p.total_s),
                None => l.push_str("--"),
            },
            (Field::StepTarget, Some((_, i))) => self.push_target_field(&mut l, steps.get(i).unwrap_or(&OPEN)),
            (Field::NextStep, Some((_, i))) => match steps.get(i + 1) {
                Some(s) => self.push_next(&mut l, s),
                None => l.push_str("END"),
            },
            (Field::WorkoutLeft, Some((p, _))) => {
                let total = workout::graph_total_s(steps);
                push_duration(&mut l, total.saturating_sub(p.graph_at(steps, self.ride.dist_m())), false);
            }
            (Field::StepTime, Some((p, _))) => match p.left(steps, self.ride.dist_m()) {
                Left::Time(t) => push_duration(&mut l, t, false),
                Left::Distance(m) => push_km(&mut l, m),
                Left::Lap => l.push_str("LAP"),
                Left::Done => l.push_str("--"),
            },
            _ => l.push_str("--"),
        }
        l
    }

    /// [`Field::StepTarget`]'s edge bar: how the reading (power over the GAUGE POWER window, heart rate, cadence) sits against the band, as the
    /// gauge's marker ([`workout::gauge_color`]); BLACK without a reading or a target.
    pub(crate) fn target_bar(&self) -> u16 {
        use storage::workout_file::Kind;
        let steps = self.plan.steps();
        let Some(s) = self.plan.player.and_then(|p| p.step(steps)).and_then(|i| steps.get(i)) else { return ui::BLACK };
        let reading = match s.target.kind {
            // the gauge's window (GAUGE POWER), so the bar and the marker agree
            Kind::Power => self.power.rolling(self.settings.wkt_avg),
            Kind::Hr => self.live.get(Metric::Hr),
            Kind::Cadence => self.live.get(Metric::Cadence),
            Kind::Open => None,
        };
        match (reading, s.target.range(self.settings.ftp, self.settings.max_hr)) {
            (Some(v), Some(band)) => workout::gauge_color(v, band),
            _ => ui::BLACK,
        }
    }

    /// What the workout gauge in a box with `cols` bar columns shows now: the step's reading (power over the window
    /// set in the menu, heart rate, cadence), coloured by how far it is from the band ([`workout::gauge_color`]); with
    /// no band (no workout, an open step) the power in its zone's colour.
    pub(crate) fn gauge_now(&self, cols: u32) -> GaugeShown {
        use storage::workout_file::Kind;
        let steps = self.plan.steps();
        let step = self.plan.player.and_then(|p| p.step(steps)).and_then(|i| steps.get(i));
        let (kind, band) = workout::gauge_target(step, self.settings.ftp, self.settings.max_hr);
        let avg = self.settings.wkt_avg;
        let reading = match kind {
            Kind::Hr => self.live.get(Metric::Hr),
            Kind::Cadence => self.live.get(Metric::Cadence),
            _ => self.power.rolling(avg),
        };
        let mut value = Line::new();
        match reading {
            Some(v) => value.push_u32(u32::from(v)),
            None => value.push_str("--"),
        }
        let color = match (reading, band) {
            (Some(v), Some(b)) => workout::gauge_color(v, b),
            (Some(w), None) => ride::power_zone_color(w, self.settings.ftp),
            (None, _) => ui::WHITE,
        };
        let (ftp, max_hr) = (self.settings.ftp, self.settings.max_hr);
        let marker =
            reading.zip(band).map(|(v, b)| workout::gauge_x(v, workout::gauge_scale(kind, b, ftp, max_hr), cols));
        GaugeShown { kind, avg, band, step: step.is_some(), pace: None, value, color, marker }
    }

    /// A gauge's title, and its tag (the pacer's target in a box too short for the target line). The pacer's: "TOP"
    /// and the time left to the top.
    fn gauge_title(g: &GaugeShown, h: u32) -> (Line<12>, Line<4>) {
        use storage::workout_file::Kind;
        let (mut title, mut tag) = (Line::<12>::new(), Line::<4>::new());
        match g.kind {
            _ if g.pace.is_some() => {
                title.push_str("TOP");
                if let Some(s) = g.pace.filter(|&s| s > 0) {
                    title.push_str(" ");
                    title.push_u32(u32::from(s) / 60);
                    title.push_str(":");
                    title.push_u32(u32::from(s) / 10 % 6);
                    title.push_u32(u32::from(s) % 10);
                }
                if let Some((lo, hi)) = g.band.filter(|_| h < 100) {
                    tag.push_u32((u32::from(lo) + u32::from(hi)) / 2);
                }
            }
            Kind::Hr => title.push_str("HR"),
            Kind::Cadence => title.push_str("RPM"),
            _ => {
                title.push_str("POWER ");
                title.push_str(WKT_AVGS.get(g.avg).copied().unwrap_or(""));
            }
        }
        (title, tag)
    }

    /// The workout gauge in box `bx` whole, showing `g`: the title says what and over which window ("POWER 3S",
    /// "HR"), the target under the bar ("200-220"; OPEN for a step without one).
    pub(crate) fn draw_gauge(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), g: GaugeShown) {
        let (title, tag) = Self::gauge_title(&g, bx.3);
        let mut target = Line::<12>::new();
        match g.band {
            Some((lo, hi)) => {
                target.push_u32(u32::from(lo));
                if hi != lo {
                    target.push_str("-");
                    target.push_u32(u32::from(hi));
                }
            }
            None if g.step => target.push_str("OPEN"),
            None => {}
        }
        let bar = g.bar(ui::gauge_cols(bx), (self.settings.ftp, self.settings.max_hr));
        ui::draw_gauge(c, bx, (title.as_str(), tag.as_str(), target.as_str()), g.value.as_str(), ui::WHITE, bar);
    }

    /// The workout gauge drawn as `old`: whole for another step, window or reading kind; else only the number's
    /// characters that changed (all of it in a new colour) and the marker's old and new columns.
    pub(crate) fn update_gauge(&self, b: &mut impl Board, old: GaugeShown, bx: (u32, u32, u32, u32)) -> Drawn {
        let cols = ui::gauge_cols(bx);
        let now = if old.pace.is_some() { self.pace_now(cols) } else { self.gauge_now(cols) };
        let mut p = Panel(b, self.area(), self.look());
        if !now.same_frame(&old) {
            self.draw_gauge(&mut p, bx, now);
            return Drawn::Gauge(now);
        }
        if now.pace != old.pace {
            // the time counts down: the digits that changed
            let ((was, _), (title, tag)) = (Self::gauge_title(&old, bx.3), Self::gauge_title(&now, bx.3));
            ui::update_gauge_title(&mut p, bx, (was.as_str(), title.as_str()), tag.as_str());
        }
        if now.value != old.value {
            ui::draw_gauge_value(&mut p, bx, old.value.as_str(), now.value.as_str(), ui::WHITE); // the marker has the state
        }
        let zones = (self.settings.ftp, self.settings.max_hr);
        if let Some(bar) = now.bar(cols, zones).filter(|_| (now.marker, now.color) != (old.marker, old.color)) {
            ui::draw_gauge_marker(&mut p, bx, old.marker.map(|m| (m, old.color)), bar);
        }
        Drawn::Gauge(now)
    }
}
