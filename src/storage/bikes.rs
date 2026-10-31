//! Three bikes (roadmap 3c): each with its wheel, weight and gears, its own odometer and the wear of five parts
//! (distance since each was last replaced, and an optional service interval), and the file that keeps them,
//! `\Rustfw\bikes.bin` (app/bikes.rs reads and writes it). Pure logic.
//!
//! **Each ride counted once.** A ride's distance goes to the active bike from the ride scan's list
//! ([`crate::stock::Scan`], the rides whole in `\Activities`, newest first), never from the ride in memory: every
//! ride whose start (its name, to the minute: the list has no seconds) is after [`Bikes::counted`] is added, and
//! `counted` moves to the newest, in the same copy of the file as the sums it gave; the files (first clusters) of the
//! rides counted in that minute are kept with it ([`Bikes::at_mark`]), so a second ride started in the same minute
//! and scanned later counts too. A copy holds the sums and the mark together, so a power cut before the write, or
//! during it (the older copy is read), leaves both as they were and the next scan adds the same rides again, once; a
//! ride is in the list only once its file is finished in `\Activities` (read back whole and renamed by the recorder),
//! so nothing is added before that. Rides of either firmware count (a ride on stock goes to the bike active here).
//! The migration sets the mark a minute before it, or before the ride open then (the rides before are in BIKE 1's
//! odometer already, the settings' ODOMETER, or not this firmware's); with no clock set, the first count only sets it.
//!
//! **The clock.** A ride named more than a day after now (a clock set wrong when it was recorded) is left out, and
//! does not move the mark. Limit: if the clock goes back (set wrong, then right), the rides named before the mark are
//! not counted until the clock passes it again.
//!
//! **The file**, two 512-byte copies as `places.bin` (nav/places.rs): the newest whole copy (a good magic, version,
//! length and CRC-32; the higher sequence number) wins, and a write goes over the other one, so a write cut at any
//! point leaves the newest copy before it whole. A copy, little endian:
//!
//! ```text
//! 0    "RBIK"          magic
//! 4    version (u16)   1
//! 6    length (u16)    bytes of data at 12: 140 now; a later build appends and raises it (a reader takes what it knows)
//! 8    seq (u32)       higher is newer
//! 12   active (u8)     0 to 2; then a zero byte
//! 14   counted         the start of the newest ride counted: year (u16; 0 none yet), month, day, hour, minute
//! 20   at mark         the first clusters (u32 each, 0 none) of up to 3 rides counted in that minute
//! 32   3 bikes         40 bytes each: wheel (u16, mm), weight (u16, 100 g), CHAINRINGS, CASSETTE (u8, presets),
//!                      odometer (u32, m), then 5 parts: distance since RESET (u32, m), SERVICE EVERY (u16, km; 0 off)
//! 508  CRC-32 of 0..508
//! ```
//!
//! A copy with a value out of its range (an active bike past 2, a wheel not in [`crate::WHEELS`], a weight past
//! [`crate::menu::BIKE_RANGE`], a preset past its table, an interval not in [`EVERY_KM`]) is not read: no build of
//! this firmware writes one.

use crate::history::Start;

/// Bikes kept.
pub const BIKES: usize = 3;

/// Parts followed per bike.
pub const PARTS: usize = 5;

/// The parts' names (BRAKES: the brake pads; "BRAKE PADS" and a distance do not fit a menu row).
pub const PART_NAMES: [&str; PARTS] = ["CHAIN", "CASSETTE", "TYRE F", "TYRE R", "BRAKES"];

/// SERVICE EVERY's choices, km (0: OFF).
pub const EVERY_KM: [u16; 11] = [0, 500, 1000, 1500, 2000, 3000, 4000, 5000, 6000, 8000, 10_000];

const MAGIC: &[u8; 4] = b"RBIK";
const VERSION: u16 = 1;
const DATA_AT: usize = 12;
const AT_MARK_AT: usize = 20;
/// Rides of the mark's minute kept by their file.
pub const AT_MARK: usize = 3;
const BIKES_AT: usize = AT_MARK_AT + 4 * AT_MARK;
const BIKE_BYTES: usize = 10 + PARTS * 6;
/// Bytes of data this build writes (length).
pub const DATA_LEN: usize = BIKES_AT - DATA_AT + BIKES * BIKE_BYTES;
const CRC_AT: usize = 508;
const _: () = assert!(DATA_AT + DATA_LEN <= CRC_AT);

/// A part's wear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// Distance since it was last RESET, m.
    pub m: u32,
    /// SERVICE EVERY, km (0: off); one of [`EVERY_KM`].
    pub every_km: u16,
}

impl Part {
    /// Past its service interval.
    pub fn due(&self) -> bool {
        self.every_km > 0 && self.m / 1000 >= u32::from(self.every_km)
    }
}

/// A bike's wheel, weight and gears: the settings' while it is the active one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gear {
    /// Wheel circumference, mm (one of [`crate::WHEELS`]).
    pub wheel_mm: u16,
    /// Weight, 100 g.
    pub bike_hg: u16,
    /// CHAINRINGS preset ([`crate::CHAINRINGS`]).
    pub chainrings: u8,
    /// CASSETTE preset ([`crate::CASSETTES`]).
    pub cassette: u8,
}

impl Gear {
    /// A bike nothing was set for: the settings' defaults.
    pub const DEFAULT: Gear =
        Gear { wheel_mm: crate::WHEELS[1], bike_hg: crate::power::BIKE_HG, chainrings: 0, cassette: 0 };

    /// The settings' (the active bike's).
    pub fn of(s: &crate::Settings) -> Gear {
        Gear {
            wheel_mm: s.wheel_mm,
            bike_hg: s.bike_hg,
            chainrings: u8::try_from(s.chainrings).unwrap_or(0),
            cassette: u8::try_from(s.cassette).unwrap_or(0),
        }
    }

    /// Into the settings (the bike made the active one).
    pub fn apply(&self, s: &mut crate::Settings) {
        (s.wheel_mm, s.bike_hg) = (self.wheel_mm, self.bike_hg);
        (s.chainrings, s.cassette) = (usize::from(self.chainrings), usize::from(self.cassette));
    }

    fn valid(&self) -> bool {
        crate::WHEELS.contains(&self.wheel_mm)
            && crate::menu::BIKE_RANGE.contains(&self.bike_hg)
            && usize::from(self.chainrings) < crate::CHAINRINGS.len()
            && usize::from(self.cassette) < crate::CASSETTES.len()
    }
}

/// A bike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bike {
    /// Its wheel, weight and gears.
    pub gear: Gear,
    /// Every ride's distance on it, m.
    pub odo_m: u32,
    /// The parts, in [`PART_NAMES`]' order.
    pub parts: [Part; PARTS],
}

impl Bike {
    const NEW: Bike = Bike { gear: Gear::DEFAULT, odo_m: 0, parts: [Part { m: 0, every_km: 0 }; PARTS] };
}

/// The three bikes, the active one, and the last ride counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bikes {
    /// BIKE 1 to 3.
    pub list: [Bike; BIKES],
    /// The active bike (0 to 2): its gear is the settings', the rides go to it.
    pub active: u8,
    /// The start of the newest ride counted; year 0: none yet (the next count only sets it).
    pub counted: Start,
    /// The first clusters of the rides counted that started in `counted`'s minute (0: none).
    pub at_mark: [u32; AT_MARK],
}

/// A ride of the scan's list as [`Bikes::count`] takes it: its start, its file's first cluster, its distance (m).
pub type Seen = (Start, u32, u32);

impl Default for Bikes {
    fn default() -> Self {
        Self::new()
    }
}

impl Bikes {
    /// Three bikes nothing was set for, BIKE 1 active, no ride counted.
    pub const fn new() -> Self {
        Bikes { list: [Bike::NEW; BIKES], active: 0, counted: (0, 0, 0, 0, 0), at_mark: [0; AT_MARK] }
    }

    /// The first file, from the single bike of the settings (`gear`) and its ODOMETER: BIKE 1, active; the rides
    /// started after `counted` (a minute before now; year 0 with no clock: the next count sets it) go to it.
    ///
    /// ```
    /// use bryton_rustfw::bikes::{Bikes, Gear};
    /// let g = Gear { wheel_mm: 2136, ..Gear::DEFAULT };
    /// let b = Bikes::migrate(g, 12_345_000, (2026, 10, 31, 8, 59));
    /// assert_eq!((b.active, b.list[0].gear, b.list[0].odo_m, b.list[1].odo_m), (0, g, 12_345_000, 0));
    /// ```
    pub fn migrate(gear: Gear, odo_m: u32, counted: Start) -> Bikes {
        let mut b = Bikes::new();
        (b.list[0].gear, b.list[0].odo_m, b.counted) = (gear, odo_m, counted);
        b
    }

    /// The active bike.
    pub fn active(&self) -> &Bike {
        self.list.get(usize::from(self.active)).unwrap_or(&self.list[0])
    }

    /// See [`Bikes::active`].
    pub fn active_mut(&mut self) -> &mut Bike {
        let k = usize::from(self.active).min(BIKES - 1);
        &mut self.list[k]
    }

    /// The rides of the scan (any order) newer than the last counted go to the active bike's odometer and parts, and
    /// the mark moves to the newest of them (module doc); a ride started after `limit` (a day after now) is left out.
    /// With no mark yet, only the mark is set (to the newest ride, or before any). True if anything changed.
    ///
    /// ```
    /// use bryton_rustfw::bikes::Bikes;
    /// let mut b = Bikes::new();
    /// let limit = (2026, 10, 3, 0, 0);
    /// assert!(b.count([((2026, 10, 1, 9, 0), 7, 40_000)].into_iter(), limit)); // the mark only
    /// assert_eq!(b.list[0].odo_m, 0);
    /// let rides = [((2026, 10, 2, 9, 0), 9, 30_000), ((2026, 10, 1, 9, 0), 7, 40_000)];
    /// assert!(b.count(rides.into_iter(), limit));
    /// assert!(!b.count(rides.into_iter(), limit), "counted once");
    /// assert_eq!((b.list[0].odo_m, b.list[0].parts[0].m, b.counted), (30_000, 30_000, (2026, 10, 2, 9, 0)));
    /// ```
    pub fn count(&mut self, rides: impl Iterator<Item = Seen> + Clone, limit: Start) -> bool {
        let rides = rides.filter(move |r| r.0 <= limit);
        if self.counted.0 == 0 {
            let newest = rides.clone().map(|r| r.0).max();
            // before any ride (named from 2000) when there is none
            (self.counted, self.at_mark) = (newest.unwrap_or((1, 1, 1, 0, 0)), [0; AT_MARK]);
            for (_, file, _) in rides.filter(|r| Some(r.0) == newest) {
                self.mark_file(file);
            }
            return true;
        }
        let (mark, at) = (self.counted, self.at_mark);
        let new = rides.filter(move |r| r.0 > mark || (r.0 == mark && !at.contains(&r.1)));
        let Some(newest) = new.clone().map(|r| r.0).max() else { return false };
        let mut changed = newest > mark;
        if changed {
            (self.counted, self.at_mark) = (newest, [0; AT_MARK]);
        }
        for (start, file, m) in new {
            // shortcut: past AT_MARK rides started in one minute, the others are not counted (never twice)
            if start == newest && !self.mark_file(file) {
                continue;
            }
            changed = true;
            let bike = self.active_mut();
            bike.odo_m = bike.odo_m.saturating_add(m);
            for p in &mut bike.parts {
                p.m = p.m.saturating_add(m);
            }
        }
        changed
    }

    /// Keeps `file` as counted in the mark's minute; false when there is no room.
    fn mark_file(&mut self, file: u32) -> bool {
        let Some(slot) = self.at_mark.iter_mut().find(|s| **s == 0) else { return false };
        *slot = file;
        true
    }

    /// The active bike's first part past its service interval, if any.
    pub fn due(&self) -> Option<usize> {
        self.active().parts.iter().position(Part::due)
    }

    /// A copy of the file with sequence number `seq`.
    pub fn to_sector(&self, seq: u32) -> [u8; 512] {
        let mut s = [0u8; 512];
        s[..4].copy_from_slice(MAGIC);
        s[4..6].copy_from_slice(&VERSION.to_le_bytes());
        s[6..8].copy_from_slice(&(DATA_LEN as u16).to_le_bytes());
        s[8..12].copy_from_slice(&seq.to_le_bytes());
        s[12] = self.active;
        let (y, mo, d, h, mi) = self.counted;
        s[14..16].copy_from_slice(&y.to_le_bytes());
        s[16..20].copy_from_slice(&[mo, d, h, mi]);
        for (c, e) in self.at_mark.iter().zip(s[AT_MARK_AT..BIKES_AT].chunks_exact_mut(4)) {
            e.copy_from_slice(&c.to_le_bytes());
        }
        for (b, e) in self.list.iter().zip(s[BIKES_AT..].chunks_exact_mut(BIKE_BYTES)) {
            e[0..2].copy_from_slice(&b.gear.wheel_mm.to_le_bytes());
            e[2..4].copy_from_slice(&b.gear.bike_hg.to_le_bytes());
            (e[4], e[5]) = (b.gear.chainrings, b.gear.cassette);
            e[6..10].copy_from_slice(&b.odo_m.to_le_bytes());
            for (p, f) in b.parts.iter().zip(e[10..].chunks_exact_mut(6)) {
                f[0..4].copy_from_slice(&p.m.to_le_bytes());
                f[4..6].copy_from_slice(&p.every_km.to_le_bytes());
            }
        }
        let crc = crate::inflate::crc32(s[..CRC_AT].iter().copied());
        s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        s
    }

    /// A copy [`Bikes::to_sector`] wrote: its sequence number and bikes; None if it is not one (module doc).
    ///
    /// ```
    /// use bryton_rustfw::bikes::Bikes;
    /// let mut b = Bikes::new();
    /// b.active = 2;
    /// let mut s = b.to_sector(7);
    /// assert_eq!(Bikes::from_sector(&s), Some((7, b)));
    /// s[30] ^= 1;
    /// assert_eq!(Bikes::from_sector(&s), None);
    /// ```
    pub fn from_sector(s: &[u8; 512]) -> Option<(u32, Bikes)> {
        let le16 = |a: usize| Some(u16::from_le_bytes(s.get(a..a + 2)?.try_into().ok()?));
        let le32 = |a: usize| Some(u32::from_le_bytes(s.get(a..a + 4)?.try_into().ok()?));
        if s.get(..4)? != MAGIC
            || le16(4)? != VERSION
            || le32(CRC_AT)? != crate::inflate::crc32(s.get(..CRC_AT)?.iter().copied())
        {
            return None;
        }
        let len = usize::from(le16(6)?);
        if !(DATA_LEN..=CRC_AT - DATA_AT).contains(&len) || usize::from(*s.get(12)?) >= BIKES || *s.get(13)? != 0 {
            return None;
        }
        let mut out = Bikes::new();
        out.active = *s.get(12)?;
        out.counted = (le16(14)?, *s.get(16)?, *s.get(17)?, *s.get(18)?, *s.get(19)?);
        for (k, c) in out.at_mark.iter_mut().enumerate() {
            *c = le32(AT_MARK_AT + 4 * k)?;
        }
        for (k, b) in out.list.iter_mut().enumerate() {
            let at = BIKES_AT + k * BIKE_BYTES;
            b.gear = Gear {
                wheel_mm: le16(at)?,
                bike_hg: le16(at + 2)?,
                chainrings: *s.get(at + 4)?,
                cassette: *s.get(at + 5)?,
            };
            b.odo_m = le32(at + 6)?;
            for (i, p) in b.parts.iter_mut().enumerate() {
                let f = at + 10 + 6 * i;
                *p = Part { m: le32(f)?, every_km: le16(f + 4)? };
                if !EVERY_KM.contains(&p.every_km) {
                    return None;
                }
            }
            if !b.gear.valid() {
                return None;
            }
        }
        Some((le32(8)?, out))
    }

    /// The newer of the file's two copies that check out (its 1024 bytes), if any.
    pub fn newest(file: &[u8; 1024]) -> Option<(u32, Bikes)> {
        let copy = |k: usize| Bikes::from_sector(file.get(512 * k..512 * (k + 1))?.try_into().ok()?);
        match (copy(0), copy(1)) {
            (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Bikes {
        let mut b = Bikes::migrate(
            Gear { wheel_mm: crate::WHEELS[3], bike_hg: 85, chainrings: 2, cassette: 3 },
            1234,
            (0, 0, 0, 0, 0),
        );
        b.active = 1;
        b.counted = (2026, 10, 30, 7, 45);
        b.list[1].parts[0] = Part { m: 2_999_999, every_km: 3000 };
        b.list[2].parts[4] = Part { m: u32::MAX, every_km: 10_000 };
        b
    }

    /// Both copies whole: the higher sequence; one damaged: the other; neither: none; a value out of range is refused
    /// even with a good CRC; a longer length (a later build's appended data) is read.
    #[test]
    fn round_trip_newest_and_damage() {
        let (old, new) = (Bikes::new(), two());
        let mut file = [0u8; 1024];
        file[..512].copy_from_slice(&new.to_sector(4));
        file[512..].copy_from_slice(&old.to_sector(3));
        assert_eq!(Bikes::newest(&file), Some((4, new)));
        file[100] ^= 0x10;
        assert_eq!(Bikes::newest(&file), Some((3, old)), "the other copy");
        file[600] ^= 0x10;
        assert_eq!(Bikes::newest(&file), None);
        let resealed = |mut s: [u8; 512]| {
            let crc = crate::inflate::crc32(s[..CRC_AT].iter().copied());
            s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
            s
        };
        for (at, v) in [(12, 3u8), (13, 1), (BIKES_AT, 7), (BIKES_AT + 4, 6), (BIKES_AT + 5, 7), (BIKES_AT + 14, 1)] {
            let mut s = new.to_sector(5);
            s[at] = v;
            assert_eq!(Bikes::from_sector(&resealed(s)), None, "byte {at} = {v}");
        }
        let mut s = new.to_sector(5);
        s[6] = (DATA_LEN + 20) as u8;
        assert_eq!(Bikes::from_sector(&resealed(s)), Some((5, new)), "a later build's length");
        s[6] = (DATA_LEN - 1) as u8;
        assert_eq!(Bikes::from_sector(&resealed(s)), None, "short");
    }

    const LIMIT: Start = (2026, 11, 2, 0, 0);

    /// Two rides started in one minute, scanned one at a time (the second saved later): both count, once each; a
    /// fourth in that minute (past [`AT_MARK`]) is never counted, nor any twice.
    #[test]
    fn two_rides_in_one_minute_count_once_each() {
        let mut b = two();
        let t = (2026, 10, 31, 9, 0);
        assert!(b.count([(t, 11, 1000)].into_iter(), LIMIT));
        let both = [(t, 12, 200), (t, 11, 1000)];
        assert!(b.count(both.into_iter(), LIMIT));
        assert!(!b.count(both.into_iter(), LIMIT));
        assert_eq!((b.list[1].odo_m, b.at_mark), (1200, [11, 12, 0]));
        let five = [(t, 15, 1), (t, 14, 1), (t, 13, 30), (t, 12, 200), (t, 11, 1000)];
        assert!(b.count(five.into_iter(), LIMIT));
        assert!(!b.count(five.into_iter(), LIMIT));
        assert_eq!((b.list[1].odo_m, b.at_mark), (1201, [11, 12, 15]), "a third kept (the first listed), past it none");
        assert_eq!(Bikes::from_sector(&b.to_sector(1)), Some((1, b)), "the files kept in the copy");
    }

    /// A ride named a day or more ahead (a clock set wrong) is left out and does not move the mark: a normal one
    /// after it counts.
    #[test]
    fn a_future_ride_does_not_move_the_mark() {
        let mut b = two();
        assert!(!b.count([((2027, 1, 1, 9, 0), 3, 5000)].into_iter(), LIMIT));
        let rides = [((2027, 1, 1, 9, 0), 3, 5000), ((2026, 10, 31, 9, 0), 4, 700)];
        assert!(b.count(rides.into_iter(), LIMIT));
        assert_eq!((b.list[1].odo_m, b.counted), (700, (2026, 10, 31, 9, 0)));
    }

    /// The rides go to the active bike only, once each, the parts with them; a part past its interval is due.
    #[test]
    fn count_goes_to_the_active_bike_once() {
        let mut b = two();
        let rides =
            [((2026, 10, 31, 8, 0), 3, 1_000), ((2026, 10, 30, 7, 45), 4, 50_000), ((2026, 10, 29, 9, 0), 5, 9)];
        assert_eq!(b.due(), None);
        b.at_mark = [4, 0, 0];
        assert!(b.count(rides.into_iter(), LIMIT));
        assert_eq!((b.list[1].odo_m, b.list[1].parts[0].m, b.list[0].odo_m), (1_000, 3_000_999, 1234));
        assert_eq!(b.due(), Some(0), "CHAIN past 3000 km");
        assert!(!b.count(rides.into_iter(), LIMIT));
        b.active = 2;
        assert!(b.count([((2026, 11, 1, 8, 0), 6, 5)].into_iter(), LIMIT));
        assert_eq!(b.list[2].parts[4].m, u32::MAX, "saturates");
        assert_eq!(b.due(), Some(4));
    }
}
