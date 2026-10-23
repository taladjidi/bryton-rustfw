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

/// Charge in percent for a voltage at rest, from a typical lithium-ion curve (inferred: stock's own table is not
/// traced). Reads high while charging.
///
/// ```
/// use bryton_rustfw::battery::percent;
/// assert_eq!((percent(4250), percent(3800), percent(3200)), (100, 45, 0));
/// ```
pub fn percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 10] = [
        (3300, 0),
        (3600, 5),
        (3700, 15),
        (3750, 30),
        (3800, 45),
        (3850, 55),
        (3900, 63),
        (4000, 78),
        (4100, 90),
        (4200, 100),
    ];
    let mut prev = CURVE[0];
    for (v, p) in CURVE {
        if mv <= v {
            if v == prev.0 {
                return p;
            }
            let t = u32::from(mv.saturating_sub(prev.0)) * u32::from(p - prev.1) / u32::from(v - prev.0);
            return prev.1 + t as u8;
        }
        prev = (v, p);
    }
    100
}

/// Below this, on battery, the unit switches off (stock: at 0 %), before the cell is drawn too low.
pub const EMPTY_MV: u16 = 3300;

/// Readings below [`EMPTY_MV`] in a row before switching off, so one dip under load does not.
pub const EMPTY_READINGS: u8 = 3;

/// The battery as the app follows it: a smoothed voltage, and the low readings in a row.
#[derive(Default)]
pub struct Battery {
    mv: Option<u16>,
    low: u8,
}

impl Battery {
    /// Takes a reading (`charging`: USB power present). True when the unit should switch off: on battery and
    /// below [`EMPTY_MV`] [`EMPTY_READINGS`] times in a row.
    pub fn update(&mut self, mv: u16, charging: bool) -> bool {
        // a quarter of each new reading: smooths the noise and load dips without lagging minutes behind
        self.mv = Some(self.mv.map_or(mv, |old| ((3 * u32::from(old) + u32::from(mv)) / 4) as u16));
        self.low = if !charging && mv < EMPTY_MV { self.low + 1 } else { 0 };
        self.low >= EMPTY_READINGS
    }

    /// The smoothed voltage, mV.
    pub fn millivolts(&self) -> Option<u16> {
        self.mv
    }

    /// The charge estimate, percent.
    pub fn percent(&self) -> Option<u8> {
        self.mv.map(percent)
    }
}

#[cfg(test)]
mod tests {
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
    fn percent_is_monotonic() {
        let mut last = 0;
        for mv in (3000..4400).step_by(10) {
            let p = percent(mv);
            assert!(p >= last && p <= 100, "{mv} mV");
            last = p;
        }
    }
}
