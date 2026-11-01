//! The files of two 512-byte copies in `\Rustfw\` (`pages.bin`, `settings.bin`, `places.bin`, `bikes.bin`): the
//! newest whole copy wins (a good magic and CRC; the higher sequence number), and a write goes over the other one, so
//! a write cut at any point leaves the newest copy before it whole. Each file keeps its own layout, magic and version;
//! this module has what they share: [`newest`], the common header of the last three ([`seal`], [`open`]), and the
//! app's state of a file ([`Copies`]: read or not, a change waiting, the FAT copies known equal). Pure logic.
//!
//! The common header, little endian (settings.bin, places.bin, bikes.bin; pages.bin has its own, ui/page.rs):
//!
//! ```text
//! 0    magic (4 bytes)
//! 4    version (u16)
//! 6    length (u16)    bytes of data after the header (each file says where its data starts)
//! 8    seq (u32)       higher is newer
//! 508  CRC-32 of 0..508
//! ```

/// Where the common header's CRC-32 is (the data ends there).
pub const CRC_AT: usize = 508;

/// The newer of a file's two copies (its 1024 bytes) that `read` takes, by `seq`; the first on a tie; None if neither.
///
/// ```
/// use bryton_rustfw::copies::newest;
/// let mut file = [0u8; 1024];
/// (file[0], file[512]) = (3, 4);
/// let read = |s: &[u8; 512]| (s[0] != 0).then_some([s[0], s[1]]); // [seq, data]
/// let seq = |c: &[u8; 2]| u32::from(c[0]);
/// assert_eq!(newest(&file, read, seq), Some([4, 0]));
/// file[512] = 0; // the newer one damaged: the older
/// assert_eq!(newest(&file, read, seq), Some([3, 0]));
/// (file[512], file[513]) = (3, 1); // a tie: the first
/// assert_eq!(newest(&file, read, seq), Some([3, 0]));
/// ```
pub fn newest<C>(file: &[u8; 1024], read: impl Fn(&[u8; 512]) -> Option<C>, seq: impl Fn(&C) -> u32) -> Option<C> {
    let copy = |k: usize| read(file.get(512 * k..512 * (k + 1))?.try_into().ok()?);
    match (copy(0), copy(1)) {
        (Some(a), Some(b)) => Some(if seq(&b) > seq(&a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Writes the common header into `s` (its data written already), then the CRC-32 of 0..508.
pub fn seal(s: &mut [u8; 512], magic: &[u8; 4], version: u16, len: u16, seq: u32) {
    s[..4].copy_from_slice(magic);
    s[4..6].copy_from_slice(&version.to_le_bytes());
    s[6..8].copy_from_slice(&len.to_le_bytes());
    s[8..12].copy_from_slice(&seq.to_le_bytes());
    let crc = crate::inflate::crc32(s[..CRC_AT].iter().copied());
    s[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
}

/// A copy's header: its length and sequence number when the magic, the version and the CRC-32 are right, else None.
///
/// ```
/// use bryton_rustfw::copies::{open, seal};
/// let mut s = [0u8; 512];
/// seal(&mut s, b"RTST", 2, 40, 7);
/// assert_eq!(open(&s, b"RTST", 2), Some((40, 7)));
/// assert_eq!(open(&s, b"RTST", 1), None, "another version");
/// s[100] ^= 1;
/// assert_eq!(open(&s, b"RTST", 2), None, "damaged");
/// ```
pub fn open(s: &[u8; 512], magic: &[u8; 4], version: u16) -> Option<(usize, u32)> {
    let le16 = |a: usize| crate::storage::bytes::le16(s, a);
    let le32 = |a: usize| crate::storage::bytes::le32(s, a);
    if s[..4] != *magic || le16(4)? != version || le32(CRC_AT)? != crate::inflate::crc32(s[..CRC_AT].iter().copied()) {
        return None;
    }
    Some((usize::from(le16(6)?), le32(8)?))
}

/// The app's state of one such file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Copies {
    /// Read, or known not to be there (no file; for pages.bin, no volume). A failed read leaves it false: tried
    /// again, and the file is not written until then (a write needs the newest copy's number, and "no file" would
    /// write a first copy beside a newer one).
    pub loaded: bool,
    /// Reads that failed.
    pub tries: u8,
    /// A change waits to be written.
    pub dirty: bool,
    /// The file was read whole (1024 bytes) and the FAT copies were not suspect: written over in place, one sector,
    /// no FAT sector touched. Else the next write creates or grows it, and a cut in an earlier one may have left the
    /// FAT copies different in a sector, so they are made equal first (`fat::Fat::sync_fats_from`, from `sync`).
    pub sound: bool,
    /// FAT sectors compared so far.
    pub sync: u32,
    /// The newest copy's sequence number.
    pub seq: u32,
    /// Not on the card whole, and its writer never makes it (bikes.bin, places.bin: a ride's start does, the
    /// recorder's `make_kept`): a change waits until a ride has started.
    pub no_file: bool,
    /// The last failed read was an I/O error (a card not answering): not counted in `tries`, so the passes go on
    /// asking (cheap: the eMMC layer refuses a new init during its backoff), and edits of the file wait for it.
    pub io_failed: bool,
}

impl Copies {
    /// Nothing read.
    pub const fn new() -> Self {
        Copies {
            loaded: false,
            tries: 0,
            dirty: false,
            sound: false,
            sync: 0,
            seq: 0,
            no_file: false,
            io_failed: false,
        }
    }

    /// A read of the file ended as `read` (its bytes, or why not), the FAT copies `suspect` or not: Some(whole) once
    /// read or known not there, None when it failed (counted in `tries` unless it was an I/O error, tried again). Any error but NotFound fails,
    /// or with `only_io` an I/O error only (pages.bin: no volume reads as no file, the registers hold the pages).
    ///
    /// ```
    /// use bryton_rustfw::{copies::Copies, fat::Error};
    /// let mut c = Copies::new();
    /// assert_eq!(c.read(Err(Error::NotFat32), false, false), None);
    /// assert_eq!((c.loaded, c.tries), (false, 1));
    /// assert_eq!(c.read(Err(Error::NotFat32), true, false), Some(false), "pages.bin: no volume is no file");
    /// assert_eq!(c.read(Err(Error::Corrupt), false, false), None);
    /// assert_eq!((c.tries, c.io_failed), (2, false), "a damaged volume counts and is not asked again for ever");
    /// assert_eq!(c.read(Err(Error::Io), true, false), None);
    /// assert_eq!((c.tries, c.io_failed), (2, true), "an I/O error is not counted: asked again at every pass");
    /// assert_eq!(c.read(Err(Error::Io), false, false), None);
    /// assert_eq!(c.tries, 2);
    /// assert_eq!(c.read(Ok(1024), false, false), Some(true));
    /// assert!(c.loaded && c.sound && !c.io_failed, "read: no longer waiting");
    /// ```
    pub fn read(&mut self, read: Result<usize, crate::fat::Error>, only_io: bool, suspect: bool) -> Option<bool> {
        use crate::fat::Error;
        if matches!(read, Err(e) if e != Error::NotFound && (!only_io || e == Error::Io)) {
            self.io_failed = read == Err(Error::Io);
            if !self.io_failed {
                self.tries = self.tries.saturating_add(1);
            }
            return None;
        }
        (self.loaded, self.io_failed, self.sound) = (true, false, read == Ok(1024) && !suspect);
        Some(read == Ok(1024))
    }
}
