//! The file service on the phone bond, reads only (roadmap 9, A1): the GATT table (a vendor service, Control to write,
//! Data to notify, both needing the encrypted bond), the commands arriving from the phone, and the answer streamed
//! a few notifications a pass. The frames and the protocol are in link/filesvc.rs.
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
//! (`PPCP` of app/phone.rs and PHY auto) when the answer ends, stalls, is cancelled or the link drops. The
//! UART (1 Mbaud, about 100 KB/s raw, 2.6 ms for a full notification) is then the cap.

use crate::ble::{self, Char};
use crate::fat::{self, Cursor, File, Seek};
use crate::filesvc::{self as fs, Frame, Status};
use crate::*;

/// Frames tried in one pass.
const PER_PASS: usize = 8;
/// Directory entries examined for the next ride in one pass.
const SCAN: usize = 8;
/// File bytes read from the card at once: a few chunks, so the card is read once per [`PER_PASS`] frames.
const BLOCK: usize = 1024;
/// The loop's pass while a READ runs on the fast link, ms (the stack takes a few notifications per connection event).
pub const FAST_POLL_MS: u32 = 10;
/// The connection parameters of a READ on the fast link: 15 to 30 ms (iOS accepts an interval minimum of 15 ms with a
/// maximum 15 ms above it), no slave latency, supervision 4 s.
pub const FAST_PARAMS: ble::ConnParams = (12, 24, 0, 400);
/// The data length asked for during a READ, octets (the largest, 251).
const FAST_DLE: u16 = 251;
/// A transfer whose frame the stack takes for this long is dropped.
const STALL_MS: u32 = 5000;

/// What is being answered. The listing's cursor holds a sector: no heap, so the enum is as large as it (the state
/// lives once, in the app's static cell).
#[allow(clippy::large_enum_variant)]
enum Job {
    Idle,
    List {
        seq: u8,
        /// The listing, once the folder is open.
        cur: Option<Cursor>,
        count: u32,
    },
    Read {
        seq: u8,
        name: fs::Name,
        /// The next byte to read from the card.
        offset: u32,
        file: Option<File>,
        seek: Seek,
        /// Bytes sent.
        count: u32,
    },
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
        }
    }

    /// The notification's longest value on this link: the MTU less the ATT header.
    fn frame_len(&self) -> usize {
        usize::from(self.mtu.saturating_sub(3)).clamp(fs::FRAME_MAX, fs::FRAME_BIG)
    }

    /// The link is gone (or reset): back to the defaults, nothing to give back.
    pub(crate) fn link_down(&mut self) {
        self.reset();
        (self.mtu, self.boosted) = (ble::ATT_MTU, false);
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
        let mut f = [0; fs::FRAME_BIG];
        let n = Frame::End { seq, status, count }.encode(&mut f);
        (self.out, self.out_len, self.pending, self.job, self.blk_len, self.blk_pos) = (f, n, 0, Job::Idle, 0, 0);
    }

    /// The stack took the frame in `out`: what it carried counts.
    fn taken(&mut self) {
        if let Job::List { count, .. } | Job::Read { count, .. } = &mut self.job {
            *count = count.saturating_add(self.pending);
        }
        (self.out_len, self.pending) = (0, 0);
    }

    /// Ends the answer under way, if any, with `status` (the card is about to be used, or a ride began): what the
    /// phone got is what the END's count says.
    pub(crate) fn abort(&mut self, status: Status) {
        let (seq, count) = match self.job {
            Job::List { seq, count, .. } | Job::Read { seq, count, .. } => (seq, count),
            Job::Idle => return,
        };
        self.finish(seq, status, count);
    }

    /// Builds the next frame of the job into `out`; false when there is none this pass (nothing to answer, or the
    /// scan found no ride within its bound). Ending (a failure, or the listing's or file's end) ends the answer with
    /// its status and what was sent.
    fn fill(&mut self, d: &mut impl fat::Disk) -> bool {
        let (seq, count) = match self.job {
            Job::Idle => return false,
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
            Job::Idle => Ok(false),
            Job::List { cur, .. } => {
                let mut fat = fat::Fat::mount(d).map_err(io)?;
                if cur.is_none() {
                    // a card with no \Activities\ has no rides: an empty list, not an error
                    match fat.find(d, fat.root(), "Activities").and_then(|dir| fat.cursor(dir)) {
                        Ok(c) => *cur = Some(c),
                        Err(fat::Error::NotFound) => return Err(Status::Ok),
                        Err(_) => return Err(Status::Io),
                    }
                }
                let Some(c) = cur else { return Ok(false) };
                for _ in 0..SCAN {
                    let Some(e) = fat.list(d, c).map_err(io)? else { return Err(Status::Ok) };
                    if let (Some(name), false) = (fs::ride_name(e.name()), e.file.dir) {
                        *pending = 1;
                        *out_len = Frame::Entry { seq, name, size: e.file.size }.encode(&mut f);
                        *out = f;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Job::Read { name, offset, file, seek, .. } => {
                if *blk_pos >= *blk_len {
                    let mut fat = fat::Fat::mount(d).map_err(io)?;
                    if file.is_none() {
                        let mut buf = [0; fs::FILE_NAME_LEN];
                        let n = fs::file_name(name, &mut buf).ok_or(Status::NotFound)?;
                        let find = |e| if e == fat::Error::NotFound { Status::NotFound } else { Status::Io };
                        let dir = fat.find(d, fat.root(), "Activities").map_err(find)?;
                        let x = fat.find(d, dir, n).map_err(find)?;
                        if x.dir {
                            return Err(Status::NotFound);
                        }
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

/// The count a job has sent (`or` for none).
fn count_of(job: &Job, or: u32) -> u32 {
    match job {
        Job::List { count, .. } | Job::Read { count, .. } => *count,
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
pub(crate) fn add_service(b: &mut impl Board, rx: &mut nrf::Rx, frame: usize) -> Option<(u16, u16)> {
    let ty = ble::uuid_vs_add(b, rx, &fs::BASE).ok()?;
    let svc = ble::service_add_vs(b, rx, (fs::SERVICE, ty)).ok()?;
    let control = Char {
        uuid: fs::CONTROL,
        props: ble::WRITE,
        value: &[0],
        max: fs::FRAME_MAX as u16,
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
    let c = ble::characteristic_add_vs(b, rx, svc, &control, ty).ok()?;
    let d = ble::characteristic_add_vs(b, rx, svc, &data, ty).ok()?;
    Some((c.0, d.0))
}

impl Phone {
    /// Whether a write to `handle` is for the file service's Control.
    pub(crate) fn is_control(&self, handle: u16) -> bool {
        self.files.control() == Some(handle)
    }
}

impl App {
    /// The phone wrote Control: a command to answer. Only on the encrypted link (the stack enforces it too).
    pub(crate) fn files_command(&mut self, written: &[u8]) {
        // rides are GPS tracks: the bond, not just an encrypted link (Just Works pairing during the PAIR window)
        if !self.phone.secure || self.phone.stored.bond.is_none() {
            return;
        }
        let busy = !self.ride_settled() || self.emmc.lent();
        let svc = &mut self.phone.files;
        svc.stall_ms = 0;
        match fs::Command::parse(written) {
            Ok(fs::Command::Cancel) => svc.reset(),
            Ok(fs::Command::List { seq }) if !busy => {
                svc.reset();
                svc.job = Job::List { seq, cur: None, count: 0 };
            }
            Ok(fs::Command::Read { seq, offset, name }) if !busy => {
                svc.reset();
                svc.job = Job::Read { seq, name, offset, file: None, seek: Seek::new(), count: 0 };
            }
            Ok(fs::Command::List { seq } | fs::Command::Read { seq, .. }) => svc.finish(seq, Status::Busy, 0),
            Err((seq, status)) => svc.finish(seq, status, 0),
        }
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
        let _ = ble::conn_param_update(b, rx, conn, FAST_PARAMS);
        let _ = ble::data_length_set(b, rx, conn, FAST_DLE);
        let _ = ble::phy_set(b, rx, conn, 2);
        self.phone.files.boosted = true;
    }

    /// Gives the normal parameters back (`PPCP` of app/phone.rs, PHY auto); the data length stays (it
    /// costs nothing idle).
    fn unboost(&mut self, b: &mut impl Board, conn: u16) {
        let rx = &mut self.rx;
        let _ = ble::conn_param_update(b, rx, conn, super::phone::PPCP);
        let _ = ble::phy_set(b, rx, conn, 0);
        self.phone.files.boosted = false;
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
                break; // the queue is full (or Data is not subscribed): the same frame next pass
            }
            svc.taken();
            (svc.stall_ms, sent) = (0, true);
        }
        let svc = &mut self.phone.files;
        if svc.out_len > 0 && !sent {
            svc.stall_ms = svc.stall_ms.saturating_add(self.dt_ms);
            if svc.stall_ms >= STALL_MS {
                svc.reset();
            }
        }
    }
}
