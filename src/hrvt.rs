//! The heart rate variability thresholds from ordinary rides: the power at DFA alpha1 = 0.75 (HRVT1, the first
//! threshold, about the aerobic threshold) and at 0.5 (HRVT2, about the second), from the full copies' `Alpha1`
//! and power ([`crate::recorder`], `\Rustfw\Activities`), one ride at a time, then the median of the recent rides.
//! Pure logic, no heap, bounded: a ring of 2 minutes of power and [`BINS`] power bins per ride, [`RIDES`] kept.
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
//! alpha1, `P = m a1 + q` (their equation: power the dependent variable), read at 0.75 and 0.5. Their 5 % artifact
//! gate is applied per point, not per workout: a record whose DFA window had more than 5 % of its beats dropped (the
//! full copy's `Artifact` field) gives no alpha1 (fitread.rs, `MAX_ARTIFACTS`; full copies from before that field:
//! every point). Not here: pedalling under 90 % of the time (outdoors that would drop most rides; the 120 s mean
//! counts coasting).
//!
//! **Which points.** The ramp studies fit alpha1's linear part, about 1.0 to 0.5 (Schaffarczyk et al. 2024); above
//! 1.0 (easy riding) it scatters up to 1.5 or 2 with little change in power. Cutting the points by their own alpha1
//! would keep those whose noise pushed them under 1.0 and drop the others, which biases the slope (11 W on HRVT1 in
//! the tests); so the points go into 10 W power bins ([`BIN_W`]), and the fit takes every point from the first bin
//! whose mean alpha1 is under [`FIT_MAX`]: a cut on power, nearly free of noise.
//!
//! **Not least squares**: alpha1 scatters a lot at a steady power (about 0.15), and least squares of power on a
//! noisy alpha1 shrinks the slope by the share of alpha1's variance that is noise (regression dilution), which
//! pulls both thresholds toward the ride's mean power (more than 15 W on HRVT2 in the tests). Deming regression
//! (Deming 1943; Cornbleet and Gochman 1979, "Incorrect least-squares regression coefficients in method-comparison
//! analysis", Clin Chem 25: 432-438) weighs the errors of both by their variance ratio [`DELTA`]:
//! `m = (Syy - d Sxx + sqrt((Syy - d Sxx)^2 + 4 d Sxy^2)) / (2 Sxy)`, x alpha1, y power, d = [`DELTA`].
//!
//! **This ride's gates** (inferred, tuned on synthetic rides): at least half the points under [`FIT_MAX`]
//! (Andriolo's), at least [`MIN_POINTS`] points in the fit (5 minutes of them) spanning [`MIN_SPAN_W`] of power (a
//! ride all in one zone gives no slope), alpha1 falling as power rises, r^2 at least [`MIN_R2`], and a threshold
//! given only up to [`REACH_PCT`] above the hardest 2 minutes in the fit (no long extrapolation): an easy ride gives
//! HRVT1 at best, rarely HRVT2.
//!
//! **Over rides** ([`Recent`]): the median of the rides of the last [`DAYS`] days that gave one (the [`RIDES`] most
//! recent), trusted from [`MIN_RIDES`] rides.

use crate::power_curve::FILL_S;

/// The window of alpha1 and of the power paired with it, s.
pub const WINDOW_S: usize = 120;
/// A point every this many seconds.
pub const EVERY_S: u32 = 5;
/// The first seconds of a ride left out (the strap settling).
pub const SKIP_S: u32 = 300;
/// Points the fit needs (5 minutes of them).
pub const MIN_POINTS: u32 = 60;
/// alpha1's dynamic range ends here: at least half a ride's points must be under it (Andriolo et al. 2024), and the
/// fit takes the power above the one where it is crossed (module doc).
pub const FIT_MAX: f32 = 1.0;
/// The ratio of the error variances of a point's power and its alpha1, W^2 per unit of alpha1 squared, for the
/// Deming fit (module doc): 10 W for a 2-minute mean against the alpha1 window (inferred) over 0.15 for alpha1
/// (inferred, the scatter of alpha1 at a steady power in Andriolo et al. 2024's and Schaffarczyk et al. 2024's
/// plots).
pub const DELTA: f64 = (10.0 * 10.0) / (0.15 * 0.15);
/// The power range the fit's points must span, W: a ride all at one intensity gives no slope (inferred).
pub const MIN_SPAN_W: u16 = 40;
/// The fit's r^2 at least (inferred).
pub const MIN_R2: f32 = 0.3;
/// A threshold is read only up to this much above the hardest 2 minutes in the fit, percent (no long
/// extrapolation: an easy ride gives HRVT1 at best).
pub const REACH_PCT: u32 = 10;
/// alpha1 at the first and the second threshold.
pub const A1_VT1: f32 = 0.75;
/// See [`A1_VT1`].
pub const A1_VT2: f32 = 0.5;
/// Rides kept at most, the most recent.
pub const RIDES: usize = 32;
/// Rides older than this many days are not read.
pub const DAYS: u16 = 42;
/// Rides giving a threshold before its median is trusted.
pub const MIN_RIDES: u8 = 3;
/// The power bins' width, W, and how many (the last takes everything above).
pub const BIN_W: u16 = 10;
/// See [`BIN_W`]: 0 to 800 W.
pub const BINS: usize = 80;
/// Points a bin needs before its mean alpha1 places the dynamic range's start (30 s of points).
const BIN_MIN: f32 = 6.0;

/// One ride's thresholds, W (None: not from this ride).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Thresholds {
    /// Power at alpha1 = 0.75.
    pub vt1: Option<u16>,
    /// Power at alpha1 = 0.5.
    pub vt2: Option<u16>,
}

/// A power bin's points: count, and the sums of x = alpha1 and d = power less the bin's centre (small, so f32 keeps
/// them exact enough): x, x^2, d, d^2, x d.
#[derive(Clone, Copy, Debug, Default)]
struct Bin {
    n: f32,
    x: f32,
    xx: f32,
    d: f32,
    dd: f32,
    xd: f32,
}

/// The fit of a ride: `P = m a1 + q`, its r^2, and the top of its power range (W).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// Slope, W per unit of alpha1 (negative).
    pub m: f32,
    /// Intercept, W.
    pub q: f32,
    /// Pearson's r^2.
    pub r2: f32,
    /// The top of the highest power bin in the fit, W.
    pub top: u16,
}

/// One ride, a record at a time: the last 2 minutes of power, and the points in power bins.
#[derive(Debug)]
pub struct RideFit {
    ring: [u16; WINDOW_S],
    head: usize,
    len: usize,
    sum: u32,
    first: Option<u32>,
    last: Option<u32>,
    bins: [Bin; BINS],
    /// Points in all, and those with alpha1 under [`FIT_MAX`].
    all: u32,
    low: u32,
}

impl Default for RideFit {
    fn default() -> Self {
        Self::new()
    }
}

impl RideFit {
    /// No record yet.
    pub const fn new() -> Self {
        const B: Bin = Bin { n: 0.0, x: 0.0, xx: 0.0, d: 0.0, dd: 0.0, xd: 0.0 };
        RideFit {
            ring: [0; WINDOW_S],
            head: 0,
            len: 0,
            sum: 0,
            first: None,
            last: None,
            bins: [B; BINS],
            all: 0,
            low: 0,
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
        if let Some(x) = a1
            && self.len == WINDOW_S
            && self.sum > 0
            && t - first >= SKIP_S
            && t.is_multiple_of(EVERY_S)
        {
            let y = self.sum as f32 / WINDOW_S as f32;
            let k = ((y / f32::from(BIN_W)) as usize).min(BINS - 1);
            let d = y - centre(k);
            if let Some(b) = self.bins.get_mut(k) {
                *b = Bin { n: b.n + 1.0, x: b.x + x, xx: b.xx + x * x, d: b.d + d, dd: b.dd + d * d, xd: b.xd + x * d };
            }
            self.all += 1;
            self.low += u32::from(x < FIT_MAX);
        }
    }

    /// The fit (Deming) of the points from the power where alpha1's dynamic range starts, if the ride passes the
    /// gates (module doc).
    pub fn fit(&self) -> Option<Fit> {
        let ([n, sx, sy, sxx, sxy, syy], top) = self.selected()?;
        let (vx, vy, cxy) = (n * sxx - sx * sx, n * syy - sy * sy, n * sxy - sx * sy);
        if !(vx > 0.0 && vy > 0.0 && cxy < 0.0) {
            return None; // no slope, or alpha1 rising with power
        }
        let r2 = cxy * cxy / (vx * vy);
        // Deming regression (module doc): the errors of both, their variance ratio DELTA (power's over alpha1's)
        let dv = vy - DELTA * vx;
        let m = (dv + sqrt(dv * dv + 4.0 * DELTA * cxy * cxy)) / (2.0 * cxy);
        if !(m.is_finite() && r2 >= f64::from(MIN_R2)) {
            return None;
        }
        Some(Fit { m: m as f32, q: ((sy - m * sx) / n) as f32, r2: r2 as f32, top })
    }

    /// The points the fit takes, as sums (n, x, y, x^2, x y, y^2; x alpha1, y power), and the top of their power
    /// range (W), if the ride passes the gates on them.
    fn selected(&self) -> Option<([f64; 6], u16)> {
        if self.low * 2 < self.all {
            return None; // mostly easy riding: alpha1 out of its dynamic range
        }
        // the range starts at the first bin (by power) whose mean alpha1 is under FIT_MAX: a cut on power, which is
        // nearly free of noise, so the points kept are not chosen by their own alpha1's noise
        let cut = self.bins.iter().position(|b| b.n >= BIN_MIN && b.x <= FIT_MAX * b.n)?;
        let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        let mut top = cut;
        for (k, b) in self.bins.iter().enumerate().skip(cut).filter(|(_, b)| b.n > 0.0) {
            let c = f64::from(centre(k));
            let (bn, bx, bd) = (f64::from(b.n), f64::from(b.x), f64::from(b.d));
            (n, sx, sxx) = (n + bn, sx + bx, sxx + f64::from(b.xx));
            sy += bn * c + bd;
            syy += bn * c * c + 2.0 * c * bd + f64::from(b.dd);
            sxy += c * bx + f64::from(b.xd);
            top = k;
        }
        let span = (top - cut + 1) as u32 * u32::from(BIN_W);
        if n < f64::from(MIN_POINTS) || span < u32::from(MIN_SPAN_W) {
            return None;
        }
        let top = ((top + 1) * usize::from(BIN_W)).min(usize::from(u16::MAX)) as u16;
        Some(([n, sx, sy, sxx, sxy, syy], top))
    }

    /// The ride's thresholds.
    ///
    /// ```
    /// use bryton_rustfw::hrvt::RideFit;
    /// // a ride climbing from 190 to 261 W, alpha1 falling as 1.3 - (P - 150) / 200 (0.75 at 260 W, 0.5 at 310)
    /// let mut r = RideFit::new();
    /// for t in 0..3600u32 {
    ///     let p = 190 + t / 50;
    ///     r.sample(t, Some(p as u16), Some((1300 - (p - 150) * 5) as u16));
    /// }
    /// let got = r.thresholds();
    /// assert!(got.vt1.unwrap().abs_diff(260) <= 3, "{got:?}"); // the 2-minute mean lags a little
    /// assert_eq!(got.vt2, None); // over 10 % above the ride's hardest 2 minutes
    /// ```
    pub fn thresholds(&self) -> Thresholds {
        let Some(f) = self.fit() else { return Thresholds::default() };
        let reach = u32::from(f.top) * (100 + REACH_PCT) / 100;
        let at = |a1: f32| {
            let p = f.m * a1 + f.q;
            (p > 0.0 && p < 2000.0 && (p as u32) <= reach).then_some((p + 0.5) as u16)
        };
        Thresholds { vt1: at(A1_VT1), vt2: at(A1_VT2) }
    }
}

/// The centre of power bin `k`, W.
fn centre(k: usize) -> f32 {
    (k as f32 + 0.5) * f32::from(BIN_W)
}

/// The recent rides' thresholds: each ride's, with its day, and their medians.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recent {
    /// (day, HRVT1, HRVT2) of the rides that gave one (0: none), the most recent [`RIDES`].
    rides: [(u16, u16, u16); RIDES],
    n: u8,
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
        Recent { rides: [(0, 0, 0); RIDES], n: 0, read: 0 }
    }

    /// One more ride read, of `day` (days since 1970, in any order: the folder's); past [`RIDES`] giving a value,
    /// it takes the place of the oldest kept if it is more recent.
    pub fn add(&mut self, day: u16, t: Thresholds) {
        self.read = self.read.saturating_add(1);
        if t.vt1.is_none() && t.vt2.is_none() {
            return;
        }
        let e = (day, t.vt1.unwrap_or(0), t.vt2.unwrap_or(0));
        if let Some(slot) = self.rides.get_mut(usize::from(self.n)) {
            *slot = e;
            self.n += 1;
        } else if let Some(old) = self.rides.iter_mut().min_by_key(|r| r.0).filter(|r| r.0 < day) {
            *old = e;
        }
    }

    /// The median of the rides' HRVT1 and how many gave one.
    ///
    /// ```
    /// use bryton_rustfw::hrvt::{Recent, Thresholds};
    /// let mut r = Recent::new();
    /// for w in [210, 190, 260, 205] {
    ///     r.add(20_000, Thresholds { vt1: Some(w), vt2: None });
    /// }
    /// r.add(20_000, Thresholds::default());
    /// assert_eq!((r.vt1(), r.vt2(), r.read), ((Some(207), 4), (None, 0), 5));
    /// ```
    pub fn vt1(&self) -> (Option<u16>, u8) {
        self.median(|r| r.1)
    }

    /// The same for HRVT2.
    pub fn vt2(&self) -> (Option<u16>, u8) {
        self.median(|r| r.2)
    }

    /// HRVT2 when enough rides gave one ([`MIN_RIDES`]).
    pub fn vt2_trusted(&self) -> Option<u16> {
        Some(self.vt2()).filter(|v| v.1 >= MIN_RIDES)?.0
    }

    /// The median of `f` over the rides that have it (the mean of the middle two for an even count), and how many.
    fn median(&self, f: impl Fn(&(u16, u16, u16)) -> u16) -> (Option<u16>, u8) {
        let mut s = [0u16; RIDES];
        let mut k = 0;
        for v in self.rides.iter().take(usize::from(self.n)).map(f).filter(|&v| v > 0) {
            if let Some(slot) = s.get_mut(k) {
                *slot = v;
                k += 1;
            }
        }
        let Some(s) = s.get_mut(..k) else { return (None, 0) };
        s.sort_unstable();
        let mid = |i: usize| s.get(i).copied().map(u32::from);
        let m = match k {
            0 => None,
            _ if k % 2 == 1 => mid(k / 2),
            _ => mid(k / 2 - 1).zip(mid(k / 2)).map(|(a, b)| (a + b) / 2),
        };
        (m.map(|v| v as u16), k as u8)
    }
}

/// The square root (`core` has none): a first guess from the exponent halved, then Newton's steps.
fn sqrt(s: f64) -> f64 {
    if !(s > 0.0 && s.is_finite()) {
        return 0.0;
    }
    let mut x = f64::from_bits((s.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..6 {
        x = 0.5 * (x + s / x);
    }
    x
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;

    /// What a ride gave, for a look at a real one (storage/thresholds.rs `real_rides`): the points, those under
    /// [`FIT_MAX`], and each power bin's count and mean alpha1.
    pub(crate) fn describe(r: &RideFit) -> std::string::String {
        let mut s = std::format!("{} points, {} under {FIT_MAX}; bins W:n:a1", r.all, r.low);
        for (k, b) in r.bins.iter().enumerate().filter(|(_, b)| b.n > 0.0) {
            s += &std::format!(" {}:{}:{:.2}", k * usize::from(BIN_W), b.n, b.x / b.n);
        }
        s
    }

    /// A ride of `secs` s: power `p(t)`, alpha1 from it by the rider's line (0.75 at `vt1`, 0.5 at `vt2`), with
    /// noise of standard deviation `noise` on alpha1 (about normal: the sum of four uniform draws).
    pub(crate) fn ride(secs: u32, p: impl Fn(u32) -> u16, vt1: f32, vt2: f32, noise: f32) -> RideFit {
        let mut r = RideFit::new();
        let slope = -0.25 / (vt2 - vt1); // alpha1 per W
        let mut x = 7u32;
        let mut uniform = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x % 2001) as f32 / 1000.0 - 1.0 // standard deviation 1 / sqrt(3)
        };
        for t in 0..secs {
            let jitter = (uniform() + uniform() + uniform() + uniform()) / 1.1547;
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

    /// alpha1 as noisy as on the road (standard deviation 0.15): the Deming fit keeps both thresholds within a few
    /// watts, where least squares of power on alpha1 pulls them toward the ride's mean power (regression dilution).
    #[test]
    fn realistic_noise() {
        let r = ride(4800, |t| (140 + t / 600 * 20) as u16, 210.0, 265.0, 0.15);
        let got = r.thresholds();
        assert!(got.vt1.unwrap().abs_diff(210) <= 5 && got.vt2.unwrap().abs_diff(265) <= 5, "{got:?}");
        // least squares on the same sums
        let ([n, sx, sy, sxx, sxy, _], _) = r.selected().unwrap();
        let m = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        let ols = |a1: f64| (sy - m * sx) / n + m * a1;
        assert!(ols(0.5) < 265.0 - 15.0, "least squares: HRVT2 {} W", ols(0.5));
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
        assert_eq!(r.all, 0, "never 120 s of power in a row");
        r.sample(10, Some(200), Some(700));
        assert_eq!(r.last, Some(999));
    }

    #[test]
    fn median_and_cap() {
        let mut r = Recent::new();
        assert_eq!(r.vt1(), (None, 0));
        // 40 rides read in no order of days: the 32 most recent kept (days 8 to 39, HRVT1 208 to 239)
        for k in (0..40u16).rev().step_by(2).chain((0..40).step_by(2)) {
            r.add(k, Thresholds { vt1: Some(200 + k), vt2: (k % 2 == 0).then_some(260) });
        }
        assert_eq!((r.vt1(), r.vt2(), r.read), ((Some(223), 32), (Some(260), 16), 40));
        assert_eq!(r.vt2_trusted(), Some(260));
        r.add(1, Thresholds { vt1: Some(100), vt2: None });
        assert_eq!(r.vt1(), (Some(223), 32), "older than all kept: left out");
    }
}
