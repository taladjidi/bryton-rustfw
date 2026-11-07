//! The phone's side of the unit's file service (src/link/filesvc.rs): a sans-IO state machine. Swift writes the bytes
//! it returns to Control and feeds every Data notification to [`FileClient::on_data`]; frames are encoded and decoded
//! by the firmware's own `filesvc` (no second copy of the format).
//!
//! Chunk sizes are never assumed: each CHUNK says its own length. The whole file is kept in memory, so a dropped
//! READ (BUSY, a link drop, 5 s of silence) resumes with [`FileClient::resume_command`] from the bytes kept.

use bryton_rustfw::filesvc::{Command, FRAME_MAX, Folder, Frame, Key, NAME_LEN, Status};
use std::sync::Mutex;

/// No frame for this long while an answer is awaited means the unit dropped it (filesvc.rs, "Dropped and moved").
pub const SILENCE_MS: u64 = 5_000;
/// Largest file accepted (a ride is a few MB at most): a hostile LIST cannot make the app allocate without bound.
const FILE_MAX: usize = 64 << 20;
/// Most rides kept from one LIST.
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
            _ => FileEvent::Ignored,
        }
    }

    fn end_error(&mut self, s: Status) -> FileEvent {
        match s {
            // the mode stays: the caller resumes from the bytes kept
            Status::Busy => FileEvent::Busy,
            Status::NotFound => self.fail(FileError::NotFound),
            Status::BadRequest => self.fail(FileError::BadRequest),
            Status::Io | Status::Ok => self.fail(FileError::Card),
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

    /// The bytes that stop the answer under way; the download is kept for [`FileClient::resume_command`].
    pub fn cancel_command(&self) -> Vec<u8> {
        self.lock().encode(Command::Cancel)
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
        matches!(s.mode, Mode::Listing | Mode::Reading) && now_ms.saturating_sub(s.last_ms) >= SILENCE_MS
    }

    /// The rides of the last LIST (complete after [`FileEvent::Listed`]).
    pub fn entries(&self) -> Vec<RideEntry> {
        self.lock().entries.clone()
    }

    /// Bytes kept, the total, and the speed.
    pub fn progress(&self) -> Progress {
        let s = self.lock();
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
