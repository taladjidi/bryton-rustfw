//! The power curve (mean-maximal power: the best average power held for each of a set of durations), merged
//! across rides as bests with the day of the ride that set them, and the critical power and W' fitted to it.
//!
//! **One pass per ride** ([`Rolling`]): a ring of the last 20 minutes of power, one second a slot, and a running
//! sum per duration; each second adds the new watts and takes off the ones that left the window, so a ride costs
//! 13 additions per second whatever the durations. The 30 and 60 minute bests run on a second ring of 10-second
//! averages (3 KB for both rings instead of 7 KB for an hour of seconds; the app keeps the scan in its state, which
//! lives on the stack): their windows start on a 10 s boundary, which can cost them up to 10 s of the effort,
//! under 1 % of the best.
//!
//! **Gaps.** A window never spans a pause: when the records stop for more than [`FILL_S`] seconds (the timer
//! stopped, auto-pause, a reset) or a record has no power (the meter dropped out), the windows start again. A gap
//! up to [`FILL_S`] seconds (smart recording, a missed second) is filled with the next sample. A meter that
//! sends 0 W (coasting) counts as 0 W: that is a real second of riding. Golden Cheetah does otherwise: its mean
//! maximal computation fills every gap in the recording, up to an hour, with 0 W (RideFileCache.cpp
//! `MeanMaxComputer::run`, github.com/GoldenCheetah/GoldenCheetah, checked 2026-10-23), which lowers long bests
//! taken across a stop; here a stop ends the effort.
//!
//! **Critical power** ([`fit_cp`]): the 2-parameter model of Monod and Scherrer (1965, "The work capacity of a
//! synergic muscular group", Ergonomics 8: 329-338): the work done to exhaustion is linear in the time,
//! `work = CP x t + W'`; Jones et al. (2010, "Critical power: implications for determination of VO2max and
//! exercise tolerance", Med Sci Sports Exerc 42: 1876-1890) on its meaning and the 2 to 15 or 20 minute range
//! over which it holds. **Fitted as an envelope**, as Golden Cheetah's default fit (below): the model through
//! one best of 2 to 5 minutes ([`SHORT_S`], it sets W') and one of 12 to 30 ([`LONG_S`], it sets CP) that no best of
//! either window rises above. A best is a lower bound of what the rider can do (a 3-minute best is often ridden
//! inside a longer effort, never all-out), so the model must not pass under the best ones, and the sub-maximal ones
//! must not pull it down. In work against time it is the edge of the upper hull of the bests that joins the two
//! windows: at most 3 x 3 candidate lines, each checked against 6 points, no iteration. The fit's quality: how many
//! bests of 2 to 30 minutes sit within [`SUBMAX_PCT`] of the model (those it rests on; one further under it was not
//! a maximal effort) and how far the model is from them (or under one between the windows).
//!
//! What the reference tools do (checked 2026-10-26):
//! - **Golden Cheetah** fits by default the "envelope" (PDModel.cpp, `fit(Envelope)`): CP the largest
//!   `P(t) / (1 + tau / t)` over an aerobic window (720 to 1200 s for the 2-parameter model), tau the largest
//!   `(P(t) / CP - 1) t` over an anaerobic one (120 to 200 s), alternated to convergence: the model lies on or above
//!   every best, so sub-maximal ones never pull it down, but two points decide it. Its other fits: least squares
//!   (as above, 2 to 20 min), and the 3-parameter, extended and Ward-Smith models.
//! - **Intervals.icu** (forum.intervals.icu/t/model-power-curve/1007 and /t/how-accurate-is-eftp/1639, its
//!   developer's posts; the API's `PowerModel`, intervals.icu/api/v1/docs: types `MS_2P`, `MORTON_3P`,
//!   `FFT_CURVES`, `ECP`, each with the indexes of the curve points it used): the 2-parameter Monod and Scherrer
//!   model through two maximal efforts, one of 3 to 5 minutes and one of 10 to 30; Morton's 3-parameter model
//!   (Morton 1996, "A 3-parameter critical power model", Ergonomics 39: 611-619) from efforts of 5 s, 3 to 5 min
//!   and 10 to 30 min; and its default eFTP, one maximal effort of 3 to 30 minutes matched to curves from
//!   FastFitness.Tips data shaped by Morton's model, its W' "the point on the actual power curve that has the most
//!   energy above FTP". The window is the chosen curve's (42 days, 90 days, a season). How the 2-parameter model
//!   picks its two points is not published.
//! - **WKO** (TrainingPeaks, "scientific basis of the new power duration model in WKO4", Coggan): its own
//!   power-duration model (Pmax, FRC, mFTP) fitted to the whole mean-maximal curve; not published.
//!
//! Why W' differs between them: with CP nearly fixed by the long bests, W' is the work above CP at the short end,
//! so it follows whichever short best a method trusts. A 2 or 3-minute best barely above the 5-minute one (never
//! ridden all-out) pulls a least-squares fit's W' down by several kJ (this firmware's first fit, least squares over
//! 2 to 20 minutes, did); a fit through a 4 or 5 minute best and a 15 to 30 minute one (Intervals.icu's 2-parameter
//! model, Golden Cheetah's envelope, the fit here) does not see it.
//!
//! **The estimate's FTP** ([`Cp::ftp`]): CP itself. Intervals.icu's API gives each model's `ftp` equal to its
//! `criticalPower` for the 2-parameter, Morton and extended models (only its own eFTP curves differ); Allen and
//! Coggan's 95 % of the 20-minute best is the field test's rule, not a model of the curve.
//!
//! In f32, no libm.

/// The durations of the curve, seconds.
pub const DURATIONS: [u16; 13] = [1, 5, 15, 30, 60, 120, 180, 300, 480, 720, 1200, 1800, 3600];
/// How many.
pub const N: usize = DURATIONS.len();
/// Durations up to 20 minutes go on a ring of seconds; the longer ones on a ring of 10-second averages.
const FINE: usize = 1200;
const SPLIT: usize = 11;
const BLOCK: u32 = 10;
/// The long durations in blocks.
const COARSE_LENS: [u16; N - SPLIT] = [180, 360];
const COARSE: usize = 360;
const _: () =
    assert!(DURATIONS[SPLIT - 1] as usize == FINE && DURATIONS[SPLIT] as u32 == COARSE_LENS[0] as u32 * BLOCK);
const _: () = assert!(DURATIONS[N - 1] as u32 == COARSE_LENS[1] as u32 * BLOCK && COARSE_LENS[1] as usize == COARSE);
/// The longest gap between records filled (with the next sample) rather than ending the efforts.
pub const FILL_S: u32 = 3;
/// The durations the critical power is fitted on: 2 to 30 minutes (Intervals.icu's longest effort for its
/// 2-parameter model; Golden Cheetah's least squares stops at 20).
pub const CP_RANGE: core::ops::RangeInclusive<u16> = 120..=1800;
/// A best more than this far below the model, percent of the best, was not a maximal effort: not counted in the
/// fit's quality (inferred: maximal bests sit within 2 to 3 % of a 2-parameter model from 3 to 30 minutes).
pub const SUBMAX_PCT: f32 = 3.0;
/// The short window (2 minutes to this, s) sets W', the long one ([`LONG_S`] to 30 minutes) CP: Intervals.icu's
/// "3 to 5 minutes" and "10 to 30 minutes" efforts, Golden Cheetah's anaerobic and aerobic windows (120 to 200 s,
/// 720 to 1200 s), widened to the curve's durations.
pub const SHORT_S: u16 = 300;
/// See [`SHORT_S`].
pub const LONG_S: u16 = 720;
/// The fit is trusted (it then sets the ride's CP and W': app/pace.rs) when it rests on at least this many bests
/// (the two that set it and one more that agrees)...
pub const TRUST_POINTS: u8 = 3;
/// ...and the model within this many percent of each of them.
pub const TRUST_ERR_PCT: u8 = 5;

/// The last `L` values, and how many since the last gap.
struct Ring<const L: usize> {
    v: [u16; L],
    head: usize,
    len: usize,
}

impl<const L: usize> Ring<L> {
    const fn new() -> Self {
        Ring { v: [0; L], head: 0, len: 0 }
    }

    /// Adds `x`: each window (`lens`, in values) gets it in its running sum and loses the value that left it, and
    /// keeps its best average.
    fn push(&mut self, x: u16, lens: &[u16], sums: &mut [u32], best: &mut [u16]) {
        for ((sum, best), &d) in sums.iter_mut().zip(best).zip(lens) {
            let d = usize::from(d);
            *sum += u32::from(x);
            if self.len >= d {
                *sum -= u32::from(self.v.get((self.head + L - d) % L).copied().unwrap_or(0)); // d values back
            }
            if self.len + 1 >= d {
                *best = (*best).max((*sum / d as u32) as u16);
            }
        }
        if let Some(slot) = self.v.get_mut(self.head) {
            *slot = x;
        }
        self.head = (self.head + 1) % L;
        self.len = (self.len + 1).min(L);
    }
}

/// One ride's best powers, computed as the samples come.
pub struct Rolling {
    fine: Ring<FINE>,
    coarse: Ring<COARSE>,
    sums: [u32; N],
    /// The 10-second block being summed, and its seconds.
    acc: u32,
    in_block: u32,
    last: Option<u32>,
    /// The ride's best average power per duration ([`DURATIONS`]), W; 0: not held that long.
    pub best: [u16; N],
}

impl Default for Rolling {
    fn default() -> Self {
        Self::new()
    }
}

impl Rolling {
    /// No ride yet.
    pub const fn new() -> Self {
        Rolling { fine: Ring::new(), coarse: Ring::new(), sums: [0; N], acc: 0, in_block: 0, last: None, best: [0; N] }
    }

    /// A new ride: its bests and its windows cleared.
    pub fn reset(&mut self) {
        self.restart();
        (self.last, self.best) = (None, [0; N]);
    }

    /// The efforts start again (a gap, a dropout).
    fn restart(&mut self) {
        (self.fine.len, self.coarse.len, self.sums, self.acc, self.in_block) = (0, 0, [0; N], 0, 0);
    }

    /// The record at FIT time `t`, with its power if it has one (records out of order are skipped).
    ///
    /// ```
    /// use bryton_rustfw::power_curve::Rolling;
    /// let mut r = Rolling::new();
    /// for t in 0..10 {
    ///     r.sample(t, Some(if t < 5 { 300 } else { 100 }));
    /// }
    /// assert_eq!(&r.best[..3], &[300, 300, 0]); // 1 s, 5 s; never 15 s
    /// ```
    pub fn sample(&mut self, t: u32, power: Option<u16>) {
        let gap = match self.last {
            Some(last) if t <= last => return,
            Some(last) => t - last,
            None => u32::MAX,
        };
        self.last = Some(t);
        let Some(w) = power else {
            return self.restart(); // a dropout ends the efforts
        };
        let fill = if gap > FILL_S {
            self.restart();
            1
        } else {
            gap
        };
        for _ in 0..fill {
            self.push(w);
        }
    }

    fn push(&mut self, w: u16) {
        let (sums, long_sums) = self.sums.split_at_mut(SPLIT);
        let (best, long_best) = self.best.split_at_mut(SPLIT);
        self.fine.push(w, &DURATIONS[..SPLIT], sums, best);
        self.acc += u32::from(w);
        self.in_block += 1;
        if self.in_block == BLOCK {
            self.coarse.push((self.acc / BLOCK) as u16, &COARSE_LENS, long_sums, long_best);
            (self.acc, self.in_block) = (0, 0);
        }
    }
}

/// The best power per duration over several rides, and the day each was set (days since 1970).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Curve {
    /// W per [`DURATIONS`]; 0: none.
    pub watts: [u16; N],
    /// The day of the ride that set it.
    pub day: [u16; N],
}

impl Curve {
    /// Keeps a ride's bests where they beat the curve.
    pub fn merge(&mut self, best: &[u16; N], day: u16) {
        for ((w, d), &b) in self.watts.iter_mut().zip(&mut self.day).zip(best) {
            if b > *w {
                (*w, *d) = (b, day);
            }
        }
    }
}

/// Critical power and W', with how much to trust them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cp {
    /// Critical power, W.
    pub cp: u16,
    /// W', J.
    pub w_prime: u32,
    /// Bests of 2 to 30 minutes within [`SUBMAX_PCT`] of the model: those it rests on (the two that set it among
    /// them).
    pub points: u8,
    /// Bests of 2 to 30 minutes ([`CP_RANGE`]), at most 7.
    pub of: u8,
    /// The largest gap between the model and those bests, or a best over the model (between the windows), percent
    /// of the best.
    pub err_pct: u8,
}

impl Cp {
    /// Enough bests on the model, close enough to it ([`TRUST_POINTS`], [`TRUST_ERR_PCT`]); the curve is the last
    /// 90 days' ([`crate::stock::WINDOW_DAYS`]), so it is recent by construction.
    pub fn trusted(&self) -> bool {
        self.points >= TRUST_POINTS && self.err_pct <= TRUST_ERR_PCT
    }

    /// The FTP this estimate gives: CP (module doc).
    pub fn ftp(&self) -> u16 {
        self.cp
    }
}

/// The 2-parameter critical power fit as an envelope (module doc): None without a best of 2 to 5 minutes and one
/// of 12 to 30, or for a fit that makes no sense (CP or W' not positive).
///
/// ```
/// use bryton_rustfw::power_curve::{Curve, DURATIONS, fit_cp};
/// let mut c = Curve::default();
/// for (w, &t) in c.watts.iter_mut().zip(&DURATIONS) {
///     *w = (250 + (20_000 + u32::from(t) / 2) / u32::from(t)) as u16; // CP 250 W, W' 20 kJ, rounded
/// }
/// let cp = fit_cp(&c).unwrap();
/// assert_eq!((cp.cp, (cp.w_prime + 500) / 1000, cp.points), (250, 20, 7));
/// c.watts[6] = 300; // a 3-minute best well under the 361 W the model gives: not ridden all-out
/// let cp = fit_cp(&c).unwrap();
/// assert_eq!((cp.cp, (cp.w_prime + 500) / 1000, cp.points, cp.of), (250, 20, 6, 7));
/// assert!(cp.trusted());
/// ```
pub fn fit_cp(c: &Curve) -> Option<Cp> {
    // (t, work) of each best of 2 to 30 minutes, and whether it is in the short or the long window
    let best = |i: usize| {
        let (t, w) = (*DURATIONS.get(i)?, *c.watts.get(i)?);
        (CP_RANGE.contains(&t) && w > 0).then(|| (f32::from(t), f32::from(t) * f32::from(w)))
    };
    let window = |i: usize| DURATIONS.get(i).is_some_and(|&t| t <= SHORT_S || t >= LONG_S);
    let short = |i: usize| DURATIONS.get(i).is_some_and(|&t| t <= SHORT_S);
    // the line through a short best and a long one that every best of both windows is under (half a watt allowed:
    // the bests are rounded); the upper hull has one such edge, so the first found is it
    let mut fit = None;
    'pairs: for s in (0..N).filter(|&i| short(i)) {
        for l in (0..N).filter(|&i| !short(i) && window(i)) {
            let (Some((ts, ws)), Some((tl, wl))) = (best(s), best(l)) else { continue };
            let cp = (wl - ws) / (tl - ts);
            let w_prime = ws - cp * ts;
            let under = |i: usize| best(i).is_none_or(|(t, w)| !window(i) || w <= cp * t + w_prime + 0.5 * t);
            if (0..N).all(under) {
                fit = Some((cp, w_prime));
                break 'pairs;
            }
        }
    }
    let (cp, w_prime) = fit?;
    if !(cp >= 1.0 && w_prime >= 1.0 && cp < 2000.0 && w_prime < 200_000.0) {
        return None;
    }
    let (mut of, mut points, mut err) = (0u8, 0u8, 0f32);
    for (t, work) in (0..N).filter_map(best) {
        let (w, model) = (work / t, cp + w_prime / t);
        let e = (model - w) / w; // under the model: positive
        of += 1;
        if e * 100.0 <= SUBMAX_PCT {
            points += 1;
            err = err.max(if e < 0.0 { -e } else { e });
        }
    }
    Some(Cp {
        cp: (cp + 0.5) as u16,
        w_prime: (w_prime + 0.5) as u32,
        points,
        of,
        err_pct: (err * 100.0 + 0.5).min(255.0) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against sums done by hand: 10 s at 400 W in a ride at 200 W.
    #[test]
    fn bests_by_hand() {
        let mut r = Rolling::new();
        for t in 0..4000u32 {
            r.sample(t, Some(if (100..110).contains(&t) { 400 } else { 200 }));
        }
        assert_eq!(r.best[0], 400); // 1 s
        assert_eq!(r.best[1], 400); // 5 s
        assert_eq!(r.best[2], (10 * 400 + 5 * 200) / 15); // 15 s: the 10 s and 5 s more
        assert_eq!(r.best[3], (10 * 400 + 20 * 200) / 30);
        assert_eq!(r.best[12], ((10 * 400 + 3590 * 200) / 3600) as u16); // an hour
    }

    /// A pause ends the efforts; a dropout too; a short gap is filled; samples out of order are skipped.
    #[test]
    fn gaps() {
        let mut r = Rolling::new();
        for t in 0..20 {
            r.sample(t, Some(300));
        }
        for t in 600..620 {
            r.sample(t, Some(300)); // after a 10-minute stop
        }
        assert_eq!((r.best[2], r.best[3]), (300, 0), "15 s held, 30 s not across the stop");
        r.reset();
        for t in 0..40 {
            r.sample(t, (t != 20).then_some(300));
        }
        assert_eq!((r.best[2], r.best[3]), (300, 0), "a dropout");
        r.reset();
        for t in (0..60).step_by(2) {
            r.sample(t, Some(300));
        }
        r.sample(10, Some(1000));
        assert_eq!((r.best[3], r.best[0]), (300, 300), "one second in two, filled; the late sample skipped");
    }

    #[test]
    fn merge_keeps_the_best_and_its_day() {
        let mut c = Curve::default();
        let mut a = [100; N];
        c.merge(&a, 10);
        a[0] = 900;
        a[5] = 50;
        c.merge(&a, 20);
        assert_eq!((c.watts[0], c.day[0], c.watts[5], c.day[5]), (900, 20, 100, 10));
    }

    /// The envelope by hand: of the lines through a short best and a long one, the one no best rises above.
    #[test]
    fn cp_fit_by_hand() {
        let mut c = Curve::default();
        // 3, 5 and 12 minutes only: work 300 x 180 = 54000, 280 x 300 = 84000, 250 x 720 = 180000
        for (t, w) in [(180, 300), (300, 280), (720, 250)] {
            let i = DURATIONS.iter().position(|&d| d == t).unwrap();
            c.watts[i] = w;
        }
        // through 3 and 12 minutes: CP 126000 / 540 = 233.3, W' 12000, under the 5 minutes (82000 < 84000);
        // through 5 and 12: CP 96000 / 420 = 228.6, W' 84000 - 68571 = 15429, over the 3 minutes (56571 > 54000)
        let got = fit_cp(&c).unwrap();
        assert_eq!((got.cp, got.w_prime), (229, 15_429));
        // the 3 minutes 4.8 % under the model: not counted
        assert_eq!((got.points, got.of, got.err_pct, got.trusted()), (2, 3, 0, false));
        c.watts[5] = 0;
        c.watts[6] = 0;
        c.watts[7] = 0;
        assert_eq!(fit_cp(&c), None, "no short best");
        assert_eq!(fit_cp(&Curve::default()), None);
    }

    /// The curve of `f(t)` W at each duration.
    fn curve(f: impl Fn(f32) -> f32) -> Curve {
        let mut c = Curve::default();
        for (w, &t) in c.watts.iter_mut().zip(&DURATIONS) {
            *w = (f(f32::from(t)) + 0.5) as u16;
        }
        c
    }

    /// Least squares of work on time over the bests of 2 to 30 minutes (the first fit here, for comparison).
    fn least_squares(c: &Curve) -> (f32, f32) {
        let (mut n, mut st, mut sw, mut stt, mut stw) = (0f32, 0f32, 0f32, 0f32, 0f32);
        for (&t, &w) in DURATIONS.iter().zip(&c.watts).filter(|(t, w)| CP_RANGE.contains(t) && **w > 0) {
            let (t, work) = (f32::from(t), f32::from(t) * f32::from(w));
            (n, st, sw, stt, stw) = (n + 1.0, st + t, sw + work, stt + t * t, stw + t * work);
        }
        let cp = (n * stw - st * sw) / (n * stt - st * st);
        (cp, (sw - cp * st) / n)
    }

    /// A clean 3-parameter curve (Morton 1996: P = CP + W' / (t - k), k = W' / (CP - Pmax)): the 2-parameter fit
    /// of its 2 to 30 minutes stays close, its CP a little higher and its W' a little lower, as the two models
    /// differ.
    #[test]
    fn morton_curve() {
        let (cp, wp, pmax) = (250.0, 20_000.0, 1000.0);
        let k = wp / (cp - pmax);
        let got = fit_cp(&curve(|t| cp + wp / (t - k))).unwrap();
        assert!((250..=255).contains(&got.cp) && (16_000..=20_000).contains(&got.w_prime), "{got:?}");
        assert!(got.trusted(), "{got:?}");
    }

    /// Sub-maximal bests (2, 3 and 8 minutes ridden under the rider's best) leave the fit as it was, where a plain
    /// least squares loses W' to them.
    #[test]
    fn submaximal_bests_left_out() {
        let mut c = curve(|t| 260.0 + 18_000.0 / t);
        let plain = least_squares(&c);
        for (i, f) in [(5, 0.92), (6, 0.92), (8, 0.95)] {
            c.watts[i] = (f32::from(c.watts[i]) * f) as u16;
        }
        let got = fit_cp(&c).unwrap();
        assert_eq!((got.cp, (got.w_prime + 500) / 1000, got.points, got.of), (260, 18, 4, 7), "{got:?}");
        assert!(got.trusted());
        let ls = least_squares(&c);
        assert!(ls.1 < plain.1 - 2000.0, "{ls:?} {plain:?}");
    }

    /// No short or no long best: no fit. Two bests: a fit, not trusted. A short best under CP: none.
    #[test]
    fn too_few_points() {
        let clean = curve(|t| 260.0 + 18_000.0 / t);
        let only = |keep: &[usize]| {
            let mut c = Curve::default();
            keep.iter().for_each(|&i| c.watts[i] = clean.watts[i]);
            fit_cp(&c)
        };
        assert_eq!(only(&[7, 10]).map(|c| (c.cp, c.points, c.trusted())), Some((260, 2, false)), "two points");
        assert_eq!(only(&[5, 6, 7, 8]), None, "2 to 8 minutes: nothing sets CP");
        assert_eq!(only(&[9, 10, 11]), None, "12 to 30 minutes: nothing sets W'");
        assert_eq!(only(&[6, 9, 11]).map(|c| (c.cp, c.points, c.trusted())), Some((260, 3, true)), "3 points");
        let mut c = clean;
        (c.watts[5], c.watts[6], c.watts[7]) = (0, 0, 250); // the only short best, under the long ones
        assert_eq!(fit_cp(&c), None);
    }
}
