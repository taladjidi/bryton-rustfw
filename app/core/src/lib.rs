//! The phone app's core: turns the raw bytes the bike computer's GATT server sends into what the screen shows.
//! Swift gets it through UniFFI (proc-macro mode, `uniffi::setup_scaffolding!`); the Swift side only moves bytes
//! from CoreBluetooth to here.
//!
//! Sources:
//! - Bluetooth SIG, Battery Service 1.0: Battery Level 0x2A19 is one `uint8`, 0 to 100 percent.
//! - Bluetooth SIG, Device Information Service 1.1: Manufacturer Name String 0x2A29 is a UTF-8 string.
//! - Apple, `CBCentralManager.retrieveConnectedPeripherals(withServices:)` (developer.apple.com/documentation/
//!   corebluetooth/cbcentralmanager/retrieveconnectedperipherals(withservices:)): returns the peripherals the system
//!   holds connected that expose the given services; an app then connects and reads. Used with 0x180F / 0x180A,
//!   the services the unit leaves open (iOS hides HID from apps). That the call finds Battery Service
//!   peripherals bonded by the system is reported by third-party apps, not stated by Apple (AirBattery PR 2,
//!   LucasStop/LogiBattery issue 121): to be confirmed on the phone.

uniffi::setup_scaffolding!();

/// Longest maker string kept, in bytes: a longer value is not from a sane unit.
const MAKER_MAX: usize = 64;

/// What the app knows about the unit; a field is `None` when its bytes were missing or invalid.
#[derive(Debug, PartialEq, Eq, uniffi::Record)]
pub struct UnitStatus {
    /// Manufacturer Name String (0x2A29), e.g. "rustfw".
    pub maker: Option<String>,
    /// Battery Level (0x2A19), 0 to 100.
    pub battery_percent: Option<u8>,
}

/// Battery Level: exactly one byte, at most 100.
pub fn battery_percent(raw: &[u8]) -> Option<u8> {
    match raw {
        [p] if *p <= 100 => Some(*p),
        _ => None,
    }
}

/// Manufacturer Name: UTF-8, trimmed, not empty, at most [`MAKER_MAX`] bytes, no control characters.
pub fn maker(raw: &[u8]) -> Option<String> {
    let s = core::str::from_utf8(raw).ok()?.trim_end_matches('\0').trim();
    (!s.is_empty() && s.len() <= MAKER_MAX && !s.chars().any(char::is_control)).then(|| s.to_owned())
}

/// Decodes the two characteristics' bytes (`None`: not read).
#[uniffi::export]
pub fn decode_status(maker_raw: Option<Vec<u8>>, battery_raw: Option<Vec<u8>>) -> UnitStatus {
    UnitStatus {
        maker: maker_raw.as_deref().and_then(maker),
        battery_percent: battery_raw.as_deref().and_then(battery_percent),
    }
}

/// The 16-bit id of the unit's first vendor UUID, from the firmware's own table: proves the shared code builds here.
#[uniffi::export]
pub fn firmware_vendor_uuid16() -> u16 {
    bryton_rustfw::phone::uuid16(&bryton_rustfw::phone::VS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_bounds() {
        assert_eq!(battery_percent(&[0]), Some(0));
        assert_eq!(battery_percent(&[100]), Some(100));
        assert_eq!(battery_percent(&[101]), None);
        assert_eq!(battery_percent(&[]), None);
        assert_eq!(battery_percent(&[50, 1]), None);
    }

    #[test]
    fn maker_validation() {
        assert_eq!(maker(b"rustfw").as_deref(), Some("rustfw"));
        assert_eq!(maker(b" rustfw\0\0").as_deref(), Some("rustfw"));
        assert_eq!(maker(b""), None);
        assert_eq!(maker(&[0xff, 0xfe]), None);
        assert_eq!(maker(b"a\x07b"), None);
        assert_eq!(maker(&[b'x'; 64]).map(|m| m.len()), Some(64));
        assert_eq!(maker(&[b'x'; 65]), None);
    }

    #[test]
    fn status_never_panics_on_any_bytes() {
        let (mut makers, mut levels) = (0usize, 0usize);
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let pair = decode_status(Some(vec![a, b]), Some(vec![a, b]));
                makers += usize::from(pair.maker.is_some());
            }
            let single = decode_status(Some(vec![a]), Some(vec![a]));
            makers += usize::from(single.maker.is_some());
            levels += usize::from(single.battery_percent.is_some());
        }
        // the loops must reach the accepting branches too, or the test proves nothing about them
        assert!(makers > 0 && levels > 0);
        assert_eq!(decode_status(None, None), UnitStatus { maker: None, battery_percent: None });
        assert_eq!(
            decode_status(Some(b"rustfw".to_vec()), Some(vec![87])),
            UnitStatus { maker: Some("rustfw".into()), battery_percent: Some(87) }
        );
    }

    #[test]
    fn firmware_item() {
        assert_eq!(firmware_vendor_uuid16(), 0xf431);
    }
}
