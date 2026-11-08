//! A workout the phone sends, written into stock's calendar (`\Workout\calendar\<YYYYMMDD>\`), or a course into
//! `\Courses\` (2026-11-08), and a listed one deleted: the file service's only writes (link/filesvc.rs, WRITE_BEGIN /
//! WRITE_DATA / WRITE_END / DELETE; the app's side in app/filesvc.rs). Pure FAT work over [`DiskWrite`], with the FAT
//! writer's own write order (fat/write.rs) under it. A [`Target`] says where a file goes and what it must be; the
//! temporary file, the commit and the delete are the same for both, and so are the power-cut guarantees below.
//!
//! **Courses** ([`Target::Course`]): FIT files the unit follows ([`course_file::is_course`] on the read-back: the
//! phone converts GPX), at most [`COURSE_MAX`] bytes, named `<name>.fit` by the phone ([`CourseName`]: no dot nor
//! separator, so never outside `\Courses`, and the FAT writer gives it its long name and a hashed 8.3 alias).
//! `\Courses` is made by the first course sent, at the commit's step 2 as a day folder is (so a refused send leaves
//! the root as it was), with [`Fat::ensure_dir`] and never `ensure_own_dir`, whose repair would empty a torn
//! `\Courses` and orphan the courses copied into it over USB. Stock never reads it (STORAGE_AND_MAPS.md section 3, in
//! the bryton repository's reverse engineering notes, names `\PlanTrip` for the app's routes: Bryton's own format,
//! which the Bryton app manages, so never written here). A name taken by a file of the same size and CRC-32 is the
//! same course sent again (ok, written once); by any other, BAD REQUEST (never written over: the phone deletes it
//! first); so is a name COURSES would show for another course (a `.gpx` of the same stem, a `\PlanTrip` route, `_`
//! against a space: [`course_file::survey`]); and COURSES full ([`course_file::LIST`] of `\PlanTrip` and `\Courses`
//! together) is FULL, so a sent course never misses from the unit's menu. These checks read nothing of the course and
//! run at [`begin`] too, before the temporary file is touched. The course being followed is the app's to protect
//! (app/filesvc.rs: BUSY).
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
//! one lost chain (the file's clusters) after a cut in a two-write window costs less than that code. For a course
//! that chain is up to 64 clusters (1 MB in 16 KB clusters), and the same after a cut inside a delete (its entry
//! marked, its chain not yet freed). The firmware never reclaims it; a computer's fsck frees it (or keeps it as a
//! `.CHK`), and the card's room check counts it as taken meanwhile.
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
/// reads up to three times that in one pass of the loop (the CRC, then the course, and on a name already taken the
/// file there's CRC), about 0.3 s at the card's 10.5 MB/s (docs/status.md), and it is some 40,000 FIT records (25 bytes each, inferred), a point every 10 m over 400 km:
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

/// Folder `name` in `parent`, None if there is none; a file of that name is BAD REQUEST (never written over).
fn find_dir(fat: &mut Fat, d: &mut impl DiskWrite, parent: File, name: &str) -> Result<Option<File>, Status> {
    match fat.find(d, parent, name) {
        Ok(f) if f.dir => Ok(Some(f)),
        Ok(_) => Err(Status::BadRequest),
        Err(Error::NotFound) => Ok(None),
        Err(e) => Err(card(e)),
    }
}

/// The checks of course `c` of `size` bytes that read nothing of it, at [`begin`] (so a refusal touches no
/// temporary file) and again at [`commit`]: a file of its name in `\Courses` (`dir`; None: not made yet) of another
/// size is BAD REQUEST, of the same size it may be the same course sent again (the commit compares the CRC); a name
/// COURSES shows for another course (a `.gpx` of the same stem, a `\PlanTrip` route, `_` against a space) is BAD
/// REQUEST; `\Courses` at [`DAY_MAX`], or COURSES at its [`course_file::LIST`], is FULL: a sent course never misses
/// from the unit's menu.
fn course_room(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    dir: Option<File>,
    c: &CourseName,
    size: u32,
) -> Result<(), Status> {
    let n = c.file_name();
    if let Some(dir) = dir {
        match fat.find(d, dir, n) {
            Ok(f) if !f.dir && f.size == size => return Ok(()),
            Ok(_) => return Err(Status::BadRequest),
            Err(Error::NotFound) => {}
            Err(e) => return Err(card(e)),
        }
    }
    let (count, clash) = course_file::survey(fat, d, n).map_err(card)?;
    if clash {
        return Err(Status::BadRequest);
    }
    let here = match dir {
        Some(dir) => listed(fat, d, dir, &Target::Course(*c))?,
        None => 0,
    };
    if here >= DAY_MAX || count >= course_file::LIST {
        return Err(Status::Full);
    }
    Ok(())
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
        Target::Day(_) => drop(calendar(fat, d)?),
        Target::Course(c) => {
            let dir = find_dir(fat, d, fat.root(), COURSES)?;
            course_room(fat, d, dir, c, size)?;
        }
    }
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
            let dir = match find_dir(fat, d, cal, day)? {
                Some(dir) => dir,
                None => fat.ensure_dir(d, cal, day, t).map_err(card)?,
            };
            (dir, core::str::from_utf8(&by_crc).map_err(|_| Status::Io)?)
        }
        Target::Course(c) => {
            let root = fat.root();
            let dir = find_dir(fat, d, root, COURSES)?;
            course_room(fat, d, dir, c, h.size)?;
            // the first course makes `\Courses` (never `ensure_own_dir`: its repair would empty a torn one, the
            // courses copied over USB with it)
            let dir = match dir {
                Some(dir) => dir,
                None => fat.ensure_dir(d, root, COURSES, t).map_err(card)?,
            };
            (dir, c.file_name())
        }
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
    use crate::fat::write::tests::{Cut, fsck, fsck_cut, fsck_torn};
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
        cut_every_delete(volume(), &intervals(), &W, None);
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

    /// The files of the folder `to` writes into, sorted by name: (name, bytes).
    fn folder_files(img: &[u8], to: &Target) -> Vec<(std::string::String, Vec<u8>)> {
        let p = path(to, b"");
        let dir = p.rsplit_once('/').unwrap().0;
        let mut d = Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let Ok(f) = fat.open(&mut d, dir) else { return Vec::new() };
        let mut c = fat.cursor(f).unwrap();
        let mut out = Vec::new();
        while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
            let n = std::string::String::from_utf8(e.name().to_vec()).unwrap();
            if !e.file.dir {
                out.push((n.clone(), read_all(&mut fat, &mut d, &std::format!("{dir}/{n}")).unwrap()));
            }
        }
        out.sort();
        out
    }

    /// `files` with `(name, file)` added, sorted.
    fn plus(files: &[(std::string::String, Vec<u8>)], to: &Target, file: &[u8]) -> Vec<(std::string::String, Vec<u8>)> {
        let p = path(to, file);
        let mut v = files.to_vec();
        v.push((p.rsplit_once('/').unwrap().1.into(), file.to_vec()));
        v.sort();
        v
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

    /// A power cut after every sector the delete of `file` (sent for `to` on volume `base`) writes: the folder as it
    /// was with the file whole, or as it was without it; fsck passes (a lost chain at most, the file's clusters); and a
    /// second delete or a re-send goes through. `dump`: the volume a cut left between the entry marked deleted and its
    /// chain freed is written for check.sh's fsck_msdos / fsck.vfat under that name.
    fn cut_every_delete(base: Vec<u8>, file: &[u8], to: &Target, dump: Option<&str>) {
        let mut d = Ram(base, 0);
        send(&mut d, file, to).unwrap();
        let with = folder_files(&d.0, to);
        let base = d.0;
        let without: Vec<_> = with.iter().filter(|f| plus(&[], to, file)[0].0 != f.0).cloned().collect();
        assert_eq!(without.len() + 1, with.len());
        let mut full = Cut(Ram(base.clone(), 0), u32::MAX);
        delete_sent(&mut full, to, file).unwrap();
        let writes = u32::MAX - full.1;
        assert!(writes >= 3, "{writes}");
        assert_eq!(folder_files(&full.0.0, to), without);
        let mut dumped = dump.is_none();
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
            let left = folder_files(&c.0.0, to);
            assert!(left == with || left == without, "cut at {at}");
            if !dumped && scan.lost == chain(file) {
                crate::fat::tests::dump(&std::format!("{}_lost{}", dump.unwrap_or_default(), scan.lost), &c.0.0);
                dumped = true;
            }
            c.1 = u32::MAX;
            if left == without {
                send(&mut c, file, to).unwrap_or_else(|e| panic!("re-send after a cut at {at}: {e:?}"));
                assert_eq!(folder_files(&c.0.0, to), with, "cut at {at}");
            } else {
                delete_sent(&mut c, to, file).unwrap_or_else(|e| panic!("again after a cut at {at}: {e:?}"));
                assert_eq!(folder_files(&c.0.0, to), without, "cut at {at}");
            }
        }
        assert!(dumped, "a cut between the entry and its chain");
    }

    /// A power cut after every sector sending `file` for `to` on volume `base` writes (and before the first): the
    /// target folder exactly as it was, the volume passes fsck (a lost chain only between the remove and the link,
    /// the file's clusters), and the next send, over what the cut left, goes through and leaves the folder as it was
    /// plus the file, the volume clean but for that lost chain. The last write is the one that shows the file: every
    /// cut before it leaves the folder as it was. `dump`: the volume cut between the remove and the link is written
    /// for check.sh's fsck_msdos / fsck.vfat under that name. The writes of the whole send, for a torn cut.
    fn cut_every_write(base: Vec<u8>, file: &[u8], to: &Target, dump: Option<&str>) -> u32 {
        let before = folder_files(&base, to);
        let mut full = Cut(Ram(base.clone(), 0), u32::MAX);
        send(&mut full, file, to).unwrap();
        let writes = u32::MAX - full.1;
        assert!(writes > 5, "{writes} writes");
        assert_eq!(folder_files(&full.0.0, to), plus(&before, to, file));
        // clusters already lost on `base` (no_courses: the old folder's)
        let (mut none, mut lost_seen, lost0) = (0, 0, fsck(&base).unwrap());
        for at in 0..writes {
            let mut c = Cut(Ram(base.clone(), 0), at);
            assert_eq!(send(&mut c, file, to), Err(Status::Io), "cut at {at}");
            let img = c.0.0;
            let scan = fsck_cut(&img).unwrap_or_else(|e| panic!("cut at {at}: {e}"));
            assert!(
                scan.lost <= lost0 + chain(file) && scan.past.is_empty(),
                "cut at {at}: {} lost, {:?} past",
                scan.lost,
                scan.past
            );
            assert!(scan.apart <= 1, "cut at {at}: FATs apart in one sector at most");
            lost_seen += scan.lost - lost0;
            assert_eq!(folder_files(&img, to), before, "cut at {at}: the folder as it was");
            none += usize::from(sent(&img, to, file).is_none());
            if let (Some(name), true) = (dump, at + 2 == writes) {
                assert_eq!(scan.lost, lost0 + chain(file), "between the remove and the link: the chain lost");
                // the count in the name: check.sh's fsck allows that many orphan clusters there, nothing else
                crate::fat::tests::dump(&std::format!("{name}_lost{}", scan.lost), &img);
            }
            // the boot's FAT sync, then the phone sends again: written, nothing more lost
            let mut d = Ram(img, 0);
            Fat::mount(&mut d).unwrap().sync_fats(&mut d).unwrap();
            send(&mut d, file, to).unwrap_or_else(|e| panic!("again after a cut at {at}: {e:?}"));
            assert_eq!(folder_files(&d.0, to), plus(&before, to, file), "cut at {at}");
            assert_eq!(fsck(&d.0), Ok(scan.lost), "cut at {at}");
        }
        std::println!("{writes} writes: {none} cuts left nothing, {lost_seen} a lost chain");
        assert!(none == writes as usize && lost_seen > 0, "{none} {lost_seen}");
        writes
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
    /// ([`cut_every_write`]).
    #[test]
    fn a_cut_at_every_write_leaves_the_old_calendar_or_the_whole_workout() {
        cut_every_write(volume(), &intervals(), &W, None);
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

    /// The longest name, two long name pieces with its `.fit`.
    const LONG: &str = "Col du Galibier 2-b_";

    /// The test volume with `\Courses` filled with text files (no course) until its end marker sits at slot 15 of a
    /// sector: the next entry of more than one slot goes into the next sector, the slots left marked deleted.
    fn courses_at_slot_15() -> Vec<u8> {
        let mut d = Ram(volume(), 0);
        for i in 0..64 {
            let mut fat = Fat::mount(&mut d).unwrap();
            let dir = fat.open(&mut d, COURSES).unwrap();
            let h = fat.chain(&mut d, dir.cluster).unwrap();
            let per = fat.cluster_bytes() / SECTOR as u32;
            let mut end = None;
            for k in 0..h.clusters * per {
                let mut s = [0u8; SECTOR];
                fat.read_sector(&mut d, &h, k, &mut s).unwrap();
                if let Some(i) = s.chunks(32).position(|e| e[0] == 0) {
                    end = Some(i);
                    break;
                }
            }
            match end {
                Some(15) => return d.0,
                // a name of one long name piece takes 2 slots, of two pieces 3
                Some(e) if (15 - e % 16) % 2 == 1 => {
                    crate::fat::tests::put_file(&mut d, COURSES, &std::format!("note {i:02} long.txt"), b"x")
                }
                _ => crate::fat::tests::put_file(&mut d, COURSES, &std::format!("n{i:02}.txt"), b"x"),
            }
        }
        panic!("slot 15 never reached");
    }

    /// The same for a course, a short name and the longest one, then the longest into a folder whose end marker is at
    /// slot 15 (its entry in the next sector): `\Courses` exactly as it was or with the whole course.
    #[test]
    fn a_cut_at_every_write_of_a_course_leaves_the_folder_as_it_was_or_with_the_whole_course() {
        cut_every_write(volume(), &turns(), &course("Zig zag"), Some("course_cut_unlinked"));
        cut_every_write(volume(), &turns(), &course(LONG), None);
        cut_every_write(courses_at_slot_15(), &turns(), &course(LONG), None);
    }

    /// The test volume without `\Courses`: its entry removed ([`Fat::remove`]: its clusters and its files' lost).
    fn no_courses() -> Vec<u8> {
        let mut d = Ram(volume(), 0);
        let at = d.0.windows(11).position(|w| w == b"COURSES    ").expect("the folder's 8.3 entry");
        let mut fat = Fat::mount(&mut d).unwrap();
        let (entry_lba, slot) = ((at / SECTOR) as u32, (at % SECTOR / 32) as u8);
        let h = Handle { entry_lba, slot, dir: fat.root().cluster, ..Handle::default() };
        fat.remove(&mut d, &h).unwrap();
        assert!(fat.open(&mut d, COURSES).is_err());
        d.0
    }

    /// A disk that keeps the sector of every write, in order.
    struct Log(Ram, Vec<u32>);

    impl crate::fat::Disk for Log {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
            self.0.read(lba, buf)
        }
    }

    impl DiskWrite for Log {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
            self.1.extend((0..buf.len().div_ceil(SECTOR) as u32).map(|i| lba + i));
            self.0.write(lba, buf)
        }
    }

    /// The first course on a card without `\Courses` makes it: a cut after every sector leaves the volume clean but
    /// for what it had lost and the file's chain (or the new folder's cluster), never a course in part; the folder's
    /// entry torn every 8 bytes leaves no course and a volume fsck takes (without the orphan check), but for one known
    /// gap (2026-11-08, reported): a tear inside the 8.3 entry after its name and before its cluster leaves a live
    /// `\Courses` on cluster 0 (`Fat::mkdir` writes its entry in one write, fat/write.rs "A create or a folder's
    /// entry is still one write"). Then every later course send answers I/O and writes nothing, until a computer's
    /// chkdsk mends the entry; nothing else on the volume is touched.
    #[test]
    fn a_cut_while_the_first_course_makes_its_folder() {
        let (base, file, to) = (no_courses(), turns(), course("Zig zag"));
        cut_every_write(base.clone(), &file, &to, None);
        let lost0 = fsck(&base).unwrap();
        // the folder's entry: the last write to the root directory (its first cluster: the test volume's root)
        let mut log = Log(Ram(base.clone(), 0), Vec::new());
        send(&mut log, &file, &to).unwrap();
        let root = {
            let fat = Fat::mount(&mut log.0).unwrap();
            let lba = fat.lba(fat.root().cluster);
            lba..lba + fat.cluster_bytes() / SECTOR as u32
        };
        let at = log.1.iter().rposition(|s| root.contains(s)).unwrap() as u32;
        let mut gap = 0;
        for keep in (1..SECTOR).step_by(8) {
            let mut t = crate::fat::write::tests::Torn { ram: Ram(base.clone(), 0), budget: at, keep, cut_at: 0 };
            assert_eq!(send(&mut t, &file, &to), Err(Status::Io), "{keep} bytes");
            assert_eq!(sent(&t.ram.0, &to, &file), None, "{keep} bytes");
            match fsck_torn(&t.ram.0) {
                Ok(lost) => assert!(lost <= lost0 + 1, "{keep} bytes: {lost} lost"),
                Err(e) => {
                    // the known gap: \Courses live on cluster 0, and a send after it refused, nothing written
                    let mut d = Ram(t.ram.0.clone(), 0);
                    let mut fat = Fat::mount(&mut d).unwrap();
                    assert_eq!(fat.open(&mut d, COURSES).err(), Some(Error::Corrupt), "{keep} bytes: {e}");
                    let img = d.0.clone();
                    assert_eq!(send(&mut d, &file, &to), Err(Status::Io), "{keep} bytes");
                    assert!(d.0 == img, "{keep} bytes: nothing written");
                    gap += 1;
                }
            }
        }
        std::println!("torn \\Courses entry: {gap} of 64 tears leave it on cluster 0");
        assert!(gap <= 2, "{gap}");
    }

    /// And for its delete.
    #[test]
    fn a_cut_at_every_write_of_a_course_delete() {
        cut_every_delete(volume(), &turns(), &course("Zig zag"), Some("course_cut_mid_delete"));
        cut_every_delete(volume(), &turns(), &course(LONG), None);
        cut_every_delete(courses_at_slot_15(), &turns(), &course(LONG), None);
    }

    /// The link's sectors torn (a card without reliable write keeps the first bytes of the sector it was writing): the
    /// longest name into a folder at slot 15, its last three writes (the entries in the next sector, the slots left
    /// marked, the entries made live) torn every 8 bytes: fsck passes without the orphan check, the course is whole or
    /// absent, and sent again it is there.
    #[test]
    fn a_torn_link_leaves_the_course_whole_or_absent() {
        let (base, file, to) = (courses_at_slot_15(), turns(), course(LONG));
        let writes = cut_every_write(base.clone(), &file, &to, None);
        let mut torn = 0;
        for at in writes - 3..writes {
            for keep in (1..SECTOR).step_by(8) {
                let mut t = crate::fat::write::tests::Torn { ram: Ram(base.clone(), 0), budget: at, keep, cut_at: 0 };
                assert_eq!(send(&mut t, &file, &to), Err(Status::Io), "write {at}, {keep} bytes");
                let lost = fsck_torn(&t.ram.0).unwrap_or_else(|e| panic!("write {at}, {keep} bytes: {e}"));
                assert!(lost <= chain(&file), "write {at}, {keep} bytes: {lost} lost");
                let got = sent(&t.ram.0, &to, &file);
                assert!(got.as_deref().is_none_or(|g| g == file), "write {at}, {keep} bytes: a part of it");
                torn += 1;
            }
        }
        assert!(torn > 100, "{torn}");
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
        let img = d.0.clone();
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(begin(&mut fat, &mut d, &to, other.len() as u32, T).err(), Some(Status::BadRequest), "at once");
        assert!(d.0 == img, "the temporary file untouched");
        assert_eq!(send(&mut d, &other, &to), Err(Status::BadRequest));
        assert_eq!(sent(&d.0, &to, &file).as_deref(), Some(&file[..]), "kept, once");
        // another case of the same name is the same name on FAT; `_` for a space is the same name in COURSES
        assert_eq!(send(&mut d, &other, &course("COL DU GALIBIER 2")), Err(Status::BadRequest));
        assert_eq!(send(&mut d, &file, &course("Col_du Galibier_2")), Err(Status::BadRequest));
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

    /// Names COURSES would show for another course: a `.gpx` of the same stem in `\Courses`, a `\PlanTrip` route,
    /// `_` against a space, any case, or the first 20 characters of a longer name. Refused at once, nothing written.
    #[test]
    fn a_course_shown_under_another_ones_name_is_refused() {
        let file = turns();
        let mut d = Ram(volume(), 0);
        crate::fat::tests::put_file(&mut d, "PlanTrip", "Route du Col de la Croix.fit", b"route");
        let img = d.0.clone();
        // the card's own: Courses/Hill.gpx, Hill_Garmin.fit, Zigzag.gpx; PlanTrip/Hill.fit
        for n in ["hill", "Zigzag", "Hill Garmin", "HILL_GARMIN", "Route du Col de la C", "Route_du Col de la C"] {
            let mut fat = Fat::mount(&mut d).unwrap();
            assert_eq!(
                begin(&mut fat, &mut d, &course(n), file.len() as u32, T).err(),
                Some(Status::BadRequest),
                "{n}"
            );
            assert_eq!(send(&mut d, &file, &course(n)), Err(Status::BadRequest), "{n}");
        }
        assert!(d.0 == img, "nothing written");
        send(&mut d, &file, &course("Route du Col de la")).unwrap();
        assert_eq!(fsck(&d.0), Ok(0));
    }

    /// Courses that are not, too big, a CRC that lies, a full card: refused, the folder unchanged. A card without
    /// `\Courses`: a refused send leaves the root as it was; the first good one makes the folder.
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
        // no \Courses on the card: a refused send leaves the root as it was, the first course makes the folder
        let at = d.0.windows(11).position(|w| w == b"COURSES    ").expect("the folder's 8.3 entry");
        d.0[at + 6] = b'X'; // its long name no longer matches: not found
        let root = |img: &[u8]| {
            let mut d = Ram(img.to_vec(), 0);
            let mut fat = Fat::mount(&mut d).unwrap();
            let mut c = fat.cursor(fat.root()).unwrap();
            core::iter::from_fn(|| fat.list(&mut d, &mut c).unwrap()).map(|e| e.name().to_vec()).collect::<Vec<_>>()
        };
        let before = root(&d.0);
        assert_eq!(send(&mut d, &one_point, &to), Err(Status::BadRequest));
        assert_eq!(root(&d.0), before, "the root as it was");
        send(&mut d, &file, &to).unwrap();
        assert_eq!(sent(&d.0, &to, &file).as_deref(), Some(&file[..]));
        assert_eq!(root(&d.0).len(), before.len() + 1, "\\Courses made");
        assert_eq!(fsck_torn(&d.0), Ok(0), "the old folder's long name an orphan: this test's doing");
        // a file called Courses: never written over
        let mut d = Ram(volume(), 0);
        d.0[at + 6] = b'X';
        let mut fat = Fat::mount(&mut d).unwrap();
        fat.create(&mut d, fat.root(), COURSES, T).unwrap();
        assert_eq!(send(&mut d, &file, &to), Err(Status::BadRequest));
    }

    /// COURSES lists [`course_file::LIST`] courses of `\PlanTrip` and `\Courses` together: a course past them is FULL
    /// at once (it would never show), the same one sent again still ok. `\Courses` takes [`DAY_MAX`] course files at
    /// most (empty ones count there, not in COURSES) and none once longer than [`WALK`] entries.
    #[test]
    fn a_full_course_list_or_folder_refuses_one_more() {
        let file = turns();
        let mut d = Ram(volume(), 0);
        let mut have = || {
            let mut fat = Fat::mount(&mut d).unwrap();
            course_file::survey(&mut fat, &mut d, "x.fit").unwrap().0
        };
        for i in have()..course_file::LIST - 1 {
            crate::fat::tests::put_file(&mut d, COURSES, &std::format!("C{i}.{}", ["fit", "gpx"][i % 2]), b"x");
        }
        crate::fat::tests::put_file(&mut d, COURSES, "notes.txt", b"x");
        send(&mut d, &file, &course("Last")).unwrap();
        let img = d.0.clone();
        let mut fat = Fat::mount(&mut d).unwrap();
        assert_eq!(begin(&mut fat, &mut d, &course("One more"), 100, T).err(), Some(Status::Full), "at once");
        assert!(d.0 == img, "the temporary file untouched");
        assert_eq!(send(&mut d, &file, &course("One more")), Err(Status::Full));
        assert_eq!(fsck(&d.0), Ok(0));
        send(&mut d, &file, &course("Last")).unwrap();
        // empty course files: not in COURSES, but numbered by the listing
        let mut d = Ram(volume(), 0);
        for i in 0..DAY_MAX {
            crate::fat::tests::put_file(&mut d, COURSES, &std::format!("E{i}.fit"), b"");
        }
        assert_eq!(send(&mut d, &file, &course("Late")), Err(Status::Full));
        // past WALK entries: FULL whatever the count
        let mut d = Ram(volume(), 0);
        for i in 0..=WALK {
            crate::fat::tests::put_file(&mut d, COURSES, &std::format!("n{i}.txt"), b"x");
        }
        assert_eq!(send(&mut d, &file, &course("Late")), Err(Status::Full));
        assert!(is_course(b"A.GPX") && is_course(b"b.fit") && !is_course(b".gpx") && !is_course(b"a.gp"));
    }
}
