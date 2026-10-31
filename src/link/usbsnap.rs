//! What the USB core and its ULPI PHY looked like at the first failure of a link (2026-10-31, docs/usb.md "Next
//! tries" 1): high-speed bulk IN traffic drops on this board and the unit only restarts at the replug, so the board
//! reads a few registers at the first give-up or drop ([`Keeper`]) and the app keeps them in RTC backup registers
//! ([`pack`]), which survive the reset, for `tools/dev.py usbdrop` to decode (that tool mirrors this layout).
//!
//! PHY register numbers: ULPI Specification 1.1 register map (Function Control 0x04, Interface Control 0x07, USB
//! Interrupt Status 0x13 with HostDisconnect 0, VbusValid 1, SessValid 2, SessEnd 3, IdGnd 4, Debug 0x15 with
//! LineState in bits 1:0), from memory of the spec and the USB33x0 datasheet, not re-checked online ("inferred").

/// The failure that took the snapshot: `Msc::send` gave up after 1 s.
pub const EVENT_SEND: u8 = 1;
/// The failure that took the snapshot: the watch decided a drop.
pub const EVENT_DROP: u8 = 2;

/// The registers read at a failure. `phy` is `None` where the PHY never answered (a finding in itself).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsbSnap {
    /// [`EVENT_SEND`] or [`EVENT_DROP`].
    pub event: u8,
    /// ULPI registers 0x04, 0x13, 0x15, 0x07 (Function Control, USB Interrupt Status, Debug, Interface Control).
    pub phy: [Option<u8>; 4],
    /// OTG_GINTSTS (0x014).
    pub gintsts: u32,
    /// OTG_GOTGINT (0x004).
    pub gotgint: u32,
    /// OTG_DSTS (0x808).
    pub dsts: u32,
    /// OTG_DIEPINT1 (0x928).
    pub diepint1: u32,
    /// OTG_DTXFSTS1 (0x918).
    pub dtxfsts1: u32,
    /// OTG_DIEPCTL1 (0x920).
    pub diepctl1: u32,
}

/// Words of a packed snapshot: 22 bytes, 3 to a word behind the tag byte.
pub const WORDS: usize = 8;
/// A packed word's tag, bits 24..31.
pub const TAG: u32 = 0xd2;

/// The snapshot as bytes, the most telling first (fewer than [`WORDS`] registers may be free): 0 event (low nibble)
/// with which PHY registers answered (high nibble, in `phy` order), 1 PHY 0x04, 2 PHY 0x15, 3..4 GINTSTS bits 8..23,
/// 5 PHY 0x13, 6..8 DSTS (22 bits), 9 GINTSTS bits 0..7, 10 GINTSTS bits 24..31, 11..12 DIEPINT1, 13..14 DTXFSTS1,
/// 15..18 DIEPCTL1, 19..20 GOTGINT (16 bits), 21 PHY 0x07; little endian.
fn bytes(s: &UsbSnap) -> [u8; WORDS * 3] {
    let p = |i: usize| s.phy.get(i).copied().flatten().unwrap_or(0);
    let ok = (0..4).fold(0, |a, i| a | u8::from(s.phy.get(i).is_some_and(Option::is_some)) << i);
    let (g, d) = (s.gintsts.to_le_bytes(), s.dsts.to_le_bytes());
    let mut b = [0u8; WORDS * 3];
    let mut at = 0;
    let mut put = |v: &[u8]| {
        for (d, x) in b.iter_mut().skip(at).zip(v) {
            *d = *x;
        }
        at += v.len();
    };
    put(&[s.event & 15 | ok << 4, p(0), p(2), g[1], g[2], p(1), d[0], d[1], d[2], g[0], g[3]]);
    put(&s.diepint1.to_le_bytes()[..2]);
    put(&s.dtxfsts1.to_le_bytes()[..2]);
    put(&s.diepctl1.to_le_bytes());
    put(&s.gotgint.to_le_bytes()[..2]);
    put(&[p(3)]);
    b
}

/// The backup words: [`TAG`] in bits 24..31, three bytes of the snapshot below it (the private `bytes` lists them).
pub fn pack(s: &UsbSnap) -> [u32; WORDS] {
    let b = bytes(s);
    core::array::from_fn(|i| {
        let w = |k: usize| u32::from(b.get(3 * i + k).copied().unwrap_or(0)) << (8 * k);
        TAG << 24 | w(0) | w(1) | w(2)
    })
}

/// The snapshot back from words; `None` if the first is not tagged. Words that are missing or untagged read as
/// zero bytes (a build with fewer free registers stores a prefix).
#[cfg(test)]
pub fn unpack(words: &[u32]) -> Option<UsbSnap> {
    if words.first()? >> 24 != TAG {
        return None;
    }
    let mut b = [0u8; WORDS * 3];
    for (i, w) in words.iter().take(WORDS).enumerate().filter(|(_, w)| *w >> 24 == TAG) {
        for k in 0..3 {
            b[3 * i + k] = (w >> (8 * k)) as u8;
        }
    }
    let n = |at: usize, len: usize| (0..len).fold(0u32, |a, k| a | u32::from(b[at + k]) << (8 * k));
    let phy = |i: usize, at: usize| (b[0] >> 4 & (1 << i) != 0).then_some(b[at]);
    let g = |k: usize| u32::from(b[[9, 3, 4, 10][k]]) << (8 * k);
    Some(UsbSnap {
        event: b[0] & 15,
        phy: [phy(0, 1), phy(1, 5), phy(2, 2), phy(3, 21)],
        gintsts: g(0) | g(1) | g(2) | g(3),
        dsts: n(6, 3),
        diepint1: n(11, 2),
        dtxfsts1: n(13, 2),
        diepctl1: n(15, 4),
        gotgint: n(19, 2),
    })
}

/// Keeps the first snapshot of a link and hands it over once: a later failure must not overwrite what the first
/// showed (the first is the closest to the cause).
#[derive(Default)]
pub struct Keeper {
    snap: Option<UsbSnap>,
    taken: bool,
}

impl Keeper {
    /// A failure: `read` takes the snapshot (slow: the PHY reads), only for the first one.
    pub fn note(&mut self, read: impl FnOnce() -> UsbSnap) {
        if self.snap.is_none() && !self.taken {
            self.snap = Some(read());
        }
    }

    /// The snapshot, once.
    pub fn take(&mut self) -> Option<UsbSnap> {
        let s = self.snap.take();
        self.taken |= s.is_some();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UsbSnap {
        UsbSnap {
            event: EVENT_DROP,
            phy: [Some(0x45), Some(0x16), None, Some(0x01)],
            gintsts: 0x04f0_1030,
            gotgint: 0x0000_0404,
            dsts: 0x003f_ff06,
            diepint1: 0x2081,
            dtxfsts1: 0x0200,
            diepctl1: 0x8008_0240,
        }
    }

    #[test]
    fn pack_round_trips_and_tags_every_word() {
        let w = pack(&sample());
        assert!(w.iter().all(|w| w >> 24 == TAG));
        assert_eq!(w[0], 0xd2 << 24 | 0x45 << 8 | 0xb2, "event 2, PHY 0x04 0x13 0x07 answered, then 0x04");
        let mut s = sample();
        s.dsts &= 0x003f_ffff; // 22 bits
        assert_eq!(unpack(&w), Some(s));
        assert_eq!(unpack(&[0x1234]), None);
    }

    #[test]
    fn a_prefix_of_words_keeps_the_telling_registers() {
        let s = unpack(&pack(&sample())[..1]).unwrap();
        assert_eq!((s.event, s.phy[0], s.phy[2], s.gintsts), (EVENT_DROP, Some(0x45), None, 0));
    }

    #[test]
    fn three_words_reach_dsts() {
        let s = unpack(&pack(&sample())[..3]).unwrap();
        assert_eq!((s.gintsts, s.dsts, s.phy[1]), (0x04f0_1030 & 0x00ff_ff00, 0x003f_ff06, Some(0x16)));
    }

    #[test]
    fn the_keeper_keeps_the_first_and_hands_it_once() {
        let mut k = Keeper::default();
        k.note(sample);
        k.note(|| panic!("a second failure must not read again"));
        assert_eq!(k.take(), Some(sample()));
        k.note(|| panic!("nor after the hand over"));
        assert_eq!(k.take(), None);
    }
}
