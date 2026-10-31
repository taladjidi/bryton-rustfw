//! OpenStreetMap map packs: vector tiles made on the computer by `tools/map_pack.py` from an OpenStreetMap
//! extract, copied by the owner to `\Rustfw\Maps\osm\` (read only here). The data is (c) OpenStreetMap
//! contributors, ODbL (the About page says so; the licence asks for that notice, openstreetmap.org/copyright).
//!
//! The records are **stock's decoded tile cache** (`H` and `P` files of `\Map\Data\Temp`, ghidra/STORAGE_AND_MAPS.md
//! 4.2 and 4.5), so `ui/tiles.rs` draws them with the code it has for stock's tiles: the H file is a count u32,
//! then 10-byte records (layer u8: 1 road, 2 water area, 3 waterway; point count u16; offset u32 into P; class u8;
//! sub u8; one 0 byte), the P file the points, 4 bytes each (x u16, y u16, 4096 units a tile side, y down, inside
//! 0..=4096: the tool clips to the tile). Water rings: outer positive by the surveyor's formula, holes negative.
//! Road classes (the tool's choice, drawn by stock's tables): 1 motorway and trunk, 2 primary, 3 secondary,
//! 4 tertiary, 6 residential, unclassified, 7 service, 8 track and path (sub 7: brown) or cycleway (sub 1:
//! purple), 10 railway; sub 2: tunnel (not drawn for minor roads).
//!
//! **Pack** (version 1, little-endian), one file per zoom and aligned block of 16 x 16 XYZ tiles,
//! `OSM<z>_<x / 16>_<y / 16>.PAK` (the pack tool writes zooms 11 to 14 by default; one file is a few hundred KB):
//! - 0: magic `OSMP`; 4: version u16 = 1; 6: zoom u8; 7: block u8 = 16; 8: x0 u32, 12: y0 u32, the block's first
//!   tile (x0 = 16 (x / 16)).
//! - 16: the index, 16 x 16 entries of (H offset u32, H length u32, P offset u32, P length u32), row-major, entry i
//!   at 16 + 16 i: never across a sector, so a tile is found with one sector read. H offset 0: no tile (outside the
//!   area packed); an empty tile inside it is a count of 0 (H length 4, P length 0).
//! - then the tiles' H and P blobs, each 4-byte aligned.
//!
//! Our own design (no spec to follow); the pack tool's selftest reads it back with an independent reader.

use crate::text::Line;

/// The packs' folder on the volume.
pub const DIR: &str = "Rustfw/Maps/osm";
/// Tiles a side of a pack's block.
pub const BLOCK: u32 = 16;
const HEAD: u32 = 16;
/// The index's end: the first blob byte.
const DATA: u32 = HEAD + 16 * BLOCK * BLOCK;

/// The pack holding tile (`z`, `x`, `y`): `OSM<z>_<x / 16>_<y / 16>.PAK`.
pub fn name(z: u8, x: u32, y: u32) -> Line<32> {
    let mut l = Line::new();
    l.push_str("OSM");
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
    h[..4] == *b"OSMP"
        && h[4..6] == [1, 0]
        && h[6] == z
        && u32::from(h[7]) == BLOCK
        && u32_at(8) == x / BLOCK * BLOCK
        && u32_at(12) == y / BLOCK * BLOCK
}

/// Where tile (`x`, `y`)'s index entry is in its pack.
pub fn entry_at(x: u32, y: u32) -> u32 {
    HEAD + 16 * ((y % BLOCK) * BLOCK + x % BLOCK)
}

/// A tile's two blobs in its pack: (offset, length) of H and of P.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// H's offset in the pack.
    pub h: u32,
    /// H's length.
    pub hlen: u32,
    /// P's offset in the pack.
    pub p: u32,
    /// P's length.
    pub plen: u32,
}

/// The tile's blobs from its index entry in a pack of `size` bytes; None: no tile there, or an entry that cannot
/// be right (a blob outside the data, H too short for its count, a count over the tile's cap).
pub fn entry(e: [u8; 16], size: u32) -> Option<Entry> {
    let w = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
    let (h, hlen, p, plen) = (w(0), w(4), w(8), w(12));
    let inside = |off: u32, len: u32| off >= DATA && off.checked_add(len).is_some_and(|end| end <= size);
    ((4..=4 + 10 * 65_535).contains(&hlen) && inside(h, hlen) && inside(p, plen)).then_some(Entry { h, hlen, p, plen })
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    /// A feature: (layer, class, sub, points in 4096 units).
    pub type Feat = (u8, u8, u8, Vec<(i16, i16)>);

    /// A pack of the block (`bx`, `by`) at zoom `z` with these tiles (x, y, features): the format above.
    pub fn pack(z: u8, bx: u32, by: u32, tiles: &[((u32, u32), Vec<Feat>)]) -> Vec<u8> {
        let mut out = std::vec![0u8; DATA as usize];
        out[..4].copy_from_slice(b"OSMP");
        out[4..6].copy_from_slice(&[1, 0]);
        out[6] = z;
        out[7] = BLOCK as u8;
        out[8..12].copy_from_slice(&(bx * BLOCK).to_le_bytes());
        out[12..16].copy_from_slice(&(by * BLOCK).to_le_bytes());
        for &((x, y), ref feats) in tiles {
            let (mut h, mut p) = (Vec::from((feats.len() as u32).to_le_bytes()), Vec::new());
            for (layer, class, sub, pts) in feats {
                h.push(*layer);
                h.extend((pts.len() as u16).to_le_bytes());
                h.extend((p.len() as u32).to_le_bytes());
                h.extend([*class, *sub, 0]);
                for &(u, v) in pts {
                    p.extend(u.to_le_bytes());
                    p.extend(v.to_le_bytes());
                }
            }
            let (ho, po) = (out.len() as u32, (out.len() + h.len()) as u32);
            out.extend(&h);
            out.extend(&p);
            let at = entry_at(x, y) as usize;
            for (i, v) in [ho, h.len() as u32, po, p.len() as u32].into_iter().enumerate() {
                out[at + 4 * i..at + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// The synthetic area at zoom 14 round 0 N 0 E (tiles 8191..8192, made up): files (name, bytes). Tile (8192,
    /// 8192), south-east of the origin: a motorway, a rail line, a track, a cycleway, a tunnel of a residential
    /// road (hidden), a lake with an island (a hole); tile (8191, 8192): a residential road and a river; the two
    /// tiles north of them empty (inside the area).
    pub fn fixture() -> Vec<(String, Vec<u8>)> {
        let ring = |x0: i16, y0: i16, x1: i16, y1: i16| std::vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)];
        let mut island = ring(1900, 2050, 2100, 2250);
        island.reverse(); // a hole: negative
        let se = std::vec![
            (1, 1, 0, std::vec![(0, 1500), (4096, 1900)]),
            (1, 10, 0, std::vec![(0, 1400), (4096, 1400)]),
            (1, 8, 7, std::vec![(0, 2600), (4096, 2600)]),
            (1, 8, 1, std::vec![(1600, 4096), (1600, 1000)]),
            (1, 6, 2, std::vec![(2500, 0), (2500, 4096)]),
            (2, 1, 0, ring(1700, 1900, 2300, 2450)),
            (2, 1, 0, island),
        ];
        let sw =
            std::vec![(1, 6, 0, std::vec![(0, 2000), (4096, 2000)]), (3, 1, 0, std::vec![(1600, 0), (1600, 4096)])];
        let t = |tx, ty, f| ((tx, ty), f);
        let name = |bx, by| std::format!("OSM14_{bx}_{by}.PAK");
        std::vec![
            (name(511, 512), pack(14, 511, 512, &[t(8191, 8192, sw)])),
            (name(512, 512), pack(14, 512, 512, &[t(8192, 8192, se)])),
            (name(511, 511), pack(14, 511, 511, &[t(8191, 8191, Vec::new())])),
            (name(512, 511), pack(14, 512, 511, &[t(8192, 8191, Vec::new())])),
        ]
    }

    #[test]
    fn index_and_header() {
        let (n, bytes) = &fixture()[1];
        let h: [u8; 16] = bytes[..16].try_into().unwrap();
        assert_eq!(name(14, 8192, 8192).as_str(), n.as_str());
        assert!(header_ok(&h, 14, 8200, 8207) && !header_ok(&h, 14, 8208, 8192) && !header_ok(&h, 13, 8192, 8192));
        let at = entry_at(8192, 8192) as usize;
        let e = entry(bytes[at..at + 16].try_into().unwrap(), bytes.len() as u32).unwrap();
        assert!(e.hlen == 4 + 10 * 7 && e.plen > 0 && e.p == e.h + e.hlen);
        // no tile, a blob past the end, an H that cannot hold a count
        assert_eq!(entry([0; 16], bytes.len() as u32), None);
        assert_eq!(entry(bytes[at..at + 16].try_into().unwrap(), e.p), None);
        let mut short = bytes[at..at + 16].to_vec();
        short[4] = 3;
        assert_eq!(entry(short.try_into().unwrap(), bytes.len() as u32), None);
    }
}
