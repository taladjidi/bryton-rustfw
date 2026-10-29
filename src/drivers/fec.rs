//! ANT+ FE-C, the Fitness Equipment profile's trainer side (device type 17, 8192 / 32768 s = 4 Hz, 2457 MHz, as
//! [`crate::sensors::Kind::Trainer`] opens it): the data pages a trainer sends and the control pages the head unit
//! sends it, as bytes. What they mean for the ride is `trainer.rs`; this file only turns bytes into a
//! [`Reading`] and a [`Command`] into bytes, so BLE FTMS can be a second file like it.
//!
//! Sources (2026-10-25; the profile document itself, thisisant.com "ANT+ Device Profile - Fitness Equipment", is
//! behind the ANT+ members' login, so the layouts are what two open implementations and stock agree on):
//! - openant, github.com/Tigge/openant `openant/devices/fitness_equipment.py`: pages 16, 17, 25, 26, 71 (command
//!   status: byte 1 the page of the command, byte 3 its status), the basic resistance (page 48, 0.5 % units in
//!   byte 7) and target power (page 49, 0.25 W units in bytes 6-7) messages.
//! - ant-plus, github.com/Loghorn/ant-plus `src/fitness-equipment-sensors.ts`: page 16 (speed in 0.001 m/s in 4-5;
//!   heart rate in 6, its source in byte 7 bits 0-1, 0 invalid), page 25 (cadence in 2, power in the low 12 bits of
//!   5-6, 0xfff invalid, the status in the upper nibble of 6, in byte 7 the target-power flags in bits 0-1 and the
//!   state in bits 4-6), page 51 (track resistance: grade in 0.01 % with an offset of 200 % in 5-6, the rolling
//!   resistance coefficient in 5e-5 units in 7) and page 55 (user configuration: weight 0.01 kg in 1-2, bike weight
//!   0.05 kg in 12 bits over the upper nibble of 4 and byte 5, wheel diameter in cm in 6 with the millimetres in the
//!   low nibble of 4, gear ratio 0.03 in 7, 0 invalid; that library masks the bike weight's high bits, the 12 bits
//!   are used whole here).
//! - openant `openant/devices/common.py`: common pages 80 (hardware revision in 3, manufacturer in 4-5, model number
//!   in 6-7) and 81 (software revision, supplemental in 2 (0xff unused) and main in 3; serial number in 4-7).
//! - stock itself (Ghidra notes/agent_b2.md, `AntFec_*`): it decodes pages 1, 16, 25, 54, 71, 80, 81, 82 and sends
//!   pages 1 (calibration), 48, 49, 51 and 55, with the same units.
//! - The data request page 70 and the capabilities page 54 (max resistance N in 5-6, modes in the low bits of 7)
//!   are from the profile as developer pages restate it; inferred, not seen on a trainer.
//!
//! - Calibration (pages 1 and 2), GoldenCheetah, github.com/GoldenCheetah/GoldenCheetah `src/ANT/ANTMessage.cpp`
//!   (`fecRequestCalibration`, the decoder, citing "ANT+ Device Profile Fitness Equipment, Rev 4.1, 6.4.1 Data Page 1")
//!   and `ANTChannel.cpp` / `ANT.h`. **Page 1, the request** (head unit to trainer, acknowledged): byte 1 the
//!   calibration asked, 0x40 zero offset, 0x80 spin-down; byte 2 reserved 0; byte 3 temperature, 4-5 zero offset, 6-7
//!   spin-down time, all 0xff (invalid) in a request. **Page 1, the response** (trainer, at the end): byte 1 the
//!   same two bits, set for a calibration that succeeded (GoldenCheetah takes either bit as success); byte 3 the
//!   temperature (0.5 degree units from -25, 0xff invalid); 4-5 the zero offset; 6-7 the spin-down time in ms.
//!   **Page 2, in progress** (trainer, repeated during the calibration): byte 1 the calibrations pending (the same
//!   two bits); byte 2 the conditions: bits 4-5 the temperature (1 too low, 2 ok, 3 too high), bits 6-7 the speed (1
//!   too low, 2 ok); byte 3 the temperature; 4-5 the target speed, 0.001 m/s (0xffff invalid); 6-7 the target
//!   spin-down time, ms. The trainer asks for the calibration in page 25's status nibble (bit 0 power / zero offset,
//!   bit 1 resistance / spin-down). Inferred, not in the open sources: that both bits may be asked in one request
//!   (this unit asks for the spin-down, and the zero offset only when the trainer asks for it), and that a trainer
//!   leaves the calibration by itself when no request follows (nothing is sent to cancel).
//!
//! Page 26 (torque) is not decoded: the trainer sends power in page 25. Page 17 (settings) neither.

use crate::trainer::{Command, MAX_W, Reading};

/// Bits of [`Reading::seen`]: the pages decoded.
pub mod seen {
    /// Page 16, general FE data.
    pub const P16: u8 = 1;
    /// Page 25, trainer-specific data.
    pub const P25: u8 = 2;
    /// Page 54, capabilities.
    pub const P54: u8 = 4;
    /// Page 71, command status.
    pub const P71: u8 = 8;
    /// Page 80, manufacturer.
    pub const P80: u8 = 16;
    /// Page 81, product.
    pub const P81: u8 = 32;
    /// Page 1, calibration response.
    pub const P1: u8 = 64;
    /// Page 2, calibration in progress.
    pub const P2: u8 = 128;
}

/// Takes the 8 data bytes of a broadcast from the trainer into `r`; pages not listed in the module doc change
/// nothing.
///
/// ```
/// use bryton_rustfw::{fec, trainer::Reading};
/// let mut r = Reading::default();
/// fec::decode([25, 7, 90, 0, 0, 0xc8, 0x00, 0x30], &mut r);
/// assert_eq!((r.power, r.cadence, r.state), (Some(200), Some(90), 3));
/// ```
pub fn decode(d: [u8; 8], r: &mut Reading) {
    let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    match d[0] {
        // general FE data: speed in mm/s (0xffff: none), the heart rate with its source in byte 7
        16 => {
            r.seen |= seen::P16;
            r.speed_x10 = Some(u16_at(4)).filter(|&v| v != 0xffff).map(|v| (u32::from(v) * 36 / 1000) as u16);
            r.hr = Some(d[6]).filter(|&h| h != 0 && h != 0xff && d[7] & 3 != 0);
        }
        // trainer data: cadence, power (12 bits), status, target flags and state
        25 => {
            r.seen |= seen::P25;
            r.cadence = Some(d[2]).filter(|&c| c != 0xff);
            r.power = Some(u16_at(5) & 0x0fff).filter(|&p| p != 0x0fff);
            r.status = d[6] >> 4 & 7;
            r.limits = d[7] & 3;
            r.state = d[7] >> 4 & 7;
        }
        // the calibration's response: what succeeded, the spin-down time
        1 => {
            r.seen |= seen::P1;
            r.cal_result = Some((d[1] & 0xc0, Some(u16_at(6)).filter(|&v| v != 0xffff)));
            r.cal_results = r.cal_results.wrapping_add(1);
        }
        // the calibration in progress: what is pending, the conditions, the target speed and spin-down time
        2 => {
            r.seen |= seen::P2;
            r.cal_pending = d[1] & 0xc0;
            r.cal_cond = d[2];
            r.cal_target_x10 = Some(u16_at(4)).filter(|&v| v != 0xffff).map(|v| (u32::from(v) * 36 / 1000) as u16);
            r.cal_target_ms = Some(u16_at(6)).filter(|&v| v != 0xffff);
            r.cal_pages = r.cal_pages.wrapping_add(1);
        }
        54 => {
            r.seen |= seen::P54;
            r.max_resistance = Some(u16_at(5)).filter(|&m| m != 0xffff);
            r.modes = Some(d[7] & 7);
        }
        71 => {
            r.seen |= seen::P71;
            r.last_cmd = Some((d[1], d[3]));
        }
        80 => {
            r.seen |= seen::P80;
            r.maker = Some(u16_at(4));
            r.hw = Some(d[3]).filter(|&h| h != 0xff);
            r.model = Some(u16_at(6)).filter(|&m| m != 0xffff);
        }
        81 => {
            r.seen |= seen::P81;
            r.serial = Some(u32::from_le_bytes([d[4], d[5], d[6], d[7]])).filter(|&s| s != u32::MAX);
            // the version is main / 10, or (main x 100 + supplemental) / 1000 when byte 2 is not 0xff (openant
            // common.py): x 100 here, FIT's software_version scale
            let (supp, main) = (u16::from(d[2]), u16::from(d[3]));
            r.sw_x100 = (main != 0xff).then_some(if supp == 0xff { main * 10 } else { (main * 100 + supp) / 10 });
        }
        _ => {}
    }
}

/// The control page for `c`: 48 basic resistance, 49 target power, 51 track resistance, 55 user configuration.
///
/// ```
/// use bryton_rustfw::{fec, trainer::Command};
/// assert_eq!(fec::encode(Command::Power(800)), [0x31, 255, 255, 255, 255, 255, 0x20, 0x03]);
/// assert_eq!(fec::encode(Command::Track { grade_x100: -150, crr: 80 })[5..], [0x8a, 0x4d, 80]);
/// ```
pub fn encode(c: Command) -> [u8; 8] {
    match c {
        // zero offset and spin-down bits in byte 1; the rest invalid
        Command::Calibrate(req) => [0x01, req & 0xc0, 0, 255, 255, 255, 255, 255],
        Command::Resistance(half) => [0x30, 255, 255, 255, 255, 255, 255, half.min(200)],
        Command::Power(q) => {
            let [lo, hi] = q.min(MAX_W * 4).to_le_bytes(); // page 49's field holds 0.25 W units to 16383 W
            [0x31, 255, 255, 255, 255, 255, lo, hi]
        }
        Command::Track { grade_x100, crr } => {
            // grade + 200 % in 0.01 % units
            let g = (i32::from(grade_x100) + 20_000).clamp(0, 40_000) as u16;
            let [lo, hi] = g.to_le_bytes();
            [0x33, 255, 255, 255, 255, lo, hi, crr]
        }
        Command::User { weight_cg, bike_50g, wheel_mm } => {
            let [w0, w1] = weight_cg.to_le_bytes();
            let bike = bike_50g.min(0xffe);
            let (cm, mm) = (wheel_mm / 10, wheel_mm % 10);
            [0x37, w0, w1, 255, (mm as u8) | (bike as u8 & 0xf) << 4, (bike >> 4) as u8, cm.min(254) as u8, 0]
        }
    }
}

/// The request for data page `page` (common page 70, one transmission asked for, not acknowledged).
///
/// ```
/// assert_eq!(bryton_rustfw::fec::request(54), [0x46, 255, 255, 255, 255, 1, 54, 1]);
/// ```
pub fn request(page: u8) -> [u8; 8] {
    [0x46, 255, 255, 255, 255, 1, page, 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages() {
        let mut r = Reading::default();
        decode([16, 25, 40, 12, 0xd0, 0x07, 120, 0x33], &mut r);
        assert_eq!(r.speed_x10, Some(72), "2000 mm/s is 7.2 km/h");
        assert_eq!(r.hr, Some(120));
        decode([16, 25, 0, 0, 0xff, 0xff, 0xff, 0x30], &mut r);
        assert_eq!((r.speed_x10, r.hr), (None, None), "invalid values");
        decode([25, 1, 0xff, 0, 0, 0xff, 0xff, 0x12], &mut r);
        assert_eq!((r.power, r.cadence, r.limits, r.state), (None, None, 2, 1));
        decode([25, 1, 85, 0, 0, 0x2c, 0x71, 0x30], &mut r);
        assert_eq!((r.power, r.status, r.needs_user(), r.needs_calibration()), (Some(300), 7, true, true));
        decode([54, 255, 255, 255, 255, 0xe8, 0x03, 0x07], &mut r);
        assert_eq!((r.max_resistance, r.modes), (Some(1000), Some(7)));
        decode([71, 0x31, 0, 0, 255, 255, 0x20, 0x03], &mut r);
        assert_eq!(r.last_cmd, Some((0x31, 0)));
        assert_eq!(r.seen, seen::P16 | seen::P25 | seen::P54 | seen::P71);
    }

    #[test]
    fn calibration_pages() {
        let mut r = Reading::default();
        // speed too low (condition 1), target 9.723 m/s = 35.0 km/h, spin-down 3000 ms
        decode([2, 0x80, 0x60, 90, 0xfb, 0x25, 0xb8, 0x0b], &mut r);
        assert_eq!(
            (r.cal_pending, r.cal_cond >> 6, r.cal_target_x10, r.cal_target_ms),
            (0x80, 1, Some(350), Some(3000))
        );
        decode([2, 0x80, 0xa0, 90, 0xff, 0xff, 0xff, 0xff], &mut r);
        assert_eq!((r.cal_cond >> 6, r.cal_target_x10, r.cal_target_ms, r.cal_pages), (2, None, None, 2));
        decode([1, 0x80, 0, 90, 0xff, 0xff, 0xc4, 0x09], &mut r);
        assert_eq!((r.cal_result, r.cal_results), (Some((0x80, Some(2500))), 1));
        assert_eq!(r.seen, seen::P1 | seen::P2);
        assert_eq!(encode(Command::Calibrate(0xff)), [1, 0xc0, 0, 255, 255, 255, 255, 255], "only the two bits");
    }

    #[test]
    fn user_page() {
        // 75.5 kg, 9.5 kg bike (190 x 0.05), a 668 mm wheel
        let p = encode(Command::User { weight_cg: 7550, bike_50g: 190, wheel_mm: 668 });
        assert_eq!(p, [0x37, 0x7e, 0x1d, 255, 8 | 0xe0, 0x0b, 66, 0]);
    }

    /// The target power goes to the trainer's own limit ([`MAX_W`]), not to 1000 W (review of ecbe541).
    #[test]
    fn power_up_to_max_w() {
        let page = |q| u16::from_le_bytes([encode(Command::Power(q))[6], encode(Command::Power(q))[7]]);
        assert_eq!(page(MAX_W * 4), 8000, "2000 W in quarter watts");
        assert_eq!(page(5000), 5000, "1250 W is not cut at 1000 W");
        assert_eq!(page(u16::MAX), MAX_W * 4, "and no further");
    }
}
