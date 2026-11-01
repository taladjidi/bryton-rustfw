//! Files: the FAT32 volume on the eMMC (read, and written by the ride recorder only), the FIT ride file (written
//! and read), the recorder, the IGN map packs (read), and what the stock firmware left on the volume (its rides,
//! settings, planned workouts and courses).

pub mod bikes;
pub mod bytes;
pub mod copies;
pub mod course_file;
pub mod fat;
pub mod faultlog;
pub mod fit;
pub mod fitread;
pub mod history;
pub mod ign;
pub mod inflate;
pub mod ini;
pub mod named;
pub mod osm;
pub mod recorder;
pub mod route;
pub mod screenshot;
pub mod stock;
pub mod thresholds;
pub mod trash;
pub mod volume;
pub mod workout_file;

/// `s.copy_within(src, dest)` that cannot panic: elements out of range are skipped (an edit on malformed counts
/// changes less, never takes the unit down). Overlap-safe like the original.
pub(crate) fn move_within<T: Copy>(s: &mut [T], src: core::ops::Range<usize>, dest: usize) {
    let n = src.end.saturating_sub(src.start);
    let mut cp = |i: usize| {
        if let Some(&v) = s.get(src.start.saturating_add(i))
            && let Some(d) = s.get_mut(dest.saturating_add(i))
        {
            *d = v;
        }
    };
    if dest <= src.start {
        (0..n).for_each(&mut cp);
    } else {
        (0..n).rev().for_each(&mut cp);
    }
}

/// The byte index of the last ASCII `b` in `s` (`str::rfind` pulls in core's memrchr and its bounds checks).
pub(crate) fn rfind_ascii(s: &str, b: u8) -> Option<usize> {
    s.bytes().rposition(|c| c == b)
}

/// Seals a record sector (recorder/rec.rs, trash.rs): the FIT CRC-16 of its first 510 bytes in its last two.
pub(crate) fn seal16(s: &mut [u8; fat::SECTOR]) {
    let crc = fit::crc(0, &s[..fat::SECTOR - 2]);
    s[fat::SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
}

/// True if `s` holds its seal ([`seal16`]): a sector torn or never written does not.
pub(crate) fn sealed16(s: &[u8; fat::SECTOR]) -> bool {
    fit::crc(0, &s[..fat::SECTOR - 2]).to_le_bytes() == s[fat::SECTOR - 2..]
}

/// The `N` bytes of `b` from `i`, zeros when `b` is too short (a malformed record reads as empty, never panics).
pub(crate) fn take<const N: usize>(b: &[u8], i: usize) -> [u8; N] {
    i.checked_add(N).and_then(|end| b.get(i..end)).and_then(|s| s.try_into().ok()).unwrap_or([0; N])
}

/// The pack holding tile (`z`, `x`, `y`) of a pack family (ign.rs, osm.rs): `<prefix><z>_<x / block>_<y / block>.PAK`.
pub(crate) fn pack_name(prefix: &str, block: u32, z: u8, x: u32, y: u32) -> crate::text::Line<32> {
    let mut l = crate::text::Line::new();
    l.push_str(prefix);
    l.push_u32(u32::from(z));
    l.push_str("_");
    l.push_u32(x / block);
    l.push_str("_");
    l.push_u32(y / block);
    l.push_str(".PAK");
    l
}

/// A pack's 16-byte header is a version 1 pack of `magic` for tile (`z`, `x`, `y`)'s block of `block` tiles a side.
pub(crate) fn pack_header_ok(h: &[u8; 16], magic: &[u8; 4], block: u32, z: u8, x: u32, y: u32) -> bool {
    let u32_at = |i: usize| u32::from_le_bytes(take(h, i));
    h[..4] == *magic
        && h[4..6] == [1, 0]
        && h[6] == z
        && u32::from(h[7]) == block
        && u32_at(8) == x / block * block
        && u32_at(12) == y / block * block
}

/// Where tile (`x`, `y`)'s index entry of `size` bytes is in its pack, after the 16-byte header.
pub(crate) fn pack_entry_at(size: u32, block: u32, x: u32, y: u32) -> u32 {
    16 + size * ((y % block) * block + x % block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_within_matches_copy_within_both_overlaps() {
        for (src, dest) in [(2..7, 0), (0..5, 3), (3..8, 3), (1..4, 6)] {
            let mut a: [u8; 10] = core::array::from_fn(|i| i as u8);
            let mut b = a;
            a.copy_within(src.clone(), dest);
            move_within(&mut b, src, dest);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn move_within_skips_what_is_out_of_range() {
        let mut a = [1u8, 2, 3, 4];
        move_within(&mut a, 2..9, 1); // the source runs past the end: only 3, 4 exist
        assert_eq!(a, [1, 3, 4, 4]);
        move_within(&mut a, 0..3, 2); // the destination runs past the end: the last one is dropped
        assert_eq!(a, [1, 3, 1, 3]);
        #[allow(clippy::reversed_empty_ranges)] // the point: a malformed range moves nothing
        move_within(&mut a, 3..1, 0);
        assert_eq!(a, [1, 3, 1, 3]);
    }

    #[test]
    fn rfind_ascii_matches_rfind() {
        for s in ["", "a.b.c", "no dots", ".x", "x.", "\u{e9}.\u{e9}"] {
            assert_eq!(rfind_ascii(s, b'.'), s.rfind('.'), "{s}");
        }
    }

    #[test]
    fn take_at_the_end_gives_zeros() {
        let b = [1u8, 2, 3, 4];
        assert_eq!(take::<2>(&b, 2), [3, 4]);
        assert_eq!(take::<2>(&b, 3), [0, 0]);
        assert_eq!(take::<2>(&b, usize::MAX), [0, 0]);
    }
}
