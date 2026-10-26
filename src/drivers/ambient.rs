//! The chips on the sensor bus (I2C4, BOARD.md 16), against [`Board::i2c_read`] / [`Board::i2c_write`]: the ambient
//! light sensor (LTR-308ALS), the barometer (LPS22HH, read for its temperature and pressure) and the accelerometer
//! (only switched off: stock leaves it running). Plus the automatic backlight: a level from the ambient light, smoothed
//! and with hysteresis so it does not flicker between two levels.
//!
//! Power first: the light sensor measures only while the automatic backlight (or the sensors menu) needs it, then
//! continuously (100 ms conversions, read every [`crate::LIGHT_MS`], so the backlight follows the light within about
//! a second; the sensor draws about a tenth of a milliamp, nothing beside the backlight it steers), and is in
//! standby otherwise; the barometer only converts when asked (one-shot).
//!
//! The LTR-308ALS register fields (MEAS_RATE: resolution in bits 6..4, 0 = 20 bits / 400 ms, 2 = 18 bits / 100 ms;
//! rate in bits 2..0, 2 = every 100 ms; lux = count x 0.6 / gain / (integration time / 100 ms)) are from DFRobot's
//! driver (<https://github.com/DFRobot/DFRobot_LTR308>, DFRobot_LTR308.h / .cpp), which follows the datasheet.

use crate::Board;

/// Ambient light sensor's 7-bit address (stock: 0xa6 as an 8-bit address).
pub const LIGHT_ADDR: u8 = 0x53;
/// Barometer's 7-bit address (stock: 0xb8).
pub const BARO_ADDR: u8 = 0x5c;
/// Accelerometer's 7-bit address (stock: 0x30).
pub const ACCEL_ADDR: u8 = 0x18;

// LTR-308ALS registers (the ones stock uses; their meaning from the part's datasheet, inferred part)
const MAIN_CTRL: u8 = 0x00;
const MEAS_RATE: u8 = 0x04;
const GAIN: u8 = 0x05;
const PART_ID: u8 = 0x06;
const ALS_DATA: u8 = 0x0d;
/// MAIN_CTRL: ALS_EN. 0 is standby.
const ALS_EN: u8 = 0x02;
/// PART_ID as the light sensor answers it (stock checks it).
pub const LIGHT_ID: u8 = 0xb1;

// LPS22HH registers
const WHO_AM_I: u8 = 0x0f;
const CTRL_REG1: u8 = 0x10;
const CTRL_REG2: u8 = 0x11;
const STATUS: u8 = 0x27;
const PRESS_OUT: u8 = 0x28;
/// WHO_AM_I of the LPS22HH (stock checks it).
pub const BARO_ID: u8 = 0xb3;

/// Which chips answered at start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chips {
    /// The light sensor (PART_ID read back as [`LIGHT_ID`]).
    pub light: bool,
    /// The barometer (WHO_AM_I read back as [`BARO_ID`]).
    pub baro: bool,
}

/// Reads one register; None if the chip did not answer.
fn reg(b: &mut impl Board, addr: u8, r: u8) -> Option<u8> {
    let mut v = [0];
    b.i2c_read(addr, r, &mut v).then_some(v[0])
}

/// Finds the chips and puts each in its lowest-power state: the light sensor in standby, set for 18 bits in 100 ms
/// every 100 ms at gain 6, the barometer powered down with block data update (one-shot conversions
/// only), the accelerometer powered down (CTRL_REG1 0x07: output rate 0, axes enabled, as stock's "off").
pub fn init(b: &mut impl Board) -> Chips {
    let _ = b.i2c_write(ACCEL_ADDR, 0x20, &[0x07]);
    let light = reg(b, LIGHT_ADDR, PART_ID) == Some(LIGHT_ID)
        && b.i2c_write(LIGHT_ADDR, MAIN_CTRL, &[0])
        && b.i2c_write(LIGHT_ADDR, MEAS_RATE, &[0x22]) // 18 bits / 100 ms, every 100 ms while enabled
        && b.i2c_write(LIGHT_ADDR, GAIN, &[0x02]); // gain 6
    let baro = reg(b, BARO_ADDR, WHO_AM_I) == Some(BARO_ID) && b.i2c_write(BARO_ADDR, CTRL_REG1, &[0x02]); // ODR 0, BDU
    Chips { light, baro }
}

/// Starts measuring (the sensor leaves standby): a new reading every 100 ms, the first 100 ms on ([`light_read`]).
pub fn light_start(b: &mut impl Board) -> bool {
    b.i2c_write(LIGHT_ADDR, MAIN_CTRL, &[ALS_EN])
}

/// The ambient light in tenths of a lux (None: no answer): the last conversion. Called at least 100 ms after
/// [`light_start`] and every [`crate::LIGHT_MS`] after, so there always is a new one: MAIN_STATUS (ALS_NEW) is not
/// read first, which halves the bus time (two transactions of about 200 us each, the CPU polling I2C4: 2026-10-26).
/// Does not stop the sensor: [`light_stop`] does.
pub fn light_read(b: &mut impl Board) -> Option<u32> {
    let mut d = [0; 3];
    b.i2c_read(LIGHT_ADDR, ALS_DATA, &mut d).then(|| lux10(u32::from_le_bytes([d[0], d[1], d[2], 0])))
}

/// Back to standby.
pub fn light_stop(b: &mut impl Board) {
    let _ = b.i2c_write(LIGHT_ADDR, MAIN_CTRL, &[0]);
}

/// A light reading (18 bits, gain 6, 100 ms) in tenths of a lux: 0.6 / (gain 6 x 1) = 0.1 lux per count, so the
/// count itself. Full scale 26214 lux, the top of stock's last band, as stock's 20-bit setting.
///
/// ```
/// assert_eq!(bryton_rustfw::ambient::lux10(100), 100); // 10.0 lux
/// assert_eq!(bryton_rustfw::ambient::lux10(0xff_ffff), 262_143); // full scale
/// ```
pub fn lux10(raw: u32) -> u32 {
    raw & 0x3_ffff
}

/// Starts one barometer conversion (one-shot); [`baro_read`] gets it once done (tens of ms).
pub fn baro_start(b: &mut impl Board) -> bool {
    b.i2c_write(BARO_ADDR, CTRL_REG2, &[0x11]) // IF_ADD_INC (its reset value), ONE_SHOT
}

/// The last conversion: (pressure in 1/4096 hPa, the chip's own unit; temperature in C x 100); None if not ready or
/// no answer.
pub fn baro_read_raw(b: &mut impl Board) -> Option<(u32, i16)> {
    if reg(b, BARO_ADDR, STATUS)? & 0x03 != 0x03 {
        return None; // T_DA and P_DA
    }
    let mut d = [0; 5];
    if !b.i2c_read(BARO_ADDR, PRESS_OUT, &mut d) {
        return None;
    }
    let p = i32::from_le_bytes([0, d[0], d[1], d[2]]) >> 8; // 24-bit two's complement, 4096 per hPa
    Some((p.max(0) as u32, i16::from_le_bytes([d[3], d[4]])))
}

/// The last conversion: (pressure in hPa x 10, temperature in C x 100); None if not ready or no answer.
pub fn baro_read(b: &mut impl Board) -> Option<(u32, i16)> {
    baro_read_raw(b).map(|(p, t)| ((p * 10 + 2048) / 4096, t))
}

/// The automatic backlight: ambient light bands (upper edge, tenths of a lux) and the backlight level in each:
/// stock's curve (its setting 3, BOARD.md 16), its bands of 5, 20, 50 and 120 lux, rising with the light indoors and
/// down to the 1 % minimum above 120 lux, where the panel ("high reflectance" TFT, as Bryton sells it) is read by the
/// light it reflects and the backlight adds little but drain. The darkest band is dimmer than stock's 14 % (8 %, its
/// setting 1): in the dark the eye needs little (the owner, 2026-10-23).
// ponytail: one curve; stock offers five (a setting) if someone wants another
pub const AUTO_BANDS: [(u32, u8); 5] = [(50, 8), (200, 40), (500, 50), (1200, 70), (u32::MAX, 1)];

/// Smoothing and hysteresis for [`AUTO_BANDS`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Auto {
    /// The smoothed light, tenths of a lux (None before the first reading).
    avg: Option<u32>,
    band: usize,
}

impl Auto {
    /// Takes one reading (tenths of a lux) and returns the level: the average moves half way to each reading (four a
    /// second: a change of light shows within a second), and the band changes only once the average is a quarter past its edge, so a light that hovers on
    /// an edge keeps one level.
    ///
    /// ```
    /// use bryton_rustfw::ambient::Auto;
    /// let mut a = Auto::default();
    /// assert_eq!(a.update(10), 8); // 1 lux: dark
    /// assert_eq!(a.update(100), 8); // one 10 lux reading: half way, not past the edge's margin
    /// ```
    pub fn update(&mut self, lux10: u32) -> u8 {
        let avg = self.avg.map_or(lux10, |a| (u64::from(a) + u64::from(lux10)).div_ceil(2) as u32);
        let band = AUTO_BANDS.iter().position(|&(top, _)| avg <= top).unwrap_or(AUTO_BANDS.len() - 1);
        let top = |i: usize| AUTO_BANDS.get(i).map_or(u32::MAX, |&(t, _)| t);
        if self.avg.is_none()
            || (band > self.band && avg > top(self.band).saturating_add(top(self.band) / 4))
            || (band < self.band && self.band > 0 && avg < top(self.band - 1) - top(self.band - 1) / 4)
        {
            self.band = band;
        }
        self.avg = Some(avg);
        AUTO_BANDS.get(self.band).map_or(AUTO_BANDS[0].1, |&(_, level)| level)
    }

    /// The smoothed light, tenths of a lux.
    pub fn lux10(&self) -> Option<u32> {
        self.avg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_and_hysteresis() {
        assert!(AUTO_BANDS.windows(2).all(|w| w[0].0 < w[1].0), "edges in order");
        let indoors = &AUTO_BANDS[..AUTO_BANDS.len() - 1];
        assert!(indoors.windows(2).all(|w| w[0].1 <= w[1].1), "indoors: more light, more backlight");
        let mut a = Auto::default();
        assert_eq!(a.update(0), 8);
        for _ in 0..20 {
            a.update(300); // 30 lux
        }
        assert_eq!(a.update(300), 50);
        // hovering on the 50 lux edge (45..55): the level stays
        for i in 0..40 {
            assert_eq!(a.update(if i % 2 == 0 { 450 } else { 550 }), 50, "reading {i}");
        }
        for _ in 0..20 {
            a.update(10_000);
        }
        assert_eq!(a.update(10_000), 1, "daylight: the minimum, the panel reflects it");
        for _ in 0..30 {
            a.update(0);
        }
        assert_eq!(a.update(0), 8, "dark again");
    }
}
