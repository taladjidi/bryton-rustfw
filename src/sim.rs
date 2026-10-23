//! A simulated S510 for host tests. It implements [`Board`] with:
//! - a model of the ST7789: it checks the command bytes (power, sleep, pixel format, RAMCTRL, windows) and
//!   paints its framebuffer from RAMWR as the real panel would;
//! - a clock driven by the delays, so the RTC advances as the app waits;
//! - keys scripted as a function of time;
//! - a Nordic chip ([`Nordic`]) that answers the commands the way the real one did on the unit (BOARD.md 11);
//! - an eMMC ([`EmmcCard`]) that answers the commands as JEDEC says, and fails the test on any write command.
//! - the sensor bus ([`Bus`]): the light sensor, the barometer and the accelerometer (BOARD.md 16).
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

type KeyScript = Box<dyn Fn(u64) -> u8>;

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
    /// RTC backup registers (they survive an app restart, as on the unit).
    pub backup: [u32; 32],
    /// The battery voltage the ADC sees, mV (None: the ADC did not come up), and USB power.
    pub battery_mv: Option<u16>,
    pub charger: bool,
    pub gnss: Gnss,
    /// Pixels written to the panel so far (the drawing cost: each one is 16 bits over SPI).
    pub pixels: u64,
    pub emmc: EmmcCard,
    pub bus: Bus,
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
            state: 4,
            busy_left: 0,
            card_wide: true,
            host_wide: false,
            host_fast: false,
            powered: false,
        }
    }
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
            ![20, 24, 25, 26, 27, 28, 29, 35, 36, 38, 42, 49].contains(&index),
            "write, erase, protect or lock command {index} to the eMMC"
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
            (12, 5) => {
                self.state = 4;
                r1(before)
            }
            _ => Err(Timeout), // illegal in this state: no answer
        }
    }

    fn read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, crate::emmc::BusError> {
        use crate::emmc::BusError::{Crc, Timeout};
        assert!(buf.len() % 512 == 0 && !buf.is_empty(), "eMMC read of {} bytes", buf.len());
        let s = self.cmd_data(index, arg)?;
        if self.host_wide != self.card_wide {
            return Err(Crc); // the bus widths differ: the data comes out as garbage
        }
        if index == 8 {
            buf.fill(0);
            buf[212..216].copy_from_slice(&EMMC_SECTORS.to_le_bytes());
            buf[183] = u8::from(self.card_wide);
            buf[192] = 8; // EXT_CSD_REV
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
    /// The barometer's pressure and temperature (hPa x 10, C x 100); None: no barometer.
    pub baro: Option<(u32, i16)>,
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
        match addr {
            LIGHT_ADDR => {
                let Some(lux10) = self.lux10 else { return false };
                let ready = self.light_on_since.is_some_and(|t| now >= t + 400);
                let raw = (lux10 * 4).min(0xf_ffff);
                let mut regs = self.light;
                regs[0x06] = LIGHT_ID;
                regs[0x07] = if ready { 0x08 } else { 0 };
                regs[0x0d..0x10].copy_from_slice(&raw.to_le_bytes()[..3]);
                if reg == 0x0d {
                    assert!(ready, "light read before its conversion");
                    assert_eq!((regs[0x04], regs[0x05]), (0x05, 0x02), "the lux scale assumes 20 bits, gain 6");
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
                regs[0x28..0x2b].copy_from_slice(&(p * 4096 / 10).to_le_bytes()[..3]);
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

/// Where the simulated fix is (degrees x 1e-7).
pub const FIX_LAT: i32 = 488_566_123;
pub const FIX_LON: i32 = -22_945_678;

/// The GNSS receiver as seen over SPI: once powered, a NAV-PVT frame every second (with `fix`: 3D at that speed,
/// else no fix, as indoors on the unit); 0xff when it has nothing.
#[derive(Default)]
pub struct Gnss {
    pub powered: bool,
    /// Ground speed of a 3D fix in mm/s; None: no fix.
    pub fix: Option<i32>,
    /// Where a fix is, as a function of the simulated time in ms: latitude, longitude (degrees x 1e-7) and heading
    /// (degrees x 1e-5); None: [`FIX_LAT`], [`FIX_LON`], heading north.
    pub route: Option<fn(u64) -> (i32, i32, i32)>,
    /// Altitude of a fix in mm as a function of the simulated time in ms; None: 0.
    pub alt: Option<fn(u64) -> i32>,
    /// Every byte received, in order (0xff included).
    pub got: Vec<u8>,
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
        self.next_ms = now + 1000;
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
}

/// The Nordic chip as seen over its link: replies to the commands as the unit did, scans, and has ANT channels that
/// hear the [`AntSensor`]s in range.
#[derive(Default)]
pub struct Nordic {
    /// False: the chip never answers (not running).
    pub dead: bool,
    /// Reset since power-up: before that, commands would go nowhere.
    pub reset: bool,
    pub enabled: bool,
    /// Simulated time (ms) at which the running scan times out.
    scan_end: Option<u64>,
    /// ANT+ sensors in range.
    pub sensors: Vec<AntSensor>,
    pub channels: [Channel; 8],
    /// Every payload received, in order.
    pub got: Vec<Vec<u8>>,
    rx: crate::nrf::Rx,
    out: std::collections::VecDeque<u8>,
}

/// An ANT+ sensor in range: its device type and number, and its 8 data bytes as a function of the time (ms).
pub struct AntSensor {
    pub device_type: u8,
    pub id: u16,
    pub data: fn(u64) -> [u8; 8],
}

/// An ANT channel: the ID it was set to, open or not, the sensor it found, its next broadcast, when it opened and
/// its low-priority search time-out (2.5 s units).
#[derive(Default, Clone, Copy)]
pub struct Channel {
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
            let hit = |s: &AntSensor| s.device_type == c.device_type && (c.device == 0 || c.device == s.id);
            let Some(i) = c.found.or_else(|| self.sensors.iter().position(hit)) else {
                if c.search_timeout != 0xff && now >= c.opened + 2500 * u64::from(c.search_timeout) {
                    // nothing found: EVENT_RX_SEARCH_TIMEOUT, then the channel closes
                    self.channels[ch].open = false;
                    self.ant_event(&[1, 0x40, ch as u8, 1, 1], 1, ch as u8);
                    self.ant_event(&[1, 0x40, ch as u8, 1, 7], 7, ch as u8);
                }
                continue;
            };
            self.channels[ch].found = Some(i);
            let data = (self.sensors[i].data)(now);
            self.ant_event(&[&[9, 0x4e, ch as u8][..], &data].concat(), 0x80, ch as u8); // EVENT_RX
        }
    }

    fn queue(&mut self, payload: &[u8]) {
        if !self.dead {
            self.out.extend((payload.len() as u16).to_le_bytes());
            self.out.extend(payload);
        }
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
            self.scan_end = Some(now + 1000 * u64::from(u16::from_le_bytes([t0, t1])));
            return;
        }
        if let [6, op, ch, rest @ ..] = p {
            let ch = usize::from(*ch);
            match (*op, rest) {
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
        let reply: Vec<u8> = match p {
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
    ((v / 10) << 4 | v % 10) as u32
}

impl Sim {
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
            light_level: 0,
            standby: false,
            ms: 0,
            start_s: h * 3600 + m * 60 + s,
            keys: Box::new(|_| 0),
            record: None,
            dirty: false,
            nordic: Nordic::default(),
            backup: [0; 32],
            battery_mv: Some(3900),
            charger: false,
            gnss: Gnss::default(),
            pixels: 0,
            emmc: EmmcCard::default(),
            bus: Bus {
                lux10: Some(3000), // 300 lux: indoors
                light: [0; 0x20],
                light_on_since: None,
                light_on_ms: 0,
                light_reads: 0,
                baro: Some((10_132, 2150)),
                baro_done: None,
                baro_conversions: 0,
                accel_ctrl1: 0x77,
            },
        }
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
        self.nordic = Nordic { dead: self.nordic.dead, sensors, reset: true, ..Nordic::default() };
        self.ms += 950;
    }

    fn nrf_send(&mut self, data: &[u8]) {
        let (n, now) = (&mut self.nordic, self.ms);
        assert!(n.reset, "bytes to the Nordic chip before its reset");
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
        self.emmc.read(index, arg, buf)
    }
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        self.bus.read(self.ms, addr, reg, buf)
    }

    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool {
        self.bus.write(self.ms, addr, reg, data)
    }

    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize {
        if self.nordic.scan_end.is_some_and(|end| self.ms >= end) {
            self.nordic.scan_end = None;
            self.nordic.queue(&[2, 0x1b, 0, 0xff, 0xff, 1]); // BLE_GAP_EVT_TIMEOUT, source scan
        }
        self.nordic.ant_tick(self.ms);
        let n = buf.len().min(self.nordic.out.len());
        for (slot, b) in buf.iter_mut().zip(self.nordic.out.drain(..n)) {
            *slot = b;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::Frame;
    use crate::ui::{Field, Rec, Strip, UsbDot};
    /// A stand-in for the ANT+ network key (the real one is never in the repository).
    const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    /// The owner's strap, as the unit paired it.
    const STRAP: SensorId = SensorId { device: 25126, trans: 0xf1 };

    fn hr72(_: u64) -> [u8; 8] {
        [4, 0, 0, 0, 0, 0, 0, 72]
    }
    fn hr80(_: u64) -> [u8; 8] {
        [4, 0, 0, 0, 0, 0, 0, 80]
    }
    /// Speed and cadence: every second one crank turn and three wheel turns.
    fn spd_cad(ms: u64) -> [u8; 8] {
        let (t, n) = (((ms / 1000) * 1024) as u16, (ms / 1000) as u16);
        let [t0, t1] = t.to_le_bytes();
        let ([c0, c1], [w0, w1]) = (n.to_le_bytes(), (3 * n).to_le_bytes());
        [t0, t1, c0, c1, t0, t1, w0, w1]
    }
    fn power300(_: u64) -> [u8; 8] {
        [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01]
    }

    fn run(sim: &mut Sim, app: &mut App, ms: u64) {
        let end = sim.ms + ms;
        while sim.ms < end && !sim.standby {
            app.step(sim);
            sim.delay_ms(POLL_MS);
        }
    }

    /// The screen drawn step by step on the panel equals the screen drawn at once from the state (the screenshot).
    fn assert_consistent(sim: &Sim, app: &App) {
        for y in 0..H {
            let (drawn, panel) = (app.render_row(y).px, &sim.screen[(y * W) as usize..((y + 1) * W) as usize]);
            let bad: Vec<_> = (0..W as usize).filter(|&x| drawn[x] != panel[x]).collect();
            assert!(
                bad.is_empty(),
                "row {y}, x {bad:?}: screenshot {:04x}, panel {:04x}",
                drawn[bad[0]],
                panel[bad[0]]
            );
        }
    }

    /// Ride page `n` of `app` as the drawing code means it, with these values, under a strip at the sim's time
    /// (`radio`: the Nordic chip answered).
    fn ride(sim: &mut Sim, app: &App, n: usize, values: &[&str], radio: bool) -> Vec<u16> {
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        let mut f = Frame::new();
        crate::ui::draw_strip(
            &mut f,
            &Strip {
                time: (h, m),
                radio,
                gps: app.strip.gps,
                blink: app.strip.blink,
                usb: UsbDot::Off,
                battery: Some(63),
                charging: false,
                rec: app.strip.rec,
            },
        );
        let page = app.pages()[n];
        for (i, v) in values.iter().enumerate() {
            let bx = crate::ui::field_box(page.layout, crate::ui::RIDE_H, i).unwrap();
            crate::ui::draw_field_frame(&mut f, bx, page.fields[i]);
            crate::ui::draw_field_value(&mut f, bx, page.fields[i], v, zone_fg(page.fields[i], v));
        }
        f.px
    }

    /// Heart rate and power in their zone's colour (default max HR and FTP), the rest white.
    fn zone_fg(f: Field, v: &str) -> u16 {
        match (f, v.parse::<u16>()) {
            (Field::Hr, Ok(v)) => crate::ride::hr_color(crate::ride::hr_zone(v, crate::ride::MAX_HR)),
            (Field::Power, Ok(v)) => crate::ride::power_color(crate::ride::power_zone(v, crate::ride::FTP)),
            _ => crate::ui::WHITE,
        }
    }

    fn hm(sim: &mut Sim) -> std::string::String {
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        format!("{h:02}:{m:02}")
    }

    /// The main screen as the drawing code means it, at the sim's time, under a strip with `usb`.
    fn main_screen(sim: &mut Sim, app: &App, usb: UsbDot) -> Vec<u16> {
        let (h, m, s) = rtc_time_from_tr(sim.rtc().0);
        let mut f = Frame::new();
        crate::ui::draw_strip(
            &mut f,
            &Strip {
                time: (h, m),
                radio: true,
                gps: app.strip.gps,
                blink: app.strip.blink,
                usb,
                battery: Some(63),
                charging: false,
                rec: app.strip.rec,
            },
        );
        crate::ui::draw_main_time(&mut f, [h / 10, h % 10, m / 10, m % 10, s / 10, s % 10]);
        crate::ui::draw_ferris(&mut f);
        f.px
    }

    /// The main screen: the strip, the time big with its seconds, Ferris.
    #[test]
    fn boots_into_the_main_screen() {
        let mut sim = Sim::new(12, 34, 56);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        app.set_usb(&mut sim, UsbDot::Ready);
        assert!(sim.visible());
        let pos = |c| sim.cmds.iter().position(|&x| x == c).unwrap();
        assert!(pos(SWRESET) < pos(SLPOUT) && pos(SLPOUT) < pos(DISPON));
        assert!(main_screen(&mut sim, &app, UsbDot::Ready) == sim.screen);
        assert_consistent(&sim, &app);
        sim.save("preview");
    }

    /// Stock's order: reset, sd_ble_enable, then the version; no ANT without a key, no scan before the list.
    #[test]
    fn starts_the_nordic_chip() {
        let mut sim = Sim::new(12, 0, 0);
        App::start(&mut sim, None);
        assert_eq!(sim.nordic.got, [vec![0, 0x60], vec![0, 0x65, 1]]);
    }

    /// A chip that never answers costs one timeout per command, then the pages run and the label is red.
    #[test]
    fn a_silent_nordic_chip_does_not_stop_the_pages() {
        let mut sim = Sim::new(12, 0, 0);
        sim.nordic.dead = true;
        let t0 = sim.ms;
        let mut app = App::start(&mut sim, Some(KEY));
        assert!(sim.ms - t0 < 3000, "start took {} ms", sim.ms - t0);
        assert_eq!(sim.nordic.got, [vec![0, 0x60]], "gave up after the first timeout");
        run(&mut sim, &mut app, 2000);
        app.press(&mut sim, Press::Short(Key::Page));
        assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], false) == sim.screen, "BLE in red");
    }

    #[test]
    fn ticks_through_midnight() {
        let mut sim = Sim::new(23, 59, 58);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 3000);
        assert_eq!(hm(&mut sim), "00:00");
        assert!(main_screen(&mut sim, &app, UsbDot::Off) == sim.screen);
    }

    /// Page goes from the main screen through the ride pages that are on (page 3 is the map by default), and back.
    #[test]
    fn page_turns_the_ride_pages() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(0));
        assert!(ride(&mut sim, &app, 0, &["--"; 3], true) == sim.screen, "full height, no Ferris");
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(1));
        assert_eq!(app.pages()[1].fields[..4], [Field::Speed, Field::Hr, Field::Cadence, Field::Power]);
        assert!(ride(&mut sim, &app, 1, &["--"; 4], true) == sim.screen);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(2), "the map");
        assert_consistent(&sim, &app);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Main);
        assert_consistent(&sim, &app);
    }

    /// Holding Page opens the menu; Page / Lap move, Start on NEARBY BLE opens the list and scans: two devices in
    /// three reports, strongest first, "2 NEARBY..." until the timeout. Back goes out level by level.
    #[test]
    fn menu_lists_the_devices_nearby() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Long(Key::Page));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
        app.press(&mut sim, Press::Short(Key::Lap));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, NEARBY_ROW), "Lap from the top wraps to NEARBY BLE");
        assert_consistent(&sim, &app);
        sim.save("menu");
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Nearby);
        assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
        run(&mut sim, &mut app, 1500);
        let names: Vec<_> = app.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
        assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
        assert!(app.scanning);
        assert_consistent(&sim, &app);
        sim.save("nearby");
        run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
        assert!(!app.scanning);
        assert_consistent(&sim, &app);
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, NEARBY_ROW));
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Main);
        assert_consistent(&sim, &app);
    }

    /// Back returns to where the menu was opened, row by row: a submenu to its row, the menu to the ride page.
    #[test]
    fn back_returns_where_it_came_from() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(1));
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..GPS_ROW {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, GPS_ROW), "the submenu's row");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Ride(1), "the page the menu was opened from");
        assert_consistent(&sim, &app);
        // switching that page off from the menu: Back shows the main screen instead
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..2 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // PAGE 2
        while app.pages()[1].layout != 0 {
            app.press(&mut sim, Press::Short(Key::Start)); // FIELDS round to OFF
        }
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Main);
        assert_consistent(&sim, &app);
    }

    /// A key held in a menu moves the selection on each repeat, and its release does not move it once more.
    #[test]
    fn held_key_scrolls_a_menu() {
        let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..5749).contains(&ms) { Key::Page as u8 } else { 0 });
        let mut app = App::start(&mut sim, None);
        assert!(sim.ms < 5000);
        app.press(&mut sim, Press::Long(Key::Page));
        let left = 7000 - sim.ms;
        run(&mut sim, &mut app, left);
        // held 750 ms: repeats at 500, 600, 700 ms, then the release
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 3));
        assert_consistent(&sim, &app);
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Menu(Menu::Main, 4), "the next press acts");
    }

    /// Sensors menu: Start on a sensor searches; the first one heard is paired (kept in its backup register) and
    /// its value reaches the ride page. With a key, ANT is started and every channel assigned, none opened yet.
    #[test]
    fn pairs_sensors_from_the_menu() {
        let mut sim = Sim::new(9, 0, 0);
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
        sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: spd_cad });
        let mut app = App::start(&mut sim, Some(KEY));
        let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
        assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2], "enable, key, assign 5 channels");
        assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
        app.press(&mut sim, Press::Short(Key::Start)); // heart rate: search
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // speed + cadence: search
        run(&mut sim, &mut app, 3000);
        assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "strap paired and kept");
        assert_eq!(sim.backup[Kind::SpeedCadence.backup()], SensorId { device: 555, trans: 0xf1 }.to_word());
        assert_consistent(&sim, &app);
        sim.save("sensors");
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 200);
        assert!(ride(&mut sim, &app, 0, &["22.7", "72", "60"], true) == sim.screen, "22.7 km/h, HR 72, 60 rpm");
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 200);
        assert!(ride(&mut sim, &app, 1, &["22.7", "72", "60", "--"], true) == sim.screen);
        sim.save("ride");
    }

    /// After a restart the paired sensor's channel opens for it only: a friend's strap is not taken. Start on it
    /// in the sensors menu forgets it (channel closed, register cleared).
    #[test]
    fn keeps_the_paired_sensor_until_forgotten() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 999, data: hr80 }); // a friend's strap
        let mut app = App::start(&mut sim, Some(KEY));
        assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
        run(&mut sim, &mut app, 2000);
        app.press(&mut sim, Press::Short(Key::Page));
        assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], true) == sim.screen, "the friend's strap is not shown");
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(sim.nordic.got.iter().filter(|p| p[..2] == [6, 0xc5]).count(), 1, "closed");
        assert_eq!(sim.backup[Kind::Hr.backup()], 0, "forgotten");
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 1000);
        assert_eq!(sim.backup[Kind::Hr.backup()], SensorId { device: 999, trans: 0xf1 }.to_word(), "paired anew");
    }

    /// A paired strap that is not sending: its channel searches 30 s, closes, rests a minute, searches again; the
    /// strap is found when it comes back.
    #[test]
    fn a_silent_sensor_searches_then_rests() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        let mut app = App::start(&mut sim, Some(KEY));
        let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
        assert_eq!(opens(&sim), 1);
        assert!(sim.nordic.got.contains(&vec![6, 0xda, 0, crate::nrf::SEARCH_TIMEOUT]));
        run(&mut sim, &mut app, 30_000 + 1000);
        assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
        run(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 2000);
        assert_eq!(opens(&sim), 1, "resting");
        run(&mut sim, &mut app, 2000);
        assert_eq!(opens(&sim), 2, "searching again");
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72 });
        run(&mut sim, &mut app, 1000);
        assert_eq!(app.live.get(Metric::Hr), Some(72), "found");
        run(&mut sim, &mut app, 60_000);
        assert!(sim.nordic.channels[0].open, "a sensor that sends keeps its channel");
    }

    /// The page menu: Start cycles the layout and the fields; kept in a backup register, read back at the next
    /// start. A power meter's value shows in the new field.
    #[test]
    fn pages_are_configured_from_the_menu() {
        let mut sim = Sim::new(9, 0, 0);
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power300 });
        let mut app = App::start(&mut sim, Some(KEY));
        app.press(&mut sim, Press::Long(Key::Page));
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 0));
        app.press(&mut sim, Press::Short(Key::Start)); // layout 3 -> 4
        app.press(&mut sim, Press::Short(Key::Page));
        for _ in 0..2 {
            app.press(&mut sim, Press::Short(Key::Start)); // field 1: speed -> cadence -> power
        }
        assert_consistent(&sim, &app);
        sim.save("page_menu");
        let want = PageCfg {
            layout: 4,
            fields: [Field::Power, Field::Hr, Field::Cadence, Field::Time, Field::Time, Field::Time],
        };
        assert_eq!(app.pages()[0], want);
        assert_eq!(PageCfg::from_word(sim.backup[14]), Some(want));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 1000);
        let t = hm(&mut sim);
        assert_consistent(&sim, &app);
        assert!(ride(&mut sim, &app, 0, &["300", "--", "85", &t], true) == sim.screen);
        let app = App::start(&mut sim, Some(KEY));
        assert_eq!(app.pages()[0], want, "read back after a restart");
    }

    /// The battery level in the strip (green while charging); an empty battery switches the unit off, but not
    /// while charging.
    #[test]
    fn battery_in_the_strip_and_off_when_empty() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        assert_eq!(app.strip.battery, Some(63), "3.9 V");
        sim.charger = true;
        sim.battery_mv = Some(3100);
        run(&mut sim, &mut app, 3 * u64::from(BATTERY_MS) + 1000);
        assert!(!sim.standby && app.strip.charging, "charging: stays on");
        sim.charger = false;
        run(&mut sim, &mut app, 2 * u64::from(BATTERY_MS));
        assert!(!sim.standby, "two low readings: not yet");
        run(&mut sim, &mut app, u64::from(BATTERY_MS));
        assert!(sim.standby && !sim.display_on, "empty: switched off");
    }

    /// The receiver is powered at start and gets its configuration (RAM layer) once; with a fix, "GPS" turns green
    /// and the speed comes from it when no speed sensor is paired; it is powered off with the unit.
    #[test]
    fn gps_fix_and_speed() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        assert!(!sim.gnss.powered, "powered later, after its line has been low long enough");
        run(&mut sim, &mut app, 3000);
        assert!(sim.gnss.powered);
        let sent = |sim: &Sim, f: [u8; 32]| sim.gnss.got.windows(32).any(|w| w == f);
        assert!(sent(&sim, crate::gnss::config()), "configuration");
        let frames = sim.gnss.got.iter().filter(|&&b| b == 0xb5).count();
        assert_eq!(frames, 2, "the RTC time (14 h accuracy, no offset learnt yet), no position yet");
        assert_eq!(app.strip.gps, Some(0), "searching");
        sim.gnss.fix = Some(7000); // 25.2 km/h
        run(&mut sim, &mut app, 2000);
        assert_eq!(app.strip.gps, Some(3), "9 satellites: 3 bars");
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 100);
        let (h, m, _) = rtc_time_from_tr(sim.rtc().0);
        let mut want = Frame::new();
        want.px = ride(&mut sim, &app, 0, &["25.2", "--", "--"], true);
        let strip = Strip {
            time: (h, m),
            radio: true,
            gps: Some(3),
            blink: true,
            usb: UsbDot::Off,
            battery: Some(63),
            charging: false,
            rec: Rec::Idle,
        };
        crate::ui::draw_strip(&mut want, &strip);
        assert!(want.px == sim.screen, "GPS speed, green GPS");
        sim.save("gps");
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..GPS_ROW {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
        run(&mut sim, &mut app, 1100);
        assert_consistent(&sim, &app);
        sim.save("gps_menu");
        app.press(&mut sim, Press::Short(Key::Back));
        app.press(&mut sim, Press::Short(Key::Back));
        // the fix taught the RTC's offset (+2 h) and the position (to 1e-4 degrees): kept for the next power-up
        let aid = crate::gnss::Aid::from_words([sim.backup[17], sim.backup[18]]);
        assert_eq!(aid, crate::gnss::Aid { offset_15min: Some(8), pos: Some((488_566_000, -22_945_000)) });
        sim.gnss.powered = false; // a power cycle: the board drives the line low at boot
        sim.gnss.off_since = sim.ms;
        let mut again = App::start(&mut sim, None);
        sim.gnss.got.clear();
        run(&mut sim, &mut again, 2500);
        let (tr, dr) = sim.rtc();
        let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), crate::rtc_date_from_dr(dr));
        let utc = crate::from_epoch_s(crate::epoch_s((y, mo, d, h, m, s)) - 2 * 3600);
        let sent_time = (0..3).any(|back| {
            sim.gnss
                .got
                .windows(32)
                .any(|w| w == crate::gnss::mga_time(crate::from_epoch_s(crate::epoch_s(utc) - back), 2))
        });
        assert!(sent_time, "UTC from the RTC at the next power-up");
        assert!(sent(&sim, crate::gnss::mga_pos(488_566_000, -22_945_000, 10_000)), "and the last position");
        sim.gnss.powered = false; // silent receiver: the fix ages out
        run(&mut sim, &mut app, u64::from(PVT_STALE_MS) + 200);
        assert_eq!(app.strip.gps, None);
        app.press(&mut sim, Press::Long(Key::Back));
        assert!(!sim.gnss.powered, "off with the unit");
    }

    /// The backlight: level and time-out from the menu (kept with the wheel size); after the time-out any key
    /// brings it back (and acts), Back only brings it back.
    #[test]
    fn backlight_level_and_timeout() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        assert_eq!(sim.light_level, 100, "100 % and never off by default");
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..5 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // LIGHT %: 60
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Start)); // LIGHT OFF: 10 s
        assert_eq!(sim.light_level, 60);
        assert_consistent(&sim, &app);
        sim.save("light_menu");
        app.press(&mut sim, Press::Short(Key::Back));
        run(&mut sim, &mut app, 11_000);
        assert_eq!(sim.light_level, 0, "timed out");
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!((sim.light_level, app.screen()), (60, Screen::Ride(0)), "a key brings it back and acts");
        run(&mut sim, &mut app, 11_000);
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(sim.light_level, 60, "Back after the time-out: on, not toggled off");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(sim.light_level, 0, "then Back toggles");
        let again = App::start(&mut sim, None);
        assert_eq!(sim.light_level, 60, "kept: {:x}", sim.backup[11]);
        drop(again);
    }

    /// The text of sensors menu row `i`: label and value.
    fn sensor_row(app: &App, i: usize) -> std::string::String {
        let (label, value, _) = app.menu_row(Menu::Sensors, i).unwrap();
        format!("{} {}", label.as_str(), value.as_str())
    }

    /// The text of main menu row `i`: label and value.
    fn main_row(app: &App, i: usize) -> std::string::String {
        let (label, value, _) = app.menu_row(Menu::Main, i).unwrap();
        format!("{} {}", label.as_str(), value.as_str())
    }

    /// AUTO: the backlight follows the ambient light, smoothed, without flicker on a band edge, and the light
    /// sensor measures only now and then, and not at all while the backlight is off.
    #[test]
    fn auto_backlight_follows_the_ambient_light() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        assert_eq!(sim.bus.accel_ctrl1, 0x07, "the accelerometer switched off");
        run(&mut sim, &mut app, 30_000);
        assert_eq!(sim.bus.light_reads, 0, "a fixed level and no menu: the light sensor is never used");
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..5 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        for _ in 0..4 {
            app.press(&mut sim, Press::Short(Key::Start)); // 60, 30, 10, AUTO
        }
        assert_eq!(main_row(&app, 5), "LIGHT % AUTO");
        assert_eq!(Settings::from_word(sim.backup[11]).light_level, LIGHT_AUTO, "kept");
        run(&mut sim, &mut app, 4000);
        assert_eq!(sensor_row(&app, 5), "LUX 300");
        assert_eq!(sensor_row(&app, 6), "TEMP C 21.5");
        assert_eq!(sensor_row(&app, 7), "HPA 1013");
        assert_consistent(&sim, &app);
        sim.save("sensor_rows");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(sim.light_level, 85, "300 lux: a bright room");

        sim.bus.lux10 = Some(5); // 0.5 lux: dark
        run(&mut sim, &mut app, 60_000);
        assert_eq!(sim.light_level, 14);
        sim.bus.lux10 = Some(300); // 30 lux
        run(&mut sim, &mut app, 40_000);
        assert_eq!(sim.light_level, 50);

        // a light hovering on the 50 lux edge: one level throughout
        let mut levels = std::collections::BTreeSet::new();
        for i in 0..40 {
            sim.bus.lux10 = Some(if i % 2 == 0 { 450 } else { 550 });
            run(&mut sim, &mut app, 3000);
            levels.insert(sim.light_level);
        }
        assert_eq!(levels.into_iter().collect::<Vec<_>>(), [50]);

        // power: one reading per LIGHT_MS, the sensor measuring a fifth of the time at most
        let (on0, reads0, t0) = (sim.bus.light_on_ms, sim.bus.light_reads, sim.ms);
        run(&mut sim, &mut app, 60_000);
        let (on, reads) = (sim.bus.light_on_ms - on0, sim.bus.light_reads - reads0);
        assert!((19..=21).contains(&reads), "{reads} readings in 60 s");
        assert!(on * 5 <= sim.ms - t0, "measuring {on} ms in 60 s");

        app.press(&mut sim, Press::Short(Key::Back)); // backlight off: no readings
        let reads = sim.bus.light_reads;
        run(&mut sim, &mut app, 30_000);
        assert_eq!(sim.bus.light_reads, reads);
        assert_eq!(sim.light_level, 0);
    }

    /// Without the chips the app runs as before: the rows say so, AUTO keeps a fixed level.
    #[test]
    fn missing_sensor_chips() {
        let mut sim = Sim::new(9, 0, 0);
        sim.bus.lux10 = None;
        sim.bus.baro = None;
        sim.backup[11] = 0xa5 << 24 | (LIGHT_AUTO as u32) << 16 | 2105;
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 10_000);
        assert_eq!(sim.light_level, LIGHT_LEVELS[1]);
        assert_eq!(sensor_row(&app, 5), "LUX NONE");
        assert_eq!(sensor_row(&app, 6), "TEMP C NONE");
    }

    /// The barometer converts once per reading (one-shot), every 10 s.
    #[test]
    fn barometer_one_shot() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 60_000);
        assert!((6..=8).contains(&sim.bus.baro_conversions), "{}", sim.bus.baro_conversions);
        assert_eq!(app.baro, Some((10_132, 2150)));
        sim.bus.baro = Some((9_500, -512));
        run(&mut sim, &mut app, 20_000);
        assert_eq!(sensor_row(&app, 6), "TEMP C -5.1");
        assert_eq!(sensor_row(&app, 7), "HPA 950");
    }

    #[test]
    fn back_short_press_toggles_the_backlight() {
        // Presses after the start (which takes about 1 s with the Nordic chip): at 3 s and at 4 s.
        let back = |ms| (3000..3200).contains(&ms) || (4000..4100).contains(&ms);
        let mut sim = Sim::new(8, 0, 0).keys(move |ms| if back(ms) { Key::Back as u8 } else { 0 });
        let mut app = App::start(&mut sim, None);
        let t0 = sim.ms;
        assert!(t0 < 3000);
        run(&mut sim, &mut app, 3500 - t0);
        assert!(!sim.backlight);
        run(&mut sim, &mut app, 1000);
        assert!(sim.backlight);
    }

    #[test]
    fn back_held_3s_powers_off_after_release() {
        let mut sim = Sim::new(8, 0, 0).keys(|ms| if (1000..6000).contains(&ms) { Key::Back as u8 } else { 0 });
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 10_000);
        assert!(sim.standby);
        assert!(sim.ms >= 6000, "standby while the key is still down would wake the unit at once");
        assert!(sim.asleep && !sim.display_on && !sim.backlight);
        assert_eq!(sim.nordic.got.last(), Some(&vec![0, 0x32]), "the radio chip switched off too");
        assert_eq!(sim.tones, [1500, 2500, 0, 2500, 1500, 0], "rising at power-on, falling at power-off");
    }

    /// A synthetic ride at 8 m/s (28.8 km/h): 240 s east with a gentle wiggle, then a loop of 600 m radius
    /// counterclockwise, starting heading east. Latitude, longitude (1e-7 degrees) and heading (1e-5 degrees).
    fn ride_route(ms: u64) -> (i32, i32, i32) {
        route_from(ms, (FIX_LAT, FIX_LON), 1.0)
    }

    /// The same ride turned half a circle (west, then a loop south), from `origin`: `sign` -1.
    fn route_from(ms: u64, (lat0, lon0): (i32, i32), sign: f64) -> (i32, i32, i32) {
        let at = |t: f64| {
            if t < 240.0 {
                (8.0 * t, 30.0 * (t * core::f64::consts::TAU / 60.0).sin())
            } else {
                let a = (t - 240.0) * 8.0 / 600.0;
                (1920.0 + 600.0 * a.sin(), 600.0 - 600.0 * a.cos())
            }
        };
        let t = ms as f64 / 1000.0;
        let ((x, y), (x1, y1)) = (at(t), at(t + 0.5));
        let (x, y, x1, y1) = (sign * x, sign * y, sign * x1, sign * y1);
        let heading = (x1 - x).atan2(y1 - y).to_degrees().rem_euclid(360.0);
        let m_per_unit = 0.011_132; // metres per 1e-7 degree of latitude, as map.rs
        let cos = (f64::from(lat0) * 1e-7).to_radians().cos();
        (lat0 + (y / m_per_unit) as i32, lon0 + (x / m_per_unit / cos) as i32, (heading * 1e5) as i32)
    }

    /// The ride west and south of 0 N 0 E, where the synthetic tiles are (tools/fat_image.py).
    fn tiles_route(ms: u64) -> (i32, i32, i32) {
        route_from(ms, crate::tiles::tests::ORIGIN, -1.0)
    }

    /// The basemap from the synthetic tiles (on the FAT volume macOS made, through the eMMC driver): drawn at
    /// the last known position before a fix, kept at the first fix, then under the ride. Between view changes the
    /// arrow and the track restore it in small boxes and the eMMC is not read; the screenshot equals the panel.
    #[test]
    fn basemap_under_the_map() {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some(crate::tiles::tests::ORIGIN) };
        [sim.backup[17], sim.backup[18]] = aid.to_words();
        sim.gnss.route = Some(tiles_route);
        let mut app = App::start(&mut sim, None);
        app.set_map_memory(crate::tiles::tests::memory());
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        run(&mut sim, &mut app, 500);
        let bg = |sim: &Sim| sim.screen.iter().filter(|&&p| p == crate::tiles::BACKGROUND).count();
        sim.save("basemap_last_known");
        assert!(bg(&sim) > 30_000, "the basemap: {}", bg(&sim));
        assert_consistent(&sim, &app);
        std::println!("basemap at the last known position: {:?}", app.basemap_stats());
        let reads = |sim: &Sim| sim.emmc.cmds.iter().filter(|c| c.0 == 17 || c.0 == 18).count();
        sim.gnss.fix = Some(8000);
        let view = app.map_view(2);
        run(&mut sim, &mut app, 3000);
        assert_eq!(app.map_view(2), view, "the first fix, inside the view: the same view, no new basemap");
        assert_consistent(&sim, &app);
        let t0 = sim.ms;
        let (mut refits, mut refit_px, mut small_px, mut small_reads, mut drawn) = (0, 0, 0, 0, 0);
        let mut seconds = 0;
        while sim.ms < t0 + 840_000 {
            let (view, px, r) = (app.map_view(0), sim.pixels, reads(&sim));
            app.step(&mut sim);
            sim.delay_ms(POLL_MS);
            if app.map_view(0) != view {
                refits += 1;
                refit_px += sim.pixels - px;
                drawn += u64::from(app.basemap_stats().px);
                std::println!("view change at {} s: {:?}", (sim.ms - t0) / 1000, app.basemap_stats());
            } else {
                small_px += sim.pixels - px;
                small_reads += reads(&sim) - r;
            }
            if (sim.ms - t0) / 1000 > seconds {
                seconds = (sim.ms - t0) / 1000;
                if seconds % 60 == 0 {
                    assert_consistent(&sim, &app);
                }
                if seconds == 120 {
                    sim.save("basemap_ride");
                }
            }
        }
        assert_consistent(&sim, &app);
        sim.save("basemap_loop");
        let s = (sim.ms - t0) / 1000;
        std::println!(
            "basemap ride: {refits} view changes ({} panel px, {} buffer px each), {} panel px/s between them",
            refit_px / refits.max(1),
            drawn / refits.max(1),
            small_px / s
        );
        assert_eq!(small_reads, 0, "the eMMC is read on view changes only");
        assert!(small_px / s < 2000, "{} px/s between view changes", small_px / s);
        assert!(bg(&sim) > 20_000, "the basemap under the loop");
    }

    /// The map page (page 3 by default) through a ride: "NO FIX" until the first fix, then the track and the arrow;
    /// fitted again a few times while the ride goes out, never once the loop is closed; between those, only small
    /// boxes are redrawn. The panel always equals the screenshot rendered row by row.
    /// Before any fix, the map shows the last known position (kept from an earlier power-up) as a grey dot instead
    /// of "NO FIX"; the first fix replaces it with the arrow.
    #[test]
    fn map_shows_the_last_known_position_until_a_fix() {
        let mut sim = Sim::new(9, 0, 0);
        let aid = crate::gnss::Aid { offset_15min: Some(8), pos: Some((FIX_LAT / 1000 * 1000, FIX_LON / 1000 * 1000)) };
        [sim.backup[17], sim.backup[18]] = aid.to_words();
        let mut app = App::start(&mut sim, None);
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        assert_eq!(app.screen(), Screen::Ride(2), "page 3: the map");
        run(&mut sim, &mut app, 500);
        // the middle of the map, where the dot or the arrow is (the strip and the scale bar are white too)
        let count = |sim: &Sim, c: u16| {
            let (cx, cy) = (W / 2, crate::ui::RIDE_Y + crate::ui::RIDE_H / 2);
            let mut n = 0;
            for y in cy - 20..cy + 20 {
                n += (cx - 20..cx + 20).filter(|&x| sim.screen[(y * W + x) as usize] == c).count();
            }
            n
        };
        sim.save("map_last_known");
        assert_eq!(count(&sim, crate::ui::GREY), 49, "the grey dot, 7 x 7");
        assert_eq!(count(&sim, crate::ui::WHITE), 0, "no arrow, no NO FIX text");
        assert_consistent(&sim, &app);
        sim.gnss.fix = Some(5000);
        run(&mut sim, &mut app, 3000);
        assert!(!app.track.stale() && count(&sim, crate::ui::WHITE) > 20, "the fix: the arrow");
        assert_consistent(&sim, &app);
    }

    #[test]
    fn map_page_follows_a_ride() {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        let mut app = App::start(&mut sim, None);
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        assert_eq!(app.screen(), Screen::Ride(2));
        run(&mut sim, &mut app, 3000);
        assert_consistent(&sim, &app);
        sim.save("map_nofix");
        sim.gnss.fix = Some(8000);
        let area = crate::map::area(crate::ui::field_box(1, crate::ui::RIDE_H, 0).unwrap());
        let t0 = sim.ms;
        let (mut refits, mut refit_px, mut small_px, mut late_refits) = (0, 0, 0, 0);
        let mut seconds = 0;
        while sim.ms < t0 + 840_000 {
            let (view, px) = (app.map_view(0), sim.pixels);
            app.step(&mut sim);
            sim.delay_ms(POLL_MS);
            let cost = sim.pixels - px;
            if app.map_view(0) != view {
                refits += 1;
                refit_px += cost;
                late_refits += u32::from(sim.ms > t0 + 720_000);
            } else {
                small_px += cost;
            }
            if (sim.ms - t0) / 1000 > seconds {
                seconds = (sim.ms - t0) / 1000;
                if seconds % 60 == 0 {
                    assert_consistent(&sim, &app);
                }
                match seconds {
                    30 => sim.save("map_start"),
                    240 => sim.save("map_out"),
                    _ => {}
                }
            }
        }
        assert_consistent(&sim, &app);
        sim.save("map_loop");
        let s = (sim.ms - t0) / 1000;
        std::println!(
            "map: {refits} full redraws ({} px each), {} px/s otherwise, {} px/s in all, over {s} s",
            refit_px / refits.max(1),
            small_px / s,
            (refit_px + small_px) / s
        );
        assert!((2..=12).contains(&refits), "fitted again as the ride grows: {refits}");
        assert_eq!(late_refits, 0, "the loop closed: the view stays");
        assert!(small_px / s < 2000, "{} px/s between full redraws", small_px / s);
        let v = app.map_view(0).unwrap();
        let (x, y) = v.px(area, app.track.pos().unwrap());
        assert_eq!(sim.screen[(y as u32 * W + x as u32) as usize], crate::ui::WHITE, "the arrow at the position");
        let track = sim.screen.iter().filter(|&&p| p == crate::ui::ACCENT).count();
        assert!(track > 500, "the track drawn: {track} px");
        // the map as one field of three, over speed and heart rate
        app.pages[0] =
            PageCfg { layout: 3, fields: [Field::Map, Field::Speed, Field::Hr, Field::Time, Field::Time, Field::Time] };
        app.press(&mut sim, Press::Short(Key::Page));
        app.press(&mut sim, Press::Short(Key::Page));
        assert_eq!(app.screen(), Screen::Ride(0));
        run(&mut sim, &mut app, 10_000);
        assert_consistent(&sim, &app);
        sim.save("map_field");
    }

    /// Heart rate rising from 90 to 209 over two minutes, then again: every zone at the default max of 190.
    fn hr_ramp(ms: u64) -> [u8; 8] {
        [4, 0, 0, 0, 0, 0, 0, 90 + (ms / 1000 % 120) as u8]
    }
    /// Power rising from 100 to 395 W over a minute, then again: every zone at the default FTP of 200 W.
    fn power_ramp(ms: u64) -> [u8; 8] {
        let [p0, p1] = (100 + (ms / 1000 % 60) as u16 * 5).to_le_bytes();
        [0x10, 1, 0xff, 85, 0, 0, p0, p1]
    }
    /// Climbing 0.2 m/s from 100 m.
    fn climb(ms: u64) -> i32 {
        100_000 + (ms / 5) as i32
    }

    /// The ride keys and fields: a ride on the GPS route (28.8 km/h) with a strap and a power meter. Start, a lap,
    /// a stop (auto-pause), Start to pause and resume, a long Start to end. The fields' values follow, heart rate and
    /// power in their zone colours, the graph's bars too; the strip's time is green while recording, orange when
    /// paused. Few pixels a second while riding.
    #[test]
    fn a_ride_with_laps_and_pauses() {
        let mut sim = Sim::new(9, 0, 0);
        sim.gnss.route = Some(ride_route);
        sim.gnss.alt = Some(climb);
        sim.gnss.fix = Some(8000);
        sim.backup[Kind::Hr.backup()] = STRAP.to_word();
        sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
        sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr_ramp });
        sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_ramp });
        let mut app = App::start(&mut sim, Some(KEY));
        use Field::*;
        app.pages[0] = PageCfg { layout: 6, fields: [Timer, Dist, AvgSpeed, AvgHr, AvgPower, Graph] };
        app.pages[1] = PageCfg { layout: 6, fields: [Hr, Power, MaxSpeed, LapTime, LapDist, Ascent] };
        app.press(&mut sim, Press::Short(Key::Page));
        run(&mut sim, &mut app, 5000);
        let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
        assert_eq!((text(&app, Timer), text(&app, Dist), app.strip.rec), ("0:00:00".into(), "0.00".into(), Rec::Idle));
        app.press(&mut sim, Press::Short(Key::Start));
        let (t0, mut px0) = (sim.ms, sim.pixels);
        let mut seconds = 0;
        while sim.ms < t0 + 720_000 {
            run(&mut sim, &mut app, 1000);
            seconds += 1;
            if seconds % 30 == 0 {
                assert_consistent(&sim, &app);
            }
            if seconds == 60 {
                let px_s = (sim.pixels - px0) / 60;
                std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), first minute: {px_s} px/s");
            }
            if seconds == 600 {
                px0 = sim.pixels; // the averages have settled
            }
        }
        // the averages of ramps settle as a long ride's do: from then on, mostly the timer, the distance, the graph
        let px_s = (sim.pixels - px0) / 120;
        std::println!("ride page (timer, km, avg km/h, avg HR, avg W, graph), minutes 10 to 12: {px_s} px/s");
        assert!(px_s < 1500, "{px_s} px/s while riding");
        assert_eq!(app.strip.rec, Rec::Running);
        let timer = app.ride.timer_s();
        assert!((719..=721).contains(&timer), "timer {timer}");
        assert_eq!(text(&app, Dist), "5.76", "8 m/s for 720 s");
        assert_eq!(text(&app, AvgSpeed), "28.8");
        let bx = |i| crate::ui::field_box(6, crate::ui::RIDE_H, i).unwrap();
        let colors_in = |sim: &Sim, (x, y, w, h): (u32, u32, u32, u32)| {
            let mut c: Vec<u16> = (y..y + h)
                .flat_map(|y| (x..x + w).map(move |x| (x, y)))
                .map(|(x, y)| sim.screen[(y * W + x) as usize])
                .collect();
            c.sort();
            c.dedup();
            c
        };
        let graph = colors_in(&sim, bx(5));
        let zones = crate::ride::ZONE_COLORS.iter().filter(|c| graph.contains(c)).count();
        assert!(zones >= 5, "the power bars in their zones' colours: {zones} colours");
        sim.save("ride_page");
        app.press(&mut sim, Press::Short(Key::Lap));
        app.press(&mut sim, Press::Short(Key::Page));
        let (t0, px0) = (sim.ms, sim.pixels);
        run(&mut sim, &mut app, 60_000);
        let px_s = (sim.pixels - px0) * 1000 / (sim.ms - t0);
        std::println!("ride page (HR, power changing every second, max km/h, lap time, lap km, ascent): {px_s} px/s");
        // the value colours: the zone of the heart rate and power on screen
        let (hr, w) = (app.live.get(Metric::Hr).unwrap(), app.live.get(Metric::Power).unwrap());
        let hr_fg = crate::ride::hr_color(crate::ride::hr_zone(hr, 190));
        let w_fg = crate::ride::power_color(crate::ride::power_zone(w, 200));
        assert!(colors_in(&sim, bx(0)).contains(&hr_fg) && colors_in(&sim, bx(1)).contains(&w_fg), "zone colours");
        assert_eq!((text(&app, LapTime), text(&app, LapDist)), ("0:01:00".into(), "0.48".into()));
        assert_eq!(text(&app, MaxSpeed), "28.8");
        let ascent = app.ride.ascent_m();
        assert!((150..=157).contains(&ascent), "0.2 m/s for 780 s: {ascent} m");
        assert_consistent(&sim, &app);
        sim.save("ride_page2");
        // stopped: auto-pause, the timer stops
        sim.gnss.fix = Some(0);
        run(&mut sim, &mut app, 3000);
        let timer = app.ride.timer_s();
        run(&mut sim, &mut app, 30_000);
        assert_eq!((app.ride.timer_s(), app.strip.rec), (timer, Rec::Paused), "auto-paused");
        assert_consistent(&sim, &app);
        sim.save("ride_autopause");
        sim.gnss.fix = Some(8000);
        run(&mut sim, &mut app, 10_000);
        assert!(app.ride.timer_s() >= timer + 8 && app.strip.rec == Rec::Running, "moving again");
        app.press(&mut sim, Press::Short(Key::Start));
        let (timer, dist) = (app.ride.timer_s(), app.ride.dist_m());
        run(&mut sim, &mut app, 10_000);
        assert_eq!((app.ride.timer_s(), app.ride.dist_m(), app.strip.rec), (timer, dist, Rec::Paused), "paused");
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 5000);
        assert!(app.ride.timer_s() > timer, "resumed");
        app.press(&mut sim, Press::Long(Key::Start));
        assert_eq!(app.ride.state(), crate::ride::State::Running, "a long Start does not end a running ride");
        app.press(&mut sim, Press::Short(Key::Start));
        app.press(&mut sim, Press::Long(Key::Start));
        run(&mut sim, &mut app, 2000);
        assert_eq!((app.ride.state(), app.strip.rec), (crate::ride::State::Idle, Rec::Idle), "ended");
        assert!(app.ride.dist_m() > 2000, "the totals stay on screen");
        assert_consistent(&sim, &app);
        sim.save("ride_ended");
    }

    /// Max HR and FTP from the menu: Start edits the value in place, Lap steps it up (1 bpm, 5 W), Page down,
    /// holding a key repeats, Start keeps it; kept in the wheel's register beside the other settings.
    #[test]
    fn max_hr_and_ftp_from_the_menu() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.press(&mut sim, Press::Long(Key::Page));
        for _ in 0..4 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // wheel: next size
        for _ in 0..3 {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        app.press(&mut sim, Press::Short(Key::Start)); // edit MAX HR
        for _ in 0..5 {
            app.press(&mut sim, Press::Short(Key::Lap)); // 190 -> 195
        }
        app.press(&mut sim, Press::Short(Key::Page)); // 194
        sim.save("max_hr_edit");
        app.press(&mut sim, Press::Short(Key::Start)); // kept
        app.press(&mut sim, Press::Short(Key::Page)); // FTP row
        app.press(&mut sim, Press::Short(Key::Start));
        for _ in 0..200 {
            app.press(&mut sim, Press::Repeat(Key::Page)); // held: 200 W down to the floor
        }
        app.press(&mut sim, Press::Short(Key::Page)); // the release does not step again
        app.press(&mut sim, Press::Short(Key::Lap)); // 55
        app.press(&mut sim, Press::Short(Key::Back)); // kept, still in the menu
        assert!(matches!(app.screen, Screen::Menu(Menu::Main, 8)));
        assert_consistent(&sim, &app);
        sim.save("zones_menu");
        let again = App::start(&mut sim, None);
        assert_eq!((again.settings.max_hr, again.settings.ftp, again.settings.wheel_mm), (194, 55, WHEELS[2]));
        assert_eq!(
            (again.settings.light_level, again.settings.light_timeout),
            (0, 0),
            "the backlight settings untouched"
        );
    }

    /// Continuous capture: 4 s with a Page press, one BMP per changed frame in target/sim/run/.
    #[test]
    fn record_a_run() {
        let mut sim = Sim::new(9, 59, 57).keys(|ms| if ms / 100 == 25 { Key::Page as u8 } else { 0 }).record("run");
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 4000);
        let frames = std::fs::read_dir(out_dir().join("run")).unwrap().count();
        assert!(frames >= 2, "the first page, then the second, got {frames}");
    }
}

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
mod dev_tests {
    use super::*;
    use crate::dev::{Effect, Session, Stats};

    fn app_at(h: u64, m: u64, s: u64) -> (Sim, App) {
        let mut sim = Sim::new(h, m, s);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        (sim, app)
    }

    /// A minimal image as pack.py makes it: SP, reset vector, tag, size word.
    fn image(size: u32) -> Vec<u8> {
        let mut img = vec![0u8; size as usize];
        img[0..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
        img[4..8].copy_from_slice(&0xd000_0101u32.to_le_bytes());
        img[0x94..0x98].copy_from_slice(b"R039");
        img[0x98..0x9c].copy_from_slice(&size.to_le_bytes());
        img
    }

    fn load(s: &mut Session, app: &App, pc: &mut Pc, img: &[u8], sum: u32) -> Effect {
        let cmd = format!("load {} {}\n", img.len(), sum);
        assert_eq!(s.feed(cmd.as_bytes(), app, Stats::default(), pc), Effect::None);
        let mut effect = Effect::None;
        for chunk in img.chunks(512) {
            effect = s.feed(chunk, app, Stats::default(), pc);
        }
        effect
    }

    #[test]
    fn peek_and_poke_registers_only() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        let mut s = Session::new();
        s.feed(b"poke 0x40013014 4098\npeek 0x40013010 2\n", &app, Stats::default(), &mut pc);
        assert_eq!(pc.got, b"ok\n00000000\n00001002\n");
        for bad in [
            "peek 0x10000000",
            "peek 0x2001fffc 2",
            "poke 0x20000000 1",
            "poke 0xd0000000 1",
            "peek 0x40013001",
            "peek 0x5ffffffc 2",
            "peek 0x40000000 65",
            "peek 0x",
            "poke 0x40013000",
        ] {
            pc.got.clear();
            s.feed(format!("{bad}\n").as_bytes(), &app, Stats::default(), &mut pc);
            assert!(pc.got.starts_with(b"err usage"), "{bad}");
        }
        assert_eq!(pc.regs.len(), 1);
    }

    #[test]
    fn info_answers() {
        let (_, app) = app_at(1, 2, 3);
        let mut pc = Pc::default();
        Session::new().feed(b"info\n", &app, Stats { uptime_ms: 1234, awake_permille: 0 }, &mut pc);
        assert_eq!(
            pc.got,
            format!("bryton-rustfw {} up 1234 ms, awake 0 permille\n", env!("CARGO_PKG_VERSION")).as_bytes()
        );
    }

    /// The screenshot (rendered row by row from the app's state) must equal what the panel shows.
    #[test]
    fn shot_equals_the_panel() {
        let (sim, app) = app_at(12, 34, 56);
        let mut pc = Pc::default();
        Session::new().feed(b"shot\n", &app, Stats::default(), &mut pc);
        let header = b"shot 240 320\n";
        assert_eq!(&pc.got[..header.len()], header);
        let px: Vec<u16> = pc.got[header.len()..].chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
        assert!(px == sim.screen);
    }

    #[test]
    fn load_runs_a_good_image() {
        let (_, app) = app_at(0, 0, 0);
        let (mut s, mut pc) = (Session::new(), Pc::default());
        let img = image(3000);
        let sum = img.iter().map(|&b| u32::from(b)).sum();
        assert_eq!(load(&mut s, &app, &mut pc, &img, sum), Effect::Run(3000));
        assert_eq!(pc.got, b"ready\nok\n");
        assert!(pc.staged == img);
    }

    #[test]
    fn load_refuses_bad_images_and_recovers() {
        let (_, app) = app_at(0, 0, 0);
        let (mut s, mut pc) = (Session::new(), Pc::default());
        let img = image(3000);
        assert_eq!(load(&mut s, &app, &mut pc, &img, 1), Effect::None);
        assert!(pc.got.ends_with(b"err checksum\n"));
        let mut bad = image(3000);
        bad[0x94] = b'X';
        let sum = bad.iter().map(|&b| u32::from(b)).sum();
        pc = Pc::default();
        assert_eq!(load(&mut s, &app, &mut pc, &bad, sum), Effect::None);
        assert!(pc.got.ends_with(b"err no R### tag at 0x94\n"));
        pc = Pc::default();
        assert_eq!(s.feed(b"load 99999999 0\nreset\n", &app, Stats::default(), &mut pc), Effect::Reset);
        assert!(pc.got.starts_with(b"err usage"));
        assert!(pc.got.ends_with(b"ok\n"));
    }
}

/// The eMMC driver (emmc.rs) against the card model, and the dev link's `sector` and `emmc` commands.
#[cfg(test)]
mod emmc_tests {
    use super::*;
    use crate::dev::{Effect, Session, Stats, send_layout, send_sector};
    use crate::emmc::{BusError, Card, Emmc, Error, Layout, SECTOR};

    /// The FAT32 volume macOS made (fat.rs tests), read through the driver and the card model: files equal the
    /// tiles, and the reads of a file are multi-block (CMD18) where whole sectors go to the buffer.
    #[test]
    fn reads_files_from_the_fat_volume() {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let mut emmc = Emmc::new();
        let mut d = crate::emmc::Volume(&mut emmc, &mut sim);
        let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
        let f = fat.open(&mut d, "Map/Data/Temp/Zoom14/P/8192_8192_0.bin").unwrap();
        let mut buf = vec![0u8; f.size as usize];
        assert_eq!(fat.read_at(&mut d, &f, 0, &mut buf), Ok(buf.len()));
        let h = fat.open(&mut d, "map/data/temp/zoom14/h/8192_8192_0.bin").unwrap();
        let mut n = [0u8; 4];
        fat.read_at(&mut d, &h, 0, &mut n).unwrap();
        assert_eq!(u32::from_le_bytes(n) * 10 + 4, h.size, "an H file: count, then 10 bytes a feature");
        // the same bytes as the image read directly
        let mut ram = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
        let mut direct = vec![0u8; f.size as usize];
        crate::fat::Fat::mount(&mut ram).unwrap().read_at(&mut ram, &f, 0, &mut direct).unwrap();
        assert_eq!(buf, direct);
        let frag = fat.open(&mut d, "frag.bin").unwrap(); // 200 KB: whole clusters
        let mut big = vec![0u8; frag.size as usize];
        assert_eq!(fat.read_at(&mut d, &frag, 0, &mut big), Ok(big.len()));
        assert!(sim.emmc.cmds.iter().any(|c| c.0 == 18), "multi-block reads");
        assert!(sim.emmc.cmds.iter().all(|c| [0, 1, 2, 3, 6, 7, 8, 12, 13, 16, 17, 18].contains(&c.0)));
    }

    /// Identification from the state the bootloader leaves (selected, 4-bit), without stock's SD probes, then the
    /// layout from SEC_COUNT.
    #[test]
    fn init_finds_the_layout() {
        let mut sim = Sim::new(9, 0, 0);
        let t0 = sim.ms;
        let card = Card::init(&mut sim).unwrap();
        assert!(sim.ms - t0 < 20, "init took {} ms", sim.ms - t0);
        assert_eq!(card.layout, Layout { sectors: EMMC_SECTORS, fat_end: 0x03a2_bfe6, os_start: 0x03a3_b000 });
        let order: Vec<u8> = sim.emmc.cmds.iter().map(|c| c.0).collect();
        assert_eq!(order, [0, 1, 1, 1, 1, 2, 3, 7, 13, 16, 8, 6, 13, 8]);
        assert!(sim.emmc.cmds.contains(&(1, 0x40ff_8000)) && sim.emmc.cmds.contains(&(3, 0x1_0000)));
        assert!(sim.emmc.host_wide && sim.emmc.card_wide && sim.emmc.host_fast);
    }

    #[test]
    fn reads_single_and_multiple_sectors() {
        let mut sim = Sim::new(9, 0, 0);
        let card = Card::init(&mut sim).unwrap();
        let l = card.layout;
        let mut one = [0u8; SECTOR];
        for lba in [0, 1, l.fat_end, l.os_start, EMMC_SECTORS - 1] {
            card.read(&mut sim, lba, &mut one).unwrap();
            assert_eq!(one, emmc_sector(lba), "sector {lba:#x}");
        }
        let mut eight = [0u8; 8 * SECTOR];
        sim.emmc.cmds.clear();
        card.read(&mut sim, EMMC_SECTORS - 8, &mut eight).unwrap(); // up to the last sector
        for (i, s) in eight.chunks_exact(SECTOR).enumerate() {
            assert_eq!(s, emmc_sector(EMMC_SECTORS - 8 + i as u32));
        }
        assert_eq!(sim.emmc.cmds, [(13, 0x1_0000), (18, EMMC_SECTORS - 8), (12, 0)]);
        // nothing past the end, nothing that is not whole sectors: refused before any command
        sim.emmc.cmds.clear();
        assert_eq!(card.read(&mut sim, EMMC_SECTORS - 7, &mut eight), Err(Error::Range));
        assert_eq!(card.read(&mut sim, u32::MAX, &mut one), Err(Error::Range));
        assert_eq!(card.read(&mut sim, 0, &mut one[..100]), Err(Error::Range));
        assert_eq!(card.read(&mut sim, 0, &mut []), Err(Error::Range));
        assert!(sim.emmc.cmds.is_empty());
        // the card still works after them
        card.read(&mut sim, 5, &mut one).unwrap();
        assert_eq!(one, emmc_sector(5));
    }

    /// A card that never answers fails at once; one that stays busy, after the 1 s JEDEC allows.
    #[test]
    fn a_silent_or_busy_card_costs_bounded_time() {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.dead = true;
        let t0 = sim.ms;
        assert_eq!(Card::init(&mut sim), Err(Error::Bus(1, BusError::Timeout)));
        assert!(sim.ms - t0 < 10);
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.busy_polls = u32::MAX;
        let t0 = sim.ms;
        assert_eq!(Card::init(&mut sim), Err(Error::Busy));
        assert!(sim.ms - t0 <= 1010, "{} ms", sim.ms - t0);
    }

    /// After a bus error the card is forgotten and identified again from CMD0 at the next read.
    #[test]
    fn identifies_again_after_an_error() {
        let mut sim = Sim::new(9, 0, 0);
        let (mut emmc, mut buf) = (Emmc::new(), [0u8; SECTOR]);
        emmc.read(&mut sim, 7, &mut buf).unwrap();
        sim.emmc.dead = true;
        assert_eq!(emmc.read(&mut sim, 7, &mut buf), Err(Error::Bus(13, BusError::Timeout)));
        sim.emmc.dead = false;
        sim.emmc.cmds.clear();
        emmc.read(&mut sim, 8, &mut buf).unwrap();
        assert_eq!((sim.emmc.cmds[0], buf), ((0, 0), emmc_sector(8)));
    }

    /// The eMMC is not touched at boot, only by the dev link; a dead card answers an error, the app runs on, and
    /// the card is identified again once it answers.
    #[test]
    fn sector_command_reads_and_survives_a_dead_card() {
        let mut sim = Sim::new(9, 0, 0);
        let mut app = App::start(&mut sim, None);
        app.step(&mut sim);
        assert!(sim.emmc.cmds.is_empty(), "the eMMC was touched at boot");
        let (mut session, mut pc, mut emmc) = (Session::new(), Pc::default(), Emmc::new());
        let mut serve = |sim: &mut Sim, session: &mut Session, pc: &mut Pc, line: &[u8]| {
            pc.got.clear();
            match session.feed(line, &app, Stats::default(), pc) {
                Effect::Sector(n) => {
                    let mut buf = [0u8; SECTOR];
                    let r = emmc.read(sim, n, &mut buf).map(|()| &buf);
                    send_sector(pc, r);
                }
                Effect::Emmc => send_layout(pc, emmc.layout(sim)),
                e => panic!("{e:?}"),
            }
        };
        sim.emmc.dead = true;
        serve(&mut sim, &mut session, &mut pc, b"sector 0\n");
        assert_eq!(pc.got, b"err cmd 1 timeout\n");
        sim.emmc.dead = false;
        serve(&mut sim, &mut session, &mut pc, b"emmc\n");
        assert_eq!(pc.got, b"emmc 03a3e000 03a2bfe6 03a3b000\n");
        serve(&mut sim, &mut session, &mut pc, b"sector 0x03a3b000\n");
        let hex: std::string::String = emmc_sector(0x03a3_b000).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(pc.got, format!("{hex}\n").as_bytes());
        serve(&mut sim, &mut session, &mut pc, b"sector 0x03a3e000\n");
        assert_eq!(pc.got, b"err range\n");
        pc.got.clear();
        assert_eq!(session.feed(b"sector\nsector 1 2\n", &app, Stats::default(), &mut pc), Effect::None);
        assert_eq!(pc.got, b"err usage: sector <n>\nerr usage: sector <n>\n");
        let t = sim.ms;
        app.step(&mut sim); // the app goes on
        sim.delay_ms(1000);
        app.step(&mut sim);
        assert!(sim.ms > t && sim.visible());
    }
}

/// Layer 4 of the testing strategy (AGENTS.md): random bytes through every parser and decoder that takes bytes
/// from outside (the Nordic chip, the GNSS receiver). A panic there would freeze the unit.
#[cfg(test)]
mod fuzz {
    extern crate std;
    /// xorshift32: deterministic, so a failure reproduces.
    fn bytes(seed: u32, n: usize) -> impl Iterator<Item = u8> {
        let mut x = seed;
        (0..n).map(move |_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            // lengths and markers more often than chance, so the parsers get past their headers
            match x % 16 {
                0 => 0xb5,
                1 => 0x62,
                2 => 44,
                3 => 0,
                _ => (x >> 8) as u8,
            }
        })
    }

    #[test]
    fn nordic_packets() {
        let mut rx = crate::nrf::Rx::new();
        for seed in 1..=4u32 {
            for b in bytes(seed, 1 << 20) {
                if let Some(p) = rx.push(b) {
                    let _ = crate::nrf::event_id(p);
                    let _ = crate::nrf::ant_broadcast(p);
                    let _ = crate::nrf::ant_closed(p);
                    if let Some(r) = crate::nrf::adv_report(p) {
                        let _ = (crate::nrf::ad_name(r.data), crate::nrf::ad_maker(r.data));
                    }
                }
            }
        }
    }

    /// Random bytes from the computer, with command words mixed in, into the dev link protocol.
    #[test]
    fn dev_link_input() {
        use crate::dev::{Session, Stats};
        let mut sim = super::Sim::new(9, 0, 0);
        let app = crate::App::start(&mut sim, None);
        let words: [&[u8]; 10] =
            [b"peek ", b"poke ", b"load ", b"key ", b"0x4", b"\n", b"shot\n", b"bat\n", b"sector ", b"emmc\n"];
        for seed in 1..=4u32 {
            let (mut session, mut pc) = (Session::new(), super::Pc::default());
            let mut buf = std::vec::Vec::new();
            for (k, b) in bytes(seed, 1 << 16).enumerate() {
                if b % 5 == 0 {
                    buf.extend_from_slice(words[usize::from(b) % words.len()]);
                } else {
                    buf.push(b);
                }
                if k % 64 == 63 {
                    let _ = session.feed(&buf, &app, Stats::default(), &mut pc);
                    buf.clear();
                    pc.got.clear();
                }
            }
        }
    }

    #[test]
    fn ubx_frames() {
        let mut parser = crate::gnss::Parser::new();
        for seed in 1..=4u32 {
            for b in bytes(seed, 1 << 20) {
                if let Some((c, i, p)) = parser.push(b) {
                    let _ = crate::gnss::pvt(c, i, p);
                }
            }
        }
    }
}
