//! What the card's volume holds, for STORAGE (SYSTEM > STORAGE): its size, its free space (FSInfo's free count when
//! it is whole, else the FAT counted a pass at a time), the rides in `\Activities`, and the oldest of them.
//!
//! Read only, a bounded amount a pass ([`Survey::step`]); the app runs it while the card is idle, never during a
//! ride (app/storage.rs). A damaged volume shows "--" (the survey stops, [`Survey::failed`]), never a panic.
//! Formats: Microsoft's FAT32 specification (FSInfo) and the FAT as fat/mod.rs reads it.

use crate::fat::{Cursor, Disk, Entry, Error, Fat};
use crate::history::Start;
use crate::stock::ride_start;
use crate::ui::text::Line;

/// The oldest rides kept (STORAGE's rows).
pub const OLDEST: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Mount,
    Free,
    Rides,
    Done,
}

/// The survey of the card: built up over passes, read once a screen opens.
pub struct Survey {
    stage: Stage,
    fat: Option<Fat>,
    /// The listing of `\Activities`, opened in the rides stage.
    dir: Option<Cursor>,
    /// The next FAT cluster to count, and the free clusters counted so far.
    next: u32,
    counted: u32,
    /// Free clusters: from FSInfo, else counted.
    free: Option<u32>,
    /// Rides in `\Activities` (stock's names), and the oldest [`OLDEST`] of them, the oldest first.
    rides: u32,
    oldest: [Option<Entry>; OLDEST],
    failed: bool,
}

impl Default for Survey {
    fn default() -> Self {
        Self::new()
    }
}

impl Survey {
    /// Nothing read yet.
    pub const fn new() -> Self {
        Survey {
            stage: Stage::Mount,
            fat: None,
            dir: None,
            next: 2,
            counted: 0,
            free: None,
            rides: 0,
            oldest: [None; OLDEST],
            failed: false,
        }
    }

    /// Starts again: the screen opened, or a ride was deleted (its count and list are stale).
    pub fn start(&mut self) {
        *self = Self::new();
    }

    /// Everything read (or the read failed).
    pub fn done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// The read hit a damaged volume: the values are "--" from here on.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Reads at most `sectors` FAT sectors (and as many directory sectors); true once done.
    pub fn step(&mut self, d: &mut impl Disk, sectors: u32) -> bool {
        match self.run(d, sectors) {
            Ok(done) => done,
            Err(_) => {
                self.failed = true;
                self.stage = Stage::Done;
                true
            }
        }
    }

    fn run(&mut self, d: &mut impl Disk, sectors: u32) -> Result<bool, Error> {
        if self.stage == Stage::Mount {
            let fat = Fat::mount(d)?;
            self.free = fat.fsinfo_free(d);
            self.fat = Some(fat);
            self.stage = if self.free.is_some() { Stage::Rides } else { Stage::Free };
        }
        if self.stage == Stage::Free {
            let fat = self.fat.as_ref().ok_or(Error::Corrupt)?;
            if fat.count_free(d, &mut self.next, &mut self.counted, sectors)? {
                self.free = Some(self.counted);
                self.stage = Stage::Rides;
            }
            return Ok(false);
        }
        if self.stage == Stage::Rides {
            let fat = self.fat.as_mut().ok_or(Error::Corrupt)?;
            if self.dir.is_none() {
                match fat.open(d, "Activities") {
                    Ok(dir) => self.dir = Some(fat.cursor(dir)?),
                    Err(Error::NotFound) => {
                        self.stage = Stage::Done; // no rides yet
                        return Ok(true);
                    }
                    Err(e) => return Err(e),
                }
            }
            let c = self.dir.as_mut().ok_or(Error::Corrupt)?;
            // a listing sector holds 16 entries: at most `sectors` of them read, a call an entry
            for _ in 0..sectors.saturating_mul(16) {
                let Some(e) = fat.list(d, c)? else {
                    self.stage = Stage::Done;
                    return Ok(true);
                };
                if !e.file.dir && ride_start(e.name()).is_some() {
                    self.rides += 1;
                    keep(&mut self.oldest, e);
                }
            }
        }
        Ok(false)
    }

    /// The volume's data size, bytes; None until read or after a failure.
    pub fn total(&self) -> Option<u64> {
        let fat = self.fat.as_ref().filter(|_| !self.failed)?;
        Some(fat.clusters_bytes(fat.clusters()))
    }

    /// The free space, bytes; None until counted or after a failure.
    pub fn free(&self) -> Option<u64> {
        let (fat, n) = (self.fat.as_ref().filter(|_| !self.failed)?, self.free?);
        Some(fat.clusters_bytes(n))
    }

    /// The rides in `\Activities`, once all are counted.
    pub fn rides(&self) -> Option<u32> {
        (self.done() && !self.failed).then_some(self.rides)
    }

    /// Ride `k` of the oldest (0 the oldest): its file entry.
    pub fn oldest(&self, k: usize) -> Option<Entry> {
        self.oldest.get(k).copied().flatten()
    }
}

/// Keeps the [`OLDEST`] rides by name: `YYMMDDhhmmss.fit` sorts by date, so the oldest first.
fn keep(oldest: &mut [Option<Entry>; OLDEST], e: Entry) {
    let Some(at) = oldest.iter().position(|o| o.is_none_or(|o| o.name() > e.name())) else { return };
    // at < OLDEST: the rides from `at` on move one place down, the last one falls off
    oldest.copy_within(at..OLDEST - 1, at + 1);
    if let Some(slot) = oldest.get_mut(at) {
        *slot = Some(e);
    }
}

/// A size for a menu value: "7.4 GB", "512.0 MB", "880 B" (none: "--"). Bytes are shown to a tenth from the KB on.
pub fn size_text<const N: usize>(l: &mut Line<N>, bytes: Option<u64>) {
    let Some(bytes) = bytes else { return l.push_str("--") };
    let (unit, div) = match bytes {
        0..1024 => ("B", 1),
        1024..0x10_0000 => ("KB", 1024),
        0x10_0000..0x4000_0000 => ("MB", 0x10_0000),
        _ => ("GB", 0x4000_0000),
    };
    let tenths = bytes * 10 / div;
    if div == 1 || tenths >= 100 {
        // whole units from 10 up, as a label beside it has room for 7 characters
        l.push_u32(u32::try_from(tenths / 10).unwrap_or(u32::MAX));
    } else {
        l.push_u32(u32::try_from(tenths / 10).unwrap_or(0));
        l.push_str(".");
        l.push_u32(u32::try_from(tenths % 10).unwrap_or(0));
    }
    l.push_str(" ");
    l.push_str(unit);
}

/// A ride's day for a menu label, "26-10-23" as its file name says it (a label beside its size has 8 characters).
pub fn day_text<const N: usize>(l: &mut Line<N>, (y, m, d, _, _): Start) {
    let two = |l: &mut Line<N>, v: u32| {
        if v < 10 {
            l.push_str("0");
        }
        l.push_u32(v);
    };
    two(l, u32::from(y % 100));
    l.push_str("-");
    two(l, u32::from(m));
    l.push_str("-");
    two(l, u32::from(d));
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use crate::fat::SECTOR;
    use std::vec::Vec;

    #[test]
    fn sizes_read_as_the_menu_shows_them() {
        let text = |b: Option<u64>| {
            let mut l = Line::<12>::new();
            size_text(&mut l, b);
            std::string::String::from(l.as_str())
        };
        assert_eq!(text(None), "--");
        assert_eq!(text(Some(880)), "880 B");
        assert_eq!(text(Some(1536)), "1.5 KB");
        assert_eq!(text(Some(512 << 20)), "512 MB");
        assert_eq!(text(Some(13 << 10)), "13 KB");
        assert_eq!(text(Some(7_400_000_000)), "6.8 GB");
    }

    /// The survey of a whole volume, a few sectors a pass.
    fn survey(img: Vec<u8>) -> Survey {
        let mut d = crate::fat::tests::Ram(img, 0);
        let mut s = Survey::new();
        while !s.step(&mut d, 3) {}
        s
    }

    #[test]
    fn free_space_is_fsinfos_or_the_fats_count() {
        let img = crate::fat::tests::image();
        let mut d = crate::fat::tests::Ram(img.clone(), 0);
        let fat = Fat::mount(&mut d).unwrap();
        let (mut next, mut free) = (2, 0);
        assert!(fat.count_free(&mut d, &mut next, &mut free, u32::MAX).unwrap());
        assert_eq!(fat.fsinfo_free(&mut d), Some(free), "the test volume's FSInfo is whole");
        let whole = Some(fat.clusters_bytes(free));
        assert_eq!(survey(img.clone()).free(), whole);
        // the FSInfo lead signature damaged: no count from it, the survey counts the FAT to the same
        let info = usize::from(u16::from_le_bytes([img[48], img[49]])) * SECTOR;
        let mut bad = img.clone();
        bad[info] = 0;
        assert_eq!(fat.fsinfo_free(&mut crate::fat::tests::Ram(bad.clone(), 0)), None);
        assert_eq!(survey(bad).free(), whole);
        // a count of 0xffffffff ("not known") or past the clusters: the same fallback
        let mut unknown = img.clone();
        unknown[info + 488..info + 492].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(survey(unknown).free(), whole);
    }

    #[test]
    fn rides_are_counted_and_the_oldest_kept_first() {
        let img = crate::storage::trash::tests::volume();
        let mut d = crate::fat::tests::Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, "Activities").unwrap();
        let mut c = fat.cursor(dir).unwrap();
        let mut names = Vec::new();
        while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
            if !e.file.dir && ride_start(e.name()).is_some() {
                names.push(std::string::String::from_utf8_lossy(e.name()).into_owned());
            }
        }
        names.sort();
        let s = survey(img);
        assert!(!names.is_empty());
        assert_eq!(s.rides(), Some(names.len() as u32));
        for (k, want) in names.iter().take(OLDEST).enumerate() {
            assert_eq!(s.oldest(k).map(|e| e.name().to_vec()), Some(want.as_bytes().to_vec()));
        }
        assert!(s.oldest(OLDEST).is_none());
    }

    #[test]
    fn a_ride_label_has_its_day() {
        let mut l = Line::<16>::new();
        day_text(&mut l, (2026, 10, 3, 8, 5));
        assert_eq!(l.as_str(), "26-10-03");
    }
}
