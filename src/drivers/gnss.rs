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

    /// Between frames (not inside one).
    pub fn idle(&self) -> bool {
        self.pos == 0
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
/// on SPI (what the receiver already does after stock, seen on the unit; sent so a receiver that lost its backup
/// memory does the same), and AssistNow Autonomous on (the receiver predicts orbits from the ones it received;
/// stock enables it too, "Enable_Assist_Auto"), and the power mode: continuous (FULL), or with `saver` cyclic
/// tracking (PSMCT, CFG-PM-OPERATEMODE 2), which the MAX-M10S data sheet (UBX-20035208 R08, table 15, 3 V, 1 Hz)
/// gives as 5 mA at VCC tracking instead of 9.5 for the default GPS + Galileo + BeiDou B1I, at some cost in accuracy
/// (the receiver sleeps between fixes; docs/power.md). Key IDs from u-blox's M10 SPG 5.10 interface description
/// (UBX-21035062, CFG-PM). Four keys: the payload's 24 bytes fill the 32-byte frame.
///
/// ```
/// use bryton_rustfw::gnss::config;
/// assert_eq!(config(false)[25..29], [0x01, 0x00, 0xd0, 0x20], "CFG-PM-OPERATEMODE");
/// assert_eq!((config(false)[29], config(true)[29]), (0, 2), "FULL, PSMCT");
/// ```
pub fn config(saver: bool) -> [u8; 32] {
    const MSGOUT_UBX_NAV_PVT_SPI: u32 = 0x2091_000a;
    const SPIOUTPROT_NMEA: u32 = 0x107a_0002;
    const ANA_USE_ANA: u32 = 0x1023_0001;
    const PM_OPERATEMODE: u32 = 0x20d0_0001;
    let mut p = [0u8; 24];
    p[..4].copy_from_slice(&[0, 0x01, 0, 0]); // version 0, layers: RAM
    let keys =
        [(MSGOUT_UBX_NAV_PVT_SPI, 1), (SPIOUTPROT_NMEA, 0), (ANA_USE_ANA, 1), (PM_OPERATEMODE, 2 * u8::from(saver))];
    for (i, (key, value)) in keys.iter().enumerate() {
        p[4 + 5 * i..8 + 5 * i].copy_from_slice(&key.to_le_bytes());
        p[8 + 5 * i] = *value;
    }
    frame(0x06, 0x8a, &p)
}

/// GPS POWER AUTO's thresholds and why. The trade (u-blox M10 integration manuals, e.g. SAM-M10Q UBX-22020019 R01,
/// 2.6.2 "Power save mode"): cyclic tracking (PSMCT) keeps the 1 Hz navigation output ("In the POT state the receiver
/// continues to output position fixes according to the CFG-RATE-*"), but in its power optimized tracking state "no new
/// signals are acquired and no data is downloaded", SBAS is neither downloaded nor used in power save mode, and a weak
/// or lost signal sends it back to tracking. u-blox gives no accuracy figure for it. So AUTO takes it only where little
/// is lost: fast, straight, under open sky, well after the fix, and goes back to continuous tracking at once otherwise.
///
/// Riding faster than this (mm/s, 20 km/h) is fast enough for cyclic tracking: a road pace. Under
/// [`AUTO_SLOW_MM_S`] (15 km/h) is MTB, technical ground and steep climbs, where the owner wants the resolution; the
/// gap between the two is the hysteresis, so a speed around one edge does not switch modes back and forth.
pub const AUTO_FAST_MM_S: u32 = 5556;
/// See [`AUTO_FAST_MM_S`].
pub const AUTO_SLOW_MM_S: u32 = 4167;
/// A turn: the heading moved more than this (degrees x 1e-5) within [`AUTO_TURN_S`] seconds. A road bend at 20 km/h
/// that turns 30 degrees in 5 s has a radius of about 53 m; hairpins, roundabouts and trail switchbacks turn more.
pub const AUTO_TURN: i32 = 30 * 100_000;
/// See [`AUTO_TURN`].
pub const AUTO_TURN_S: usize = 5;
/// Seconds of good fixes before cyclic tracking: as stock's own switch in its power save mode
/// (`Gps_AutoSelectConstellationGroup` 0xd0254ab0, docs/power.md: about 10 minutes riding with a 3D fix), and so the
/// receiver has gathered satellites and their data first (the manual: more signals tracked at the start help it stay
/// in power optimized tracking; no data download there).
pub const AUTO_WARM_S: u16 = 600;
/// Seconds every condition must hold in a row before cyclic tracking (any second that fails goes back to continuous at
/// once): at most one switch to SAVER a minute, so the modes cannot flap.
pub const AUTO_STEADY_S: u16 = 60;
/// Stock's other conditions for its switch (the same function): at least 8 satellites and a horizontal accuracy under
/// 5 m, as mm.
pub const AUTO_SATS: u8 = 8;
/// See [`AUTO_SATS`].
pub const AUTO_HACC_MM: u32 = 5000;

/// GPS POWER AUTO: chooses cyclic tracking (true) or continuous tracking, one fix (a second) at a time; thresholds
/// above.
///
/// ```
/// use bryton_rustfw::gnss::{Adaptive, Pvt, AUTO_STEADY_S, AUTO_WARM_S};
/// let fast = Pvt { fix: 3, fix_ok: true, sats: 12, h_acc_mm: 2000, speed_mm_s: 9000, ..Pvt::default() };
/// let mut a = Adaptive::default();
/// let saver = (0..AUTO_WARM_S + AUTO_STEADY_S).map(|_| a.fix(Some(fast), true)).last();
/// assert_eq!(saver, Some(true), "fast and straight, a fix for long enough");
/// assert!(!a.fix(Some(Pvt { speed_mm_s: 3000, ..fast }), true), "slow: continuous at once");
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct Adaptive {
    /// Seconds of good fixes in a row (up to [`AUTO_WARM_S`]).
    warm_s: u16,
    /// Seconds in a row every condition held (up to [`AUTO_STEADY_S`]).
    steady_s: u16,
    /// The last headings, degrees x 1e-5, one a fix (a ring): the next slot, and how many are kept.
    heads: [i32; AUTO_TURN_S],
    next: usize,
    n: usize,
    saver: bool,
}

impl Adaptive {
    /// Cyclic tracking chosen.
    pub fn saver(&self) -> bool {
        self.saver
    }

    /// Takes the second's fix (None: none) and whether a ride is running; returns the choice.
    pub fn fix(&mut self, pvt: Option<Pvt>, riding: bool) -> bool {
        let Some(p) = pvt.filter(|p| p.good() && p.fix == 3) else {
            *self = Adaptive::default(); // lost: continuous, and the warm-up again
            return false;
        };
        self.warm_s = self.warm_s.saturating_add(1).min(AUTO_WARM_S);
        let fast = p.speed_mm_s >= if self.saver { AUTO_SLOW_MM_S } else { AUTO_FAST_MM_S };
        let diff = |h: i32| {
            let d = (i64::from(p.heading) - i64::from(h)).rem_euclid(360 * 100_000); // any i32 from the receiver
            d.min(360 * 100_000 - d)
        };
        let turning = self.heads.iter().take(self.n).any(|&h| diff(h) > i64::from(AUTO_TURN));
        if let Some(slot) = self.heads.get_mut(self.next) {
            *slot = p.heading;
        }
        self.next = (self.next + 1) % AUTO_TURN_S;
        self.n = (self.n + 1).min(AUTO_TURN_S);
        let good = riding
            && fast
            && !turning
            && self.warm_s >= AUTO_WARM_S
            && p.sats >= AUTO_SATS
            && p.h_acc_mm <= AUTO_HACC_MM;
        self.steady_s = if good { self.steady_s.saturating_add(1).min(AUTO_STEADY_S) } else { 0 };
        self.saver = good && (self.saver || self.steady_s >= AUTO_STEADY_S);
        self.saver
    }
}

/// UBX-MGA-INI-TIME_UTC: the time, so the receiver can use the orbits it keeps (it loses its own clock when
/// powered off, seen on the unit). `utc`: year, month, day, hour, minute, second; `acc_s`: its accuracy.
pub fn mga_time(utc: (u16, u8, u8, u8, u8, u8), acc_s: u16) -> [u8; 32] {
    let (y, mo, d, h, mi, s) = utc;
    let mut p = [0u8; 24];
    p[0] = 0x10; // type: TIME_UTC; version 0; ref 0: none (on receipt)
    p[3] = 0x80; // leap seconds unknown
    p[4..6].copy_from_slice(&y.to_le_bytes());
    p[6..12].copy_from_slice(&[mo, d, h, mi, s, 0]);
    p[16..18].copy_from_slice(&acc_s.to_le_bytes());
    frame(0x13, 0x40, &p)
}

/// UBX-MGA-INI-POS_LLH: an approximate position (degrees x 1e-7), `acc_m` its accuracy.
pub fn mga_pos(lat: i32, lon: i32, acc_m: u32) -> [u8; 32] {
    let mut p = [0u8; 20];
    p[0] = 0x01; // type: POS_LLH; version 0
    p[4..8].copy_from_slice(&lat.to_le_bytes());
    p[8..12].copy_from_slice(&lon.to_le_bytes());
    p[16..20].copy_from_slice(&acc_m.saturating_mul(100).to_le_bytes()); // cm; altitude 0 with that accuracy
    frame(0x13, 0x40, &p)
}

/// The last position and the RTC's offset from UTC, as kept in two RTC backup registers between power-offs:
/// a tag, the offset in 15-minute steps (7 bits, signed) and latitude in 1e-4 degrees (21 bits, signed); a tag
/// and longitude in 1e-4 degrees (22 bits, signed). 1e-4 degrees is about 11 m, plenty for aiding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aid {
    /// Local time (the RTC) minus UTC, in 15-minute steps; None until a fix with a valid time.
    pub offset_15min: Option<i8>,
    /// Last position, degrees x 1e-7 (kept to 1e-4 degrees); None until a fix.
    pub pos: Option<(i32, i32)>,
}

impl Aid {
    /// As two backup register words.
    ///
    /// ```
    /// use bryton_rustfw::gnss::Aid;
    /// let a = Aid { offset_15min: Some(-20), pos: Some((5_123_000, 4_234_000)) };
    /// assert_eq!(Aid::from_words(a.to_words()), a);
    /// assert_eq!(Aid::from_words([0, 0]), Aid { offset_15min: None, pos: None });
    /// ```
    pub fn to_words(self) -> [u32; 2] {
        let field = |v: i32, bits: u32| (v as u32) & ((1 << bits) - 1);
        let (lat, lon) = self.pos.map_or((0, 0), |(a, o)| (a / 1000, o / 1000));
        let offset = self.offset_15min.map_or(0x40, |o| field(i32::from(o), 7)); // -64: unknown
        let pos_tag = if self.pos.is_some() { 0xa } else { 0x5 };
        [(0xa << 28) | (offset << 21) | field(lat, 21), (pos_tag << 28) | field(lon, 22)]
    }

    /// From the words [`Aid::to_words`] wrote (anything else reads as unknown).
    pub fn from_words([w0, w1]: [u32; 2]) -> Self {
        let signed = |v: u32, bits: u32| ((v << (32 - bits)) as i32) >> (32 - bits);
        if w0 >> 28 != 0xa {
            return Aid { offset_15min: None, pos: None };
        }
        let offset = signed(w0 >> 21, 7) as i8;
        Aid {
            offset_15min: (offset != -64).then_some(offset),
            pos: (w1 >> 28 == 0xa).then(|| (signed(w0, 21) * 1000, signed(w1, 22) * 1000)),
        }
    }
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
    /// Horizontal accuracy estimate, mm.
    pub h_acc_mm: u32,
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

/// Signal bars (1 to 4) for a fix from `sats` satellites used: under 6, 9, 12, then 12 and more.
///
/// ```
/// use bryton_rustfw::gnss::bars;
/// assert_eq!([4, 6, 9, 12, 20].map(bars), [1, 2, 3, 4, 4]);
/// ```
pub fn bars(sats: u8) -> u8 {
    match sats {
        0..6 => 1,
        6..9 => 2,
        9..12 => 3,
        _ => 4,
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
        h_acc_mm: i32_at(40) as u32,
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
        let c = config(true);
        let mut parser = Parser::new();
        let got: std::vec::Vec<_> = c.iter().filter_map(|&b| parser.push(b).map(|(c, i, p)| (c, i, p.len()))).collect();
        assert_eq!(got, [(0x06, 0x8a, 24)]);
        for f in [mga_time((2026, 10, 23, 15, 4, 5), 2), mga_pos(5_123_000, 4_234_000, 10_000)] {
            let mut parser = Parser::new();
            let got: std::vec::Vec<_> =
                f.iter().filter_map(|&b| parser.push(b).map(|(c, i, p)| (c, i, p.len()))).collect();
            assert!(got == [(0x13, 0x40, 24)] || got == [(0x13, 0x40, 20)], "{got:?}");
        }
        assert_eq!(c.len() % 4, 0);
    }
}
