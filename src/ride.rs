//! The ride: Start / pause / laps / end, auto-pause, the totals the ride fields show, heart-rate and power zones,
//! and the history behind the graph field. Pure logic, fed once a second by the app.
//!
//! **Recorder hook.** Nothing here is stored: every change of state and every second of ride comes out as an
//! [`Event`], and the app hands each one to one place (`App::recorder` in lib.rs), where the FIT writer (fit.rs)
//! plugs in once the eMMC has a driver.

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
    /// Altitude of a good fix, mm.
    pub alt_mm: Option<i32>,
}

/// Under this speed (km/h x 10) the ride is auto-paused: the timer stops, nothing is added.
pub const AUTO_PAUSE_X10: u16 = 20;

/// Altitude gain is counted once it exceeds the last low point by this much (GPS altitude is noisy).
pub const ASCENT_HYSTERESIS_MM: i32 = 5000;

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
    timer_s: u32,
    dist9: u64,
    max_speed: u16,
    hr: Avg,
    power: Avg,
    cadence: Avg,
    ascent_mm: u32,
    alt_low: Option<i32>,
    lap_timer_s: u32,
    lap_dist9: u64,
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
            timer_s: 0,
            dist9: 0,
            max_speed: 0,
            hr: Avg { sum: 0, n: 0 },
            power: Avg { sum: 0, n: 0 },
            cadence: Avg { sum: 0, n: 0 },
            ascent_mm: 0,
            alt_low: None,
            lap_timer_s: 0,
            lap_dist9: 0,
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
        // ponytail: no speed at all (no sensor, no fix) keeps the timer running; a trainer with power only works
        self.moving = i.speed_x10.is_none_or(|v| v >= AUTO_PAUSE_X10);
        if self.moving {
            self.timer_s = self.timer_s.saturating_add(1);
            let v = i.speed_x10.unwrap_or(0);
            self.dist9 = self.dist9.saturating_add(u64::from(v) * 250);
            self.max_speed = self.max_speed.max(v);
            self.hr.add(i.hr);
            self.power.add(i.power); // zeros count: coasting is part of the average power
            self.cadence.add(i.cadence.filter(|&c| c > 0)); // zeros do not: coasting is not a cadence
            if let Some(alt) = i.alt_mm {
                let low = self.alt_low.get_or_insert(alt);
                if alt < *low {
                    *low = alt;
                } else if alt - *low >= ASCENT_HYSTERESIS_MM {
                    self.ascent_mm = self.ascent_mm.saturating_add((alt - *low) as u32);
                    *low = alt;
                }
            }
        }
        Some(Event::Second)
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
// Zones
// ---------------------------------------------------------------------------------------------------------------

/// Zone colours, RGB565, from stock's zone palette (seven ABGR words at 0xd0086a8c..0xd0086aa4 of the R039
/// image, read by its zone colour lookup at 0xd01530bc): grey #b4b4b4, blue #506eff, green #7dff1e, yellow
/// #ffd000, orange #ffa600, red #ff302a, purple #9a1590.
pub const ZONE_COLORS: [u16; 7] = [0xb5b6, 0x537f, 0x7fe3, 0xfe80, 0xfd20, 0xf985, 0x98b2];

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

/// The colour of heart-rate zone `z` (1 to 5): grey, blue, green, orange, red.
pub fn hr_color(z: u8) -> u16 {
    ZONE_COLORS[[0, 1, 2, 4, 5][usize::from(z.clamp(1, 5) - 1)]]
}

/// The colour of power zone `z` (1 to 7): grey, blue, green, yellow, orange, red, purple.
pub fn power_color(z: u8) -> u16 {
    ZONE_COLORS[usize::from(z.clamp(1, 7) - 1)]
}

// ---------------------------------------------------------------------------------------------------------------
// History, for the graph field
// ---------------------------------------------------------------------------------------------------------------

/// Seconds per bar of the graph.
pub const BAR_S: u32 = 10;
/// Bars kept: more than the widest graph shows (57), so a graph drawn a few bars ago can still be compared.
pub const BARS: usize = 64;

/// Heart rate and power averaged over [`BAR_S`] seconds, the last [`BARS`] bars (0: no reading).
pub struct History {
    hr: [u8; BARS],
    power: [u16; BARS],
    /// Bars pushed so far.
    n: u32,
    sum: [u32; 2],
    count: [u32; 2],
    secs: u32,
}

impl Default for History {
    fn default() -> Self {
        History { hr: [0; BARS], power: [0; BARS], n: 0, sum: [0; 2], count: [0; 2], secs: 0 }
    }
}

impl History {
    /// One second of readings; true when a bar was pushed.
    pub fn second(&mut self, hr: Option<u16>, power: Option<u16>) -> bool {
        for (k, v) in [hr, power].into_iter().enumerate() {
            if let Some(v) = v {
                self.sum[k] = self.sum[k].saturating_add(u32::from(v));
                self.count[k] += 1;
            }
        }
        self.secs += 1;
        if self.secs < BAR_S {
            return false;
        }
        let avg = |k: usize| self.sum[k].checked_div(self.count[k]).unwrap_or(0);
        let i = self.n as usize % BARS;
        self.hr[i] = avg(0).min(255) as u8;
        self.power[i] = avg(1).min(u32::from(u16::MAX)) as u16;
        self.n = self.n.wrapping_add(1);
        (self.sum, self.count, self.secs) = ([0; 2], [0; 2], 0);
        true
    }

    /// Bars pushed so far.
    pub fn len(&self) -> u32 {
        self.n
    }

    /// True before the first bar.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Bar `k` (counted from the first one ever), power or heart rate; 0 if it is not kept (or never was).
    pub fn bar(&self, k: u32, power: bool) -> u16 {
        if k >= self.n || self.n - k > BARS as u32 {
            return 0;
        }
        let i = k as usize % BARS;
        if power { self.power[i] } else { u16::from(self.hr[i]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(speed: u16) -> Input {
        Input { speed_x10: Some(speed), hr: Some(140), cadence: Some(90), power: Some(200), alt_mm: None }
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
    }

    #[test]
    fn history_bars() {
        let mut h = History::default();
        for s in 0..25u16 {
            h.second(Some(100 + s), if s < 10 { None } else { Some(300) });
        }
        assert_eq!(h.len(), 2);
        assert_eq!((h.bar(0, false), h.bar(1, false), h.bar(0, true), h.bar(1, true)), (104, 114, 0, 300));
        assert_eq!(h.bar(2, false), 0, "not yet");
        for _ in 0..BAR_S * BARS as u32 {
            h.second(Some(1), None);
        }
        assert_eq!(h.bar(1, false), 0, "dropped");
    }
}
