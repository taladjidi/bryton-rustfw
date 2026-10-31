//! Training load: CTL (fitness), ATL (fatigue) and TSB (form) from the daily TSS of the scanned rides, and month totals.
//!
//! - **TSS of a ride** ([`ride_tss`]): from power, NP and the time with power against FTP ([`crate::power::tss`]);
//!   without power, the hrTSS ([`crate::hrtss`]) of the ride's *average* heart rate over its moving time. The scan keeps
//!   no per-second heart rate, so that hrTSS is **inferred** to be low for a variable ride (TRIMP grows faster than
//!   linearly with heart rate, and an average flattens it).
//! - **The model** (Coggan's Performance Management Chart, "Training and Racing with a Power Meter", 2nd ed. 2010;
//!   TrainingPeaks' help page on the PMC): CTL and ATL are exponentially weighted averages of the daily TSS with time
//!   constants of 42 and 7 days, `x[d] = x[d-1] + (TSS[d] - x[d-1]) / N`, and TSB is the form the rider starts the day
//!   with, **yesterday's CTL minus yesterday's ATL**. Read in: trainerroad.com/forum/t/performance-management-chart/18713,
//!   groups.google.com/g/golden-cheetah-users/c/h_KSL1oUmLI, uphillathlete.com/?p=60379 and
//!   fascatcoaching.com/tips/realtstwkt. This is the linear form (weight k = 1/N), **inferred** to match TrainingPeaks;
//!   WKO may use 1 - e^(-1/N), which weighs ATL's last day about 7 % differently. Days without a ride count 0; the
//!   series starts at 0, `WARM` days before the chart.
//! - **Where the rides come from:** the scan's index ([`crate::history::Index`], the newest [`RIDES`] rides). A rider
//!   with more rides than that in `DAYS` days gets a CTL that is too low at the far end; the oldest month of a full
//!   index is partial and is left out of [`Load::months`]; [`Load::partial`] says the window may be cut short.
//! - Fixed point (1/256), no heap, nothing written to the card.

use crate::history::{Index, RIDES, Ride};
use crate::{epoch_s, hrtss};

/// Days the chart shows.
pub const SHOWN: usize = 90;
/// Days before the chart that the averages run over, so that they start close to the rider's real level (the 42-day
/// average forgets its start as e^(-120/42), under 6 %).
const WARM: usize = 120;
const DAYS: usize = SHOWN + WARM;
/// CTL's and ATL's time constants, days.
const CTL_D: i32 = 42;
const ATL_D: i32 = 7;
/// Months kept, newest first.
pub const MONTHS: usize = 12;

/// One month's totals, saturated to 16 bits (a month of 6500 km, 45 days, 65 km of ascent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Month {
    /// Year x 12 + month - 1.
    pub ym: u16,
    /// Rides.
    pub rides: u16,
    /// Distance, units of 100 m.
    pub dist_100m: u16,
    /// Moving time, minutes.
    pub minutes: u16,
    /// Ascent, m.
    pub ascent_m: u16,
    /// TSS.
    pub tss: u16,
}

/// One day of the chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Day {
    /// That day's TSS.
    pub tss: u16,
    /// CTL at the end of the day.
    pub ctl: i16,
    /// ATL at the end of the day.
    pub atl: i16,
    /// The form the day started with: the day before's CTL minus its ATL.
    pub tsb: i16,
}

/// The daily TSS of the last `DAYS` days, yesterday's averages and the month totals.
pub struct Load {
    /// TSS per day, oldest first, today last.
    daily: [u16; DAYS],
    /// CTL and ATL at the end of yesterday, x256.
    y: (i32, i32),
    months: [Month; MONTHS],
    n: usize,
    built: bool,
    /// The index was full and its oldest ride is inside the window: older rides may be missing.
    partial: bool,
    /// A ride of the window got the heart rate estimate.
    hr_est: bool,
}

/// TSS of a ride: from power at `ftp`, else the hrTSS of its average heart rate; None with neither.
pub fn ride_tss(r: &Ride, ftp: u16, lthr: u16, max_hr: u16, female: bool) -> Option<u32> {
    ride_tss_est(r, ftp, lthr, max_hr, female).map(|t| t.0)
}

/// As [`ride_tss`], and whether it is the heart rate estimate. A power ride of zero TSS (a meter that dropped out)
/// with a heart rate falls back to the heart rate.
pub fn ride_tss_est(r: &Ride, ftp: u16, lthr: u16, max_hr: u16, female: bool) -> Option<(u32, bool)> {
    if ftp > 0
        && let Some(t) = r.tss(ftp).filter(|&t| t > 0 || r.avg_hr.is_none())
    {
        return Some((t, false));
    }
    let mut h = hrtss::Hrtss::default();
    h.seconds(r.avg_hr.map(u16::from), r.timer_s, max_hr, female);
    h.score(lthr, max_hr, female).map(|t| (t, true))
}

fn step(x: i32, tss: u32, n: i32) -> i32 {
    x + (tss.min(u32::from(u16::MAX)) as i32 * 256 - x) / n
}

fn whole(q: i32) -> i16 {
    ((q + 128) >> 8).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn sat(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

fn ym_of(r: &Ride) -> u16 {
    r.start.0.saturating_mul(12).saturating_add(u16::from(r.start.1.max(1)) - 1)
}

impl Load {
    /// Nothing built yet.
    pub const fn new() -> Self {
        let m = Month { ym: 0, rides: 0, dist_100m: 0, minutes: 0, ascent_m: 0, tss: 0 };
        Load { daily: [0; DAYS], y: (0, 0), months: [m; MONTHS], n: 0, built: false, partial: false, hr_est: false }
    }

    /// Built from a scan.
    pub fn is_built(&self) -> bool {
        self.built
    }

    /// The index was full and reaches back less than the window: older rides may be missing.
    pub fn partial(&self) -> bool {
        self.partial
    }

    /// A ride of the window was scored from its average heart rate (an estimate).
    pub fn hr_est(&self) -> bool {
        self.hr_est
    }

    /// Builds the series ending on `today` (days since 1970) and the month totals from the scan's `index`, `tss`
    /// giving a ride's score (its settings: FTP, LTHR).
    pub fn build(&mut self, today: u16, ix: &Index, tss: impl Fn(&Ride) -> Option<(u32, bool)>) {
        self.daily.fill(0);
        (self.partial, self.hr_est) = (false, false);
        self.months.fill(Month::default());
        self.n = 0;
        for r in (0..RIDES).map_while(|i| ix.get(i)) {
            let (y, mo, d, ..) = r.start;
            let day = u16::try_from(epoch_s((y, mo, d, 0, 0, 0)) / 86_400).unwrap_or(0);
            let (t, est) = tss(r).unwrap_or((0, false));
            let back = usize::from(today.wrapping_sub(day));
            if day <= today
                && let Some(slot) = (DAYS - 1).checked_sub(back).and_then(|i| self.daily.get_mut(i))
            {
                *slot = slot.saturating_add(sat(t));
                self.hr_est |= est;
            }
            // the index is newest first: a month is one run
            let ym = ym_of(r);
            let same = self.n.checked_sub(1).and_then(|k| self.months.get(k)).is_some_and(|m| m.ym == ym);
            let k = if same { self.n.wrapping_sub(1) } else { self.n };
            if let Some(m) = self.months.get_mut(k) {
                if !same {
                    (m.ym, self.n) = (ym, self.n + 1);
                }
                m.rides = m.rides.saturating_add(1);
                m.dist_100m = m.dist_100m.saturating_add(sat(r.dist_m / 100));
                m.minutes = m.minutes.saturating_add(sat(r.timer_s / 60));
                m.ascent_m = m.ascent_m.saturating_add(r.ascent_m.unwrap_or(0));
                m.tss = m.tss.saturating_add(sat(t));
            }
        }
        // a full index has cut its oldest month short, and the window too when its oldest ride is inside it
        self.partial = ix.get(RIDES - 1).is_some_and(|o| {
            let (y, m, d, ..) = o.start;
            let day = u16::try_from(epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0);
            usize::from(today.saturating_sub(day)) < DAYS
        });
        if let (Some(old), Some(m)) = (ix.get(RIDES - 1), self.n.checked_sub(1).and_then(|k| self.months.get(k)))
            && m.ym == ym_of(old)
        {
            self.n -= 1;
        }
        let mut y = (0, 0);
        for &t in self.daily.iter().take(DAYS - 1) {
            y = (step(y.0, t.into(), CTL_D), step(y.1, t.into(), ATL_D));
        }
        (self.y, self.built) = (y, true);
    }

    /// The months, newest first.
    pub fn months(&self) -> &[Month] {
        self.months.get(..self.n).unwrap_or(&[])
    }

    /// Today so far: `extra` is the TSS of a ride on its way, not scanned yet. None before a build.
    pub fn now(&self, extra: u32) -> Option<Day> {
        let t = u32::from(self.daily.last().copied()?).saturating_add(extra);
        let (c, a) = (step(self.y.0, t, CTL_D), step(self.y.1, t, ATL_D));
        self.built.then_some(Day { tss: sat(t), ctl: whole(c), atl: whole(a), tsb: whole(self.y.0 - self.y.1) })
    }

    /// The last [`SHOWN`] days, oldest first, today with `extra` added.
    pub fn days(&self, extra: u32) -> [Day; SHOWN] {
        let (mut c, mut a) = (0, 0);
        let mut out = [Day::default(); SHOWN];
        for (i, &t) in self.daily.iter().enumerate() {
            let tsb = c - a;
            let t = u32::from(t).saturating_add(if i + 1 == DAYS { extra } else { 0 });
            (c, a) = (step(c, t, CTL_D), step(a, t, ATL_D));
            if let Some(o) = i.checked_sub(WARM).and_then(|k| out.get_mut(k)) {
                *o = Day { tss: sat(t), ctl: whole(c), atl: whole(a), tsb: whole(tsb) };
            }
        }
        out
    }
}

impl Default for Load {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil_from_days;
    use crate::history::NONE;

    extern crate std;

    const FTP: u16 = 250;
    const TODAY: u16 = 20_000;

    /// A ride `back` days before TODAY worth `tss` at FTP 250 (NP 250, so TSS = seconds / 36).
    fn ride(back: u16, tss: u32) -> Ride {
        let (y, m, d) = civil_from_days(i32::from(TODAY - back));
        Ride {
            start: (y, m, d, 8, 0),
            np: Some(FTP),
            power_s: tss * 36,
            timer_s: tss * 36,
            dist_m: 30_000,
            ascent_m: Some(300),
            ..NONE
        }
    }

    fn built(rides: &[(u16, u32)]) -> Load {
        let mut ix = Index::new();
        for &(back, t) in rides {
            ix.add(ride(back, t));
        }
        let mut l = Load::new();
        l.build(TODAY, &ix, |r| ride_tss_est(r, FTP, 170, 190, false));
        l
    }

    /// The textbook recurrence in f64 over daily TSS, oldest first: (CTL, ATL) after each day.
    fn reference(tss: &[f64]) -> std::vec::Vec<(f64, f64)> {
        let (mut c, mut a) = (0.0, 0.0);
        tss.iter()
            .map(|&t| {
                c += (t - c) / 42.0;
                a += (t - a) / 7.0;
                (c, a)
            })
            .collect()
    }

    #[test]
    fn tss_from_power_and_from_heart_rate() {
        assert_eq!(ride_tss(&ride(0, 100), FTP, 170, 190, false), Some(100));
        let hr = Ride { avg_hr: Some(170), timer_s: 3600, ..NONE };
        assert_eq!(ride_tss(&hr, FTP, 170, 190, false), Some(100), "an hour at LTHR");
        assert_eq!(ride_tss(&hr, 0, 170, 190, false), Some(100), "no FTP set: the heart rate");
        assert_eq!(ride_tss(&NONE, FTP, 170, 190, false), None);
        assert_eq!(ride_tss(&hr, FTP, 0, 190, false), None, "no LTHR");
        // a meter that dropped out: power ride of 0 TSS with a heart rate falls back to it, and says so
        let dropped = Ride { np: Some(0), power_s: 3600, avg_hr: Some(170), timer_s: 3600, ..NONE };
        assert_eq!(ride_tss_est(&dropped, FTP, 170, 190, false), Some((100, true)));
        assert_eq!(ride_tss_est(&ride(0, 100), FTP, 170, 190, false), Some((100, false)));
        let no_hr = Ride { np: Some(0), power_s: 3600, ..NONE };
        assert_eq!(ride_tss_est(&no_hr, FTP, 170, 190, false), Some((0, false)), "no heart rate to fall back on");
    }

    /// A steady 100 a day: the averages follow the recurrence and head for 100, ATL faster than CTL.
    #[test]
    fn constant_load_converges() {
        let rides: std::vec::Vec<_> = (0..120).map(|b| (b, 100)).collect();
        let l = built(&rides);
        let r = reference(&[100.0; 120]);
        let now = l.now(0).unwrap();
        assert!((f64::from(now.ctl) - r[119].0).abs() <= 1.0, "{now:?} against {:?}", r[119]);
        assert!((f64::from(now.atl) - r[119].1).abs() <= 1.0);
        assert!(now.ctl > 90 && now.atl >= 99 && now.ctl < now.atl);
        let last = l.days(0)[SHOWN - 1];
        assert_eq!((last.ctl, last.atl, last.tss), (now.ctl, now.atl, 100));
        assert!((f64::from(last.tsb) - (r[118].0 - r[118].1)).abs() <= 1.0, "yesterday's CTL minus ATL");
    }

    /// A rest week after steady training: ATL falls faster than CTL, so form turns positive.
    #[test]
    fn a_rest_week_drops_fatigue_before_fitness() {
        let rides: std::vec::Vec<_> = (7..127).map(|b| (b, 100)).collect();
        let l = built(&rides);
        let mut t = [100.0; 127];
        t[120..].fill(0.0);
        let r = reference(&t);
        let d = l.days(0);
        let (before, after) = (d[SHOWN - 8], d[SHOWN - 1]);
        let (dc, da) = (before.ctl - after.ctl, before.atl - after.atl);
        assert!(dc > 0 && da > 2 * dc, "ATL fell {da}, CTL {dc}");
        assert!(before.tsb < 0 && after.tsb > 0, "form turns positive: {before:?} {after:?}");
        assert!((f64::from(after.ctl) - r[126].0).abs() <= 1.0 && (f64::from(after.atl) - r[126].1).abs() <= 1.0);
    }

    /// A ride on its way counts in today's numbers and in the last day of the chart, not in today's form.
    #[test]
    fn the_ride_on_its_way_adds_to_today() {
        let l = built(&[(1, 100)]);
        let (a, b) = (l.now(0).unwrap(), l.now(100).unwrap());
        assert!(b.ctl > a.ctl && b.atl > a.atl && a.tsb == b.tsb);
        assert_eq!(l.days(100)[SHOWN - 1].tss, 100);
        assert_eq!(Load::new().now(50), None);
    }

    #[test]
    fn month_totals_sum_the_rides() {
        // 2024-10-04 is TODAY: 0 and 1 days back share October's month (the 3rd), 40 and 70 days back do not
        let l = built(&[(0, 50), (1, 80), (40, 100), (70, 60)]);
        let m = l.months();
        assert_eq!(m.len(), 3);
        assert_eq!(
            (m[0].rides, m[0].tss, m[0].dist_100m, m[0].minutes, m[0].ascent_m),
            (2, 130, 600, 130 * 36 / 60, 600)
        );
        assert_eq!(m.iter().map(|m| m.rides).sum::<u16>(), 4);
        assert_eq!(m.iter().map(|m| m.tss).sum::<u16>(), 290);
        assert!(m.windows(2).all(|w| w[0].ym > w[1].ym), "newest first");
    }

    #[test]
    fn a_full_index_leaves_its_cut_month_out() {
        let rides: std::vec::Vec<_> = (0..RIDES as u16).map(|b| (b, 10)).collect();
        let l = built(&rides);
        let (y, m, _) = civil_from_days(i32::from(TODAY) - (RIDES as i32 - 1));
        assert!(l.months().iter().all(|k| k.ym != y * 12 + u16::from(m) - 1), "the cut month is out");
        assert!(l.months().iter().map(|m| m.rides).sum::<u16>() as usize <= RIDES);
    }
}
