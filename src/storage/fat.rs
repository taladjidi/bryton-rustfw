//! A minimal FAT32 reader over a "read sectors" function ([`Disk`]): the boot sector, cluster chains, directories
//! with 8.3 and long names (matched without regard to ASCII case), files read at an offset. The write path (the ride
//! recorder's: create, append, size, move, sync the FATs) is in `write`, over [`DiskWrite`]; a [`Disk`] that is
//! only that cannot be written.
//!
//! The volume is the eMMC's from sector 0 (BOARD.md 15: no MBR, the FAT32 boot sector is sector 0). Every loop is
//! bounded (a chain by the volume's cluster count, a directory by FAT's 65536 entries), and a damaged volume gives
//! [`Error::Corrupt`], never a panic or a hang.
//!
//! Format: en.wikipedia.org/wiki/Design_of_the_FAT_file_system (directory entry offsets 0x14 / 0x1a / 0x1c, the
//! first byte 0x00 / 0xe5 / 0x05, the long name entry layout and its checksum, FAT32 entries of 28 bits with
//! 0x?ffffff8.. as the end of a chain). Checked against a volume macOS made (the tests).
//!
//! Why not a crate (checked 2026-10-23):
//! - `embedded-sdmmc` 0.9.0 (no_std, no heap): `VolumeManager::open_volume` reads an MBR at block 0 and fails
//!   with "Invalid MBR signature" without one, and the unit has none (BOARD.md 15); files are found by
//!   `ToShortFileName` (8.3) only, and stock's tiles have long names (`8192_8192_0.bin`) whose 8.3 aliases the
//!   writer chose. Both are work-arounds (a fake block 0, our own long name scan over `iterate_dir_lfn`) on top of
//!   4 more crates (byteorder, embedded-hal, embedded-io, heapless) and its write path, which we must never use.
//! - `fatfs` (rafalh): the last release (0.3.6, January 2023) needs `std`, or the nightly-only `core_io`, and 0.4
//!   is unreleased (git only).
//! - This reader is about 300 lines with its tests, read only by construction, no `unsafe`.

/// Bytes per sector: the only size this reader takes (the unit's, BOARD.md 15).
pub const SECTOR: usize = 512;
/// Directory entries a FAT directory may hold at most.
const DIR_ENTRIES: u32 = 65_536;
/// Characters of a long name (20 entries of 13).
const LFN: usize = 260;

pub mod write;

/// Sectors of a volume, read whole.
pub trait Disk {
    /// Reads `buf.len() / 512` sectors from `lba` on.
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error>;
}

/// Sectors of a volume that can also be written (`write`).
pub trait DiskWrite: Disk {
    /// Writes `buf.len() / 512` sectors from `lba` on; Ok once they are on the medium.
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error>;
}

/// Why a volume, a path or a file could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The disk failed to read or write.
    Io,
    /// Sector 0 is not a FAT32 boot sector this reader takes.
    NotFat32,
    /// No such name in the directory.
    NotFound,
    /// A cluster chain or a directory entry that cannot be right (a loop, a cluster out of range, a file longer
    /// than its chain).
    Corrupt,
    /// A write that cannot be done: a name already there or not allowed, no free cluster.
    Refused,
}

/// A file or directory: its first cluster (0: empty file) and size (directories: 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    /// First cluster; 0 for an empty file.
    pub cluster: u32,
    /// Size in bytes.
    pub size: u32,
    /// A directory.
    pub dir: bool,
}

/// Bytes of a name [`Fat::list`] gives (longer long names are cut).
pub const NAME: usize = 64;

/// Where a read of a file ended ([`Fat::read_from`]): the cluster's place in the chain, and the cluster (0: none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seek {
    index: u32,
    cluster: u32,
}

impl Seek {
    /// No read yet: from the first cluster.
    pub const fn new() -> Seek {
        Seek { index: 0, cluster: 0 }
    }
}

/// Where a directory listing is: the sector read last, and the long name pieces seen since the last entry.
pub struct Cursor {
    cluster: u32,
    /// Sector within the cluster.
    sector: u32,
    /// Entry within `buf`.
    entry: usize,
    /// Entries read so far (bounds a loop in the chain).
    seen: u32,
    done: bool,
    buf: [u8; SECTOR],
    lfn: [u16; LFN],
    lfn_sum: Option<u8>,
}

/// A directory entry as [`Fat::list`] gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file or directory.
    pub file: File,
    name: [u8; NAME],
    len: u8,
}

impl Entry {
    /// Its name, ASCII (other characters as '?').
    pub fn name(&self) -> &[u8] {
        self.name.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// A mounted FAT32 volume: its geometry and the FAT sector read last.
pub struct Fat {
    fat_start: u32,
    /// Sectors per FAT, and how many FATs (1 or 2): the writer keeps every copy the same.
    fat_size: u32,
    fats: u32,
    /// The FSInfo sector (0: none).
    fsinfo: u32,
    data_start: u32,
    spc: u32,
    clusters: u32,
    root: u32,
    total: u32,
    cached: Option<u32>,
    sector: [u8; SECTOR],
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b.get(i).copied().unwrap_or(0), b.get(i + 1).copied().unwrap_or(0)])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from(u16_at(b, i)) | u32::from(u16_at(b, i + 2)) << 16
}

impl Fat {
    /// Reads the boot sector (sector 0) and checks it is FAT32 with 512-byte sectors.
    pub fn mount(d: &mut impl Disk) -> Result<Fat, Error> {
        let mut s = [0u8; SECTOR];
        d.read(0, &mut s)?;
        let spc = u32::from(s[13]);
        let (reserved, fats) = (u32::from(u16_at(&s, 14)), u32::from(s[16]));
        let (total, fat_size, root) = (u32_at(&s, 32), u32_at(&s, 36), u32_at(&s, 44));
        let fat32 = u16_at(&s, 510) == 0xaa55
            && u16_at(&s, 11) == SECTOR as u16
            && spc.is_power_of_two()
            && reserved > 0
            && (1..=2).contains(&fats)
            && u16_at(&s, 17) == 0 // no FAT12/16 root directory
            && u16_at(&s, 22) == 0 // FAT12/16 FAT size
            && fat_size > 0;
        let data_start = fats.checked_mul(fat_size).and_then(|f| f.checked_add(reserved)).ok_or(Error::NotFat32)?;
        if !fat32 || total <= data_start {
            return Err(Error::NotFat32);
        }
        // the FAT must hold an entry for every cluster
        let clusters = ((total - data_start) / spc).min(fat_size.saturating_mul(SECTOR as u32 / 4).saturating_sub(2));
        let fsinfo = u32::from(u16_at(&s, 48)).min(reserved.saturating_sub(1));
        let fat = Fat {
            fat_start: reserved,
            fat_size,
            fats,
            fsinfo,
            data_start,
            spc,
            clusters,
            root,
            total,
            cached: None,
            sector: [0; SECTOR],
        };
        if !fat.valid(root) {
            return Err(Error::NotFat32);
        }
        Ok(fat)
    }

    /// The root directory.
    pub fn root(&self) -> File {
        File { cluster: self.root, size: 0, dir: true }
    }

    fn valid(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster - 2 < self.clusters
    }

    fn read(&mut self, d: &mut impl Disk, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        if lba.checked_add(n).is_none_or(|end| end > self.total) {
            return Err(Error::Corrupt);
        }
        d.read(lba, buf)
    }

    /// Drops the FAT sector kept: the next look-up reads it again (the volume was written by someone else).
    pub fn forget(&mut self) {
        self.cached = None;
    }

    /// The cluster after `cluster` in its chain; None at the end of the chain.
    fn next(&mut self, d: &mut impl Disk, cluster: u32) -> Result<Option<u32>, Error> {
        match self.entry(d, cluster)? {
            0x0fff_fff8.. => Ok(None),
            n if self.valid(n) => Ok(Some(n)),
            _ => Err(Error::Corrupt), // free, bad or out of range inside a chain
        }
    }

    /// The FAT entry of `cluster` (28 bits), from the first FAT, through the one sector kept.
    fn entry(&mut self, d: &mut impl Disk, cluster: u32) -> Result<u32, Error> {
        let lba = self.fat_start + cluster / (SECTOR as u32 / 4);
        if self.cached != Some(lba) {
            self.cached = None;
            let mut s = [0u8; SECTOR];
            self.read(d, lba, &mut s)?;
            (self.sector, self.cached) = (s, Some(lba));
        }
        Ok(u32_at(&self.sector, (cluster as usize % (SECTOR / 4)) * 4) & 0x0fff_ffff)
    }

    /// The `index`th cluster of the chain from `first`.
    fn nth(&mut self, d: &mut impl Disk, first: u32, index: u32) -> Result<u32, Error> {
        if index >= self.clusters || !self.valid(first) {
            return Err(Error::Corrupt);
        }
        let mut c = first;
        for _ in 0..index {
            c = self.next(d, c)?.ok_or(Error::Corrupt)?;
        }
        Ok(c)
    }

    pub(crate) fn lba(&self, cluster: u32) -> u32 {
        self.data_start + (cluster - 2) * self.spc
    }

    /// A listing of directory `dir` from its first entry ([`Fat::list`]).
    pub fn cursor(&self, dir: File) -> Result<Cursor, Error> {
        if !dir.dir {
            return Err(Error::NotFound);
        }
        if !self.valid(dir.cluster) {
            return Err(Error::Corrupt);
        }
        Ok(Cursor {
            cluster: dir.cluster,
            sector: 0,
            entry: SECTOR / 32, // nothing read yet
            seen: 0,
            done: false,
            buf: [0; SECTOR],
            lfn: [0; LFN],
            lfn_sum: None,
        })
    }

    /// The next file or directory entry at `c` (the raw 32 bytes) and whether a whole long name ([`Cursor::lfn`])
    /// goes with it; None at the end of the directory. Deleted entries, long name pieces and the volume label are
    /// skipped.
    fn next_raw(&mut self, d: &mut impl Disk, c: &mut Cursor) -> Result<Option<([u8; 32], bool)>, Error> {
        loop {
            if c.done {
                return Ok(None);
            }
            if c.seen >= DIR_ENTRIES {
                return Err(Error::Corrupt); // longer than any directory can be: a loop
            }
            if c.entry >= SECTOR / 32 {
                if c.seen > 0 {
                    c.sector += 1;
                }
                if c.sector >= self.spc {
                    match self.next(d, c.cluster)? {
                        Some(n) => (c.cluster, c.sector) = (n, 0),
                        None => {
                            c.done = true;
                            return Ok(None);
                        }
                    }
                }
                let mut s = [0u8; SECTOR];
                self.read(d, self.lba(c.cluster) + c.sector, &mut s)?;
                (c.buf, c.entry) = (s, 0);
            }
            let mut e = [0u8; 32];
            e.copy_from_slice(c.buf.get(c.entry * 32..c.entry * 32 + 32).ok_or(Error::Corrupt)?);
            c.entry += 1;
            c.seen += 1;
            match (e[0], e[11]) {
                (0, _) => c.done = true,       // the end of the directory
                (0xe5, _) => c.lfn_sum = None, // deleted
                (ord, 0x0f) => c.lfn_sum = lfn_piece(&mut c.lfn, &e, ord, c.lfn_sum),
                (_, attr) if attr & 0x08 != 0 => c.lfn_sum = None, // the volume label
                _ => {
                    let long = c.lfn_sum == Some(short_sum(&e));
                    c.lfn_sum = None;
                    return Ok(Some((e, long)));
                }
            }
        }
    }

    /// The file or directory a raw directory entry names.
    fn file_of(&self, e: &[u8; 32]) -> Result<File, Error> {
        let cluster = u32::from(u16_at(e, 20)) << 16 | u32::from(u16_at(e, 26));
        let dir = e[11] & 0x10 != 0;
        match cluster {
            0 if dir => Ok(self.root()), // ".." in a folder of the root
            c if c != 0 && !self.valid(c) => Err(Error::Corrupt),
            _ => Ok(File { cluster, size: if dir { 0 } else { u32_at(e, 28) }, dir }),
        }
    }

    /// The next entry of a listing: its name (the long one if it has one, else the 8.3 one) and its file. Entries
    /// that point outside the volume are skipped. None at the end; one entry per call, so a caller can spread a
    /// long directory over several passes.
    pub fn list(&mut self, d: &mut impl Disk, c: &mut Cursor) -> Result<Option<Entry>, Error> {
        while let Some((e, long)) = self.next_raw(d, c)? {
            let Ok(file) = self.file_of(&e) else { continue };
            let mut out = Entry { file, name: [0; NAME], len: 0 };
            let mut put = |b: u8| {
                if let Some(slot) = out.name.get_mut(usize::from(out.len)) {
                    *slot = b;
                    out.len += 1;
                }
            };
            if long {
                for &ch in c.lfn.iter().take_while(|&&ch| ch != 0 && ch != 0xffff) {
                    put(if ch < 0x80 { ch as u8 } else { b'?' });
                }
            } else {
                let trim = |b: &[u8]| b.iter().rposition(|&ch| ch != b' ').map_or(0, |i| i + 1);
                // byte 12: Windows NT's lower case flags (0x08 the name, 0x10 the extension), which macOS sets too
                let lower = |b: u8, bit: u8| if e[12] & bit != 0 { b.to_ascii_lowercase() } else { b };
                let (base, ext) = (&e[..8], &e[8..11]);
                base[..trim(base)].iter().for_each(|&b| put(lower(b, 0x08)));
                if trim(ext) > 0 {
                    put(b'.');
                    ext[..trim(ext)].iter().for_each(|&b| put(lower(b, 0x10)));
                }
            }
            return Ok(Some(out));
        }
        Ok(None)
    }

    /// The entry called `name` in directory `dir`: its long name or its 8.3 name, ASCII case ignored.
    pub fn find(&mut self, d: &mut impl Disk, dir: File, name: &str) -> Result<File, Error> {
        let mut c = self.cursor(dir)?;
        while let Some((e, long)) = self.next_raw(d, &mut c)? {
            if (long && lfn_eq(&c.lfn, name)) || short_eq(&e, name) {
                return self.file_of(&e);
            }
        }
        Err(Error::NotFound)
    }

    /// Follows `path` ('/' or '\\' between names) from the root.
    pub fn open(&mut self, d: &mut impl Disk, path: &str) -> Result<File, Error> {
        self.open_in(d, self.root(), path)
    }

    /// Follows `path` from directory `dir`.
    pub fn open_in(&mut self, d: &mut impl Disk, dir: File, path: &str) -> Result<File, Error> {
        let mut f = dir;
        for name in path.split(['/', '\\']).filter(|n| !n.is_empty()) {
            f = self.find(d, f, name)?;
        }
        Ok(f)
    }

    /// Reads file `f` from byte `offset` into `buf`: the number of bytes read, less than `buf.len()` only at the end
    /// of the file. Whole sectors go straight into `buf`, as many as one cluster holds in one disk read.
    pub fn read_at(&mut self, d: &mut impl Disk, f: &File, offset: u32, buf: &mut [u8]) -> Result<usize, Error> {
        self.read_from(d, f, offset, buf, &mut Seek::default())
    }

    /// [`Fat::read_at`] that starts walking the chain from where the last read of `f` through `seek` ended (when
    /// that is not past `offset`), and leaves `seek` where this one ends: reading on through a large file then
    /// costs no FAT reads from its first cluster again. `seek` belongs to `f` alone.
    pub fn read_from(
        &mut self,
        d: &mut impl Disk,
        f: &File,
        offset: u32,
        buf: &mut [u8],
        seek: &mut Seek,
    ) -> Result<usize, Error> {
        let n = (f.size.saturating_sub(offset) as usize).min(buf.len());
        let cluster_bytes = self.spc * SECTOR as u32;
        let mut done = 0;
        let mut index = offset / cluster_bytes;
        let mut cluster = match (n, *seek) {
            (0, _) => 0,
            (_, Seek { index: at, cluster }) if cluster != 0 && at <= index => self.nth(d, cluster, index - at)?,
            _ => self.nth(d, f.cluster, index)?,
        };
        while done < n {
            let pos = offset + done as u32;
            if done > 0 && pos.is_multiple_of(cluster_bytes) {
                cluster = self.next(d, cluster)?.ok_or(Error::Corrupt)?;
                index += 1;
            }
            *seek = Seek { index, cluster };
            let in_cluster = pos % cluster_bytes;
            let lba = self.lba(cluster) + in_cluster / SECTOR as u32;
            let left = n - done;
            let rest = buf.get_mut(done..n).ok_or(Error::Corrupt)?;
            if in_cluster.is_multiple_of(SECTOR as u32) && left >= SECTOR {
                let sectors = ((left / SECTOR) as u32).min(self.spc - in_cluster / SECTOR as u32);
                let len = sectors as usize * SECTOR;
                self.read(d, lba, rest.get_mut(..len).ok_or(Error::Corrupt)?)?;
                done += len;
            } else {
                let mut s = [0u8; SECTOR];
                self.read(d, lba, &mut s)?;
                let at = (in_cluster % SECTOR as u32) as usize;
                let len = (SECTOR - at).min(left);
                rest.get_mut(..len).ok_or(Error::Corrupt)?.copy_from_slice(s.get(at..at + len).ok_or(Error::Corrupt)?);
                done += len;
            }
        }
        Ok(n)
    }
}

/// Puts long name entry `e` (sequence `ord`) into `lfn`; the checksum while the pieces so far are whole.
fn lfn_piece(lfn: &mut [u16; LFN], e: &[u8], ord: u8, sum: Option<u8>) -> Option<u8> {
    let n = usize::from(ord & 0x1f);
    if n == 0 || n * 13 > LFN {
        return None;
    }
    if ord & 0x40 != 0 {
        lfn.fill(0); // the last piece comes first: a new name
    } else if sum != Some(e[13]) {
        return None;
    }
    const AT: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (i, &a) in AT.iter().enumerate() {
        lfn[(n - 1) * 13 + i] = u16_at(e, a);
    }
    Some(e[13])
}

/// The checksum of a short name that its long name entries carry.
fn short_sum(e: &[u8]) -> u8 {
    e.iter().take(11).fold(0u8, |s, &c| s.rotate_right(1).wrapping_add(c))
}

/// The long name (up to its NUL) equals `name`, ASCII case ignored.
fn lfn_eq(lfn: &[u16; LFN], name: &str) -> bool {
    let len = lfn.iter().position(|&c| c == 0 || c == 0xffff).unwrap_or(LFN);
    let mut it = name.encode_utf16();
    lfn.iter().take(len).all(|&c| it.next().is_some_and(|n| fold(c) == fold(n))) && it.next().is_none()
}

fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// The 8.3 name of entry `e` ("NAME.EXT", padding dropped) equals `name`, ASCII case ignored.
fn short_eq(e: &[u8], name: &str) -> bool {
    let trim = |b: &[u8]| b.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1);
    let (base, ext) = (&e[..8], &e[8..11]);
    let (base, ext) = (&base[..trim(base)], &ext[..trim(ext)]);
    let mut first = base.first().copied();
    if first == Some(0x05) {
        first = Some(0xe5); // a name starting with 0xe5 is stored as 0x05
    }
    let short = first.into_iter().chain(base.iter().skip(1).copied());
    let dotted = short.chain((!ext.is_empty()).then_some(b'.')).chain(ext.iter().copied());
    let mut want = name.bytes();
    dotted.map(u16::from).all(|c| want.next().is_some_and(|n| fold(c) == fold(u16::from(n)))) && want.next().is_none()
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The test volume made by macOS (tools/fat_image.py), expanded from test-data/fat32.sparse.
    pub fn image() -> Vec<u8> {
        let sparse = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/test-data/fat32.sparse")).unwrap();
        let mut img = Vec::new();
        let mut i = 0;
        while i < sparse.len() {
            let w = |k: usize| u32::from_le_bytes(sparse[k..k + 4].try_into().unwrap()) as usize;
            let (lba, n) = (w(i), w(i + 4));
            i += 8;
            img.resize(img.len().max((lba + n) * SECTOR), 0);
            img[lba * SECTOR..(lba + n) * SECTOR].copy_from_slice(&sparse[i..i + n * SECTOR]);
            i += n * SECTOR;
        }
        img
    }

    /// A disk in memory, zeros past its end, with a count of the reads.
    pub struct Ram(pub Vec<u8>, pub u32);

    impl Disk for Ram {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
            self.1 += 1;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.0.get(lba as usize * SECTOR + i).copied().unwrap_or(0);
            }
            Ok(())
        }
    }

    pub fn read_all(fat: &mut Fat, d: &mut Ram, path: &str) -> Result<Vec<u8>, Error> {
        let f = fat.open(d, path)?;
        let mut v = std::vec![0u8; f.size as usize];
        assert_eq!(fat.read_at(d, &f, 0, &mut v)?, v.len());
        Ok(v)
    }

    #[test]
    fn mounts_and_reads_the_macos_volume() {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!((fat.spc, fat.fat_start), (32, 32), "as on the unit");
        assert_eq!(read_all(&mut fat, &mut d, "SHORT.TXT").unwrap(), b"hello");
        assert_eq!(read_all(&mut fat, &mut d, "short.txt").unwrap(), b"hello", "case ignored");
        let json = read_all(&mut fat, &mut d, "progress planner/A LONG FILE NAME.JSON").unwrap();
        assert_eq!(json.len(), 40_000);
        assert!(json.iter().enumerate().all(|(i, &b)| b == (i * 7 + 3) as u8));
        let v = read_all(&mut fat, &mut d, "Map/Data/Temp/version.ini").unwrap();
        assert_eq!(v, b"1.2.1\0");
        // a directory over several clusters, with deleted entries (macOS's ._ files)
        let last = read_all(&mut fat, &mut d, "many/last.bin").unwrap();
        assert!(last.iter().enumerate().all(|(i, &b)| b == i as u8));
        assert_eq!(fat.open(&mut d, "many/file number 599.txt").map(|f| f.size), Ok(0));
        assert_eq!(fat.open(&mut d, "many/file number 600.txt"), Err(Error::NotFound));
        assert_eq!(fat.open(&mut d, "Map/Data/Temp/Zoom14/H/8190_8191_0.bin").map(|f| f.size), Ok(5));
        // a file in pieces (it filled the holes the ._ files left)
        let frag = fat.open(&mut d, "frag.bin").unwrap();
        let mut c = frag.cluster;
        let mut pieces = 1;
        while let Some(n) = fat.next(&mut d, c).unwrap() {
            pieces += u32::from(n != c + 1);
            c = n;
        }
        assert!(pieces > 1, "frag.bin is fragmented");
        let mut buf = [0u8; 4];
        for i in [0u32, 4095, 4096, 4097, 20_000, 49_999] {
            assert_eq!(fat.read_at(&mut d, &frag, i * 4, &mut buf), Ok(4));
            assert_eq!(u32::from_le_bytes(buf), i);
        }
        // unaligned reads across sector and cluster edges equal the whole file's bytes
        let all = read_all(&mut fat, &mut d, "frag.bin").unwrap();
        for (off, len) in [(1, 600), (16_383, 2), (16_000, 40_000), (199_990, 100)] {
            let mut b = std::vec![0u8; len];
            let n = fat.read_at(&mut d, &frag, off, &mut b).unwrap();
            assert_eq!(&b[..n], &all[off as usize..(off as usize + len).min(all.len())]);
        }
        let h = read_all(&mut fat, &mut d, "MAP/DATA/TEMP/ZOOM14/H/8192_8192_0.BIN").unwrap();
        assert_eq!(u32::from_le_bytes(h[..4].try_into().unwrap()) as usize * 10 + 4, h.len(), "a tile's H file");
    }

    /// A listing gives every live entry once, by its long name when it has one, across clusters.
    #[test]
    fn lists_directories() {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let names = |fat: &mut Fat, d: &mut Ram, path: &str| {
            let dir = fat.open(d, path).unwrap();
            let mut c = fat.cursor(dir).unwrap();
            let mut v = Vec::new();
            while let Some(e) = fat.list(d, &mut c).unwrap() {
                v.push((std::string::String::from_utf8(e.name().to_vec()).unwrap(), e.file));
            }
            v
        };
        let many = names(&mut fat, &mut d, "many");
        let files: Vec<&str> = many.iter().map(|e| e.0.as_str()).filter(|n| n.starts_with("file number")).collect();
        assert_eq!(files.len(), 600);
        assert_eq!(files[599], "file number 599.txt");
        let last = many.iter().find(|e| e.0 == "last.bin").unwrap();
        assert_eq!(Ok(last.1), fat.open(&mut d, "many/last.bin"));
        let root = names(&mut fat, &mut d, "/");
        assert!(root.iter().any(|e| e.0 == "SHORT.TXT" && !e.1.dir));
        assert!(root.iter().any(|e| e.0 == "Progress Planner" && e.1.dir));
        let f = fat.open(&mut d, "SHORT.TXT").unwrap();
        assert_eq!(fat.cursor(f).err(), Some(Error::NotFound), "a file");
    }

    #[test]
    fn whole_sectors_in_one_read() {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, "frag.bin").unwrap();
        let mut b = std::vec![0u8; 16_384];
        d.1 = 0;
        fat.read_at(&mut d, &f, 0, &mut b).unwrap();
        assert_eq!(d.1, 1, "one cluster, one read");
    }

    /// Damaged volumes: errors, never a panic or a hang.
    #[test]
    fn corruptions_are_errors() {
        let img = image();
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let frag = fat.open(&mut d, "frag.bin").unwrap();
        let fat_at = |c: u32| 32 * SECTOR + c as usize * 4;
        // a loop in a file: its second cluster points back to its first
        let second = fat.next(&mut d, frag.cluster).unwrap().unwrap();
        let mut bad = img.clone();
        bad[fat_at(second)..fat_at(second) + 4].copy_from_slice(&frag.cluster.to_le_bytes());
        let mut d = Ram(bad, 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mut b = std::vec![0u8; 200_000];
        // the size bounds the read (the loop's bytes repeat: a loop is not worth a visited set here)
        assert_eq!(fat.read_at(&mut d, &frag, 0, &mut b), Ok(200_000));
        // a loop in a directory chain: bounded, NotFound or Corrupt
        let many = fat.open(&mut d, "many").unwrap();
        let mut bad = img.clone();
        bad[fat_at(many.cluster)..fat_at(many.cluster) + 4].copy_from_slice(&many.cluster.to_le_bytes());
        let mut d = Ram(bad, 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(fat.open(&mut d, "many/last.bin"), Err(Error::Corrupt));
        // a chain into a free cluster, or out of the volume
        for next in [0u32, 0x0fff_fff0, 0x0fff_fff7] {
            let mut bad = img.clone();
            bad[fat_at(frag.cluster)..fat_at(frag.cluster) + 4].copy_from_slice(&next.to_le_bytes());
            let mut d = Ram(bad, 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            assert_eq!(fat.read_at(&mut d, &frag, 0, &mut b), Err(Error::Corrupt), "next {next:#x}");
        }
        // a size longer than the chain
        let big = File { size: u32::MAX, ..frag };
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(fat.read_at(&mut d, &big, 300_000, &mut b), Err(Error::Corrupt));
        assert_eq!(fat.read_at(&mut d, &big, u32::MAX - 10, &mut b), Err(Error::Corrupt));
        // boot sectors that are not FAT32 with 512-byte sectors
        for (at, v) in [(12, 0x10u8), (13, 3), (16, 0), (17, 1), (22, 1), (510, 0), (44, 0)] {
            let mut bad = img.clone();
            bad[at] = v;
            assert_eq!(Fat::mount(&mut Ram(bad, 0)).err(), Some(Error::NotFat32), "byte {at} = {v}");
        }
        assert_eq!(Fat::mount(&mut Ram(Vec::new(), 0)).err(), Some(Error::NotFat32), "an empty disk");
        // garbage everywhere: no panic
        let mut seed = 1u32;
        for _ in 0..200 {
            let mut bad = img.clone();
            for _ in 0..64 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let at = (seed >> 8) as usize % (2 * 1024 * 1024).min(bad.len());
                if at >= SECTOR {
                    bad[at] = (seed >> 3) as u8;
                }
            }
            let mut d = Ram(bad, 0);
            if let Ok(mut fat) = Fat::mount(&mut d) {
                for path in ["Map/Data/Temp/Zoom14/P/8192_8192_0.bin", "many/last.bin", "frag.bin"] {
                    if let Ok(f) = fat.open(&mut d, path) {
                        let _ = fat.read_at(&mut d, &f, 0, &mut b);
                    }
                }
            }
        }
    }
}
