//! The simulated eMMC (`EmmcCard`): the card as SDMMC1 sees it, strict like `gnss_model.rs`: its rules with their
//! sources, and a loud failure (an assert) where the driver does what the real card would punish with garbage.
//!
//! Sources: JEDEC JESD84-B51 (eMMC 5.1) is not public; the R1 bits, their types and clear conditions as Linux's
//! include/linux/mmc/mmc.h (fetched 2026-11-02: "e" error, "r" set in the command's own answer, "x" set during
//! execution and read with CMD13; clear condition "b" the previous command's, cleared by the next valid one, "c"
//! clear by read), which match the SD Physical Layer Simplified Specification's card status table; what a host
//! must check, as drivers/mmc/core/block.c (fetched 2026-11-02: `mmc_blk_busy_cb` "Accumulate response error bits"
//! over the CMD13 polls, `mmc_blk_status_error` checks the command's, the stop's (CMD12) and the polls' status).
//! - E1 a write takes time ([`super::Cost`]: per command and per sector, **inferred** until benched on the unit), and
//!   every [`super::Cost::stall_every`] sectors the card stays busy [`super::Cost::stall_ms`] after the write
//!   (garbage collection, **inferred** 300 ms): CMD13 answers PRG (state 7), not READY_FOR_DATA, until it ends, so
//!   the driver's own 1 ms polls wait it out, and give up (`Busy`) past [`crate::emmc::TRIES`]. A CMD0 does not end
//!   it: CMD1 answers busy until the programming is over (**inferred**). `prog_polls` is the count-based variant.
//! - E2 R1 error bits: ILLEGAL_COMMAND (a command not legal in the state: no answer, the bit in the next answer,
//!   type "er, b"), WP_VIOLATION (a write touching [`EmmcCard::wp`]: nothing lands, the bit in the next answer, CMD12's
//!   for an open-ended CMD25, else the first CMD13; "erx, c"), and the bits a test injects: in the next answer
//!   ([`EmmcCard::pending`]), in a write's own ([`EmmcCard::write_answer`]), in its stop's ([`EmmcCard::stop_error`])
//!   or in the first CMD13 after its data, or after its stop: CMD12 is R1b, answered before the programming busy
//!   ([`EmmcCard::prog_error`]: ERROR, CC_ERROR...: "erx, c"). An answer that carries pending bits clears them
//!   (clear by read). Which bits a host checks where: [`DATA_ERRORS`], [`COMMAND_ERRORS`].
//! - E3 the CRC status token of a written block: [`EmmcCard::bad`] answers a CRC error (`BusError::Crc`).
//! - E4 an URGENT_BKOPS-style stall ([`EmmcCard::bkops`]): busy as E1 for N ms, and R1's EXCEPTION_EVENT (bit 6,
//!   "sr, a": a status, not an error) in every answer meanwhile (EXT_CSD EXCEPTION_EVENTS_STATUS URGENT_BKOPS; that
//!   the unit's card raises it, and for how long, is **inferred**).
//! - E5 a power cut mid-write ([`EmmcCard::cut_after`], `tear`, `tear_prefix`): a plain write may leave the sector
//!   being written garbage, a reliable write keeps it old or new.
//! - E6 identification at 400 kHz only, no HS_TIMING switch (asserts).
//!
//! Not modelled: erase and discard, the cache (CACHE_CTRL), BKOPS started by the host, a stall between the blocks of
//! one multi-block write (the controller's data time-out on the unit).

extern crate std;

use super::{EMMC_SECTORS, emmc_sector};
use std::vec::Vec;

/// R1 ADDRESS_OUT_OF_RANGE (mmc.h `R1_OUT_OF_RANGE`, "er, c").
pub const OUT_OF_RANGE: u32 = 1 << 31;
/// R1 WP_VIOLATION ("erx, c").
pub const WP_VIOLATION: u32 = 1 << 26;
/// R1 ILLEGAL_COMMAND ("er, b").
pub const ILLEGAL_COMMAND: u32 = 1 << 22;
/// R1 EXCEPTION_EVENT ("sr, a"): not an error.
pub const EXCEPTION_EVENT: u32 = 1 << 6;
/// The R1 error bits that describe a write's data, found during it or its programming (Linux block.c `CMD_ERRORS`):
/// reported in its stop's answer or a CMD13 poll as well as in its own answer (mmc.h).
pub const DATA_ERRORS: [(u32, &str); 7] = [
    (1 << 31, "OUT_OF_RANGE"),
    (1 << 30, "ADDRESS_ERROR"),
    (1 << 29, "BLOCK_LEN_ERROR"),
    (1 << 26, "WP_VIOLATION"),
    (1 << 21, "CARD_ECC_FAILED"),
    (1 << 20, "CC_ERROR"),
    (1 << 19, "ERROR"),
];
/// The other R1 error bits (mmc.h): about the command whose answer carries them (COM_CRC_ERROR and ILLEGAL_COMMAND,
/// "b": the previous command's), so the driver rejects them in a write's own answer only. UNDERRUN and OVERRUN
/// (17, 18) are stream-mode bits, outside Linux's R1_STATUS mask too; the erase and lock bits stay in since a card
/// may set them on any command.
pub const COMMAND_ERRORS: [(u32, &str); 7] = [
    (1 << 28, "ERASE_SEQ_ERROR"),
    (1 << 27, "ERASE_PARAM"),
    (1 << 24, "LOCK_UNLOCK_FAILED"),
    (1 << 23, "COM_CRC_ERROR"),
    (1 << 22, "ILLEGAL_COMMAND"),
    (1 << 16, "CID_CSD_OVERWRITE"),
    (1 << 7, "SWITCH_ERROR"),
];

/// The eMMC as SDMMC1 sees it: the JEDEC states (idle 0, ready 1, ident 2, stby 3, tran 4, data 5), CMD1 busy for a
/// few polls, an EXT_CSD with [`EMMC_SECTORS`], sector `n` filled with [`emmc_sector`]. It starts as the bootloader
/// leaves it: selected, in the transfer state, on a 4-bit bus. Driver mistakes that a real card would punish with
/// garbage fail the test (identification at 25 MHz, any write or erase command); illegal commands are not answered and
/// set ILLEGAL_COMMAND in the next answer; the module doc lists the rules.
pub struct EmmcCard {
    /// Never answers (missing or dead card).
    pub dead: bool,
    /// CMD1 answers busy this many times after a CMD0 (u32::MAX: forever).
    pub busy_polls: u32,
    /// Every command, in order: (index, argument).
    pub cmds: Vec<(u8, u32)>,
    /// What the card holds from sector 0 on, zeros after it; empty: [`emmc_sector`] everywhere.
    pub image: Vec<u8>,
    /// Sectors written so far; the power goes after this many (the card then never answers again).
    pub sectors_written: u64,
    pub cut_after: Option<u64>,
    /// Writes touching these sectors fail (a CRC error), nothing of them landing: a card failing in one place.
    pub bad: Vec<core::ops::Range<u32>>,
    /// Some(n): only the next `n` writes touching [`EmmcCard::bad`] fail (a card failing once there).
    pub bad_left: Option<u32>,
    /// Reads touching these sectors fail (a read error), the card otherwise as it is: a file that cannot be read yet.
    pub unreadable: Vec<core::ops::Range<u32>>,
    /// Bytes of sectors read so far (the cost of a pass: [`crate::App::SCAN_IDLE`]).
    pub bytes_read: u64,
    /// A cut tears the sector being written (garbage in it, as JEDEC allows for a plain write); a reliable write's
    /// sector keeps its old data.
    pub tear: bool,
    /// With [`EmmcCard::tear`]: the torn sector keeps its first k bytes new and the rest old instead, a sector a
    /// reader may accept (the review of 2026-11-01, finding 3; inferred model).
    pub tear_prefix: Option<usize>,
    /// CMD13 answers "programming" (state 7, not ready for data) this many times after each write, so the driver's
    /// 1 ms retry runs; [`crate::emmc::TRIES`] or more: the driver gives up (`Busy`) though the write landed.
    pub prog_polls: u32,
    /// Writes touching [`EmmcCard::bad`] land their sectors before the first bad one, then fail: a multi-sector
    /// write cut short, instead of one that lands nothing.
    pub bad_partial: bool,
    /// Writes to these sectors are dropped and still answered as done: a card that lies.
    pub lie: Option<core::ops::Range<u32>>,
    /// The "programming" answers left (see `prog_polls`).
    prog_left: u32,
    /// EXT_CSD WR_REL_PARAM and REL_WR_SEC_C (emmc.rs): the reliable write offered; none by default.
    pub rel_param: u8,
    pub rel_sectors: u8,
    /// CMD23's block count and reliable bit, for the CMD25 that follows.
    count: Option<(u32, bool)>,
    state: u32,
    busy_left: u32,
    pub(super) card_wide: bool,
    pub(super) host_wide: bool,
    pub(super) host_fast: bool,
    pub(super) powered: bool,
    /// Error bits for the next answer (E2), cleared by it. A test may add its own.
    pub pending: u32,
    /// Error bits the card finds while programming the next write (E2, the "x" types: ERROR, CC_ERROR,
    /// CARD_ECC_FAILED...): put in [`EmmcCard::pending`] once its data is in, so the next answer carries them.
    pub prog_error: u32,
    /// Error bits the next write's stop (CMD12 after an open-ended CMD25) answers with: found while the data came in.
    pub stop_error: u32,
    /// Error bits the next write command's own answer carries.
    pub write_answer: u32,
    /// `prog_error` of an open-ended write, held until its stop has answered (CMD12 is R1b: its answer goes out
    /// before the programming busy, so the first CMD13 after it reports what the programming found).
    after_stop: u32,
    /// Write protected sectors (E2).
    pub wp: Vec<core::ops::Range<u32>>,
    /// Busy programming until this time of the simulator's clock, ms (E1, E4).
    pub busy_until: u64,
    /// EXCEPTION_EVENT in the answers while busy (E4).
    pub urgent_bkops: bool,
    /// The simulator's clock, ms, as of the command being answered.
    pub(super) now: u64,
}

impl Default for EmmcCard {
    fn default() -> Self {
        EmmcCard {
            dead: false,
            busy_polls: 3,
            cmds: Vec::new(),
            image: Vec::new(),
            sectors_written: 0,
            cut_after: None,
            bad: Vec::new(),
            bad_left: None,
            unreadable: Vec::new(),
            bytes_read: 0,
            tear: false,
            tear_prefix: None,
            prog_polls: 0,
            bad_partial: false,
            lie: None,
            prog_left: 0,
            rel_param: 0,
            rel_sectors: 0,
            count: None,
            state: 4,
            busy_left: 0,
            card_wide: true,
            host_wide: false,
            host_fast: false,
            powered: false,
            pending: 0,
            prog_error: 0,
            stop_error: 0,
            write_answer: 0,
            after_stop: 0,
            wp: Vec::new(),
            busy_until: 0,
            urgent_bkops: false,
            now: 0,
        }
    }
}

/// The response each command the driver sends with `emmc_cmd` takes. Source: the MMC command set (JESD84-B51, not
/// public; the names and response types as Linux's include/linux/mmc/mmc.h and the driver's own notes, inferred).
/// R1b (busy after the response) is R1 here, as [`crate::emmc::Resp`] says. A command not listed panics: the test
/// fails until its response type is added here.
pub(super) fn expected_resp(index: u8) -> crate::emmc::Resp {
    use crate::emmc::Resp;
    match index {
        0 | 4 | 15 => Resp::None,
        1 => Resp::R3,
        2 | 9 | 10 => Resp::R2,
        3 | 6 | 7 | 8 | 12 | 13 | 16 | 17 | 18 | 23 | 24 | 25 => Resp::R1,
        _ => panic!("unlisted eMMC command {index}: add it to expected_resp"),
    }
}

impl EmmcCard {
    /// Busy for `ms` from `now` with URGENT_BKOPS raised (E4): every command but CMD13 waits it out.
    pub fn bkops(&mut self, now: u64, ms: u64) {
        (self.busy_until, self.urgent_bkops) = (now + ms, true);
    }

    fn busy(&self) -> bool {
        self.now < self.busy_until
    }

    /// The state as the card is: the transfer state reads as programming (7) while busy (E1).
    fn state(&self) -> u32 {
        if self.state == 4 && self.busy() { 7 } else { self.state }
    }

    /// Card status as R1 carries it for a command received in state `st`: the state, READY_FOR_DATA but while
    /// programming, EXCEPTION_EVENT during a BKOPS stall, and the pending errors, which this answer clears (E2).
    fn answer(&mut self, st: u32) -> u32 {
        let s = st << 9 | u32::from(st != 7) << 8 | u32::from(self.busy() && self.urgent_bkops) << 6 | self.pending;
        self.pending = 0;
        s
    }

    /// A command not legal in the state: no answer, ILLEGAL_COMMAND in the next one (E2).
    fn illegal(&mut self) -> crate::emmc::BusError {
        self.pending |= ILLEGAL_COMMAND;
        crate::emmc::BusError::Timeout
    }

    pub(super) fn cmd(&mut self, index: u8, arg: u32) -> Result<[u32; 4], crate::emmc::BusError> {
        use crate::emmc::BusError::Timeout;
        assert!(self.powered, "eMMC command {index} before the controller was powered");
        self.cmds.push((index, arg));
        assert!(
            ![20, 26, 27, 28, 29, 35, 36, 38, 42, 49].contains(&index),
            "erase, protect, lock or CSD / CID command {index} to the eMMC"
        );
        assert!(index != 6 || arg == crate::emmc::SWITCH_BUS_4BIT, "CMD6 other than the bus width: {arg:#x}");
        if [1, 2, 3].contains(&index) {
            assert!(!self.host_fast, "identification command {index} at 25 MHz");
        }
        if self.dead {
            // CMD0 has no response: the controller only knows it went out
            return if index == 0 { Ok([0; 4]) } else { Err(Timeout) };
        }
        let st = self.state(); // the state the command came in, which its answer carries
        let r1 = |c: &mut Self, st| Ok([c.answer(st), 0, 0, 0]);
        match (index, st) {
            (0, _) => {
                (self.state, self.card_wide, self.busy_left, self.pending) = (0, false, self.busy_polls, 0);
                Ok([0; 4])
            }
            (1, 0 | 1) => {
                self.state = 1;
                if self.busy() {
                    return Ok([0x00ff_8080, 0, 0, 0]); // still programming what came before the CMD0 (inferred)
                }
                if self.busy_left > 0 {
                    self.busy_left -= 1;
                    return Ok([0x00ff_8080, 0, 0, 0]);
                }
                Ok([0xc0ff_8080, 0, 0, 0]) // ready, sector addressing
            }
            (2, 1) => {
                self.state = 2;
                Ok([0x1501_0044, 0x4a52_3341, 0x0000_0000, 0x0000_0001]) // a CID
            }
            (3, 2) => {
                assert_eq!(arg, 1 << 16, "RCA");
                self.state = 3;
                r1(self, st)
            }
            (7, 3) if arg == 1 << 16 => {
                self.state = 4;
                r1(self, st)
            }
            (13, 4) if arg == 1 << 16 && self.prog_left > 0 => {
                self.prog_left -= 1;
                r1(self, 7) // programming, not ready for data
            }
            (13, _) if arg == 1 << 16 && st >= 3 => r1(self, st),
            (16, 4) => {
                assert_eq!(arg, 512, "block length");
                r1(self, st)
            }
            (6, 4) => {
                self.card_wide = true;
                r1(self, st)
            }
            (23, 4) => {
                let (n, rel) = (arg & 0xffff, arg & 1 << 31 != 0);
                assert!(n > 0, "CMD23 of no block");
                if rel {
                    assert!(
                        self.rel_param & 4 != 0 || self.rel_sectors != 0,
                        "a reliable write the card does not offer"
                    );
                    assert!(self.rel_param & 4 != 0 || n == 1, "a legacy reliable write of {n} sectors");
                }
                self.count = Some((n, rel));
                r1(self, st)
            }
            (12, 5 | 6) => {
                self.state = 4;
                if st == 6 {
                    self.pending |= core::mem::take(&mut self.stop_error);
                }
                let r = r1(self, st);
                self.pending |= core::mem::take(&mut self.after_stop); // for the first CMD13 after the stop
                r
            }
            _ => Err(self.illegal()),
        }
    }

    pub(super) fn read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, crate::emmc::BusError> {
        use crate::emmc::BusError::{Crc, Timeout};
        assert!(buf.len().is_multiple_of(512) && !buf.is_empty(), "eMMC read of {} bytes", buf.len());
        let s = self.cmd_data(index, arg)?;
        if index == 8 {
            if self.host_wide != self.card_wide {
                return Err(Crc); // the bus widths differ: the data comes out as garbage
            }
            buf.fill(0);
            buf[212..216].copy_from_slice(&EMMC_SECTORS.to_le_bytes());
            buf[183] = u8::from(self.card_wide);
            buf[192] = 8; // EXT_CSD_REV
            (buf[166], buf[222]) = (self.rel_param, self.rel_sectors);
            return Ok(s);
        }
        if arg.saturating_add((buf.len() / 512) as u32) > EMMC_SECTORS {
            self.pending |= OUT_OF_RANGE;
            return Err(Timeout); // out of range: no data, the controller's data time-out
        }
        if index == 18 {
            self.state = 5; // sending until CMD12, whatever the host makes of the data
        }
        let end = u64::from(arg) + (buf.len() / 512) as u64;
        if self.unreadable.iter().any(|b| u64::from(b.start) < end && u64::from(arg) < u64::from(b.end)) {
            return Err(Crc);
        }
        if self.host_wide != self.card_wide {
            return Err(Crc); // the bus widths differ: the data comes out as garbage
        }
        for (i, chunk) in buf.chunks_exact_mut(512).enumerate() {
            let lba = arg + i as u32;
            match self.image.len() {
                0 => chunk.copy_from_slice(&emmc_sector(lba)),
                len => {
                    let at = (lba as usize * 512).min(len);
                    let have = &self.image[at..(at + 512).min(len)];
                    chunk[..have.len()].copy_from_slice(have);
                    chunk[have.len()..].fill(0);
                }
            }
        }
        self.bytes_read += buf.len() as u64;
        Ok(s)
    }

    /// CMD24 / CMD25: the sectors land one by one, until the power cut if one is set.
    pub(super) fn write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, crate::emmc::BusError> {
        use crate::emmc::BusError::Timeout;
        assert!(self.powered, "eMMC write before the controller was powered");
        assert!(buf.len().is_multiple_of(512) && !buf.is_empty(), "eMMC write of {} bytes", buf.len());
        assert!([24, 25].contains(&index), "eMMC write command {index}");
        let count = self.count.take();
        let blocks = (buf.len() / 512) as u32;
        match count {
            Some((n, _)) => assert!(index == 25 && n == blocks, "CMD23 for {n} blocks, then CMD{index} of {blocks}"),
            None => assert_eq!(index == 24, blocks == 1, "CMD24 is one block, CMD25 more"),
        }
        let reliable = count.is_some_and(|c| c.1);
        self.cmds.push((index, arg));
        if self.dead {
            return Err(Timeout);
        }
        if self.state() != 4 {
            return Err(self.illegal());
        }
        let end = u64::from(arg) + (buf.len() / 512) as u64;
        assert!(end <= u64::from(EMMC_SECTORS - crate::emmc::RESERVED), "a write at or past B: {arg:#x}");
        let s = self.answer(4) | core::mem::take(&mut self.write_answer); // the answer goes out before the data
        // receiving: an open-ended CMD25 until CMD12, a counted one until its count, which ends it (`tran`)
        self.state = if index == 25 { 6 } else { 4 };
        let tran = |c: &mut Self| {
            if count.is_some() {
                c.state = 4;
            }
        };
        if self.wp.iter().any(|w| u64::from(w.start) < end && arg < w.end) {
            self.pending |= WP_VIOLATION; // found during the data: nothing lands, the next answer says so
            tran(self);
            return Ok(s);
        }
        let bad = self.bad.iter().filter(|b| u64::from(b.start) < end && arg < b.end).map(|b| b.start.max(arg)).min();
        let bad = bad.filter(|_| self.bad_left != Some(0));
        if bad.is_some() {
            self.bad_left = self.bad_left.map(|n| n - 1);
        }
        // the sectors that land: all, or with `bad_partial` those before the first bad one
        let lands = match bad {
            Some(b) if self.bad_partial => (b - arg) as usize,
            Some(_) => return Err(crate::emmc::BusError::Crc),
            None => buf.len() / 512,
        };
        assert!(!self.image.is_empty(), "a write to the card without an image");
        for (i, chunk) in buf.chunks_exact(512).enumerate().take(lands) {
            let at = (arg as usize + i) * 512;
            if self.image.len() < at + 512 {
                self.image.resize(at + 512, 0);
            }
            if self.cut_after.is_some_and(|c| self.sectors_written >= c) {
                self.dead = true; // the power is gone: this sector and the rest never land
                if self.tear && !reliable {
                    let s = &mut self.image[at..at + 512];
                    match self.tear_prefix {
                        // a prefix new, the rest old
                        Some(k) => s[..k].copy_from_slice(&chunk[..k]),
                        // half old, half new, the rest garbage (inferred model)
                        None => {
                            s[..128].copy_from_slice(&chunk[..128]);
                            s[256..].iter_mut().enumerate().for_each(|(k, b)| *b = (k as u8).wrapping_mul(97) ^ 0x5a);
                        }
                    }
                }
                return Err(Timeout);
            }
            if !self.lie.as_ref().is_some_and(|l| l.contains(&(arg + i as u32))) {
                self.image[at..at + 512].copy_from_slice(chunk);
            }
            self.sectors_written += 1;
        }
        if bad.is_some() {
            return Err(crate::emmc::BusError::Crc);
        }
        self.prog_left = self.prog_polls;
        let found = core::mem::take(&mut self.prog_error);
        if index == 25 && count.is_none() {
            self.after_stop |= found;
        } else {
            self.pending |= found; // the first CMD13 reports it
        }
        tran(self);
        Ok(s)
    }

    fn cmd_data(&mut self, index: u8, arg: u32) -> Result<u32, crate::emmc::BusError> {
        assert!(self.powered, "eMMC command {index} before the controller was powered");
        self.cmds.push((index, arg));
        assert!([8, 17, 18].contains(&index), "eMMC data command {index}");
        if self.dead {
            return Err(crate::emmc::BusError::Timeout);
        }
        if self.state() != 4 {
            return Err(self.illegal());
        }
        Ok(self.answer(4))
    }
}
