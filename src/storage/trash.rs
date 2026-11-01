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
//! ride's (its first sector's CRC: a computer's fsck may have freed it and something taken it). A ride a computer
//! renamed or moved meanwhile (`\Backup`), or a lost chain its repair named, is left where it is named: never two
//! names on one chain, and that ride not deleted (the history keeps it). A suffix taken then
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
    let dir = fat.ensure_own_dir(d, fat.root(), DIR, t)?;
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
    let mine = fat.ensure_own_dir(d, fat.root(), DIR, t)?;
    let deleted = fat.ensure_own_dir(d, mine, DELETED, t)?;
    if k == 0 { Ok(deleted) } else { fat.ensure_own_dir(d, deleted, "Full", t) }
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
        let mut new = [None; 2]; // the folders a name goes into (not those the file is in already)
        for ((dir, f), new) in dirs.iter().zip(r.files).zip(new.iter_mut()) {
            let (Some(dir), Some((first, _, _))) = (dir, f) else { continue };
            match fat.find(d, *dir, file_name(&r.name, k, &mut buf)?) {
                Ok(f) if f.cluster == first => {}
                Ok(_) => continue 'next,
                Err(Error::NotFound) => *new = Some(*dir),
                Err(e) => return Err(e),
            }
        }
        for dir in new.iter().flatten().filter(|_| room) {
            fat.make_room(d, *dir, file_name(&r.name, k, &mut buf)?)?;
        }
        r.suffix = k;
        return Ok(k != from);
    }
    Err(Error::Refused)
}

/// Moves the ride whose file in `\Activities` is `ride` (its first cluster: the history's), and its full copy if
/// any, into `\Rustfw\Deleted` (module doc); first finishes a move a cut left. `t`: (DOS date, time) of the new
/// entries and folders. [`Error::NotFound`] when no ride in `\Activities` has that chain (moved or renamed by a
/// computer since the history was made, or a pending move of it found it so: the record then done, the ride where
/// the computer put it). A move of this ride a failure left (a card error midway) is finished: Ok, it is deleted.
/// [`Error::Refused`]: not deleted, the ride where it was, nothing pending (the card full, every suffix taken; the
/// record written and done).
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
/// was one and the ride moved (false: none, or put back, or left where a computer named it). Reads one sector when
/// there is none.
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
/// taken) is put back instead (`put_back`). True if moved; false if put back, or if the ride was left where a
/// computer named it meanwhile (`moves`: it was never deleted, so the history keeps it).
fn settle(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, rec: &mut Handle, t: (u16, u16)) -> Result<bool, Error> {
    let moved = match moves(fat, d, r, rec, t) {
        Ok(moved) => moved,
        Err(Error::Refused) => {
            // a put back refused leaves the move pending: never `Refused` out of here ([`delete`]: nothing pending)
            put_back(fat, d, r, t).map_err(|e| if e == Error::Refused { Error::Io } else { e })?;
            false
        }
        Err(e) => return Err(e),
    };
    r.pending = false;
    fat.write_sectors(d, rec, 0, &r.to_sector())?;
    Ok(moved)
}

/// A move undone ([`settle`]): each file of `r` back in its old folder, under its old name (taken by another file
/// meanwhile: a copy moved stays moved, a chain named by no entry takes `<name>-j.fit`), if its chain is still the
/// ride's. A file moved already is
/// linked back, then taken out of its new folder (so neither copy stays half deleted, and it has a name at every
/// cut, whatever a computer did to its bytes); room is made before anything changes.
/// When the old folder cannot take a name (the card full, its last slot used), a file named by no entry gets its
/// old entry back ([`Fat::revive`]: no cluster), and a file moved already stays moved (in sight, nothing lost).
/// Nothing is made: a folder gone is an error. A file put back in none of these ways: [`Error::Io`], the move
/// left pending.
fn put_back(fat: &mut Fat, d: &mut impl DiskWrite, r: &Rec, t: (u16, u16)) -> Result<(), Error> {
    let mut buf = [0u8; 20];
    'files: for k in 0..2 {
        let (Some((first, size, crc)), Some(from), Some(to)) =
            (r.files.get(k).copied().flatten(), FROM.get(k), TO.get(k))
        else {
            continue;
        };
        let src = fat.open(d, from)?;
        let moved = match fat.open(d, to) {
            Ok(dst) => match fat.locate(d, dst, file_name(&r.name, r.suffix, &mut buf)?) {
                Ok(h) if h.first == first => Some(h),
                Ok(_) | Err(Error::NotFound | Error::Refused) => None,
                Err(e) => return Err(e),
            },
            Err(Error::NotFound) => None,
            Err(e) => return Err(e),
        };
        // named in the old folder already (linked back before a cut), under any name (a computer may have renamed it),
        // or anywhere a computer may have put it (`\Backup`) or a repair named a lost chain (`named_on_top`)
        if named_in(fat, d, src, b"", first)?.is_some() || named_on_top(fat, d, first)? {
            if let Some(h) = moved {
                fat.remove(d, &h)?; // linked back already: its name in \Rustfw\Deleted goes now
            }
            continue;
        }
        // only the ride's chain goes back (a copy in \Rustfw\Deleted a computer changed stays there, named)
        if !(first == 0 || still_ours(fat, d, first, size, crc, true)?) {
            continue;
        }
        // under its old name, the one both firmwares' histories find; taken by another file meanwhile, a copy moved
        // stays where it is named, and a chain named by no entry takes the first free `<name>-j.fit` (a name anywhere
        // beats a move pending for good, the drive read only)
        let mut j = 0;
        loop {
            match fat.find(d, src, file_name(&r.name, j, &mut buf)?) {
                Err(Error::NotFound) => break,
                Ok(_) if moved.is_some() => continue 'files,
                Ok(_) if j < SUFFIXES => j += 1,
                Ok(_) => return Err(Error::Io),
                Err(e) => return Err(e),
            }
        }
        let name = file_name(&r.name, j, &mut buf)?;
        let room = match fat.make_room(d, src, name) {
            Ok(()) => true,
            Err(Error::Refused) => false,
            Err(e) => return Err(e),
        };
        if room {
            // a copy moved: linked back first, its name in \Rustfw\Deleted removed after, so it has a name at every
            // cut (two for a moment, the record pending: the next start removes the second)
            let size = moved.map_or(size, |h| h.size); // a copy a computer made longer: all of it
            fat.link(d, src, name, &Handle { first, size, ..Handle::default() }, t)?;
            if let Some(h) = moved {
                fat.remove(d, &h)?;
            }
        } else if moved.is_none() && !(j == 0 && fat.revive(d, src, name, first, size)?) {
            return Err(Error::Io);
        }
    }
    Ok(())
}

/// The chain from `first` named by a file of the root or of one of its folders (one level down): a computer's
/// repair of a lost chain (Windows 9x's `FILE0000.CHK`, dosfsck's `FSCK0000.REC` in the root, chkdsk /f's
/// `\FOUND.nnn`, Android's `\LOST.DIR`), or a ride a computer moved or renamed (`\Backup`, `\Activities`). Linked
/// again, the two names would share the chain. An empty file (cluster 0) never matches. Cost: the root and each of
/// its folders read whole (each at most FAT's 65536 entries, 4096 sectors; on stock's layout a few folders, the
/// bulk `\Activities` at about 5 rides a sector), only after a cut or a computer's change (`moves`, `put_back`).
pub(crate) fn named_on_top(fat: &mut Fat, d: &mut impl DiskWrite, first: u32) -> Result<bool, Error> {
    if first == 0 {
        return Ok(false);
    }
    let mut c = fat.cursor(fat.root())?;
    while let Some(e) = fat.list(d, &mut c)? {
        if !e.file.dir && e.file.cluster == first {
            return Ok(true);
        }
        if e.file.dir {
            let mut c = match fat.cursor(e.file) {
                Ok(c) => c,
                Err(Error::Corrupt) => continue,
                Err(e) => return Err(e),
            };
            loop {
                match fat.list(d, &mut c) {
                    Ok(Some(f)) if !f.file.dir && f.file.cluster == first => return Ok(true),
                    Ok(Some(_)) => {}
                    Ok(None) | Err(Error::Corrupt) => break, // a damaged folder names nothing we can trust
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(false)
}

/// Every entry of folder `from` on the chain `first` under a name of the ride's (`<name>*`: an older build's put
/// back gave suffixes), marked deleted; true if there was one. A folder that cannot be read: none there.
fn unlink_old(fat: &mut Fat, d: &mut impl DiskWrite, from: &str, name: &[u8; 12], first: u32) -> Result<bool, Error> {
    let src = match fat.open(d, from) {
        Ok(src) => src,
        Err(Error::NotFound | Error::Corrupt | Error::Refused) => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut any = false;
    for _ in 0..=SUFFIXES {
        let Some(e) = named_in(fat, d, src, name, first)? else { break };
        // a name the listing cannot give back as it is ('?' for what is not ASCII) is not looked up: left
        let Ok(n) = core::str::from_utf8(e.name()) else { break };
        match fat.locate(d, src, n) {
            Ok(h) if h.first == first => fat.remove(d, &h)?,
            Err(Error::Io) => return Err(Error::Io),
            _ => break, // not found by that name, or another entry of it first: never remove that
        }
        any = true;
    }
    Ok(any)
}

/// The first entry of folder `dir` on the chain `first` whose name starts with `name` (the ride's, or empty: any):
/// the whole folder read (not name by name, which a free name stopped). A folder that cannot be read past a point:
/// none after it.
fn named_in(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    dir: File,
    name: &[u8],
    first: u32,
) -> Result<Option<crate::fat::Entry>, Error> {
    if first == 0 {
        return Ok(None); // an empty file: any empty `<name>.fit` would match
    }
    let mut c = fat.cursor(dir)?;
    loop {
        match fat.list(d, &mut c) {
            Ok(Some(e)) if !e.file.dir && e.file.cluster == first && e.name().starts_with(name) => return Ok(Some(e)),
            Ok(Some(_)) => {}
            Ok(None) | Err(Error::Corrupt) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

/// The chain from `first` named in folder `path` under any name. A folder that cannot be read: not named there.
fn named_at(fat: &mut Fat, d: &mut impl DiskWrite, path: &str, first: u32) -> Result<bool, Error> {
    match fat.open(d, path) {
        Ok(dir) => Ok(named_in(fat, d, dir, b"", first)?.is_some()),
        Err(Error::NotFound | Error::Corrupt | Error::Refused) => Ok(false),
        Err(e) => Err(e),
    }
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
///
/// False if the clean copy was left where a computer named it (renamed in `\Activities`, moved to `\Backup`) while
/// the move was pending: not deleted then, and its full copy left where it is named too.
fn moves(fat: &mut Fat, d: &mut impl DiskWrite, r: &mut Rec, rec: &mut Handle, t: (u16, u16)) -> Result<bool, Error> {
    if ready(fat, d, r, true, t)? {
        fat.write_sectors(d, rec, 0, &r.to_sector())?;
    }
    let mut buf = [0u8; 20];
    let mut skipped = false;
    for k in 0..2 {
        let (Some((first, size, crc)), Some(from)) = (r.files.get(k).copied().flatten(), FROM.get(k)) else { continue };
        let dst = to_dir(fat, d, k, t)?;
        match fat.find(d, dst, file_name(&r.name, r.suffix, &mut buf)?) {
            Ok(f) if f.cluster == first => {
                // a name a cut put back left there: one name only. A computer renamed or moved that one meanwhile:
                // it stays where the computer put it, and the name here goes (as `put_back` does)
                if !unlink_old(fat, d, from, &r.name, first)? && named_elsewhere(fat, d, from, first)? {
                    let h = fat.locate(d, dst, file_name(&r.name, r.suffix, &mut buf)?)?;
                    if h.first == first {
                        fat.remove(d, &h)?;
                        skipped |= k == 0;
                    }
                }
                continue; // moved already
            }
            Ok(_) => return Err(Error::Refused), // free in `ready`, and nothing wrote since
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        if skipped && (first == 0 || named_at(fat, d, from, first)?) {
            continue; // the clean copy left in sight: the full copy stays with it
        }
        let gone = !unlink_old(fat, d, from, &r.name, first)?;
        // a chain named still, under a name not the ride's (a computer renamed or moved it, a damaged folder's twin
        // name hid it from `unlink_old`), or by a repair: in sight, left there (linked, two names would share it)
        let named = gone && named_elsewhere(fat, d, from, first)?;
        skipped |= k == 0 && named;
        if first == 0 || still_ours(fat, d, first, size, crc, gone)? && !named {
            fat.link(d, dst, file_name(&r.name, r.suffix, &mut buf)?, &Handle { first, size, ..Handle::default() }, t)?;
        }
    }
    Ok(!skipped)
}

/// The chain from `first` named in folder `from` or near the root ([`named_on_top`]), under any name.
fn named_elsewhere(fat: &mut Fat, d: &mut impl DiskWrite, from: &str, first: u32) -> Result<bool, Error> {
    Ok(named_at(fat, d, from, first)? || named_on_top(fat, d, first)?)
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, read_all};
    use crate::fat::write::tests::{Cut, fsck, stock, stock_kept};
    use std::vec::Vec;

    const T: (u16, u16) = (0x5b58, 0x6000);
    const RIDE: &str = "Activities/261020080000.fit";

    /// The test volume with a full copy of the 20 Oct ride in `\Rustfw\Activities`.
    pub(crate) fn volume() -> Vec<u8> {
        let mut d = Ram(crate::fat::tests::image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let mut data = read_all(&mut fat, &mut d, RIDE).unwrap();
        data.reverse(); // not the same bytes as the clean copy
        let mine = fat.ensure_own_dir(&mut d, fat.root(), DIR, T).unwrap();
        let dir = fat.ensure_own_dir(&mut d, mine, ACTIVITIES, T).unwrap();
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
        crate::fat::tests::dump("history_deleted", &d.0);
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
                let mut d = Cut(Ram(img.clone(), 0), budget);
                let _ = delete(&mut d, &ride, T);
                d.0.0
            })
            .find(|img| file(img, RIDE).is_none() && file(img, "Rustfw/Deleted/261020080000.fit").is_none())
            .unwrap()
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
        let mut whole = Ram(d.0.clone(), 0);
        delete(&mut whole, &cluster(&d.0, RIDE), T).unwrap();
        assert_eq!(dir_clusters(&whole.0, TO[0]), dir_clusters(&d.0, TO[0]) + 1, "the folder grew");
        cut_every_sector(d.0, true);
    }

    fn cut_every_sector(img: Vec<u8>, grows: bool) {
        let (clean, full) = (file(&img, RIDE).unwrap(), file(&img, "Rustfw/Activities/261020080000.fit").unwrap());
        let ride = cluster(&img, RIDE);
        // stock's files but the ride moved (the review of 2026-11-01, finding 4): bytes, entries, chains
        let st = stock(&img);
        let kept = |img: &[u8], at: &str| stock_kept(&st, img, &[], &[RIDE]).unwrap_or_else(|e| panic!("{at}: {e}"));
        let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
        delete(&mut whole, &ride, T).unwrap();
        let writes = u32::MAX - whole.1;
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
            let mut d = Cut(Ram(img.clone(), 0), budget);
            assert_eq!(delete(&mut d, &ride, T), Err(Error::Io), "cut after {budget}");
            // the second FAT may differ in the sector the cut fell in (fat/write.rs), put right by the next writer
            let lost = match fsck(&d.0.0) {
                Ok(lost) => lost,
                Err(e) if e.contains("FATs differ") => 1,
                Err(e) => panic!("cut after {budget}: {e}"),
            };
            let (a, b) = places(&d.0.0);
            assert!(a <= 1 && b <= 1, "cut after {budget}: never in two places");
            kept(&d.0.0, &std::format!("cut after {budget}"));
            assert!(lost > 0 || (a, b) == (1, 1), "cut after {budget}: a copy out of sight is a lost chain");
            // the next start
            d.1 = u32::MAX;
            let finished = finish(&mut d, T).unwrap();
            if finished {
                let lost = fsck(&d.0.0);
                assert!(lost == Ok(0) || grows && lost == Ok(1), "cut after {budget}, then the start: {lost:?}");
            }
            assert_eq!(places(&d.0.0), (1, 1), "cut after {budget}: each copy in one place");
            if file(&d.0.0, RIDE).is_some() {
                delete(&mut d, &ride, T).unwrap();
            }
            assert_eq!(places(&d.0.0), (1, 1));
            assert!(file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some(), "cut after {budget}: deleted");
            // a cut while a folder or the record's file was made (no record yet) leaves its cluster lost: harmless
            let lost = fsck(&d.0.0).unwrap_or_else(|e| panic!("cut after {budget}: {e}"));
            assert!(lost == 0 || (!finished || grows) && lost == 1, "cut after {budget}: {lost} lost");
            kept(&d.0.0, &std::format!("cut after {budget}, then the start"));
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
        assert_eq!(finish(&mut d, T), Ok(false), "named by the repair: left there");
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
        let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
        assert_eq!(finish(&mut whole, T), Ok(true));
        let writes = u32::MAX - whole.1;
        for budget in 0..writes {
            let mut d = Cut(Ram(img.clone(), 0), budget);
            let _ = finish(&mut d, T);
            d.1 = u32::MAX;
            finish(&mut d, T).unwrap();
            let what = std::format!("cut after {budget} of {writes}");
            assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{what}: {e}")), Ok(0));
            for p in ["Rustfw/Deleted/261020080000-1.fit", "Rustfw/Deleted/Full/261020080000-1.fit"] {
                assert!(file(&d.0.0, p).is_some_and(|f| !f.is_empty()), "{what}: {p}");
            }
            assert_eq!(file(&d.0.0, "Rustfw/Deleted/261020080000-2.fit"), None, "{what}");
        }
        // the file in the way deleted from outside before the next start: the suffix the record has holds (a first
        // copy linked under `-1`, a record that still said none: linked again under the plain name, the chain named
        // twice)
        let under = |img: &[u8], s: &str| {
            ["Rustfw/Deleted/", "Rustfw/Deleted/Full/"]
                .map(|dir| file(img, &std::format!("{dir}261020080000{s}.fit")).is_some_and(|f| !f.is_empty()))
        };
        for budget in 0..writes {
            let mut d = Cut(Ram(img.clone(), 0), budget);
            let _ = finish(&mut d, T);
            d.1 = u32::MAX;
            let mut fat = Fat::mount(&mut d).unwrap();
            let del = fat.open(&mut d, TO[0]).unwrap();
            let h = fat.locate(&mut d, del, "261020080000.fit").unwrap();
            assert_eq!(h.first, 0, "the file in the way");
            fat.remove(&mut d, &h).unwrap();
            finish(&mut d, T).unwrap();
            let what = std::format!("cut after {budget} of {writes}, the file in the way gone");
            assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{what}: {e}")), Ok(0));
            let one = matches!(
                (under(&d.0.0, ""), under(&d.0.0, "-1")),
                ([true, true], [false, false]) | ([false, false], [true, true])
            );
            assert!(one, "{what}: both copies under one name");
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
        crowd(img, TO[0]);
        if full {
            crate::fat::write::tests::fill(img, 0, true);
        }
    }

    /// Folder `path` with all the slots of its last cluster used but two (deleted entries, as many deletes leave):
    /// no room for one more name without a new cluster.
    fn crowd(img: &mut [u8], path: &str) {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, path).unwrap();
        let last = fat.chain(&mut d, dir.cluster).unwrap().last;
        let (lba, slots) = (fat.lba(last) as usize * SECTOR, fat.cluster_bytes() as usize / 32);
        let end = (0..slots).find(|k| img[lba + k * 32] == 0).unwrap();
        for k in end..slots - 2 {
            img[lba + k * 32] = 0xe5;
            img[lba + k * 32 + 11] = 0x20;
        }
    }

    /// The clusters of folder `path`'s chain.
    fn dir_clusters(img: &[u8], path: &str) -> u32 {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, path).unwrap();
        fat.chain(&mut d, dir.cluster).unwrap().clusters
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

    /// The clean copy moved already under the record's suffix, then the full copy's name taken there.
    fn moved_then_taken() -> Vec<u8> {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, TO[0]).unwrap();
        let h = Handle { first: ride.cluster, size: ride.size, ..Handle::default() };
        fat.link(&mut d, del, "261020080000.fit", &h, T).unwrap(); // as the finish's first half
        let full = fat.open(&mut d, TO[1]).unwrap();
        fat.create(&mut d, full, "261020080000.fit", T).unwrap();
        d.0
    }

    /// The review of 2026-11-01: the clean copy moved already under the record's suffix, the full copy's name then
    /// taken there: no later suffix for both (the clean chain linked a second time), the move put back instead,
    /// the moved copy too (it stayed in `\Rustfw\Deleted`, the full copy where it was: a delete half done).
    #[test]
    fn a_file_moved_already_keeps_its_suffix() {
        let mut d = Ram(moved_then_taken(), 0);
        assert_eq!(finish(&mut d, T), Ok(false));
        assert_eq!(fsck(&d.0), Ok(0), "no chain named twice");
        assert!(file(&d.0, RIDE).is_some_and(|f| !f.is_empty()), "the clean copy back");
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_none());
        assert!(file(&d.0, "Rustfw/Deleted/261020080000-1.fit").is_none());
        assert!(file(&d.0, "Rustfw/Activities/261020080000.fit").is_some(), "the full copy where it was");
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
    }

    /// The review of 2026-11-01 (revive, 1): a copy moved already into `\Rustfw\Deleted` whose first sector a
    /// computer changed, in a move put back: not the ride's chain any more, so it is not moved back, and it keeps
    /// its name in `\Rustfw\Deleted` at every cut (it was removed first, then left named nowhere).
    #[test]
    fn a_changed_copy_in_deleted_keeps_its_name() {
        let ride = cluster(&volume(), RIDE);
        let mut img = moved_then_taken();
        let mut d = Ram(img.clone(), 0);
        let fat = Fat::mount(&mut d).unwrap();
        let at = fat.lba(ride.cluster) as usize * SECTOR;
        img[at..at + 16].fill(0x42);
        no_room(&mut img, true);
        let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
        assert_eq!(finish(&mut whole, T), Ok(false));
        let writes = u32::MAX - whole.1;
        for budget in 0..=writes {
            let mut d = Cut(Ram(img.clone(), 0), budget);
            let _ = finish(&mut d, T);
            d.1 = u32::MAX;
            let _ = finish(&mut d, T);
            let at = std::format!("cut after {budget} of {writes}");
            assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{at}: {e}")), Ok(0));
            assert!(file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some(), "{at}: still named");
            assert_eq!(finish(&mut d, T), Ok(false), "{at}: nothing pending");
        }
    }

    /// The re-review of beaa07a: a put back cut between its link back and the removal of the copy's name in
    /// `\Rustfw\Deleted`, then room for the move at the next start: the move finishes and the name put back goes,
    /// so the chain keeps one name (it kept both, the record done: a delete of either freed it under the other).
    #[test]
    fn a_cut_put_back_then_a_move_leaves_one_name() {
        let mut img = moved_then_taken();
        no_room(&mut img, true);
        let two = |d: &Cut| file(&d.0.0, RIDE).is_some() && file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some();
        let mut d = (0..20)
            .map(|budget| {
                let mut d = Cut(Ram(img.clone(), 0), budget);
                let _ = finish(&mut d, T);
                d
            })
            .find(two)
            .expect("a cut after the link back, before the removal");
        d.1 = u32::MAX;
        let mut fat = Fat::mount(&mut d).unwrap();
        let full = fat.open(&mut d, TO[1]).unwrap();
        let other = fat.locate(&mut d, full, "261020080000.fit").unwrap();
        fat.remove(&mut d, &other).unwrap(); // the name in the way gone: the move can finish
        assert_eq!(finish(&mut d, T), Ok(true));
        // the file removed by hand was made empty: no cluster (`Fat::create`), none lost
        assert_eq!(fsck(&d.0.0), Ok(0), "one name on the chain");
        assert!(file(&d.0.0, RIDE).is_none() && file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some());
        assert!(file(&d.0.0, "Rustfw/Deleted/Full/261020080000.fit").is_some_and(|f| !f.is_empty()));
    }

    /// The final review of the follow-up: a name an older build's put back gave in `\Activities` (`<name>-1.fit`,
    /// the plain name then taken), the cut leaving the copy in `\Rustfw\Deleted` too, and the file in the way
    /// deleted from outside meanwhile (stock's USB): the next start finds the chain's name whatever its suffix and
    /// never links a second (it looked name by name and stopped at the free plain one). Through the put back (the
    /// card full) and through the move (room).
    #[test]
    fn a_suffixed_name_left_behind_is_found() {
        let ride = cluster(&volume(), RIDE);
        for full in [true, false] {
            let mut d = Ram(moved_then_taken(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let acts = fat.open(&mut d, ACTIVITIES).unwrap();
            let h = Handle { first: ride.cluster, size: ride.size, ..Handle::default() };
            fat.link(&mut d, acts, "261020080000-1.fit", &h, T).unwrap();
            if full {
                no_room(&mut d.0, true);
            } else {
                let full = fat.open(&mut d, TO[1]).unwrap();
                let other = fat.locate(&mut d, full, "261020080000.fit").unwrap();
                fat.remove(&mut d, &other).unwrap(); // the move can finish (an empty file: no cluster lost)
            }
            assert_eq!(finish(&mut d, T), Ok(!full), "full {full}");
            assert_eq!(fsck(&d.0), Ok(0), "full {full}: one name on the chain");
            let names = ["Activities/261020080000-1.fit", RIDE, "Rustfw/Deleted/261020080000.fit"];
            assert_eq!(names.iter().filter(|p| file(&d.0, p).is_some()).count(), 1, "full {full}");
            assert_eq!(finish(&mut d, T), Ok(false), "full {full}: nothing pending");
        }
    }

    /// The final review of the follow-up: a put back whose old name in `\Activities` a computer gave to another file
    /// meanwhile: the copy moved stays in `\Rustfw\Deleted`, named, the record done; never a `<name>-1.fit` in
    /// `\Activities`, which neither firmware's history lists.
    #[test]
    fn a_put_back_never_invents_a_suffix() {
        let mut d = Ram(moved_then_taken(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        fat.create(&mut d, acts, "261020080000.fit", T).unwrap();
        no_room(&mut d.0, true);
        assert_eq!(finish(&mut d, T), Ok(false));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some_and(|f| !f.is_empty()), "where it is named");
        assert!(file(&d.0, "Activities/261020080000-1.fit").is_none());
        assert_eq!(file(&d.0, RIDE), Some(Vec::new()), "the other file");
        assert_eq!(fsck(&d.0), Ok(0));
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
    }

    /// A chain named by no entry whose old name in `\Activities` another file took meanwhile, on a full card: put
    /// back under the first free `<name>-1.fit` (it stayed pending for good, the drive read only).
    #[test]
    fn a_nameless_chain_whose_name_is_taken_gets_a_suffix() {
        let clean = file(&volume(), RIDE).unwrap();
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        fat.create(&mut d, acts, "261020080000.fit", T).unwrap();
        no_room(&mut d.0, true);
        assert_eq!(finish(&mut d, T), Ok(false));
        assert!(file(&d.0, "Activities/261020080000-1.fit") == Some(clean), "named");
        assert_eq!(fsck(&d.0), Ok(0));
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
    }

    /// A name on the chain the listing gives with a '?' (a character not ASCII: `261020080000-\u{e9}.fit`, made by a
    /// computer) cannot be looked up: left as it is, the copy's name in `\Rustfw\Deleted` removed (one name on the
    /// chain), the full copy left with it, and the record done (every start failed on it).
    #[test]
    fn a_name_the_listing_cannot_give_back_does_not_stop_the_move() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(moved_then_taken(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        let h = Handle { first: ride.cluster, size: ride.size, ..Handle::default() };
        fat.link(&mut d, acts, "261020080000-x.fit", &h, T).unwrap();
        let full = fat.open(&mut d, TO[1]).unwrap();
        let other = fat.locate(&mut d, full, "261020080000.fit").unwrap();
        fat.remove(&mut d, &other).unwrap(); // the move can finish
        // the 'x' (the second piece's first unit: the first holds "261020080000-") made an e acute
        let x: &[u8] = &[0x78, 0, b'.', 0, b'f', 0, b'i', 0];
        let at = d.0.windows(x.len()).position(|w| w == x).unwrap();
        d.0[at] = 0xe9;
        assert_eq!(finish(&mut d, T), Ok(false), "left where the computer named it");
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
        assert_eq!(fsck(&d.0), Ok(0), "one name on the chain");
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_none());
        assert!(file(&d.0, "Rustfw/Activities/261020080000.fit").is_some_and(|f| !f.is_empty()), "the full copy too");
    }

    /// The re-review of beaa07a: a copy in `\Rustfw\Deleted` a computer made longer, put back: linked with its own
    /// size, not the record's (its last bytes cut off).
    #[test]
    fn a_longer_copy_is_put_back_whole() {
        let mut img = moved_then_taken();
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let del = fat.open(&mut d, TO[0]).unwrap();
        let mut h = fat.locate(&mut d, del, "261020080000.fit").unwrap();
        let longer = h.size + 10;
        assert!(fat.clusters_for(longer) <= h.clusters, "within its chain");
        fat.set_size(&mut d, &mut h, longer, T).unwrap();
        img = d.0;
        no_room(&mut img, true);
        let mut d = Ram(img, 0);
        assert_eq!(finish(&mut d, T), Ok(false));
        assert_eq!(file(&d.0, RIDE).map(|f| f.len()), Some(longer as usize));
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// The re-review of beaa07a: an unrelated `\FOUND.000` whose chain loops (damaged) names nothing: the move
    /// a cut left still finishes (the error kept it pending at every start, then the card was lent).
    #[test]
    fn a_damaged_found_folder_does_not_stop_the_move() {
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let found = fat.mkdir(&mut d, fat.root(), "FOUND.000", T).unwrap();
        let u = |img: &[u8], at: usize, n: usize| {
            img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b))
        };
        let (fat0, fat_len) = (u(&d.0, 14, 2) * SECTOR, u(&d.0, 36, 4) * SECTOR);
        for copy in [0, fat_len] {
            let at = fat0 + copy + found.cluster as usize * 4;
            d.0[at..at + 4].copy_from_slice(&found.cluster.to_le_bytes()); // its chain loops on itself
        }
        assert_eq!(finish(&mut d, T), Ok(true));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some(), "moved");
    }

    /// The review of 2026-11-01 (revive, 3): a chain the cut left nameless that a computer's repair named meanwhile
    /// (`\FOUND.000\FILE0000.CHK`, chkdsk /f), its bytes the ride's: neither linked nor made live again (the two names
    /// would share the chain), on a card with room (the move) and on a full one (the put back).
    #[test]
    fn a_chain_a_repair_named_is_not_named_again() {
        let ride = cluster(&volume(), RIDE);
        for (full, path) in [false, true].into_iter().flat_map(|f| [(f, "FOUND.000/FILE0000.CHK"), (f, "FILE0000.CHK")])
        {
            let mut d = Ram(cut_midway(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let dir = match path.split_once('/') {
                Some((dir, _)) => fat.mkdir(&mut d, fat.root(), dir, T).unwrap(),
                None => fat.root(), // Windows 9x's, in the root
            };
            let h = Handle { first: ride.cluster, size: ride.size, ..Handle::default() };
            fat.link(&mut d, dir, "FILE0000.CHK", &h, T).unwrap();
            if full {
                no_room(&mut d.0, true);
                crowd(&mut d.0, ACTIVITIES);
            }
            assert!(finish(&mut d, T).is_ok(), "full {full}, {path}");
            assert_eq!(fsck(&d.0), Ok(0), "full {full}, {path}: one name on the chain");
            assert!(file(&d.0, path).is_some());
            assert!(file(&d.0, RIDE).is_none() && file(&d.0, "Rustfw/Deleted/261020080000.fit").is_none());
            assert_eq!(finish(&mut d, T), Ok(false), "full {full}: nothing pending");
        }
    }

    /// The review of 2026-11-01: a move that cannot finish on a card so full that `\Activities` cannot take the name
    /// either: the ride's old entry made live again (no cluster), under its long name, the record done; with that
    /// name given to another file meanwhile, nothing can be done: a card error (`Io`, never `Refused`, which says
    /// nothing is pending), the record still pending.
    #[test]
    fn a_ride_that_cannot_be_linked_back_gets_its_entry_back() {
        let clean = file(&volume(), RIDE).unwrap();
        let mut img = cut_midway();
        no_room(&mut img, true);
        crowd(&mut img, ACTIVITIES);
        let mut d = Ram(img.clone(), 0);
        assert_eq!(finish(&mut d, T), Ok(false), "put back");
        assert!(file(&d.0, RIDE) == Some(clean), "by its long name");
        assert_eq!(fsck(&d.0), Ok(0));
        assert_eq!(finish(&mut d, T), Ok(false), "nothing pending");
        // the name taken (made before the card filled: it takes a cluster)
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        fat.create(&mut d, acts, "261020080000.fit", T).unwrap();
        no_room(&mut d.0, true);
        crowd(&mut d.0, ACTIVITIES);
        assert_eq!(finish(&mut d, T), Err(Error::Io));
        assert_eq!(finish(&mut d, T), Err(Error::Io), "still pending");
        assert!(fsck(&d.0).is_ok_and(|lost| lost > 0), "the ride's chain named only by the record");
    }

    /// The review of 2026-11-01: a put back cut at every sector it writes (a link back, a moved copy taken out
    /// first, an entry made live again), then the next start: each copy in exactly one place, the volume whole,
    /// the record done.
    #[test]
    fn a_put_back_survives_a_cut_at_every_sector() {
        let full_card = |mut img: Vec<u8>, crowded: bool| {
            no_room(&mut img, true);
            if crowded {
                crowd(&mut img, ACTIVITIES);
            }
            img
        };
        let states = [
            ("linked back", full_card(cut_midway(), false)),
            ("moved copy back", full_card(moved_then_taken(), false)),
            ("entry back", full_card(cut_midway(), true)),
        ];
        let count = |img: &[u8], paths: &[&str]| paths.iter().filter(|p| file(img, p).is_some()).count();
        let clean = [RIDE, "Rustfw/Deleted/261020080000.fit", "Rustfw/Deleted/261020080000-1.fit"];
        let full = ["Rustfw/Activities/261020080000.fit", "Rustfw/Deleted/Full/261020080000-1.fit"];
        for (what, img) in states {
            let st = stock(&img); // stock's files but the ride put back
            let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
            assert_eq!(finish(&mut whole, T), Ok(false), "{what}");
            let writes = u32::MAX - whole.1;
            assert!(writes >= 2, "{what}: {writes} writes");
            for budget in 0..writes {
                let mut d = Cut(Ram(img.clone(), 0), budget);
                assert_eq!(finish(&mut d, T), Err(Error::Io), "{what}, cut after {budget}");
                d.1 = u32::MAX;
                assert_eq!(finish(&mut d, T), Ok(false), "{what}, cut after {budget}, then the start");
                let at = std::format!("{what}, cut after {budget} of {writes}");
                assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{at}: {e}")), Ok(0));
                assert_eq!((count(&d.0.0, &clean), count(&d.0.0, &full)), (1, 1), "{at}");
                assert!(file(&d.0.0, RIDE).is_some(), "{at}: put back");
                stock_kept(&st, &d.0.0, &[], &[RIDE]).unwrap_or_else(|e| panic!("{at}: {e}"));
                assert_eq!(finish(&mut d, T), Ok(false), "{at}: nothing pending");
            }
        }
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
            let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
            run(&mut whole).unwrap();
            let writes = u32::MAX - whole.1;
            for budget in 0..=writes {
                let mut d = Cut(Ram(img.clone(), 0), budget);
                let _ = run(&mut d);
                d.1 = u32::MAX;
                finish(&mut d, T).unwrap();
                if file(&d.0.0, RIDE).is_some() {
                    delete(&mut d, &ride, T).unwrap(); // cut before its record: asked again
                }
                let at = std::format!("{what}, cut after {budget} of {writes}");
                assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{at}: {e}")), Ok(0));
                assert!(file(&d.0.0, "Rustfw/Deleted/261020080000-1.fit") == Some(clean.clone()), "{at}: clean");
                assert!(file(&d.0.0, "Rustfw/Deleted/Full/261020080000-1.fit") == Some(full.clone()), "{at}: full");
                assert_eq!(
                    file(&d.0.0, "Rustfw/Deleted/Full/261020080000.fit"),
                    Some(Vec::new()),
                    "{at}: the other file"
                );
                for p in [RIDE, "Rustfw/Activities/261020080000.fit", "Rustfw/Deleted/261020080000.fit"] {
                    assert!(file(&d.0.0, p).is_none(), "{at}: {p}");
                }
            }
        }
    }

    /// The record round trip: each file's CRC, or none (a flag-1 slot of the first builds, or an empty file); a
    /// sector torn, or sealed under another tag, is no record.
    #[test]
    fn the_record_reads_back_what_was_written() {
        let r = Rec {
            pending: true,
            name: *b"261020080000",
            suffix: 7,
            files: [Some((5, 900, None)), Some((9, 4, Some(3)))],
        };
        let s = r.to_sector();
        assert_eq!(Rec::from_sector(&s), Some(r));
        let mut old = s;
        old[32] = 1; // a flag-1 slot: its CRC bytes ignored
        old[32 + 12] = 1;
        crate::storage::seal16(&mut old);
        assert_eq!(Rec::from_sector(&old).map(|r| r.files[0]), Some(Some((5, 900, None))));
        let mut torn = s;
        torn[100] ^= 1;
        assert_eq!(Rec::from_sector(&torn), None, "torn");
        let mut other = s;
        other[0] = b'X';
        crate::storage::seal16(&mut other);
        assert_eq!(Rec::from_sector(&other), None, "another tag");
    }

    /// The volume of [`volume`] after a delete cut right after its record: the move pending, the ride still named
    /// in `\Activities`.
    pub(crate) fn pending_named() -> Vec<u8> {
        let img = volume();
        let ride = cluster(&img, RIDE);
        let pending = |img: &[u8]| {
            let s = file(img, "Rustfw/delete.rec").unwrap_or_default();
            s.get(..SECTOR).and_then(|s| Rec::from_sector(s.try_into().ok()?)).is_some_and(|r| r.pending)
        };
        (0..200)
            .map(|budget| {
                let mut d = Cut(Ram(img.clone(), 0), budget);
                let _ = delete(&mut d, &ride, T);
                d.0.0
            })
            .find(|img| file(img, RIDE).is_some() && pending(img))
            .unwrap()
    }

    /// A ride still named when the move is finished is moved whatever a computer wrote into it meanwhile (its
    /// first sector's CRC is checked only for a chain named by no entry): it was named nowhere.
    #[test]
    fn a_named_ride_a_computer_changed_is_still_moved() {
        let mut img = pending_named();
        let at = cluster(&img, RIDE).cluster;
        let at = Fat::mount(&mut Ram(img.clone(), 0)).unwrap().lba(at) as usize * SECTOR;
        img[at..at + 16].fill(0x42);
        let changed = file(&img, RIDE).unwrap();
        let mut d = Ram(img, 0);
        assert_eq!(finish(&mut d, T), Ok(true));
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000.fit"), Some(changed));
        assert_eq!(file(&d.0, RIDE), None);
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A chain the cut left nameless that a computer's fsck cut short (its first cluster now the last): not the
    /// ride's whole any more, so it is not linked with the ride's size (a size past its chain), whatever its first
    /// sector.
    #[test]
    fn a_nameless_chain_cut_short_is_not_linked() {
        let ride = cluster(&volume(), RIDE);
        let mut d = Ram(cut_midway(), 0);
        let fat = Fat::mount(&mut d).unwrap();
        assert!(ride.size > fat.cluster_bytes(), "more than one cluster");
        let (fat0, fat_len) = fats(&d.0);
        for copy in [0, fat_len] {
            let at = fat0 + copy + ride.cluster as usize * 4;
            d.0[at..at + 4].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
        }
        assert_eq!(finish(&mut d, T), Ok(true));
        assert_eq!(file(&d.0, "Rustfw/Deleted/261020080000.fit"), None, "not the ride whole");
        assert!(fsck(&d.0).is_ok(), "{:?}", fsck(&d.0));
    }

    /// The first FAT's offset and one FAT's length in bytes, from the boot sector.
    fn fats(img: &[u8]) -> (usize, usize) {
        let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b));
        (u(14, 2) * SECTOR, u(36, 4) * SECTOR)
    }

    /// The second FAT made different from the first in its last sector, as a cut while it was written leaves it.
    fn fats_differ(img: &mut [u8]) {
        let (fat0, fat_len) = fats(img);
        img[fat0 + 2 * fat_len - SECTOR] ^= 1;
        assert!(fsck(img).is_err_and(|e| e.contains("FATs differ")));
    }

    /// A delete that makes a folder or the record's file (no record to finish after a cut there) first makes the
    /// FATs equal, and only then: with everything made, it never reads the FATs whole (the difference is still
    /// there after it).
    #[test]
    fn the_fats_are_made_equal_only_when_something_is_made() {
        let made = |img: Vec<u8>, full: bool| {
            let mut d = Ram(img, 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            if full {
                to_dir(&mut fat, &mut d, 1, T).unwrap();
            }
            rec_file(&mut fat, &mut d, T).unwrap();
            d.0
        };
        // nothing made; the record's file only (no full copy, no \Rustfw\Deleted); everything
        for (what, img, equal) in [
            ("nothing", volume(), true),
            ("the record", made(crate::fat::tests::image(), false), true),
            ("everything", made(volume(), true), false),
        ] {
            let ride = cluster(&img, RIDE);
            let mut d = Ram(img, 0);
            fats_differ(&mut d.0);
            delete(&mut d, &ride, T).unwrap();
            assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some(), "{what}");
            let fsck = fsck(&d.0);
            assert_eq!(fsck.is_ok(), equal, "{what}: {fsck:?}");
        }
    }

    /// A move that cannot finish (the card full) whose put back finds every name of the ride's taken in
    /// `\Activities` (the plain one and `-1` to `-99`): a card error, the move left pending, no 100th name made up.
    #[test]
    fn a_put_back_with_every_suffix_taken_stays_pending() {
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        let mut buf = [0u8; 20];
        for j in 0..=SUFFIXES {
            let name = file_name(b"261020080000", j, &mut buf).unwrap();
            fat.create(&mut d, acts, name, T).unwrap();
        }
        no_room(&mut d.0, true);
        assert_eq!(finish(&mut d, T), Err(Error::Io));
        assert_eq!(finish(&mut d, T), Err(Error::Io), "still pending");
        assert_eq!(file(&d.0, "Activities/261020080000-00.fit"), None);
    }

    /// A move that cannot finish (the card full) whose put back finds `\Activities` gone (a computer removed it):
    /// the error says so, never `Refused` (nothing pending), the move left pending.
    #[test]
    fn a_put_back_without_its_folder_says_why() {
        let mut d = Ram(cut_midway(), 0);
        let fat = Fat::mount(&mut d).unwrap();
        let root = fat.lba(fat.root().cluster) as usize * SECTOR;
        let slot = (0..fat.cluster_bytes() as usize / 32)
            .map(|k| root + 32 * k)
            .find(|&at| d.0[at..at + 6] == *b"ACTIVI" && d.0[at + 11] & 0x10 != 0)
            .unwrap();
        d.0[slot] = 0xe5;
        no_room(&mut d.0, true);
        assert_eq!(finish(&mut d, T), Err(Error::NotFound));
        assert_eq!(finish(&mut d, T), Err(Error::NotFound), "still pending");
    }

    /// An unrelated file in a repair's folder (`\FOUND.000\FILE0000.CHK` of another chain) does not stop the move.
    #[test]
    fn an_unrelated_repair_file_does_not_stop_the_move() {
        let mut d = Ram(cut_midway(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let found = fat.mkdir(&mut d, fat.root(), "FOUND.000", T).unwrap();
        fat.create(&mut d, found, "FILE0000.CHK", T).unwrap();
        assert_eq!(finish(&mut d, T), Ok(true));
        assert!(file(&d.0, "Rustfw/Deleted/261020080000.fit").is_some(), "moved");
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A folder a damage left with two entries of the ride's name, another file's first: the move never removes
    /// that one (the name looked up finds it, not the ride's), and never links the ride's chain, still named, a
    /// second time.
    #[test]
    fn another_file_of_the_rides_name_keeps_its_entry() {
        const OTHER: &str = "Activities/261001070000.fit";
        let mut img = pending_named();
        let other = file(&img, OTHER).unwrap();
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        let ride = fat.locate(&mut d, acts, "261020080000.fit").unwrap();
        let next = fat.locate(&mut d, acts, "261001070000.fit").unwrap();
        let at = |h: &Handle| h.entry_lba as usize * SECTOR + usize::from(h.slot) * 32;
        let (a, b) = (at(&ride), at(&next));
        assert!(a < b, "the ride's entry first");
        // the chains swapped: the first entry of the ride's name on the other file's chain, the second (its long
        // name rewritten: the 8.3 entry's checksum holds) on the ride's
        for k in [20, 21, 26, 27, 28, 29, 30, 31] {
            img.swap(a + k, b + k);
        }
        let lfn = b - 32;
        assert_eq!((img[lfn] & 0x1f, img[lfn + 11]), (1, 0x0f), "the long name's first piece");
        for (&c, at) in b"261020080000".iter().zip([1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28]) {
            img[lfn + at] = c;
        }
        assert_eq!(fsck(&img), Ok(0));
        let mut d = Ram(img, 0);
        assert!(finish(&mut d, T).is_ok());
        assert_eq!(file(&d.0, RIDE), Some(other), "the other file still named");
        assert_eq!(fsck(&d.0), Ok(0), "one name on each chain");
    }

    /// A ride a computer renamed in `\Activities` while its move was pending (a cut, then the card lent): still
    /// named, so never linked a second time, on a card with room (the move) and on a full one (the put back).
    #[test]
    fn a_ride_renamed_meanwhile_keeps_one_name() {
        for full in [false, true] {
            let mut d = Ram(pending_named(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let acts = fat.open(&mut d, ACTIVITIES).unwrap();
            let h = fat.locate(&mut d, acts, "261020080000.fit").unwrap();
            fat.link(&mut d, acts, "my ride.fit", &h, T).unwrap();
            fat.remove(&mut d, &h).unwrap();
            if full {
                no_room(&mut d.0, true);
            }
            assert!(finish(&mut d, T).is_ok(), "full {full}");
            assert_eq!(fsck(&d.0), Ok(0), "full {full}: one name on the chain");
            assert!(file(&d.0, "Activities/my ride.fit").is_some(), "full {full}");
            assert_eq!(finish(&mut d, T), Ok(false), "full {full}: nothing pending");
        }
    }

    /// [`finish`] on `img` cut after every sector it writes, then the next start's: the volume whole, `check` on
    /// it, nothing left pending.
    fn finish_cut_at_every_sector(img: &[u8], check: impl Fn(&[u8], &str)) {
        let mut whole = Cut(Ram(img.to_vec(), 0), u32::MAX);
        finish(&mut whole, T).unwrap();
        let writes = u32::MAX - whole.1;
        for budget in 0..=writes {
            let mut d = Cut(Ram(img.to_vec(), 0), budget);
            let _ = finish(&mut d, T);
            d.1 = u32::MAX;
            finish(&mut d, T).unwrap();
            let at = std::format!("cut after {budget} of {writes}");
            assert_eq!(fsck(&d.0.0).map_err(|e| std::format!("{at}: {e}")), Ok(0));
            check(&d.0.0, &at);
            assert_eq!(finish(&mut d, T), Ok(false), "{at}: nothing pending");
        }
    }

    /// The Opus review of trash.rs, 1: a put back cut between its link back into `\Activities` and the removal of
    /// the copy's name in `\Rustfw\Deleted`, then a computer renaming the ride there, then a start with room: the
    /// name in `\Rustfw\Deleted` goes, the computer's stays (both stayed on one chain: `unlink_old` looks for the
    /// ride's own name only), the full copy where it was.
    #[test]
    fn a_cut_put_back_then_a_rename_leaves_one_name() {
        let mut img = moved_then_taken();
        no_room(&mut img, true);
        let two = |d: &Cut| file(&d.0.0, RIDE).is_some() && file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some();
        let mut d = (0..20)
            .map(|budget| {
                let mut d = Cut(Ram(img.clone(), 0), budget);
                let _ = finish(&mut d, T);
                d
            })
            .find(two)
            .expect("a cut after the link back, before the removal");
        d.1 = u32::MAX;
        let mut fat = Fat::mount(&mut d).unwrap();
        let acts = fat.open(&mut d, ACTIVITIES).unwrap();
        let h = fat.locate(&mut d, acts, "261020080000.fit").unwrap();
        fat.link(&mut d, acts, "my ride.fit", &h, T).unwrap(); // renamed by a computer
        fat.remove(&mut d, &h).unwrap();
        let full = fat.open(&mut d, TO[1]).unwrap();
        let other = fat.locate(&mut d, full, "261020080000.fit").unwrap();
        fat.remove(&mut d, &other).unwrap(); // the name in the way gone: room for the move
        let img = d.0.0;
        assert_eq!(finish(&mut Ram(img.clone(), 0), T), Ok(false), "not deleted");
        finish_cut_at_every_sector(&img, |img, at| {
            let names = [RIDE, "Activities/my ride.fit", "Rustfw/Deleted/261020080000.fit"];
            let named: Vec<_> = names.iter().filter(|p| file(img, p).is_some()).collect();
            assert_eq!(named, [&"Activities/my ride.fit"], "{at}: exactly one name");
            assert!(file(img, "Rustfw/Activities/261020080000.fit").is_some(), "{at}: the full copy where it was");
            assert!(file(img, "Rustfw/Deleted/Full/261020080000.fit").is_none(), "{at}");
        });
    }

    /// The Opus review of trash.rs, 2: a ride a computer moved into another folder (`\Backup`) while its delete was
    /// pending: still named, so never linked into `\Rustfw\Deleted` (two names on one chain), on a card with room
    /// (the move) and on a full one (the put back); the full copy stays with it; a delete of it then is
    /// `NotFound`, the record done.
    #[test]
    fn a_ride_moved_to_another_folder_keeps_one_name() {
        let ride = cluster(&volume(), RIDE);
        for full in [false, true] {
            let mut d = Ram(pending_named(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let backup = fat.mkdir(&mut d, fat.root(), "Backup", T).unwrap();
            let acts = fat.open(&mut d, ACTIVITIES).unwrap();
            let h = fat.locate(&mut d, acts, "261020080000.fit").unwrap();
            fat.link(&mut d, backup, "261020080000.fit", &h, T).unwrap();
            fat.remove(&mut d, &h).unwrap();
            if full {
                no_room(&mut d.0, true);
            }
            assert_eq!(finish(&mut Ram(d.0.clone(), 0), T), Ok(false), "full {full}: not deleted");
            finish_cut_at_every_sector(&d.0, |img, at| {
                assert!(file(img, "Backup/261020080000.fit").is_some(), "full {full}, {at}: where the computer put it");
                for p in [RIDE, "Rustfw/Deleted/261020080000.fit", "Rustfw/Deleted/Full/261020080000.fit"] {
                    assert!(file(img, p).is_none(), "full {full}, {at}: {p}");
                }
                assert!(file(img, "Rustfw/Activities/261020080000.fit").is_some(), "full {full}, {at}: the full copy");
            });
            assert_eq!(delete(&mut d, &ride, T), Err(Error::NotFound), "full {full}");
            assert_eq!(fsck(&d.0), Ok(0), "full {full}");
            assert_eq!(finish(&mut d, T), Ok(false), "full {full}: the record done");
            assert!(file(&d.0, "Backup/261020080000.fit").is_some(), "full {full}");
        }
    }

    /// An I/O error at every read of a delete, then the next start: each copy in exactly one place, the volume
    /// whole (an error swallowed while the ride's old entry was looked up linked its chain a second time).
    #[test]
    fn a_read_error_at_every_read() {
        use crate::fat::Disk;
        /// A disk whose read number `.1` fails, once.
        struct Bad(Ram, u32);
        impl Disk for Bad {
            fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
                if self.0.1 == self.1 {
                    self.0.1 += 1;
                    return Err(Error::Io);
                }
                self.0.read(lba, buf)
            }
        }
        impl DiskWrite for Bad {
            fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
                self.0.write(lba, buf)
            }
        }
        let img = volume();
        let ride = cluster(&img, RIDE);
        let mut whole = Ram(img.clone(), 0);
        delete(&mut whole, &ride, T).unwrap();
        let reads = whole.1;
        let one = |img: &[u8], paths: &[&str]| paths.iter().filter(|p| file(img, p).is_some()).count();
        for n in 0..reads {
            let mut d = Bad(Ram(img.clone(), 0), n);
            let r = delete(&mut d, &ride, T);
            assert!(r.is_ok() || r == Err(Error::Io), "read {n}: {r:?}");
            if r.is_ok() {
                assert!(file(&d.0.0, RIDE).is_none(), "read {n}: Ok, the ride moved"); // an error swallowed left it
            }
            d.1 = u32::MAX;
            finish(&mut d, T).unwrap();
            if file(&d.0.0, RIDE).is_some() {
                delete(&mut d, &ride, T).unwrap();
            }
            let at = std::format!("read {n} of {reads}");
            let lost = fsck(&d.0.0).unwrap_or_else(|e| panic!("{at}: {e}"));
            assert!(lost <= 1, "{at}: {lost} lost");
            assert_eq!(one(&d.0.0, &[RIDE, "Rustfw/Deleted/261020080000.fit"]), 1, "{at}");
            assert!(file(&d.0.0, "Rustfw/Deleted/261020080000.fit").is_some(), "{at}: deleted");
        }
    }
}
