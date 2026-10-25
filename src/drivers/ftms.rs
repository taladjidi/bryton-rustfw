//! Bluetooth FTMS, the Fitness Machine Service (0x1826), a smart trainer's side: what its advertising says, its
//! characteristics' values as a [`Reading`], and a [`Command`] as a Control Point write. The sibling of `fec.rs`
//! (ANT+ FE-C): what the values mean for the ride is `trainer.rs`, the link (connect, discover, subscribe, control)
//! is app/ftms.rs.
//!
//! Sources (2026-10-25):
//! - Bluetooth SIG, Fitness Machine Service 1.0 (FTMS_v1.0.pdf, bluetooth.com/specifications/specs): the Service
//!   Data AD type (3.1: flags, bit 0 machine available; machine type, bit 5 indoor bike), Indoor Bike Data (4.9: the
//!   flags and the fields in their order; More Data, bit 0, set means the speed is not in this notification), the
//!   Control Point (4.16: op codes 0x00 request control, 0x04 target resistance level u8 in 0.1, 0x05 target power
//!   s16 W, 0x11 indoor bike simulation: wind s16 0.001 m/s, grade s16 0.01 %, rolling resistance u8 0.0001, wind
//!   resistance u8 0.01 kg/m; the response 0x80, the request's op code, the result: 1 success, 2 not supported, 3
//!   invalid parameter, 4 failed, 5 control not permitted; one procedure at a time), Fitness Machine Status (4.17:
//!   0xff control permission lost, 0x01 reset), Fitness Machine Feature (4.3: two u32, the second the target settings:
//!   bit 2 resistance, bit 3 power, bit 13 indoor bike simulation), the supported ranges (4.13, 4.14: minimum,
//!   maximum, increment, each s16; the resistance in 0.1).
//! - The field sizes (u16 speed in 0.01 km/h, u16 cadence in 0.5 rpm, u24 distance, s16 power, energy u16 + u16 +
//!   u8, u8 heart rate...) are the GATT Specification Supplement's; pycycling (github.com/zacharyedwardbull/pycycling,
//!   `ftms_parsers/indoor_bike_data.py`) reads them the same way, and reads the cadence bit (2) as present when set.
//!   Checked against the specification text (FTMS 1.0, table 4.10): every flag but two is "field present when set";
//!   More Data (bit 0) is the other way round, and so is Instantaneous Cadence (bit 2: "fields present" when 0, "not
//!   present" when 1), which no trainer does (pycycling's users: inferred, not seen here).
//! - The errors a Control Point write can get (CSS Part B 1.2): 0xfd, its CCCD not set for indications; 0xfe, a
//!   procedure already in progress. The SoftDevice gives them as GATT status 0x0100 + the ATT error.

use crate::trainer::{Command, Reading};

/// The service.
pub const SERVICE: u16 = 0x1826;
/// Fitness Machine Feature (read).
pub const FEATURE: u16 = 0x2acc;
/// Indoor Bike Data (notify).
pub const BIKE_DATA: u16 = 0x2ad2;
/// Supported Resistance Level Range (read).
pub const RESISTANCE_RANGE: u16 = 0x2ad6;
/// Supported Power Range (read).
pub const POWER_RANGE: u16 = 0x2ad8;
/// Fitness Machine Control Point (write, indicate).
pub const CONTROL: u16 = 0x2ad9;
/// Fitness Machine Status (notify).
pub const STATUS: u16 = 0x2ada;

/// GATT status of a Control Point write refused because its CCCD is not set for indications.
pub const CCCD_NOT_SET: u16 = 0x01fd;

/// Request Control: the first write, before any target.
pub const REQUEST_CONTROL: [u8; 1] = [0x00];

/// Whether advertising data `ad` is an FTMS indoor bike's: its Service Data (AD type 0x16, UUID 0x1826, flags with
/// bit 0 "available", the machine type with bit 5 "indoor bike"); or, from a trainer that leaves the service data
/// out, 0x1826 in its list of 16-bit service UUIDs (AD types 0x02 / 0x03).
///
/// ```
/// use bryton_rustfw::ftms::advertised;
/// assert!(advertised(&[2, 0x01, 0x06, 6, 0x16, 0x26, 0x18, 0x01, 0x20, 0x00]));
/// assert!(!advertised(&[6, 0x16, 0x26, 0x18, 0x01, 0x01, 0x00]), "a treadmill");
/// assert!(advertised(&[5, 0x03, 0x0d, 0x18, 0x26, 0x18]));
/// assert!(!advertised(&[3, 0x03, 0x0d, 0x18]));
/// ```
pub fn advertised(ad: &[u8]) -> bool {
    let mut rest = ad;
    while let [len, kind, ..] = *rest {
        let Some(field) = rest.get(2..1 + usize::from(len)) else { return false };
        match (kind, field) {
            (0x16, [0x26, 0x18, flags, t0, t1, ..]) => {
                return flags & 1 != 0 && u16::from_le_bytes([*t0, *t1]) & 0x20 != 0;
            }
            (0x02 | 0x03, list) if list.chunks_exact(2).any(|u| u == SERVICE.to_le_bytes()) => return true,
            _ => {}
        }
        rest = rest.get(1 + usize::from(len)..).unwrap_or(&[]);
    }
    false
}

/// One Indoor Bike Data notification into `r`: the speed, cadence, power and heart rate it carries (a record split
/// over several notifications, More Data, updates each from the one that has it).
///
/// ```
/// use bryton_rustfw::{ftms, trainer::Reading};
/// let mut r = Reading::default();
/// // speed 30.00 km/h, cadence 90 rpm, power 250 W
/// ftms::bike_data(&[0x44, 0x00, 0xb8, 0x0b, 180, 0, 250, 0], &mut r);
/// assert_eq!((r.speed_x10, r.cadence, r.power), (Some(300), Some(90), Some(250)));
/// ```
pub fn bike_data(d: &[u8], r: &mut Reading) {
    let Some(&[f0, f1]) = d.get(..2) else { return };
    let flags = u16::from_le_bytes([f0, f1]);
    let mut at = 2;
    // each field in the specification's order: (flag bit, present when set, its size)
    let fields: [(u8, bool, usize); 13] = [
        (0, false, 2), // instantaneous speed
        (1, true, 2),  // average speed
        (2, true, 2),  // instantaneous cadence
        (3, true, 2),  // average cadence
        (4, true, 3),  // total distance
        (5, true, 2),  // resistance level
        (6, true, 2),  // instantaneous power
        (7, true, 2),  // average power
        (8, true, 5),  // expended energy: total, per hour, per minute
        (9, true, 1),  // heart rate
        (10, true, 1), // metabolic equivalent
        (11, true, 2), // elapsed time
        (12, true, 2), // remaining time
    ];
    for (bit, when, size) in fields {
        if (flags >> bit & 1 != 0) != when {
            continue;
        }
        let Some(v) = d.get(at..at + size) else { return }; // cut short: what came before stands
        at += size;
        let u = u16::from_le_bytes([v[0], *v.get(1).unwrap_or(&0)]);
        match bit {
            0 => r.speed_x10 = Some(u / 10),
            2 => r.cadence = Some((u / 2).min(254) as u8),
            6 => r.power = Some((u as i16).max(0) as u16),
            9 => r.hr = Some(v[0]).filter(|&h| h != 0),
            _ => {}
        }
    }
}

/// The Fitness Machine Feature value: the modes it can be driven in, as [`Reading::modes`] has them (bit 0
/// resistance, 1 power, 2 simulation).
///
/// ```
/// let feature = [0x02, 0x40, 0, 0, 0x0c, 0x20, 0, 0]; // cadence, power; resistance, power and simulation targets
/// assert_eq!(bryton_rustfw::ftms::modes(&feature), Some(7));
/// ```
pub fn modes(d: &[u8]) -> Option<u8> {
    let t = u32::from_le_bytes(d.get(4..8)?.try_into().ok()?);
    Some(u8::from(t & 1 << 2 != 0) | u8::from(t & 1 << 3 != 0) << 1 | u8::from(t & 1 << 13 != 0) << 2)
}

/// A supported range's minimum and maximum (raw, as the trainer gives them).
pub fn range(d: &[u8]) -> Option<(i16, i16)> {
    match *d {
        [a, b, c, e, ..] => Some((i16::from_le_bytes([a, b]), i16::from_le_bytes([c, e]))),
        _ => None,
    }
}

/// A Fitness Machine Status notification that takes the control away: control permission lost (0xff), or a reset
/// (0x01, after which the machine waits to be taken over again).
pub fn control_lost(d: &[u8]) -> bool {
    matches!(d.first(), Some(0xff | 0x01))
}

/// A Control Point indication: the response (0x80) to op code `op` with its result.
///
/// ```
/// assert_eq!(bryton_rustfw::ftms::response(&[0x80, 0x05, 0x01]), Some((0x05, 0x01)));
/// assert_eq!(bryton_rustfw::ftms::response(&[0x05, 0x01]), None);
/// ```
pub fn response(d: &[u8]) -> Option<(u8, u8)> {
    match *d {
        [0x80, op, result, ..] => Some((op, result)),
        _ => None,
    }
}

/// A response as FE-C's command status page has it ([`Reading::last_cmd`]: the page of the command, the status 0
/// done, 1 failed, 2 not supported, 3 rejected), so the trainer screen says the same for either link.
///
/// ```
/// assert_eq!(bryton_rustfw::ftms::as_fec((0x05, 1)), Some((0x31, 0)), "target power, done");
/// assert_eq!(bryton_rustfw::ftms::as_fec((0x11, 5)), Some((0x33, 3)), "simulation, not permitted");
/// ```
pub fn as_fec((op, result): (u8, u8)) -> Option<(u8, u8)> {
    let page = match op {
        0x04 => 0x30,
        0x05 => 0x31,
        0x11 => 0x33,
        _ => return None,
    };
    let status = match result {
        1 => 0,
        2 => 2,
        4 => 1,
        _ => 3,
    };
    Some((page, status))
}

/// Wind resistance coefficient sent in SIM, kg/m x 100: 0.51, a rider on the hoods (Zwift's and Garmin's default
/// for FE-C's page 50, inferred: FTMS has no default).
pub const CW: u8 = 51;

/// The Control Point write for `c` (its bytes and length), None for what FTMS has no op code for (the rider's
/// weight: FTMS takes it from the User Data Service, not used here). `resistance` is the trainer's supported range
/// in 0.1 units (its Supported Resistance Level Range), which the percent of [`Command::Resistance`] is spread over.
///
/// ```
/// use bryton_rustfw::{ftms, trainer::Command};
/// let w = |c| ftms::encode(c, (0, 200)).map(|(b, n)| b[..n].to_vec());
/// assert_eq!(w(Command::Power(800)), Some(vec![0x05, 200, 0]), "200 W");
/// assert_eq!(w(Command::Resistance(100)), Some(vec![0x04, 100]), "50 % of 0 to 20.0");
/// // -1.5 %, Crr 0.004 (80 x 5e-5 is 40 x 1e-4)
/// assert_eq!(w(Command::Track { grade_x100: -150, crr: 80 }), Some(vec![0x11, 0, 0, 0x6a, 0xff, 40, 51]));
/// ```
pub fn encode(c: Command, (lo, hi): (i16, i16)) -> Option<([u8; 7], usize)> {
    let mut b = [0; 7];
    let n = match c {
        Command::Power(q) => {
            let [p0, p1] = (q / 4).to_le_bytes();
            b[..3].copy_from_slice(&[0x05, p0, p1]);
            3
        }
        Command::Resistance(half) => {
            let (lo, hi) = (i32::from(lo), i32::from(hi.max(lo)));
            let level = lo + (hi - lo) * i32::from(half.min(200)) / 200;
            // the level is a u8 in 0.1 units (FTMS 1.0, 4.16): 25.5 at most, whatever range the trainer reports
            b[..2].copy_from_slice(&[0x04, level.clamp(0, 255) as u8]);
            2
        }
        Command::Track { grade_x100, crr } => {
            let [g0, g1] = grade_x100.to_le_bytes();
            b = [0x11, 0, 0, g0, g1, crr / 2, CW];
            7
        }
        Command::User { .. } | Command::Calibrate(_) => return None, // FE-C pages only
    };
    Some((b, n))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// A record split in two (More Data): the first without the speed, the second with it; each updates what it has,
    /// and a notification cut short leaves the rest as it was.
    #[test]
    fn split_records_and_short_ones() {
        let mut r = Reading::default();
        // More Data, cadence 80 rpm, total distance 1234 m, heart rate 140
        bike_data(&[0x15, 0x02, 160, 0, 0xd2, 0x04, 0x00, 140], &mut r);
        assert_eq!((r.speed_x10, r.cadence, r.hr), (None, Some(80), Some(140)));
        // speed 25.5 km/h, power -3 W (coasting, a brake): 0
        bike_data(&[0x40, 0x00, 0xf6, 0x09, 0xfd, 0xff], &mut r);
        assert_eq!((r.speed_x10, r.power, r.cadence), (Some(255), Some(0), Some(80)));
        bike_data(&[0x40, 0x00, 0x10, 0x27, 0xc8], &mut r);
        assert_eq!((r.speed_x10, r.power), (Some(1000), Some(0)), "the power cut short: not read");
        bike_data(&[0x40], &mut r);
        // every field: the heart rate is past the energy's 5 bytes
        let mut all = std::vec![0xfe, 0x1f];
        all.extend([1, 0, 2, 0, 3, 0, 4, 0, 0, 0, 0, 5, 0, 200, 0, 6, 0, 0, 0, 0, 0, 0, 150, 9, 1, 0, 2, 0]);
        bike_data(&all, &mut r);
        assert_eq!((r.cadence, r.power, r.hr), (Some(1), Some(200), Some(150)));
    }

    #[test]
    fn resistance_spread_over_the_range() {
        let level = |half, range| encode(Command::Resistance(half), range).map(|(b, _)| b[1]);
        assert_eq!(level(200, (10, 100)), Some(100));
        assert_eq!(level(0, (10, 100)), Some(10));
        assert_eq!(level(40, (0, 1000)), Some(200), "a range past the u8: 20 %");
        assert_eq!(level(200, (0, 1000)), Some(255), "clamped");
        assert_eq!(level(100, (50, -5)), Some(50), "a range upside down: its minimum");
        assert_eq!(encode(Command::User { weight_cg: 1, bike_50g: 1, wheel_mm: 1 }, (0, 1)), None);
    }

    #[test]
    fn advertising_cut_short() {
        let ad = [2, 0x01, 0x06, 6, 0x16, 0x26, 0x18, 0x01, 0x20, 0x00];
        for n in 0..ad.len() {
            assert!(!advertised(&ad[..n]), "cut at {n}");
        }
        assert!(!advertised(&[6, 0x16, 0x26, 0x18, 0x00, 0x20, 0x00]), "not available");
    }
}
