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
//! (recorder.rs). [`Ride::to_words`] goes into its recovery record, and [`Ride::resumed`] brings a ride back after
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
    /// Altitude, mm: the barometer's (calibrated) when it reads, else a good fix's.
    pub alt_mm: Option<i32>,
    /// Which altitude that is: 0 the receiver's, else the barometer on one calibration (`altitude.rs`). A change
    /// forgets the low point and the climb window (the two are not on the same level).
    pub alt_ref: u8,
    /// An indoor ride ([`Profile::Indoor`]): auto-paused on no power and no cadence, not on speed (a trainer's
    /// flywheel speed lags, and some trainers report a speed at 0 rpm: inferred).
    pub indoor: bool,
}

/// Words of [`Ride::to_words`].
pub const RIDE_WORDS: usize = 16;

/// Under this speed (km/h x 10) the ride is auto-paused: the timer stops, nothing is added.
pub const AUTO_PAUSE_X10: u16 = 20;

/// An indoor ride is auto-paused after this many seconds with no power and no cadence.
pub const INDOOR_PAUSE_S: u8 = 3;

/// Altitude gain is counted once it exceeds the last low point by this much (GPS altitude is noisy).
pub const ASCENT_HYSTERESIS_MM: i32 = 5000;

/// The same for the barometer: its noise after smoothing is a few decimetres (wind, speed changes at the case's
/// vent, inferred; the chip's own is 5 cm), so 2 m keeps small climbs and counts no noise.
pub const BARO_HYSTERESIS_MM: i32 = 2000;

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
    /// Seconds in a row with no power and no cadence (indoor auto-pause).
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
            (self.lap_timer_s, self.lap_dist9) = (self.timer_s, self.dist9);
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
        if self.state != State::Running {
            return None;
        }
        if i.indoor {
            // pedalling is power or cadence; neither reported at all (no sensor) keeps the timer running
            let still = i.power.is_none_or(|p| p == 0) && i.cadence.is_none_or(|c| c == 0);
            let heard = i.power.is_some() || i.cadence.is_some();
            self.still_s = if still && heard { self.still_s.saturating_add(1) } else { 0 };
            self.moving = self.still_s < INDOOR_PAUSE_S;
        } else {
            self.still_s = 0;
            // ponytail: no speed at all (no sensor, no fix) keeps the timer running; a trainer with power only works
            self.moving = i.speed_x10.is_none_or(|v| v >= AUTO_PAUSE_X10);
        }
        if self.moving {
            self.timer_s = self.timer_s.saturating_add(1);
            let v = i.speed_x10.unwrap_or(0);
            self.dist9 = self.dist9.saturating_add(u64::from(v) * 250);
            self.max_speed = self.max_speed.max(v);
            self.hr.add(i.hr);
            self.max_hr = self.max_hr.max(i.hr.unwrap_or(0));
            // an altitude no road has (a receiver's garbage) is no altitude: every difference below stays in range
            let alt_mm = i.alt_mm.filter(|a| ALT_RANGE_MM.contains(a));
            if i.alt_ref != self.alt_ref {
                (self.alt_ref, self.alt_low) = (i.alt_ref, None);
                self.climb.second(None, self.dist9); // starts again
            }
            let hysteresis = if self.alt_ref == 0 { ASCENT_HYSTERESIS_MM } else { BARO_HYSTERESIS_MM };
            self.climb.second(alt_mm, self.dist9);
            self.power.add(i.power); // zeros count: coasting is part of the average power
            self.cadence.add(i.cadence.filter(|&c| c > 0)); // zeros do not: coasting is not a cadence
            if let Some(alt) = alt_mm {
                let low = self.alt_low.get_or_insert(alt);
                if alt < *low {
                    *low = alt;
                } else if alt - *low >= hysteresis {
                    self.ascent_mm = self.ascent_mm.saturating_add((alt - *low) as u32);
                    *low = alt;
                }
            }
        }
        Some(Event::Second)
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
}

// ---------------------------------------------------------------------------------------------------------------
// Zones
// ---------------------------------------------------------------------------------------------------------------

/// Zone colours, RGB565: stock's palette for the first three (seven ABGR words at 0xd0086a8c..0xd0086aa4 of the R039
/// image, read by its zone colour lookup at 0xd01530bc: grey #b4b4b4, blue #506eff, green #7dff1e), a lemon yellow
/// where stock's #ffd000 is next to the orange, then magenta, violet and cyan where stock has orange, red and purple: orange is the menu selection and the info
/// banners (`ui::ACCENT`) and red the alarms (`ui::RED`), and a zone must never look like either (UI review 2, P3:
/// heart rate zone 4 was exactly the banner orange). Tested: no zone colour is near either.
pub const ZONE_COLORS: [u16; 7] = [0xb5b6, 0x537f, 0x7fe3, 0xffe0, 0xf81f, 0x901f, 0x07ff];

/// What kind of ride this is: on the road, or indoors on a trainer (the bike is on it, the GPS has nothing to say).
/// Chosen at Start from the trainer heard, or set by hand (app/profile.rs); the indoor one skips the map pages in
/// the round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Outdoors: today's behaviour.
    #[default]
    Road,
    /// On a trainer.
    Indoor,
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

/// Heart-rate zone 1 to 5 of `bpm` for a maximum of `max`: under 60, 70, 80, 90 % of it, then above
/// (conventional percent-of-max zones; stock's own default is seven zones).
///
/// ```
/// use bryton_rustfw::ride::hr_zone;
/// assert_eq!([100, 114, 140, 160, 175].map(|b| hr_zone(b, 190)), [1, 2, 3, 4, 5]);
/// ```
pub fn hr_zone(bpm: u16, max: u16) -> u8 {
    let pct = u32::from(bpm) * 100 / u32::from(max.max(1));
    [60, 70, 80, 90].iter().filter(|&&t| pct >= t).count() as u8 + 1
}

/// Power zone 1 to 7 of `w` for an FTP of `ftp` (Coggan's: 55, 75, 90, 105, 120, 150 % of FTP).
///
/// ```
/// use bryton_rustfw::ride::power_zone;
/// assert_eq!([100, 120, 170, 200, 220, 280, 320].map(|w| power_zone(w, 200)), [1, 2, 3, 4, 5, 6, 7]);
/// ```
pub fn power_zone(w: u16, ftp: u16) -> u8 {
    let pct = u32::from(w) * 100 / u32::from(ftp.max(1));
    [56, 76, 91, 106, 121, 151].iter().filter(|&&t| pct >= t).count() as u8 + 1
}

/// The colour of a DFA alpha1 reading (x 100): above the aerobic threshold's 0.75 green, between it and 0.5 yellow,
/// below magenta (Rogers et al.); the zone palette, as the owner's rule keeps orange and red for the banner and alarms.
pub fn dfa_color(v: u16) -> u16 {
    ZONE_COLORS[if v >= 75 {
        2
    } else if v >= 50 {
        3
    } else {
        4
    }]
}

/// The colour of heart-rate zone `z` (1 to 5): grey, blue, green, magenta, violet (the palette's lemon yellow and
/// cyan are the power zones').
pub fn hr_color(z: u8) -> u16 {
    ZONE_COLORS[[0, 1, 2, 4, 5][usize::from(z.clamp(1, 5) - 1)]]
}

/// The colour of power zone `z` (1 to 7): grey, blue, green, lemon yellow, magenta, violet, cyan.
pub fn power_color(z: u8) -> u16 {
    ZONE_COLORS[usize::from(z.clamp(1, 7) - 1)]
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

    fn at(speed: u16) -> Input {
        Input {
            speed_x10: Some(speed),
            hr: Some(140),
            cadence: Some(90),
            power: Some(200),
            alt_mm: None,
            alt_ref: 0,
            indoor: false,
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
            r.second(&alt(140_000 + s * 500, 1)); // 4.5 m climbed: counted past 2 m
        }
        assert_eq!(r.ascent_m(), 4);
        let mut g = Ride::new();
        g.start_pause();
        for s in 0..10 {
            g.second(&alt(100_000 + s * 500, 0)); // the same climb on the receiver: under its 5 m
        }
        assert_eq!(g.ascent_m(), 0);
    }

    /// Indoors the ride pauses itself on no power and no cadence for 3 s, whatever the speed says, and goes on at the
    /// first pedal stroke; on the road it is the speed.
    #[test]
    fn indoor_auto_pause_follows_power_and_cadence() {
        let indoor = |speed, power, cadence| Input { speed_x10: Some(speed), power, cadence, indoor: true, ..at(0) };
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
