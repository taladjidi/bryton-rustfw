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
    /// No response within [`TIMEOUT_MS`]: the chip is not running, or the link is out of step.
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
    let mut buf = [0; 16];
    for _ in 0..TIMEOUT_MS {
        let n = b.nrf_recv(&mut buf);
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
        b.delay_ms(1);
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
