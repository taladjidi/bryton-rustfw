//! USB mass storage, device side: SCSI commands over the Bulk-Only Transport, so a computer mounts the eMMC's FAT
//! volume ("BRYTON") as a drive. Free of any transport: the bulk endpoints are a [`Pipe`], the sectors a
//! [`Media`]; `board/usb.rs` is the USB class (the interface, its two bulk endpoints, the two class requests) and
//! `app/drive.rs` lends the card ([`crate::emmc::Lent`]).
//!
//! The drive is a removable medium (as a card reader): the interface is always there beside the dev link, the
//! medium only while the app has lent the card ([`Drive::insert`]); without it the drive answers NOT READY,
//! MEDIUM NOT PRESENT, and a computer shows nothing. Inserted, the next command gets UNIT ATTENTION, NOT READY TO
//! READY CHANGE, which is how a computer learns it may mount it (it polls TEST UNIT READY). An eject (START STOP
//! UNIT, LoEj) takes the medium out again ([`Event::Ejected`]).
//!
//! Sources (checked 2026-10-24):
//! - USB Mass Storage Class, Bulk-Only Transport rev 1.0 (usb.org, usbmassbulk_10.pdf): the CBW (5.1: 31 bytes,
//!   "USBC", tag, length, direction bit 7, LUN, CB length 1..16) and the CSW (5.2: 13 bytes, "USBS", tag,
//!   residue, status 0 / 1 / 2); the 13 cases (6.7): a device that has less to send than the host asked "may send
//!   fill data to pad up to a total of dCBWDataTransferLength" with the residue the relevant data left out (6.7.2),
//!   and may "accept a total of dCBWDataTransferLength" when it takes less (6.7.3), which is what this does every
//!   time, so it never stalls a data phase; a CBW not valid stalls Bulk-In, Bulk-Out data is discarded, until a
//!   Reset Recovery (6.6.1); the class requests Bulk-Only Mass Storage Reset 0xff and Get Max LUN 0xfe (3.1,
//!   3.2: usb.rs).
//! - SCSI (T10 SPC-3 / SBC-2, as Seagate's public "SCSI Commands Reference Manual" lays them out): INQUIRY's 36
//!   standard bytes (RMB bit 7 of byte 1), REQUEST SENSE's fixed format (0x70, key, additional length 10, ASC,
//!   ASCQ), the MODE SENSE(6) / (10) headers (WP: bit 7 of the device-specific byte), READ CAPACITY(10) (last LBA,
//!   block length), READ / WRITE / VERIFY(10) (LBA bytes 2..5, count 7..8), START STOP UNIT (LoEj bit 1, Start bit 0
//!   of byte 4), PREVENT ALLOW MEDIUM REMOVAL (byte 4 bit 0), and the sense codes used below.
//! - READ FORMAT CAPACITIES (0x23) from the USB UFI command spec 1.0 (usb.org, usbmass-ufi10.pdf, 4.10): a 4-byte
//!   list header, one 8-byte descriptor, descriptor code 2 formatted media, 3 no media present.
//! - The `usbd-storage` crate 3.0.0 (crates.io; MIT) and TinyUSB's `msc_device.c` (github.com/hathach/tinyusb) were
//!   read for the same command set; neither is used (AGENTS.md: the crate would add `num_enum` and its proc-macro
//!   tree, and its transport lives behind usb-device, where the simulator cannot drive it).

/// The bulk packet size at high speed.
pub const PACKET: usize = 512;
/// Bytes per block: the eMMC sector.
pub const BLOCK: usize = 512;
/// Blocks read or written at once (one multi-block eMMC command, 8 KB).
pub const CHUNK: usize = 16;

/// The two bulk endpoints, as the protocol needs them. Every wait is bounded by the implementation.
pub trait Pipe {
    /// The next packet from the computer (Bulk-Out), at most [`PACKET`] bytes; None if none came in time.
    fn recv(&mut self, buf: &mut [u8; PACKET]) -> Option<usize>;
    /// Sends one packet (at most [`PACKET`] bytes) on Bulk-In; false if the computer did not take it in time.
    fn send(&mut self, data: &[u8]) -> bool;
    /// Stalls Bulk-In (a CBW not valid; the computer resets the interface).
    fn stall(&mut self);
}

/// The sectors offered.
pub trait Media {
    /// How many blocks; None if the card cannot be identified.
    fn blocks(&mut self) -> Option<u32>;
    /// Writes are allowed (else the drive is write protected).
    fn writable(&self) -> bool;
    /// Reads whole blocks from `lba` on into `buf`.
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> bool;
    /// Writes whole blocks from `lba` on.
    fn write(&mut self, lba: u32, buf: &[u8]) -> bool;
}

/// What a command did that the app should know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Nothing.
    None,
    /// The computer ejected the medium: the card is the app's again.
    Ejected,
}

/// Sense key, additional sense code and qualifier (SPC).
type Sense = (u8, u8, u8);
const NO_SENSE: Sense = (0, 0, 0);
const NOT_PRESENT: Sense = (0x02, 0x3a, 0x00);
const READ_ERROR: Sense = (0x03, 0x11, 0x00);
const WRITE_ERROR: Sense = (0x03, 0x0c, 0x00);
const BAD_OPCODE: Sense = (0x05, 0x20, 0x00);
const OUT_OF_RANGE: Sense = (0x05, 0x21, 0x00);
const BAD_FIELD: Sense = (0x05, 0x24, 0x00);
const REMOVAL_PREVENTED: Sense = (0x05, 0x53, 0x02);
const CHANGED: Sense = (0x06, 0x28, 0x00);
const PROTECTED: Sense = (0x07, 0x27, 0x00);

/// CSW status.
const PASSED: u8 = 0;
const FAILED: u8 = 1;
const PHASE: u8 = 2;

/// The drive's state between commands.
#[derive(Debug, Default)]
pub struct Drive {
    present: bool,
    attention: bool,
    prevent: bool,
    sense: Sense,
    /// A CBW was not valid: nothing more until the computer resets the interface.
    halted: bool,
    /// The computer wrote since the medium was inserted.
    wrote: bool,
    /// The last commands, for the dev link's `drivelog` (to find what makes a computer reset the device).
    pub log: Log,
}

/// One logged command: its operation code (0xff a bus reset, 0xfe a Bulk-Only reset, 0xfd a CBW not valid), first
/// block, block count, status, residue.
pub type Entry = (u8, u32, u32, u8, u32);

/// The last [`LOG`] commands, a ring.
#[derive(Clone, Copy, Debug)]
pub struct Log {
    /// Entries, the oldest overwritten.
    pub ring: [Entry; LOG],
    /// Entries ever logged: `ring[n % LOG]` is the next.
    pub n: u32,
}

/// Commands kept by [`Log`].
pub const LOG: usize = 32;

impl Default for Log {
    fn default() -> Self {
        Log::new()
    }
}

impl Log {
    const fn new() -> Self {
        Log { ring: [(0, 0, 0, 0, 0); LOG], n: 0 }
    }

    /// Logs `e`.
    pub fn push(&mut self, e: Entry) {
        if let Some(slot) = self.ring.get_mut(self.n as usize % LOG) {
            *slot = e;
        }
        self.n = self.n.wrapping_add(1);
    }

    /// The entries, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        let n = self.n as usize;
        (n.saturating_sub(LOG)..n).filter_map(|i| self.ring.get(i % LOG))
    }
}

/// What a command's data phase is, as the device sees it.
enum Data {
    None,
    In([u8; 36], usize),
    Read(u32, u32),
    Write(u32, u32),
}

fn be16(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 2).map_or(0, |x| u32::from(u16::from_be_bytes([x[0], x[1]])))
}

fn be32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

fn byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

impl Drive {
    /// No medium.
    pub const fn new() -> Self {
        Drive {
            present: false,
            attention: false,
            prevent: false,
            sense: NO_SENSE,
            halted: false,
            wrote: false,
            log: Log::new(),
        }
    }

    /// The medium is in (the card lent): the computer is told at its next command.
    pub fn insert(&mut self) {
        *self = Drive { present: true, attention: true, log: self.log, ..Drive::new() };
    }

    /// The medium is taken out by the unit (the computer may still hold it: see [`Drive::prevented`]).
    pub fn remove(&mut self) {
        (self.present, self.attention) = (false, true);
    }

    /// The medium is in.
    pub fn present(&self) -> bool {
        self.present
    }

    /// The computer asked that the medium stay in (it has it mounted).
    pub fn prevented(&self) -> bool {
        self.present && self.prevent
    }

    /// The computer wrote since the medium went in.
    pub fn wrote(&self) -> bool {
        self.wrote
    }

    /// Bulk-Only Mass Storage Reset (the class request): ready for a CBW again.
    pub fn reset(&mut self) {
        self.halted = false;
        self.log.push((0xfe, 0, 0, 0, 0));
    }

    /// A USB bus reset: ready for a CBW again, as after the class reset (a CBW not valid before it must not leave the
    /// drive deaf after the computer resets the port).
    pub fn bus_reset(&mut self) {
        self.halted = false;
        self.log.push((0xff, 0, 0, 0, 0));
    }

    /// A packet from the computer while no command runs: a CBW, run to its CSW (the data phase through `pipe`).
    pub fn packet(&mut self, cbw: &[u8], pipe: &mut impl Pipe, media: &mut impl Media) -> Event {
        if self.halted {
            return Event::None; // discarded until the reset (6.6.1)
        }
        let len = usize::from(byte(cbw, 14));
        if cbw.len() != 31 || cbw.get(..4) != Some(b"USBC") || !(1..=16).contains(&len) {
            self.halted = true;
            self.log.push((0xfd, cbw.len() as u32, u32::from(byte(cbw, 0)), 0, 0));
            pipe.stall();
            return Event::None;
        }
        let tag = [cbw[4], cbw[5], cbw[6], cbw[7]];
        let host = u32::from_le_bytes([cbw[8], cbw[9], cbw[10], cbw[11]]);
        let host_in = cbw[12] & 0x80 != 0;
        let cb = &cbw[15..15 + len];
        let mut event = Event::None;
        let (status, data) = self.command(cb, media, &mut event);
        let (status, residue) = self.transfer(status, data, host, host_in, pipe, media);
        let op = byte(cb, 0);
        let (lba, count) = if matches!(op, 0x28 | 0x2a | 0x2f) { (be32(cb, 2), be16(cb, 7)) } else { (0, host) };
        self.log.push((op, lba, count, status, residue));
        let mut csw = [0u8; 13];
        csw[..4].copy_from_slice(b"USBS");
        csw[4..8].copy_from_slice(&tag);
        csw[8..12].copy_from_slice(&residue.to_le_bytes());
        csw[12] = status;
        pipe.send(&csw);
        event
    }

    fn fail(&mut self, sense: Sense) -> (u8, Data) {
        self.sense = sense;
        (FAILED, Data::None)
    }

    /// The command's outcome before any data moves: its status and its data phase.
    fn command(&mut self, cb: &[u8], media: &mut impl Media, event: &mut Event) -> (u8, Data) {
        let op = byte(cb, 0);
        if !matches!(op, 0x12 | 0x03) {
            if core::mem::take(&mut self.attention) {
                return self.fail(CHANGED);
            }
            self.sense = NO_SENSE;
        }
        let mut out = [0u8; 36];
        let blocks = if self.present { media.blocks() } else { None };
        let wp = if media.writable() { 0 } else { 0x80 };
        match (op, blocks) {
            (0x12, _) if byte(cb, 1) & 1 != 0 => self.fail(BAD_FIELD), // EVPD pages: none
            (0x12, _) => {
                // INQUIRY: direct access, removable, SPC-2, format 2; vendor, product, revision
                out[..5].copy_from_slice(&[0x00, 0x80, 0x04, 0x02, 31]);
                out[8..36].copy_from_slice(b"rustfw  S510 FAT volume 0.1 ");
                (PASSED, Data::In(out, 36.min(be16(cb, 3) as usize)))
            }
            (0x03, _) => {
                // REQUEST SENSE, fixed format; the sense is then cleared
                let (key, asc, ascq) = core::mem::replace(&mut self.sense, NO_SENSE);
                out[..18].copy_from_slice(&[0x70, 0, key, 0, 0, 0, 0, 10, 0, 0, 0, 0, asc, ascq, 0, 0, 0, 0]);
                (PASSED, Data::In(out, 18.min(usize::from(byte(cb, 4)))))
            }
            (0x1e, _) => {
                self.prevent = byte(cb, 4) & 1 != 0; // PREVENT ALLOW MEDIUM REMOVAL
                (PASSED, Data::None)
            }
            (0x1b, _) if byte(cb, 4) & 3 == 2 => {
                // START STOP UNIT, LoEj without Start: eject
                if self.prevent && self.present {
                    return self.fail(REMOVAL_PREVENTED);
                }
                if self.present {
                    *event = Event::Ejected;
                }
                self.present = false;
                (PASSED, Data::None)
            }
            (0x1b, _) => (PASSED, Data::None),
            (0x1a, _) => {
                out[..4].copy_from_slice(&[3, 0, wp, 0]); // MODE SENSE(6): the header, no page
                (PASSED, Data::In(out, 4.min(usize::from(byte(cb, 4)))))
            }
            (0x5a, _) => {
                out[..8].copy_from_slice(&[0, 6, 0, wp, 0, 0, 0, 0]); // MODE SENSE(10)
                (PASSED, Data::In(out, 8.min(be16(cb, 7) as usize)))
            }
            (0x23, b) => {
                // READ FORMAT CAPACITIES: one descriptor, formatted media or none
                out[3] = 8;
                out[4..8].copy_from_slice(&b.unwrap_or(0).to_be_bytes());
                out[8] = if b.is_some() { 2 } else { 3 };
                out[10] = (BLOCK >> 8) as u8;
                (PASSED, Data::In(out, 12.min(be16(cb, 7) as usize)))
            }
            (_, None) if matches!(op, 0x00 | 0x25 | 0x28 | 0x2a | 0x2f | 0x35) => self.fail(NOT_PRESENT),
            (0x00 | 0x35, _) => (PASSED, Data::None), // TEST UNIT READY; SYNCHRONIZE CACHE: writes are synchronous
            (0x25, Some(n)) => {
                out[..4].copy_from_slice(&n.saturating_sub(1).to_be_bytes()); // READ CAPACITY(10): the last LBA
                out[4..8].copy_from_slice(&(BLOCK as u32).to_be_bytes());
                (PASSED, Data::In(out, 8))
            }
            (0x28 | 0x2a | 0x2f, Some(n)) => {
                let (lba, count) = (be32(cb, 2), be16(cb, 7));
                if lba.checked_add(count).is_none_or(|end| end > n) {
                    return self.fail(OUT_OF_RANGE);
                }
                match op {
                    0x2a if wp != 0 => self.fail(PROTECTED),
                    0x2a => (PASSED, Data::Write(lba, count)),
                    0x28 => (PASSED, Data::Read(lba, count)),
                    _ => (PASSED, Data::None), // VERIFY: the card checks its own ECC; nothing to compare
                }
            }
            _ => self.fail(BAD_OPCODE),
        }
    }

    /// The data phase against what the host expects (BOT 6.7): IN is always padded to the host's length, OUT
    /// always taken whole; a direction or a length the device cannot match is a phase error. Returns the status and
    /// the residue.
    fn transfer(
        &mut self,
        status: u8,
        data: Data,
        host: u32,
        host_in: bool,
        pipe: &mut impl Pipe,
        media: &mut impl Media,
    ) -> (u8, u32) {
        let want = match &data {
            Data::None => 0,
            Data::In(_, n) => *n as u32,
            Data::Read(_, n) | Data::Write(_, n) => n.saturating_mul(BLOCK as u32),
        };
        let device_in = matches!(data, Data::In(..) | Data::Read(..));
        // cases 2, 3, 7, 8, 10, 13
        let phase = want > 0 && (host < want || host_in != device_in);
        if host == 0 {
            return (if phase { PHASE } else { status }, 0);
        }
        let mut buf = [0u8; CHUNK * BLOCK];
        let mut status = if phase { PHASE } else { status };
        let mut done = 0u32; // bytes of the host's length moved
        let mut relevant = 0u32;
        match data {
            _ if phase || !host_in => {}
            Data::In(bytes, n) if n > 0 => {
                if pipe.send(bytes.get(..n.min(host as usize)).unwrap_or(&[])) {
                    (done, relevant) = (n as u32, n as u32);
                }
            }
            Data::Read(lba, count) => {
                let mut at = 0;
                while at < count {
                    let n = (count - at).min(CHUNK as u32);
                    let chunk = &mut buf[..n as usize * BLOCK];
                    if !media.read(lba + at, chunk) {
                        status = FAILED;
                        self.sense = READ_ERROR;
                        break;
                    }
                    if !chunk.chunks(PACKET).all(|p| pipe.send(p)) {
                        return (PHASE, host); // the computer stopped reading: it resets
                    }
                    at += n;
                    done += n * BLOCK as u32;
                }
                relevant = done;
            }
            _ => {}
        }
        if host_in {
            // the rest padded (6.7.2)
            let zeros = [0u8; PACKET];
            while done < host {
                let n = (host - done).min(PACKET as u32);
                if !pipe.send(&zeros[..n as usize]) {
                    return (PHASE, host - relevant);
                }
                done += n;
            }
            return (status, host - relevant);
        }
        // OUT: every byte the host sends taken (6.7.3); those of a WRITE written a chunk at a time
        let (lba, count) = match data {
            Data::Write(lba, count) if !phase => (lba, count),
            _ => (0, 0),
        };
        let mut packet = [0u8; PACKET];
        let mut fill = 0usize;
        let mut at = 0u32;
        while done < host {
            let Some(n) = pipe.recv(&mut packet) else { return (PHASE, host - relevant) };
            let n = n.min((host - done) as usize);
            done += n as u32;
            if at >= count {
                continue; // past the command's data: discarded
            }
            let take = n.min(buf.len() - fill);
            buf[fill..fill + take].copy_from_slice(&packet[..take]);
            fill += take;
            let blocks = ((count - at) as usize).min(CHUNK);
            if fill == blocks * BLOCK {
                if status == PASSED && !media.write(lba + at, &buf[..fill]) {
                    status = FAILED;
                    self.sense = WRITE_ERROR;
                }
                self.wrote = true;
                (at, fill) = (at + blocks as u32, 0);
                relevant += (blocks * BLOCK) as u32;
            }
        }
        (status, host - relevant)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::collections::VecDeque;
    use std::vec::Vec;

    /// A computer's side of the bulk endpoints: packets queued for the device, packets received.
    #[derive(Default)]
    pub struct Host {
        pub out: VecDeque<Vec<u8>>,
        pub got: Vec<Vec<u8>>,
        pub stalled: bool,
    }

    impl Pipe for Host {
        fn recv(&mut self, buf: &mut [u8; PACKET]) -> Option<usize> {
            let p = self.out.pop_front()?;
            buf[..p.len()].copy_from_slice(&p);
            Some(p.len())
        }
        fn send(&mut self, data: &[u8]) -> bool {
            assert!(data.len() <= PACKET);
            self.got.push(data.to_vec());
            true
        }
        fn stall(&mut self) {
            self.stalled = true;
        }
    }

    impl Host {
        /// The bytes received, the CSW last: (data, status, residue, tag ok).
        pub fn take(&mut self, tag: u32) -> (Vec<u8>, u8, u32) {
            let csw = self.got.pop().expect("a CSW");
            assert_eq!(csw.len(), 13);
            assert_eq!(&csw[..4], b"USBS");
            assert_eq!(csw[4..8], tag.to_le_bytes(), "the CSW's tag is the CBW's");
            let data = self.got.drain(..).flatten().collect();
            (data, csw[12], u32::from_le_bytes([csw[8], csw[9], csw[10], csw[11]]))
        }
    }

    /// A CBW: tag, host length, direction in, the command block.
    pub fn cbw(tag: u32, len: u32, dir_in: bool, cb: &[u8]) -> Vec<u8> {
        let mut c = std::vec![0u8; 31];
        c[..4].copy_from_slice(b"USBC");
        c[4..8].copy_from_slice(&tag.to_le_bytes());
        c[8..12].copy_from_slice(&len.to_le_bytes());
        c[12] = if dir_in { 0x80 } else { 0 };
        c[14] = cb.len() as u8;
        c[15..15 + cb.len()].copy_from_slice(cb);
        c
    }

    /// READ(10) / WRITE(10) command blocks.
    pub fn rw10(op: u8, lba: u32, n: u16) -> [u8; 10] {
        let mut cb = [0u8; 10];
        cb[0] = op;
        cb[2..6].copy_from_slice(&lba.to_be_bytes());
        cb[7..9].copy_from_slice(&n.to_be_bytes());
        cb
    }

    /// A RAM disk: its bytes, writable.
    pub struct Ram(pub Vec<u8>, pub bool);

    impl Media for Ram {
        fn blocks(&mut self) -> Option<u32> {
            Some((self.0.len() / BLOCK) as u32)
        }
        fn writable(&self) -> bool {
            self.1
        }
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> bool {
            let at = lba as usize * BLOCK;
            buf.copy_from_slice(&self.0[at..at + buf.len()]);
            true
        }
        fn write(&mut self, lba: u32, buf: &[u8]) -> bool {
            let at = lba as usize * BLOCK;
            self.0[at..at + buf.len()].copy_from_slice(buf);
            true
        }
    }

    pub fn ram() -> Ram {
        Ram((0..100 * BLOCK).map(|i| (i / BLOCK) as u8 ^ i as u8).collect(), true)
    }

    fn run(
        d: &mut Drive,
        h: &mut Host,
        m: &mut Ram,
        tag: u32,
        len: u32,
        dir_in: bool,
        cb: &[u8],
    ) -> (Vec<u8>, u8, u32) {
        d.packet(&cbw(tag, len, dir_in, cb), h, m);
        h.take(tag)
    }

    fn sense(d: &mut Drive, h: &mut Host, m: &mut Ram) -> (u8, u8, u8) {
        let (s, st, _) = run(d, h, m, 99, 18, true, &[0x03, 0, 0, 0, 18, 0]);
        assert_eq!(st, PASSED);
        (s[2], s[12], s[13])
    }

    #[test]
    fn no_medium_then_inserted_then_ejected() {
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        assert_eq!(run(&mut d, &mut h, &mut m, 1, 0, false, &[0; 6]).1, FAILED, "TEST UNIT READY: no medium");
        assert_eq!(sense(&mut d, &mut h, &mut m), NOT_PRESENT);
        let (inq, st, res) = run(&mut d, &mut h, &mut m, 2, 36, true, &[0x12, 0, 0, 0, 36, 0]);
        assert_eq!((st, res, inq.len(), inq[1]), (PASSED, 0, 36, 0x80), "INQUIRY: removable");
        d.insert();
        assert_eq!(run(&mut d, &mut h, &mut m, 3, 0, false, &[0; 6]).1, FAILED);
        assert_eq!(sense(&mut d, &mut h, &mut m), CHANGED, "medium changed: the computer mounts");
        assert_eq!(run(&mut d, &mut h, &mut m, 4, 0, false, &[0; 6]).1, PASSED);
        let (cap, st, _) = run(&mut d, &mut h, &mut m, 5, 8, true, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(st, PASSED);
        assert_eq!(cap, [0, 0, 0, 99, 0, 0, 2, 0], "last LBA 99, 512-byte blocks");
        run(&mut d, &mut h, &mut m, 6, 0, false, &[0x1e, 0, 0, 0, 1, 0]); // prevent
        assert!(d.prevented());
        assert_eq!(run(&mut d, &mut h, &mut m, 7, 0, false, &[0x1b, 0, 0, 0, 2, 0]).1, FAILED, "eject prevented");
        run(&mut d, &mut h, &mut m, 8, 0, false, &[0x1e, 0, 0, 0, 0, 0]); // allow
        assert_eq!(d.packet(&cbw(9, 0, false, &[0x1b, 0, 0, 0, 2, 0]), &mut h, &mut m), Event::Ejected);
        assert_eq!(h.take(9).1, PASSED);
        assert!(!d.present());
        assert_eq!(run(&mut d, &mut h, &mut m, 10, 0, false, &[0; 6]).1, FAILED);
    }

    #[test]
    fn reads_and_writes_with_residues() {
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        d.insert();
        let _ = sense(&mut d, &mut h, &mut m); // REQUEST SENSE does not clear the attention
        run(&mut d, &mut h, &mut m, 1, 0, false, &[0; 6]);
        let want = m.0[3 * BLOCK..40 * BLOCK].to_vec();
        let (data, st, res) = run(&mut d, &mut h, &mut m, 2, 37 * 512, true, &rw10(0x28, 3, 37));
        assert_eq!((st, res), (PASSED, 0));
        assert!(data == want, "37 blocks over 3 chunks");
        // the host asks for more than the command reads: padded, the residue says so (case 5)
        let (data, st, res) = run(&mut d, &mut h, &mut m, 3, 3 * 512, true, &rw10(0x28, 0, 1));
        assert_eq!((data.len(), st, res), (3 * 512, PASSED, 1024));
        // a write of 20 blocks
        let new: Vec<u8> = (0..20 * BLOCK).map(|i| (i * 7) as u8).collect();
        h.out = new.chunks(PACKET).map(|p| p.to_vec()).collect();
        let (_, st, res) = run(&mut d, &mut h, &mut m, 4, 20 * 512, false, &rw10(0x2a, 50, 20));
        assert_eq!((st, res), (PASSED, 0));
        assert!(m.0[50 * BLOCK..70 * BLOCK] == new[..]);
        assert!(d.wrote());
        // past the end: refused before any data, the host's data taken and dropped (case 9)
        h.out = std::iter::repeat_n(std::vec![0u8; 512], 2).collect();
        let (_, st, res) = run(&mut d, &mut h, &mut m, 5, 1024, false, &rw10(0x2a, 99, 2));
        assert_eq!((st, res), (FAILED, 1024));
        assert!(h.out.is_empty());
        assert_eq!(sense(&mut d, &mut h, &mut m), OUT_OF_RANGE);
        // the host expects less than the command reads: phase error (case 7)
        assert_eq!(run(&mut d, &mut h, &mut m, 6, 512, true, &rw10(0x28, 0, 2)).1, PHASE);
        // the host expects to send for a read: phase error, its data taken (case 10)
        h.out.push_back(std::vec![0u8; 512]);
        assert_eq!(run(&mut d, &mut h, &mut m, 7, 512, false, &rw10(0x28, 0, 1)).1, PHASE);
        assert!(h.out.is_empty());
        // no data expected by the host for a read (case 2)
        assert_eq!(run(&mut d, &mut h, &mut m, 8, 0, true, &rw10(0x28, 0, 1)), (Vec::new(), PHASE, 0));
    }

    #[test]
    fn write_protected_and_unknown_commands() {
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        m.1 = false;
        d.insert();
        run(&mut d, &mut h, &mut m, 1, 0, false, &[0; 6]);
        let (ms, _, _) = run(&mut d, &mut h, &mut m, 2, 192, true, &[0x1a, 0, 0x3f, 0, 192, 0]);
        assert_eq!((ms.len(), ms[2]), (192, 0x80), "MODE SENSE(6): WP, padded to the host's length");
        let (ms, _, res) = run(&mut d, &mut h, &mut m, 3, 8, true, &[0x5a, 0, 0x3f, 0, 0, 0, 0, 0, 8, 0]);
        assert_eq!((ms[3], res), (0x80, 0));
        h.out.push_back(std::vec![0u8; 512]);
        assert_eq!(run(&mut d, &mut h, &mut m, 4, 512, false, &rw10(0x2a, 0, 1)).1, FAILED);
        assert_eq!(sense(&mut d, &mut h, &mut m), PROTECTED);
        assert_eq!(run(&mut d, &mut h, &mut m, 5, 0, false, &[0xa0, 0, 0, 0, 0, 0]).1, FAILED);
        assert_eq!(sense(&mut d, &mut h, &mut m), BAD_OPCODE);
        let (fc, st, _) = run(&mut d, &mut h, &mut m, 6, 252, true, &[0x23, 0, 0, 0, 0, 0, 0, 0, 252, 0]);
        assert_eq!((st, &fc[..12]), (PASSED, &[0, 0, 0, 8, 0, 0, 0, 100, 2, 0, 2, 0][..]));
    }

    #[test]
    fn a_bad_cbw_stalls_until_reset() {
        let (mut d, mut h, mut m) = (Drive::new(), Host::default(), ram());
        let mut bad = cbw(1, 0, false, &[0; 6]);
        bad[0] = b'X';
        d.packet(&bad, &mut h, &mut m);
        assert!(h.stalled && h.got.is_empty(), "no CSW for a CBW not valid");
        d.packet(&cbw(2, 0, false, &[0; 6]), &mut h, &mut m);
        assert!(h.got.is_empty(), "nothing until the reset");
        d.reset();
        assert_eq!(run(&mut d, &mut h, &mut m, 3, 0, false, &[0; 6]).1, FAILED);
        d.packet(&cbw(4, 0, false, &[0; 6])[..30], &mut h, &mut m);
        assert!(d.halted, "a short CBW is not valid");
        d.reset();
        d.packet(&cbw(5, 0, false, &[0; 17][..0]), &mut h, &mut m);
        assert!(d.halted, "a CB length of 0 is not valid");
        d.bus_reset();
        assert!(!d.halted, "a bus reset too: the computer resetting the port gets a drive that listens");
        let ops: Vec<u8> = d.log.iter().map(|e| e.0).collect();
        assert_eq!(ops, [0xfd, 0xfe, 0x00, 0xfd, 0xfe, 0xfd, 0xff], "logged: bad CBWs, resets, the command");
    }
}
