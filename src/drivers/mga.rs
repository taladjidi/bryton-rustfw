//! AssistNow Offline data for the GNSS receiver: UBX-MGA-ANO messages (class 0x13, id 0x20), read from
//! `\Rustfw\mga.ubx` on the card, where the computer put the file `tools/assist.py` downloaded from u-blox's
//! AssistNow Offline service. Only the messages dated today go to the receiver (it keeps the orbit prediction of
//! the day, which lets it find satellites without a download from them).
//!
//! Sources: u-blox M10 SPG 5.10 interface description UBX-21035062, UBX-MGA-ANO (76-byte payload: type 0, version,
//! svId, gnssId, year since 2000, month, day, reserved, 64 bytes of data, 4 reserved; layout cross-checked with
//! pyubx2's `ubxtypes_set.py`); the AssistNow Offline file is a plain concatenation of these messages. Flow control:
//! the description asks for MGA-ACK or a pace the interface can take; this firmware paces (one message per 100 ms,
//! 840 bytes/s, under a 9600 baud UART's 960 B/s and far under SPI's rate) and does not wait for acks.
//!
//! [`Ano`] is the reader: the app hands it the file a sector at a time and takes the messages out one by one. A
//! missing, stale (no message of today), damaged or endless file ends quietly: the frames that fail their checksum
//! are skipped by the UBX parser, and the scan stops at [`MAX_SCAN`] bytes.

use crate::gnss::Parser;

/// The file, in this firmware's folder.
pub const FILE: &str = "Rustfw/mga.ubx";

/// Bytes of the file looked at at most: a week of GPS, Galileo and GLONASS at one day's resolution is about 60 KB.
pub const MAX_SCAN: u32 = 512 * 1024;

/// Bytes of a message on the wire, padded with 0xff to three 32-byte transfers (the receiver ignores 0xff): header
/// 6, payload 76, checksum 2.
pub const WIRE: usize = 96;

/// One message's payload length.
const PAYLOAD: usize = 76;

/// Reads the file and picks the messages of one day.
pub struct Ano {
    parser: Parser,
    /// Year since 2000, month, day wanted.
    today: (u8, u8, u8),
    /// The sector in hand, its valid bytes, and the next byte to look at.
    sector: [u8; 512],
    len: usize,
    at: usize,
    /// File offset of the next sector to read; None: nothing to read (not started, or the end).
    next: Option<u32>,
    /// Messages found so far.
    pub found: u16,
}

impl Default for Ano {
    fn default() -> Self {
        Self::new()
    }
}

impl Ano {
    /// Not started: reads nothing.
    pub const fn new() -> Self {
        Ano { parser: Parser::new(), today: (0, 0, 0), sector: [0; 512], len: 0, at: 0, next: None, found: 0 }
    }

    /// Starts over for the day (year, month, day), from the file's start.
    pub fn start(&mut self, utc: (u16, u8, u8)) {
        *self = Ano::new();
        self.today = (utc.0.saturating_sub(2000).min(255) as u8, utc.1, utc.2);
        self.next = Some(0);
    }

    /// Nothing more to read or take.
    pub fn done(&self) -> bool {
        self.next.is_none() && self.at >= self.len
    }

    /// The offset and the buffer for the next read; None while the sector in hand is not used up, or at the end.
    pub fn want(&mut self) -> Option<(u32, &mut [u8; 512])> {
        let at = self.next.filter(|_| self.at >= self.len)?;
        Some((at, &mut self.sector))
    }

    /// The read at [`Ano::want`]'s offset gave `n` bytes (0 or a failed read: the end; less than a sector: the last).
    pub fn filled(&mut self, n: usize) {
        let n = n.min(512);
        let off = self.next.unwrap_or(0);
        (self.len, self.at) = (n, 0);
        self.next = Some(off + 512).filter(|&o| n == 512 && o < MAX_SCAN);
    }

    /// The next message of the day in the sector in hand, as it goes on the wire; None when the sector has no more.
    pub fn take(&mut self) -> Option<[u8; WIRE]> {
        while let Some(&b) = self.sector.get(self.at).filter(|_| self.at < self.len) {
            self.at += 1;
            let Some((0x13, 0x20, p)) = self.parser.push(b) else { continue };
            if p.len() != PAYLOAD || p[0] != 0 || (p[4], p[5], p[6]) != self.today {
                continue;
            }
            let mut f = [0xff; WIRE];
            f[..6].copy_from_slice(&[0xb5, 0x62, 0x13, 0x20, PAYLOAD as u8, 0]);
            f[6..6 + PAYLOAD].copy_from_slice(p);
            let (mut a, mut c) = (0u8, 0u8);
            for &x in &f[2..6 + PAYLOAD] {
                a = a.wrapping_add(x);
                c = c.wrapping_add(a);
            }
            (f[6 + PAYLOAD], f[7 + PAYLOAD]) = (a, c);
            self.found = self.found.saturating_add(1);
            return Some(f);
        }
        None
    }
}

/// A message as the service writes it (for the tests and the simulator): satellite `sv` of system `gnss`, valid
/// on the date (year since 2000, month, day).
pub fn ano(gnss: u8, sv: u8, date: (u8, u8, u8)) -> [u8; 84] {
    let mut f = [0u8; 84];
    f[..6].copy_from_slice(&[0xb5, 0x62, 0x13, 0x20, PAYLOAD as u8, 0]);
    f[8..13].copy_from_slice(&[sv, gnss, date.0, date.1, date.2]);
    f[14..78].fill(sv ^ date.2); // stands for the 64 data bytes
    let (mut a, mut c) = (0u8, 0u8);
    for &x in &f[2..82] {
        a = a.wrapping_add(x);
        c = c.wrapping_add(a);
    }
    (f[82], f[83]) = (a, c);
    f
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// Everything of `file` for `day`, read as the app does: a sector at a time.
    fn run(file: &[u8], day: (u16, u8, u8)) -> Vec<[u8; WIRE]> {
        let mut r = Ano::new();
        r.start(day);
        let mut out = Vec::new();
        while !r.done() {
            while let Some(f) = r.take() {
                out.push(f);
            }
            if let Some((off, buf)) = r.want() {
                let from = (off as usize).min(file.len());
                let n = (file.len() - from).min(512);
                buf[..n].copy_from_slice(&file[from..from + n]);
                r.filled(n);
            }
        }
        out
    }

    #[test]
    fn only_today_goes_and_checksums_hold() {
        let mut file = Vec::new();
        for day in 25..28u8 {
            for sv in 1..=40 {
                file.extend(ano(0, sv, (26, 10, day)));
            }
        }
        let got = run(&file, (2026, 10, 26));
        assert_eq!(got.len(), 40, "one day of the three");
        let mut p = Parser::new();
        let n = got[7].iter().filter_map(|&b| p.push(b).map(|(c, i, q)| (c, i, q[2], q[6]))).count();
        assert_eq!(n, 1, "a whole frame with a good checksum");
        assert!(run(&file, (2026, 10, 29)).is_empty(), "stale: nothing");
        assert!(run(&[], (2026, 10, 26)).is_empty(), "empty file");
    }

    #[test]
    fn damage_and_noise_are_skipped() {
        let mut file = Vec::new();
        file.extend([0xff; 7]);
        let mut bad = ano(0, 1, (26, 10, 26));
        bad[20] ^= 1; // payload flipped: checksum fails
        file.extend(bad);
        file.extend(ano(2, 5, (26, 10, 26)));
        let mut cut = ano(0, 9, (26, 10, 26)).to_vec();
        cut.truncate(60);
        file.extend(cut); // truncated: it swallows the start of the next frame (10), which is lost with it
        file.extend(ano(0, 10, (26, 10, 26)));
        file.extend(ano(0, 11, (26, 10, 26)));
        let got = run(&file, (2026, 10, 26));
        assert_eq!(got.iter().map(|f| f[8]).collect::<Vec<_>>(), [5, 11]);
    }
}
