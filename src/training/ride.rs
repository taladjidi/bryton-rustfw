//! The ride: Start / pause / laps / end, auto-pause, the totals the ride fields show, heart-rate and power zones,
//! and the history behind the graph field. Pure logic, fed once a second by the app.
//!
//! **Climbing** ([`Climb`]): VAM is the metres climbed per hour (Ferrari's *velocità ascensionale media*, as
//! Garmin and Wahoo show it), here over the last minute of moving time; grade is rise over run, x 100, over the
//! last 30 s of moving time and at least 20 m (shorter, GPS altitude noise of a metre or two would swing it by
//! several percent). Both from the barometer's altitude when it reads (altitude.rs, a reading a second, steadier), else from GPS's.
//!
//! **Recorder hook.** Nothing here is stored: every change of state and every second of ride comes out as an
//! [`Event`], and the app hands each one to one place (`App::recorder` in lib.rs), which feeds the recorder
//! (storage/recorder/mod.rs). [`Ride::to_words`] goes into its recovery record, and [`Ride::resumed`] brings a ride back after
//! a reset.

/// Where the ride is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not started (or ended): the fields keep the last ride's totals until the next Start.
    Idle,
    /// The timer runs (unless auto-paused: see [`Ride::moving`]).
    Running,
    /// Paused with Start: a long Start ends the ride, a short one resumes.
    Paused,
}

/// What the recorder gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A new ride (the totals were reset): open the file.
    Start,
    /// Paused (timer stop event).
    Pause,
    /// Resumed (timer start event).
    Resume,
    /// A lap ended.
    Lap,
    /// The ride ended: finish the file.
    End,
    /// One second of a running ride: a record.
    Second,
}

/// One second of input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    /// km/h x 10 (the wheel sensor when paired, else the GPS).
    pub speed_x10: Option<u16>,
    /// Beats per minute.
    pub hr: Option<u16>,
    /// rpm.
    pub cadence: Option<u16>,
    /// W.
    pub power: Option<u16>,
    /// The power meter's balance: the right pedal's share, %, when it reports one (drivers/sensors.rs).
    pub balance: Option<u16>,
    /// Altitude, mm: the barometer's (calibrated) when it reads, else a good fix's.
    pub alt_mm: Option<i32>,
    /// Which altitude that is: 0 the receiver's, else the barometer on one calibration (`altitude.rs`). A change
    /// forgets the low point and the climb window (the two may not be on the same level).
    pub alt_ref: u8,
    /// The part of this second's altitude change that is a correction (a calibration's shift let go:
    /// `altitude::Altimeter::corr`), not a climb: moved out of the low point and the climb window, paused too.
    pub alt_corr_mm: i32,
    /// The ride's profile: its auto-pause. Indoors on no power and no cadence, not on speed (a trainer's flywheel
    /// speed lags, and some trainers report a speed at 0 rpm: inferred); MTB on a speed under [`AUTO_PAUSE_X10`] for
    /// [`MTB_PAUSE_S`] in a row (a slow climb or a trail junction is not a stop); ROAD on the speed at once.
    pub profile: Profile,
    /// AUTO PAUSE: 0 the profile's own rule, 255 never paused, else the km/h under which the ride is paused
    /// (`settings::AUTO_PAUSE_KMH`).
    pub pause_kmh: u8,
}

/// Words of [`Ride::to_words`].
pub const RIDE_WORDS: usize = 16;

/// Under this speed (km/h x 10) the ride is auto-paused: the timer stops, nothing is added.
pub const AUTO_PAUSE_X10: u16 = 20;

/// An indoor ride is auto-paused after this many seconds with no power and no cadence.
pub const INDOOR_PAUSE_S: u8 = 3;

/// An MTB ride is auto-paused after this many seconds in a row under [`AUTO_PAUSE_X10`] (the owner's choice,
/// 2026-10-27).
pub const MTB_PAUSE_S: u8 = 5;

/// Altitude gain is counted once it exceeds the last low point by this much (GPS altitude is noisy).
pub const ASCENT_HYSTERESIS_MM: i32 = 5000;

/// The same for the barometer. Measured on a real 2 h ride (2026-10-26, altitude.rs's filter on its readings): the
/// filtered altitude stays within -1.4 to +1.3 m of a 31 s centred mean (0.5 % to 99.5 %), so 3 m is over that noise
/// from crest to trough; 2 m counted 150 m on that ride where 3 m counted 120 m and the smoothed profile about 120 m.
/// Golden Cheetah's default elevation hysteresis is 3 m too (its wiki, UG_Preferences_General); Strava counts 2 m
/// with barometric data after its own smoothing (support.strava.com/hc/en-us/articles/216919447).
pub const BARO_HYSTERESIS_MM: i32 = 3000;

/// Uphill and downhill distance count the moving seconds at a grade of at least this much (per mille, as
/// [`Ride::grade_x10`]: 10 is 1 %, the grade of the last 30 s). Under 1 % a road is flat for a rider.
pub const SLOPE_X10: i32 = 10;

/// A sum and a count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Avg {
    sum: u32,
    n: u32,
}

impl Avg {
    fn add(&mut self, v: Option<u16>) {
        if let Some(v) = v {
            self.sum = self.sum.saturating_add(u32::from(v));
            self.n = self.n.saturating_add(1);
        }
    }

    fn get(&self) -> Option<u16> {
        (self.n > 0).then(|| (self.sum / self.n) as u16)
    }
}

/// The ride's state and totals. Distance is kept in 1/9 mm: km/h x 10 for one second is exactly 250 of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ride {
    state: State,
    moving: bool,
    /// Seconds in a row with no power and no cadence (indoor auto-pause), or under [`AUTO_PAUSE_X10`] (MTB's).
    still_s: u8,
    timer_s: u32,
    dist9: u64,
    max_speed: u16,
    hr: Avg,
    power: Avg,
    cadence: Avg,
    ascent_mm: u32,
    alt_low: Option<i32>,
    alt_ref: u8,
    lap_timer_s: u32,
    lap_dist9: u64,
    max_hr: u16,
    climb: Climb,
    /// Seconds running, auto-pauses included (a Start pause is not): the trip time.
    elapsed_s: u32,
    laps: u32,
    last_lap_s: Option<u32>,
    last_lap_w: Option<u16>,
    /// The lap that ended: its distance (m) and average heart rate, for the lap banner.
    last_lap_m: u32,
    last_lap_hr: Option<u16>,
    /// This lap's heart rate while moving.
    lap_hr: Avg,
    /// The lap's power, its own [`Power`]: average and NP over the lap's moving seconds.
    lap_power: crate::power::Power,
    lap_ascent_mm: u32,
    lap_descent_mm: u32,
    descent_mm: u32,
    /// The highest point since the last low point's hysteresis: the descent counts from it.
    alt_high: Option<i32>,
    max_cadence: u16,
    max_alt_mm: Option<i32>,
    /// Distance (1/9 mm) at a grade up to +1 % ([`SLOPE_X10`]), and down to -1 %.
    up9: u64,
    down9: u64,
    /// The balance's right share, % (the moving seconds with a reading).
    balance: Avg,
}

impl Default for Ride {
    fn default() -> Self {
        Self::new()
    }
}

impl Ride {
    /// No ride yet.
    pub const fn new() -> Self {
        Ride {
            state: State::Idle,
            moving: false,
            still_s: 0,
            timer_s: 0,
            dist9: 0,
            max_speed: 0,
            hr: Avg { sum: 0, n: 0 },
            power: Avg { sum: 0, n: 0 },
            cadence: Avg { sum: 0, n: 0 },
            ascent_mm: 0,
            alt_low: None,
            alt_ref: 0,
            lap_timer_s: 0,
            lap_dist9: 0,
            max_hr: 0,
            climb: Climb::new(),
            elapsed_s: 0,
            laps: 0,
            last_lap_s: None,
            last_lap_w: None,
            last_lap_m: 0,
            last_lap_hr: None,
            lap_hr: Avg { sum: 0, n: 0 },
            lap_power: crate::power::Power::new(),
            lap_ascent_mm: 0,
            lap_descent_mm: 0,
            descent_mm: 0,
            alt_high: None,
            max_cadence: 0,
            max_alt_mm: None,
            up9: 0,
            down9: 0,
            balance: Avg { sum: 0, n: 0 },
        }
    }

    /// The Start key: starts a new ride, pauses, or resumes.
    pub fn start_pause(&mut self) -> Event {
        match self.state {
            State::Idle => {
                *self = Ride { state: State::Running, moving: true, ..Ride::new() };
                Event::Start
            }
            State::Running => {
                self.state = State::Paused;
                Event::Pause
            }
            State::Paused => {
                self.state = State::Running;
                Event::Resume
            }
        }
    }

    /// The Lap key: a new lap while running.
    pub fn lap(&mut self) -> Option<Event> {
        (self.state == State::Running).then(|| {
            // the lap that ends keeps its time and average; the new one starts from zero
            (self.last_lap_s, self.last_lap_w) = (Some(self.lap_s()), self.lap_power.avg());
            (self.last_lap_m, self.last_lap_hr, self.lap_hr) =
                (self.lap_dist_m(), self.lap_hr.get(), Avg { sum: 0, n: 0 });
            self.laps = self.laps.saturating_add(1);
            (self.lap_timer_s, self.lap_dist9) = (self.timer_s, self.dist9);
            (self.lap_power, self.lap_ascent_mm, self.lap_descent_mm) = (crate::power::Power::new(), 0, 0);
            Event::Lap
        })
    }

    /// A long Start: ends a paused ride (the totals stay on screen).
    pub fn end(&mut self) -> Option<Event> {
        (self.state == State::Paused).then(|| {
            self.state = State::Idle;
            Event::End
        })
    }

    /// One second: the timer and the totals move if the ride runs and is moving. [`Event::Second`] while running.
    pub fn second(&mut self, i: &Input) -> Option<Event> {
        if self.state != State::Idle && i.alt_corr_mm != 0 {
            if let Some(low) = &mut self.alt_low {
                *low = low.saturating_add(i.alt_corr_mm);
            }
            self.alt_high = self.alt_high.map(|h| h.saturating_add(i.alt_corr_mm));
            self.max_alt_mm = self.max_alt_mm.map(|h| h.saturating_add(i.alt_corr_mm));
            self.climb.shift(i.alt_corr_mm);
        }
        if self.state != State::Running {
            return None;
        }
        self.elapsed_s = self.elapsed_s.saturating_add(1);
        // ponytail: no speed at all (no sensor, no fix) keeps the timer running; a trainer with power only works
        let under = match i.pause_kmh {
            0 => AUTO_PAUSE_X10,
            255 => 0,
            k => u16::from(k) * 10,
        };
        let slow = i.speed_x10.is_some_and(|v| v < under);
        match i.profile {
            Profile::Indoor => {
                // pedalling is power or cadence; neither reported at all (no sensor) keeps the timer running
                let still = i.power.is_none_or(|p| p == 0) && i.cadence.is_none_or(|c| c == 0);
                let heard = i.power.is_some() || i.cadence.is_some();
                self.still_s = if still && heard { self.still_s.saturating_add(1) } else { 0 };
                self.moving = self.still_s < INDOOR_PAUSE_S;
            }
            Profile::Mtb => {
                self.still_s = if slow { self.still_s.saturating_add(1) } else { 0 };
                self.moving = self.still_s < MTB_PAUSE_S;
            }
            Profile::Road => {
                self.still_s = 0;
                self.moving = !slow;
            }
        }
        if i.pause_kmh == 255 {
            (self.still_s, self.moving) = (0, true); // AUTO PAUSE OFF: the timer runs whatever the profile
        }
        if self.moving {
            self.timer_s = self.timer_s.saturating_add(1);
            let v = i.speed_x10.unwrap_or(0);
            let step9 = u64::from(v) * 250;
            self.dist9 = self.dist9.saturating_add(step9);
            self.max_speed = self.max_speed.max(v);
            self.hr.add(i.hr);
            self.lap_hr.add(i.hr);
            self.max_hr = self.max_hr.max(i.hr.unwrap_or(0));
            // an altitude no road has (a receiver's garbage) is no altitude: every difference below stays in range
            let alt_mm = i.alt_mm.filter(|a| ALT_RANGE_MM.contains(a));
            if i.alt_ref != self.alt_ref {
                (self.alt_ref, self.alt_low, self.alt_high, self.max_alt_mm) = (i.alt_ref, None, None, None);
                self.climb.second(None, self.dist9); // starts again
            }
            let hysteresis = if self.alt_ref == 0 { ASCENT_HYSTERESIS_MM } else { BARO_HYSTERESIS_MM };
            self.climb.second(alt_mm, self.dist9);
            // the grade of the climbing window (None until it spans 20 m) and its rise or fall past the ascent's
            // hysteresis: flat noise is no slope, so the second's distance goes up or down only with both
            if let (Some(g), Some(rise)) = (self.climb.grade_x10(), self.climb.rise_mm()) {
                if g >= SLOPE_X10 && rise >= hysteresis {
                    self.up9 = self.up9.saturating_add(step9);
                } else if g <= -SLOPE_X10 && rise <= -hysteresis {
                    self.down9 = self.down9.saturating_add(step9);
                }
            }
            self.power.add(i.power); // zeros count: coasting is part of the average power
            self.balance.add(i.balance.filter(|&b| b <= 100));
            self.cadence.add(i.cadence.filter(|&c| c > 0)); // zeros do not: coasting is not a cadence
            self.max_cadence = self.max_cadence.max(i.cadence.unwrap_or(0));
            self.lap_power.sample(i.power, true);
            if let Some(alt) = alt_mm {
                self.max_alt_mm = Some(self.max_alt_mm.map_or(alt, |m| m.max(alt)));
                let low = self.alt_low.get_or_insert(alt);
                if alt < *low {
                    *low = alt;
                } else if alt - *low >= hysteresis {
                    let up = (alt - *low) as u32;
                    self.ascent_mm = self.ascent_mm.saturating_add(up);
                    self.lap_ascent_mm = self.lap_ascent_mm.saturating_add(up);
                    *low = alt;
                }
                // the same with the high point: a fall of the hysteresis is descent
                let high = self.alt_high.get_or_insert(alt);
                if alt > *high {
                    *high = alt;
                } else if high.saturating_sub(alt) >= hysteresis {
                    let down = high.saturating_sub(alt) as u32;
                    self.descent_mm = self.descent_mm.saturating_add(down);
                    self.lap_descent_mm = self.lap_descent_mm.saturating_add(down);
                    *high = alt;
                }
            }
        }
        Some(Event::Second)
    }

    /// The average right pedal share, % (the power meter's balance over the moving seconds that had one).
    pub fn balance_right(&self) -> Option<u16> {
        self.balance.get()
    }

    /// The totals as words, for the recorder's recovery record ([`Ride::resumed`] reads them back).
    pub fn to_words(&self) -> [u32; RIDE_WORDS] {
        let (d, l) = (self.dist9, self.lap_dist9);
        [
            self.timer_s,
            d as u32,
            (d >> 32) as u32,
            u32::from(self.max_speed),
            self.hr.sum,
            self.hr.n,
            self.power.sum,
            self.power.n,
            self.cadence.sum,
            self.cadence.n,
            self.ascent_mm,
            self.lap_timer_s,
            l as u32,
            (l >> 32) as u32,
            u32::from(self.max_hr),
            0,
        ]
    }

    /// A ride from [`Ride::to_words`], paused (a ride resumed after a reset: Start goes on, a long Start ends it).
    /// The climbing window starts again.
    pub fn resumed(w: &[u32; RIDE_WORDS]) -> Ride {
        let avg = |i: usize| Avg { sum: w[i], n: w[i + 1] };
        let dist9 = u64::from(w[1]) | u64::from(w[2]) << 32;
        let lap_dist9 = (u64::from(w[12]) | u64::from(w[13]) << 32).min(dist9);
        Ride {
            state: State::Paused,
            timer_s: w[0],
            elapsed_s: w[0], // the trip time starts from the moving time: it is never less than the timer's
            dist9,
            max_speed: w[3] as u16,
            hr: avg(4),
            power: avg(6),
            cadence: avg(8),
            ascent_mm: w[10],
            lap_timer_s: w[11].min(w[0]),
            lap_dist9,
            max_hr: w[14] as u16,
            ..Ride::new()
        }
    }

    /// Where the ride is.
    pub fn state(&self) -> State {
        self.state
    }

    /// Running and not auto-paused.
    pub fn moving(&self) -> bool {
        self.state == State::Running && self.moving
    }

    /// Moving time, seconds.
    pub fn timer_s(&self) -> u32 {
        self.timer_s
    }

    /// Distance, metres.
    pub fn dist_m(&self) -> u32 {
        (self.dist9 / 9000).min(u64::from(u32::MAX)) as u32
    }

    /// Average moving speed, km/h x 10.
    pub fn avg_speed_x10(&self) -> Option<u16> {
        // 1/9 mm per s to km/h x 10: x 36 / 9000 = / 250
        (self.timer_s > 0).then(|| (self.dist9 / 250 / u64::from(self.timer_s)).min(u64::from(u16::MAX)) as u16)
    }

    /// Top speed, km/h x 10.
    pub fn max_speed_x10(&self) -> u16 {
        self.max_speed
    }

    /// Average heart rate while moving.
    pub fn avg_hr(&self) -> Option<u16> {
        self.hr.get()
    }

    /// Max heart rate while moving (None before a reading).
    pub fn max_hr(&self) -> Option<u16> {
        (self.max_hr > 0).then_some(self.max_hr)
    }

    /// VAM, m/h over the last minute of moving time.
    pub fn vam(&self) -> Option<i32> {
        self.climb.vam()
    }

    /// Grade, % x 10, over the last 30 s of moving time.
    pub fn grade_x10(&self) -> Option<i32> {
        self.climb.grade_x10()
    }

    /// Average power while moving (zeros included).
    pub fn avg_power(&self) -> Option<u16> {
        self.power.get()
    }

    /// Average cadence while moving (zeros left out).
    pub fn avg_cadence(&self) -> Option<u16> {
        self.cadence.get()
    }

    /// Altitude gained, metres.
    pub fn ascent_m(&self) -> u32 {
        self.ascent_mm / 1000
    }

    /// Seconds running since the start, auto-pauses included (a Start pause is not). Over a resume it starts from the
    /// moving time (the timer), the stops before the reset not being in the recovery record.
    pub fn elapsed_s(&self) -> u32 {
        self.elapsed_s
    }

    /// Laps taken (the Lap key, a workout's steps). Not kept over a resume.
    pub fn laps(&self) -> u32 {
        self.laps
    }

    /// The last lap's moving time, seconds (None before a lap).
    pub fn last_lap_s(&self) -> Option<u32> {
        self.last_lap_s
    }

    /// The last lap's average power, W (None before a lap with power).
    pub fn last_lap_avg_power(&self) -> Option<u16> {
        self.last_lap_w
    }

    /// The last lap's distance, metres.
    pub fn last_lap_dist_m(&self) -> u32 {
        self.last_lap_m
    }

    /// The last lap's average heart rate (None without one).
    pub fn last_lap_hr(&self) -> Option<u16> {
        self.last_lap_hr
    }

    /// True when this lap passed `m` metres or `s` seconds of moving time (0: no limit). The lap count starts again
    /// at each lap, manual or auto, so auto laps fall every `m` since the last lap, not at multiples of the total.
    pub fn lap_reached(&self, m: u32, s: u32) -> bool {
        (m > 0 && self.lap_dist_m() >= m) || (s > 0 && self.lap_s() >= s)
    }

    /// This lap's average power, W (zeros counted, as [`Ride::avg_power`]).
    pub fn lap_avg_power(&self) -> Option<u16> {
        self.lap_power.avg()
    }

    /// This lap's normalized power, W (None before 30 s with power).
    pub fn lap_np(&self) -> Option<u16> {
        self.lap_power.np()
    }

    /// Top cadence, rpm (None before one).
    pub fn max_cadence(&self) -> Option<u16> {
        (self.max_cadence > 0).then_some(self.max_cadence)
    }

    /// Highest altitude, metres (None before a reading; from the current altitude source).
    pub fn max_alt_m(&self) -> Option<i32> {
        self.max_alt_mm.map(|a| a.div_euclid(1000))
    }

    /// Altitude lost, metres: the same hysteresis as [`Ride::ascent_m`].
    pub fn descent_m(&self) -> u32 {
        self.descent_mm / 1000
    }

    /// This lap's ascent, metres.
    pub fn lap_ascent_m(&self) -> u32 {
        self.lap_ascent_mm / 1000
    }

    /// This lap's descent, metres.
    pub fn lap_descent_m(&self) -> u32 {
        self.lap_descent_mm / 1000
    }

    /// Distance ridden at a grade of +1 % or more, metres.
    pub fn uphill_m(&self) -> u32 {
        (self.up9 / 9000).min(u64::from(u32::MAX)) as u32
    }

    /// Distance ridden at a grade of -1 % or less, metres.
    pub fn downhill_m(&self) -> u32 {
        (self.down9 / 9000).min(u64::from(u32::MAX)) as u32
    }

    /// This lap's moving time, seconds.
    pub fn lap_s(&self) -> u32 {
        self.timer_s - self.lap_timer_s
    }

    /// This lap's distance, metres.
    pub fn lap_dist_m(&self) -> u32 {
        ((self.dist9 - self.lap_dist9) / 9000).min(u64::from(u32::MAX)) as u32
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Climbing
// ---------------------------------------------------------------------------------------------------------------

/// Seconds between climbing samples.
const CLIMB_STEP_S: u32 = 5;
/// Samples kept: a minute, plus the one it starts from.
const CLIMB_N: usize = 13;
/// Samples back for the grade: 30 s.
const GRADE_BACK: usize = 6;
/// The grade wants this much distance (1/9 mm: 20 m).
const GRADE_MIN_DIST9: u64 = 20 * 9000;

/// Altitude and distance every `CLIMB_STEP_S` moving seconds, the last minute: VAM and grade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Climb {
    alt_mm: [i32; CLIMB_N],
    dist9: [u64; CLIMB_N],
    /// Samples since the altitude was last missing, up to [`CLIMB_N`]; the next slot; seconds to the next sample.
    n: usize,
    i: usize,
    secs: u32,
}

impl Climb {
    const fn new() -> Self {
        Climb { alt_mm: [0; CLIMB_N], dist9: [0; CLIMB_N], n: 0, i: 0, secs: 0 }
    }

    /// A correction of the altitude (not a climb): the samples move with it.
    fn shift(&mut self, d: i32) {
        for a in &mut self.alt_mm {
            *a = a.saturating_add(d);
        }
    }

    /// One moving second: the altitude (None: no fix, which starts again) and the ride's distance.
    fn second(&mut self, alt_mm: Option<i32>, dist9: u64) {
        let Some(alt) = alt_mm else {
            (self.n, self.secs) = (0, 0);
            return;
        };
        if self.secs > 0 {
            self.secs -= 1;
            return;
        }
        self.secs = CLIMB_STEP_S - 1;
        if let (Some(a), Some(d)) = (self.alt_mm.get_mut(self.i), self.dist9.get_mut(self.i)) {
            (*a, *d) = (alt, dist9);
        }
        self.i = (self.i + 1) % CLIMB_N;
        self.n = (self.n + 1).min(CLIMB_N);
    }

    /// The sample `back` samples before the latest, and the latest.
    fn span(&self, back: usize) -> Option<((i32, u64), (i32, u64))> {
        if back >= self.n {
            return None;
        }
        let at = |k: usize| {
            let j = (self.i + 2 * CLIMB_N - 1 - k) % CLIMB_N;
            Some((*self.alt_mm.get(j)?, *self.dist9.get(j)?))
        };
        Some((at(back)?, at(0)?))
    }

    /// Metres climbed per hour over the last minute (a descent is negative).
    ///
    /// ```
    /// use bryton_rustfw::ride::{Input, Ride};
    /// let mut r = Ride::new();
    /// r.start_pause();
    /// for s in 0..61 {
    ///     r.second(&Input { speed_x10: Some(100), alt_mm: Some(s * 250), ..Input::default() });
    /// }
    /// assert_eq!(r.vam(), Some(900), "0.25 m/s: 900 m/h");
    /// ```
    fn vam(&self) -> Option<i32> {
        let ((a0, _), (a1, _)) = self.span(CLIMB_N - 1)?;
        let s = (CLIMB_N as i64 - 1) * i64::from(CLIMB_STEP_S);
        Some((i64::from(a1 - a0) * 3600 / 1000 / s) as i32)
    }

    /// Rise over run over the last 30 s, % x 10 (None under 20 m).
    ///
    /// ```
    /// use bryton_rustfw::ride::{Input, Ride};
    /// let mut r = Ride::new();
    /// r.start_pause();
    /// for s in 0..31 {
    ///     // 5 m/s (18 km/h), climbing 0.4 m/s: 8 %
    ///     r.second(&Input { speed_x10: Some(180), alt_mm: Some(s * 400), ..Input::default() });
    /// }
    /// assert_eq!(r.grade_x10(), Some(80));
    /// ```
    fn grade_x10(&self) -> Option<i32> {
        let ((a0, d0), (a1, d1)) = self.span(GRADE_BACK)?;
        let run = d1.saturating_sub(d0);
        // alt mm x 1000 / (dist9 / 9) mm = per mille: x 9000 / dist9
        (run >= GRADE_MIN_DIST9).then(|| (i64::from(a1 - a0) * 9000 / run as i64) as i32)
    }

    /// The altitude change over the grade's window, mm (None before the window spans 20 m).
    fn rise_mm(&self) -> Option<i32> {
        let ((a0, d0), (a1, d1)) = self.span(GRADE_BACK)?;
        (d1.saturating_sub(d0) >= GRADE_MIN_DIST9).then(|| a1 - a0)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Zones
// ---------------------------------------------------------------------------------------------------------------

/// Zone colours, RGB565: Intervals.icu's zone palette (the owner's choice, 2026-10-26: teal, green, olive, brown,
/// wine, purple, slate, RGB (47, 98, 89), (47, 98, 35), (148, 125, 49), (143, 87, 42), (124, 32, 59), (68, 45, 124),
/// (60, 55, 72)), each channel x 1.5 so the 8 px bars read on the black panel. Heart rate's five zones take the first
/// five, so a colour means the same zone on both. Kept apart from the menu orange (`ui::ACCENT`) and the alarm red
/// (`ui::RED`) (tested).
pub const ZONE_COLORS: [u16; 7] = [0x4c90, 0x4c86, 0xddc9, 0xd408, 0xb98b, 0x6237, 0x5a8d];

/// What kind of ride this is: on the road, off road, or indoors on a trainer (the bike is on it, the GPS has nothing
/// to say). Chosen at Start from BIKE > PROFILE and the trainer heard, or set by hand (app/profile.rs); the indoor one
/// skips the map pages in the round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Outdoors: today's behaviour.
    #[default]
    Road,
    /// On a trainer.
    Indoor,
    /// Off road: sub_sport mountain in the file, continuous GNSS tracking, a time-based auto-pause.
    Mtb,
}

/// A zone shown with hysteresis on the value, not on time: the bar follows the number it sits on at once, but a value
/// on a zone's edge does not flip it every second: the zone shown stays while the value is within `margin` of it
/// (the owner saw time-held bars lag their numbers, 2026-10-26).
///
/// ```
/// use bryton_rustfw::ride::{ZoneHold, power_zone};
/// let (mut z, zone) = (ZoneHold::default(), |w| power_zone(w, 200)); // zone 3 from 152 W, zone 4 from 182 W
/// assert_eq!(z.update(Some(170), 4, zone), 3, "the first zone at once");
/// assert_eq!(z.update(Some(183), 4, zone), 3, "1 W into zone 4: within 4 W of zone 3, kept");
/// assert_eq!(z.update(Some(186), 4, zone), 4, "4 W in: zone 4");
/// assert_eq!(z.update(Some(180), 4, zone), 4, "back 2 W under the edge: kept");
/// assert_eq!(z.update(Some(300), 4, zone), 6, "a jump far away: at once");
/// assert_eq!(z.update(None, 4, zone), 0, "no reading: no zone");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneHold {
    shown: u8,
}

impl ZoneHold {
    /// This second's value `v` (None: no reading), zoned by `zone`; the zone to show, 0 for none.
    pub fn update(&mut self, v: Option<u16>, margin: u16, zone: impl Fn(u16) -> u8) -> u8 {
        self.shown = match v {
            None => 0,
            Some(v) => {
                let near =
                    |s: u8| s != 0 && (zone(v.saturating_sub(margin)) == s || zone(v.saturating_add(margin)) == s);
                if near(self.shown) { self.shown } else { zone(v) }
            }
        };
        self.shown
    }

    /// The zone shown now, 0 for none.
    pub fn shown(&self) -> u8 {
        self.shown
    }
}

/// Altitudes a ride can have, mm (the Dead Sea's shore to above Everest): outside, the receiver's reading is dropped.
pub const ALT_RANGE_MM: core::ops::RangeInclusive<i32> = -500_000..=9_000_000;

/// Max heart rate before it is set (stock's default, `MHR` in its UserProfile.ini: 190).
pub const MAX_HR: u16 = 190;
/// FTP before it is set, W (conventional).
pub const FTP: u16 = 200;

/// Where power zones 2 to 7 start, % of FTP (Coggan's 55, 75, 90, 105, 120, 150, rounded up so a whole-percent
/// reading falls in the zone above the edge). The one copy: zones, gauge and workout files read it.
pub const POWER_ZONE_PCT: [u32; 6] = [56, 76, 91, 106, 121, 151];
/// Where heart-rate zones 2 to 5 start, % of the maximum.
pub const HR_ZONE_PCT: [u32; 4] = [60, 70, 80, 90];

/// Heart-rate zone 1 to 5 of `bpm` for a maximum of `max`: under 60, 70, 80, 90 % of it, then above
/// (conventional percent-of-max zones; stock's own default is seven zones).
///
/// ```
/// use bryton_rustfw::ride::hr_zone;
/// assert_eq!([100, 114, 140, 160, 175].map(|b| hr_zone(b, 190)), [1, 2, 3, 4, 5]);
/// ```
pub fn hr_zone(bpm: u16, max: u16) -> u8 {
    let pct = u32::from(bpm) * 100 / u32::from(max.max(1));
    HR_ZONE_PCT.iter().filter(|&&t| pct >= t).count() as u8 + 1
}

/// Power zone 1 to 7 of `w` for an FTP of `ftp` (Coggan's: 55, 75, 90, 105, 120, 150 % of FTP).
///
/// ```
/// use bryton_rustfw::ride::power_zone;
/// assert_eq!([100, 120, 170, 200, 220, 280, 320].map(|w| power_zone(w, 200)), [1, 2, 3, 4, 5, 6, 7]);
/// ```
pub fn power_zone(w: u16, ftp: u16) -> u8 {
    let pct = u32::from(w) * 100 / u32::from(ftp.max(1));
    POWER_ZONE_PCT.iter().filter(|&&t| pct >= t).count() as u8 + 1
}

/// The colour of a DFA alpha1 reading (x 100): above the aerobic threshold's 0.75 green, between it and 0.5 olive,
/// below brown (Rogers et al.), from the zone palette.
pub fn dfa_color(v: u16) -> u16 {
    ZONE_COLORS[if v >= 75 {
        1
    } else if v >= 50 {
        2
    } else {
        3
    }]
}

/// The colour of heart-rate zone `z` (1 to 5): teal, green, olive, brown, wine, as power's first five.
pub fn hr_color(z: u8) -> u16 {
    ZONE_COLORS[usize::from(z.clamp(1, 5) - 1)]
}

/// The colour of power zone `z` (1 to 7): teal, green, olive, brown, wine, purple, slate (Intervals.icu's).
pub fn power_color(z: u8) -> u16 {
    ZONE_COLORS[usize::from(z.clamp(1, 7) - 1)]
}

/// The colour of the power zone of `w` watts at an FTP of `ftp`.
pub fn power_zone_color(w: u16, ftp: u16) -> u16 {
    power_color(power_zone(w, ftp))
}

/// The colour of the heart-rate zone of `bpm` at a maximum of `max`.
pub fn hr_zone_color(bpm: u16, max: u16) -> u16 {
    hr_color(hr_zone(bpm, max))
}

/// The seconds the ride spent in each heart-rate zone and each power zone: the moving seconds with a reading. The app
/// keeps it (not the recovery record), so a Start or a resume starts it from zero, as the laps do.
///
/// ```
/// use bryton_rustfw::ride::ZoneTime;
/// let mut t = ZoneTime::default();
/// t.second(Some(2), Some(7));
/// t.second(Some(2), None);
/// assert_eq!([t.hr_s(2), t.hr_s(1), t.power_s(7), t.power_s(0)], [Some(2), Some(0), Some(1), None]);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneTime {
    hr: [u32; 5],
    power: [u32; 7],
}

impl ZoneTime {
    /// One moving second in heart-rate zone `hr` (1 to 5) and power zone `power` (1 to 7); None: no reading.
    pub fn second(&mut self, hr: Option<u8>, power: Option<u8>) {
        if let Some(s) = hr.and_then(|z| usize::from(z).checked_sub(1)).and_then(|i| self.hr.get_mut(i)) {
            *s = s.saturating_add(1);
        }
        if let Some(s) = power.and_then(|z| usize::from(z).checked_sub(1)).and_then(|i| self.power.get_mut(i)) {
            *s = s.saturating_add(1);
        }
    }

    /// The seconds in heart-rate zone `z`, None for no such zone.
    pub fn hr_s(&self, z: u8) -> Option<u32> {
        usize::from(z).checked_sub(1).and_then(|i| self.hr.get(i)).copied()
    }

    /// The seconds in power zone `z`, None for no such zone.
    pub fn power_s(&self, z: u8) -> Option<u32> {
        usize::from(z).checked_sub(1).and_then(|i| self.power.get(i)).copied()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// History, for the graph field
// ---------------------------------------------------------------------------------------------------------------

/// Seconds per bar (one pixel column) of the graph.
pub const BAR_S: u32 = 2;
/// Bars kept: more than the widest graph shows (232), so a graph drawn a few bars ago can still be compared.
pub const BARS: usize = 256;
/// The smoothing: each second moves the average 1 / 2^`EMA_SHIFT` of the way to the reading (a time constant of
/// about 7.5 s), so the noise from one second to the next does not make the graph jitter.
const EMA_SHIFT: u32 = 3;

/// What a graph can plot: one series each in [`History`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plot {
    /// Power, W (smoothed).
    Power,
    /// Heart rate, bpm (smoothed).
    Hr,
    /// W' balance, in 10 J (0 when drained or below).
    Wbal,
    /// DFA alpha1 x 100.
    Dfa,
    /// Altitude, m + [`ALT_OFFSET`] (so a reading is never 0).
    Alt,
}

impl Plot {
    /// Every plot, in the order a tap on a graph goes through them.
    pub const ALL: [Plot; 5] = [Plot::Power, Plot::Hr, Plot::Wbal, Plot::Dfa, Plot::Alt];
}

/// Added to altitudes kept by [`History`] (the Dead Sea is at -430 m).
pub const ALT_OFFSET: i32 = 1000;

/// An altitude graph's scale (low, high, m + [`ALT_OFFSET`]) around the altitudes it shows (0: none): in 20 m steps,
/// 60 m at least, so it is not redrawn for every metre.
///
/// ```
/// use bryton_rustfw::ride::{ALT_OFFSET, alt_scale};
/// let o = ALT_OFFSET as u32;
/// assert_eq!(alt_scale([0, o + 105, o + 131].into_iter()), (o + 100, o + 160));
/// assert_eq!(alt_scale([o + 10, o + 250].into_iter()), (o, o + 260));
/// assert_eq!(alt_scale([0].into_iter()), (o, o + 60));
/// ```
pub fn alt_scale(seen: impl Iterator<Item = u32>) -> (u32, u32) {
    let (lo, hi) = seen.filter(|&v| v != 0).fold((u32::MAX, 0), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if hi == 0 {
        return (ALT_OFFSET as u32, ALT_OFFSET as u32 + 60);
    }
    let lo = lo / 20 * 20;
    (lo, (hi.div_ceil(20) * 20).max(lo + 60))
}
/// Metres one column of the altitude along the distance starts with (doubled each time they fill: 4 km, 8 km...).
const DIST_STEP_M: u32 = 8;
/// Columns of the altitude along the distance (at least the card's profile width).
const DIST_COLS: usize = 256;
/// The most columns one second may skip and still be filled with its altitude (a fast descent); more is a glitch.
const DIST_GAP: usize = 4;
/// Plots kept.
pub const PLOTS: usize = Plot::ALL.len();
/// Bars of the whole ride kept: when full, neighbouring bars are merged and each bar covers twice the time.
pub const WHOLE: usize = 512;

/// The series a graph can show, sampled every [`BAR_S`] seconds: the last [`BARS`] bars, and the whole ride at a
/// resolution that halves each time [`WHOLE`] bars are filled (0: no reading in that bar). Power and heart rate are
/// smoothed (an exponential moving average); W' balance, DFA alpha1 and altitude are smooth already and are taken
/// as they are at the end of each bar.
///
/// Stock's graph field draws 216 one-second samples per metric, one pixel column each in its zone's colour, side
/// by side (Ghidra: `Ride_PushHistorySample` 0xd0194b50, `MeterBarChartP` refresh 0xd017574a, Embedded Wizard
/// `ChartsBarChart_Draw` 0xd01c3270 with a bar width of 1). This one smooths them first, and keeps 2 s a column.
pub struct History {
    bars: [[u16; BARS]; PLOTS],
    /// Bars pushed so far.
    n: u32,
    /// The averages of power and heart rate, x16 (0: none yet), and the last value of the others.
    avg: [u32; PLOTS],
    /// A reading came in during this bar's seconds.
    seen: [bool; PLOTS],
    secs: u32,
    whole: [[u16; WHOLE]; PLOTS],
    /// Whole-ride bars filled, bars of [`BAR_S`] each one covers, how many of those are in the one being filled
    /// with their sums and counts, and how many times the whole ride was halved.
    whole_n: u32,
    per: u32,
    acc_n: u32,
    acc: [(u32, u32); PLOTS],
    halved: u32,
    /// The altitude along the distance (the summary's profile): column k is the altitude where the ride was
    /// `k * dist_step` m along; columns filled, and metres a column covers (doubled when full).
    by_dist: [u16; DIST_COLS],
    dist_n: u32,
    dist_step: u32,
}

impl Default for History {
    fn default() -> Self {
        History {
            bars: [[0; BARS]; PLOTS],
            n: 0,
            avg: [0; PLOTS],
            seen: [false; PLOTS],
            secs: 0,
            whole: [[0; WHOLE]; PLOTS],
            whole_n: 0,
            per: 1,
            acc_n: 0,
            acc: [(0, 0); PLOTS],
            halved: 0,
            by_dist: [0; DIST_COLS],
            dist_n: 0,
            dist_step: DIST_STEP_M,
        }
    }
}

impl History {
    /// One second of readings, in [`Plot::ALL`]'s order; true when a bar was pushed.
    ///
    /// ```
    /// use bryton_rustfw::ride::{History, Plot};
    /// let mut h = History::default();
    /// h.second([Some(100), None, None, None, None]);
    /// h.second([Some(300), None, None, None, None]); // one second at 300 W moves the bar an eighth of the way
    /// assert_eq!(h.bar(0, Plot::Power), 125);
    /// ```
    pub fn second(&mut self, v: [Option<u16>; PLOTS]) -> bool {
        for (k, v) in v.into_iter().enumerate() {
            let (Some(v), Some(a)) = (v, self.avg.get_mut(k)) else { continue };
            *a = if k >= 2 {
                u32::from(v) << 4
            } else {
                // the first reading, or the first after a bar without any, starts the average
                let v = u32::from(v) << 4;
                if *a == 0 { v } else { *a - (*a >> EMA_SHIFT) + (v >> EMA_SHIFT) }
            };
            if let Some(s) = self.seen.get_mut(k) {
                *s = true;
            }
        }
        self.secs += 1;
        if self.secs < BAR_S {
            return false;
        }
        let i = self.n as usize % BARS;
        for k in 0..PLOTS {
            if !self.seen[k] {
                self.avg[k] = 0;
            }
            self.seen[k] = false;
            let bar = ((self.avg[k] + 8) >> 4).min(u32::from(u16::MAX)) as u16;
            self.bars[k][i] = bar;
            if bar != 0 {
                self.acc[k] = (self.acc[k].0 + u32::from(bar), self.acc[k].1 + 1);
            }
        }
        self.n = self.n.wrapping_add(1);
        self.secs = 0;
        self.acc_n += 1;
        if self.acc_n >= self.per {
            self.push_whole();
        }
        true
    }

    /// The bar of the whole ride being filled is complete: kept (the whole ride halved first when full).
    fn push_whole(&mut self) {
        if self.whole_n as usize >= WHOLE {
            for series in &mut self.whole {
                for j in 0..WHOLE / 2 {
                    series[j] = mean(series[2 * j], series[2 * j + 1]);
                }
            }
            self.whole_n = (WHOLE / 2) as u32;
            self.per *= 2;
            self.halved += 1;
        }
        let i = self.whole_n as usize;
        for (series, acc) in self.whole.iter_mut().zip(&mut self.acc) {
            if let Some(b) = series.get_mut(i) {
                *b = if acc.1 == 0 { 0 } else { (acc.0 / acc.1) as u16 };
            }
            *acc = (0, 0);
        }
        self.whole_n += 1;
        self.acc_n = 0;
    }

    /// The altitude (m + [`ALT_OFFSET`]) at `dist_m` into the ride, once a second. Columns the ride skipped (a fast
    /// second is longer than a column) take the same altitude; standing still rewrites the same column.
    pub fn dist_second(&mut self, dist_m: u32, alt: Option<u16>) {
        let Some(alt) = alt else { return };
        while (dist_m / self.dist_step) as usize >= DIST_COLS {
            for j in 0..DIST_COLS / 2 {
                self.by_dist[j] = mean(self.by_dist[2 * j], self.by_dist[2 * j + 1]);
            }
            self.by_dist[DIST_COLS / 2..].fill(0);
            self.dist_n = self.dist_n.div_ceil(2);
            self.dist_step *= 2;
        }
        let i = (dist_m / self.dist_step) as usize;
        // only a small gap is bridged: nothing filled yet (the first reading, or after a resume emptied the history)
        // or a jump (a GPS glitch) starts at `i`, the columns before stay empty (0)
        let gap = i.saturating_sub(self.dist_n as usize);
        let from = if self.dist_n == 0 || gap > DIST_GAP { i } else { (self.dist_n as usize).min(i) };
        if let Some(cols) = self.by_dist.get_mut(from..=i) {
            cols.fill(alt);
        }
        self.dist_n = self.dist_n.max(i as u32 + 1);
    }

    /// Columns of the altitude along the distance filled so far.
    pub fn dist_len(&self) -> u32 {
        self.dist_n
    }

    /// Column `k` of the altitude along the distance (0: none).
    pub fn dist_bar(&self, k: u32) -> u16 {
        self.by_dist.get(k as usize).copied().unwrap_or(0)
    }

    /// Bars pushed so far.
    pub fn len(&self) -> u32 {
        self.n
    }

    /// True before the first bar.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Bar `k` (counted from the first one ever) of `plot`; 0 if it is not kept (or never was).
    pub fn bar(&self, k: u32, plot: Plot) -> u16 {
        if k >= self.n || self.n - k > BARS as u32 {
            return 0;
        }
        self.bars[plot as usize][k as usize % BARS]
    }

    /// The whole ride: bars kept, how many times it was halved (each time, every bar changes), and bar `k`.
    pub fn whole_len(&self) -> u32 {
        self.whole_n
    }

    /// See [`History::whole_len`].
    pub fn whole_halved(&self) -> u32 {
        self.halved
    }

    /// See [`History::whole_len`]: seconds one whole-ride bar covers.
    #[cfg(test)]
    pub fn whole_bar_s(&self) -> u32 {
        self.per * BAR_S
    }

    /// See [`History::whole_len`].
    pub fn whole_bar(&self, k: u32, plot: Plot) -> u16 {
        self.whole[plot as usize].get(k as usize).copied().unwrap_or(0)
    }
}

/// The mean of two bars, a missing one (0) left out.
fn mean(a: u16, b: u16) -> u16 {
    match (a, b) {
        (0, x) | (x, 0) => x,
        _ => ((u32::from(a) + u32::from(b)) / 2) as u16,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn balance_average_ignores_samples_without_one() {
        let mut r = Ride::new();
        r.start_pause(); // Start
        for b in [Some(60), None, Some(40), Some(101)] {
            r.second(&Input { balance: b, ..at(250) });
        }
        assert_eq!(r.balance_right(), Some(50), "60 and 40 %; no reading and 101 % (not a share) are left out");
    }

    fn at(speed: u16) -> Input {
        Input {
            speed_x10: Some(speed),
            hr: Some(140),
            cadence: Some(90),
            power: Some(200),
            balance: None,
            alt_mm: None,
            alt_ref: 0,
            alt_corr_mm: 0,
            profile: Profile::Road,
            pause_kmh: 0,
        }
    }

    /// A change of altitude source is not a climb, and the barometer's hysteresis is finer than the receiver's.
    #[test]
    fn an_altitude_reference_change_and_the_barometers_hysteresis() {
        let mut r = Ride::new();
        r.start_pause();
        let alt = |mm, alt_ref| Input { alt_mm: Some(mm), alt_ref, ..at(200) };
        for _ in 0..10 {
            r.second(&alt(100_000, 0));
        }
        for _ in 0..10 {
            r.second(&alt(140_000, 1)); // the barometer, on another level: 40 m that nobody climbed
        }
        assert_eq!(r.ascent_m(), 0);
        for d in [0, 1000, 0, 1000, 0] {
            r.second(&alt(140_000 + d, 1)); // a metre of wiggle: nothing
        }
        assert_eq!(r.ascent_m(), 0);
        for s in 0..10 {
            r.second(&alt(140_000 + s * 500, 1)); // 4.5 m climbed: 3 m counted past 3 m, 1.5 m waiting
        }
        assert_eq!(r.ascent_m(), 3);
        let mut g = Ride::new();
        g.start_pause();
        for s in 0..10 {
            g.second(&alt(100_000 + s * 500, 0)); // the same climb on the receiver: under its 5 m
        }
        assert_eq!(g.ascent_m(), 0);
    }

    /// A correction let go a second at a time (altitude.rs `corr`) is no climb, running or paused, and no VAM.
    #[test]
    fn a_correction_is_not_a_climb() {
        let mut r = Ride::new();
        r.start_pause();
        let alt = |mm, corr| Input { alt_mm: Some(mm), alt_ref: 1, alt_corr_mm: corr, ..at(200) };
        r.second(&alt(100_000, 0));
        for s in 1..=60 {
            r.second(&alt(100_000 + s * 500, 500)); // 30 m up, all of it the correction
        }
        assert_eq!((r.ascent_m(), r.vam()), (0, Some(0)));
        r.start_pause(); // paused: the correction goes on
        for s in 61..=80 {
            r.second(&alt(100_000 + s * 500, 500));
        }
        r.start_pause();
        for s in 0..=8 {
            r.second(&alt(140_000 + s * 500, 0)); // then 4 m climbed: counted
        }
        assert_eq!(r.ascent_m(), 3);
    }

    /// Indoors the ride pauses itself on no power and no cadence for 3 s, whatever the speed says, and goes on at the
    /// first pedal stroke; on the road it is the speed.
    #[test]
    fn indoor_auto_pause_follows_power_and_cadence() {
        let indoor =
            |speed, power, cadence| Input { speed_x10: Some(speed), power, cadence, profile: Profile::Indoor, ..at(0) };
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..10 {
            r.second(&indoor(0, Some(180), Some(85))); // no speed, but pedalling: the timer runs
        }
        assert_eq!(r.timer_s(), 10);
        for _ in 0..2 {
            r.second(&indoor(300, Some(0), Some(0))); // the flywheel still turns
        }
        assert!(r.moving(), "two seconds: not yet");
        r.second(&indoor(300, Some(0), Some(0)));
        assert!(!r.moving(), "the third: paused, speed or not");
        let t = r.timer_s();
        for _ in 0..5 {
            r.second(&indoor(300, Some(0), Some(0)));
        }
        assert_eq!(r.timer_s(), t, "the timer waits");
        r.second(&indoor(0, Some(0), Some(60))); // one pedal stroke of cadence: back
        assert!(r.moving());
        // no power or cadence reported at all keeps it running
        for _ in 0..10 {
            r.second(&indoor(0, None, None));
        }
        assert!(r.moving());
        // on the road the speed decides, as before
        let mut road = Ride::new();
        road.start_pause();
        for _ in 0..5 {
            road.second(&Input { power: Some(0), cadence: Some(0), ..at(0) });
        }
        assert!(!road.moving(), "road: no speed");
    }

    /// MTB pauses after 5 s in a row under 2 km/h, never on a slow climb, and goes on at the first second above.
    #[test]
    fn mtb_auto_pause_waits_five_seconds() {
        let mtb = |speed| Input { profile: Profile::Mtb, ..at(speed) };
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..600 {
            r.second(&mtb(30)); // 3 km/h up a wall for 10 minutes
        }
        assert!(r.moving());
        assert_eq!(r.timer_s(), 600, "a 3 km/h climb never pauses");
        for _ in 0..4 {
            r.second(&mtb(0));
        }
        assert!(r.moving(), "4 s stopped: not paused");
        assert_eq!(r.timer_s(), 604);
        r.second(&mtb(19));
        assert!(!r.moving(), "the fifth second under 2 km/h: paused");
        r.second(&mtb(0));
        assert_eq!(r.timer_s(), 604, "the timer waits");
        r.second(&mtb(21));
        assert!(r.moving(), "above 2 km/h: back at once");
        assert_eq!(r.timer_s(), 605);
        for _ in 0..4 {
            r.second(&mtb(0));
        }
        r.second(&mtb(25));
        for _ in 0..4 {
            r.second(&mtb(0));
        }
        assert!(r.moving(), "the count starts again after a second above");
        let mut road = Ride::new();
        road.start_pause();
        road.second(&at(0));
        assert!(!road.moving(), "road: at once, as before");
    }

    /// AUTO PAUSE: a chosen speed replaces the profile's 2 km/h, OFF never pauses (indoors too), 0 keeps the rule.
    #[test]
    fn auto_pause_threshold() {
        let moving = |profile, pause_kmh, speed| {
            let mut r = Ride::new();
            r.start_pause();
            for _ in 0..10 {
                r.second(&Input { profile, pause_kmh, ..at(speed) });
            }
            r.moving()
        };
        assert!(moving(Profile::Road, 0, 25) && !moving(Profile::Road, 0, 15), "the rule: under 2 km/h");
        assert!(!moving(Profile::Road, 5, 45) && moving(Profile::Road, 5, 55), "5 km/h");
        assert!(!moving(Profile::Road, 8, 79) && moving(Profile::Road, 8, 80), "8 km/h");
        assert!(moving(Profile::Road, 255, 0), "OFF: never");
        assert!(moving(Profile::Mtb, 255, 0), "OFF on an MTB");
        assert!(!moving(Profile::Mtb, 3, 29), "3 km/h on an MTB, after its five seconds");
        let still = Input { profile: Profile::Indoor, power: Some(0), cadence: Some(0), ..at(0) };
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..10 {
            r.second(&Input { pause_kmh: 255, ..still });
        }
        assert!(r.moving(), "OFF: a trainer ride stopped is not paused");
    }

    #[test]
    fn start_pause_lap_end() {
        let mut r = Ride::new();
        assert_eq!(r.second(&at(288)), None, "idle: nothing counts");
        assert_eq!(r.lap(), None);
        assert_eq!(r.start_pause(), Event::Start);
        for _ in 0..100 {
            assert_eq!(r.second(&at(288)), Some(Event::Second));
        }
        assert_eq!((r.timer_s(), r.dist_m()), (100, 800), "28.8 km/h is 8 m/s");
        assert_eq!((r.avg_speed_x10(), r.max_speed_x10()), (Some(288), 288));
        assert_eq!(r.lap(), Some(Event::Lap));
        for _ in 0..10 {
            r.second(&at(0));
        }
        assert!(!r.moving() && r.timer_s() == 100, "auto-paused");
        for _ in 0..50 {
            r.second(&at(360));
        }
        assert_eq!((r.lap_s(), r.lap_dist_m()), (50, 500));
        assert_eq!(r.avg_speed_x10(), Some(312), "1300 m in 150 s");
        assert_eq!(r.end(), None, "running: a long Start does not end it");
        assert_eq!(r.start_pause(), Event::Pause);
        r.second(&at(300));
        assert_eq!(r.timer_s(), 150, "paused");
        assert_eq!(r.start_pause(), Event::Resume);
        assert_eq!(r.start_pause(), Event::Pause);
        assert_eq!(r.end(), Some(Event::End));
        assert_eq!((r.state(), r.dist_m()), (State::Idle, 1300), "the totals stay");
        assert_eq!((r.avg_hr(), r.avg_power(), r.avg_cadence()), (Some(140), Some(200), Some(90)));
        r.start_pause();
        assert_eq!(r.dist_m(), 0, "a new ride starts from zero");
    }

    #[test]
    fn ascent_with_hysteresis() {
        let mut r = Ride::new();
        r.start_pause();
        // noise of +-2 m around 100 m, then a climb to 150 m, a descent and a climb of 20 m
        let alts = [100, 102, 98, 101, 99, 110, 130, 150, 148, 151, 120, 125, 140];
        for a in alts {
            r.second(&Input { alt_mm: Some(a * 1000), ..at(200) });
        }
        assert_eq!(r.ascent_m(), 52 + 20, "98 to 150, then 120 to 140");
        // a receiver's garbage in between (overflowed the differences before, a panic in debug): no ascent, no panic
        for a in [i32::MAX, i32::MIN, 140_000, i32::MAX, 141_000] {
            r.second(&Input { alt_mm: Some(a), ..at(200) });
        }
        assert_eq!(r.ascent_m(), 52 + 20, "the garbage dropped");
        assert!(r.vam().is_none_or(|v| v.abs() < 10_000) && r.grade_x10().is_none_or(|g| g.abs() < 1000));
    }

    /// Descent counts the falls of the hysteresis from the high point, as ascent counts the rises from the low one.
    #[test]
    fn descent_with_hysteresis() {
        let mut r = Ride::new();
        r.start_pause();
        for a in [100, 102, 98, 101, 99, 110, 130, 150, 148, 151, 120, 125, 140] {
            r.second(&Input { alt_mm: Some(a * 1000), ..at(200) });
        }
        assert_eq!(r.descent_m(), 31, "151 to 120; the noise under 5 m is nothing");
        assert_eq!(r.ascent_m(), 72, "as ascent_with_hysteresis: the fall is not a rise");
        for a in [i32::MAX, i32::MIN, 100_000, i32::MAX, 0] {
            r.second(&Input { alt_mm: Some(a), ..at(200) });
        }
        assert_eq!(r.descent_m(), 31 + 40 + 100, "140 to 100, 100 to 0; the garbage is dropped, no panic");
    }

    /// The lap's own fields: its average and NP, its ascent and descent, its time kept as the last lap when it ends,
    /// and the count; a new lap starts from zero. Max cadence and max altitude are the ride's.
    #[test]
    fn lap_fields_and_the_last_lap() {
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..30 {
            r.second(&Input { alt_mm: Some(100_000), cadence: Some(80), power: Some(100), ..at(200) });
        }
        assert_eq!((r.laps(), r.last_lap_s(), r.last_lap_avg_power()), (0, None, None));
        r.lap();
        assert_eq!((r.laps(), r.last_lap_s(), r.last_lap_avg_power()), (1, Some(30), Some(100)));
        assert_eq!((r.lap_avg_power(), r.lap_np(), r.lap_ascent_m()), (None, None, 0));
        // 60 s climbing 0.5 m a second at 300 W and 110 rpm
        let (a0, d0) = (r.ascent_mm, r.descent_mm);
        for i in 0..60 {
            r.second(&Input { alt_mm: Some(100_000 + i * 500), cadence: Some(110), power: Some(300), ..at(200) });
        }
        assert_eq!((r.lap_avg_power(), r.lap_np()), (Some(300), Some(300)));
        assert_eq!(r.lap_ascent_mm, r.ascent_mm - a0, "this lap's ascent is the ride's since the lap");
        assert_eq!((r.descent_mm, r.lap_descent_m()), (d0, 0));
        assert_eq!((r.max_cadence(), r.max_alt_m()), (Some(110), Some(129)));
        r.lap();
        assert_eq!((r.laps(), r.last_lap_s(), r.last_lap_avg_power()), (2, Some(60), Some(300)));
        assert_eq!((r.lap_avg_power(), r.lap_ascent_m(), r.lap_descent_m()), (None, 0, 0));
    }

    /// Auto laps: a distance lap at each step since the last lap (a manual lap at 7 km makes the next at 12 km), a
    /// time lap on moving time only (a pause does not count), no limit (0) never.
    #[test]
    fn lap_reached_counts_from_the_last_lap_and_in_moving_time() {
        let mut r = Ride::new();
        r.start_pause();
        let mut laps = std::vec::Vec::new();
        // 36 km/h: 10 m a second
        for s in 1..=1700 {
            r.second(&at(360));
            if s == 700 {
                assert!(r.lap().is_some(), "a manual lap at 7 km");
            }
            if r.lap_reached(5000, 0) {
                r.lap();
                laps.push(r.dist_m() / 1000);
            }
        }
        assert_eq!(laps, [5, 12, 17], "auto at 5, manual at 7, then 5 km after it (12), not at 10");
        let mut r = Ride::new();
        r.start_pause();
        let mut at_km = std::vec::Vec::new();
        for _ in 0..1600 {
            r.second(&at(360));
            if r.lap_reached(5000, 0) {
                r.lap();
                at_km.push(r.dist_m() / 1000);
            }
        }
        assert_eq!(at_km, [5, 10, 15]);
        assert!(!r.lap_reached(0, 0), "OFF does nothing");
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..590 {
            r.second(&at(360));
        }
        assert!(!r.lap_reached(0, 600));
        r.start_pause(); // paused: the clock stops
        for _ in 0..100 {
            r.second(&at(360));
        }
        r.start_pause();
        assert!(!r.lap_reached(0, 600), "paused time does not count");
        for _ in 0..10 {
            r.second(&at(360));
        }
        assert!(r.lap_reached(0, 600));
    }

    /// Uphill and downhill: the distance at a grade of +1 % or more, and at -1 % or less; flat riding is neither.
    #[test]
    fn uphill_and_downhill_distance() {
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..40 {
            r.second(&Input { alt_mm: Some(100_000), ..at(200) });
        }
        assert_eq!((r.uphill_m(), r.downhill_m()), (0, 0), "flat");
        // 0.5 m a second over 5.6 m a second: about 9 %
        for i in 0..60 {
            r.second(&Input { alt_mm: Some(100_000 + i * 500), ..at(200) });
        }
        assert!(r.uphill_m() > 150 && r.downhill_m() == 0, "up {} down {}", r.uphill_m(), r.downhill_m());
        // then the same down: the grade of the window turns once it is all descent
        let up = r.uphill_m();
        for i in 0..60 {
            r.second(&Input { alt_mm: Some(129_500 - i * 500), ..at(200) });
        }
        assert!(r.downhill_m() > 150, "down {}", r.downhill_m());
        assert!(r.uphill_m() >= up && r.uphill_m() < up + 100, "the 30 s window lags about 15 s");
    }

    /// A resumed ride's trip time is never less than its timer, and the flat ground with GNSS noise of 4 m (under the
    /// 5 m hysteresis) is neither uphill nor downhill.
    #[test]
    fn resumed_trip_time_and_flat_noise() {
        let mut r = Ride::new();
        r.start_pause();
        for _ in 0..50 {
            r.second(&Input { alt_mm: Some(100_000), ..at(200) });
        }
        let back = Ride::resumed(&r.to_words());
        assert!(
            back.elapsed_s() >= back.timer_s() && back.elapsed_s() == 50,
            "trip {} timer {}",
            back.elapsed_s(),
            back.timer_s()
        );
        // +-4 m every second, alternating: the 30 s window starts and ends on the same phase, so no rise
        let mut r = Ride::new();
        r.start_pause();
        for i in 0..240 {
            let alt = if i % 2 == 0 { 104_000 } else { 96_000 };
            r.second(&Input { alt_mm: Some(alt), ..at(200) });
        }
        assert_eq!((r.uphill_m(), r.downhill_m()), (0, 0), "noise under the hysteresis");
    }

    #[test]
    fn history_bars() {
        let mut h = History::default();
        let bars = |h: &History, plot| (0..h.len()).map(|k| h.bar(k, plot)).collect::<std::vec::Vec<_>>();
        let hp = |hr: Option<u16>, p: Option<u16>| [p, hr, None, None, None];
        // power steady at 200 W but for one second at 400 W; heart rate only from the third second
        for s in 0..12 {
            h.second(hp((s >= 2).then_some(150), Some(if s == 5 { 400 } else { 200 })));
        }
        assert_eq!(bars(&h, Plot::Power), [200, 200, 225, 219, 215, 211], "the spike smoothed, then fading");
        assert_eq!(bars(&h, Plot::Hr), [0, 150, 150, 150, 150, 150], "no reading, then from the first one");
        assert_eq!(h.bar(6, Plot::Hr), 0, "not yet");
        // a ramp: the bars follow it a few seconds late, without a step
        for s in 0..20 {
            h.second(hp(None, Some(200 + 10 * s)));
        }
        let ramp = &bars(&h, Plot::Power)[6..];
        assert!(ramp.windows(2).all(|w| w[1] > w[0] && w[1] - w[0] <= 20), "{ramp:?}");
        assert_eq!(ramp[9], 327, "about 6 s (63 W) behind the ramp, at 390 W");
        assert_eq!(h.bar(6, Plot::Hr), 0, "a bar without readings");
        for p in [None, None, None, Some(100)] {
            h.second(hp(None, p));
        }
        assert_eq!(
            (h.bar(16, Plot::Power), h.bar(17, Plot::Power)),
            (0, 100),
            "after a gap, from the reading, not the old average"
        );
        for _ in 0..BAR_S * BARS as u32 {
            h.second(hp(Some(1), None));
        }
        assert_eq!(h.bar(1, Plot::Hr), 0, "dropped");
    }

    /// The whole ride: one bar per fine bar until full, then halved (neighbours merged, a missing one left out).
    #[test]
    fn whole_ride_halves() {
        let mut h = History::default();
        for s in 0..(WHOLE as u32 + 2) * BAR_S {
            let alt = (ALT_OFFSET as u32 + s / BAR_S) as u16; // one metre a bar
            h.second([None, None, None, None, Some(alt)]);
        }
        assert_eq!((h.whole_len(), h.whole_halved(), h.whole_bar_s()), (WHOLE as u32 / 2 + 1, 1, 2 * BAR_S));
        let alt = |k| i32::from(h.whole_bar(k, Plot::Alt)) - ALT_OFFSET;
        assert_eq!((alt(0), alt(1), alt(255)), (0, 2, 510), "pairs merged (0, 1 -> 0; 2, 3 -> 2)");
        assert_eq!(alt(256), 512, "then a bar per two");
        assert_eq!(mean(0, 7), 7);
    }

    /// The altitude follows the distance: a long stop adds no columns, a halving keeps the shape.
    #[test]
    fn altitude_by_distance() {
        let mut h = History::default();
        let o = ALT_OFFSET as u16;
        for m in 0..80 {
            h.dist_second(m * 5, Some(o + m as u16)); // climbs 1 m a second, 5 m a second
        }
        for _ in 0..3000 {
            h.dist_second(395, Some(o + 79)); // a long stop
        }
        assert_eq!(h.dist_len(), 395 / DIST_STEP_M + 1, "the stop adds no column");
        assert!(h.dist_bar(0) <= o + 1 && h.dist_bar(1) <= o + 3, "the climb is at the start");
        assert!(h.dist_bar(h.dist_len() - 2) > o + 70);
        for m in 80..2000 {
            h.dist_second(m * 5, Some(o + 79));
        }
        assert!(h.dist_step > DIST_STEP_M, "halved");
        assert!(h.dist_bar(0) < o + 13 && h.dist_bar(h.dist_len() - 1) == o + 79);
    }

    /// Altitude first read far into the ride (or after a resume emptied the history) leaves the columns before it
    /// empty; a one-second jump of the distance (a GPS glitch) is not filled with a flat run.
    #[test]
    fn altitude_by_distance_gaps() {
        let o = ALT_OFFSET as u16;
        let mut h = History::default();
        h.dist_second(800, Some(o + 5)); // column 100
        assert!(h.dist_bar(99) == 0 && h.dist_bar(100) == o + 5, "no fake flat line before the first reading");
        h.dist_second(808, Some(o + 6));
        h.dist_second(1800, Some(o + 9)); // a glitch: 124 columns in a second
        assert!(h.dist_bar(102) == 0 && h.dist_bar(225) == o + 9, "the jump leaves a gap");
        h.dist_second(1832, Some(o + 9)); // 4 columns: a fast descent, bridged
        assert!(h.dist_bar(226) == o + 9 && h.dist_bar(228) == o + 9);
    }
}
