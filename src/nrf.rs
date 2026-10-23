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
/// Packet type: an ANT command to the chip. Its response comes back as [`RESP`] (seen on the unit).
pub const ANT_CMD: u8 = 6;
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

// ---------------------------------------------------------------------------------------------------------------
// ANT+ heart rate (BOARD.md 11)
// ---------------------------------------------------------------------------------------------------------------

/// The ANT channel used for heart rate (stock's choice).
pub const HR_CHANNEL: u8 = 0;

/// Enables ANT and opens a heart-rate channel that pairs with the first strap it hears, with stock's settings:
/// `key` is the ANT+ network key (licensed: build.rs reads it from a private file, never from this repository).
pub fn ant_hr_open(b: &mut impl Board, rx: &mut Rx, key: [u8; 8]) -> Result<(), Error> {
    let [k0, k1, k2, k3, k4, k5, k6, k7] = key;
    let ch = HR_CHANNEL;
    let cmds: [&[u8]; 8] = [
        // enable: 15 channels, 0 encrypted, event queue 32, then stock's buffer pointer and size (sent as stock
        // sends them; the chip allocates its own, inferred)
        &[ANT_CMD, 0xfa, 1, 0x0f, 0, 0x20, 0, 0x7c, 0x16, 0x06, 0x24, 0xfc, 0x05],
        &[ANT_CMD, 0xcc, 0, k0, k1, k2, k3, k4, k5, k6, k7], // network 0 key
        &[ANT_CMD, 0xc2, ch, 0, 0, 0],                       // assign: slave, network 0, no extended
        &[ANT_CMD, 0xd3, ch, 0, 0, 0x78, 0],                 // channel id: any device number, type 0x78 (HR)
        // ponytail: searches until a strap answers (stock's 0xff), so the radio stays on; a timeout when it costs
        &[ANT_CMD, 0xda, ch, 0xff],       // low-priority search timeout
        &[ANT_CMD, 0xcd, ch, 0x39],       // 2457 MHz, ANT+
        &[ANT_CMD, 0xd1, ch, 0x86, 0x1f], // period 8070 / 32768 s (4.06 Hz)
        &[ANT_CMD, 0xc4, ch, 0, 0],       // open
    ];
    for cmd in cmds {
        call(b, rx, cmd, &mut [])?;
    }
    Ok(())
}

/// The heart rate (bpm) in ANT event `p`, if it is a broadcast on [`HR_CHANNEL`]: `[ANT_EVT][size][0x4e][channel]
/// [8 data bytes]...`, 44 bytes; ANT+ HR puts the computed heart rate in data byte 7. 0 means no reading.
///
/// ```
/// let mut p = [0u8; 44];
/// p[..12].copy_from_slice(&[8, 9, 0x4e, 0, 4, 0, 0, 0, 0, 0, 0, 72]);
/// assert_eq!(bryton_rustfw::nrf::ant_hr(&p), Some(72));
/// ```
pub fn ant_hr(p: &[u8]) -> Option<u8> {
    match p {
        [ANT_EVT, _, 0x4e, HR_CHANNEL, data @ ..] if p.len() == 44 => data.get(7).copied(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Devices nearby
// ---------------------------------------------------------------------------------------------------------------

/// One device heard while scanning.
#[derive(Clone, Copy)]
pub struct Device {
    /// Address, least significant byte first.
    pub addr: [u8; 6],
    /// Signal strength of its last report, dBm.
    pub rssi: i8,
    name: [u8; Device::NAME],
    name_len: u8,
    /// What kind of device it is, from its manufacturer data ([`ad_maker`]).
    pub maker: Option<&'static str>,
}

impl Device {
    /// Longest name kept (the screen shows fewer).
    pub const NAME: usize = 16;

    /// The advertised name, printable ASCII only (other bytes become '?'); empty if it never sent one.
    pub fn name(&self) -> &str {
        core::str::from_utf8(self.name.get(..usize::from(self.name_len)).unwrap_or(&[])).unwrap_or("")
    }
}

/// What kind of device sent `data`, from its manufacturer data (AD type 0xff: a Bluetooth SIG company ID, then
/// the company's own bytes). Phones and laptops advertise no name, only this and a random address that changes
/// every few minutes. Apple's first byte is its Continuity message type and Microsoft's beacon carries a device type
/// (both documented by public reverse engineering, e.g. furiousMAC/continuity; not from the stock firmware).
///
/// ```
/// use bryton_rustfw::nrf::ad_maker;
/// assert_eq!(ad_maker(&[2, 0x01, 0x1a, 5, 0xff, 0x4c, 0, 0x12, 2]), Some("Find My"));
/// assert_eq!(ad_maker(&[6, 0xff, 0x06, 0, 0x01, 0x09, 0x20]), Some("Windows PC"));
/// ```
// ponytail: a handful of companies and types; the full SIG list (thousands) is not worth the image size.
pub fn ad_maker(data: &[u8]) -> Option<&'static str> {
    let mut rest = data;
    while let [len, kind, ..] = *rest {
        let field = rest.get(2..1 + usize::from(len))?;
        if let (0xff, [a, b, own @ ..]) = (kind, field) {
            return Some(match (u16::from_le_bytes([*a, *b]), own) {
                (0x004c, [0x02, ..]) => "iBeacon",
                (0x004c, [0x07, ..]) => "AirPods",
                (0x004c, [0x09, ..]) => "AirPlay",
                (0x004c, [0x12, ..]) => "Find My", // an AirTag, or an Apple device's lost-mode beacon
                (0x004c, [0x05 | 0x0c | 0x0f | 0x10, ..]) => "iPhone/Mac", // AirDrop, Handoff, Nearby
                (0x004c, _) => "Apple",
                (0x0006, [0x01, t, ..]) if matches!(t & 0x1f, 9 | 15) => "Windows PC",
                (0x0006, [0x01, t, ..]) if t & 0x1f == 1 => "Xbox",
                (0x0006, _) => "Microsoft",
                (0x0059, _) => "Nordic",
                (0x006b, _) => "Polar",
                (0x0075, _) => "Samsung",
                (0x0087, _) => "Garmin",
                (0x00e0, _) => "Google",
                (0x0171, _) => "Amazon",
                _ => return None,
            });
        }
        rest = rest.get(1 + usize::from(len)..)?;
    }
    None
}

/// The distinct devices heard while scanning (up to [`Nearby::MAX`] remembered; the count stops there).
pub struct Nearby {
    devices: [Device; Nearby::MAX],
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
        const NONE: Device = Device { addr: [0; 6], rssi: 0, name: [0; Device::NAME], name_len: 0, maker: None };
        Nearby { devices: [NONE; Self::MAX], count: 0 }
    }

    /// Notes a report: its signal strength, and its name if it carries one. True if the device is new.
    pub fn add(&mut self, r: &AdvReport) -> bool {
        let known = self.devices[..self.count].iter().position(|d| d.addr == r.addr);
        let new = known.is_none() && self.count < Self::MAX;
        let Some(d) = (match known {
            Some(i) => self.devices.get_mut(i),
            None if new => {
                self.count += 1;
                self.devices.get_mut(self.count - 1)
            }
            None => None,
        }) else {
            return false;
        };
        d.addr = r.addr;
        d.rssi = r.rssi;
        if let Some(m) = ad_maker(r.data) {
            d.maker = Some(m);
        }
        if let Some(name) = ad_name(r.data) {
            let n = name.len().min(Device::NAME);
            for (slot, &c) in d.name.iter_mut().zip(&name[..n]) {
                *slot = if c.is_ascii_graphic() || c == b' ' { c } else { b'?' };
            }
            d.name_len = n as u8;
        }
        new
    }

    /// Distinct devices heard.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The devices, strongest signal first (a sorted copy: 32 x 24 bytes of stack).
    pub fn strongest(&self) -> impl Iterator<Item = Device> {
        let mut d = self.devices;
        d[..self.count].sort_unstable_by_key(|d| core::cmp::Reverse(d.rssi));
        d.into_iter().take(self.count)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn report(addr: u8, rssi: i8, data: &[u8]) -> AdvReport<'_> {
        AdvReport { addr: [addr, 0, 0, 0, 0, 0], rssi, data }
    }

    #[test]
    fn nearby_keeps_names_and_the_last_rssi_strongest_first() {
        let mut n = Nearby::new();
        assert!(n.add(&report(1, -80, &[5, 0xff, 0x4c, 0, 0x10, 6])));
        assert_eq!(n.strongest().next().unwrap().maker, Some("iPhone/Mac"));
        assert!(n.add(&report(2, -50, &[4, 0x09, b'H', b'R', 0xff])));
        assert!(!n.add(&report(1, -40, &[3, 0x08, b'P', b'C'])), "same address: not new");
        let list: std::vec::Vec<_> =
            n.strongest().map(|d| (d.addr[0], d.rssi, std::string::String::from(d.name()))).collect();
        assert_eq!(list, [(1, -40, "PC".into()), (2, -50, "HR?".into())]);
        for i in 0..40 {
            n.add(&report(10 + i, -60, &[]));
        }
        assert_eq!(n.count(), Nearby::MAX);
    }
}
