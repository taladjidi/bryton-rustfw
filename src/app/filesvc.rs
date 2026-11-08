//! The file service on the phone bond (roadmap 9, A1; the workouts folder and its writes, 2026-11-07): the GATT table
//! (a vendor service, Control to write, Data to notify, both needing the encrypted bond), the commands arriving from
//! the phone, and the answer streamed a few notifications a pass. The frames and the protocol are in link/filesvc.rs.
//!
//! **Folders:** LIST and READ serve `\Activities\`; LIST_IN and READ_IN name a [`fs::Folder`] of a table (the rides, and
//! `\Workout\calendar\<day>\`), never a path. A workout's key is its day and its number among that day's `.fit`
//! files (link/filesvc.rs); the listing walks the day folders, then each day's files, [`SCAN`] entries a pass, and
//! leaves out files longer than [`workout_file::FILE_MAX`] (the unit does not show them either; a skipped file counts
//! against [`SCAN`] like any entry). The key carries a check of the file's name and size ([`fs::name_check`]) and
//! READ_IN gives it back: a day folder that changed between the two answers NOT FOUND, never another file. Finding a
//! workout walks at most [`LOCATE_MAX`] entries of its day. Two cursors, so
//! `Job` holds two sectors while a workout listing runs.
//!
//! **Courses** (folder 2, `\Courses\`, 2026-11-08): listed as one folder's files (`.fit` and `.gpx`, a two-digit
//! key and the same check), written and deleted as the workouts are (storage/workout_write.rs, [`Target`]); the
//! course followed, or being read to follow, is never deleted (BUSY), and an open COURSES menu is listed again after
//! a course is written or deleted, so it never offers a file that has gone.
//!
//! **Writes** (WRITE_BEGIN / WRITE_DATA / WRITE_END / DELETE, folders 1 and 2): done as the command arrives, in
//! [`App::files_command`], since each is small: a WRITE_DATA buffers its bytes in the read block and writes the
//! sectors it fills (two at most for the largest frame), WRITE_END reads the file back (8 KB at most for a workout, 1 MB
//! for a course, twice) and moves it into its folder (storage/workout_write.rs has the order and why a cut leaves the
//! folder whole). WRITE_BEGIN is answered once the temporary file is ready, a WRITE_DATA only when refused (out of order, past the size, a card
//! error: the write is dropped). Only in a build that may write the card (READ ONLY otherwise), and a write the phone
//! leaves for [`STALL_MS`] is dropped. A failed card write makes the FAT copies suspect, as a screenshot's does.
//!
//! **Never under the recorder:** a command while a ride runs, is paused or is being saved ([`App::ride_settled`]), or
//! while the card is lent to a computer, is answered BUSY; a transfer under way when one of those begins ends with
//! BUSY, and nothing of the card is read from then on. The recording path never calls into this file.
//!
//! **Bounded work:** [`PER_PASS`] frames a pass at most; a pass reads the card once per [`BLOCK`] of file bytes
//! (a few chunks), or scans [`SCAN`] directory entries for the next ride. The stack queues one notification
//! (`BLE_GATTS_HVN_TX_QUEUE_SIZE_DEFAULT`, recalled, inferred; [`nrf::FAST_QUEUE`] on the fast link), so the pass that
//! finds it full leaves the frame for the next one; a transfer the phone does not take for [`STALL_MS`] (Data not subscribed, the link gone quiet) is
//! dropped. While a transfer runs the loop keeps its 50 ms pass ([`App::pass_ms`]).
//!
//! **Throughput.** At MTU 23 (18 file bytes a notification, one queued, a pass of 50 ms): 20 notifications a second,
//! 360 B/s, a 400 KB ride in 18.5 minutes. On the fast link ([`nrf::FAST_MTU`], docs/roadmap.md 3c, reopened
//! 2026-11-07) the frame is the negotiated MTU less 3 (242 file bytes), the stack queues [`nrf::FAST_QUEUE`], a pass
//! fills the queue and refills it when the stack reports the slots free (`HVN_TX_COMPLETE`), and while a READ runs the
//! loop passes every [`FAST_POLL_MS`]. **Only during a READ** the unit also asks for data length 251, the 2M PHY and
//! 15 to 30 ms without latency ([`FAST_PARAMS`]; Apple's Accessory Design Guidelines, QA1931), and gives them back
//! (`PPCP` of app/phone.rs and PHY auto) when the answer ends, stalls, is cancelled or the link drops. The unit
//! measured 3 KB/s on 2026-11-07 (docs/status.md), so what the link really has is kept in [`LinkInfo`] (the interval,
//! PHY and data length from the stack's events, the boost calls' result codes, the last READ's bytes, time, frames and
//! confirmations, the frames the stack refused for a full queue, the passes that sent) and shown by `dev.py info`; the connection event length extension (`nrf::OPT_CONN_EVT_EXT`) keeps an
//! event going while data is queued. The
//! UART (1 Mbaud, about 100 KB/s raw, 2.6 ms for a full notification) is then the cap.
//!
//! **16.6 KB/s on the unit (2026-11-07, 20989f2a), and why.** 383,218 B in 23 s: 81 notifications a second, 1152 of 1871
//! confirmations counted (TX_COMPLETEs kept while a command waits are merged, `nrf::Rx::keep`), 1.2 per connection event. Each costs the loop one blocking UART round trip of about 3.1 ms
//! (`dev.py prof`: the READ's step time is that and nothing else), the loop was awake about 60 % (inferred: `prof` covered 38.6 s, of which 13 s was the READ), and the
//! chip's events were read once per pass (at the step's start, or kept while a command waited), so a freed slot waited
//! for the pass's 10 ms sleep and the step before it was refilled: the stack's queue ran empty between passes. Now the
//! chip's bytes wake the sleep during the READ's pass (`LoopPolicy::begin_sleep`, `Wake::bytes` without `io`), so an
//! HVN_TX_COMPLETE reaches [`App::step_files`] at once and refills the slots it freed. What this cannot change: how
//! many notifications the phone accepts an event. `full` against `turns` in `dev.py info` says which limit a READ met:
//! `full` high, the stack's queue ([`nrf::FAST_QUEUE`], 4; 8 is the next try, the unit decides: a chip that cannot
//! hold it falls back to no fast link at all) or the phone; `full` near 0 and a low rate, the loop or the UART.

use crate::ble::{self, Char};
use crate::fat::write::Handle;
use crate::fat::{self, Cursor, File, Seek};
use crate::filesvc::{self as fs, Frame, Status};
use crate::storage::workout_file;
use crate::storage::workout_write::{self, Target};
use crate::*;

/// Frames tried in one pass.
const PER_PASS: usize = 8;
/// Directory entries examined for the next ride in one pass.
const SCAN: usize = 8;
/// Entries of a day folder [`locate`] walks at most (the listing issues numbers below 100).
const LOCATE_MAX: usize = workout_write::WALK;
/// File bytes read from the card at once: a few chunks, so the card is read once per [`PER_PASS`] frames.
const BLOCK: usize = 1024;
/// The loop's pass while a READ runs on the fast link, ms (the stack takes a few notifications per connection event).
pub const FAST_POLL_MS: u32 = 10;
const _: () = assert!(FAST_POLL_MS < POLL_MS);
/// The connection parameters of a READ on the fast link: 15 to 30 ms (iOS accepts an interval minimum of 15 ms with a
/// maximum 15 ms above it), no slave latency, supervision 4 s.
pub const FAST_PARAMS: ble::ConnParams = (12, 24, 0, 400);
/// Passes a failed restore of the link parameters is tried again (a BUSY or a time-out), then dropped.
const RESTORE_TRIES: u8 = 20;
/// The data length asked for during a READ, octets (the largest, 251).
const FAST_DLE: u16 = 251;
/// A transfer whose frame the stack takes for this long is dropped.
const STALL_MS: u32 = 5000;

/// What the phone link really has, for `dev.py info` (src/link/dev.rs): the values the stack reported by events and the
/// result codes of the boost calls, and the last READ's size, time and the stack's confirmations. Zero: not seen.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LinkInfo {
    /// The connection interval in force, 1.25 ms units (the connect and update events).
    pub interval: u16,
    /// The PHY the link transmits on (1 or 2; PHY update event).
    pub phy: u8,
    /// The data length in force, octets we send (data length update event).
    pub dle: u16,
    /// Result codes of the last boost's calls (parameters, data length, PHY): 0 accepted, else the error code (cut to
    /// 254; 255 the chip did not answer).
    pub boost: [u8; 3],
    /// The last READ: file bytes the stack took, ms from its command to its last frame, notifications taken and
    /// HVN_TX_COMPLETE confirmations counted (since its command).
    pub bytes: u32,
    /// See `bytes`.
    pub ms: u32,
    /// See `bytes`.
    pub sent: u32,
    /// See `bytes`. Confirmations the unit saw: an event the pass drops while a command waits is not counted, so this
    /// can fall short of `sent` (the simulator: 433 of 488).
    pub hvn: u32,
    /// See `bytes`. Frames the stack refused because its queue was full (the chip is the limit, not the unit's loop);
    /// 0 with a slow READ means the queue ran empty, so the unit's pacing or the UART is the limit.
    pub full: u32,
    /// See `bytes`. Passes that sent at least one frame.
    pub turns: u32,
    /// The time of the READ's command, [`App`]'s `up_ms`.
    t0: u32,
}

impl LinkInfo {
    /// A READ begins at `now`.
    fn begin(&mut self, now: u32) {
        (self.bytes, self.ms, self.sent, self.hvn, self.full, self.turns, self.t0) = (0, 0, 0, 0, 0, 0, now);
    }

    /// The stack took a READ frame of `bytes` file bytes at `now`.
    fn frame(&mut self, now: u32, bytes: u32) {
        self.bytes = self.bytes.saturating_add(bytes);
        self.sent = self.sent.saturating_add(1);
        self.ms = now.saturating_sub(self.t0);
    }

    /// The stack confirmed `n` notifications.
    pub(crate) fn confirmed(&mut self, n: u8) {
        self.hvn = self.hvn.saturating_add(u32::from(n));
    }

    /// A boost call's result as one byte.
    fn code(r: Result<(), nrf::Error>) -> u8 {
        match r {
            Ok(()) => 0,
            Err(nrf::Error::Code(c)) => c.clamp(1, 254) as u8,
            Err(nrf::Error::Timeout) => 255,
        }
    }

    /// The link is gone: what it had is not true any more (the last READ stays).
    fn link_down(&mut self) {
        (self.interval, self.phy, self.dle, self.boost) = (0, 0, 0, [0; 3]);
    }

    /// An event of the phone's link that tells what it has.
    pub(crate) fn seen(&mut self, e: &ble::Event) {
        match *e {
            ble::Event::ConnParamUpdate { params, .. } => self.interval = params.0,
            ble::Event::Connected { params: Some(p), .. } => self.interval = p.0,
            ble::Event::PhyUpdate { status: 0, tx, .. } => self.phy = tx,
            ble::Event::DataLengthUpdate { octets, .. } => self.dle = octets.0,
            _ => {}
        }
    }
}

/// What is being answered. The listing's cursor holds a sector: no heap, so the enum is as large as it (the state
/// lives once, in the app's static cell).
#[allow(clippy::large_enum_variant)]
enum Job {
    Idle,
    List {
        seq: u8,
        folder: fs::Folder,
        /// LIST (ENTRY frames, rides only) rather than LIST_IN.
        legacy: bool,
        /// The listing, once the folder is open (the workouts: the day folders).
        cur: Option<Cursor>,
        /// The workouts: the day's files, its date and the next file's number.
        files: Option<Cursor>,
        day: [u8; 8],
        ord: u8,
        count: u32,
    },
    Read {
        seq: u8,
        folder: fs::Folder,
        key: fs::Key,
        /// The next byte to read from the card.
        offset: u32,
        file: Option<File>,
        seek: Seek,
        /// Bytes sent.
        count: u32,
    },
    /// A workout or course being received: where it goes, its CRC-32 and the bytes received so far into the temporary
    /// file `h` (its size the file's). The bytes of the sector being filled wait in `blk`.
    Write {
        seq: u8,
        to: Target,
        crc: u32,
        got: u32,
        h: Handle,
    },
}

impl Job {
    fn list(seq: u8, folder: fs::Folder, legacy: bool) -> Job {
        Job::List { seq, folder, legacy, cur: None, files: None, day: [0; 8], ord: 0, count: 0 }
    }

    fn read(seq: u8, folder: fs::Folder, key: fs::Key, offset: u32) -> Job {
        Job::Read { seq, folder, key, offset, file: None, seek: Seek::new(), count: 0 }
    }
}

/// The file service's state.
pub(crate) struct FileSvc {
    /// Control's and Data's value handles: None when the service could not be added.
    pub(crate) handles: Option<(u16, u16)>,
    job: Job,
    /// The frame built and not yet taken by the stack (`out_len` 0: none).
    out: [u8; fs::FRAME_BIG],
    out_len: usize,
    /// What the frame in `out` adds to the job's count once the stack takes it (an entry, or file bytes): a count of
    /// what was built would tell the phone of bytes it never got.
    pending: u32,
    /// File bytes read and not yet framed.
    blk: [u8; BLOCK],
    blk_len: usize,
    blk_pos: usize,
    /// How long the stack has refused the frame.
    stall_ms: u32,
    /// The ATT MTU of the phone's link: 23 until it exchanged one ([`App`]'s MtuRequest handling).
    pub(crate) mtu: u16,
    /// The link parameters of a READ are in force (to be given back).
    pub(crate) boosted: bool,
    /// The link's parameters as the phone last set them (the connect event, the last update outside a READ): what a
    /// READ gives back. None: not known, nothing is sent back (the link keeps whatever the phone chose).
    pub(crate) link: Option<ble::ConnParams>,
    /// Passes the restore has been tried ([`RESTORE_TRIES`]).
    restore_tries: u8,
    /// The longest value Data holds (the characteristic's max): the frame never passes it.
    pub(crate) max_frame: usize,
    /// What the link really has (`dev.py info`).
    pub(crate) info: LinkInfo,
}

impl FileSvc {
    pub(crate) const fn new() -> Self {
        FileSvc {
            handles: None,
            job: Job::Idle,
            out: [0; fs::FRAME_BIG],
            out_len: 0,
            pending: 0,
            blk: [0; BLOCK],
            blk_len: 0,
            blk_pos: 0,
            stall_ms: 0,
            mtu: ble::ATT_MTU,
            boosted: false,
            link: None,
            restore_tries: 0,
            max_frame: fs::FRAME_MAX,
            info: LinkInfo {
                interval: 0,
                phy: 0,
                dle: 0,
                boost: [0; 3],
                bytes: 0,
                ms: 0,
                sent: 0,
                hvn: 0,
                full: 0,
                turns: 0,
                t0: 0,
            },
        }
    }

    /// The notification's longest value on this link: the MTU less the ATT header.
    fn frame_len(&self) -> usize {
        usize::from(self.mtu.saturating_sub(3)).clamp(fs::FRAME_MAX, self.max_frame.clamp(fs::FRAME_MAX, fs::FRAME_BIG))
    }

    /// The link is gone (or reset): back to the defaults, nothing to give back.
    pub(crate) fn link_down(&mut self) {
        self.reset();
        self.info.link_down();
        (self.mtu, self.boosted, self.link, self.restore_tries) = (ble::ATT_MTU, false, None, 0);
    }

    /// A READ runs on a link with a larger MTU: the loop passes quickly.
    pub(crate) fn fast(&self) -> bool {
        self.boosted && self.active()
    }

    /// An answer is being streamed (the loop keeps its short pass).
    pub(crate) fn active(&self) -> bool {
        !matches!(self.job, Job::Idle) || self.out_len > 0
    }

    /// A frame waits for the stack.
    #[cfg(test)]
    pub(crate) fn has_pending(&self) -> bool {
        self.out_len > 0
    }

    /// The link is gone: nothing is answered.
    pub(crate) fn reset(&mut self) {
        (self.job, self.out_len, self.pending, self.blk_len, self.blk_pos, self.stall_ms) = (Job::Idle, 0, 0, 0, 0, 0);
    }

    /// Control's handle, when the service exists.
    fn control(&self) -> Option<u16> {
        self.handles.map(|h| h.0)
    }

    /// Replaces whatever is pending with an END frame.
    fn finish(&mut self, seq: u8, status: Status, count: u32) {
        self.reply(seq, status, count);
        (self.job, self.blk_len, self.blk_pos) = (Job::Idle, 0, 0);
    }

    /// Replaces whatever is pending with an END frame, the job kept (WRITE_BEGIN's answer).
    fn reply(&mut self, seq: u8, status: Status, count: u32) {
        let mut f = [0; fs::FRAME_BIG];
        let n = Frame::End { seq, status, count }.encode(&mut f);
        (self.out, self.out_len, self.pending) = (f, n, 0);
    }

    /// The stack took the frame in `out`: what it carried counts.
    fn taken(&mut self, now: u32) {
        if let Job::List { count, .. } | Job::Read { count, .. } = &mut self.job {
            *count = count.saturating_add(self.pending);
        }
        if matches!(self.job, Job::Read { .. }) {
            self.info.frame(now, self.pending);
        }
        (self.out_len, self.pending) = (0, 0);
    }

    /// Ends the answer under way, if any, with `status` (the card is about to be used, or a ride began): what the
    /// phone got is what the END's count says.
    pub(crate) fn abort(&mut self, status: Status) {
        let (seq, count) = match self.job {
            Job::List { seq, count, .. } | Job::Read { seq, count, .. } | Job::Write { seq, got: count, .. } => {
                (seq, count)
            }
            Job::Idle => return,
        };
        self.finish(seq, status, count);
    }

    /// Builds the next frame of the job into `out`; false when there is none this pass (nothing to answer, or the
    /// scan found no ride within its bound). Ending (a failure, or the listing's or file's end) ends the answer with
    /// its status and what was sent.
    fn fill(&mut self, d: &mut impl fat::Disk) -> bool {
        let (seq, count) = match self.job {
            Job::Idle | Job::Write { .. } => return false, // a write answers as its commands arrive
            Job::List { seq, count, .. } | Job::Read { seq, count, .. } => (seq, count),
        };
        match self.build(d, seq) {
            Ok(got) => got,
            Err(status) => {
                self.finish(seq, status, count_of(&self.job, count));
                true
            }
        }
    }

    /// The work of [`FileSvc::fill`]; Err(status) ends the answer.
    fn build(&mut self, d: &mut impl fat::Disk, seq: u8) -> Result<bool, Status> {
        let max = self.frame_len();
        let FileSvc { job, out, out_len, pending, blk, blk_len, blk_pos, .. } = self;
        let mut f = [0; fs::FRAME_BIG];
        let io = |_| Status::Io;
        match job {
            Job::Idle | Job::Write { .. } => Ok(false),
            Job::List { folder, legacy, cur, files, day, ord, .. } => {
                let mut fat = fat::Fat::mount(d).map_err(io)?;
                if cur.is_none() {
                    // a card with no such folder has no files: an empty list, not an error
                    let path = match folder {
                        fs::Folder::Activities => "Activities",
                        fs::Folder::Workout => workout_write::CALENDAR,
                        fs::Folder::Courses => workout_write::COURSES,
                    };
                    match fat.open(d, path).and_then(|dir| fat.cursor(dir)) {
                        Ok(c) => *cur = Some(c),
                        Err(fat::Error::NotFound) => return Err(Status::Ok),
                        Err(_) => return Err(Status::Io),
                    }
                }
                let Some(c) = cur else { return Ok(false) };
                for _ in 0..SCAN {
                    if let Some(fc) = files {
                        let Some(e) = fat.list(d, fc).map_err(io)? else {
                            *files = None;
                            continue;
                        };
                        if e.file.dir || !workout_write::is_fit(e.name()) {
                            continue;
                        }
                        let n = *ord;
                        *ord = ord.saturating_add(1);
                        let mut key = [0; 10];
                        key[..8].copy_from_slice(day);
                        key[8..].copy_from_slice(&two(n));
                        if n < 100 && e.file.size as usize <= workout_file::FILE_MAX {
                            *pending = 1;
                            let check = fs::name_check(e.name(), e.file.size);
                            *out_len = entry(seq, *legacy, *folder, &key, check, e.file.size, &mut f).unwrap_or(0);
                            *out = f;
                            return Ok(*out_len > 0);
                        }
                        continue;
                    }
                    let Some(e) = fat.list(d, c).map_err(io)? else { return Err(Status::Ok) };
                    if *folder == fs::Folder::Workout {
                        if let (true, Ok(dn)) = (e.file.dir, <[u8; 8]>::try_from(e.name()))
                            && workout_file::folder_day(&dn).is_some()
                        {
                            *files = Some(fat.cursor(e.file).map_err(io)?);
                            (*day, *ord) = (dn, 0);
                        }
                    } else if *folder == fs::Folder::Courses {
                        if e.file.dir || !workout_write::is_course(e.name()) {
                            continue;
                        }
                        let n = *ord;
                        *ord = ord.saturating_add(1);
                        if n < 100 {
                            *pending = 1;
                            let check = fs::name_check(e.name(), e.file.size);
                            *out_len = entry(seq, *legacy, *folder, &two(n), check, e.file.size, &mut f).unwrap_or(0);
                            *out = f;
                            return Ok(*out_len > 0);
                        }
                    } else if let (Some(name), false) = (fs::ride_name(e.name()), e.file.dir) {
                        *pending = 1;
                        *out_len = entry(seq, *legacy, *folder, &name, 0, e.file.size, &mut f).unwrap_or(0);
                        *out = f;
                        return Ok(*out_len > 0);
                    }
                }
                Ok(false)
            }
            Job::Read { folder, key, offset, file, seek, .. } => {
                if *blk_pos >= *blk_len {
                    let mut fat = fat::Fat::mount(d).map_err(io)?;
                    if file.is_none() {
                        let x = locate(&mut fat, d, *folder, *key)?;
                        if *offset > x.size {
                            return Err(Status::BadRequest);
                        }
                        *file = Some(x);
                    }
                    let Some(x) = file else { return Ok(false) };
                    match fat.read_from(d, x, *offset, blk, seek).map_err(io)? {
                        0 => return Err(Status::Ok),
                        n => (*blk_len, *blk_pos, *offset) = (n, 0, offset.saturating_add(n as u32)),
                    }
                }
                let data = blk.get(*blk_pos..*blk_len).unwrap_or(&[]);
                let n = data.len().min(max.saturating_sub(2));
                *out_len = Frame::Chunk { seq, data: data.get(..n).unwrap_or(&[]) }
                    .encode(f.get_mut(..max).unwrap_or(&mut []));
                *out = f;
                *blk_pos += n;
                *pending = n as u32;
                Ok(true)
            }
        }
    }
}

/// `n` (below 100) as two digits: a file's number in a listed folder.
fn two(n: u8) -> [u8; 2] {
    [b'0' + n / 10 % 10, b'0' + n % 10]
}

/// The entry frame for `key`, in LIST's shape or LIST_IN's.
fn entry(seq: u8, legacy: bool, folder: fs::Folder, key: &[u8], check: u16, size: u32, f: &mut [u8]) -> Option<usize> {
    Some(if legacy {
        Frame::Entry { seq, name: key.try_into().ok()?, size }.encode(f)
    } else {
        Frame::EntryIn { seq, key: fs::Key::new(folder, key)?.with_check(check), size }.encode(f)
    })
}

/// The file `key` of `folder` on the card (a directory is not one). A workout's key is a day folder's name and the
/// number of the `.fit` among that folder's files, counted as the listing counts them; the file now at that place
/// must have the name and size the key's check was made of, else NOT FOUND.
fn locate(fat: &mut fat::Fat, d: &mut impl fat::Disk, folder: fs::Folder, k: fs::Key) -> Result<fat::File, Status> {
    let x = if folder == fs::Folder::Activities {
        let mut buf = [0; fs::FILE_NAME_LEN];
        let n = fs::file_name(k.bytes(), &mut buf).ok_or(Status::NotFound)?;
        let dir = fat.find(d, fat.root(), "Activities").map_err(found)?;
        fat.find(d, dir, n).map_err(found)?
    } else {
        locate_listed(fat, d, folder, k)?.1.file
    };
    if x.dir || (folder == fs::Folder::Workout && x.size as usize > workout_file::FILE_MAX) {
        return Err(Status::NotFound);
    }
    Ok(x)
}

/// A FAT error as a lookup answers it.
fn found(e: fat::Error) -> Status {
    if e == fat::Error::NotFound { Status::NotFound } else { Status::Io }
}

/// The workout or course of key `k` in `folder` ([`locate`]): its folder (a day's, or `\Courses`) and its entry (a
/// file its listing counts, never a folder), else NOT FOUND.
fn locate_listed(
    fat: &mut fat::Fat,
    d: &mut impl fat::Disk,
    folder: fs::Folder,
    k: fs::Key,
) -> Result<(File, fat::Entry), Status> {
    let (dir, num, lists): (_, _, fn(&[u8]) -> bool) = match folder {
        fs::Folder::Workout => {
            let (day, num) = k.bytes().split_at_checked(8).ok_or(Status::NotFound)?;
            let day = core::str::from_utf8(day).map_err(|_| Status::NotFound)?;
            let dir = fat.open(d, workout_write::CALENDAR).map_err(found)?;
            (fat.find(d, dir, day).map_err(found)?, num, workout_write::is_fit)
        }
        fs::Folder::Courses => {
            (fat.open(d, workout_write::COURSES).map_err(found)?, k.bytes(), workout_write::is_course)
        }
        fs::Folder::Activities => return Err(Status::NotFound),
    };
    let num = num.iter().fold(0usize, |v, &c| v * 10 + usize::from(c.wrapping_sub(b'0')));
    let mut c = fat.cursor(dir).map_err(found)?;
    let mut n = 0;
    for _ in 0..LOCATE_MAX {
        let Some(e) = fat.list(d, &mut c).map_err(found)? else { break };
        if e.file.dir || !lists(e.name()) {
            continue;
        }
        if n == num {
            return Some((dir, e))
                .filter(|_| fs::name_check(e.name(), e.file.size) == k.check())
                .ok_or(Status::NotFound);
        }
        n += 1;
    }
    Err(Status::NotFound)
}

/// The count a job has sent (`or` for none).
fn count_of(job: &Job, or: u32) -> u32 {
    match job {
        Job::List { count, .. } | Job::Read { count, .. } | Job::Write { got: count, .. } => *count,
        Job::Idle => or,
    }
}

impl Default for FileSvc {
    fn default() -> Self {
        Self::new()
    }
}

/// Adds the service: its vendor base, then Control and Data. None when the chip refuses any of it (the phone link
/// works without the file service).
pub(crate) fn add_service(b: &mut impl Board, rx: &mut nrf::Rx, frame: usize) -> Option<(u16, u16, usize)> {
    let ty = ble::uuid_vs_add(b, rx, &fs::BASE).ok()?;
    let svc = ble::service_add_vs(b, rx, (fs::SERVICE, ty)).ok()?;
    // Control as large as Data: a WRITE_DATA fills the fast link's frame (a table that cannot hold it: 20 bytes, below)
    let control = Char {
        uuid: fs::CONTROL,
        props: ble::WRITE,
        value: &[0],
        max: frame as u16,
        read: ble::NO_ACCESS,
        write: ble::ENCRYPTED,
    };
    let data = Char {
        uuid: fs::DATA,
        props: ble::NOTIFY,
        value: &[0],
        max: frame as u16,
        read: ble::ENCRYPTED,
        write: ble::NO_ACCESS,
    };
    let c = match ble::characteristic_add_vs(b, rx, svc, &control, ty) {
        Ok(c) => c,
        Err(_) if frame > fs::FRAME_MAX => {
            ble::characteristic_add_vs(b, rx, svc, &Char { max: fs::FRAME_MAX as u16, ..control }, ty).ok()?
        }
        Err(_) => return None,
    };
    // a table that cannot hold the big value gets the 20 bytes of the default MTU (the frame follows the max)
    match ble::characteristic_add_vs(b, rx, svc, &data, ty) {
        Ok(d) => Some((c.0, d.0, frame)),
        Err(_) if frame > fs::FRAME_MAX => {
            let small = Char { max: fs::FRAME_MAX as u16, ..data };
            let d = ble::characteristic_add_vs(b, rx, svc, &small, ty).ok()?;
            Some((c.0, d.0, fs::FRAME_MAX))
        }
        Err(_) => None,
    }
}

impl Phone {
    /// Whether a write to `handle` is for the file service's Control.
    pub(crate) fn is_control(&self, handle: u16) -> bool {
        self.files.control() == Some(handle)
    }
}

impl App {
    /// The phone wrote Control: a command to answer. Only on the encrypted link (the stack enforces it too).
    pub(crate) fn files_command(&mut self, b: &mut impl Board, written: &[u8]) {
        // rides are GPS tracks: the bond, not just an encrypted link (Just Works pairing during the PAIR window)
        if !self.phone.secure || self.phone.stored.bond.is_none() {
            return;
        }
        let (busy, now) = (!self.ride_settled() || self.emmc.lent(), self.up_ms);
        let svc = &mut self.phone.files;
        svc.stall_ms = 0;
        let cmd = fs::Command::parse(written);
        if let Ok(
            c @ (fs::Command::WriteBegin { .. }
            | fs::Command::WriteCourse { .. }
            | fs::Command::WriteData { .. }
            | fs::Command::WriteEnd { .. }
            | fs::Command::Delete { .. }),
        ) = cmd
        {
            return self.files_write(b, c, busy);
        }
        match cmd {
            Ok(fs::Command::Cancel) => svc.reset(),
            Ok(fs::Command::List { seq }) if !busy => {
                svc.reset();
                svc.job = Job::list(seq, fs::Folder::Activities, true);
            }
            Ok(fs::Command::ListIn { seq, folder }) if !busy => {
                svc.reset();
                svc.job = Job::list(seq, folder, false);
            }
            Ok(fs::Command::Read { seq, offset, name }) if !busy => {
                svc.reset();
                match fs::Key::new(fs::Folder::Activities, &name) {
                    Some(key) => {
                        svc.job = Job::read(seq, fs::Folder::Activities, key, offset);
                        svc.info.begin(now);
                    }
                    None => svc.finish(seq, Status::BadRequest, 0),
                }
            }
            Ok(fs::Command::ReadIn { seq, folder, offset, key }) if !busy => {
                svc.reset();
                svc.job = Job::read(seq, folder, key, offset);
                svc.info.begin(now);
            }
            Ok(
                fs::Command::List { seq }
                | fs::Command::Read { seq, .. }
                | fs::Command::ListIn { seq, .. }
                | fs::Command::ReadIn { seq, .. },
            ) => svc.finish(seq, Status::Busy, 0),
            Ok(c @ (fs::Command::ProfileGet { .. } | fs::Command::ProfileSet { .. })) => self.files_profile(b, c, busy),
            Ok(_) => {} // the writes: files_write
            Err((seq, status)) => svc.finish(seq, status, 0),
        }
    }

    /// A LIST or READ under way ends with its END (BUSY, what it sent) before a command's answer, refusal or not:
    /// notified now, since the answer takes its place (a full queue loses it: the phone then sees the new answer only).
    fn abort_transfer(&mut self, b: &mut impl Board) {
        if matches!(self.phone.files.job, Job::List { .. } | Job::Read { .. }) {
            self.phone.files.abort(Status::Busy);
            let svc = &mut self.phone.files;
            if let (Some(conn), Some((_, data))) = (self.phone.conn, svc.handles) {
                let frame = svc.out.get(..svc.out_len).unwrap_or(&[]);
                if ble::notify(b, &mut self.rx, conn, data, frame).is_ok() {
                    svc.taken(self.up_ms);
                }
            }
        }
    }

    /// PROFILE_GET and PROFILE_SET ([`App::files_command`]): the profile is built here only, so `files_command`'s
    /// frame stays small. A write under way is dropped, as a LIST would drop it.
    #[inline(never)]
    fn files_profile(&mut self, b: &mut impl Board, c: fs::Command, busy: bool) {
        self.abort_transfer(b);
        self.phone.files.reset();
        match c {
            fs::Command::ProfileGet { seq } => {
                let mut f = [0; fs::FRAME_BIG];
                let n = Frame::Profile { seq, profile: self.training_profile() }.encode(&mut f);
                let svc = &mut self.phone.files;
                (svc.out, svc.out_len, svc.pending) = (f, n, 0);
            }
            fs::Command::ProfileSet { seq, profile } => {
                let (status, count) = self.profile_set(b, profile, busy);
                self.phone.files.finish(seq, status, count);
            }
            _ => {}
        }
    }

    /// The settings the phone reads and sets (link/filesvc.rs "Profile").
    fn training_profile(&self) -> fs::Profile {
        let (s, byte) = (&self.settings, |v: u16| u8::try_from(v).unwrap_or(u8::MAX));
        fs::Profile {
            ftp: s.ftp,
            max_hr: byte(s.max_hr),
            lthr: byte(s.lthr),
            weight_kg: byte(s.weight_kg),
            wprime_j: s.wprime_j,
            zones_lthr: s.zones_lthr,
            female: s.female,
        }
    }

    /// PROFILE_SET: the answer's status and count. A value must be in the menu's range, or be the one held (so a
    /// profile read and sent back unchanged is never refused), or be 0 for the LTHR (unset). Values the unit already
    /// holds answer OK, count 0, even in a ride. BUSY also while a setting is being edited in the menu. Each field is
    /// assigned only when it differs. Applied as the menus do (`setting_edited`, then
    /// `save_misc`): the file is written between rides, the registers hold it until then.
    #[inline(never)]
    fn profile_set(&mut self, b: &mut impl Board, p: fs::Profile, busy: bool) -> (Status, u32) {
        let now = self.training_profile();
        let ok = |r: core::ops::RangeInclusive<u16>, new: u16, old: u16| new == old || r.contains(&new);
        let lthr_ok = |new: u16, old: u16| new == 0 || ok(menu::LTHR_RANGE, new, old);
        let b8 = u16::from;
        let valid = ok(menu::FTP_RANGE, p.ftp, now.ftp)
            && ok(menu::MAX_HR_RANGE, b8(p.max_hr), b8(now.max_hr))
            && lthr_ok(b8(p.lthr), b8(now.lthr))
            && ok(menu::WEIGHT_RANGE, b8(p.weight_kg), b8(now.weight_kg))
            && ok(menu::WPRIME_RANGE, p.wprime_j, now.wprime_j);
        if !valid {
            return (Status::BadRequest, 0);
        }
        if p == now {
            return (Status::Ok, 0);
        }
        if busy || self.menu.editing {
            return (Status::Busy, 0);
        }
        if !self.card_writable {
            return (Status::ReadOnly, 0);
        }
        if !self.settings_ready(b) {
            return (Status::Io, 0);
        }
        let s = &mut self.settings;
        if p.ftp != now.ftp {
            s.ftp = p.ftp;
        }
        if p.wprime_j != now.wprime_j {
            s.wprime_j = p.wprime_j;
        }
        if p.lthr != now.lthr {
            s.lthr = b8(p.lthr);
        }
        if p.weight_kg != now.weight_kg {
            s.weight_kg = b8(p.weight_kg);
        }
        (s.zones_lthr, s.female) = (p.zones_lthr, p.female);
        if p.max_hr != now.max_hr {
            s.max_hr = b8(p.max_hr);
            self.setting_edited(menu::Setting::MaxHr);
        }
        self.save_misc(b);
        self.notice("PROFILE FROM PHONE");
        if matches!(self.screen, Screen::Menu(..)) {
            self.show(b); // an open settings row shows the new value
        }
        (Status::Ok, 1)
    }

    /// A write command ([`App::files_command`]): refused (and the write under way dropped) unless the card may be
    /// written now; else done on the card at once, bounded (module doc).
    #[inline(never)]
    fn files_write(&mut self, b: &mut impl Board, c: fs::Command, busy: bool) {
        let (seq, writes) = match c {
            fs::Command::WriteBegin { seq, .. }
            | fs::Command::WriteCourse { seq, .. }
            | fs::Command::Delete { seq, .. } => (seq, true),
            fs::Command::WriteData { seq, .. } | fs::Command::WriteEnd { seq } => (seq, false),
            _ => return,
        };
        self.abort_transfer(b);
        let writing = matches!(self.phone.files.job, Job::Write { .. });
        let why = if busy {
            Some(Status::Busy)
        } else if !self.card_writable {
            Some(Status::ReadOnly)
        } else if writes == writing {
            Some(Status::BadRequest) // a WRITE_BEGIN or DELETE mid-write, or data with no write begun
        } else {
            None
        };
        if let Some(why) = why {
            return self.phone.files.finish(seq, why, count_of(&self.phone.files.job, 0));
        }
        let t = fat::write::dos_time(self.rec_time().0);
        let r = self.write_step(b, c, t);
        let svc = &mut self.phone.files;
        let course = matches!(c, fs::Command::Delete { folder: fs::Folder::Courses, .. })
            || matches!((c, &svc.job), (fs::Command::WriteEnd { .. }, Job::Write { to: Target::Course(_), .. }));
        match (r, c) {
            (Ok(()), fs::Command::WriteBegin { .. } | fs::Command::WriteCourse { .. }) => svc.reply(seq, Status::Ok, 0),
            (Ok(()), fs::Command::WriteData { .. }) => {}
            (Ok(()), fs::Command::WriteEnd { .. }) => {
                let got = count_of(&svc.job, 0);
                svc.finish(seq, Status::Ok, got);
            }
            (Ok(()), _) => svc.finish(seq, Status::Ok, 1),
            (Err(e), _) => {
                let got = count_of(&svc.job, 0);
                svc.finish(seq, e, got);
                if e == Status::Io {
                    self.fats_unsure(); // the next writes compare the FAT copies first
                }
            }
        }
        // an open COURSES lists the folder again (a course read to follow keeps the list it was picked from)
        if course
            && r.is_ok()
            && matches!(self.screen, Screen::Menu(Menu::Courses, _))
            && let Some(cs) = self.courses.as_deref_mut().filter(|cs| !cs.loading())
        {
            let _ = cs.open(&mut emmc::Volume(&mut self.emmc, b));
            self.show(b);
        }
    }

    /// The card work of a write command `c`; Err: its answer.
    fn write_step(&mut self, b: &mut impl Board, c: fs::Command, t: (u16, u16)) -> Result<(), Status> {
        if !matches!(c, fs::Command::WriteData { .. }) && self.fats_suspect {
            // a failed write before: the FAT copies (and FSInfo's count, which the room check reads) made right before
            // a step that takes or frees clusters (as storage/screenshot.rs's write)
            let mut d = emmc::Volume(&mut self.emmc, b);
            fat::Fat::mount(&mut d).and_then(|mut fat| fat.sync_fats(&mut d)).map_err(|_| Status::Io)?;
            self.fats_equal();
        }
        let mut d = emmc::Volume(&mut self.emmc, b);
        let mount = |d: &mut emmc::Volume<_>| fat::Fat::mount(d).map_err(|_| Status::Io);
        let svc = &mut self.phone.files;
        let courses = self.courses.as_deref();
        match c {
            fs::Command::WriteBegin { seq, size, crc, .. } | fs::Command::WriteCourse { seq, size, crc, .. } => {
                let to = match c {
                    fs::Command::WriteCourse { name, .. } => Target::Course(name),
                    fs::Command::WriteBegin { day, .. } if workout_file::folder_day(&day).is_some() => Target::Day(day),
                    _ => return Err(Status::BadRequest),
                };
                svc.reset();
                let mut fat = mount(&mut d)?;
                let h = workout_write::begin(&mut fat, &mut d, &to, size, t)?;
                (svc.job, svc.blk) = (Job::Write { seq, to, crc, got: 0, h }, [0; BLOCK]);
                Ok(())
            }
            fs::Command::WriteData { offset, data, .. } => {
                let FileSvc { job: Job::Write { got, h, .. }, blk, blk_len, .. } = svc else {
                    return Err(Status::BadRequest);
                };
                let end = got.checked_add(data.len() as u32).filter(|&e| *got == offset && e <= h.size);
                let Some(end) = end else { return Err(Status::BadRequest) };
                let mut rest = data;
                while !rest.is_empty() {
                    let room = fat::SECTOR - *blk_len;
                    let (now, later) = rest.split_at(room.min(rest.len()));
                    blk.get_mut(*blk_len..*blk_len + now.len()).ok_or(Status::Io)?.copy_from_slice(now);
                    (*blk_len, *got, rest) = (*blk_len + now.len(), *got + now.len() as u32, later);
                    if *blk_len == fat::SECTOR || *got == end && end == h.size {
                        // mounted only when a sector fills: most frames only buffer
                        let s: &[u8; fat::SECTOR] = blk.first_chunk().ok_or(Status::Io)?;
                        let mut fat = mount(&mut d)?;
                        workout_write::put(&mut fat, &mut d, h, (*got - 1) / fat::SECTOR as u32, s)?;
                        (*blk, *blk_len) = ([0; BLOCK], 0);
                    }
                }
                Ok(())
            }
            fs::Command::WriteEnd { .. } => {
                let Job::Write { to, crc, got, h, .. } = svc.job else { return Err(Status::BadRequest) };
                if got != h.size {
                    return Err(Status::Mismatch);
                }
                let mut fat = mount(&mut d)?;
                workout_write::commit(&mut fat, &mut d, &h, &to, crc, t)
            }
            fs::Command::Delete { folder, key, .. } => {
                svc.reset();
                let mut fat = mount(&mut d)?;
                let (dir, e) = locate_listed(&mut fat, &mut d, folder, key)?;
                if folder == fs::Folder::Courses && courses.is_some_and(|cs| cs.uses(e.file)) {
                    return Err(Status::Busy); // the course followed, or being read to follow
                }
                workout_write::delete(&mut fat, &mut d, dir, &e)
            }
            _ => Err(Status::BadRequest),
        }
    }

    /// What the phone link has, for `info`: the ATT MTU, whether the chip took the event length extension, and the
    /// link's `LinkInfo`.
    pub fn phone_link(&self) -> (u16, bool, LinkInfo) {
        (self.phone.files.mtu, self.ble.is_some_and(|v| v.ext), self.phone.files.info)
    }

    /// The fast link is on: the chip has the larger MTU and the PHONE menu's FAST LINK row is on.
    pub(crate) fn fast_ok(&self) -> bool {
        self.ble.is_some_and(|v| v.mtu > ble::ATT_MTU) && self.phone.stored.fast
    }

    /// The Server Rx MTU to answer an Exchange MTU Request on `conn` with, and the link's MTU once the phone's
    /// `client` is known: the smaller. The trainer's link and a unit without the fast link stay at 23.
    pub(crate) fn mtu_reply(&mut self, b: &mut impl Board, conn: u16, client: u16) {
        let phone = Some(conn) == self.phone.conn;
        let ours = if phone && self.fast_ok() { nrf::FAST_MTU } else { ble::ATT_MTU };
        if ble::exchange_mtu_reply(b, &mut self.rx, conn, ours).is_ok() && phone {
            self.phone.files.mtu = client.min(ours).max(ble::ATT_MTU);
        }
    }

    /// The link parameters of a READ, asked for once. Every answer is optional (a chip or phone that refuses costs
    /// speed, nothing else: the errors are dropped).
    fn boost(&mut self, b: &mut impl Board, conn: u16) {
        let rx = &mut self.rx;
        let boost = [
            LinkInfo::code(ble::conn_param_update(b, rx, conn, FAST_PARAMS)),
            LinkInfo::code(ble::data_length_set(b, rx, conn, FAST_DLE)),
            LinkInfo::code(ble::phy_set(b, rx, conn, 2)),
        ];
        self.phone.files.info.boost = boost;
        self.phone.files.boosted = true;
    }

    /// Gives back what the phone had: its last connection parameters (none known: no update is sent, the link keeps
    /// what the phone set) and the 1M PHY; the data length stays (it costs nothing idle). A restore that fails (BUSY,
    /// a time-out) is tried again next pass, [`RESTORE_TRIES`] times.
    fn unboost(&mut self, b: &mut impl Board, conn: u16) {
        let rx = &mut self.rx;
        let link = self.phone.files.link;
        let param = link.map_or(Ok(()), |p| ble::conn_param_update(b, rx, conn, p));
        let phy = ble::phy_set(b, rx, conn, 1);
        let svc = &mut self.phone.files;
        svc.restore_tries = svc.restore_tries.saturating_add(1);
        if (param.is_ok() && phy.is_ok()) || svc.restore_tries >= RESTORE_TRIES {
            (svc.boosted, svc.restore_tries) = (false, 0);
        }
    }

    /// The answer's next frames, [`PER_PASS`] at most.
    pub(crate) fn step_files(&mut self, b: &mut impl Board, conn: u16) {
        let Some((_, data)) = self.phone.files.handles else { return };
        if self.phone.files.boosted && (!self.phone.files.active() || !self.fast_ok()) {
            self.unboost(b, conn); // the answer ended (or the row went off): the normal parameters, whatever the way
        }
        if !self.phone.secure || self.phone.stored.bond.is_none() || !self.phone.files.active() {
            return;
        }
        if !matches!(self.phone.files.job, Job::Idle) && (!self.ride_settled() || self.emmc.lent()) {
            self.phone.files.abort(Status::Busy); // a ride began under the transfer
        }
        let svc = &self.phone.files;
        if !svc.boosted && matches!(svc.job, Job::Read { .. }) && svc.mtu > ble::ATT_MTU && self.fast_ok() {
            self.boost(b, conn);
        }
        let mut sent = false;
        for _ in 0..PER_PASS {
            if self.phone.files.out_len == 0 {
                let mut d = emmc::Volume(&mut self.emmc, b);
                if !self.phone.files.fill(&mut d) {
                    break;
                }
            }
            let svc = &mut self.phone.files;
            let frame = svc.out.get(..svc.out_len).unwrap_or(&[]);
            if ble::notify(b, &mut self.rx, conn, data, frame).is_err() {
                if matches!(svc.job, Job::Read { .. }) {
                    svc.info.full = svc.info.full.saturating_add(1);
                }
                break; // the queue is full (or Data is not subscribed): the same frame next pass
            }
            svc.taken(self.up_ms);
            (svc.stall_ms, sent) = (0, true);
        }
        let svc = &mut self.phone.files;
        if sent && matches!(svc.job, Job::Read { .. }) {
            svc.info.turns = svc.info.turns.saturating_add(1);
        }
        // a write the phone stopped feeding is dropped as a frame it stopped taking (the temporary file stays, unseen)
        if (svc.out_len > 0 && !sent) || (svc.out_len == 0 && matches!(svc.job, Job::Write { .. })) {
            svc.stall_ms = svc.stall_ms.saturating_add(self.dt_ms);
            if svc.stall_ms >= STALL_MS {
                svc.reset();
            }
        }
    }
}
