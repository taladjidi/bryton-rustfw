//! Favourite places: up to [`MAX`] positions the rider saved (SAVE PLACE), each with the altitude known there, and
//! the file that keeps them, `\Rustfw\places.bin` (app/places.rs reads and writes it). Pure logic.
//!
//! **Altitude at a place.** Stock does the same (its `Baro_CheckAltCalibrationAtLocation` sets the barometric
//! altitude from a saved place: ghidra/notes agent_a2.md, agent_b1.md, as altitude.rs says); here a place within
//! [`NEAR_M`] with an altitude, stopped, calibrates the barometer to it (app/places.rs, altitude.rs `By::Place`).
//!
//! **The file**, two 512-byte copies as `settings.bin` (app/settings_file.rs): the newest whole copy (a good magic,
//! version, length and CRC-32; the higher sequence number) wins, and a write goes over the other one, so a write cut
//! at any point leaves the newest copy before it whole. A copy, little endian:
//!
//! ```text
//! 0    "RPLC"          magic
//! 4    version (u16)   1
//! 6    length (u16)    bytes of places at 12: [`ENTRY`] each, at most [`MAX`] of them
//! 8    seq (u32)       higher is newer
//! 12   places          latitude, longitude (i32, degrees x 1e-7), altitude (i16, m; -32768 unknown), number (u8,
//!                      the name is PLACE and it, 1 to 16), a zero byte
//! 508  CRC-32 of 0..508
//! ```
//!
//! A copy with a place out of range (a latitude past 90 degrees, an altitude past [`crate::ride::ALT_RANGE_MM`], a
//! number 0 or used twice) is not read: no build of this firmware writes one.
//!
//! Distances and bearings on the local plane at the rider, as the course's (course.rs `step_mm`): within a few
//! metres in a kilometre, enough for "1.2 KM NE".

use crate::text::Line;

/// Places kept.
pub const MAX: usize = 16;

/// A place this near (m) is where the rider is: its altitude calibrates the barometer.
pub const NEAR_M: u32 = 30;

/// Bytes of a place in the file.
pub const ENTRY: usize = 12;

const MAGIC: &[u8; 4] = b"RPLC";
const VERSION: u16 = 1;
const DATA_AT: usize = 12;
const CRC_AT: usize = 508;
const NO_ALT: i16 = i16::MIN;
const _: () = assert!(DATA_AT + MAX * ENTRY <= CRC_AT);

/// A saved place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Degrees x 1e-7.
    pub lat: i32,
    /// Degrees x 1e-7.
    pub lon: i32,
    /// Altitude, m; `i16::MIN` unknown ([`Place::alt_mm`]).
    alt_m: i16,
    /// Its name is PLACE and this number (1 to [`MAX`]).
    pub num: u8,
}

impl Place {
    /// The altitude known there, mm.
    pub fn alt_mm(&self) -> Option<i32> {
        (self.alt_m != NO_ALT).then(|| i32::from(self.alt_m) * 1000)
    }

    /// Its name: "PLACE 3".
    pub fn name(&self, l: &mut Line<16>) {
        l.push_str("PLACE ");
        l.push_u32(u32::from(self.num));
    }
}

/// An altitude as kept (to the metre); None or out of range: unknown.
fn alt_m(alt_mm: Option<i32>) -> i16 {
    alt_mm.filter(|a| crate::ride::ALT_RANGE_MM.contains(a)).map_or(NO_ALT, |a| (a + 500).div_euclid(1000) as i16)
}

/// The places saved, oldest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Places {
    list: [Place; MAX],
    n: u8,
}

impl Default for Places {
    fn default() -> Self {
        Places::new()
    }
}

impl Places {
    /// None saved.
    pub const fn new() -> Self {
        Places { list: [Place { lat: 0, lon: 0, alt_m: NO_ALT, num: 0 }; MAX], n: 0 }
    }

    /// How many.
    pub fn len(&self) -> usize {
        usize::from(self.n).min(MAX)
    }

    /// None saved.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Place `k`, oldest first.
    pub fn get(&self, k: usize) -> Option<&Place> {
        self.list.get(..self.len())?.get(k)
    }

    /// Saves a place here, named by the first number free; its index, or None when [`MAX`] are saved.
    ///
    /// ```
    /// use bryton_rustfw::places::Places;
    /// let mut p = Places::new();
    /// assert_eq!(p.add(5_123_456, 4_234_567, Some(212_400)), Some(0));
    /// assert_eq!((p.get(0).unwrap().num, p.get(0).unwrap().alt_mm()), (1, Some(212_000)));
    /// ```
    pub fn add(&mut self, lat: i32, lon: i32, alt_mm: Option<i32>) -> Option<usize> {
        let k = self.len();
        let num = (1..=MAX as u8).find(|&m| self.list[..k].iter().all(|p| p.num != m))?;
        let slot = self.list.get_mut(k)?;
        *slot = Place { lat, lon, alt_m: alt_m(alt_mm), num };
        self.n += 1;
        Some(k)
    }

    /// Deletes place `k` (the later ones move up).
    pub fn remove(&mut self, k: usize) {
        let n = self.len();
        if k < n {
            self.list.copy_within(k + 1..n, k);
            self.n -= 1;
        }
    }

    /// Sets place `k`'s altitude (mm; None: unknown).
    pub fn set_alt(&mut self, k: usize, alt_mm: Option<i32>) {
        let n = self.len();
        if let Some(p) = self.list.get_mut(..n).and_then(|l| l.get_mut(k)) {
            p.alt_m = alt_m(alt_mm);
        }
    }

    /// The nearest place with an altitude to (`lat`, `lon`): its index and how far, m.
    pub fn nearest_with_alt(&self, lat: i32, lon: i32) -> Option<(usize, u32)> {
        (0..self.len())
            .filter_map(|k| {
                self.get(k).filter(|p| p.alt_m != NO_ALT).map(|p| (k, dist_bearing((lat, lon), (p.lat, p.lon)).0))
            })
            .min_by_key(|&(_, d)| d)
    }

    /// A copy of the file with sequence number `seq`.
    pub fn to_sector(&self, seq: u32) -> [u8; 512] {
        let mut s = [0u8; 512];
        let n = self.len();
        s[..4].copy_from_slice(MAGIC);
        s[4..6].copy_from_slice(&VERSION.to_le_bytes());
        s[6..8].copy_from_slice(&((n * ENTRY) as u16).to_le_bytes());
        s[8..12].copy_from_slice(&seq.to_le_bytes());
        for (p, e) in self.list[..n].iter().zip(s[DATA_AT..CRC_AT].chunks_exact_mut(ENTRY)) {
            e[..4].copy_from_slice(&p.lat.to_le_bytes());
            e[4..8].copy_from_slice(&p.lon.to_le_bytes());
            e[8..10].copy_from_slice(&p.alt_m.to_le_bytes());
            e[10] = p.num;
        }
        let crc = crate::inflate::crc32(s[..CRC_AT].iter().copied());
        s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        s
    }

    /// A copy [`Places::to_sector`] wrote: its sequence number and places; None if it is not one (module doc).
    ///
    /// ```
    /// use bryton_rustfw::places::Places;
    /// let mut p = Places::new();
    /// p.add(5_123_456, 4_234_567, None);
    /// let mut s = p.to_sector(7);
    /// assert_eq!(Places::from_sector(&s), Some((7, p)));
    /// s[13] ^= 1;
    /// assert_eq!(Places::from_sector(&s), None);
    /// ```
    pub fn from_sector(s: &[u8; 512]) -> Option<(u32, Places)> {
        let le16 = |a: usize| crate::storage::bytes::le16(s, a);
        let le32 = |a: usize| crate::storage::bytes::le32(s, a);
        if s.get(..4)? != MAGIC
            || le16(4)? != VERSION
            || le32(CRC_AT)? != crate::inflate::crc32(s.get(..CRC_AT)?.iter().copied())
        {
            return None;
        }
        let len = usize::from(le16(6)?);
        if !len.is_multiple_of(ENTRY) || len > MAX * ENTRY {
            return None;
        }
        let mut out = Places::new();
        for e in s.get(DATA_AT..DATA_AT + len)?.chunks_exact(ENTRY) {
            let i32_at = |a: usize| Some(i32::from_le_bytes(e.get(a..a + 4)?.try_into().ok()?));
            let p = Place {
                lat: i32_at(0)?,
                lon: i32_at(4)?,
                alt_m: i16::from_le_bytes(e.get(8..10)?.try_into().ok()?),
                num: *e.get(10)?,
            };
            let alt_ok = p.alt_m == NO_ALT || crate::ride::ALT_RANGE_MM.contains(&(i32::from(p.alt_m) * 1000));
            let used = out.list[..out.len()].iter().any(|q| q.num == p.num);
            if p.lat.unsigned_abs() > 900_000_000
                || p.lon.unsigned_abs() > 1_800_000_000
                || !alt_ok
                || !(1..=MAX as u8).contains(&p.num)
                || used
                || *e.get(11)? != 0
            {
                return None;
            }
            let k = out.len();
            *out.list.get_mut(k)? = p;
            out.n += 1;
        }
        Some((le32(8)?, out))
    }

    /// The newer of the file's two copies that check out (its 1024 bytes), if any.
    pub fn newest(file: &[u8; 1024]) -> Option<(u32, Places)> {
        let copy = |k: usize| Places::from_sector(file.get(512 * k..512 * (k + 1))?.try_into().ok()?);
        match (copy(0), copy(1)) {
            (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

/// From `a` to `b` (degrees x 1e-7): the distance, m, and the bearing, degrees clockwise from north (0 to 359).
///
/// ```
/// use bryton_rustfw::places::dist_bearing;
/// assert_eq!(dist_bearing((0, 0), (8983, 0)), (99, 0)); // 100 m north (to the metre below)
/// assert_eq!(dist_bearing((0, 0), (0, 8983)).1, 90);
/// assert_eq!(dist_bearing((0, 0), (-8983, -8983)).1, 225);
/// ```
pub fn dist_bearing(a: (i32, i32), b: (i32, i32)) -> (u32, u16) {
    let cos = crate::map::cos_q16(a.0);
    let clamp = |v: i64| v.clamp(-(1 << 31), 1 << 31);
    // mm east and north, as course.rs step_mm
    let dy = clamp((i64::from(b.0) - i64::from(a.0)) * 11_132 / 1000);
    let dx = clamp((i64::from(b.1) - i64::from(a.1)) * 11_132 * cos / 65_536 / 1000);
    let mm = crate::course::step_mm(a, b);
    // angle_deg is counter-clockwise from its first vector: north to the place, turned to clockwise
    let deg = -crate::course::angle_deg((0, 1), (dx, dy));
    ((mm / 1000).min(u64::from(u32::MAX)) as u32, deg.rem_euclid(360) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Places {
        let mut p = Places::new();
        p.add(5_123_456, 4_234_567, Some(212_000));
        p.add(-5_000_000, 179_999_999, None);
        p.add(899_999_999, -1_800_000_000, Some(-400_000));
        p
    }

    #[test]
    fn round_trip_newest_and_damage() {
        let p = three();
        let (a, b) = (Places::new().to_sector(4), p.to_sector(5));
        let mut file = [0u8; 1024];
        file[..512].copy_from_slice(&a);
        file[512..].copy_from_slice(&b);
        assert_eq!(Places::newest(&file), Some((5, p)), "the higher sequence");
        file[512 + 20] ^= 0x10; // the newer damaged: the older
        assert_eq!(Places::newest(&file), Some((4, Places::new())));
        file[3] = b'X'; // both: none
        assert_eq!(Places::newest(&file), None);
        // a copy of zeros (a file just made whole) is no copy
        assert_eq!(Places::from_sector(&[0; 512]), None);
    }

    #[test]
    fn foreign_copies_are_refused() {
        let recrc = |s: &mut [u8; 512]| {
            let crc = crate::inflate::crc32(s[..CRC_AT].iter().copied());
            s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        };
        let good = three().to_sector(1);
        for (at, v) in [(4, 2u8), (6, 13), (6, 255), (DATA_AT + 3, 0x40), (DATA_AT + 10, 0), (DATA_AT + 11, 1)] {
            let mut s = good;
            s[at] = v;
            recrc(&mut s);
            assert_eq!(Places::from_sector(&s), None, "byte {at} = {v}");
        }
        // a number used twice
        let mut s = good;
        s[DATA_AT + ENTRY + 10] = 1;
        recrc(&mut s);
        assert_eq!(Places::from_sector(&s), None);
    }

    #[test]
    fn add_full_remove_and_numbers() {
        let mut p = Places::new();
        for k in 0..MAX {
            assert_eq!(p.add(k as i32, 0, None), Some(k));
        }
        assert_eq!(p.add(0, 0, None), None, "full");
        p.remove(3);
        assert_eq!((p.len(), p.get(3).unwrap().num), (MAX - 1, 5));
        assert_eq!(p.add(0, 0, Some(1_000)).map(|k| p.get(k).unwrap().num), Some(4), "the number freed");
        p.remove(MAX); // past the end: nothing
        assert_eq!(p.len(), MAX);
        p.set_alt(0, Some(9_000_001)); // out of range: unknown
        assert_eq!(p.get(0).unwrap().alt_mm(), None);
        let mut l = Line::<16>::new();
        p.get(0).unwrap().name(&mut l);
        assert_eq!(l.as_str(), "PLACE 1");
    }

    #[test]
    fn the_nearest_with_an_altitude() {
        let p = three();
        assert_eq!(p.nearest_with_alt(5_123_456 + 1000, 4_234_567), Some((0, 11)));
        let mut q = Places::new();
        q.add(0, 0, None);
        assert_eq!(q.nearest_with_alt(0, 0), None, "no altitude known there");
    }
}
