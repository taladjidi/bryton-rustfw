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
//! - stock itself (Ghidra notes/agent_b2.md, `AntFec_*`): it decodes pages 1, 16, 25, 54, 71, 80, 81, 82 and sends
//!   pages 1 (calibration), 48, 49, 51 and 55, with the same units.
//! - The data request page 70 and the capabilities page 54 (max resistance N in 5-6, modes in the low bits of 7)
//!   are from the profile as developer pages restate it; inferred, not seen on a trainer.
//!
//! Page 26 (torque) is not decoded: the trainer sends power in page 25. Page 17 (settings) neither.

use crate::trainer::{Command, Reading};

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
        }
        81 => {
            r.seen |= seen::P81;
            r.serial = Some(u32::from_le_bytes([d[4], d[5], d[6], d[7]])).filter(|&s| s != u32::MAX);
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
        Command::Resistance(half) => [0x30, 255, 255, 255, 255, 255, 255, half.min(200)],
        Command::Power(q) => {
            let [lo, hi] = q.min(4000).to_le_bytes();
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
    fn user_page() {
        // 75.5 kg, 9.5 kg bike (190 x 0.05), a 668 mm wheel
        let p = encode(Command::User { weight_cg: 7550, bike_50g: 190, wheel_mm: 668 });
        assert_eq!(p, [0x37, 0x7e, 0x1d, 255, 8 | 0xe0, 0x0b, 66, 0]);
    }
}
