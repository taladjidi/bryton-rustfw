//! Power match for ERG: the trainer's own power reading is a little off from the rider's power meter (the owner's
//! D900 holds about 20 W away from the Sigeyi), so the target sent is corrected until the METER reads the workout's
//! target. What Garmin calls power match, and Zwift's "use the power meter as the power source".
//!
//! The ratio `r = trainer / meter` is estimated from the two readings, once a second, with an exponential average
//! (time constant [`TAU_S`]; the first sample seeds it) and clamped to [`MIN_R`]..=[`MAX_R`]. The trainer reading high
//! against the meter (r above 1) means the meter sees less than the target, so the target sent is `target * r`:
//! higher. Samples are taken only with ERG held, both powers fresh, above [`MIN_W`] and pedalling, and not in the
//! [`SETTLE_S`] after a target change or a resume (the trainer is still settling). With a source lost the ratio is
//! held [`HOLD_S`], then eases back to 1.0 over [`DECAY_S`]. Fixed point (Q16), no division by zero, no indexing.
//!
//! The ratio the commands use ([`PowerMatch::ratio`]) moves only when the corrected target would move by
//! [`STEP_W`] or more, or at a target change, so a drifting average does not send a page every second.

/// 1.0 in Q16.
pub const ONE: i32 = 1 << 16;
/// The lowest and highest ratio, Q16: 0.85 and 1.15.
pub const MIN_R: i32 = ONE * 85 / 100;
/// See [`MIN_R`].
pub const MAX_R: i32 = ONE * 115 / 100;
/// Both powers must be above this, W.
pub const MIN_W: u16 = 30;
/// Seconds skipped after a target change or a resume.
pub const SETTLE_S: u8 = 5;
/// The average's time constant, seconds (one update a second).
pub const TAU_S: i32 = 30;
/// Seconds a lost source holds the ratio before it eases back.
pub const HOLD_S: u16 = 60;
/// Seconds the ease back to 1.0 takes.
pub const DECAY_S: i32 = 30;
/// The least move of the corrected target that changes the ratio the commands use, W.
pub const STEP_W: i64 = 2;

/// The correction's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerMatch {
    r: i32,
    applied: i32,
    seeded: bool,
    erg: bool,
    settle: u8,
    target: u16,
    stale_s: u16,
    dec: i32,
}

impl Default for PowerMatch {
    fn default() -> Self {
        Self::new()
    }
}

/// `target_w` times `ratio_q16`, rounded, W.
///
/// ```
/// use bryton_rustfw::power_match::{apply, ONE};
/// assert_eq!(apply(200, ONE), 200);
/// assert_eq!(apply(200, ONE * 11 / 10), 220, "the trainer reads 10 % high: ask for more");
/// ```
pub fn apply(target_w: u16, ratio_q16: i32) -> u16 {
    let w = (i64::from(target_w) * i64::from(ratio_q16) + i64::from(ONE) / 2) >> 16;
    w.clamp(0, i64::from(u16::MAX)) as u16
}

impl PowerMatch {
    /// No estimate: the ratio is 1.0.
    pub const fn new() -> Self {
        PowerMatch { r: ONE, applied: ONE, seeded: false, erg: false, settle: SETTLE_S, target: 0, stale_s: 0, dec: 0 }
    }

    /// One second. `erg`: ERG is held now, for `target_w` (the rider's, not the corrected one); `meter` and `trainer`
    /// the powers if heard within 2 s; `rpm` the cadence.
    ///
    /// ```
    /// use bryton_rustfw::power_match::{apply, PowerMatch};
    /// let mut pm = PowerMatch::new();
    /// for _ in 0..20 {
    ///     pm.update(true, 200, Some(180), Some(200), 90); // the meter reads 20 W under the trainer
    /// }
    /// assert!(apply(200, pm.ratio()) > 215, "so the target sent goes up");
    /// ```
    pub fn update(&mut self, erg: bool, target_w: u16, meter: Option<u16>, trainer: Option<u16>, rpm: u8) {
        self.erg = erg;
        if !erg {
            self.settle = SETTLE_S; // a resume settles too
            return;
        }
        if target_w != self.target {
            (self.target, self.settle, self.applied) = (target_w, SETTLE_S, self.r);
        }
        let (Some(m), Some(t)) = (meter, trainer) else {
            self.lost();
            self.sync();
            return;
        };
        self.stale_s = 0;
        if self.settle > 0 {
            self.settle -= 1;
        } else if m > MIN_W && t > MIN_W && rpm > 0 {
            let obs = ((i64::from(t) << 16) / i64::from(m)).clamp(i64::from(MIN_R), i64::from(MAX_R)) as i32;
            self.r = if self.seeded { self.r + (obs - self.r) / TAU_S } else { obs };
            self.seeded = true;
        }
        self.sync();
    }

    /// A source is silent this second: hold, then ease back to 1.0.
    fn lost(&mut self) {
        self.stale_s = self.stale_s.saturating_add(1);
        if self.stale_s <= HOLD_S {
            return;
        }
        if self.stale_s == HOLD_S + 1 {
            self.dec = match (self.r - ONE) / DECAY_S {
                0 => (self.r - ONE).signum(),
                d => d,
            };
        }
        let next = self.r - self.dec;
        self.r = if (self.r - ONE).signum() == (next - ONE).signum() { next } else { ONE };
        if self.r == ONE {
            self.seeded = false;
        }
    }

    /// The commands' ratio follows the estimate once the target it makes has moved by [`STEP_W`].
    fn sync(&mut self) {
        if (i64::from(self.target) * i64::from(self.r - self.applied)).abs() >> 16 >= STEP_W {
            self.applied = self.r;
        }
    }

    /// The ratio the commands use, Q16.
    pub fn ratio(&self) -> i32 {
        self.applied
    }

    /// The correction in force as a percentage, while ERG is held and there is an estimate.
    pub fn pct(&self) -> Option<i32> {
        (self.erg && self.seeded).then(|| ((i64::from(self.applied - ONE) * 100 + i64::from(ONE) / 2) >> 16) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trainer reading `k` x the meter, at `target`, for `s` seconds.
    fn run(pm: &mut PowerMatch, s: u32, target: u16, k: f64) {
        for _ in 0..s {
            let m = (f64::from(target) / k) as u16;
            pm.update(true, target, Some(m), Some(target), 90);
        }
    }

    #[test]
    fn a_trainer_reading_high_is_asked_for_more() {
        let mut pm = PowerMatch::new();
        run(&mut pm, 80, 200, 1.1);
        let sent = apply(200, pm.ratio());
        assert!((218..=222).contains(&sent), "{sent}: 200 x 1.1");
        assert_eq!(pm.pct(), Some(10));
        let mut low = PowerMatch::new();
        run(&mut low, 80, 200, 0.9);
        assert!(apply(200, low.ratio()) < 184, "reading low: asked for less");
    }

    #[test]
    fn the_ratio_is_clamped() {
        let mut pm = PowerMatch::new();
        run(&mut pm, 60, 200, 1.5);
        assert_eq!(pm.r, MAX_R, "1.15");
        run(&mut pm, 600, 200, 0.5);
        assert!(pm.r - MIN_R < 40, "0.85, to the average's integer step");
    }

    #[test]
    fn a_lost_meter_holds_then_eases_back() {
        let mut pm = PowerMatch::new();
        run(&mut pm, 60, 200, 1.1);
        let held = pm.ratio();
        for _ in 0..HOLD_S {
            pm.update(true, 200, None, Some(200), 90);
        }
        assert_eq!(pm.ratio(), held, "held for 60 s");
        for _ in 0..DECAY_S + 2 {
            pm.update(true, 200, None, Some(200), 90);
        }
        assert_eq!(pm.r, ONE, "back to 1.0");
        assert_eq!(apply(200, pm.ratio()), 200);
        assert_eq!(pm.pct(), None, "and nothing shown");
    }

    #[test]
    fn steps_and_resumes_are_skipped() {
        let mut pm = PowerMatch::new();
        run(&mut pm, 20, 200, 1.0);
        let r = pm.r;
        run(&mut pm, u32::from(SETTLE_S), 250, 1.1); // a new target, the trainer settling
        assert_eq!(pm.r, r, "no sample in the 5 s after a target change");
        run(&mut pm, 1, 250, 1.1);
        assert_ne!(pm.r, r);
        let r = pm.r;
        pm.update(false, 250, None, None, 0);
        run(&mut pm, u32::from(SETTLE_S), 250, 1.15);
        assert_eq!(pm.r, r, "nor after a resume");
    }

    #[test]
    fn low_power_and_no_cadence_take_no_sample() {
        let mut pm = PowerMatch::new();
        for _ in 0..20 {
            pm.update(true, 25, Some(20), Some(25), 90);
            pm.update(true, 200, Some(150), Some(200), 0);
            pm.update(true, 200, Some(30), Some(200), 90);
        }
        assert_eq!((pm.r, pm.pct()), (ONE, None));
    }

    #[test]
    fn the_commands_ratio_moves_by_two_watts_or_more() {
        let mut pm = PowerMatch::new();
        run(&mut pm, 10, 200, 1.0);
        let mut moves = 0;
        let mut last = pm.ratio();
        for _ in 0..90 {
            pm.update(true, 200, Some(182), Some(200), 90);
            if pm.ratio() != last {
                moves += 1;
                assert!(apply(200, pm.ratio()).abs_diff(apply(200, last)) >= 2);
                last = pm.ratio();
            }
        }
        assert!((1..=10).contains(&moves), "{moves} moves for a 20 W offset");
    }
}
