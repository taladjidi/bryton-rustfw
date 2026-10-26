//! The FTP ramp test: the workout it plays ([`workout`]), the stop rules, the best minute and the FTP from it, and the
//! first ventilatory threshold from DFA a1 ([`Ramp::hrvt1`]). Pure logic, no heap, fed one second at a time while
//! the ride's timer runs (app/ramp.rs plays the workout through the ordinary player, in ERG).
//!
//! **Protocol** (sources read 2026-10-26):
//! - TrainerRoad's Ramp Test (support.trainerroad.com, "Ramp Test FAQs" and "How to Test With the Ramp Test"): a
//!   5-minute warm-up, then a step every minute, each a little harder, until the rider cannot hold the target;
//!   FTP is 75 % of the best one-minute power. It does not publish the step in watts: its steps are set from the
//!   rider's current FTP.
//! - Zwift's Ramp Test (from memory, not re-checked: inferred): about 100 W to start and 20 W more each minute,
//!   the same 75 % of the best minute. A fixed start and step suit one rider's size; a stale FTP here would make
//!   it far too easy or too hard.
//!
//! Taken here: **TrainerRoad's shape with steps in % of the current FTP setting** (so a stale FTP still gives a
//! sensible start and a test of 10 to 15 minutes, and the trainer's ERG holds the watts): 5 min at 50 %, then
//! [`RAMP_STEPS`] steps of one minute from 56 % in +6 % each (FTP 250 W: 140 W, then +15 W a minute), the last one
//! open (Lap ends it), then 5 min at 45 % to cool down. Stop rules: cadence under 60 rpm for 5 s, power under half
//! the target for 10 s (the first 10 s of the ramp excepted), or Lap. FTP = 75 % of the best rolling 60 s power since
//! the ramp began, rounded to 1 W.
//!
//! **HRVT1.** Rogers, Giles, Draper, Mourot, Gronwald, "A New Detection Method Defining the Aerobic Threshold for
//! Endurance Exercise and Training Prescription Based on Fractal Correlation Properties of Heart Rate Variability",
//! Front. Physiol. 11:596567 (2021, doi 10.3389/fphys.2020.596567): in an incremental test, the first ventilatory
//! threshold is where DFA a1 falls to 0.75 (abstract read; the paper's text could not be fetched, so the choice of
//! one regression line over the whole ramp is this module's, inferred). Each second with the 2 minute window full
//! gives one point, the mean power of the last 2 minutes (the span DFA a1 is computed over, hrv.rs) against a1; a
//! least-squares line through them is solved for a1 = 0.75. No result unless there are [`MIN_POINTS`] points, the
//! slope is negative, the powers span [`MIN_SPREAD_W`], and the crossing lies among the powers seen.

use crate::storage::workout::{Kind, Length, Step, Target, Workout};

/// The warm-up's length, s, and its power, % of FTP.
pub const WARMUP_S: u32 = 300;
const WARMUP_PCT: u16 = 50;
/// The first ramp step's power, % of FTP, and the rise of each step.
const START_PCT: u16 = 56;
const STEP_PCT: u16 = 6;
/// Each ramp step's length, s.
pub const STEP_S: u32 = 60;
/// Ramp steps (the last one open).
pub const RAMP_STEPS: usize = 30;
/// The cool-down's length, s, and its power, % of FTP.
pub const COOL_S: u32 = 300;
const COOL_PCT: u16 = 45;
/// The index of the first ramp step and of the cool-down in [`workout`].
pub const FIRST: usize = 1;
/// See [`FIRST`].
pub const COOL: usize = FIRST + RAMP_STEPS;

/// Seconds at the start of the ramp that no stop rule counts (the trainer takes a moment to hold the target).
const GRACE_S: u32 = 10;
/// Cadence under this (rpm) for [`CAD_S`] seconds stops the test.
pub const CAD_MIN: u16 = 60;
/// See [`CAD_MIN`].
pub const CAD_S: u32 = 5;
/// Power under this % of the target for [`FADE_S`] seconds stops the test.
pub const FADE_PCT: u32 = 50;
/// See [`FADE_PCT`].
pub const FADE_S: u32 = 10;
/// The window of the best minute, s, and of DFA a1, s.
const MINUTE: usize = 60;
const WINDOW: usize = 120;
/// Points the regression needs, and the spread of power it needs, W.
pub const MIN_POINTS: u32 = 60;
/// See [`MIN_POINTS`].
pub const MIN_SPREAD_W: u16 = 20;
/// DFA a1 values (hundredths) taken as valid.
const A1_RANGE: core::ops::RangeInclusive<u16> = 30..=150;

/// The ramp test as a workout: warm-up, [`RAMP_STEPS`] one-minute steps (percentages of the FTP setting, so the
/// player works out the watts from the FTP in force), cool-down.
///
/// ```
/// use bryton_rustfw::ramp;
/// let w = ramp::workout();
/// let steps = w.steps();
/// assert_eq!(steps.len(), 32);
/// let watts = |k: usize| steps[k].target.range(250, 190).map(|(lo, _)| lo);
/// assert_eq!([0, 1, 2, 3, 30, 31].map(watts), [125, 140, 155, 170, 575, 113].map(Some), "FTP 250 W");
/// assert_eq!(watts(1).map(|w| w * 100 / 250), Some(56));
/// ```
pub fn workout() -> Workout {
    let step = |len, pct, intensity| Step {
        len,
        target: Target { kind: Kind::Power, lo: pct, hi: pct, pct: true },
        intensity,
    };
    let mut all = [step(Length::Time(STEP_S), 0, 5); RAMP_STEPS + 2];
    for (k, s) in all.iter_mut().enumerate() {
        *s = match k {
            0 => step(Length::Time(WARMUP_S), WARMUP_PCT, 2),
            COOL => step(Length::Time(COOL_S), COOL_PCT, 3),
            k if k == COOL - 1 => step(Length::Open, START_PCT + STEP_PCT * (k - FIRST) as u16, 5),
            k => step(Length::Time(STEP_S), START_PCT + STEP_PCT * (k - FIRST) as u16, 5),
        };
    }
    Workout::built("RAMP TEST", &all)
}

/// Step `i` of [`workout`] is a ramp step.
pub fn in_ramp(i: usize) -> bool {
    (FIRST..COOL).contains(&i)
}

/// What the test found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The best rolling minute, W.
    pub best_w: u16,
    /// 75 % of it, W.
    pub ftp: u16,
    /// The ramp's length, s.
    pub secs: u32,
    /// Power at DFA a1 = 0.75, W: None without enough clean data.
    pub aet: Option<u16>,
}

/// The FTP from the sum of the best 60 s of power: 75 % of the mean, rounded to 1 W.
///
/// ```
/// use bryton_rustfw::ramp::{best_w, ftp_of};
/// assert_eq!((best_w(60 * 300), ftp_of(60 * 300)), (300, 225));
/// assert_eq!(ftp_of(60 * 301), 226, "225.75");
/// assert_eq!(ftp_of(60 * 300 + 30), 225, "225.375");
/// assert_eq!(ftp_of(0), 0);
/// ```
pub fn ftp_of(sum_60s: u32) -> u16 {
    ((u64::from(sum_60s) * 3 + 120) / 240).min(u64::from(u16::MAX)) as u16
}

/// The mean of the best 60 s from their sum, rounded.
pub fn best_w(sum_60s: u32) -> u16 {
    ((sum_60s + 30) / 60).min(u32::from(u16::MAX)) as u16
}

/// The ramp as it goes.
#[derive(Clone, Debug)]
pub struct Ramp {
    ring: [u16; WINDOW],
    head: usize,
    filled: usize,
    sum60: u32,
    sum120: u32,
    best60: u32,
    secs: u32,
    low_cad: u32,
    low_pow: u32,
    n: u32,
    sp: f64,
    sa: f64,
    spp: f64,
    spa: f64,
    pmin: u16,
    pmax: u16,
}

impl Default for Ramp {
    fn default() -> Self {
        Self::new()
    }
}

impl Ramp {
    /// A ramp that has not begun.
    pub const fn new() -> Self {
        Ramp {
            ring: [0; WINDOW],
            head: 0,
            filled: 0,
            sum60: 0,
            sum120: 0,
            best60: 0,
            secs: 0,
            low_cad: 0,
            low_pow: 0,
            n: 0,
            sp: 0.0,
            sa: 0.0,
            spp: 0.0,
            spa: 0.0,
            pmin: u16::MAX,
            pmax: 0,
        }
    }

    /// One ramp second: the `power` and `cadence` read (a missing power counts as 0, a missing cadence not at all),
    /// the step's `target_w` and DFA a1 in hundredths if the strap gives one. True: the test is over (a stop rule).
    pub fn second(&mut self, power: Option<u16>, cadence: Option<u16>, target_w: u16, a1: Option<u16>) -> bool {
        let p = power.unwrap_or(0);
        // the slot at `head` holds the second 120 ago, the one MINUTE back leaves the best minute's window
        let old = |ring: &[u16; WINDOW], at: usize| u32::from(ring.get(at).copied().unwrap_or(0));
        self.sum120 = self.sum120 + u32::from(p) - old(&self.ring, self.head);
        self.sum60 = self.sum60 + u32::from(p) - old(&self.ring, (self.head + WINDOW - MINUTE) % WINDOW);
        if let Some(s) = self.ring.get_mut(self.head) {
            *s = p;
        }
        self.head = (self.head + 1) % WINDOW;
        self.filled = (self.filled + 1).min(WINDOW);
        self.secs = self.secs.saturating_add(1);
        if self.filled >= MINUTE {
            self.best60 = self.best60.max(self.sum60);
        }
        if let Some(a) = a1.filter(|a| A1_RANGE.contains(a) && self.filled >= WINDOW) {
            let mean = f64::from(self.sum120) / WINDOW as f64;
            let a = f64::from(a);
            self.n += 1;
            (self.sp, self.sa, self.spp, self.spa) =
                (self.sp + mean, self.sa + a, self.spp + mean * mean, self.spa + mean * a);
            let m = mean as u16;
            (self.pmin, self.pmax) = (self.pmin.min(m), self.pmax.max(m));
        }
        if self.secs <= GRACE_S {
            return false;
        }
        self.low_cad = if cadence.is_some_and(|c| c < CAD_MIN) { self.low_cad + 1 } else { 0 };
        self.low_pow = if u32::from(p) * 100 < u32::from(target_w) * FADE_PCT { self.low_pow + 1 } else { 0 };
        self.low_cad >= CAD_S || self.low_pow >= FADE_S
    }

    /// The result so far; None until a whole minute has been ridden.
    pub fn outcome(&self) -> Option<Outcome> {
        (self.best60 > 0).then(|| Outcome {
            best_w: best_w(self.best60),
            ftp: ftp_of(self.best60),
            secs: self.secs,
            aet: self.hrvt1(),
        })
    }

    /// The power where DFA a1 crosses 0.75 on the line through the points, W.
    ///
    /// ```
    /// use bryton_rustfw::ramp::Ramp;
    /// // a1 falls 0.025 per 10 W of mean power, from 1.00 at 150 W: 0.75 at 250 W
    /// let mut r = Ramp::new();
    /// for s in 0..1200u32 {
    ///     let power = 100 + (s / 4) as u16; // 1 W per 4 s
    ///     let mean = power - 15; // the mean of the last 2 minutes trails by 60 s
    ///     let a1 = (100 - (i32::from(mean) - 150) / 4).clamp(35, 140) as u16;
    ///     r.second(Some(power), Some(90), 0, Some(a1));
    /// }
    /// let aet = r.outcome().and_then(|o| o.aet).unwrap();
    /// assert!((247..=253).contains(&aet), "{aet}");
    /// ```
    pub fn hrvt1(&self) -> Option<u16> {
        if self.n < MIN_POINTS || self.pmax.saturating_sub(self.pmin) < MIN_SPREAD_W {
            return None;
        }
        let n = f64::from(self.n);
        let num = n * self.spa - self.sp * self.sa;
        let den = n * self.spp - self.sp * self.sp;
        if !(den > 1e-6 && num < 0.0) {
            return None;
        }
        let p = (self.sp * num + (75.0 * n - self.sa) * den) / (n * num);
        (p >= f64::from(self.pmin) && p <= f64::from(self.pmax) + 0.5).then_some((p + 0.5) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(r: &mut Ramp, secs: u32, p: u16, cad: u16, target: u16) -> Option<u32> {
        (0..secs).find(|_| r.second(Some(p), Some(cad), target, None)).map(|s| s + 1)
    }

    #[test]
    fn steps_from_the_ftp() {
        let w = workout();
        let s = w.steps();
        assert_eq!(s.len(), RAMP_STEPS + 2);
        assert_eq!(s.first().map(|s| s.len), Some(Length::Time(WARMUP_S)));
        assert_eq!(s.get(COOL - 1).map(|s| s.len), Some(Length::Open), "the last ramp step waits for Lap");
        assert_eq!(s.get(COOL).map(|s| s.len), Some(Length::Time(COOL_S)));
        assert!(s.iter().skip(FIRST).take(RAMP_STEPS - 1).all(|s| s.len == Length::Time(STEP_S)));
        // a stale FTP of 120 and a strong one of 400: the start follows
        let start = |ftp| s[FIRST].target.range(ftp, 190).map(|r| r.0);
        assert_eq!([start(120), start(250), start(400)], [67, 140, 224].map(Some));
        assert_eq!(w.total_s(), WARMUP_S + STEP_S * (RAMP_STEPS as u32 - 1) + COOL_S);
    }

    #[test]
    fn stops_on_cadence_and_on_fade() {
        let mut r = Ramp::new();
        // cadence under 60: the 5th second after the grace
        assert_eq!(feed(&mut r, 10, 200, 40, 200), None, "the first 10 s count for nothing");
        assert_eq!(feed(&mut r, 10, 200, 40, 200), Some(5));
        let mut r = Ramp::new();
        feed(&mut r, 30, 200, 90, 200);
        assert_eq!(feed(&mut r, 20, 200, 59, 200), Some(5));
        // a pedal stroke back resets the count
        let mut r = Ramp::new();
        feed(&mut r, 30, 200, 90, 200);
        assert_eq!(feed(&mut r, 4, 200, 30, 200), None);
        feed(&mut r, 1, 200, 80, 200);
        assert_eq!(feed(&mut r, 4, 200, 30, 200), None);
        // power under half the target for 10 s; 100 W of 200 is not under
        let mut r = Ramp::new();
        feed(&mut r, 30, 200, 90, 200);
        assert_eq!(feed(&mut r, 30, 100, 90, 200), None);
        assert_eq!(feed(&mut r, 30, 99, 90, 200), Some(10));
        // no cadence reading: only the power rule
        let mut r = Ramp::new();
        assert_eq!((0..40).find(|_| r.second(Some(200), None, 200, None)), None);
        assert!((0..20).any(|_| r.second(None, None, 200, None)));
    }

    #[test]
    fn best_minute_and_ftp() {
        let mut r = Ramp::new();
        assert_eq!(r.outcome(), None, "no whole minute yet");
        feed(&mut r, 59, 300, 90, 0);
        assert_eq!(r.outcome(), None);
        // climbing 1 W a second: the best 60 s is the last
        for s in 0..300u16 {
            r.second(Some(100 + s), Some(90), 0, None);
        }
        let o = r.outcome().unwrap();
        // the last 60 s: 100 + 240 ..= 100 + 299 averages 369.5
        assert_eq!(o.best_w, 370);
        assert_eq!(o.ftp, ftp_of(60 * 369 + 30));
        assert_eq!(o.secs, 359);
        // then a drop: the best stays
        feed(&mut r, 100, 150, 90, 0);
        assert_eq!(r.outcome().map(|o| o.best_w), Some(370));
    }

    #[test]
    fn hrvt1_needs_clean_data_and_a_falling_line() {
        let run = |a1: &dyn Fn(u32, u16) -> Option<u16>| {
            let mut r = Ramp::new();
            for s in 0..1200u32 {
                let p = 100 + (s / 4) as u16;
                r.second(Some(p), Some(90), 0, a1(s, p));
            }
            r.outcome().and_then(|o| o.aet)
        };
        let falling = |_: u32, p: u16| Some((130 - i32::from(p) / 4).clamp(31, 149) as u16);
        let aet = run(&falling).unwrap();
        // the mean trails the power by 60 s of ramp, 15 W: a1 = 130 - (mean + 15) / 4 is 0.75 at a mean of 205 W
        assert!((202..=208).contains(&aet), "{aet}");
        assert_eq!(run(&|_, _| None), None, "no strap");
        assert_eq!(run(&|s, p| falling(s, p).filter(|_| s < 150)), None, "too few points");
        assert_eq!(run(&|_, _| Some(100)), None, "flat: no crossing");
        assert_eq!(run(&|_, p| Some(40 + p / 4)), None, "rising");
        assert_eq!(run(&|_, _| Some(0)), None, "invalid values are left out");
        assert_eq!(run(&|_, p| Some(240 - p / 4)), None, "a1 never gets to 0.75 within the powers ridden");
    }
}
