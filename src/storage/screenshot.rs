//! A screenshot as a BMP file in `\Screenshot\`, stock's folder and name for them (`YYYYMMDDhhmmss.bmp`: the bryton
//! repository's ghidra/STORAGE_AND_MAPS.md 2.1), so either firmware and a computer see them in one place.
//!
//! **The format** (Microsoft's BITMAPINFOHEADER, learn.microsoft.com/en-us/windows/win32/api/wingdi/
//! ns-wingdi-bitmapinfoheader, checked 2026-10-30: a positive height is a bottom-up DIB; BI_BITFIELDS (3) is "RGB
//! with color masks, valid for 16-bpp", the three DWORD masks red, green, blue right after the header; a row's stride
//! is its bytes rounded up to a DWORD). The panel's own pixels, RGB565, two bytes each, little endian: no conversion,
//! and 480 bytes a row needs no padding.
//!
//! ```text
//! 0   "BM", file size (u32), 0 (u32), pixels at 66 (u32)          BITMAPFILEHEADER, 14 bytes
//! 14  40, width 240, height 320 (bottom-up), 1 plane, 16 bits,     BITMAPINFOHEADER, 40 bytes
//!     BI_BITFIELDS, image bytes, 2835 px/m (72 dpi) twice, 0, 0
//! 54  masks 0xf800, 0x07e0, 0x001f                                 12 bytes
//! 66  rows, the screen's last first, 480 bytes each
//! ```
//!
//! **Write order** (AGENTS.md, golden rule: the card is shared with the ride): the free space is checked first (the
//! file and the recorder's [`FULL_RESERVE`], FSInfo's count; none known: nothing written), then the folder made if
//! missing (the root's directory written once, as for `\Activities`), then the file's clusters and pixels as a chain
//! no entry names yet, and only then its entry, whole, with its size ([`Fat::link`]). So a power cut leaves either
//! no file (a lost chain, which a computer's fsck turns into a `.CHK` and nothing else minds: `fat::write`) or the
//! whole picture, never a partial file; a write that fails frees the chain but its first cluster.

use crate::fat::write::Handle;
use crate::fat::{DiskWrite, Error, Fat, File};
use crate::recorder::FULL_RESERVE;
use crate::ui::{H, W};

/// The folder, in the root (stock's).
pub const DIR: &str = "Screenshot";

/// Bytes before the pixels: the two headers and the three masks.
pub const HEAD: usize = 66;

/// Bytes a row: 240 pixels of two bytes, a multiple of 4 (no padding).
pub const ROW: usize = 2 * W as usize;

/// The file's size.
pub const SIZE: u32 = (HEAD + ROW * H as usize) as u32;

/// Sectors written per command: 4 KB on the stack.
pub const CHUNK: usize = 8;

/// The headers and masks (module doc).
///
/// ```
/// let h = bryton_rustfw::storage::screenshot::header();
/// assert_eq!(&h[..2], b"BM");
/// assert_eq!(u32::from_le_bytes(h[2..6].try_into().unwrap()), 66 + 240 * 320 * 2);
/// ```
pub fn header() -> [u8; HEAD] {
    let mut h = [0u8; HEAD];
    let words: [(usize, u32); 15] = [
        (2, SIZE),
        (6, 0),
        (10, HEAD as u32),
        (14, 40),
        (18, W),
        (22, H),
        (30, 3), // BI_BITFIELDS
        (34, SIZE - HEAD as u32),
        (38, 2835),
        (42, 2835),
        (46, 0),
        (50, 0),
        (54, 0xf800),
        (58, 0x07e0),
        (62, 0x001f),
    ];
    h[..2].copy_from_slice(b"BM");
    for (at, v) in words {
        if let Some(d) = h.get_mut(at..at + 4) {
            d.copy_from_slice(&v.to_le_bytes());
        }
    }
    h[26..30].copy_from_slice(&[1, 0, 16, 0]); // planes, bits per pixel
    h
}

/// Fills `buf` with the file's bytes from offset `at` on (zeros past its end); `row(y)` gives screen row `y`, asked
/// once for each row the buffer touches, in the file's order (the bottom row first).
pub fn fill(at: usize, buf: &mut [u8], mut row: impl FnMut(u32) -> [u16; W as usize]) {
    let head = header();
    let mut cur: Option<(usize, [u16; W as usize])> = None;
    for (i, b) in buf.iter_mut().enumerate() {
        let o = at + i;
        *b = match o.checked_sub(HEAD) {
            None => head.get(o).copied().unwrap_or(0),
            Some(p) if p < ROW * H as usize => {
                let r = p / ROW;
                let px = match cur {
                    Some((k, px)) if k == r => px,
                    _ => {
                        let px = row(H - 1 - r as u32); // r < H
                        cur = Some((r, px));
                        px
                    }
                };
                let v = px.get(p % ROW / 2).copied().unwrap_or(0);
                v.to_le_bytes().get(p % 2).copied().unwrap_or(0)
            }
            Some(_) => 0,
        };
    }
}

/// `YYYYMMDDhhmmss.bmp` of a local time (stock's name).
///
/// ```
/// let n = bryton_rustfw::storage::screenshot::name((2026, 10, 30, 9, 5, 7));
/// assert_eq!(n.as_str(), "20261030090507.bmp");
/// ```
pub fn name((y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8)) -> crate::text::Line<20> {
    let mut l = crate::text::Line::new();
    l.push_u32(u32::from(y));
    for v in [mo, d, h, mi, s] {
        if v < 10 {
            l.push_str("0");
        }
        l.push_u32(u32::from(v));
    }
    l.push_str(".bmp");
    l
}

/// A screenshot being written: its folder and its chain, which no entry names until [`Shot::finish`].
#[derive(Debug)]
pub struct Shot {
    dir: File,
    h: Handle,
}

impl Shot {
    /// Room checked, the folder made if missing, `name` free in it: [`Error::Refused`] when the card has not the
    /// file's room and [`FULL_RESERVE`] left (FSInfo's count; unknown counts as none) or the name is taken.
    pub fn begin(fat: &mut Fat, d: &mut impl DiskWrite, name: &str, t: (u16, u16)) -> Result<Shot, Error> {
        let free = fat.free_clusters(d)?.map_or(0, u64::from) * u64::from(fat.cluster_bytes());
        if free < u64::from(SIZE) + FULL_RESERVE {
            return Err(Error::Refused);
        }
        let dir = fat.ensure_dir(d, fat.root(), DIR, t)?;
        match fat.find(d, dir, name) {
            Err(Error::NotFound) => Ok(Shot { dir, h: Handle::default() }),
            Ok(_) => Err(Error::Refused),
            Err(e) => Err(e),
        }
    }

    /// The file's sectors from `sector` on (whole sectors), clusters taken as needed.
    pub fn write(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, sector: u32, buf: &[u8]) -> Result<(), Error> {
        fat.write_sectors(d, &mut self.h, sector, buf)
    }

    /// The entry, whole, with the file's size: the picture shows.
    pub fn finish(mut self, fat: &mut Fat, d: &mut impl DiskWrite, name: &str, t: (u16, u16)) -> Result<(), Error> {
        self.h.size = SIZE;
        fat.link(d, self.dir, name, &self.h, t).map(|_| ())
    }

    /// A write failed: the chain freed but its first cluster (best effort: the card may be failing).
    pub fn abandon(mut self, fat: &mut Fat, d: &mut impl DiskWrite) {
        if self.h.clusters > 1 {
            let _ = fat.trim(d, &mut self.h, 0);
        }
    }
}

/// The decoder the simulator's tests read a screenshot back with.
#[cfg(test)]
pub mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// The pixels of a BMP this module writes, top row first; None if its headers are not this module's.
    pub fn decode(f: &[u8]) -> Option<Vec<u16>> {
        let u = |at: usize| u32::from_le_bytes(f[at..at + 4].try_into().unwrap());
        let ok = f.len() == SIZE as usize
            && &f[..2] == b"BM"
            && u(2) == SIZE
            && u(10) == HEAD as u32
            && u(14) == 40
            && (u(18), u(22)) == (W, H)
            && f[26..30] == [1, 0, 16, 0]
            && u(30) == 3
            && (u(54), u(58), u(62)) == (0xf800, 0x07e0, 0x001f);
        ok.then(|| {
            (0..H as usize)
                .flat_map(|y| {
                    let r = HEAD + (H as usize - 1 - y) * ROW;
                    f[r..r + ROW].chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect::<Vec<_>>()
                })
                .collect()
        })
    }

    fn pixel(x: usize, y: u32) -> u16 {
        (x as u16).wrapping_mul(31) ^ (y as u16).wrapping_mul(257)
    }

    /// Filled in chunks of any size from any offset, the file decodes to the rows given, and each row is asked for
    /// about once (not once a pixel).
    #[test]
    fn fills_decode_to_the_rows() {
        for chunk in [1, 7, 480, 512, 4096, SIZE as usize] {
            let mut f = std::vec![0u8; SIZE as usize];
            let mut asked = 0;
            for (k, c) in f.chunks_mut(chunk).enumerate() {
                fill(k * chunk, c, |y| {
                    asked += 1;
                    core::array::from_fn(|x| pixel(x, y))
                });
            }
            let px = decode(&f).expect("headers");
            for y in 0..H {
                for x in 0..W as usize {
                    assert_eq!(px[y as usize * W as usize + x], pixel(x, y), "chunk {chunk}, ({x}, {y})");
                }
            }
            assert!(asked <= H as usize + SIZE as usize / chunk + 1, "chunk {chunk}: {asked} rows asked");
        }
        let mut past = [1u8; 8];
        fill(SIZE as usize, &mut past, |_| [0xffff; W as usize]);
        assert_eq!(past, [0; 8], "past the end: zeros");
    }
}
