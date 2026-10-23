//! The basemap under the map field: roads, waterways and water from stock's decoded tile cache on the FAT volume
//! (`\Map\Data\Temp\Zoom<z>\H|P\<x>_<y>_0.bin`, Web Mercator XYZ tiles, ghidra/STORAGE_AND_MAPS.md 4.2 and 4.5),
//! drawn **once per view** into a pixel buffer in SDRAM. The map copies that buffer to the panel and restores
//! from it under the arrow and the track, so moving them never draws a road again.
//!
//! Why it is fast:
//! - **Nothing is drawn while the view stays.** A view change (first fix, re-centre, zoom) draws once; the arrow
//!   and new track segments are small boxes copied from the buffer ([`crate::map::update`]).
//! - **The eMMC is read on view changes only**, and only for tiles not already held: up to [`SLOTS`] tiles stay
//!   decoded in SDRAM (least recently used goes first). Of P, only the bytes of the features drawn are read.
//! - **Integers per point:** each tile gets one affine map from its 4096-unit grid to screen pixels (16.16 fixed
//!   point), worked out once in floating point; a point then costs two multiply-adds and two shifts.
//! - **Straight into the buffer:** a road is a square brush swept along each segment, one `fill` of a slice per
//!   row, clipped first; no per-pixel calls, no anti-aliasing. Water is filled by rows (even-odd).
//! - **Draw order by sorting once:** each tile's features are sorted by draw key at load, and the tiles are merged
//!   pass by pass, so a motorway in one tile still lies over a path in the next.
//!
//! From stock (STORAGE_AND_MAPS.md 4.5, 4.6, and its AXI SRAM tables 0x2400bdc8 widths, 0x2400c880 visibility):
//! the file layout, layer and class ids, the road visibility by zoom, road widths by zoom and class, the colours,
//! the draw order (water, waterways, roads class 13 down to 1), tunnels of minor roads skipped, water polygons from
//! zoom 13. Our choices: widths are half of stock's (stock's are at 512 pixels a tile; bryton_tile.py halves them
//! too), no dashes on paths, no landcover, park or labels, the zoom from the view's scale with a fall-back to
//! coarser cached zooms.
//!
//! Sources:
//! - Tile maths: wiki.openstreetmap.org/wiki/Slippy_map_tilenames (x = n (lon + 180) / 360,
//!   y = n (1 - ln(tan lat + sec lat) / pi) / 2, the northwest corner of a tile); ln(tan + sec) = atanh(sin).
//! - Vector tiles: github.com/mapbox/vector-tile-spec 2.1, 4.3.4.4 (outer rings have a positive area by the
//!   surveyor's formula in tile coordinates, y down: holes are negative) and 4.3.4 (geometry may pass the extent).
//! - Layers: openmaptiles.org/schema (transportation, water, waterway classes); stock maps them to its own ids
//!   (STORAGE_AND_MAPS.md 4.5: road class 13 "boundary" and 10 / 11 transit / rail are stock's, not the schema's).
//! - Clipping: Cohen-Sutherland, en.wikipedia.org/wiki/Cohen%E2%80%93Sutherland_algorithm (outcodes, the
//!   intersection with the border of the outside end, repeat).
//! - Fill: scanline even-odd with a half-open rule at vertices (en.wikipedia.org/wiki/Scanline_rendering); each
//!   row tests every edge of the ring, which is cheap for the few short water rings drawn; an active edge table is
//!   the step up if long rings (landcover) are ever drawn.
//! - Not `embedded-graphics` 0.8.1: its thick lines are parallel 1-pixel Bresenham lines sent pixel by pixel through
//!   `DrawTarget::draw_iter` (src/primitives/line/thick_points.rs), and it brings 5 crates (az, byteorder,
//!   float-cmp, micromath, embedded-graphics-core); here a road row is one slice `fill` into the buffer.

use crate::fat::{self, Disk, Fat, File};
use crate::map::{Rect, View, cos_q16};
use crate::text::Line;

/// Pixels of the largest map area (a full ride page, 240 x 296).
pub const PIXELS: usize = 240 * 296;
/// Tiles kept decoded.
pub const SLOTS: usize = 6;
/// Features kept a tile (the largest cached tile seen draws about 5000).
const FEATS: usize = 8192;
/// Points kept a tile, x and y (the largest seen: about 18,000).
const PTS: usize = 32_768;
const SLOT_WORDS: usize = FEATS * 4 + PTS * 2;
/// The memory the basemap needs, 16-bit words: the pixels, then the tiles (1.3 MB).
pub const MEM_WORDS: usize = PIXELS + SLOTS * SLOT_WORDS;
/// Bytes read from a file at a time (on the stack while drawing).
const IO: usize = 8192;
/// The finest zoom stock caches.
const MAX_ZOOM: u8 = 14;
/// Coarser zooms tried when the view's own is not cached.
const FALLBACK: u8 = 3;
/// A tile is drawn at least this wide (pixels): a tile of 512 pixels, as stock draws them, give or take 1.4x;
/// so at most 2 x 2 tiles cover the map.
const TILE_PX: f64 = 362.0;
/// Metres per 1e-7 degree of latitude, as map.rs.
const M_PER_E7: f64 = 0.011_132;

/// RGB565 of 8-bit red, green, blue.
const fn rgb(r: u32, g: u32, b: u32) -> u16 {
    ((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16
}

/// Stock's colours (STORAGE_AND_MAPS.md 4.6).
pub const BACKGROUND: u16 = rgb(173, 183, 169);
const MOTORWAY: u16 = rgb(220, 150, 95);
const MAJOR: u16 = rgb(240, 220, 112);
const MINOR: u16 = rgb(255, 255, 255);
const RAIL: u16 = rgb(0, 0, 0);
const CYCLEWAY: u16 = rgb(100, 0, 104);
const UNPAVED: u16 = rgb(123, 61, 32);
const WATER: u16 = rgb(55, 180, 200);

/// Road classes shown by zoom 0..16: bit `class` (stock's table at 0x2400c880, transportation row).
const ROADS_SHOWN: [u16; 17] = [0, 2, 2, 2, 2, 2, 6, 6, 0xe, 0xe, 0x1e, 0x1e, 0x7e, 0xfe, 0x1ffe, 0x1ffe, 0x1ffe];
/// Road widths in pixels at 512 pixels a tile, by zoom 0..15 and class (stock's floats at 0x2400bdc8).
const WIDTHS: [[u8; 14]; 16] = [
    [0; 14],
    [10, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 4, 4, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 4, 4, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 4, 6, 6, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 6, 6, 6, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 8, 6, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 8, 6, 6, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 10, 8, 6, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 10, 8, 6, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 10, 8, 6, 4, 2, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 10, 8, 6, 4, 4, 2, 2, 2, 2, 2, 2, 2, 2],
    [10, 10, 8, 6, 4, 4, 2, 2, 2, 2, 2, 2, 2, 2],
    [16, 14, 14, 12, 10, 10, 4, 4, 4, 2, 4, 2, 2, 2],
    [18, 16, 14, 14, 12, 12, 6, 6, 4, 2, 4, 2, 2, 2],
];
/// Layer ids (STORAGE_AND_MAPS.md 4.5).
const ROAD: u16 = 1;
const WATER_AREA: u16 = 2;
const WATERWAY: u16 = 3;
/// Draw keys, in order: water rings, their holes, waterways, then roads class 13 (key 3) down to class 1 (15).
const KEYS: u16 = 16;
/// Points of a water ring filled at most (projected on the stack).
const RING: usize = 1024;

/// What one view change cost: for the measurements (sim.rs) and the dev link.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Tiles read from the eMMC (not already held).
    pub tiles_read: u32,
    /// Bytes read from files.
    pub bytes_read: u32,
    /// Features and points projected.
    pub features: u32,
    /// Points projected.
    pub points: u32,
    /// Pixels written into the buffer.
    pub px: u32,
    /// The zoom drawn (0: none).
    pub zoom: u8,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    /// (zoom, x, y); zoom 0: empty.
    key: (u8, u32, u32),
    feats: usize,
    used: u32,
}

/// The basemap: its memory, the volume, the tiles held, and what the pixel buffer holds.
pub struct Basemap {
    mem: Option<&'static mut [u16]>,
    fat: Option<Fat>,
    /// The card or the volume failed: no basemap until the next power-up.
    failed: bool,
    /// `Zoom<z>\H` and `Zoom<z>\P` of the last zoom looked at.
    dirs: Option<(u8, Option<(File, File)>)>,
    slots: [Slot; SLOTS],
    tick: u32,
    /// What the buffer was drawn for (area, view, origin), and whether it holds a map (else "NO MAP").
    drawn: Option<(Rect, View, (i32, i32))>,
    map: bool,
    /// The last view change's cost.
    pub stats: Stats,
}

impl Default for Basemap {
    fn default() -> Self {
        Self::new()
    }
}

/// What the map shows under the track for a view.
pub enum Layer<'a> {
    /// The basemap, row by row, as wide as the area.
    Map(&'a [u16]),
    /// No tiles there (or no card): "NO MAP" on black.
    NoMap,
    /// Not drawn for this view: black.
    None,
}

impl Basemap {
    /// No memory yet: nothing drawn.
    pub const fn new() -> Self {
        Basemap {
            mem: None,
            fat: None,
            failed: false,
            dirs: None,
            slots: [Slot { key: (0, 0, 0), feats: 0, used: 0 }; SLOTS],
            tick: 0,
            drawn: None,
            map: false,
            stats: Stats { tiles_read: 0, bytes_read: 0, features: 0, points: 0, px: 0, zoom: 0 },
        }
    }

    /// Gives it its memory, at least [`MEM_WORDS`] words (less: ignored).
    pub fn set_memory(&mut self, mem: &'static mut [u16]) {
        if mem.len() >= MEM_WORDS {
            self.mem = Some(mem);
            self.drawn = None;
        }
    }

    /// What it shows for `view` in `area`.
    pub fn layer(&self, area: Rect, view: View, origin: (i32, i32)) -> Layer<'_> {
        match (&self.mem, self.drawn == Some((area, view, origin))) {
            (Some(m), true) if self.map => Layer::Map(m.get(..(area.2 * area.3) as usize).unwrap_or(&[])),
            (_, true) => Layer::NoMap,
            _ => Layer::None,
        }
    }

    /// Draws the basemap of `view` in `area` into the buffer (positions in metres from `origin`, degrees x 1e-7),
    /// unless it already holds it. Reads the tiles it lacks from `d`.
    pub fn render(&mut self, d: &mut impl Disk, area: Rect, view: View, origin: (i32, i32)) {
        if self.drawn == Some((area, view, origin)) || self.mem.is_none() {
            return;
        }
        self.stats = Stats::default();
        self.map = match self.draw(d, area, view, origin) {
            Ok(drawn) => drawn,
            Err(_) => {
                self.failed = true; // the card or the volume: not again this power-up
                false
            }
        };
        self.drawn = Some((area, view, origin));
    }

    /// Draws into the buffer; false: no tiles for this view.
    fn draw(&mut self, d: &mut impl Disk, area: Rect, view: View, origin: (i32, i32)) -> Result<bool, fat::Error> {
        let (w, h) = (area.2, area.3);
        if self.failed || w <= 0 || h <= 0 || (w * h) as usize > PIXELS {
            return Ok(false);
        }
        if self.fat.is_none() {
            self.fat = Some(Fat::mount(d)?);
        }
        let Some(fat) = self.fat.as_mut() else { return Ok(false) };
        let frame = Frame::new(view, origin, w, h);
        // the zoom: the view's own, else a coarser one whose tile under the centre is cached
        let mut found = None;
        for z in (frame.zoom.saturating_sub(FALLBACK).max(1)..=frame.zoom).rev() {
            if self.dirs.is_none_or(|(dz, _)| dz != z) {
                let dir = |fat: &mut Fat, d: &mut _, hp| {
                    let mut p = Line::<32>::new();
                    p.push_str("Map/Data/Temp/Zoom");
                    p.push_u32(u32::from(z));
                    p.push_str(hp);
                    fat.open(d, p.as_str())
                };
                let dirs = match (dir(fat, d, "/H"), dir(fat, d, "/P")) {
                    (Ok(h), Ok(p)) => Some((h, p)),
                    (Err(fat::Error::Io), _) | (_, Err(fat::Error::Io)) => return Err(fat::Error::Io),
                    _ => None,
                };
                self.dirs = Some((z, dirs));
            }
            let Some((_, Some((hdir, _)))) = self.dirs else { continue };
            let (x, y) = frame.centre_tile(z);
            match fat.find(d, hdir, tile_name(x, y).as_str()) {
                Ok(_) => {
                    found = Some(z);
                    break;
                }
                Err(fat::Error::Io) => return Err(fat::Error::Io),
                Err(_) => {}
            }
        }
        let (Some(z), Some((_, Some((hdir, pdir))))) = (found, self.dirs) else { return Ok(false) };
        self.stats.zoom = z;
        // the tiles on screen: at most 2 x 2 around the centre's, as tiles are drawn wider than the map
        let (cx, cy) = frame.centre_tile(z);
        let mut tiles = [(0u32, 0u32, Proj::default()); 4];
        let mut n = 0;
        for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (x, y) = (cx as i64 + dx, cy as i64 + dy);
            if x < 0 || y < 0 || x >= 1 << z || y >= 1 << z || n == tiles.len() {
                continue;
            }
            let p = frame.proj(z, x as u32, y as u32);
            if p.on_screen(w, h) {
                tiles[n] = (x as u32, y as u32, p);
                n += 1;
            }
        }
        let Some(mem) = self.mem.as_deref_mut() else { return Ok(false) };
        let Some((px, store)) = mem.split_at_mut_checked(PIXELS) else { return Ok(false) };
        // the tiles not held go into the slots used longest ago (not one of these)
        self.tick = self.tick.wrapping_add(1);
        let mut at = [usize::MAX; 4];
        for (i, &(x, y, _)) in tiles.iter().take(n).enumerate() {
            if let Some(s) = self.slots.iter().position(|s| s.key == (z, x, y)) {
                at[i] = s;
                self.slots[s].used = self.tick;
            }
        }
        let mut io = [0u8; IO];
        for (i, &(x, y, _)) in tiles.iter().take(n).enumerate() {
            if at[i] != usize::MAX {
                continue;
            }
            let free = (0..SLOTS).filter(|s| !at.contains(s)).min_by_key(|&s| self.slots[s].used);
            let Some(s) = free else { continue };
            let Some(words) = store.get_mut(s * SLOT_WORDS..(s + 1) * SLOT_WORDS) else { continue };
            let (feats, pts) = words.split_at_mut(FEATS * 4);
            let (feats, _) = feats.as_chunks_mut::<4>();
            self.slots[s] = Slot { key: (0, 0, 0), feats: 0, used: self.tick };
            let name = tile_name(x, y);
            let got = match (fat.find(d, hdir, name.as_str()), fat.find(d, pdir, name.as_str())) {
                (Ok(h), Ok(p)) => load(&mut OnCard { fat, d, h, p }, feats, pts, &mut io, &mut self.stats),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
            let feats = match got {
                Ok(k) => k,
                Err(fat::Error::Io) => return Err(fat::Error::Io),
                Err(_) => 0, // missing or damaged: drawn empty
            };
            self.slots[s] = Slot { key: (z, x, y), feats, used: self.tick };
            self.stats.tiles_read += 1;
            at[i] = s;
        }
        // draw: the background, then each draw key over all tiles
        let px = px.get_mut(..(w * h) as usize).unwrap_or(&mut []);
        px.fill(BACKGROUND);
        self.stats.px += (w * h) as u32;
        let style = Style::of(frame.display_zoom);
        let mut cursor = [0usize; 4];
        let mut buf = Raster { px, w, h, written: 0 };
        for key in 0..KEYS {
            for (i, &(_, _, proj)) in tiles.iter().take(n).enumerate() {
                let Some(slot) = self.slots.get(at[i]) else { continue };
                let Some(words) = store.get(at[i] * SLOT_WORDS..(at[i] + 1) * SLOT_WORDS) else { continue };
                let (feats, pts) = words.split_at(FEATS * 4);
                let feats = feats.as_chunks::<4>().0.get(..slot.feats).unwrap_or(&[]);
                while let Some(f) = feats.get(cursor[i]).filter(|f| f[1] == key) {
                    cursor[i] += 1;
                    let Some(pts) = pts.get(usize::from(f[0]) * 2..(usize::from(f[0]) + usize::from(f[2])) * 2) else {
                        continue;
                    };
                    self.stats.features += 1;
                    self.stats.points += u32::from(f[2]);
                    buf.feature(&proj, &style, f[3], pts);
                }
            }
        }
        self.stats.px += buf.written;
        Ok(true)
    }
}

/// "x_y_0.bin".
fn tile_name(x: u32, y: u32) -> Line<24> {
    let mut l = Line::new();
    l.push_u32(x);
    l.push_str("_");
    l.push_u32(y);
    l.push_str("_0.bin");
    l
}

/// A tile's two files, H (false) and P (true), read at an offset.
trait Pair {
    fn size(&self, p: bool) -> u32;
    fn read(&mut self, p: bool, offset: u32, buf: &mut [u8]) -> Result<usize, fat::Error>;
}

/// A tile's files on the FAT volume.
struct OnCard<'a, D: Disk> {
    fat: &'a mut Fat,
    d: &'a mut D,
    h: File,
    p: File,
}

impl<D: Disk> Pair for OnCard<'_, D> {
    fn size(&self, p: bool) -> u32 {
        if p { self.p.size } else { self.h.size }
    }
    fn read(&mut self, p: bool, offset: u32, buf: &mut [u8]) -> Result<usize, fat::Error> {
        let f = if p { self.p } else { self.h };
        self.fat.read_at(self.d, &f, offset, buf)
    }
}

/// Reads a tile into `feats` and `pts`: the features drawn, sorted by draw key, as [first point, key, points,
/// kind]; kind = layer | class << 2 | sub << 6 | hole << 10. Returns how many.
fn load(
    t: &mut impl Pair,
    feats: &mut [[u16; 4]],
    pts: &mut [u16],
    io: &mut [u8; IO],
    stats: &mut Stats,
) -> Result<usize, fat::Error> {
    let mut head = [0u8; 4];
    t.read(false, 0, &mut head)?;
    let count = u32::from_le_bytes(head);
    if count == 0 {
        return Ok(0); // an empty tile: the count and one 0 byte (Map_WriteEmptyTileCache)
    }
    if count.checked_mul(10).and_then(|b| b.checked_add(4)) != Some(t.size(false)) {
        return Err(fat::Error::Corrupt); // as stock checks H (STORAGE_AND_MAPS.md 4.8)
    }
    // H: the features drawn, with their offsets into P
    let mut n = 0;
    let mut i = 0;
    while i < count && n < feats.len() {
        let batch = (count - i).min((IO / 10) as u32);
        let bytes = io.get_mut(..batch as usize * 10).ok_or(fat::Error::Corrupt)?;
        t.read(false, 4 + i * 10, bytes)?;
        stats.bytes_read += bytes.len() as u32;
        for r in bytes.chunks_exact(10) {
            let (layer, npts, class, sub) = (u16::from(r[0]), u16::from_le_bytes([r[1], r[2]]), r[7], r[8]);
            let off = u32::from_le_bytes([r[3], r[4], r[5], r[6]]);
            let min = if layer == WATER_AREA { 3 } else { 2 };
            if !(ROAD..=WATERWAY).contains(&layer) || class == 0 || class > 13 || npts < min || n == feats.len() {
                continue;
            }
            let kind = layer | u16::from(class) << 2 | u16::from(sub.min(15)) << 6;
            if let Some(f) = feats.get_mut(n) {
                *f = [off as u16, (off >> 16) as u16, npts, kind];
                n += 1;
            }
        }
        i += batch;
    }
    // P: the points of those features, in file order, through a window of `io`
    let psize = t.size(true);
    let (mut start, mut len) = (0u32, 0usize);
    let mut next = 0usize; // the next free point
    let mut kept = 0;
    for k in 0..n {
        let Some(&[lo, hi, count, kind]) = feats.get(k) else { break };
        let off = u32::from(lo) | u32::from(hi) << 16;
        let points = usize::from(count);
        if u64::from(off) + 4 * points as u64 > u64::from(psize) {
            continue; // past the end of P: dropped
        }
        if next + points > pts.len() / 2 {
            break; // the slot is full: the rest of the tile is dropped
        }
        let mut area2 = 0i64; // twice the ring's area (surveyor's formula), for water holes
        let mut first = (0i64, 0i64);
        let mut prev = (0i64, 0i64);
        for j in 0..points {
            let pos = off + 4 * j as u32;
            if pos < start || pos + 4 > start + len as u32 {
                start = pos & !(fat::SECTOR as u32 - 1);
                let want = (psize - start).min(IO as u32) as usize;
                len = t.read(true, start, io.get_mut(..want).ok_or(fat::Error::Corrupt)?)?;
                stats.bytes_read += len as u32;
                if pos + 4 > start + len as u32 {
                    return Err(fat::Error::Corrupt);
                }
            }
            let at = (pos - start) as usize;
            let b = io.get(at..at + 4).ok_or(fat::Error::Corrupt)?;
            let (x, y) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
            if let Some(p) = pts.get_mut((next + j) * 2..(next + j) * 2 + 2) {
                p.copy_from_slice(&[x, y]);
            }
            let q = (i64::from(x as i16), i64::from(y as i16));
            if j == 0 {
                first = q;
            } else {
                area2 += prev.0 * q.1 - q.0 * prev.1;
            }
            prev = q;
        }
        area2 += prev.0 * first.1 - first.0 * prev.1;
        let layer = kind & 3;
        let hole = layer == WATER_AREA && area2 < 0; // MVT: outer rings positive, holes negative
        let key = match layer {
            WATER_AREA => u16::from(hole),
            WATERWAY => 2,
            _ => 3 + 13 - (kind >> 2 & 15),
        };
        if let Some(f) = feats.get_mut(kept) {
            *f = [next as u16, key, count, kind | u16::from(hole) << 10];
        }
        kept += 1;
        next += points;
    }
    let kept_feats = feats.get_mut(..kept).unwrap_or(&mut []);
    heapsort(kept_feats);
    Ok(kept)
}

/// Sorts features by draw key (`f[1]`), in place: heapsort, O(n log n) and a few hundred bytes of code where
/// core's sort_unstable costs about 6 KB of image.
fn heapsort(v: &mut [[u16; 4]]) {
    fn down(v: &mut [[u16; 4]], mut root: usize, end: usize) {
        loop {
            let mut child = 2 * root + 1;
            if child >= end {
                return;
            }
            if child + 1 < end && v[child][1] < v[child + 1][1] {
                child += 1;
            }
            if v[root][1] >= v[child][1] {
                return;
            }
            v.swap(root, child);
            root = child;
        }
    }
    let n = v.len();
    for root in (0..n / 2).rev() {
        down(v, root, n);
    }
    for end in (1..n).rev() {
        v.swap(0, end);
        down(v, 0, end);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Projection: Web Mercator tiles to the map's plane (metres east and north of the first fix, equirectangular with
// map.rs's constants), then to pixels of the area.
// ---------------------------------------------------------------------------------------------------------------

/// A tile's grid (4096 units a side) to area pixels, 16.16 fixed point: x = (ax u + bx) >> 16, y likewise.
#[derive(Clone, Copy, Default)]
struct Proj {
    ax: i64,
    bx: i64,
    ay: i64,
    by: i64,
}

impl Proj {
    fn at(&self, u: u16, v: u16) -> (i64, i64) {
        ((self.ax * i64::from(u as i16) + self.bx) >> 16, (self.ay * i64::from(v as i16) + self.by) >> 16)
    }

    /// The tile overlaps the area.
    fn on_screen(&self, w: i32, h: i32) -> bool {
        let (x0, y0) = self.at(0, 0);
        let (x1, y1) = self.at(4096, 4096);
        x0 < i64::from(w) && x1 > 0 && y0 < i64::from(h) && y1 > 0
    }
}

/// A view as the tiles need it: the plane's scales at the origin, and the zooms.
struct Frame {
    /// Pixels per metre.
    scale: f64,
    /// Metres per degree x 1e-7 east (with map.rs's cosine) and north.
    east: f64,
    lon0: f64,
    /// The origin's latitude, its sine and cosine, its Mercator ordinate (radians).
    phi0: f64,
    sin0: f64,
    cos0: f64,
    psi0: f64,
    /// The view's centre, metres, and the area's half size, pixels.
    cx: f64,
    cy: f64,
    half: (f64, f64),
    /// The zoom whose tiles are drawn about 512 pixels wide (up to 15), and the finest cached one for it.
    display_zoom: u8,
    zoom: u8,
}

impl Frame {
    fn new(view: View, (lat0, lon0): (i32, i32), w: i32, h: i32) -> Frame {
        let east = M_PER_E7 * cos_q16(lat0) as f64 / 65_536.0;
        let phi0 = f64::from(lat0) * 1e-7 * core::f64::consts::PI / 180.0;
        let (sin0, cos0) = (sin(phi0), cos(phi0));
        let scale = 16.0 / view.mpp16 as f64;
        let world_px = 3.6e9 * east * scale; // the whole world at zoom 0, pixels
        let mut display_zoom = 1u8;
        while display_zoom < 15 && world_px / f64::from(1u32 << (display_zoom + 1)) >= TILE_PX {
            display_zoom += 1;
        }
        Frame {
            scale,
            east,
            lon0: f64::from(lon0),
            phi0,
            sin0,
            cos0,
            psi0: atanh(sin0),
            cx: f64::from(view.cx),
            cy: f64::from(view.cy),
            half: (f64::from(w / 2), f64::from(h / 2)),
            display_zoom,
            zoom: display_zoom.min(MAX_ZOOM),
        }
    }

    /// Metres north of the origin of Mercator ordinate `psi` (radians): the latitude by its series around the
    /// origin's (gd' = cos, gd'' = -sin cos, gd''' = -(cos^2 - sin^2) cos), within metres 100 km away.
    fn north(&self, psi: f64) -> f64 {
        let (s, c, d) = (self.sin0, self.cos0, psi - self.psi0);
        let dphi = c * d - 0.5 * s * c * d * d - (c * c - s * s) * c * d * d * d / 6.0;
        dphi * 180.0 / core::f64::consts::PI * 1e7 * M_PER_E7
    }

    /// Tile (`x`, `y`) of zoom `z` to area pixels.
    fn proj(&self, z: u8, x: u32, y: u32) -> Proj {
        let units = f64::from(1u32 << z) * 4096.0; // grid units around the world
        // east: lon (1e-7 degrees) = U / units x 3.6e9 - 1.8e9, linear in u
        let ax = 3.6e9 / units * self.east * self.scale;
        let x0 = ((f64::from(x) * 4096.0 / units * 3.6e9 - 1.8e9 - self.lon0) * self.east - self.cx) * self.scale;
        // north: Mercator, linear within the tile between its top and bottom edges
        let psi = |v: f64| core::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) * 4096.0 + v) / units);
        let (top, bottom) = (self.north(psi(0.0)), self.north(psi(4096.0)));
        let ay = -(bottom - top) / 4096.0 * self.scale;
        let y0 = -(top - self.cy) * self.scale;
        let q = |v: f64| (v * 65_536.0) as i64;
        Proj { ax: q(ax), bx: q(x0 + self.half.0), ay: q(ay), by: q(y0 + self.half.1) }
    }

    /// The tile of zoom `z` under the view's centre.
    fn centre_tile(&self, z: u8) -> (u32, u32) {
        let n = f64::from(1u32 << z);
        let lon = self.lon0 + self.cx / self.east;
        let phi = self.phi0 + self.cy / (1e7 * M_PER_E7) * core::f64::consts::PI / 180.0;
        let x = (lon + 1.8e9) / 3.6e9 * n;
        let y = (1.0 - atanh(sin(phi)) / core::f64::consts::PI) / 2.0 * n;
        // not f64::clamp: its assertion message formats floats, which pulls in 8 KB of core::fmt
        let clamp = |t: f64| t.max(0.0).min(n - 1.0) as u32;
        (clamp(x), clamp(y))
    }
}

/// Floating point without libm: series accurate to about 1e-13 in the ranges used here (|x| <= pi/2).
fn sin(x: f64) -> f64 {
    let (mut term, mut sum) = (x, x);
    for k in 1..10 {
        term *= -x * x / f64::from((2 * k) * (2 * k + 1));
        sum += term;
    }
    sum
}

fn cos(x: f64) -> f64 {
    let (mut term, mut sum) = (1.0, 1.0);
    for k in 1..10 {
        term *= -x * x / f64::from((2 * k - 1) * (2 * k));
        sum += term;
    }
    sum
}

/// Natural logarithm: the exponent from the bits, then 2 atanh((m - 1) / (m + 1)) for the mantissa.
fn ln(x: f64) -> f64 {
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));
    if m > core::f64::consts::SQRT_2 {
        m /= 2.0;
        e += 1;
    }
    let t = (m - 1.0) / (m + 1.0);
    let (mut term, mut sum) = (t, t);
    for k in 1..12 {
        term *= t * t;
        sum += term / f64::from(2 * k + 1);
    }
    e as f64 * core::f64::consts::LN_2 + 2.0 * sum
}

/// The Mercator ordinate of a latitude whose sine is `s`.
fn atanh(s: f64) -> f64 {
    0.5 * ln((1.0 + s) / (1.0 - s))
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing into the buffer
// ---------------------------------------------------------------------------------------------------------------

/// How features look at a display zoom.
struct Style {
    roads: u16,
    widths: [u8; 14],
    water: bool,
}

impl Style {
    fn of(zoom: u8) -> Style {
        let z = usize::from(zoom);
        Style {
            roads: ROADS_SHOWN.get(z).copied().unwrap_or(0),
            widths: WIDTHS.get(z).copied().unwrap_or([0; 14]),
            water: zoom >= 13,
        }
    }

    /// Colour and width of a feature of `kind`; None: not drawn.
    fn look(&self, kind: u16) -> Option<(u16, i32)> {
        let (layer, class, sub) = (kind & 3, kind >> 2 & 15, kind >> 6 & 15);
        match layer {
            WATER_AREA if self.water => Some((if kind & 1 << 10 != 0 { BACKGROUND } else { WATER }, 0)),
            WATERWAY => Some((WATER, 1)),
            ROAD if self.roads & 1 << class != 0 && !(class >= 6 && sub == 2) => {
                let color = match (class, sub) {
                    (1, _) => MOTORWAY,
                    (2..=5, _) => MAJOR,
                    (10 | 11, _) => RAIL,
                    (_, 1) => CYCLEWAY,
                    (8, 7) => UNPAVED,
                    _ => MINOR,
                };
                let w = self.widths.get(usize::from(class)).copied().unwrap_or(2);
                Some((color, i32::from(w / 2).max(1)))
            }
            _ => None,
        }
    }
}

/// The area's pixels, `w` x `h`, and the count written.
struct Raster<'a> {
    px: &'a mut [u16],
    w: i32,
    h: i32,
    written: u32,
}

impl Raster<'_> {
    fn feature(&mut self, proj: &Proj, style: &Style, kind: u16, pts: &[u16]) {
        let Some((color, width)) = style.look(kind) else { return };
        let pairs = pts.as_chunks::<2>().0;
        if width == 0 {
            self.ring(proj, pairs, color);
            return;
        }
        let (lo, hi) = ((width - 1) / 2, width / 2);
        let m = i64::from(hi) + 1;
        let mut prev = None;
        for &[u, v] in pairs {
            let p = proj.at(u, v);
            if let Some(a) = prev
                && let Some((a, b)) = clip(a, p, (-m, -m, i64::from(self.w) + m, i64::from(self.h) + m))
            {
                self.line(a, b, lo, hi, color);
            }
            prev = Some(p);
        }
    }

    /// A segment swept by a square brush of `lo + 1 + hi` pixels, one run a row.
    fn line(&mut self, a: (i32, i32), b: (i32, i32), lo: i32, hi: i32, color: u16) {
        let (a, b) = if a.1 <= b.1 { (a, b) } else { (b, a) };
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        for y in (a.1 - lo).max(0)..=(b.1 + hi).min(self.h - 1) {
            // the brush centres covering row y lie on rows y - hi ..= y + lo of the segment, each row from half a
            // row above to half a row below (in half rows), so a shallow line has no gaps
            let (c0, c1) = ((2 * (y - hi) - 1).max(2 * a.1), (2 * (y + lo) + 1).min(2 * b.1));
            let at = |c: i32| a.0 + dx * (c - 2 * a.1) / (2 * dy);
            let (x0, x1) = if dy == 0 { (a.0, b.0) } else { (at(c0), at(c1)) };
            self.run(y, x0.min(x1) - lo, x0.max(x1) + hi, color);
        }
    }

    /// Pixels `x0..=x1` of row `y`, clipped.
    fn run(&mut self, y: i32, x0: i32, x1: i32, color: u16) {
        let (x0, x1) = (x0.max(0), x1.min(self.w - 1));
        if x0 > x1 || y < 0 || y >= self.h {
            return;
        }
        let row = (y * self.w) as usize;
        if let Some(s) = self.px.get_mut(row + x0 as usize..=row + x1 as usize) {
            s.fill(color);
            self.written += s.len() as u32;
        }
    }

    /// A ring filled row by row (even-odd, sampled at pixel centres).
    fn ring(&mut self, proj: &Proj, pairs: &[[u16; 2]], color: u16) {
        let n = pairs.len();
        if n > RING {
            return; // ponytail: a longer ring is not drawn; project it twice (bounds, then rows) if one matters
        }
        let mut ring = [(0i32, 0i32); RING];
        let lim = |v: i64| v.clamp(-(1 << 20), 1 << 20) as i32; // far points stay far, without overflow
        let (mut top, mut bottom) = (i32::MAX, i32::MIN);
        for (r, &[u, v]) in ring.iter_mut().zip(pairs) {
            let (x, y) = proj.at(u, v);
            *r = (lim(x), lim(y));
            (top, bottom) = (top.min(r.1), bottom.max(r.1));
        }
        if n < 3 || bottom < 0 || top >= self.h {
            return;
        }
        let mut xs = [0i32; 64];
        for y in top.max(0)..=bottom.min(self.h - 1) {
            let mut k = 0;
            let mut a = ring[n - 1];
            for &b in &ring[..n] {
                // edges crossing the row's centre, half open so a vertex counts once
                if (a.1 <= y) != (b.1 <= y) && k < xs.len() {
                    let x = i64::from(a.0) + i64::from(b.0 - a.0) * i64::from(y - a.1) / i64::from(b.1 - a.1);
                    xs[k] = x as i32;
                    k += 1;
                }
                a = b;
            }
            let xs = &mut xs[..k];
            for i in 1..xs.len() {
                // insertion sort: a row crosses a few edges (core's sort costs 2 KB of image)
                let mut j = i;
                while j > 0 && xs[j - 1] > xs[j] {
                    xs.swap(j - 1, j);
                    j -= 1;
                }
            }
            for pair in xs.chunks_exact(2) {
                self.run(y, pair[0], pair[1] - 1, color);
            }
        }
    }
}

/// Segment `a`-`b` cut to rectangle `r` (x0, y0, x1, y1) (Cohen-Sutherland); None if it misses it.
fn clip(mut a: (i64, i64), mut b: (i64, i64), r: (i64, i64, i64, i64)) -> Option<((i32, i32), (i32, i32))> {
    let code = |p: (i64, i64)| {
        u8::from(p.0 < r.0) | u8::from(p.0 > r.2) << 1 | u8::from(p.1 < r.1) << 2 | u8::from(p.1 > r.3) << 3
    };
    for _ in 0..8 {
        let (ca, cb) = (code(a), code(b));
        if ca | cb == 0 {
            return Some(((a.0 as i32, a.1 as i32), (b.0 as i32, b.1 as i32)));
        }
        if ca & cb != 0 {
            return None;
        }
        let c = if ca != 0 { ca } else { cb };
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let p = if c & 4 != 0 {
            (a.0 + dx * (r.1 - a.1) / dy, r.1)
        } else if c & 8 != 0 {
            (a.0 + dx * (r.3 - a.1) / dy, r.3)
        } else if c & 1 != 0 {
            (r.0, a.1 + dy * (r.0 - a.0) / dx)
        } else {
            (r.2, a.1 + dy * (r.2 - a.0) / dx)
        };
        if c == ca {
            a = p;
        } else {
            b = p;
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, image};
    use crate::ui::{H, W};
    use std::boxed::Box;
    use std::vec;

    /// 0 N 0 E, where the synthetic tiles of test-data/fat32.sparse meet (tools/fat_image.py): Zoom14 and Zoom12
    /// tiles 8191..8192 and 2047..2048 around it.
    pub const ORIGIN: (i32, i32) = (0, 0);
    const AREA: Rect = (0, 24, 240, 296);

    pub fn memory() -> &'static mut [u16] {
        Box::leak(vec![0u16; MEM_WORDS].into_boxed_slice())
    }

    fn basemap() -> Basemap {
        let mut b = Basemap::new();
        b.set_memory(memory());
        b
    }

    /// Saves the area's pixels as a screen, black around (target/sim/`name`.bmp).
    fn save(px: &[u16], area: Rect, name: &str) {
        let mut screen = vec![0u16; (W * H) as usize];
        for y in 0..area.3 {
            for x in 0..area.2 {
                screen[((y + area.1) * W as i32 + x + area.0) as usize] = px[(y * area.2 + x) as usize];
            }
        }
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/sim");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(std::format!("{name}.bmp")), crate::sim::bmp(&screen)).unwrap();
    }

    fn count(px: &[u16], c: u16) -> usize {
        px.iter().filter(|&&p| p == c).count()
    }

    /// The synthetic tiles at three scales: every kind of feature drawn in its colour, the lake's island a hole, the
    /// tunnel not drawn; zoom 13 is not cached, so 8 m a pixel falls back to 12.
    #[test]
    fn draws_the_tiles() {
        let mut d = Ram(image(), 0);
        let mut b = basemap();
        for (mpp16, zoom, name) in [(32, 14, "basemap_z14_2m"), (48, 14, "basemap_z14_3m"), (128, 12, "basemap_z12_8m")]
        {
            // centred on the south-west tile's lake and tunnel at zoom 14
            let v = View { cx: -2000, cy: -700, mpp16 };
            b.render(&mut d, AREA, v, ORIGIN);
            std::println!("{name}: {:?}", b.stats);
            assert_eq!(b.stats.zoom, zoom, "{name}");
            let Layer::Map(px) = b.layer(AREA, v, ORIGIN) else { panic!("{name}: no map") };
            save(px, AREA, name);
            assert!(count(px, BACKGROUND) > 5000, "{name}: ground");
            assert!(count(px, MINOR) + count(px, MAJOR) + count(px, MOTORWAY) > 1000, "{name}: roads");
            if zoom == 14 {
                for (c, what) in [(MINOR, "minor"), (MAJOR, "major"), (WATER, "water"), (RAIL, "rail")] {
                    assert!(count(px, c) > 200, "{name}: {what}");
                }
                for (c, what) in [(CYCLEWAY, "cycleway"), (UNPAVED, "unpaved")] {
                    assert!(count(px, c) > 20, "{name}: {what}");
                }
                // in tile 8191_8192 (south-west of the origin): the lake at (900, 900), its island at (600, 900),
                // the tunnel's middle at (1000, 1300)
                let f = Frame::new(v, ORIGIN, AREA.2, AREA.3);
                let p = f.proj(14, 8191, 8192);
                let at = |u, v| {
                    let (x, y) = p.at(u, v);
                    assert!(
                        (0..i64::from(AREA.2)).contains(&x) && (0..i64::from(AREA.3)).contains(&y),
                        "{name}: off screen"
                    );
                    px[(y * i64::from(AREA.2) + x) as usize]
                };
                assert_eq!(at(900, 900), WATER, "{name}: the lake");
                assert_eq!(at(600, 900), BACKGROUND, "{name}: the island");
                assert_eq!(at(1000, 1300), BACKGROUND, "{name}: no tunnel");
            }
        }
        // the same view again: nothing read, nothing drawn
        let (reads, v) = (d.1, View { cx: -2000, cy: -700, mpp16: 128 });
        b.render(&mut d, AREA, v, ORIGIN);
        assert_eq!(d.1, reads);
        // a view nearby: the tiles held are not read again
        b.render(&mut d, AREA, View { cx: -1900, cy: -700, mpp16: 48 }, ORIGIN);
        assert_eq!(b.stats.tiles_read, 0, "held");
        // nowhere near the tiles: NO MAP
        let far = View { cx: 50_000, cy: 0, mpp16: 48 };
        b.render(&mut d, AREA, far, ORIGIN);
        assert!(matches!(b.layer(AREA, far, ORIGIN), Layer::NoMap));
        assert!(matches!(b.layer(AREA, View { cx: 1, cy: 0, mpp16: 48 }, ORIGIN), Layer::None));
    }

    /// No volume on the card: NO MAP, and no more tries.
    #[test]
    fn no_volume_no_map() {
        let mut d = Ram(vec![0u8; 4096], 0);
        let mut b = basemap();
        let v = View { cx: 0, cy: 0, mpp16: 48 };
        b.render(&mut d, AREA, v, ORIGIN);
        assert!(matches!(b.layer(AREA, v, ORIGIN), Layer::NoMap));
        let reads = d.1;
        b.render(&mut d, AREA, View { cx: 9, ..v }, ORIGIN);
        assert_eq!(d.1, reads, "failed: not tried again");
    }

    /// Tile x and y of zoom `z` at a latitude and longitude, by the OSM wiki's formula (std's floats).
    fn osm_tile(z: u8, lat: f64, lon: f64) -> (f64, f64) {
        let n = f64::from(1u32 << z);
        let r = lat.to_radians();
        let y = (1.0
            - std::primitive::f64::ln(std::primitive::f64::tan(r) + 1.0 / std::primitive::f64::cos(r))
                / core::f64::consts::PI)
            / 2.0;
        ((lon + 180.0) / 360.0 * n, y * n)
    }

    #[test]
    fn floating_point_without_libm() {
        for x in [-1.5, -0.7, 0.0, 0.3, 0.785, 1.2, 1.5707] {
            assert!((sin(x) - std::primitive::f64::sin(x)).abs() < 1e-12, "sin {x}");
            assert!((cos(x) - std::primitive::f64::cos(x)).abs() < 1e-12, "cos {x}");
        }
        for x in [1e-9, 0.01, 0.5, 1.0, 1.5, 2.0, 10.0, 12345.678] {
            assert!((ln(x) - std::primitive::f64::ln(x)).abs() < 1e-12, "ln {x}");
        }
        // the tile under a position, as the OSM wiki's formula (made-up places)
        for (lat, lon) in [(-0.0001, 0.0001), (33.3, -100.0), (60.0, 25.5), (-45.1, 170.2)] {
            let f = Frame::new(View { cx: 0, cy: 0, mpp16: 48 }, ((lat * 1e7) as i32, (lon * 1e7) as i32), 240, 296);
            for z in [9u8, 12, 14] {
                let (x, y) = osm_tile(z, lat, lon);
                assert_eq!(f.centre_tile(z), (x as u32, y as u32), "{lat},{lon} zoom {z}");
            }
        }
    }

    /// Tile corners land where the plane puts their latitude and longitude (map.rs's projection), at a made-up place
    /// and 30 km away.
    #[test]
    fn tiles_meet_the_track_projection() {
        let origin = (472_345_600, 81_234_500); // 47.23456 N 8.12345 E, made up
        let v = View { cx: 0, cy: 0, mpp16: 16 }; // 1 m a pixel
        let f = Frame::new(v, origin, 240, 296);
        let (tx, ty) = osm_tile(14, 47.23456, 8.12345);
        let lat = |ty: f64| {
            let n = core::f64::consts::PI * (1.0 - 2.0 * ty / 16384.0);
            std::primitive::f64::atan(std::primitive::f64::sinh(n)).to_degrees()
        };
        let lon = |tx: f64| tx / 16384.0 * 360.0 - 180.0;
        for (tx, ty) in [(tx as u32, ty as u32), (tx as u32 + 20, ty as u32 + 20)] {
            let p = f.proj(14, tx, ty);
            for (u, v) in [(0u16, 0u16), (4096, 4096)] {
                let (x, y) = p.at(u, v);
                let mut t = crate::map::Track::new();
                t.seed(origin.0, origin.1);
                let (ex, ey) = (f64::from(tx) + f64::from(u) / 4096.0, f64::from(ty) + f64::from(v) / 4096.0);
                t.add((lat(ey) * 1e7) as i32, (lon(ex) * 1e7) as i32, 0);
                let (ex, ey) = t.pos().unwrap();
                assert!((x - 120 - i64::from(ex)).abs() <= 1, "x {x} vs {ex}");
                assert!((148 - y - i64::from(ey)).abs() <= 1, "y {y} vs {ey}");
            }
        }
    }

    /// A tile's files in memory.
    struct Files(std::vec::Vec<u8>, std::vec::Vec<u8>);

    impl Pair for Files {
        fn size(&self, p: bool) -> u32 {
            (if p { &self.1 } else { &self.0 }).len() as u32
        }
        fn read(&mut self, p: bool, offset: u32, buf: &mut [u8]) -> Result<usize, fat::Error> {
            let f = if p { &self.1 } else { &self.0 };
            let from = (offset as usize).min(f.len());
            let n = buf.len().min(f.len() - from);
            buf[..n].copy_from_slice(&f[from..from + n]);
            Ok(n)
        }
    }

    #[test]
    fn heapsort_sorts() {
        let mut seed = 7u32;
        for n in [0, 1, 2, 3, 10, 1000] {
            let mut v: std::vec::Vec<[u16; 4]> = (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    [i, (seed >> 16) as u16 % 16, 0, 0]
                })
                .collect();
            heapsort(&mut v);
            assert!(v.windows(2).all(|w| w[0][1] <= w[1][1]));
            assert_eq!(v.len(), usize::from(n));
        }
    }

    #[test]
    fn clips_segments() {
        let r = (0, 0, 100, 50);
        assert_eq!(clip((10, 10), (20, 20), r), Some(((10, 10), (20, 20))));
        assert_eq!(clip((-100, 25), (200, 25), r), Some(((0, 25), (100, 25))));
        assert_eq!(clip((-10, -10), (-5, 60), r), None);
        assert_eq!(clip((50, -1_000_000_000), (50, 1_000_000_000), r), Some(((50, 0), (50, 50))));
    }

    /// Every tile of a real `\Map\Data\Temp` folder decodes and draws without a panic, at every display zoom.
    /// Ignored: real tiles stay outside the repository (AGENTS.md, "Public repository"); point BRYTON_TILES at a
    /// copy of the folder (`cargo test-host every_cached_tile -- --ignored`).
    #[test]
    #[ignore]
    fn every_cached_tile() {
        let Ok(root) = std::env::var("BRYTON_TILES") else {
            std::println!("BRYTON_TILES not set: skipped");
            return;
        };
        let mut mem = vec![0u16; SLOT_WORDS];
        let mut px = vec![0u16; PIXELS];
        let (mut n, mut stats) = (0, Stats::default());
        for z in 1..=14u8 {
            let Ok(dir) = std::fs::read_dir(std::format!("{root}/Zoom{z}/H")) else { continue };
            for e in dir.flatten() {
                let name = e.file_name().into_string().unwrap();
                let h = std::fs::read(e.path()).unwrap();
                let p = std::fs::read(std::format!("{root}/Zoom{z}/P/{name}")).unwrap();
                let (feats, pts) = mem.split_at_mut(FEATS * 4);
                let feats = feats.as_chunks_mut::<4>().0;
                let k = load(&mut Files(h, p), feats, pts, &mut [0; IO], &mut stats).unwrap();
                // the tile over the area
                let proj = Proj { ax: 240 * 16, bx: 0, ay: 296 * 16, by: 0 };
                for zoom in 1..=15 {
                    let mut r = Raster { px: &mut px, w: 240, h: 296, written: 0 };
                    for f in &feats[..k] {
                        let at = usize::from(f[0]) * 2;
                        r.feature(&proj, &Style::of(zoom), f[3], &pts[at..at + usize::from(f[2]) * 2]);
                    }
                }
                n += 1;
            }
        }
        std::println!("{n} tiles, {stats:?}");
        assert!(n > 0, "no tiles under {root}");
    }
}
