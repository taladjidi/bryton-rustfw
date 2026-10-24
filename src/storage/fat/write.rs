//! The FAT32 write path: what the ride recorder needs and nothing more. Create a file or a folder with a long name,
//! write a file's sectors (clusters taken as it grows), set its size, give a chain a new name elsewhere (the move
//! of a finished ride), mark an entry deleted, and make the second FAT equal to the first.
//!
//! **Write order** (AGENTS.md, golden rule; "Share the storage with stock"): a power cut between any two sector
//! writes leaves a volume that stock (uC/FS), a computer and [`Fat`] accept. At worst a lost chain (clusters
//! marked used that no entry names, which fsck turns into a FILE0000.CHK and nothing else minds) or the second FAT
//! differing from the first in the one sector being written (the first is the one every reader uses;
//! [`Fat::sync_fats`] puts it right after a cut). Never a chain that runs into a free cluster, never a size longer
//! than its chain, never two entries on one chain, never a half-written directory entry:
//! - **a new cluster:** marked end of chain first, then linked from the chain's last (one FAT sector when both
//!   are in it, which is the usual case: one write per FAT copy; else the new one's sector first); each FAT sector
//!   is written to FAT 1, then FAT 2;
//! - **a file grows:** clusters first, then the data, then the size in its entry ([`Fat::set_size`]);
//! - **a file shrinks** (only after a resume): the size first, then the chain cut and the rest freed;
//! - **a new entry:** its cluster is taken first; the entry (long name pieces and the 8.3 entry) always lands in
//!   one sector, written in one write, so a cut leaves all of it or none. When the free slots at the end of the
//!   directory are too few in that sector, the entry goes into the next sector (a new cluster, zeroed, when the
//!   directory has no next sector) and then the slots left before it are marked deleted (0xe5), which is the write
//!   that makes it visible (the directory ends at the first 0x00 entry);
//! - **a move:** the old entry marked deleted, then the new one written. A cut between leaves the chain lost (the
//!   recorder's recovery record names it and finishes the move); the other order would leave two entries on one
//!   chain, which a computer's fsck calls cross-linked and a delete of either would free under the other;
//! - **a delete** only marks the entry (and its long name pieces) 0xe5: the rest of the entry, its first cluster
//!   included, stays, so a lost file can be found by hand. Only the recorder's own entries are ever deleted.
//! - **FSInfo** (free count, next free hint) is written after the FAT sectors it counts; a cut between leaves the
//!   count a little high, which the FAT32 spec allows (it is a hint) and [`Fat::sync_fats`] puts right.
//!
//! Never written: the boot sector, the volume label, the FAT's first two entries (media and dirty flags), any
//! sector past the volume (and the eMMC driver refuses any sector at or past B, BOARD.md 15). Never a format.
//!
//! Format: Microsoft's FAT32 File System Specification (fatgen103, 2000: the 0x00 end-of-directory rule, FSInfo's
//! signatures 0x41615252 / 0x61417272 / 0xaa550000 and its free count and next free at 488 / 492, the 28-bit
//! entries whose top 4 bits a writer keeps, DOS date and time) and en.wikipedia.org/wiki/Design_of_the_FAT_file_system
//! (long name entries, their checksum, `.` and `..`).
//!
//! Why own code and not a crate (checked 2026-10-24): `embedded-sdmmc` 0.9 creates 8.3 names only (stock's ride
//! names, `YYMMDDhhmmss.fit`, need a long name), wants an MBR (the unit has none), and its write order is not
//! documented: whether a cut leaves a free cluster in a chain is what this module must be able to say and test
//! (sim.rs `recorder_tests` cut the power after every sector). `fatfs` needs `std` or nightly `core_io`. This is
//! about 450 lines with no heap, every loop bounded by the volume's cluster count or FAT's 65536 entries a
//! directory.

use super::{DIR_ENTRIES, Disk, DiskWrite, Error, Fat, File, NAME, SECTOR, lfn_eq, short_eq, short_sum, u32_at};

/// End of chain, as written.
const EOC: u32 = 0x0fff_ffff;
/// Directory entries in a sector.
const ENTRIES: usize = SECTOR / 32;
/// Directory attribute.
pub const DIR: u8 = 0x10;
/// Archive attribute (a file).
pub const ARCHIVE: u8 = 0x20;

/// DOS date and time of a local time (year, month, day, hour, minute, second): (date, time), 2-second steps.
///
/// ```
/// assert_eq!(bryton_rustfw::fat::write::dos_time((2026, 10, 24, 13, 45, 31)), ((46 << 9) | (10 << 5) | 24, (13 << 11) | (45 << 5) | 15));
/// ```
pub fn dos_time((y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8)) -> (u16, u16) {
    let date = (y.clamp(1980, 2107) - 1980) << 9 | u16::from(mo & 0xf) << 5 | u16::from(d & 0x1f);
    (date, u16::from(h) << 11 | u16::from(mi & 0x3f) << 5 | u16::from(s / 2))
}

/// A file open for writing: where its 8.3 entry is, and its chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Handle {
    /// The directory sector holding its 8.3 entry, and the entry's index in that sector.
    pub entry_lba: u32,
    /// See `entry_lba`.
    pub slot: u8,
    /// First and last cluster, and how many the chain has.
    pub first: u32,
    /// See `first`.
    pub last: u32,
    /// See `first`.
    pub clusters: u32,
    /// Size in bytes, as its entry says.
    pub size: u32,
}

impl Handle {
    /// The file as the reader takes it.
    pub fn file(&self) -> File {
        File { cluster: self.first, size: self.size, dir: false }
    }
}

/// A name this writer gives: 1 to [`NAME`] printable ASCII characters, none that FAT forbids, no leading or
/// trailing space or dot.
fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=NAME).contains(&b.len())
        && b.iter().all(|&c| (0x20..0x7f).contains(&c) && !b"\"*/:<>?\\|".contains(&c))
        && ![b' ', b'.'].contains(&b[0])
        && b.last().is_some_and(|c| ![b' ', b'.'].contains(c))
}

/// The directory entries of `name`: its long name pieces (last first) then the 8.3 entry `short`; how many.
fn dir_entries(
    name: &str,
    short: [u8; 11],
    attr: u8,
    cluster: u32,
    size: u32,
    time: (u16, u16),
) -> ([[u8; 32]; 6], usize) {
    let mut out = [[0u8; 32]; 6];
    let units: [u16; 65] = core::array::from_fn(|i| match name.as_bytes().get(i) {
        Some(&c) => u16::from(c),
        None if i == name.len() => 0,
        None => 0xffff,
    });
    let pieces = name.len().div_ceil(13).min(5);
    let sum = short_sum(&short);
    const AT: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (k, e) in out.iter_mut().take(pieces).enumerate() {
        // entry k holds piece (pieces - k): the last piece first
        let n = pieces - k;
        e[0] = n as u8 | if k == 0 { 0x40 } else { 0 };
        e[11] = 0x0f;
        e[13] = sum;
        for (i, &a) in AT.iter().enumerate() {
            let u = units.get((n - 1) * 13 + i).copied().unwrap_or(0xffff);
            e[a..a + 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    let e = &mut out[pieces];
    e[..11].copy_from_slice(&short);
    e[11] = attr;
    let (date, t) = time;
    for at in [14, 22] {
        e[at..at + 2].copy_from_slice(&t.to_le_bytes());
    }
    for at in [16, 18, 24] {
        e[at..at + 2].copy_from_slice(&date.to_le_bytes());
    }
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    (out, pieces + 1)
}

impl Fat {
    /// Bytes per cluster.
    pub fn cluster_bytes(&self) -> u32 {
        self.spc * SECTOR as u32
    }

    fn write(&self, d: &mut impl DiskWrite, lba: u32, buf: &[u8]) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        if lba.checked_add(n).is_none_or(|end| end > self.total) || lba < self.fat_start {
            return Err(Error::Corrupt); // never the boot sector, FSInfo or past the volume
        }
        d.write(lba, buf)
    }

    /// Sets FAT entries in order, each FAT sector written to every FAT copy before the next sector is touched.
    fn set_entries(&mut self, d: &mut impl DiskWrite, changes: &[(u32, u32)]) -> Result<(), Error> {
        let mut dirty = false;
        for &(c, v) in changes {
            if !self.valid(c) {
                return Err(Error::Corrupt);
            }
            let lba = self.fat_start + c / (SECTOR as u32 / 4);
            if self.cached != Some(lba) {
                if dirty {
                    self.flush_fat(d)?;
                }
                self.entry(d, c)?; // loads the sector
            }
            let at = (c as usize % (SECTOR / 4)) * 4;
            let keep = u32_at(&self.sector, at) & 0xf000_0000; // the top 4 bits are reserved: kept
            if let Some(s) = self.sector.get_mut(at..at + 4) {
                s.copy_from_slice(&(keep | v).to_le_bytes());
            }
            dirty = true;
        }
        if dirty { self.flush_fat(d) } else { Ok(()) }
    }

    fn flush_fat(&mut self, d: &mut impl DiskWrite) -> Result<(), Error> {
        let Some(lba) = self.cached else { return Err(Error::Corrupt) };
        let s = self.sector;
        for k in 0..self.fats {
            if let Err(e) = self.write(d, lba + k * self.fat_size, &s) {
                self.cached = None; // the disk may hold either
                return Err(e);
            }
        }
        Ok(())
    }

    /// A free cluster from `from` on (wrapping), the volume's clusters at most.
    fn find_free(&mut self, d: &mut impl Disk, from: u32) -> Result<u32, Error> {
        let start = if self.valid(from) { from - 2 } else { 0 };
        for i in 0..self.clusters {
            let c = 2 + (start + i) % self.clusters;
            if self.entry(d, c)? == 0 {
                return Ok(c);
            }
        }
        Err(Error::Refused) // full
    }

    /// FSInfo's free count and next free hint, if the sector has its signatures.
    fn fsinfo_read(&mut self, d: &mut impl Disk) -> Result<Option<(u32, u32, [u8; SECTOR])>, Error> {
        if self.fsinfo == 0 {
            return Ok(None);
        }
        let mut s = [0u8; SECTOR];
        d.read(self.fsinfo, &mut s)?;
        let ok = u32_at(&s, 0) == 0x4161_5252 && u32_at(&s, 484) == 0x6141_7272 && u32_at(&s, 508) == 0xaa55_0000;
        Ok(ok.then(|| (u32_at(&s, 488), u32_at(&s, 492), s)))
    }

    /// FSInfo after `taken` clusters were allocated (negative: freed): the free count moved (unless unknown,
    /// 0xffffffff) and the hint set to `next`. `exact` replaces the count.
    fn fsinfo_update(
        &mut self,
        d: &mut impl DiskWrite,
        taken: i64,
        next: u32,
        exact: Option<u32>,
    ) -> Result<(), Error> {
        let Some((free, _, mut s)) = self.fsinfo_read(d)? else {
            if let (Some(n), 1) = (exact, self.fsinfo) {
                // signatures lost (a cut tore the sector): made again where every formatter puts it, sector 1
                let mut s = [0u8; SECTOR];
                s[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
                s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
                s[488..492].copy_from_slice(&n.to_le_bytes());
                s[492..496].copy_from_slice(&(if self.valid(next) { next } else { 2 }).to_le_bytes());
                s[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
                return d.write(self.fsinfo, &s);
            }
            return Ok(());
        };
        let free = match exact {
            Some(n) => n,
            None if free == u32::MAX => free,
            None => (i64::from(free) - taken).clamp(0, i64::from(self.clusters)) as u32,
        };
        s[488..492].copy_from_slice(&free.to_le_bytes());
        s[492..496].copy_from_slice(&(if self.valid(next) { next } else { 2 }).to_le_bytes());
        d.write(self.fsinfo, &s) // sector 1: below fat_start, so not through Fat::write's check
    }

    /// FSInfo's free cluster count, None when unknown (no FSInfo, or 0xffffffff). A few clusters high after a cut.
    pub fn free_clusters(&mut self, d: &mut impl Disk) -> Result<Option<u32>, Error> {
        Ok(self.fsinfo_read(d)?.map(|(free, _, _)| free).filter(|&n| n <= self.clusters))
    }

    /// The hint where to look for a free cluster: FSInfo's, else the start.
    fn hint(&mut self, d: &mut impl Disk) -> Result<u32, Error> {
        Ok(self.fsinfo_read(d)?.map_or(2, |(_, next, _)| next))
    }

    /// The chain from `first`: its last cluster and its length.
    fn chain_end(&mut self, d: &mut impl Disk, first: u32) -> Result<(u32, u32), Error> {
        if first == 0 {
            return Ok((0, 0));
        }
        if !self.valid(first) {
            return Err(Error::Corrupt);
        }
        let (mut c, mut n) = (first, 1);
        while let Some(next) = self.next(d, c)? {
            (c, n) = (next, n + 1);
            if n > self.clusters {
                return Err(Error::Corrupt); // a loop
            }
        }
        Ok((c, n))
    }

    /// Grows `h`'s chain to `want` clusters (end of chain first, then the link), then FSInfo.
    fn grow(&mut self, d: &mut impl DiskWrite, h: &mut Handle, want: u32) -> Result<(), Error> {
        let mut taken = 0;
        while h.clusters < want {
            let from = if h.clusters == 0 { self.hint(d)? } else { h.last + 1 };
            let c = self.find_free(d, from)?;
            if h.clusters == 0 {
                self.set_entries(d, &[(c, EOC)])?;
                h.first = c;
            } else {
                self.set_entries(d, &[(c, EOC), (h.last, c)])?;
            }
            (h.last, h.clusters, taken) = (c, h.clusters + 1, taken + 1);
        }
        if taken > 0 {
            self.fsinfo_update(d, taken, h.last + 1, None)?;
        }
        Ok(())
    }

    /// Writes `buf` (whole sectors) to `h`'s sectors from `sector` on, taking clusters as needed; one disk write
    /// per cluster touched. The size is [`Fat::set_size`]'s.
    pub fn write_sectors(
        &mut self,
        d: &mut impl DiskWrite,
        h: &mut Handle,
        sector: u32,
        buf: &[u8],
    ) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        if n == 0 || !buf.len().is_multiple_of(SECTOR) {
            return Err(Error::Corrupt);
        }
        let end = sector.checked_add(n).ok_or(Error::Corrupt)?;
        self.grow(d, h, end.div_ceil(self.spc))?;
        let mut c = self.nth(d, h.first, sector / self.spc)?;
        let mut at = sector;
        let mut done = 0;
        while at < end {
            let in_cluster = at % self.spc;
            if done > 0 && in_cluster == 0 {
                c = self.next(d, c)?.ok_or(Error::Corrupt)?;
            }
            let k = (self.spc - in_cluster).min(end - at);
            let bytes = buf.get(done * SECTOR..(done + k as usize) * SECTOR).ok_or(Error::Corrupt)?;
            self.write(d, self.lba(c) + in_cluster, bytes)?;
            (at, done) = (at + k, done + k as usize);
        }
        Ok(())
    }

    /// Reads `h`'s file sector `sector` (whole, past its size too: the chain's bytes).
    pub fn read_sector(
        &mut self,
        d: &mut impl Disk,
        h: &Handle,
        sector: u32,
        buf: &mut [u8; SECTOR],
    ) -> Result<(), Error> {
        let c = self.nth(d, h.first, sector / self.spc)?;
        self.read(d, self.lba(c) + sector % self.spc, buf)
    }

    /// Sets `h`'s size and write time in its entry; then, if the chain is longer than the size needs (a resume
    /// cut the file back), cuts it there and frees the rest (the size first: never a size past the chain).
    pub fn set_size(
        &mut self,
        d: &mut impl DiskWrite,
        h: &mut Handle,
        size: u32,
        time: (u16, u16),
    ) -> Result<(), Error> {
        let need = size.div_ceil(self.spc * SECTOR as u32).max(1);
        if need > h.clusters {
            return Err(Error::Corrupt);
        }
        let mut s = [0u8; SECTOR];
        self.read(d, h.entry_lba, &mut s)?;
        let at = usize::from(h.slot) * 32;
        let e = s.get_mut(at..at + 32).ok_or(Error::Corrupt)?;
        e[22..24].copy_from_slice(&time.1.to_le_bytes());
        e[24..26].copy_from_slice(&time.0.to_le_bytes());
        e[18..20].copy_from_slice(&time.0.to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        self.write(d, h.entry_lba, &s)?;
        h.size = size;
        self.trim(d, h, size)
    }

    /// Cuts `h`'s chain after the clusters `size` bytes need (at least one) and frees the rest: its size already
    /// that, or no entry at all ([`Fat::chain`]).
    pub fn trim(&mut self, d: &mut impl DiskWrite, h: &mut Handle, size: u32) -> Result<(), Error> {
        let need = size.div_ceil(self.spc * SECTOR as u32).max(1);
        if need < h.clusters {
            let keep = self.nth(d, h.first, need - 1)?;
            let mut rest = self.next(d, keep)?;
            self.set_entries(d, &[(keep, EOC)])?;
            (h.last, h.clusters) = (keep, need);
            let mut freed = 0i64;
            while let Some(c) = rest {
                rest = self.next(d, c)?;
                self.set_entries(d, &[(c, 0)])?;
                freed += 1;
                if freed > i64::from(self.clusters) {
                    return Err(Error::Corrupt);
                }
            }
            self.fsinfo_update(d, -freed, keep + 1, None)?;
        }
        Ok(())
    }

    /// The chain from `first` as a handle without an entry: its sectors can be written in place, never its size.
    pub fn chain(&mut self, d: &mut impl Disk, first: u32) -> Result<Handle, Error> {
        let (last, clusters) = self.chain_end(d, first)?;
        Ok(Handle { first, last, clusters, ..Handle::default() })
    }

    /// The entry called `name` in `dir`, opened for writing.
    pub fn locate(&mut self, d: &mut impl Disk, dir: File, name: &str) -> Result<Handle, Error> {
        let mut c = self.cursor(dir)?;
        while let Some((e, long)) = self.next_raw(d, &mut c)? {
            if (long && lfn_eq(&c.lfn, name)) || short_eq(&e, name) {
                let f = self.file_of(&e)?;
                let (last, clusters) = self.chain_end(d, f.cluster)?;
                let slot = c.entry.checked_sub(1).ok_or(Error::Corrupt)? as u8;
                let entry_lba = self.lba(c.cluster) + c.sector;
                return Ok(Handle { entry_lba, slot, first: f.cluster, last, clusters, size: f.size });
            }
        }
        Err(Error::NotFound)
    }

    /// Fails unless `name` is one this writer gives and nothing in `dir` has it.
    fn check_new(&mut self, d: &mut impl Disk, dir: File, name: &str) -> Result<(), Error> {
        if !valid_name(name) {
            return Err(Error::Refused);
        }
        match self.find(d, dir, name) {
            Err(Error::NotFound) => Ok(()),
            Ok(_) => Err(Error::Refused),
            Err(e) => Err(e),
        }
    }

    /// An 8.3 name for `name` that nothing in `dir` has: 8 characters of a hash of the long name (so no `~N`
    /// numbering to search), and the extension's first 3 letters or digits.
    fn alias(&mut self, d: &mut impl Disk, dir: File, name: &str) -> Result<[u8; 11], Error> {
        let ext = name.rfind('.').map_or("", |i| name.get(i + 1..).unwrap_or(""));
        let mut short = [b' '; 11];
        for (s, c) in short[8..].iter_mut().zip(ext.bytes().filter(u8::is_ascii_alphanumeric)) {
            *s = c.to_ascii_uppercase();
        }
        let fnv = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3));
        for salt in 0..16u64 {
            let mut h = fnv ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            for s in short[..8].iter_mut() {
                *s = b"0123456789ABCDEFGHIJKLMNOPQRSTUV"[(h & 31) as usize];
                h >>= 5;
            }
            let mut text = [0u8; 12];
            text[..8].copy_from_slice(&short[..8]);
            let mut n = 8;
            if short[8] != b' ' {
                text[8] = b'.';
                n = 9;
                for &c in short[8..].iter().filter(|&&c| c != b' ') {
                    text[n] = c;
                    n += 1;
                }
            }
            let text = core::str::from_utf8(&text[..n]).map_err(|_| Error::Corrupt)?;
            match self.find(d, dir, text) {
                Err(Error::NotFound) => return Ok(short),
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Refused)
    }

    /// Writes cluster `c` whole: sector 0 is `first`, the others zeros.
    fn fill_cluster(&mut self, d: &mut impl DiskWrite, c: u32, first: &[u8; SECTOR]) -> Result<(), Error> {
        self.write(d, self.lba(c), first)?;
        let zero = [0u8; 8 * SECTOR]; // 8 sectors a command: 4 commands for the unit's 32-sector clusters
        let mut k = 1;
        while k < self.spc {
            let n = (self.spc - k).min(8);
            self.write(d, self.lba(c) + k, zero.get(..n as usize * SECTOR).ok_or(Error::Corrupt)?)?;
            k += n;
        }
        Ok(())
    }

    /// Puts `entries` at the end of directory `dir`, in one sector (module doc); where the last one (the 8.3
    /// entry) went: its sector and index.
    fn add_entries(&mut self, d: &mut impl DiskWrite, dir: File, entries: &[[u8; 32]]) -> Result<(u32, u8), Error> {
        let n = entries.len();
        let put = |s: &mut [u8; SECTOR], at: usize| {
            for (k, e) in entries.iter().enumerate() {
                if let Some(slot) = s.get_mut((at + k) * 32..(at + k + 1) * 32) {
                    slot.copy_from_slice(e);
                }
            }
        };
        let mut cluster = dir.cluster;
        for _ in 0..DIR_ENTRIES / (ENTRIES as u32) {
            for sec in 0..self.spc {
                let lba = self.lba(cluster) + sec;
                let mut s = [0u8; SECTOR];
                self.read(d, lba, &mut s)?;
                let Some(i) = (0..ENTRIES).find(|&i| s[i * 32] == 0) else { continue };
                if i + n <= ENTRIES {
                    put(&mut s, i);
                    self.write(d, lba, &s)?;
                    return Ok((lba, (i + n - 1) as u8));
                }
                // too few slots left here: the entry goes first into the next sector, then the slots left here
                // are marked deleted, which makes it visible
                let target = if sec + 1 < self.spc {
                    let mut t = [0u8; SECTOR];
                    self.read(d, lba + 1, &mut t)?;
                    put(&mut t, 0);
                    self.write(d, lba + 1, &t)?;
                    lba + 1
                } else {
                    self.extend_dir(d, cluster, entries)?
                };
                for k in i..ENTRIES {
                    s[k * 32] = 0xe5;
                }
                self.write(d, lba, &s)?;
                return Ok((target, (n - 1) as u8));
            }
            match self.next(d, cluster)? {
                Some(c) => cluster = c,
                // every slot taken to the end of the chain: no end marker to move
                None => return Ok((self.extend_dir(d, cluster, entries)?, (n - 1) as u8)),
            }
        }
        Err(Error::Corrupt)
    }

    /// A new cluster after `last` in a directory's chain, holding `entries` first and zeros: written whole, then
    /// linked. Its first sector.
    fn extend_dir(&mut self, d: &mut impl DiskWrite, last: u32, entries: &[[u8; 32]]) -> Result<u32, Error> {
        let c = self.find_free(d, last + 1)?;
        let mut s = [0u8; SECTOR];
        for (slot, e) in s.chunks_exact_mut(32).zip(entries) {
            slot.copy_from_slice(e);
        }
        self.fill_cluster(d, c, &s)?;
        self.set_entries(d, &[(c, EOC), (last, c)])?;
        self.fsinfo_update(d, 1, c + 1, None)?;
        Ok(self.lba(c))
    }

    /// A new entry `name` in `dir` on the chain from `first` (already allocated), `size` bytes.
    fn add_named(
        &mut self,
        d: &mut impl DiskWrite,
        dir: File,
        name: &str,
        attr: u8,
        (first, size): (u32, u32),
        time: (u16, u16),
    ) -> Result<Handle, Error> {
        self.check_new(d, dir, name)?;
        let short = self.alias(d, dir, name)?;
        let (entries, n) = dir_entries(name, short, attr, first, size, time);
        let (entry_lba, slot) = self.add_entries(d, dir, entries.get(..n).ok_or(Error::Corrupt)?)?;
        let (last, clusters) = self.chain_end(d, first)?;
        Ok(Handle { entry_lba, slot, first, last, clusters, size })
    }

    /// Creates file `name` in `dir`, empty, with one cluster taken (so its entry never changes cluster).
    pub fn create(&mut self, d: &mut impl DiskWrite, dir: File, name: &str, time: (u16, u16)) -> Result<Handle, Error> {
        self.check_new(d, dir, name)?;
        let mut h = Handle::default();
        self.grow(d, &mut h, 1)?;
        self.add_named(d, dir, name, ARCHIVE, (h.first, 0), time)
    }

    /// Creates folder `name` in `dir`: its cluster written (`.`, `..`, zeros), taken, then its entry.
    pub fn mkdir(&mut self, d: &mut impl DiskWrite, dir: File, name: &str, time: (u16, u16)) -> Result<File, Error> {
        self.check_new(d, dir, name)?;
        let hint = self.hint(d)?;
        let c = self.find_free(d, hint)?;
        let parent = if dir.cluster == self.root { 0 } else { dir.cluster };
        let mut s = [0u8; SECTOR];
        let (dot, _) = dir_entries(".", *b".          ", DIR, c, 0, time);
        let (dotdot, _) = dir_entries("..", *b"..         ", DIR, parent, 0, time);
        s[..32].copy_from_slice(&dot[1]); // [0] is the long name piece: not wanted for dot entries
        s[32..64].copy_from_slice(&dotdot[1]);
        self.fill_cluster(d, c, &s)?;
        self.set_entries(d, &[(c, EOC)])?;
        self.fsinfo_update(d, 1, c + 1, None)?;
        self.add_named(d, dir, name, DIR, (c, 0), time)?;
        Ok(File { cluster: c, size: 0, dir: true })
    }

    /// Folder `name` in `dir`, made if missing.
    pub fn ensure_dir(
        &mut self,
        d: &mut impl DiskWrite,
        dir: File,
        name: &str,
        time: (u16, u16),
    ) -> Result<File, Error> {
        match self.find(d, dir, name) {
            Ok(f) if f.dir => Ok(f),
            Ok(_) => Err(Error::Refused), // a file has the name
            Err(Error::NotFound) => self.mkdir(d, dir, name, time),
            Err(e) => Err(e),
        }
    }

    /// A new entry `name` in `dir` for the chain of `h` (the second half of a move: [`Fat::remove`] first).
    pub fn link(
        &mut self,
        d: &mut impl DiskWrite,
        dir: File,
        name: &str,
        h: &Handle,
        time: (u16, u16),
    ) -> Result<Handle, Error> {
        self.add_named(d, dir, name, ARCHIVE, (h.first, h.size), time)
    }

    /// Marks `h`'s entry, and the long name pieces just before it in its sector, deleted (0xe5); nothing else
    /// changes, its chain stays taken (module doc: a move).
    pub fn remove(&mut self, d: &mut impl DiskWrite, h: &Handle) -> Result<(), Error> {
        let mut s = [0u8; SECTOR];
        self.read(d, h.entry_lba, &mut s)?;
        let slot = usize::from(h.slot);
        let short: [u8; 32] = s.get(slot * 32..slot * 32 + 32).and_then(|e| e.try_into().ok()).ok_or(Error::Corrupt)?;
        let sum = short_sum(&short);
        s[slot * 32] = 0xe5;
        for k in (0..slot).rev() {
            if s[k * 32 + 11] != 0x0f || s[k * 32 + 13] != sum || s[k * 32] == 0xe5 {
                break;
            }
            s[k * 32] = 0xe5;
        }
        self.write(d, h.entry_lba, &s)
    }

    /// Whether the FAT sector whose first entry is cluster `first`'s holds only what a FAT holds: free, a cluster
    /// of the volume, bad or end of chain (the reserved entries 0 and 1, and those past the volume, anything). A
    /// sector torn by a cut mid-write (garbage, which a card without reliable write may leave: emmc.rs) almost
    /// never is: one entry in 2^28 values is a cluster of a 470 k cluster volume, and 128 entries a sector.
    fn sane(&self, first: u32, s: &[u8]) -> bool {
        s.chunks_exact(4).enumerate().all(|(i, e)| {
            let (c, v) = (first + i as u32, u32::from_le_bytes([e[0], e[1], e[2], e[3]]) & 0x0fff_ffff);
            !self.valid(c) || v == 0 || self.valid(v) || v >= 0x0fff_fff7
        })
    }

    /// Makes every FAT copy equal to the first (only the sectors that differ are written), and FSInfo's free count
    /// the true one: after a cut (module doc). A sector of the first FAT that is not sane (`Fat::sane`) where the
    /// second's is (the cut tore it) takes the second's instead. Reads the whole FAT: about 15 MB on the unit's
    /// volume, so only when a cut may have happened. The free clusters.
    pub fn sync_fats(&mut self, d: &mut impl DiskWrite) -> Result<u32, Error> {
        const RUN: u32 = 8;
        let mut one = [0u8; RUN as usize * SECTOR];
        let mut other = [0u8; RUN as usize * SECTOR];
        let mut free = 0u32;
        let mut sector = 0;
        while sector < self.fat_size {
            let k = RUN.min(self.fat_size - sector);
            let len = k as usize * SECTOR;
            let (a, b) = (one.get_mut(..len).ok_or(Error::Corrupt)?, other.get_mut(..len).ok_or(Error::Corrupt)?);
            self.read(d, self.fat_start + sector, a)?;
            for copy in 1..self.fats {
                let lba = self.fat_start + copy * self.fat_size + sector;
                self.read(d, lba, b)?;
                for (j, (x, y)) in a.chunks_exact_mut(SECTOR).zip(b.chunks_exact(SECTOR)).enumerate() {
                    if x == y {
                        continue;
                    }
                    let first = (sector + j as u32) * (SECTOR as u32 / 4);
                    if !self.sane(first, x) && self.sane(first, y) {
                        // the first FAT's sector torn by a cut while it was written: the copy is the one before it
                        self.write(d, self.fat_start + sector + j as u32, y)?;
                        x.copy_from_slice(y);
                        self.cached = None;
                    } else {
                        self.write(d, lba + j as u32, x)?;
                    }
                }
            }
            for (i, e) in a.chunks_exact(4).enumerate() {
                let c = sector * (SECTOR as u32 / 4) + i as u32;
                if self.valid(c) && u32::from_le_bytes([e[0], e[1], e[2], e[3]]) & 0x0fff_ffff == 0 {
                    free += 1;
                }
            }
            sector += k;
        }
        let hint = self.hint(d)?;
        self.fsinfo_update(d, 0, hint, Some(free))?;
        Ok(free)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::super::tests::{Ram, image};
    use super::*;
    use std::vec::Vec;

    impl DiskWrite for Ram {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
            let at = lba as usize * SECTOR;
            if self.0.len() < at + buf.len() {
                self.0.resize(at + buf.len(), 0);
            }
            self.0[at..at + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    /// What fsck would say of a volume: Err on a chain into a free cluster or out of the volume, a cross-link, a
    /// size past its chain, FATs that differ; Ok(lost clusters: taken, named by no entry). Walks every directory.
    pub fn fsck(img: &[u8]) -> Result<u32, std::string::String> {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).map_err(|e| std::format!("mount {e:?}"))?;
        let fs = fat.fat_size as usize * SECTOR;
        let start = fat.fat_start as usize * SECTOR;
        if fat.fats == 2 && img[start..start + fs] != img[start + fs..start + 2 * fs] {
            return Err("the FATs differ".into());
        }
        let n = fat.clusters + 2;
        let mut owner = std::vec![0u32; n as usize];
        let mut dirs = std::vec![fat.root()];
        let mut id = 0;
        while let Some(dir) = dirs.pop() {
            let mut chains = std::vec![(dir.cluster, u32::MAX)];
            let mut c = fat.cursor(dir).map_err(|e| std::format!("{e:?}"))?;
            while let Some((e, _)) = fat.next_raw(&mut d, &mut c).map_err(|e| std::format!("dir {e:?}"))? {
                if e[..11] == *b".          " || e[..11] == *b"..         " {
                    continue;
                }
                let f = fat.file_of(&e).map_err(|e| std::format!("entry {e:?}"))?;
                if f.dir {
                    dirs.push(f);
                } else if f.cluster != 0 {
                    chains.push((f.cluster, f.size));
                }
            }
            for (first, size) in chains {
                id += 1;
                let mut k = first;
                let mut len = 0u32;
                loop {
                    if !fat.valid(k) {
                        return Err(std::format!("chain of {first} leaves the volume at {k}"));
                    }
                    if owner[k as usize] == id {
                        return Err(std::format!("loop in {first}"));
                    }
                    if owner[k as usize] != 0 {
                        return Err(std::format!("cross-linked cluster {k} ({first})"));
                    }
                    owner[k as usize] = id;
                    len += 1;
                    match fat.entry(&mut d, k).unwrap() {
                        0 => return Err(std::format!("chain of {first} runs into free cluster {k}")),
                        0x0fff_fff8.. => break,
                        nx => k = nx,
                    }
                }
                if size != u32::MAX && size.div_ceil(fat.spc * SECTOR as u32) > len {
                    return Err(std::format!("file at {first}: size {size} past its chain of {len}"));
                }
            }
        }
        let mut lost = 0;
        for c in 2..n {
            let v = fat.entry(&mut d, c).unwrap();
            if v != 0 && owner[c as usize] == 0 && v != 0x0fff_fff7 {
                lost += 1;
            }
        }
        Ok(lost)
    }

    /// Free clusters as the FAT says, and FSInfo's count.
    pub fn free_counts(img: &[u8]) -> (u32, u32) {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let free = (2..fat.clusters + 2).filter(|&c| fat.entry(&mut d, c).unwrap() == 0).count() as u32;
        (free, fat.fsinfo_read(&mut d).unwrap().map_or(u32::MAX, |f| f.0))
    }

    const T: (u16, u16) = (0x5b58, 0x6000);

    fn read_all(fat: &mut Fat, d: &mut Ram, path: &str) -> Vec<u8> {
        let f = fat.open(d, path).unwrap();
        let mut v = std::vec![0u8; f.size as usize];
        assert_eq!(fat.read_at(d, &f, 0, &mut v).unwrap(), v.len());
        v
    }

    /// Create, write, size, move: the reader sees it, fsck finds nothing, FSInfo stays exact.
    #[test]
    fn create_write_move() {
        let img = image();
        assert_eq!(fsck(&img), Ok(0), "the macOS volume as made");
        let (free0, info0) = free_counts(&img);
        let mut d = Ram(img, 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.ensure_dir(&mut d, fat.root(), "Rustfw", T).unwrap();
        assert_eq!(fat.ensure_dir(&mut d, fat.root(), "rustfw", T), Ok(dir), "found again, case ignored");
        let mut h = fat.create(&mut d, dir, "261024134531.fit", T).unwrap();
        assert_eq!(fat.create(&mut d, dir, "261024134531.FIT", T).err(), Some(Error::Refused), "taken");
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 13 + i / 511) as u8).collect();
        let padded = {
            let mut p = data.clone();
            p.resize(data.len().div_ceil(SECTOR) * SECTOR, 0);
            p
        };
        // in pieces, as the recorder writes: across cluster edges
        for (a, b) in [(0usize, 3usize), (3, 40), (40, padded.len() / SECTOR)] {
            fat.write_sectors(&mut d, &mut h, a as u32, &padded[a * SECTOR..b * SECTOR]).unwrap();
        }
        fat.set_size(&mut d, &mut h, data.len() as u32, T).unwrap();
        assert_eq!(read_all(&mut fat, &mut d, "Rustfw/261024134531.fit"), data);
        assert_eq!(fsck(&d.0), Ok(0));
        let had = fat.open(&mut d, "Activities").is_ok();
        let acts = fat.ensure_dir(&mut d, fat.root(), "Activities", T).unwrap();
        fat.remove(&mut d, &h).unwrap();
        assert_eq!(fat.open(&mut d, "Rustfw/261024134531.fit"), Err(Error::NotFound));
        assert_eq!(fsck(&d.0), Ok(7), "between the two halves of a move: the chain is lost, nothing worse");
        fat.link(&mut d, acts, "261024134531.fit", &h, T).unwrap();
        assert_eq!(read_all(&mut fat, &mut d, "Activities/261024134531.fit"), data);
        assert_eq!(fsck(&d.0), Ok(0));
        let (free, info) = free_counts(&d.0);
        assert_eq!(free0 - free, 7 + 2 - u32::from(had), "seven clusters of data, the folders made");
        assert_eq!(info0 - info, free0 - free, "FSInfo follows");
        // the deleted entry keeps its first cluster (found by hand)
        let mut s = [0u8; SECTOR];
        d.read(h.entry_lba, &mut s).unwrap();
        let e = &s[usize::from(h.slot) * 32..][..32];
        assert_eq!(e[0], 0xe5);
        assert_eq!(u32::from(u16::from_le_bytes([e[26], e[27]])), h.first & 0xffff);
    }

    /// A folder filled past a sector edge and past its cluster: entries pad, then the folder grows, and every
    /// name is found.
    #[test]
    fn a_folder_grows() {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.mkdir(&mut d, fat.root(), "many rides", T).unwrap();
        for i in 0..400 {
            let name = std::format!("{:012}.fit", 260_101_000_000u64 + i);
            fat.create(&mut d, dir, &name, T).unwrap();
        }
        for i in [0, 199, 399] {
            let name = std::format!("{:012}.fit", 260_101_000_000u64 + i);
            assert_eq!(fat.open(&mut d, &std::format!("many rides/{name}")).map(|f| f.size), Ok(0));
        }
        let mut c = fat.cursor(dir).unwrap();
        let mut n = 0;
        while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
            n += usize::from(e.name().ends_with(b".fit"));
        }
        assert_eq!(n, 400);
        assert_eq!(fsck(&d.0), Ok(0));
        assert!(fat.chain_end(&mut d, dir.cluster).unwrap().1 > 1, "past one cluster");
    }

    /// Cut back after a resume: the size first, the extra clusters freed.
    #[test]
    fn shrink_frees_the_rest() {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mut h = fat.create(&mut d, fat.root(), "big.bin", T).unwrap();
        fat.write_sectors(&mut d, &mut h, 0, &[7u8; 100 * SECTOR]).unwrap();
        fat.set_size(&mut d, &mut h, 100 * SECTOR as u32, T).unwrap();
        let (free, _) = free_counts(&d.0);
        fat.set_size(&mut d, &mut h, 1000, T).unwrap();
        assert_eq!(h.clusters, 1);
        assert_eq!(free_counts(&d.0), (free + 3, free + 3));
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// The writer's inputs, pseudo-random (seeded: a failure reproduces): names, offsets, sizes, on a good volume
    /// and on damaged ones. Errors, never a panic, a hang, or a write outside the volume's data and FATs.
    #[test]
    fn fuzz_writer() {
        let img = image();
        let mut seed = 0x1234_5678u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..60 {
            let mut bad = img.clone();
            if round % 2 == 1 {
                for _ in 0..32 {
                    let at = SECTOR + (rnd() as usize) % (bad.len().min(2 << 20) - SECTOR);
                    bad[at] = rnd() as u8;
                }
            }
            let mut d = Ram(bad, 0);
            let Ok(mut fat) = Fat::mount(&mut d) else { continue };
            let boot = d.0[..SECTOR].to_vec();
            for _ in 0..20 {
                let len = 1 + rnd() as usize % 70;
                let name: std::string::String = (0..len).map(|_| (0x20 + rnd() % 0x60) as u8 as char).collect();
                let root = fat.root();
                match rnd() % 4 {
                    0 => {
                        let _ = fat.mkdir(&mut d, root, &name, T);
                    }
                    _ => {
                        if let Ok(mut h) = fat.create(&mut d, root, &name, T) {
                            let at = (rnd() % 200) as u32;
                            let n = 1 + rnd() as usize % 40;
                            if fat.write_sectors(&mut d, &mut h, at, &std::vec![1u8; n * SECTOR]).is_ok() {
                                let size = (rnd() as u32) % ((at + n as u32) * SECTOR as u32 + 1);
                                let _ = fat.set_size(&mut d, &mut h, size, T);
                            }
                            if rnd() % 3 == 0 {
                                let _ = fat.remove(&mut d, &h);
                            }
                        }
                    }
                }
            }
            assert_eq!(d.0[..SECTOR], boot, "the boot sector is never written");
            if round % 2 == 0 {
                assert_eq!(fsck(&d.0).map(|_| ()), Ok(()), "round {round}");
            }
        }
    }
}
