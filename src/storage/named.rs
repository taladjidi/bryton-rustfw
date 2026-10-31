//! Named points along a followed course: the peaks, volcanoes and saddles (cols, passes) with their elevation that
//! stock's cached map tiles hold, found once when the course loads and kept (at most [`MAX`], within [`NEAR_M`] of
//! the route, in order along it).
//!
//! **Where the names are:** stock's decoded tile cache (`\Map\Data\Temp\Zoom<z>\H|P\<x>_<y>_0.bin`, the format in
//! ghidra/STORAGE_AND_MAPS.md 4.5, which the basemap reads too, `crate::tiles`). Layer 13 `mountain_peak` records
//! have one point; P holds, after it, `u8 len, name, 0x00, u64 ele`. The `ele` as an unsigned 64-bit little-endian
//! number of metres is **inferred** from a real cache; the stock document (STORAGE_AND_MAPS.md 4.5) says only
//! "8 bytes". Classes: 1 peak, 2 volcano, 3 saddle. Layer 5 `poi`
//! (toilets, cafe, shelter...) and layer 7 `water_name` carry no drinking water class and no elevation: not read.
//! Peaks exist in the cache at zoom 9, 11 and 12 only (inferred from a real cache); the finest first, a name seen
//! twice kept once. A tile stock never showed is not there: no names, no markers (said in docs/status.md).
//!
//! **Cost:** one bounded step per pass ([`Pois::step`]): 128 course points to find their tiles, then 50 records
//! of one tile's H and at most `P_READS_STEP` P reads (a tile's files are opened once, their handles kept). Any failure ends the scan: an extra, never the ride.
//!
//! **Position:** a tile's grid is 4096 units; longitude is linear in it, latitude is Mercator's, worked out at the
//! tile's top and bottom edge by bisection (no inverse functions in `no_std`) and taken as linear between them
//! (the error is a few metres at zoom 12).

use crate::course::Course;
use crate::fat::{self, Disk, Fat};
use crate::route::tile_of;
use crate::text::Line;
use crate::tiles::{atanh, sin};

/// Points kept at most.
pub const MAX: usize = 32;
/// A point is kept within this of the route, m.
pub const NEAR_M: u32 = 100;
/// Name bytes kept (ASCII).
pub const NAME: usize = 24;
/// The cache zooms that hold names, finest first.
const ZOOMS: [u8; 3] = [12, 11, 9];
/// Tiles remembered a zoom (a course longer than that is scanned over its first tiles).
const TILES: usize = 32;
/// Course points looked at a step.
const POINTS_STEP: usize = 128;
/// H records read a step (a sector holds 51).
const RECORDS_STEP: u32 = 50;
/// Layer id of `mountain_peak` (STORAGE_AND_MAPS.md 4.5).
const PEAK_LAYER: u8 = 13;
/// P reads (a peak record each, at most 80 bytes) a step: with one H read of 500 bytes, the byte budget of a pass.
const P_READS_STEP: u32 = 8;

/// A named point on the route.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Poi {
    /// Distance along the course, m.
    pub along_m: u32,
    /// Elevation, m.
    pub ele_m: u16,
    /// Distance from the route, m.
    pub off_m: u8,
    /// A saddle (col, pass), else a peak.
    pub col: bool,
    n: u8,
    name: [u8; NAME],
}

impl Poi {
    /// The name, upper case ASCII (accents dropped).
    pub fn name(&self) -> &str {
        self.name.get(..usize::from(self.n)).and_then(|b| core::str::from_utf8(b).ok()).unwrap_or("")
    }

    /// A point from a decoded name (any UTF-8): folded to the pixel font's capitals.
    pub fn new(name: &[u8], ele_m: u16, col: bool, along_m: u32, off_m: u32) -> Poi {
        let mut p = Poi { along_m, ele_m, off_m: off_m.min(255) as u8, col, ..Poi::default() };
        let s = core::str::from_utf8(name).unwrap_or("");
        for c in s.chars() {
            let b = match c {
                'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => b'A',
                'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => b'E',
                'î' | 'ï' | 'Î' | 'Ï' => b'I',
                'ô' | 'ö' | 'Ô' | 'Ö' => b'O',
                'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => b'U',
                'ç' | 'Ç' => b'C',
                c if c.is_ascii_graphic() || c == ' ' => c.to_ascii_uppercase() as u8,
                _ => b'?',
            };
            match p.name.get_mut(usize::from(p.n)) {
                Some(slot) => *slot = b,
                None => break,
            }
            p.n += 1;
        }
        p
    }
}

/// A record of P after the point: the point, the name and the elevation (None: not a whole record).
///
/// ```
/// use bryton_rustfw::named::parse_point;
/// let mut b = vec![0x10, 0x00, 0x20, 0x00, 3];
/// b.extend_from_slice(b"Pic\0");
/// b.extend_from_slice(&1777u64.to_le_bytes());
/// assert_eq!(parse_point(&b), Some(((16, 32), &b"Pic"[..], 1777)));
/// assert_eq!(parse_point(&b[..b.len() - 1]), None);
/// ```
pub fn parse_point(b: &[u8]) -> Option<((i16, i16), &[u8], u64)> {
    let x = i16::from_le_bytes([*b.first()?, *b.get(1)?]);
    let y = i16::from_le_bytes([*b.get(2)?, *b.get(3)?]);
    let len = usize::from(*b.get(4)?);
    let name = b.get(5..5 + len)?;
    (*b.get(5 + len)? == 0).then_some(())?;
    let e = b.get(6 + len..14 + len)?;
    let ele = u64::from_le_bytes([e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7]]);
    Some(((x, y), name, ele))
}

/// The latitude (degrees x 1e-7) of tile row `row` (a whole number, 0 to 2^z) at zoom `z`: the inverse of the
/// Mercator row by bisection (wiki.openstreetmap.org/wiki/Slippy_map_tilenames).
fn lat_of_row(z: u8, row: u32) -> i32 {
    let target = f64::from(row) / f64::from(1u32 << z);
    let (mut lo, mut hi) = (-85.0511f64, 85.0511f64);
    for _ in 0..32 {
        let mid = (lo + hi) / 2.0;
        let y = (1.0 - atanh(sin(mid * core::f64::consts::PI / 180.0)) / core::f64::consts::PI) / 2.0;
        if y > target { lo = mid } else { hi = mid }
    }
    ((lo + hi) / 2.0 * 1e7) as i32
}

/// Where the scan is. All integers, so the all-zero value is "not started" and the static stays in `.bss`.
#[derive(Clone, Copy)]
struct Scan {
    /// The course ([`Course::id`]) it is for.
    id: u32,
    phase: u8,
    /// Course points looked at.
    pt: u16,
    tiles: [[(u16, u16); TILES]; 3],
    nt: [u8; 3],
    /// The zoom and tile being read, and its next H record.
    z: u8,
    t: u8,
    rec: u32,
    /// Latitudes of the tile's top and bottom edge.
    lat: (i32, i32),
    /// The open tile's H and P files: (first cluster, size).
    files: [(u32, u32); 2],
}

const NEW: u8 = 0;
const COLLECT: u8 = 1;
const READ: u8 = 2;
const DONE: u8 = 3;

/// The named points of the course followed, in order along it.
pub struct Pois {
    list: [Poi; MAX],
    n: usize,
    scan: Scan,
}

impl Default for Pois {
    fn default() -> Self {
        Self::new()
    }
}

impl Pois {
    /// None, nothing scanned.
    pub const fn new() -> Self {
        let p = Poi { along_m: 0, ele_m: 0, off_m: 0, col: false, n: 0, name: [0; NAME] };
        let scan = Scan {
            id: 0,
            phase: NEW,
            pt: 0,
            tiles: [[(0, 0); TILES]; 3],
            nt: [0; 3],
            z: 0,
            t: 0,
            rec: 0,
            lat: (0, 0),
            files: [(0, 0); 2],
        };
        Pois { list: [p; MAX], n: 0, scan }
    }

    /// The points found so far, by distance along the course.
    pub fn list(&self) -> &[Poi] {
        self.list.get(..self.n).unwrap_or(&[])
    }

    /// Course points looked at so far.
    #[cfg(test)]
    pub fn collected(&self) -> usize {
        usize::from(self.scan.pt)
    }

    /// The scan has ended (found or not).
    pub fn done(&self) -> bool {
        self.scan.phase == DONE
    }

    /// The first point at or after `along` m: its index and the point.
    pub fn next(&self, along: u32) -> Option<(usize, &Poi)> {
        self.list().iter().enumerate().find(|(_, p)| p.along_m >= along)
    }

    /// Adds a point: in order along the course; a point without a name, or a name already there, is not added; full, the one farthest
    /// from the route goes (the new one if it is the farthest).
    pub fn offer(&mut self, p: Poi) {
        if p.n == 0 || self.list().iter().any(|q| q.name() == p.name()) {
            return;
        }
        if self.n == MAX {
            let Some((worst, w)) = self.list().iter().enumerate().max_by_key(|(_, q)| q.off_m) else { return };
            if w.off_m <= p.off_m {
                return;
            }
            self.list.copy_within(worst + 1..MAX, worst);
            self.n -= 1;
        }
        let at = self.list().partition_point(|q| q.along_m <= p.along_m);
        self.list.copy_within(at..self.n, at + 1);
        if let Some(slot) = self.list.get_mut(at) {
            *slot = p;
            self.n += 1;
        }
    }

    fn restart(&mut self, id: u32) {
        self.n = 0;
        self.scan.id = id;
        self.scan.phase = COLLECT;
        (self.scan.pt, self.scan.nt, self.scan.z, self.scan.t, self.scan.rec) = (0, [0; 3], 0, 0, 0);
    }

    /// One bounded step of the scan for `c`, a followed and loaded course; a new course starts it again.
    pub fn step(&mut self, c: &Course, d: &mut impl Disk) {
        if self.scan.phase == NEW || self.scan.id != c.id() {
            self.restart(c.id());
        }
        match self.scan.phase {
            COLLECT => self.collect(c),
            READ => {
                if self.read(c, d).is_err() {
                    self.scan.phase = DONE; // an extra: dropped
                }
            }
            _ => {}
        }
    }

    /// The tiles the course crosses, at each zoom, from the next [`POINTS_STEP`] points.
    fn collect(&mut self, c: &Course) {
        let pts = c.points();
        let from = usize::from(self.scan.pt);
        for i in from..(from + POINTS_STEP).min(pts.len()) {
            let Some(&(px, py)) = pts.get(i) else { continue };
            // the point and its four corners [`NEAR_M`] away: a tile the route only passes beside holds names too
            let r = NEAR_M as i32;
            for (dx, dy) in [(0, 0), (-r, -r), (r, -r), (-r, r), (r, r)] {
                let Some((lat, lon)) = c.lat_lon((px.saturating_add(dx), py.saturating_add(dy))) else { continue };
                let (x, y) = tile_of(lat, lon);
                for (zi, &z) in ZOOMS.iter().enumerate() {
                    let t = ((x >> (14 - u32::from(z))) as u16, (y >> (14 - u32::from(z))) as u16);
                    let n = usize::from(self.scan.nt.get(zi).copied().unwrap_or(0));
                    let Some(row) = self.scan.tiles.get_mut(zi) else { continue };
                    if !row.get(..n).unwrap_or(&[]).contains(&t)
                        && let Some(slot) = row.get_mut(n)
                    {
                        *slot = t;
                        self.scan.nt[zi] += 1;
                    }
                }
            }
        }
        let next = (from + POINTS_STEP).min(pts.len());
        self.scan.pt = next.min(usize::from(u16::MAX)) as u16;
        if next >= pts.len() {
            self.scan.phase = if pts.len() < 2 { DONE } else { READ };
        }
    }

    fn skip_tile(&mut self) {
        self.scan.t += 1;
        self.scan.rec = 0;
    }

    /// A step of reading: the next tile of the zoom, or the next records of the tile open.
    fn read(&mut self, c: &Course, d: &mut impl Disk) -> Result<(), fat::Error> {
        let (zi, ti) = (usize::from(self.scan.z), usize::from(self.scan.t));
        let Some(&z) = ZOOMS.get(zi) else {
            self.scan.phase = DONE;
            return Ok(());
        };
        if ti >= usize::from(self.scan.nt.get(zi).copied().unwrap_or(0)) {
            (self.scan.z, self.scan.t, self.scan.rec) = (self.scan.z + 1, 0, 0);
            return Ok(());
        }
        let (x, y) = self.scan.tiles.get(zi).and_then(|r| r.get(ti)).copied().unwrap_or((0, 0));
        let mut fat = Fat::mount(d)?;
        if self.scan.rec == 0 {
            // a tile's files are looked for once; their handles are kept for the passes that read it
            let mut name = Line::<24>::new();
            name.push_u32(u32::from(x));
            name.push_str("_");
            name.push_u32(u32::from(y));
            name.push_str("_0.bin");
            let mut open = |hp: &str| -> Result<Option<fat::File>, fat::Error> {
                let mut path = Line::<48>::new();
                path.push_str("Map/Data/Temp/Zoom");
                path.push_u32(u32::from(z));
                path.push_str(hp);
                let dir = match fat.open(d, path.as_str()) {
                    Ok(dir) => dir,
                    Err(fat::Error::Io) => return Err(fat::Error::Io),
                    Err(_) => return Ok(None),
                };
                match fat.find(d, dir, name.as_str()) {
                    Ok(f) => Ok(Some(f)),
                    Err(fat::Error::Io) => Err(fat::Error::Io),
                    Err(_) => Ok(None),
                }
            };
            let (h, p) = (open("/H")?, open("/P")?);
            match (h, p) {
                (Some(h), Some(p)) if h.size >= 4 && (h.size - 4) % 10 == 0 => {
                    self.scan.files = [(h.cluster, h.size), (p.cluster, p.size)];
                }
                _ => {
                    self.skip_tile(); // missing, or H not 4 + 10 n bytes (as stock checks it)
                    return Ok(());
                }
            }
            let n = 1u32 << z;
            self.scan.lat = (lat_of_row(z, u32::from(y)), lat_of_row(z, (u32::from(y) + 1).min(n)));
        }
        let file = |(cluster, size)| fat::File { cluster, size, dir: false };
        let (h, p) = (file(self.scan.files[0]), file(self.scan.files[1]));
        let count = h.size.saturating_sub(4) / 10;
        let first = self.scan.rec;
        if first >= count {
            self.skip_tile();
            return Ok(());
        }
        let last = (first + RECORDS_STEP).min(count);
        let mut buf = [0u8; 10 * RECORDS_STEP as usize];
        let want = buf.get_mut(..10 * last.saturating_sub(first) as usize).ok_or(fat::Error::Corrupt)?;
        fat.read_at(d, &h, 4 + 10 * first, want)?;
        let (mut reads, mut upto) = (0, first);
        for r in want.chunks_exact(10) {
            if reads == P_READS_STEP {
                break; // the rest at the next pass
            }
            upto += 1;
            let (layer, npts, class) = (r[0], u16::from_le_bytes([r[1], r[2]]), r[7]);
            let off = u32::from_le_bytes([r[3], r[4], r[5], r[6]]);
            if layer != PEAK_LAYER || npts != 1 || !(1..=3).contains(&class) || off.saturating_add(14) > p.size {
                continue;
            }
            reads += 1;
            let mut rec = [0u8; 4 + 1 + 64 + 1 + 8];
            let got = fat.read_at(d, &p, off, &mut rec)?;
            let Some(((gx, gy), nm, ele)) = rec.get(..got).and_then(parse_point) else { continue };
            if !(0..=4096).contains(&gx) || !(0..=4096).contains(&gy) {
                continue;
            }
            let (top, bot) = self.scan.lat;
            let lat = i64::from(top) + (i64::from(bot) - i64::from(top)) * i64::from(gy) / 4096;
            let lon = (i64::from(x) * 4096 + i64::from(gx)) * 3_600_000_000 / (4096 << z) - 1_800_000_000;
            let Some(plane) = c.plane_of(lat as i32, lon as i32) else { continue };
            if let Some((off_m, along_m)) = c.near(plane, NEAR_M) {
                self.offer(Poi::new(nm, ele.min(u64::from(u16::MAX)) as u16, class == 3, along_m, off_m));
            }
        }
        self.scan.rec = upto;
        if upto >= count {
            self.skip_tile();
        }
        Ok(())
    }
}

/// Memory for a test app (leaked, as main.rs's static).
#[cfg(test)]
pub(crate) fn memory() -> &'static mut Pois {
    extern crate std;
    std::boxed::Box::leak(std::boxed::Box::new(Pois::new()))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn poi(name: &str, along: u32, off: u32) -> Poi {
        Poi::new(name.as_bytes(), 1000, false, along, off)
    }

    #[test]
    fn names_fold_to_capitals() {
        let p = Poi::new("Col de Test \u{c9}t\u{e9}".as_bytes(), 1333, true, 5, 3);
        assert_eq!(p.name(), "COL DE TEST ETE");
        assert_eq!(Poi::new("Aiguille de la Grande Montagne Imaginaire".as_bytes(), 1, false, 0, 0).name().len(), NAME);
    }

    #[test]
    fn points_stay_in_order_and_bounded() {
        let mut s = Pois::new();
        for (i, a) in [900u32, 100, 500, 100].iter().enumerate() {
            s.offer(poi(["B", "A", "C", "A"][i], *a, 10));
        }
        let names: std::vec::Vec<_> = s.list().iter().map(|p| p.name()).collect();
        assert_eq!(names, ["A", "C", "B"], "ordered by distance along, a name once");
        assert_eq!(s.next(101).map(|(i, p)| (i, p.name())), Some((1, "C")));
        assert_eq!(s.next(901), None);
        // full: the farthest from the route goes
        let mut s = Pois::new();
        for i in 0..MAX as u32 {
            s.offer(poi(&std::format!("P{i}"), i * 10, 50 + i % 7));
        }
        s.offer(poi("NEAR", 5, 1));
        s.offer(poi("FAR", 6, 99));
        assert_eq!(s.list().len(), MAX);
        assert!(s.list().iter().any(|p| p.name() == "NEAR") && s.list().iter().all(|p| p.name() != "FAR"));
        assert!(s.list().windows(2).all(|w| w[0].along_m <= w[1].along_m));
    }

    #[test]
    fn rows_to_latitudes() {
        // zoom 1: the middle row is the equator, row 0 the Mercator limit
        assert!(lat_of_row(1, 1).abs() < 100);
        assert!((lat_of_row(1, 0) - 850_511_000).abs() < 10_000);
        assert!(lat_of_row(12, 100) > lat_of_row(12, 101));
    }

    #[test]
    fn a_short_record_is_not_a_point() {
        assert_eq!(parse_point(&[]), None);
        assert_eq!(parse_point(&[0, 0, 0, 0, 200]), None);
    }
}
