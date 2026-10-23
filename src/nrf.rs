//! The Nordic BLE / ANT chip (BOARD.md 11): Nordic's nRF5 serialization over a byte link. The board only moves
//! bytes ([`Board::nrf_reset`], [`Board::nrf_send`], [`Board::nrf_recv`]); the framing and the commands are here.
//!
//! Framing: a 2-byte little-endian length, then the payload `[type][op][arguments]`. A response is
//! `[RESP][op][result u32][outputs]`. There is no resync: after a broken frame, reset the chip.

use crate::Board;

/// Packet type: a BLE command to the chip.
pub const CMD: u8 = 0;
/// Packet type: the chip's response to a command.
pub const RESP: u8 = 1;
/// Packet type: a BLE event from the chip.
pub const BLE_EVT: u8 = 2;
/// Packet type: an ANT event from the chip.
pub const ANT_EVT: u8 = 8;

/// `sd_ble_enable`: starts the BLE stack (no arguments here; the RAM base is not serialized).
pub const SD_BLE_ENABLE: u8 = 0x60;
/// `sd_ble_version_get`: the version fields (Bryton fills them with its own numbers, BOARD.md 11).
pub const SD_BLE_VERSION_GET: u8 = 0x65;
/// `sd_power_system_off`: the chip powers down until its next reset. No response.
pub const SD_POWER_SYSTEM_OFF: u8 = 0x32;
/// `sd_ble_gap_scan_start` (API v5: flags, interval, window, timeout).
pub const SD_BLE_GAP_SCAN_START: u8 = 0x8a;
/// `sd_ble_gap_scan_stop`.
pub const SD_BLE_GAP_SCAN_STOP: u8 = 0x8b;
/// BLE event id: an advertising report.
pub const EVT_ADV_REPORT: u16 = 0x1d;
/// BLE event id: a timeout (the scan's, here).
pub const EVT_TIMEOUT: u16 = 0x1b;

/// Largest payload kept; a longer one is read through and dropped.
pub const MAX: usize = 256;

/// How long a command may take before it is given up (stock waits 5 s; replies take a few ms).
pub const TIMEOUT_MS: u32 = 500;

/// Reassembles packets from the byte stream.
pub struct Rx {
    buf: [u8; MAX],
    header: [u8; 2],
    got: usize,
    want: Option<usize>,
}

impl Default for Rx {
    fn default() -> Self {
        Self::new()
    }
}

impl Rx {
    /// Waiting for a length header.
    pub const fn new() -> Self {
        Rx { buf: [0; MAX], header: [0; 2], got: 0, want: None }
    }

    /// Takes one byte; returns the payload when it completes a packet. A payload longer than [`MAX`] is read
    /// through (so the stream stays in step) and dropped.
    ///
    /// ```
    /// let mut rx = bryton_rustfw::nrf::Rx::new();
    /// let got: Vec<_> = [2, 0, 1, 0x60].iter().filter_map(|&b| rx.push(b).map(|p| p.to_vec())).collect();
    /// assert_eq!(got, [vec![1, 0x60]]);
    /// ```
    pub fn push(&mut self, byte: u8) -> Option<&[u8]> {
        let Some(want) = self.want else {
            self.header[self.got] = byte;
            self.got += 1;
            if self.got == 2 {
                self.got = 0;
                let n = usize::from(u16::from_le_bytes(self.header));
                if n > 0 {
                    self.want = Some(n);
                }
            }
            return None;
        };
        if let Some(slot) = self.buf.get_mut(self.got) {
            *slot = byte;
        }
        self.got += 1;
        if self.got < want {
            return None;
        }
        self.want = None;
        self.got = 0;
        self.buf.get(..want) // None (dropped) when it did not fit
    }
}

/// Why a command did not succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No response within [`TIMEOUT_MS`] of silence: the chip is not running, or the link is out of step.
    Timeout,
    /// The chip answered with this error code (NRF_ERROR_*, BLE_ERROR_*).
    Code(u32),
}

/// Sends one packet: the length header, then `payload`.
pub fn send(b: &mut impl Board, payload: &[u8]) {
    b.nrf_send(&(payload.len() as u16).to_le_bytes());
    b.nrf_send(payload);
}

/// Sends the command `payload` (`[CMD][op][arguments]`) and waits for its response; copies the outputs into `out`
/// and returns their length. Packets that arrive meanwhile (events) are dropped.
// ponytail: blocking, and events are dropped; an event queue when something listens for events (scanning).
pub fn call(b: &mut impl Board, rx: &mut Rx, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    send(b, payload);
    let op = payload.get(1).copied();
    // One byte at a time: what follows the response (events) must stay in the chip's stream for the caller.
    let mut buf = [0; 1];
    // Bounded both ways: TIMEOUT_MS of silence, or 64 KB of other packets (a scan's events) without the response.
    let (mut idle_ms, mut bytes) = (0, 0u32);
    while idle_ms < TIMEOUT_MS && bytes < 0x1_0000 {
        let n = b.nrf_recv(&mut buf);
        bytes += n as u32;
        for &byte in &buf[..n] {
            let Some(p) = rx.push(byte) else { continue };
            if p.len() < 6 || p[0] != RESP || Some(p[1]) != op {
                continue;
            }
            let result = u32::from_le_bytes([p[2], p[3], p[4], p[5]]);
            if result != 0 {
                return Err(Error::Code(result));
            }
            let outputs = &p[6..];
            let len = outputs.len().min(out.len());
            out[..len].copy_from_slice(&outputs[..len]);
            return Ok(len);
        }
        if n == 0 {
            b.delay_ms(1);
            idle_ms += 1;
        }
    }
    Err(Error::Timeout)
}

/// The version fields as Bryton fills them (stock logs "Image / Bootloader / SD").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    /// Nordic firmware image number (stock keeps it in RTC backup register 12).
    pub image: u8,
    /// Bootloader number.
    pub bootloader: u16,
    /// SoftDevice number.
    pub sd: u16,
}

/// Resets the chip as stock does, enables BLE and reads the version: proof that the chip runs and the link works.
pub fn start(b: &mut impl Board) -> Result<Version, Error> {
    b.nrf_reset();
    let mut rx = Rx::new();
    let mut out = [0; 8];
    call(b, &mut rx, &[CMD, SD_BLE_ENABLE], &mut out)?;
    let n = call(b, &mut rx, &[CMD, SD_BLE_VERSION_GET, 1], &mut out)?;
    match out[..n] {
        [image, b0, b1, s0, s1, ..] => {
            Ok(Version { image, bootloader: u16::from_le_bytes([b0, b1]), sd: u16::from_le_bytes([s0, s1]) })
        }
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// Powers the chip down until its next reset (stock's power-off; the next start's reset pulse wakes it).
pub fn system_off(b: &mut impl Board) {
    send(b, &[CMD, SD_POWER_SYSTEM_OFF]);
}

/// Starts an active scan for `seconds` (0: until stopped), with stock's low-power timing: a 30 ms window every
/// 60 ms. Reports then arrive as events ([`adv_report`]); a [`EVT_TIMEOUT`] event ends it.
pub fn scan_start(b: &mut impl Board, rx: &mut Rx, seconds: u16) -> Result<(), Error> {
    let [t0, t1] = seconds.to_le_bytes();
    // params present, flags = active, interval 0x60 and window 0x30 in 0.625 ms units, timeout in s
    call(b, rx, &[CMD, SD_BLE_GAP_SCAN_START, 1, 1, 0x60, 0, 0x30, 0, t0, t1], &mut []).map(|_| ())
}

/// The id of BLE event packet `p` (`[BLE_EVT][id u16][fields]`).
pub fn event_id(p: &[u8]) -> Option<u16> {
    match p {
        [BLE_EVT, a, b, ..] => Some(u16::from_le_bytes([*a, *b])),
        _ => None,
    }
}

/// One advertising report: who, how loud, and the advertised data.
pub struct AdvReport<'a> {
    /// Address, least significant byte first.
    pub addr: [u8; 6],
    /// Received signal strength, dBm.
    pub rssi: i8,
    /// Advertising data: `[len][type][len - 1 bytes]` structures.
    pub data: &'a [u8],
}

/// Decodes `p` if it is a BLE_GAP_EVT_ADV_REPORT (API v5 layout, BOARD.md 11): `[2][1d 00][conn u16][peer addr 7]
/// [direct addr 7][rssi][flags][dlen][present][data]`.
pub fn adv_report(p: &[u8]) -> Option<AdvReport<'_>> {
    if event_id(p)? != EVT_ADV_REPORT || p.len() < 23 {
        return None;
    }
    let mut addr = [0; 6];
    addr.copy_from_slice(&p[6..12]);
    let data = p.get(23..23 + usize::from(p[21]))?;
    Some(AdvReport { addr, rssi: p[19] as i8, data })
}

/// The advertised name in `data` (complete or shortened), if any.
///
/// ```
/// let data = [2, 0x01, 0x06, 5, 0x09, b'B', b'i', b'k', b'e'];
/// assert_eq!(bryton_rustfw::nrf::ad_name(&data), Some(&b"Bike"[..]));
/// ```
pub fn ad_name(data: &[u8]) -> Option<&[u8]> {
    let mut rest = data;
    while let [len, kind, ..] = *rest {
        let field = rest.get(2..1 + usize::from(len))?;
        if kind == 0x09 || kind == 0x08 {
            return Some(field);
        }
        rest = rest.get(1 + usize::from(len)..)?;
    }
    None
}

/// The distinct devices heard while scanning (up to [`Nearby::MAX`] remembered; the count stops there).
pub struct Nearby {
    addrs: [[u8; 6]; Nearby::MAX],
    count: usize,
}

impl Default for Nearby {
    fn default() -> Self {
        Self::new()
    }
}

impl Nearby {
    /// How many distinct addresses are remembered.
    pub const MAX: usize = 32;

    /// Nobody heard yet.
    pub const fn new() -> Self {
        Nearby { addrs: [[0; 6]; Self::MAX], count: 0 }
    }

    /// Notes `addr`; true if it is new.
    pub fn add(&mut self, addr: [u8; 6]) -> bool {
        if self.addrs[..self.count].contains(&addr) || self.count == Self::MAX {
            return false;
        }
        self.addrs[self.count] = addr;
        self.count += 1;
        true
    }

    /// Distinct devices heard.
    pub fn count(&self) -> usize {
        self.count
    }
}
