//! The recovery record ([`Rec`]: one sector of `ride.rec`, its layout and CRC) and the ride's names (stock's
//! `YYMMDDhhmmss`), split from the recorder; the module doc there tells how they are used.

use super::{Local, Phase, TAG};
use crate::fat::{Error, SECTOR};
use crate::fit::{self, RECORD_LEN};
use crate::ride::RIDE_WORDS;

/// One recovery record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rec {
    pub(super) seq: u32,
    pub(super) phase: Phase,
    pub(super) name: [u8; 12],
    pub(super) cluster: u32,
    pub(super) fit: [u8; RECORD_LEN],
    pub(super) ride: [u32; RIDE_WORDS],
    /// The save goes to `\Rustfw\Discarded` (module doc), byte 22: 0 in the records of the builds before it, which
    /// stay valid (no new [`TAG`]: a ride in progress at the update is resumed).
    pub(super) discard: bool,
}

impl Rec {
    pub(super) fn to_sector(self) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        s[..4].copy_from_slice(&TAG);
        s[4..8].copy_from_slice(&self.seq.to_le_bytes());
        s[8] = self.phase as u8;
        s[9..21].copy_from_slice(&self.name);
        s[21] = u8::try_from(RIDE_WORDS).unwrap_or(0); // a constant under 256
        s[22] = u8::from(self.discard);
        s[24..28].copy_from_slice(&self.cluster.to_le_bytes());
        s[32..32 + RECORD_LEN].copy_from_slice(&self.fit);
        for (c, w) in s[128..128 + 4 * RIDE_WORDS].chunks_exact_mut(4).zip(self.ride) {
            c.copy_from_slice(&w.to_le_bytes());
        }
        let crc = fit::crc(0, &s[..SECTOR - 2]);
        s[SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
        s
    }

    pub(super) fn from_sector(s: &[u8; SECTOR]) -> Option<Rec> {
        if s[..4] != TAG
            || usize::from(s[21]) != RIDE_WORDS
            || fit::crc(0, &s[..SECTOR - 2]).to_le_bytes() != s[SECTOR - 2..]
        {
            return None;
        }
        let word = |i: usize| s.get(i..i + 4).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes);
        let phase = match s[8] {
            0 => Phase::Done,
            1 => Phase::Starting,
            2 => Phase::Recording,
            3 => Phase::Saving,
            _ => return None,
        };
        let mut r = Rec {
            seq: word(4),
            phase,
            name: [0; 12],
            cluster: word(24),
            fit: [0; RECORD_LEN],
            ride: core::array::from_fn(|k| word(128 + 4 * k)),
            discard: s[22] == 1,
        };
        r.name.copy_from_slice(&s[9..21]);
        r.fit.copy_from_slice(&s[32..32 + RECORD_LEN]);
        Some(r)
    }
}

/// `<name>.fit` as text.
pub(super) fn file_name<'a>(name: &[u8; 12], out: &'a mut [u8; 16]) -> Result<&'a str, Error> {
    for (o, &c) in out.iter_mut().zip(name.iter().chain(b".fit")) {
        *o = c;
    }
    core::str::from_utf8(out).map_err(|_| Error::Corrupt)
}

/// Stock's ride name of a local time: `YYMMDDhhmmss`.
///
/// ```
/// assert_eq!(&bryton_rustfw::recorder::ride_name((2026, 10, 24, 7, 5, 9)), b"261024070509");
/// ```
pub fn ride_name((y, mo, d, h, mi, s): Local) -> [u8; 12] {
    let mut n = [b'0'; 12];
    for (pair, v) in n.as_chunks_mut::<2>().0.iter_mut().zip([(y % 100) as u8, mo, d, h, mi, s]) {
        *pair = [b'0' + (v / 10) % 10, b'0' + v % 10];
    }
    n
}

/// The name one second later (a name already taken). ponytail: the date does not roll over at midnight.
///
/// ```
/// # use bryton_rustfw::recorder::ride_name;
/// assert_eq!(bryton_rustfw::recorder::next_name(&ride_name((2026, 1, 2, 9, 59, 59))), ride_name((2026, 1, 2, 10, 0, 0)));
/// ```
pub fn next_name(n: &[u8; 12]) -> [u8; 12] {
    let digit = |i: usize| n.get(i).map_or(0, |c| c.saturating_sub(b'0') % 10);
    let v = |i: usize| digit(i) * 10 + digit(i + 1);
    let (mut h, mut m, mut s) = (v(6), v(8), v(10) + 1);
    if s >= 60 {
        (s, m) = (0, m + 1);
    }
    if m >= 60 {
        (m, h) = (0, (h + 1) % 24);
    }
    let mut out = *n;
    for (pair, x) in out[6..].as_chunks_mut::<2>().0.iter_mut().zip([h, m, s]) {
        *pair = [b'0' + x / 10, b'0' + x % 10];
    }
    out
}
