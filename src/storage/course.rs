//! Courses on the card, read only: the routes Bryton's app syncs (`\PlanTrip\*.fit`) and the FIT or GPX files
//! copied over USB (`\Courses\*.fit`, `*.gpx`). [`Courses::open`] lists them from their folders, [`Courses::load`]
//! reads one into the course model ([`crate::course`]); nothing is written.
//!
//! Files are read in [`IO`]-byte reads and streamed: a FIT file through the FIT reader with the position
//! ([`crate::fitread::Reader::with_position`]), a GPX file through [`Gpx`]; their points go one by one to the
//! model, which thins them. At most [`MAX_BYTES`] of a file. A file is read a bounded amount per main loop pass
//! ([`Courses::step`]), as stock's rides are scanned: the course to follow, and the lengths of the listed ones (a
//! file at a time), so no pass reads a whole file (review 2: opening COURSES read up to 32 files of 4 MB in one).
//!
//! **FIT courses** (Garmin's FIT SDK, the course encoding cookbook: github.com/garmin/fit-csharp-sdk
//! `Cookbook/CourseEncode/Program.cs`, read 2026-10-24): `file_id` (type 6, course), `course` (name, sport), a
//! `lap`, timer `event`s, then a `record` per point (position_lat, position_long, altitude, distance), with
//! `course_point`s (turns, summits) among them. The records' positions, altitudes and distances are read (the
//! distance: the course's own length, [`crate::course::Odometer`]), and the course points' positions and types
//! (cues, [`crate::course::cues`]).
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
//! The climbs of 250 are not used: the model finds its own the same way for every file. Nor are the turns of
//! `voiceTrip.fit`: their type values could not be established (the Ghidra notes name the turn dialog,
//! `ApplicationTurnInfoDialog_UpdateContent`, not the values; the owner's routes have none), so a route's turns are
//! found on its shape as a GPX track's are.
//!
//! **GPX** (GPX 1.1 schema, www.topografix.com/GPX/1/1/, read 2026-10-24): `<trkpt lat=".." lon="..">` (in
//! `<trk><trkseg>`) and `<rtept>` (in `<rte>`) are `wptType`: `lat` and `lon` attributes in decimal degrees (WGS84),
//! an optional `<ele>` child in metres; the values are `xsd:decimal` (no exponent). [`Gpx`] reads only that: track
//! and route points in file order, their elevation, and where a `trk`, `trkseg` or `rte` starts (a jump there is a
//! gap, [`crate::course::Odometer`]); and cues: a `trkpt`, `rtept` or `wpt` (waypoint) whose `type`, `sym` or
//! `name` (the first, the more specific) is a turn word ([`crate::course::Turn::from_text`]: BRouter's and
//! RideWithGPS's conventions, in course/cues.rs), a track or route point's on that point
//! ([`Item::CueHere`]), a waypoint's by its position; anything else (other waypoints, names, extensions) is skipped. A tag
//! longer than [`TAG`] bytes is cut (its attributes past that are not seen).
//!
//! Nothing here panics or loops on any input (fuzzed in sim.rs `fuzz`): every loop is bounded by the file's size,
//! the list or a buffer.

use crate::course::{Course, Item, Odometer, Pt, Tracker, Turn};
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
    /// Its length was measured (else not yet).
    pub measured: bool,
}

impl Entry {
    const EMPTY: Entry = Entry {
        file: File { cluster: 0, size: 0, dir: false },
        name: [0; NAME],
        name_len: 0,
        gpx: false,
        dist_m: None,
        measured: false,
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

    /// Lists the course files in `DIRS` (the first [`LIST`]) from their folders alone: their names and sizes. Their
    /// lengths come later, a file at a time ([`Courses::step`]).
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
                if let Some(slot) = self.e.get_mut(self.len) {
                    *slot = e;
                    self.len += 1;
                }
            }
        }
        Ok(())
    }

    /// The first course whose length is not known yet.
    fn unmeasured(&self) -> Option<usize> {
        self.e.get(..self.len)?.iter().position(|e| !e.measured)
    }
}

/// A file read a bounded amount at a time ([`Stream::step`]): where it is, and its reader (FIT or GPX).
struct Stream {
    file: File,
    gpx: bool,
    offset: u32,
    fit: Reader,
    xml: Gpx,
}

impl Stream {
    fn new(e: &Entry) -> Self {
        Stream { file: e.file, gpx: e.gpx, offset: 0, fit: Reader::with_position(), xml: Gpx::new() }
    }

    /// Reads on for about `budget` bytes, each point (degrees x 1e-7, altitude in dm) and cue to `f` in order;
    /// true once the file is done (its end, [`MAX_BYTES`], or the end of its FIT data).
    #[inline(never)] // its read buffer lives in its own frame
    fn step(
        &mut self,
        fat: &mut Fat,
        d: &mut impl Disk,
        budget: u32,
        f: &mut dyn FnMut(Item),
    ) -> Result<bool, fat::Error> {
        let mut io = [0u8; IO];
        let (size, mut spent) = (self.file.size.min(MAX_BYTES), 0u32);
        while self.offset < size && spent < budget {
            let n = fat.read_at(d, &self.file, self.offset, &mut io)?;
            if n == 0 {
                return Ok(true);
            }
            (self.offset, spent) = (self.offset.saturating_add(n as u32), spent.saturating_add(n as u32));
            let buf = io.get(..n).unwrap_or(&[]);
            if self.gpx {
                self.xml.push_slice(buf, f);
                continue;
            }
            self.fit.push_slice(buf, |m| match m {
                Msg::Record(r) => {
                    if let Some((lat, lon)) = r.pos {
                        let (lat, lon, dist_m) = (degrees_e7(lat), degrees_e7(lon), r.dist_cm.map(|cm| cm / 100));
                        f(Item::Point(Pt { lat, lon, alt_dm: r.alt_dm, split: false, dist_m }));
                    }
                }
                Msg::CoursePoint { pos: Some((lat, lon)), kind: Some(k), dist_cm } => {
                    f(Item::Cue(degrees_e7(lat), degrees_e7(lon), Turn::from_fit(k), dist_cm.map(|cm| cm / 100)));
                }
                _ => {}
            });
            if self.fit.result().is_some() {
                return Ok(true);
            }
        }
        Ok(self.offset >= size)
    }
}

/// The file being read across the main loop's passes.
enum Job {
    /// None.
    Idle,
    /// Course `k`'s length so far.
    Measure { k: usize, s: Stream, odo: Odometer },
    /// Course `k` read into the model, to follow it.
    Load { k: usize, s: Stream },
}

/// What a [`Courses::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Nothing to read.
    Idle,
    /// Read on; not done yet.
    Reading,
    /// A course's length is known (the list changed).
    Measured,
    /// The course asked for ([`Courses::load`]) is read: followed if true (two points at least).
    Loaded(bool),
}

/// The course memory: the list, the course followed and where the rider is on it, and the file being read. About
/// 70 KB: main.rs keeps it in AXI SRAM and hands it to the app.
pub struct Courses {
    /// The course files found.
    pub list: List,
    /// The course read last.
    pub course: Course,
    /// Where the rider is on it.
    pub at: Tracker,
    /// The course followed (its file); None: none.
    pub on: Option<File>,
    job: Job,
}

impl Default for Courses {
    fn default() -> Self {
        Self::new()
    }
}

impl Courses {
    /// Nothing listed or followed.
    pub const fn new() -> Self {
        Courses { list: List::new(), course: Course::new(), at: Tracker::new(), on: None, job: Job::Idle }
    }

    /// Lists the course files (their folders only: quick); their lengths are measured by the next steps.
    pub fn open(&mut self, d: &mut impl Disk) -> Result<(), fat::Error> {
        if matches!(self.job, Job::Measure { .. }) {
            self.job = Job::Idle;
        }
        self.list.read(d)?;
        self.next_job();
        Ok(())
    }

    /// Reads course `k` of the list, over the next steps, to follow it (none is followed meanwhile).
    pub fn load(&mut self, k: usize) {
        let Some(e) = self.list.get(k) else { return };
        self.job = Job::Load { k, s: Stream::new(e) };
        self.on = None;
        self.course.clear();
    }

    /// Lengths are still being measured.
    pub fn measuring(&self) -> bool {
        matches!(self.job, Job::Measure { .. })
    }

    /// The next course to measure, if any.
    fn next_job(&mut self) {
        let next = self.list.unmeasured().and_then(|k| Some((k, Stream::new(self.list.get(k)?))));
        if let (Job::Idle, Some((k, s))) = (&self.job, next) {
            self.job = Job::Measure { k, s, odo: Odometer::new() };
        }
    }

    /// Reads on for about `budget` bytes: the course being loaded, else the next length to measure. A file that
    /// cannot be read ends as it is (no length; not followed): the others go on.
    #[inline(never)]
    pub fn step(&mut self, d: &mut impl Disk, budget: u32) -> Progress {
        if matches!(self.job, Job::Idle) {
            return Progress::Idle;
        }
        let fat = Fat::mount(d);
        let progress = match &mut self.job {
            Job::Idle => Progress::Idle,
            Job::Measure { k, s, odo } => {
                let done = fat.and_then(|mut fat| {
                    s.step(&mut fat, d, budget, &mut |it| {
                        if let Item::Point(pt) = it {
                            odo.step(pt);
                        }
                    })
                });
                if done == Ok(false) {
                    return Progress::Reading;
                }
                if let Some(e) = self.list.e.get_mut(..self.list.len).and_then(|e| e.get_mut(*k)) {
                    e.dist_m = odo.len_m();
                    e.measured = true;
                }
                Progress::Measured
            }
            Job::Load { k, s } => {
                let course = &mut self.course;
                let done = fat.and_then(|mut fat| s.step(&mut fat, d, budget, &mut |it| course.take(it)));
                if done == Ok(false) {
                    return Progress::Reading;
                }
                course.finish();
                let ok = done.is_ok() && course.points().len() >= 2;
                let file = self.list.get(*k).map(|e| e.file);
                (self.on, self.at) = (file.filter(|_| ok), Tracker::new());
                Progress::Loaded(ok && file.is_some())
            }
        };
        self.job = Job::Idle;
        self.next_job();
        progress
    }
}

/// A GPX reader, a byte at a time: track and route points (`trkpt`, `rtept`) with their elevation, and the cues of
/// points and waypoints (`wpt`) whose `type`, `sym` or `name` is a turn word (see the module doc).
pub struct Gpx {
    tag: [u8; TAG],
    tag_len: usize,
    in_tag: bool,
    /// Inside a point's `ele`, `type`, `sym` or `name`: which, and its text so far (longer: past the end, unread).
    text: Option<(Text, [u8; 24], usize)>,
    /// The point open.
    pt: Option<Open>,
    /// A track, segment or route has started since the last point: the next one starts a segment.
    split: bool,
}

/// A point's child element whose text is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Text {
    Ele,
    /// A turn word, by rank: `name` 1, `sym` 2, `type` 3 (the more specific wins).
    Cue(u8),
}

/// A point being read: latitude, longitude (degrees x 1e-7), elevation (dm), a waypoint (a cue only, not a point of
/// the way), its cue and the rank of the element it came from.
#[derive(Clone, Copy, Debug)]
struct Open {
    lat: i32,
    lon: i32,
    alt: Option<i32>,
    wpt: bool,
    cue: Option<(u8, Turn)>,
}

impl Default for Gpx {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpx {
    /// At the start of a file.
    pub const fn new() -> Self {
        Gpx { tag: [0; TAG], tag_len: 0, in_tag: false, text: None, pt: None, split: false }
    }

    /// The next bytes of the file; each point and cue to `f` as it ends.
    pub fn push_slice(&mut self, buf: &[u8], f: &mut dyn FnMut(Item)) {
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
                if let (Some((what, text, n)), Some(p)) = (self.text.take(), self.pt.as_mut()) {
                    // past the buffer: unread (None), not cut
                    let t = text.get(..n).unwrap_or(b"?").trim_ascii();
                    match what {
                        Text::Ele => p.alt = decimal(t, 1).and_then(|v| i32::try_from(v).ok()),
                        Text::Cue(rank) => {
                            if let Some(turn) = Turn::from_text(t).filter(|_| p.cue.is_none_or(|c| c.0 < rank)) {
                                p.cue = Some((rank, turn));
                            }
                        }
                    }
                }
            } else if let Some((_, text, n)) = self.text.as_mut() {
                if let Some(slot) = text.get_mut(*n) {
                    *slot = b;
                }
                *n = (*n + 1).min(text.len() + 1);
            }
        }
    }

    /// A whole tag (between `<` and `>`).
    fn tag_end(&mut self, f: &mut dyn FnMut(Item)) {
        let t = self.tag.get(..self.tag_len).unwrap_or(&[]);
        let name = t.split(|&b| b.is_ascii_whitespace() || b == b'/' && t.first() != Some(&b'/')).next();
        let text = |w| Some((w, [0; 24], 0));
        match name.unwrap_or(&[]) {
            n @ (b"trkpt" | b"rtept" | b"wpt") => {
                let wpt = n == b"wpt";
                let ll = attr(t, b"lat").zip(attr(t, b"lon"));
                self.pt =
                    ll.filter(|&(la, lo)| la.abs() <= 900_000_000 && lo.abs() <= 1_800_000_000).map(|(la, lo)| {
                        // in range: the casts keep the values
                        Open { lat: la as i32, lon: lo as i32, alt: None, wpt, cue: None }
                    });
                if t.last() == Some(&b'/') {
                    self.emit(f);
                }
            }
            b"/trkpt" | b"/rtept" | b"/wpt" => self.emit(f),
            b"ele" => self.text = text(Text::Ele),
            b"name" => self.text = text(Text::Cue(1)),
            b"sym" => self.text = text(Text::Cue(2)),
            b"type" => self.text = text(Text::Cue(3)),
            b"trk" | b"trkseg" | b"rte" => self.split = true,
            _ => {}
        }
    }

    fn emit(&mut self, f: &mut dyn FnMut(Item)) {
        let Some(p) = self.pt.take() else { return };
        if !p.wpt {
            let split = core::mem::take(&mut self.split);
            f(Item::Point(Pt { lat: p.lat, lon: p.lon, alt_dm: p.alt, split, dist_m: None }));
        }
        if let Some((_, turn)) = p.cue {
            // a track point's cue is on that very point: placed on its pass of the course (an out-and-back's way
            // back, a lollipop's loop), not on the first pass through the same place
            f(if p.wpt { Item::Cue(p.lat, p.lon, turn, None) } else { Item::CueHere(turn) });
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
            g.push_slice(part, &mut |it| {
                if let Item::Point(p) = it {
                    v.push((p.lat, p.lon, p.alt_dm));
                }
            });
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

    /// Each track, segment and route starts a segment at its first point; the list's length leaves out a jump of
    /// more than 1 km where one starts (as the model, course.rs `gaps_between_segments`).
    #[test]
    fn gpx_segments() {
        let text = br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"/><trkpt lat="0.01" lon="0"/></trkseg>
            <trkseg><trkpt lat="0.011" lon="0"/></trkseg></trk>
            <trk><trkseg><trkpt lat="0.1" lon="0"/><trkpt lat="0.11" lon="0"/></trkseg></trk>
            <rte><rtept lat="0.2" lon="0"/></rte></gpx>"#;
        let (mut g, mut v, mut odo) = (Gpx::new(), Vec::new(), Odometer::new());
        g.push_slice(text, &mut |it| {
            if let Item::Point(p) = it {
                odo.step(p);
                v.push(p.split);
            }
        });
        assert_eq!(v, [true, false, true, true, false, true]);
        let m = odo.len_m().unwrap();
        assert!(m.abs_diff(1112 + 111 + 1112) <= 2, "the 9.9 km and 10 km jumps left out: {m}");
    }

    /// Cues in GPX: BRouter's "gpsies" style (`wpt` with `sym` and `type`), its Locus style (`sym` on a track
    /// point), RideWithGPS's (`name` on a waypoint); `type` over `sym` over `name`; a waypoint that is no cue (a
    /// cafe), a word too long for the buffer and a waypoint without a position give none. A waypoint is no point
    /// of the way.
    #[test]
    fn gpx_cues() {
        let text = br#"<gpx><wpt lon="0.001" lat="0.002"><name>right</name><sym>tslr</sym><type>TSLR</type></wpt>
            <wpt lat="0.003" lon="0.004"><name>Left</name></wpt>
            <wpt lat="0.005" lon="0.006"><name>Cafe</name><sym>Water</sym></wpt>
            <wpt lat="0.007" lon="0.008"><name>Cafe du Port</name></wpt>
            <wpt lat="0.009" lon="0.010"><type>leftleftleftleftleftleftleftleft</type></wpt>
            <wpt><name>left</name></wpt>
            <trk><trkseg><trkpt lat="0" lon="0"><ele>1</ele><sym>left_sharp</sym></trkpt>
            <trkpt lat="0.001" lon="0"><name>u-turn</name></trkpt></trkseg></trk></gpx>"#;
        for chunk in [1, 5, 4096] {
            let (mut g, mut v) = (Gpx::new(), Vec::new());
            for part in text.chunks(chunk) {
                g.push_slice(part, &mut |it| v.push(it));
            }
            let pt = |lat, lon, alt_dm, split| Item::Point(Pt { lat, lon, alt_dm, split, dist_m: None });
            assert_eq!(
                v,
                [
                    Item::Cue(20_000, 10_000, Turn::SlightRight, None),
                    Item::Cue(30_000, 40_000, Turn::Left, None),
                    Item::Cue(50_000, 60_000, Turn::Water, None),
                    pt(0, 0, Some(10), true),
                    Item::CueHere(Turn::SharpLeft),
                    pt(10_000, 0, None, false),
                    Item::CueHere(Turn::UTurn),
                ],
                "chunks of {chunk}"
            );
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

    /// Course `k` loaded, a step of 4 KB at a time: how it ended.
    fn load(c: &mut Courses, d: &mut impl Disk, k: usize) -> Progress {
        c.load(k);
        for _ in 0..10_000 {
            match c.step(d, 4096) {
                Progress::Reading => {}
                p => return p,
            }
        }
        panic!("never loaded");
    }

    /// The synthetic card's courses (tools/fat_image.py): stock's route, a Garmin course and a GPX of the same hill,
    /// listed with their lengths; each reads the same.
    #[test]
    fn synthetic_card() {
        let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
        let mut c = Courses::new();
        c.open(&mut d).unwrap();
        assert!(c.list.get(0).is_some_and(|e| !e.measured), "listed from the folders alone");
        let mut steps = 0;
        while c.step(&mut d, 4096) != Progress::Idle {
            steps += 1;
        }
        assert!(steps > 4, "the lengths over several steps of 4 KB: {steps}");
        let names: Vec<std::string::String> = (0..c.list.len()).map(|i| c.list.get(i).unwrap().name().into()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, ["BROKEN", "HILL", "HILL", "HILL GARMIN", "TURNS", "ZIGZAG"], "folders' order: {names:?}");
        let broken = names.iter().position(|n| n == "BROKEN").unwrap();
        for i in (0..names.len()).filter(|&i| names[i].starts_with("HILL")) {
            let e = *c.list.get(i).unwrap();
            assert!(e.dist_m.unwrap().abs_diff(5000) <= 10, "{}: {:?}", e.name(), e.dist_m);
            assert_eq!(load(&mut c, &mut d, i), Progress::Loaded(true), "{i}");
            let k = c.course.climbs();
            assert_eq!(k.len(), 1, "{i}: {k:?}");
            assert!(k[0].start_m.abs_diff(2000) <= 50 && k[0].gain_m.abs_diff(120) <= 3, "{i}: {k:?}");
            // its two corners found on its shape (the Garmin file's summit is no turn), the summit kept
            let cues: Vec<_> = c.course.cues().iter().map(|q| (q.at_m, q.turn)).collect();
            let turns: Vec<_> = cues.iter().filter(|q| q.1.is_turn()).collect();
            assert_eq!(turns.iter().map(|q| q.1).collect::<Vec<_>>(), [Turn::Left, Turn::Right], "{i}: {cues:?}");
            assert!(turns[0].0.abs_diff(2000) <= 25 && turns[1].0.abs_diff(4000) <= 25, "{i}: {cues:?}");
            let summits = cues.iter().filter(|q| q.1 == Turn::Summit).count();
            assert_eq!(summits, usize::from(names[i] == "HILL GARMIN"), "{i}: {cues:?}");
        }
        // the zigzag: its FIT course's cues are where the GPX track's turns are found, the water point besides
        let at = |n: &str| names.iter().position(|x| x == n).unwrap();
        let (zigzag, turns_fit) = (at("ZIGZAG"), at("TURNS"));
        assert_eq!(load(&mut c, &mut d, zigzag), Progress::Loaded(true));
        let found: Vec<_> = c.course.cues().iter().map(|q| (q.at_m, q.turn)).collect();
        assert_eq!(load(&mut c, &mut d, turns_fit), Progress::Loaded(true));
        let stated: Vec<_> = c.course.cues().iter().map(|q| (q.at_m, q.turn)).collect();
        let water = stated.iter().position(|q| q.1 == Turn::Water).expect("the water point");
        assert!((3140..=3190).contains(&stated[water].0), "halfway along the winding road: {stated:?}");
        let mut turns = stated.clone();
        turns.remove(water);
        assert_eq!(turns.len(), found.len(), "{stated:?} {found:?}");
        for (s, f) in turns.iter().zip(&found) {
            assert!(s.1 == f.1 && s.0.abs_diff(f.0) <= 12, "{s:?} {f:?}: {stated:?} {found:?}");
        }
        assert!(c.list.get(broken).unwrap().dist_m.is_none(), "not a FIT file: no points");
        assert_eq!(load(&mut c, &mut d, broken), Progress::Loaded(false));
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
