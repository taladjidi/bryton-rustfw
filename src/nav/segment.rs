//! A live segment file, `\Rustfw\Segments\<id>.seg` (docs/segments.md, "The file"): the reader of stock's live segment
//! package, `LiveSegmentData.bin` unzipped; and the rider's PR file beside it, `<id>.pr` ([`PrCopy`], step S5). Pure
//! logic over a byte slice: nothing here touches the card (app/segments.rs reads and writes).
//!
//! **Source:** stock's reconstruction `ghidra/recovered_src/ride/live_segments.c` (bryton repo, read 2026-10-29). Facts
//! only (offsets, sizes, types); no code copied. Its layout, as this module reads it, little endian:
//! - header, 0x200 bytes: `b0` at 0x00 (not read); id u32 at 0x01; version u32 at 0x05 (stock's update counter);
//!   fit_offset u32 at 0x09; name, 32 bytes at 0x0d (ASCII, NUL padded); distance u32 m at 0x2d; a u32 at 0x31
//!   (unknown: not read); elevation u32 m at 0x35 (the gain; feet are a display conversion); 40 (lat, lon) f32 pairs at
//!   0x39 (stock's start detection reads them); ntargets u8 at 0x179; targets of 0x11 bytes from 0x17a: the type (two
//!   ASCII digits), one byte (not read; inferred 0), the label (10 bytes), the total time u32 s; at most 7 (the area
//!   ends at 0x1fd); the enabled flags at 0x1fd, one per bike (the comment gives the values 1/2/3: inferred to be
//!   flags, read as 3 bytes).
//! - data, from 0x200: npoints u32; ntargets u32 (the comment says its low byte is used: read as the whole word, which
//!   must equal the header's: inferred); ntargets x 13 descriptor bytes (not read); then u32 times, point by point:
//!   each point's ntargets elapsed seconds, in the target order.
//! - the FIT course, from fit_offset to the end of the file (stock imports it as `\PlanTrip\<id>.fit`): read by
//!   [`crate::fitread`] with its positions and distances, its CRCs checked there.
//!
//! Target types, from the codes of stock's comment (the same effort types as Strava's Live Segments API): 05 KOM, 06
//! QOM, 07 PR, 09 carrot, 11 wolf, 12 most recent, 13 best of the last 5. Any other code is [`Error::Target`].
//!
//! **Cross-check against docs/segments.md:** no difference in offsets, sizes, types or the fit_offset formula. Left as
//! inferred: the meaning of the enabled bytes, the byte at 0x31, and the target's byte after its type. The checks of
//! "Limits checked on read" are this firmware's own (stock checks none of them: inferred, it trusts its own imports).
//!
//! **Storage:** a [`Segment`] is about 10 KB, so a caller keeps one in a static cell and parses into it
//! ([`Segment::parse`]); nothing is allocated. Points are the course model's [`Pt`] (degrees x 1e-7, as
//! [`crate::course`] reads them), so the matcher can reuse the course code. The times of the kept targets are u16
//! seconds: [`PREFER`] names the kept ones, PR first, then KOM, QOM and the most recent.
//!
//! Nothing here panics: every read is checked (`get`, `first_chunk`, `chunks_exact`), and the module denies
//! `clippy::indexing_slicing` outside the tests.

#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::cast_possible_truncation))]

use crate::course::Pt;
use crate::fitread::{self, Msg, Reader};
use crate::history::degrees_e7;

/// Bytes of a file at most (stock's package is a few KB; the tool decimates a segment to [`POINTS`] points).
pub const MAX_BYTES: usize = 64 * 1024;
/// The header's size: the index reads only this much.
pub const HEADER: usize = 0x200;
/// Points at most, and at least 2 (a segment has a start and a finish).
pub const POINTS: usize = 400;
/// Bytes read from the card at a time by [`Segment::parse_with`].
pub const CHUNK: usize = 1024;
/// Targets in a file at most (the header area after 0x17a ends at 0x1fd).
pub const TARGETS: usize = 7;
/// Targets whose times are kept per point (the first of each effort in [`PREFER`]).
pub const KEPT: usize = 4;
/// The (lat, lon) pairs in the header (stock's `latlon`, 0x140 bytes).
pub const START_PAIRS: usize = 40;
/// The efforts whose times are kept, in column order: the rider's PR first (the PR file of S5 wins over the source's),
/// then the KOM and the QOM, the competitors every rider compares with, then the most recent. The others (carrot,
/// wolf, best of 5) are read past and dropped, so a file with all seven fits the same storage.
pub const PREFER: [Effort; KEPT] = [Effort::Pr, Effort::Kom, Effort::Qom, Effort::Recent];

const NAME: usize = 32;
const TARGETS_AT: usize = 0x17a;
const TARGET_LEN: usize = 0x11;
const ENABLED_AT: usize = 0x1fd;
/// The data section: npoints and ntargets (u32 each), then a 13-byte descriptor per target, then the times.
const DATA: usize = HEADER;
const DESC: usize = 13;

/// What a target measures: stock's type codes, which are Strava's effort types.
// Discriminants from 1: an `Option<Effort>` is None at 0, so a `Segment` of zeros is the empty one (a static in
// AXI SRAM, which starts all zeros: AGENTS.md, Memory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Effort {
    /// `05`: the KOM (the fastest time of all).
    Kom = 1,
    /// `06`: the QOM (the fastest woman's time).
    Qom,
    /// `07`: the rider's own best.
    Pr,
    /// `09`: the carrot (a pace to beat, a time set by a rider the app picks).
    Carrot,
    /// `11`: the wolf (the rider behind, to stay ahead of).
    Wolf,
    /// `12`: the most recent effort.
    Recent,
    /// `13`: the best of the last 5 efforts.
    Best5,
}

impl Effort {
    /// The effort of a two-digit type code (stock's `05` to `13`), or None for any other code.
    ///
    /// ```
    /// use bryton_rustfw::segment::Effort;
    /// assert_eq!(Effort::from_code(*b"07"), Some(Effort::Pr));
    /// assert_eq!(Effort::from_code(*b"08"), None);
    /// ```
    pub fn from_code(code: [u8; 2]) -> Option<Self> {
        match code {
            [b'0', b'5'] => Some(Effort::Kom),
            [b'0', b'6'] => Some(Effort::Qom),
            [b'0', b'7'] => Some(Effort::Pr),
            [b'0', b'9'] => Some(Effort::Carrot),
            [b'1', b'1'] => Some(Effort::Wolf),
            [b'1', b'2'] => Some(Effort::Recent),
            [b'1', b'3'] => Some(Effort::Best5),
            _ => None,
        }
    }
}

/// One target of the file: its effort, its label (raw bytes, NUL padded; empty means the effort's default name) and
/// its total time in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Goal {
    /// What it measures.
    pub effort: Effort,
    /// The label as the file has it (10 bytes, NUL padded).
    pub label: [u8; 10],
    /// The total time, s.
    pub total_s: u32,
}

/// The header of a segment file: what the index reads (0x200 bytes, [`Header::parse`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The segment's id (the source's decimal id; the file's name).
    pub id: u32,
    /// The update counter of the file.
    pub version: u32,
    /// Where the FIT course starts in the file (checked against the data section by [`Segment::parse`]).
    pub fit_offset: u32,
    /// The name, raw (ASCII, NUL padded); [`Header::name`] is the part before the NUL.
    pub name: [u8; NAME],
    /// The length of the segment, m.
    pub distance_m: u32,
    /// The elevation gain, m.
    pub elevation_m: u32,
    /// The first (lat, lon) pairs, degrees x 1e-7 (stock's start detection reads them). A pair that is not finite or
    /// not on the globe is stored as (0, 0) and ignored (an unused pair is zeros in the files seen: inferred).
    pub start: [(i32, i32); START_PAIRS],
    /// The targets in the file, 1 to [`TARGETS`].
    pub ntargets: usize,
    /// The targets; the first `ntargets` are Some (see [`Header::targets`]).
    pub targets: [Option<Goal>; TARGETS],
    /// The enabled flags, one per bike (0x1fd..0x200).
    pub enabled: [u8; 3],
}

impl Header {
    /// The header of a file, from its first [`HEADER`] bytes only (the index reads no more). Checks what the header
    /// alone can: its length, the target count (1 to [`TARGETS`]), each target's type code ([`Error::Target`]) and
    /// the start pairs on the globe.
    ///
    /// ```
    /// use bryton_rustfw::segment::{Error, Header};
    /// assert_eq!(Header::parse(&[0; 0x100]), Err(Error::Short));
    /// ```
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        if b.len() < HEADER {
            return Err(Error::Short);
        }
        let ntargets = usize::from(byte_at(b, 0x179)?);
        if !(1..=TARGETS).contains(&ntargets) {
            return Err(Error::Targets);
        }
        let mut targets = [None; TARGETS];
        for (k, slot) in targets.iter_mut().enumerate().take(ntargets) {
            let at = TARGETS_AT + TARGET_LEN * k;
            let effort = Effort::from_code(le(b, at)?).ok_or(Error::Target)?;
            *slot = Some(Goal { effort, label: le(b, at + 3)?, total_s: u32_at(b, at + 13)? });
        }
        let mut start = [(0, 0); START_PAIRS];
        for (i, pair) in start.iter_mut().enumerate() {
            let at = 0x39 + 8 * i;
            // an unused or broken pair is (0, 0): stock's start detection never reads it (inferred)
            *pair = match (e7(f32_at(b, at)?, 90.0), e7(f32_at(b, at + 4)?, 180.0)) {
                (Ok(lat), Ok(lon)) => (lat, lon),
                _ => (0, 0),
            };
        }
        Ok(Header {
            id: u32_at(b, 0x01)?,
            version: u32_at(b, 0x05)?,
            fit_offset: u32_at(b, 0x09)?,
            name: le(b, 0x0d)?,
            distance_m: u32_at(b, 0x2d)?,
            elevation_m: u32_at(b, 0x35)?,
            start,
            ntargets,
            targets,
            enabled: le(b, ENABLED_AT)?,
        })
    }

    /// The targets of the file, in file order.
    pub fn targets(&self) -> impl Iterator<Item = Goal> + '_ {
        self.targets.iter().flatten().copied()
    }

    /// The name: the bytes before the first NUL.
    pub fn name(&self) -> &[u8] {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(NAME);
        self.name.get(..end).unwrap_or(&[])
    }

    const fn blank() -> Self {
        Header {
            id: 0,
            version: 0,
            fit_offset: 0,
            name: [0; NAME],
            distance_m: 0,
            elevation_m: 0,
            start: [(0, 0); START_PAIRS],
            ntargets: 0,
            targets: [None; TARGETS],
            enabled: [0; 3],
        }
    }
}

/// A parsed segment: its header, its points, and the times of the kept targets at each point. About 10.2 KB (the altitudes 1.6 KB of it): keep it
/// in a static cell. Not `Copy`: copying it would copy that much.
#[derive(Debug)]
pub struct Segment {
    /// The header of the file.
    pub header: Header,
    /// Points in use (`[..npoints]` of each array); 0 when the last [`Segment::parse`] failed.
    pub npoints: usize,
    /// Latitude of each point, degrees x 1e-7.
    pub lat: [i32; POINTS],
    /// Longitude of each point, degrees x 1e-7.
    pub lon: [i32; POINTS],
    /// Distance of each point from the start of the segment, m: the FIT record's distance, else the length of the
    /// steps before it ([`crate::course::step_mm`]).
    pub dist_m: [u32; POINTS],
    /// Altitude of each point, dm (the FIT record's; meaningful only when `has_alt`).
    pub alt_dm: [i32; POINTS],
    /// Every point of the file had an altitude: the profile ([`Segment::alt_at`]) exists.
    pub has_alt: bool,
    /// The effort of each kept column of `times` (None: no such target in the file).
    pub kept: [Option<Effort>; KEPT],
    /// `times[point][column]`: the elapsed seconds of the kept effort in that column at that point (0 in a column
    /// with no effort).
    pub times: [[u16; KEPT]; POINTS],
}

impl Default for Segment {
    fn default() -> Self {
        Self::new()
    }
}

impl Segment {
    /// An empty segment: the storage a caller keeps (in a static cell, it is large), parsed into by [`Segment::parse`].
    pub const fn new() -> Self {
        Segment {
            header: Header::blank(),
            npoints: 0,
            lat: [0; POINTS],
            lon: [0; POINTS],
            dist_m: [0; POINTS],
            alt_dm: [0; POINTS],
            has_alt: false,
            kept: [None; KEPT],
            times: [[0; KEPT]; POINTS],
        }
    }

    /// Point `i` as a course point ([`Pt`]: position, altitude if the file has them, distance along the segment), or None
    /// past [`Segment::npoints`].
    pub fn pt(&self, i: usize) -> Option<Pt> {
        if i >= self.npoints {
            return None;
        }
        Some(Pt {
            lat: *self.lat.get(i)?,
            lon: *self.lon.get(i)?,
            alt_dm: self.has_alt.then(|| self.alt_dm.get(i).copied()).flatten(),
            split: false,
            dist_m: self.dist_m.get(i).copied(),
        })
    }

    /// The altitude `d` m along the segment, dm, interpolated between the two points around it; None without altitudes.
    pub fn alt_at(&self, d: u32) -> Option<i32> {
        let n = self.npoints.min(POINTS);
        if !self.has_alt || n == 0 {
            return None;
        }
        let dist = self.dist_m.get(..n)?;
        let i = dist.partition_point(|&x| x <= d);
        let a0 = *self.alt_dm.get(i.checked_sub(1)?)?;
        let (d0, d1) = (*dist.get(i - 1)?, dist.get(i).copied());
        let (Some(d1), Some(&a1)) = (d1, self.alt_dm.get(i)) else { return Some(a0) };
        if d1 <= d0 {
            return Some(a0);
        }
        let step = (i64::from(a1) - i64::from(a0)) * i64::from(d.saturating_sub(d0)) / i64::from(d1 - d0);
        i32::try_from(i64::from(a0) + step).ok().or(Some(a0))
    }

    /// Parses a whole file into `self`, with every check of the module doc. Everything from an earlier parse is
    /// cleared first, so a failed parse or a file with fewer targets never shows the last file's data. On an error
    /// the segment has no points (`npoints` is 0).
    pub fn parse(&mut self, b: &[u8]) -> Result<(), Error> {
        self.parse_with(b.len(), |at, buf| {
            let src = b.get(at..)?;
            let n = src.len().min(buf.len());
            buf.get_mut(..n)?.copy_from_slice(src.get(..n)?);
            Some(n)
        })
    }

    /// [`Segment::parse`] of a file of `size` bytes that is read in pieces of [`CHUNK`] bytes, so the caller needs no
    /// buffer of the whole file (up to [`MAX_BYTES`]). `read(at, buf)` fills `buf` from byte `at` of the file and gives
    /// the bytes it put there (0 at the end), or None when the card failed ([`Error::Io`]).
    pub fn parse_with(
        &mut self,
        size: usize,
        mut read: impl FnMut(usize, &mut [u8]) -> Option<usize>,
    ) -> Result<(), Error> {
        self.npoints = 0;
        self.header = Header::blank();
        self.kept = [None; KEPT];
        self.times = [[0; KEPT]; POINTS];
        self.has_alt = false;
        if size > MAX_BYTES {
            return Err(Error::TooBig);
        }
        let mut head = [0u8; DATA + 8];
        read_all(&mut read, 0, &mut head)?;
        let header = Header::parse(&head)?;
        let n = header.ntargets;
        let np = usize::try_from(u32_at(&head, DATA)?).map_err(|_| Error::Points)?;
        if !(2..=POINTS).contains(&np) {
            return Err(Error::Points);
        }
        if usize::try_from(u32_at(&head, DATA + 4)?) != Ok(n) {
            return Err(Error::Targets);
        }
        let times_at = DATA + 8 + DESC * n;
        let fit_at = times_at + 4 * np * n;
        if usize::try_from(header.fit_offset) != Ok(fit_at) {
            return Err(Error::FitOffset);
        }
        if fit_at > size {
            return Err(Error::Short);
        }
        let mut chunk = [0u8; CHUNK];

        // the times: each target's must not fall along the points; the kept ones fit a u16 (a time past 65535 s
        // rejects the file). The total time of a target is not checked against its column's last time.
        let cols = keep(&header);
        let mut prev = [0u32; TARGETS];
        let (mut word, mut have, mut k) = ([0u8; 4], 0usize, 0usize);
        let mut at = times_at;
        while at < fit_at {
            let part = chunk.get_mut(..CHUNK.min(fit_at - at)).ok_or(Error::Short)?;
            read_all(&mut read, at, part)?;
            at += part.len();
            for &byte in part.iter() {
                *word.get_mut(have).ok_or(Error::Short)? = byte;
                have += 1;
                if have < 4 {
                    continue;
                }
                (have, k) = (0, k + 1);
                let (i, t) = ((k - 1) / n, (k - 1) % n);
                let v = u32::from_le_bytes(word);
                let last = prev.get_mut(t).ok_or(Error::Times)?;
                if v < *last {
                    return Err(Error::Times);
                }
                *last = v;
                if let Some(c) = cols.iter().position(|&x| x == Some(t)) {
                    let w = u16::try_from(v).map_err(|_| Error::Times)?;
                    let cell = self.times.get_mut(i).and_then(|row| row.get_mut(c)).ok_or(Error::Times)?;
                    *cell = w;
                }
            }
        }

        // the course: its records are the points, one each, with a position on the globe
        let mut rd = Reader::with_position();
        let (mut got, mut bad) = (0usize, None::<Error>);
        let (la, lo, di, al) = (&mut self.lat, &mut self.lon, &mut self.dist_m, &mut self.alt_dm);
        let mut alts = 0usize;
        let (mut at, mut used) = (fit_at, 0usize);
        while at < size && bad.is_none() {
            let part = chunk.get_mut(..CHUNK.min(size - at)).ok_or(Error::Short)?;
            read_all(&mut read, at, part)?;
            at += part.len();
            let took = rd.push_slice(part, |m| {
                let Msg::Record(r) = m else { return };
                if bad.is_some() {
                    return;
                }
                let Some((lat, lon)) = r.pos else {
                    bad = Some(Error::NoPosition);
                    return;
                };
                let (lat, lon) = (degrees_e7(lat), degrees_e7(lon));
                if lat.unsigned_abs() > 900_000_000 || lon.unsigned_abs() > 1_800_000_000 {
                    bad = Some(Error::Earth);
                    return;
                }
                if got >= np {
                    bad = Some(Error::Points);
                    return;
                }
                let prev = got.checked_sub(1).and_then(|j| Some((*la.get(j)?, *lo.get(j)?, *di.get(j)?)));
                let dist = match (r.dist_cm, prev) {
                    (Some(cm), _) => cm / 100,
                    (None, Some((pla, plo, pd))) => {
                        let step_m = u32::try_from(crate::course::step_mm((pla, plo), (lat, lon)) / 1000);
                        pd.saturating_add(step_m.unwrap_or(u32::MAX))
                    }
                    (None, None) => 0,
                };
                match (la.get_mut(got), lo.get_mut(got), di.get_mut(got), al.get_mut(got)) {
                    (Some(a), Some(o), Some(d), Some(h)) => {
                        (*a, *o, *d) = (lat, lon, dist);
                        // the profile exists only when every point had an altitude
                        (*h, alts) = r.alt_dm.map_or((0, alts), |v| (v, alts + 1));
                        got += 1;
                    }
                    _ => bad = Some(Error::Points),
                }
            });
            used += took;
            if took < part.len() {
                break; // the reader is done: what follows is trailing
            }
        }
        if let Some(e) = bad {
            return Err(e);
        }
        match rd.result() {
            None => return Err(Error::FitTruncated),
            Some(Err(fitread::Error::Header)) => return Err(Error::FitHeader),
            Some(Err(fitread::Error::Undefined)) => return Err(Error::FitUndefined),
            Some(Err(fitread::Error::Truncated)) => return Err(Error::FitTruncated),
            Some(Err(fitread::Error::Crc)) => return Err(Error::FitCrc),
            Some(Ok(())) => {}
        }
        if used != size - fit_at {
            return Err(Error::Trailing);
        }
        if got != np {
            return Err(Error::Points);
        }
        self.has_alt = alts == np;
        self.kept = cols.map(|c| c.and_then(|t| header.targets().nth(t)).map(|g| g.effort));
        self.header = header;
        self.npoints = np;
        Ok(())
    }
}

/// Fills `buf` from byte `at` of the file in as many reads as it takes; each must give bytes (the loop ends).
fn read_all(read: &mut impl FnMut(usize, &mut [u8]) -> Option<usize>, at: usize, buf: &mut [u8]) -> Result<(), Error> {
    let mut done = 0;
    while done < buf.len() {
        let rest = buf.get_mut(done..).ok_or(Error::Io)?;
        match read(at + done, rest).ok_or(Error::Io)? {
            0 => return Err(Error::Short),
            n if n <= rest.len() => done += n,
            _ => return Err(Error::Io),
        }
    }
    Ok(())
}

/// The kept targets: for each of [`PREFER`], the index of the file's first target of that effort (None if none).
fn keep(h: &Header) -> [Option<usize>; KEPT] {
    let mut out = [None; KEPT];
    for (slot, e) in out.iter_mut().zip(PREFER) {
        *slot = h.targets().position(|g| g.effort == e);
    }
    out
}

/// Why a file is not a live segment this reader takes. Every check has its own variant; a file failing one is
/// skipped (and counted by the caller), never a panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// More than [`MAX_BYTES`].
    TooBig,
    /// Shorter than its header, its data section, or the `fit_offset` it states.
    Short,
    /// The header's target count is not 1 to [`TARGETS`], or the data section's count differs from it.
    Targets,
    /// A target's type code is none of stock's (see [`Effort::from_code`]).
    Target,
    /// `npoints` is not 2 to [`POINTS`], or the course has a different number of records.
    Points,
    /// `fit_offset` is not 0x200 + 8 + 13 x ntargets + 4 x npoints x ntargets.
    FitOffset,
    /// A target's time falls along the points, or a kept target's time does not fit a u16.
    Times,
    /// A position (a header pair or a course point) is not finite or not on the globe.
    Earth,
    /// A course record has no position.
    NoPosition,
    /// The FIT course's header is not a FIT header, or its header CRC is wrong.
    FitHeader,
    /// The FIT course has a data message on a local type it never defined.
    FitUndefined,
    /// The FIT course ends inside a message, or before its data size.
    FitTruncated,
    /// The FIT course's file CRC is wrong.
    FitCrc,
    /// Bytes follow the FIT course's end (its CRC).
    Trailing,
    /// The card failed while the file was read.
    Io,
}

fn le<const N: usize>(b: &[u8], at: usize) -> Result<[u8; N], Error> {
    b.get(at..).and_then(|s| s.first_chunk::<N>()).copied().ok_or(Error::Short)
}

fn byte_at(b: &[u8], at: usize) -> Result<u8, Error> {
    le::<1>(b, at).map(|[v]| v)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    le::<4>(b, at).map(u32::from_le_bytes)
}

fn f32_at(b: &[u8], at: usize) -> Result<f32, Error> {
    le::<4>(b, at).map(f32::from_le_bytes)
}

/// A coordinate of the file (degrees, f32) as degrees x 1e-7, when it is finite and within `limit` degrees.
#[allow(clippy::cast_possible_truncation)] // |v| <= 180, so the product fits an i32 (1.8e9 < 2^31); rounds half away
fn e7(v: f32, limit: f32) -> Result<i32, Error> {
    if v.is_finite() && v.abs() <= limit {
        Ok((f64::from(v) * 1e7 + if v < 0.0 { -0.5 } else { 0.5 }) as i32)
    } else {
        Err(Error::Earth)
    }
}

/// Bytes of one copy of a `.pr` file (two sectors: the header and up to [`POINTS`] u16 times fit).
pub const PR_COPY: usize = 1024;
/// Bytes of a whole `.pr` file: two copies.
pub const PR_FILE: usize = 2 * PR_COPY;
const PR_MAGIC: &[u8; 4] = b"RSPR";
const PR_VERSION: u16 = 1;
const PR_TIMES: usize = 24;
const PR_CRC: usize = PR_COPY - 4;

/// Why a copy of a `.pr` file is not read (each check its own, so the fuzz can count them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrError {
    /// Not [`PR_COPY`] bytes.
    Size,
    /// Not "RSPR".
    Magic,
    /// Another layout.
    Version,
    /// `npoints` not 2 to [`POINTS`].
    Points,
    /// The CRC-32 of the copy is wrong (a copy torn by a cut, or never written).
    Crc,
    /// The times fall along the points, the first is not 0, or the last is not the total (0 included).
    Times,
}

/// One copy of `\Rustfw\Segments\<id>.pr`, the rider's best effort on a segment (docs/segments.md, "Efforts"),
/// written by app/segments.rs after a ride is saved, read when the segment is loaded. Two copies of 1024 bytes, the
/// settings file's scheme (app/settings_file.rs): the newest whole copy wins, a write goes over the other one.
/// A copy, little endian:
///
/// ```text
/// 0    "RSPR"           magic
/// 4    version (u16)    1
/// 6    npoints (u16)    2 to 400: the segment's points the times are for
/// 8    seq (u32)        higher is newer
/// 12   id (u32)         the segment's id (the file's name)
/// 16   date (u32)       FIT time of the effort's start (s since 1989-12-31 00:00 UTC)
/// 20   total (u32)      the effort's time, s: the last point's time
/// 24   times            npoints x u16: the elapsed seconds at each point, the first 0, never falling
/// 1020 CRC-32 of 0..1020
/// ```
///
/// ```
/// use bryton_rustfw::segment::{PrCopy, pr_copy};
/// let copy = pr_copy(1, 7, 1_000, &[0, 30, 61]).unwrap();
/// let p = PrCopy::parse(&copy).unwrap();
/// assert_eq!((p.id, p.total_s, p.npoints()), (7, 61, 3));
/// assert_eq!(p.time(1), Some(30));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrCopy<'a> {
    /// Higher is newer.
    pub seq: u32,
    /// The segment.
    pub id: u32,
    /// FIT time of the effort's start.
    pub date: u32,
    /// The effort's time, s.
    pub total_s: u32,
    times: &'a [u8],
}

impl<'a> PrCopy<'a> {
    /// Reads one copy ([`PR_COPY`] bytes), every check of the layout done.
    pub fn parse(c: &'a [u8]) -> Result<Self, PrError> {
        if c.len() != PR_COPY {
            return Err(PrError::Size);
        }
        let u16_at = |at: usize| c.get(at..).and_then(|s| s.first_chunk::<2>()).map(|b| u16::from_le_bytes(*b));
        let u32_at = |at: usize| c.get(at..).and_then(|s| s.first_chunk::<4>()).map(|b| u32::from_le_bytes(*b));
        if c.get(..4) != Some(&PR_MAGIC[..]) {
            return Err(PrError::Magic);
        }
        if u16_at(4) != Some(PR_VERSION) {
            return Err(PrError::Version);
        }
        let np = usize::from(u16_at(6).ok_or(PrError::Size)?);
        if !(2..=POINTS).contains(&np) {
            return Err(PrError::Points);
        }
        if u32_at(PR_CRC) != Some(crate::inflate::crc32(c.iter().take(PR_CRC).copied())) {
            return Err(PrError::Crc);
        }
        let times = c.get(PR_TIMES..PR_TIMES + 2 * np).ok_or(PrError::Size)?;
        let p = PrCopy {
            seq: u32_at(8).ok_or(PrError::Size)?,
            id: u32_at(12).ok_or(PrError::Size)?,
            date: u32_at(16).ok_or(PrError::Size)?,
            total_s: u32_at(20).ok_or(PrError::Size)?,
            times,
        };
        let mut prev = 0u16;
        for (i, t) in p.iter().enumerate() {
            if t < prev || (i == 0 && t != 0) {
                return Err(PrError::Times);
            }
            prev = t;
        }
        if p.total_s == 0 || u32::from(prev) != p.total_s {
            return Err(PrError::Times);
        }
        Ok(p)
    }

    /// The newer of a file's two copies that check out ([`PR_FILE`] bytes), if any.
    pub fn newest(file: &'a [u8]) -> Option<Self> {
        let (a, b) = (file.get(..PR_COPY)?, file.get(PR_COPY..PR_FILE)?);
        match (Self::parse(a).ok(), Self::parse(b).ok()) {
            // serial number order (RFC 1982): a sequence that wrapped past u32::MAX is still newer
            (Some(x), Some(y)) => Some(if (y.seq.wrapping_sub(x.seq) as i32) > 0 { y } else { x }),
            (x, y) => x.or(y),
        }
    }

    /// The points it has times for.
    pub fn npoints(&self) -> usize {
        self.times.len() / 2
    }

    /// The time at point `i`, s.
    pub fn time(&self, i: usize) -> Option<u16> {
        self.times.get(2 * i..).and_then(|s| s.first_chunk::<2>()).map(|b| u16::from_le_bytes(*b))
    }

    /// The times, point by point.
    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.times.chunks_exact(2).map(|b| b.first_chunk::<2>().map_or(0, |x| u16::from_le_bytes(*x)))
    }
}

/// A copy of a `.pr` file ([`PrCopy`]) for `times` (2 to [`POINTS`], the first 0, never falling): None for times
/// that are not those.
pub fn pr_copy(seq: u32, id: u32, date: u32, times: &[u16]) -> Option<[u8; PR_COPY]> {
    let np = u16::try_from(times.len()).ok().filter(|&n| (2..=POINTS).contains(&usize::from(n)))?;
    let mut c = [0u8; PR_COPY];
    fn put(c: &mut [u8], at: usize, v: &[u8]) -> Option<()> {
        c.get_mut(at..at + v.len()).map(|d| d.copy_from_slice(v))
    }
    put(&mut c, 0, PR_MAGIC)?;
    put(&mut c, 4, &PR_VERSION.to_le_bytes())?;
    put(&mut c, 6, &np.to_le_bytes())?;
    put(&mut c, 8, &seq.to_le_bytes())?;
    put(&mut c, 12, &id.to_le_bytes())?;
    put(&mut c, 16, &date.to_le_bytes())?;
    put(&mut c, 20, &u32::from(*times.last()?).to_le_bytes())?;
    for (i, t) in times.iter().enumerate() {
        put(&mut c, PR_TIMES + 2 * i, &t.to_le_bytes())?;
    }
    let crc = crate::inflate::crc32(c.iter().take(PR_CRC).copied());
    put(&mut c, PR_CRC, &crc.to_le_bytes())?;
    PrCopy::parse(&c).ok().map(|_| c)
}

/// Test builders: a valid file and any file the checks can be broken on. Shared with the fuzz (src/sim/fuzz.rs).
#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// A valid file of `np` points due east along the equator from 0 N 10 E, about 50 m apart, with the targets
    /// `codes`, each target's time at point `i` being `10 i + target`, and a FIT course of `np` records.
    pub(crate) fn synth(np: usize, codes: &[[u8; 2]]) -> Vec<u8> {
        let n = codes.len();
        let times: Vec<u32> = (0..np * n).map(|k| 10 * (k / n) as u32 + (k % n) as u32).collect();
        raw(np as u32, n as u32, codes, &times, np, true, 0.0)
    }

    fn put(f: &mut Vec<u8>, at: usize, v: &[u8]) {
        if f.len() < at + v.len() {
            f.resize(at + v.len(), 0);
        }
        f[at..at + v.len()].copy_from_slice(v);
    }

    /// The builder behind every test file: the header with `codes` as its targets (total time 600 + the target's
    /// index), the data section with `np` and `ntargets_field`, `times` as given (point by point), and the FIT course
    /// of `records` records, at latitude `lat_deg`, with their positions or without (`pos`). `fit_offset` is the
    /// formula's value for `np` (so a file with wrong counts fails on the count, not on the offset).
    pub(crate) fn raw(
        np: u32,
        ntargets_field: u32,
        codes: &[[u8; 2]],
        times: &[u32],
        records: usize,
        pos: bool,
        lat_deg: f64,
    ) -> Vec<u8> {
        raw_alt(np, ntargets_field, codes, times, records, pos, lat_deg, None)
    }

    /// Altitude of record `r` in FIT's units ((m + 500) x 5), for [`raw_alt`].
    pub(crate) type AltFn<'a> = &'a dyn Fn(usize) -> Option<u16>;

    /// [`synth`] with a FIT altitude (field 2) in each record where `alt` gives one.
    pub(crate) fn synth_alt(np: usize, codes: &[[u8; 2]], alt: AltFn) -> Vec<u8> {
        let n = codes.len();
        let times: Vec<u32> = (0..np * n).map(|k| 10 * (k / n) as u32 + (k % n) as u32).collect();
        raw_alt(np as u32, n as u32, codes, &times, np, true, 0.0, Some(alt))
    }

    /// [`raw`] with the altitude of each record, when `alt` is given (a record it gives None for carries the field
    /// with FIT's invalid value 0xFFFF).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn raw_alt(
        np: u32,
        ntargets_field: u32,
        codes: &[[u8; 2]],
        times: &[u32],
        records: usize,
        pos: bool,
        lat_deg: f64,
        alt: Option<AltFn>,
    ) -> Vec<u8> {
        let n = codes.len();
        let mut f = std::vec![0u8; HEADER];
        let fit_at = DATA + 8 + DESC * n + 4 * (np as usize) * n;
        put(&mut f, 0x01, &4_000_000_001u32.to_le_bytes());
        put(&mut f, 0x05, &3u32.to_le_bytes());
        put(&mut f, 0x09, &(fit_at as u32).to_le_bytes());
        put(&mut f, 0x0d, &b"Test climb"[..]);
        put(&mut f, 0x2d, &(np * 50).to_le_bytes());
        put(&mut f, 0x35, &20u32.to_le_bytes());
        for i in 0..START_PAIRS {
            put(&mut f, 0x39 + 8 * i, &0f32.to_le_bytes());
            put(&mut f, 0x3d + 8 * i, &(10.0f32 + 0.00045 * i as f32).to_le_bytes());
        }
        put(&mut f, 0x179, &[n as u8]);
        for (k, code) in codes.iter().enumerate() {
            let at = TARGETS_AT + TARGET_LEN * k;
            put(&mut f, at, code);
            put(&mut f, at + 3, &b"LBL"[..]);
            put(&mut f, at + 13, &(600 + k as u32).to_le_bytes());
        }
        put(&mut f, ENABLED_AT, &[1, 0, 0]);
        put(&mut f, DATA, &np.to_le_bytes());
        put(&mut f, DATA + 4, &ntargets_field.to_le_bytes());
        for (k, t) in times.iter().enumerate() {
            put(&mut f, DATA + 8 + DESC * n + 4 * k, &t.to_le_bytes());
        }
        // the FIT course: a definition of record (global 20) with 253 time, 0 and 1 position, 5 distance
        let fields: &[(u8, u8, u8)] = match (pos, alt.is_some()) {
            (true, false) => &[(253, 4, 0x86), (0, 4, 0x85), (1, 4, 0x85), (5, 4, 0x86)],
            (true, true) => &[(253, 4, 0x86), (0, 4, 0x85), (1, 4, 0x85), (5, 4, 0x86), (2, 2, 0x84)],
            (false, false) => &[(253, 4, 0x86), (5, 4, 0x86)],
            (false, true) => &[(253, 4, 0x86), (5, 4, 0x86), (2, 2, 0x84)],
        };
        let mut body = std::vec![0x40, 0, 0, 20, 0, fields.len() as u8];
        for &(num, size, base) in fields {
            body.extend_from_slice(&[num, size, base]);
        }
        for r in 0..records {
            body.push(0);
            body.extend_from_slice(&(r as u32).to_le_bytes());
            if pos {
                let lat = (lat_deg * 2_147_483_648.0 / 180.0).round() as i32;
                let lon = ((10.0 + 0.00045 * r as f64) * 2_147_483_648.0 / 180.0).round() as i32;
                body.extend_from_slice(&lat.to_le_bytes());
                body.extend_from_slice(&lon.to_le_bytes());
            }
            body.extend_from_slice(&(r as u32 * 5000).to_le_bytes());
            if let Some(alt) = alt {
                body.extend_from_slice(&alt(r).unwrap_or(0xFFFF).to_le_bytes());
            }
        }
        let mut fit = std::vec![12u8, 0x10, 0, 0];
        fit.extend_from_slice(&(body.len() as u32).to_le_bytes());
        fit.extend_from_slice(b".FIT");
        fit.extend_from_slice(&body);
        let crc = crate::fit::crc(0, &fit);
        fit.extend_from_slice(&crc.to_le_bytes());
        put(&mut f, fit_at, &fit);
        f
    }

    /// The times of a valid file: `10 i + t` for point `i` and target `t`.
    fn times(np: usize, n: usize) -> Vec<u32> {
        (0..np * n).map(|k| 10 * (k / n) as u32 + (k % n) as u32).collect()
    }

    const VALID: [[u8; 2]; 5] = [*b"07", *b"05", *b"06", *b"12", *b"13"];

    fn err(f: &[u8]) -> Error {
        Segment::new().parse(f).unwrap_err()
    }

    #[test]
    fn round_trip() {
        let f = synth(5, &VALID);
        let mut s = Segment::new();
        assert_eq!(s.parse(&f), Ok(()));
        assert_eq!(s.npoints, 5);
        assert_eq!(s.header.id, 4_000_000_001);
        assert_eq!(s.header.name(), b"Test climb");
        assert_eq!((s.header.distance_m, s.header.elevation_m, s.header.enabled), (250, 20, [1, 0, 0]));
        assert_eq!(s.header.targets().count(), 5);
        // PR, KOM, QOM and the most recent are kept, the best of 5 is not
        assert_eq!(s.kept, [Some(Effort::Pr), Some(Effort::Kom), Some(Effort::Qom), Some(Effort::Recent)]);
        for i in 0..5 {
            let want = [0, 1, 2, 3].map(|t| 10 * i as u16 + t);
            assert_eq!(s.times[i], want, "point {i}");
        }
        // positions go through FIT's semicircles (1e-7 degree each): within 2 units
        assert!(s.lat[0].abs() <= 2 && (s.lon[0] - 100_000_000).abs() <= 2);
        assert!((s.lon[4] - (100_000_000 + 4 * 4500)).abs() <= 2, "lon {}", s.lon[4]);
        // the distance along: FIT's distance, 50 m a record
        assert_eq!(s.dist_m[..5], [0, 50, 100, 150, 200]);
        assert_eq!(s.pt(4).map(|p| (p.lon, p.dist_m)), Some((s.lon[4], Some(200))));
        assert_eq!(s.pt(5), None);
        // the header alone: the index reads it from the first 0x200 bytes
        let h = Header::parse(&f[..HEADER]).unwrap();
        assert_eq!((h.id, h.fit_offset, h.ntargets), (4_000_000_001, s.header.fit_offset, 5));
        assert_eq!(h.start[0], (0, 100_000_000));
    }

    #[test]
    fn sizes_and_lengths() {
        assert_eq!(err(&std::vec![0u8; MAX_BYTES + 1]), Error::TooBig);
        let f = synth(5, &VALID);
        assert_eq!(err(&f[..HEADER - 1]), Error::Short);
        let fit_at = DATA + 8 + DESC * 5 + 4 * 5 * 5;
        assert_eq!(err(&f[..fit_at - 1]), Error::Short);
        assert_eq!(err(&f[..DATA + 4]), Error::Short);
        let mut s = Segment::new();
        assert_eq!(s.parse(&f[..fit_at + 4]), Err(Error::FitTruncated));
        assert_eq!(err(&[f.clone(), std::vec![0]].concat()), Error::Trailing);
        let mut big = f.clone();
        big.resize(MAX_BYTES, 0);
        assert_eq!(err(&big), Error::Trailing);
    }

    #[test]
    fn header_checks() {
        let f = synth(4, &VALID);
        let mut g = f.clone();
        g[0x179] = 0;
        assert_eq!(err(&g), Error::Targets);
        g[0x179] = 8;
        assert_eq!(err(&g), Error::Targets);
        // the data section's count differs from the header's
        assert_eq!(err(&raw(4, 4, &VALID, &times(4, 5), 4, true, 0.0)), Error::Targets);
        let mut g = f.clone();
        g[0x17a..0x17c].copy_from_slice(b"99");
        assert_eq!(err(&g), Error::Target);
        let mut g = f.clone();
        // a start pair off the globe, or NaN, is unused: (0, 0), and the file still parses
        g[0x39..0x3d].copy_from_slice(&91f32.to_le_bytes());
        g[0x3d..0x41].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(Header::parse(&g).map(|h| (h.start[0], h.start[1])), Ok(((0, 0), (0, 100_004_501))));
        assert_eq!(Segment::new().parse(&g), Ok(()));
        let mut g = f.clone();
        g[0x09..0x0d].copy_from_slice(&0x1234u32.to_le_bytes());
        assert_eq!(err(&g), Error::FitOffset);
    }

    #[test]
    fn point_checks() {
        assert_eq!(err(&raw(1, 5, &VALID, &times(1, 5), 1, true, 0.0)), Error::Points);
        assert_eq!(err(&raw(401, 5, &VALID, &times(401, 5), 401, true, 0.0)), Error::Points);
        assert_eq!(err(&raw(4, 5, &VALID, &times(4, 5), 3, true, 0.0)), Error::Points);
        assert_eq!(err(&raw(4, 5, &VALID, &times(4, 5), 5, true, 0.0)), Error::Points);
        assert_eq!(err(&raw(4, 5, &VALID, &times(4, 5), 4, false, 0.0)), Error::NoPosition);
        assert_eq!(err(&raw(4, 5, &VALID, &times(4, 5), 4, true, 95.0)), Error::Earth);
    }

    #[test]
    fn time_checks() {
        // point 2's time of the first target falls below point 1's
        let mut t = times(4, 5);
        t[2 * 5] = 0;
        assert_eq!(err(&raw(4, 5, &VALID, &t, 4, true, 0.0)), Error::Times);
        // a kept target (PR, the first) past a u16: every point, so the times still rise
        let mut t = times(4, 5);
        for i in 0..4 {
            t[i * 5] = 70_000 + 10 * i as u32;
        }
        assert_eq!(err(&raw(4, 5, &VALID, &t, 4, true, 0.0)), Error::Times);
        // the same past a u16 on a target that is not kept (the best of 5) is fine
        let mut t = times(4, 5);
        for i in 0..4 {
            t[i * 5 + 4] = 70_000 + 10 * i as u32;
        }
        assert!(raw(4, 5, &VALID, &t, 4, true, 0.0).len() > HEADER);
        assert_eq!(parse_ok(&raw(4, 5, &VALID, &t, 4, true, 0.0)), Ok(()));
    }

    fn parse_ok(f: &[u8]) -> Result<(), Error> {
        Segment::new().parse(f)
    }

    #[test]
    fn course_checks() {
        let f = synth(4, &VALID);
        let fit_at = DATA + 8 + DESC * 5 + 4 * 4 * 5;
        let mut g = f.clone();
        g[fit_at + 8] = b'X';
        assert_eq!(err(&g), Error::FitHeader);
        let mut g = f.clone();
        let last = g.len() - 1;
        g[last] ^= 0x55;
        assert_eq!(err(&g), Error::FitCrc);
        let mut g = f.clone();
        g[fit_at + 12] = 0;
        assert_eq!(err(&g), Error::FitUndefined);
        assert_eq!(err(&f[..f.len() - 5]), Error::FitTruncated);
        // the header of a valid file parses; so does a single-point course refused for its count
        assert_eq!(parse_ok(&f), Ok(()));
    }

    #[test]
    fn no_targets_kept_is_valid() {
        // only the best of 5 and a carrot: nothing kept, the times are still read and checked
        let f = synth(3, &[*b"09", *b"13"]);
        let mut s = Segment::new();
        assert_eq!(s.parse(&f), Ok(()));
        assert_eq!(s.kept, [None; KEPT]);
        assert_eq!(s.npoints, 3);
    }

    #[test]
    fn a_parse_clears_the_last_one() {
        // a 4-target file, then a 1-target file: the columns the second file does not have are zero and None
        let mut s = Segment::new();
        assert_eq!(s.parse(&synth(4, &VALID)), Ok(()));
        assert_eq!(s.times[3][3], 33);
        assert_eq!(s.parse(&synth(3, &[*b"07"])), Ok(()));
        assert_eq!(s.kept, [Some(Effort::Pr), None, None, None]);
        for i in 0..3 {
            assert_eq!(s.times[i][1..], [0, 0, 0], "point {i}");
        }
        assert_eq!(s.times[2][0], 20);
        // a failed parse leaves nothing of the last file either
        assert!(s.parse(&[0u8; 10]).is_err());
        assert_eq!((s.kept, s.header.ntargets, s.times[0]), ([None; KEPT], 0, [0; KEPT]));
    }

    /// Altitudes: kept per point in dm and interpolated along the distance; a file without them (or with a hole)
    /// parses with no profile.
    #[test]
    fn altitude_profile() {
        let codes = [*b"07"];
        // 100 m at point 0, rising 1 m per 50 m point
        let up = |r: usize| Some(((600 + r) * 5) as u16);
        let mut s = Segment::new();
        s.parse(&synth_alt(20, &codes, &up)).unwrap();
        assert!(s.has_alt);
        assert_eq!(s.pt(3).and_then(|p| p.alt_dm), Some(1030));
        assert_eq!(
            (s.alt_at(0), s.alt_at(50), s.alt_at(75), s.alt_at(950)),
            (Some(1000), Some(1010), Some(1015), Some(1190))
        );
        assert_eq!(s.alt_at(5000), Some(1190), "past the end: the last");
        let mut hole = Segment::new();
        hole.parse(&synth_alt(20, &codes, &|r| (r != 7).then(|| up(r)).flatten())).unwrap();
        assert!(!hole.has_alt && hole.alt_at(100).is_none() && hole.pt(2).unwrap().alt_dm.is_none());
        let mut none = Segment::new();
        none.parse(&synth(20, &codes)).unwrap();
        assert!(none.alt_at(100).is_none());
        none.parse(&synth_alt(20, &codes, &up)).unwrap();
        none.parse(&synth(20, &codes)).unwrap();
        assert!(!none.has_alt, "a later file without them clears the profile");
    }

    #[test]
    fn size_is_bounded() {
        // 400 x (lat, lon, distance: 4 bytes each) + 400 x 4 kept times (2 bytes): 8000 bytes of arrays, the header
        // about 600 more. 6 KB is below the arrays' own size, so the bound is what the layout costs.
        assert!(core::mem::size_of::<Segment>() <= 11 * 1024, "{} bytes", core::mem::size_of::<Segment>());
    }

    #[test]
    fn failed_parse_leaves_no_points() {
        let mut s = Segment::new();
        assert_eq!(s.parse(&synth(4, &VALID)), Ok(()));
        assert_eq!(s.parse(&[0u8; 10]), Err(Error::Short));
        assert_eq!(s.npoints, 0);
    }

    /// Reading in pieces: any read size gives the same segment as the whole file, and a card that fails or ends early
    /// gives an error, never a panic or a loop.
    #[test]
    fn reads_in_pieces() {
        let f = synth(120, &[*b"07", *b"05", *b"12"]);
        let mut whole = Segment::new();
        whole.parse(&f).unwrap();
        for piece in [1usize, 3, 7, 512, 1024, 5000] {
            let mut s = Segment::new();
            let r = s.parse_with(f.len(), |at, buf| {
                let n = piece.min(buf.len()).min(f.len().saturating_sub(at));
                buf.get_mut(..n)?.copy_from_slice(f.get(at..at + n)?);
                Some(n)
            });
            assert_eq!(r, Ok(()), "piece {piece}");
            assert_eq!((s.npoints, s.lat, s.times), (whole.npoints, whole.lat, whole.times));
            assert_eq!((s.dist_m, s.kept, s.header), (whole.dist_m, whole.kept, whole.header));
        }
        let mut s = Segment::new();
        assert_eq!(s.parse_with(f.len(), |_, _| None), Err(Error::Io));
        assert_eq!(s.parse_with(f.len(), |_, _| Some(0)), Err(Error::Short));
        assert_eq!(s.parse_with(f.len(), |_, b| Some(b.len() + 1)), Err(Error::Io));
        let mut n = 0;
        let mut dies = |at: usize, buf: &mut [u8]| {
            n += 1;
            let k = buf.len().min(f.len() - at);
            buf.get_mut(..k)?.copy_from_slice(f.get(at..at + k)?);
            (n < 4).then_some(k)
        };
        assert_eq!(s.parse_with(f.len(), &mut dies), Err(Error::Io));
        assert_eq!(s.npoints, 0);
    }

    /// The file `tools/segments.py fixture` writes (synthetic: a straight line at 0 N 10 E; its selftest checks the
    /// committed copy equals a fresh one), read by this reader.
    #[test]
    fn the_tools_fixture_parses() {
        let f = include_bytes!("../../test-data/segment/synthetic.seg");
        let mut s = Segment::new();
        assert_eq!(s.parse(f), Ok(()));
        assert_eq!((s.npoints, s.header.ntargets, s.header.distance_m, s.header.id), (82, 4, 2000, 4_000_000_001));
        assert_eq!(s.kept, [Some(Effort::Pr), Some(Effort::Kom), None, Some(Effort::Recent)]);
        assert_eq!(s.times.get(81).copied(), Some([370, 330, 0, 415]));
        assert_eq!(s.dist_m.get(81).copied(), Some(2000));
    }

    /// A PR file: a copy round-trips, the newest whole copy wins, a torn one leaves the other, and each check fails
    /// on its own field.
    #[test]
    fn pr_copies() {
        let times: Vec<u16> = (0..20).map(|i| 8 * i).collect();
        let a = pr_copy(4, 9, 77, &times).unwrap();
        let p = PrCopy::parse(&a).unwrap();
        assert_eq!((p.seq, p.id, p.date, p.total_s, p.npoints()), (4, 9, 77, 152, 20));
        assert_eq!(p.iter().collect::<Vec<_>>(), times);
        let b = pr_copy(5, 9, 78, &times).unwrap();
        let mut file = [a, b].concat();
        assert_eq!(PrCopy::newest(&file).unwrap().seq, 5);
        // a sequence past u32::MAX is newer still
        let wrapped = [pr_copy(u32::MAX, 9, 0, &times).unwrap(), pr_copy(0, 9, 0, &times).unwrap()].concat();
        assert_eq!(PrCopy::newest(&wrapped).unwrap().seq, 0);
        file[PR_COPY + 100] ^= 1; // the newer torn by a cut
        assert_eq!(PrCopy::newest(&file).unwrap().seq, 4);
        file[3] ^= 1;
        assert_eq!(PrCopy::newest(&file), None);
        assert_eq!(PrCopy::newest(&file[..PR_COPY]), None, "not whole");
        // the writer refuses what the reader would
        assert!(pr_copy(1, 9, 0, &[0]).is_none());
        assert!(pr_copy(1, 9, 0, &[0, 5, 4]).is_none());
        assert!(pr_copy(1, 9, 0, &[1, 5]).is_none());
        assert!(pr_copy(1, 9, 0, &[0, 0]).is_none(), "a time of 0");
        assert!(pr_copy(1, 9, 0, &[0; POINTS + 1]).is_none());
        let recrc = |c: &mut [u8]| {
            let crc = crate::inflate::crc32(c[..PR_CRC].iter().copied());
            c[PR_CRC..].copy_from_slice(&crc.to_le_bytes());
        };
        let broken = |at: usize, v: &[u8], crc: bool| {
            let mut c = a;
            c[at..at + v.len()].copy_from_slice(v);
            if crc {
                recrc(&mut c);
            }
            PrCopy::parse(&c).err()
        };
        assert_eq!(PrCopy::parse(&a[..1000]).err(), Some(PrError::Size));
        assert_eq!(broken(0, b"RSPQ", true), Some(PrError::Magic));
        assert_eq!(broken(4, &2u16.to_le_bytes(), true), Some(PrError::Version));
        assert_eq!(broken(6, &1u16.to_le_bytes(), true), Some(PrError::Points));
        assert_eq!(broken(6, &401u16.to_le_bytes(), true), Some(PrError::Points));
        assert_eq!(broken(30, &[1], false), Some(PrError::Crc));
        assert_eq!(broken(20, &153u32.to_le_bytes(), true), Some(PrError::Times), "the total not the last time");
        assert_eq!(broken(PR_TIMES + 4, &200u16.to_le_bytes(), true), Some(PrError::Times), "falling");
        assert_eq!(broken(PR_TIMES, &1u16.to_le_bytes(), true), Some(PrError::Times), "a first time");
    }

    /// Every file of the directory named by `SEGMENTS_DIR` parses (the tool's real output; not in the default run).
    #[test]
    #[ignore = "reads the .seg files of $SEGMENTS_DIR"]
    fn built_files_parse() {
        let dir = std::env::var("SEGMENTS_DIR").expect("SEGMENTS_DIR");
        let (mut ok, mut bad) = (0, std::vec::Vec::new());
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let b = std::fs::read(e.path()).unwrap();
            match Segment::new().parse(&b) {
                Ok(()) => ok += 1,
                Err(err) => bad.push(err),
            }
        }
        std::println!("parsed {ok}, failed {}: {bad:?}", bad.len());
        assert!(bad.is_empty());
    }
}
