//! Little-endian fields read from a byte slice, bounds-checked: `None` when the field runs past the end, never a
//! panic. One reader for the unit's own files (`storage::bikes`, `storage::route`, the places and the settings
//! sectors) and the BLE events, so each caller's out-of-range case is the same `None`.

/// Copies the printable ASCII of `src` into the name buffer `dst` (others become '?'), as long as `dst` holds, and
/// sets `len`. True when the name changed. The one copy of the loop behind every fixed name buffer (a scanned
/// device's, a trainer's, a course's).
///
/// ```
/// use bryton_rustfw::storage::bytes::{ascii, set_ascii};
/// let (mut buf, mut len) = ([0; 4], 0);
/// assert!(set_ascii(&mut buf, &mut len, b"Kick\xffr"));
/// assert_eq!(ascii(&buf, len), "Kick");
/// assert!(!set_ascii(&mut buf, &mut len, b"Kick"), "same name");
/// set_ascii(&mut buf, &mut len, b"a\x01");
/// assert_eq!(ascii(&buf, len), "a?");
/// ```
pub fn set_ascii(dst: &mut [u8], len: &mut u8, src: &[u8]) -> bool {
    let n = src.len().min(dst.len());
    let mut changed = usize::from(*len) != n;
    for (slot, &c) in dst.iter_mut().zip(src) {
        let c = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
        changed |= *slot != c;
        *slot = c;
    }
    *len = n as u8;
    changed
}

/// The name in the first `len` bytes of `buf`, empty when it is not UTF-8 or `len` runs past `buf`.
pub fn ascii(buf: &[u8], len: u8) -> &str {
    core::str::from_utf8(buf.get(..usize::from(len)).unwrap_or(&[])).unwrap_or("")
}

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
