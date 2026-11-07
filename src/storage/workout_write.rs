//! A workout the phone sends, written into stock's calendar (`\Workout\calendar\<YYYYMMDD>\`), and a listed one
//! deleted: the file service's only writes (link/filesvc.rs, WRITE_BEGIN / WRITE_DATA / WRITE_END / DELETE; the
//! app's side in app/filesvc.rs). Pure FAT work over [`DiskWrite`], with the FAT writer's own write order
//! (fat/write.rs) under it.
//!
//! **Where the bytes go first:** `\Rustfw\workout.tmp`, this firmware's own folder (AGENTS.md, "Share the storage
//! with stock": stock never reads it), sized at [`begin`] (the chain taken, then the size set), then each 512-byte
//! sector written in place as the data comes ([`put`]): no FAT or directory write during the transfer, so a cut there
//! leaves a whole, named temporary file and nothing else. It is not removed after a cut: the next [`begin`] writes
//! over it (and frees what a longer one had past the new size). Removing it would free a chain, which goes through a
//! moment where no entry names it (a lost chain after a cut); kept, it costs one cluster of the own folder.
//!
//! **The commit** ([`commit`]), in this order:
//! 1. the temporary file read back whole from the card, its size and CRC-32 compared with what the phone gave, and
//!    parsed as a workout ([`workout_file::parse`]): anything else is refused, nothing written;
//! 2. the day folder made in `\Workout\calendar\` if missing (never `\Workout` or `\Workout\calendar` themselves: a
//!    card without them gets NOT FOUND; stock makes them when it syncs), the name looked up (taken by the same size:
//!    the phone sent it again after a lost answer, done), and room made for its entry ([`Fat::make_room`]);
//! 3. the temporary file's entry marked deleted ([`Fat::remove`]);
//! 4. the same chain linked into the day folder under its new name, whole, with its size ([`Fat::link`]: written
//!    deleted, then made live by its first bytes).
//!
//! **A power cut or a reset** at any point leaves the calendar as it was or with the whole workout, never a part of
//! one: before step 3 the calendar holds nothing new (at most an empty day folder, which stock and this firmware
//! list as no workouts); between 3 and 4 the chain is named by no entry, a lost chain of one cluster on the unit
//! (8 KB at most, a 16 KB cluster) that a computer's fsck reports and turns into a `.CHK`, as a screenshot cut does
//! (storage/screenshot.rs); after 4 the workout is there whole. The FAT writer's own steps that take a cluster (the
//! temporary file's first, at [`begin`], before its size names it; a new day folder's, before its entry) leave one
//! lost cluster the same way when cut inside. No cluster is ever named twice, none is freed while an entry may name
//! it, and the phone learns of a cut by the END that never came, and sends again (written over the temporary file,
//! nothing more lost: the tests cut after every sector, `a_cut_at_every_write...` here and in src/sim/filesvc_tests.rs).
//!
//! Why no recovery record (as `trash.rs` keeps for a ride's move): the workout is not a ride, the phone still holds it,
//! and a record would have to tell a lost chain from one a computer's fsck named meanwhile (trash.rs `named_on_top`);
//! one lost cluster after a cut in a two-write window costs less than that code.
//!
//! **Names:** `<CRC-32 as 8 hex digits>.fit` (12 characters, a valid 8.3 name too), so the same file sent twice gets
//! the same name and two different files in a day almost never share one (a clash answers BAD REQUEST). Stock names
//! its own after the name the phone gave it (`\workout\calendar\%s%s%s\%s.fit`, ghidra Workout_ImportCalendarJson) and
//! lists a day folder's files by a pattern (Workout_ScanTodayCalendarFiles; `*.fit`, inferred), cutting the extension
//! for the list: any `.fit` name is one it reads.
//!
//! **Delete** ([`delete`]): the entry marked deleted (never zeroed: golden rule), then its chain freed, which no live
//! entry names any more. A cut between leaves a lost chain. Stock's `.json` beside its own workouts is left (stock's
//! file, which neither list reads).

use crate::fat::write::Handle;
use crate::fat::{DiskWrite, Entry, Error, Fat, File, SECTOR};
use crate::filesvc::Status;
use crate::recorder::{DIR, FULL_RESERVE};
use crate::storage::workout_file;

/// The temporary file, in `\Rustfw`.
pub const TMP: &str = "workout.tmp";
/// Stock's calendar, which holds a folder a day.
pub const CALENDAR: &str = "Workout/calendar";

/// A card error as the file service answers it: no room is FULL, the rest a card error.
fn card(e: Error) -> Status {
    if e == Error::Refused { Status::Full } else { Status::Io }
}

/// The temporary file made (or the one left kept) and sized for `size` bytes, the card's room checked first (the file
/// and the recorder's [`FULL_RESERVE`], FSInfo's count; unknown counts as none): its handle for [`put`].
pub fn begin(fat: &mut Fat, d: &mut impl DiskWrite, size: u32, t: (u16, u16)) -> Result<Handle, Status> {
    if size == 0 || size as usize > workout_file::FILE_MAX {
        return Err(Status::BadRequest);
    }
    let free = fat.free_clusters(d).map_err(card)?.map_or(0, u64::from) * u64::from(fat.cluster_bytes());
    if free < u64::from(size) + FULL_RESERVE {
        return Err(Status::Full);
    }
    let own = fat.ensure_own_dir(d, fat.root(), DIR, t).map_err(card)?;
    let mut h = match fat.locate(d, own, TMP) {
        Ok(h) => h,
        Err(Error::NotFound) => fat.create(d, own, TMP, t).map_err(card)?,
        Err(e) => return Err(card(e)),
    };
    // the chain first (its last sector written), then the size: never a size past the chain
    fat.write_sectors(d, &mut h, size.div_ceil(SECTOR as u32) - 1, &[0; SECTOR]).map_err(card)?;
    fat.set_size(d, &mut h, size, t).map_err(card)?;
    Ok(h)
}

/// Sector `k` of the temporary file, written in place: never past its size (the chain never grows here).
pub fn put(fat: &mut Fat, d: &mut impl DiskWrite, h: &mut Handle, k: u32, s: &[u8; SECTOR]) -> Result<(), Status> {
    if k >= h.size.div_ceil(SECTOR as u32) {
        return Err(Status::BadRequest);
    }
    fat.write_sectors(d, h, k, s).map_err(card)
}

/// The name of the workout whose CRC-32 is `crc`: `XXXXXXXX.fit`.
///
/// ```
/// assert_eq!(&bryton_rustfw::storage::workout_write::name(0x01ab_cdef), b"01ABCDEF.fit");
/// ```
pub fn name(crc: u32) -> [u8; 12] {
    let mut n = *b"00000000.fit";
    for (i, c) in n.iter_mut().take(8).enumerate() {
        *c = b"0123456789ABCDEF".get((crc >> (28 - 4 * i) & 15) as usize).copied().unwrap_or(b'0');
    }
    n
}

/// The temporary file `h` checked (module doc, step 1) and moved into day folder `day` (steps 2 to 4).
pub fn commit(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    h: &Handle,
    day: &[u8; 8],
    crc: u32,
    t: (u16, u16),
) -> Result<(), Status> {
    check(fat, d, h, crc)?;
    let (n, day) = (name(crc), core::str::from_utf8(day).map_err(|_| Status::BadRequest)?);
    let n = core::str::from_utf8(&n).map_err(|_| Status::Io)?;
    workout_file::folder_day(day.as_bytes()).ok_or(Status::BadRequest)?;
    let cal = match fat.open(d, CALENDAR) {
        Ok(c) if c.dir => c,
        Ok(_) | Err(Error::NotFound) => return Err(Status::NotFound),
        Err(e) => return Err(card(e)),
    };
    let dir = fat.ensure_dir(d, cal, day, t).map_err(card)?;
    match fat.find(d, dir, n) {
        Ok(f) if !f.dir && f.size == h.size => return Ok(()), // sent again after a lost answer: there whole
        Ok(_) => return Err(Status::BadRequest),
        Err(Error::NotFound) => {}
        Err(e) => return Err(card(e)),
    }
    fat.make_room(d, dir, n).map_err(card)?;
    fat.remove(d, h).map_err(card)?;
    // from here to the link the chain is named by nothing: a cut leaves it lost, the calendar as it was
    fat.link(d, dir, n, h, t).map_err(card)?;
    Ok(())
}

/// Step 1 of [`commit`]: the file as the card holds it, its size and CRC-32 `crc`, and a workout.
#[inline(never)] // its buffer lives in its own frame, only while it runs
fn check(fat: &mut Fat, d: &mut impl DiskWrite, h: &Handle, crc: u32) -> Result<(), Status> {
    let mut buf = [0u8; workout_file::FILE_MAX];
    let f = File { cluster: h.first, size: h.size, dir: false };
    let n = fat.read_at(d, &f, 0, &mut buf).map_err(|_| Status::Io)?;
    let got = buf.get(..n).ok_or(Status::Io)?;
    if n != h.size as usize || crate::inflate::crc32(got.iter().copied()) != crc {
        return Err(Status::Mismatch);
    }
    workout_file::parse(got).map(|_| ()).map_err(|_| Status::BadRequest)
}

/// Deletes `e`, an entry of day folder `dir` (module doc): its entry marked deleted, then its chain freed.
pub fn delete(fat: &mut Fat, d: &mut impl DiskWrite, dir: File, e: &Entry) -> Result<(), Status> {
    let first = e.file.cluster;
    let h = if first == 0 {
        // an empty file has no chain to find it by
        let n = core::str::from_utf8(e.name()).map_err(|_| Status::NotFound)?;
        fat.locate(d, dir, n)
    } else {
        fat.locate_chain(d, dir, first)
    };
    let h = h.map_err(|e| if e == Error::NotFound { Status::NotFound } else { card(e) })?;
    fat.remove(d, &h).map_err(card)?;
    if first == 0 {
        return Ok(());
    }
    match fat.locate_chain(d, dir, first) {
        Err(Error::NotFound) => {}
        _ => return Ok(()), // still named (never seen): the chain stays
    }
    let mut c = fat.chain(d, first).map_err(card)?;
    fat.free_chain(d, &mut c).map_err(card)
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, read_all};
    use crate::fat::write::tests::{Cut, fsck, fsck_cut};
    use crate::storage::workout_file::tests::intervals;
    use std::vec::Vec;

    const T: (u16, u16) = (0x5b68, 0x6000);
    const DAY: &[u8; 8] = b"20261108";

    /// The whole write of `file` on `d`: begin, every sector, commit.
    pub(crate) fn write_all(d: &mut impl DiskWrite, file: &[u8], day: &[u8; 8]) -> Result<(), Status> {
        let mut fat = Fat::mount(d).map_err(card)?;
        let mut h = begin(&mut fat, d, file.len() as u32, T)?;
        for (k, c) in file.chunks(SECTOR).enumerate() {
            let mut s = [0; SECTOR];
            s[..c.len()].copy_from_slice(c);
            put(&mut fat, d, &mut h, k as u32, &s)?;
        }
        commit(&mut fat, d, &h, day, crate::inflate::crc32(file.iter().copied()), T)
    }

    /// The `.fit` files of `\Workout\calendar\<day>`: (name, bytes).
    pub(crate) fn day_files(img: &[u8], day: &str) -> Vec<(std::string::String, Vec<u8>)> {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let Ok(dir) = fat.open(&mut d, &std::format!("{CALENDAR}/{day}")) else { return Vec::new() };
        let mut c = fat.cursor(dir).unwrap();
        let mut out = Vec::new();
        while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
            let n = std::string::String::from_utf8(e.name().to_vec()).unwrap();
            if !e.file.dir && n.ends_with(".fit") {
                out.push((n.clone(), read_all(&mut fat, &mut d, &std::format!("{CALENDAR}/{day}/{n}")).unwrap()));
            }
        }
        out
    }

    /// `data` as `name` in the day folder of [`DAY`] (made if missing), as another writer would leave it.
    fn put_in_day(d: &mut Ram, name: &str, data: &[u8]) {
        let mut fat = Fat::mount(d).unwrap();
        let cal = fat.open(d, CALENDAR).unwrap();
        let dir = fat.ensure_dir(d, cal, "20261108", T).unwrap();
        let mut h = fat.create(d, dir, name, T).unwrap();
        if !data.is_empty() {
            let mut s = [0; SECTOR];
            s[..data.len()].copy_from_slice(data);
            fat.write_sectors(d, &mut h, 0, &s).unwrap();
            fat.set_size(d, &mut h, data.len() as u32, T).unwrap();
        }
    }

    fn volume() -> Vec<u8> {
        crate::fat::tests::image()
    }

    #[test]
    fn a_workout_is_written_named_by_its_crc_and_read_by_the_calendar() {
        let file = intervals();
        let mut d = Ram(volume(), 0);
        write_all(&mut d, &file, DAY).unwrap();
        let crc = crate::inflate::crc32(file.iter().copied());
        let n = std::string::String::from_utf8(name(crc).to_vec()).unwrap();
        assert_eq!(day_files(&d.0, "20261108"), [(n, file.clone())]);
        assert_eq!(fsck(&d.0), Ok(0), "clean, the temporary file's chain moved");
        // the unit's own reader of the calendar (and so what stock reads: the .fit of the day) sees it
        let cal = workout_file::calendar(&mut d, (2026, 11, 8), crate::ride::Zones::default()).unwrap();
        assert_eq!(cal.iter().count(), 1);
        // sent again (the answer lost): answered ok, written once
        write_all(&mut d, &file, DAY).unwrap();
        assert_eq!(day_files(&d.0, "20261108").len(), 1);
        // a second one the same day, then a delete of the first
        let mut other = file.clone();
        let at = other.len() - 3; // a byte of the last message's data, its file CRC put right
        other[at] ^= 1;
        let n = other.len() - 2;
        let c = crate::fit::crc(0, &other[..n]).to_le_bytes();
        other[n..].copy_from_slice(&c);
        write_all(&mut d, &other, DAY).unwrap();
        assert_eq!(day_files(&d.0, "20261108").len(), 2);
        let mut fat = Fat::mount(&mut d).unwrap();
        let dir = fat.open(&mut d, "Workout/calendar/20261108").unwrap();
        let mut cur = fat.cursor(dir).unwrap();
        let e = core::iter::from_fn(|| fat.list(&mut d, &mut cur).unwrap()).find(|e| !e.file.dir).unwrap();
        delete(&mut fat, &mut d, dir, &e).unwrap();
        assert_eq!(day_files(&d.0, "20261108").len(), 1);
        assert_eq!(fsck(&d.0), Ok(0), "the deleted workout's chain freed");
        crate::fat::tests::dump("workout_written", &d.0);
    }

    #[test]
    fn refused_files_never_reach_the_calendar() {
        let file = intervals();
        let mut d = Ram(volume(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(begin(&mut fat, &mut d, 0, T).err(), Some(Status::BadRequest));
        assert_eq!(begin(&mut fat, &mut d, workout_file::FILE_MAX as u32 + 1, T).err(), Some(Status::BadRequest));
        // the bytes are not the CRC given, or no workout (a CRC that matches): refused, the calendar unchanged
        let crc = crate::inflate::crc32(file.iter().copied());
        let not_fit = std::vec![0x55u8; 600];
        for (bytes, crc, want) in [
            (&file[..], crc ^ 1, Status::Mismatch),
            (&not_fit[..], crate::inflate::crc32(not_fit.iter().copied()), Status::BadRequest),
        ] {
            let mut h = begin(&mut fat, &mut d, bytes.len() as u32, T).unwrap();
            for (k, c) in bytes.chunks(SECTOR).enumerate() {
                let mut s = [0; SECTOR];
                s[..c.len()].copy_from_slice(c);
                put(&mut fat, &mut d, &mut h, k as u32, &s).unwrap();
            }
            assert_eq!(put(&mut fat, &mut d, &mut h, 99, &[0; SECTOR]), Err(Status::BadRequest), "past the size");
            assert_eq!(commit(&mut fat, &mut d, &h, DAY, crc, T), Err(want));
        }
        assert!(day_files(&d.0, "20261108").is_empty());
        assert_eq!(fsck(&d.0), Ok(0));
        // a day that is no date
        let mut h = begin(&mut fat, &mut d, file.len() as u32, T).unwrap();
        for (k, c) in file.chunks(SECTOR).enumerate() {
            let mut s = [0; SECTOR];
            s[..c.len()].copy_from_slice(c);
            put(&mut fat, &mut d, &mut h, k as u32, &s).unwrap();
        }
        assert_eq!(commit(&mut fat, &mut d, &h, b"20261341", crc, T), Err(Status::BadRequest));
        assert!(day_files(&d.0, "20261341").is_empty());
        // its name taken by a file of another size: refused, that file kept
        let n = std::string::String::from_utf8(name(crc).to_vec()).unwrap();
        put_in_day(&mut d, &n, b"other");
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(commit(&mut fat, &mut d, &h, DAY, crc, T), Err(Status::BadRequest));
        assert_eq!(day_files(&d.0, "20261108"), [(n, b"other".to_vec())]);
        // the largest file is taken; a card whose FSInfo counts no room is FULL
        assert!(begin(&mut fat, &mut d, workout_file::FILE_MAX as u32, T).is_ok());
        d.0[SECTOR + 488..SECTOR + 492].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(begin(&mut fat, &mut d, 100, T).err(), Some(Status::Full));
    }

    /// An empty `.fit` (no chain) is deleted by its name; a workout's chain is freed.
    #[test]
    fn delete_takes_an_empty_file_and_frees_a_chain() {
        let mut d = Ram(volume(), 0);
        put_in_day(&mut d, "EMPTY.fit", b"");
        write_all(&mut d, &intervals(), DAY).unwrap();
        for _ in 0..2 {
            let mut fat = Fat::mount(&mut d).unwrap();
            let dir = fat.open(&mut d, "Workout/calendar/20261108").unwrap();
            let mut cur = fat.cursor(dir).unwrap();
            let e = core::iter::from_fn(|| fat.list(&mut d, &mut cur).unwrap()).find(|e| !e.file.dir).unwrap();
            delete(&mut fat, &mut d, dir, &e).unwrap();
        }
        assert!(day_files(&d.0, "20261108").is_empty());
        assert_eq!(fsck(&d.0), Ok(0), "nothing lost");
    }

    /// A power cut after every sector the whole write makes (and before the first): the calendar holds nothing new
    /// or the whole workout, the volume passes fsck (a lost chain only between the remove and the link), and the next
    /// write, over what the cut left, goes through and leaves the volume clean but for that lost chain.
    #[test]
    fn a_cut_at_every_write_leaves_the_old_calendar_or_the_whole_workout() {
        let file = intervals();
        let base = volume();
        let mut full = Cut(Ram(base.clone(), 0), u32::MAX);
        write_all(&mut full, &file, DAY).unwrap();
        let writes = u32::MAX - full.1;
        assert!(writes > 5, "{writes} writes");
        let (mut none, mut whole, mut lost_seen) = (0, 0, 0);
        for at in 0..writes {
            let mut c = Cut(Ram(base.clone(), 0), at);
            assert_eq!(write_all(&mut c, &file, DAY), Err(Status::Io), "cut at {at}");
            let img = c.0.0;
            let scan = fsck_cut(&img).unwrap_or_else(|e| panic!("cut at {at}: {e}"));
            assert!(scan.lost <= 1 && scan.past.is_empty(), "cut at {at}: {} lost, {:?} past", scan.lost, scan.past);
            assert!(scan.apart <= 1, "cut at {at}: FATs apart in one sector at most");
            lost_seen += scan.lost;
            match day_files(&img, "20261108").as_slice() {
                [] => none += 1,
                [(_, got)] if *got == file => whole += 1,
                other => panic!("cut at {at}: {other:?}"),
            }
            // the boot's FAT sync, then the phone sends again: written, nothing more lost
            let mut d = Ram(img, 0);
            Fat::mount(&mut d).unwrap().sync_fats(&mut d).unwrap();
            write_all(&mut d, &file, DAY).unwrap_or_else(|e| panic!("again after a cut at {at}: {e:?}"));
            assert_eq!(day_files(&d.0, "20261108").len(), 1, "cut at {at}");
            assert_eq!(fsck(&d.0), Ok(scan.lost), "cut at {at}");
        }
        std::println!("{writes} writes: {none} cuts left nothing, {whole} the whole workout, {lost_seen} a lost chain");
        // the last write is the one that shows the workout: every cut before it leaves the calendar as it was
        assert!(none == writes && whole == 0 && lost_seen > 0, "{none} {whole} {lost_seen}");
        assert_eq!(
            day_files(&full.0.0, "20261108"),
            [(
                std::string::String::from_utf8(name(crate::inflate::crc32(file.iter().copied())).to_vec()).unwrap(),
                file
            )]
        );
    }
}
