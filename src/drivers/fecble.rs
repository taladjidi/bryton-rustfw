//! FE-C over BLE, the second way a Bluetooth trainer is driven: trainers without FTMS (older Tacx scheme, the
//! Van Rysel D500 and D900 among them) carry the ANT+ FE-C pages (`fec.rs`) inside ANT message frames over one vendor GATT
//! service. This file is the framing and what the advertising says; the pages are `fec.rs`'s, the link is
//! app/ftms.rs.
//!
//! Sources (2026-10-26):
//! - Tacx "FE-C over BLE" (Garmin's Tacx Trainer Service documentation is given on request only,
//!   developer.garmin.com/tacx-trainer/overview names the connection type): the service 6e40fec1-b5a3-f393-e0a9-
//!   e50e24dcca9e, 6e40fec2 (notify, from the trainer) and 6e40fec3 (write, to it); pycycling,
//!   github.com/zacharyedwardbull/pycycling `tacx_trainer_control.py`: a frame is `A4`, a length (9), the ANT
//!   message id (`4F` acknowledged data written, `4E` broadcast notified), a channel byte (5), the 8 page bytes and
//!   a checksum; notifications are read by position (data at byte 4), the page number first. Seen on the owner's
//!   D500 from a computer (2026-10-26): the 128-bit UUID in its advertising, the characteristics above.
//! - The checksum: the ANT message format's is the XOR of all the bytes before it (the sync included). pycycling
//!   computes the sum of the bytes after the sync instead, and does not check what it receives; the trainers it
//!   works with seem to take either or none. So both are accepted here, and the one the trainer's last notification
//!   used is the one written back ([`parse`] says which); XOR until one is heard. Inferred, not seen on the unit.

/// The service's 128-bit UUID, least significant byte first (bytes 12 and 13 are its 16-bit part, [`SERVICE`]), as
/// `ble::uuid_vs_add` takes a base.
pub const BASE: [u8; 16] =
    [0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 0xc1, 0xfe, 0x40, 0x6e];
/// The service's 16-bit part.
pub const SERVICE: u16 = 0xfec1;
/// The trainer's messages (notify).
pub const NOTIFY: u16 = 0xfec2;
/// The messages to the trainer (write).
pub const WRITE: u16 = 0xfec3;

/// A frame's length: sync, length, id, channel, 8 page bytes, checksum.
pub const FRAME: usize = 13;

/// How a Bluetooth trainer is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Proto {
    /// The Fitness Machine Service (`ftms.rs`).
    #[default]
    Ftms,
    /// FE-C pages over the vendor service of this file.
    Fec,
}

/// What a trainer's advertising data says it speaks: FTMS first when it offers both, else FE-C over BLE when the
/// 128-bit service list (AD types 0x06 / 0x07) holds [`BASE`].
///
/// ```
/// use bryton_rustfw::fecble::{self, Proto};
/// let mut ad = vec![2, 0x01, 0x06, 17, 0x07];
/// ad.extend(fecble::BASE);
/// assert_eq!(fecble::advertised(&ad), Some(Proto::Fec));
/// assert_eq!(fecble::advertised(&[5, 0x03, 0x0d, 0x18, 0x26, 0x18]), Some(Proto::Ftms));
/// assert_eq!(fecble::advertised(&ad[..20]), None, "cut short");
/// ```
pub fn advertised(ad: &[u8]) -> Option<Proto> {
    if crate::ftms::advertised(ad) {
        return Some(Proto::Ftms);
    }
    let mut rest = ad;
    while let [len, kind, ..] = *rest {
        let field = rest.get(2..1 + usize::from(len))?;
        if matches!(kind, 0x06 | 0x07) && field.chunks_exact(16).any(|u| u == BASE) {
            return Some(Proto::Fec);
        }
        rest = rest.get(1 + usize::from(len)..)?;
    }
    None
}

/// The checksum of `d` (a frame without its last byte): the ANT message's XOR of everything, or (`sum`) pycycling's
/// sum of everything after the sync byte.
fn checksum(d: &[u8], sum: bool) -> u8 {
    if sum { d.iter().skip(1).fold(0u8, |a, &b| a.wrapping_add(b)) } else { d.iter().fold(0, |a, &b| a ^ b) }
}

/// The frame that writes `page` to the trainer (acknowledged data, channel 5), its checksum as `sum` says.
///
/// ```
/// let f = bryton_rustfw::fecble::frame([0x46, 255, 255, 255, 255, 1, 54, 1], false);
/// assert_eq!(f[..4], [0xa4, 9, 0x4f, 5]);
/// assert_eq!(f[12], f[..12].iter().fold(0, |a, b| a ^ b));
/// ```
pub fn frame(page: [u8; 8], sum: bool) -> [u8; FRAME] {
    let mut f = [0xa4, 9, 0x4f, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    f[4..12].copy_from_slice(&page);
    f[12] = checksum(&f[..12], sum);
    f
}

/// The frame a trainer notifies for `page` (a broadcast, `4E`), its checksum as `sum` says.
pub fn broadcast(page: [u8; 8], sum: bool) -> [u8; FRAME] {
    let mut f = frame(page, sum);
    f[2] = 0x4e;
    f[12] = checksum(&f[..12], sum);
    f
}

/// The page of a notification (a broadcast or acknowledged data message of 9 bytes), and whether its checksum was
/// the sum's (else the XOR's); None for anything else, a wrong checksum included.
///
/// ```
/// use bryton_rustfw::fecble;
/// let page = [25, 1, 85, 0, 0, 0x2c, 0x71, 0x30];
/// let mut f = fecble::broadcast(page, false);
/// assert_eq!(fecble::parse(&f), Some((page, false)));
/// f[5] ^= 1;
/// assert_eq!(fecble::parse(&f), None);
/// ```
pub fn parse(d: &[u8]) -> Option<([u8; 8], bool)> {
    let f = d.get(..FRAME)?;
    if f[0] != 0xa4 || f[1] != 9 || !matches!(f[2], 0x4e | 0x4f) {
        return None;
    }
    let sum = if f[12] == checksum(&f[..12], false) {
        false
    } else if f[12] == checksum(&f[..12], true) {
        true
    } else {
        return None;
    };
    Some((f[4..12].try_into().ok()?, sum))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn frames_round_trip_in_the_checksum_they_use() {
        let page = [16, 25, 40, 12, 0xd0, 0x07, 120, 0x33];
        let f = broadcast(page, true);
        assert_eq!(parse(&f), Some((page, true)));
        let mut f = broadcast(page, false);
        assert_eq!(parse(&f), Some((page, false)));
        assert_eq!(parse(&f[..12]), None, "short");
        f[12] ^= 0x10;
        assert_eq!(parse(&f), None, "bad checksum");
    }

    #[test]
    fn advertising_lists() {
        let mut ad = std::vec![17, 0x06];
        ad.extend(BASE);
        assert_eq!(advertised(&ad), Some(Proto::Fec));
        ad[5] ^= 1;
        assert_eq!(advertised(&ad), None, "another UUID");
        let mut two = std::vec![3, 0x03, 0x18, 0x18, 33, 0x07];
        two.extend([0u8; 16]);
        two.extend(BASE);
        assert_eq!(advertised(&two), Some(Proto::Fec), "the second of a list");
    }
}
