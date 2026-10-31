//! A simulated S510 for host tests. It implements [`Board`] with:
//! - a model of the ST7789: it checks the command bytes (power, sleep, pixel format, RAMCTRL, windows) and
//!   paints its framebuffer from RAMWR as the real panel would;
//! - a clock driven by the delays, so the RTC advances as the app waits;
//! - keys scripted as a function of time;
//! - a Nordic chip ([`Nordic`]) that answers the commands the way the real one did on the unit (BOARD.md 11);
//! - an eMMC ([`EmmcCard`]) that answers the commands as JEDEC says, takes writes below B only, and can lose power
//!   after any sector written (`cut_after`), in the middle of a multi-block write too.
//! - the sensor bus ([`Bus`]): the light sensor, the barometer and the accelerometer (BOARD.md 16);
//! - the touch controller ([`TouchChip`]), with a finger scripted as a function of time like the keys.
//!
//! Screenshots: [`Sim::save`] writes the screen as a BMP. [`Sim::record`] saves one frame after every step that
//! changed the screen (target/sim/<name>/NNNNN.bmp), which gives a continuous capture for debugging.

extern crate std;

use crate::panel::*;
use crate::ui::{H, W};
use crate::*;
use std::boxed::Box;
use std::format;
use std::path::PathBuf;
use std::vec;
use std::vec::Vec;

/// The app started on a simulator at 09:00:00 with nothing on the card.
pub fn started() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    let app = App::start(&mut sim, None);
    (sim, app)
}

type KeyScript = Box<dyn Fn(u64) -> u8>;
type FingerScript = Box<dyn Fn(u64) -> Option<(u16, u16)>>;

pub struct Sim {
    pub screen: Vec<u16>,
    window: (u32, u32, u32, u32),
    /// Every panel command, in order.
    pub cmds: Vec<u8>,
    pub powered: bool,
    pub asleep: bool,
    pub display_on: bool,
    colmod: u8,
    ramctrl: [u8; 2],
    pub backlight: bool,
    /// Every buzzer frequency set, in order (0: silence).
    pub tones: Vec<u32>,
    /// eMMC writes allowed ([`Board::emmc_writable`]): the unit's builds without EMMC_WRITE=1 say no.
    pub writable: bool,
    /// Backlight level, percent.
    pub light_level: u8,
    pub standby: bool,
    /// Simulated time since reset.
    pub ms: u64,
    start_s: u64,
    keys: KeyScript,
    record: Option<(PathBuf, u32)>,
    dirty: bool,
    pub nordic: Nordic,
    /// Times the RTC was set.
    pub rtc_sets: u32,
    /// RTC backup registers (they survive an app restart, as on the unit).
    pub backup: [u32; 32],
    /// The battery voltage the ADC sees, mV (None: the ADC did not come up), and USB power.
    pub battery_mv: Option<u16>,
    pub charger: bool,
    pub gnss: Gnss,
    /// Pixels written to the panel so far (the drawing cost: each one is 16 bits over SPI).
    pub pixels: u64,
    /// Panel windows written so far (each one also costs CASET, RASET and RAMWR: 11 command bytes).
    pub windows: u64,
    pub emmc: EmmcCard,
    pub bus: Bus,
    pub touch: TouchChip,
    /// The work an eMMC read costs, ns a byte (reading and parsing what was read: about 430 on the unit, 2.3 MB/s,
    /// `tools/dev.py bench scan`): it moves the clock ([`Sim::ms`]) and [`Board::awake_us`]. 0 (the default): free.
    pub work_ns_per_byte: u64,
    /// Work done so far, ns.
    pub awake_ns: u64,
    /// The cycle counter ([`Board::cycles`]): each read moves it on by [`Sim::cycles_step`], wrapping as CYCCNT does.
    pub cycles: u32,
    /// What each read of the cycle counter adds (0, the default: the counter stands still).
    pub cycles_step: u32,
    /// What [`Board::last_sleep`] says: ms, and the work in it, us. Default: [`POLL_MS`], none.
    pub sleep: (u32, u32),
}

/// The Sitronix touch controller (BOARD.md 17), as the protocol doc describes it: Init for 50 ms after its reset
/// is released, then Normal; INT low while a finger is down; the registers the driver reads.
pub struct TouchChip {
    /// When the reset was released (None: held in reset, or never released).
    run_since: Option<u64>,
    finger: FingerScript,
    /// A second finger, for a pinch.
    finger2: FingerScript,
    /// Point reads so far (the app reads only while a finger is down).
    pub reads: u32,
}

impl TouchChip {
    fn ready(&self, ms: u64) -> bool {
        self.run_since.is_some_and(|t| ms >= t + 50)
    }

    fn read(&mut self, ms: u64, reg: u8, buf: &mut [u8]) -> bool {
        let Some(t) = self.run_since else { return false }; // in reset: no acknowledge
        let mut regs = [0u8; 0x40];
        regs[0x00] = 0x05; // version
        regs[0x01] = if ms >= t + 50 { 0x00 } else { 0x01 }; // Normal, else Init
        regs[0x04..0x07].copy_from_slice(&[0x01, 0xf0, 0x40]); // 240 x 320
        regs[0x0c..0x10].copy_from_slice(&[0, 0, 1, 2]);
        regs[0x3f] = 2;
        if let Some((x, y)) = (self.finger)(ms).filter(|_| self.ready(ms)) {
            regs[0x12..0x15].copy_from_slice(&[0x80 | (x >> 4 & 0x70) as u8 | (y >> 8 & 7) as u8, x as u8, y as u8]);
        }
        if let Some((x, y)) = (self.finger2)(ms).filter(|_| self.ready(ms)) {
            regs[0x16..0x19].copy_from_slice(&[0x80 | (x >> 4 & 0x70) as u8 | (y >> 8 & 7) as u8, x as u8, y as u8]);
        }
        if reg == 0x12 {
            self.reads += 1;
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = regs.get(usize::from(reg) + i).copied().unwrap_or(0);
        }
        true
    }
}

/// SEC_COUNT of the simulated eMMC: about 31 GB, as the unit's.
pub const EMMC_SECTORS: u32 = 0x03a3_e000;

/// The eMMC as SDMMC1 sees it: the JEDEC states (idle 0, ready 1, ident 2, stby 3, tran 4, data 5), CMD1 busy for a
/// few polls, an EXT_CSD with [`EMMC_SECTORS`], sector `n` filled with [`emmc_sector`]. It starts as the bootloader
/// leaves it: selected, in the transfer state, on a 4-bit bus. Driver mistakes that a real card would punish with
/// garbage fail the test (identification at 25 MHz, any write or erase command); illegal commands are not answered.
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
    /// EXT_CSD WR_REL_PARAM and REL_WR_SEC_C (emmc.rs): the reliable write offered; none by default.
    pub rel_param: u8,
    pub rel_sectors: u8,
    /// CMD23's block count and reliable bit, for the CMD25 that follows.
    count: Option<(u32, bool)>,
    state: u32,
    busy_left: u32,
    card_wide: bool,
    host_wide: bool,
    host_fast: bool,
    powered: bool,
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
            rel_param: 0,
            rel_sectors: 0,
            count: None,
            state: 4,
            busy_left: 0,
            card_wide: true,
            host_wide: false,
            host_fast: false,
            powered: false,
        }
    }
}

/// Start held on a paused ride, then SAVE on the end-ride screen (selected when it opens: Start): the end and save.
pub(crate) fn save_ride(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Long(Key::Start));
    app.press(sim, Press::Short(Key::Start));
}

/// What the simulated eMMC holds in sector `lba` when it has no image: the sector number, then a pattern that depends on it.
pub fn emmc_sector(lba: u32) -> [u8; 512] {
    let mut s = [0u8; 512];
    for (i, b) in s.iter_mut().enumerate() {
        *b = (i as u8) ^ (lba as u8).wrapping_mul(7) ^ (lba >> 8) as u8;
    }
    s[..4].copy_from_slice(&lba.to_le_bytes());
    s
}

impl EmmcCard {
    /// Card status as R1 carries it: the state, ready for data.
    fn status(&self) -> u32 {
        self.state << 9 | 1 << 8
    }

    fn cmd(&mut self, index: u8, arg: u32) -> Result<[u32; 4], crate::emmc::BusError> {
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
        let before = self.status();
        let r1 = |s| Ok([s, 0, 0, 0]);
        match (index, self.state) {
            (0, _) => {
                (self.state, self.card_wide, self.busy_left) = (0, false, self.busy_polls);
                Ok([0; 4])
            }
            (1, 0 | 1) => {
                self.state = 1;
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
                r1(before)
            }
            (7, 3) if arg == 1 << 16 => {
                self.state = 4;
                r1(before)
            }
            (13, _) if arg == 1 << 16 && self.state >= 3 => r1(before),
            (16, 4) => {
                assert_eq!(arg, 512, "block length");
                r1(before)
            }
            (6, 4) => {
                self.card_wide = true;
                r1(before)
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
                r1(before)
            }
            (12, 5 | 6) => {
                self.state = 4;
                r1(before)
            }
            _ => Err(Timeout), // illegal in this state: no answer
        }
    }

    fn read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, crate::emmc::BusError> {
        use crate::emmc::BusError::{Crc, Timeout};
        assert!(buf.len().is_multiple_of(512) && !buf.is_empty(), "eMMC read of {} bytes", buf.len());
        let s = self.cmd_data(index, arg)?;
        let end = u64::from(arg) + (buf.len() / 512) as u64;
        if self.unreadable.iter().any(|b| u64::from(b.start) < end && u64::from(arg) < u64::from(b.end)) {
            return Err(Crc);
        }
        if self.host_wide != self.card_wide {
            return Err(Crc); // the bus widths differ: the data comes out as garbage
        }
        if index == 8 {
            buf.fill(0);
            buf[212..216].copy_from_slice(&EMMC_SECTORS.to_le_bytes());
            buf[183] = u8::from(self.card_wide);
            buf[192] = 8; // EXT_CSD_REV
            (buf[166], buf[222]) = (self.rel_param, self.rel_sectors);
            return Ok(s);
        }
        if arg.saturating_add((buf.len() / 512) as u32) > EMMC_SECTORS {
            return Err(Timeout); // out of range: no data
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
        if index == 18 {
            self.state = 5; // sending until CMD12
        }
        self.bytes_read += buf.len() as u64;
        Ok(s)
    }

    /// CMD24 / CMD25: the sectors land one by one, until the power cut if one is set.
    fn write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, crate::emmc::BusError> {
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
        if self.dead || self.state != 4 {
            return Err(Timeout);
        }
        let end = u64::from(arg) + (buf.len() / 512) as u64;
        assert!(end <= u64::from(EMMC_SECTORS - crate::emmc::RESERVED), "a write at or past B: {arg:#x}");
        if self.bad.iter().any(|b| u64::from(b.start) < end && arg < b.end) && self.bad_left != Some(0) {
            self.bad_left = self.bad_left.map(|n| n - 1);
            return Err(crate::emmc::BusError::Crc);
        }
        assert!(!self.image.is_empty(), "a write to the card without an image");
        let s = self.status();
        for (i, chunk) in buf.chunks_exact(512).enumerate() {
            let at = (arg as usize + i) * 512;
            if self.image.len() < at + 512 {
                self.image.resize(at + 512, 0);
            }
            if self.cut_after.is_some_and(|c| self.sectors_written >= c) {
                self.dead = true; // the power is gone: this sector and the rest never land
                if self.tear && !reliable {
                    // the sector being programmed is left half old, half new, the rest garbage (inferred model)
                    let s = &mut self.image[at..at + 512];
                    s[..128].copy_from_slice(&chunk[..128]);
                    s[256..].iter_mut().enumerate().for_each(|(k, b)| *b = (k as u8).wrapping_mul(97) ^ 0x5a);
                }
                return Err(Timeout);
            }
            self.image[at..at + 512].copy_from_slice(chunk);
            self.sectors_written += 1;
        }
        if index == 25 && count.is_none() {
            self.state = 6; // receiving until CMD12 (with CMD23 first, the count ends it)
        }
        Ok(s)
    }

    fn cmd_data(&mut self, index: u8, arg: u32) -> Result<u32, crate::emmc::BusError> {
        assert!(self.powered, "eMMC command {index} before the controller was powered");
        self.cmds.push((index, arg));
        assert!([8, 17, 18].contains(&index), "eMMC data command {index}");
        if self.dead || self.state != 4 {
            return Err(crate::emmc::BusError::Timeout);
        }
        Ok(self.status())
    }
}

/// The chips on the sensor bus, as their registers behave (BOARD.md 16).
pub struct Bus {
    /// The ambient light, tenths of a lux; None: no light sensor (it never acknowledges).
    pub lux10: Option<u32>,
    /// The light sensor's registers (written ones kept).
    light: [u8; 0x20],
    /// When the light sensor left standby (None: in standby).
    light_on_since: Option<u64>,
    /// Time the light sensor has spent measuring, ms (up to its last standby).
    pub light_on_ms: u64,
    /// Light readings taken.
    pub light_reads: u32,
    /// Transactions on the bus (reads and writes, any chip): each one the CPU polls through on the unit.
    pub transactions: u32,
    /// The barometer's pressure and temperature (hPa x 10, C x 100); None: no barometer.
    pub baro: Option<(u32, i16)>,
    /// The barometer follows this altitude (mm, a function of the time in ms) on the sea-level pressure `baro_p0`
    /// (1/4096 hPa), instead of `baro`'s pressure: a simulated climb, with `baro_noise` counts of 1/4096 hPa of
    /// pseudo-random noise on each reading.
    pub baro_alt: Option<fn(u64) -> i32>,
    /// See `baro_alt`.
    pub baro_p0: u32,
    /// See `baro_alt`.
    pub baro_noise: u32,
    /// When the last one-shot conversion is done (None: none asked since the last read).
    baro_done: Option<u64>,
    /// One-shot conversions asked.
    pub baro_conversions: u32,
    /// The accelerometer's CTRL_REG1 (0x77, running at 400 Hz, as stock leaves it).
    pub accel_ctrl1: u8,
}

impl Bus {
    fn read(&mut self, now: u64, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        use crate::ambient::*;
        self.transactions += 1;
        match addr {
            LIGHT_ADDR => {
                let Some(lux10) = self.lux10 else { return false };
                let ready = self.light_on_since.is_some_and(|t| now >= t + 100);
                let raw = lux10.min(0x3_ffff);
                let mut regs = self.light;
                regs[0x06] = LIGHT_ID;
                regs[0x07] = if ready { 0x08 } else { 0 };
                regs[0x0d..0x10].copy_from_slice(&raw.to_le_bytes()[..3]);
                if reg == 0x0d {
                    assert!(ready, "light read before its conversion");
                    assert_eq!((regs[0x04], regs[0x05]), (0x22, 0x02), "the lux scale assumes 18 bits, gain 6");
                    self.light_reads += 1;
                }
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = regs[(usize::from(reg) + i) % regs.len()];
                }
                true
            }
            BARO_ADDR => {
                let Some((p, t)) = self.baro else { return false };
                let ready = self.baro_done.is_some_and(|d| now >= d);
                let mut regs = [0u8; 0x30];
                regs[0x0f] = BARO_ID;
                regs[0x27] = if ready { 0x03 } else { 0 };
                let raw = match self.baro_alt {
                    Some(f) => {
                        let h = f64::from(f(now)) / 44_307_694.0;
                        let noise = (now.wrapping_mul(2_654_435_761) >> 7) as u32 % (2 * self.baro_noise + 1);
                        ((f64::from(self.baro_p0) * (1.0 - h).powf(1.0 / 0.190284)) as u32)
                            .saturating_add(noise)
                            .saturating_sub(self.baro_noise)
                    }
                    None => p * 4096 / 10,
                };
                regs[0x28..0x2b].copy_from_slice(&raw.to_le_bytes()[..3]);
                regs[0x2b..0x2d].copy_from_slice(&t.to_le_bytes());
                if reg == 0x28 {
                    self.baro_done = None;
                }
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = regs[(usize::from(reg) + i) % regs.len()];
                }
                true
            }
            ACCEL_ADDR => true,
            _ => false,
        }
    }

    fn write(&mut self, now: u64, addr: u8, reg: u8, data: &[u8]) -> bool {
        use crate::ambient::*;
        self.transactions += 1;
        match addr {
            LIGHT_ADDR if self.lux10.is_some() => {
                for (i, &v) in data.iter().enumerate() {
                    self.light[(usize::from(reg) + i) % 0x20] = v;
                }
                if reg == 0 {
                    match (data[0] & 0x02 != 0, self.light_on_since) {
                        (true, None) => self.light_on_since = Some(now),
                        (false, Some(t)) => {
                            self.light_on_ms += now - t;
                            self.light_on_since = None;
                        }
                        _ => {}
                    }
                }
                true
            }
            BARO_ADDR if self.baro.is_some() => {
                if reg == 0x11 && data[0] & 1 != 0 {
                    self.baro_done = Some(now + 20);
                    self.baro_conversions += 1;
                }
                true
            }
            ACCEL_ADDR => {
                if reg == 0x20 {
                    self.accel_ctrl1 = data[0];
                }
                true
            }
            _ => false,
        }
    }
}

/// Where the simulated fix is (degrees x 1e-7): in the sea near 0 N 0 E, nowhere real, as the synthetic volume's
/// rides and tiles (AGENTS.md, "Public repository").
pub const FIX_LAT: i32 = 5_123_456;
pub const FIX_LON: i32 = 4_234_567;

/// A simulated ride's position over time: ms to latitude, longitude (degrees x 1e-7) and heading (degrees x 1e-5).
pub type Route = fn(u64) -> (i32, i32, i32);

/// The GNSS receiver as seen over SPI: once powered, a NAV-PVT frame every second (with `fix`: 3D at that speed,
/// else no fix, as indoors on the unit); 0xff when it has nothing.
#[derive(Default)]
pub struct Gnss {
    pub powered: bool,
    /// Ground speed of a 3D fix in mm/s; None: no fix.
    pub fix: Option<i32>,
    /// Where a fix is, as a function of the simulated time in ms: latitude, longitude (degrees x 1e-7) and heading
    /// (degrees x 1e-5); None: [`FIX_LAT`], [`FIX_LON`], heading north.
    pub route: Option<Route>,
    /// Altitude of a fix in mm as a function of the simulated time in ms; None: 0.
    pub alt: Option<fn(u64) -> i32>,
    /// Every byte received, in order (0xff included).
    pub got: Vec<u8>,
    /// CFG-PM-OPERATEMODE as the last CFG-VALSET set it (0 FULL, 2 PSMCT; 0 at power-up, the receiver's default),
    /// and each change: (simulated ms, mode).
    pub pm_mode: u8,
    pub pm_log: Vec<(u64, u8)>,
    /// Each CFG-SIGNAL key a CFG-VALSET set (CFG-SIGNAL-GPS_ENA ... GLO_ENA), in order: (key, value).
    pub signals: Vec<(u32, u8)>,
    /// Each UBX-MGA-ANO received whole: (simulated ms, svId, gnssId, year since 2000, month, day).
    pub ano: Vec<(u64, u8, u8, u8, u8, u8)>,
    /// Horizontal accuracy of a fix, mm (0: 3 m).
    pub h_acc_mm: u32,
    /// Its own UBX framing of what it receives.
    rx: crate::gnss::Parser,
    /// UTC of the next report, seconds since 1970 (set by the board: its RTC minus 2 h).
    utc: i64,
    /// When its power line was last driven low (0: from reset, as the board does at boot).
    off_since: u64,
    out: std::collections::VecDeque<u8>,
    next_ms: u64,
}

impl Gnss {
    fn tick(&mut self, now: u64) {
        if !self.powered || now < self.next_ms {
            return;
        }
        // its own 1 Hz clock, not the reads' (a read every 200 ms would stretch it to 1.2 s); late by a second or
        // more (powered, or not read for a while): from now
        self.next_ms = if now >= self.next_ms + 1000 { now + 1000 } else { self.next_ms + 1000 };
        let mut p = [0u8; 92];
        p[4..12].copy_from_slice(&[0xe5, 0x07, 3, 7, 0, 0, 0, 0xf0]);
        if let Some(speed) = self.fix {
            let (y, mo, d, h, mi, sec) = crate::from_epoch_s(self.utc);
            p[4..6].copy_from_slice(&y.to_le_bytes());
            p[6..12].copy_from_slice(&[mo, d, h, mi, sec, 0x07]); // date and time valid
            (p[20], p[21], p[23]) = (3, 1, 9);
            let (lat, lon, heading) = self.route.map_or((FIX_LAT, FIX_LON, 0), |f| f(now));
            p[24..28].copy_from_slice(&lon.to_le_bytes());
            p[28..32].copy_from_slice(&lat.to_le_bytes());
            p[36..40].copy_from_slice(&self.alt.map_or(0, |f| f(now)).to_le_bytes());
            p[40..44].copy_from_slice(&(if self.h_acc_mm == 0 { 3000 } else { self.h_acc_mm }).to_le_bytes());
            p[60..64].copy_from_slice(&speed.to_le_bytes());
            p[64..68].copy_from_slice(&heading.to_le_bytes());
        }
        let body: Vec<u8> = [0x01, 0x07, 92, 0].iter().chain(&p).copied().collect();
        let (mut a, mut b) = (0u8, 0u8);
        for &x in &body {
            a = a.wrapping_add(x);
            b = b.wrapping_add(a);
        }
        self.out.extend([0xb5, 0x62]);
        self.out.extend(body);
        self.out.extend([a, b]);
    }

    /// A byte from the host: CFG-VALSET's keys taken (the RAM layer only, as this firmware sends; each key's value
    /// size from its bits 28..30, u-blox M10 interface description UBX-21035062, "Configuration interface").
    fn receive(&mut self, now: u64, byte: u8) {
        let Some((class, id, p)) = self.rx.push(byte) else { return };
        if (class, id) == (0x13, 0x20) {
            assert_eq!(p.len(), 76, "MGA-ANO is 76 bytes");
            self.ano.push((now, p[2], p[3], p[4], p[5], p[6]));
            return;
        }
        if (class, id) != (0x06, 0x8a) {
            return;
        }
        assert_eq!(p.get(..2), Some(&[0, 0x01][..]), "VALSET version 0, the RAM layer only");
        let mut at = 4;
        while let Some(k) = p.get(at..at + 4) {
            let key = u32::from_le_bytes([k[0], k[1], k[2], k[3]]);
            let size = match key >> 28 & 7 {
                1 | 2 => 1,
                3 => 2,
                4 => 4,
                _ => 8,
            };
            if (0x1031_0000..0x1031_0100).contains(&key) {
                self.signals.push((key, p[at + 4]));
            }
            if key == 0x20d0_0001 {
                let mode = p[at + 4];
                if mode != self.pm_mode || self.pm_log.is_empty() {
                    self.pm_log.push((now, mode));
                }
                self.pm_mode = mode;
            }
            at += 4 + size;
        }
    }
}

/// The Nordic chip as seen over its link: replies to the commands as the unit did, scans, and has ANT channels that
/// hear the [`AntSensor`]s in range.
#[derive(Default)]
pub struct Nordic {
    /// False: the chip never answers (not running).
    pub dead: bool,
    /// A BLE command the chip refuses (NRF_ERROR_NO_MEM), as one whose configuration it lacks would.
    pub refuse: Option<u8>,
    /// An ANT command the chip refuses on a channel (op, channel), with an error code.
    pub ant_refuse: Option<[u8; 2]>,
    /// Reset since power-up: before that, commands would go nowhere.
    pub reset: bool,
    /// Hung: no answer, no event, until the next reset (unlike `dead`, which a reset does not cure).
    pub hung: bool,
    /// Resets so far, and the simulated time (ms) of each.
    pub resets: Vec<u64>,
    /// Damage to the chip's stream: bytes dropped, added or flipped (xorshift state, per mille of the bytes).
    pub chaos: Option<(u32, u32)>,
    pub enabled: bool,
    /// Simulated time (ms) at which the running scan times out.
    scan_end: Option<u64>,
    /// ANT+ sensors in range.
    pub sensors: Vec<AntSensor>,
    pub channels: [Channel; 8],
    /// The FE-C trainer (device type 0x11) among `sensors` is this model: its pages instead of the sensor's `data`.
    pub trainer: SimTrainer,
    /// The power meter's (device type 0x0b) answers to calibration requests (page 0x01).
    pub meter: SimMeter,
    /// The chip refuses an acknowledged message (op 0xc8) with TRANSFER_IN_PROGRESS for 250 ms (a channel period)
    /// after one went out, as the real one does while the last is still queued (review of ecbe541); the count of
    /// those refusals, and when the chip is free again.
    pub ack_model: bool,
    pub ack_busy_until: u64,
    pub ack_refused: u32,
    /// The chip never answers an acknowledged message (the 0xc8 layout is inferred, not seen on the unit).
    pub ack_silent: bool,
    /// Acknowledged messages that fail on air: EVENT_TRANSFER_TX_FAILED (code 6) after each, instead of
    /// TX_COMPLETED, while this is non-zero (counted down); the trainer does not get them.
    pub ack_fail: u32,
    /// Acknowledged messages that fail on air and whose TX_FAILED never reaches the unit (read during another
    /// command's wait, or among bytes a resync dropped), counted down.
    pub ack_lost: u32,
    /// The next GATT client event of this id on this connection is lost (conn, id): an answer that never comes.
    pub drop_gattc: Option<(u16, u16)>,
    /// Every GATT client event of this id on this connection is lost (conn, id).
    pub mute_gattc: Option<(u16, u16)>,
    /// The network keys set (`06 cc [net] [key]`), by network.
    pub keys: [Option<[u8; 8]>; 2],
    /// The BLE peripheral side (ble_sim.rs), and the phone in range.
    pub server: ble_sim::Server,
    pub phone: Option<ble_sim::PhoneSim>,
    /// The FTMS trainer in range (ble_sim.rs), on connection 1 once the unit connects to it.
    pub ftms: Option<ble_sim::FtmsSim>,
    /// The `sd_ble_cfg_set` arguments received since the reset; `sd_ble_enable` after them answers NO_MEM (a chip
    /// whose RAM cannot hold them); connections asked for (`sd_ble_gap_connect`) and the parameter updates sent.
    pub cfg: Vec<Vec<u8>>,
    pub no_mem: bool,
    pub connects: u32,
    pub param_updates: Vec<Vec<u8>>,
    /// Every payload received, in order.
    pub got: Vec<Vec<u8>>,
    rx: crate::nrf::Rx,
    out: std::collections::VecDeque<u8>,
}

/// A simulated FE-C trainer (drivers/fec.rs): sends pages 16 and 25 (and 80, 81, 54 when asked), applies the control
/// pages it is sent (power follows the target in ERG, the speed follows the grade in SIM) and answers each with
/// page 71. Every control page received is kept in `got`.
#[derive(Default)]
pub struct SimTrainer {
    /// Every control page received, in order.
    pub got: Vec<[u8; 8]>,
    /// The data pages asked for (page 70), in order.
    pub asked: Vec<u8>,
    /// The page of the last control command (0: none yet, the trainer is on its own), its target power W, resistance
    /// 0.5 %, grade 0.01 %.
    pub mode: u8,
    pub target_w: u16,
    pub resistance: u8,
    pub grade_x100: i32,
    /// The power the rider makes when the trainer does not hold a target, W.
    pub rider_w: u16,
    /// A ceiling on the power it makes, W (0: none): a rider who cannot hold the target in ERG.
    pub cap_w: u16,
    /// The rider has stopped pedalling: no power, no cadence, whatever the target.
    pub stopped: bool,
    /// The cadence it reports while there is power, rpm (0: 90).
    pub rpm: u8,
    /// The status nibble of page 25 (1 and 2 calibration, 4 user configuration), and its target flags.
    pub status: u8,
    pub limits: u8,
    /// The user configuration page last received.
    pub user: Option<[u8; 8]>,
    /// The power and speed (mm/s) now.
    pub power: u16,
    pub speed_mm_s: u32,
    /// Distance, mm; pages sent.
    pub dist_mm: u64,
    /// Fuzzing: the pages are pseudo-random ones of the profile (xorshift state), not the model's.
    pub fuzz: Option<u32>,
    /// A calibration is scripted to fail (its response says nothing succeeded), or the trainer ignores the request.
    pub cal_fails: bool,
    pub cal_mute: bool,
    /// Where the calibration is: 0 none, 1 waiting for the speed, 2 coasting (ticks counted in `cal_coast`).
    pub cal: u8,
    /// The calibration requests received (byte 1), in order.
    pub cal_requests: Vec<u8>,
    /// The target speed of the calibration, mm/s (0: 8333, 30 km/h).
    pub cal_target_mm_s: u16,
    cal_coast: u8,
    ticks: u32,
    events: u8,
    acc: u16,
    answer: Option<[u8; 8]>,
    capabilities: bool,
}

impl SimTrainer {
    fn command(&mut self, d: [u8; 8]) {
        self.got.push(d);
        match d[0] {
            0x30 => (self.mode, self.resistance) = (0x30, d[7]),
            0x31 => (self.mode, self.target_w) = (0x31, u16::from_le_bytes([d[6], d[7]]) / 4),
            0x33 => {
                self.mode = 0x33;
                self.grade_x100 = i32::from(u16::from_le_bytes([d[5], d[6]])) - 20_000;
            }
            0x01 => {
                self.cal_requests.push(d[1]);
                if !self.cal_mute && d[1] & 0xc0 != 0 && self.cal == 0 {
                    (self.cal, self.cal_coast) = (1, 0);
                }
                return;
            }
            0x37 => {
                self.user = Some(d);
                self.status &= !4;
            }
            0x46 => {
                self.asked.push(d[6]);
                self.capabilities |= d[6] == 54;
                return;
            }
            _ => return,
        }
        self.answer = Some([71, d[0], 0xff, 0, 0xff, 0xff, d[6], d[7]]); // passed
    }

    /// The next page, a quarter second on.
    fn page(&mut self) -> [u8; 8] {
        self.ticks += 1;
        let t = self.ticks;
        if let Some(x) = &mut self.fuzz {
            let mut next = || {
                *x ^= *x << 13;
                *x ^= *x >> 17;
                *x ^= *x << 5;
                *x
            };
            let page = [16, 25, 25, 54, 71, 80, 81, 0, 255, 1, 2][(next() % 11) as usize];
            let mut d: [u8; 8] = core::array::from_fn(|_| (next() >> 8) as u8);
            d[0] = page;
            return d;
        }
        // calibrating, the brake is off: the rider's power, whatever the mode was
        self.power = if self.mode == 0x31 && self.cal == 0 { self.target_w } else { self.rider_w };
        if self.stopped {
            self.power = 0;
        }
        if self.cap_w != 0 {
            self.power = self.power.min(self.cap_w);
        }
        self.speed_mm_s = match self.mode {
            0x33 => (10_000 - self.grade_x100 * 8).clamp(1000, 20_000) as u32,
            0x30 if self.resistance > 0 => 8000 - u32::from(self.resistance) * 20,
            // free (the release): the rider's own speed for the power made
            0x30 => u32::from(self.power) * 42,
            _ => u32::from(self.power) * 42,
        };
        self.dist_mm += u64::from(self.speed_mm_s) / 4;
        if let Some(a) = self.answer.take() {
            return a;
        }
        if core::mem::take(&mut self.capabilities) {
            return [54, 255, 255, 255, 255, 0xe8, 0x03, 0x07];
        }
        if self.cal != 0 && t % 4 == 3 {
            let target = if self.cal_target_mm_s == 0 { 8333 } else { self.cal_target_mm_s };
            if self.cal == 1 && self.speed_mm_s >= u32::from(target) {
                self.cal = 2;
            }
            if self.cal == 2 {
                self.cal_coast += 1;
                if self.cal_coast > 6 {
                    // the end: the response; the trainer stops asking when it succeeded
                    self.cal = 0;
                    let ok = if self.cal_fails { 0 } else { 0x80 };
                    self.status &= if self.cal_fails { 0xff } else { !3 };
                    return [1, ok, 0, 90, 0xff, 0xff, 0xc4, 0x09]; // 2500 ms
                }
            }
            let speed = if self.cal == 1 { 1 } else { 2 };
            let [t0, t1] = target.to_le_bytes();
            return [2, 0x80, speed << 6 | 2 << 4, 90, t0, t1, 0xb8, 0x0b]; // spin-down time 3000 ms
        }
        match t % 64 {
            1 => return [80, 255, 255, 1, 0x0f, 0x00, 0x34, 0x12], // manufacturer 15
            2 => return [81, 255, 255, 1, 0x78, 0x56, 0x34, 0x12],
            _ => {}
        }
        if t.is_multiple_of(4) {
            let [s0, s1] = (self.speed_mm_s as u16).to_le_bytes();
            [16, 25, t as u8, (self.dist_mm / 1000) as u8, s0, s1, 0xff, 0x38]
        } else {
            self.events = self.events.wrapping_add(1);
            self.acc = self.acc.wrapping_add(self.power);
            let [a0, a1] = self.acc.to_le_bytes();
            let [p0, p1] = self.power.to_le_bytes();
            [
                25,
                self.events,
                if self.power > 0 {
                    [90, self.rpm][usize::from(self.rpm > 0)]
                } else if self.stopped {
                    0
                } else {
                    0xff
                },
                a0,
                a1,
                p0,
                p1 & 0xf | self.status << 4,
                0x30 | self.limits,
            ]
        }
    }
}

/// How the simulated power meter answers a manual zero request.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum MeterAnswer {
    /// Not at all.
    #[default]
    Silent,
    /// 0xAC with this offset.
    Zero(i16),
    /// 0xAF.
    Fail,
}

/// The power meter's calibration side: the pages it received, how it answers, its auto zero status byte (0xff: none),
/// and the answer being broadcast (page, broadcasts left).
#[derive(Default)]
pub struct SimMeter {
    pub got: Vec<[u8; 8]>,
    pub answer: MeterAnswer,
    pub auto: Option<u8>,
    pub reply: Option<([u8; 8], u8)>,
}

/// An ANT sensor in range: its device type and number, its 8 data bytes as a function of the time (ms), the
/// network it is on (0 ANT+, 1 Shimano's: heard only by a channel assigned to it once that network has a key), and
/// the pages it sends as acknowledged data (message 0x4f, as a Di2 sends its button page) rather than broadcast.
pub struct AntSensor {
    pub device_type: u8,
    pub id: u16,
    pub data: fn(u64) -> [u8; 8],
    pub network: u8,
    pub ack: &'static [u8],
}

/// An ANT channel: the network and ID it was set to, open or not, the sensor it found, its next broadcast, when it
/// opened and its low-priority search time-out (2.5 s units).
#[derive(Default, Clone, Copy)]
pub struct Channel {
    net: Option<u8>,
    device_type: u8,
    device: u16,
    pub open: bool,
    found: Option<usize>,
    next: u64,
    opened: u64,
    search_timeout: u8,
}

/// The advertisers the simulated chip hears: two devices, one of them named, one report repeated.
pub const ADVERTISERS: [(u8, i8, &[u8]); 3] = [
    (0xa1, -40, &[2, 0x01, 0x06, 5, 0x09, b'B', b'i', b'k', b'e']),
    (0xb2, -70, &[2, 0x01, 0x06, 5, 0xff, 0x4c, 0, 0x10, 6]),
    (0xa1, -42, &[]),
];

/// A BLE_GAP_EVT_ADV_REPORT payload as the chip encodes it (BOARD.md 11).
fn adv_report(addr: u8, rssi: i8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![2, 0x1d, 0, 0xff, 0xff, 0, addr, 1, 2, 3, 4, 5];
    p.extend([0; 7]);
    p.extend([rssi as u8, 0, data.len() as u8, 1]);
    p.extend(data);
    p
}

impl Nordic {
    /// An ANT event packet: `message` then the event code and the channel, 44 bytes.
    fn ant_event(&mut self, message: &[u8], event: u8, ch: u8) {
        let mut p = vec![8];
        p.extend(message);
        p.resize(42, 0);
        p.extend([event, ch]);
        self.queue(&p);
    }

    /// Broadcasts from the open channels that hear a sensor, every 250 ms.
    fn ant_tick(&mut self, now: u64) {
        for ch in 0..self.channels.len() {
            let c = self.channels[ch];
            if !c.open || now < c.next {
                continue;
            }
            self.channels[ch].next = c.next + 250;
            let keyed = |n: u8| self.keys.get(usize::from(n)).is_some_and(Option::is_some);
            let hit = |s: &AntSensor| {
                c.net == Some(s.network)
                    && keyed(s.network)
                    && s.device_type == c.device_type
                    && (c.device == 0 || c.device == s.id)
            };
            // a sensor taken out of range (removed) is lost: the channel searches again
            let found = c.found.filter(|&i| i < self.sensors.len());
            let Some(i) = found.or_else(|| self.sensors.iter().position(hit)) else {
                if c.search_timeout != 0xff && now >= c.opened + 2500 * u64::from(c.search_timeout) {
                    // nothing found: EVENT_RX_SEARCH_TIMEOUT, then the channel closes
                    self.channels[ch].open = false;
                    self.ant_event(&[1, 0x40, ch as u8, 1, 1], 1, ch as u8);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8);
                }
                continue;
            };
            self.channels[ch].found = Some(i);
            let mut data =
                if self.sensors[i].device_type == 0x11 { self.trainer.page() } else { (self.sensors[i].data)(now) };
            if let (0x0b, Some((page, n))) = (self.sensors[i].device_type, &mut self.meter.reply) {
                data = *page; // the calibration answer, a few broadcasts
                *n -= 1;
                if *n == 0 {
                    self.meter.reply = None;
                }
            }
            let id = if self.sensors[i].ack.contains(&data[0]) { 0x4f } else { 0x4e };
            self.ant_event(&[&[9, id, ch as u8][..], &data].concat(), 0x80, ch as u8); // EVENT_RX
        }
    }

    fn queue(&mut self, payload: &[u8]) {
        if self.dead || self.hung {
            return;
        }
        let frame = [&(payload.len() as u16).to_le_bytes()[..], payload].concat();
        let Some((x, rate)) = &mut self.chaos else {
            return self.out.extend(frame);
        };
        for byte in frame {
            *x ^= *x << 13;
            *x ^= *x >> 17;
            *x ^= *x << 5;
            if *x % 1000 >= *rate {
                self.out.push_back(byte);
                continue;
            }
            match (*x >> 12) % 3 {
                0 => {}                                                // lost
                1 => self.out.extend([byte, (*x >> 20) as u8]),        // a stray byte after it
                _ => self.out.push_back(byte ^ 1 << ((*x >> 20) % 8)), // a bit flipped
            }
        }
    }

    /// A stray byte in the stream, between frames.
    pub fn stray(&mut self, byte: u8) {
        self.out.push_back(byte);
    }

    fn take(&mut self, p: &[u8], now: u64) {
        self.got.push(p.to_vec());
        if p == [0, 0x32] {
            return; // sd_power_system_off: no response
        }
        if let [0, 0x8a, 1, _, _, _, _, _, t0, t1] = *p {
            self.queue(&[1, 0x8a, 0, 0, 0, 0]);
            for (addr, rssi, data) in ADVERTISERS {
                self.queue(&adv_report(addr, rssi, data));
            }
            let trainer = self.ftms.as_ref().filter(|t| t.connectable && !t.connected).map(|t| t.reports());
            for (addr, rssi, data) in trainer.into_iter().flatten() {
                self.queue(&adv_report(addr, rssi, &data));
            }
            self.scan_end = Some(now + 1000 * u64::from(u16::from_le_bytes([t0, t1])));
            return;
        }
        if let [6, op, ch, rest @ ..] = p {
            if self.ant_refuse == Some([*op, *ch]) {
                self.queue(&[1, *op, 0x15, 0, 0, 0]);
                return;
            }
            let ch = usize::from(*ch);
            match (*op, rest) {
                (0xcc, k) if k.len() == 8 => {
                    if let Some(slot) = self.keys.get_mut(ch) {
                        *slot = k.try_into().ok(); // the channel byte is the network number here
                    }
                }
                (0xc2, [_, net, _]) => self.channels[ch].net = Some(*net),
                (0xd3, [d0, d1, t, _]) => {
                    self.channels[ch].device = u16::from_le_bytes([*d0, *d1]);
                    self.channels[ch].device_type = *t;
                }
                (0xc4, _) => {
                    assert!(!self.channels[ch].open, "channel {ch} opened twice");
                    self.channels[ch] =
                        Channel { open: true, found: None, next: now, opened: now, ..self.channels[ch] };
                }
                (0xda, [t]) => self.channels[ch].search_timeout = *t,
                // sd_ant_acknowledge_message_tx: the size, then the 8 bytes (nrf::ant_ack)
                (0xc8, _) if self.ack_silent => return,
                (0xc8, [8, ..]) if self.ack_model && now < self.ack_busy_until => {
                    self.ack_refused += 1;
                    self.queue(&[1, 0xc8, 0x1f, 0x40, 0, 0]); // NRF_ERROR_... TRANSFER_IN_PROGRESS (0x401f)
                    return;
                }
                (0xc8, [8, d @ ..]) if d.len() == 8 && self.channels[ch].device_type == 0x0b => {
                    self.ack_busy_until = now + 250;
                    if self.ack_fail > 0 {
                        self.ack_fail -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        self.ant_event(&[1, 0x40, ch as u8, 1, 6], 6, ch as u8); // EVENT_TRANSFER_TX_FAILED
                        return;
                    }
                    let d: [u8; 8] = d.try_into().unwrap_or([0; 8]);
                    self.meter.got.push(d);
                    let st = self.meter.auto.unwrap_or(0xff);
                    self.meter.reply = match (d[0], d[1], self.meter.answer) {
                        (1, 0xaa, MeterAnswer::Zero(o)) => {
                            let [o0, o1] = o.to_le_bytes();
                            Some(([1, 0xac, st, 0xff, 0xff, 0xff, o0, o1], 3))
                        }
                        (1, 0xaa, MeterAnswer::Fail) => Some(([1, 0xaf, st, 0xff, 0xff, 0xff, 0, 0], 3)),
                        (1, 0xab, _) if self.meter.auto.is_some() => {
                            self.meter.auto = Some(d[2]);
                            Some(([1, 0x12, 2 | d[2], 0xff, 0xff, 0xff, 0xff, 0xff], 3))
                        }
                        _ => None,
                    };
                    self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 5], 5, ch as u8); // EVENT_TRANSFER_TX_COMPLETED
                    return;
                }
                (0xc8, [8, d @ ..]) if d.len() == 8 && self.channels[ch].device_type == 0x11 => {
                    self.ack_busy_until = now + 250;
                    if self.ack_fail > 0 {
                        self.ack_fail -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        self.ant_event(&[1, 0x40, ch as u8, 1, 6], 6, ch as u8); // EVENT_TRANSFER_TX_FAILED
                        return;
                    }
                    if self.ack_lost > 0 {
                        self.ack_lost -= 1;
                        self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                        return;
                    }
                    self.trainer.command(d.try_into().unwrap_or([0; 8]));
                    self.queue(&[1, 0xc8, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 5], 5, ch as u8); // EVENT_TRANSFER_TX_COMPLETED
                    return;
                }
                (0xc5, _) => {
                    self.channels[ch].open = false;
                    self.queue(&[1, 0xc5, 0, 0, 0, 0]);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8); // EVENT_CHANNEL_CLOSED
                    return;
                }
                (0xd4, []) => {
                    let c = self.channels[ch];
                    let id = c.found.map_or(c.device, |i| self.sensors[i].id);
                    let [d0, d1] = id.to_le_bytes();
                    self.queue(&[1, 0xd4, 0, 0, 0, 0, d0, d1, c.device_type, if id > 0 { 0xf1 } else { 0 }]);
                    return;
                }
                _ => {}
            }
            self.queue(&[1, *op, 0, 0, 0, 0]); // ANT commands answer as RESP (seen on the unit)
            return;
        }
        if self.take_ble(p, now) {
            return;
        }
        let reply: Vec<u8> = match p {
            [0, 0x60] if self.no_mem && !self.cfg.is_empty() => vec![1, 0x60, 4, 0, 0, 0], // NRF_ERROR_NO_MEM
            [0, 0x60] => {
                self.enabled = true;
                vec![1, 0x60, 0, 0, 0, 0]
            }
            [0, 0x65, 1] if !self.enabled => vec![1, 0x65, 0x01, 0x30, 0, 0], // BLE_ERROR_NOT_ENABLED
            [0, 0x65, 1] => vec![1, 0x65, 0, 0, 0, 0, 4, 1, 0, 0x32, 0],
            [0, op, ..] => vec![1, *op, 6, 0, 0, 0], // NRF_ERROR_NOT_SUPPORTED
            _ => return,
        };
        self.queue(&reply);
    }
}

fn to_bcd(v: u64) -> u32 {
    (((v / 10) << 4) | (v % 10)) as u32
}

impl Sim {
    /// This firmware's default settings in the backup registers, as after its first start: the next start does not
    /// import stock's ([`App::first_start`]).
    pub fn own_settings(&mut self) {
        self.backup[crate::WHEEL_BKP] = crate::Settings::from_word(0).to_word();
    }

    /// The ride pages of builds before 2026-10-24 (speed with heart rate and cadence; four fields; the map whole),
    /// which the tests of the map, the sensors and the menus were written against.
    pub fn classic_pages(&mut self) {
        use crate::ui::Field::*;
        let pages = [
            crate::PageCfg { layout: 3, fields: [Speed, Hr, Cadence, Time, Time, Time] },
            crate::PageCfg { layout: 4, fields: [Speed, Hr, Cadence, Power, Time, Time] },
            crate::PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
        ];
        for (regs, p) in crate::PAGE_BKP.iter().zip(pages) {
            for (&r, w) in regs.iter().zip(p.to_words()) {
                self.backup[r] = w;
            }
        }
    }

    /// A unit whose RTC reads `h:m:s` at reset, no key pressed.
    pub fn new(h: u64, m: u64, s: u64) -> Self {
        Sim {
            screen: vec![0x1234; (W * H) as usize], // panel RAM is garbage until cleared
            window: (0, W - 1, 0, H - 1),
            cmds: Vec::new(),
            powered: false,
            asleep: true,
            display_on: false,
            colmod: 0x06,
            ramctrl: [0x10, 0xf0],
            backlight: false,
            tones: Vec::new(),
            writable: true,
            light_level: 0,
            standby: false,
            ms: 0,
            start_s: h * 3600 + m * 60 + s,
            keys: Box::new(|_| 0),
            record: None,
            dirty: false,
            nordic: Nordic::default(),
            rtc_sets: 0,
            backup: core::array::from_fn(|i| if i == 18 { 5 << 28 | 1 << 22 } else { 0 }), // AUTO OFF off: tests idle long
            battery_mv: Some(3900),
            charger: false,
            gnss: Gnss::default(),
            pixels: 0,
            windows: 0,
            emmc: EmmcCard::default(),
            bus: Bus {
                lux10: Some(3000), // 300 lux: indoors
                light: [0; 0x20],
                light_on_since: None,
                light_on_ms: 0,
                light_reads: 0,
                transactions: 0,
                baro: Some((10_132, 2150)),
                baro_alt: None,
                baro_p0: crate::altitude::P_STD,
                baro_noise: 0,
                baro_done: None,
                baro_conversions: 0,
                accel_ctrl1: 0x77,
            },
            touch: TouchChip { run_since: None, finger: Box::new(|_| None), finger2: Box::new(|_| None), reads: 0 },
            work_ns_per_byte: 0,
            awake_ns: 0,
            cycles: 0,
            cycles_step: 0,
            sleep: (POLL_MS, 0),
        }
    }

    /// The finger on the touch panel as a function of the simulated time in ms (None: lifted).
    pub fn finger(mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) -> Self {
        self.touch.finger = Box::new(f);
        self
    }

    /// A second finger on the touch panel, as [`Sim::finger`].
    pub fn finger2(mut self, f: impl Fn(u64) -> Option<(u16, u16)> + 'static) -> Self {
        self.touch.finger2 = Box::new(f);
        self
    }

    /// Keys held as a function of the simulated time in ms: a mask of [`Key`] codes.
    pub fn keys(mut self, f: impl Fn(u64) -> u8 + 'static) -> Self {
        self.keys = Box::new(f);
        self
    }

    /// What a person would see: powered, awake, display on, backlight on.
    pub fn visible(&self) -> bool {
        self.powered && !self.asleep && self.display_on && self.backlight
    }

    /// Saves a frame after every step that changed the screen, into target/sim/`name`/.
    pub fn record(mut self, name: &str) -> Self {
        let dir = out_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        self.record = Some((dir, 0));
        self
    }

    /// Writes the screen to target/sim/`name`.bmp.
    pub fn save(&self, name: &str) {
        std::fs::create_dir_all(out_dir()).unwrap();
        std::fs::write(out_dir().join(format!("{name}.bmp")), bmp(&self.screen)).unwrap();
    }
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/sim")
}

/// RGB565 screen as a 24-bit BMP (bottom-up rows; 240 * 3 bytes is already a multiple of 4).
pub fn bmp(px: &[u16]) -> Vec<u8> {
    let mut out = b"BM".to_vec();
    for v in [54 + W * H * 3, 0, 54, 40, W, H] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[1, 0, 24, 0]); // planes, bits per pixel
    out.extend_from_slice(&[0; 24]);
    for y in (0..H).rev() {
        for x in 0..W {
            let p = px[(y * W + x) as usize] as u32;
            out.extend_from_slice(&[((p & 0x1f) << 3) as u8, (((p >> 5) & 0x3f) << 2) as u8, ((p >> 11) << 3) as u8]);
        }
    }
    out
}

fn span(p: &[u8], limit: u32) -> (u32, u32) {
    assert_eq!(p.len(), 4, "window needs 4 parameter bytes");
    let (a, b) = (u32::from(p[0]) << 8 | u32::from(p[1]), u32::from(p[2]) << 8 | u32::from(p[3]));
    assert!(a <= b && b < limit, "window {a}..{b} outside 0..{limit}");
    (a, b)
}

impl Board for Sim {
    fn panel_power_on(&mut self) {
        self.powered = true;
        self.ms += 150;
    }

    fn panel_cmd(&mut self, cmd: u8, params: &[u8]) {
        assert!(self.powered, "command 0x{cmd:02x} to an unpowered panel");
        self.cmds.push(cmd);
        match cmd {
            SWRESET => (self.asleep, self.display_on) = (true, false),
            SLPIN => self.asleep = true,
            SLPOUT => self.asleep = false,
            DISPON => self.display_on = true,
            DISPOFF => self.display_on = false,
            COLMOD => self.colmod = params[0],
            RAMCTRL => self.ramctrl = [params[0], params[1]],
            CASET => (self.window.0, self.window.1) = span(params, W),
            RASET => (self.window.2, self.window.3) = span(params, H),
            RAMWR => panic!("RAMWR goes through panel_ram_write"),
            _ => {}
        }
    }

    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16) {
        assert_eq!(self.ramctrl[0] & 0x03, 0, "RAMCTRL routes RAM writes to the RGB interface, not SPI");
        assert_eq!(self.colmod & 0x07, 0x05, "pixel format is not 16-bit");
        let (x0, x1, y0, y1) = self.window;
        assert_eq!(count, (x1 - x0 + 1) * (y1 - y0 + 1), "pixel count does not fill the window");
        self.pixels += u64::from(count);
        self.windows += 1;
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.screen[(y * W + x) as usize] = next();
            }
        }
        self.dirty = true;
    }

    fn beep(&mut self, hz: u32) {
        self.tones.push(hz);
    }

    fn backlight(&mut self, level: u8) {
        self.backlight = level > 0;
        self.light_level = level;
    }

    fn keys_down(&mut self) -> u8 {
        (self.keys)(self.ms)
    }

    fn rtc(&mut self) -> (u32, u32) {
        // 2026-10-23 at the start; the day rolls over at midnight
        let t = self.start_s + self.ms / 1000;
        let (y, mo, d) = crate::civil_from_days(crate::days_from_civil(2026, 10, 23) + (t / 86400) as i32);
        let t = t % 86400;
        let tr = to_bcd(t / 3600) << 16 | to_bcd(t / 60 % 60) << 8 | to_bcd(t % 60);
        (tr, to_bcd(u64::from(y) % 100) << 16 | to_bcd(u64::from(mo)) << 8 | to_bcd(u64::from(d)))
    }

    fn rtc_set(&mut self, tr: u32, dr: u32) {
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let days = i64::from(crate::days_from_civil(y, mo, d) - crate::days_from_civil(2026, 10, 23));
        let t = days * 86400 + i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s);
        assert!(t >= (self.ms / 1000) as i64, "the simulated clock starts on 2026-10-23");
        self.start_s = t as u64 - self.ms / 1000;
        self.rtc_sets += 1;
    }

    fn delay_ms(&mut self, ms: u32) {
        self.ms += u64::from(ms);
        if let Some((dir, n)) = &mut self.record
            && self.dirty
        {
            std::fs::write(dir.join(format!("{:05}.bmp", *n)), bmp(&self.screen)).unwrap();
            *n += 1;
            self.dirty = false;
        }
    }

    fn standby(&mut self) {
        self.standby = true;
    }

    fn nrf_reset(&mut self) {
        let sensors = std::mem::take(&mut self.nordic.sensors);
        let mut phone = self.nordic.phone.take();
        if let Some(p) = &mut phone {
            p.hang_up();
        }
        let mut ftms = self.nordic.ftms.take();
        if let Some(t) = &mut ftms {
            t.hang_up();
        }
        let no_mem = self.nordic.no_mem;
        let (dead, refuse, ant_refuse) = (self.nordic.dead, self.nordic.refuse, self.nordic.ant_refuse);
        let trainer = std::mem::take(&mut self.nordic.trainer);
        let meter = std::mem::take(&mut self.nordic.meter);
        let (ack_model, ack_silent) = (self.nordic.ack_model, self.nordic.ack_silent);
        let (mut resets, chaos) = (std::mem::take(&mut self.nordic.resets), self.nordic.chaos);
        resets.push(self.ms);
        self.nordic = Nordic {
            dead,
            refuse,
            ant_refuse,
            sensors,
            trainer,
            meter,
            ack_model,
            ack_silent,
            phone,
            ftms,
            no_mem,
            reset: true,
            resets,
            chaos,
            ..Nordic::default()
        };
        self.ms += 950;
    }

    fn nrf_send(&mut self, data: &[u8]) {
        let (n, now) = (&mut self.nordic, self.ms);
        assert!(n.reset, "bytes to the Nordic chip before its reset");
        if n.hung {
            return;
        }
        for &b in data {
            if let Some(p) = n.rx.push(b) {
                let p = p.to_vec();
                n.take(&p, now);
            }
        }
    }

    fn battery_adc(&mut self) -> Option<(u16, u16, u16)> {
        // VDDA 3.3 V: VREFINT reads its calibration value; the pin sees the battery through the divider by 1.5
        const CAL: u16 = 24_000;
        let mv = self.battery_mv?;
        Some(((u32::from(mv) * 2 * 65535).div_ceil(3 * 3300) as u16, CAL, CAL))
    }

    fn gnss_power(&mut self, on: bool) {
        if on && !self.gnss.powered {
            // as on the unit: switched on too soon after its line was driven low, the receiver does not start
            assert!(
                self.ms >= self.gnss.off_since + 500,
                "GNSS powered {} ms after off",
                self.ms - self.gnss.off_since
            );
            self.gnss.next_ms = self.ms + 500; // its first fix report
        }
        if !on && self.gnss.powered {
            self.gnss.off_since = self.ms;
            self.gnss.pm_mode = 0; // its RAM layer lost with the supply
        }
        self.gnss.powered = on;
    }

    fn gnss_transfer(&mut self, buf: &mut [u8]) {
        assert_eq!(buf.len() % 4, 0, "SPI transfers move 4 bytes at a time");
        let (tr, dr) = self.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let g = &mut self.gnss;
        g.utc = crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600; // the RTC runs on UTC + 2 h
        g.tick(self.ms);
        for b in buf.iter_mut() {
            g.got.push(*b);
            if g.powered {
                g.receive(self.ms, *b);
            }
            *b = if g.powered { g.out.pop_front().unwrap_or(0xff) } else { 0xff };
        }
    }

    fn charger_present(&mut self) -> bool {
        self.charger
    }

    fn backup_read(&mut self, n: usize) -> u32 {
        self.backup[n]
    }

    fn backup_write(&mut self, n: usize, v: u32) {
        self.backup[n] = v;
    }

    fn emmc_power_on(&mut self) {
        let e = &mut self.emmc;
        (e.powered, e.host_fast, e.host_wide) = (true, false, false);
        self.ms += 2;
    }

    fn emmc_bus(&mut self, fast: bool, wide: bool) {
        (self.emmc.host_fast, self.emmc.host_wide) = (fast, wide);
    }

    fn emmc_cmd(&mut self, index: u8, arg: u32, _: crate::emmc::Resp) -> Result<[u32; 4], crate::emmc::BusError> {
        self.emmc.cmd(index, arg)
    }

    fn emmc_read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, crate::emmc::BusError> {
        if self.work_ns_per_byte > 0 {
            let before = self.awake_ns / 1_000_000;
            self.awake_ns += buf.len() as u64 * self.work_ns_per_byte;
            self.ms += self.awake_ns / 1_000_000 - before; // the work takes time as well
        }
        self.emmc.read(index, arg, buf)
    }

    fn awake_us(&mut self) -> u32 {
        (self.awake_ns / 1000) as u32
    }

    fn cycles(&mut self) -> u32 {
        self.cycles = self.cycles.wrapping_add(self.cycles_step);
        self.cycles
    }

    fn last_sleep(&mut self) -> (u32, u32) {
        self.sleep
    }

    fn emmc_writable(&self) -> bool {
        self.writable
    }

    fn emmc_write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, crate::emmc::BusError> {
        self.emmc.write(index, arg, buf)
    }
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        self.bus.read(self.ms, addr, reg, buf)
    }

    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool {
        self.bus.write(self.ms, addr, reg, data)
    }

    fn touch_read(&mut self, reg: u8, buf: &mut [u8]) -> bool {
        self.touch.read(self.ms, reg, buf)
    }

    fn touch_reset(&mut self, run: bool) {
        self.touch.run_since = if run { self.touch.run_since.or(Some(self.ms)) } else { None };
    }

    fn touch_int(&mut self) -> bool {
        self.touch.ready(self.ms) && (self.touch.finger)(self.ms).is_some()
    }

    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize {
        if self.nordic.scan_end.is_some_and(|end| self.ms >= end) {
            self.nordic.scan_end = None;
            self.nordic.queue(&[2, 0x1b, 0, 0xff, 0xff, 1]); // BLE_GAP_EVT_TIMEOUT, source scan
        }
        self.nordic.ant_tick(self.ms);
        self.nordic.phone_tick(self.ms);
        self.nordic.ftms_tick(self.ms);
        let n = buf.len().min(self.nordic.out.len());
        for (slot, b) in buf.iter_mut().zip(self.nordic.out.drain(..n)) {
            *slot = b;
        }
        n
    }
}

/// Passes of the main loop for `ms`: a step, then the sleep (the test helper every sim test module shares).
fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

#[cfg(test)]
mod assist_tests;
#[cfg(test)]
mod course_tests;
#[cfg(test)]
mod curve_tests;
#[cfg(test)]
mod laps_tests;
#[cfg(test)]
mod map_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod perf_tests;
#[cfg(test)]
mod pmc_tests;
mod poi_tests;
#[cfg(test)]
mod ride_tests;
#[cfg(test)]
mod segment_tests;
#[cfg(test)]
mod start_cue_tests;
#[cfg(test)]
mod stock_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod workout_tests;

mod ble_sim;
pub use ble_sim::{Act, FtmsSim, PhoneSim};

#[cfg(test)]
mod phone_tests;

#[cfg(test)]
mod link_tests;

#[cfg(test)]
mod altitude_tests;

#[cfg(test)]
mod fecble_tests;
#[cfg(test)]
mod ftms_tests;
#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod power_tests;
#[cfg(test)]
mod powercal_tests;
#[cfg(test)]
mod ramp_tests;
#[cfg(test)]
mod shift_tests;
#[cfg(test)]
mod trainer_tests;

#[cfg(test)]
mod page_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod switch_tests;

#[cfg(test)]
mod settings_tests;

#[cfg(test)]
mod theme_tests;

/// The computer at the other end of the dev link: what it received, and the staging area.
#[derive(Default)]
pub struct Pc {
    pub got: Vec<u8>,
    pub staged: Vec<u8>,
    /// Registers poked, read back by peek (0 if never written).
    pub regs: std::collections::BTreeMap<u32, u32>,
}

impl crate::dev::Host for Pc {
    fn send(&mut self, data: &[u8]) {
        self.got.extend_from_slice(data);
    }
    fn stage(&mut self, offset: u32, data: &[u8]) {
        assert_eq!(offset as usize, self.staged.len(), "staged out of order");
        self.staged.extend_from_slice(data);
    }
    fn peek(&mut self, addr: u32) -> u32 {
        self.regs.get(&addr).copied().unwrap_or(0)
    }
    fn poke(&mut self, addr: u32, value: u32) {
        self.regs.insert(addr, value);
    }
}

#[cfg(test)]
mod dev_tests;

#[cfg(test)]
mod emmc_tests;

#[cfg(test)]
mod fuzz;

#[cfg(test)]
mod recorder_tests;

#[cfg(test)]
mod drive_tests;

#[cfg(test)]
mod diag_tests;
