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
//! over which it holds. Fitted by least squares on the curve's points from 2 to 20 minutes, as Golden Cheetah's
//! linear regression of work on time over 120 < t <= 1200 s (PDModel.cpp `CP2Model`, checked 2026-10-23).
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
/// The durations the critical power is fitted on: 2 to 20 minutes.
pub const CP_RANGE: core::ops::RangeInclusive<u16> = 120..=1200;

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
    /// Points of the curve the fit used (at most the durations in [`CP_RANGE`], 6).
    pub points: u8,
    /// The largest gap between the model's power and the curve's at those points, percent of the curve's: a few
    /// percent when the bests are true maximal efforts, more when some are not.
    pub err_pct: u8,
}

/// The 2-parameter critical power fit (module doc): None with fewer than 3 points in 2 to 20 minutes, or a fit
/// that makes no sense (CP or W' not positive).
///
/// ```
/// use bryton_rustfw::power_curve::{Curve, DURATIONS, fit_cp};
/// let mut c = Curve::default();
/// for (w, &t) in c.watts.iter_mut().zip(&DURATIONS) {
///     *w = (250 + (20_000 + u32::from(t) / 2) / u32::from(t)) as u16; // CP 250 W, W' 20 kJ, rounded
/// }
/// let cp = fit_cp(&c).unwrap();
/// assert_eq!((cp.cp, (cp.w_prime + 500) / 1000, cp.points), (250, 20, 6));
/// ```
pub fn fit_cp(c: &Curve) -> Option<Cp> {
    let (mut n, mut st, mut sw, mut stt, mut stw) = (0f32, 0f32, 0f32, 0f32, 0f32);
    let points = || DURATIONS.iter().zip(&c.watts).filter(|(t, w)| CP_RANGE.contains(t) && **w > 0);
    for (&t, &w) in points() {
        let (t, work) = (f32::from(t), f32::from(t) * f32::from(w));
        (n, st, sw, stt, stw) = (n + 1.0, st + t, sw + work, stt + t * t, stw + t * work);
    }
    let den = n * stt - st * st;
    if n < 3.0 || den <= 0.0 {
        return None;
    }
    let cp = (n * stw - st * sw) / den;
    let w_prime = (sw - cp * st) / n;
    if !(cp > 0.0 && w_prime > 0.0 && cp < 2000.0 && w_prime < 200_000.0) {
        return None;
    }
    let mut err = 0f32;
    for (&t, &w) in points() {
        let model = cp + w_prime / f32::from(t);
        let e = (model - f32::from(w)) / f32::from(w);
        err = err.max(if e < 0.0 { -e } else { e });
    }
    Some(Cp {
        cp: (cp + 0.5) as u16,
        w_prime: (w_prime + 0.5) as u32,
        points: n as u8,
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

    /// Points off the model: the fit by hand (least squares of work on time) and the error it reports.
    #[test]
    fn cp_fit_by_hand() {
        let mut c = Curve::default();
        // 3, 5 and 12 minutes only: work 300 x 180 = 54000, 280 x 300 = 84000, 250 x 720 = 180000
        for (t, w) in [(180, 300), (300, 280), (720, 250)] {
            let i = DURATIONS.iter().position(|&d| d == t).unwrap();
            c.watts[i] = w;
        }
        // slope = (n Stw - St Sw) / (n Stt - St^2), with t = 180, 300, 720
        let (t, w) = ([180f64, 300.0, 720.0], [54_000f64, 84_000.0, 180_000.0]);
        let (st, sw) = (t.iter().sum::<f64>(), w.iter().sum::<f64>());
        let stt: f64 = t.iter().map(|x| x * x).sum();
        let stw: f64 = t.iter().zip(&w).map(|(x, y)| x * y).sum();
        let cp = (3.0 * stw - st * sw) / (3.0 * stt - st * st);
        let wp = (sw - cp * st) / 3.0;
        let got = fit_cp(&c).unwrap();
        assert_eq!((got.cp, got.w_prime, got.points), (cp.round() as u16, wp.round() as u32, 3));
        assert!(got.err_pct > 0 && got.err_pct < 10, "{got:?}");
        c.watts[5] = 0;
        c.watts[6] = 0;
        assert_eq!(fit_cp(&c), None, "one point left in range");
        assert_eq!(fit_cp(&Curve::default()), None);
    }
}
