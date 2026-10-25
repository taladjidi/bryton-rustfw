//! What Bryton's stock firmware left on the shared volume (AGENTS.md, "Share the storage with stock"), read only:
//! - **its rides** (`\Activities\YYMMDDhhmmss.fit`, ghidra/STORAGE_AND_MAPS.md 2.1): [`Scan`] goes through them
//!   a bounded number of bytes per call, so the app spreads it over its passes and stays responsive; it gives the
//!   power curve of the last [`WINDOW_DAYS`] days, CP and W' ([`crate::power_curve`]) and the distance and time
//!   of this week, this year and all time ([`Summary`]), and the ride history's index ([`Scan::index`]). Each
//!   ride is decoded once ever: its [`Entry`] (64 bytes) goes into the [`Cache`], this firmware's own
//!   `\Rustfw\scan.bin`, which a start reads first (the totals at once) so that only new or changed rides are
//!   read; the sums are made from the entries (again on a new day: the windows move);
//! - **its settings** (`\System\*.ini`): [`import`] maps what has an equivalent here ([`Imported`]).
//!
//! A ride counts only if its file is whole (header, CRC) and has its session: `tmp.fit` (stock's ride in
//! progress) and the GNSS logs (`.ubx`) do not have a ride's name and are never opened. The ride's day is its
//! name's (local time, as stock names them). Nothing of stock's is ever written: only the cache, in this firmware's
//! folder ([`Cache::write`]); a ride HISTORY's DELETE moved away ([`crate::trash`]) leaves the cache at once
//! ([`Scan::forget`]).
//!
//! Stock keeps a summary per ride (`\System\History\Summary\<name>.sum` / `.csum`, ghidra/STORAGE_AND_MAPS.md
//! 3), which would be cheaper to read than the FIT file, but its layout is not established (version-dependent
//! offsets, medium confidence in the Ghidra notes), and this firmware's own rides have none: not used.

use crate::fat::{self, Cursor, Disk, DiskWrite, Fat, File, Seek};
use crate::fitread::{Msg, Reader};
use crate::history::{Acc, Index, Ride, Start};
use crate::ini::{get, number};
use crate::nrf::SensorId;
use crate::page::{CELLS, Page};
use crate::power_curve::{Curve, Rolling};
use crate::ui::Field;

/// The power curve covers the rides of this many days back.
pub const WINDOW_DAYS: u16 = 90;
/// Bytes read from the card per disk read: 16 sectors in one multi-block command.
pub const IO: usize = 8192;

/// Distance and time of some rides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Rides.
    pub rides: u16,
    /// Distance, m.
    pub dist_m: u32,
    /// Timer (moving) time, s.
    pub time_s: u32,
}

impl Totals {
    fn add(&mut self, r: &Ride) {
        self.rides = self.rides.saturating_add(1);
        self.dist_m = self.dist_m.saturating_add(r.dist_m);
        self.time_s = self.time_s.saturating_add(r.timer_s);
    }
}

/// What a scan found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// The best powers of the rides in the window.
    pub curve: Curve,
    /// This week (from Monday), this calendar year, all time.
    pub week: Totals,
    /// See `week`.
    pub year: Totals,
    /// See `week`.
    pub all: Totals,
    /// Rides in the window with power.
    pub with_power: u16,
    /// Files with a ride's name that were not whole rides (cut, bad CRC, no session, unreadable).
    pub skipped: u16,
    /// Bytes read.
    pub bytes: u32,
}

/// Where a scan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Never started.
    Idle,
    /// Going; [`Scan::step`] again.
    Running,
    /// Every ride read.
    Done,
    /// The volume or the folder could not be read.
    Failed(fat::Error),
}

/// The day of a ride from its file name, `YYMMDDhhmmss.fit` (stock's, 2-digit year from 2000), as days since
/// 1970; None for any other name.
///
/// ```
/// use bryton_rustfw::stock::ride_day;
/// assert_eq!(ride_day(b"261023081500.FIT"), Some(20_749)); // 2026-10-23
/// assert_eq!([ride_day(b"tmp.fit"), ride_day(b"20261023081500.ubx"), ride_day(b"261323081500.fit")], [None; 3]);
/// ```
pub fn ride_day(name: &[u8]) -> Option<u16> {
    let (y, m, d, _, _) = ride_start(name)?;
    u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).ok()
}

/// The start of a ride from its file name, `YYMMDDhhmmss.fit` (local time); None for any other name.
///
/// ```
/// use bryton_rustfw::stock::ride_start;
/// assert_eq!(ride_start(b"261023081500.fit"), Some((2026, 10, 23, 8, 15)));
/// ```
pub fn ride_start(name: &[u8]) -> Option<Start> {
    let (digits, ext) = (name.get(..12)?, name.get(12..)?);
    if !ext.eq_ignore_ascii_case(b".fit") || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let two = |i: usize| digits.get(i..i + 2).map_or(0, |d| (d[0] - b'0') * 10 + (d[1] - b'0'));
    let (y, m, d) = (2000 + u16::from(two(0)), two(2), two(4));
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || two(6) > 23 || two(8) > 59 || two(10) > 59 {
        return None;
    }
    Some((y, m, d, two(6), two(8)))
}

/// Stock's data pages at most (`[Page1]` to `[Page4]`).
pub const STOCK_PAGES: usize = 4;

/// Rides the cache keeps at most (`CACHE_RIDES` x [`ENTRY`] bytes: 256 KB in AXI SRAM, main.rs
/// `make_scan_cache`). Past it, a new ride is read at every start and not counted.
// ponytail: 4096 rides is 20 years at 200 a year; a bigger table (or one paged from the file) when that is near.
pub const CACHE_RIDES: usize = 4096;
/// Bytes of a ride in the cache ([`Entry`]).
pub const ENTRY: usize = 64;
/// Bytes of the cache file's header.
const HEAD: usize = 64;
/// The cache's folder (this firmware's own, the recorder's) and its name.
pub const CACHE_DIR: &str = "Rustfw";
/// See [`CACHE_DIR`].
pub const CACHE_NAME: &str = "scan.bin";
const MAGIC: [u8; 4] = *b"RFSC";
/// Raised whenever an entry's layout or meaning changes: an older cache is then rebuilt.
const VERSION: u16 = 1;
/// The cache in memory: the file's image, header and entries, in whole sectors.
pub const CACHE_BYTES: usize = (HEAD + CACHE_RIDES * ENTRY).next_multiple_of(fat::SECTOR);

/// A ride as the cache keeps it: what the overview, the power curve and the history need of it, so its file is
/// decoded once ever. Its identity is its file (first cluster and size) and its start (its name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The ride as the history lists it.
    pub ride: Ride,
    /// A whole ride with its session, counted; false for a file with a ride's name that is not one (cut, bad CRC,
    /// no session), kept so that it is not read again. A file the card or the FAT failed to give is not kept: read
    /// again at the next start.
    pub counted: bool,
    /// Its best powers per [`crate::power_curve::DURATIONS`], W (0: none).
    pub best: [u16; crate::power_curve::N],
}

/// The bytes that name a ride in the cache: its first cluster, size and start (an entry's first 14).
fn key(file: &File, (y, mo, d, h, mi): Start) -> [u8; 14] {
    let mut k = [0u8; 14];
    k[0..4].copy_from_slice(&file.cluster.to_le_bytes());
    k[4..8].copy_from_slice(&file.size.to_le_bytes());
    k[8..10].copy_from_slice(&y.to_le_bytes());
    k[10..14].copy_from_slice(&[mo, d, h, mi]);
    k
}

impl Entry {
    /// Its 64 bytes, little endian: 0 the key (first cluster u32, size u32, year u16, month, day, hour, minute),
    /// 14 flags (bit 0: counted), 15 average and 16 maximum heart rate (0xff: none), 18 ascent, 20 average power,
    /// 22 NP (u16, 0xffff: none), 24 moving time s, 28 distance m (u32), 32 seconds with power, 34 work kJ (u16,
    /// saturated: 18 hours, 65 MJ), 36 the 13 best powers (u16).
    pub fn to_bytes(&self) -> [u8; ENTRY] {
        let r = &self.ride;
        let mut b = [0u8; ENTRY];
        b[..14].copy_from_slice(&key(&r.file, r.start));
        b[14] = u8::from(self.counted);
        (b[15], b[16]) = (r.avg_hr.map_or(0xff, |v| v.min(0xfe)), r.max_hr.map_or(0xff, |v| v.min(0xfe)));
        let u16_ = |v: Option<u16>| v.map_or(0xffff, |v| v.min(0xfffe)).to_le_bytes();
        let sat = |v: u32| (v.min(0xffff) as u16).to_le_bytes();
        b[18..20].copy_from_slice(&u16_(r.ascent_m));
        b[20..22].copy_from_slice(&u16_(r.avg_power));
        b[22..24].copy_from_slice(&u16_(r.np));
        b[24..28].copy_from_slice(&r.timer_s.to_le_bytes());
        b[28..32].copy_from_slice(&r.dist_m.to_le_bytes());
        b[32..34].copy_from_slice(&sat(r.power_s));
        b[34..36].copy_from_slice(&sat(r.kj));
        for (i, w) in self.best.iter().enumerate() {
            b[36 + 2 * i..38 + 2 * i].copy_from_slice(&w.to_le_bytes());
        }
        b
    }

    /// The entry in `b` (at least [`ENTRY`] bytes; None if shorter).
    pub fn from_bytes(b: &[u8]) -> Option<Entry> {
        let b: &[u8; ENTRY] = b.get(..ENTRY)?.try_into().ok()?;
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let opt16 = |i: usize| Some(u16_at(i)).filter(|&v| v != 0xffff);
        let opt8 = |v: u8| Some(v).filter(|&v| v != 0xff);
        let ride = Ride {
            file: File { cluster: u32_at(0), size: u32_at(4), dir: false },
            start: (u16_at(8), b[10], b[11], b[12], b[13]),
            timer_s: u32_at(24),
            dist_m: u32_at(28),
            ascent_m: opt16(18),
            avg_power: opt16(20),
            np: opt16(22),
            power_s: u32::from(u16_at(32)),
            kj: u32::from(u16_at(34)),
            avg_hr: opt8(b[15]),
            max_hr: opt8(b[16]),
        };
        let best = core::array::from_fn(|i| u16_at(36 + 2 * i));
        Some(Entry { ride, counted: b[14] & 1 != 0, best })
    }

    /// The ride's day, days since 1970.
    fn day(&self) -> u16 {
        let (y, m, d, _, _) = self.ride.start;
        u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0)
    }
}

/// The scan's cache (`\Rustfw\scan.bin`): an [`Entry`] per ride file of `\Activities` read once, so a start reads
/// only the rides that are new or changed, and drops the ones that are gone. In memory it is the file's image.
///
/// File: a 64-byte header (`RFSC`, version u16, entry size u16, entry count u32, then a FIT CRC-16 of the header's
/// first 12 bytes followed by the entries), the entries, the file's size exactly 64 + count x 64. Anything else (a
/// cut while it was written, another version, a damaged sector) is ignored whole and the rides are read again: a
/// cache is never trusted in part.
pub struct Cache {
    bytes: [u8; CACHE_BYTES],
    len: usize,
    seen: [u32; CACHE_RIDES / 32],
    /// The entry after the last one found: the next ride listed is usually that one (entries keep the directory's
    /// order).
    hint: usize,
    /// The file on the card checked out when read, or was written whole since: no cut to mend. False at first (all
    /// zeros: the cache lands in .bss, link.x).
    whole: bool,
    /// Not whole: the FAT copies made equal up to this sector ([`Cache::prepare`]), and to the end.
    sync: u32,
    synced: bool,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    /// Empty.
    pub const fn new() -> Self {
        Cache {
            bytes: [0; CACHE_BYTES],
            len: 0,
            seen: [0; CACHE_RIDES / 32],
            hint: 0,
            whole: false,
            sync: 0,
            synced: false,
        }
    }

    /// Entries kept.
    pub fn len(&self) -> usize {
        self.len
    }

    /// No entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entry `i`.
    pub fn get(&self, i: usize) -> Option<Entry> {
        if i >= self.len {
            return None;
        }
        Entry::from_bytes(self.bytes.get(HEAD + i * ENTRY..)?)
    }

    fn mark(&mut self, i: usize) {
        if let Some(w) = self.seen.get_mut(i / 32) {
            *w |= 1 << (i % 32);
        }
    }

    fn was_seen(&self, i: usize) -> bool {
        self.seen.get(i / 32).is_some_and(|w| w & 1 << (i % 32) != 0)
    }

    /// The entry with key `k`, from the hint on, then from the start.
    fn find(&mut self, k: &[u8; 14]) -> Option<usize> {
        let at = |i: usize| HEAD + i * ENTRY;
        let hit = |c: &Cache, i: usize| c.bytes.get(at(i)..at(i) + 14) == Some(&k[..]);
        let i = (self.hint..self.len).chain(0..self.hint.min(self.len)).find(|&i| hit(self, i))?;
        self.hint = i + 1;
        Some(i)
    }

    /// Adds `e`, marked seen; false when full.
    fn push(&mut self, e: &Entry) -> bool {
        let Some(slot) = self.bytes.get_mut(HEAD + self.len * ENTRY..HEAD + (self.len + 1) * ENTRY) else {
            return false;
        };
        if self.len >= CACHE_RIDES {
            return false;
        }
        slot.copy_from_slice(&e.to_bytes());
        self.mark(self.len);
        self.len += 1;
        true
    }

    /// Adds entry bytes `e` as they are (the fuzz tests' caches).
    #[cfg(test)]
    pub(crate) fn push_raw(&mut self, e: &[u8; ENTRY]) {
        if let Some(slot) = self.bytes.get_mut(HEAD + self.len * ENTRY..HEAD + (self.len + 1) * ENTRY) {
            slot.copy_from_slice(e);
            self.len += 1;
        }
    }

    /// Keeps only the entries seen since [`Scan::start`] (their order kept); true if any went.
    fn keep_seen(&mut self) -> bool {
        let mut kept = 0;
        for i in 0..self.len {
            if !self.was_seen(i) {
                continue;
            }
            if kept != i {
                self.bytes.copy_within(HEAD + i * ENTRY..HEAD + (i + 1) * ENTRY, HEAD + kept * ENTRY);
            }
            kept += 1;
        }
        let gone = kept != self.len;
        self.bytes.get_mut(HEAD + kept * ENTRY..).unwrap_or(&mut []).fill(0); // the file's padding
        (self.len, self.seen, self.hint) = (kept, [0; CACHE_RIDES / 32], 0);
        gone
    }

    /// The CRC of the header's first 12 bytes and the entries.
    fn crc(&self) -> u16 {
        let head = self.bytes.get(..12).unwrap_or(&[]);
        crate::fit::crc(crate::fit::crc(0, head), self.bytes.get(HEAD..HEAD + self.len * ENTRY).unwrap_or(&[]))
    }

    /// The header written for the entries kept; the file's size.
    fn seal(&mut self) -> usize {
        let mut h = [0u8; HEAD];
        h[..4].copy_from_slice(&MAGIC);
        h[4..6].copy_from_slice(&VERSION.to_le_bytes());
        h[6..8].copy_from_slice(&(ENTRY as u16).to_le_bytes());
        h[8..12].copy_from_slice(&(self.len as u32).to_le_bytes());
        self.bytes[..HEAD].copy_from_slice(&h);
        let crc = self.crc();
        self.bytes[12..14].copy_from_slice(&crc.to_le_bytes());
        HEAD + self.len * ENTRY
    }

    /// The file's image, whole sectors (after [`Cache::seal`]).
    fn sectors(&self) -> &[u8] {
        let n = (HEAD + self.len * ENTRY).next_multiple_of(fat::SECTOR);
        self.bytes.get(..n).unwrap_or(&[])
    }

    /// Takes the `size` bytes just read into [`Cache::bytes`] as the file, if they check out; else empty.
    fn take(&mut self, size: usize) -> bool {
        let b = &self.bytes;
        let n = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
        let ok = b[..4] == MAGIC
            && b[4..6] == VERSION.to_le_bytes()
            && b[6..8] == (ENTRY as u16).to_le_bytes()
            && b[14..HEAD].iter().all(|&x| x == 0)
            && n <= CACHE_RIDES
            && size == HEAD + n * ENTRY;
        self.len = if ok { n } else { 0 };
        let ok = ok && self.crc() == u16::from_le_bytes([b[12], b[13]]);
        if !ok {
            self.len = 0;
            self.bytes.fill(0); // in place: a 256 KB temporary would not fit the stack
        }
        self.bytes.get_mut(HEAD + self.len * ENTRY..).unwrap_or(&mut []).fill(0);
        (self.seen, self.hint, self.whole) = ([0; CACHE_RIDES / 32], 0, ok);
        ok
    }

    /// Reads it from the volume, whole or not at all; false when there is none or it does not check out.
    pub fn read(&mut self, d: &mut impl Disk) -> bool {
        let mut fat = match Fat::mount(d) {
            Ok(f) => f,
            Err(_) => return self.take(0),
        };
        let f = fat.open(d, CACHE_DIR).and_then(|dir| fat.open_in(d, dir, CACHE_NAME));
        let size = match f {
            Ok(f) if !f.dir && (f.size as usize) <= CACHE_BYTES => fat.read_at(d, &f, 0, &mut self.bytes),
            _ => Ok(0),
        };
        self.take(size.unwrap_or(0))
    }

    /// Before a write after a cache that did not check out (or none): the FAT copies made equal
    /// ([`Fat::sync_fats_from`]), `sectors` of each FAT at most a call, so the 15 MB of the unit's FATs (3 to 4 s
    /// in one go) are spread over passes; true once done (at once when the cache is whole). A cut midway leaves the
    /// cache as it was (not whole): the next start does it again from the first sector.
    pub fn prepare(&mut self, d: &mut impl DiskWrite, sectors: u32) -> Result<bool, fat::Error> {
        if self.whole || self.synced {
            return Ok(true);
        }
        let mut fat = Fat::mount(d)?;
        let (next, _) = fat.sync_fats_from(d, self.sync, sectors)?;
        self.sync = next;
        self.synced = next >= fat.fat_sectors();
        Ok(self.synced)
    }

    /// Writes it as `\Rustfw\scan.bin` (created if needed) with the FAT writer's order (fat/write.rs): an old file
    /// is first made one that does not check out (its first sector zeroed), then the sectors are written over it,
    /// then its size. A cut anywhere leaves the volume whole for stock and a computer, and a file the next start
    /// ignores (its rides read again); the second FAT may then differ from the first in the sector being written,
    /// so a write after a cache that did not check out (or none) makes them equal first ([`Cache::prepare`] over
    /// passes, else [`Fat::sync_fats`] here: both FATs read). `time`: the FAT entry's (DOS date, time).
    pub fn write(&mut self, d: &mut impl DiskWrite, time: (u16, u16)) -> Result<(), fat::Error> {
        let size = self.seal();
        let mut fat = Fat::mount(d)?;
        // once: a write that fails may leave the copies different again
        let synced = core::mem::take(&mut self.synced);
        self.sync = 0;
        if !self.whole && !synced {
            fat.sync_fats(d)?;
        }
        let dir = fat.ensure_dir(d, fat.root(), CACHE_DIR, time)?;
        self.whole = false; // until it is written whole
        let mut h = match fat.locate(d, dir, CACHE_NAME) {
            Err(fat::Error::NotFound) => fat.create(d, dir, CACHE_NAME, time)?,
            other => {
                let mut h = other?;
                fat.write_sectors(d, &mut h, 0, &[0; fat::SECTOR])?;
                h
            }
        };
        fat.write_sectors(d, &mut h, 0, self.sectors())?;
        fat.set_size(d, &mut h, size as u32, time)?;
        self.whole = true;
        Ok(())
    }
}

/// A scan of stock's rides, a bounded amount per [`Scan::step`], over the [`Cache`] when it has one.
pub struct Scan {
    status: Status,
    fat: Option<Fat>,
    dir: Option<Cursor>,
    /// The ride being read: its file, how far, its day, its start; where its chain was left.
    file: Option<(File, u32, u16, Start)>,
    seek: Seek,
    reader: Reader,
    rolling: Rolling,
    /// Its session and totals so far.
    acc: Acc,
    /// Today, the first day of this week and of this year, days since 1970.
    today: u16,
    week: u16,
    year: u16,
    cache: Option<&'static mut Cache>,
    /// The cache was read (or tried), it changed since it was read or written, `out` is whole.
    loaded: bool,
    dirty: bool,
    shown: bool,
    /// Files listed so far (shown as progress).
    pub listed: u16,
    /// Rides read (not found in the cache) since the scan started.
    pub read: u16,
    /// What was found so far.
    pub out: Summary,
    /// The rides read whole, newest first (the ride history).
    pub index: Index,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

/// The first day of the week and of the year of `day` (days since 1970).
fn weeks(today: u16, y: u16) -> (u16, u16) {
    let day = |y, m, d| u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0);
    (today - (today + 3) % 7, day(y, 1, 1)) // 1970-01-01 was a Thursday: (days + 3) % 7 is 0 on Monday
}

impl Scan {
    /// No scan yet.
    pub const fn new() -> Self {
        Scan {
            status: Status::Idle,
            fat: None,
            dir: None,
            file: None,
            seek: Seek::new(),
            reader: Reader::new(),
            rolling: Rolling::new(),
            acc: Acc::new(),
            today: 0,
            week: 0,
            year: 0,
            cache: None,
            loaded: false,
            dirty: false,
            shown: false,
            listed: 0,
            read: 0,
            out: Summary {
                curve: Curve { watts: [0; crate::power_curve::N], day: [0; crate::power_curve::N] },
                week: Totals { rides: 0, dist_m: 0, time_s: 0 },
                year: Totals { rides: 0, dist_m: 0, time_s: 0 },
                all: Totals { rides: 0, dist_m: 0, time_s: 0 },
                with_power: 0,
                skipped: 0,
                bytes: 0,
            },
            index: Index::new(),
        }
    }

    /// Gives the scan its cache: from then on it reads it before the rides and keeps every ride read in it.
    pub fn set_cache(&mut self, c: &'static mut Cache) {
        self.cache = Some(c);
    }

    /// The cache, if it has one.
    pub fn cache(&self) -> Option<&Cache> {
        self.cache.as_deref()
    }

    /// Bytes of rides read since the scan started.
    pub fn read_bytes(&self) -> u32 {
        self.out.bytes
    }

    /// Where it is.
    pub fn status(&self) -> Status {
        self.status
    }

    /// [`Scan::out`] and [`Scan::index`] hold every ride: a scan finished, or the cache read.
    pub fn ready(&self) -> bool {
        self.shown
    }

    /// The cache holds rides not written yet.
    pub fn unsaved(&self) -> bool {
        self.dirty && self.status != Status::Running
    }

    fn set_today(&mut self, (y, m, d): (u16, u8, u8)) {
        self.today = u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0);
        (self.week, self.year) = weeks(self.today, y);
    }

    /// Reads the cache once (nothing without one), and shows its rides at once: totals, curve, history.
    pub fn load(&mut self, d: &mut impl Disk, date: (u16, u8, u8)) {
        if self.loaded || self.cache.is_none() {
            return;
        }
        if self.status != Status::Running {
            self.set_today(date);
        }
        self.read_cache(d);
    }

    fn read_cache(&mut self, d: &mut impl Disk) {
        self.loaded = true;
        if let Some(c) = &mut self.cache
            && c.read(d)
        {
            self.recount();
            self.shown = true;
        }
    }

    /// A new day (`date`) moves the week, the year and the curve's window: counted again from the cache, without
    /// reading a ride (nothing without a cache, or while a scan runs).
    pub fn new_day(&mut self, date: (u16, u8, u8)) {
        let (y, m, d) = date;
        let today = u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0);
        if today != self.today && self.cache.is_some() && self.shown && self.status != Status::Running {
            self.set_today(date);
            self.recount();
        }
    }

    /// Starts again from the first ride, with today's date (y, m, d) for the window and the totals. With a cache,
    /// what it shows stays until the scan ends.
    pub fn start(&mut self, date: (u16, u8, u8)) {
        self.set_today(date);
        (self.status, self.fat, self.dir, self.file, self.listed, self.read) =
            (Status::Running, None, None, None, 0, 0);
        self.out.bytes = 0;
        match &mut self.cache {
            Some(c) => (c.seen, c.hint) = ([0; CACHE_RIDES / 32], 0),
            None => (self.out, self.shown) = (Summary::default(), false),
        }
        if self.cache.is_none() {
            self.index.clear();
        }
    }

    /// The volume may have been written since the last step (this firmware's rides, settings, the cache): the FAT
    /// sector kept is read again, so no chain is followed through a stale one.
    pub fn forget_fat(&mut self) {
        if let Some(f) = &mut self.fat {
            f.forget();
        }
    }

    /// A ride moved away (HISTORY's DELETE, [`crate::trash`]): its entry dropped from the cache, and the totals, the
    /// power curve and the index counted again at once, no ride read; the cache written later ([`Scan::save`]). A
    /// scan running starts again (its marks follow the entries' places). Without a cache the sums cannot be
    /// undone: the rides are read again from `date`.
    pub fn forget(&mut self, file: &File, start: Start, date: (u16, u8, u8)) {
        let Some(c) = &mut self.cache else { return self.start(date) };
        if let Some(i) = c.find(&key(file, start)) {
            let len = c.len;
            c.bytes.copy_within(HEAD + (i + 1) * ENTRY..HEAD + len * ENTRY, HEAD + i * ENTRY);
            c.bytes.get_mut(HEAD + (len - 1) * ENTRY..HEAD + len * ENTRY).unwrap_or(&mut []).fill(0);
            (c.len, c.hint) = (len - 1, 0);
            self.dirty = true;
        }
        if self.status == Status::Running {
            self.start(date);
        }
        self.recount();
    }

    /// Goes on for about `budget` bytes read (a directory entry counts 32); false once it is done or failed.
    pub fn step(&mut self, d: &mut impl Disk, budget: u32) -> bool {
        let mut spent = 0u32;
        while self.status == Status::Running && spent < budget {
            if let Err(e) = self.advance(d, &mut spent) {
                self.status = Status::Failed(e);
            }
        }
        self.status == Status::Running
    }

    /// Writes the cache if it changed ([`Cache::write`]), after [`Cache::prepare`] with `sectors` of each FAT a
    /// call (still [`Scan::unsaved`] until it is done); once only on a failure (the next start reads again what
    /// was not kept).
    pub fn save(&mut self, d: &mut impl DiskWrite, time: (u16, u16), sectors: u32) -> Result<(), fat::Error> {
        if !self.unsaved() {
            return Ok(());
        }
        let Some(c) = &mut self.cache else {
            self.dirty = false;
            return Ok(());
        };
        match c.prepare(d, sectors) {
            Ok(false) => Ok(()),
            Ok(true) => {
                self.dirty = false;
                c.write(d, time)
            }
            Err(e) => {
                self.dirty = false;
                Err(e)
            }
        }
    }

    #[inline(never)] // its read buffer lives in its own frame, not in the caller's (the main loop's)
    fn advance(&mut self, d: &mut impl Disk, spent: &mut u32) -> Result<(), fat::Error> {
        if !self.loaded && self.cache.is_some() {
            *spent += IO as u32;
            self.read_cache(d);
            return Ok(());
        }
        let fat = match &mut self.fat {
            Some(f) => f,
            None => {
                *spent += fat::SECTOR as u32;
                self.fat.insert(Fat::mount(d)?)
            }
        };
        let dir = match &mut self.dir {
            Some(c) => c,
            None => {
                let f = fat.open(d, "Activities")?;
                *spent += fat::SECTOR as u32;
                self.dir.insert(fat.cursor(f)?)
            }
        };
        let Some((file, offset, day, start)) = self.file else {
            *spent += 32;
            match fat.list(d, dir)? {
                None => self.finish(),
                Some(e) => {
                    if let (false, Some(day), Some(start)) = (e.file.dir, ride_day(e.name()), ride_start(e.name())) {
                        self.listed = self.listed.saturating_add(1);
                        if let Some(c) = &mut self.cache
                            && let Some(i) = c.find(&key(&e.file, start))
                        {
                            c.mark(i); // read before, unchanged
                            return Ok(());
                        }
                        self.file = Some((e.file, 0, day, start));
                        (self.reader, self.acc, self.seek) = (Reader::new(), Acc::new(), Seek::new());
                        self.rolling.reset();
                    }
                }
            }
            return Ok(());
        };
        let mut io = [0u8; IO]; // on the stack for this call only: the scan stays small between passes
        let n = match fat.read_from(d, &file, offset, &mut io, &mut self.seek) {
            Ok(n) => n,
            Err(_) if self.cache.is_some() => {
                // the card or the FAT failed (a read error, a chain changed under a FAT sector kept): not kept as
                // not a ride, or it would stay out for good; read again at the next start (2026-10-25 review)
                self.file = None;
                return Ok(());
            }
            Err(_) => {
                self.end_file(false); // one bad file: skipped, the scan goes on
                return Ok(());
            }
        };
        *spent += n as u32;
        self.out.bytes = self.out.bytes.saturating_add(n as u32);
        let (rolling, acc) = (&mut self.rolling, &mut self.acc);
        self.reader.push_slice(io.get(..n).unwrap_or(&[]), |m| {
            if let Msg::Record(r) = m {
                rolling.sample(r.time, r.power); // every ride's bests: the cache keeps them, the window moves
            }
            acc.take(&m);
        });
        match self.reader.result() {
            Some(r) => self.end_file(r.is_ok()),
            None if n == 0 => self.end_file(false), // the file ended before the FIT data did
            None => self.file = Some((file, offset + n as u32, day, start)),
        }
        Ok(())
    }

    /// The ride read: its entry kept in the cache (counted at the end), or counted now without one. Counted only
    /// if it was whole, with a session.
    fn end_file(&mut self, whole: bool) {
        let Some((file, _, _, start)) = self.file.take() else { return };
        self.read = self.read.saturating_add(1);
        let ride = self.acc.ride(file, start).filter(|_| whole);
        let counted = ride.is_some();
        let ride = ride.unwrap_or(Ride { file, start, ..crate::history::NONE });
        let e = Entry { ride, counted, best: self.rolling.best };
        match &mut self.cache {
            // ponytail: a full cache keeps no more (CACHE_RIDES)
            Some(c) => self.dirty |= c.push(&e),
            None => self.count(&e),
        }
    }

    /// Every ride listed: the cache keeps those (gone files dropped) and counts them all again.
    fn finish(&mut self) {
        self.status = Status::Done;
        if let Some(c) = &mut self.cache {
            self.dirty |= c.keep_seen();
            self.recount();
        }
        self.shown = true;
    }

    /// `out` and `index` from the cache's entries.
    fn recount(&mut self) {
        let bytes = self.out.bytes;
        self.out = Summary { bytes, ..Summary::default() };
        self.index.clear();
        let n = self.cache.as_ref().map_or(0, |c| c.len);
        for i in 0..n {
            if let Some(e) = self.cache.as_ref().and_then(|c| c.get(i)) {
                self.count(&e);
            }
        }
    }

    /// One ride into `out` and `index`.
    fn count(&mut self, e: &Entry) {
        let o = &mut self.out;
        if !e.counted {
            o.skipped = o.skipped.saturating_add(1);
            return;
        }
        let (r, day) = (&e.ride, e.day());
        self.index.add(*r);
        o.all.add(r);
        if day >= self.year {
            o.year.add(r);
        }
        if day >= self.week {
            o.week.add(r);
        }
        if e.best[0] > 0 && self.today.saturating_sub(day) < WINDOW_DAYS {
            o.with_power = o.with_power.saturating_add(1);
            o.curve.merge(&e.best, day);
        }
    }

    /// Reads stock's settings files ([`import`]), each into a buffer of [`IO`] bytes on the stack: `\System\DeviceData.ini` for the bike
    /// in use and the backlight, then `UserProfile.ini`, `Sensor.ini` and that bike's `Bike<n>Profile.ini`. A missing file gives
    /// nothing from it.
    #[inline(never)] // as `advance`
    pub fn import(&self, d: &mut impl Disk) -> Result<Imported, fat::Error> {
        let mut io = [0u8; IO];
        let mut fat = Fat::mount(d)?;
        let mut imp = Imported::default();
        let read = |fat: &mut Fat, d: &mut _, path: &str, io: &mut [u8; IO]| -> usize {
            fat.open(d, path).and_then(|f| fat.read_at(d, &f, 0, io)).unwrap_or(0)
        };
        let n = read(&mut fat, d, "System/DeviceData.ini", &mut io);
        let bike = imp.device(io.get(..n).unwrap_or(&[]));
        let n = read(&mut fat, d, "System/UserProfile.ini", &mut io);
        imp.user(io.get(..n).unwrap_or(&[]));
        let n = read(&mut fat, d, "System/Sensor.ini", &mut io);
        imp.sensors(io.get(..n).unwrap_or(&[]));
        let mut path = *b"System/Bike1Profile.ini";
        path[11] = b'1' + bike;
        let n = read(&mut fat, d, core::str::from_utf8(&path).unwrap_or(""), &mut io);
        imp.bike(io.get(..n).unwrap_or(&[]));
        Ok(imp)
    }
}

/// Stock's settings that have an equivalent here. None: not found, or out of the range this firmware takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// FTP, W (UserProfile.ini `[FTP] BaseVale`, stock's spelling).
    pub ftp: Option<u16>,
    /// Max heart rate, bpm (`[MHR] BaseVale`).
    pub max_hr: Option<u16>,
    /// Weight, 0.1 kg (`[UserProfile] Weight_kg`).
    pub weight_dg: Option<u16>,
    /// Critical power, W (`[UserProfile] CP`).
    pub cp: Option<u16>,
    /// W', J (`[UserProfile] Wbalance`: inferred to be W' in J from its values and stock's W' balance field).
    pub w_prime: Option<u32>,
    /// Wheel circumference, mm (Sensor.ini, a speed sensor's `Wheel`; unit inferred).
    pub wheel_mm: Option<u16>,
    /// ANT+ sensors per [`crate::sensors::Kind::ALL`] (Sensor.ini).
    pub sensors: [Option<SensorId>; crate::sensors::Kind::ALL.len()],
    /// What the Di2 buttons do, by [`crate::sensors::Press`] (Sensor.ini, the paired Di2's `leftpress`,
    /// `rightpress`, `leftlongpress`, `rightlongpress`: [`crate::menu::Remote::from_stock`]).
    pub remote: Option<[crate::menu::Remote; 4]>,
    /// Ride pages (the bike's `[Page1]` to `[Page4]`, the enabled ones first).
    pub pages: [Option<Page>; STOCK_PAGES],
    /// Eat and drink reminders, minutes of ride time, and the heart rate alert, bpm (the bike's `[Reminder<n>]`).
    pub eat_min: Option<u16>,
    /// See `eat_min`.
    pub drink_min: Option<u16>,
    /// See `eat_min`.
    pub hr_alert: Option<u16>,
    /// The backlight follows the ambient light (DeviceData.ini `[DeviceSys] brightnessAuto=1`).
    pub light_auto: bool,
}

/// Stock's field ids (`GridN=` in Bike`<n>`Profile.ini) and the field here that shows the same value, or the
/// closest. The ids are the indexes of stock's field title table (GetDataFieldTitle 0xd0150ef8, English titles
/// read from its string blocks: ghidra/STORAGE_AND_MAPS.md 3.2; bryton-S510-mods docs/how-it-works.md "Data
/// fields" confirms 27, 28, 29). A ring or bar of a value maps to the value. 12 is "Non" in stock: the patched stock
/// firmware of bryton-S510-mods shows DFA a1 there (P9), so a page that has it was made on it (its W' balance,
/// P10, was field 180). Ids not here (altitude, zones, lap averages, pedal dynamics...) have no equivalent yet.
pub const STOCK_FIELDS: [(u8, Field); 52] = {
    use Field::*;
    [
        (0, Kcal),         // "Calories"
        (1, Temp),         // "Temp."
        (2, Speed),        // "Speed"
        (3, AvgSpeed),     // "Avg Speed"
        (4, MaxSpeed),     // "Max Speed"
        (5, Dist),         // "Distance"
        (7, Time),         // "Time" (of day, inferred: "Ride Time" and "Trip Time" are the timers)
        (8, Timer),        // "Ride Time"
        (12, Dfa),         // "Non"; P9's DFA a1
        (16, Ascent),      // "Alt. Gain"
        (18, Grade),       // "Gradient"
        (21, Hr),          // "Heart Rate"
        (22, AvgHr),       // "Avg HR"
        (23, MaxHr),       // "Max HR"
        (24, Cadence),     // "Cadence"
        (25, AvgCadence),  // "Avg CAD"
        (27, Power30s),    // "30s PW"
        (28, Power3s),     // "3s PW"
        (29, Power),       // "Power"
        (30, MaxPower),    // "Max Power"
        (31, AvgPower),    // "Avg Power"
        (44, Kcal),        // "Calories"
        (48, LapDist),     // "LapDistance"
        (50, LapTime),     // "LapTime"
        (58, TotalKm),     // "ODO"
        (65, Wkg),         // "w/kg" (3 s power here)
        (67, If),          // "IF"
        (68, Np),          // "NP"
        (69, Tss),         // "TSS"
        (70, Balance),     // "CurPB-LR" (pedal balance)
        (78, Power10s),    // "10s Power"
        (80, Kj),          // "Total Work"
        (92, ToGo),        // "To Dest."
        (95, Map),         // "Turn Info": the map in a data page (ghidra recovered_src/ui/page_switch.c)
        (96, Speed),       // "SPD Ring"
        (97, Speed),       // "SPD Bar"
        (98, Cadence),     // "CAD Ring"
        (99, Cadence),     // "CAD Bar"
        (100, Hr),         // "HR Ring"
        (101, Hr),         // "HR Bar"
        (102, Power),      // "PW Ring"
        (103, Power),      // "PW Bar"
        (104, Power3s),    // "3s PW Ring"
        (105, Power10s),   // "10s PW Ring"
        (106, Power30s),   // "30s PW Ring"
        (120, Vam),        // "VAM"
        (141, StepTarget), // "Target PW"
        (142, StepTarget), // "Target Cad"
        (143, StepTarget), // "Target HR"
        (144, StepTime),   // "Step Rem."
        (147, StepTime),   // "Step Rem."
        (180, Wbal),       // "W' Bal" (ghidra recovered_src/ui/data_fields.c)
    ]
};

fn stock_field(id: u32) -> Option<Field> {
    STOCK_FIELDS.iter().find(|(i, _)| u32::from(*i) == id).map(|&(_, f)| f)
}

/// `prefix`, `suffix`, then the decimal `n` (a section or key name such as `HRSensor1`, `Grid10`), in `buf`.
fn named<'a>(buf: &'a mut [u8; 16], prefix: &str, suffix: &str, n: u32) -> &'a str {
    let mut l = crate::text::Line::<16>::new();
    l.push_str(prefix);
    l.push_str(suffix);
    l.push_u32(n);
    let b = l.as_str().as_bytes();
    let len = b.len().min(16);
    buf[..len].copy_from_slice(&b[..len]);
    core::str::from_utf8(&buf[..len]).unwrap_or("")
}

impl Imported {
    /// From UserProfile.ini.
    pub fn user(&mut self, t: &[u8]) {
        // to u16 only once in range: `as` first would turn 65736 into 200 (the review, 2026-10-24)
        let in_range = |v: u32, r: core::ops::RangeInclusive<u16>| u16::try_from(v).ok().filter(|v| r.contains(v));
        let ftp = number(t, "FTP", "BaseVale", 0).and_then(|v| in_range(v, crate::menu::FTP_RANGE));
        self.ftp = ftp.map(|v| (v + 2) / 5 * 5); // the menu steps by 5
        self.max_hr = number(t, "MHR", "BaseVale", 0).and_then(|v| in_range(v, crate::menu::MAX_HR_RANGE));
        self.weight_dg =
            number(t, "UserProfile", "Weight_kg", 1).filter(|v| (200..=2500).contains(v)).map(|v| v as u16);
        self.cp = number(t, "UserProfile", "CP", 0).filter(|v| (50..=1000).contains(v)).map(|v| v as u16);
        self.w_prime = number(t, "UserProfile", "Wbalance", 0).filter(|v| (1000..=100_000).contains(v));
    }

    /// From Sensor.ini: per kind, the first active ANT sensor (`HWType=0`, inferred: the BLE ones carry a `Mac`)
    /// of the sections `HRSensor<n>`, `SPDSensor<n>`, `CADSensor<n>`, `COMSensor<n>` (combined speed and cadence,
    /// inferred from the name), `PWRSensor<n>`, `ESSSensor<n>` (ANT+ shifting) and `DI2Sensor<n>`
    /// (`Cfg_SensorIniReadEntry` 0xd0200e94); a speed sensor's wheel; the Di2's button actions.
    pub fn sensors(&mut self, t: &[u8]) {
        const PREFIX: [&str; 7] = ["HR", "SPD", "CAD", "COM", "PWR", "ESS", "DI2"];
        for (k, (prefix, slot)) in PREFIX.iter().zip(&mut self.sensors).enumerate() {
            for n in 1..=4u32 {
                let mut buf = [0u8; 16];
                let section = named(&mut buf, prefix, "Sensor", n);
                let id = number(t, section, "ID", 0).filter(|&id| id > 0 && id < 1 << 20);
                let ant = number(t, section, "HWType", 0) == Some(0) && number(t, section, "active", 0) == Some(1);
                if let (Some(id), true, None) = (id, ant, *slot) {
                    // a 20-bit device number: the top 4 bits go in the transmission type's upper nibble
                    *slot = Some(SensorId { device: id as u16, trans: ((id >> 16) as u8) << 4 });
                    if *prefix == "DI2" {
                        let press = |key| crate::menu::Remote::from_stock(number(t, section, key, 0).unwrap_or(0));
                        let keys = ["leftpress", "rightpress", "leftlongpress", "rightlongpress"];
                        self.remote = Some(keys.map(press));
                    }
                }
                let wheel = number(t, section, "Wheel", 0).filter(|w| (1000..=3000).contains(w));
                if (k == 1 || k == 3) && self.wheel_mm.is_none() {
                    self.wheel_mm = wheel.map(|w| w as u16);
                }
            }
        }
    }

    /// From DeviceData.ini: the backlight, and the bike in use (`[DeviceSys] ActBike`, 0 for Bike1Profile.ini:
    /// Cfg_SetActiveBike writes the 0-based index there, ghidra notes/agent_a1.md), returned.
    pub fn device(&mut self, t: &[u8]) -> u8 {
        // brightnessAuto: written with brightness and AutoBrnVal by Cfg_DevDataApplySettingById (agent_a2.md); 1 on,
        // inferred. The manual `brightness` is not taken: its scale is not checked.
        self.light_auto = number(t, "DeviceSys", "brightnessAuto", 0) == Some(1);
        number(t, "DeviceSys", "ActBike", 0).unwrap_or(0).min(2) as u8
    }

    /// From Bike`<n>`Profile.ini: the pages ([`Imported::pages`]) and the reminders ([`Imported::reminders`]).
    pub fn bike(&mut self, t: &[u8]) {
        self.pages(t);
        self.reminders(t);
    }

    /// `[Page1]` to `[Page4]`, stock's data pages (the others have a `name` and are lap, map, climb... pages),
    /// the enabled ones in order: `isEnabled` 1 or 2 (stock's FUN_d015f248 turns 2 into 1, and a data page shows
    /// when it is 1: ApplicationglobalSetting_IsPageAvailable 0xd01535e0), each with the fields of its
    /// `Grid<type>` line (`type`: the field count; Page_LoadBikeProfilePage reads one list per count, so the others
    /// are what the page would show with another count). The fields with an equivalent here, in stock's order and
    /// each once, at most 8, give the first layout of that many fields; a page with none is left out.
    pub fn pages(&mut self, t: &[u8]) {
        let mut out = 0;
        for n in 1..=4u32 {
            let mut buf = [0u8; 16];
            let section = named(&mut buf, "Page", "", n);
            let count = number(t, section, "type", 0).unwrap_or(0);
            if number(t, section, "isEnabled", 0).unwrap_or(0) == 0 || get(t, section, "name").is_some() {
                continue;
            }
            let mut buf = [0u8; 16];
            let Some(list) = get(t, section, named(&mut buf, "Grid", "", count)) else { continue };
            let mut fields = [Field::Time; CELLS];
            let mut k = 0;
            for id in list.split(|&b| b == b',').filter_map(|v| crate::ini::decimal(v, 0)) {
                let Some(f) = stock_field(id) else { continue };
                if fields.get(..k).is_some_and(|had| had.contains(&f)) {
                    continue; // a ring and the number of one value
                }
                if let Some(slot) = fields.get_mut(k) {
                    *slot = f;
                    k += 1;
                }
            }
            let page = crate::page::preset_of(k).and_then(|id| Page::preset(id, fields));
            if let (Some(page), Some(slot)) = (page, self.pages.get_mut(out)) {
                *slot = Some(page);
                out += 1;
            }
        }
    }

    /// `[Reminder1]` to `[Reminder6]` (ghidra recovered_src/ride/reminders.c): the ones on (`status=1`) that have
    /// an equivalent here. Every so many minutes of trip time (`WhichReminder=0`, `ReminderFreq=2` repeats,
    /// `Timer` in s): `ReminderType` 0 Drink, 1 Eat; heart rate over (`WhichReminder=5`, `HR`). Rounded to the
    /// menu's steps of 5.
    pub fn reminders(&mut self, t: &[u8]) {
        let fives = |v: u32, r: core::ops::RangeInclusive<u16>| {
            u16::try_from(v.saturating_add(2) / 5 * 5).ok().map(|v| v.clamp(*r.start(), *r.end()))
        };
        for n in 1..=6u32 {
            let mut buf = [0u8; 16];
            let section = named(&mut buf, "Reminder", "", n);
            let key = |k| number(t, section, k, 0);
            if key("status") != Some(1) {
                continue;
            }
            match (key("WhichReminder"), key("ReminderFreq"), key("ReminderType")) {
                (Some(0), Some(2), Some(ty @ 0..=1)) => {
                    let min = key("Timer").map(|s| s / 60).filter(|&m| m > 0).and_then(|m| fives(m, 5..=120));
                    let slot = if ty == 0 { &mut self.drink_min } else { &mut self.eat_min };
                    *slot = slot.or(min);
                }
                (Some(5), _, _) => {
                    let hr = key("HR").filter(|&h| h > 0).and_then(|h| fives(h, 100..=230));
                    self.hr_alert = self.hr_alert.or(hr);
                }
                _ => {}
            }
        }
    }
}

/// Stock's settings from the three files' text ([`Imported::user`], [`Imported::sensors`], [`Imported::bike`]).
pub fn import(user: &[u8], sensor: &[u8], bike: &[u8]) -> Imported {
    let mut i = Imported::default();
    i.user(user);
    i.sensors(sensor);
    i.bike(bike);
    i
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, image};
    use crate::power_curve::fit_cp;

    const TODAY: (u16, u8, u8) = (2026, 10, 23);

    fn day(y: u16, m: u8, d: u8) -> u16 {
        (crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400) as u16
    }

    /// The made-up rides of tools/fat_image.py, against values worked out by hand there.
    #[test]
    fn scans_the_synthetic_rides() {
        let mut d = Ram(image(), 0);
        let mut s = Scan::new();
        assert_eq!(s.status(), Status::Idle);
        s.start(TODAY);
        let mut passes = 0;
        while s.step(&mut d, 4096) {
            passes += 1;
            assert!(passes < 1000);
        }
        assert_eq!(s.status(), Status::Done);
        assert!(passes > 5, "spread over passes: {passes}");
        let o = s.out;
        assert_eq!((s.listed, o.skipped, o.with_power), (6, 2, 2), "6 ride names, a bad CRC and a cut file");
        assert_eq!(o.all, Totals { rides: 4, dist_m: 37_400, time_s: 4200 });
        assert_eq!(o.year, Totals { rides: 3, dist_m: 31_400, time_s: 3600 });
        assert_eq!(o.week, Totals { rides: 2, dist_m: 19_400, time_s: 2400 });
        let (a, b) = (day(2026, 10, 20), day(2026, 10, 1));
        assert_eq!(o.curve.watts, [400, 400, 400, 400, 400, 400, 350, 310, 300, 275, 265, 260, 0]);
        assert_eq!(o.curve.day, [a, a, a, a, a, a, a, a, b, a, a, a, 0], "480 s from the ride with the stop");
        assert!(fit_cp(&o.curve).is_some());
        // the same in one go
        let mut one = Scan::new();
        one.start(TODAY);
        while one.step(&mut d, u32::MAX) {}
        assert_eq!(one.out, o);
    }

    /// The test volume with `k` more rides (a ride name a minute apart on 2026-10-10, each a copy of the
    /// 30-minute ride of 2026-10-20), through the FAT writer.
    pub(crate) fn with_rides(k: u32) -> std::vec::Vec<u8> {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let ride = crate::fat::tests::read_all(&mut fat, &mut d, "Activities/261020080000.fit").unwrap();
        let mut padded = ride.clone();
        padded.resize(ride.len().div_ceil(fat::SECTOR) * fat::SECTOR, 0);
        let dir = fat.open(&mut d, "Activities").unwrap();
        for i in 0..k {
            let name = std::format!("261010{:02}{:02}00.fit", i / 60, i % 60);
            let mut h = fat.create(&mut d, dir, &name, (0, 0)).unwrap();
            fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
            fat.set_size(&mut d, &mut h, ride.len() as u32, (0, 0)).unwrap();
        }
        d.0
    }

    /// A cache of its own (as main.rs gives the app one), on the heap for the test.
    pub(crate) fn cache() -> &'static mut Cache {
        std::boxed::Box::leak(std::boxed::Box::new(Cache::new()))
    }

    /// A whole scan of `d`, `budget` bytes a step: the scan and its passes.
    fn scan(d: &mut impl Disk, cache: Option<&'static mut Cache>, budget: u32) -> (Scan, u32) {
        let mut s = Scan::new();
        if let Some(c) = cache {
            s.set_cache(c);
        }
        s.start(TODAY);
        let mut passes = 1;
        while s.step(d, budget) {
            passes += 1;
            assert!(passes < 100_000);
        }
        (s, passes)
    }

    /// Adds `name` to `\Activities`: a copy of the 30-minute ride of 2026-10-20.
    fn add_ride(d: &mut Ram, name: &str) {
        let mut fat = Fat::mount(d).unwrap();
        let ride = crate::fat::tests::read_all(&mut fat, d, "Activities/261020080000.fit").unwrap();
        let mut padded = ride.clone();
        padded.resize(ride.len().div_ceil(fat::SECTOR) * fat::SECTOR, 0);
        let dir = fat.open(d, "Activities").unwrap();
        let mut h = fat.create(d, dir, name, (0, 0)).unwrap();
        fat.write_sectors(d, &mut h, 0, &padded).unwrap();
        fat.set_size(d, &mut h, ride.len() as u32, (0, 0)).unwrap();
    }

    /// What a scan costs on a volume of 70 rides (the owner's count): bytes, disk reads, passes, host time;
    /// printed. With a cache that was written, no ride is read.
    #[test]
    fn scan_cost() {
        let mut d = Ram(with_rides(64), 0);
        let t = std::time::Instant::now();
        let (mut s, passes) = scan(&mut d, Some(cache()), 96 * 1024);
        let ns = t.elapsed().as_nanos() as u64;
        let b = u64::from(s.out.bytes);
        std::println!(
            "scan, first start: {} rides, {b} bytes, {} disk reads, {passes} passes of 96 KB, {} ns/byte on the host",
            s.listed,
            d.1,
            ns / b.max(1)
        );
        assert_eq!((s.out.all.rides, s.read), (4 + 64, 70));
        s.save(&mut d, (0, 0), u32::MAX).unwrap();
        d.1 = 0;
        let t = std::time::Instant::now();
        let (again, passes) = scan(&mut d, Some(cache()), 96 * 1024);
        std::println!(
            "scan, next start: {} rides, {} bytes of rides, {} disk reads, {passes} passes, {} us on the host",
            again.listed,
            again.out.bytes,
            d.1,
            t.elapsed().as_micros()
        );
        assert_eq!((again.out.bytes, again.read), (0, 0), "no ride read");
        assert_eq!(again.out, Summary { bytes: 0, ..s.out });
    }

    /// The cache: written after the first scan, read at the next start (totals at once, nothing read), and a ride
    /// added, removed or changed is picked up; the volume checks out under fsck.
    #[test]
    fn the_cache_follows_the_rides() {
        let mut d = Ram(image(), 0);
        let (mut first, _) = scan(&mut d, Some(cache()), 4096);
        let want = Summary { bytes: 0, ..first.out };
        assert_eq!((want.all.rides, want.skipped), (4, 2));
        assert!(first.unsaved());
        first.save(&mut d, (0, 0), u32::MAX).unwrap();
        assert!(!first.unsaved());
        assert_eq!(crate::fat::write::tests::fsck(&d.0), Ok(0));
        // the next start: the totals before any ride is read
        let mut s = Scan::new();
        s.set_cache(cache());
        assert!(!s.ready());
        s.load(&mut d, TODAY);
        assert!(s.ready());
        assert_eq!((s.out, s.index.len()), (want, first.index.len()));
        assert_eq!(s.index.get(0), first.index.get(0));
        s.start(TODAY);
        while s.step(&mut d, 4096) {}
        assert_eq!((s.read, s.out, s.unsaved()), (0, want, false), "nothing read, nothing to write");
        // a ride added, one removed (the 2025 one), one changed (cut short)
        add_ride(&mut d, "261022180000.fit");
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, "Activities").unwrap();
        let old = fat.locate(&mut d, dir, "250315090000.fit").unwrap();
        fat.remove(&mut d, &old).unwrap();
        let mut cut = fat.locate(&mut d, dir, "261001070000.fit").unwrap();
        fat.set_size(&mut d, &mut cut, 1000, (0, 0)).unwrap();
        s.start(TODAY);
        while s.step(&mut d, 4096) {}
        assert_eq!(s.read, 2, "the new ride and the changed one");
        let all = Totals { rides: 3, dist_m: 37_400 - 6000 - 12_000 + 14_400, time_s: 4200 - 600 - 1200 + 1800 };
        assert_eq!((s.out.all, s.out.skipped), (all, 3), "the cut one counts as not whole");
        assert!(s.unsaved());
        s.save(&mut d, (0, 0), u32::MAX).unwrap();
        // and once more from the card: the same as a scan without a cache
        let (c, _) = scan(&mut d, Some(cache()), 4096);
        let (none, _) = scan(&mut d, None, 4096);
        assert_eq!((c.read, c.out), (0, Summary { bytes: 0, ..none.out }));
        assert_eq!(crate::fat::write::tests::fsck(&d.0), Ok(1), "the removed ride's chain: the test's remove");
    }

    /// A new day moves the week, the year and the curve's window, from the cache: no ride read.
    #[test]
    fn a_new_day_counts_again_from_the_cache() {
        let mut d = Ram(image(), 0);
        let (s, _) = scan(&mut d, Some(cache()), 1 << 20);
        assert_eq!(s.out.week.rides, 2);
        let mut s = s;
        s.save(&mut d, (0, 0), u32::MAX).unwrap();
        let bytes = s.out.bytes;
        s.new_day((2026, 10, 26)); // the next Monday
        assert_eq!((s.out.week, s.out.year.rides, s.out.bytes), (Totals::default(), 3, bytes));
        s.new_day((2027, 3, 1)); // a new year, the October rides out of the 90 days
        assert_eq!((s.out.year.rides, s.out.all.rides, s.out.with_power), (0, 4, 0));
        assert_eq!(s.out.curve.watts, [0; crate::power_curve::N]);
    }

    /// A cache that does not check out is ignored whole and written again.
    #[test]
    fn a_damaged_cache_is_rebuilt() {
        let mut d = Ram(image(), 0);
        let (mut first, _) = scan(&mut d, Some(cache()), 4096);
        first.save(&mut d, (0, 0), u32::MAX).unwrap();
        let mut fat = Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, "Rustfw/scan.bin").unwrap();
        let lba = fat.lba(f.cluster) as usize * fat::SECTOR;
        for at in [5, 9, HEAD + 3, HEAD + 2 * ENTRY + 40] {
            d.0[lba + at] ^= 0x10;
            let mut s = Scan::new();
            s.set_cache(cache());
            s.load(&mut d, TODAY);
            assert!(!s.ready(), "byte {at} damaged: not taken");
            s.start(TODAY);
            while s.step(&mut d, 4096) {}
            assert_eq!((s.read, s.out.all), (6, first.out.all), "every ride read again");
            s.save(&mut d, (0, 0), u32::MAX).unwrap();
            let (c, _) = scan(&mut d, Some(cache()), 4096);
            assert_eq!(c.read, 0, "written whole again");
        }
    }

    /// A disk whose power goes after `.1` more sector writes: nothing written from then on.
    struct Cut(Ram, u32);

    impl Disk for Cut {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), fat::Error> {
            self.0.read(lba, buf)
        }
    }

    impl DiskWrite for Cut {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), fat::Error> {
            for (i, s) in buf.chunks(fat::SECTOR).enumerate() {
                if self.1 == 0 {
                    return Err(fat::Error::Io);
                }
                self.1 -= 1;
                self.0.write(lba + i as u32, s)?;
            }
            Ok(())
        }
    }

    /// The power cut after every sector of the cache's writes (the first one, which makes the folder and the
    /// file, and a later one over it): the volume stays whole for fsck, and the next start's totals are right.
    #[test]
    fn a_cut_while_the_cache_is_written() {
        let base = with_rides(16);
        let (want, _) = scan(&mut Ram(base.clone(), 0), None, 1 << 20);
        // a written cache, then a ride more for the second write
        let mut d = Ram(base.clone(), 0);
        let (mut s, _) = scan(&mut d, Some(cache()), 1 << 20);
        s.save(&mut d, (0, 0), u32::MAX).unwrap();
        add_ride(&mut d, "261022180000.fit");
        let (want2, _) = scan(&mut Ram(d.0.clone(), 0), None, 1 << 20);
        let mut cuts = 0;
        for (img, want) in [(base, want.out), (d.0, want2.out)] {
            let want = Summary { bytes: 0, ..want };
            for k in 0.. {
                let mut cut = Cut(Ram(img.clone(), 0), k);
                let mut s = Scan::new();
                s.set_cache(cache());
                s.load(&mut cut, TODAY);
                s.start(TODAY);
                while s.step(&mut cut, 1 << 20) {}
                let done = s.save(&mut cut, (0, 0), u32::MAX).is_ok();
                // the next start: right once its scan ends (the cache, or the rides read again), and its own write
                // mends what the cut left (FATs that differ)
                let mut d = cut.0;
                let mut next = Scan::new();
                next.set_cache(cache());
                next.load(&mut d, TODAY);
                next.start(TODAY);
                while next.step(&mut d, 1 << 20) {}
                assert_eq!(Summary { bytes: 0, ..next.out }, want, "the next start, cut after {k} sectors");
                next.save(&mut d, (0, 0), u32::MAX).unwrap();
                let f = crate::fat::write::tests::fsck(&d.0);
                assert!(f.is_ok(), "cut after {k} sectors: {f:?}");
                if done {
                    assert_eq!(next.read, 0);
                    break;
                }
                cuts += 1;
            }
        }
        assert!(cuts > 4, "{cuts} cuts");
    }

    /// A disk whose read of 4 KB or more at sectors `.1` .. `.1 + 64` fails once with `.3`.
    struct Flaky(Ram, u32, bool, fat::Error);

    impl Disk for Flaky {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), fat::Error> {
            if self.2 && (self.1..self.1 + 64).contains(&lba) && buf.len() >= 4096 {
                self.2 = false;
                return Err(self.3);
            }
            self.0.read(lba, buf)
        }
    }

    impl DiskWrite for Flaky {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), fat::Error> {
            self.0.write(lba, buf)
        }
    }

    /// A ride the card or the FAT failed to give once (a read error, a chain changed under a FAT sector kept:
    /// Corrupt) is not kept in the cache as not a ride: the next start reads it again and counts it (2026-10-25
    /// review: it stayed out for good).
    #[test]
    fn a_read_error_is_not_kept() {
        let base = image();
        let (good, _) = scan(&mut Ram(base.clone(), 0), None, 1 << 20);
        for e in [fat::Error::Io, fat::Error::Corrupt] {
            let mut d = Ram(base.clone(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let f = fat.open(&mut d, "Activities/261020080000.fit").unwrap();
            let mut fl = Flaky(d, fat.lba(f.cluster), true, e);
            let (mut s, _) = scan(&mut fl, Some(cache()), 1 << 20);
            assert!(!fl.2, "the error happened");
            assert_eq!(s.out.all.rides, good.out.all.rides - 1, "{e:?}: out this time");
            s.save(&mut fl, (0, 0), u32::MAX).unwrap();
            let (next, _) = scan(&mut fl, Some(cache()), 1 << 20);
            assert_eq!((next.read, next.out.all), (1, good.out.all), "{e:?}: read again at the next start");
        }
    }

    /// The scan's FAT sector is read again after [`Scan::forget_fat`]: a ride saved meanwhile (its chain in a FAT
    /// sector the scan kept from before) is read whole.
    #[test]
    fn a_ride_saved_during_the_scan() {
        let mut d = Ram(image(), 0);
        let mut s = Scan::new();
        s.start(TODAY);
        while s.listed < 2 {
            assert!(s.step(&mut d, 32));
        }
        add_ride(&mut d, "261022180000.fit"); // after the ones listed: the scan comes to it
        s.forget_fat();
        while s.step(&mut d, 4096) {}
        let (want, _) = scan(&mut d, None, 1 << 20);
        assert_eq!(s.out.all, want.out.all);
    }

    #[test]
    fn an_entry_in_bytes_and_back() {
        let ride = Ride {
            file: File { cluster: 1234, size: 99_000, dir: false },
            start: (2026, 10, 23, 8, 15),
            timer_s: 3600,
            dist_m: 30_000,
            ascent_m: Some(450),
            avg_power: None,
            np: Some(230),
            power_s: 3500,
            kj: 700,
            avg_hr: Some(140),
            max_hr: None,
        };
        let e = Entry { ride, counted: true, best: core::array::from_fn(|i| 600 - i as u16 * 20) };
        assert_eq!(Entry::from_bytes(&e.to_bytes()), Some(e));
        assert_eq!(Entry::from_bytes(&[0; ENTRY - 1]), None);
    }

    #[test]
    fn a_volume_without_rides() {
        let mut s = Scan::new();
        s.start(TODAY);
        while s.step(&mut Ram(std::vec::Vec::new(), 0), 1 << 20) {}
        assert_eq!(s.status(), Status::Failed(fat::Error::NotFat32));
    }

    /// Made-up settings files in stock's layout (the keys and sections of a unit's files, none of its values).
    #[test]
    fn imports_stock_settings() {
        let user = b"[Info]\nVer=1000\n\n[UserProfile]\nWbalance=21000\nCP=255\nWeight_kg=72.50000\n\n[FTP]\n\
            Zone1L=0\nBaseVale=263\n\n[MHR]\nZone7H=65535\nBaseVale=188\n\n[LTHR]\nBaseVale=170\n\0";
        let sensor = b"[Sensor]\r\nHR_Num=2\r\n[HRSensor1]\r\nID=0\r\nHWType=0\r\nactive=1\r\n[HRSensor2]\r\n\
            ID=1234\r\nHWType=0\r\nactive=1\r\n[SPDSensor1]\r\nWheel=2105.000000\r\nID=70000\r\nHWType=0\r\n\
            active=1\r\n[PWRSensor1]\r\nID=4321\r\nHWType=1\r\nactive=1\r\n";
        // Page1 shown (isEnabled 2), its Grid3 the one shown; Page2 off; Page3 with more fields than a layout here,
        // a ring and its value, and ids without an equivalent (14 altitude, 32 zone); Page4 only those; Page5 a lap
        // page. Reminders: drink every 45 min (on), eat (off), HR over 172, a clock alarm (no equivalent).
        let bike = b"[Info]\nVer=6001\n[Page1]\nGrid2=96,5\nGrid3=2,21,7\ntype=3\nisEnabled=2\n\n[Page2]\n\
            Grid4=3,18,7,0\ntype=4\nisEnabled=0\n[Page3]\nGrid10=103,29,14,12,180,32,13,68,8,5\ntype=10\n\
            gridtype=1\nisEnabled=1\n[Page4]\nGrid2=14,32\ntype=2\nisEnabled=1\n[Page5]\nname=1012\ntype=6\n\
            isEnabled=1\nGrid6=2,2,2,2,2,2\n[Reminder1]\nTimer=2700\nWhichReminder=0\nReminderFreq=2\n\
            ReminderType=0\nstatus=1\n[Reminder2]\nTimer=3600\nWhichReminder=0\nReminderFreq=2\nReminderType=1\n\
            status=0\n[Reminder3]\nHR=172\nWhichReminder=5\nReminderFreq=1\nReminderType=5\nstatus=1\n\
            [Reminder4]\nAlerm_Hour=7\nWhichReminder=1\nReminderFreq=1\nReminderType=3\nstatus=1\n";
        let i = import(user, sensor, bike);
        let got = (i.ftp, i.max_hr, i.weight_dg, i.cp, i.w_prime);
        assert_eq!(got, (Some(265), Some(188), Some(725), Some(255), Some(21_000)));
        assert_eq!(i.wheel_mm, Some(2105));
        assert_eq!(i.sensors[0], Some(SensorId { device: 1234, trans: 0 }), "the first with an ID");
        assert_eq!(i.sensors[1], Some(SensorId { device: 4464, trans: 0x10 }), "20-bit device number 70000");
        assert_eq!(i.sensors[4], None, "a BLE power meter");
        use Field::*;
        let p0 = i.pages[0].unwrap();
        assert_eq!((p0.cells(), &p0.fields[..3]), (3, &[Speed, Hr, Time][..]));
        let p1 = i.pages[1].unwrap();
        let want = [Power, Dfa, Wbal, Np, Timer, Dist];
        assert_eq!((p1.cells(), &p1.fields[..6]), (6, &want[..]), "each once, the 6 with an equivalent");
        assert_eq!(i.pages[2], None, "page 4 has nothing mapped; page 5 is not a data page");
        assert_eq!((i.drink_min, i.eat_min, i.hr_alert), (Some(45), None, Some(170)), "HR to the menu's 5s");
        let mut d = Imported::default();
        assert_eq!((d.device(b"[DeviceSys]\nbrightnessAuto=1\nActBike=2\n"), d.light_auto), (2, true));
        assert_eq!((d.device(b"[DeviceSys]\nbrightnessAuto=0\n"), d.light_auto), (0, false));
        assert_eq!(import(b"", b"\0\0", b"[Page1"), Imported::default());
    }

    /// The owner's drive backups, when they are there (never in the repository): every ride read, aggregates
    /// printed only. `cargo test-host stock_backups -- --ignored --nocapture`, with BRYTON_DRIVE pointing at a
    /// copy of the drive's root.
    #[test]
    #[ignore]
    fn stock_backups() {
        let Ok(root) = std::env::var("BRYTON_DRIVE") else {
            std::println!("BRYTON_DRIVE not set: skipped");
            return;
        };
        let (mut rides, mut power, mut bad, mut bytes, mut same_max) = (0, 0, 0, 0, 0);
        for e in std::fs::read_dir(std::format!("{root}/Activities")).unwrap().flatten() {
            let name = e.file_name().into_string().unwrap();
            if ride_day(name.as_bytes()).is_none() {
                continue;
            }
            let f = std::fs::read(e.path()).unwrap();
            bytes += f.len();
            let (mut r, mut roll, mut session) = (Reader::new(), Rolling::new(), None);
            for &b in &f {
                match r.push(b) {
                    Some(Msg::Record(x)) => roll.sample(x.time, x.power),
                    Some(Msg::Session(s)) => session = Some(s),
                    _ => {}
                }
            }
            match (r.result(), session) {
                (Some(Ok(())), Some(s)) => {
                    rides += 1;
                    power += usize::from(roll.best[0] > 0);
                    // the 1 s best against the session's own maximum: a check of the reader
                    same_max += usize::from(roll.best[0] > 0 && s.max_power == Some(roll.best[0]));
                }
                _ => bad += 1,
            }
        }
        std::println!(
            "{rides} rides ({power} with power, {same_max} whose 1 s best is the session's maximum), {bad} not whole, {} MB",
            bytes >> 20
        );
        let sys = |f: &str| std::fs::read(std::format!("{root}/System/{f}")).unwrap_or_default();
        let mut i = Imported::default();
        let bike = std::format!("Bike{}Profile.ini", 1 + i.device(&sys("DeviceData.ini")));
        let bike = sys(&bike);
        i.user(&sys("UserProfile.ini"));
        i.sensors(&sys("Sensor.ini"));
        i.bike(&bike);
        let found = [i.ftp.is_some(), i.max_hr.is_some(), i.weight_dg.is_some(), i.cp.is_some(), i.w_prime.is_some()];
        let sensors = i.sensors.iter().filter(|s| s.is_some()).count();
        let pages = i.pages.iter().filter(|p| p.is_some()).count();
        std::println!("settings found (ftp, max hr, weight, cp, w'): {found:?}; {sensors} sensors, {pages} pages");
        let alerts = [i.eat_min.is_some(), i.drink_min.is_some(), i.hr_alert.is_some(), i.light_auto];
        std::println!("reminders and backlight found (eat, drink, hr, auto light): {alerts:?}");
        // how much of the data pages shown maps: counts and stock's generic field ids only, never the owner's values
        let (mut shown, mut mapped, mut unmapped) = (0, 0, std::collections::BTreeSet::new());
        for n in 1..=4 {
            let section = std::format!("Page{n}");
            let count = number(&bike, &section, "type", 0).unwrap_or(0);
            if number(&bike, &section, "isEnabled", 0).unwrap_or(0) == 0 || get(&bike, &section, "name").is_some() {
                continue;
            }
            let list = get(&bike, &section, &std::format!("Grid{count}")).unwrap_or(&[]);
            for id in list.split(|&b| b == b',').filter_map(|v| crate::ini::decimal(v, 0)) {
                shown += 1;
                match stock_field(id) {
                    Some(_) => mapped += 1,
                    None => _ = unmapped.insert(id),
                }
            }
        }
        std::println!(
            "data pages shown: {mapped} of {shown} fields have an equivalent; stock ids without: {unmapped:?}"
        );
        assert!(rides > 0);
    }
}
