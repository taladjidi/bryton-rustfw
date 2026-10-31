//! hrTSS: a training stress score from heart rate, for a ride without power. The Banister TRIMP of each second,
//! summed, over the TRIMP of an hour at the lactate threshold heart rate (LTHR), times 100: an hour at LTHR is 100,
//! as an hour at FTP is for TSS ([`crate::power::tss`]).
//!
//! Sources:
//! - Banister, E. W. (1991), "Modeling elite athletic performance", in Physiological Testing of Elite Athletes:
//!   TRIMP = minutes x HRr x 0.64 x e^(k x HRr), with HRr = (HR - resting) / (max - resting) and k = 1.92 for men,
//!   1.67 for women (Morton et al. 1990 fitted the 0.64 and the k).
//! - hrTSS as TRIMP against an hour at LTHR: a community reconstruction (TrainerRoad forum, "Formula to Calculate
//!   HrTSS", trainerroad.com/forum/t/formula-to-calculate-hrtss/22982). TrainingPeaks does not publish its own
//!   (a table of points per hour by zone), so this is **inferred** to be close to it, not the same.
//!
//! No `exp` in `core`: the 0.64 e^(k x) curve is a table at every 5 % of HRr (x 1000, from python `round(1000 * 0.64 *
//! exp(k * i / 20))`), interpolated in between; a test compares it against the real function.
//!
//! The resting heart rate is a constant ([`REST_HR`]): there is no setting for it.

/// Resting heart rate assumed, bpm (a typical adult's; no setting holds the rider's).
pub const REST_HR: u16 = 60;

/// 0.64 e^(1.92 x) x 1000 for x = 0, 0.05 ... 1 (men).
const MALE: [u32; 21] = [
    640, 704, 775, 854, 940, 1034, 1139, 1253, 1379, 1518, 1671, 1840, 2025, 2229, 2454, 2701, 2973, 3273, 3603, 3966,
    4365,
];
/// 0.64 e^(1.67 x) x 1000 for x = 0, 0.05 ... 1 (women).
const FEMALE: [u32; 21] = [
    640, 696, 756, 822, 894, 972, 1056, 1148, 1248, 1357, 1475, 1604, 1743, 1895, 2060, 2239, 2434, 2646, 2877, 3127,
    3400,
];

/// The TRIMP weight of one minute at `hr` x 1000: HRr x 0.64 e^(k x HRr) (HRr held 0 to 1, as `hr` between resting
/// and `max_hr`) x 1000; None while `max_hr` is not over the resting rate.
fn weight(hr: u16, max_hr: u16, female: bool) -> Option<u64> {
    let span = u32::from(max_hr.checked_sub(REST_HR).filter(|&s| s > 0)?);
    let x = (u32::from(hr.saturating_sub(REST_HR)) * 1000 / span).min(1000); // HRr per mille
    let t = if female { &FEMALE } else { &MALE };
    let (i, frac) = ((x / 50) as usize, x % 50);
    let (a, b) = (t.get(i).copied()?, t.get(i + 1).or(t.get(i)).copied()?);
    Some(u64::from((a + (b - a) * frac / 50) * x / 1000))
}

/// The weights of the moving seconds so far.
///
/// ```
/// use bryton_rustfw::hrtss::Hrtss;
/// let mut h = Hrtss::default();
/// for _ in 0..3600 {
///     h.second(Some(170), 190, false);
/// }
/// assert_eq!(h.score(170, 190, false), Some(100), "an hour at LTHR");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hrtss {
    sum: u64,
}

impl Hrtss {
    /// One moving second at `hr` (None: no reading, nothing counted).
    pub fn second(&mut self, hr: Option<u16>, max_hr: u16, female: bool) {
        self.seconds(hr, 1, max_hr, female);
    }

    /// `n` moving seconds at `hr` (a ride's average, for the scan's rides: [`crate::load`]).
    pub fn seconds(&mut self, hr: Option<u16>, n: u32, max_hr: u16, female: bool) {
        self.sum += hr.and_then(|h| weight(h, max_hr, female)).unwrap_or(0) * u64::from(n);
    }

    /// The score for an LTHR of `lthr`: None while LTHR is unset or `max_hr` is not over the resting rate, or nothing
    /// was counted.
    pub fn score(&self, lthr: u16, max_hr: u16, female: bool) -> Option<u32> {
        let at_lthr = weight(lthr, max_hr, female).filter(|_| lthr > 0)?;
        (self.sum > 0).then(|| (self.sum * 100 / (3600 * at_lthr.max(1))).min(u64::from(u32::MAX)) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables are the real curve, and the interpolation stays within 1 % of it.
    #[test]
    fn weights_follow_banister() {
        for (female, k) in [(false, 1.92f64), (true, 1.67)] {
            for hr in (REST_HR + 10..=190).step_by(7) {
                let x = f64::from(hr - REST_HR) / 130.0;
                let want = 1000.0 * x * 0.64 * (k * x).exp();
                let got = weight(hr, 190, female).unwrap() as f64;
                assert!((got - want).abs() <= want * 0.01 + 1.0, "{hr} bpm, female {female}: {got} against {want}");
            }
        }
    }

    /// A worked example: rest 60, max 190, LTHR 170, 90 minutes at 150 bpm. HRr 0.692 against 0.846 at LTHR:
    /// 0.692 x 0.64 e^(1.92 x 0.692) = 1.67 and 0.846 x 0.64 e^(1.92 x 0.846) = 2.75 per minute, so
    /// 90 x 1.67 / (60 x 2.75) x 100 = 91 for a man (k 1.92); a woman's k (1.67) gives about 95.
    #[test]
    fn worked_example() {
        let (mut m, mut f) = (Hrtss::default(), Hrtss::default());
        for _ in 0..5400 {
            m.second(Some(150), 190, false);
            f.second(Some(150), 190, true);
        }
        let exact = |k: f64| {
            let w = |hr: f64| (hr - 60.0) / 130.0 * 0.64 * (k * (hr - 60.0) / 130.0).exp();
            90.0 * w(150.0) / (60.0 * w(170.0)) * 100.0
        };
        assert!(
            (exact(1.92) - 91.0).abs() < 1.0 && (exact(1.67) - 95.0).abs() < 1.5,
            "{} {}",
            exact(1.92),
            exact(1.67)
        );
        assert!(m.score(170, 190, false).unwrap().abs_diff(exact(1.92) as u32) <= 1);
        assert!(f.score(170, 190, true).unwrap().abs_diff(exact(1.67) as u32) <= 1);
    }

    #[test]
    fn nothing_without_the_inputs() {
        let mut h = Hrtss::default();
        assert_eq!(h.score(170, 190, false), None, "nothing counted");
        h.second(Some(150), 190, false);
        h.second(None, 190, false);
        assert_eq!(h.score(0, 190, false), None, "LTHR unset");
        assert_eq!(h.score(170, 60, false), None, "max not over rest");
        assert!(h.score(170, 190, false).is_some());
    }
}
