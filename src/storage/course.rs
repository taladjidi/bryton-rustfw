//! Courses on the card, read only: the routes Bryton's app syncs (`\PlanTrip\*.fit`) and the FIT or GPX files
//! copied over USB (`\Courses\*.fit`, `*.gpx`). [`List::read`] lists them with their length, [`Courses::load`]
//! reads one into the course model ([`crate::course`]); nothing is written.
//!
//! Files are read in [`IO`]-byte reads and streamed: a FIT file through the FIT reader with the position
//! ([`crate::fitread::Reader::with_position`]), a GPX file through [`Gpx`]; their points go one by one to the
//! model, which thins them. At most [`MAX_BYTES`] of a file.
//!
//! **FIT courses** (Garmin's FIT SDK, the course encoding cookbook: github.com/garmin/fit-csharp-sdk
//! `Cookbook/CourseEncode/Program.cs`, read 2026-10-24): `file_id` (type 6, course), `course` (name, sport), a
//! `lap`, timer `event`s, then a `record` per point (position_lat, position_long, altitude, distance), with
//! `course_point`s (turns, summits) among them. Only the records' positions and altitudes are read.
//!
//! **Bryton's routes** (`\PlanTrip\<name>.fit`, written by its app; read on the host from one unit's backup,
//! 2026-10-24: 14 files, every one the same layout; field meanings inferred from how the values relate):
//! a FIT file (header 14 bytes, protocol 1.0, profile 21.67, good CRCs) with no Garmin message at all, only
//! messages 248 to 254, in this order:
//! - 248 (once): two uint16, 0 and 1 (a version, inferred);
//! - 254 (once): 1 point count (uint16), 2 to 5 the bounding box (sint32, degrees x 1e6: north, south, east, west;
//!   each equals the points' extreme), 6 the length (uint32, m: within 0.25 % of the points' own sum in every file,
//!   inferred), 7 and 8 the highest and lowest altitude (uint16, encoded as the points'), 9 and 10 the ascent and
//!   descent (uint16, m: a little under the points' summed rises, as from smoothed altitudes);
//! - 251 (once per point): a uint16 counting 0, 1, 2... (meaning unknown);
//! - 253 (once): the count of 250 messages;
//! - 250 (a few): 1 a point index, 2 a type (enum), 3 and 4 uint32, 5 a 32-byte name. In the routes, types 190 and
//!   191 come in pairs, from a point to a later one higher up: the climbs the app found (inferred), the rest 0. In
//!   `voiceTrip.fit` (the app's last navigation), other types with street names: turns (inferred);
//! - 252 (once): the point count again;
//! - 249 (once per point, last): 1 latitude, 2 longitude (sint32, degrees x 1e6), 3 altitude (uint16, (m + 500) x 5
//!   as FIT's: in that unit 254's ascent is in metres and the altitudes are those of the terrain), 4 grade (sint16, % x 100
//!   in whole percent; not in `voiceTrip.fit`; not read: the model works out its own); points about 20 to 30 m
//!   apart.
//!
//! The climbs of 250 are not used: the model finds its own the same way for every file.
//!
//! **GPX** (GPX 1.1 schema, www.topografix.com/GPX/1/1/, read 2026-10-24): `<trkpt lat=".." lon="..">` (in
//! `<trk><trkseg>`) and `<rtept>` (in `<rte>`) are `wptType`: `lat` and `lon` attributes in decimal degrees (WGS84),
//! an optional `<ele>` child in metres; the values are `xsd:decimal` (no exponent). [`Gpx`] reads only that: track
//! and route points in file order, their elevation; anything else (waypoints, names, extensions) is skipped. A tag
//! longer than [`TAG`] bytes is cut (its attributes past that are not seen).
//!
//! Nothing here panics or loops on any input (fuzzed in sim.rs `fuzz`): every loop is bounded by the file's size,
//! the list or a buffer.

use crate::course::{Course, Tracker, step_mm};
use crate::fat::{self, Disk, Fat, File};
use crate::fitread::{Msg, Reader};
use crate::history::degrees_e7;

/// Courses listed at most.
pub const LIST: usize = 32;
/// Characters of a course's name kept.
pub const NAME: usize = 20;
/// Bytes read from the card per disk read.
pub const IO: usize = crate::stock::IO;
/// A file longer than this is read only this far (a GPX of 4 MB is about 30 000 points).
pub const MAX_BYTES: u32 = 4 << 20;
/// The folders listed: stock's routes, then the files copied over USB.
const DIRS: [&str; 2] = ["PlanTrip", "Courses"];
/// Bytes of a GPX tag kept.
pub const TAG: usize = 128;

/// A course file: its name (capitals, as the pixel font has them), its kind and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file.
    pub file: File,
    name: [u8; NAME],
    name_len: u8,
    /// A GPX file (else FIT).
    pub gpx: bool,
    /// Its length, m (None: no points read).
    pub dist_m: Option<u32>,
}

impl Entry {
    const EMPTY: Entry = Entry {
        file: File { cluster: 0, size: 0, dir: false },
        name: [0; NAME],
        name_len: 0,
        gpx: false,
        dist_m: None,
    };

    /// Its name, without the extension.
    pub fn name(&self) -> &str {
        core::str::from_utf8(self.name.get(..usize::from(self.name_len)).unwrap_or(&[])).unwrap_or("")
    }
}

/// The courses found.
pub struct List {
    e: [Entry; LIST],
    len: usize,
}

impl Default for List {
    fn default() -> Self {
        Self::new()
    }
}

impl List {
    /// None.
    pub const fn new() -> Self {
        List { e: [Entry::EMPTY; LIST], len: 0 }
    }

    /// Course `i`.
    pub fn get(&self, i: usize) -> Option<&Entry> {
        self.e.get(..self.len).and_then(|e| e.get(i))
    }

    /// How many.
    pub fn len(&self) -> usize {
        self.len
    }

    /// None found.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Lists the course files in `DIRS` (the first [`LIST`]), each read once for its length.
    #[inline(never)] // the reads' buffer lives in points()'s frame, not in the caller's
    pub fn read(&mut self, d: &mut impl Disk) -> Result<(), fat::Error> {
        self.len = 0;
        let mut fat = Fat::mount(d)?;
        for dir in DIRS {
            let Ok(dir) = fat.open(d, dir) else { continue };
            let mut c = fat.cursor(dir)?;
            while let Some(f) = fat.list(d, &mut c)? {
                let n = f.name();
                let ext = |e: &[u8]| n.len() > 4 && n.get(n.len() - 4..).is_some_and(|x| x.eq_ignore_ascii_case(e));
                let gpx = ext(b".gpx");
                if f.file.dir || f.file.size == 0 || !(gpx || ext(b".fit")) || self.len >= LIST {
                    continue;
                }
                let mut e = Entry { file: f.file, gpx, ..Entry::EMPTY };
                for (slot, &b) in e.name.iter_mut().zip(n.get(..n.len() - 4).unwrap_or(&[])) {
                    *slot = match b {
                        b'a'..=b'z' => b.to_ascii_uppercase(),
                        b'A'..=b'Z' | b'0'..=b'9' | b' ' | b'-' | b'.' => b,
                        b'_' => b' ',
                        _ => b'?',
                    };
                    e.name_len += 1;
                }
                let (mut last, mut mm, mut count) = (None, 0u64, 0u32);
                points(&mut fat, d, &f.file, gpx, &mut |lat, lon, _| {
                    if let Some(q) = last {
                        mm = mm.saturating_add(step_mm(q, (lat, lon)));
                    }
                    (last, count) = (Some((lat, lon)), count.saturating_add(1));
                })?;
                e.dist_m = (count >= 2).then_some((mm / 1000).min(u64::from(u32::MAX)) as u32);
                if let Some(slot) = self.e.get_mut(self.len) {
                    *slot = e;
                    self.len += 1;
                }
            }
        }
        Ok(())
    }
}

/// The points of `file` (degrees x 1e-7, altitude in dm), to `f` in order.
#[inline(never)] // its read buffer lives in its own frame
fn points(
    fat: &mut Fat,
    d: &mut impl Disk,
    file: &File,
    gpx: bool,
    f: &mut dyn FnMut(i32, i32, Option<i32>),
) -> Result<(), fat::Error> {
    let mut io = [0u8; IO];
    let (mut fit, mut xml) = (Reader::with_position(), Gpx::new());
    let (mut offset, size) = (0u32, file.size.min(MAX_BYTES));
    while offset < size {
        let n = fat.read_at(d, file, offset, &mut io)?;
        if n == 0 {
            break;
        }
        offset += n as u32;
        let buf = io.get(..n).unwrap_or(&[]);
        if gpx {
            xml.push_slice(buf, f);
            continue;
        }
        fit.push_slice(buf, |m| {
            if let Msg::Record(r) = m
                && let Some((lat, lon)) = r.pos
            {
                f(degrees_e7(lat), degrees_e7(lon), r.alt_dm);
            }
        });
        if fit.result().is_some() {
            break;
        }
    }
    Ok(())
}

/// The course memory: the list, the course followed and where the rider is on it. About 70 KB: main.rs keeps it
/// in AXI SRAM and hands it to the app.
pub struct Courses {
    /// The course files found.
    pub list: List,
    /// The course read last.
    pub course: Course,
    /// Where the rider is on it.
    pub at: Tracker,
    /// The course followed (its file); None: none.
    pub on: Option<File>,
}

impl Default for Courses {
    fn default() -> Self {
        Self::new()
    }
}

impl Courses {
    /// Nothing listed or followed.
    pub const fn new() -> Self {
        Courses { list: List::new(), course: Course::new(), at: Tracker::new(), on: None }
    }

    /// Reads course `k` of the list and follows it; false if it has fewer than two points (then none is followed).
    #[inline(never)]
    pub fn load(&mut self, d: &mut impl Disk, k: usize) -> Result<bool, fat::Error> {
        let Some(&e) = self.list.get(k) else { return Ok(false) };
        let mut fat = Fat::mount(d)?;
        self.on = None;
        let course = &mut self.course;
        course.clear();
        let read = points(&mut fat, d, &e.file, e.gpx, &mut |lat, lon, alt| course.push(lat, lon, alt));
        course.finish();
        read?;
        let ok = course.points().len() >= 2;
        (self.on, self.at) = (ok.then_some(e.file), Tracker::new());
        Ok(ok)
    }
}

/// A GPX reader, a byte at a time: track and route points (`trkpt`, `rtept`) with their elevation (see the module
/// doc).
pub struct Gpx {
    tag: [u8; TAG],
    tag_len: usize,
    in_tag: bool,
    /// Inside `<ele>`: its text so far.
    ele: Option<([u8; 24], usize)>,
    /// The point open: latitude, longitude (degrees x 1e-7), elevation (dm).
    pt: Option<(i32, i32, Option<i32>)>,
}

impl Default for Gpx {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpx {
    /// At the start of a file.
    pub const fn new() -> Self {
        Gpx { tag: [0; TAG], tag_len: 0, in_tag: false, ele: None, pt: None }
    }

    /// The next bytes of the file; each point to `f` as it ends.
    pub fn push_slice(&mut self, buf: &[u8], f: &mut dyn FnMut(i32, i32, Option<i32>)) {
        for &b in buf {
            if self.in_tag {
                if b == b'>' {
                    self.in_tag = false;
                    self.tag_end(f);
                } else if let Some(slot) = self.tag.get_mut(self.tag_len) {
                    *slot = b;
                    self.tag_len += 1;
                }
            } else if b == b'<' {
                (self.in_tag, self.tag_len) = (true, 0);
                if let (Some((text, n)), Some(p)) = (self.ele.take(), self.pt.as_mut()) {
                    p.2 = decimal(text.get(..n).unwrap_or(&[]).trim_ascii(), 1).and_then(|v| i32::try_from(v).ok());
                }
            } else if let Some((text, n)) = self.ele.as_mut()
                && let Some(slot) = text.get_mut(*n)
            {
                *slot = b;
                *n += 1;
            }
        }
    }

    /// A whole tag (between `<` and `>`).
    fn tag_end(&mut self, f: &mut dyn FnMut(i32, i32, Option<i32>)) {
        let t = self.tag.get(..self.tag_len).unwrap_or(&[]);
        let name = t.split(|&b| b.is_ascii_whitespace() || b == b'/' && t.first() != Some(&b'/')).next();
        match name.unwrap_or(&[]) {
            b"trkpt" | b"rtept" => {
                let ll = attr(t, b"lat").zip(attr(t, b"lon"));
                self.pt =
                    ll.filter(|&(la, lo)| la.abs() <= 900_000_000 && lo.abs() <= 1_800_000_000).map(|(la, lo)| {
                        (la as i32, lo as i32, None) // in range: the casts keep the values
                    });
                if t.last() == Some(&b'/') {
                    self.emit(f);
                }
            }
            b"/trkpt" | b"/rtept" => self.emit(f),
            b"ele" => self.ele = Some(([0; 24], 0)),
            _ => {}
        }
    }

    fn emit(&mut self, f: &mut dyn FnMut(i32, i32, Option<i32>)) {
        if let Some((lat, lon, ele)) = self.pt.take() {
            f(lat, lon, ele);
        }
    }
}

/// Attribute `name`'s value in tag text `t` as degrees x 1e-7.
fn attr(t: &[u8], name: &[u8]) -> Option<i64> {
    let mut i = 0;
    while let Some(rest) = t.get(i..).filter(|r| !r.is_empty()) {
        let after = rest.strip_prefix(name).filter(|_| i > 0 && t.get(i - 1).is_some_and(u8::is_ascii_whitespace));
        if let Some(after) = after {
            let after = after.trim_ascii_start().strip_prefix(b"=")?.trim_ascii_start();
            let q = *after.first().filter(|&&q| q == b'"' || q == b'\'')?;
            let value = after.get(1..)?.split(|&b| b == q).next()?;
            return decimal(value.trim_ascii(), 7);
        }
        i += 1;
    }
    None
}

/// An `xsd:decimal` ("-12.345") as an integer with `places` decimal places (more are dropped); None if it is not
/// one, has more than 12 digits before the point, or does not fit an i64.
///
/// ```
/// use bryton_rustfw::storage::course::decimal;
/// assert_eq!(decimal(b"-12.3456789", 7), Some(-123_456_789));
/// assert_eq!(decimal(b"+0.5", 1), Some(5));
/// assert_eq!(decimal(b"812", 1), Some(8120));
/// assert_eq!(decimal(b"1e5", 1), None);
/// ```
pub fn decimal(s: &[u8], places: u32) -> Option<i64> {
    let (neg, s) = match s.first() {
        Some(b'-') => (true, s.get(1..)?),
        Some(b'+') => (false, s.get(1..)?),
        _ => (false, s),
    };
    let mut parts = s.splitn(2, |&b| b == b'.');
    let (int, frac) = (parts.next()?, parts.next().unwrap_or(&[]));
    if int.len() > 12 || (int.is_empty() && frac.is_empty()) {
        return None;
    }
    let mut v: i64 = 0;
    for &b in int {
        v = v.checked_mul(10)?.checked_add(i64::from(b.checked_sub(b'0').filter(|&d| d <= 9)?))?;
    }
    for k in 0..places as usize {
        let d = match frac.get(k) {
            Some(&b) => i64::from(b.checked_sub(b'0').filter(|&d| d <= 9)?),
            None => 0,
        };
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    if !frac.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// Course memory for a test app (leaked: it lives as long as the test, as main.rs's static).
    pub fn memory() -> &'static mut Courses {
        std::boxed::Box::leak(std::boxed::Box::new(Courses::new()))
    }

    fn gpx_points(text: &[u8], chunk: usize) -> Vec<(i32, i32, Option<i32>)> {
        let (mut g, mut v) = (Gpx::new(), Vec::new());
        for part in text.chunks(chunk) {
            g.push_slice(part, &mut |la, lo, e| v.push((la, lo, e)));
        }
        v
    }

    /// Track and route points with and without elevation, attributes in either order and quote, a waypoint and
    /// names skipped, any chunking.
    #[test]
    fn gpx() {
        let text = br#"<?xml version="1.0"?><gpx version="1.1" creator="x"><wpt lat="1" lon="1"><ele>9</ele></wpt>
            <trk><name>A <b> trip</name><trkseg>
            <trkpt lat="0.0001" lon="-0.0002"><ele>812.5</ele><time>2026-10-24T08:00:00Z</time></trkpt>
            <trkpt lon='0.0003' lat='-0.0004'/>
            <trkpt lat="91" lon="0"><ele>1</ele></trkpt>
            </trkseg></trk><rte><rtept lat="1.5" lon="2"><ele> -3 </ele></rtept></rte></gpx>"#;
        let want = std::vec![(1000, -2000, Some(8125)), (-4000, 3000, None), (15_000_000, 20_000_000, Some(-30))];
        for chunk in [1, 7, 4096] {
            assert_eq!(gpx_points(text, chunk), want, "chunks of {chunk}");
        }
    }

    /// Long digit runs (review 2: 12 digits and 7 places overflowed i64): refused, the point with them too.
    #[test]
    fn decimal_overflow() {
        assert_eq!(decimal(b"999999999999.5", 7), None);
        assert_eq!(decimal(b"-999999999999", 7), None);
        assert_eq!(decimal(b"922337203686", 7), None, "just past i64");
        assert_eq!(decimal(b"45.5", 7), Some(455_000_000));
        let text = br#"<trkpt lat="999999999999.9" lon="1"/><trkpt lat="1" lon="2"/>"#;
        assert_eq!(gpx_points(text, 4096), [(10_000_000, 20_000_000, None)]);
    }

    /// The synthetic card's courses (tools/fat_image.py): stock's route, a Garmin course and a GPX of the same hill,
    /// listed with their lengths; each reads the same.
    #[test]
    fn synthetic_card() {
        let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
        let mut c = Courses::new();
        c.list.read(&mut d).unwrap();
        let names: Vec<_> = (0..c.list.len()).map(|i| c.list.get(i).unwrap().name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, ["BROKEN", "HILL", "HILL", "HILL GARMIN"], "in the folders' order: {names:?}");
        let broken = names.iter().position(|&n| n == "BROKEN").unwrap();
        for i in (0..4).filter(|&i| i != broken) {
            let e = *c.list.get(i).unwrap();
            assert!(e.dist_m.unwrap().abs_diff(5000) <= 10, "{}: {:?}", e.name(), e.dist_m);
            assert!(c.load(&mut d, i).unwrap(), "{i}");
            let k = c.course.climbs();
            assert_eq!(k.len(), 1, "{i}: {k:?}");
            assert!(k[0].start_m.abs_diff(2000) <= 50 && k[0].gain_m.abs_diff(120) <= 3, "{i}: {k:?}");
        }
        assert!(c.list.get(broken).unwrap().dist_m.is_none(), "not a FIT file: no points");
        assert!(!c.load(&mut d, broken).unwrap());
        assert_eq!(c.on, None);
    }

    /// The owner's routes, from a backup of the card outside the repository (set BRYTON_PLANTRIP to its PlanTrip
    /// folder): every one reads, its length and climbs printed.
    #[test]
    #[ignore]
    fn owner_routes() {
        let dir = std::env::var("BRYTON_PLANTRIP").expect("BRYTON_PLANTRIP");
        for f in std::fs::read_dir(dir).unwrap() {
            let path = f.unwrap().path();
            if path.extension().is_none_or(|e| e != "fit") {
                continue;
            }
            let bytes = std::fs::read(path).unwrap();
            let mut r = Reader::with_position();
            let mut c = Course::new();
            r.push_slice(&bytes, |m| {
                if let Msg::Record(x) = m
                    && let Some((la, lo)) = x.pos
                {
                    c.push(degrees_e7(la), degrees_e7(lo), x.alt_dm);
                }
            });
            c.finish();
            assert_eq!(r.result(), Some(Ok(())));
            std::println!("{} points, {} m, climbs {:?}", c.points().len(), c.len_m(), c.climbs());
        }
    }
}
