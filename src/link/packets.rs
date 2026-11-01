//! Which packets a bulk IN reply is sent in, as pure logic: packets of the bus's size, and a zero-length packet when a
//! reply ends on a full one (USB 2.0, 5.8.3: the host cannot tell a transfer ended on a full packet from one still
//! coming). Seen 2026-11-01: a 256 B `info` reply at full speed (4 x 64 B) was held until the next packet.

/// The sizes of the packets of one write, in order (one empty packet for no bytes).
#[derive(Clone, Copy)]
pub struct Packets {
    left: usize,
    mps: usize,
    zero: bool,
}

impl Iterator for Packets {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        if self.left == 0 {
            return core::mem::take(&mut self.zero).then_some(0);
        }
        let n = self.left.min(self.mps);
        self.left -= n;
        Some(n)
    }
}

/// The transmit side: remembers whether the last write ended on a full packet.
pub struct Tx {
    mps: usize,
    last_full: bool,
}

impl Tx {
    /// A transmitter for packets of `mps` bytes (not 0).
    pub const fn new(mps: usize) -> Self {
        Tx { mps, last_full: false }
    }

    /// The packets to send for `len` bytes; `len` 0 is one empty packet.
    pub fn write(&mut self, len: usize) -> Packets {
        self.last_full = len > 0 && len.is_multiple_of(self.mps);
        Packets { left: len, mps: self.mps, zero: len == 0 }
    }

    /// Nothing could be sent (nobody reading): the next flush owes no empty packet.
    pub fn drop_output(&mut self) {
        self.last_full = false;
    }

    /// Whether a flush must send an empty packet now to end the transfer.
    pub fn needs_zlp(&self) -> bool {
        self.last_full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packet sizes sent for `len` bytes then a flush, as `Link::flush` does.
    struct Sent {
        sizes: [usize; 40],
        n: usize,
    }

    impl Sent {
        fn all(&self) -> &[usize] {
            &self.sizes[..self.n]
        }
    }

    fn reply(tx: &mut Tx, len: usize) -> Sent {
        let mut s = Sent { sizes: [0; 40], n: 0 };
        let mut push = |p: usize| {
            s.sizes[s.n] = p;
            s.n += 1;
        };
        if len > 0 {
            tx.write(len).for_each(&mut push);
        }
        if tx.needs_zlp() {
            tx.write(0).for_each(&mut push);
        }
        s
    }

    #[test]
    fn zlp_exactly_when_the_last_packet_is_full() {
        for mps in [64, 512] {
            for len in [0, 1, 63, 64, 65, 128, 255, 256, 512, 513, 1024] {
                let v = reply(&mut Tx::new(mps), len);
                let v = v.all();
                assert_eq!(v.iter().sum::<usize>(), len);
                assert!(v.iter().all(|&n| n <= mps));
                let full = len > 0 && len % mps == 0;
                assert_eq!(v.last() == Some(&0), full, "mps {mps} len {len}: {v:?}");
                assert_eq!(v.len(), len.div_ceil(mps) + usize::from(full));
            }
        }
    }

    #[test]
    fn examples() {
        assert_eq!(reply(&mut Tx::new(64), 256).all(), [64, 64, 64, 64, 0]);
        assert_eq!(reply(&mut Tx::new(64), 65).all(), [64, 1]);
        assert_eq!(reply(&mut Tx::new(512), 512).all(), [512, 0]);
        assert!(reply(&mut Tx::new(64), 0).all().is_empty());
    }

    #[test]
    fn replies_back_to_back_and_a_second_flush() {
        let mut tx = Tx::new(64);
        assert_eq!(reply(&mut tx, 64).all(), [64, 0]);
        // nothing buffered after the empty packet: no second one
        assert!(reply(&mut tx, 0).all().is_empty());
        assert_eq!(reply(&mut tx, 63).all(), [63]);
        assert_eq!(reply(&mut tx, 128).all(), [64, 64, 0]);
    }

    #[test]
    fn dropped_output_owes_nothing() {
        let mut tx = Tx::new(64);
        let _ = tx.write(64);
        tx.drop_output();
        assert!(!tx.needs_zlp());
    }
}
