//! The u-blox M10 GNSS receiver (BOARD.md 13): UBX framing, the NAV-PVT fix, and the configuration this firmware
//! sends. Pure logic: the board only moves SPI bytes ([`Board::gnss_transfer`](crate::Board::gnss_transfer)).
//!
//! UBX frame: `b5 62 [class][id][length u16][payload][ck_a][ck_b]`, an 8-bit Fletcher checksum over class to
//! payload. On SPI the receiver sends 0xff when it has nothing; 0xff is also ignored on its input.

/// Largest payload kept (NAV-PVT is 92 bytes); a longer frame is read through and dropped.
pub const MAX: usize = 100;

/// Reassembles UBX frames from the SPI byte stream.
pub struct Parser {
    buf: [u8; MAX],
    /// Bytes of the current frame seen so far (0: waiting for 0xb5).
    pos: usize,
    class_id: (u8, u8),
    len: usize,
    ck: (u8, u8),
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// Waiting for a frame.
    pub const fn new() -> Self {
        Parser { buf: [0; MAX], pos: 0, class_id: (0, 0), len: 0, ck: (0, 0) }
    }

    /// Takes one byte; returns the class, id and payload of a frame it completes with a good checksum.
    ///
    /// ```
    /// let mut p = bryton_rustfw::gnss::Parser::new();
    /// let frame = bryton_rustfw::gnss::frame(0x0a, 0x04, &[1, 2]);
    /// let mut got = None;
    /// for &b in [0xff, 0xff].iter().chain(&frame[..10]) {
    ///     if let Some((c, i, p)) = p.push(b) { got = Some((c, i, p.to_vec())); }
    /// }
    /// assert_eq!(got, Some((0x0a, 0x04, vec![1, 2])));
    /// ```
    pub fn push(&mut self, b: u8) -> Option<(u8, u8, &[u8])> {
        let at = self.pos;
        self.pos += 1;
        match at {
            0 => self.pos = usize::from(b == 0xb5),
            1 if b != 0x62 => self.pos = usize::from(b == 0xb5),
            1 => self.ck = (0, 0),
            2..=5 => {
                self.sum(b);
                match at {
                    2 => self.class_id.0 = b,
                    3 => self.class_id.1 = b,
                    4 => self.len = usize::from(b),
                    _ => self.len |= usize::from(b) << 8,
                }
            }
            _ if at < 6 + self.len => {
                self.sum(b);
                if let Some(slot) = self.buf.get_mut(at - 6) {
                    *slot = b;
                }
            }
            _ if at == 6 + self.len => {
                if b != self.ck.0 {
                    self.pos = 0;
                }
            }
            _ => {
                self.pos = 0;
                if b == self.ck.1 && self.len <= MAX {
                    return Some((self.class_id.0, self.class_id.1, self.buf.get(..self.len)?));
                }
            }
        }
        None
    }

    fn sum(&mut self, b: u8) {
        self.ck.0 = self.ck.0.wrapping_add(b);
        self.ck.1 = self.ck.1.wrapping_add(self.ck.0);
    }
}

/// A UBX frame for `payload` (at most 24 bytes here), padded with 0xff to a multiple of 4 bytes (the SPI
/// transfers move 4 bytes at a time; the receiver ignores 0xff).
pub fn frame(class: u8, id: u8, payload: &[u8]) -> [u8; 32] {
    let mut f = [0xff; 32];
    let n = payload.len().min(24);
    f[..6].copy_from_slice(&[0xb5, 0x62, class, id, n as u8, 0]);
    f[6..6 + n].copy_from_slice(&payload[..n]);
    let (mut a, mut b) = (0u8, 0u8);
    for &x in &f[2..6 + n] {
        a = a.wrapping_add(x);
        b = b.wrapping_add(a);
    }
    f[6 + n] = a;
    f[7 + n] = b;
    f
}

/// UBX-CFG-VALSET to the RAM layer only (never flash or battery-backed RAM): NAV-PVT once per fix on SPI, no NMEA
/// on SPI. What the receiver already does after stock (seen on the unit); sent so a receiver that lost its backup
/// memory does the same. Key IDs from u-blox's M10 interface description.
pub fn config() -> [u8; 32] {
    const MSGOUT_UBX_NAV_PVT_SPI: u32 = 0x2091_000a;
    const SPIOUTPROT_NMEA: u32 = 0x107a_0002;
    let mut p = [0u8; 14];
    p[..4].copy_from_slice(&[0, 0x01, 0, 0]); // version 0, layers: RAM
    p[4..8].copy_from_slice(&MSGOUT_UBX_NAV_PVT_SPI.to_le_bytes());
    p[8] = 1;
    p[9..13].copy_from_slice(&SPIOUTPROT_NMEA.to_le_bytes());
    p[13] = 0;
    frame(0x06, 0x8a, &p)
}

/// A fix as UBX-NAV-PVT reports it (the fields this firmware uses).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pvt {
    /// UTC date and time: year, month, day, hour, minute, second; valid when [`Pvt::time_ok`].
    pub utc: (u16, u8, u8, u8, u8, u8),
    /// Date and time fully resolved (`valid` bits 0 to 2).
    pub time_ok: bool,
    /// Fix type: 0 none, 2 2D, 3 3D (and 1 dead reckoning, 4 GNSS + DR, 5 time only).
    pub fix: u8,
    /// The fix is valid (flags `gnssFixOK`).
    pub fix_ok: bool,
    /// Satellites used.
    pub sats: u8,
    /// Longitude and latitude, degrees x 1e-7.
    pub lon: i32,
    /// Latitude, degrees x 1e-7.
    pub lat: i32,
    /// Height above mean sea level, mm.
    pub alt_mm: i32,
    /// Ground speed, mm/s.
    pub speed_mm_s: u32,
    /// Heading of motion, degrees x 1e-5.
    pub heading: i32,
}

impl Pvt {
    /// A fix good for position and speed: 2D or 3D, flagged valid.
    pub fn good(&self) -> bool {
        self.fix_ok && (self.fix == 2 || self.fix == 3)
    }
}

/// Decodes a NAV-PVT payload (class 0x01, id 0x07, 92 bytes).
pub fn pvt(class: u8, id: u8, p: &[u8]) -> Option<Pvt> {
    if (class, id) != (0x01, 0x07) || p.len() < 92 {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
    let i32_at = |i: usize| i32::from_le_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
    Some(Pvt {
        utc: (u16_at(4), p[6], p[7], p[8], p[9], p[10]),
        time_ok: p[11] & 0x07 == 0x07,
        fix: p[20],
        fix_ok: p[21] & 1 != 0,
        sats: p[23],
        lon: i32_at(24),
        lat: i32_at(28),
        alt_mm: i32_at(36),
        speed_mm_s: i32_at(60).max(0) as u32,
        heading: i32_at(64),
    })
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// A NAV-PVT payload as the unit's receiver sent it indoors (no fix), with speed and a fix patched in.
    fn sample(fix: u8, speed_mm_s: i32) -> [u8; 92] {
        let mut p = [0u8; 92];
        p[4..12].copy_from_slice(&[0xe5, 0x07, 3, 7, 0, 0, 0, 0xf0]); // 2021-03-07 00:00:00, as seen indoors
        p[20] = fix;
        p[21] = u8::from(fix >= 2);
        p[23] = 9;
        p[60..64].copy_from_slice(&speed_mm_s.to_le_bytes());
        p
    }

    #[test]
    fn parses_frames_from_the_spi_stream_with_idle_fill() {
        let payload = sample(3, 7000);
        let mut f = std::vec::Vec::from([0xff; 5]);
        let (mut a, mut b) = (0u8, 0u8);
        let body: std::vec::Vec<u8> = [0x01, 0x07, 92, 0].iter().chain(&payload).copied().collect();
        for &x in &body {
            a = a.wrapping_add(x);
            b = b.wrapping_add(a);
        }
        f.extend([0xb5, 0x62]);
        f.extend(&body);
        f.extend([a, b, 0xff, 0xff]);
        let mut parser = Parser::new();
        let mut fixes = std::vec::Vec::new();
        for &x in f.iter().chain(&f) {
            if let Some((c, i, p)) = parser.push(x) {
                fixes.push(pvt(c, i, p).unwrap());
            }
        }
        assert_eq!(fixes.len(), 2, "both frames, 0xff fill between them");
        assert!(fixes[0].good() && fixes[0].speed_mm_s == 7000 && fixes[0].sats == 9);
        assert_eq!(fixes[0].utc, (2021, 3, 7, 0, 0, 0));
        f[20] ^= 1; // corrupt a payload byte: the checksum drops the frame
        let mut parser = Parser::new();
        assert!(f.iter().all(|&x| parser.push(x).is_none()));
    }

    #[test]
    fn config_is_a_valid_frame() {
        let c = config();
        let mut parser = Parser::new();
        let got: std::vec::Vec<_> = c.iter().filter_map(|&b| parser.push(b).map(|(c, i, p)| (c, i, p.len()))).collect();
        assert_eq!(got, [(0x06, 0x8a, 14)]);
        assert_eq!(c.len() % 4, 0);
    }
}
