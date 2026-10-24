//! IGN map packs: Plan IGN v2 raster tiles (Licence Ouverte / Etalab 2.0: "(c) IGN" on screen) made by
//! `tools/ign_pack.py` from the tiles of data.geopf.fr, copied by the owner to `\Rustfw\Maps\` (read only here).
//!
//! **Format** (version 1, little-endian), one file per zoom and aligned block of 32 x 32 Web Mercator XYZ tiles,
//! `IGN<z>_<x / 32>_<y / 32>.PAK`: a file stays far below FAT's 4 GB, the volume needs no 165,000 small files, and
//! reaching a tile walks at most a block's cluster chain (about 25 MB: 1600 clusters of 16 KB, 13 FAT sectors).
//! - 0: magic `IGNP`; 4: version u16 = 1; 6: zoom u8; 7: block u8 = 32; 8: x0 u32, 12: y0 u32, the block's first
//!   tile (x0 = 32 (x / 32)).
//! - 16: the index, 32 x 32 entries (offset u32, length u32), row-major, entry i at 16 + 8 i: never across a
//!   sector, so a tile is found with one sector read. Offset 0: no tile.
//! - Each tile: n - 1 (u8), n RGB565 colours (u16, n <= 256), then its 256 x 256 colour indices (rows top down, a
//!   byte a pixel) as one LZ4 block of 65536 bytes.
//!
//! **LZ4** ([`lz4`]): the block format, github.com/lz4/lz4 doc/lz4_Block_format.md: a token (literal length in the
//! high 4 bits, match length - 4 in the low 4; 15 means more length bytes follow, each added, until one below
//! 255), the literals, a 2-byte little-endian offset (0 is invalid), the match copied a byte at a time in
//! increasing order (it may overlap what it writes), the last sequence literals only. Written here rather than
//! taken from `lz4_flex` 0.11: its safe decoder wants the whole input in one slice (66 KB that this firmware would
//! have to hold) and decodes to bytes, where this one pulls bytes through an iterator from the card and writes
//! palette colours straight into the tile's pixels; about 40 lines, every length checked against the output.

use crate::fat::{self, Disk, Fat, File, Seek};
use crate::text::Line;

/// The packs' folder on the volume.
pub const DIR: &str = "Rustfw/Maps";
/// A tile's side, pixels.
pub const SIDE: usize = 256;
/// A tile's pixels.
pub const TILE: usize = SIDE * SIDE;
/// Tiles a side of a pack's block.
pub const BLOCK: u32 = 32;
const HEAD: u32 = 16;
/// The index's end: the first tile byte.
const DATA: u32 = HEAD + 8 * BLOCK * BLOCK;
/// A tile at most: the count, 256 colours, and LZ4's worst case for 65536 bytes (n + n / 255 + 16).
const BLOB_MAX: u32 = 1 + 512 + 65_536 + 65_536 / 255 + 16;

/// The pack holding tile (`z`, `x`, `y`): `IGN<z>_<x / 32>_<y / 32>.PAK`.
pub fn name(z: u8, x: u32, y: u32) -> Line<32> {
    let mut l = Line::new();
    l.push_str("IGN");
    l.push_u32(u32::from(z));
    l.push_str("_");
    l.push_u32(x / BLOCK);
    l.push_str("_");
    l.push_u32(y / BLOCK);
    l.push_str(".PAK");
    l
}

/// The header is a version 1 pack of tile (`z`, `x`, `y`)'s block.
pub fn header_ok(h: &[u8; HEAD as usize], z: u8, x: u32, y: u32) -> bool {
    let u32_at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    h[..4] == *b"IGNP"
        && h[4..6] == [1, 0]
        && h[6] == z
        && u32::from(h[7]) == BLOCK
        && u32_at(8) == x / BLOCK * BLOCK
        && u32_at(12) == y / BLOCK * BLOCK
}

/// Where tile (`x`, `y`)'s index entry is in its pack.
pub fn entry_at(x: u32, y: u32) -> u32 {
    HEAD + 8 * ((y % BLOCK) * BLOCK + x % BLOCK)
}

/// The tile's (offset, length) from its index entry in a pack of `size` bytes; None: no tile there, or an entry
/// that cannot be right.
pub fn entry(e: [u8; 8], size: u32) -> Option<(u32, u32)> {
    let (off, len) = (u32::from_le_bytes([e[0], e[1], e[2], e[3]]), u32::from_le_bytes([e[4], e[5], e[6], e[7]]));
    (off >= DATA && (4..=BLOB_MAX).contains(&len) && off.checked_add(len).is_some_and(|end| end <= size))
        .then_some((off, len))
}

/// Decodes a tile from `src` (its bytes, from the colour count on) into `out` as RGB565: true when it is whole.
pub fn decode(src: &mut impl Iterator<Item = u8>, out: &mut [u16]) -> bool {
    let Some(n) = src.next() else { return false };
    let mut pal = [0u16; 256];
    for p in pal.iter_mut().take(usize::from(n) + 1) {
        match (src.next(), src.next()) {
            (Some(lo), Some(hi)) => *p = u16::from_le_bytes([lo, hi]),
            _ => return false,
        }
    }
    let Some(out) = out.get_mut(..TILE) else { return false };
    lz4(src, out, |i| pal[usize::from(i)]) == Some(TILE)
}

/// Decodes one LZ4 block from `src` into `out`, each literal byte through `map`: the length decoded, None when the
/// block is damaged (an offset of 0 or before the start, a length past `out`, the input ending inside a
/// sequence). Never reads or writes out of bounds; every loop ends within `out.len()` steps or the input's end.
///
/// ```
/// // "abcabcabca": 3 literals, then a match of 7 at offset 3 (overlapping), then the last literal
/// let block = [0x33, b'a', b'b', b'c', 3, 0, 0x10, b'a'];
/// let mut out = [0u8; 16];
/// assert_eq!(bryton_rustfw::ign::lz4(&mut block.into_iter(), &mut out, |b| b), Some(11));
/// assert_eq!(&out[..11], b"abcabcabcaa");
/// ```
pub fn lz4<T: Copy>(src: &mut impl Iterator<Item = u8>, out: &mut [T], map: impl Fn(u8) -> T) -> Option<usize> {
    let mut n: usize = 0;
    loop {
        let token = src.next()?;
        let lit = length(src, usize::from(token >> 4), out.len())?;
        for d in out.get_mut(n..n.checked_add(lit)?)? {
            *d = map(src.next()?);
        }
        n += lit;
        let Some(lo) = src.next() else { return Some(n) }; // the last sequence: literals only
        let off = usize::from(lo) | usize::from(src.next()?) << 8;
        let len = length(src, usize::from(token & 15), out.len())? + 4;
        if off == 0 || off > n || len > out.len() - n {
            return None;
        }
        for i in n..n + len {
            *out.get_mut(i)? = *out.get(i - off)?;
        }
        n += len;
    }
}

/// A sequence's length from its 4-bit field `n`: 15 and more bytes added; None past `limit` or at the input's end.
fn length(src: &mut impl Iterator<Item = u8>, mut n: usize, limit: usize) -> Option<usize> {
    if n == 15 {
        loop {
            let b = src.next()?;
            n += usize::from(b);
            if n > limit + 15 {
                return None; // more than any output: damaged (and this bounds the loop)
            }
            if b != 255 {
                break;
            }
        }
    }
    Some(n)
}

/// A file's bytes from an offset to an end, read through `buf` (the LZ4 decoder's input). A read error ends the
/// bytes and stays in `error`.
pub struct Bytes<'a, D: Disk> {
    fat: &'a mut Fat,
    d: &'a mut D,
    file: File,
    seek: &'a mut Seek,
    pos: u32,
    end: u32,
    buf: &'a mut [u8],
    at: usize,
    len: usize,
    /// The read that failed, if one did.
    pub error: Option<fat::Error>,
    /// Bytes read from the file.
    pub read: u32,
}

impl<'a, D: Disk> Bytes<'a, D> {
    /// Bytes `from..from + len` of `file`; `seek` is `file`'s ([`Fat::read_from`]).
    pub fn new(
        fat: &'a mut Fat,
        d: &'a mut D,
        file: File,
        seek: &'a mut Seek,
        (from, len): (u32, u32),
        buf: &'a mut [u8],
    ) -> Self {
        let end = from.saturating_add(len);
        Bytes { fat, d, file, seek, pos: from, end, buf, at: 0, len: 0, error: None, read: 0 }
    }
}

impl<D: Disk> Iterator for Bytes<'_, D> {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        if self.at >= self.len {
            if self.pos >= self.end || self.error.is_some() {
                return None;
            }
            let want = ((self.end - self.pos) as usize).min(self.buf.len());
            let buf = self.buf.get_mut(..want)?;
            match self.fat.read_from(self.d, &self.file, self.pos, buf, self.seek) {
                Ok(n) if n > 0 => {
                    (self.at, self.len, self.pos, self.read) = (0, n, self.pos + n as u32, self.read + n as u32)
                }
                Ok(_) => return None,
                Err(e) => {
                    self.error = Some(e);
                    return None;
                }
            }
        }
        let b = self.buf.get(self.at).copied();
        self.at += 1;
        b
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The synthetic packs `tools/ign_pack.py fixture` made (test-data/ign: no map, a formula), and their names.
    pub fn fixture() -> Vec<(std::string::String, Vec<u8>)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test-data/ign");
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name().into_string().unwrap(), std::fs::read(e.path()).unwrap())
            })
            .collect();
        v.sort();
        v
    }

    /// The synthetic tile's colour at its pixel (`i`, `j`): `synthetic` in tools/ign_pack.py.
    pub fn synthetic(z: u8, x: u32, y: u32, i: u32, j: u32) -> u16 {
        const RGB: [(u32, u32, u32); 5] =
            [(250, 248, 240), (160, 160, 160), (230, 120, 40), (90, 170, 220), (20, 20, 20)];
        let (gx, gy) = (x * 256 + i, y * 256 + j);
        let mut c = 0;
        if gx % 64 == 0 || gy % 64 == 0 {
            c = 1;
        }
        if (gx + gy) % 97 < 6 {
            c = 2;
        }
        let r = 20 + (x + y + u32::from(z)) % 3 * 10;
        if (i as i32 - 128).pow(2) + (j as i32 - 128).pow(2) < (r * r) as i32 {
            c = 3;
        }
        if i == j {
            c = 4;
        }
        let (r, g, b) = RGB[c];
        ((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16
    }

    /// A tile of a pack in memory: its blob.
    fn blob(pack: &[u8], x: u32, y: u32) -> Option<&[u8]> {
        let at = entry_at(x, y) as usize;
        let (off, len) = entry(pack[at..at + 8].try_into().unwrap(), pack.len() as u32)?;
        Some(&pack[off as usize..(off + len) as usize])
    }

    /// The python packer's output (python-lz4, high compression) decodes to the formula, pixel for pixel; the index
    /// finds every tile of the block and the one left out is missing.
    #[test]
    fn decodes_the_packers_tiles() {
        let packs = fixture();
        let (name16, pack) = packs.iter().find(|(n, _)| n == "IGN16_1024_1024.PAK").unwrap();
        let h = 1u32 << 15;
        assert_eq!(name16.as_str(), name(16, h, h).as_str());
        assert!(header_ok(pack[..16].try_into().unwrap(), 16, h, h + 1));
        assert!(!header_ok(pack[..16].try_into().unwrap(), 15, h, h));
        assert!(!header_ok(pack[..16].try_into().unwrap(), 16, h + 32, h));
        let mut out = std::vec![0u16; TILE];
        for (x, y) in [(h, h), (h + 1, h), (h + 1, h + 1)] {
            let b = blob(pack, x, y).unwrap();
            assert!(decode(&mut b.iter().copied(), &mut out), "{x},{y}");
            for j in 0..256 {
                for i in 0..256 {
                    assert_eq!(out[(j * 256 + i) as usize], synthetic(16, x, y, i, j), "{x},{y} at {i},{j}");
                }
            }
            // cut short: not whole
            assert!(!decode(&mut b[..b.len() - 1].iter().copied(), &mut out));
        }
        assert!(blob(pack, h, h + 1).is_none(), "the tile left out");
        assert!(blob(pack, h + 5, h + 5).is_none(), "outside the fixture");
        // every pack's header names its own block
        for (n, p) in &packs {
            let z: u8 = n[3..5].parse().unwrap();
            let k = 1u32 << (z - 1);
            assert!([(k - 1, k - 1), (k, k - 1), (k - 1, k), (k, k)].iter().any(|&(x, y)| {
                name(z, x, y).as_str() == n.as_str() && header_ok(p[..16].try_into().unwrap(), z, x, y)
            }));
        }
    }

    /// Every tile of real packs decodes whole: the bytes a tile and the decode time (on the computer).
    /// Ignored: IGN tiles of a real area stay outside the repository (AGENTS.md, "Public repository"); point
    /// IGN_PACKS at `tools/ign_pack.py pack`'s output (`cargo test-host every_packed_tile -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn every_packed_tile() {
        let Ok(dir) = std::env::var("IGN_PACKS") else {
            std::println!("IGN_PACKS not set: skipped");
            return;
        };
        let mut out = std::vec![0u16; TILE];
        let (mut n, mut bytes, mut t) = (0u64, 0u64, std::time::Duration::ZERO);
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = e.file_name().into_string().unwrap();
            if !name.ends_with(".PAK") {
                continue;
            }
            let pack = std::fs::read(e.path()).unwrap();
            let (z, x, y) = {
                let v: Vec<u32> = name[3..name.len() - 4].split('_').map(|s| s.parse().unwrap()).collect();
                (v[0] as u8, v[1] * BLOCK, v[2] * BLOCK)
            };
            assert!(header_ok(pack[..16].try_into().unwrap(), z, x, y), "{name}");
            for j in 0..BLOCK {
                for i in 0..BLOCK {
                    let Some(b) = blob(&pack, x + i, y + j) else { continue };
                    let t0 = std::time::Instant::now();
                    assert!(decode(&mut b.iter().copied(), &mut out), "{name} {i},{j}");
                    t += t0.elapsed();
                    (n, bytes) = (n + 1, bytes + b.len() as u64);
                }
            }
        }
        std::println!(
            "{n} tiles, {} bytes a tile, {} us a tile to decode",
            bytes / n.max(1),
            t.as_micros() as u64 / n.max(1)
        );
        assert!(n > 0);
    }

    #[test]
    fn lz4_edge_cases() {
        let mut out = [0u8; 8];
        let id = |b| b;
        assert_eq!(lz4(&mut [0x00].into_iter(), &mut out, id), Some(0), "the empty block");
        assert_eq!(lz4(&mut [].into_iter(), &mut out, id), None, "no token");
        // offset 0, offset before the start, a match past the end, a literal run past the end
        assert_eq!(lz4(&mut [0x10, 1, 0, 0, 0x00].into_iter(), &mut out, id), None);
        assert_eq!(lz4(&mut [0x10, 1, 2, 0].into_iter(), &mut out, id), None);
        assert_eq!(lz4(&mut [0x1f, 1, 1, 0, 0].into_iter(), &mut out, id), None);
        assert_eq!(lz4(&mut [0x90, 1, 2, 3, 4, 5, 6, 7, 8, 9].into_iter(), &mut out, id), None);
        // offset 1 repeats the last byte: 1 literal + 7, then the closing token (no literals)
        assert_eq!(lz4(&mut [0x13, 7, 1, 0, 0x00].into_iter(), &mut out, id), Some(8));
        assert_eq!(out, [7; 8]);
        // endless 255s: stops at the limit
        assert_eq!(lz4(&mut [0xf0].into_iter().chain(core::iter::repeat(255)), &mut out, id), None);
    }
}
