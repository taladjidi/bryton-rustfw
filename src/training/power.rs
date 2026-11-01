//! Power metrics derived from the power meter, once a second: 3, 10 and 30 s rolling averages, and over the ride
//! max power, normalized power, IF, TSS, work, kcal and W' balance. Pure logic, integers only, no loop over a
//! window: running sums over a 32-second ring.
//!
//! Sources (checked 2026-10-23):
//! - **NP** (Coggan): the 30 s rolling average of power, from the 30th second on, raised to the 4th power, averaged,
//!   4th root (TrainingPeaks, <https://www.trainingpeaks.com/coach-blog/normalized-power-how-coaches-use/>; the
//!   same steps in Allen and Coggan, *Training and Racing with a Power Meter*). The 30 s average is rounded to the
//!   watt before the 4th power (under 0.5 W of error on NP), and the roots are `u64::isqrt`.
//! - **IF** = NP / FTP; **TSS** = (s x NP x IF) / (FTP x 3600) x 100, s the seconds counted (TrainingPeaks,
//!   <https://www.trainingpeaks.com/learn/articles/estimating-training-stress-score-tss/>).
//! - **kcal** from work: kcal = kJ / (4.184 x 0.24), a gross efficiency of 24 %, which gives the usual "1 kJ of
//!   work is about 1 kcal burned" (0.996 kcal per kJ; trainright.com, "energy expenditure, calories, kilojoules
//!   and power in cycling", <https://trainright.com/energy-expenditure-calories-kilojoules-and-power-in-cycling/>:
//!   gross efficiency 20 to 25 %, 1:1 in practice).
//! - **W' balance**: the differential form (Froncioni and Clarke, as Golden Cheetah's "Differential" model,
//!   `src/Metrics/WPrime.cpp`: above CP `W += CP - P`, below `W += (CP - P)(W' - W) / W'`; the form Skiba et al.
//!   2015, Eur J Appl Physiol 115:703, give as the differential model). Chosen over Skiba's 2012 integral model,
//!   which needs `exp` and the whole history. The patch project's planned P10 (bryton-S510-mods 9887d48) used the
//!   same form with CP = FTP, starting full at each ride and not clamped below 0 (a negative W'bal means CP or W' is
//!   set too low). Kept here in mJ, so the recovery near full does not stall on integer division.

use crate::ride::{Ride, State, ZONE_COLORS};

/// The rolling averages offered, seconds.
pub const WINDOWS: [u32; 3] = [3, 10, 30];
/// Words of [`Power::to_words`].
pub const POWER_WORDS: usize = 7;
/// Seconds of power kept: at least the longest window.
const RING: usize = 32;
const _: () = assert!(RING >= 30);

/// W' before it is set, J (the patch project's P10 default; typical values are 10 to 30 kJ).
pub const WPRIME_J: u16 = 20_000;
/// Bike weight before it is set, 100 g (bike, bottles and kit: the climb pacer's).
pub const BIKE_HG: u16 = 90;
/// Rider weight before it is set, kg (conventional).
pub const WEIGHT_KG: u16 = 75;

/// The power metrics, live and over the ride.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    ring: [u16; RING],
    /// The slot the next second goes into.
    i: usize,
    /// Seconds in the ring since the readings started (or came back), up to [`RING`].
    n: usize,
    /// The sums of the last 3, 10, 30 s.
    sums: [u32; 3],
    /// The NP window: the last 30 s of the ride's power, a dropout after power was heard as 0 W (apart from the
    /// display ring above, which restarts on a dropout: a lost sensor reads "--"), its slot, count and sum.
    wring: [u16; 30],
    wi: usize,
    wn: usize,
    wsum: u32,
    /// Power has been heard this ride (or lap, carried by [`Power::new_lap`]): a later dropout counts as 0 W.
    heard: bool,
    /// The ride: seconds with power, max, the sum of 30 s averages to the 4th power and their count, work (J).
    secs: u32,
    max: u16,
    np_sum: u64,
    np_n: u32,
    work_j: u64,
    /// W' balance, mJ (None: full, no ride yet).
    wbal_mj: Option<i64>,
}

impl Default for Power {
    fn default() -> Self {
        Self::new()
    }
}

impl Power {
    /// No power yet.
    pub const fn new() -> Self {
        Power {
            ring: [0; RING],
            wring: [0; 30],
            wi: 0,
            wn: 0,
            wsum: 0,
            heard: false,
            i: 0,
            n: 0,
            sums: [0; 3],
            secs: 0,
            max: 0,
            np_sum: 0,
            np_n: 0,
            work_j: 0,
            wbal_mj: None,
        }
    }

    /// A new ride: the ride's totals back to zero, W' full (the rolling averages go on).
    pub fn new_ride(&mut self) {
        *self = Power {
            ring: self.ring,
            i: self.i,
            n: self.n,
            sums: self.sums,
            wring: self.wring,
            wi: self.wi,
            wn: self.wn,
            wsum: self.wsum,
            ..Power::default()
        };
    }

    /// The ride's totals as words, for the recorder's recovery record ([`Power::resume`] reads them back): the NP
    /// sum (2 words) and count, the work (J, held to `u32`: 596 h at 2000 W), the seconds, max power with `heard`
    /// (bit 16) and a W' balance there (bit 17), the W' balance (mJ, within +-65 535 000).
    pub fn to_words(&self) -> [u32; POWER_WORDS] {
        let wbal = self.wbal_mj.unwrap_or(0).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let flags = u32::from(self.max) | u32::from(self.heard) << 16 | u32::from(self.wbal_mj.is_some()) << 17;
        let work = self.work_j.min(u64::from(u32::MAX)) as u32;
        [self.np_sum as u32, (self.np_sum >> 32) as u32, self.np_n, work, self.secs, flags, wbal as u32]
    }

    /// A ride resumed after a reset: its totals from [`Power::to_words`] (all zeros, a record of a build before
    /// them: [`Power::new_ride`], the totals then cover the time since the resume), the rolling averages go on.
    /// The NP window starts again: the first 29 s after the resume add no NP sample.
    pub fn resume(&mut self, w: &[u32; POWER_WORDS]) {
        self.new_ride();
        let [lo, hi, np_n, work, secs, flags, wbal] = *w;
        (self.np_sum, self.np_n, self.work_j, self.secs) =
            (u64::from(lo) | u64::from(hi) << 32, np_n, work.into(), secs);
        (self.max, self.heard) = (flags as u16, flags & 1 << 16 != 0);
        self.wbal_mj = (flags & 1 << 17 != 0).then_some(i64::from(wbal as i32));
    }

    /// One second: `p` the power now (None: no meter or stale), `ride` after its own second, CP and W' (J).
    /// The totals count the ride's moving seconds; W' balance runs while the ride is not idle (a stop recovers),
    /// a second without a reading counting as 0 W.
    pub fn second(&mut self, p: Option<u16>, ride: &Ride, cp: u16, wprime_j: u16) {
        self.sample(p, ride.moving());
        if ride.state() != State::Idle {
            let full = i64::from(wprime_j.max(1)) * 1000;
            let (cp, p) = (i64::from(cp), i64::from(p.unwrap_or(0)));
            let w = self.wbal_mj.unwrap_or(full);
            let w = if p > cp { w - (p - cp) * 1000 } else { w + (full - w) * (cp - p) / i64::from(wprime_j.max(1)) };
            self.wbal_mj = Some(w.clamp(-full, full));
        } else {
            self.wbal_mj = None; // "None before a ride": an ended or discarded ride's value is not the next one's
        }
    }

    /// One second's power, counted in the ride's totals if `moving`: [`Power::second`] without W' balance (a ride
    /// read back from its file, a second a record).
    pub fn sample(&mut self, p: Option<u16>, moving: bool) {
        // The display ring restarts on a dropout (a lost sensor reads "--"). The ride's totals do not: once power
        // was heard, a dropout is a second at 0 W in the NP window, the work and the seconds, as Golden Cheetah's NP
        // (src/Metrics/Coggan.cpp `IsoPower`) rolls over every sample, none skipped, and counts them all in its
        // seconds, so TSS counts the dropout's time and the averages agree. Before the first reading: no meter.
        match p {
            Some(p) => self.push(p),
            None => (self.n, self.sums) = (0, [0; 3]),
        }
        self.heard |= p.is_some();
        let q = p.or(self.heard.then_some(0));
        if let Some(q) = q {
            self.push_np(q);
        }
        if let (Some(p), true) = (q, moving) {
            self.secs = self.secs.saturating_add(1);
            self.max = self.max.max(p);
            self.work_j = self.work_j.saturating_add(u64::from(p));
            if self.wn >= 30 {
                let a = u64::from((self.wsum + 15) / 30);
                self.np_sum = self.np_sum.saturating_add(a * a * a * a);
                self.np_n = self.np_n.saturating_add(1);
            }
        }
    }

    /// A new lap's power: nothing counted, `heard` carried over from the ride so a dropout in its first seconds is
    /// a 0 W second as it is in the ride.
    pub fn new_lap(heard: bool) -> Power {
        Power { heard, ..Power::new() }
    }

    fn push_np(&mut self, q: u16) {
        if self.wn >= 30 {
            let old = self.wring.get(self.wi).copied().unwrap_or(0);
            self.wsum = self.wsum.saturating_sub(u32::from(old));
        }
        if let Some(slot) = self.wring.get_mut(self.wi) {
            *slot = q;
        }
        self.wsum = self.wsum.saturating_add(u32::from(q));
        self.wi = (self.wi + 1) % 30;
        self.wn = (self.wn + 1).min(30);
    }

    fn push(&mut self, p: u16) {
        for (sum, &w) in self.sums.iter_mut().zip(&WINDOWS) {
            if self.n >= w as usize {
                let old = self.ring.get((self.i + RING - w as usize) % RING).copied().unwrap_or(0);
                *sum = sum.saturating_sub(u32::from(old));
            }
            *sum = sum.saturating_add(u32::from(p));
        }
        if let Some(slot) = self.ring.get_mut(self.i) {
            *slot = p;
        }
        self.i = (self.i + 1) % RING;
        self.n = (self.n + 1).min(RING);
    }

    /// The average of the last `WINDOWS[k]` seconds (of the seconds there are, while fewer), W.
    ///
    /// ```
    /// use bryton_rustfw::{power::Power, ride::Ride};
    /// let mut p = Power::default();
    /// for w in [100, 200, 300, 400] {
    ///     p.second(Some(w), &Ride::new(), 200, 20_000);
    /// }
    /// assert_eq!([0, 1, 2].map(|k| p.rolling(k)), [Some(300), Some(250), Some(250)]);
    /// ```
    pub fn rolling(&self, k: usize) -> Option<u16> {
        let (&sum, &w) = (self.sums.get(k)?, WINDOWS.get(k)?);
        let n = self.n.min(w as usize) as u32;
        (n > 0).then(|| (sum / n) as u16)
    }

    /// The average of the last `secs` seconds with power (of the seconds there are, while fewer), W: the big power
    /// value's smoothing. The 1 s value is the live reading, not this (app/fields.rs).
    ///
    /// ```
    /// use bryton_rustfw::{power::Power, ride::Ride};
    /// let mut p = Power::default();
    /// for w in [100, 200, 300, 400, 500] {
    ///     p.second(Some(w), &Ride::new(), 200, 20_000);
    /// }
    /// assert_eq!([3, 5, 10].map(|s| p.average(s)), [Some(400), Some(300), Some(300)]);
    /// ```
    pub fn average(&self, secs: usize) -> Option<u16> {
        let n = self.n.min(secs).min(RING);
        let sum: u32 =
            (1..=n).map(|back| u32::from(self.ring.get((self.i + RING - back) % RING).copied().unwrap_or(0))).sum();
        (n > 0).then(|| (sum / n as u32) as u16)
    }

    /// The ride's max power (None before a second with power).
    pub fn max(&self) -> Option<u16> {
        (self.secs > 0).then_some(self.max)
    }

    /// Normalized power, W (None before 30 s with power).
    pub fn np(&self) -> Option<u16> {
        (self.np_n > 0).then(|| (self.np_sum / u64::from(self.np_n)).isqrt().isqrt().min(u64::from(u16::MAX)) as u16)
    }

    /// Intensity factor x 100: NP / FTP (None without an FTP: a ratio to 0 W says nothing, not IF 25000).
    pub fn if_x100(&self, ftp: u16) -> Option<u32> {
        self.np().filter(|_| ftp > 0).map(|np| u32::from(np) * 100 / u32::from(ftp))
    }

    /// Training stress score: (s x NP x IF) / (FTP x 3600) x 100 = s x NP^2 x 100 / (FTP^2 x 3600).
    pub fn tss(&self, ftp: u16) -> Option<u32> {
        let np = self.np().filter(|_| ftp > 0)?;
        Some(tss(self.secs, np, ftp))
    }

    /// Average power over the seconds counted, W (None before one).
    pub fn avg(&self) -> Option<u16> {
        (self.secs > 0).then(|| (self.work_j / u64::from(self.secs)).min(u64::from(u16::MAX)) as u16)
    }

    /// Work, kJ.
    pub fn kj(&self) -> u32 {
        (self.work_j / 1000).min(u64::from(u32::MAX)) as u32
    }

    /// Energy burned, kcal: work / (4.184 kJ per kcal x 24 % efficiency).
    pub fn kcal(&self) -> u32 {
        (self.work_j * 100 / (4184 * 24)).min(u64::from(u32::MAX)) as u32
    }

    /// W' balance, J (None before a ride).
    pub fn wbal_j(&self) -> Option<i32> {
        self.wbal_mj.map(|w| (w / 1000) as i32)
    }
}

/// Training stress score of `secs` seconds with power at normalized power `np`: s x NP^2 x 100 / (FTP^2 x 3600).
///
/// ```
/// assert_eq!(bryton_rustfw::power::tss(3600, 250, 250), 100); // an hour at FTP
/// ```
pub fn tss(secs: u32, np: u16, ftp: u16) -> u32 {
    let (np, ftp) = (u64::from(np), u64::from(ftp.max(1)));
    // seconds held to 277 h so that np^2 x 100 x seconds fits in 64 bits whatever a damaged file says (found by the fuzz)
    (u64::from(secs.min(1_000_000)) * np * np * 100 / (ftp * ftp * 3600)).min(u64::from(u32::MAX)) as u32
}

/// 3 s power per kg, x 10.
///
/// ```
/// assert_eq!(bryton_rustfw::power::wkg_x10(262, 75), 34);
/// ```
pub fn wkg_x10(w: u16, kg: u16) -> u32 {
    u32::from(w) * 10 / u32::from(kg.max(1))
}

/// The colour of a W' balance of `j` out of `wprime_j` as it drains: green over 75 %, olive over 50 %, brown over
/// 25 %, then wine (the zone palette).
///
/// ```
/// use bryton_rustfw::{power::wbal_color, ride::ZONE_COLORS};
/// assert_eq!([20_000, 12_000, 6000, -10].map(|j| wbal_color(j, 20_000)), [1, 2, 3, 4].map(|i| ZONE_COLORS[i]));
/// ```
pub fn wbal_color(j: i32, wprime_j: u16) -> u16 {
    let pct = i64::from(j) * 100 / i64::from(wprime_j.max(1));
    let zone = [75, 50, 25].iter().filter(|&&t| pct <= t).count();
    ZONE_COLORS.get(1 + zone).copied().unwrap_or(ZONE_COLORS[4])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tss_holds_its_seconds() {
        assert_eq!(tss(u32::MAX, u16::MAX, 1), tss(1_000_000, u16::MAX, 1), "no overflow from a damaged file");
    }
    use crate::ride::Input;

    /// A second with no reading after power was heard counts as 0 W in the window and in the seconds (Golden Cheetah's
    /// `IsoPower` rolls over every sample): 60 s at 200 W, 30 s of dropout, 60 s at 200 W.
    #[test]
    fn a_dropout_is_zero_watts() {
        let (mut p, r) = (Power::default(), riding());
        for _ in 0..60 {
            p.second(Some(200), &r, 250, 20_000);
        }
        let before = p.np();
        for _ in 0..30 {
            p.second(None, &r, 250, 20_000);
        }
        assert_eq!(p.rolling(2), None, "the display reads -- while the sensor is lost");
        assert_eq!(p.average(5), None);
        for _ in 0..60 {
            p.second(Some(200), &r, 250, 20_000);
        }
        assert_eq!(p.secs, 150, "the dropout's seconds count toward TSS");
        assert!(p.np() < before, "{:?} < {before:?}", p.np());
        let mut q = Power::default(); // no meter at all: nothing counted
        for _ in 0..60 {
            q.second(None, &r, 250, 20_000);
        }
        assert_eq!((q.secs, q.np()), (0, None));
    }

    /// With the ring kept through an auto-pause (the ride is not moving), the paused seconds sit in the 30 s window
    /// but are not counted: NP of the seconds after the stop is pulled down by the zeros, `secs` is the moving ones.
    #[test]
    fn np_across_an_auto_pause() {
        let (mut p, r) = (Power::default(), riding());
        let mut stopped = Ride::new();
        stopped.start_pause();
        stopped.second(&Input { speed_x10: Some(0), ..Input::default() });
        assert!(!stopped.moving());
        for _ in 0..60 {
            p.second(Some(200), &r, 250, 20_000);
        }
        for _ in 0..30 {
            p.second(Some(0), &stopped, 250, 20_000);
        }
        assert_eq!(p.secs, 60, "paused seconds are not counted");
        for _ in 0..30 {
            p.second(Some(200), &r, 250, 20_000);
        }
        let np_after = p.np().unwrap();
        assert!(np_after < 200, "{np_after}");
        assert_eq!(p.secs, 90);
    }

    /// No FTP: no IF and no TSS (the screens show "--"), not a ratio to 0 W.
    #[test]
    fn no_ftp_no_score() {
        let (mut p, r) = (Power::default(), riding());
        for _ in 0..60 {
            p.second(Some(200), &r, 250, 20_000);
        }
        assert_eq!((p.if_x100(0), p.tss(0)), (None, None));
        assert!(p.if_x100(200).is_some() && p.tss(200).is_some());
    }

    fn riding() -> Ride {
        let mut r = Ride::new();
        r.start_pause();
        r.second(&Input { speed_x10: Some(300), ..Input::default() });
        r
    }

    /// Steady power: NP equals it, IF and TSS as TrainingPeaks defines them (an hour at FTP is IF 1.00, TSS 100).
    #[test]
    fn an_hour_at_ftp() {
        let (mut p, r) = (Power::default(), riding());
        for _ in 0..3600 {
            p.second(Some(250), &r, 250, 20_000);
        }
        assert_eq!((p.np(), p.if_x100(250), p.tss(250)), (Some(250), Some(100), Some(100)));
        assert_eq!((p.kj(), p.kcal(), p.max()), (900, 896, Some(250)), "250 W x 3600 s = 900 kJ; / 1.004");
        assert_eq!(p.wbal_j(), Some(20_000), "at CP, W' stays full");
    }

    /// Alternating 1 min at 400 W and 1 min at 100 W: NP is well above the average (250 W). The hand computation:
    /// the 30 s averages ramp linearly between 100 and 400 for 30 s at each change and sit at 100 or 400 between;
    /// NP is the 4th root of the mean of their 4th powers: 315.2 W (Python, `p` the 3600 readings:
    /// `avgs = [round(sum(p[i - 29:i + 1]) / 30) for i in range(29, 3600)]`,
    /// `(sum(a**4 for a in avgs) / len(avgs)) ** 0.25`).
    #[test]
    fn np_of_intervals() {
        let (mut p, r) = (Power::default(), riding());
        for s in 0..3600 {
            p.second(Some(if s / 60 % 2 == 0 { 400 } else { 100 }), &r, 250, 20_000);
        }
        let np = p.np().unwrap();
        assert!((314..=316).contains(&np), "NP {np}");
        assert_eq!(p.max(), Some(400));
        assert_eq!(p.if_x100(250), Some(u32::from(np) * 100 / 250));
    }

    /// The rolling averages forget what left the window, and start again after a gap in the readings.
    #[test]
    fn rolling_windows() {
        let (mut p, r) = (Power::default(), Ride::new());
        for s in 0..40 {
            p.second(Some(if s < 35 { 100 } else { 400 }), &r, 250, 20_000);
        }
        // last 5 s at 400: 3 s all 400; 10 s half; 30 s 5 of 30
        assert_eq!([0, 1, 2].map(|k| p.rolling(k)), [Some(400), Some(250), Some(150)]);
        p.second(None, &r, 250, 20_000);
        assert_eq!(p.rolling(0), None, "stale meter");
        p.second(Some(50), &r, 250, 20_000);
        assert_eq!(p.rolling(2), Some(50), "from the reading on");
        assert_eq!(p.max(), None, "no ride: no totals");
    }

    /// W' balance: 60 s at CP + 200 W takes 12 kJ; recovery below CP follows (CP - P)(W' - W)/W' a second, so
    /// at 0 W and CP 250 the deficit shrinks by a factor (1 - 250/20000) each second: after 120 s, 12 kJ x
    /// 0.9875^120 = 2.65 kJ still missing (Golden Cheetah's differential model).
    #[test]
    fn wbal_drains_and_recovers() {
        let (mut p, r) = (Power::default(), riding());
        for _ in 0..60 {
            p.second(Some(450), &r, 250, 20_000);
        }
        assert_eq!(p.wbal_j(), Some(8000));
        for _ in 0..120 {
            p.second(Some(0), &r, 250, 20_000);
        }
        let missing = 20_000 - p.wbal_j().unwrap();
        assert!((2645..=2665).contains(&missing), "{missing} J missing");
        for _ in 0..3000 {
            p.second(None, &r, 250, 20_000); // no reading: as at rest
        }
        assert!(p.wbal_j().unwrap() >= 19_999, "back to full");
        for _ in 0..200 {
            p.second(Some(400), &r, 250, 20_000);
        }
        assert_eq!(p.wbal_j(), Some(-10_000), "below 0: CP or W' set too low");
        p.new_ride();
        assert_eq!((p.wbal_j(), p.kj(), p.rolling(0)), (None, 0, Some(400)), "a new ride: full again");
        p.second(Some(400), &r, 250, 20_000);
        assert!(p.wbal_j().is_some());
        let idle = Ride::new();
        p.second(Some(0), &idle, 250, 20_000);
        assert_eq!(p.wbal_j(), None, "no ride: no W' balance left over from the last one");
    }
}
