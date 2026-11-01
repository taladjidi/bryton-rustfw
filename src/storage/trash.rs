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
//! no entry, so first the folders and one suffix free in both, then a record, `\Rustfw\delete.rec` (one sector: a
//! tag, "pending", the name, the suffix, each file's first cluster, size and first sector's CRC, a CRC), says what is
//! being moved, then room for both new entries is made (a folder grown by a cluster, under the record: a cut there
//! is finished, the FATs made equal, at the next start), and only then does an entry change; after the last move it
//! says "done". A cut anywhere leaves either the record not written (nothing
//! moved yet: a torn record does not check out and is ignored), or a pending record, which the next start finishes
//! ([`finish`]): the FATs made equal first ([`Fat::sync_fats`]), then each file is found in its new place (done), or
//! removed from its old place if still there, and linked into the new one if its chain is still whole and still the
//! ride's (its first sector's CRC: a computer's fsck may have freed it and something taken it). A suffix taken then
//! is recorded before any entry of that start changes, and a folder removed meanwhile is made again. A move that
//! cannot be finished (the card full since, every suffix taken) is undone instead: each file named by no entry is
//! linked back under its old name (`put_back`), the record then done, so a pending record never keeps the card
//! from a computer for good. The app finishes a pending move at the scan's start and before lending the card to a
//! computer (app/drive.rs), which would see the ride in neither folder. Tested by cutting the power after every
//! sector written (below).
//!
//! **The FAT sync** (`Fat::sync_fats`, both FATs read whole: 3 to 4 s on the owner's 15 MB, estimated) runs in one
//! call, as the recorder's (storage/recorder/mod.rs, "After a cut"): only at the first delete on a card (the
//! folders made) and when a cut left a move pending, before any chain is read; spreading it would leave a move half
//! known across passes.

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
pub(crate) struct Rec {
    pub(crate) pending: bool,
    pub(crate) name: [u8; 12],
    pub(crate) suffix: u8,
    pub(crate) files: [Option<Moved>; 2],
}

impl Rec {
    pub(crate) fn to_sector(self) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        s[..4].copy_from_slice(&TAG);
        s[4] = u8::from(self.pending);
        s[5..17].copy_from_slice(&self.name);
        s[17] = self.suffix;
        for (slot, f) in s[32..].chunks_exact_mut(16).zip(&self.files) {
            let (Some((first, size, crc)), Ok(slot)) = (f, <&mut [u8; 16]>::try_from(slot)) else { continue };
            slot[0] = 2;
            slot[4..8].copy_from_slice(&first.to_le_bytes());
            slot[8..12].copy_from_slice(&size.to_le_bytes());
            slot[12] = u8::from(crc.is_some());
            slot[14..16].copy_from_slice(&crc.unwrap_or(0).to_le_bytes());
        }
        super::seal16(&mut s);
        s
    }

    pub(crate) fn from_sector(s: &[u8; SECTOR]) -> Option<Rec> {
        if s[..4] != TAG || !super::sealed16(s) {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes(super::take(s, i));
        let file = |k: usize| {
            let at = 32 + 16 * k;
            let flag = *s.get(at)?;
            let crc = (flag == 2 && s.get(at + 12) == Some(&1)).then(|| u16::from_le_bytes(super::take(s, at + 14)));
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
    let mut put = |bytes: &[u8]| -> Result<(), Error> {
        let end = n + bytes.len();
        out.get_mut(n..end).ok_or(Error::Corrupt)?.copy_from_slice(bytes);
        n = end;
        Ok(())
    };
    if suffix > 0 {
        put(b"-")?;
        if suffix >= 10 {
            put(&[b'0' + suffix / 10 % 10])?;
        }
        put(&[b'0' + suffix % 10])?;
    }
    put(b".fit")?;
    core::str::from_utf8(out.get(..n).ok_or(Error::Corrupt)?).map_err(|_| Error::Corrupt)
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

/// Readies the move of `r` before any entry changes: its folders made (a computer may have removed one), the first
/// suffix from `r.suffix` whose name is, in each folder, free or the file itself (moved already: so both files end
/// under one suffix), then, if `room`, room for each new name ([`Fat::make_room`]: a full card refuses here, never
/// after an old entry went, the ride then named nowhere; the review of 2026-10-31; only with the record written, so
/// a cut while a folder grows is finished at the next start, the FATs made equal). A file already moved under `r.suffix` keeps
/// it: no later suffix then (the file would be linked a second time), [`Error::Refused`] if the other's name is
/// taken there. True if the suffix changed.
fn ready(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, room: bool, t: (u16, u16)) -> Result<bool, Error> {
    let mut buf = [0u8; 20];
    let mut dirs = [None; 2];
    for (k, (dir, f)) in dirs.iter_mut().zip(r.files).enumerate() {
        if f.is_some() {
            *dir = Some(to_dir(fat, d, k, t)?);
        }
    }
    let from = r.suffix;
    let mut last = SUFFIXES;
    for (dir, f) in dirs.iter().zip(r.files) {
        let (Some(dir), Some((first, _, _))) = (dir, f) else { continue };
        if fat.find(d, *dir, file_name(&r.name, from, &mut buf)?).is_ok_and(|f| f.cluster == first) {
            last = from;
        }
    }
    'next: for k in from..=last {
        for (dir, f) in dirs.iter().zip(r.files) {
            let (Some(dir), Some((first, _, _))) = (dir, f) else { continue };
            match fat.find(d, *dir, file_name(&r.name, k, &mut buf)?) {
                Ok(f) if f.cluster == first => {}
                Ok(_) => continue 'next,
                Err(Error::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        for dir in dirs.iter().flatten().filter(|_| room) {
            fat.make_room(d, *dir, file_name(&r.name, k, &mut buf)?)?;
        }
        r.suffix = k;
        return Ok(k != from);
    }
    Err(Error::Refused)
}

/// Moves the ride whose file in `\Activities` is `ride` (its first cluster: the history's), and its full copy if
/// any, into `\Rustfw\Deleted` (module doc); first finishes a move a cut left. `t`: (DOS date, time) of the new
/// entries and folders. [`Error::NotFound`] when no ride in `\Activities` has that chain (gone since the history
/// was made). A move of this ride a failure left (a card error midway) is finished: Ok, it is deleted.
/// [`Error::Refused`]: not deleted, the ride where it was, nothing pending (the card full, every suffix taken; the
/// record written and done).
#[inline(never)] // in its own frame, not the main loop's
pub fn delete(d: &mut impl DiskWrite, ride: &File, t: (u16, u16)) -> Result<(), Error> {
    let mut fat = Fat::mount(d)?;
    // a pending move that cannot even be put back: not this delete's refusal (Refused: nothing pending, the card
    // full), so a card error to the app, which then keeps finishing it
    let left = finish_on(&mut fat, d, t).map_err(|e| if e == Error::Refused { Error::Io } else { e })?;
    if left.is_some_and(|r| r.files[0].map(|f| f.0) == Some(ride.cluster)) {
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
    for (slot, from) in files.iter_mut().zip(FROM.iter()) {
        let dir = match fat.open(d, from) {
            Ok(dir) => dir,
            Err(Error::NotFound) => continue, // no full copies at all
            Err(e) => return Err(e),
        };
        *slot = match fat.locate(d, dir, file_name(&name, 0, &mut buf)?) {
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
    let mut r = Rec { pending: true, name, suffix: 0, files };
    ready(&mut fat, d, &mut r, false, t)?;
    let mut rec = rec_file(&mut fat, d, t)?;
    fat.write_sectors(d, &mut rec, 0, &r.to_sector())?;
    if settle(&mut fat, d, &mut r, &mut rec, t)? { Ok(()) } else { Err(Error::Refused) }
}

/// At start-up, and before the card is lent to a computer: finishes a move a cut left (module doc); true if there
/// was one. Reads one sector when there is none.
#[inline(never)]
pub fn finish(d: &mut impl DiskWrite, t: (u16, u16)) -> Result<bool, Error> {
    let mut fat = Fat::mount(d)?;
    Ok(finish_on(&mut fat, d, t)?.is_some())
}

/// The move finished, if one was pending and could be (else put back: None).
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
    Ok(settle(fat, d, &mut r, &mut rec, t)?.then_some(r))
}

/// [`moves`], then the record done. A move that cannot be finished ([`Error::Refused`]: the card full, every suffix
/// taken) is put back instead (`put_back`). True if moved.
fn settle(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, rec: &mut Handle, t: (u16, u16)) -> Result<bool, Error> {
    let moved = match moves(fat, d, r, rec, t) {
        Ok(()) => true,
        Err(Error::Refused) => {
            put_back(fat, d, r, t)?;
            false
        }
        Err(e) => return Err(e),
    };
    r.pending = false;
    fat.write_sectors(d, rec, 0, &r.to_sector())?;
    Ok(moved)
}

/// Each file of `r` named by no entry (neither moved nor still in its old place) linked back under its old name in
/// its old folder, if its chain is still the ride's: a move undone ([`settle`]). Room made first, so a card still
/// full refuses before any entry changes and the move stays pending. Nothing is made: a folder gone is an error.
fn put_back(fat: &mut Fat, d: &mut impl DiskWrite, r: &Rec, t: (u16, u16)) -> Result<(), Error> {
    let mut buf = [0u8; 20];
    for k in 0..2 {
        let (Some((first, size, crc)), Some(from), Some(to)) =
            (r.files.get(k).copied().flatten(), FROM.get(k), TO.get(k))
        else {
            continue;
        };
        let moved = match fat.open(d, to) {
            Ok(dst) => fat.find(d, dst, file_name(&r.name, r.suffix, &mut buf)?).is_ok_and(|f| f.cluster == first),
            Err(Error::NotFound) => false,
            Err(e) => return Err(e),
        };
        let src = fat.open(d, from)?;
        let name = file_name(&r.name, 0, &mut buf)?;
        let there = match fat.locate(d, src, name) {
            Ok(h) => h.first == first,
            Err(Error::NotFound | Error::Refused) => false,
            Err(e) => return Err(e),
        };
        if moved || there || !(first == 0 || still_ours(fat, d, first, size, crc, true)?) {
            continue;
        }
        fat.make_room(d, src, name)?;
        fat.link(d, src, name, &Handle { first, size, ..Handle::default() }, t)?;
    }
    Ok(())
}

/// The chain from `first` is still the ride's: whole for `size`, and, if it is named by no entry (`gone`: a
/// computer's fsck may have freed it and something taken it), its first sector the one the record says.
fn still_ours(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    first: u32,
    size: u32,
    crc: Option<u16>,
    gone: bool,
) -> Result<bool, Error> {
    match fat.chain(d, first) {
        Ok(h) => Ok(h.clusters.checked_mul(fat.cluster_bytes()).is_none_or(|b| b >= size)
            && (!gone || crc.is_none() || first_crc(fat, d, &h)? == crc)),
        Err(Error::Corrupt) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Each file of `r` moved, or found moved already: the old entry marked deleted if it is still there, then the new
/// one, unless it is there already. A chain named by no entry is linked only if it is still the ride's: whole for
/// its size and its first sector the one the record says (a computer's fsck may have freed it meanwhile, and
/// something else taken it: linked, two files would share clusters). The suffix and the room come first
/// ([`ready`]); a new suffix goes into the record `rec` before any entry of this call changes, so a second cut
/// finishes with the same one, in both folders. Shortcut: after a cut that left a chain nameless, that rewrite
/// of the one-sector record relies on the card's atomic sector write (enhanced reliable write, docs/journal.md
/// 2026-10-24); two slots, as `ride.rec` has, if a card without it is ever used.
fn moves(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, rec: &mut Handle, t: (u16, u16)) -> Result<(), Error> {
    if ready(fat, d, r, true, t)? {
        fat.write_sectors(d, rec, 0, &r.to_sector())?;
    }
    let mut buf = [0u8; 20];
    for k in 0..2 {
        let (Some((first, size, crc)), Some(from)) = (r.files.get(k).copied().flatten(), FROM.get(k)) else { continue };
        let dst = to_dir(fat, d, k, t)?;
        match fat.find(d, dst, file_name(&r.name, r.suffix, &mut buf)?) {
            Ok(f) if f.cluster == first => continue, // moved already
            Ok(_) => return Err(Error::Refused),     // free in `ready`, and nothing wrote since
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
        if first == 0 || still_ours(fat, d, first, size, crc, gone)? {
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
        cut_every_sector(volume(), false);
    }

    /// The same where `\Rustfw\Deleted` must grow a cluster for the name ([`ready`]'s room, under the record), on a
    /// card that has one (the review of 2026-11-01): a cut while it grows may leave that cluster lost (one).
    #[test]
    fn power_cut_at_every_sector_while_the_folder_grows() {
        let mut d = Ram(volume(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        to_dir(&mut fat, &mut d, 1, T).unwrap();
        rec_file(&mut fat, &mut d, T).unwrap();
        no_room(&mut d.0, false);
        cut_every_sector(d.0, true);
    }

    fn cut_every_sector(img: Vec<u8>, grows: bool) {
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
                let lost = fsck(&d.0);
                assert!(lost == Ok(0) || grows && lost == Ok(1), "cut after {budget}, then the start: {lost:?}");
            }
            assert_eq!(places(&d.0), (1, 1), "cut after {budget}: each copy in one place");
            if file(&d.0, RIDE).is_some() {
                delete(&mut d, &ride, T).unwrap();
            }
            assert_eq!(places(&d.0), (1, 1));
            assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some(), "cut after {budget}: deleted");
            // a cut while a folder or the record's file was made (no record yet) leaves its cluster lost: harmless
            let lost = fsck(&d.0).unwrap_or_else(|e| panic!("cut after {budget}: {e}"));
            assert!(lost == 0 || (!finished || grows) && lost == 1, "cut after {budget}: {lost} lost");
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

    /// `\Rustfw\Deleted` (made already) with no room for one more name without a new cluster (all its slots used
    /// but two, as many deletes leave), on a card with no free cluster if `full` (the owner's, nearly full of map
    /// tiles).
    pub(crate) fn no_room(img: &mut [u8], full: bool) {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, TO[0]).unwrap();
        assert_eq!(fat.chain(&mut d, del.cluster).unwrap().clusters, 1, "one cluster");
        let (lba, slots) = (fat.lba(del.cluster) as usize * SECTOR, fat.cluster_bytes() as usize / 32);
        let end = (0..slots).find(|k| img[lba + k * 32] == 0).unwrap();
        for k in end..slots - 2 {
            img[lba + k * 32] = 0xe5;
            img[lba + k * 32 + 11] = 0x20;
        }
        if full {
            crate::fat::write::tests::fill(img, 0, true);
        }
    }

    /// [`volume`] with the folders and the record made, on a full card ([`no_room`]).
    pub(crate) fn full_card() -> Vec<u8> {
        let mut d = Ram(volume(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        to_dir(&mut fat, &mut d, 1, T).unwrap();
        rec_file(&mut fat, &mut d, T).unwrap();
        no_room(&mut d.0, true);
        d.0
    }

    /// The review of 2026-11-01: a move a cut left halfway (a record of an older build: the clean copy named
    /// nowhere) on a card full since: it cannot be finished, so it is put back, the ride in `\Activities` again and
    /// the record done (it stayed pending for good, the card lent write protected, every delete refused).
    #[test]
    fn a_move_that_cannot_finish_is_put_back() {
        let ride = cluster(&volume(), RIDE);
        let clean = file(&volume(), RIDE).unwrap();
        let mut d = Ram(cut_midway(), 0);
        no_room(&mut d.0, true);
        assert_eq!(file(&d.0, RIDE), None);
        assert_eq!(finish(&mut d, T), Ok(false), "not moved: put back");
        assert!(file(&d.0, RIDE) == Some(clean), "the ride back where it was");
        assert!(file(&d.0, "Rustfw/Activities/261020080000.fit").is_some(), "the full copy where it was");
        assert_eq!(fsck(&d.0), Ok(0));
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
        assert_eq!(delete(&mut d, &ride, T), Err(Error::Refused), "still full: refused, nothing moved");
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// The review of 2026-11-01: the clean copy moved already under the record's suffix, the full copy's name then
    /// taken there: no later suffix for both (the clean chain linked a second time), the move put back instead.
    #[test]
    fn a_file_moved_already_keeps_its_suffix() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, TO[0]).unwrap();
        let h = Handle { first: ride.cluster, size: ride.size, ..Handle::default() };
        fat.link(&mut d, del, "261020080000.fit", &h, T).unwrap(); // as the finish's first half
        let full = fat.open(&mut d, TO[1]).unwrap();
        fat.create(&mut d, full, "261020080000.fit", T).unwrap();
        assert_eq!(finish(&mut d, T), Ok(false));
        assert_eq!(fsck(&d.0), Ok(0), "no chain named twice");
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some_and(|f| !f.is_empty()));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000-1.fit").is_none());
        assert!(file(&d.0, "Rustfw/Activities/261020080000.fit").is_some(), "the full copy where it was");
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
    }

    /// The review of 2026-10-31 (storage, 1): a delete on a full card whose `\Rustfw\Deleted` needs a cluster for
    /// the new name is refused before any entry changes: the ride still in `\Activities`, no move pending (the
    /// old entry went first, and the ride was in neither folder).
    #[test]
    fn a_full_card_refuses_the_delete_with_the_ride_in_place() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(full_card(), 0);
        assert_eq!(delete(&mut d, &ride, T), Err(Error::Refused));
        assert!(file(&d.0, RIDE).is_some(), "the ride where it was");
        assert_eq!(finish(&mut d, T), Ok(false), "no move pending");
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// The review of 2026-10-31 (storage, 6): a name taken in one folder only (a computer put a file of the ride's
    /// name in `\Rustfw\Deleted\Full`), for a delete and for a move a cut left, then a cut at every sector and the
    /// next start: the suffix is chosen before any entry changes, so both copies end under the same one (`-1`), each
    /// in one place, no chain named twice (the clean copy was linked under no suffix, then the record said `-1`,
    /// and a cut after that linked it again).
    #[test]
    fn a_name_taken_in_one_folder_gives_both_copies_one_suffix() {
        let taken = |img: Vec<u8>| {
            let mut d = Ram(img, 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let full = to_dir(&mut fat, &mut d, 1, T).unwrap();
            fat.create(&mut d, full, "261020080000.fit", T).unwrap();
            rec_file(&mut fat, &mut d, T).unwrap(); // nothing left for the delete to make (a cluster a cut would lose)
            d.0
        };
        let ride = cluster(&volume(), RIDE);
        let (clean, full) =
            (file(&volume(), RIDE).unwrap(), file(&volume(), "Rustfw/Activities/261020080000.fit").unwrap());
        for (what, img) in [("delete", taken(volume())), ("finish", taken(cut_midway()))] {
            let run = |d: &mut Cut| if what == "delete" { delete(d, &ride, T) } else { finish(d, T).map(|_| ()) };
            let mut whole = Cut(img.clone(), u64::MAX);
            run(&mut whole).unwrap();
            let writes = u64::MAX - whole.1;
            for budget in 0..=writes {
                let mut d = Cut(img.clone(), budget);
                let _ = run(&mut d);
                d.1 = u64::MAX;
                finish(&mut d, T).unwrap();
                if file(&d.0, RIDE).is_some() {
                    delete(&mut d, &ride, T).unwrap(); // cut before its record: asked again
                }
                let at = std::format!("{what}, cut after {budget} of {writes}");
                assert_eq!(fsck(&d.0).map_err(|e| std::format!("{at}: {e}")), Ok(0));
                assert!(file(&d.0, "Rustfw/Deleted/261020080000-1.fit") == Some(clean.clone()), "{at}: clean");
                assert!(file(&d.0, "Rustfw/Deleted/Full/261020080000-1.fit") == Some(full.clone()), "{at}: full");
                assert_eq!(
                    file(&d.0, "Rustfw/Deleted/Full/261020080000.fit"),
                    Some(Vec::new()),
                    "{at}: the other file"
                );
                for p in [RIDE, "Rustfw/Activities/261020080000.fit", "Rustfw/Deleted/261020080000.fit"] {
                    assert!(file(&d.0, p).is_none(), "{at}: {p}");
                }
            }
        }
    }
}
