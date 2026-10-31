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

/// Sorts `s` by `key`, smallest first, equal keys kept in order: an insertion sort for the few tens of items the
/// unit sorts (the nRF's nearby devices, 32; FITNESS's rides, 32), with no panic path linked (core's `sort_unstable`
/// keeps one for an `Ord` that is not a total order). Quadratic: only for small slices.
///
/// ```
/// let mut v = [3, 1, 2];
/// bryton_rustfw::storage::bytes::sort_by_key(&mut v, |&x| x);
/// assert_eq!(v, [1, 2, 3]);
/// ```
pub fn sort_by_key<T, K: Ord>(s: &mut [T], key: impl Fn(&T) -> K) {
    for i in 1..s.len() {
        for j in (1..=i).rev() {
            match s.get_mut(j - 1..=j) {
                Some([a, b]) if key(b) < key(a) => core::mem::swap(a, b),
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::sort_by_key;

    #[test]
    fn sort_by_key_matches_core_sort() {
        let mut x = 0x1234_5678u32;
        for n in 0..40 {
            let mut v: std::vec::Vec<(u8, u16)> = (0..n)
                .map(|i| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    ((x % 7) as u8, i)
                })
                .collect();
            let mut want = v.clone();
            want.sort_by_key(|p| p.0); // stable, as ours
            sort_by_key(&mut v, |p| p.0);
            assert_eq!(v, want, "n {n}");
        }
    }
}
