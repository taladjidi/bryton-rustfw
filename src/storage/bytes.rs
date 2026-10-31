//! Little-endian fields read from a byte slice, bounds-checked: `None` when the field runs past the end, never a
//! panic. One reader for the unit's own files (`storage::bikes`, `storage::route`, the places and the settings
//! sectors) and the BLE events, so each caller's out-of-range case is the same `None`.

/// The u16 at byte `at` of `b`, little-endian; `None` if fewer than two bytes are there.
///
/// ```
/// use bryton_rustfw::storage::bytes::le16;
/// assert_eq!(le16(&[1, 2, 3], 1), Some(0x0302));
/// assert_eq!(le16(&[1, 2, 3], 2), None);
/// ```
pub fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*b.get(at..)?.first_chunk::<2>()?))
}

/// The u32 at byte `at` of `b`, little-endian; `None` if fewer than four bytes are there.
///
/// ```
/// use bryton_rustfw::storage::bytes::le32;
/// assert_eq!(le32(&[1, 2, 3, 4, 5], 1), Some(0x05040302));
/// assert_eq!(le32(&[1, 2, 3, 4, 5], 2), None);
/// ```
pub fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*b.get(at..)?.first_chunk::<4>()?))
}
