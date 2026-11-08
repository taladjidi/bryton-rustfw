//! The file service on the phone bond (roadmap 9, step A1): the phone lists and downloads the rides in
//! `\Activities\` and the planned workouts, and sends or deletes a workout (2026-11-07, the only writes); the unit
//! answers. Pure `no_std` frame formats, encode and decode both ways, with no firmware
//! types in the API, so the phone app's Rust core can reuse this very file.
//!
//! **GATT** (all attributes need the encrypted bond, as the HID ones do). A vendor 128-bit base, random
//! (`aa3c0000-98d8-4772-99ed-dfb117c6746f`, generated for this project); the 16-bit part (bytes 12 and 13 of the
//! little endian form) names the attribute:
//! - service `aa3c0001-...`;
//! - Control `aa3c0002-...`: the phone writes a command (write with response, 20 bytes at the default MTU, up to
//!   [`FRAME_BIG`] on the fast link: WRITE_DATA is the only command that grows);
//! - Data `aa3c0003-...`: the unit notifies the answer (the phone enables its CCCD first).
//!
//! **Sizes:** the ATT MTU stays 23, so a write or a notification carries 20 bytes ([`FRAME_MAX`]); one number to
//! change for a larger MTU, plus the constants derived from it here.
//!
//! **Commands** (Control), the first byte is the command, `seq` a byte the phone chooses and every answer frame
//! echoes, so frames still in flight from an earlier command are told apart:
//! ```text
//! LIST    01 seq                          the rides, one ENTRY each, then END
//! READ    02 seq off[4] name[12]          file `name`.fit from byte `off` (u32 little endian), CHUNKs, then END
//! CANCEL  03                              stops the answer under way, silently
//! LIST_IN 04 seq folder                   the files of a [`Folder`], one ENTRY_IN each, then END
//! READ_IN 05 seq folder off[4] len key[len] (check[2])   file `key` of `folder` from byte `off`, CHUNKs, then END
//! WRITE_BEGIN 06 seq folder day[8] size[4] crc[4]     a workout of `size` bytes for day `day` (YYYYMMDD) and its
//!                                                     CRC-32 (gzip's); END ok once the unit is ready for the data
//! WRITE_BEGIN 06 seq 02 size[4] crc[4] len name[len]  a course of `size` bytes, its CRC-32 and its name (folder 2)
//! WRITE_DATA  07 seq off[4] data[1..]                 the next bytes, in order (`off`: the bytes sent before); no
//!                                                     answer unless refused (an END, and the write is dropped)
//! WRITE_END   08 seq                                  checked (size, CRC, a FIT workout), then made visible; END
//! DELETE      09 seq folder len key[len] check[2]     a listed workout or course removed; END (count 1)
//! PROFILE_GET 0A seq                      the training profile: one PROFILE frame (works during a ride)
//! PROFILE_SET 0B seq profile[9]           the profile set as the menus set it; END (count 1 changed, 0 as held)
//! ```
//! **Profile** (2026-11-08), 9 bytes, little endian: `version` (1), `ftp` u16 (W), `max_hr` u8 (bpm), `lthr` u8
//! (bpm), `weight` u8 (kg), `wprime` u16 (J), `flags` u8 (bit 0: zones from the LTHR, not the max; bit 1: female;
//! the others 0). A SET with another version, a reserved bit or a value outside the menus' ranges is BAD REQUEST;
//! BUSY during a ride or while the card is lent, READ ONLY in a build that may not write the card.
//! **Courses** (2026-11-08, folder 2, `\Courses\`): the key is the file's two-digit number among the folder's `.fit`
//! and `.gpx` files, in directory order, with the check as a workout's. WRITE_BEGIN names the course ([`CourseName`]:
//! 1 to 20 letters, digits, spaces, `-`, `_`), and the unit writes `<name>.fit`: 12 bytes and the name, so 8
//! characters fit the default MTU's 20-byte write and all 20 any larger one. Only a FIT course the unit follows
//! (`course_file::is_course`, checked on the read-back at WRITE_END), at most `workout_write::COURSE_MAX` bytes. A
//! name taken by a file of other bytes is BAD REQUEST (never written over: the phone deletes it first), by the same
//! bytes ok; so is a name COURSES shows for another course (a `.gpx`, a `\PlanTrip` route, `_` for a space), and
//! COURSES full (64 courses) is FULL, both answered at WRITE_BEGIN. The course being followed (or read to follow) is
//! never deleted: BUSY. The first course makes `\Courses`. LIST_IN numbers the first 200 entries of the folder only,
//! as READ_IN and DELETE reach no further.
//! **Writes** (2026-11-07): folder 1 (and 2, above), and only a file the unit reads as a workout ([`crate::storage::workout_file::parse`],
//! at most `FILE_MAX` bytes). The phone never names a path or a file: the unit names it `<CRC-32, 8 hex digits>.fit`
//! in `\Workout\calendar\<day>\` (stock lists the `.fit` files of a day folder, inferred). A WRITE_DATA carries 14 bytes
//! at the default MTU, up to 238 on the fast link. The same file sent again (its answer lost) answers ok, written once.
//! The order of the card writes is storage/workout_write.rs's: nothing shows in the calendar before the whole file
//! was read back from the card and checked.
//! **Folders** (2026-11-07, the workouts step): LIST_IN and READ_IN name one of a small table, never a path.
//! `folder` 0 is `\Activities\` (key: the twelve digits, as above), 1 is `\Workout\calendar\<YYYYMMDD>\` (key: the
//! day's eight digits then the file's two-digit number among that day's `.fit` files, in directory order, 10
//! bytes: a workout's long name never travels, the phone reads the name from the file). More folders take the
//! next numbers (2 is `\Courses\`, below); an unknown one is BAD REQUEST. **The check** (u16 little endian, [`name_check`]: the FIT CRC of the
//! file's name as listed, then of its size) is what ENTRY_IN saw and READ_IN gives back: a workout's key is a place in
//! the directory order, so a folder that changed between the two (a file added or removed, one rewritten) would
//! serve another file under the same key; the unit answers NOT FOUND when the file now at that place has another
//! name or size, never another file. The check is on the wire for the workout and course folders only (a ride's frames
//! would pass the default MTU's 20 bytes with it): `\Activities\` has none, the key is the name. LIST and READ stay as they were, for `\Activities\` only.
//! `name` is the twelve digits of a stock ride name, `YYMMDDhhmmss` (`YYMMDDhhmmss.fit` in `\Activities\`), as
//! LIST gives them. Nothing else of the card is reachable: no path, no other folder.
//! A new command replaces the one under way. A READ resumes a dropped link: ask again from the bytes already kept.
//!
//! **Answers** (Data):
//! ```text
//! ENTRY   81 seq name[12] size[4]         one ride (size in bytes, u32 little endian)
//! ENTRY_IN 84 seq len key[len] (check[2]) size[4]   one file of LIST_IN
//! CHUNK   82 seq data[1..]                   the next bytes of the file, in order from `off`: up to 18, and up
//!                                            to 242 on a link whose ATT MTU grew (frame = MTU - 3, 2026-11-07)
//! END     83 seq status count[4]          the answer is complete; count: entries or bytes sent (u32 little endian)
//! ```
//! Status: 0 ok, 1 not found, 2 busy, 3 bad request, 4 card error; for the writes also 5 read only (a build that may
//! not write the card), 6 mismatch (WRITE_END: the bytes received are not the size or the CRC WRITE_BEGIN gave), 7
//! full. BUSY while a ride runs, is paused or is being saved, or while the card is lent to a computer: the unit never
//! reads or writes under the recorder (a ride starting during a write drops it, BUSY). A bad command gets an
//! END with BAD REQUEST (and the `seq` it carried, 0 if it carried none). END after the last CHUNK is how the phone
//! knows the file is whole: its bytes equal `count`, and `off + count` equals the size LIST gave.
//!
//! **Dropped and moved:** a frame the stack does not take for 5 s (Data not subscribed, the link quiet) drops the
//! answer silently: no frame for 5 s means dropped, re-issue READ from the bytes kept. A ride deleted from HISTORY
//! while it is read ends the answer with BUSY, so the phone checks the bytes it holds against the size LIST gave.
//! END's `count` is only what the stack took, so `off + count` is where to resume.
//! **Who:** the stored bond and an encrypted link are both required. The protection is the pairing itself: Just Works,
//! accepted only during the 60 s PAIR window of the PHONE menu, so whoever pairs then can read the rides (GPS tracks).
//!
//! Notifications are ordered and the link is reliable, so a CHUNK carries no offset; the phone counts.

/// The vendor base, little endian, bytes 12 and 13 (the 16-bit part) zero: what `sd_ble_uuid_vs_add` takes.
pub const BASE: [u8; 16] =
    [0x6f, 0x74, 0xc6, 0x17, 0xb1, 0xdf, 0xed, 0x99, 0x72, 0x47, 0xd8, 0x98, 0x00, 0x00, 0x3c, 0xaa];
/// The service's 16-bit part.
pub const SERVICE: u16 = 1;
/// Control's 16-bit part (the phone writes).
pub const CONTROL: u16 = 2;
/// Data's 16-bit part (the unit notifies).
pub const DATA: u16 = 3;

/// A write or a notification's longest value at the default ATT MTU (23) less its 3-byte header: what a phone that
/// never exchanges the MTU gets, and Control's size.
pub const FRAME_MAX: usize = 20;
/// The longest frame, at the largest ATT MTU (247, `nrf::FAST_MTU`) less its 3-byte header (2026-11-07: the frame
/// formats are unchanged, a CHUNK just carries up to `frame - 2` bytes; the phone takes whatever length arrives).
pub const FRAME_BIG: usize = 244;
/// A ride's name: twelve digits.
pub const NAME_LEN: usize = 12;
/// A ride's name, ASCII digits.
pub type Name = [u8; NAME_LEN];
/// `YYMMDDhhmmss.fit`: the file name of a ride.
pub const FILE_NAME_LEN: usize = NAME_LEN + 4;

/// Longest key of a file in a [`Folder`].
pub const KEY_MAX: usize = 12;

/// The folders LIST_IN and READ_IN can name: a table, so a phone never reaches the rest of the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    /// `\Activities\`: the rides.
    Activities = 0,
    /// `\Workout\calendar\<day>\`: the planned workouts.
    Workout = 1,
    /// `\Courses\`: the courses (routes) the unit follows.
    Courses = 2,
}

impl Folder {
    /// The folder a byte stands for.
    pub fn from_u8(v: u8) -> Option<Folder> {
        match v {
            0 => Some(Folder::Activities),
            1 => Some(Folder::Workout),
            2 => Some(Folder::Courses),
            _ => None,
        }
    }

    /// The bytes of check after a key on the wire: a workout's or a course's key is a place in the directory order, a
    /// ride's the name.
    pub fn check_len(self) -> usize {
        if self == Folder::Activities { 0 } else { 2 }
    }

    /// Whether `k` is a key of this folder: twelve digits (a ride), ten (a day and a number) or two (a course's
    /// number).
    pub fn key_ok(self, k: &[u8]) -> bool {
        let want = match self {
            Folder::Activities => NAME_LEN,
            Folder::Workout => 10,
            Folder::Courses => 2,
        };
        k.len() == want && k.iter().all(u8::is_ascii_digit)
    }
}

/// A file's key in a [`Folder`], checked on creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    len: u8,
    b: [u8; KEY_MAX],
    check: u16,
}

/// The check of a file: its name as listed and its size (see the module doc).
pub fn name_check(name: &[u8], size: u32) -> u16 {
    crate::fit::crc(crate::fit::crc(0, name), &size.to_le_bytes())
}

impl Key {
    /// The key `k` of `folder`, None if it is not one.
    pub fn new(folder: Folder, k: &[u8]) -> Option<Key> {
        let mut b = [0; KEY_MAX];
        b.get_mut(..k.len())?.copy_from_slice(k);
        folder.key_ok(k).then_some(Key { len: k.len() as u8, b, check: 0 })
    }

    /// This key with the check of the file it was listed for.
    pub fn with_check(mut self, check: u16) -> Key {
        self.check = check;
        self
    }

    /// The bytes of check its folder carries on the wire (all but a ride's twelve digits).
    fn check_len(&self) -> usize {
        if usize::from(self.len) == NAME_LEN { 0 } else { 2 }
    }

    /// Its check (0 for a ride).
    pub fn check(&self) -> u16 {
        self.check
    }

    /// Its bytes, ASCII digits.
    pub fn bytes(&self) -> &[u8] {
        self.b.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// Characters of a course's name WRITE_BEGIN carries at most: what the COURSES menu shows
/// (`crate::storage::course_file::NAME`).
pub const COURSE_NAME: usize = 20;

/// A course's name as WRITE_BEGIN carries it (folder 2), checked on creation: the file is `<name>.fit` in `\Courses\`.
/// 1 to [`COURSE_NAME`] ASCII letters, digits, spaces, `-` and `_` (what the COURSES menu shows; it shows `_` as a
/// space), neither first nor last a space, and none of Windows' reserved device names (CON, PRN, AUX, NUL, COM0 to
/// COM9, LPT0 to LPT9, any case: `NUL.fit` is NUL there; learn.microsoft.com, "Naming Files, Paths, and Namespaces").
/// No dot, separator, control character or byte past ASCII, so the name never leaves the folder nor makes another
/// extension, and the FAT writer (its long names are ASCII) takes it whole.
///
/// ```
/// use bryton_rustfw::filesvc::CourseName;
/// assert_eq!(CourseName::new(b"Col du Galibier").unwrap().file_name(), "Col du Galibier.fit");
/// for bad in [&b"../x"[..], b"a.fit", b"", b" a", b"nul", b"Com1", b"caf\xc3\xa9", b"a\tb", b"123456789012345678901"] {
///     assert_eq!(CourseName::new(bad), None, "{bad:?}");
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CourseName {
    len: u8,
    /// The name, then `.fit`.
    b: [u8; COURSE_NAME + 4],
}

impl CourseName {
    /// The name `n`, None if it is not one (see the type).
    pub fn new(n: &[u8]) -> Option<CourseName> {
        let ok = (1..=COURSE_NAME).contains(&n.len())
            && n.iter().all(|&c| c.is_ascii_alphanumeric() || b" -_".contains(&c))
            && n.first() != Some(&b' ')
            && n.last() != Some(&b' ')
            && !reserved(n);
        let mut b = [0; COURSE_NAME + 4];
        b.get_mut(..n.len())?.copy_from_slice(n);
        b.get_mut(n.len()..n.len() + 4)?.copy_from_slice(b".fit");
        ok.then_some(CourseName { len: n.len() as u8, b })
    }

    /// The name, without `.fit`.
    pub fn bytes(&self) -> &[u8] {
        self.b.get(..usize::from(self.len)).unwrap_or(&[])
    }

    /// The file's name, `<name>.fit`.
    pub fn file_name(&self) -> &str {
        core::str::from_utf8(self.b.get(..usize::from(self.len) + 4).unwrap_or(&[])).unwrap_or("")
    }
}

/// A Windows device name (see [`CourseName`]), any case.
fn reserved(n: &[u8]) -> bool {
    let is = |w: &[u8]| n.get(..3).is_some_and(|p| p.eq_ignore_ascii_case(w));
    match n.len() {
        3 => is(b"CON") || is(b"PRN") || is(b"AUX") || is(b"NUL"),
        4 => (is(b"COM") || is(b"LPT")) && n.get(3).is_some_and(u8::is_ascii_digit),
        _ => false,
    }
}

/// How an answer ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Complete.
    Ok = 0,
    /// No such ride.
    NotFound = 1,
    /// A ride is on, or the card is lent: ask again later.
    Busy = 2,
    /// The command is not one, or its offset is past the end of the file.
    BadRequest = 3,
    /// The card did not answer.
    Io = 4,
    /// A write in a build that may not write the card.
    ReadOnly = 5,
    /// WRITE_END: the bytes received are not the size or the CRC-32 WRITE_BEGIN gave.
    Mismatch = 6,
    /// The card has not the room (the recorder's reserve kept).
    Full = 7,
}

impl Status {
    /// The status a byte stands for.
    pub fn from_u8(v: u8) -> Option<Status> {
        Some(match v {
            0 => Status::Ok,
            1 => Status::NotFound,
            2 => Status::Busy,
            3 => Status::BadRequest,
            4 => Status::Io,
            5 => Status::ReadOnly,
            6 => Status::Mismatch,
            7 => Status::Full,
            _ => return None,
        })
    }
}

/// A command the phone writes to Control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// List the rides.
    List {
        /// Echoed by every frame of the answer.
        seq: u8,
    },
    /// Read ride `name` from byte `offset`.
    Read {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// First byte wanted.
        offset: u32,
        /// The ride.
        name: Name,
    },
    /// Stop the answer under way.
    Cancel,
    /// List the files of a folder.
    ListIn {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// Where.
        folder: Folder,
    },
    /// Read file `key` of `folder` from byte `offset`.
    ReadIn {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// Where.
        folder: Folder,
        /// First byte wanted.
        offset: u32,
        /// The file.
        key: Key,
    },
    /// A workout of `size` bytes for `day` follows (folder 1 only).
    WriteBegin {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// Where: [`Folder::Workout`], the only one parsed.
        folder: Folder,
        /// The day, `YYYYMMDD`, ASCII digits.
        day: [u8; 8],
        /// Bytes of the file.
        size: u32,
        /// CRC-32 of the whole file (gzip's, [`crate::inflate::crc32`]).
        crc: u32,
    },
    /// A course of `size` bytes named `name` follows (folder 2).
    WriteCourse {
        /// Echoed by every frame of the answer.
        seq: u8,
        /// Bytes of the file.
        size: u32,
        /// CRC-32 of the whole file (gzip's, [`crate::inflate::crc32`]).
        crc: u32,
        /// Its name: the file is `<name>.fit` in `\Courses\`.
        name: CourseName,
    },
    /// The next bytes of the file being written.
    WriteData {
        /// Echoed by an END.
        seq: u8,
        /// Bytes sent before these.
        offset: u32,
        /// The bytes, one or more.
        data: &'a [u8],
    },
    /// The file is whole: checked, then made visible.
    WriteEnd {
        /// Echoed by the END.
        seq: u8,
    },
    /// The training profile is wanted.
    ProfileGet {
        /// Echoed by the PROFILE frame.
        seq: u8,
    },
    /// Sets the training profile.
    ProfileSet {
        /// Echoed by the END.
        seq: u8,
        /// The new values.
        profile: Profile,
    },
    /// Deletes file `key` of `folder` (folder 1 or 2; the key carries its check).
    Delete {
        /// Echoed by the END.
        seq: u8,
        /// Where: [`Folder::Workout`] or [`Folder::Courses`], the only ones parsed.
        folder: Folder,
        /// The file, as listed.
        key: Key,
    },
}

const LIST: u8 = 1;
const READ: u8 = 2;
const CANCEL: u8 = 3;
const LIST_IN: u8 = 4;
const READ_IN: u8 = 5;
const WRITE_BEGIN: u8 = 6;
const WRITE_DATA: u8 = 7;
const WRITE_END: u8 = 8;
const DELETE: u8 = 9;
const PROFILE_GET: u8 = 10;
const PROFILE_SET: u8 = 11;
const PROFILE: u8 = 0x85;
/// The profile layout's version byte.
pub const PROFILE_VERSION: u8 = 1;
/// A profile's bytes after the version: ftp 2, max HR, LTHR, weight, W' 2, flags.
const PROFILE_LEN: usize = 8;

/// The training profile of PROFILE_GET / PROFILE_SET (module doc). Ranges are the unit's to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// FTP, W.
    pub ftp: u16,
    /// Maximum heart rate, bpm.
    pub max_hr: u8,
    /// Lactate threshold heart rate, bpm.
    pub lthr: u8,
    /// Rider weight, kg.
    pub weight_kg: u8,
    /// W', J.
    pub wprime_j: u16,
    /// Heart-rate zones come from the LTHR, not the maximum.
    pub zones_lthr: bool,
    /// Female (TRIMP's constant).
    pub female: bool,
}

impl Profile {
    /// The 9 bytes, version first.
    fn bytes(&self) -> [u8; PROFILE_LEN + 1] {
        let (f, w) = (self.ftp.to_le_bytes(), self.wprime_j.to_le_bytes());
        let flags = u8::from(self.zones_lthr) | u8::from(self.female) << 1;
        [PROFILE_VERSION, f[0], f[1], self.max_hr, self.lthr, self.weight_kg, w[0], w[1], flags]
    }

    /// From the 9 bytes; None for another version or a reserved flag bit.
    fn from_bytes(p: &[u8]) -> Option<Profile> {
        let [v, f0, f1, max_hr, lthr, weight_kg, w0, w1, flags] = *<&[u8; PROFILE_LEN + 1]>::try_from(p).ok()?;
        (v == PROFILE_VERSION && flags & !3 == 0).then_some(Profile {
            ftp: u16::from_le_bytes([f0, f1]),
            max_hr,
            lthr,
            weight_kg,
            wprime_j: u16::from_le_bytes([w0, w1]),
            zones_lthr: flags & 1 != 0,
            female: flags & 2 != 0,
        })
    }
}
const ENTRY: u8 = 0x81;
const CHUNK: u8 = 0x82;
const END: u8 = 0x83;
const ENTRY_IN: u8 = 0x84;

/// `v` at `at` in `out`; the end of what is written (unchanged when it does not fit: never, for the frames here).
fn put(out: &mut [u8], at: usize, v: &[u8]) -> usize {
    match out.get_mut(at..at + v.len()) {
        Some(d) => {
            d.copy_from_slice(v);
            at + v.len()
        }
        None => at,
    }
}

/// Four bytes at `at` as a little endian number.
fn u16_at(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(p.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(p: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?))
}

/// Twelve ASCII digits.
pub fn is_name(b: &[u8]) -> bool {
    b.len() == NAME_LEN && b.iter().all(u8::is_ascii_digit)
}

/// The ride name of a file called `YYMMDDhhmmss.fit` (any case), else None: stock's `tmp.fit` and anything else are
/// not rides here.
///
/// ```
/// use bryton_rustfw::filesvc::ride_name;
/// assert_eq!(ride_name(b"251026093000.fit"), Some(*b"251026093000"));
/// assert_eq!(ride_name(b"251026093000.FIT"), Some(*b"251026093000"));
/// assert_eq!(ride_name(b"tmp.fit"), None);
/// assert_eq!(ride_name(b"251026093000.gpx"), None);
/// ```
pub fn ride_name(file: &[u8]) -> Option<Name> {
    let (name, ext) = (file.get(..NAME_LEN)?, file.get(NAME_LEN..)?);
    (is_name(name) && ext.eq_ignore_ascii_case(b".fit")).then(|| {
        let mut n = [0; NAME_LEN];
        n.copy_from_slice(name);
        n
    })
}

/// The file name of ride `name`, `YYMMDDhhmmss.fit`, in `out`.
///
/// ```
/// use bryton_rustfw::filesvc::file_name;
/// let mut buf = [0; 16];
/// assert_eq!(file_name(b"251026093000", &mut buf), Some("251026093000.fit"));
/// assert_eq!(file_name(b"../../ble.bin", &mut buf), None);
/// ```
pub fn file_name<'a>(name: &[u8], out: &'a mut [u8; FILE_NAME_LEN]) -> Option<&'a str> {
    if !is_name(name) {
        return None;
    }
    let (a, b) = out.split_at_mut(NAME_LEN);
    a.copy_from_slice(name);
    b.copy_from_slice(b".fit");
    core::str::from_utf8(out).ok()
}

impl<'a> Command<'a> {
    /// A command from what was written, or the `seq` it carried (0: none) and why it is refused.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::{Command, Status};
    /// assert_eq!(Command::parse(&[1, 7]), Ok(Command::List { seq: 7 }));
    /// assert_eq!(Command::parse(&[3]), Ok(Command::Cancel));
    /// assert_eq!(Command::parse(&[9, 7]), Err((7, Status::BadRequest)));
    /// assert_eq!(Command::parse(&[2, 7, 0, 0, 0, 0, b'x']), Err((7, Status::BadRequest)));
    /// ```
    pub fn parse(p: &'a [u8]) -> Result<Command<'a>, (u8, Status)> {
        let seq = p.get(1).copied().unwrap_or(0);
        let bad = Err((seq, Status::BadRequest));
        match p.first() {
            Some(&LIST) if p.len() == 2 => Ok(Command::List { seq }),
            Some(&CANCEL) if p.len() == 1 => Ok(Command::Cancel),
            Some(&READ) if p.len() == 6 + NAME_LEN => match (u32_at(p, 2), p.get(6..)) {
                (Some(offset), Some(n)) if is_name(n) => {
                    let mut name = [0; NAME_LEN];
                    name.copy_from_slice(n);
                    Ok(Command::Read { seq, offset, name })
                }
                _ => bad,
            },
            Some(&LIST_IN) if p.len() == 3 => match p.get(2).copied().and_then(Folder::from_u8) {
                Some(folder) => Ok(Command::ListIn { seq, folder }),
                None => bad,
            },
            Some(&READ_IN) if p.len() >= 8 => {
                let (folder, offset) = (p.get(2).copied().and_then(Folder::from_u8), u32_at(p, 3));
                let l = usize::from(p.get(7).copied().unwrap_or(0));
                let cl = folder.map_or(0, Folder::check_len);
                let key = p.get(8..8 + l).filter(|_| p.len() == 8 + l + cl);
                let check = if cl == 0 { Some(0) } else { u16_at(p, 8 + l) };
                match (folder, offset, key.and_then(|k| Key::new(folder?, k)), check) {
                    (Some(folder), Some(offset), Some(key), Some(check)) => {
                        Ok(Command::ReadIn { seq, folder, offset, key: key.with_check(check) })
                    }
                    _ => bad,
                }
            }
            Some(&WRITE_BEGIN) if p.len() == 19 && p.get(2) == Some(&(Folder::Workout as u8)) => {
                match (p.get(3..11).and_then(|d| <[u8; 8]>::try_from(d).ok()), u32_at(p, 11), u32_at(p, 15)) {
                    (Some(day), Some(size), Some(crc)) if day.iter().all(u8::is_ascii_digit) => {
                        Ok(Command::WriteBegin { seq, folder: Folder::Workout, day, size, crc })
                    }
                    _ => bad,
                }
            }
            Some(&WRITE_BEGIN) if p.get(2) == Some(&(Folder::Courses as u8)) => {
                let l = usize::from(p.get(11).copied().unwrap_or(0));
                let name = p.get(12..).filter(|n| n.len() == l).and_then(CourseName::new);
                match (u32_at(p, 3), u32_at(p, 7), name) {
                    (Some(size), Some(crc), Some(name)) => Ok(Command::WriteCourse { seq, size, crc, name }),
                    _ => bad,
                }
            }
            Some(&WRITE_DATA) if (7..=FRAME_BIG).contains(&p.len()) => match (u32_at(p, 2), p.get(6..)) {
                (Some(offset), Some(data)) => Ok(Command::WriteData { seq, offset, data }),
                _ => bad,
            },
            Some(&WRITE_END) if p.len() == 2 => Ok(Command::WriteEnd { seq }),
            Some(&PROFILE_GET) if p.len() == 2 => Ok(Command::ProfileGet { seq }),
            Some(&PROFILE_SET) if p.len() == 2 + PROFILE_LEN + 1 => match p.get(2..).and_then(Profile::from_bytes) {
                Some(profile) => Ok(Command::ProfileSet { seq, profile }),
                None => bad,
            },
            Some(&DELETE) => {
                let folder = p.get(2).copied().and_then(Folder::from_u8).filter(|&f| f != Folder::Activities);
                let l = usize::from(p.get(3).copied().unwrap_or(0));
                let key = p.get(4..4 + l).filter(|_| p.len() == 6 + l).zip(folder).and_then(|(k, f)| Key::new(f, k));
                match (folder, key, u16_at(p, 4 + l)) {
                    (Some(folder), Some(key), Some(check)) => {
                        Ok(Command::Delete { seq, folder, key: key.with_check(check) })
                    }
                    _ => bad,
                }
            }
            _ => bad,
        }
    }

    /// The bytes to write for this command; their count.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::Command;
    /// let c = Command::Read { seq: 3, offset: 0x0102_0304, name: *b"251026093000" };
    /// let mut f = [0; 20];
    /// let n = c.encode(&mut f);
    /// assert_eq!(Command::parse(&f[..n]), Ok(c));
    /// ```
    pub fn encode(&self, out: &mut [u8]) -> usize {
        match self {
            Command::List { seq } => put(out, 0, &[LIST, *seq]),
            Command::Cancel => put(out, 0, &[CANCEL]),
            Command::Read { seq, offset, name } => {
                let n = put(out, 0, &[READ, *seq]);
                let n = put(out, n, &offset.to_le_bytes());
                put(out, n, name)
            }
            Command::ListIn { seq, folder } => put(out, 0, &[LIST_IN, *seq, *folder as u8]),
            Command::ReadIn { seq, folder, offset, key } => {
                let n = put(out, 0, &[READ_IN, *seq, *folder as u8]);
                let n = put(out, n, &offset.to_le_bytes());
                let n = put(out, n, &[key.len]);
                let n = put(out, n, key.bytes());
                put(out, n, key.check.to_le_bytes().get(..folder.check_len()).unwrap_or(&[]))
            }
            Command::WriteBegin { seq, folder, day, size, crc } => {
                let n = put(out, 0, &[WRITE_BEGIN, *seq, *folder as u8]);
                let n = put(out, n, day);
                let n = put(out, n, &size.to_le_bytes());
                put(out, n, &crc.to_le_bytes())
            }
            Command::WriteCourse { seq, size, crc, name } => {
                let n = put(out, 0, &[WRITE_BEGIN, *seq, Folder::Courses as u8]);
                let n = put(out, n, &size.to_le_bytes());
                let n = put(out, n, &crc.to_le_bytes());
                let n = put(out, n, &[name.len]);
                put(out, n, name.bytes())
            }
            Command::WriteData { seq, offset, data } => {
                let n = put(out, 0, &[WRITE_DATA, *seq]);
                let n = put(out, n, &offset.to_le_bytes());
                put(out, n, data.get(..data.len().min(out.len().saturating_sub(6))).unwrap_or(&[]))
            }
            Command::WriteEnd { seq } => put(out, 0, &[WRITE_END, *seq]),
            Command::ProfileGet { seq } => put(out, 0, &[PROFILE_GET, *seq]),
            Command::ProfileSet { seq, profile } => {
                let n = put(out, 0, &[PROFILE_SET, *seq]);
                put(out, n, &profile.bytes())
            }
            Command::Delete { seq, folder, key } => {
                let n = put(out, 0, &[DELETE, *seq, *folder as u8, key.len]);
                let n = put(out, n, key.bytes());
                put(out, n, &key.check.to_le_bytes())
            }
        }
    }
}

/// A frame the unit notifies on Data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// One ride of a LIST.
    Entry {
        /// The command's.
        seq: u8,
        /// The ride.
        name: Name,
        /// Its size, bytes.
        size: u32,
    },
    /// One file of a LIST_IN.
    EntryIn {
        /// The command's.
        seq: u8,
        /// The file.
        key: Key,
        /// Its size, bytes.
        size: u32,
    },
    /// Bytes of a READ.
    Chunk {
        /// The command's.
        seq: u8,
        /// Up to [`FRAME_BIG`] less 2 bytes (18 at the default MTU).
        data: &'a [u8],
    },
    /// The answer to a PROFILE_GET.
    Profile {
        /// The command's.
        seq: u8,
        /// The unit's profile.
        profile: Profile,
    },
    /// The answer is over.
    End {
        /// The command's.
        seq: u8,
        /// How it went.
        status: Status,
        /// Entries or bytes sent in the answer.
        count: u32,
    },
}

impl<'a> Frame<'a> {
    /// The bytes to notify for this frame; their count (a CHUNK's data is cut to fit `out`).
    ///
    /// ```
    /// use bryton_rustfw::filesvc::{Frame, Status};
    /// let mut f = [0; 20];
    /// let n = Frame::End { seq: 2, status: Status::Busy, count: 0 }.encode(&mut f);
    /// assert_eq!(&f[..n], &[0x83, 2, 2, 0, 0, 0, 0]);
    /// ```
    pub fn encode(&self, out: &mut [u8]) -> usize {
        match self {
            Frame::Entry { seq, name, size } => {
                let n = put(out, 0, &[ENTRY, *seq]);
                let n = put(out, n, name);
                put(out, n, &size.to_le_bytes())
            }
            Frame::EntryIn { seq, key, size } => {
                let n = put(out, 0, &[ENTRY_IN, *seq, key.len]);
                let n = put(out, n, key.bytes());
                let n = put(out, n, key.check.to_le_bytes().get(..key.check_len()).unwrap_or(&[]));
                put(out, n, &size.to_le_bytes())
            }
            Frame::Chunk { seq, data } => {
                let n = put(out, 0, &[CHUNK, *seq]);
                put(out, n, data.get(..data.len().min(out.len().saturating_sub(2))).unwrap_or(&[]))
            }
            Frame::Profile { seq, profile } => {
                let n = put(out, 0, &[PROFILE, *seq]);
                put(out, n, &profile.bytes())
            }
            Frame::End { seq, status, count } => {
                let n = put(out, 0, &[END, *seq, *status as u8]);
                put(out, n, &count.to_le_bytes())
            }
        }
    }

    /// A frame from a notification's value, None if it is not one.
    ///
    /// ```
    /// use bryton_rustfw::filesvc::Frame;
    /// assert_eq!(Frame::parse(&[0x82, 1, 9, 8]), Some(Frame::Chunk { seq: 1, data: &[9, 8] }));
    /// assert_eq!(Frame::parse(&[0x82, 1]), None);
    /// assert_eq!(Frame::parse(&[0x55]), None);
    /// ```
    pub fn parse(p: &'a [u8]) -> Option<Frame<'a>> {
        let seq = *p.get(1)?;
        match *p.first()? {
            ENTRY if p.len() == 2 + NAME_LEN + 4 => {
                let mut name = [0; NAME_LEN];
                name.copy_from_slice(p.get(2..2 + NAME_LEN)?);
                is_name(&name).then_some(Frame::Entry { seq, name, size: u32_at(p, 2 + NAME_LEN)? })
            }
            ENTRY_IN if p.len() > 3 => {
                // the key's length says which folder's: twelve, two or ten digits
                let l = usize::from(*p.get(2)?);
                let folder = match l {
                    NAME_LEN => Folder::Activities,
                    2 => Folder::Courses,
                    _ => Folder::Workout,
                };
                let cl = folder.check_len();
                if p.len() != 7 + l + cl {
                    return None;
                }
                let mut key = Key::new(folder, p.get(3..3 + l)?)?;
                if cl > 0 {
                    key = key.with_check(u16_at(p, 3 + l)?);
                }
                Some(Frame::EntryIn { seq, key, size: u32_at(p, 3 + l + cl)? })
            }
            CHUNK if (3..=FRAME_BIG).contains(&p.len()) => Some(Frame::Chunk { seq, data: p.get(2..)? }),
            PROFILE if p.len() == 2 + PROFILE_LEN + 1 => {
                Some(Frame::Profile { seq, profile: Profile::from_bytes(p.get(2..)?)? })
            }
            END if p.len() == 7 => Some(Frame::End { seq, status: Status::from_u8(*p.get(2)?)?, count: u32_at(p, 3)? }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    const PROFILE_SAMPLE: Profile =
        Profile { ftp: 265, max_hr: 186, lthr: 170, weight_kg: 68, wprime_j: 20_000, zones_lthr: true, female: false };

    #[test]
    fn the_profile_commands_round_trip_and_odd_ones_are_refused() {
        let mut f = [0; FRAME_MAX];
        let set = Command::ProfileSet { seq: 5, profile: PROFILE_SAMPLE };
        let n = set.encode(&mut f);
        assert_eq!(&f[..n], &[11, 5, 1, 9, 1, 186, 170, 68, 0x20, 0x4e, 1]);
        assert_eq!(Command::parse(&f[..n]), Ok(set));
        let get = Command::ProfileGet { seq: 6 };
        let n = get.encode(&mut f);
        assert_eq!(Command::parse(&f[..n]), Ok(get));
        let bad = Err((5, Status::BadRequest));
        let mut g = [11, 5, 1, 9, 1, 186, 170, 68, 0x20, 0x4e, 1];
        assert_eq!(Command::parse(&g[..10]), bad, "short");
        g[2] = 2;
        assert_eq!(Command::parse(&g), bad, "another version");
        g[2] = 1;
        g[10] = 4;
        assert_eq!(Command::parse(&g), bad, "a reserved flag");
        assert_eq!(Command::parse(&[10, 5, 0]), bad, "GET with a tail");
    }

    #[test]
    fn every_frame_round_trips() {
        let mut f = [0; FRAME_MAX];
        let data = [7u8; FRAME_MAX - 2];
        for fr in [
            Frame::Entry { seq: 1, name: *b"251026093000", size: 123_456 },
            Frame::Chunk { seq: 2, data: &data },
            Frame::Chunk { seq: 2, data: &data[..1] },
            Frame::End { seq: 3, status: Status::Io, count: u32::MAX },
            Frame::Profile { seq: 4, profile: PROFILE_SAMPLE },
        ] {
            let n = fr.encode(&mut f);
            assert_eq!(Frame::parse(&f[..n]), Some(fr));
        }
    }

    #[test]
    fn a_long_chunk_is_cut_and_a_bad_frame_refused() {
        let mut f = [0; FRAME_MAX];
        assert_eq!(Frame::Chunk { seq: 0, data: &[1; 40] }.encode(&mut f), FRAME_MAX);
        let mut big = [0; FRAME_BIG];
        let data = [5u8; 300];
        let n = Frame::Chunk { seq: 1, data: &data }.encode(&mut big);
        assert_eq!((n, Frame::parse(&big[..n]).is_some()), (FRAME_BIG, true));
        assert_eq!(Frame::parse(&[0x83, 0, 9, 0, 0, 0, 0]), None, "an unknown status");
        assert_eq!(Frame::parse(&[]), None);
    }

    #[test]
    fn folder_commands_round_trip_and_refuse_paths() {
        let key = Key::new(Folder::Workout, b"2026110101").unwrap().with_check(name_check(b"A.FIT", 900));
        let mut f = [0; FRAME_MAX];
        for c in [
            Command::ListIn { seq: 4, folder: Folder::Workout },
            Command::ReadIn { seq: 5, folder: Folder::Workout, offset: 77, key },
            Command::ReadIn {
                seq: 6,
                folder: Folder::Activities,
                offset: 0,
                key: Key::new(Folder::Activities, b"251026093000").unwrap(),
            },
        ] {
            let n = c.encode(&mut f);
            assert_eq!(Command::parse(&f[..n]), Ok(c));
        }
        let n = Frame::EntryIn { seq: 1, key, size: 900 }.encode(&mut f);
        assert_eq!(Frame::parse(&f[..n]), Some(Frame::EntryIn { seq: 1, key, size: 900 }));
        assert!(n <= FRAME_MAX, "an entry fits the default MTU");
        let n = Command::ReadIn { seq: 5, folder: Folder::Workout, offset: 0, key }.encode(&mut f);
        assert_eq!((n, &f[n - 2..n]), (20, &key.check().to_le_bytes()[..]), "the check ends READ_IN, 20 bytes");
        assert_eq!(Command::parse(&f[..n - 1]), Err((5, Status::BadRequest)), "a READ_IN without its check");
        let bad = Err((7, Status::BadRequest));
        assert_eq!(Command::parse(&[4, 7, 9]), bad, "unknown folder");
        assert_eq!(
            Command::parse(&[5, 7, 1, 0, 0, 0, 0, 10, b'.', b'.', b'/', b'x', b'x', b'x', b'x', b'x', b'x', b'x']),
            bad
        );
        assert_eq!(Command::parse(&[5, 7, 1, 0, 0, 0, 0, 12, b'1', b'1']), bad, "length lies");
        assert_eq!(Command::parse(b"\x05\x07\x01\0\0\0\0\x0c251026093000"), bad, "a ride's key in the workout folder");
    }

    #[test]
    fn write_commands_round_trip_fit_both_mtus_and_refuse_other_folders() {
        let key = Key::new(Folder::Workout, b"2026110101").unwrap().with_check(0xbeef);
        let data = [9u8; 300];
        let mut f = [0; FRAME_BIG];
        let begin = Command::WriteBegin { seq: 1, folder: Folder::Workout, day: *b"20261108", size: 900, crc: 0xdead };
        for (c, len) in [
            (begin, 19),
            (Command::WriteData { seq: 2, offset: 7, data: &data[..14] }, FRAME_MAX),
            (Command::WriteData { seq: 2, offset: 7, data: &data[..FRAME_BIG - 6] }, FRAME_BIG),
            (Command::WriteEnd { seq: 3 }, 2),
            (Command::Delete { seq: 4, folder: Folder::Workout, key }, 16),
        ] {
            let n = c.encode(&mut f);
            assert_eq!((n, Command::parse(&f[..n])), (len, Ok(c)));
        }
        let n = Command::WriteData { seq: 2, offset: 7, data: &data }.encode(&mut f);
        assert_eq!(n, FRAME_BIG, "cut to the frame");
        let bad = Err((7, Status::BadRequest));
        assert_eq!(Command::parse(b"\x06\x07\x0020261108\x01\0\0\0\0\0\0\0"), bad, "rides are never written");
        assert_eq!(Command::parse(b"\x06\x07\x012026110x\x01\0\0\0\0\0\0\0"), bad, "a day of digits only");
        assert_eq!(Command::parse(b"\x06\x07\x0120261108\x01\0\0\0\0\0\0"), bad, "short");
        assert_eq!(Command::parse(b"\x07\x07\0\0\0\0"), bad, "WRITE_DATA without data");
        assert_eq!(Command::parse(&[7; FRAME_BIG + 1]), bad, "longer than the largest frame");
        assert_eq!(Command::parse(b"\x08\x07\x00"), bad, "WRITE_END with more");
        assert_eq!(Command::parse(b"\x09\x07\x00\x0c251026093000\0\0"), bad, "a ride is never deleted");
        assert_eq!(Command::parse(b"\x09\x07\x00\x0a2026110101\0\0"), bad, "nor anything in folder 0");
        assert_eq!(Command::parse(b"\x09\x07\x01\x0a2026110101\0"), bad, "DELETE without its check");
        assert_eq!(Command::parse(b"\x09\x07\x01\x0a../../ble.b\0\0"), bad, "a path");
        for v in 5..=7 {
            assert_eq!(Status::from_u8(v).map(|s| s as u8), Some(v));
        }
    }

    /// Folder 2: LIST_IN, READ_IN, DELETE and an ENTRY_IN of a course round-trip; WRITE_BEGIN carries a name, 8
    /// characters within the default MTU's 20 bytes and 20 within any larger one; names that leave the folder, make
    /// another extension or are no ASCII are refused.
    #[test]
    fn course_commands_round_trip_and_refuse_bad_names() {
        let key = Key::new(Folder::Courses, b"07").unwrap().with_check(0x1234);
        let name = |n: &[u8]| CourseName::new(n).unwrap();
        let mut f = [0; FRAME_BIG];
        for (c, len) in [
            (Command::ListIn { seq: 1, folder: Folder::Courses }, 3),
            (Command::ReadIn { seq: 2, folder: Folder::Courses, offset: 5, key }, 12),
            (Command::Delete { seq: 3, folder: Folder::Courses, key }, 8),
            (Command::WriteCourse { seq: 4, size: 900, crc: 0xdead, name: name(b"Galibier") }, FRAME_MAX),
            (Command::WriteCourse { seq: 5, size: 1, crc: 0, name: name(b"Col du Galibier 2-b_") }, 32),
        ] {
            let n = c.encode(&mut f);
            assert_eq!((n, Command::parse(&f[..n])), (len, Ok(c)));
        }
        let n = Frame::EntryIn { seq: 1, key, size: 900 }.encode(&mut f);
        assert_eq!(Frame::parse(&f[..n]), Some(Frame::EntryIn { seq: 1, key, size: 900 }));
        assert_eq!(name(b"a b").file_name(), "a b.fit");
        let bad = Err((7, Status::BadRequest));
        let begin = |n: &[u8]| {
            let mut v = std::vec![WRITE_BEGIN, 7, 2, 1, 0, 0, 0, 0, 0, 0, 0, n.len() as u8];
            v.extend_from_slice(n);
            v
        };
        assert!(Command::parse(&begin(b"ok")).is_ok());
        for n in [
            &b""[..],
            b"../a",
            b"a/b",
            b"a\\b",
            b"x.gpx",
            b".",
            b"a\0b",
            b"a\nb",
            b"caf\xc3\xa9",
            b" lead",
            b"trail ",
            b"CON",
            b"prn",
            b"Aux",
            b"nul",
            b"COM1",
            b"lpt9",
            b"a:b",
            b"a*b",
            b"123456789012345678901",
        ] {
            assert_eq!(Command::parse(&begin(n)), bad, "{n:?}");
        }
        assert!(
            CourseName::new(b"CONS").is_some()
                && CourseName::new(b"COM").is_some()
                && CourseName::new(b"LPTX").is_some()
        );
        let mut lies = begin(b"abc");
        lies[11] = 4;
        assert_eq!(Command::parse(&lies), bad, "the length lies");
        assert_eq!(Command::parse(&lies[..11]), bad, "no name");
        assert_eq!(Command::parse(b"\x09\x07\x02\x0a2026110101\0\0"), bad, "a workout's key in the course folder");
        assert_eq!(Command::parse(b"\x09\x07\x03\x0207\0\0"), bad, "no folder 3");
        assert_eq!(Command::parse(b"\x09\x07\x02\x0207\0"), bad, "DELETE without its check");
        assert_eq!(Command::parse(b"\x05\x07\x02\0\0\0\0\x0207\0"), bad, "READ_IN without its check");
        for folder in [0, 3] {
            let mut other = begin(b"ok");
            other[2] = folder;
            assert_eq!(Command::parse(&other), bad, "a course's WRITE_BEGIN in folder {folder}");
        }
        // a ride's ENTRY_IN (twelve digits, no check) still parses as one
        let ride = Key::new(Folder::Activities, b"251026093000").unwrap();
        let n = Frame::EntryIn { seq: 1, key: ride, size: 9 }.encode(&mut f);
        assert_eq!((n, Frame::parse(&f[..n])), (19, Some(Frame::EntryIn { seq: 1, key: ride, size: 9 })));
    }

    #[test]
    fn the_base_keeps_its_16_bit_part_free() {
        assert_eq!(&BASE[12..14], &[0, 0]);
    }
}
