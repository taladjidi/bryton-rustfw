//! The Nordic BLE / ANT chip (BOARD.md 11): Nordic's nRF5 serialization over a byte link. The board only moves
//! bytes ([`Board::nrf_reset`], [`Board::nrf_send`], [`Board::nrf_recv`]); the framing and the commands are here.
//!
//! Framing: a 2-byte little-endian length, then the payload `[type][op][arguments]`. A response is
//! `[RESP][op][result u32][outputs]`. There is no marker to resync on: a lost or stray byte leaves the length framing
//! out of step. [`Rx`] counts the signs of that (`strikes`, `timeouts`); `app/link.rs` watches them, drains and
//! retries, and failing that resets the chip and starts it again.

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
/// BLE event id: a notification or indication from the peer (BLE_GATTC_EVT_HVX).
pub const EVT_HVX: u16 = 0x39;

/// Largest payload kept; a longer one is read through and dropped.
pub const MAX: usize = 256;

/// How long a command may take before it is given up (stock waits 5 s; replies take a few ms).
pub const TIMEOUT_MS: u32 = 500;

/// A length header past this cannot be a frame (a BLE event is at most one ATT packet, about 250 bytes, and a
/// header): it counts as garbage and is not read through, so a stray byte does not swallow up to 64 KB of real frames.
pub const GARBAGE: usize = 512;

/// Longest BLE event kept while a command waits ([`Rx::pop`]).
pub const KEPT: usize = 128;

/// Reassembles packets from the byte stream, and keeps the BLE events that arrive while a command waits for its
/// response (a phone's connection or pairing must not be lost to an ANT command's wait; advertising reports are).
pub struct Rx {
    buf: [u8; MAX],
    header: [u8; 2],
    got: usize,
    want: Option<usize>,
    /// The last packet's length (in `buf`).
    last: usize,
    kept: [[u8; KEPT]; 8],
    kept_len: [u8; 8],
    kept_n: usize,
    /// The last command that failed: its op and the error (`u32::MAX`: no answer). For the unit's first runs: the
    /// PHONE menu shows it when the link could not be set up.
    pub failed: Option<(u8, u32)>,
    /// A good frame (a type the chip sends) arrived since this was last taken (`app/link.rs`).
    pub heard: bool,
    /// Garbage in a row since the last good frame: a length header past [`GARBAGE`], a frame of a type the chip
    /// never sends. Two in a row mean the framing is out of step.
    pub strikes: u8,
    /// Commands in a row that got no response, since the last good frame.
    pub timeouts: u8,
}

impl Default for Rx {
    fn default() -> Self {
        Self::new()
    }
}

impl Rx {
    /// Waiting for a length header.
    pub const fn new() -> Self {
        Rx {
            buf: [0; MAX],
            header: [0; 2],
            got: 0,
            want: None,
            last: 0,
            kept: [[0; KEPT]; 8],
            kept_len: [0; 8],
            kept_n: 0,
            failed: None,
            heard: false,
            strikes: 0,
            timeouts: 0,
        }
    }

    /// Forgets the frame being read and the events kept, and the signs of a broken link: the stream is read again
    /// from a frame boundary (after a drain, or a chip reset).
    pub fn resync(&mut self) {
        (self.got, self.want, self.last, self.kept_n) = (0, None, 0, 0);
        (self.strikes, self.timeouts) = (0, 0);
    }

    /// Keeps the last packet for [`Rx::pop`] if it is a BLE event other than an advertising report and fits (eight
    /// at most; when full, the oldest notification ([`EVT_HVX`]) makes room for another kind of event, else the new
    /// one is dropped). The main loop keeps them too, and acts on them once the bytes read are all
    /// framed: a command sent from the middle of a read would take the rest of the stream out of order.
    pub fn keep_last(&mut self) {
        let p = self.buf.get(..self.last).unwrap_or(&[]);
        let id = event_id(p);
        if id.is_none_or(|id| id == EVT_ADV_REPORT) || p.len() > KEPT {
            return;
        }
        // full: the oldest notification from the phone makes room for any other event (a connection's end, a GATT
        // answer the discovery waits for), so a flood of ANCS notifications cannot hide them
        if self.kept_n == self.kept.len()
            && id != Some(EVT_HVX)
            && let Some(i) = (0..self.kept_n).find(|&i| event_id(&self.kept[i]) == Some(EVT_HVX))
        {
            self.kept[i..].rotate_left(1);
            self.kept_len[i..].rotate_left(1);
            self.kept_n -= 1;
        }
        if let (Some(slot), Some(len)) = (self.kept.get_mut(self.kept_n), self.kept_len.get_mut(self.kept_n)) {
            slot[..p.len()].copy_from_slice(p);
            *len = p.len() as u8;
            self.kept_n += 1;
        }
    }

    /// The oldest event kept while a command waited, copied into `out`; its length.
    pub fn pop(&mut self, out: &mut [u8; KEPT]) -> Option<usize> {
        if self.kept_n == 0 {
            return None;
        }
        let n = usize::from(self.kept_len[0]);
        out.copy_from_slice(&self.kept[0]);
        self.kept.rotate_left(1);
        self.kept_len.rotate_left(1);
        self.kept_n -= 1;
        Some(n)
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
                if n > GARBAGE {
                    self.strikes = self.strikes.saturating_add(1);
                } else if n > 0 {
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
        self.last = if want <= MAX { want } else { 0 };
        let good = match self.buf.get(..want) {
            Some([RESP | BLE_EVT | ANT_EVT, ..]) => Some(true),
            Some(_) => Some(false), // a type the chip never sends
            None => None,           // did not fit: dropped
        };
        match good {
            Some(true) => (self.heard, self.strikes, self.timeouts) = (true, 0, 0),
            Some(false) => self.strikes = self.strikes.saturating_add(1),
            None => {}
        }
        self.buf.get(..want)
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

/// Reads packets until `f` returns Some for one (other packets are dropped), or [`TIMEOUT_MS`] of silence, or
/// 64 KB of other packets (a scan's events). One byte at a time: what follows stays in the chip's stream.
fn wait<T>(b: &mut impl Board, rx: &mut Rx, mut f: impl FnMut(&[u8]) -> Option<T>) -> Option<T> {
    let mut buf = [0; 1];
    let (mut idle_ms, mut bytes) = (0, 0u32);
    while idle_ms < TIMEOUT_MS && bytes < 0x1_0000 {
        let n = b.nrf_recv(&mut buf);
        bytes += n as u32;
        if n == 0 {
            b.delay_ms(1);
            idle_ms += 1;
        } else if let Some(p) = rx.push(buf[0]) {
            if let Some(t) = f(p) {
                return Some(t);
            }
            rx.keep_last();
        }
    }
    None
}

/// Sends the command `payload` (`[type][op][arguments]`) and waits for its response; copies the outputs into `out`
/// and returns their length. Packets that arrive meanwhile are dropped, BLE events but advertising reports kept
/// ([`Rx::pop`]).
pub fn call(b: &mut impl Board, rx: &mut Rx, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    send(b, payload);
    let op = payload.get(1).copied();
    let response = |p: &[u8]| match *p {
        [RESP, o, r0, r1, r2, r3, ref outputs @ ..] if Some(o) == op => {
            Some(match u32::from_le_bytes([r0, r1, r2, r3]) {
                0 => {
                    let len = outputs.len().min(out.len());
                    out[..len].copy_from_slice(&outputs[..len]);
                    Ok(len)
                }
                code => Err(Error::Code(code)),
            })
        }
        _ => None,
    };
    let r = wait(b, rx, response).unwrap_or(Err(Error::Timeout));
    if r == Err(Error::Timeout) {
        rx.timeouts = rx.timeouts.saturating_add(1);
    }
    if let (Err(e), Some(op)) = (r, op) {
        rx.failed = Some((op, if let Error::Code(c) = e { c } else { u32::MAX }));
    }
    r
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
// ANT+ (BOARD.md 11): one channel per sensor kind (sensors.rs), all assigned at start, then opened and closed.
// ---------------------------------------------------------------------------------------------------------------

/// A paired sensor: its ANT channel ID, so the channel finds that sensor and no other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorId {
    /// Device number (the sensor's own, printed on some).
    pub device: u16,
    /// Transmission type (its upper nibble extends the device number to 20 bits).
    pub trans: u8,
}

impl SensorId {
    /// As kept in a 32-bit backup register, behind a tag byte so an empty or foreign value reads as no sensor.
    ///
    /// ```
    /// use bryton_rustfw::nrf::SensorId;
    /// let s = SensorId { device: 25126, trans: 0xf1 };
    /// assert_eq!(SensorId::from_word(s.to_word()), Some(s));
    /// assert_eq!(SensorId::from_word(0), None);
    /// ```
    pub fn to_word(self) -> u32 {
        (0xa5 << 24) | (u32::from(self.trans) << 16) | u32::from(self.device)
    }

    /// The sensor kept in `w`, if [`SensorId::to_word`] wrote it.
    pub fn from_word(w: u32) -> Option<Self> {
        (w >> 24 == 0xa5).then_some(SensorId { device: w as u16, trans: (w >> 16) as u8 })
    }
}

/// ANT event code: the channel is closed (after [`ant_close`], or a search that timed out).
const EVENT_CHANNEL_CLOSED: u8 = 0x07;

/// Low-priority search time-out, in 2.5 s units: 12 = 30 s. Then the chip closes the channel and the app reopens it
/// later ([`crate::SEARCH_REST_MS`]), so a sensor that is not sending (a strap not worn) does not keep the radio
/// listening all the time. Stock uses 0xff (searching for ever).
pub const SEARCH_TIMEOUT: u8 = 12;

/// The channel of ANT event `p` if it reports a channel closed.
///
/// ```
/// let mut p = [0u8; 44];
/// p[0] = 8;
/// (p[42], p[43]) = (7, 3);
/// assert_eq!(bryton_rustfw::nrf::ant_closed(&p), Some(3));
/// assert_eq!(bryton_rustfw::nrf::ant_closed(&[2, 0x1b, 0]), None, "any other packet, short ones included");
/// ```
pub fn ant_closed(p: &[u8]) -> Option<u8> {
    match p {
        [ANT_EVT, .., EVENT_CHANNEL_CLOSED, ch] if p.len() == 44 => Some(*ch),
        _ => None,
    }
}

/// Enables ANT, sets the network keys (network 0 the ANT+ one, licensed; network 1 Shimano's, if given: build.rs
/// reads both from private files, never from this repository) and assigns channel `ch` as a receiver on network
/// `nets[ch]`, as stock does at start. A channel on network 1 without its key is left unassigned (its opening then
/// fails). Returns whether network 1 is up: its key and its channels accepted (else only the Di2 is off).
pub fn ant_start(
    b: &mut impl Board,
    rx: &mut Rx,
    key: [u8; 8],
    private: Option<[u8; 8]>,
    nets: &[u8],
) -> Result<bool, Error> {
    // enable: 15 channels, 0 encrypted, event queue 32, then stock's buffer pointer and size (sent as stock sends
    // them; the chip allocates its own, inferred)
    call(b, rx, &[ANT_CMD, 0xfa, 1, 0x0f, 0, 0x20, 0, 0x7c, 0x16, 0x06, 0x24, 0xfc, 0x05], &mut [])?;
    fn set_key(
        b: &mut impl Board,
        rx: &mut Rx,
        net: u8,
        [k0, k1, k2, k3, k4, k5, k6, k7]: [u8; 8],
    ) -> Result<usize, Error> {
        call(b, rx, &[ANT_CMD, 0xcc, net, k0, k1, k2, k3, k4, k5, k6, k7], &mut [])
    }
    set_key(b, rx, 0, key)?;
    // Shimano's network failing, its key or its channel, leaves the ANT+ sensors as they are (review of 571e613: a
    // refused channel 6 took ANT down whole)
    let mut private = private.is_some_and(|k| set_key(b, rx, 1, k).is_ok());
    for (ch, &net) in (0..).zip(nets) {
        let assign = [ANT_CMD, 0xc2, ch, 0, net, 0]; // assign: slave, network `net`, no extended
        if net == 0 {
            call(b, rx, &assign, &mut [])?;
        } else if private {
            private = call(b, rx, &assign, &mut []).is_ok();
        }
    }
    Ok(private)
}

/// Opens (closed, assigned) channel `ch` for sensors of `device_type` with stock's settings: for `id` only, or for
/// the first one heard (pairing) when None.
pub fn ant_open(
    b: &mut impl Board,
    rx: &mut Rx,
    ch: u8,
    device_type: u8,
    period: u16,
    id: Option<SensorId>,
) -> Result<(), Error> {
    let SensorId { device, trans } = id.unwrap_or(SensorId { device: 0, trans: 0 }); // 0, 0: any
    let [d0, d1] = device.to_le_bytes();
    let [p0, p1] = period.to_le_bytes();
    let cmds: [&[u8]; 5] = [
        &[ANT_CMD, 0xd3, ch, d0, d1, device_type, trans], // channel id
        &[ANT_CMD, 0xda, ch, SEARCH_TIMEOUT],             // low-priority search time-out
        &[ANT_CMD, 0xcd, ch, 0x39],                       // 2457 MHz, ANT+
        &[ANT_CMD, 0xd1, ch, p0, p1],
        &[ANT_CMD, 0xc4, ch, 0, 0], // open
    ];
    for cmd in cmds {
        call(b, rx, cmd, &mut [])?;
    }
    Ok(())
}

/// Closes channel `ch` and waits for the chip to report it closed, so it can be opened again with another ID.
pub fn ant_close(b: &mut impl Board, rx: &mut Rx, ch: u8) -> Result<(), Error> {
    call(b, rx, &[ANT_CMD, 0xc5, ch], &mut [])?;
    let closed = |p: &[u8]| (ant_closed(p) == Some(ch)).then_some(());
    wait(b, rx, closed).ok_or(Error::Timeout)
}

/// The sensor channel `ch` locked onto (`sd_ant_channel_id_get`, op 0xd4, seen on the unit): the reply's outputs
/// are the device number, the device type and the transmission type. None while it still searches (number 0).
pub fn ant_channel_id(b: &mut impl Board, rx: &mut Rx, ch: u8) -> Result<Option<SensorId>, Error> {
    let mut out = [0; 4];
    match (call(b, rx, &[ANT_CMD, 0xd4, ch], &mut out)?, out) {
        (4, [d0, d1, _, trans]) if [d0, d1] != [0, 0] => {
            Ok(Some(SensorId { device: u16::from_le_bytes([d0, d1]), trans }))
        }
        _ => Ok(None),
    }
}

/// The channel and the 8 data bytes of ANT event `p`, if it is data: a broadcast `[ANT_EVT][size][0x4e][channel]
/// [8 data bytes]...`, or the same as acknowledged data (0x4f: Shimano's button page, stock's
/// `Ant_Di2_OnAntEvent`), 44 bytes in all.
///
/// ```
/// let mut p = [0u8; 44];
/// p[..12].copy_from_slice(&[8, 9, 0x4e, 3, 4, 0, 0, 0, 0, 0, 0, 72]);
/// assert_eq!(bryton_rustfw::nrf::ant_broadcast(&p), Some((3, [4, 0, 0, 0, 0, 0, 0, 72])));
/// p[2] = 0x4f;
/// assert_eq!(bryton_rustfw::nrf::ant_broadcast(&p), Some((3, [4, 0, 0, 0, 0, 0, 0, 72])), "acknowledged");
/// ```
pub fn ant_broadcast(p: &[u8]) -> Option<(u8, [u8; 8])> {
    match p {
        [ANT_EVT, _, 0x4e | 0x4f, ch, d0, d1, d2, d3, d4, d5, d6, d7, ..] if p.len() == 44 => {
            Some((*ch, [*d0, *d1, *d2, *d3, *d4, *d5, *d6, *d7]))
        }
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

    /// Frames `p` into `rx` and keeps it, as the main loop does.
    fn keep(rx: &mut Rx, p: &[u8]) {
        for &byte in (p.len() as u16).to_le_bytes().iter().chain(p) {
            if rx.push(byte).is_some() {
                rx.keep_last();
            }
        }
    }

    #[test]
    fn a_flood_of_notifications_does_not_push_out_a_disconnection() {
        // an iPhone's notifications after subscribing (ANCS's pre-existing ones), then the link's end
        let mut rx = Rx::new();
        for uid in 0..12u8 {
            keep(&mut rx, &[BLE_EVT, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x12, 0, 1, 1, 0, uid]);
        }
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        let mut out = [0; KEPT];
        let mut ids = std::vec::Vec::new();
        while let Some(n) = rx.pop(&mut out) {
            ids.push(event_id(&out[..n]));
        }
        assert_eq!(ids.len(), 8);
        assert_eq!(ids.last(), Some(&Some(0x11)), "the disconnection kept, an old notification dropped");
    }

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
