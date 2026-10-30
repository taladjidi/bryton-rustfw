//! Reminders and alerts during a ride, fed one second at a time: eat and drink every so many minutes of ride time
//! (stock has six reminder kinds by time, distance, energy or heart rate; Garmin and Wahoo eat / drink by time), and
//! heart rate over a limit. Pure logic: the app says each one in a banner and beeps.
//!
//! The heart-rate alert is not a nag: it fires after [`HR_OVER_S`] seconds over the limit, then again only once the
//! heart rate has come back [`HR_REARM_BPM`] under it, and at most every [`HR_EVERY_S`].

/// Seconds over the heart-rate limit before the alert.
pub const HR_OVER_S: u32 = 10;
/// Below the limit by this much re-arms the alert.
pub const HR_REARM_BPM: u16 = 5;
/// At most one heart-rate alert this often.
pub const HR_EVERY_S: u32 = 60;

/// Cadence under this, rpm, in ERG is too slow: the trainer keeps the watts, so the resistance climbs.
pub const CADENCE_LOW_RPM: u16 = 50;
/// Seconds under it before the warning.
pub const CADENCE_LOW_S: u32 = 5;
/// At most one cadence warning this often.
pub const CADENCE_EVERY_S: u32 = 60;

/// Seconds past a limit (power, cadence, speed, heart rate low, DFA a1) before its alert.
pub const LIMIT_HOLD_S: u32 = 5;
/// At most one alert of a limit this often.
pub const LIMIT_EVERY_S: u32 = 60;
/// The limits in the settings (`Settings::limits`), 0 being off: power high, power low, cadence high, cadence low,
/// speed high, speed low (km/h), heart rate low, then the reminders by distance (km), kcal and kJ, and the DFA a1
/// alert (hundredths, under it).
pub const LIMITS: usize = 11;

/// What the limits and the reminders are checked against, one second of a moving ride.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    /// Watts.
    pub power: Option<u16>,
    /// rpm.
    pub cadence: Option<u16>,
    /// km/h x 10.
    pub speed_x10: Option<u16>,
    /// bpm.
    pub hr: Option<u16>,
    /// DFA a1 x 100 (None without the strap's beats).
    pub dfa_x100: Option<u16>,
    /// Ride distance, m.
    pub dist_m: u32,
    /// Energy, kcal.
    pub kcal: u32,
    /// Work, kJ.
    pub kj: u32,
}

/// One limit's state: seconds past it, armed, seconds since it last said.
#[derive(Clone, Copy, Debug)]
struct Band {
    held: u32,
    armed: bool,
    since: u32,
}

impl Default for Band {
    fn default() -> Self {
        Band::NEW
    }
}

impl Band {
    const NEW: Band = Band { held: 0, armed: true, since: LIMIT_EVERY_S };

    /// One second: `v` against `limit` (same unit; 0 is off), over it when `high`, else under it. True when the alert
    /// is due: [`LIMIT_HOLD_S`] seconds past, armed, and [`LIMIT_EVERY_S`] since the last. Back inside by 5 % of the
    /// limit (at least 1) re-arms; no reading counts as inside without re-arming.
    fn step(&mut self, v: Option<u32>, limit: u32, high: bool) -> bool {
        self.since = self.since.saturating_add(1);
        let margin = (limit / 20).max(1);
        match v.filter(|_| limit > 0) {
            Some(v) if (high && v > limit) || (!high && v < limit) => self.held = self.held.saturating_add(1),
            Some(v) if (high && v + margin <= limit) || (!high && v >= limit + margin) => {
                (self.held, self.armed) = (0, true)
            }
            None if limit == 0 => (self.held, self.armed) = (0, true),
            _ => self.held = 0,
        }
        let due = self.armed && self.held >= LIMIT_HOLD_S && self.since >= LIMIT_EVERY_S;
        if due {
            (self.armed, self.since) = (false, 0);
        }
        due
    }
}

/// What to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// Time to eat.
    Eat,
    /// Time to drink.
    Drink,
    /// Heart rate over the limit.
    HrHigh,
    /// Cadence under [`CADENCE_LOW_RPM`] in ERG.
    CadenceLow,
    /// Power over its limit.
    PowerHigh,
    /// Power under its limit.
    PowerLow,
    /// Cadence over its limit.
    CadHigh,
    /// Cadence under its limit.
    CadUnder,
    /// Speed over its limit.
    SpeedHigh,
    /// Speed under its limit.
    SpeedLow,
    /// Heart rate under its limit.
    HrLow,
    /// DFA a1 under its limit (the aerobic threshold, patch P9).
    DfaLow,
    /// Another distance step ridden.
    Dist,
    /// Another kcal step burned.
    Kcal,
    /// Another kJ step done.
    Kj,
    /// The sun sets within the SUNSET ALERT's minutes (once a ride).
    Sunset,
}

impl Alert {
    /// Every alert, in the order they are said (the heart rate first).
    pub const ALL: [Alert; 16] = [
        Alert::HrHigh,
        Alert::HrLow,
        Alert::PowerHigh,
        Alert::PowerLow,
        Alert::CadenceLow,
        Alert::CadHigh,
        Alert::CadUnder,
        Alert::SpeedHigh,
        Alert::SpeedLow,
        Alert::DfaLow,
        Alert::Eat,
        Alert::Drink,
        Alert::Dist,
        Alert::Kcal,
        Alert::Kj,
        Alert::Sunset,
    ];

    /// Whether this one is an alarm (a limit passed, red) and not a reminder.
    pub fn alarm(self) -> bool {
        !matches!(self, Alert::Eat | Alert::Drink | Alert::Dist | Alert::Kcal | Alert::Kj | Alert::Sunset)
    }

    /// Whether `text` is one of the banners of an alert.
    pub fn is_text(text: &str) -> bool {
        Alert::ALL.iter().any(|a| a.text() == text)
    }

    /// The banner's text.
    pub fn text(self) -> &'static str {
        match self {
            Alert::Eat => "EAT REMINDER",
            Alert::Drink => "DRINK REMINDER",
            Alert::HrHigh => "HEART RATE HIGH",
            Alert::CadenceLow => "CADENCE LOW",
            Alert::PowerHigh => "POWER HIGH",
            Alert::PowerLow => "POWER LOW",
            Alert::CadHigh => "CADENCE HIGH",
            Alert::CadUnder => "CADENCE UNDER",
            Alert::SpeedHigh => "SPEED HIGH",
            Alert::SpeedLow => "SPEED LOW",
            Alert::HrLow => "HEART RATE LOW",
            Alert::DfaLow => "DFA A1 LOW",
            Alert::Dist => "DISTANCE REMINDER",
            Alert::Kcal => "KCAL REMINDER",
            Alert::Kj => "KJ REMINDER",
            Alert::Sunset => "SUNSET SOON",
        }
    }
}

/// Alerts due and not said yet: one bit each, so two in the same second (or a catch-up of several) are not lost to
/// the one the banner shows; said one at a time, the heart rate first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Queue(u16);

impl Queue {
    /// Adds `a` (nothing for `None`).
    pub fn push(&mut self, a: Option<Alert>) {
        self.0 |= a.map_or(0, |a| 1 << a as u8);
    }

    /// Takes the next to say ([`Alert::ALL`]'s order).
    pub fn pop(&mut self) -> Option<Alert> {
        let a = Alert::ALL.into_iter().find(|&a| self.0 & 1 << a as u8 != 0)?;
        self.0 &= !(1 << a as u8);
        Some(a)
    }
}

/// The reminders' and alerts' state for one ride.
#[derive(Clone, Copy, Debug, Default)]
pub struct Alerts {
    over_s: u32,
    armed: bool,
    since_hr_s: u32,
    /// The timer at the last call (a reminder is due when the timer reaches its minute, not while it stands still).
    last_timer_s: u32,
    /// Reminders due and not said yet (one a second): bit 0 eat, bit 1 drink.
    pending: u8,
    /// Seconds in a row under [`CADENCE_LOW_RPM`] in ERG, and since the last warning.
    slow_s: u32,
    since_cadence_s: u32,
    /// The limits' states: [`LIMITS`] 0 to 6, then the DFA a1's.
    bands: [Band; 8],
    /// The distance (km), kcal and kJ at the last call, and whether there was one.
    prev: [u32; 3],
    primed: bool,
    /// The sunset alert has said its one this ride (a pause and a resume keep it; a reset-resume is a new [`Alerts`],
    /// so it says again if still within the window).
    sun_said: bool,
}

impl Alerts {
    /// A new ride: everything armed.
    pub fn new() -> Self {
        Alerts {
            over_s: 0,
            armed: true,
            since_hr_s: HR_EVERY_S,
            last_timer_s: 0,
            pending: 0,
            slow_s: 0,
            since_cadence_s: CADENCE_EVERY_S,
            bands: [Band::NEW; 8],
            prev: [0; 3],
            primed: false,
            sun_said: false,
        }
    }

    /// A pause or a stop: every limit's hold starts again (armed and the minute are kept).
    pub fn reset_limits(&mut self) {
        for b in self.bands.iter_mut() {
            b.held = 0;
        }
    }

    /// One second of a moving ride: the limits `set` ([`LIMITS`]) against `r` into `q`. A limit alerts after
    /// [`LIMIT_HOLD_S`] seconds past it (not on one sample), re-arms once back inside by 5 %, and says at most every
    /// [`LIMIT_EVERY_S`]; no sensor, no alert. A reminder by distance, kcal or kJ fires once each time the ride crosses
    /// a multiple of its step (the first call, and a step set mid-ride, do not).
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts, Queue, Reading};
    /// let mut a = Alerts::new();
    /// let mut set = [0; 11];
    /// set[0] = 300; // power high
    /// let mut q = Queue::default();
    /// let r = Reading { power: Some(350), ..Reading::default() };
    /// for _ in 0..4 { a.limits(&r, &set, &mut q); }
    /// assert_eq!(q.pop(), None, "4 s is not enough");
    /// a.limits(&r, &set, &mut q);
    /// assert_eq!(q.pop(), Some(Alert::PowerHigh));
    /// ```
    pub fn limits(&mut self, r: &Reading, set: &[u16; LIMITS], q: &mut Queue) {
        let v = |o: Option<u16>| o.map(u32::from);
        // a reading of 0 is no reading for the low limits: coasting or a standstill does not alert
        let nz = |o: Option<u16>| o.filter(|&x| x > 0).map(u32::from);
        let l = |i: usize, k: u32| set.get(i).map_or(0, |&x| u32::from(x) * k);
        // (reading, limit, high, alert) for each band
        let checks = [
            (v(r.power), l(0, 1), true, Alert::PowerHigh),
            (nz(r.power), l(1, 1), false, Alert::PowerLow),
            (v(r.cadence), l(2, 1), true, Alert::CadHigh),
            (nz(r.cadence), l(3, 1), false, Alert::CadUnder),
            (v(r.speed_x10), l(4, 10), true, Alert::SpeedHigh),
            (nz(r.speed_x10), l(5, 10), false, Alert::SpeedLow),
            (v(r.hr), l(6, 1), false, Alert::HrLow),
            (v(r.dfa_x100), l(10, 1), false, Alert::DfaLow),
        ];
        for (band, (val, limit, high, alert)) in self.bands.iter_mut().zip(checks) {
            if band.step(val, limit, high) {
                q.push(Some(alert));
            }
        }
        let now = [r.dist_m / 1000, r.kcal, r.kj];
        for (k, alert) in [Alert::Dist, Alert::Kcal, Alert::Kj].into_iter().enumerate() {
            let step = l(7 + k, 1);
            let (Some(prev), Some(&n)) = (self.prev.get_mut(k), now.get(k)) else { continue };
            if self.primed && step > 0 && n / step > *prev / step {
                q.push(Some(alert));
            }
            *prev = n;
        }
        self.primed = true;
    }

    /// One second of a moving ride: the cadence under [`CADENCE_LOW_RPM`] for [`CADENCE_LOW_S`] seconds with `erg` on
    /// (the trainer holds the watts) gives [`Alert::CadenceLow`], at most once in [`CADENCE_EVERY_S`]. No reading is
    /// not slow (no sensor), and a pause or no ERG starts the count again.
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts};
    /// let mut a = Alerts::new();
    /// let fired: Vec<_> = (1..=130).filter_map(|t| a.cadence(Some(40), true).map(|x| (t, x))).collect();
    /// assert_eq!(fired, [(5, Alert::CadenceLow), (65, Alert::CadenceLow), (125, Alert::CadenceLow)]);
    /// assert_eq!((1..=100).find_map(|_| a.cadence(Some(40), false)), None, "not in ERG");
    /// ```
    pub fn cadence(&mut self, rpm: Option<u16>, erg: bool) -> Option<Alert> {
        self.since_cadence_s = self.since_cadence_s.saturating_add(1);
        if erg && rpm.is_some_and(|c| c < CADENCE_LOW_RPM) {
            self.slow_s = self.slow_s.saturating_add(1);
        } else {
            self.slow_s = 0;
        }
        if self.slow_s >= CADENCE_LOW_S && self.since_cadence_s >= CADENCE_EVERY_S {
            self.since_cadence_s = 0;
            return Some(Alert::CadenceLow);
        }
        None
    }

    /// One second of a moving ride: the sunset alert, `lead_min` minutes before the sunset (0: off), `left_min` the
    /// minutes to it (None once it has set, and without a fix: no sun times, no alert). Said once a ride.
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts};
    /// let mut a = Alerts::new();
    /// assert_eq!(a.sunset(Some(30), 30), Some(Alert::Sunset));
    /// assert_eq!(a.sunset(Some(5), 30), None, "once a ride");
    /// assert_eq!(Alerts::new().sunset(Some(90), 30), None, "not yet");
    /// assert_eq!(Alerts::new().sunset(Some(10), 0), None, "OFF");
    /// assert_eq!(Alerts::new().sunset(None, 30), None, "no fix, or set");
    /// ```
    pub fn sunset(&mut self, left_min: Option<u32>, lead_min: u16) -> Option<Alert> {
        if self.sun_said || lead_min == 0 || left_min.is_none_or(|m| m > u32::from(lead_min)) {
            return None;
        }
        self.sun_said = true;
        Some(Alert::Sunset)
    }

    /// One second of a moving ride: `timer_s` the ride's timer after it, `hr` the heart rate; the settings `eat_min`
    /// and `drink_min` (0: off) and `hr_high` (0: off). At most one alert a second: the heart rate first, then eat,
    /// then drink.
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts};
    /// let mut a = Alerts::new();
    /// let fired: Vec<_> = (1..=3600).filter_map(|t| a.second(t, None, 45, 20, 0).map(|x| (t, x))).collect();
    /// assert_eq!(fired[..3], [(1200, Alert::Drink), (2400, Alert::Drink), (2700, Alert::Eat)]);
    /// ```
    pub fn second(
        &mut self,
        timer_s: u32,
        hr: Option<u16>,
        eat_min: u16,
        drink_min: u16,
        hr_high: u16,
    ) -> Option<Alert> {
        self.since_hr_s = self.since_hr_s.saturating_add(1);
        match hr {
            Some(h) if hr_high > 0 && h > hr_high => self.over_s += 1,
            Some(h) if h + HR_REARM_BPM <= hr_high => (self.over_s, self.armed) = (0, true),
            _ => self.over_s = 0,
        }
        // the reminders due are kept first: one said a second, the heart rate's alert ahead of them
        let moved = timer_s != core::mem::replace(&mut self.last_timer_s, timer_s);
        let due = |min: u16| moved && min > 0 && timer_s > 0 && timer_s.is_multiple_of(u32::from(min) * 60);
        self.pending |= u8::from(due(eat_min)) | u8::from(due(drink_min)) << 1;
        if self.armed && self.over_s >= HR_OVER_S && self.since_hr_s >= HR_EVERY_S {
            (self.armed, self.since_hr_s) = (false, 0);
            return Some(Alert::HrHigh);
        }
        let (bit, alert) = match self.pending {
            0 => return None,
            p if p & 1 != 0 => (1, Alert::Eat),
            _ => (2, Alert::Drink),
        };
        self.pending &= !bit;
        Some(alert)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn heart_rate_alert_does_not_nag() {
        let mut a = Alerts::new();
        let mut fired = 0;
        // 15 s at 180 over a 170 limit: one alert, after 10 s
        for t in 1..=15 {
            if a.second(t, Some(180), 0, 0, 170).is_some() {
                assert_eq!(t, HR_OVER_S);
                fired += 1;
            }
        }
        // hovering just under the limit, not 5 under: no new alert when it goes over again
        for t in 16..=30 {
            assert_eq!(a.second(t, Some(168), 0, 0, 170), None);
        }
        for t in 31..=100 {
            fired += usize::from(a.second(t, Some(175), 0, 0, 170).is_some());
        }
        assert_eq!(fired, 1, "not re-armed");
        // well under, then over again after a minute: a second alert
        a.second(101, Some(150), 0, 0, 170);
        for t in 102..=200 {
            fired += usize::from(a.second(t, Some(175), 0, 0, 170).is_some());
        }
        assert_eq!(fired, 2);
        // off
        let mut b = Alerts::new();
        assert!((1..100).all(|t| b.second(t, Some(200), 0, 0, 0).is_none()));
    }

    #[test]
    fn the_queue_keeps_what_is_due_together() {
        let mut q = Queue::default();
        q.push(Some(Alert::Drink));
        q.push(Some(Alert::CadenceLow));
        q.push(Some(Alert::HrHigh));
        q.push(None);
        q.push(Some(Alert::Drink));
        assert_eq!(
            [q.pop(), q.pop(), q.pop(), q.pop()],
            [Some(Alert::HrHigh), Some(Alert::CadenceLow), Some(Alert::Drink), None]
        );
    }

    #[test]
    fn cadence_warning_waits_five_seconds_and_does_not_nag() {
        let mut a = Alerts::new();
        // pedalling fine, then slow: the count starts when it goes slow, 4 s are not enough
        assert!((1..=10).all(|_| a.cadence(Some(85), true).is_none()));
        assert!((1..=4).all(|_| a.cadence(Some(45), true).is_none()));
        assert_eq!(a.cadence(Some(80), true), None, "back up: the count starts again");
        assert!((1..=4).all(|_| a.cadence(Some(0), true).is_none()));
        assert_eq!(a.cadence(Some(0), true), Some(Alert::CadenceLow), "5 s at 0 rpm");
        // still slow: not again before a minute
        assert!((1..60).all(|_| a.cadence(Some(0), true).is_none()));
        assert_eq!(a.cadence(Some(0), true), Some(Alert::CadenceLow));
        // no reading, or ERG off: nothing
        let mut b = Alerts::new();
        assert!((1..=100).all(|_| b.cadence(None, true).is_none() && b.cadence(Some(0), false).is_none()));
    }

    #[test]
    fn eat_then_drink_when_both_are_due_and_nothing_while_paused() {
        let mut a = Alerts::new();
        assert_eq!(a.second(0, None, 30, 20, 0), None, "not at the start");
        assert_eq!(a.second(3600, None, 30, 20, 0), Some(Alert::Eat));
        assert_eq!(a.second(3600, None, 30, 20, 0), Some(Alert::Drink), "the next second");
        assert_eq!(a.second(3600, None, 30, 20, 0), None, "paused on the minute: once only");
    }

    /// The heart-rate alert and an eat minute (and a drink minute) in the same second: none is lost (review 2).
    #[test]
    fn heart_rate_alert_keeps_the_reminders_due() {
        let mut a = Alerts::new();
        let fired: std::vec::Vec<_> =
            (1..=20).filter_map(|t| a.second(1190 + t, Some(180), 20, 20, 170).map(|x| (1190 + t, x))).collect();
        assert_eq!(fired, [(1200, Alert::HrHigh), (1201, Alert::Eat), (1202, Alert::Drink)]);
    }

    fn run(a: &mut Alerts, set: &[u16; LIMITS], r: Reading, secs: u32) -> std::vec::Vec<u32> {
        let mut q = Queue::default();
        (1..=secs)
            .filter(|_| {
                a.limits(&r, set, &mut q);
                q.pop().is_some()
            })
            .collect()
    }

    #[test]
    fn a_limit_fires_after_the_hold_and_rearms_with_a_margin() {
        let mut set = [0; LIMITS];
        set[0] = 300;
        let mut a = Alerts::new();
        let hi = Reading { power: Some(320), ..Reading::default() };
        assert_eq!(run(&mut a, &set, hi, 10), [LIMIT_HOLD_S], "once, after the hold");
        assert!(run(&mut a, &set, hi, 100).is_empty(), "still over: not armed");
        // just under the limit (inside the 15 W margin) does not re-arm
        run(&mut a, &set, Reading { power: Some(290), ..hi }, 5);
        assert!(run(&mut a, &set, hi, 100).is_empty());
        // well under re-arms: the next time over fires after the hold
        run(&mut a, &set, Reading { power: Some(200), ..hi }, 3);
        assert_eq!(run(&mut a, &set, hi, 70), [LIMIT_HOLD_S]);
        // 4 s over, back, 4 s over: never
        let mut b = Alerts::new();
        for _ in 0..3 {
            assert!(run(&mut b, &set, hi, 4).is_empty());
            assert!(run(&mut b, &set, Reading { power: Some(250), ..hi }, 1).is_empty());
        }
    }

    #[test]
    fn a_pause_restarts_the_hold_and_zero_is_no_reading_for_low_limits() {
        let mut set = [0; LIMITS];
        (set[0], set[1], set[3], set[5]) = (300, 100, 60, 15);
        let mut a = Alerts::new();
        let hi = Reading { power: Some(350), ..Reading::default() };
        assert!(run(&mut a, &set, hi, 4).is_empty());
        a.reset_limits();
        assert!(run(&mut a, &set, hi, 1).is_empty(), "4 s, a pause, 1 s: not 5");
        assert_eq!(run(&mut a, &set, hi, 4), [4]);
        let zero = Reading { power: Some(0), cadence: Some(0), speed_x10: Some(0), ..Reading::default() };
        assert!(run(&mut Alerts::new(), &set, zero, 100).is_empty(), "coasting, standing still");
    }

    #[test]
    fn low_limits_need_a_sensor_and_off_is_off() {
        let mut set = [0; LIMITS];
        (set[1], set[3], set[5], set[6], set[10]) = (100, 60, 15, 100, 75);
        let mut a = Alerts::new();
        assert!(run(&mut a, &set, Reading::default(), 100).is_empty(), "no sensor");
        let low = Reading {
            power: Some(50),
            cadence: Some(40),
            speed_x10: Some(100),
            hr: Some(80),
            dfa_x100: Some(60),
            ..Reading::default()
        };
        let mut q = Queue::default();
        for _ in 0..LIMIT_HOLD_S {
            a.limits(&low, &set, &mut q);
        }
        let got: std::vec::Vec<_> = core::iter::from_fn(|| q.pop()).collect();
        assert_eq!(got, [Alert::HrLow, Alert::PowerLow, Alert::CadUnder, Alert::SpeedLow, Alert::DfaLow]);
        let high = Reading { power: Some(2000), cadence: Some(250), speed_x10: Some(900), hr: Some(220), ..low };
        assert!(run(&mut Alerts::new(), &[0; LIMITS], high, 100).is_empty(), "off");
        assert!(run(&mut Alerts::new(), &[0; LIMITS], low, 100).is_empty(), "off");
    }

    #[test]
    fn reminders_by_distance_kcal_and_kj_fire_once_at_each_multiple() {
        let mut set = [0; LIMITS];
        (set[7], set[8], set[9]) = (10, 100, 200);
        let mut a = Alerts::new();
        let mut q = Queue::default();
        let mut fired = std::vec::Vec::new();
        // the first call, at 25 km, does not fire; each value is held 3 s
        for km in 25..=41u32 {
            let r = Reading { dist_m: km * 1000 + 500, kcal: km * 10, kj: km * 20, ..Reading::default() };
            for _ in 0..3 {
                a.limits(&r, &set, &mut q);
                while let Some(x) = q.pop() {
                    fired.push((km, x));
                }
            }
        }
        let at = |km| [(km, Alert::Dist), (km, Alert::Kcal), (km, Alert::Kj)];
        assert_eq!(fired, [at(30), at(40)].concat());
    }
}
