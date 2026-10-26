//! The heart rate variability thresholds from ordinary rides: the power at DFA alpha1 = 0.75 (HRVT1, the first
//! threshold, about the aerobic threshold) and at 0.5 (HRVT2, about the second), from the full copies' `Alpha1`
//! and power ([`crate::recorder`], `\Rustfw\Activities`), one ride at a time, then the median of the recent rides.
//! Pure logic, no heap, bounded: a ring of 2 minutes of power and running sums per ride, [`RIDES`] results kept.
//!
//! **The thresholds.** Rogers, Giles, Draper, Hoos and Gronwald 2021 ("A new detection method defining the aerobic
//! threshold for endurance exercise and training prescription based on fractal correlation properties of heart rate
//! variability", Front. Physiol. 11: 596567): alpha1 = 0.75 at the first ventilatory threshold. Rogers, Giles,
//! Draper, Mourot and Gronwald 2021 ("Detection of the anaerobic threshold in endurance sports: validation of a new
//! method using correlation properties of heart rate variability", J. Funct. Morphol. Kinesiol. 6: 38): 0.5 at the
//! second (VT2), 15 runners, r = 0.78 by heart rate. **How loose:** in cyclists, Schaffarczyk, Rogers, Reer and
//! Gronwald (Front. Physiol. 2024, 15: 1329360, a ramp, 2 min windows every 5 s, linear regression of alpha1 over
//! about 1.0 to 0.5) found HRVT2 5 +- 22 W from VT2 (no significant bias, ICC 0.92), but HRVT1 45 +- 35 W above VT1
//! (it matched a 2.5 mmol/l lactate better, 10 +- 21 W): a value here is a guide within some 20 to 40 W, not a
//! test. The fixed 0.75 and 0.5 are not fitted to the rider.
//!
//! **From ordinary rides** (not a ramp): Andriolo, Rummel and Gronwald 2024 ("Relationship of cycling power and
//! non-linear heart rate variability from everyday workout data", Sensors 24: 4468, the AI Endurance data), taken
//! here: alpha1 on a 120 s window every 5 s (hrv.rs: 2 minutes of beats, recomputed every 5), power as the mean
//! over the same 120 s; the first 5 minutes left out (the strap settling); points with no power left out; a ride
//! used only when at least half its points are in alpha1's dynamic range (below 1.0); a linear fit of power on
//! alpha1, `P = m a1 + q`, read at 0.75 and 0.5, over the points of alpha1 up to [`FIT_MAX`] (as the ramp studies fit
//! its linear part). Their other gates, not here: workouts with more than 5 %
//! artifacts (the full copy does not record the share hrv.rs dropped, only alpha1: recording it is the next step),
//! and with pedalling under 90 % of the time (outdoors that would drop most rides; the 120 s mean counts coasting).
//!
//! **This ride's gates** (inferred, tuned on synthetic rides): at least [`MIN_POINTS`] points (5 minutes of them),
//! alpha1 spread over at least [`MIN_SPREAD`] (a ride all in one zone gives no slope), the slope negative (more
//! power, lower alpha1), r^2 at least [`MIN_R2`], and a threshold given only when the ride's alpha1 came within
//! [`REACH`] of it (no long extrapolation): an easy ride gives HRVT1 at best, rarely HRVT2.
//!
//! **Over rides** ([`Recent`]): the median of the rides of the last [`DAYS`] days that gave one, trusted from
//! [`MIN_RIDES`] rides.

use crate::power_curve::FILL_S;

/// The window of alpha1 and of the power paired with it, s.
pub const WINDOW_S: usize = 120;
/// A point every this many seconds.
pub const EVERY_S: u32 = 5;
/// The first seconds of a ride left out (the strap settling).
pub const SKIP_S: u32 = 300;
/// Points a ride needs.
pub const MIN_POINTS: u32 = 60;
/// The fit takes the points up to this alpha1: its dynamic range, where it falls about linearly with the
/// intensity (Schaffarczyk et al. 2024 fitted "about 1.0 to 0.5"); above it, easy riding, alpha1 scatters up to
/// 1.5 or 2 with little change in power, which flattens a straight line. At least half a ride's points must be in
/// it (Andriolo et al. 2024).
pub const FIT_MAX: f32 = 1.0;
/// alpha1's spread a ride needs in the fit's range (largest less smallest).
pub const MIN_SPREAD: f32 = 0.3;
/// The fit's r^2 at least.
pub const MIN_R2: f32 = 0.3;
/// A threshold is read only when the ride's lowest alpha1 came within this of it.
pub const REACH: f32 = 0.1;
/// alpha1 at the first and the second threshold.
pub const A1_VT1: f32 = 0.75;
/// See [`A1_VT1`].
pub const A1_VT2: f32 = 0.5;
/// Rides kept at most, the most recent first read.
pub const RIDES: usize = 32;
/// Rides older than this many days are not read.
pub const DAYS: u16 = 42;
/// Rides giving a threshold before its median is trusted.
pub const MIN_RIDES: u8 = 3;

/// One ride's thresholds, W (None: not from this ride).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Thresholds {
    /// Power at alpha1 = 0.75.
    pub vt1: Option<u16>,
    /// Power at alpha1 = 0.5.
    pub vt2: Option<u16>,
}

/// One ride, a record at a time: the last 2 minutes of power and the sums of the fit.
#[derive(Debug)]
pub struct RideFit {
    ring: [u16; WINDOW_S],
    head: usize,
    len: usize,
    sum: u32,
    first: Option<u32>,
    last: Option<u32>,
    /// Points in the fit (alpha1 up to [`FIT_MAX`]): count, the sums of x = alpha1 and y = power (f64: ten
    /// thousand points of 300 W squared), alpha1's smallest and largest.
    n: u32,
    sums: [f64; 5],
    /// Points in all, alpha1 above [`FIT_MAX`] too.
    all: u32,
    a1_min: f32,
    a1_max: f32,
}

impl Default for RideFit {
    fn default() -> Self {
        Self::new()
    }
}

impl RideFit {
    /// No record yet.
    pub const fn new() -> Self {
        RideFit {
            ring: [0; WINDOW_S],
            head: 0,
            len: 0,
            sum: 0,
            first: None,
            last: None,
            n: 0,
            sums: [0.0; 5],
            all: 0,
            a1_min: f32::MAX,
            a1_max: 0.0,
        }
    }

    /// The record at FIT time `t`, its power and alpha1 x 1000 (records out of order are skipped; a gap over
    /// [`FILL_S`] or a record without power starts the window again, a shorter gap is filled with this sample as
    /// the power curve does).
    pub fn sample(&mut self, t: u32, power: Option<u16>, alpha1_milli: Option<u16>) {
        let gap = match self.last {
            Some(last) if t <= last => return,
            Some(last) => t - last,
            None => u32::MAX,
        };
        self.last = Some(t);
        let first = *self.first.get_or_insert(t);
        let Some(w) = power else {
            (self.len, self.sum) = (0, 0);
            return;
        };
        let fill = if gap > FILL_S {
            (self.len, self.sum) = (0, 0);
            1
        } else {
            gap
        };
        for _ in 0..fill {
            if self.len == WINDOW_S {
                self.sum -= u32::from(self.ring.get(self.head).copied().unwrap_or(0));
            }
            if let Some(s) = self.ring.get_mut(self.head) {
                *s = w;
            }
            self.sum += u32::from(w);
            self.head = (self.head + 1) % WINDOW_S;
            self.len = (self.len + 1).min(WINDOW_S);
        }
        let a1 = alpha1_milli.map(|m| f32::from(m) / 1000.0).filter(|a| *a > 0.0 && *a < 3.0);
        if let Some(a1) = a1
            && self.len == WINDOW_S
            && self.sum > 0
            && t - first >= SKIP_S
            && t.is_multiple_of(EVERY_S)
        {
            self.all += 1;
            if a1 <= FIT_MAX {
                let (x, y) = (f64::from(a1), f64::from(self.sum) / WINDOW_S as f64);
                for (s, v) in self.sums.iter_mut().zip([x, y, x * x, x * y, y * y]) {
                    *s += v;
                }
                self.n += 1;
                (self.a1_min, self.a1_max) = (self.a1_min.min(a1), self.a1_max.max(a1));
            }
        }
    }

    /// The fit `P = m a1 + q` and its r^2, if the ride passes the gates (module doc).
    pub fn fit(&self) -> Option<(f32, f32, f32)> {
        let n = f64::from(self.n);
        let [sx, sy, sxx, sxy, syy] = self.sums;
        let (vx, vy, cxy) = (n * sxx - sx * sx, n * syy - sy * sy, n * sxy - sx * sy);
        if self.n < MIN_POINTS || self.n * 2 < self.all || self.a1_max - self.a1_min < MIN_SPREAD {
            return None;
        }
        if !(vx > 0.0 && vy > 0.0) {
            return None;
        }
        let m = cxy / vx;
        let r2 = cxy * cxy / (vx * vy);
        if !(m < 0.0 && r2 >= f64::from(MIN_R2)) {
            return None;
        }
        Some((m as f32, ((sy - m * sx) / n) as f32, r2 as f32))
    }

    /// The ride's thresholds.
    ///
    /// ```
    /// use bryton_rustfw::hrvt::RideFit;
    /// // a ride climbing from 150 to 278 W, alpha1 falling as 1.3 - (P - 150) / 200 (0.75 at 260 W, 0.5 at 310)
    /// let mut r = RideFit::new();
    /// for t in 0..3600u32 {
    ///     let p = 150 + t / 28;
    ///     r.sample(t, Some(p as u16), Some((1300 - (p - 150) * 5) as u16));
    /// }
    /// let got = r.thresholds();
    /// assert!(got.vt1.unwrap().abs_diff(260) <= 3, "{got:?}"); // the 2-minute mean lags a little
    /// assert_eq!(got.vt2, None); // alpha1 never came near 0.5
    /// ```
    pub fn thresholds(&self) -> Thresholds {
        let Some((m, q, _)) = self.fit() else { return Thresholds::default() };
        let at = |a1: f32| {
            let p = m * a1 + q;
            (self.a1_min <= a1 + REACH && p > 0.0 && p < 2000.0).then_some((p + 0.5) as u16)
        };
        Thresholds { vt1: at(A1_VT1), vt2: at(A1_VT2) }
    }
}

/// The recent rides' thresholds: each ride's, and their medians.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recent {
    vt1: [u16; RIDES],
    vt2: [u16; RIDES],
    n1: u8,
    n2: u8,
    /// Rides read (full copies of the last [`DAYS`] days).
    pub read: u8,
}

impl Default for Recent {
    fn default() -> Self {
        Self::new()
    }
}

impl Recent {
    /// None yet.
    pub const fn new() -> Self {
        Recent { vt1: [0; RIDES], vt2: [0; RIDES], n1: 0, n2: 0, read: 0 }
    }

    /// One more ride read; past [`RIDES`] giving a value, the others are left out.
    pub fn add(&mut self, t: Thresholds) {
        self.read = self.read.saturating_add(1);
        for (v, list, n) in [(t.vt1, &mut self.vt1, &mut self.n1), (t.vt2, &mut self.vt2, &mut self.n2)] {
            if let (Some(v), Some(slot)) = (v, list.get_mut(usize::from(*n))) {
                *slot = v;
                *n += 1;
            }
        }
    }

    /// The median of the rides' HRVT1 and how many gave one.
    ///
    /// ```
    /// use bryton_rustfw::hrvt::{Recent, Thresholds};
    /// let mut r = Recent::new();
    /// for w in [210, 190, 260, 205] {
    ///     r.add(Thresholds { vt1: Some(w), vt2: None });
    /// }
    /// r.add(Thresholds::default());
    /// assert_eq!((r.vt1(), r.vt2(), r.read), ((Some(207), 4), (None, 0), 5));
    /// ```
    pub fn vt1(&self) -> (Option<u16>, u8) {
        (median(&self.vt1, self.n1), self.n1)
    }

    /// The same for HRVT2.
    pub fn vt2(&self) -> (Option<u16>, u8) {
        (median(&self.vt2, self.n2), self.n2)
    }

    /// HRVT2 when enough rides gave one ([`MIN_RIDES`]).
    pub fn vt2_trusted(&self) -> Option<u16> {
        self.vt2().0.filter(|_| self.n2 >= MIN_RIDES)
    }
}

/// The median of the first `n` of `v` (the mean of the middle two for an even count); None for none.
fn median(v: &[u16; RIDES], n: u8) -> Option<u16> {
    let mut s = *v;
    let k = usize::from(n).min(RIDES);
    let s = s.get_mut(..k)?;
    s.sort_unstable();
    let hi = *s.get(k / 2)?;
    let lo = if k % 2 == 0 { *s.get(k / 2 - 1)? } else { hi };
    Some(((u32::from(lo) + u32::from(hi)) / 2) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ride of `secs` s: power `p(t)`, alpha1 from it by the rider's line (0.75 at `vt1`, 0.5 at `vt2`), with
    /// noise of +- `noise` on alpha1.
    pub(crate) fn ride(secs: u32, p: impl Fn(u32) -> u16, vt1: f32, vt2: f32, noise: f32) -> RideFit {
        let mut r = RideFit::new();
        let slope = -0.25 / (vt2 - vt1); // alpha1 per W
        let mut x = 7u32;
        for t in 0..secs {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let jitter = (x % 2001) as f32 / 1000.0 - 1.0;
            // alpha1 follows the 2-minute mean (its own window): the mean of the last 120 s of p
            let lo = t.saturating_sub(WINDOW_S as u32 - 1);
            let mean = (lo..=t).map(|s| f32::from(p(s))).sum::<f32>() / (t - lo + 1) as f32;
            let a1 = (0.75 + (mean - vt1) * slope + jitter * noise).clamp(0.2, 1.6);
            r.sample(1000 + t, Some(p(t)), Some((a1 * 1000.0) as u16));
        }
        r
    }

    /// Steps of 10 minutes from 140 to 300 W: both thresholds found, within a few watts.
    #[test]
    fn steps_find_both() {
        let r = ride(4800, |t| (140 + t / 600 * 20) as u16, 210.0, 265.0, 0.05);
        let got = r.thresholds();
        assert!(got.vt1.unwrap().abs_diff(210) <= 6 && got.vt2.unwrap().abs_diff(265) <= 6, "{got:?}");
    }

    /// A ride all in one zone gives nothing; nor does a short one, or one alpha1 does not follow.
    #[test]
    fn gates() {
        let flat = ride(3600, |_| 180, 210.0, 265.0, 0.03);
        assert_eq!(flat.thresholds(), Thresholds::default(), "one zone: no spread");
        let short = ride(500, |t| (140 + t / 3) as u16, 210.0, 265.0, 0.0);
        assert_eq!(short.fit(), None, "under 5 minutes of points after the first 5");
        let mut noise = RideFit::new();
        let mut x = 3u32;
        for t in 0..3600u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            noise.sample(t, Some((150 + t / 30) as u16), Some((300 + x % 700) as u16));
        }
        assert_eq!(noise.fit(), None, "alpha1 unrelated to power: r^2 too low");
        let easy = ride(3600, |t| (100 + t / 60) as u16, 260.0, 320.0, 0.02);
        assert_eq!(easy.thresholds(), Thresholds::default(), "mostly above 1.0: out of the dynamic range");
    }

    /// No power for a while starts the window again; records out of order are skipped.
    #[test]
    fn dropouts() {
        let mut r = RideFit::new();
        for t in 0..1000u32 {
            r.sample(t, (t % 100 != 0).then_some(200), Some(700));
        }
        assert_eq!(r.n, 0, "never 120 s of power in a row");
        r.sample(10, Some(200), Some(700));
        assert_eq!(r.last, Some(999));
    }

    #[test]
    fn median_and_cap() {
        let mut r = Recent::new();
        assert_eq!(r.vt1(), (None, 0));
        for k in 0..40u16 {
            r.add(Thresholds { vt1: Some(200 + k), vt2: (k % 2 == 0).then_some(260) });
        }
        assert_eq!((r.vt1(), r.vt2(), r.read), ((Some(215), 32), (Some(260), 20), 40));
        assert_eq!(r.vt2_trusted(), Some(260));
    }
}
