//! The ride recorder (AGENTS.md, "Golden rule: never lose a ride"): the ride as a FIT file ([`fit::Writer`]) on
//! stock's FAT volume ([`fat::write`]), a checkpoint at least every minute, two recovery records, a resume after any
//! reset or power cut, and a save that cannot lose the ride.
//!
//! **Files** (AGENTS.md, "Share the storage with stock"): the ride in progress is `\Rustfw\YYMMDDhhmmss.fit` (local
//! start time, stock's name) and its recovery records `\Rustfw\ride.rec`, in this firmware's own folder, so neither
//! firmware resumes the other's ride; a saved ride is `\Activities\YYMMDDhhmmss.fit`, where stock, its app, the
//! history and a computer find it. Stock's `\Activities\tmp.fit` and `\System\tmp*.fit` are never touched.
//!
//! **RAM, not the card, each second.** A record is about 30 bytes, appended to a [`TAIL`]-byte buffer that holds the
//! file from the sector the next byte goes in. The card is written at a checkpoint only: every [`CHECKPOINT_S`]
//! seconds of ride, at a pause, at the end, and earlier when the buffer has less than [`LOW`] bytes left (a minute
//! of every field is about 1.9 KB with the sector carried over). So the card sees about four commands a minute and
//! a power cut loses at most the last minute, as stock (ghidra/STORAGE_AND_MAPS.md 3: a checkpoint every 60
//! samples). Writing each sector as it fills would not shorten that (the file and its size are only valid at a
//! checkpoint) and would cost more commands.
//!
//! **A checkpoint**, in this order: the FIT trailer and header in the buffer ([`fit::Writer::checkpoint`]); the
//! new sectors (clusters taken first, [`fat::write`]); sector 0 with the new header; the size in the directory
//! entry; then the recovery record. A cut before the record leaves the previous record, whose file is the bytes
//! up to its length, which the writes after it never changed except its 2-byte trailer and the header, which the
//! resume writes again from the record.
//!
//! **The recovery record** is one sector, written alternately to sector 0 and 1 of `ride.rec` (a cut while
//! writing one leaves the other whole): a tag, a sequence number (the larger valid one wins: never the data
//! length, so a finished ride's record cannot pass for a new ride's), the phase (done, starting, recording, saving),
//! the file's identity (its name and first cluster: a resume opens only that file and checks both), the FIT
//! writer's state ([`fit::RECORD_LEN`] bytes, its own CRC) and the app's ride totals ([`ride::Ride::to_words`]);
//! the sector's CRC last.
//!
//! **The save** (End), each step done again after a reset by [`Recorder::recover`] from the phase in the record:
//! 1. the end messages and a checkpoint whose record says "saving";
//! 2. the file read back whole from the card: its FIT CRC must be right (a bad read keeps everything and retries);
//! 3. its entry in `\Rustfw` marked deleted (0xe5 only: [`fat::write`]);
//! 4. a new entry `\Activities\YYMMDDhhmmss.fit` on the same chain (a name taken by another file: a second later);
//! 5. a record "done".
//!
//! Between 3 and 4 the chain is named by no entry for a moment (a lost chain, never a cross-link); the record
//! names it, and 4 is done again at the next start. So the ride in progress exists until the saved one does.
//!
//! **After a cut** the second FAT may differ from the first in the sector being written and FSInfo's free count
//! may be a few clusters high: [`fat::Fat::sync_fats`] runs whenever a record is found unfinished, and when the
//! folders are made (a cut there leaves no record).
//!
//! Errors: nothing here panics; a failing card leaves the data in RAM and is tried again after [`RETRY`] calls;
//! samples that do not fit the buffer meanwhile are counted ([`Recorder::dropped`]), never the file lost.

use crate::fat::write::{Handle, dos_time};
use crate::fat::{DiskWrite, Error, Fat, File, SECTOR};
use crate::fit::{self, RECORD_LEN, Sample, Sink, Writer};
use crate::ride::RIDE_WORDS;

/// This firmware's folder, its recovery records, stock's rides folder.
pub const DIR: &str = "Rustfw";
/// See [`DIR`].
pub const REC: &str = "ride.rec";
/// See [`DIR`].
pub const ACTIVITIES: &str = "Activities";
/// Bytes of file buffered in RAM.
pub const TAIL: usize = 2048;
/// Seconds of ride between checkpoints at most.
pub const CHECKPOINT_S: u32 = 60;
/// A checkpoint is due when the buffer has less than this left (room for some seconds more if it fails).
pub const LOW: usize = 512;
/// Calls of [`Recorder::io`] to wait after an error before trying the card again.
pub const RETRY: u32 = 100;
/// Bytes a record message takes at most, with a new definition and the trailer after it.
const RECORD_MAX: usize = 6 + 3 * 10 + 1 + 27 + 2;
/// Record tag (and layout version).
const TAG: [u8; 4] = *b"RFR1";

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
        if s[..4] != TAG || fit::crc(0, &s[..SECTOR - 2]).to_le_bytes() != s[SECTOR - 2..] {
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
    /// Calls to wait before trying the card again.
    wait: u32,
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
    /// No ride.
    pub const fn new() -> Self {
        Recorder {
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
            wait: 0,
            dropped: 0,
            errors: 0,
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
    /// made at the next [`Recorder::io`]). Ignored while a ride is still being saved.
    pub fn start(&mut self, time: u32, name: [u8; 12]) {
        if self.phase != Phase::Done {
            return;
        }
        (self.tail, self.tail_off, self.len, self.head) = ([0; TAIL], 0, 0, [0; 14]);
        let (mut sink, _) = self.ram();
        let Ok(w) = Writer::begin(&mut sink, time) else { return };
        (self.writer, self.last_rec) = (w, time);
        self.len = 14 + w.data_len();
        (self.phase, self.file, self.name, self.since, self.want, self.ended) =
            (Phase::Starting, None, name, 0, true, false);
    }

    /// One second of ride: its record, and the app's totals for the next recovery record.
    pub fn second(&mut self, s: &Sample, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        // one record a second, each later than the last (seconds caught up after a long pass share a clock)
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
        self.want && self.phase != Phase::Done
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
            Ok(o) => o,
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                self.wait = RETRY;
                Outcome::Failed(e)
            }
        }
    }

    fn io_inner(&mut self, d: &mut impl DiskWrite, now: Local) -> Result<Outcome, Error> {
        let t = dos_time(now);
        let mut fat = Fat::mount(d)?;
        if self.phase == Phase::Starting {
            let dir = self.setup(&mut fat, d, t)?;
            let best = self.read_records(&mut fat, d)?;
            if best.is_some_and(|r| r.phase != Phase::Done) {
                // what the last run left unfinished (the boot did not see it: its marker was lost) comes first; a
                // ride in progress goes on instead of this one
                let mut old = Recorder::new();
                (old.rec, old.seq) = (self.rec, self.seq);
                let o = old.recover_from(&mut fat, d, best, t)?;
                if matches!(o, Outcome::Resumed(..)) {
                    *self = old;
                    return Ok(o);
                }
                self.seq = old.seq;
            }
            if self.file.is_none() {
                let acts = fat.open(d, ACTIVITIES)?;
                let mut buf = [0u8; 16];
                for _ in 0..10 {
                    let n = file_name(&self.name, &mut buf)?;
                    let free = |r: Result<File, Error>| matches!(r, Err(Error::NotFound));
                    if free(fat.find(d, dir, n)) && free(fat.find(d, acts, n)) {
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
            self.save(&mut fat, d, t, first, size)?;
            (self.phase, self.want) = (Phase::Done, false);
            return Ok(Outcome::Saved(self.name));
        }
        Ok(Outcome::Nothing)
    }

    /// Makes `\Rustfw`, `\Rustfw\ride.rec` (two zeroed sectors) and `\Activities` when missing (the FATs synced
    /// first: a cut while making them leaves no record to say so). `\Rustfw`.
    fn setup(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let root = fat.root();
        let dir = fat.find(d, root, DIR);
        let rec = match dir {
            Ok(f) => fat.locate(d, f, REC).ok(),
            Err(_) => None,
        };
        let whole =
            dir.is_ok() && rec.is_some_and(|r| r.size >= 2 * SECTOR as u32) && fat.find(d, root, ACTIVITIES).is_ok();
        if !whole {
            fat.sync_fats(d)?;
        }
        let dir = fat.ensure_dir(d, root, DIR, t)?;
        fat.ensure_dir(d, root, ACTIVITIES, t)?;
        let mut rec = match fat.locate(d, dir, REC) {
            Ok(h) => h,
            Err(Error::NotFound) => fat.create(d, dir, REC, t)?,
            Err(e) => return Err(e),
        };
        if rec.size < 2 * SECTOR as u32 {
            fat.write_sectors(d, &mut rec, 0, &[0; 2 * SECTOR])?;
            fat.set_size(d, &mut rec, 2 * SECTOR as u32, t)?;
        }
        self.rec = Some(rec);
        Ok(dir)
    }

    /// The newer valid record of the two (its sequence number taken as the last one used); None if neither is.
    fn read_records(&mut self, fat: &mut Fat, d: &mut impl DiskWrite) -> Result<Option<Rec>, Error> {
        let rec = self.rec.ok_or(Error::NotFound)?;
        let mut best: Option<Rec> = None;
        for k in 0..2 {
            let mut s = [0u8; SECTOR];
            fat.read_sector(d, &rec, k, &mut s)?;
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
        let mut rec = self.rec.ok_or(Error::NotFound)?;
        let seq = self.seq.wrapping_add(1);
        let r = Rec { seq, phase, name: self.name, cluster, fit: self.writer.to_record(), ride: self.ride };
        fat.write_sectors(d, &mut rec, seq % 2, &r.to_sector())?;
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
        if self.tail_off > 0 {
            let mut s = [0u8; SECTOR];
            fat.read_sector(d, &file, 0, &mut s)?;
            s[..14].copy_from_slice(&self.head);
            fat.write_sectors(d, &mut file, 0, &s)?;
        }
        fat.set_size(d, &mut file, self.len, t)?;
        self.file = Some(file);
        self.write_record(fat, d, phase, file.first)?;
        // the buffer keeps the sector the next byte (over the trailer) goes in
        let next = (self.len - 2) / SECTOR as u32 * SECTOR as u32;
        let from = (next - self.tail_off) as usize;
        self.tail.copy_within(from.., 0);
        let keep = (self.len - next) as usize;
        if let Some(rest) = self.tail.get_mut(keep..) {
            rest.fill(0);
        }
        (self.tail_off, self.since, self.want) = (next, 0, false);
        Ok(())
    }

    /// The save from step 2 on (module doc): the file at `first`, `size` bytes, checked, moved, the record done.
    fn save(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        t: (u16, u16),
        first: u32,
        size: u32,
    ) -> Result<(), Error> {
        // 2: read back whole, through the buffer (the file is all on the card by now)
        let f = File { cluster: first, size, dir: false };
        let (mut at, mut crc) = (0u32, 0u16);
        while at < size {
            let n = fat.read_at(d, &f, at, &mut self.tail)?;
            if at == 0 && (n < 14 || self.tail[4..8] != (size.saturating_sub(16)).to_le_bytes()) {
                return Err(Error::Corrupt);
            }
            crc = fit::crc(crc, self.tail.get(..n).unwrap_or(&[]));
            at += n.max(1) as u32;
        }
        if crc != 0 || size < 16 {
            return Err(Error::Corrupt);
        }
        // 3: the entry in the folder, if still there and still this chain
        let dir = fat.open(d, DIR)?;
        let mut buf = [0u8; 16];
        let n = file_name(&self.name, &mut buf)?;
        match fat.locate(d, dir, n) {
            Ok(h) if h.first == first => fat.remove(d, &h)?,
            Ok(_) | Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        // 4: the new entry, unless there already (a name another file has: a second later)
        let acts = fat.ensure_dir(d, fat.root(), ACTIVITIES, t)?;
        let mut name = self.name;
        for _ in 0..10 {
            let n = file_name(&name, &mut buf)?;
            match fat.find(d, acts, n) {
                Ok(f) if f.cluster == first => break,
                Ok(_) => name = next_name(&name),
                Err(Error::NotFound) => {
                    let h = Handle { first, size, ..Handle::default() };
                    fat.link(d, acts, n, &h, t)?;
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        self.name = name;
        // 5
        self.write_record(fat, d, Phase::Done, first)
    }

    /// At boot: a ride the last run did not finish is reopened at its last checkpoint (the app shows it paused),
    /// or its save is finished. Reads only `\Rustfw\ride.rec` when there is nothing to do.
    /// After a failure, waits [`RETRY`] calls before trying the card again.
    #[inline(never)]
    pub fn recover(&mut self, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if self.wait > 0 {
            self.wait -= 1;
            return Outcome::Nothing;
        }
        let t = dos_time(now);
        let run = |s: &mut Self, d: &mut _| -> Result<Outcome, Error> {
            let mut fat = Fat::mount(d)?;
            let dir = fat.open(d, DIR)?;
            s.rec = Some(fat.locate(d, dir, REC)?);
            let best = s.read_records(&mut fat, d)?;
            s.recover_from(&mut fat, d, best, t)
        };
        match run(self, d) {
            Ok(o) => o,
            Err(Error::NotFound) => Outcome::Nothing, // never recorded here
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                self.wait = RETRY;
                Outcome::Failed(e)
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
    ) -> Result<Outcome, Error> {
        let Some(r) = best.filter(|r| r.phase != Phase::Done) else { return Ok(Outcome::Nothing) };
        fat.sync_fats(d)?; // the cut may have been between the two FATs
        let writer = Writer::from_record(&r.fit);
        (self.name, self.ride) = (r.name, r.ride);
        let size = writer.map_or(0, |w| 16 + w.data_len());
        match (r.phase, writer) {
            (Phase::Saving, Some(w)) => {
                self.writer = w;
                self.save(fat, d, t, r.cluster, size)?;
                Ok(Outcome::Saved(self.name))
            }
            (Phase::Recording, Some(w)) => {
                let dir = fat.open(d, DIR)?;
                let mut buf = [0u8; 16];
                let file = match fat.locate(d, dir, file_name(&r.name, &mut buf)?) {
                    Ok(h) if h.first == r.cluster && h.clusters * fat.cluster_bytes() >= size => h,
                    Ok(_) | Err(Error::NotFound) => {
                        // not this ride's file any more (a computer removed it): nothing to resume
                        self.writer = w;
                        self.write_record(fat, d, Phase::Done, r.cluster)?;
                        return Ok(Outcome::Nothing);
                    }
                    Err(e) => return Err(e),
                };
                // the buffer from the sector the trailer starts in; the header from sector 0
                (self.writer, self.last_rec) = (w, w.last_time());
                (self.file, self.len) = (Some(file), size);
                self.tail_off = (size - 2) / SECTOR as u32 * SECTOR as u32;
                self.tail = [0; TAIL];
                let mut s = [0u8; SECTOR];
                fat.read_sector(d, &file, self.tail_off / SECTOR as u32, &mut s)?;
                let keep = (size - self.tail_off) as usize;
                self.tail[..keep.min(SECTOR)].copy_from_slice(&s[..keep.min(SECTOR)]);
                fat.read_sector(d, &file, 0, &mut s)?;
                self.head.copy_from_slice(&s[..14]);
                (self.phase, self.ended) = (Phase::Recording, false);
                // the file cut back to the checkpoint, its trailer and header written again from the record
                self.checkpoint(fat, d, t, Phase::Recording)?;
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
    struct Card {
        img: Vec<u8>,
        budget: Option<u64>,
        written: u64,
        dead: bool,
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
                if self.dead || self.budget.is_some_and(|b| self.written >= b) {
                    self.dead = true;
                    return Err(Error::Io);
                }
                let at = (lba as usize + k) * SECTOR;
                if self.img.len() < at + SECTOR {
                    self.img.resize(at + SECTOR, 0);
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
        let mut last = Outcome::Nothing;
        let mut io = |r: &mut Recorder, d: &mut Card, t: u32| {
            if r.due() {
                last = r.io(d, local(t));
            }
            !d.dead
        };
        for t in from..SECS {
            match t {
                0 => r.start(T0 - 1, NAME),
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
            if !io(r, d, t) {
                return None;
            }
        }
        r.end(T0 + SECS, words(SECS));
        for _ in 0..3 {
            if !io(r, d, SECS) {
                return None;
            }
        }
        Some(last)
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
        });
        match rd.result() {
            Some(Ok(())) => Ok((times, session)),
            e => Err(std::format!("{e:?}")),
        }
    }

    const SAVED: &str = "Activities/261023100000.fit";

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
        let mut d = Card { img, budget: None, written: 0, dead: false };
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
        assert_eq!(fat.open(&mut ram, "Rustfw/261023100000.fit"), Err(Error::NotFound));
        assert_eq!(fat.open(&mut ram, "Rustfw/ride.rec").map(|f| f.size), Ok(1024));
        std::println!("{} sectors written for a {SECS} s ride, {} errors, {} dropped", d.written, r.errors, r.dropped);
        assert_eq!((r.errors, r.dropped), (0, 0));
        // a second ride with the same name: the folders are there, and it is saved a second later
        let mut r = Recorder::new();
        let mut d2 = Card { img: d.img.clone(), budget: None, written: 0, dead: false };
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
        let mut full = Card { img: img.clone(), budget: None, written: 0, dead: false };
        ride(&mut Recorder::new(), &mut full, 0).unwrap();
        let total = full.written;
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        let (mut resumed, mut saved_at_boot, mut nothing, mut max_lost, mut twice_cuts) = (0, 0, 0, 0, 0);
        for cut in 0..total {
            let what = std::format!("cut {cut} of {total}");
            let mut d = Card { img: img.clone(), budget: Some(cut), written: 0, dead: false };
            assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}: the card died");
            // the volume as the cut left it: readable, at most a FAT sector apart
            if let Err(e) = fsck(&d.img)
                && !e.contains("FATs differ")
            {
                panic!("{what}: {e}");
            }
            // reboot; and the reboot cut too, at each of its own writes, then again: the same outcome
            let cut_img = d.img;
            let mut d = Card { img: cut_img.clone(), budget: None, written: 0, dead: false };
            let mut r = Recorder::new();
            let outcome = r.recover(&mut d, local(SECS + 5));
            for k in 0..d.written {
                let mut twice = Card { img: cut_img.clone(), budget: Some(k), written: 0, dead: false };
                Recorder::new().recover(&mut twice, local(SECS + 5));
                let mut again = Card { img: twice.img, budget: None, written: 0, dead: false };
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
                Outcome::Failed(e) => panic!("{what}: recovery failed: {e:?}"),
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
