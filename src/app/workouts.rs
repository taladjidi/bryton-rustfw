//! Stock's planned workouts: the calendar read from the card, a workout opened and started, its player fed by the
//! ride's seconds, the step fields and their target colours, the workout gauge field ([`Field::WktPower`]), and the
//! tones at a step's start and count-down.

use crate::*;

/// No step: what a workout field shows past the last one.
pub(crate) const OPEN: storage::workout::Step = storage::workout::Step {
    len: storage::workout::Length::Open,
    target: storage::workout::Target::OPEN,
    intensity: 0,
};

/// What the workout gauge field shows ([`Field::WktPower`]): what it measures and over which window, the step's
/// band (None: no gauge) and whether a step runs, which change it whole; then the number, its colour and the marker's
/// column, which are redrawn alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GaugeShown {
    kind: storage::workout::Kind,
    avg: usize,
    band: Option<(u16, u16)>,
    step: bool,
    pub(crate) value: Line<8>,
    pub(crate) color: u16,
    marker: Option<u32>,
}

impl GaugeShown {
    /// The same field with only the number or the marker moved.
    fn same_frame(&self, o: &GaugeShown) -> bool {
        (self.kind, self.avg, self.band, self.step) == (o.kind, o.avg, o.band, o.step)
    }

    /// Its bar for a gauge `cols` wide: the band's columns, the marker in the number's colour.
    fn bar(&self, cols: u32) -> Option<ui::GaugeBar> {
        let band = self.band?;
        let cols_of = (workout::gauge_x(band.0, band, cols), workout::gauge_x(band.1, band, cols));
        Some((cols_of, self.marker.map(|m| (m, self.color))))
    }
}

impl App {
    // -----------------------------------------------------------------------------------------------------------
    // Planned workouts (storage/workout.rs) and the player (workout.rs)
    // -----------------------------------------------------------------------------------------------------------

    /// The buzzer for a step start and for the countdown's beeps: (Hz, ms).
    pub(crate) const STEP_TONE: [(u32, u32); 1] = [(2500, 400)];

    pub(crate) const COUNT_TONE: [(u32, u32); 1] = [(2000, 100)];

    /// Reads the workouts planned from today on (none if the card cannot be read).
    pub(crate) fn load_calendar(&mut self, b: &mut impl Board) {
        let vol = &mut emmc::Volume(&mut self.emmc, b);
        self.calendar = storage::workout::calendar(vol, self.date).unwrap_or_default();
        self.calendar_day = Some(self.date);
    }

    /// Reads planned workout `k` and shows its steps. One workout at a time: while one runs, only its own steps
    /// open (Select on a step then goes to it).
    #[inline(never)] // the workout read lives in this frame, not in the main loop's
    pub(crate) fn open_workout(&mut self, b: &mut impl Board, k: usize) {
        let Some(p) = self.calendar.get(k).copied() else { return };
        let running = self.player.is_some_and(|pl| pl.step(self.plan.steps()).is_some());
        if running && self.plan_file != Some(p.file) {
            return self.say("WORKOUT RUNNING");
        }
        if !running {
            let vol = &mut emmc::Volume(&mut self.emmc, b);
            let Some(w) = fat::Fat::mount(vol).ok().and_then(|mut fat| storage::workout::load(&mut fat, vol, &p.file))
            else {
                return self.say("UNREADABLE");
            };
            (self.plan, self.plan_file, self.player) = (w, Some(p.file), None);
        }
        self.open(Screen::Menu(Menu::Steps, 0));
    }

    /// Starts the workout opened from step `at`, and the ride if none runs; the first ride page.
    pub(crate) fn start_workout(&mut self, at: usize) {
        if self.ride.state() == ride::State::Idle {
            self.start_pause();
            self.new_ride = false; // the page is drawn anew anyway
        }
        let (p, ev) = workout::Player::start(self.plan.steps(), at, self.ride.dist_m());
        self.player = Some(p);
        self.workout_event(ev);
        (self.depth, self.screen) = (0, Screen::Ride(0));
    }

    /// Shows and sounds what the player said: a banner and a long tone for a new step ("3/14 210-230W"), a short
    /// beep for each of the last 3 seconds.
    pub(crate) fn workout_event(&mut self, ev: workout::Event) {
        let steps = self.plan.steps();
        let mut l = Line::<20>::new();
        match ev {
            workout::Event::Countdown(_) => return self.tones.play(&Self::COUNT_TONE),
            workout::Event::Done => l.push_str("WORKOUT DONE"),
            workout::Event::Step(i) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
                l.push_str(" ");
                if let Some(s) = steps.get(i) {
                    self.push_target(&mut l, s, true);
                    if s.len == storage::workout::Length::Open {
                        l.push_str(" LAP");
                    }
                }
            }
        }
        self.say(l.as_str());
        self.tones.play(&Self::STEP_TONE);
    }

    /// Step `s`'s target: "210-230", with its unit if `unit` ("W", "BPM", "RPM"); "OPEN" for none.
    pub(crate) fn push_target<const N: usize>(&self, l: &mut Line<N>, s: &storage::workout::Step, unit: bool) {
        use storage::workout::Kind;
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

    /// A step's menu row: what ends it as the label ("10:00", "2.50 KM", "LAP"), its target as the value, in the
    /// zone colour of the target's middle; the colour.
    pub(crate) fn step_row(&self, s: &storage::workout::Step, label: &mut Line<16>, value: &mut Line<12>) -> u16 {
        use storage::workout::Length;
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
        let now = self.player.and_then(|p| p.step(steps).map(|i| (p, i)));
        match (f, now) {
            (Field::Workout, Some((_, i))) => {
                l.push_u32(i as u32 + 1);
                l.push_str("/");
                l.push_u32(steps.len() as u32);
            }
            (Field::Workout, None) if self.player.is_some() => l.push_str("DONE"),
            (Field::Workout, None) => match self.calendar.iter().find(|p| p.day == self.date) {
                Some(p) => push_hm(&mut l, p.total_s),
                None => l.push_str("--"),
            },
            (Field::StepTarget, Some((_, i))) => self.push_target(&mut l, steps.get(i).unwrap_or(&OPEN), false),
            (Field::NextStep, Some((_, i))) => match steps.get(i + 1) {
                Some(s) => self.push_target(&mut l, s, false),
                None => l.push_str("END"),
            },
            (Field::StepTime, Some((p, _))) => match p.left(steps, self.ride.dist_m()) {
                Left::Time(t) => push_clock(&mut l, t),
                Left::Distance(m) => push_km(&mut l, m),
                Left::Lap => l.push_str("LAP"),
                Left::Done => l.push_str("--"),
            },
            _ => l.push_str("--"),
        }
        l
    }

    /// [`Field::StepTarget`]'s colour: green in the range, orange under, red over (3 s power, heart rate, cadence);
    /// white without a reading or a target.
    pub(crate) fn target_color(&self) -> u16 {
        use storage::workout::Kind;
        let steps = self.plan.steps();
        let Some(s) = self.player.and_then(|p| p.step(steps)).and_then(|i| steps.get(i)) else { return ui::WHITE };
        let reading = match s.target.kind {
            Kind::Power => self.power.rolling(0),
            Kind::Hr => self.live.get(Metric::Hr),
            Kind::Cadence => self.live.get(Metric::Cadence),
            Kind::Open => None,
        };
        match (reading, s.target.range(self.settings.ftp, self.settings.max_hr)) {
            (Some(v), Some((lo, _))) if v < lo => ui::ACCENT,
            (Some(v), Some((_, hi))) if v > hi => ui::RED,
            (Some(_), Some(_)) => ui::GREEN,
            _ => ui::WHITE,
        }
    }

    /// What the workout gauge in a box with `cols` bar columns shows now: the step's reading (power over the window
    /// set in the menu, heart rate, cadence), coloured by how far it is from the band ([`workout::gauge_color`]); with
    /// no band (no workout, an open step) the power in its zone's colour.
    pub(crate) fn gauge_now(&self, cols: u32) -> GaugeShown {
        use storage::workout::Kind;
        let steps = self.plan.steps();
        let step = self.player.and_then(|p| p.step(steps)).and_then(|i| steps.get(i));
        let (kind, band) = workout::gauge_target(step, self.settings.ftp, self.settings.max_hr);
        let avg = self.settings.wkt_avg;
        let reading = match kind {
            Kind::Hr => self.live.get(Metric::Hr),
            Kind::Cadence => self.live.get(Metric::Cadence),
            _ if avg < 3 => self.power.rolling(avg),
            _ => self.live.get(Metric::Power),
        };
        let mut value = Line::new();
        match reading {
            Some(v) => value.push_u32(u32::from(v)),
            None => value.push_str("--"),
        }
        let color = match (reading, band) {
            (Some(v), Some(b)) => workout::gauge_color(v, b),
            (Some(w), None) => ride::power_color(ride::power_zone(w, self.settings.ftp)),
            (None, _) => ui::WHITE,
        };
        let marker = reading.zip(band).map(|(v, b)| workout::gauge_x(v, b, cols));
        GaugeShown { kind, avg, band, step: step.is_some(), value, color, marker }
    }

    /// The workout gauge in box `bx` whole, showing `g`: the title says what and over which window ("POWER 3S",
    /// "HR"), the target under the bar ("200-220"; OPEN for a step without one).
    pub(crate) fn draw_gauge(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), g: GaugeShown) {
        use storage::workout::Kind;
        let mut title = Line::<12>::new();
        match g.kind {
            Kind::Hr => title.push_str("HR"),
            Kind::Cadence => title.push_str("RPM"),
            _ => {
                title.push_str("POWER ");
                title.push_str(WKT_AVGS.get(g.avg).copied().unwrap_or(""));
            }
        }
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
        let bar = g.bar(ui::gauge_cols(bx));
        ui::draw_gauge(c, bx, (title.as_str(), target.as_str()), g.value.as_str(), g.color, bar);
    }

    /// The workout gauge drawn as `old`: whole for another step, window or reading kind; else only the number's
    /// characters that changed (all of it in a new colour) and the marker's old and new columns.
    pub(crate) fn update_gauge(&mut self, b: &mut impl Board, old: GaugeShown, bx: (u32, u32, u32, u32)) -> Drawn {
        let cols = ui::gauge_cols(bx);
        let now = self.gauge_now(cols);
        let mut p = Panel(b, (0, self.bottom()));
        if !now.same_frame(&old) {
            self.draw_gauge(&mut p, bx, now);
            return Drawn::Gauge(now);
        }
        if (now.value, now.color) != (old.value, old.color) {
            let was = if now.color == old.color { old.value.as_str() } else { "" };
            ui::draw_gauge_value(&mut p, bx, was, now.value.as_str(), now.color);
        }
        if let Some(bar) = now.bar(cols).filter(|_| (now.marker, now.color) != (old.marker, old.color)) {
            ui::draw_gauge_marker(&mut p, bx, old.bar(cols).and_then(|b| b.1), bar);
        }
        Drawn::Gauge(now)
    }
}
