//! The battery: millivolts from the ADC readings, a charge estimate, and when to switch off. Pure logic: the
//! board reads ADC3 (BOARD.md 12).

/// Battery voltage in mV: VDDA from VREFINT (`vref`, against `cal`, its reading at 3.3 V), then the battery
/// channel `raw` through the board's divider by 1.5 (stock's formula, BOARD.md 12). All readings 16-bit.
///
/// ```
/// // VDDA 3.3 V (VREFINT reads its calibration value), the pin at half scale: 1.65 V, so 2.475 V on the battery
/// assert_eq!(bryton_rustfw::battery::millivolts(32768, 24000, 24000), Some(2475));
/// ```
pub fn millivolts(raw: u16, vref: u16, cal: u16) -> Option<u16> {
    if vref == 0 {
        return None;
    }
    let vdda = 3300 * u64::from(cal) / u64::from(vref);
    u16::try_from(u64::from(raw) * vdda * 3 / (2 * 65535)).ok()
}

/// Stock's voltage table (BOARD.md 12): the cell's millivolts at each percent 0..=99, as Bryton characterised it.
/// The u16 table at 0xd01fb1ec in stock R039, read by Batt_VoltageToPercent (0xd01faeee; ghidra/notes/agent_d01e8000.md).
const STOCK: [u16; 100] = [
    3450, 3485, 3510, 3527, 3540, 3560, 3580, 3602, 3626, 3642, 3652, 3660, 3666, 3669, 3673, 3678, 3684, 3688, 3694,
    3697, 3702, 3707, 3711, 3715, 3719, 3722, 3725, 3728, 3731, 3734, 3736, 3738, 3741, 3743, 3746, 3748, 3751, 3753,
    3755, 3758, 3760, 3763, 3765, 3769, 3771, 3775, 3778, 3781, 3784, 3787, 3791, 3795, 3799, 3803, 3806, 3811, 3815,
    3820, 3825, 3829, 3834, 3839, 3845, 3850, 3856, 3862, 3868, 3875, 3882, 3889, 3895, 3902, 3909, 3915, 3921, 3928,
    3935, 3941, 3948, 3954, 3962, 3970, 3979, 3989, 4001, 4012, 4025, 4035, 4045, 4053, 4060, 4067, 4074, 4084, 4092,
    4101, 4111, 4120, 4130, 4140,
];

/// Charge in percent for a voltage at rest, as stock shows it: 100 from 4141 mV, 0 below 3450 mV, else the smallest
/// percent whose table entry reaches the reading (stock's binary search over its table, BOARD.md 12). Reads high
/// while charging.
///
/// ```
/// use bryton_rustfw::battery::percent;
/// assert_eq!((percent(4250), percent(3800), percent(3200)), (100, 53, 0));
/// ```
pub fn percent(mv: u16) -> u8 {
    if mv >= 4141 {
        100
    } else if mv < STOCK[0] {
        0
    } else {
        STOCK.partition_point(|&v| v < mv).max(1) as u8
    }
}

/// [`percent`] in tenths of a percent, on the same table: a drain of a few minutes shows where the whole percent
/// would stand still, interpolated between the table's steps. The integer part of this is what [`percent`] returns.
///
/// ```
/// use bryton_rustfw::battery::tenths;
/// assert_eq!((tenths(3800), tenths(3825), tenths(4300)), (530, 587, 1000));
/// ```
pub fn tenths(mv: u16) -> u32 {
    if mv >= 4141 {
        return 1000;
    }
    if mv < STOCK[0] {
        return 0;
    }
    // the percent i with STOCK[i - 1] < mv <= STOCK[i], i in 1..=99
    // clamped so no arithmetic below can overflow (the firmware has no panic site)
    let i = STOCK.partition_point(|&v| v < mv).clamp(1, 99);
    let (Some(&lo), Some(&hi)) = (STOCK.get(i - 1), STOCK.get(i)) else { return 1000 };
    // at STOCK[0] itself (the first percent) the offset saturates to 0
    i as u32 * 10 + 9 * u32::from(mv.saturating_sub(lo.saturating_add(1))) / u32::from(hi.saturating_sub(lo).max(1))
}

/// Fewest minutes on the battery before a drain is shown: the first minutes after a start are not (the reading
/// settles, the first load and the GNSS search start).
pub const MIN_MINUTES: usize = 10;

/// The drain is fitted over the last this many minutes on the battery (a ride's minutes, not the whole history).
pub const WINDOW: usize = 20;

/// A drain under this, tenths of a percent an hour, is not shown: the cell has not measurably dropped. 3.4 % an hour
/// is the idle overview's drain, measured 2026-10-28 (docs/power.md, "The main loop while idle"). The cell's capacity
/// is unknown (inferred: about 1200 mAh, docs/power.md), so no model answers in its place.
pub const MODEL_RATE: i64 = 34;

/// Below this, on battery, the unit switches off (stock: at 0 %), before the cell is drawn too low.
pub const EMPTY_MV: u16 = 3300;

/// Readings a minute (the app reads the battery every 10 s).
const PER_MINUTE: u32 = 6;

/// Readings below [`EMPTY_MV`] in a row before switching off, so one dip under load does not.
pub const EMPTY_READINGS: u8 = 3;

/// Minutes of battery history kept (one smoothed reading a minute): 4 hours.
pub const LOG: usize = 240;

/// The battery as the app follows it: a smoothed voltage, the low readings in a row, and a history (to measure
/// the drain on the unit: `tools/dev.py bat`).
pub struct Battery {
    mv: Option<u16>,
    low: u8,
    /// Ring of one entry a minute: mV, bit 15 set while charging.
    log: [u16; LOG],
    logged: usize,
    readings: u32,
}

impl Default for Battery {
    fn default() -> Self {
        Battery { mv: None, low: 0, log: [0; LOG], logged: 0, readings: 0 }
    }
}

impl Battery {
    /// Takes a reading (`charging`: USB power present). True when the unit should switch off: on battery and
    /// below [`EMPTY_MV`] [`EMPTY_READINGS`] times in a row.
    pub fn update(&mut self, mv: u16, charging: bool) -> bool {
        // a quarter of each new reading: smooths the noise and load dips without lagging minutes behind
        self.mv = Some(self.mv.map_or(mv, |old| ((3 * u32::from(old) + u32::from(mv)) / 4) as u16));
        self.low = if !charging && mv < EMPTY_MV { self.low.saturating_add(1) } else { 0 };
        if self.readings.is_multiple_of(PER_MINUTE)
            && let (Some(mv), Some(slot)) = (self.mv, self.log.get_mut(self.logged % LOG))
        {
            *slot = (mv & 0x7fff) | if charging { 0x8000 } else { 0 };
            self.logged += 1;
        }
        self.readings = self.readings.wrapping_add(1);
        self.low >= EMPTY_READINGS
    }

    /// The history, oldest first: (mV, charging), one a minute, the last one taken at the first reading of the
    /// current minute.
    pub fn history(&self) -> impl Iterator<Item = (u16, bool)> + '_ {
        let n = self.logged.min(LOG);
        (self.logged - n..self.logged).map(|i| {
            let e = self.log.get(i % LOG).copied().unwrap_or(0);
            (e & 0x7fff, e & 0x8000 != 0)
        })
    }

    /// The charge estimate, percent.
    pub fn percent(&self) -> Option<u8> {
        self.mv.map(percent)
    }

    /// The time left on the battery, minutes; None while charging (the last run on the battery is cut by the
    /// charger's entries), before [`MIN_MINUTES`] of it, or while the drain is under [`MODEL_RATE`] (the cell has not
    /// measurably dropped, so no time is given). The drain is the least-squares slope of the last [`WINDOW`]
    /// minutes, in tenths of a percent an hour, so one dip in the window moves it little and the curve's steps do not
    /// matter.
    ///
    /// ```
    /// use bryton_rustfw::battery::Battery;
    /// let mut b = Battery::default();
    /// for m in 0..12 {
    ///     for _ in 0..6 {
    ///         b.update(3900 - m, false);
    ///     }
    /// }
    /// assert!(b.minutes_left().is_some_and(|m| m > 60));
    /// ```
    pub fn minutes_left(&self) -> Option<u32> {
        // the last run on the battery, newest first
        let mut ys = [0i64; WINDOW];
        let mut n = 0;
        let mut i = self.logged;
        while n < WINDOW && i > self.logged.saturating_sub(LOG) {
            i -= 1;
            let e = self.log.get(i % LOG).copied().unwrap_or(0);
            let Some(slot) = ys.get_mut(n) else { break };
            if e & 0x8000 != 0 {
                break;
            }
            *slot = i64::from(tenths(e & 0x7fff));
            n += 1;
        }
        if n < MIN_MINUTES {
            return None;
        }
        // y = a + b x, x the minutes back from the newest: b is the drain a minute
        let (nn, mut sx, mut sy, mut sxy, mut sxx) = (n as i64, 0i64, 0i64, 0i64, 0i64);
        for (x, &y) in ys.iter().take(n).enumerate() {
            let x = x as i64;
            (sx, sy, sxy, sxx) = (sx + x, sy + y, sxy + x * y, sxx + x * x);
        }
        let den = nn * sxx - sx * sx; // positive: MIN_MINUTES points at distinct minutes
        let num = nn * sxy - sx * sy; // drain a minute x den, in tenths
        let rate = (num * 60).checked_div(den)?; // tenths of a percent an hour
        if rate < MODEL_RATE {
            return None;
        }
        let now = ((sy * den - num * sx).checked_div(nn * den)?).max(0); // the fitted charge at the newest minute, tenths
        u32::try_from((now * 60).checked_div(rate)?).ok()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn switches_off_only_after_low_readings_in_a_row_on_battery() {
        let mut b = Battery::default();
        assert!(!b.update(3200, false));
        assert!(!b.update(3200, false));
        assert!(!b.update(3200, true), "charging: never");
        assert!(!b.update(3200, false));
        assert!(!b.update(3400, false), "one good reading starts again");
        assert!(!b.update(3200, false) && !b.update(3200, false));
        assert!(b.update(3200, false));
    }

    #[test]
    fn history_keeps_the_last_four_hours_a_minute_apart() {
        let mut b = Battery::default();
        for i in 0..(LOG as u32 + 10) * PER_MINUTE {
            b.update(4000 - (i / PER_MINUTE) as u16, i < 30);
        }
        let h: std::vec::Vec<_> = b.history().collect();
        assert_eq!(h.len(), LOG);
        assert!(h.windows(2).all(|w| w[1].0 <= w[0].0), "oldest first, falling");
        assert!(!h[0].1 && h[LOG - 1].0 < 3800);
    }

    #[test]
    fn percent_is_monotonic() {
        let mut last = 0;
        for mv in (3000..4400).step_by(10) {
            let p = percent(mv);
            assert!(p >= last && p <= 100, "{mv} mV");
            last = p;
        }
    }

    /// One minute on the battery at `mv(m)` for each minute `m` of `from..to`.
    fn feed(b: &mut Battery, from: u16, to: u16, charging: bool, mv: impl Fn(u16) -> u16) {
        for m in from..to {
            for _ in 0..PER_MINUTE {
                b.update(mv(m), charging);
            }
        }
    }

    #[test]
    fn steady_drain_gives_the_time_left() {
        // 4000 to 3941 mV in an hour: 9 % an hour on this stretch (3.1 % an hour at half that drain, under MODEL_RATE),
        // 77 % left at the end, about 8.6 h
        let mut b = Battery::default();
        feed(&mut b, 0, 60, false, |m| 4000 - m);
        let left = b.minutes_left().unwrap();
        assert!((480..=560).contains(&left), "{left} minutes");
    }

    #[test]
    fn a_backlight_dip_in_the_window_moves_the_time_little() {
        let steady = || {
            let mut b = Battery::default();
            feed(&mut b, 0, 60, false, |m| 4000 - m);
            b.minutes_left().unwrap()
        };
        // a load sag of 10 mV for one minute (a backlight burst on the cell's resistance), inside the last 20
        let mut b = Battery::default();
        feed(&mut b, 0, 60, false, |m| 4000 - m - if m == 50 { 10 } else { 0 });
        let dip = b.minutes_left().unwrap();
        assert!(dip.abs_diff(steady()) * 7 <= steady(), "{dip} against {}", steady());
    }

    #[test]
    fn a_plateau_gives_no_time_until_the_cell_drops() {
        // 3850 mV for an hour: the percent does not move, so no time is given
        let mut b = Battery::default();
        feed(&mut b, 0, 60, false, |_| 3850);
        assert_eq!(b.minutes_left(), None);
    }

    #[test]
    fn a_rising_voltage_gives_no_time_and_a_long_history_still_does() {
        let mut b = Battery::default();
        feed(&mut b, 0, 60, false, |m| 3800 + m / 3);
        assert_eq!(b.minutes_left(), None, "rising: no drain");
        // 300 minutes wrap the ring (LOG entries): the last 20 still fit a steady 30 mV an hour
        let mut b = Battery::default();
        feed(&mut b, 0, 300, false, |m| 4000 - m / 2);
        let left = b.minutes_left().unwrap();
        assert!((500..=1000).contains(&left), "{left} minutes");
    }

    #[test]
    fn percent_matches_stock() {
        // stock's search, ported literally: lo 0, hi 99, mid 50, up to 8 rounds
        fn stock(mv: u16) -> u8 {
            if mv >= 4141 {
                return 100;
            }
            if mv < 3450 {
                return 0;
            }
            let (mut lo, mut hi, mut mid) = (0usize, 99usize, 50usize);
            for _ in 0..8 {
                let mut h = mid;
                if STOCK[mid] < mv {
                    lo = mid;
                    h = hi;
                }
                mid = (h + lo) / 2;
                hi = h;
                if h - lo < 2 {
                    break;
                }
            }
            hi as u8
        }
        assert!(STOCK.windows(2).all(|w| w[0] < w[1]), "strictly rising, so tenths divides by a step");
        let mut last = 0;
        for mv in 0..=u16::MAX {
            assert_eq!(percent(mv), stock(mv), "{mv} mV");
            assert!(tenths(mv) >= last, "{mv} mV");
            assert_eq!(tenths(mv) / 10, u32::from(percent(mv)), "{mv} mV");
            last = tenths(mv);
        }
    }

    #[test]
    fn charging_and_the_first_minutes_give_no_time() {
        let mut b = Battery::default();
        feed(&mut b, 0, 30, true, |_| 4100);
        assert_eq!(b.minutes_left(), None, "on the charger");
        feed(&mut b, 30, 39, false, |m| 4100 - m);
        assert_eq!(b.minutes_left(), None, "nine minutes off it");
        feed(&mut b, 39, 40, false, |m| 4100 - m);
        assert!(b.minutes_left().is_some(), "ten minutes off it, the charger's are left out");
    }
}
