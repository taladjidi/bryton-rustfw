//! The phone's side of the unit's file service (src/link/filesvc.rs): a sans-IO state machine. Swift writes the bytes
//! it returns to Control and feeds every Data notification to [`FileClient::on_data`]; frames are encoded and decoded
//! by the firmware's own `filesvc` (no second copy of the format).
//!
//! Chunk sizes are never assumed: each CHUNK says its own length. The whole file is kept in memory, so a dropped
//! READ (BUSY, a link drop, 5 s of silence) resumes with [`FileClient::resume_command`] from the bytes kept.

use bryton_rustfw::filesvc::{Command, FRAME_BIG, FRAME_MAX, Folder, Frame, Key, NAME_LEN, Profile, Status};
use std::sync::Mutex;

/// No frame for this long while an answer is awaited means the unit dropped it (filesvc.rs, "Dropped and moved").
pub const SILENCE_MS: u64 = 5_000;
/// Largest file accepted (a ride is a few MB at most): a hostile LIST cannot make the app allocate without bound.
const FILE_MAX: usize = 64 << 20;
/// Most rides kept from one LIST.
/// The largest workout the unit takes (src/storage/workout_write.rs, 8 KB).
const WRITE_MAX: usize = bryton_rustfw::storage::workout_file::FILE_MAX;
/// WRITE_DATA's header: kind, seq, offset.
const DATA_HEAD: usize = 6;
const ENTRIES_MAX: usize = 4096;

/// One ride of the unit's LIST.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RideEntry {
    /// `YYMMDDhhmmss`.
    pub name: String,
    /// Bytes.
    pub size: u32,
}

/// A folder of the unit LIST_IN and READ_IN can name (src/link/filesvc.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum UnitFolder {
    /// The rides, `\Activities\`.
    Activities,
    /// The planned workouts, `\Workout\calendar\<day>\`: the key is `YYYYMMDD` and the file's number.
    Workout,
}

impl From<UnitFolder> for Folder {
    fn from(f: UnitFolder) -> Folder {
        match f {
            UnitFolder::Activities => Folder::Activities,
            UnitFolder::Workout => Folder::Workout,
        }
    }
}

/// The unit's training profile (src/link/filesvc.rs "Profile"): what PROFILE_GET gives and PROFILE_SET takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UnitProfile {
    /// FTP, W (50 to 600).
    pub ftp: u16,
    /// Maximum heart rate, bpm (100 to 230).
    pub max_hr: u8,
    /// Lactate threshold heart rate, bpm (100 to 230).
    pub lthr: u8,
    /// Rider weight, kg (30 to 150).
    pub weight_kg: u8,
    /// W', J (5000 to 40000).
    pub wprime_j: u16,
    /// The heart-rate zones come from the LTHR, not the maximum.
    pub zones_lthr: bool,
    /// Female (TRIMP's constant).
    pub female: bool,
}

impl From<Profile> for UnitProfile {
    fn from(p: Profile) -> Self {
        UnitProfile {
            ftp: p.ftp,
            max_hr: p.max_hr,
            lthr: p.lthr,
            weight_kg: p.weight_kg,
            wprime_j: p.wprime_j,
            zones_lthr: p.zones_lthr,
            female: p.female,
        }
    }
}

impl From<UnitProfile> for Profile {
    fn from(p: UnitProfile) -> Self {
        Profile {
            ftp: p.ftp,
            max_hr: p.max_hr,
            lthr: p.lthr,
            weight_kg: p.weight_kg,
            wprime_j: p.wprime_j,
            zones_lthr: p.zones_lthr,
            female: p.female,
        }
    }
}

/// Why a list or a download failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FileError {
    /// The unit has no such ride.
    NotFound,
    /// The unit refused the command.
    BadRequest,
    /// The unit's card did not answer.
    Card,
    /// More bytes than LIST's size, or a ride too large.
    TooLong,
    /// The answer ended OK with fewer bytes than LIST's size (a short file, never a silent truncation).
    Short,
    /// END's count differs from the bytes received in the answer.
    CountMismatch,
    /// A write was refused: this build of the unit does not write its card (the RAM run's).
    ReadOnly,
    /// A write was refused: the day already holds as many workouts as the unit keeps.
    Full,
    /// A write was refused: the unit received other bytes than it was told (size or CRC). Send it again.
    Mismatch,
    /// WRITE_BEGIN found no `\Workout\calendar` on the card: the unit has no workout calendar yet.
    NoCalendar,
    /// A write or delete was refused: the unit is riding or its card is lent to a computer.
    Busy,
}

/// What a frame did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FileEvent {
    /// Not a frame, a stale one or nothing awaited: dropped.
    Ignored,
    /// A ride was added to the list.
    Entry,
    /// The list is complete.
    Listed,
    /// File bytes arrived.
    Progress,
    /// The file is whole and checked.
    Finished,
    /// The unit is busy (riding, USB): [`FileClient::resume_command`] later.
    Busy,
    /// The answer ended OK but the file is not whole yet: [`FileClient::resume_command`] at once. A resume that
    /// brings no byte fails as [`FileError::Short`], so a short file is never taken for whole.
    Partial,
    /// WRITE_BEGIN was accepted: send the data with [`FileClient::write_next_command`].
    WriteReady,
    /// The unit has the workout, checked and visible.
    Written,
    /// The unit removed the workout.
    Deleted,
    /// The unit's profile arrived: [`FileClient::profile`].
    Profile,
    /// The unit took the profile (it held other values), or already held it.
    ProfileSet,
    /// The transfer failed.
    Failed {
        /// Why.
        error: FileError,
    },
}

/// How far a download is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct Progress {
    /// Bytes kept.
    pub received: u32,
    /// The size LIST gave.
    pub total: u32,
    /// Bytes per second since the first chunk of this download (0 until two instants differ).
    pub bytes_per_s: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Idle,
    Listing,
    Reading,
    Done,
    /// WRITE_BEGIN sent, its END awaited.
    WriteBegin,
    /// The unit is ready for WRITE_DATA (it answers only a refusal).
    Sending,
    /// WRITE_END sent, its END awaited.
    Ending,
    /// DELETE sent, its END awaited.
    Deleting,
    /// PROFILE_GET sent, its PROFILE frame awaited.
    ProfileGet,
    /// PROFILE_SET sent, its END awaited.
    ProfileSet,
}

struct Inner {
    mode: Mode,
    seq: u8,
    entries: Vec<RideEntry>,
    /// The last LIST_IN's files (`name` is the key): kept apart, so listing the workouts leaves the rides listed.
    files_in: Vec<(RideEntry, u16)>,
    /// The check READ_IN gives back for the file being read (see link/filesvc.rs).
    read_check: u16,
    /// The folder of the list under way, and of the download under way (None: LIST and READ, the rides).
    list_folder: Option<Folder>,
    read_folder: Option<Folder>,
    name: String,
    size: u32,
    file: Vec<u8>,
    /// Bytes received since the current READ was issued: END's count must equal it.
    answer: u32,
    /// List answer: entries received.
    listed: u32,
    last_ms: u64,
    first_ms: Option<u64>,
    first_bytes: usize,
    now_ms: u64,
    /// A workout being written: its bytes and how many were handed out.
    out: Vec<u8>,
    sent: usize,
    /// The last PROFILE the unit gave.
    profile: Option<UnitProfile>,
}

/// The file service client; one command at a time (a new one replaces the one under way, as on the unit).
#[derive(uniffi::Object)]
pub struct FileClient(Mutex<Inner>);

impl Inner {
    fn encode(&self, c: Command) -> Vec<u8> {
        let mut f = [0; FRAME_MAX];
        let n = c.encode(&mut f);
        f.get(..n).unwrap_or(&[]).to_vec()
    }

    fn read_cmd(&mut self) -> Vec<u8> {
        self.seq = self.seq.wrapping_add(1);
        self.answer = 0;
        self.mode = Mode::Reading;
        let mut name = [b'0'; NAME_LEN];
        if let Some(b) = self.name.as_bytes().get(..NAME_LEN) {
            name.copy_from_slice(b);
        }
        let offset = u32::try_from(self.file.len()).unwrap_or(u32::MAX);
        let key =
            self.read_folder.and_then(|f| Some((f, Key::new(f, self.name.as_bytes())?.with_check(self.read_check))));
        match key {
            Some((folder, key)) => self.encode(Command::ReadIn { seq: self.seq, folder, offset, key }),
            None => self.encode(Command::Read { seq: self.seq, offset, name }),
        }
    }

    fn fail(&mut self, error: FileError) -> FileEvent {
        self.mode = Mode::Idle;
        FileEvent::Failed { error }
    }

    fn frame(&mut self, f: Frame) -> FileEvent {
        match (&self.mode, f) {
            (Mode::Listing, Frame::Entry { seq, name, size }) if seq == self.seq => {
                if self.entries.len() >= ENTRIES_MAX {
                    return self.fail(FileError::TooLong);
                }
                self.listed += 1;
                // the firmware's parser checked the twelve digits
                self.entries.push(RideEntry { name: String::from_utf8_lossy(&name).into_owned(), size });
                FileEvent::Entry
            }
            (Mode::Listing, Frame::EntryIn { seq, key, size }) if seq == self.seq && self.list_folder.is_some() => {
                if self.files_in.len() >= ENTRIES_MAX {
                    return self.fail(FileError::TooLong);
                }
                self.listed += 1;
                let name = String::from_utf8_lossy(key.bytes()).into_owned();
                self.files_in.push((RideEntry { name, size }, key.check()));
                FileEvent::Entry
            }
            (Mode::Listing, Frame::End { seq, status, count }) if seq == self.seq => match status {
                Status::Ok if count == self.listed => {
                    self.mode = Mode::Idle;
                    FileEvent::Listed
                }
                Status::Ok => self.fail(FileError::CountMismatch),
                s => self.end_error(s),
            },
            (Mode::Reading, Frame::Chunk { seq, data }) if seq == self.seq => {
                if self.file.len() + data.len() > (self.size as usize).min(FILE_MAX) {
                    return self.fail(FileError::TooLong);
                }
                if self.first_ms.is_none() {
                    (self.first_ms, self.first_bytes) = (Some(self.now_ms), self.file.len());
                }
                self.file.extend_from_slice(data);
                self.answer += u32::try_from(data.len()).unwrap_or(0);
                FileEvent::Progress
            }
            (Mode::Reading, Frame::End { seq, status, count }) if seq == self.seq => match status {
                Status::Ok if count != self.answer => self.fail(FileError::CountMismatch),
                Status::Ok if self.file.len() < self.size as usize && self.answer > 0 => FileEvent::Partial,
                Status::Ok if self.file.len() != self.size as usize => self.fail(FileError::Short),
                Status::Ok => {
                    self.mode = Mode::Done;
                    FileEvent::Finished
                }
                s => self.end_error(s),
            },
            (Mode::WriteBegin, Frame::End { seq, status, .. }) if seq == self.seq => match status {
                Status::Ok => {
                    self.mode = Mode::Sending;
                    FileEvent::WriteReady
                }
                Status::NotFound => self.fail(FileError::NoCalendar),
                s => self.write_error(s),
            },
            // WRITE_DATA is answered only when refused
            (Mode::Sending, Frame::End { seq, status, .. }) if seq == self.seq && status != Status::Ok => {
                self.write_error(status)
            }
            (Mode::Ending, Frame::End { seq, status, .. }) if seq == self.seq => match status {
                Status::Ok => {
                    self.mode = Mode::Idle;
                    FileEvent::Written
                }
                s => self.write_error(s),
            },
            (Mode::Deleting, Frame::End { seq, status, count }) if seq == self.seq => match status {
                Status::Ok if count == 1 => {
                    self.mode = Mode::Idle;
                    FileEvent::Deleted
                }
                Status::Ok => self.fail(FileError::CountMismatch),
                s => self.write_error(s),
            },
            (Mode::ProfileGet, Frame::Profile { seq, profile }) if seq == self.seq => {
                (self.mode, self.profile) = (Mode::Idle, Some(profile.into()));
                FileEvent::Profile
            }
            (Mode::ProfileGet, Frame::End { seq, status, .. }) if seq == self.seq => self.fail(match status {
                Status::BadRequest => FileError::BadRequest,
                Status::Busy => FileError::Busy,
                _ => FileError::Card,
            }),
            (Mode::ProfileSet, Frame::End { seq, status, .. }) if seq == self.seq => match status {
                Status::Ok => {
                    self.mode = Mode::Idle;
                    FileEvent::ProfileSet
                }
                s => self.write_error(s),
            },
            _ => FileEvent::Ignored,
        }
    }

    fn write_error(&mut self, s: Status) -> FileEvent {
        self.fail(match s {
            Status::Busy => FileError::Busy,
            Status::ReadOnly => FileError::ReadOnly,
            Status::Mismatch => FileError::Mismatch,
            Status::Full => FileError::Full,
            Status::NotFound => FileError::NotFound,
            Status::BadRequest => FileError::BadRequest,
            Status::Io | Status::Ok => FileError::Card,
        })
    }

    fn end_error(&mut self, s: Status) -> FileEvent {
        match s {
            // the mode stays: the caller resumes from the bytes kept
            Status::Busy => FileEvent::Busy,
            Status::NotFound => self.fail(FileError::NotFound),
            Status::BadRequest => self.fail(FileError::BadRequest),
            Status::Io | Status::Ok => self.fail(FileError::Card),
            // the writes' statuses: never the answer to a list or a read
            Status::ReadOnly | Status::Mismatch | Status::Full => self.fail(FileError::BadRequest),
        }
    }
}

#[uniffi::export]
impl FileClient {
    /// A client with nothing under way.
    #[uniffi::constructor]
    pub fn new() -> Self {
        FileClient(Mutex::new(Inner {
            mode: Mode::Idle,
            seq: 0,
            entries: Vec::new(),
            files_in: Vec::new(),
            read_check: 0,
            list_folder: None,
            read_folder: None,
            name: String::new(),
            size: 0,
            file: Vec::new(),
            answer: 0,
            listed: 0,
            last_ms: 0,
            first_ms: None,
            first_bytes: 0,
            now_ms: 0,
            out: Vec::new(),
            sent: 0,
            profile: None,
        }))
    }

    /// The bytes to write to Control to list the rides; clears the list.
    pub fn list_command(&self, now_ms: u64) -> Vec<u8> {
        let mut s = self.lock();
        s.seq = s.seq.wrapping_add(1);
        (s.mode, s.listed, s.last_ms) = (Mode::Listing, 0, now_ms);
        s.entries.clear();
        s.list_folder = None;
        let seq = s.seq;
        s.encode(Command::List { seq })
    }

    /// The bytes to write to Control to list the files of `folder` (LIST_IN); clears the last such list.
    pub fn list_in_command(&self, folder: UnitFolder, now_ms: u64) -> Vec<u8> {
        let mut s = self.lock();
        s.seq = s.seq.wrapping_add(1);
        (s.mode, s.listed, s.last_ms) = (Mode::Listing, 0, now_ms);
        s.files_in.clear();
        s.list_folder = Some(folder.into());
        let seq = s.seq;
        s.encode(Command::ListIn { seq, folder: folder.into() })
    }

    /// Starts downloading file `key` of the last LIST_IN from byte 0 (READ_IN): the bytes to write to Control; None
    /// when the key is not in that list (or too large). The finished file is [`FileClient::file`].
    pub fn read_in_command(&self, key: String, now_ms: u64) -> Option<Vec<u8>> {
        let mut s = self.lock();
        let folder = s.list_folder?;
        let (size, check) = s.files_in.iter().find(|e| e.0.name == key).map(|e| (e.0.size, e.1))?;
        if size as usize > FILE_MAX {
            return None;
        }
        (s.name, s.size, s.last_ms, s.first_ms, s.read_folder) = (key, size, now_ms, None, Some(folder));
        s.read_check = check;
        s.file.clear();
        Some(s.read_cmd())
    }

    /// The files of the last LIST_IN (complete after [`FileEvent::Listed`]); `name` is the key.
    pub fn files_in(&self) -> Vec<RideEntry> {
        self.lock().files_in.iter().map(|e| e.0.clone()).collect()
    }

    /// Starts sending `bytes` as a workout for `day` (`YYYYMMDD`, WRITE_BEGIN): the bytes to write to Control; None
    /// when the day is not eight digits or the file is empty or over the unit's 8 KB. After [`FileEvent::WriteReady`]
    /// call [`FileClient::write_next_command`] until it gives None.
    pub fn write_command(&self, day: String, bytes: Vec<u8>, now_ms: u64) -> Option<Vec<u8>> {
        let day: [u8; 8] = day.as_bytes().try_into().ok().filter(|d: &[u8; 8]| d.iter().all(u8::is_ascii_digit))?;
        if bytes.is_empty() || bytes.len() > WRITE_MAX {
            return None;
        }
        // a real date, as far as a month and a day number go (the unit makes the folder from it)
        let num = |r: std::ops::Range<usize>| day.get(r).and_then(|d| std::str::from_utf8(d).ok()?.parse::<u32>().ok());
        if !(1..=12).contains(&num(4..6)?) || !(1..=31).contains(&num(6..8)?) {
            return None;
        }
        let mut s = self.lock();
        s.seq = s.seq.wrapping_add(1);
        let size = u32::try_from(bytes.len()).ok()?;
        let crc = bryton_rustfw::inflate::crc32(bytes.iter().copied());
        (s.mode, s.last_ms, s.sent, s.out) = (Mode::WriteBegin, now_ms, 0, bytes);
        let seq = s.seq;
        Some(s.encode(Command::WriteBegin { seq, folder: Folder::Workout, day, size, crc }))
    }

    /// The next WRITE_DATA (then WRITE_END) of the write under way, for a Control write of at most `max_write` bytes
    /// (what the link gives: 20 at the default MTU, 14 bytes of data, up to 244); None until
    /// [`FileEvent::WriteReady`] and after the WRITE_END. The unit answers a WRITE_DATA only when it refuses it.
    pub fn write_next_command(&self, max_write: u32, now_ms: u64) -> Option<Vec<u8>> {
        let mut s = self.lock();
        if s.mode != Mode::Sending {
            return None;
        }
        s.last_ms = now_ms;
        let seq = s.seq;
        if s.sent >= s.out.len() {
            s.seq = s.seq.wrapping_add(1);
            s.mode = Mode::Ending;
            let seq = s.seq;
            return Some(s.encode(Command::WriteEnd { seq }));
        }
        let room = (max_write as usize).clamp(FRAME_MAX, FRAME_BIG).saturating_sub(DATA_HEAD);
        let end = (s.sent + room).min(s.out.len());
        let offset = u32::try_from(s.sent).ok()?;
        let cmd = {
            let data = s.out.get(s.sent..end)?;
            let mut f = [0; FRAME_BIG];
            let n = Command::WriteData { seq, offset, data }.encode(&mut f);
            f.get(..n)?.to_vec()
        };
        s.sent = end;
        Some(cmd)
    }

    /// Asks for the unit's training profile (PROFILE_GET; works during a ride): the bytes to write to Control. The
    /// answer is [`FileEvent::Profile`], then [`FileClient::profile`].
    pub fn profile_get_command(&self, now_ms: u64) -> Vec<u8> {
        let mut s = self.lock();
        s.seq = s.seq.wrapping_add(1);
        (s.mode, s.last_ms) = (Mode::ProfileGet, now_ms);
        let seq = s.seq;
        s.encode(Command::ProfileGet { seq })
    }

    /// Sets the unit's training profile (PROFILE_SET): the bytes to write to Control. The answer is
    /// [`FileEvent::ProfileSet`], or a failure: `BadRequest` (a value outside the unit's ranges), `Busy` (riding or on
    /// USB), `ReadOnly` (a build that does not write its card).
    pub fn profile_set_command(&self, profile: UnitProfile, now_ms: u64) -> Vec<u8> {
        let mut s = self.lock();
        s.seq = s.seq.wrapping_add(1);
        (s.mode, s.last_ms) = (Mode::ProfileSet, now_ms);
        let seq = s.seq;
        s.encode(Command::ProfileSet { seq, profile: profile.into() })
    }

    /// The last profile the unit gave (None before the first [`FileEvent::Profile`]).
    pub fn profile(&self) -> Option<UnitProfile> {
        self.lock().profile
    }

    /// Drops a write or delete under way (after a timeout, a lost link or a refusal): late answers are ignored.
    pub fn abort(&self) {
        let mut s = self.lock();
        if matches!(
            s.mode,
            Mode::WriteBegin | Mode::Sending | Mode::Ending | Mode::Deleting | Mode::ProfileGet | Mode::ProfileSet
        ) {
            s.mode = Mode::Idle;
        }
        (s.out, s.sent) = (Vec::new(), 0);
    }

    /// Deletes file `key` of the last LIST_IN (DELETE): the bytes to write to Control; None when it is not listed.
    pub fn delete_command(&self, key: String, now_ms: u64) -> Option<Vec<u8>> {
        let mut s = self.lock();
        let folder = s.list_folder?;
        let check = s.files_in.iter().find(|e| e.0.name == key).map(|e| e.1)?;
        let key = Key::new(folder, key.as_bytes())?.with_check(check);
        s.seq = s.seq.wrapping_add(1);
        (s.mode, s.last_ms) = (Mode::Deleting, now_ms);
        let seq = s.seq;
        Some(s.encode(Command::Delete { seq, folder, key }))
    }

    /// Starts downloading a listed ride from byte 0: the bytes to write to Control; None when the ride is not in the
    /// last LIST (or too large).
    pub fn read_command(&self, name: String, now_ms: u64) -> Option<Vec<u8>> {
        let mut s = self.lock();
        let size = s.entries.iter().find(|e| e.name == name).map(|e| e.size)?;
        if size as usize > FILE_MAX {
            return None;
        }
        (s.name, s.size, s.last_ms, s.first_ms, s.read_folder) = (name, size, now_ms, None, None);
        s.file.clear();
        Some(s.read_cmd())
    }

    /// Asks again for the rest of the download from the bytes kept (after BUSY, a link drop or silence); None when
    /// no download is under way. A finished file is not asked again.
    pub fn resume_command(&self, now_ms: u64) -> Option<Vec<u8>> {
        let mut s = self.lock();
        if s.mode != Mode::Reading {
            return None;
        }
        s.last_ms = now_ms;
        Some(s.read_cmd())
    }

    /// The bytes that stop the answer under way, and the download with it: chunks that were already on the way are
    /// ignored, so a cancelled read cannot finish or count as progress later (the app's sync yields this way).
    pub fn cancel_command(&self) -> Vec<u8> {
        let mut s = self.lock();
        if matches!(s.mode, Mode::Reading) {
            s.mode = Mode::Idle;
        }
        s.encode(Command::Cancel)
    }

    /// One Data notification's value, at `now_ms` (any monotonic clock).
    pub fn on_data(&self, value: Vec<u8>, now_ms: u64) -> FileEvent {
        let mut s = self.lock();
        s.now_ms = now_ms;
        let Some(f) = Frame::parse(&value) else { return FileEvent::Ignored };
        let e = s.frame(f);
        if e != FileEvent::Ignored {
            s.last_ms = now_ms;
        }
        e
    }

    /// True when an answer is awaited and nothing came for [`SILENCE_MS`]: the unit dropped it.
    pub fn silent(&self, now_ms: u64) -> bool {
        let s = self.lock();
        matches!(
            s.mode,
            Mode::Listing
                | Mode::Reading
                | Mode::WriteBegin
                | Mode::Sending
                | Mode::Ending
                | Mode::Deleting
                | Mode::ProfileGet
                | Mode::ProfileSet
        ) && now_ms.saturating_sub(s.last_ms) >= SILENCE_MS
    }

    /// The rides of the last LIST (complete after [`FileEvent::Listed`]).
    pub fn entries(&self) -> Vec<RideEntry> {
        self.lock().entries.clone()
    }

    /// Bytes kept, the total, and the speed.
    pub fn progress(&self) -> Progress {
        let s = self.lock();
        if matches!(s.mode, Mode::WriteBegin | Mode::Sending | Mode::Ending) {
            let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
            return Progress { received: n(s.sent), total: n(s.out.len()), bytes_per_s: 0 };
        }
        let ms = s.first_ms.map_or(0, |t| s.now_ms.saturating_sub(t));
        let got = s.file.len().saturating_sub(s.first_bytes) as u64;
        Progress {
            received: u32::try_from(s.file.len()).unwrap_or(u32::MAX),
            total: s.size,
            bytes_per_s: if ms == 0 { 0 } else { u32::try_from(got * 1000 / ms).unwrap_or(u32::MAX) },
        }
    }

    /// The finished file; empty until [`FileEvent::Finished`].
    pub fn file(&self) -> Vec<u8> {
        let s = self.lock();
        if s.mode == Mode::Done { s.file.clone() } else { Vec::new() }
    }
}

impl Default for FileClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FileClient {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}
