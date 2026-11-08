//! A workout the phone sends, written into stock's calendar (`\Workout\calendar\<YYYYMMDD>\`), or a course into
//! `\Courses\` (2026-11-08), and a listed one deleted: the file service's only writes (link/filesvc.rs, WRITE_BEGIN /
//! WRITE_DATA / WRITE_END / DELETE; the app's side in app/filesvc.rs). Pure FAT work over [`DiskWrite`], with the FAT
//! writer's own write order (fat/write.rs) under it. A [`Target`] says where a file goes and what it must be; the
//! temporary file, the commit and the delete are the same for both, and so are the power-cut guarantees below.
//!
//! **Courses** ([`Target::Course`]): FIT files the unit follows ([`course_file::is_course`] on the read-back: the
//! phone converts GPX), at most [`COURSE_MAX`] bytes, named `<name>.fit` by the phone ([`CourseName`]: no dot nor
//! separator, so never outside `\Courses`, and the FAT writer gives it its long name and a hashed 8.3 alias).
//! `\Courses` itself must exist (NOT FOUND otherwise): stock never makes it (STORAGE_AND_MAPS.md section 3, in the
//! bryton repository's reverse engineering notes, names `\PlanTrip` for the app's routes and no `\Courses`); it is
//! the folder the owner copies courses into over USB (course_file.rs). A name taken by a file of the same size and
//! CRC-32 is the same course sent again (ok, written once); by any other, BAD REQUEST (never written over: the phone
//! deletes it first). The course being followed is the app's to protect (app/filesvc.rs: BUSY).
//!
//! **Where the bytes go first:** `\Rustfw\Inbox\WORKOUT.TMP`, this firmware's own folder (AGENTS.md, "Share the storage
//! with stock": stock never reads it), sized at [`begin`] (the chain taken, then the size set), then each 512-byte
//! sector written in place as the data comes ([`put`]): no FAT or directory write during the transfer, so a cut there
//! leaves a whole, named temporary file and nothing else. It is not removed after a cut: the next [`begin`] writes
//! over it (and frees what a longer one had past the new size). Removing it would free a chain, which goes through a
//! moment where no entry names it (a lost chain after a cut); kept, it costs one cluster of the own folder.
//!
//! **The commit** ([`commit`]), in this order:
//! 1. the temporary file read back whole from the card, its size and CRC-32 compared with what the phone gave, and
//!    parsed as a workout ([`workout_file::parse`]) or a course: anything else is refused, nothing written;
//! 2. the day folder made in `\Workout\calendar\` if missing (never `\Workout` or `\Workout\calendar` themselves: a
//!    card without them gets NOT FOUND; stock makes them when it syncs), the name looked up (taken by the same size:
//!    the phone sent it again after a lost answer, done), and room made for its entry ([`Fat::make_room`]);
//! 3. the temporary file's entry marked deleted ([`Fat::remove`]);
//! 4. the same chain linked into the day folder under its new name, whole, with its size ([`Fat::link`]: written
//!    deleted, then made live by its first bytes).
//!
//! For a course, step 2 is `\Courses` and its name instead of the day folder and the CRC's.
//!
//! **A power cut or a reset** at any point leaves the calendar as it was or with the whole workout, never a part of
//! one: before step 3 the calendar holds nothing new (at most an empty day folder, which stock and this firmware
//! list as no workouts); between 3 and 4 the chain is named by no entry, a lost chain (the file's clusters: a
//! workout's one on the unit, 8 KB at most in a 16 KB cluster; a course's up to 64) that a computer's fsck reports and
//! turns into a `.CHK`, as a screenshot cut does
//! (storage/screenshot.rs); after 4 the workout is there whole. The FAT writer's own steps that take a cluster (the
//! temporary file's first, at [`begin`], before its size names it; a new day folder's, before its entry) leave one
//! lost chain the same way when cut inside. No cluster is ever named twice, none is freed while an entry may name
//! it, and the phone learns of a cut by the END that never came, and sends again (written over the temporary file,
//! nothing more lost: the tests cut after every sector, `a_cut_at_every_write...` here and in src/sim/filesvc_tests.rs).
//!
//! Why no recovery record (as `trash.rs` keeps for a ride's move): the workout is not a ride, the phone still holds it,
//! and a record would have to tell a lost chain from one a computer's fsck named meanwhile (trash.rs `named_on_top`);
//! one lost chain (the file's clusters) after a cut in a two-write window costs less than that code.
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
use crate::fat::{DiskWrite, Entry, Error, Fat, File, SECTOR, Seek};
use crate::filesvc::{CourseName, Status};
use crate::recorder::{DIR, FULL_RESERVE};
use crate::storage::{course_file, workout_file};

/// Where a sent file goes, and so what it must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A workout for this day (`YYYYMMDD`), in `\Workout\calendar\<day>\`, named by its CRC-32.
    Day([u8; 8]),
    /// A course in `\Courses\`, `<name>.fit`.
    Course(CourseName),
}

impl Target {
    /// Its largest file.
    fn max(&self) -> u32 {
        match self {
            Target::Day(_) => workout_file::FILE_MAX as u32,
            Target::Course(_) => COURSE_MAX,
        }
    }

    /// Whether `name` is one of the files its folder lists (and counts against [`DAY_MAX`]).
    fn lists(&self, name: &[u8]) -> bool {
        match self {
            Target::Day(_) => is_fit(name),
            Target::Course(_) => is_course(name),
        }
    }
}

/// The largest course taken, bytes: 1 MB, a quarter of what the unit reads ([`course_file::MAX_BYTES`]). WRITE_END
/// reads it back twice in one pass of the loop (the CRC, then the course), about 0.2 s at the card's 10.5 MB/s
/// (docs/status.md), and it is some 40,000 FIT records (25 bytes each, inferred), a point every 10 m over 400 km:
/// more than the model keeps ([`crate::course::POINTS`]), so a longer route only costs the transfer.
pub const COURSE_MAX: u32 = 1 << 20;
/// The courses' folder, at the root.
pub const COURSES: &str = "Courses";

/// The temporary file, in [`INBOX`]: upper case, an 8.3 name.
pub const TMP: &str = "WORKOUT.TMP";
/// Its folder, in `\Rustfw`: the entries each commit leaves deleted (the writer never reuses a slot) pile up there,
/// never in the recorder's folder.
pub const INBOX: &str = "Inbox";
/// `.fit` files a day folder holds at most, and course files `\Courses` holds: the listing numbers them in two digits
/// (link/filesvc.rs).
pub const DAY_MAX: usize = 100;
/// Entries of a day folder walked at most, here and by app/filesvc.rs to find a workout: a folder longer than this
/// takes no more (the listing could not reach a new file).
pub(crate) const WALK: usize = 200;

/// A file name ending in `.fit`, any case, with something before it: a workout's (stock's list, inferred).
pub(crate) fn is_fit(name: &[u8]) -> bool {
    ends(name, b".fit")
}

/// A course file's name: `.fit` or `.gpx` (what course_file.rs lists).
pub(crate) fn is_course(name: &[u8]) -> bool {
    is_fit(name) || ends(name, b".gpx")
}

/// `name` ends in `ext` (any case), with something before it.
fn ends(name: &[u8], ext: &[u8]) -> bool {
    name.len() > ext.len() && name.get(name.len() - ext.len()..).is_some_and(|x| x.eq_ignore_ascii_case(ext))
}

/// Stock's calendar folder, or NOT FOUND (never made here: stock makes it when it syncs).
fn calendar(fat: &mut Fat, d: &mut impl DiskWrite) -> Result<File, Status> {
    folder(fat, d, CALENDAR)
}

/// Folder `path`, or NOT FOUND (never made here).
fn folder(fat: &mut Fat, d: &mut impl DiskWrite, path: &str) -> Result<File, Status> {
    match fat.open(d, path) {
        Ok(c) if c.dir => Ok(c),
        Ok(_) | Err(Error::NotFound) => Err(Status::NotFound),
        Err(e) => Err(card(e)),
    }
}
/// Stock's calendar, which holds a folder a day.
pub const CALENDAR: &str = "Workout/calendar";

/// A card error as the file service answers it: no room is FULL, the rest a card error.
fn card(e: Error) -> Status {
    if e == Error::Refused { Status::Full } else { Status::Io }
}

/// The temporary file made (or the one left kept) and sized for `size` bytes of a file for `to`, the card's room
/// checked first (the file and the recorder's [`FULL_RESERVE`], FSInfo's count; unknown counts as none): its handle
/// for [`put`].
pub fn begin(fat: &mut Fat, d: &mut impl DiskWrite, to: &Target, size: u32, t: (u16, u16)) -> Result<Handle, Status> {
    if size == 0 || size > to.max() {
        return Err(Status::BadRequest);
    }
    let free = fat.free_clusters(d).map_err(card)?.map_or(0, u64::from) * u64::from(fat.cluster_bytes());
    if free < u64::from(size) + FULL_RESERVE {
        return Err(Status::Full);
    }
    // nothing written for a card that cannot take the file
    match to {
        Target::Day(_) => calendar(fat, d)?,
        Target::Course(_) => folder(fat, d, COURSES)?,
    };
    let own = fat.ensure_own_dir(d, fat.root(), DIR, t).map_err(card)?;
    let own = fat.ensure_own_dir(d, own, INBOX, t).map_err(card)?;
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

/// The temporary file `h` checked (module doc, step 1) and moved where `to` says (steps 2 to 4).
pub fn commit(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    h: &Handle,
    to: &Target,
    crc: u32,
    t: (u16, u16),
) -> Result<(), Status> {
    check(fat, d, h, crc, to)?;
    let by_crc = name(crc);
    let (dir, n) = match to {
        Target::Day(day) => {
            let day = core::str::from_utf8(day).map_err(|_| Status::BadRequest)?;
            workout_file::folder_day(day.as_bytes()).ok_or(Status::BadRequest)?;
            let cal = calendar(fat, d)?;
            let dir = match fat.find(d, cal, day) {
                Ok(f) if f.dir => f,
                Ok(_) => return Err(Status::BadRequest), // a file of the day's name: never written over
                Err(Error::NotFound) => fat.ensure_dir(d, cal, day, t).map_err(card)?,
                Err(e) => return Err(card(e)),
            };
            (dir, core::str::from_utf8(&by_crc).map_err(|_| Status::Io)?)
        }
        Target::Course(c) => (folder(fat, d, COURSES)?, c.file_name()),
    };
    match fat.find(d, dir, n) {
        // sent again after a lost answer: there whole
        Ok(f) if !f.dir && f.size == h.size && crc_of(fat, d, &f)? == crc => return Ok(()),
        Ok(_) => return Err(Status::BadRequest), // another file of that name: never written over
        Err(Error::NotFound) => {}
        Err(e) => return Err(card(e)),
    }
    if listed(fat, d, dir, to)? >= DAY_MAX {
        return Err(Status::Full); // the listing could not number another
    }
    fat.make_room(d, dir, n).map_err(card)?;
    fat.remove(d, h).map_err(card)?;
    // from here to the link the chain is named by nothing: a cut leaves it lost, the calendar as it was
    fat.link(d, dir, n, h, t).map_err(card)?;
    Ok(())
}

/// The files of folder `dir` its listing counts ([`Target::lists`]; folders left out); [`DAY_MAX`] for a folder of
/// more than [`WALK`] entries.
fn listed(fat: &mut Fat, d: &mut impl DiskWrite, dir: File, to: &Target) -> Result<usize, Status> {
    let mut c = fat.cursor(dir).map_err(card)?;
    let mut n = 0;
    for _ in 0..WALK {
        let Some(e) = fat.list(d, &mut c).map_err(card)? else { return Ok(n) };
        n += usize::from(to.lists(e.name()) && !e.file.dir);
    }
    match fat.list(d, &mut c).map_err(card)? {
        None => Ok(n),
        Some(_) => Ok(DAY_MAX), // past WALK: a new file there could not be found
    }
}

/// Step 1 of [`commit`]: the file as the card holds it, its size and CRC-32 `crc`, and a workout or a course.
fn check(fat: &mut Fat, d: &mut impl DiskWrite, h: &Handle, crc: u32, to: &Target) -> Result<(), Status> {
    let f = File { cluster: h.first, size: h.size, dir: false };
    if let Target::Course(_) = to {
        if crc_of(fat, d, &f)? != crc {
            return Err(Status::Mismatch); // a short read too: the CRC of fewer bytes
        }
        return match course_file::is_course(fat, d, f) {
            Ok(true) => Ok(()),
            Ok(false) | Err(Error::Corrupt) => Err(Status::BadRequest),
            Err(_) => Err(Status::Io),
        };
    }
    check_workout(fat, d, f, crc)
}

/// The CRC-32 of file `f` as the card holds it, read a buffer at a time (fewer bytes than its size if the chain is
/// shorter: then not the phone's CRC).
#[inline(never)] // its buffer lives in its own frame, only while it runs
fn crc_of(fat: &mut Fat, d: &mut impl DiskWrite, f: &File) -> Result<u32, Status> {
    let (mut buf, mut seek, mut at, mut crc) = ([0u8; course_file::IO], Seek::new(), 0u32, 0u32);
    while at < f.size {
        let n = fat.read_from(d, f, at, &mut buf, &mut seek).map_err(|_| Status::Io)?;
        if n == 0 {
            break;
        }
        crc = crate::inflate::crc32_more(crc, buf.get(..n).unwrap_or(&[]).iter().copied());
        at = at.saturating_add(n as u32);
    }
    Ok(crc)
}

/// [`check`] for a workout: read whole into a buffer, its size and CRC-32, then parsed.
#[inline(never)] // its buffer lives in its own frame, only while it runs
fn check_workout(fat: &mut Fat, d: &mut impl DiskWrite, f: File, crc: u32) -> Result<(), Status> {
    let mut buf = [0u8; workout_file::FILE_MAX];
    let n = fat.read_at(d, &f, 0, &mut buf).map_err(|_| Status::Io)?;
    let got = buf.get(..n).ok_or(Status::Io)?;
    if n != f.size as usize || crate::inflate::crc32(got.iter().copied()) != crc {
        return Err(Status::Mismatch);
    }
    workout_file::parse(got).map(|_| ()).map_err(|_| Status::BadRequest)
}

/// Deletes `e`, an entry of folder `dir` (a day folder or `\Courses`; module doc): its entry marked deleted, then
/// its chain freed.
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
    const W: Target = Target::Day(*DAY);

    /// The whole write of `file` on `d`: begin, every sector, commit.
    pub(crate) fn write_all(d: &mut impl DiskWrite, file: &[u8], day: &[u8; 8]) -> Result<(), Status> {
        send(d, file, &Target::Day(*day))
    }

    /// The whole write of `file` for `to` on `d`: begin, every sector, commit.
    pub(crate) fn send(d: &mut impl DiskWrite, file: &[u8], to: &Target) -> Result<(), Status> {
        let mut fat = Fat::mount(d).map_err(card)?;
        let mut h = begin(&mut fat, d, to, file.len() as u32, T)?;
        for (k, c) in file.chunks(SECTOR).enumerate() {
            let mut s = [0; SECTOR];
            s[..c.len()].copy_from_slice(c);
            put(&mut fat, d, &mut h, k as u32, &s)?;
        }
        commit(&mut fat, d, &h, to, crate::inflate::crc32(file.iter().copied()), T)
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
        assert_eq!(begin(&mut fat, &mut d, &W, 0, T).err(), Some(Status::BadRequest));
        assert_eq!(begin(&mut fat, &mut d, &W, workout_file::FILE_MAX as u32 + 1, T).err(), Some(Status::BadRequest));
        // the bytes are not the CRC given, or no workout (a CRC that matches): refused, the calendar unchanged
        let crc = crate::inflate::crc32(file.iter().copied());
        let not_fit = std::vec![0x55u8; 600];
        for (bytes, crc, want) in [
            (&file[..], crc ^ 1, Status::Mismatch),
            (&not_fit[..], crate::inflate::crc32(not_fit.iter().copied()), Status::BadRequest),
        ] {
            let mut h = begin(&mut fat, &mut d, &W, bytes.len() as u32, T).unwrap();
            for (k, c) in bytes.chunks(SECTOR).enumerate() {
                let mut s = [0; SECTOR];
                s[..c.len()].copy_from_slice(c);
                put(&mut fat, &mut d, &mut h, k as u32, &s).unwrap();
            }
            assert_eq!(put(&mut fat, &mut d, &mut h, 99, &[0; SECTOR]), Err(Status::BadRequest), "past the size");
            assert_eq!(commit(&mut fat, &mut d, &h, &W, crc, T), Err(want));
        }
        assert!(day_files(&d.0, "20261108").is_empty());
        assert_eq!(fsck(&d.0), Ok(0));
        // a day that is no date
        let mut h = begin(&mut fat, &mut d, &W, file.len() as u32, T).unwrap();
        for (k, c) in file.chunks(SECTOR).enumerate() {
            let mut s = [0; SECTOR];
            s[..c.len()].copy_from_slice(c);
            put(&mut fat, &mut d, &mut h, k as u32, &s).unwrap();
        }
        assert_eq!(commit(&mut fat, &mut d, &h, &Target::Day(*b"20261341"), crc, T), Err(Status::BadRequest));
        assert!(day_files(&d.0, "20261341").is_empty());
        // its name taken by a file of another size: refused, that file kept
        let n = std::string::String::from_utf8(name(crc).to_vec()).unwrap();
        put_in_day(&mut d, &n, b"other");
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(commit(&mut fat, &mut d, &h, &W, crc, T), Err(Status::BadRequest));
        assert_eq!(day_files(&d.0, "20261108"), [(n, b"other".to_vec())]);
        // the largest file is taken; a card whose FSInfo counts no room is FULL
        assert!(begin(&mut fat, &mut d, &W, workout_file::FILE_MAX as u32, T).is_ok());
        d.0[SECTOR + 488..SECTOR + 492].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(begin(&mut fat, &mut d, &W, 100, T).err(), Some(Status::Full));
    }

    /// No `\Workout\calendar` on the card: BEGIN is NOT FOUND and writes nothing.
    #[test]
    fn no_calendar_no_write() {
        let mut img = volume();
        let at = img.windows(11).position(|w| w == b"CALENDAR   ").expect("the calendar's 8.3 entry");
        img[at + 7] = b'X'; // CALENDAX: its long name no longer matches, the folder is not found
        let mut d = Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(begin(&mut fat, &mut d, &W, 100, T).err(), Some(Status::NotFound));
        assert!(d.0 == img, "nothing written");
    }

    /// A file in the calendar with the day's name (no folder): refused, never written over nor beside.
    #[test]
    fn a_file_of_the_days_name_is_not_a_day_folder() {
        let mut d = Ram(volume(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let cal = fat.open(&mut d, CALENDAR).unwrap();
        fat.create(&mut d, cal, "20261108", T).unwrap();
        assert_eq!(write_all(&mut d, &intervals(), DAY), Err(Status::BadRequest));
        let mut fat = Fat::mount(&mut d).unwrap();
        assert!(fat.open(&mut d, "Workout/calendar/20261108").is_ok_and(|f| !f.dir && f.size == 0), "left as it was");
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A day folder of more entries than [`WALK`] takes no more: the listing could not reach the new file.
    #[test]
    fn a_day_longer_than_the_walk_is_full() {
        let mut d = Ram(volume(), 0);
        for i in 0..=WALK {
            put_in_day(&mut d, &std::format!("note{i}.txt"), b"x");
        }
        assert_eq!(write_all(&mut d, &intervals(), DAY), Err(Status::Full));
        assert!(day_files(&d.0, "20261108").is_empty());
        assert!(is_fit(b"A.FIT") && !is_fit(b".fit") && !is_fit(b"a.fi"));
    }

    /// A day holds [`DAY_MAX`] workouts: the 101st is FULL, nothing written; the same file sent again is still ok.
    #[test]
    fn a_full_day_refuses_one_more() {
        let mut d = Ram(volume(), 0);
        let one = |i: u32| {
            let steps = [(0, 60_000 + i, 4, 0, 100, 110, 0)];
            crate::storage::workout_file::tests::file(5, &std::format!("W{i}"), &steps)
        };
        for i in 0..DAY_MAX as u32 {
            write_all(&mut d, &one(i), DAY).unwrap_or_else(|e| panic!("{i}: {e:?}"));
        }
        assert_eq!(day_files(&d.0, "20261108").len(), DAY_MAX);
        let mut fat = Fat::mount(&mut d).unwrap();
        let file = one(1000);
        let mut h = begin(&mut fat, &mut d, &W, file.len() as u32, T).unwrap();
        let mut s = [0; SECTOR];
        s[..file.len()].copy_from_slice(&file);
        put(&mut fat, &mut d, &mut h, 0, &s).unwrap();
        let mid = d.0.clone();
        assert_eq!(commit(&mut fat, &mut d, &h, &W, crate::inflate::crc32(file.iter().copied()), T), Err(Status::Full));
        assert!(d.0 == mid, "nothing written by the refused commit");
        assert_eq!(day_files(&d.0, "20261108").len(), DAY_MAX);
        write_all(&mut d, &one(7), DAY).unwrap();
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// A power cut after every sector a delete writes: the day folder holds the file whole or nothing, fsck passes (a
    /// lost chain at most, between the entry's delete and the chain freed), and a second delete or a re-send goes through.
    #[test]
    fn a_cut_at_every_write_of_a_delete() {
        cut_every_delete(&intervals(), &W);
    }

    /// The file `to` names for `file` on volume `img`, if it is there: the workout's CRC name in its day, or the course's.
    fn sent(img: &[u8], to: &Target, file: &[u8]) -> Option<Vec<u8>> {
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        read_all(&mut fat, &mut d, &path(to, file)).ok()
    }

    /// The path of the file `to` names for `file`.
    fn path(to: &Target, file: &[u8]) -> std::string::String {
        match to {
            Target::Day(day) => {
                let n = name(crate::inflate::crc32(file.iter().copied()));
                std::format!("{CALENDAR}/{}/{}", std::str::from_utf8(day).unwrap(), std::str::from_utf8(&n).unwrap())
            }
            Target::Course(c) => std::format!("{COURSES}/{}", c.file_name()),
        }
    }

    /// The clusters of `file` on the test volume: what one lost chain of it holds.
    fn chain(file: &[u8]) -> u32 {
        Fat::mount(&mut Ram(volume(), 0)).unwrap().clusters_for(file.len() as u32)
    }

    /// Deletes the file `to` names for `file`, found by its name in its folder.
    fn delete_sent(d: &mut impl DiskWrite, to: &Target, file: &[u8]) -> Result<(), Status> {
        let p = path(to, file);
        let (dir, n) = p.rsplit_once('/').unwrap();
        let mut fat = Fat::mount(d).map_err(card)?;
        let dir = fat.open(d, dir).map_err(card)?;
        let mut cur = fat.cursor(dir).map_err(card)?;
        let e = core::iter::from_fn(|| fat.list(d, &mut cur).ok().flatten()).find(|e| e.name() == n.as_bytes());
        delete(&mut fat, d, dir, &e.ok_or(Status::NotFound)?)
    }

    /// A power cut after every sector the delete of `file` (sent for `to`) writes: the file whole or gone, fsck passes
    /// (a lost chain at most), and a second delete or a re-send goes through.
    fn cut_every_delete(file: &[u8], to: &Target) {
        let mut d = Ram(volume(), 0);
        send(&mut d, file, to).unwrap();
        let base = d.0;
        let mut full = Cut(Ram(base.clone(), 0), u32::MAX);
        delete_sent(&mut full, to, file).unwrap();
        let writes = u32::MAX - full.1;
        assert!(writes >= 3, "{writes}");
        assert_eq!(sent(&full.0.0, to, file), None);
        for at in 0..writes {
            let mut c = Cut(Ram(base.clone(), 0), at);
            assert_eq!(delete_sent(&mut c, to, file), Err(Status::Io), "cut at {at}");
            let scan = fsck_cut(&c.0.0).unwrap_or_else(|e| panic!("cut at {at}: {e}"));
            assert!(
                scan.lost <= chain(file) && scan.past.is_empty(),
                "cut at {at}: {} lost {:?}",
                scan.lost,
                scan.past
            );
            let left = sent(&c.0.0, to, file);
            assert!(left.as_deref().is_none_or(|l| l == file), "cut at {at}");
            c.1 = u32::MAX;
            if left.is_none() {
                send(&mut c, file, to).unwrap_or_else(|e| panic!("re-send after a cut at {at}: {e:?}"));
            } else {
                delete_sent(&mut c, to, file).unwrap_or_else(|e| panic!("again after a cut at {at}: {e:?}"));
            }
        }
    }

    /// A power cut after every sector sending `file` for `to` writes (and before the first): the folder holds nothing
    /// new or the whole file, the volume passes fsck (a lost chain only between the remove and the link), and the
    /// next send, over what the cut left, goes through and leaves the volume clean but for that lost chain. The last
    /// write is the one that shows the file: every cut before it leaves the folder as it was.
    fn cut_every_write(file: &[u8], to: &Target) -> Vec<u8> {
        let base = volume();
        let mut full = Cut(Ram(base.clone(), 0), u32::MAX);
        send(&mut full, file, to).unwrap();
        let writes = u32::MAX - full.1;
        assert!(writes > 5, "{writes} writes");
        let (mut none, mut lost_seen) = (0, 0);
        for at in 0..writes {
            let mut c = Cut(Ram(base.clone(), 0), at);
            assert_eq!(send(&mut c, file, to), Err(Status::Io), "cut at {at}");
            let img = c.0.0;
            let scan = fsck_cut(&img).unwrap_or_else(|e| panic!("cut at {at}: {e}"));
            assert!(
                scan.lost <= chain(file) && scan.past.is_empty(),
                "cut at {at}: {} lost, {:?} past",
                scan.lost,
                scan.past
            );
            assert!(scan.apart <= 1, "cut at {at}: FATs apart in one sector at most");
            lost_seen += scan.lost;
            none += usize::from(sent(&img, to, file).is_none());
            // the boot's FAT sync, then the phone sends again: written, nothing more lost
            let mut d = Ram(img, 0);
            Fat::mount(&mut d).unwrap().sync_fats(&mut d).unwrap();
            send(&mut d, file, to).unwrap_or_else(|e| panic!("again after a cut at {at}: {e:?}"));
            assert_eq!(sent(&d.0, to, file).as_deref(), Some(file), "cut at {at}");
            assert_eq!(fsck(&d.0), Ok(scan.lost), "cut at {at}");
        }
        std::println!("{writes} writes: {none} cuts left nothing, {lost_seen} a lost chain");
        assert!(none == writes as usize && lost_seen > 0, "{none} {lost_seen}");
        assert_eq!(sent(&full.0.0, to, file).as_deref(), Some(file));
        full.0.0
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

    /// A power cut after every sector the whole write makes: the calendar as it was or with the whole workout
    /// ([`cut_every_write`]), and nothing else in the day.
    #[test]
    fn a_cut_at_every_write_leaves_the_old_calendar_or_the_whole_workout() {
        let file = intervals();
        let img = cut_every_write(&file, &W);
        let n = std::string::String::from_utf8(name(crate::inflate::crc32(file.iter().copied())).to_vec()).unwrap();
        assert_eq!(day_files(&img, "20261108"), [(n, file)]);
    }

    /// A FIT course of the synthetic card (tools/fat_image.py): the zigzag, with its course points.
    fn turns() -> Vec<u8> {
        let mut d = Ram(volume(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        read_all(&mut fat, &mut d, "Courses/Turns.fit").unwrap()
    }

    fn course(n: &str) -> Target {
        Target::Course(CourseName::new(n.as_bytes()).unwrap())
    }

    /// The same for a course: `\Courses` as it was or with the whole course.
    #[test]
    fn a_cut_at_every_write_of_a_course_leaves_the_folder_as_it_was_or_with_the_whole_course() {
        cut_every_write(&turns(), &course("Zig zag"));
    }

    /// And for its delete.
    #[test]
    fn a_cut_at_every_write_of_a_course_delete() {
        cut_every_delete(&turns(), &course("Zig zag"));
    }

    /// A course is written as `<name>.fit` in `\Courses`, listed by the unit's COURSES (its long name, capitals) and
    /// read whole as one to follow; sent again it is written once; another course of that name is refused, the first
    /// kept; deleted, its chain is freed.
    #[test]
    fn a_course_is_written_listed_followed_and_deleted() {
        let file = turns();
        let mut d = Ram(volume(), 0);
        let to = course("Col du Galibier 2");
        send(&mut d, &file, &to).unwrap();
        assert_eq!(sent(&d.0, &to, &file).as_deref(), Some(&file[..]));
        assert_eq!(fsck(&d.0), Ok(0));
        let mut c = course_file::Courses::new();
        c.open(&mut d).unwrap();
        let k = (0..c.list.len()).find(|&i| c.list.get(i).unwrap().name() == "COL DU GALIBIER 2").expect("listed");
        c.load(k);
        while c.step(&mut d, 1 << 20) == course_file::Progress::Reading {}
        assert!(c.on.is_some(), "followed");
        assert!(c.uses(c.list.get(k).unwrap().file));
        send(&mut d, &file, &to).unwrap();
        let mut other = file.clone();
        other.push(0); // a byte more: another file of that name
        assert_eq!(send(&mut d, &other, &to), Err(Status::BadRequest));
        assert_eq!(sent(&d.0, &to, &file).as_deref(), Some(&file[..]), "kept, once");
        // another case of the same name is the same name on FAT
        assert_eq!(send(&mut d, &other, &course("COL DU GALIBIER 2")), Err(Status::BadRequest));
        // the same size, other bytes (a course still, its FIT CRC put right): another file too
        let mut same_size = file.clone();
        let at = same_size.len() - 3;
        same_size[at] ^= 1;
        let n = same_size.len() - 2;
        let c = crate::fit::crc(0, &same_size[..n]).to_le_bytes();
        same_size[n..].copy_from_slice(&c);
        assert_eq!(send(&mut d, &same_size, &course("Other")), Ok(()), "a course");
        assert_eq!(send(&mut d, &same_size, &to), Err(Status::BadRequest));
        assert_eq!(sent(&d.0, &to, &file).as_deref(), Some(&file[..]), "kept");
        delete_sent(&mut d, &course("Other"), &same_size).unwrap();
        assert_eq!(fsck(&d.0), Ok(0), "the refused sends left the temporary file, nothing lost");
        delete_sent(&mut d, &to, &file).unwrap();
        assert_eq!(sent(&d.0, &to, &file), None);
        assert_eq!(fsck(&d.0), Ok(0), "its chain freed");
        crate::fat::tests::dump("course_written", &d.0);
    }

    /// Courses that are not, too big, a CRC that lies, a full card, no `\Courses`: refused, the folder unchanged.
    #[test]
    fn refused_courses_never_reach_the_folder() {
        let (file, to) = (turns(), course("Turns again"));
        let mut d = Ram(volume(), 0);
        let gpx = b"<gpx><trk><trkseg><trkpt lat=\"45.0\" lon=\"6.0\"/><trkpt lat=\"45.01\" lon=\"6.0\"/></trkseg></trk></gpx>";
        let one_point = {
            // a FIT the reader takes, with one point: not a course to follow
            let mut f = std::vec::Vec::new();
            let (mut w, _) = crate::fit::Writer::start(&mut f, 1_000_000).unwrap();
            let s = crate::fit::Sample { time: 1_000_001, pos: Some((1 << 20, 1 << 21)), ..Default::default() };
            w.record(&mut f, &s).unwrap();
            w.finish(&mut f, 1_000_002).unwrap();
            f
        };
        for bad in [&gpx[..], &one_point, &[0x55; 600]] {
            assert_eq!(send(&mut d, bad, &to), Err(Status::BadRequest), "{} bytes", bad.len());
        }
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(begin(&mut fat, &mut d, &to, COURSE_MAX + 1, T).err(), Some(Status::BadRequest));
        assert!(begin(&mut fat, &mut d, &to, COURSE_MAX, T).is_ok(), "the largest is taken");
        // the bytes are not the CRC given
        let mut h = begin(&mut fat, &mut d, &to, file.len() as u32, T).unwrap();
        for (k, c) in file.chunks(SECTOR).enumerate() {
            let mut s = [0; SECTOR];
            s[..c.len()].copy_from_slice(c);
            put(&mut fat, &mut d, &mut h, k as u32, &s).unwrap();
        }
        let crc = crate::inflate::crc32(file.iter().copied());
        assert_eq!(commit(&mut fat, &mut d, &h, &to, crc ^ 1, T), Err(Status::Mismatch));
        assert_eq!(sent(&d.0, &to, &file), None);
        assert_eq!(fsck(&d.0), Ok(0));
        // a card whose FSInfo counts no room is FULL
        let mut full = d.0.clone();
        full[SECTOR + 488..SECTOR + 492].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(send(&mut Ram(full, 0), &file, &to), Err(Status::Full));
        // no \Courses on the card: NOT FOUND, nothing written
        let at = d.0.windows(11).position(|w| w == b"COURSES    ").expect("the folder's 8.3 entry");
        d.0[at + 6] = b'X'; // its long name no longer matches: not found
        let img = d.0.clone();
        assert_eq!(send(&mut d, &file, &to), Err(Status::NotFound));
        assert!(d.0 == img, "nothing written");
    }

    /// `\Courses` holds [`DAY_MAX`] course files at most (`.fit` and `.gpx` count, other files do not), and takes no
    /// more once longer than [`WALK`] entries: FULL, nothing written; the same file sent again is still ok.
    #[test]
    fn a_full_course_folder_refuses_one_more() {
        let file = turns();
        let mut d = Ram(volume(), 0);
        let have = {
            let mut fat = Fat::mount(&mut d).unwrap();
            let dir = fat.open(&mut d, COURSES).unwrap();
            listed(&mut fat, &mut d, dir, &course("x")).unwrap()
        };
        for i in have..DAY_MAX - 1 {
            crate::fat::tests::put_file(&mut d, COURSES, &std::format!("C{i}.{}", ["fit", "gpx"][i % 2]), b"x");
        }
        crate::fat::tests::put_file(&mut d, COURSES, "notes.txt", b"x");
        send(&mut d, &file, &course("Last")).unwrap();
        assert_eq!(send(&mut d, &file, &course("One more")), Err(Status::Full));
        assert_eq!(sent(&d.0, &course("One more"), &file), None);
        assert_eq!(fsck(&d.0), Ok(0), "the temporary file kept, nothing lost");
        send(&mut d, &file, &course("Last")).unwrap();
        assert_eq!(fsck(&d.0), Ok(0));
        // past WALK entries: FULL whatever the count
        let mut d = Ram(volume(), 0);
        for i in 0..=WALK {
            crate::fat::tests::put_file(&mut d, COURSES, &std::format!("n{i}.txt"), b"x");
        }
        assert_eq!(send(&mut d, &file, &course("Late")), Err(Status::Full));
        assert!(is_course(b"A.GPX") && is_course(b"b.fit") && !is_course(b".gpx") && !is_course(b"a.gp"));
    }
}
