//! The eMMC on SDMMC1 (BOARD.md 15), **read only**: the card's identification, the bus set to 4 bits at 25 MHz,
//! SEC_COUNT from the EXT_CSD, single and multi-block reads, and stock's layout derived from SEC_COUNT.
//!
//! The protocol (JEDEC eMMC) lives here, against four [`Board`] primitives: power the controller on, set the bus,
//! send a command, read blocks through the FIFO. `sim.rs` has a model of the card (`EmmcCard`).
//!
//! **Nothing here writes to the card.** No CMD24 / CMD25 (write), no erase (CMD35 / 36 / 38), no lock, no CSD
//! or CID programming. The one CMD6 SWITCH is the bus width, EXT_CSD byte 183 BUS_WIDTH, a volatile byte the card
//! clears at every CMD0 and power cycle, the same switch stock and the bootloader make at each start. The
//! simulated card fails any test that sends anything else.
//!
//! Differences from stock's sequence (BOARD.md 15), all on purpose:
//! - **No CMD8 (SD SEND_IF_COND) or ACMD41 (SD SEND_OP_COND) before CMD1.** They are SD card probes from stock's
//!   generic SD / MMC driver. An eMMC does not answer them (JEDEC: CMD8 in the idle state and CMD55 are illegal
//!   for an MMC, which leaves illegal commands unanswered), so stock waits through 256 failed ACMD41 tries,
//!   about 0.5 s, for nothing. The card is soldered: it is always an eMMC. ST's own MMC driver, which the
//!   bootloader uses (HAL_MMC_Init), goes straight from CMD0 to CMD1 (inferred from ST's public HAL source).
//! - **No CMD9 (CSD).** Stock reads it for the maximum clock (TRAN_SPEED); every eMMC from MMC 4.0 on takes
//!   26 MHz at default speed, and this driver never goes above 25 MHz.
//! - **The EXT_CSD read twice**, at 1 bit then at 4 bits, as a check of the 4-bit bus before any other read.

use crate::{Board, fat};

/// Bytes per sector (block).
pub const SECTOR: usize = 512;
/// Sectors at the end of the card outside stock's FAT volume (BOARD.md 15: B = SEC_COUNT - this).
pub const RESERVED: u32 = 0x1_201a;
/// Sectors of the OS area at the very end of the card (6 MB), what the bootloader copies to SDRAM.
pub const OS_SECTORS: u32 = 0x3000;
/// The relative card address this driver gives the card (CMD3), as stock (RCA 1), in the argument's high half.
const RCA: u32 = 1 << 16;
/// CMD1 argument: 2.7 to 3.6 V, sector addressing (as stock).
const OCR_ARG: u32 = 0x40ff_8000;
/// CMD6 argument: write byte (3), EXT_CSD index 183 BUS_WIDTH, value 1 = 4 bits, command set 1 (as stock).
pub const SWITCH_BUS_4BIT: u32 = 0x03b7_0101;
/// Card status (R1) bits that are errors (JEDEC): out of range, misaligned, block length, erase sequence and
/// parameter, write protect, lock failed, command CRC, illegal command, ECC, controller, general error,
/// CID / CSD overwrite, switch error.
const STATUS_ERRORS: u32 = 0xfdf9_0080;
/// Card status: READY_FOR_DATA.
const READY_FOR_DATA: u32 = 1 << 8;
/// Card status CURRENT_STATE values (bits 9..12).
const STATE_TRAN: u32 = 4;
const STATE_DATA: u32 = 5;
const STATE_RCV: u32 = 6;
/// How many times CMD1 (1 ms apart) and CMD13 (1 ms apart when not ready) are tried: 1 s, the JEDEC limit for
/// the card's power-up.
const TRIES: u32 = 1000;

/// The response a command waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resp {
    /// None (CMD0).
    None,
    /// 48 bits with CRC: the card status (R1; R1b too, whose busy this driver waits out with CMD13).
    R1,
    /// 136 bits (R2: CID, CSD); the words from bits 127..96 down.
    R2,
    /// 48 bits whose CRC field is not a CRC (R3: the OCR).
    R3,
}

/// What the controller saw go wrong on the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    /// No response, or no data in time (also a dead or missing card).
    Timeout,
    /// CRC error on the response or on the data.
    Crc,
    /// The FIFO overflowed (data lost).
    Overrun,
}

/// Why a card operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Command `.0` failed on the bus.
    Bus(u8, BusError),
    /// The card stayed busy: CMD1 never reported ready, or CMD13 never the transfer state.
    Busy,
    /// Command `.0` answered a card status `.1` with error bits.
    Status(u8, u32),
    /// Past the card's last sector, or a buffer that is not a whole number of sectors.
    Range,
    /// The EXT_CSD's SEC_COUNT is too small for stock's layout, or differs between the 1-bit and the 4-bit read.
    ExtCsd,
}

/// Stock's layout of the card, from SEC_COUNT (BOARD.md 15).
///
/// ```
/// let l = bryton_rustfw::emmc::Layout::new(0x03a3_e000).unwrap();
/// assert_eq!(l.fat_end, 0x03a3_e000 - 0x1_201a); // B: the FAT volume is sectors 0 .. B-1
/// assert_eq!(l.os_start, 0x03a3_b000); // the last 0x3000 sectors
/// assert_eq!(l.os_start, l.fat_end + 0xf01a);
/// assert!(bryton_rustfw::emmc::Layout::new(0x1_201a).is_none());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// SEC_COUNT: the user area in sectors.
    pub sectors: u32,
    /// B: the first sector after the FAT volume (the reserved area starts here).
    pub fat_end: u32,
    /// The first sector of the OS area.
    pub os_start: u32,
}

impl Layout {
    /// The layout of a card of `sectors`; None if it is too small to hold stock's reserved area.
    pub fn new(sectors: u32) -> Option<Layout> {
        let fat_end = sectors.checked_sub(RESERVED).filter(|&b| b > 0)?;
        Some(Layout { sectors, fat_end, os_start: sectors - OS_SECTORS })
    }
}

fn cmd(b: &mut impl Board, index: u8, arg: u32, resp: Resp) -> Result<[u32; 4], Error> {
    b.emmc_cmd(index, arg, resp).map_err(|e| Error::Bus(index, e))
}

/// A command with an R1 answer, its error bits checked.
fn r1(b: &mut impl Board, index: u8, arg: u32) -> Result<u32, Error> {
    status(index, cmd(b, index, arg, Resp::R1)?[0])
}

fn status(index: u8, s: u32) -> Result<u32, Error> {
    if s & STATUS_ERRORS != 0 { Err(Error::Status(index, s)) } else { Ok(s) }
}

/// CMD13 until the card is in the transfer state and ready for data, as stock (a card left sending or receiving
/// is stopped with CMD12 first). Returns the last status; its error bits are left to the caller (reading them
/// clears them).
fn wait_ready(b: &mut impl Board) -> Result<u32, Error> {
    for _ in 0..TRIES {
        let s = cmd(b, 13, RCA, Resp::R1)?[0];
        match (s >> 9) & 0xf {
            STATE_TRAN if s & READY_FOR_DATA != 0 => return Ok(s),
            STATE_DATA | STATE_RCV => {
                cmd(b, 12, 0, Resp::R1)?;
            }
            _ => b.delay_ms(1),
        }
    }
    Err(Error::Busy)
}

/// Reads the EXT_CSD (CMD8, one 512-byte block) and returns its SEC_COUNT (bytes 212..215, little endian).
fn sec_count(b: &mut impl Board) -> Result<u32, Error> {
    let mut ext = [0u8; SECTOR];
    status(8, b.emmc_read(8, 0, &mut ext).map_err(|e| Error::Bus(8, e))?)?;
    Ok(u32::from_le_bytes([ext[212], ext[213], ext[214], ext[215]]))
}

/// An identified card, selected, in the transfer state, on a 4-bit bus at 25 MHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// Stock's layout, from the card's SEC_COUNT.
    pub layout: Layout,
    /// Sector addressing (OCR bit 30, every card over 2 GB); else byte addresses.
    sector_mode: bool,
}

impl Card {
    /// Identifies the card from CMD0, whatever state the bootloader or an earlier run left it in. About 10 ms on a
    /// card that answers; at most about 1 s on one that stays busy; a few ms on one that never answers.
    pub fn init(b: &mut impl Board) -> Result<Card, Error> {
        b.emmc_power_on();
        cmd(b, 0, 0, Resp::None)?; // GO_IDLE: from any state, bus back to 1 bit
        let mut ocr = 0;
        for _ in 0..TRIES {
            ocr = cmd(b, 1, OCR_ARG, Resp::R3)?[0];
            if ocr & (1 << 31) != 0 {
                break;
            }
            b.delay_ms(1);
        }
        if ocr & (1 << 31) == 0 {
            return Err(Error::Busy);
        }
        cmd(b, 2, 0, Resp::R2)?; // ALL_SEND_CID: ready -> ident (the CID itself is not needed)
        r1(b, 3, RCA)?; // SET_RELATIVE_ADDR: ident -> stby, RCA 1
        b.emmc_bus(true, false); // identification over: 25 MHz
        r1(b, 7, RCA)?; // SELECT: stby -> tran
        wait_ready(b)?;
        r1(b, 16, SECTOR as u32)?; // SET_BLOCKLEN 512, as stock
        let sectors = sec_count(b)?;
        r1(b, 6, SWITCH_BUS_4BIT)?;
        status(6, wait_ready(b)?)?; // SWITCH_ERROR shows here
        b.emmc_bus(true, true);
        if sec_count(b)? != sectors {
            return Err(Error::ExtCsd);
        }
        let layout = Layout::new(sectors).ok_or(Error::ExtCsd)?;
        Ok(Card { layout, sector_mode: ocr & (1 << 30) != 0 })
    }

    /// Reads `buf.len() / 512` sectors from `lba` on: CMD17 for one, CMD18 then CMD12 for more.
    pub fn read(&self, b: &mut impl Board, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        let end = lba.checked_add(n).ok_or(Error::Range)?;
        if n == 0 || !buf.len().is_multiple_of(SECTOR) || end > self.layout.sectors {
            return Err(Error::Range);
        }
        let arg = if self.sector_mode { lba } else { lba.checked_mul(SECTOR as u32).ok_or(Error::Range)? };
        wait_ready(b)?;
        let index = if n == 1 { 17 } else { 18 };
        let got = b.emmc_read(index, arg, buf).map_err(|e| Error::Bus(index, e));
        if n > 1 {
            // stop even after an error; its status is not checked: a read up to the last sector may report out of
            // range there (ST's driver ignores it too)
            let _ = cmd(b, 12, 0, Resp::R1);
        }
        status(index, got?).map(drop)
    }
}

/// The card for the dev link: identified on first use (never at boot, so a bad driver cannot affect normal use),
/// forgotten after an error so the next use starts again from CMD0 and a controller reset.
#[derive(Default)]
pub struct Emmc {
    card: Option<Card>,
}

impl Emmc {
    /// Not identified yet.
    pub const fn new() -> Self {
        Emmc { card: None }
    }

    fn card(&mut self, b: &mut impl Board) -> Result<Card, Error> {
        let card = match self.card {
            Some(c) => c,
            None => Card::init(b)?,
        };
        self.card = Some(card);
        Ok(card)
    }

    /// The card's layout (identifies the card if needed).
    pub fn layout(&mut self, b: &mut impl Board) -> Result<Layout, Error> {
        self.card(b).map(|c| c.layout)
    }

    /// Reads sectors as [`Card::read`] (identifies the card if needed).
    pub fn read(&mut self, b: &mut impl Board, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        let r = self.card(b)?.read(b, lba, buf);
        if matches!(r, Err(Error::Bus(..) | Error::Busy | Error::Status(..))) {
            self.card = None;
        }
        r
    }
}

/// The card as a [`fat::Disk`]: the FAT volume starts at sector 0 (BOARD.md 15, no MBR). Read only, as the driver.
pub struct Volume<'a, B: Board>(pub &'a mut Emmc, pub &'a mut B);

impl<B: Board> fat::Disk for Volume<'_, B> {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), fat::Error> {
        self.0.read(self.1, lba, buf).map_err(|_| fat::Error::Io)
    }
}
