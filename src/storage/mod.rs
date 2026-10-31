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

/// The `N` bytes of `b` from `i`, zeros when `b` is too short (a malformed record reads as empty, never panics).
pub(crate) fn take<const N: usize>(b: &[u8], i: usize) -> [u8; N] {
    i.checked_add(N).and_then(|end| b.get(i..end)).and_then(|s| s.try_into().ok()).unwrap_or([0; N])
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
        move_within(&mut a, 3..1, 0); // an empty (reversed) range
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
