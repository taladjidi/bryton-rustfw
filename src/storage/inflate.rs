//! gzip (RFC 1952) over deflate (RFC 1951), for the routing tiles of stock's map files (`storage::route`), written
//! here rather than taken from `miniz_oxide`.
//!
//! - **Why not `miniz_oxide`** (checked 2026-10-26, crates.io): well known and `no_std`, but its licence line is
//!   "MIT OR Zlib OR Apache-2.0" and it needs `adler2` (0BSD OR MIT OR Apache-2.0), where `deny.toml` allows MIT,
//!   Apache-2.0 and Unlicense; its decompressor wants the whole compressed tile in one slice (up to 47 KB more RAM)
//!   and carries an 11 KB state; and the dependency rule (AGENTS.md) asks for a measured need. This reader takes its
//!   input from an iterator (the card, a sector at a time, as `ign::lz4` does), its output goes to one buffer that
//!   is also the window, and it is about 180 lines and 1.5 KB of code.
//! - **Method:** the canonical Huffman decoder of Mark Adler's `puff.c` (zlib/contrib/puff, "a simple inflate written
//!   to be an unambiguous specification of the deflate format"): per bit, `count[len]` and a symbol list, no tables,
//!   a few instructions a bit. Slower than zlib's tables, and tiles are small (median 16 KB inflated).
//! - **Checks:** the gzip header (flags for the extra field, name, comment and header CRC skipped), the trailer's
//!   CRC-32 and size. Damaged input gives an [`Error`]: every index is checked, every loop ends with the finite
//!   input or the output buffer, and nothing panics (fuzzed in sim/fuzz.rs).
//! - zlib streams (header 0x78) are not read: every routing tile seen is gzip (STORAGE_AND_MAPS.md 4.7).

/// The output buffer: bytes in 16-bit words, the form of the SDRAM block main.rs hands out
/// (`hw::map_memory`), so no new `unsafe` is needed to get bytes out of it.
pub struct Scratch<'a>(pub &'a mut [u16]);

impl Scratch<'_> {
    /// Its size in bytes.
    pub fn len(&self) -> usize {
        self.0.len() * 2
    }

    /// There is no room.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Byte `i`.
    pub fn get(&self, i: usize) -> Option<u8> {
        let w = *self.0.get(i / 2)?;
        Some(if i.is_multiple_of(2) { w as u8 } else { (w >> 8) as u8 })
    }

    /// Appends byte `i`, the one after the last written (sequential output): an even byte is a whole new word, so
    /// only an odd one reads its word first: half the accesses of [`Scratch::set`] to uncached SDRAM. False when out
    /// of range.
    pub fn put(&mut self, i: usize, b: u8) -> bool {
        let Some(w) = self.0.get_mut(i / 2) else { return false };
        *w = if i.is_multiple_of(2) { u16::from(b) } else { *w & 0x00ff | u16::from(b) << 8 };
        true
    }

    /// Sets byte `i`; false when it is out of range.
    pub fn set(&mut self, i: usize, b: u8) -> bool {
        let Some(w) = self.0.get_mut(i / 2) else { return false };
        *w = if i.is_multiple_of(2) { *w & 0xff00 | u16::from(b) } else { *w & 0x00ff | u16::from(b) << 8 };
        true
    }
}

/// Why a stream could not be inflated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input ended inside the stream.
    Truncated,
    /// Not gzip, or not deflate data this reader takes (a code that is over-subscribed, a distance before the
    /// start, a reserved block type, a stored block whose length check fails).
    Bad,
    /// The output does not fit the buffer.
    TooBig,
    /// The trailer's CRC-32 or size does not match what came out.
    Crc,
}

/// Bits of the input, least significant first.
struct Bits<'a, I: Iterator<Item = u8>> {
    src: &'a mut I,
    buf: u32,
    cnt: u32,
    /// Bytes pulled from `src` through this reader.
    taken: u32,
}

impl<I: Iterator<Item = u8>> Bits<'_, I> {
    /// The next `n` (at most 16) bits.
    fn take(&mut self, n: u32) -> Result<u32, Error> {
        while self.cnt < n {
            self.buf |= u32::from(self.src.next().ok_or(Error::Truncated)?) << self.cnt;
            self.taken += 1;
            self.cnt += 8;
        }
        let v = self.buf & ((1 << n) - 1);
        (self.buf, self.cnt) = (self.buf >> n, self.cnt - n);
        Ok(v)
    }
}

/// A canonical Huffman code: how many codes there are of each length, and the symbols in code order.
struct Huffman {
    count: [u16; 16],
    symbol: [u16; 288],
}

impl Huffman {
    const EMPTY: Huffman = Huffman { count: [0; 16], symbol: [0; 288] };

    /// The code of `lens` (a length per symbol, 0: unused). Over-subscribed is an error; incomplete is allowed
    /// (decoding a code that was never assigned then fails).
    fn new(lens: &[u8]) -> Result<Huffman, Error> {
        let mut h = Huffman { count: [0; 16], symbol: [0; 288] };
        for &l in lens {
            *h.count.get_mut(usize::from(l)).ok_or(Error::Bad)? += 1;
        }
        let mut left: i32 = 1;
        for len in 1..16 {
            left = (left << 1) - i32::from(h.count[len]);
            if left < 0 {
                return Err(Error::Bad);
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + h.count[len];
        }
        for (sym, &l) in lens.iter().enumerate() {
            if l != 0 {
                let o = offs.get_mut(usize::from(l)).ok_or(Error::Bad)?;
                *h.symbol.get_mut(usize::from(*o)).ok_or(Error::Bad)? = sym as u16;
                *o += 1;
            }
        }
        Ok(h)
    }

    /// The next symbol, bit by bit as puff does.
    fn decode<I: Iterator<Item = u8>>(&self, b: &mut Bits<'_, I>) -> Result<u16, Error> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.take(1)? as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return self.symbol.get((index + (code - first)) as usize).copied().ok_or(Error::Bad);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Bad)
    }
}

/// Length codes 257..285: base length and extra bits (RFC 1951, 3.2.5).
const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// Distance codes 0..29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// The fixed codes (RFC 1951, 3.2.6).
fn fixed() -> Result<(Huffman, Huffman), Error> {
    let mut l = [0u8; 288];
    for (i, v) in l.iter_mut().enumerate() {
        *v = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    Ok((Huffman::new(&l)?, Huffman::new(&[5; 30])?))
}

/// The codes a dynamic block sends first (RFC 1951, 3.2.7).
fn dynamic<I: Iterator<Item = u8>>(b: &mut Bits<'_, I>) -> Result<(Huffman, Huffman), Error> {
    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
    let nlen = b.take(5)? as usize + 257;
    let ndist = b.take(5)? as usize + 1;
    let ncode = b.take(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(Error::Bad);
    }
    let mut lens = [0u8; 320];
    for &o in ORDER.iter().take(ncode) {
        *lens.get_mut(o).ok_or(Error::Bad)? = b.take(3)? as u8;
    }
    let code_code = Huffman::new(lens.get(..19).ok_or(Error::Bad)?)?;
    lens = [0; 320];
    let mut i = 0;
    while i < nlen + ndist {
        let sym = code_code.decode(b)?;
        let (value, rep) = match sym {
            0..=15 => (sym as u8, 1),
            16 => (*lens.get(i.checked_sub(1).ok_or(Error::Bad)?).ok_or(Error::Bad)?, 3 + b.take(2)? as usize),
            17 => (0, 3 + b.take(3)? as usize),
            _ => (0, 11 + b.take(7)? as usize),
        };
        if i + rep > nlen + ndist {
            return Err(Error::Bad);
        }
        for l in lens.get_mut(i..i + rep).ok_or(Error::Bad)? {
            *l = value;
        }
        i += rep;
    }
    if lens.get(256) == Some(&0) {
        return Err(Error::Bad); // no end-of-block code
    }
    Ok((
        Huffman::new(lens.get(..nlen).ok_or(Error::Bad)?)?,
        Huffman::new(lens.get(nlen..nlen + ndist).ok_or(Error::Bad)?)?,
    ))
}

/// CRC-32 (the gzip one, polynomial 0xedb88320) a nibble at a time: 16 words of table.
const CRC_NIBBLE: [u32; 16] = {
    let mut t = [0u32; 16];
    let mut i = 0;
    while i < 16 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 4 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CRC-32 of `bytes` (the gzip trailer's).
///
/// ```
/// assert_eq!(bryton_rustfw::inflate::crc32(b"123456789".iter().copied()), 0xcbf4_3926);
/// ```
pub fn crc32(bytes: impl Iterator<Item = u8>) -> u32 {
    !bytes.fold(!0u32, crc_byte)
}

/// One byte into a running CRC-32 (start at `!0`, finish with `!`): computed as the bytes are written, so the
/// output is not read back a second time (it lives in uncached SDRAM).
fn crc_byte(c: u32, b: u8) -> u32 {
    let c = c ^ u32::from(b);
    let c = CRC_NIBBLE[(c & 15) as usize] ^ (c >> 4);
    CRC_NIBBLE[(c & 15) as usize] ^ (c >> 4)
}

/// Where an [`Inflater`] is in the gzip member.
#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    /// The gzip header.
    Head,
    /// A deflate block's header.
    Block,
    /// A stored block with this many bytes left.
    Stored(u32),
    /// A coded block's symbols.
    Codes,
    /// The CRC-32 and size.
    Trailer,
    Done,
}

/// A gzip member inflated in slices: [`Inflater::step`] stops at a symbol boundary once `budget` bytes came out and
/// the next call goes on, with the input from [`Inflater::taken`] bytes on (the caller may open it afresh: a sector
/// a pass). The state is the bit buffer, the block and its two codes (about 1.2 KB each); the window is `out`.
pub struct Inflater {
    st: St,
    buf: u32,
    cnt: u32,
    n: usize,
    crc: u32,
    last: bool,
    lit: Huffman,
    dist: Huffman,
    /// Input bytes used so far (the bit buffer's included): where to read on from.
    pub taken: u32,
}

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Inflater {
    /// At the start of a member.
    pub const fn new() -> Inflater {
        Inflater {
            st: St::Head,
            buf: 0,
            cnt: 0,
            n: 0,
            crc: 0, // set to !0 by the header: all-zero, so a static of it stays in .bss
            last: false,
            lit: Huffman::EMPTY,
            dist: Huffman::EMPTY,
            taken: 0,
        }
    }

    /// Bytes written so far.
    pub fn written(&self) -> usize {
        self.n
    }

    /// Goes on with `src` (the bytes after [`Inflater::taken`]), at least one symbol and until `budget` more bytes
    /// are in `out`: Some(length) when the member is whole and its trailer right, None when there is more. After an
    /// error the inflater is of no more use.
    pub fn step(
        &mut self,
        src: &mut impl Iterator<Item = u8>,
        out: &mut Scratch<'_>,
        budget: usize,
    ) -> Result<Option<usize>, Error> {
        let mut b = Bits { src, buf: self.buf, cnt: self.cnt, taken: 0 };
        let r = self.run(&mut b, out, self.n.saturating_add(budget));
        (self.buf, self.cnt, self.taken) = (b.buf, b.cnt, self.taken.saturating_add(b.taken));
        r
    }

    fn run<I: Iterator<Item = u8>>(
        &mut self,
        b: &mut Bits<'_, I>,
        out: &mut Scratch<'_>,
        stop: usize,
    ) -> Result<Option<usize>, Error> {
        loop {
            match self.st {
                St::Head => {
                    self.crc = !0;
                    let (id1, id2, cm, flags) = (b.take(8)?, b.take(8)?, b.take(8)?, b.take(8)?);
                    if (id1, id2, cm) != (0x1f, 0x8b, 8) || flags & 0xe0 != 0 {
                        return Err(Error::Bad);
                    }
                    for _ in 0..6 {
                        b.take(8)?; // mtime, xfl, os
                    }
                    if flags & 4 != 0 {
                        let len = b.take(8)? | b.take(8)? << 8;
                        for _ in 0..len {
                            b.take(8)?;
                        }
                    }
                    for bit in [8, 16] {
                        if flags & bit != 0 {
                            while b.take(8)? != 0 {} // a zero-terminated name or comment (the input is finite)
                        }
                    }
                    if flags & 2 != 0 {
                        b.take(16)?;
                    }
                    self.st = St::Block;
                }
                St::Block => {
                    self.last = b.take(1)? == 1;
                    match b.take(2)? {
                        0 => {
                            // stored: to the byte boundary, LEN, NLEN, then LEN bytes
                            (b.buf, b.cnt) = (0, 0);
                            let len = b.take(16)?;
                            if b.take(16)? != !len & 0xffff {
                                return Err(Error::Bad);
                            }
                            self.st = St::Stored(len);
                        }
                        t @ (1 | 2) => {
                            (self.lit, self.dist) = if t == 1 { fixed()? } else { dynamic(b)? };
                            self.st = St::Codes;
                        }
                        _ => return Err(Error::Bad),
                    }
                }
                St::Stored(left) => {
                    let mut left = left;
                    while left > 0 && self.n < stop {
                        let v = b.take(8)? as u8;
                        if !out.put(self.n, v) {
                            return Err(Error::TooBig);
                        }
                        self.crc = crc_byte(self.crc, v);
                        (self.n, left) = (self.n + 1, left - 1);
                    }
                    self.st = if left > 0 {
                        St::Stored(left)
                    } else if self.last {
                        St::Trailer
                    } else {
                        St::Block
                    };
                    if left > 0 {
                        return Ok(None);
                    }
                }
                St::Codes => {
                    // at least one symbol, so a budget of 0 still goes on
                    loop {
                        let sym = self.lit.decode(b)? as usize;
                        if sym < 256 {
                            if !out.put(self.n, sym as u8) {
                                return Err(Error::TooBig);
                            }
                            self.crc = crc_byte(self.crc, sym as u8);
                            self.n += 1;
                        } else if sym == 256 {
                            self.st = if self.last { St::Trailer } else { St::Block };
                            break;
                        } else {
                            let s = sym - 257;
                            let len = usize::from(*LEN_BASE.get(s).ok_or(Error::Bad)?)
                                + b.take(u32::from(*LEN_EXTRA.get(s).ok_or(Error::Bad)?))? as usize;
                            let d = self.dist.decode(b)? as usize;
                            let back = usize::from(*DIST_BASE.get(d).ok_or(Error::Bad)?)
                                + b.take(u32::from(*DIST_EXTRA.get(d).ok_or(Error::Bad)?))? as usize;
                            if back > self.n {
                                return Err(Error::Bad);
                            }
                            for _ in 0..len {
                                let v = out.get(self.n - back).ok_or(Error::Bad)?;
                                if !out.put(self.n, v) {
                                    return Err(Error::TooBig);
                                }
                                self.crc = crc_byte(self.crc, v);
                                self.n += 1;
                            }
                        }
                        if self.n >= stop {
                            return Ok(None);
                        }
                    }
                }
                St::Trailer => {
                    // starts at a byte boundary: whole bytes the bit buffer still holds come first
                    let mut t = [0u8; 8];
                    for v in t.iter_mut() {
                        *v = if b.cnt >= 8 {
                            let v = b.buf as u8;
                            (b.buf, b.cnt) = (b.buf >> 8, b.cnt - 8);
                            v
                        } else {
                            b.taken += 1;
                            b.src.next().ok_or(Error::Truncated)?
                        };
                    }
                    let (crc, size) =
                        (u32::from_le_bytes([t[0], t[1], t[2], t[3]]), u32::from_le_bytes([t[4], t[5], t[6], t[7]]));
                    if size as usize != self.n || crc != !self.crc {
                        return Err(Error::Crc);
                    }
                    self.st = St::Done;
                }
                St::Done => return Ok(Some(self.n)),
            }
        }
    }
}

/// Inflates the gzip member in `src` into `out` in one go: the length written. The trailer's CRC and size are checked.
pub fn gunzip(src: &mut impl Iterator<Item = u8>, out: &mut Scratch<'_>) -> Result<usize, Error> {
    let mut inf = Inflater::new();
    loop {
        if let Some(n) = inf.step(src, out, usize::MAX)? {
            return Ok(n);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// Python's gzip (level 9, mtime 0) of a short repeated text: one fixed-code block.
    const FIXED: &str = "1f8b08000000000002ff4b4c4a4e44450a19a93939f9c82400aed86c3b24000000";
    /// Level 0: a stored block of the bytes 0 to 39.
    const STORED: &str = "1f8b08000000000000ff012800d7ff000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20212223242526273c2ea60d28000000";
    /// Level 9 of `dynamic_text`: one dynamic-code block with matches.
    const DYNAMIC: &str = "1f8b08000000000002ff65964b6e04210c442f9405c60d34c7c929a2dc3e926d512fcc06316ef0a75c2ee6e7fbd7deaf9fefdf6eb1c6de67ec63196108b3ad58c3d0e2e31386386d61719dce8f9e6b57883c9837bbee67e4da4f7dad4b3bf669d9ca6ee9f8d0b629727a7d1552de7c286efa69c7b1fb399c75e48d58cd2ec70957a1a38f0e4f05c00330a722442206bcb25073c568c76d475de3a486df792d96cee0b14eb94c1cdd4ed1760e38f19872d7902eb028fb7bc1be65054e191054287f4da956b71f7529e37c306a00b27660aad4e64da80e8b119f8bd6ccc440e806563e6a59d53d541cdbdcaf6a93261d8d5f383dc46a579c0ec66d04ee0ae6aa7ec875f1e83d8dad4e35b119d35450f7ff84c86ca6e0016dd016346b4343e07d017de849b1f3850888a80b62b334a82cee1eb8a2f5a1c7161587f2a8e11798d5693976005b1d9937cdb31a571b6b6441b152012521c6b1c4053971d81f95049eac5ba8e4b5b079e06328daf85005bbc37428dbd03818a24dc0d680f906c1d54992b0f4c2ef1cd1e747838d092a7f50a47ef1b684d67082d54e3d012fba037501933a6b8632146b80530da874e4032187dc60260c6a22080c4fc12b862e800f4256deb8c9040d305d0fa05cefbb2b1369b40f861105880841b7eb75f6fb7f45b17c8b8413060d6ee700e2084ba7c83cc8901ea12cbd7cfd01c8677cbbf6080000";

    fn dynamic_text() -> Vec<u8> {
        let (mut x, mut v) = (7u32, Vec::new());
        for _ in 0..400 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            v.extend_from_slice(std::format!("way{},", (x >> 20) % 40).as_bytes());
        }
        v
    }

    fn run(gz: &[u8], words: usize) -> Result<Vec<u8>, Error> {
        let mut mem = std::vec![0u16; words];
        let mut out = Scratch(&mut mem);
        let n = gunzip(&mut gz.iter().copied(), &mut out)?;
        Ok((0..n).map(|i| out.get(i).unwrap()).collect())
    }

    /// The three block types, against Python's gzip, byte for byte.
    #[test]
    fn inflates_what_python_deflated() {
        assert_eq!(run(&unhex(FIXED), 100).unwrap(), b"abcabcabcabcabcabc hello hello hello");
        assert_eq!(run(&unhex(STORED), 100).unwrap(), (0..40).collect::<Vec<u8>>());
        assert_eq!(run(&unhex(DYNAMIC), 4000).unwrap(), dynamic_text());
    }

    /// The optional header fields are skipped: a file name here.
    #[test]
    fn skips_the_header_fields() {
        let mut g = unhex(STORED);
        g[3] = 8;
        g.splice(10..10, *b"tile.pb\0");
        assert_eq!(run(&g, 100).unwrap(), (0..40).collect::<Vec<u8>>());
        g[3] = 4; // an extra field of 3 bytes instead
        g.splice(10..18, [3, 0, 9, 9, 9]);
        assert_eq!(run(&g, 100).unwrap(), (0..40).collect::<Vec<u8>>());
    }

    /// Not enough room, a short input, a wrong trailer: errors, never a panic or a wrong answer.
    #[test]
    fn refuses_what_is_damaged() {
        let g = unhex(DYNAMIC);
        assert_eq!(run(&g, 1000), Err(Error::TooBig), "2294 bytes in 2000");
        assert_eq!(run(&g[..g.len() - 1], 4000), Err(Error::Truncated));
        let mut bad = g.clone();
        let n = bad.len();
        bad[n - 8] ^= 1; // the CRC
        assert_eq!(run(&bad, 4000), Err(Error::Crc));
        bad = g.clone();
        bad[n - 4] ^= 1; // the size
        assert_eq!(run(&bad, 4000), Err(Error::Crc));
        assert_eq!(run(b"\x1f\x8b\x09\0\0\0\0\0\0\0", 10), Err(Error::Bad), "not deflate");
        assert_eq!(run(b"PK\x03\x04", 10), Err(Error::Bad));
        assert_eq!(run(&[], 10), Err(Error::Truncated));
        // a stored block whose NLEN is wrong; a reserved block type
        let mut s = unhex(STORED);
        s[14] ^= 1;
        assert_eq!(run(&s, 100), Err(Error::Bad));
        assert_eq!(run(b"\x1f\x8b\x08\0\0\0\0\0\0\0\x07", 10), Err(Error::Bad));
        // a distance before the start of the output: fixed block, length code 257, distance code 0 at n = 0
        assert_eq!(run(b"\x1f\x8b\x08\0\0\0\0\0\0\0\x03\x02\x00", 10), Err(Error::Bad));
    }

    /// Every cut of a good stream and every flipped bit: an error, or (a flip in the time stamp) the same bytes.
    #[test]
    fn survives_cuts_and_flips() {
        let g = unhex(DYNAMIC);
        let want = dynamic_text();
        for cut in 0..g.len() {
            assert!(run(&g[..cut], 4000).is_err(), "cut at {cut}");
        }
        for i in 0..g.len() * 8 {
            let mut b = g.clone();
            b[i / 8] ^= 1 << (i % 8);
            if let Ok(v) = run(&b, 4000) {
                assert_eq!(v, want, "bit {i}");
            }
        }
    }

    /// Codes that cannot be: over-subscribed lengths are refused, an incomplete code decodes until a missing one.
    #[test]
    fn huffman_codes() {
        assert!(Huffman::new(&[1, 1, 1]).is_err(), "three codes of one bit");
        assert!(Huffman::new(&[16]).is_err(), "longer than 15 bits");
        let h = Huffman::new(&[2, 1, 0, 2]).unwrap(); // codes 10, 0, 11 in the canonical order of the lengths
        let mut it = [0b0000_0110u8].into_iter(); // bits, least significant first: 0, 1, 1, 0
        let mut b = Bits { src: &mut it, buf: 0, cnt: 0, taken: 0 };
        assert_eq!((h.decode(&mut b), h.decode(&mut b), h.decode(&mut b)), (Ok(1), Ok(3), Ok(1)));
    }
}
