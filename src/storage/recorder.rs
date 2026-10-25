//! The ride recorder (AGENTS.md, "Golden rule: never lose a ride"): the ride as a FIT file ([`fit::Writer`]) on
//! stock's FAT volume (`fat::write`), a checkpoint at least every minute, two recovery records, a resume after any
//! reset or power cut, and a save that cannot lose the ride.
//!
//! **Files** (AGENTS.md, "Share the storage with stock"): the ride in progress is `\Rustfw\Rides\YYMMDDhhmmss.fit`
//! (local start time, stock's name) and its recovery records `\Rustfw\ride.rec`, in this firmware's own folder, so
//! neither firmware resumes the other's ride; a saved ride is `\Activities\YYMMDDhhmmss.fit`, where stock, its app,
//! the history and a computer find it. Stock's `\Activities\tmp.fit` and `\System\tmp*.fit` are never touched.
//! The ride's entry is rewritten at every checkpoint (its size), `ride.rec`'s only when it is made: in two folders,
//! they never share a directory sector, so one torn by a cut cannot take both.
//!
//! **RAM, not the card, each second.** A record is about 30 bytes, appended to a [`TAIL`]-byte buffer that holds the
//! file from the sector the next byte goes in. The card is written at a checkpoint only: every [`CHECKPOINT_S`]
//! seconds of ride, at a pause, at the end, at switching off, and earlier when the buffer has less than [`LOW`]
//! bytes left (a minute of every field is about 1.9 KB with the sector carried over). So the card sees about four
//! commands a minute and a power cut loses at most the last minute, as stock (ghidra/STORAGE_AND_MAPS.md 3: a
//! checkpoint every 60 samples). Writing each sector as it fills would not shorten that (the file and its size are
//! only valid at a checkpoint) and would cost more commands.
//!
//! **A checkpoint**, in this order: the FIT trailer and header in the buffer ([`fit::Writer::checkpoint`]); the
//! new sectors (clusters taken first, `fat::write`); sector 0 with the new header; the size in the directory
//! entry; then the recovery record. A cut before the record leaves the previous record, whose file is the bytes
//! up to its length, which the writes after it never changed except two sectors rewritten in place: the one its
//! 2-byte trailer is in and sector 0 (the header). The previous record holds both as they were (below), and the
//! resume writes them again: so a cut that tears either (garbage in the sector being written, on a card without
//! reliable write: emmc.rs) loses nothing either.
//!
//! **The recovery record** is a slot of three sectors of `ride.rec`, the two slots written in turn (a cut while
//! writing one leaves the other whole), in one write: the ride file's sector 0 and the sector its trailer ends
//! in, as the checkpoint left them; then the record: a tag (its layout version), a sequence number (the larger
//! valid one wins: never the data length, so a finished ride's record cannot pass for a new ride's), the phase
//! (done, starting, recording, saving), the file's identity (its name and first cluster: a resume opens only that
//! file and checks both), the FIT writer's state ([`fit::RECORD_LEN`] bytes, its own CRC) and the app's ride totals
//! (`ride::Ride::to_words`, their count checked); the sector's CRC last.
//!
//! **The save** (End), each step done again after a reset by [`Recorder::recover`] from the phase in the record:
//! 1. the end messages and a checkpoint whose record says "saving";
//! 2. the file read back whole from the card: its FIT CRC must be right. A read that fails is tried again; a file
//!    that reads but is not whole (a damaged sector) is left as it is in `\Rustfw\Rides` and the record says
//!    "done" ([`Outcome::Kept`]): it is never tried again, so it never stops the next ride;
//! 3. `\Activities` made if missing and a free name found (a name taken by another file: a second later);
//! 4. its entry in `\Rustfw\Rides` marked deleted (0xe5 only: `fat::write`);
//! 5. the new entry `\Activities\YYMMDDhhmmss.fit` on the same chain;
//! 6. a record "done".
//!
//! Between 4 and 5 the chain is named by no entry for a moment (a lost chain, never a cross-link); the record
//! names it, and 5 is done again at the next start. So the ride in progress exists until the saved one does.
//!
//! **One writer, one ride.** What the last run left unfinished is finished before a new ride: by the boot
//! ([`Recorder::recover`]), or, if the boot did not (its marker lost, or the card failed then), at the new ride's
//! first card work: a ride in progress is resumed in its place if the marker said it was open or it stopped less
//! than [`RESUME_S`] ago, else ended at its last record and saved; a save is finished. `recover` does nothing
//! once a ride has started.
//!
//! **After a cut** the second FAT may differ from the first in the sector being written (or that sector torn) and
//! FSInfo's free count may be a few clusters high: `fat::Fat::sync_fats` runs whenever a record is found
//! unfinished, and when the folders are made (a cut there leaves no record). A ride whose entry the cut tore is
//! saved from its record (its chain and length), not resumed.
//!
//! **Two copies of each ride** (docs/sync.md): Bryton's server refuses a ride whose records hold developer fields,
//! and Intervals.icu reads DFA a1 from one (`Alpha1`), so one file cannot serve both. Two recorders run side by side
//! ([`Kind`]), each with everything above of its own: the clean one (protocol 1.0, no developer data: what stock,
//! Bryton and Strava take, byte for byte what it was) in `\Rustfw\Rides`, `\Rustfw\ride.rec` and `\Activities`; the
//! full one (`fit::Writer::begin_with` `dev`: `Alpha1` and `RMSSD` in every record) in `\Rustfw\Full`,
//! `\Rustfw\full.rec` and `\Rustfw\Activities`, same name. The app drives the clean one first in each pass, so the
//! full one never stands between the clean one and the card, and a failing full one is only said
//! (`App::step_recorder`). Three rules keep the full one from ever costing the clean one (the review of 2026-10-24):
//! the two checkpoint together ([`Recorder::keep_up`]: the full copy's bigger records used to run its buffer low
//! first, and the clean one was 22 s behind it after a reset on the unit); the full one waits for the clean one to
//! make its file and carries on a ride left open only if it is the clean one's ([`Recorder::io_after`],
//! [`Recorder::resumed_after`]: never a full copy of one ride carried on with another's seconds); and it stops when
//! the card has less than [`FULL_RESERVE`] free (FSInfo's count), the rest being the clean one's. Writing both as the ride goes was chosen over making one from the other at the save:
//! the save, the riskiest moment, stays as short as it was, and the clean file never depends on code that writes
//! developer fields. Cost (`two_copies_cost` below, a 900 s ride): about twice the sectors written, a few more
//! a minute; nothing near the eMMC's endurance.
//!
//! Errors: nothing here panics; a failing card leaves the data in RAM and is tried again after [`RETRY`] calls;
//! samples that do not fit the buffer meanwhile are counted ([`Recorder::dropped`]), never the file lost. Only a
//! card error is tried again, and a save or a ride left unfinished only [`GIVE_UP`] times in a row: then it is
//! left as it is ([`Outcome::Kept`]), so that the next ride records.

use crate::fat::write::{Handle, dos_time};
use crate::fat::{DiskWrite, Error, Fat, File, SECTOR};
use crate::fit::{self, RECORD_LEN, Sample, Sink, Writer};
use crate::ride::RIDE_WORDS;

/// This firmware's folder, its recovery records, the folder of the ride in progress in it, stock's rides folder.
pub const DIR: &str = "Rustfw";
/// See [`DIR`].
pub const REC: &str = "ride.rec";
/// See [`DIR`].
pub const RIDES: &str = "Rides";
/// See [`DIR`].
pub const ACTIVITIES: &str = "Activities";

/// Which copy of the ride a recorder writes (module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Protocol 1.0, no developer data: `\Rustfw\Rides`, `\Rustfw\ride.rec`, then `\Activities`.
    Clean,
    /// Developer fields (`Alpha1`, `RMSSD`): `\Rustfw\Full`, `\Rustfw\full.rec`, then `\Rustfw\Activities`.
    Full,
}

impl Kind {
    /// Its recovery records' file in [`DIR`].
    fn rec(self) -> &'static str {
        match self {
            Kind::Clean => REC,
            Kind::Full => "full.rec",
        }
    }

    /// The folder of the ride in progress in [`DIR`], and its path.
    fn rides(self) -> (&'static str, &'static str) {
        match self {
            Kind::Clean => (RIDES, "Rustfw/Rides"),
            Kind::Full => ("Full", "Rustfw/Full"),
        }
    }

    /// The saved rides' folder.
    fn saved(self) -> &'static str {
        match self {
            Kind::Clean => ACTIVITIES,
            Kind::Full => FULL_PATH,
        }
    }

    /// The saved rides' folder, made if missing: `\Activities` in the root, `\Rustfw\Activities` in [`DIR`].
    fn saved_dir(self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let parent = match self {
            Kind::Clean => fat.root(),
            Kind::Full => fat.ensure_dir(d, fat.root(), DIR, t)?,
        };
        fat.ensure_dir(d, parent, ACTIVITIES, t)
    }
}

/// `\Rustfw\Activities`: the full copies of the saved rides ([`Kind::Full`]).
pub const FULL_PATH: &str = "Rustfw/Activities";
/// Free bytes (FSInfo's count) under which the full copy stops: the rest is the clean copy's. A day of riding is
/// about 3 MB of clean copy (a record about 33 bytes a second).
pub const FULL_RESERVE: u64 = 8 << 20;
/// Sectors of `ride.rec`: two slots of three (module doc).
const REC_SECTORS: u32 = 6;
/// Card failures in a row after which a save, or a ride left by the last run, is given up (kept where it is): a
/// minute of [`RETRY`]s. Only `Io` is tried again at all: any other error comes from what is on the card.
pub const GIVE_UP: u32 = 12;
/// A ride left unfinished whose last record is at most this old when a new one starts is resumed instead (the
/// boot's marker lost); an older one is saved as it is and the new ride starts.
pub const RESUME_S: u32 = 3600;
/// Bytes of file buffered in RAM.
pub const TAIL: usize = 2048;
/// Seconds of ride between checkpoints at most.
pub const CHECKPOINT_S: u32 = 60;
/// A checkpoint is due when the buffer has less than this left (room for some seconds more if it fails).
pub const LOW: usize = 512;
/// Calls of [`Recorder::io`] to wait after an error before trying the card again.
pub const RETRY: u32 = 100;
/// Bytes a record message takes at most, with a new definition (and its developer fields) and the trailer after it.
const RECORD_MAX: usize = 6 + 3 * 10 + 7 + 1 + 27 + 6 + 2;
/// Record tag and layout version: change it with any change of [`Rec`]'s layout or of `ride::Ride::to_words`
/// (the record also holds [`RIDE_WORDS`], byte 21, checked).
const TAG: [u8; 4] = *b"RFR2";
const _: () = assert!(128 + 4 * RIDE_WORDS <= SECTOR - 2 && RIDE_WORDS < 256);

/// Local time: year, month, day, hour, minute, second.
pub type Local = (u16, u8, u8, u8, u8, u8);

/// Where the recording is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No ride (or saved).
    Done = 0,
    /// Started: the file is being made.
    Starting = 1,
    /// Recording.
    Recording = 2,
    /// Ended: being saved.
    Saving = 3,
}

/// What [`Recorder::io`] or [`Recorder::recover`] did that the app should know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to tell.
    Nothing,
    /// A ride in progress was found and reopened at its last checkpoint: its totals and name (its start).
    Resumed([u32; RIDE_WORDS], [u8; 12]),
    /// A ride was saved as `\Activities\<name>.fit`.
    Saved([u8; 12]),
    /// A ride could not be saved (its file read back damaged, or the card failed [`GIVE_UP`] times in a row) and is
    /// left as it is, `\Rustfw\Rides\<name>.fit` as a rule, so that the next ride can be recorded.
    Kept([u8; 12]),
    /// The card failed (tried again later).
    Failed(Error),
}

/// One recovery record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rec {
    seq: u32,
    phase: Phase,
    name: [u8; 12],
    cluster: u32,
    fit: [u8; RECORD_LEN],
    ride: [u32; RIDE_WORDS],
}

impl Rec {
    fn to_sector(self) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        s[..4].copy_from_slice(&TAG);
        s[4..8].copy_from_slice(&self.seq.to_le_bytes());
        s[8] = self.phase as u8;
        s[9..21].copy_from_slice(&self.name);
        s[21] = RIDE_WORDS as u8;
        s[24..28].copy_from_slice(&self.cluster.to_le_bytes());
        s[32..32 + RECORD_LEN].copy_from_slice(&self.fit);
        for (c, w) in s[128..128 + 4 * RIDE_WORDS].chunks_exact_mut(4).zip(self.ride) {
            c.copy_from_slice(&w.to_le_bytes());
        }
        let crc = fit::crc(0, &s[..SECTOR - 2]);
        s[SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
        s
    }

    fn from_sector(s: &[u8; SECTOR]) -> Option<Rec> {
        if s[..4] != TAG
            || usize::from(s[21]) != RIDE_WORDS
            || fit::crc(0, &s[..SECTOR - 2]).to_le_bytes() != s[SECTOR - 2..]
        {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]);
        let phase = match s[8] {
            0 => Phase::Done,
            1 => Phase::Starting,
            2 => Phase::Recording,
            3 => Phase::Saving,
            _ => return None,
        };
        let mut r = Rec {
            seq: word(4),
            phase,
            name: [0; 12],
            cluster: word(24),
            fit: [0; RECORD_LEN],
            ride: core::array::from_fn(|k| word(128 + 4 * k)),
        };
        r.name.copy_from_slice(&s[9..21]);
        r.fit.copy_from_slice(&s[32..32 + RECORD_LEN]);
        Some(r)
    }
}

/// `<name>.fit` as text.
fn file_name<'a>(name: &[u8; 12], out: &'a mut [u8; 16]) -> Result<&'a str, Error> {
    out[..12].copy_from_slice(name);
    out[12..].copy_from_slice(b".fit");
    core::str::from_utf8(out).map_err(|_| Error::Corrupt)
}

/// Stock's ride name of a local time: `YYMMDDhhmmss`.
///
/// ```
/// assert_eq!(&bryton_rustfw::recorder::ride_name((2026, 10, 24, 7, 5, 9)), b"261024070509");
/// ```
pub fn ride_name((y, mo, d, h, mi, s): Local) -> [u8; 12] {
    let mut n = [b'0'; 12];
    for (k, v) in [(y % 100) as u8, mo, d, h, mi, s].into_iter().enumerate() {
        n[2 * k] = b'0' + (v / 10) % 10;
        n[2 * k + 1] = b'0' + v % 10;
    }
    n
}

/// The name one second later (a name already taken). ponytail: the date does not roll over at midnight.
///
/// ```
/// # use bryton_rustfw::recorder::ride_name;
/// assert_eq!(bryton_rustfw::recorder::next_name(&ride_name((2026, 1, 2, 9, 59, 59))), ride_name((2026, 1, 2, 10, 0, 0)));
/// ```
pub fn next_name(n: &[u8; 12]) -> [u8; 12] {
    let v = |i: usize| n[i].saturating_sub(b'0') % 10 * 10 + n[i + 1].saturating_sub(b'0') % 10;
    let (mut h, mut m, mut s) = (v(6), v(8), v(10) + 1);
    if s >= 60 {
        (s, m) = (0, m + 1);
    }
    if m >= 60 {
        (m, h) = (0, (h + 1) % 24);
    }
    let mut out = *n;
    for (k, x) in [h, m, s].into_iter().enumerate() {
        out[6 + 2 * k] = b'0' + x / 10;
        out[7 + 2 * k] = b'0' + x % 10;
    }
    out
}

/// The ride being recorded: the FIT writer, the RAM buffer of the file's tail, and where the file is.
pub struct Recorder {
    kind: Kind,
    phase: Phase,
    writer: Writer,
    /// The ride file, once made; the recovery records' file.
    file: Option<Handle>,
    rec: Option<Handle>,
    seq: u32,
    name: [u8; 12],
    /// The file from byte `tail_off` (a sector's start) on; the file is `len` bytes.
    tail: [u8; TAIL],
    tail_off: u32,
    len: u32,
    /// The FIT header (bytes 0..14), kept for rewriting sector 0.
    head: [u8; 14],
    /// Seconds of ride since the last checkpoint; a checkpoint is wanted now; the end is written in the buffer.
    since: u32,
    want: bool,
    ended: bool,
    /// The app's ride totals at the last second; the last record's time.
    ride: [u32; RIDE_WORDS],
    last_rec: u32,
    /// The full copy, resumed: the seconds up to here are in its file already (its checkpoint may be later than the
    /// clean one's, which the app goes on from), so they are not written twice.
    skip_to: u32,
    /// Calls to wait before trying the card again; card failures in a row.
    wait: u32,
    fails: u32,
    /// The boot's marker said a ride was open: a ride left unfinished is resumed whatever its age ([`RESUME_S`]).
    resume_old: bool,
    /// Samples (or laps, pauses) that did not fit in the buffer.
    pub dropped: u32,
    /// Card errors so far.
    pub errors: u32,
}

/// The buffer as the FIT writer's [`Sink`]: no card access (a checkpoint's flush is [`Recorder::flush`]).
struct Ram<'a> {
    tail: &'a mut [u8; TAIL],
    tail_off: u32,
    len: &'a mut u32,
    head: &'a mut [u8; 14],
}

impl Sink for Ram<'_> {
    fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), fit::Error> {
        let end = offset as usize + data.len();
        if end > self.tail_off as usize + TAIL || (offset < self.tail_off && end > self.head.len()) {
            return Err(fit::Error); // only the header goes before the buffer
        }
        for (i, &b) in data.iter().enumerate() {
            let at = offset as usize + i;
            if let Some(h) = self.head.get_mut(at) {
                *h = b;
            }
            if let Some(t) = at.checked_sub(self.tail_off as usize).and_then(|k| self.tail.get_mut(k)) {
                *t = b;
            }
        }
        Ok(())
    }

    fn set_len(&mut self, len: u32) -> Result<(), fit::Error> {
        *self.len = len;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), fit::Error> {
        Ok(())
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    /// No ride: the clean copy's recorder ([`Kind::Clean`]).
    pub const fn new() -> Self {
        Recorder::of(Kind::Clean)
    }

    /// No ride, for copy `kind`.
    pub const fn of(kind: Kind) -> Self {
        Recorder {
            kind,
            phase: Phase::Done,
            writer: Writer::new(),
            file: None,
            rec: None,
            seq: 0,
            name: [b'0'; 12],
            tail: [0; TAIL],
            tail_off: 0,
            len: 0,
            head: [0; 14],
            since: 0,
            want: false,
            ended: false,
            ride: [0; RIDE_WORDS],
            last_rec: 0,
            skip_to: 0,
            wait: 0,
            fails: 0,
            resume_old: false,
            dropped: 0,
            errors: 0,
        }
    }

    /// The ride's name (its start, a second later per name already taken).
    pub fn name(&self) -> [u8; 12] {
        self.name
    }

    /// A checkpoint here whenever `other` (the ride's other copy) has one due, unless it is waiting after a failure:
    /// the two copies checkpoint together, so at a cut neither holds seconds the other lost (the full copy's buffer
    /// runs low first, its records being bigger). Before each pass's card work.
    pub fn keep_up(&mut self, other: &Recorder) {
        if other.want && other.fails == 0 && matches!(self.phase, Phase::Starting | Phase::Recording) {
            self.want = true;
        }
    }

    /// The full copy's card work ([`Recorder::io`]), after the clean copy `clean`'s in the same pass (module doc,
    /// "Two copies"): the clean one's name while this one's file is not made (the clean one may have moved a second
    /// on, or resumed the ride left open); nothing until the clean one has made its file, so that a ride left open
    /// is carried on only when it is the clean one's; and a save only once the clean one's has begun.
    pub fn io_after(&mut self, clean: &Recorder, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if self.phase == Phase::Starting && self.file.is_none() {
            self.name = clean.name;
        }
        if clean.phase == Phase::Starting || (self.ended && clean.phase == Phase::Recording) {
            return Outcome::Nothing;
        }
        self.io(d, now)
    }

    /// The full copy reopened by [`Recorder::recover`] (`Resumed`) goes on only with the clean copy `clean`'s ride:
    /// else (the clean one saved, kept, or another ride) it is ended at its last record and saved as it is.
    pub fn resumed_after(&mut self, clean: &Recorder) {
        if self.phase == Phase::Recording && (clean.phase != Phase::Recording || clean.name != self.name) {
            self.end(0, self.ride);
        }
    }

    /// Where it is.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    fn ram(&mut self) -> (Ram<'_>, &mut Writer) {
        let sink = Ram { tail: &mut self.tail, tail_off: self.tail_off, len: &mut self.len, head: &mut self.head };
        (sink, &mut self.writer)
    }

    fn room(&self) -> usize {
        TAIL.saturating_sub(self.len.saturating_sub(self.tail_off) as usize)
    }

    /// Runs `f` on the writer if `need` bytes fit (the end's bytes kept for the end); else counts it dropped.
    fn put(&mut self, need: usize, f: impl FnOnce(&mut Writer, &mut Ram) -> Result<(), fit::Error>) {
        if !matches!(self.phase, Phase::Starting | Phase::Recording) || self.ended {
            return;
        }
        let ok = self.room() >= need + Writer::END_BYTES && {
            let (mut sink, w) = self.ram();
            f(w, &mut sink).is_ok()
        };
        if !ok {
            self.dropped = self.dropped.saturating_add(1);
        }
        // the data length is the writer's: the buffer's length follows it (the trailer is a checkpoint's)
        self.len = self.len.max(14 + self.writer.data_len());
        if self.room() < LOW + Writer::END_BYTES {
            self.want = true;
        }
    }

    /// A new ride at FIT time `time`, named after its local start: the file's first bytes, in RAM (the file is
    /// made at the next [`Recorder::io`], after what the last run left unfinished: resumed in this one's place if
    /// `resume_old` (the boot's marker said a ride was open) or if it stopped less than [`RESUME_S`] before `time`,
    /// else saved as it is). Ignored while a ride is still being saved.
    pub fn start(&mut self, time: u32, name: [u8; 12], resume_old: bool) {
        if self.phase != Phase::Done {
            return;
        }
        (self.tail, self.tail_off, self.len, self.head) = ([0; TAIL], 0, 0, [0; 14]);
        let dev = self.kind == Kind::Full;
        let (mut sink, _) = self.ram();
        let Ok(w) = Writer::begin_with(&mut sink, time, dev) else { return };
        (self.writer, self.last_rec) = (w, time);
        self.len = 14 + w.data_len();
        (self.phase, self.file, self.name, self.since, self.want, self.ended) =
            (Phase::Starting, None, name, 0, true, false);
        self.resume_old = resume_old;
    }

    /// A checkpoint at the next [`Recorder::io`], even while waiting after a failure (switching off: the last
    /// chance to put what is in RAM on the card).
    pub fn checkpoint_now(&mut self) {
        if matches!(self.phase, Phase::Starting | Phase::Recording) {
            (self.want, self.wait) = (true, 0);
        }
    }

    /// The card failed at the last try: what is in RAM since the last checkpoint is not on the card.
    pub fn failing(&self) -> bool {
        self.fails > 0
    }

    /// Card failures in a row (0 after a success).
    pub fn fails(&self) -> u32 {
        self.fails
    }

    /// Whether to try again after `e`: only a card error, and only [`GIVE_UP`] times in a row.
    fn retry(&self, e: Error) -> bool {
        e == Error::Io && self.fails + 1 < GIVE_UP
    }

    /// One second of ride: its record, and the app's totals for the next recovery record.
    pub fn second(&mut self, s: &Sample, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        // one record a second, each later than the last (seconds caught up after a long pass share a clock)
        if self.kind == Kind::Full && s.time <= self.skip_to {
            return;
        }
        let time = if s.time > self.last_rec { s.time } else { self.last_rec + 1 };
        self.last_rec = time;
        let s = Sample { time, ..*s };
        self.put(RECORD_MAX, |w, sink| w.record(sink, &s));
        self.since += 1;
        if self.since >= CHECKPOINT_S {
            self.want = true;
        }
    }

    /// Paused at `time` (a checkpoint follows, as stock).
    pub fn pause(&mut self, time: u32, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        self.put(16, |w, sink| w.pause(sink, time));
        self.want = true;
    }

    /// Resumed at `time`.
    pub fn resume(&mut self, time: u32) {
        self.put(16, |w, sink| w.resume(sink, time));
    }

    /// A lap at `time`.
    pub fn lap(&mut self, time: u32) {
        self.put(110, |w, sink| w.lap(sink, time));
    }

    /// The ride ended at `time`: the end messages in RAM, the save at the next [`Recorder::io`].
    pub fn end(&mut self, time: u32, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        if !matches!(self.phase, Phase::Starting | Phase::Recording) || self.ended {
            return;
        }
        let (mut sink, w) = self.ram();
        if w.end(&mut sink, time).is_err() {
            self.dropped = self.dropped.saturating_add(1); // cannot happen: END_BYTES are kept
        }
        self.len = self.len.max(14 + self.writer.data_len());
        (self.ended, self.want) = (true, true);
    }

    /// The card has work to do ([`Recorder::io`]).
    pub fn due(&self) -> bool {
        (self.want && self.phase != Phase::Done) || self.phase == Phase::Saving // a save is due until done
    }

    /// Does the card work that is due: make the file, a checkpoint, the save. Call it from the main loop (it takes
    /// a few ms when it writes, once a minute; the save reads the file back).
    #[inline(never)] // its sector buffers in its own frame, not the main loop's
    pub fn io(&mut self, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if !self.due() {
            return Outcome::Nothing;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Outcome::Nothing;
        }
        match self.io_inner(d, now) {
            Ok(o) => {
                self.fails = 0;
                o
            }
            Err(e) if self.kind == Kind::Full && self.phase == Phase::Starting && !self.retry(e) => {
                // the full copy could not be made (refused, or the card kept failing): this ride goes without it,
                // never held up for it
                self.errors = self.errors.saturating_add(1);
                (self.phase, self.want, self.fails) = (Phase::Done, false, 0);
                Outcome::Kept(self.name)
            }
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                self.fails = self.fails.saturating_add(1);
                self.wait = RETRY;
                Outcome::Failed(e)
            }
        }
    }

    fn io_inner(&mut self, d: &mut impl DiskWrite, now: Local) -> Result<Outcome, Error> {
        let t = dos_time(now);
        let mut fat = Fat::mount(d)?;
        let low = |fat: &mut Fat, d: &mut _| -> Result<bool, Error> {
            let free = fat.free_clusters(d)?.map(|n| u64::from(n) * u64::from(fat.cluster_bytes()));
            Ok(free.is_some_and(|b| b < FULL_RESERVE))
        };
        if self.kind == Kind::Full && !self.ended && self.phase != Phase::Saving && low(&mut fat, d)? {
            // the card nearly full: what is left is the clean copy's. This one stops where it is (its record done:
            // never resumed nor saved, a part of the ride only), said once
            if let Some(f) = self.file {
                self.write_record(&mut fat, d, Phase::Done, f.first)?;
            }
            (self.phase, self.want) = (Phase::Done, false);
            return Ok(Outcome::Kept(self.name));
        }
        let mut told = Outcome::Nothing;
        if self.phase == Phase::Starting {
            let dir = self.setup(&mut fat, d, t)?;
            let best = self.read_records(&mut fat, d)?;
            if let Some(r) = best.filter(|r| r.phase != Phase::Done) {
                // what the last run left unfinished comes first (the boot did not finish it: its marker was lost, or
                // the card failed then): one writer, one ride. A ride in progress goes on in place of this one if
                // the marker said so or it stopped a moment ago; else it is saved as it is
                let last = Writer::from_record(&r.fit).map_or(0, |w| w.last_time());
                let resume = match self.kind {
                    Kind::Clean => self.resume_old || self.last_rec.wrapping_sub(last) <= RESUME_S,
                    // the clean copy decided first ([`Recorder::io_after`]): its name is the ride left open if it
                    // carries that one on
                    Kind::Full => r.name == self.name,
                };
                fat.sync_fats(d)?; // the cut may have been between the two FATs, or torn one
                let mut old = Recorder::of(self.kind);
                (old.rec, old.seq, old.errors) = (self.rec, self.seq, self.errors);
                match old.recover_from(&mut fat, d, Some(r), t, resume) {
                    Ok(o @ Outcome::Resumed(..)) => {
                        *self = old;
                        return Ok(o);
                    }
                    Ok(o) => told = o,
                    Err(e) if self.retry(e) => return Err(e),
                    Err(_) => told = Outcome::Kept(r.name), // given up: left as it is, its record overwritten next
                }
                self.seq = self.seq.max(old.seq);
            }
            if self.file.is_none() {
                // a name free in this copy's folder and in both saved folders, so that the two copies keep one name
                let acts = fat.open(d, ACTIVITIES)?;
                let full = fat.open(d, FULL_PATH).ok(); // missing or damaged: never a reason to stop this one
                let mut buf = [0u8; 16];
                for _ in 0..10 {
                    let n = file_name(&self.name, &mut buf)?;
                    let free = |r: Result<File, Error>| matches!(r, Err(Error::NotFound));
                    if free(fat.find(d, dir, n))
                        && free(fat.find(d, acts, n))
                        && full.is_none_or(|f| free(fat.find(d, f, n)))
                    {
                        break;
                    }
                    self.name = next_name(&self.name);
                }
                self.write_record(&mut fat, d, Phase::Starting, 0)?;
                let n = file_name(&self.name, &mut buf)?;
                self.file = Some(fat.create(d, dir, n, t)?);
            }
            self.checkpoint(&mut fat, d, t, Phase::Recording)?;
            self.phase = Phase::Recording;
            if !self.ended {
                return Ok(told);
            }
        }
        if self.phase == Phase::Recording && !self.ended {
            self.checkpoint(&mut fat, d, t, Phase::Recording)?;
            return Ok(Outcome::Nothing);
        }
        if self.phase == Phase::Recording {
            self.checkpoint(&mut fat, d, t, Phase::Saving)?;
            self.phase = Phase::Saving;
        }
        if self.phase == Phase::Saving {
            let first = self.file.map_or(0, |h| h.first);
            let size = self.len;
            let o = match self.save(&mut fat, d, t, first, size) {
                Ok(o) => o,
                Err(e) if self.retry(e) => return Err(e),
                Err(_) => {
                    // given up: the ride stays where it is; "done" if the card takes it, else the boot and the next
                    // start try it once more
                    let _ = self.write_record(&mut fat, d, Phase::Done, first);
                    Outcome::Kept(self.name)
                }
            };
            (self.phase, self.want) = (Phase::Done, false);
            return Ok(o);
        }
        Ok(Outcome::Nothing)
    }

    /// Makes `\Rustfw`, `\Rustfw\ride.rec` (two zeroed sectors), `\Rustfw\Rides` and `\Activities` when missing
    /// (the FATs synced first: a cut while making them leaves no record to say so). `\Rustfw\Rides`.
    ///
    /// `ride.rec`'s entry is in `\Rustfw`, written here only; the ride's is in `\Rustfw\Rides`, rewritten at every
    /// checkpoint (its size): a sector torn by a cut cannot take both (the second review, R3).
    fn setup(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let root = fat.root();
        let dir = fat.find(d, root, DIR);
        let (rec, rides) = match dir {
            Ok(f) => (fat.locate(d, f, self.kind.rec()).ok(), fat.find(d, f, self.kind.rides().0).is_ok()),
            Err(_) => (None, false),
        };
        let whole = rides
            && rec.is_some_and(|r| r.size >= REC_SECTORS * SECTOR as u32)
            && fat.open(d, self.kind.saved()).is_ok();
        if !whole {
            fat.sync_fats(d)?;
        }
        let dir = fat.ensure_dir(d, root, DIR, t)?;
        let mut rec = match fat.locate(d, dir, self.kind.rec()) {
            Ok(h) => h,
            Err(Error::NotFound) => fat.create(d, dir, self.kind.rec(), t)?,
            Err(e) => return Err(e),
        };
        if rec.size < REC_SECTORS * SECTOR as u32 {
            fat.write_sectors(d, &mut rec, 0, &[0; REC_SECTORS as usize * SECTOR])?;
            fat.set_size(d, &mut rec, REC_SECTORS * SECTOR as u32, t)?;
        }
        self.rec = Some(rec);
        let rides = fat.ensure_dir(d, dir, self.kind.rides().0, t)?;
        fat.ensure_dir(d, root, ACTIVITIES, t)?; // both copies' names are looked for in it
        self.kind.saved_dir(fat, d, t)?;
        Ok(rides)
    }

    /// The newer valid record of the two (its sequence number taken as the last one used); None if neither is.
    fn read_records(&mut self, fat: &mut Fat, d: &mut impl DiskWrite) -> Result<Option<Rec>, Error> {
        let rec = self.rec.ok_or(Error::NotFound)?;
        let mut best: Option<Rec> = None;
        for k in 0..2 {
            let mut s = [0u8; SECTOR];
            fat.read_sector(d, &rec, 3 * k + 2, &mut s)?;
            if let Some(r) = Rec::from_sector(&s)
                && best.is_none_or(|b| r.seq > b.seq)
            {
                best = Some(r);
            }
        }
        self.seq = best.map_or(0, |r| r.seq);
        Ok(best)
    }

    fn write_record(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, phase: Phase, cluster: u32) -> Result<(), Error> {
        self.write_record_with(fat, d, phase, cluster, None)
    }

    /// The record in its slot (module doc), after the copies of the file's sector 0 and last sector when given:
    /// one write, the record last.
    fn write_record_with(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        phase: Phase,
        cluster: u32,
        copies: Option<(&[u8; SECTOR], &[u8; SECTOR])>,
    ) -> Result<(), Error> {
        let mut rec = self.rec.ok_or(Error::NotFound)?;
        let seq = self.seq.wrapping_add(1);
        let r = Rec { seq, phase, name: self.name, cluster, fit: self.writer.to_record(), ride: self.ride };
        let slot = 3 * (seq % 2);
        match copies {
            Some((first, last)) => {
                let mut buf = [0u8; 3 * SECTOR];
                buf[..SECTOR].copy_from_slice(first);
                buf[SECTOR..2 * SECTOR].copy_from_slice(last);
                buf[2 * SECTOR..].copy_from_slice(&r.to_sector());
                fat.write_sectors(d, &mut rec, slot, &buf)?;
            }
            None => fat.write_sectors(d, &mut rec, slot + 2, &r.to_sector())?,
        }
        self.seq = seq;
        Ok(())
    }

    /// A checkpoint (module doc), its record in `phase`.
    fn checkpoint(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16), phase: Phase) -> Result<(), Error> {
        let mut file = self.file.ok_or(Error::NotFound)?;
        {
            let (mut sink, w) = self.ram();
            w.checkpoint(&mut sink).map_err(|_| Error::Corrupt)?;
        }
        // the sectors from the buffer's start to the end of the file, then sector 0's header, then the size
        let n = (self.len - self.tail_off).div_ceil(SECTOR as u32) as usize;
        let bytes = self.tail.get(..n * SECTOR).ok_or(Error::Corrupt)?;
        fat.write_sectors(d, &mut file, self.tail_off / SECTOR as u32, bytes)?;
        let mut first = [0u8; SECTOR];
        if self.tail_off > 0 {
            fat.read_sector(d, &file, 0, &mut first)?;
            first[..14].copy_from_slice(&self.head);
            fat.write_sectors(d, &mut file, 0, &first)?;
        } else {
            first.copy_from_slice(self.tail.get(..SECTOR).ok_or(Error::Corrupt)?);
        }
        fat.set_size(d, &mut file, self.len, t)?;
        self.file = Some(file);
        // the record, with the two sectors the next checkpoint rewrites in place as they are now
        let next = (self.len - 2) / SECTOR as u32 * SECTOR as u32;
        let from = (next - self.tail_off) as usize;
        let mut last = [0u8; SECTOR];
        last.copy_from_slice(self.tail.get(from..from + SECTOR).ok_or(Error::Corrupt)?);
        self.write_record_with(fat, d, phase, file.first, Some((&first, &last)))?;
        // the buffer keeps the sector the next byte (over the trailer) goes in
        self.tail.copy_within(from.., 0);
        let keep = (self.len - next) as usize;
        if let Some(rest) = self.tail.get_mut(keep..) {
            rest.fill(0);
        }
        (self.tail_off, self.since, self.want) = (next, 0, false);
        Ok(())
    }

    /// The save from step 2 on (module doc): the file at `first`, `size` bytes, checked, moved, the record done:
    /// `Saved`. A file that reads back but is not a whole FIT file (a damaged sector, its chain cut short) is not
    /// moved and not tried again: it stays where it is, the record done: `Kept` (the second review, R1).
    fn save(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        t: (u16, u16),
        first: u32,
        size: u32,
    ) -> Result<Outcome, Error> {
        // 2: read back whole, through the buffer (the file is all on the card by now)
        let f = File { cluster: first, size, dir: false };
        let (mut at, mut crc, mut whole) = (0u32, 0u16, size >= 16);
        while whole && at < size {
            let n = match fat.read_at(d, &f, at, &mut self.tail) {
                Ok(n) => n,
                Err(Error::Corrupt) => 0, // the chain is not what the record says
                Err(e) => return Err(e),
            };
            whole = n > 0 && (at > 0 || (n >= 14 && self.tail[4..8] == (size - 16).to_le_bytes()));
            crc = fit::crc(crc, self.tail.get(..n).unwrap_or(&[]));
            at += n as u32;
        }
        if !whole || crc != 0 {
            self.write_record(fat, d, Phase::Done, first)?;
            return Ok(Outcome::Kept(self.name));
        }
        // 3: the folder and a free name, before 4, so that between 4 and 5 only the link is left
        let mut buf = [0u8; 16];
        let acts = self.kind.saved_dir(fat, d, t)?;
        let (mut name, mut linked) = (self.name, None);
        for _ in 0..10 {
            match fat.find(d, acts, file_name(&name, &mut buf)?) {
                Ok(f) if f.cluster == first => linked = Some(true), // a cut after 5: done already
                Ok(_) => name = next_name(&name),                   // a name another file has: a second later
                Err(Error::NotFound) => linked = Some(false),
                Err(e) => return Err(e),
            }
            if linked.is_some() {
                break;
            }
        }
        let linked = linked.ok_or(Error::Refused)?; // ten names taken: never "done" without the link
        // 4: the entry in the folder, if still there and still this chain
        let dir = fat.open(d, self.kind.rides().1)?;
        match fat.locate(d, dir, file_name(&self.name, &mut buf)?) {
            Ok(h) if h.first == first => fat.remove(d, &h)?,
            Ok(_) | Err(Error::NotFound | Error::Corrupt) => {} // gone, or its sector torn (the record is the name)
            Err(e) => return Err(e),
        }
        // 5: the new entry
        if !linked {
            let h = Handle { first, size, ..Handle::default() };
            fat.link(d, acts, file_name(&name, &mut buf)?, &h, t)?;
        }
        self.name = name;
        // 6
        self.write_record(fat, d, Phase::Done, first)?;
        Ok(Outcome::Saved(name))
    }

    /// At boot: a ride the last run did not finish is reopened at its last checkpoint (the app shows it paused),
    /// or its save is finished. Reads only `\Rustfw\ride.rec` when there is nothing to do.
    /// After a card failure, waits [`RETRY`] calls before trying again; gives up after [`GIVE_UP`] in a row, or at
    /// once on any other error (`Kept`: left as it is, and tried once more at the next start).
    /// Nothing once a new ride has started: its first [`Recorder::io`] does this first (one writer, one ride).
    #[inline(never)]
    pub fn recover(&mut self, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if self.phase != Phase::Done {
            return Outcome::Nothing;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Outcome::Nothing;
        }
        let t = dos_time(now);
        let run = |s: &mut Self, d: &mut _| -> Result<Outcome, Error> {
            let mut fat = Fat::mount(d)?;
            fat.sync_fats(d)?; // first: a FAT sector torn by the cut would break the chains read next
            let dir = fat.open(d, DIR)?;
            s.rec = Some(fat.locate(d, dir, s.kind.rec())?);
            let best = s.read_records(&mut fat, d)?;
            s.recover_from(&mut fat, d, best, t, true)
        };
        match run(self, d) {
            Ok(o) => {
                self.fails = 0;
                o
            }
            Err(Error::NotFound) => Outcome::Nothing, // never recorded here
            Err(e) => {
                self.phase = Phase::Done; // whatever was reopened: done again from the record
                self.errors = self.errors.saturating_add(1);
                if self.retry(e) {
                    self.fails += 1;
                    self.wait = RETRY;
                    Outcome::Failed(e)
                } else {
                    self.fails = 0;
                    Outcome::Kept(self.name)
                }
            }
        }
    }

    /// Waiting before trying the card again after a failure.
    pub fn waiting(&self) -> bool {
        self.wait > 0
    }

    fn recover_from(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        best: Option<Rec>,
        t: (u16, u16),
        resume: bool,
    ) -> Result<Outcome, Error> {
        let Some(r) = best.filter(|r| r.phase != Phase::Done) else { return Ok(Outcome::Nothing) };
        // the FATs were synced by the caller: the cut may have been between the two, or torn one
        let writer = Writer::from_record(&r.fit);
        (self.name, self.ride) = (r.name, r.ride);
        let size = writer.map_or(0, |w| 16 + w.data_len());
        match (r.phase, writer) {
            (Phase::Saving, Some(w)) => {
                self.writer = w;
                self.save(fat, d, t, r.cluster, size)
            }
            (Phase::Recording, Some(w)) => {
                let dir = fat.open(d, self.kind.rides().1)?;
                let mut buf = [0u8; 16];
                let fits =
                    |h: &Handle, cluster_bytes: u32| h.clusters.checked_mul(cluster_bytes).is_none_or(|b| b >= size);
                let entry = match fat.locate(d, dir, file_name(&r.name, &mut buf)?) {
                    Ok(h) if h.first == r.cluster && fits(&h, fat.cluster_bytes()) => Some(h),
                    // its entry gone: a cut tore its directory sector (a card without reliable write), or a computer
                    // removed it. Then nothing to resume: the chain the record names is saved as it is, if it still
                    // reads back as this ride's whole file, else left (`Kept`)
                    Ok(_) | Err(Error::NotFound | Error::Corrupt) => None,
                    Err(e) => return Err(e),
                };
                let mut file = match entry {
                    Some(h) => h,
                    None => fat.chain(d, r.cluster)?,
                };
                // the buffer from the sector the trailer starts in, and sector 0: from the record's copies, as they
                // were at its checkpoint (a cut while the next one rewrote them may have torn them in the file)
                (self.writer, self.last_rec, self.skip_to) = (w, w.last_time(), w.last_time());
                self.len = size;
                self.tail_off = (size - 2) / SECTOR as u32 * SECTOR as u32;
                self.tail = [0; TAIL];
                let (rec, slot) = (self.rec.ok_or(Error::NotFound)?, 3 * (r.seq % 2));
                let fits = fits(&file, fat.cluster_bytes());
                let mut s = [0u8; SECTOR];
                fat.read_sector(d, &rec, slot + 1, &mut s)?;
                let keep = (size - self.tail_off) as usize;
                self.tail[..keep.min(SECTOR)].copy_from_slice(&s[..keep.min(SECTOR)]);
                if entry.is_none() && fits {
                    fat.write_sectors(d, &mut file, self.tail_off / SECTOR as u32, &s)?;
                }
                fat.read_sector(d, &rec, slot, &mut s)?;
                self.head.copy_from_slice(&s[..14]);
                if fits && (self.tail_off > 0 || entry.is_none()) {
                    fat.write_sectors(d, &mut file, 0, &s)?;
                }
                if entry.is_none() {
                    if fits {
                        fat.trim(d, &mut file, size)?; // the clusters the next checkpoint took: free again
                    }
                    return self.save(fat, d, t, r.cluster, size);
                }
                self.file = Some(file);
                (self.phase, self.ended) = (Phase::Recording, false);
                // the file cut back to the checkpoint, its trailer and header written again from the record
                self.checkpoint(fat, d, t, Phase::Recording)?;
                if !resume {
                    // too old to go on (the second review, R6): ended at its last record, and saved
                    let (last, ride) = (self.writer.last_time(), self.ride);
                    self.end(last, ride);
                    self.checkpoint(fat, d, t, Phase::Saving)?;
                    self.phase = Phase::Saving;
                    let o = self.save(fat, d, t, file.first, self.len)?;
                    (self.phase, self.want) = (Phase::Done, false);
                    return Ok(o);
                }
                // the time the unit was off is a pause: timer stop at the last record (the app shows it paused)
                if !self.writer.paused() {
                    let last = self.writer.last_time();
                    self.put(16, |w, sink| w.pause(sink, last));
                }
                Ok(Outcome::Resumed(self.ride, self.name))
            }
            _ => {
                // starting (no ride in the file yet), or a record whose FIT state is damaged: done
                self.write_record(fat, d, Phase::Done, r.cluster)?;
                Ok(Outcome::Nothing)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::fat::Disk;
    use crate::fat::write::tests::{free_counts, fsck};
    use crate::fitread::{Msg, Reader};
    use std::vec::Vec;

    /// The card: the volume in memory, and a power cut after `budget` sectors written (the sector that would be
    /// next and every one after it never lands, within a multi-sector write too); then it never answers again.
    /// `torn`: the sector the cut falls in is left half new, half garbage (a card without reliable write).
    struct Card {
        img: Vec<u8>,
        budget: Option<u64>,
        written: u64,
        dead: bool,
        torn: bool,
        cut_at: u32,
        /// The ride second `ride` was at (the cut's, once the card died).
        now: u32,
    }

    impl Disk for Card {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
            if self.dead {
                return Err(Error::Io);
            }
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.img.get(lba as usize * SECTOR + i).copied().unwrap_or(0);
            }
            Ok(())
        }
    }

    impl DiskWrite for Card {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
            for (k, s) in buf.chunks_exact(SECTOR).enumerate() {
                let at = (lba as usize + k) * SECTOR;
                if self.img.len() < at + SECTOR {
                    self.img.resize(at + SECTOR, 0);
                }
                if self.dead || self.budget.is_some_and(|b| self.written >= b) {
                    if !self.dead {
                        self.cut_at = (at / SECTOR) as u32;
                    }
                    if self.torn && !self.dead {
                        let _ = s; // neither the old nor the new bytes: garbage
                        for (i, b) in self.img[at..at + SECTOR].iter_mut().enumerate() {
                            *b = (i as u8).wrapping_mul(97) ^ 0x5a ^ (at / SECTOR) as u8;
                        }
                    }
                    self.dead = true;
                    return Err(Error::Io);
                }
                self.img[at..at + SECTOR].copy_from_slice(s);
                self.written += 1;
            }
            Ok(())
        }
    }

    const T0: u32 = 1_161_676_800; // 2026-10-23 08:00 UTC
    const SECS: u32 = 900;
    const PAUSE: u32 = 300; // paused from here (the second itself recorded) ...
    const RESUME: u32 = 361; // ... to here
    const LAP: u32 = 500;
    const NAME: [u8; 12] = *b"261023100000";

    fn local(t: u32) -> Local {
        (2026, 10, 23, 10 + (t / 3600) as u8, (t / 60 % 60) as u8, (t % 60) as u8)
    }

    fn sample(t: u32) -> Sample {
        Sample {
            time: T0 + t,
            pos: Some((488_566_000 + 300 * t as i32, 23_522_000 - 200 * t as i32)),
            alt_mm: Some(35_000 + 100 * (t % 50) as i32),
            speed_mm_s: Some(7000 + 10 * (t % 100)),
            hr: (t >= 10).then_some(120 + (t % 40) as u8),
            cadence: (t >= 30).then_some(85 + (t % 10) as u8),
            power: (t >= 30).then_some(200 + (t % 60) as u16),
            temp_c: Some(12),
            alpha1_milli: (t >= 60).then_some(500 + (t % 700) as u16),
            rmssd_ms: (t >= 60).then_some(20 + (t % 40) as u16),
        }
    }

    fn words(t: u32) -> [u32; RIDE_WORDS] {
        let mut w = [0; RIDE_WORDS];
        w[0] = t;
        w
    }

    fn recorded(t: u32) -> bool {
        !(PAUSE + 1..RESUME).contains(&t)
    }

    /// The ride from second `from` (0: Start) to the end and its save, as the app drives the recorder: the last
    /// outcome, None once the card died.
    fn ride(r: &mut Recorder, d: &mut Card, from: u32) -> Option<Outcome> {
        ride_all(core::slice::from_mut(r), d, from).map(|o| o[0])
    }

    /// [`ride`] with several recorders, each event to each in turn and their card work in that order (the app
    /// drives the clean copy's, then the full copy's): the last outcome of each.
    fn ride_all(rs: &mut [Recorder], d: &mut Card, from: u32) -> Option<Vec<Outcome>> {
        let mut last = std::vec![Outcome::Nothing; rs.len()];
        let mut io = |rs: &mut [Recorder], d: &mut Card, t: u32| {
            pass(rs, d, t, &mut last);
            !d.dead
        };
        for t in from..SECS {
            for r in rs.iter_mut() {
                match t {
                    0 => r.start(T0 - 1, NAME, false),
                    _ if t == from => r.resume(T0 + t), // after a reset: the ride was paused
                    RESUME => r.resume(T0 + t),
                    _ => {}
                }
                if recorded(t) {
                    r.second(&sample(t), words(t));
                }
                match t {
                    PAUSE => r.pause(T0 + t, words(t)),
                    LAP => r.lap(T0 + t),
                    _ => {}
                }
            }
            if !io(rs, d, t) {
                return None;
            }
        }
        for r in rs.iter_mut() {
            r.end(T0 + SECS, words(SECS));
        }
        for _ in 0..3 {
            if !io(rs, d, SECS) {
                return None;
            }
        }
        Some(last)
    }

    /// One pass of card work as `App::step_recorder` does it: the clean copy's (`rs[0]`), then the full copy's.
    fn pass(rs: &mut [Recorder], d: &mut Card, t: u32, last: &mut [Outcome]) {
        d.now = t;
        if let [clean, full] = rs {
            clean.keep_up(full);
            full.keep_up(clean);
        }
        if rs[0].due() {
            last[0] = rs[0].io(d, local(t));
        }
        if let ([clean], [full, ..]) = rs.split_at_mut(1)
            && full.due()
        {
            last[1] = full.io_after(clean, d, local(t));
        }
    }

    /// The saved ride's record times and whether it has its session; Err if it is not a whole FIT file.
    fn saved(img: &[u8], name: &str) -> Result<(Vec<u32>, bool), std::string::String> {
        let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, name).map_err(|e| std::format!("{e:?}"))?;
        let mut v = std::vec![0u8; f.size as usize];
        fat.read_at(&mut d, &f, 0, &mut v).unwrap();
        let (mut times, mut session, mut rd) = (Vec::new(), false, Reader::new());
        rd.push_slice(&v, |m| match m {
            Msg::Record(r) => times.push(r.time),
            Msg::Session(_) => session = true,
            Msg::CoursePoint { .. } => {}
        });
        match rd.result() {
            Some(Ok(())) => Ok((times, session)),
            e => Err(std::format!("{e:?}")),
        }
    }

    const SAVED: &str = "Activities/261023100000.fit";
    const SAVED_FULL: &str = "Rustfw/Activities/261023100000.fit";

    fn check_clean(img: &[u8], what: &str) -> u32 {
        let lost = fsck(img).unwrap_or_else(|e| panic!("{what}: {e}"));
        let (free, info) = free_counts(img);
        assert_eq!(free, info, "{what}: FSInfo's free count");
        lost
    }

    /// A ride recorded and saved: in `\Activities` with stock's name, every second in it, nothing left in
    /// `\Rustfw` but the records, a clean volume.
    #[test]
    fn records_and_saves_a_ride() {
        let img = crate::fat::tests::image();
        let mut d = Card { img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        let mut r = Recorder::new();
        assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(NAME)));
        assert_eq!(r.phase(), Phase::Done);
        let (times, session) = saved(&d.img, SAVED).unwrap();
        let want: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        assert_eq!(times, want);
        assert!(session);
        assert_eq!(check_clean(&d.img, "after the save"), 0);
        // for tools/fit_check.py (fitdecode and Garmin's SDK): target/fit/recorded.fit
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let f = fat.open(&mut ram, SAVED).unwrap();
        let mut v = std::vec![0u8; f.size as usize];
        fat.read_at(&mut ram, &f, 0, &mut v).unwrap();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("recorded.fit"), &v).unwrap();
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        assert_eq!(fat.open(&mut ram, "Rustfw/Rides/261023100000.fit"), Err(Error::NotFound));
        assert_eq!(fat.open(&mut ram, "Rustfw/ride.rec").map(|f| f.size), Ok(3072));
        std::println!("{} sectors written for a {SECS} s ride, {} errors, {} dropped", d.written, r.errors, r.dropped);
        assert_eq!((r.errors, r.dropped), (0, 0));
        // a second ride with the same name: the folders are there, and it is saved a second later
        let mut r = Recorder::new();
        let mut d2 = Card { img: d.img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        assert_eq!(ride(&mut r, &mut d2, 0), Some(Outcome::Saved(*b"261023100001")));
        assert!(saved(&d2.img, "Activities/261023100001.fit").is_ok());
        assert!(saved(&d2.img, SAVED).is_ok(), "the first one is still there");
        assert!(d2.written < d.written, "no folders to make: {} >= {}", d2.written, d.written);
        assert_eq!(check_clean(&d2.img, "two rides"), 0);
    }

    /// The golden rule: the power cut after every sector of a whole ride, its start (folders made) and its save
    /// included. After each, the boot's recovery gives back the ride at its last checkpoint (at most a minute
    /// lost), it goes on and is saved, and the volume is clean (FATs equal, FSInfo exact, no chain into a free
    /// cluster, no cross-link; a lost cluster at worst).
    #[test]
    fn power_cut_at_every_sector_write() {
        let img = crate::fat::tests::image();
        let mut full = Card { img: img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        ride(&mut Recorder::new(), &mut full, 0).unwrap();
        let total = full.written;
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        let (mut resumed, mut saved_at_boot, mut nothing, mut max_lost, mut twice_cuts) = (0, 0, 0, 0, 0);
        for cut in 0..total {
            let what = std::format!("cut {cut} of {total}");
            let mut d =
                Card { img: img.clone(), budget: Some(cut), written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
            assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}: the card died");
            // the volume as the cut left it: readable, at most a FAT sector apart
            if let Err(e) = fsck(&d.img)
                && !e.contains("FATs differ")
            {
                panic!("{what}: {e}");
            }
            // reboot; and the reboot cut too, at each of its own writes, then again: the same outcome
            let cut_img = d.img;
            let mut d =
                Card { img: cut_img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
            let mut r = Recorder::new();
            let outcome = r.recover(&mut d, local(SECS + 5));
            for k in 0..d.written {
                let mut twice = Card {
                    img: cut_img.clone(),
                    budget: Some(k),
                    written: 0,
                    dead: false,
                    torn: false,
                    cut_at: 0,
                    now: 0,
                };
                Recorder::new().recover(&mut twice, local(SECS + 5));
                let mut again =
                    Card { img: twice.img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
                let o = Recorder::new().recover(&mut again, local(SECS + 6));
                // a cut save may be finished by the first reboot already: then nothing is left to do
                let same = o == outcome || (matches!(outcome, Outcome::Saved(_)) && o == Outcome::Nothing);
                assert!(same, "{what}, reboot cut at {k}: {o:?}, not {outcome:?}");
                twice_cuts += 1;
            }
            match outcome {
                Outcome::Resumed(w, name) => {
                    assert_eq!(name, NAME);
                    resumed += 1;
                    let from = w[0] + 1;
                    let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                    assert!(ride(&mut r, &mut d, from).is_some(), "{what}");
                    let (times, session) = saved(&d.img, SAVED).unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert!(session, "{what}");
                    assert!(times.windows(2).all(|p| p[0] < p[1]), "{what}: times in order");
                    let lost = all.len() - times.len();
                    assert!(lost <= CHECKPOINT_S as usize, "{what}: {lost} s lost");
                    assert!(times.iter().all(|t| all.contains(t)), "{what}");
                }
                Outcome::Saved(name) => {
                    assert_eq!(name, NAME);
                    saved_at_boot += 1;
                    let (times, session) = saved(&d.img, SAVED).unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert!(session && times == all, "{what}: the whole ride");
                }
                Outcome::Nothing => {
                    nothing += 1;
                    // cut before the first checkpoint's record: nothing was promised, and nothing is saved; the
                    // next ride's start puts the volume right (a cut while making the folders leaves no record)
                    assert!(saved(&d.img, SAVED).is_err(), "{what}");
                    // (a second later if the cut left a half-made file of that name)
                    let Some(Outcome::Saved(n)) = ride(&mut r, &mut d, 0) else { panic!("{what}: the next ride") };
                    let path = std::format!("Activities/{}.fit", core::str::from_utf8(&n).unwrap());
                    assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}");
                }
                Outcome::Failed(_) | Outcome::Kept(_) => panic!("{what}: recovery failed: {outcome:?}"),
            }
            assert_eq!(r.phase(), Phase::Done, "{what}");
            max_lost = max_lost.max(check_clean(&d.img, &what));
        }
        std::println!(
            "{total} cut points: {resumed} resumed, {saved_at_boot} saved at boot, {nothing} before the first \
             checkpoint; lost clusters at most {max_lost}; {twice_cuts} cuts of a reboot"
        );
        assert!(resumed > 50 && saved_at_boot > 0);
        assert!(max_lost <= 2);
    }

    /// The golden rule on a card without reliable write: the power cut at every sector of a whole ride, the sector
    /// being written left as garbage (the ride file's sector 0 and last sector, its directory sector, a FAT
    /// sector, FSInfo, a record). The ride is resumed (at most a minute lost) or saved from its record, never
    /// lost once its first checkpoint is on the card; the next ride is recorded and saved; the volume is clean.
    /// A torn directory sector of `\Activities` or the root (stock's entries) is the card's reliable write's to
    /// prevent (emmc.rs): here it only must not stop the recorder.
    #[test]
    fn power_cut_tearing_the_sector_at_every_write() {
        let img = crate::fat::tests::image();
        let mut full = Card { img: img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        ride(&mut Recorder::new(), &mut full, 0).unwrap();
        let total = full.written;
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        let (mut promised, mut resumed, mut saved_at_boot, mut max_lost, mut torn_root) = (false, 0, 0, 0, 0);
        // the root directory's first cluster (the boot sector's fields: fatgen103)
        let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
        let root_lba = u(14, 2) + u(16, 1) * u(36, 4) + (u(44, 4) - 2) * u(13, 1);
        let root = root_lba..root_lba + u(13, 1);
        for cut in 0..total {
            let mut d =
                Card { img: img.clone(), budget: Some(cut), written: 0, dead: false, torn: true, cut_at: 0, now: 0 };
            assert!(ride(&mut Recorder::new(), &mut d, 0).is_none());
            let what = std::format!("cut {cut} of {total}, sector {} torn", d.cut_at);
            let cut_lba = d.cut_at;
            let had = all.iter().filter(|&&a| a <= T0 + d.now).count(); // the seconds recorded when the power went
            let mut d = Card { img: d.img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
            let mut r = Recorder::new();
            match r.recover(&mut d, local(SECS + 5)) {
                Outcome::Resumed(w, _) => {
                    resumed += 1;
                    let from = w[0] + 1;
                    let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                    assert_eq!(ride(&mut r, &mut d, from), Some(Outcome::Saved(NAME)), "{what}");
                }
                Outcome::Saved(n) => {
                    assert_eq!(n, NAME, "{what}");
                    saved_at_boot += 1;
                }
                Outcome::Nothing => assert!(!promised, "{what}: a ride with a checkpoint lost"),
                o => panic!("{what}: {o:?}"),
            }
            if let Ok((times, _)) = saved(&d.img, SAVED) {
                promised = true;
                assert!(times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)), "{what}");
                let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
                assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {} s lost", had - kept);
            }
            let Some(Outcome::Saved(n)) = ride(&mut Recorder::new(), &mut d, 0) else {
                panic!("{what}: the next ride")
            };
            let path = std::format!("Activities/{}.fit", core::str::from_utf8(&n).unwrap());
            assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}: the next ride");
            let lost = check_clean(&d.img, &what);
            if lost > 2 {
                // only a torn sector of the root directory (making \Rustfw or \Activities): its other entries lost
                assert!(root.contains(&cut_lba), "{what}: {lost} clusters lost");
                torn_root += 1;
            } else {
                max_lost = max_lost.max(lost);
            }
        }
        std::println!(
            "{total} torn cuts: {resumed} resumed, {saved_at_boot} saved at boot; lost clusters at most {max_lost}; \
             {torn_root} in the root directory"
        );
        assert!(resumed > 50 && saved_at_boot > 10);
        assert!(max_lost <= 2);
    }

    fn card(img: Vec<u8>) -> Card {
        Card { img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 }
    }

    /// The first sector of file `path` on the volume `img`.
    fn first_lba(img: &[u8], path: &str) -> usize {
        let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let c = fat.open(&mut ram, path).unwrap().cluster;
        let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
        (u(14, 2) + u(16, 1) * u(36, 4) + (c - 2) * u(13, 1)) as usize
    }

    /// `secs` seconds of ride on `d`, its checkpoints done, not ended (the unit then dies).
    fn open_ride(d: &mut Card, secs: u32) -> Recorder {
        let mut r = Recorder::new();
        r.start(T0 - 1, NAME, false);
        for t in 0..secs {
            r.second(&sample(t), words(t));
            if r.due() {
                assert_eq!(r.io(d, local(t)), Outcome::Nothing);
            }
        }
        r
    }

    /// Calls io (as the main loop would, `RETRY` waits included) until it has nothing left to do: what it said.
    fn io_all(r: &mut Recorder, d: &mut impl DiskWrite, t: u32) -> Vec<Outcome> {
        let mut out = Vec::new();
        for _ in 0..100 * RETRY {
            if r.due() {
                match r.io(d, local(t)) {
                    Outcome::Nothing => {}
                    o => out.push(o),
                }
            }
        }
        out
    }

    /// The second review's R1: one damaged byte in the ride file (a bit error; a sector torn on a card without
    /// reliable write) used to fail the save's read-back forever, and every later Start with it. The ride is kept
    /// as it is in `\Rustfw\Rides`, said once, and the next ride is recorded and saved; a reboot finds nothing to do.
    #[test]
    fn a_damaged_ride_is_kept_and_the_next_one_recorded() {
        let mut d = card(crate::fat::tests::image());
        let mut r = open_ride(&mut d, 200);
        let lba = first_lba(&d.img, "Rustfw/Rides/261023100000.fit");
        d.img[lba * SECTOR + 100] ^= 0x55;
        r.end(T0 + 200, words(200));
        assert_eq!(io_all(&mut r, &mut d, 201), [Outcome::Kept(NAME)]);
        assert_eq!((r.phase(), r.failing()), (Phase::Done, false));
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        assert!(fat.open(&mut ram, "Rustfw/Rides/261023100000.fit").is_ok_and(|f| f.size > 5000), "kept");
        assert_eq!(fat.open(&mut ram, SAVED), Err(Error::NotFound));
        assert_eq!(Recorder::new().recover(&mut d, local(300)), Outcome::Nothing);
        let mut r = Recorder::new();
        assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(*b"261023100001")), "a second later: the name kept");
        assert_eq!(saved(&d.img, "Activities/261023100001.fit").map(|s| s.1), Ok(true));
        assert_eq!(check_clean(&d.img, "after"), 0);
    }

    /// A card that fails every read of one sector of the ride file at the save: tried again [`GIVE_UP`] times (a
    /// minute), then the ride is kept and the next one is recorded and saved.
    #[test]
    fn a_save_that_keeps_failing_is_given_up() {
        struct Bad(Card, usize);
        impl Disk for Bad {
            fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
                let n = buf.len() / SECTOR;
                if (lba as usize..lba as usize + n).contains(&self.1) {
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
        let mut d = card(crate::fat::tests::image());
        let mut r = open_ride(&mut d, 200);
        let lba = first_lba(&d.img, "Rustfw/Rides/261023100000.fit") + 1;
        let mut d = Bad(d, lba);
        r.end(T0 + 200, words(200));
        let out = io_all(&mut r, &mut d, 201);
        assert_eq!(out.len(), GIVE_UP as usize, "{out:?}");
        assert!(out[..out.len() - 1].iter().all(|o| *o == Outcome::Failed(Error::Io)));
        assert_eq!((out[out.len() - 1], r.phase()), (Outcome::Kept(NAME), Phase::Done));
        r.start(T0 + 1000, *b"261023110000", false);
        r.second(&sample(1001), words(1));
        r.end(T0 + 1002, words(2));
        assert_eq!(io_all(&mut r, &mut d, 1003), [Outcome::Saved(*b"261023110000")]);
    }

    /// The second review's R2: Start pressed while the boot's recovery was still pending (the card failed at boot)
    /// used to let `recover` take over the new ride's writer: both were lost. One writer, one ride: `recover` does
    /// nothing once a ride started, and the start finishes the old ride's save first.
    #[test]
    fn start_while_the_boot_recovery_is_pending() {
        let img = crate::fat::tests::image();
        let mut full = card(img.clone());
        ride(&mut Recorder::new(), &mut full, 0).unwrap();
        // a cut during the save: the next boot would finish it
        let cut_img = (0..full.written)
            .rev()
            .map(|cut| {
                let mut d = Card { budget: Some(cut), ..card(img.clone()) };
                let _ = ride(&mut Recorder::new(), &mut d, 0);
                d.img
            })
            .find(|i| matches!(Recorder::new().recover(&mut card(i.clone()), local(SECS + 5)), Outcome::Saved(_)))
            .unwrap();
        let mut d = card(cut_img);
        let mut r = Recorder::new();
        let name2 = *b"261023120000";
        r.start(T0 + 5000, name2, true);
        r.second(&sample(5000), words(0));
        assert_eq!(r.recover(&mut d, local(5000)), Outcome::Nothing);
        assert_eq!(r.phase(), Phase::Starting);
        let mut out = Vec::new();
        for t in 1..200 {
            r.second(&sample(5000 + t), words(t));
            if r.due() && r.io(&mut d, local(5000 + t)) != Outcome::Nothing {
                out.push(t);
            }
        }
        r.end(T0 + 5200, words(200));
        assert_eq!(out, [1], "the old ride saved at the first io");
        assert_eq!(io_all(&mut r, &mut d, 5201), [Outcome::Saved(name2)]);
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        assert_eq!(saved(&d.img, SAVED).map(|s| s.0 == all), Ok(true), "the old ride whole");
        assert_eq!(saved(&d.img, "Activities/261023120000.fit").map(|s| s.0.len()), Ok(200), "the new ride");
        assert_eq!(check_clean(&d.img, "after"), 0);
    }

    /// The second review's R6: a ride left open whose boot marker was lost (a flat battery) used to be resumed by
    /// the next Start, days later, in place of the new ride. Now it is resumed only if it stopped less than
    /// [`RESUME_S`] ago (or the marker said so); else it is ended at its last record and saved, and the new ride
    /// starts.
    #[test]
    fn an_old_ride_left_open_is_saved_and_a_new_one_started() {
        let mut d = card(crate::fat::tests::image());
        drop(open_ride(&mut d, 200));
        let img = d.img;
        let name2 = *b"261025100000";
        for (later, marker, resumed) in [(2 * 86_400, false, false), (600, false, true), (2 * 86_400, true, true)] {
            let what = std::format!("{later} s later, marker {marker}");
            let mut d = card(img.clone());
            let mut r = Recorder::new();
            r.start(T0 + 200 + later, name2, marker);
            r.second(&sample(200 + later), words(0));
            let out = io_all(&mut r, &mut d, 200);
            assert_eq!(r.phase(), Phase::Recording, "{what}");
            if resumed {
                assert!(matches!(out[..], [Outcome::Resumed(_, NAME)]), "{what}: {out:?}");
                continue;
            }
            assert_eq!(out, [Outcome::Saved(NAME)], "{what}");
            let (times, session) = saved(&d.img, SAVED).unwrap();
            assert!(session && (140..=200).contains(&times.len()), "{what}: {} records", times.len());
            r.end(T0 + 202 + later, words(2));
            assert_eq!(io_all(&mut r, &mut d, 300), [Outcome::Saved(name2)], "{what}");
            assert_eq!(check_clean(&d.img, &what), 0);
        }
    }

    fn pair() -> [Recorder; 2] {
        [Recorder::new(), Recorder::of(Kind::Full)]
    }

    /// The two copies of a ride: the clean one in `\Activities` as with one recorder (no developer data), the full
    /// one in `\Rustfw\Activities` with the same records and `Alpha1` / `RMSSD` in each; the cost of the second.
    #[test]
    fn two_copies_cost() {
        let img = crate::fat::tests::image();
        let mut one = card(img.clone());
        ride(&mut Recorder::new(), &mut one, 0).unwrap();
        let mut d = card(img);
        assert_eq!(ride_all(&mut pair(), &mut d, 0), Some(std::vec![Outcome::Saved(NAME); 2]));
        let read = |img: &[u8], path: &str| {
            let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
            let mut fat = Fat::mount(&mut ram).unwrap();
            crate::fat::tests::read_all(&mut fat, &mut ram, path).unwrap()
        };
        let clean = read(&d.img, SAVED);
        assert_eq!(clean, read(&one.img, SAVED), "the clean copy as with one recorder");
        assert_eq!(clean[1], 0x10);
        let full = read(&d.img, SAVED_FULL);
        assert_eq!(full[1], 0x20);
        let (times, session) = saved(&d.img, SAVED_FULL).unwrap();
        assert_eq!((times, session), saved(&d.img, SAVED).unwrap());
        assert!(full.windows(6).any(|w| w == b"Alpha1"));
        assert_eq!(check_clean(&d.img, "two copies"), 0);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("recorded_full.fit"), &full).unwrap();
        std::println!(
            "{SECS} s ride: {} sectors written with one copy, {} with two ({} B and {} B files)",
            one.written,
            d.written,
            clean.len(),
            full.len()
        );
        // a second ride the same second: both copies a second later, still one name
        let mut d2 = card(d.img.clone());
        let n2 = *b"261023100001";
        assert_eq!(ride_all(&mut pair(), &mut d2, 0), Some(std::vec![Outcome::Saved(n2); 2]));
        assert!(saved(&d2.img, "Rustfw/Activities/261023100001.fit").is_ok());
        // the cost once the folders are there: a second ride with one copy, and with two
        let mut one2 = card(one.img.clone());
        ride(&mut Recorder::new(), &mut one2, 0).unwrap();
        // the other way (docs/sync.md): the full copy alone, the clean one made from it at the save (its sectors)
        let mut full2 = card(d.img.clone());
        ride(&mut Recorder::of(Kind::Full), &mut full2, 0).unwrap();
        std::println!(
            "the next {SECS} s ride: {} sectors with one copy, {} with two, {} with the full one only (+{} to write \
             the clean file at the save)",
            one2.written,
            d2.written,
            full2.written,
            clean.len().div_ceil(SECTOR)
        );
        assert!(d2.written < 3 * one2.written);
    }

    /// The golden rule with two copies: the power cut (the sector left whole, or torn) after every sector of a ride
    /// recorded by both. The clean copy comes back as with one recorder (resumed or saved at boot, at most a minute
    /// lost); the full one is resumed or saved too, or, cut before its first checkpoint, never promised; a full
    /// one back without its clean one is ended and saved (the app's rule). The next ride gets both, the volume
    /// stays clean.
    #[test]
    fn two_copies_power_cut_at_every_sector() {
        for torn in [false, true] {
            two_copies_cut(torn);
        }
    }

    fn two_copies_cut(torn: bool) {
        let img = crate::fat::tests::image();
        let mut whole = card(img.clone());
        ride_all(&mut pair(), &mut whole, 0).unwrap();
        let total = whole.written;
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        let (mut both, mut clean_only, mut none, mut max_lost) = (0, 0, 0, 0);
        for cut in 0..total {
            let mut d = Card { budget: Some(cut), torn, ..card(img.clone()) };
            assert!(ride_all(&mut pair(), &mut d, 0).is_none());
            let what = std::format!("torn {torn}, cut {cut} of {total} (sector {})", d.cut_at);
            let had = all.iter().filter(|&&a| a <= T0 + d.now).count();
            let mut d = card(d.img);
            let mut rs = pair();
            let o = [rs[0].recover(&mut d, local(SECS + 5)), rs[1].recover(&mut d, local(SECS + 5))];
            if matches!(o[1], Outcome::Resumed(..)) {
                let [clean, full] = &mut rs;
                full.resumed_after(clean); // as App::step_recorder
            }
            match o[0] {
                Outcome::Resumed(w, _) => {
                    let from = w[0] + 1;
                    let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                    let out = ride_all(&mut rs, &mut d, from).unwrap_or_else(|| panic!("{what}"));
                    assert_eq!(out[0], Outcome::Saved(NAME), "{what}");
                }
                Outcome::Saved(n) => assert_eq!(n, NAME, "{what}"),
                Outcome::Nothing => {}
                o => panic!("{what}: {o:?}"),
            }
            io_all(&mut rs[1], &mut d, SECS + 10);
            assert_eq!([rs[0].phase(), rs[1].phase()], [Phase::Done; 2], "{what}");
            for (path, kind) in [(SAVED, "clean"), (SAVED_FULL, "full")] {
                if let Ok((times, _)) = saved(&d.img, path) {
                    assert!(
                        times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)),
                        "{what}: {kind} {:?}",
                        times.iter().map(|t| t - T0).collect::<Vec<_>>()
                    );
                    let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
                    assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {kind}: {} s lost", had - kept);
                }
            }
            match (saved(&d.img, SAVED).is_ok(), saved(&d.img, SAVED_FULL).is_ok()) {
                (true, true) => both += 1,
                (true, false) => clean_only += 1,
                (false, true) => panic!("{what}: a full copy without its clean one"),
                _ => none += 1,
            }
            // the next ride: both copies
            let out = ride_all(&mut pair(), &mut d, 0).unwrap_or_else(|| panic!("{what}: the next ride"));
            assert!(matches!(out[..], [Outcome::Saved(a), Outcome::Saved(b)] if a == b), "{what}: {out:?}");
            let lost = fsck(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
            if !torn {
                max_lost = max_lost.max(lost);
            }
        }
        std::println!(
            "two copies, torn {torn}: {total} cuts: both saved {both}, clean only {clean_only}, neither {none} \
             (cut before the first checkpoint); lost clusters at most {max_lost}"
        );
        assert!(both > total as usize / 2);
        assert!(max_lost <= 4);
    }

    /// A full copy that fails (its file damaged: the save's read-back finds it) is kept as it is and said; the
    /// clean one is saved all the same, and the next ride gets both.
    #[test]
    fn a_failing_full_copy_never_stops_the_clean_one() {
        let mut d = card(crate::fat::tests::image());
        let mut rs = pair();
        for r in rs.iter_mut() {
            r.start(T0 - 1, NAME, false);
        }
        for t in 0..200 {
            for r in rs.iter_mut() {
                r.second(&sample(t), words(t));
                if r.due() {
                    assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
                }
            }
        }
        let lba = first_lba(&d.img, "Rustfw/Full/261023100000.fit");
        d.img[lba * SECTOR + 100] ^= 0x55;
        for r in rs.iter_mut() {
            r.end(T0 + 200, words(200));
        }
        assert_eq!(io_all(&mut rs[0], &mut d, 201), [Outcome::Saved(NAME)]);
        assert_eq!(io_all(&mut rs[1], &mut d, 201), [Outcome::Kept(NAME)]);
        assert_eq!(saved(&d.img, SAVED).map(|s| (s.0.len(), s.1)), Ok((200, true)));
        assert!(saved(&d.img, SAVED_FULL).is_err());
        let out = ride_all(&mut pair(), &mut d, 0).unwrap();
        assert_eq!(out, [Outcome::Saved(*b"261023100001"); 2]);
        assert_eq!(check_clean(&d.img, "after"), 0);
    }

    /// Every second of ride from `from` to `to` (excluded) to each recorder, a pass of card work after each.
    fn seconds(rs: &mut [Recorder], d: &mut Card, from: u32, to: u32, last: &mut [Outcome]) {
        for t in from..to {
            for r in rs.iter_mut() {
                r.second(&sample(t), words(t));
            }
            pass(rs, d, t, last);
        }
    }

    /// Seen on the unit (2026-10-24): after a reset mid-ride the clean copy had 22 s fewer than the full one. The
    /// full copy's records are bigger, so its buffer ran low ([`LOW`]) and it checkpointed before its minute was up;
    /// the clean one waited for its minute. The two checkpoint together now: at any cut the clean copy holds every
    /// second the full one does.
    #[test]
    fn the_clean_copy_is_never_behind_the_full_one() {
        let mut d = card(crate::fat::tests::image());
        let (mut rs, mut last, mut at) = (pair(), [Outcome::Nothing; 2], [0u32; 2]);
        for r in rs.iter_mut() {
            r.start(T0 - 1, NAME, false);
        }
        for t in 0..SECS {
            seconds(&mut rs, &mut d, t, t + 1, &mut last);
            for (a, r) in at.iter_mut().zip(&rs) {
                if r.since == 0 {
                    *a = t; // checkpointed in this pass
                }
            }
            assert!(
                at[0] >= at[1],
                "second {t}: the clean copy's checkpoint at {}, the full one's at {}",
                at[0],
                at[1]
            );
        }
        assert_eq!((rs[0].dropped, rs[1].dropped), (0, 0));
    }

    /// The full copy's folder damaged (its entry in `\Rustfw` points out of the volume): the clean copy still
    /// records and saves the next ride.
    #[test]
    fn a_damaged_full_folder_never_stops_the_clean_copy() {
        let mut d = card(crate::fat::tests::image());
        ride_all(&mut pair(), &mut d, 0).unwrap();
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let c = Fat::mount(&mut ram).unwrap().open(&mut ram, FULL_PATH).unwrap().cluster;
        let lba = first_lba(&d.img, DIR);
        let cluster = |e: &[u8]| u32::from_le_bytes([e[26], e[27], e[20], e[21]]);
        let at = (lba * SECTOR..(lba + 1) * SECTOR)
            .step_by(32)
            .find(|&at| d.img[at] != b'.' && d.img[at + 11] & 0x10 != 0 && cluster(&d.img[at..at + 32]) == c)
            .unwrap();
        d.img[at + 20..at + 22].copy_from_slice(&[0xff, 0x0f]);
        d.img[at + 26..at + 28].copy_from_slice(&[0xf0, 0xff]);
        let out = ride_all(&mut pair(), &mut d, 0).unwrap();
        assert_eq!(out[0], Outcome::Saved(*b"261023100001"));
        let (times, session) = saved(&d.img, "Activities/261023100001.fit").unwrap();
        assert_eq!((times.len(), session), (SECS as usize - 60, true));
    }

    /// A full copy left open whose clean copy was saved (a cut between the two saves), then a new ride with the
    /// boot's marker lost: both copies start the new ride; the old full copy is saved as it was, never carried on
    /// with the new ride's seconds (a full copy goes on only with the clean copy's ride).
    #[test]
    fn the_full_copy_goes_on_only_with_the_clean_copys_ride() {
        let mut d = card(crate::fat::tests::image());
        let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
        for r in rs.iter_mut() {
            r.start(T0 - 1, NAME, false);
        }
        seconds(&mut rs, &mut d, 0, 200, &mut last);
        for r in rs.iter_mut() {
            r.end(T0 + 200, words(200));
        }
        assert_eq!(io_all(&mut rs[0], &mut d, 200), [Outcome::Saved(NAME)]);
        // the cut: the full copy's save never ran. A new ride ten minutes on
        let new = *b"261023101000";
        let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
        for r in rs.iter_mut() {
            r.start(T0 + 599, new, false);
        }
        seconds(&mut rs, &mut d, 600, 700, &mut last);
        for r in rs.iter_mut() {
            r.end(T0 + 700, words(700));
        }
        for _ in 0..3 {
            pass(&mut rs, &mut d, 700, &mut last);
        }
        assert_eq!(last, [Outcome::Saved(new); 2]);
        let (old, _) = saved(&d.img, SAVED_FULL).unwrap();
        assert!(old.iter().all(|&t| t < T0 + 200), "the old full copy as it was: {:?}", old.last());
        let clean = saved(&d.img, "Activities/261023101000.fit").unwrap();
        assert_eq!(saved(&d.img, "Rustfw/Activities/261023101000.fit"), Ok(clean));
        assert_eq!(check_clean(&d.img, "after"), 0);
    }

    /// A card nearly full: what is left is the clean copy's. With room for about one and a half clean rides, the
    /// full copy is not made and the clean one is recorded whole and saved.
    #[test]
    fn a_nearly_full_card_is_left_to_the_clean_copy() {
        let img = crate::fat::tests::image();
        let mut one = card(img.clone());
        ride(&mut Recorder::new(), &mut one, 0).unwrap();
        let keep = (free_counts(&img).0 - free_counts(&one.img).0) * 3 / 2;
        // the other free clusters marked bad in both FATs (0x0ffffff7: neither free nor lost), FSInfo told
        let mut img = img;
        let u = |img: &[u8], at: usize, n: usize| {
            img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b))
        };
        let (fat0, fat_len, fsinfo) = (u(&img, 14, 2) * SECTOR, u(&img, 36, 4) * SECTOR, u(&img, 48, 2) * SECTOR);
        let (mut free, mut kept) = (0u32, 0u32);
        for at in (fat0 + 8..fat0 + fat_len).step_by(4) {
            if u(&img, at, 4) & 0x0fff_ffff == 0 {
                free += 1;
                if free > keep {
                    for k in [0, fat_len] {
                        img[at + k..at + k + 4].copy_from_slice(&0x0fff_fff7u32.to_le_bytes());
                    }
                } else {
                    kept += 1;
                }
            }
        }
        img[fsinfo + 488..fsinfo + 492].copy_from_slice(&kept.to_le_bytes());
        let mut d = card(img);
        let out = ride_all(&mut pair(), &mut d, 0).unwrap();
        assert_eq!(out[0], Outcome::Saved(NAME));
        let (times, session) = saved(&d.img, SAVED).unwrap();
        assert_eq!((times.len(), session), (SECS as usize - 60, true), "the clean copy whole");
        assert!(saved(&d.img, SAVED_FULL).is_err());
        assert_eq!(check_clean(&d.img, "after"), 0);
    }

    /// A damaged record is never taken.
    #[test]
    fn a_damaged_record_is_refused() {
        let r = |seq, phase| Rec { seq, phase, name: NAME, cluster: 7, fit: [0; RECORD_LEN], ride: [seq; RIDE_WORDS] };
        let s = r(5, Phase::Recording).to_sector();
        assert_eq!(Rec::from_sector(&s), Some(r(5, Phase::Recording)));
        for at in [0, 4, 8, 9, 24, 40, 100, 511] {
            let mut bad = s;
            bad[at] ^= 0x10;
            assert_eq!(Rec::from_sector(&bad), None, "byte {at}");
        }
    }
}
