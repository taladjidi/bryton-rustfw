//! Heart rate variability from the strap's RR intervals ([`crate::sensors::Beats`]), one beat at a time: RMSSD
//! and DFA alpha1. Pure logic, no heap, f32 and f64 without libm (`tiles::ln`).
//!
//! - **DFA alpha1** is the patch project's P9, ported (bryton-S510-mods `patches/P9-live-dfa-a1.c`, and its check
//!   `tests/test_tools.py` `test_dfa_a1_matches_reference`): the beats of the last 2 minutes, newest first,
//!   artifacts dropped with the 5 % rule (a beat more than 5 % from the previous kept one; after 3 drops in a row
//!   the reference was the odd one, so start again from the current beat), at least 64 kept; then detrended
//!   fluctuation analysis (Peng et al. 1995, Chaos 5:82): the integrated series cut into boxes of 4 to 16 beats, a
//!   least-squares line in each, F(n) the RMS of the residuals; alpha1 is the slope of log F(n) against log n. This
//!   is the short-term exponent of Rogers et al. 2021 (Front. Physiol. 11:596567, about 0.75 at the aerobic
//!   threshold), with the 5 % artifact rule of their FatMaxxer / AlphaHRV apps as P9 has it.
//! - **RMSSD** (Task Force of the ESC and NASPE 1996, Circulation 93:1043): the root mean square of successive
//!   differences over the last 60 s of beats. A pair is left out when its beats differ by more than 20 % of the
//!   first (Malik et al. 1989's rule, the usual correction for ectopic beats), so a missed or extra beat drops
//!   its two differences instead of inflating the value.
//!
//! Both are recomputed every [`EVERY`] beats (about every 3 to 5 s while riding): the cost is bounded by the
//! window, about 4 n multiply-adds per box size for n beats.

use crate::tiles::ln;

/// Beats kept: 2 minutes at 240 bpm is 480.
pub const BEATS: usize = 512;
/// DFA alpha1's window, ms.
const DFA_MS: u32 = 120_000;
/// RMSSD's window, ms.
const RMSSD_MS: u32 = 60_000;
/// Clean beats DFA alpha1 needs (as P9), and differences RMSSD needs.
const MIN_BEATS: usize = 64;
const MIN_PAIRS: u32 = 30;
/// The values are recomputed every this many beats.
pub const EVERY: u8 = 5;

/// The last [`BEATS`] RR intervals and the values from them.
pub struct Hrv {
    rr: [u16; BEATS],
    /// The next slot, and how many are filled.
    head: usize,
    len: usize,
    /// Beats since the last computation.
    since: u8,
    alpha1: Option<f32>,
    rmssd: Option<u16>,
}

impl Default for Hrv {
    fn default() -> Self {
        Hrv { rr: [0; BEATS], head: 0, len: 0, since: 0, alpha1: None, rmssd: None }
    }
}

impl Hrv {
    /// Takes one RR interval, ms.
    pub fn push(&mut self, ms: u16) {
        if let Some(s) = self.rr.get_mut(self.head) {
            *s = ms;
        }
        self.head = (self.head + 1) % BEATS;
        self.len = (self.len + 1).min(BEATS);
        self.since += 1;
        if self.since >= EVERY {
            self.since = 0;
            self.alpha1 = self.dfa_now();
            self.rmssd = self.rmssd_now();
        }
    }

    /// DFA alpha1, once there are enough clean beats.
    pub fn alpha1(&self) -> Option<f32> {
        self.alpha1
    }

    /// RMSSD, ms, once there are enough clean pairs.
    pub fn rmssd(&self) -> Option<u16> {
        self.rmssd
    }

    /// The intervals, newest first.
    fn newest(&self) -> impl Iterator<Item = u16> + '_ {
        (1..=self.len).filter_map(|i| self.rr.get((self.head + BEATS - i) % BEATS).copied())
    }

    fn rmssd_now(&self) -> Option<u16> {
        let (mut sum, mut pairs, mut total, mut newer) = (0u64, 0u32, 0u32, None::<u16>);
        for rr in self.newest() {
            total += u32::from(rr);
            if total > RMSSD_MS {
                break;
            }
            if let Some(n) = newer.replace(rr) {
                let d = u32::from(n.abs_diff(rr));
                if d * 5 <= u32::from(rr) {
                    sum += u64::from(d * d);
                    pairs += 1;
                }
            }
        }
        (pairs >= MIN_PAIRS).then(|| (sum / u64::from(pairs)).isqrt() as u16)
    }

    fn dfa_now(&self) -> Option<f32> {
        let mut x = [0f32; BEATS];
        let (mut n, mut total, mut drops, mut kept) = (0, 0u32, 0, 0f32);
        for rr in self.newest() {
            if total >= DFA_MS {
                break;
            }
            total += u32::from(rr);
            let v = f32::from(rr);
            if kept == 0.0 || drops == 3 || (v > kept * 0.95 && v < kept * 1.05) {
                if let Some(s) = x.get_mut(n) {
                    *s = v;
                    n += 1;
                }
                kept = v;
                drops = 0;
            } else {
                drops += 1;
            }
        }
        // a1 of 0 or less is no physiology (a stretch of equal intervals, a strap's glitch: two such records seen on
        // the owner's ride, 2026-10-25), and would read as an all-out effort: no value
        dfa_a1(x.get_mut(..n)?).filter(|a| *a > 0.01 && a.is_finite())
    }
}

/// DFA alpha1 of the intervals `x` (overwritten by the integrated series), box sizes 4 to 16; None under 64.
///
/// ```
/// // a strict alternation is anti-correlated: far under the 0.5 of white noise
/// let mut x: Vec<f32> = (0..100).map(|i| if i % 2 == 0 { 800.0 } else { 820.0 }).collect();
/// assert!(bryton_rustfw::hrv::dfa_a1(&mut x).unwrap() < 0.2);
/// ```
pub fn dfa_a1(x: &mut [f32]) -> Option<f32> {
    let n = x.len();
    if n < MIN_BEATS {
        return None;
    }
    let mean = x.iter().sum::<f32>() / n as f32;
    let mut y = 0.0;
    for v in x.iter_mut() {
        y += *v - mean;
        *v = y;
    }
    // least squares of log F against log s, over the 13 box sizes
    let (mut sl, mut sf, mut sll, mut slf) = (0.0, 0.0, 0.0, 0.0);
    for s in 4..=16 {
        let sz = s as f32;
        let (sk, skk) = (sz * (sz - 1.0) / 2.0, (sz - 1.0) * sz * (2.0 * sz - 1.0) / 6.0);
        let d = sz * skk - sk * sk;
        let mut f2 = 0.0;
        for b in x.chunks_exact(s) {
            let (mut sy, mut sky) = (0.0, 0.0);
            for (k, &v) in b.iter().enumerate() {
                sy += v;
                sky += k as f32 * v;
            }
            let slope = (sz * sky - sk * sy) / d;
            let icpt = (sy - slope * sk) / sz;
            // residuals in a second pass, as P9: keeps the precision in f32
            for (k, &v) in b.iter().enumerate() {
                let r = v - icpt - slope * k as f32;
                f2 += r * r;
            }
        }
        let l = ln(f64::from(sz));
        // log of the RMS: half the log of the mean square (no sqrt needed)
        let f = 0.5 * ln(f64::from(f2 / ((n / s * s) as f32)).max(1e-30));
        sl += l;
        sf += f;
        sll += l * l;
        slf += l * f;
    }
    let a = ((13.0 * slf - sl * sf) / (13.0 * sll - sl * sl)) as f32;
    a.is_finite().then_some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// xorshift32, then the sum of 4 uniforms: near-Gaussian noise of mean 0 and standard deviation `sd`.
    pub(crate) fn noise(seed: u32, sd: f32) -> impl FnMut() -> f32 {
        let mut x = seed;
        move || {
            let mut u = 0.0;
            for _ in 0..4 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                u += x as f32 / u32::MAX as f32;
            }
            (u - 2.0) * sd * 3f32.sqrt()
        }
    }

    /// The plain DFA of P9's Python check (`test_dfa_a1_matches_reference`'s `ref`), in f64 with std's log.
    fn reference(x: &[f32]) -> f64 {
        let m = x.iter().map(|&v| f64::from(v)).sum::<f64>() / x.len() as f64;
        let y: Vec<f64> = x.iter().scan(0.0, |s, &v| Some(*s + f64::from(v) - m).inspect(|&t| *s = t)).collect();
        let pts: Vec<(f64, f64)> = (4..=16usize)
            .map(|b| {
                let (mk, mut f2) = ((b as f64 - 1.0) / 2.0, 0.0);
                for seg in y.chunks_exact(b) {
                    let my = seg.iter().sum::<f64>() / b as f64;
                    let sxx: f64 = (0..b).map(|a| (a as f64 - mk).powi(2)).sum();
                    let sl = seg.iter().enumerate().map(|(a, c)| (a as f64 - mk) * (c - my)).sum::<f64>() / sxx;
                    f2 += seg.iter().enumerate().map(|(a, c)| (c - my - sl * (a as f64 - mk)).powi(2)).sum::<f64>();
                }
                ((b as f64).ln(), (f2 / (y.len() / b * b) as f64).sqrt().ln())
            })
            .collect();
        let (ml, mf) = (pts.iter().map(|p| p.0).sum::<f64>() / 13.0, pts.iter().map(|p| p.1).sum::<f64>() / 13.0);
        pts.iter().map(|(a, c)| (a - ml) * (c - mf)).sum::<f64>()
            / pts.iter().map(|(a, _)| (a - ml).powi(2)).sum::<f64>()
    }

    /// White noise gives about 0.5 and a random walk about 1.5 (P9's check), equal to the plain reference; the
    /// same series through P9's C `dfa_a1` (built for the computer with clang, as its test does) gave the values
    /// pinned here, and P9's Python `ref` 0.490216 and 1.560239 (2026-10-23).
    #[test]
    fn dfa_matches_reference() {
        let mut g = noise(1, 20.0);
        let white: Vec<f32> = (0..200).map(|_| 800.0 + g()).collect();
        let mut walk = 0.0;
        let brown: Vec<f32> = (0..200).map(|_| 800.0 + (walk += g() / 4.0, walk).1).collect();
        for (x, expect, p9) in [(white, 0.5, 0.490219), (brown, 1.5, 1.560235)] {
            let got = dfa_a1(&mut x.clone()).unwrap();
            assert!((f64::from(got) - reference(&x)).abs() < 1e-3, "{got} vs {}", reference(&x));
            assert!((got - expect).abs() < 0.15, "{got}, expected about {expect}");
            assert!((got - p9).abs() < 1e-4, "{got}, P9's C gives {p9}");
        }
    }

    /// A steady alternation of 30 ms: RMSSD 30; an ectopic beat (short, then long) drops its pairs instead of
    /// inflating it. Nothing before enough beats.
    #[test]
    fn rmssd_and_artifacts() {
        let mut h = Hrv::default();
        for i in 0..40 {
            h.push(if i % 2 == 0 { 800 } else { 830 });
        }
        assert_eq!((h.rmssd(), h.alpha1()), (Some(30), None), "40 beats: RMSSD, not yet DFA");
        for i in 0..60 {
            h.push(if i % 2 == 0 { 800 } else { 830 });
        }
        h.push(500);
        h.push(1130);
        for _ in 0..EVERY - 2 {
            h.push(815);
        }
        assert_eq!(h.rmssd(), Some(29), "the ectopic beat left out; the 815s add two zeros");
        assert!(h.alpha1().is_some_and(|a| a < 0.5), "alternation: anti-correlated");
    }
}
