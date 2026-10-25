//! A ride deleted from HISTORY, kept: moved out of sight into `\Rustfw\Deleted`, never erased (the owner's rule: a
//! discarded ride must stay recoverable). A computer still sees it there, and moving it back (by hand, on the
//! computer) brings it back to both firmwares' histories.
//!
//! **What moves:** the ride's file `\Activities\YYMMDDhhmmss.fit` (stock's ride or this firmware's clean copy) to
//! `\Rustfw\Deleted\`, and this firmware's full copy of the same ride, `\Rustfw\Activities\<same name>`, if there is
//! one, to `\Rustfw\Deleted\Full\`, each under its name. A name already taken there (the same ride deleted, moved
//! back and deleted again, or another unit's) gets a suffix, `-1` to `-99`, the same in both folders.
//!
//! **Stock's summaries** (`\System\History\Summary\<name>.sum` / `.csum`) are left where they are: stock's own
//! files in stock's folder (AGENTS.md, "Never break it for stock"), whose layout is not established, and which no
//! reader of this firmware uses. Stock's history is rebuilt at its start from the rides it finds
//! (ghidra/recovered_src/ride/upload.c: `History_ListActivityFiles` into `\Gal2.lis`) and each entry's FIT file is
//! checked (`History_ValidateActivityFile`): a summary without its FIT is not listed, inferred, not seen on the
//! unit (docs/status.md). Moving the FIT file back brings the ride back whole, its summaries still there.
//!
//! **Never a ride lost, never in two places.** A move is the FAT writer's (fat/write.rs): the old entry marked
//! deleted (0xe5 only, never zeroed), then the new entry on the same chain. Between the two, the chain is named by
//! no entry, so first a record, `\Rustfw\delete.rec` (one sector: a tag, "pending", the name, the suffix, each
//! file's first cluster, size and first sector's CRC, a CRC), says what is being moved; after the last move it says
//! "done". A cut
//! anywhere leaves either the record not written (nothing moved yet: a torn record does not check out and is
//! ignored), or a pending record, which the next start finishes ([`finish`]): the FATs made equal first
//! ([`Fat::sync_fats`]), then each file is found in its new place (done), or removed from its old place if still
//! there, and linked into the new one if its chain is still whole and still the ride's (its first sector's CRC: a
//! computer's fsck may have freed it and something taken it). A suffix taken then is recorded before any entry
//! changes, and a folder removed meanwhile is made again. The app finishes a pending move at the scan's start and
//! before lending the card to a computer (app/drive.rs), which would see the ride in neither folder. Tested by
//! cutting the power after every sector written (below).
//!
//! **The FAT sync** (`Fat::sync_fats`, both FATs read whole: 3 to 4 s on the owner's 15 MB, estimated) runs in one
//! call, as the recorder's (recorder.rs, "After a cut"): only at the first delete on a card (the folders made) and
//! when a cut left a move pending, before any chain is read; spreading it would leave a move half known across
//! passes.

use crate::fat::write::Handle;
use crate::fat::{DiskWrite, Error, Fat, File, SECTOR};
use crate::recorder::{ACTIVITIES, DIR, FULL_PATH};

/// `\Rustfw\Deleted`: the rides deleted from the history.
pub const DELETED: &str = "Deleted";
/// The record of the move under way, in `\Rustfw`.
pub const REC: &str = "delete.rec";
/// The record's tag and layout version.
const TAG: [u8; 4] = *b"RFD1";
/// Suffixes tried after the plain name.
const SUFFIXES: u8 = 99;
/// Where each file comes from and goes: the clean copy (or stock's ride), then the full copy.
const FROM: [&str; 2] = [ACTIVITIES, FULL_PATH];
const TO: [&str; 2] = ["Rustfw/Deleted", "Rustfw/Deleted/Full"];

/// One file of a move: its first cluster, its size, and the CRC of its first sector (None: an empty file, or a
/// record of the first builds, flag 1, which had none).
type Moved = (u32, u32, Option<u16>);

/// The move under way: the ride's name (`YYMMDDhhmmss`), the suffix of the new name, and per file of [`FROM`]
/// what moves (None: no such file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rec {
    pending: bool,
    name: [u8; 12],
    suffix: u8,
    files: [Option<Moved>; 2],
}

impl Rec {
    fn to_sector(self) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        s[..4].copy_from_slice(&TAG);
        s[4] = u8::from(self.pending);
        s[5..17].copy_from_slice(&self.name);
        s[17] = self.suffix;
        for (k, f) in self.files.iter().enumerate() {
            if let Some((first, size, crc)) = f {
                let at = 32 + 16 * k;
                s[at] = 2;
                s[at + 4..at + 8].copy_from_slice(&first.to_le_bytes());
                s[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
                s[at + 12] = u8::from(crc.is_some());
                s[at + 14..at + 16].copy_from_slice(&crc.unwrap_or(0).to_le_bytes());
            }
        }
        let crc = crate::fit::crc(0, &s[..SECTOR - 2]);
        s[SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
        s
    }

    fn from_sector(s: &[u8; SECTOR]) -> Option<Rec> {
        if s[..4] != TAG || crate::fit::crc(0, &s[..SECTOR - 2]).to_le_bytes() != s[SECTOR - 2..] {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]);
        let file = |k: usize| {
            let (at, flag) = (32 + 16 * k, s[32 + 16 * k]);
            let crc = (flag == 2 && s[at + 12] == 1).then(|| u16::from_le_bytes([s[at + 14], s[at + 15]]));
            matches!(flag, 1 | 2).then(|| (word(at + 4), word(at + 8), crc))
        };
        let mut name = [0u8; 12];
        name.copy_from_slice(&s[5..17]);
        Some(Rec { pending: s[4] == 1, name, suffix: s[17], files: [file(0), file(1)] })
    }
}

/// `<name>.fit`, or `<name>-<suffix>.fit`.
fn file_name<'a>(name: &[u8; 12], suffix: u8, out: &'a mut [u8; 20]) -> Result<&'a str, Error> {
    out[..12].copy_from_slice(name);
    let mut n = 12;
    if suffix > 0 {
        out[n] = b'-';
        n += 1;
        if suffix >= 10 {
            out[n] = b'0' + suffix / 10 % 10;
            n += 1;
        }
        out[n] = b'0' + suffix % 10;
        n += 1;
    }
    out[n..n + 4].copy_from_slice(b".fit");
    core::str::from_utf8(out.get(..n + 4).ok_or(Error::Corrupt)?).map_err(|_| Error::Corrupt)
}

/// `\Rustfw\delete.rec`, made (one sector) if missing.
fn rec_file(fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<Handle, Error> {
    let dir = fat.ensure_dir(d, fat.root(), DIR, t)?;
    let mut h = match fat.locate(d, dir, REC) {
        Ok(h) => h,
        Err(Error::NotFound) => fat.create(d, dir, REC, t)?,
        Err(e) => return Err(e),
    };
    if h.size < SECTOR as u32 {
        fat.write_sectors(d, &mut h, 0, &[0; SECTOR])?;
        fat.set_size(d, &mut h, SECTOR as u32, t)?;
    }
    Ok(h)
}

/// The folder file `k` of [`TO`] goes to, made if missing (a computer may have removed it since the record).
fn to_dir(fat: &mut Fat, d: &mut impl DiskWrite, k: usize, t: (u16, u16)) -> Result<File, Error> {
    let mine = fat.ensure_dir(d, fat.root(), DIR, t)?;
    let deleted = fat.ensure_dir(d, mine, DELETED, t)?;
    if k == 0 { Ok(deleted) } else { fat.ensure_dir(d, deleted, "Full", t) }
}

/// The CRC of the first sector of the chain `h` (None for an empty file): what says, after a cut, that a chain
/// named by no entry is still the ride's.
fn first_crc(fat: &mut Fat, d: &mut impl DiskWrite, h: &Handle) -> Result<Option<u16>, Error> {
    if h.first == 0 {
        return Ok(None);
    }
    let mut s = [0u8; SECTOR];
    fat.read_sector(d, h, 0, &mut s)?;
    Ok(Some(crate::fit::crc(0, &s)))
}

/// The first suffix from `from` whose name is free in every folder of `to` the files go to.
fn free_suffix(fat: &mut Fat, d: &mut impl DiskWrite, r: &Rec, from: u8) -> Result<u8, Error> {
    let mut buf = [0u8; 20];
    'next: for k in from..=SUFFIXES {
        for (to, f) in TO.iter().zip(r.files) {
            if f.is_none() {
                continue;
            }
            let dir = fat.open(d, to)?;
            match fat.find(d, dir, file_name(&r.name, k, &mut buf)?) {
                Err(Error::NotFound) => {}
                Ok(_) => continue 'next,
                Err(e) => return Err(e),
            }
        }
        return Ok(k);
    }
    Err(Error::Refused)
}

/// Moves the ride whose file in `\Activities` is `ride` (its first cluster: the history's), and its full copy if
/// any, into `\Rustfw\Deleted` (module doc); first finishes a move a cut left. `t`: (DOS date, time) of the new
/// entries and folders. [`Error::NotFound`] when no ride in `\Activities` has that chain (gone since the history
/// was made). A move of this ride a failure left (a card error midway) is finished: Ok, it is deleted.
#[inline(never)] // in its own frame, not the main loop's
pub fn delete(d: &mut impl DiskWrite, ride: &File, t: (u16, u16)) -> Result<(), Error> {
    let mut fat = Fat::mount(d)?;
    if finish_on(&mut fat, d, t)?.is_some_and(|r| r.files[0].map(|f| f.0) == Some(ride.cluster)) {
        return Ok(());
    }
    // the name the chain has in \Activities
    let acts = fat.open(d, ACTIVITIES)?;
    let mut c = fat.cursor(acts)?;
    let mut name = None;
    while let Some(e) = fat.list(d, &mut c)? {
        if !e.file.dir && e.file.cluster == ride.cluster && crate::stock::ride_start(e.name()).is_some() {
            name = e.name().get(..12).and_then(|n| <[u8; 12]>::try_from(n).ok());
            break;
        }
    }
    let name = name.ok_or(Error::NotFound)?;
    let mut buf = [0u8; 20];
    let mut files = [None; 2];
    for (k, from) in FROM.iter().enumerate() {
        let dir = match fat.open(d, from) {
            Ok(dir) => dir,
            Err(Error::NotFound) => continue, // no full copies at all
            Err(e) => return Err(e),
        };
        files[k] = match fat.locate(d, dir, file_name(&name, 0, &mut buf)?) {
            Ok(h) => Some((h.first, h.size, first_crc(&mut fat, d, &h)?)),
            Err(Error::NotFound | Error::Refused) => None,
            Err(e) => return Err(e),
        };
    }
    if files[0].map(|f| f.0) != Some(ride.cluster) {
        return Err(Error::NotFound);
    }
    // the folders, then a free name, then the record: all before the first entry changes. Made for the first time,
    // they leave no record to finish after a cut, which may leave the second FAT different in a sector: so the
    // FATs are made equal first whenever something is to be made (as the recorder's set-up)
    let made = fat.open(d, TO[0]).is_ok()
        && (files[1].is_none() || fat.open(d, TO[1]).is_ok())
        && fat.open(d, DIR).and_then(|dir| fat.locate(d, dir, REC)).is_ok_and(|h| h.size >= SECTOR as u32);
    if !made {
        fat.sync_fats(d)?;
    }
    let mine = fat.ensure_dir(d, fat.root(), DIR, t)?;
    let deleted = fat.ensure_dir(d, mine, DELETED, t)?;
    if files[1].is_some() {
        fat.ensure_dir(d, deleted, "Full", t)?;
    }
    let mut r = Rec { pending: true, name, suffix: 0, files };
    r.suffix = free_suffix(&mut fat, d, &r, 0)?;
    let mut rec = rec_file(&mut fat, d, t)?;
    fat.write_sectors(d, &mut rec, 0, &r.to_sector())?;
    moves(&mut fat, d, &mut r, &mut rec, t)?;
    r.pending = false;
    fat.write_sectors(d, &mut rec, 0, &r.to_sector())
}

/// At start-up, and before the card is lent to a computer: finishes a move a cut left (module doc); true if there
/// was one. Reads one sector when there is none.
#[inline(never)]
pub fn finish(d: &mut impl DiskWrite, t: (u16, u16)) -> Result<bool, Error> {
    let mut fat = Fat::mount(d)?;
    Ok(finish_on(&mut fat, d, t)?.is_some())
}

/// The move finished, if one was pending.
fn finish_on(fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<Option<Rec>, Error> {
    let mut rec = match fat.open(d, DIR).and_then(|dir| fat.locate(d, dir, REC)) {
        Ok(h) if h.size >= SECTOR as u32 => h,
        Ok(_) | Err(Error::NotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut s = [0u8; SECTOR];
    fat.read_sector(d, &rec, 0, &mut s)?;
    let Some(mut r) = Rec::from_sector(&s).filter(|r| r.pending) else { return Ok(None) };
    fat.sync_fats(d)?; // first: a FAT sector the cut left different (or torn) would break the chains read next
    moves(fat, d, &mut r, &mut rec, t)?;
    r.pending = false;
    fat.write_sectors(d, &mut rec, 0, &r.to_sector())?;
    Ok(Some(r))
}

/// Each file of `r` moved, or found moved already: the old entry marked deleted if it is still there, then the new
/// one, unless it is there already. A chain named by no entry is linked only if it is still the ride's: whole for
/// its size and its first sector the one the record says (a computer's fsck may have freed it meanwhile, and
/// something else taken it: linked, two files would share clusters). A new suffix goes into the record `rec`
/// before any entry changes, so a second cut finishes with the same one, in both folders.
fn moves(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, rec: &mut Handle, t: (u16, u16)) -> Result<(), Error> {
    let mut buf = [0u8; 20];
    for k in 0..2 {
        let (Some((first, size, crc)), Some(from)) = (r.files[k], FROM.get(k)) else { continue };
        let dst = to_dir(fat, d, k, t)?;
        match fat.find(d, dst, file_name(&r.name, r.suffix, &mut buf)?) {
            Ok(f) if f.cluster == first => continue, // moved already
            Ok(_) => {
                // a name taken since (by a computer): another suffix, free in both folders, recorded first
                for j in (0..2).filter(|&j| r.files[j].is_some()) {
                    to_dir(fat, d, j, t)?;
                }
                r.suffix = free_suffix(fat, d, r, r.suffix)?;
                fat.write_sectors(d, rec, 0, &r.to_sector())?;
            }
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let gone = match fat.open(d, from).and_then(|src| fat.locate(d, src, file_name(&r.name, 0, &mut buf)?)) {
            Ok(h) if h.first == first => {
                fat.remove(d, &h)?;
                false
            }
            Ok(_) | Err(Error::NotFound | Error::Corrupt | Error::Refused) => true, // gone already
            Err(e) => return Err(e),
        };
        let ours = match fat.chain(d, first) {
            Ok(h) => {
                h.clusters.checked_mul(fat.cluster_bytes()).is_none_or(|b| b >= size)
                    && (!gone || crc.is_none() || first_crc(fat, d, &h)? == crc)
            }
            Err(Error::Corrupt) => false,
            Err(e) => return Err(e),
        };
        if ours || first == 0 {
            let dst = to_dir(fat, d, k, t)?;
            fat.link(d, dst, file_name(&r.name, r.suffix, &mut buf)?, &Handle { first, size, ..Handle::default() }, t)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::Disk;
    use crate::fat::tests::{Ram, read_all};
    use crate::fat::write::tests::fsck;
    use std::vec::Vec;

    const T: (u16, u16) = (0x5b58, 0x6000);
    const RIDE: &str = "Activities/261020080000.fit";

    /// The test volume with a full copy of the 20 Oct ride in `\Rustfw\Activities`.
    pub(crate) fn volume() -> Vec<u8> {
        let mut d = Ram(crate::fat::tests::image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mut data = read_all(&mut fat, &mut d, RIDE).unwrap();
        data.reverse(); // not the same bytes as the clean copy
        let mine = fat.ensure_dir(&mut d, fat.root(), DIR, T).unwrap();
        let dir = fat.ensure_dir(&mut d, mine, ACTIVITIES, T).unwrap();
        let mut h = fat.create(&mut d, dir, "261020080000.fit", T).unwrap();
        let mut padded = data.clone();
        padded.resize(data.len().div_ceil(SECTOR) * SECTOR, 0);
        fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
        fat.set_size(&mut d, &mut h, data.len() as u32, T).unwrap();
        d.0
    }

    fn cluster(img: &[u8], path: &str) -> File {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        fat.open(&mut d, path).unwrap()
    }

    /// The file at `path`, if any.
    pub(crate) fn file(img: &[u8], path: &str) -> Option<Vec<u8>> {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        read_all(&mut fat, &mut d, path).ok()
    }

    /// Both copies moved under their name, read back the same, the volume whole; a second delete of the same ride
    /// (moved back by hand) gets a suffix; a chain no ride has is refused.
    #[test]
    fn moves_both_copies() {
        let img = volume();
        let (clean, full) = (file(&img, RIDE).unwrap(), file(&img, "Rustfw/Activities/261020080000.fit").unwrap());
        let mut d = Ram(img, 0);
        let f = cluster(&d.0, RIDE);
        delete(&mut d, &f, T).unwrap();
        assert_eq!(file(&d.0, RIDE), None);
        assert_eq!(file(&d.0, "Rustfw/Activities/261020080000.fit"), None);
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000.fit").as_ref(), Some(&clean));
        assert_eq!(file(&d.0, "Rustfw/Deleted/Full/261020080000.fit").as_ref(), Some(&full));
        assert_eq!(fsck(&d.0), Ok(0));
        // the clean copy moved back by hand, another file put under its name in \Rustfw\Deleted, deleted again
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, "Rustfw/Deleted").unwrap();
        let h = fat.locate(&mut d, del, "261020080000.fit").unwrap();
        fat.remove(&mut d, &h).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        fat.link(&mut d, acts, "261020080000.fit", &h, T).unwrap();
        fat.create(&mut d, del, "261020080000.fit", T).unwrap();
        let f = cluster(&d.0, RIDE);
        delete(&mut d, &f, T).unwrap();
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000-1.fit").as_ref(), Some(&clean), "a suffix");
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000.fit"), Some(Vec::new()), "the other file kept");
        assert_eq!(file(&d.0, "Rustfw/Deleted/Full/261020080000.fit").as_ref(), Some(&full), "the full copy kept");
        assert_eq!(fsck(&d.0), Ok(0));
        assert_eq!(delete(&mut d, &File { cluster: 3, size: 0, dir: false }, T), Err(Error::NotFound));
        assert_eq!(finish(&mut d, T), Ok(false), "nothing left to finish");
        let mut out = [0u8; 20];
        assert_eq!(file_name(b"261020080000", 42, &mut out), Ok("261020080000-42.fit"));
    }

    /// The volume of [`volume`] after a delete of the 20 Oct ride cut where its clean copy is in neither folder (its
    /// chain lost, the record pending).
    pub(crate) fn cut_midway() -> Vec<u8> {
        let img = volume();
        let ride = cluster(&img, RIDE);
        (0..200)
            .map(|budget| {
                let mut d = Cut(img.clone(), budget);
                let _ = delete(&mut d, &ride, T);
                d.0
            })
            .find(|img| file(img, RIDE).is_none() && file(img, "Rustfw/Deleted/261020080000.fit").is_none())
            .unwrap()
    }

    /// A disk that loses power after `budget` sectors written: that write and every later one never land.
    struct Cut(Vec<u8>, u64);

    impl Disk for Cut {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.0.get(lba as usize * SECTOR + i).copied().unwrap_or(0);
            }
            Ok(())
        }
    }

    impl DiskWrite for Cut {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
            for (k, s) in buf.chunks_exact(SECTOR).enumerate() {
                if self.1 == 0 {
                    return Err(Error::Io);
                }
                self.1 -= 1;
                let at = (lba as usize + k) * SECTOR;
                if self.0.len() < at + SECTOR {
                    self.0.resize(at + SECTOR, 0);
                }
                self.0[at..at + SECTOR].copy_from_slice(s);
            }
            Ok(())
        }
    }

    /// The power cut after every sector the delete writes: the volume fsck accepts (a lost chain at most, until the
    /// next start), and after the next start's [`finish`] each copy is in exactly one place, whole, and nothing is
    /// lost; a delete again then goes through.
    #[test]
    fn power_cut_at_every_sector() {
        let img = volume();
        let (clean, full) = (file(&img, RIDE).unwrap(), file(&img, "Rustfw/Activities/261020080000.fit").unwrap());
        let ride = cluster(&img, RIDE);
        let mut whole = Cut(img.clone(), u64::MAX);
        delete(&mut whole, &ride, T).unwrap();
        let writes = u64::MAX - whole.1;
        assert!(writes > 4, "{writes} sectors");
        let places = |img: &[u8]| {
            let at = |p: &str, want: &Vec<u8>| file(img, p).map(|f| assert_eq!(&f, want, "{p}")).is_some();
            (
                u8::from(at(RIDE, &clean)) + u8::from(at("Rustfw/Deleted/261020080000.fit", &clean)),
                u8::from(at("Rustfw/Activities/261020080000.fit", &full))
                    + u8::from(at("Rustfw/Deleted/Full/261020080000.fit", &full)),
            )
        };
        for budget in 0..writes {
            let mut d = Cut(img.clone(), budget);
            assert_eq!(delete(&mut d, &ride, T), Err(Error::Io), "cut after {budget}");
            // the second FAT may differ in the sector the cut fell in (fat/write.rs), put right by the next writer
            let lost = match fsck(&d.0) {
                Ok(lost) => lost,
                Err(e) if e.contains("FATs differ") => 1,
                Err(e) => panic!("cut after {budget}: {e}"),
            };
            let (a, b) = places(&d.0);
            assert!(a <= 1 && b <= 1, "cut after {budget}: never in two places");
            assert!(lost > 0 || (a, b) == (1, 1), "cut after {budget}: a copy out of sight is a lost chain");
            // the next start
            d.1 = u64::MAX;
            let finished = finish(&mut d, T).unwrap();
            if finished {
                assert_eq!(fsck(&d.0), Ok(0), "cut after {budget}, then the start");
            }
            assert_eq!(places(&d.0), (1, 1), "cut after {budget}: each copy in one place");
            if file(&d.0, RIDE).is_some() {
                delete(&mut d, &ride, T).unwrap();
            }
            assert_eq!(places(&d.0), (1, 1));
            assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some(), "cut after {budget}: deleted");
            // a cut while a folder or the record's file was made (no record yet) leaves its cluster lost: harmless
            let lost = fsck(&d.0).unwrap_or_else(|e| panic!("cut after {budget}: {e}"));
            assert!(lost == 0 || !finished && lost == 1, "cut after {budget}: {lost} lost");
        }
    }

    /// A chain the cut left named by no entry that a computer's fsck reconnected (FILE0000.CHK) and something then
    /// overwrote: no longer the ride's (its first sector not the one recorded), so it is not linked as the ride
    /// (the review of 2026-10-25: it was, and the two files shared clusters).
    #[test]
    fn a_reused_chain_is_not_linked() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let (h, root) = (Handle { first: ride.cluster, size: ride.size, ..Handle::default() }, fat.root());
        fat.link(&mut d, root, "FILE0000.CHK", &h, T).unwrap();
        let at = fat.lba(ride.cluster) as usize * SECTOR;
        d.0[at..at + 16].fill(0x42);
        assert_eq!(finish(&mut d, T), Ok(true));
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000.fit"), None, "not the ride any more");
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A delete that failed midway (a card error) and is asked again: the move is finished and the ride is deleted
    /// (it said DELETE FAILED, the ride no longer being in `\Activities` when looked for).
    #[test]
    fn a_delete_asked_again_after_a_failure_succeeds() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(cut_midway(), 0);
        assert_eq!(delete(&mut d, &ride, T), Ok(()));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some());
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A move finished after a cut, which has to take a suffix (a computer put a file of the ride's name in
    /// `\Rustfw\Deleted` meanwhile), cut again at every sector: the suffix is recorded before any entry changes, so
    /// the last start finishes with the same one, each copy in one place under it, no cluster shared.
    #[test]
    fn a_suffix_taken_while_finishing_survives_a_second_cut() {
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, "Rustfw/Deleted").unwrap();
        fat.create(&mut d, del, "261020080000.fit", T).unwrap();
        let img = d.0;
        let mut whole = Cut(img.clone(), u64::MAX);
        assert_eq!(finish(&mut whole, T), Ok(true));
        let writes = u64::MAX - whole.1;
        for budget in 0..writes {
            let mut d = Cut(img.clone(), budget);
            let _ = finish(&mut d, T);
            d.1 = u64::MAX;
            finish(&mut d, T).unwrap();
            let what = std::format!("cut after {budget} of {writes}");
            assert_eq!(fsck(&d.0).map_err(|e| std::format!("{what}: {e}")), Ok(0));
            for p in ["Rustfw/Deleted/261020080000-1.fit", "Rustfw/Deleted/Full/261020080000-1.fit"] {
                assert!(file(&d.0, p).is_some_and(|f| !f.is_empty()), "{what}: {p}");
            }
            assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000-2.fit"), None, "{what}");
        }
    }

    /// A pending move whose folder a computer removed meanwhile: made again and finished, never left pending (it
    /// stopped every later delete).
    #[test]
    fn a_removed_folder_is_made_again() {
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mine = fat.open(&mut d, DIR).unwrap();
        let lba = fat.lba(mine.cluster) as usize * SECTOR;
        let name: Vec<u8> = "Delet".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let k = (0..SECTOR / 32).find(|k| d.0[lba + k * 32 + 1..lba + k * 32 + 11] == name[..]).unwrap();
        d.0[lba + k * 32] = 0xe5; // its long name and its 8.3 entry: gone, as a computer deletes
        d.0[lba + (k + 1) * 32] = 0xe5;
        assert_eq!(file(&d.0, "Rustfw/Deleted/Full/261020080000.fit"), None);
        assert_eq!(finish(&mut d, T), Ok(true));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some());
        assert!(fsck(&d.0).is_ok(), "{:?}", fsck(&d.0));
    }
}
