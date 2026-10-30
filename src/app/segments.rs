//! Live segments on the unit (docs/segments.md, "On the unit"; step S3): the index of the segment files, the
//! matcher fed one fix a second during a ride, the segment loaded when it starts, and what the screens (S4) read.
//! Nothing is drawn here and nothing is written to the card, ever (S5 writes the efforts).
//!
//! **Memory** (all in the one static [`Segments`], main.rs `make_segments`, AXI SRAM, link.x `.axibss`): the index
//! of at most [`MAX_INDEX`] segments as [`Entry`] (32 B each: 4 KB), the one active [`Segment`] (about 8.6 KB), the
//! matcher and some counters. No buffer of the whole file: [`Segment::parse_with`] reads the file 1 KB at a time, so
//! the load needs about 3 KB of stack, in its own function. The screens are app/segment_ui.rs.
//!
//! **Index.** Built from the headers only (the first 0x200 bytes of each `.seg` in `\Rustfw\Segments\`), [`SLICE`]
//! files a pass, not before [`START_AT_MS`] after the start and only while no ride is open (so it is fixed for a
//! ride). A file that is not a segment (name, size, header) is skipped and counted ([`Segments::skipped`]). A
//! computer that wrote to the drive restarts the unit (drive.rs `take_card_back`), which builds the index again.
//! The header's enabled bytes are not read yet (their meaning is inferred, segment.rs): S4 decides.
//!
//! **Detection.** [`App::segment_fix`] (once a second, in `App::second`) feeds the matcher: idle over the index, and
//! on [`Event::Start`] the pass loop loads the file ([`App::step_segments`]); until it is there the fixes wait.
//! A segment that cannot be loaded, or is not there within [`LOAD_WAIT_S`] s, is dropped and counted
//! ([`Segments::dropped`]). A failing segment never stops the recording: nothing here panics or waits.

use crate::fat::{self, Disk, Fat};
use crate::segment::{Effort, HEADER, Header, MAX_BYTES, Segment};
use crate::segment_match::{Entry, Event, Matcher, delta_s};
use crate::*;

/// Segments the index holds at most.
pub const MAX_INDEX: usize = 128;
/// Files whose header one pass reads at most (a header is one sector).
const SLICE: usize = 8;
/// The index is not built in the first seconds of the unit's run (never at boot).
const START_AT_MS: u32 = 5_000;
/// Seconds a started segment waits for its file before it is dropped.
const LOAD_WAIT_S: u32 = 10;
/// The folder of the segment files.
pub const DIR: &str = "Rustfw/Segments";
/// Events kept for the screens.
pub const NOTES: usize = 4;

/// Where the index build is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Build {
    /// Not started (all zeros: the static starts here).
    Wait,
    /// Reading headers, a slice a pass.
    Scan,
    /// Built (or given up: a card error leaves what was found).
    Done,
}

/// The segments: index, active segment, matcher, counters and the last events.
pub struct Segments {
    index: [Entry; MAX_INDEX],
    len: usize,
    seg: Segment,
    matcher: Matcher,
    build: Build,
    /// Directory entries passed so far by the build.
    passed: usize,
    /// Files skipped by the index build (not a segment, too many, a duplicate), since it started.
    pub skipped: u16,
    /// Segments dropped after they started (their file could not be loaded, or never came).
    pub dropped: u16,
    /// Files read for a started segment (a failing one is read once: it leaves the index).
    pub loads: u16,
    /// The segment waiting for its file, and the seconds it has waited.
    want: Option<u32>,
    waited: u32,
    /// TARGET (settings.seg_target, set each second): 0 AUTO, 1 PR, 2 KOM, 3 QOM, 4 RECENT.
    pub(crate) target: u8,
    /// The last events with the clock (s) they came at; `n` of them in all (a ring).
    notes: [(u32, Event); NOTES],
    n: usize,
}

impl Default for Segments {
    fn default() -> Self {
        Self::new()
    }
}

/// What the live segment page shows (S4), at the clock of the last fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentNow<'a> {
    /// The segment's id.
    pub id: u32,
    /// Its name (raw bytes, ASCII).
    pub name: &'a [u8],
    /// Seconds since it started.
    pub elapsed_s: u32,
    /// Elapsed minus the target's time at this distance, s (negative: ahead); None without a target.
    pub delta_s: Option<i32>,
    /// The target, as the file names it (AUTO: the PR if there is one, else the KOM).
    pub target: Option<Effort>,
    /// Distance along the segment, m.
    pub along_m: u32,
    /// Distance left to the end, m.
    pub left_m: u32,
    /// Where the target is now, m along the segment (the ghost); None without a target.
    pub ghost_m: Option<u32>,
}

impl Segments {
    /// Nothing indexed.
    pub const fn new() -> Self {
        Segments {
            index: [Entry::ZERO; MAX_INDEX],
            len: 0,
            seg: Segment::new(),
            matcher: Matcher::new(),
            build: Build::Wait,
            passed: 0,
            skipped: 0,
            dropped: 0,
            loads: 0,
            want: None,
            waited: 0,
            target: 0,
            // not an Option, and the filler is the variant that holds the others' niche: the static must start as zeros
            notes: [(0, Event::Finish { id: 0, elapsed_s: 0, deltas: [None; crate::segment::KEPT] }); NOTES],
            n: 0,
        }
    }

    /// Segments in the index.
    pub fn indexed(&self) -> usize {
        self.len
    }

    /// Whether the index is built.
    pub fn ready(&self) -> bool {
        self.build == Build::Done
    }

    /// The last events (at most [`NOTES`]), oldest first, each with the clock (s) it came at.
    pub fn events(&self) -> impl Iterator<Item = (u32, Event)> + '_ {
        let (first, k) = (self.n.saturating_sub(NOTES), self.n.min(NOTES));
        (first..first + k).filter_map(|i| self.notes.get(i % NOTES).copied())
    }

    fn note(&mut self, t: u32, ev: Event) {
        if let Some(slot) = self.notes.get_mut(self.n % NOTES) {
            *slot = (t, ev);
        }
        self.n = self.n.wrapping_add(1);
    }

    /// The column of [`Segment::kept`] the target names; AUTO, and a target the file lacks (shortcut: no message
    /// about it, the label on the page says which one is used), take the PR, else the KOM.
    pub(crate) fn column(&self) -> Option<usize> {
        let at = |e| self.seg.kept.iter().position(|&k| k == Some(e));
        let chosen = match self.target {
            1 => at(Effort::Pr),
            2 => at(Effort::Kom),
            3 => at(Effort::Qom),
            4 => at(Effort::Recent),
            _ => None,
        };
        chosen.or_else(|| at(Effort::Pr)).or_else(|| at(Effort::Kom))
    }

    /// Events seen in all (the ring keeps the last [`NOTES`]).
    pub(crate) fn count(&self) -> usize {
        self.n
    }

    /// The segment loaded last (the finished one, for the result card).
    pub(crate) fn seg(&self) -> &Segment {
        &self.seg
    }

    /// The id of indexed segment `k`.
    pub fn id_at(&self, k: usize) -> Option<u32> {
        self.index.get(..self.len)?.get(k).map(|e| e.id)
    }

    /// The header of segment `id`'s file (the banner's name, the list's distance and gain): one sector.
    #[inline(never)]
    pub(crate) fn header(d: &mut impl Disk, id: u32) -> Option<Header> {
        let mut fat = Fat::mount(d).ok()?;
        let dir = fat.open(d, DIR).ok()?;
        let mut name = Line::<16>::new();
        name.push_u32(id);
        name.push_str(".seg");
        let f = fat.open_in(d, dir, name.as_str()).ok()?;
        let mut head = [0u8; HEADER];
        (!f.dir && fat.read_at(d, &f, 0, &mut head).ok()? == HEADER).then_some(())?;
        Header::parse(&head).ok()
    }

    /// One fix (degrees x 1e-7, clock `t` s).
    fn fix(&mut self, lat: i32, lon: i32, t: u32) {
        let loaded = self.seg.npoints > 0;
        let ev = match self.matcher.active_id() {
            Some(id) if loaded && self.seg.header.id == id => self.matcher.step_active(&self.seg, lat, lon, t),
            Some(_) => {
                // started, the file not there yet (the loop loads it): a card that does not answer drops it
                self.waited = self.waited.saturating_add(1);
                if self.waited > LOAD_WAIT_S {
                    self.drop_active();
                }
                None
            }
            None if self.build == Build::Done => {
                self.matcher.step_idle(self.index.get(..self.len).unwrap_or(&[]), lat, lon, t)
            }
            None => None,
        };
        let Some(ev) = ev else { return };
        self.note(t, ev);
        if let Event::Start(id) = ev {
            self.waited = 0;
            // the file loaded last is this one: nothing to read (the index is fixed during a ride, so is the file)
            self.want = (!(loaded && self.seg.header.id == id)).then_some(id);
        }
    }

    /// The ride is over: no segment is active, none waits for its file (not a failure: not counted).
    fn stop(&mut self) {
        self.matcher.reset();
        self.want = None;
    }

    /// Takes segment `id` out of the index for the rest of the run: its file failed to load, so it is not tried again.
    #[cfg(test)]
    pub(crate) fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.index.iter().take(self.len).map(|e| e.id)
    }

    fn forget(&mut self, id: u32) {
        if let Some(i) = self.index.get(..self.len).and_then(|ix| ix.iter().position(|e| e.id == id)) {
            self.len -= 1;
            self.index.swap(i, self.len);
            if let Some(last) = self.index.get_mut(self.len) {
                *last = Entry::ZERO;
            }
        }
    }

    fn drop_active(&mut self) {
        self.matcher.reset();
        self.want = None;
        self.dropped = self.dropped.saturating_add(1);
    }

    /// The segment ridden, for the page.
    fn now(&self, clock_s: u32) -> Option<SegmentNow<'_>> {
        let id = self.matcher.active_id().filter(|&id| self.seg.header.id == id && self.seg.npoints > 0)?;
        let (along_m, elapsed_s) = (self.matcher.along_m()?, self.matcher.elapsed_s(clock_s)?);
        let len = self.seg.npoints.checked_sub(1).and_then(|l| self.seg.dist_m.get(l)).copied()?;
        let col = self.column();
        Some(SegmentNow {
            id,
            name: self.seg.header.name(),
            elapsed_s,
            delta_s: col.and_then(|c| delta_s(&self.seg, c, along_m, elapsed_s)),
            target: col.and_then(|c| self.seg.kept.get(c).copied().flatten()),
            along_m,
            left_m: len.saturating_sub(along_m),
            ghost_m: col.and_then(|c| self.ghost(c, elapsed_s)),
        })
    }

    /// Where column `col`'s effort was `elapsed_s` into the segment, m: between the two points whose times hold it.
    fn ghost(&self, col: usize, elapsed_s: u32) -> Option<u32> {
        let at = |i: usize| Some((u32::from(*self.seg.times.get(i)?.get(col)?), *self.seg.dist_m.get(i)?));
        let last = self.seg.npoints.checked_sub(1)?;
        for i in 0..last {
            let ((t0, d0), (t1, d1)) = (at(i)?, at(i + 1)?);
            if elapsed_s < t1 {
                return Some(crate::segment_match::lerp(elapsed_s, t0, t1, d0, d1));
            }
        }
        Some(at(last)?.1)
    }

    /// Reads [`SLICE`] more files' headers into the index; true when the folder is done.
    fn scan_slice(&mut self, d: &mut impl Disk) -> Result<bool, fat::Error> {
        let mut fat = Fat::mount(d)?;
        let dir = fat.open(d, DIR)?;
        let mut c = fat.cursor(dir)?;
        for _ in 0..self.passed {
            if fat.list(d, &mut c)?.is_none() {
                return Ok(true);
            }
        }
        let mut read = 0;
        while read < SLICE {
            let Some(e) = fat.list(d, &mut c)? else { return Ok(true) };
            self.passed += 1;
            let n = e.name();
            if e.file.dir || !n.get(n.len().saturating_sub(4)..).is_some_and(|x| x.eq_ignore_ascii_case(b".seg")) {
                continue;
            }
            read += 1;
            let mut head = [0u8; HEADER];
            let size = usize::try_from(e.file.size).unwrap_or(usize::MAX);
            let header = (HEADER..=MAX_BYTES)
                .contains(&size)
                .then(|| fat.read_at(d, &e.file, 0, &mut head).ok().filter(|&k| k == HEADER))
                .flatten()
                .and_then(|_| Header::parse(&head).ok());
            // the file's name is its id, as the tool writes it: a load finds it by the id
            let mut want = Line::<16>::new();
            let entry = header.and_then(|h| {
                want.push_u32(h.id);
                want.push_str(".seg");
                want.as_str().as_bytes().eq_ignore_ascii_case(n).then(|| Entry::of(&h)).flatten()
            });
            let new = entry.filter(|en| !self.index.get(..self.len).unwrap_or(&[]).iter().any(|o| o.id == en.id));
            match (new, self.index.get_mut(self.len)) {
                (Some(en), Some(slot)) => {
                    *slot = en;
                    self.len += 1;
                }
                _ => self.skipped = self.skipped.saturating_add(1),
            }
        }
        Ok(false)
    }

    /// The file of segment `id` read into the active segment.
    #[inline(never)] // its chunk buffer and the FIT reader in a frame of their own
    pub(crate) fn load(&mut self, d: &mut impl Disk, id: u32) -> Result<(), ()> {
        let mut fat = Fat::mount(d).map_err(drop)?;
        let dir = fat.open(d, DIR).map_err(drop)?;
        let mut name = Line::<16>::new();
        name.push_u32(id);
        name.push_str(".seg");
        let f = fat.open_in(d, dir, name.as_str()).map_err(drop)?;
        let size = usize::try_from(f.size).map_err(drop)?;
        if f.dir {
            return Err(());
        }
        let mut seek = fat::Seek::default();
        let read = |at: usize, buf: &mut [u8]| fat.read_from(d, &f, u32::try_from(at).ok()?, buf, &mut seek).ok();
        self.seg.parse_with(size, read).map_err(drop)?;
        (self.seg.header.id == id).then_some(()).ok_or(())
    }
}

impl App {
    /// Memory for the segments (main.rs keeps it in AXI SRAM, about 13 KB); without it there are none.
    pub fn set_segment_memory(&mut self, mem: &'static mut Segments) {
        self.segments = Some(mem);
    }

    /// The segments (their counters and events), if there is memory for them.
    pub fn segments(&self) -> Option<&Segments> {
        self.segments.as_deref()
    }

    /// The segment being ridden (S4's page).
    pub fn segment_now(&self) -> Option<SegmentNow<'_>> {
        self.segments.as_deref()?.now(self.up_ms / 1000)
    }

    /// One fix of a running ride (once a second, `App::second`): the matcher takes it.
    pub(crate) fn segment_fix(&mut self) {
        let (live, target) = (self.settings.live_segments, self.settings.seg_target as u8);
        let fix = self.gps.pvt.filter(|p| p.good());
        let clock = self.up_ms / 1000;
        let state = self.ride.state();
        let Some(s) = self.segments.as_deref_mut() else { return };
        s.target = target;
        if state == ride::State::Idle || !live {
            s.stop(); // stopped or saved: a new ride starts with none; LIVE SEGMENTS off drops the one ridden
        } else if let (ride::State::Running, Some(p)) = (state, fix) {
            s.fix(p.lat, p.lon, clock);
        }
    }

    /// The pass loop's part: the file of a segment that started, else a slice of the index build. The card is ours.
    pub(crate) fn step_segments(&mut self, b: &mut impl Board) {
        let idle = self.ride.state() == ride::State::Idle;
        let due = self.up_ms >= START_AT_MS;
        let Some(s) = self.segments.as_deref_mut() else { return };
        let mut d = emmc::Volume(&mut self.emmc, b);
        if let Some(id) = s.want.take() {
            s.loads = s.loads.saturating_add(1);
            if s.load(&mut d, id).is_err() {
                s.forget(id);
                s.drop_active();
            }
        } else if s.build != Build::Done && idle && due {
            if s.build == Build::Wait {
                (s.len, s.passed, s.skipped, s.build) = (0, 0, 0, Build::Scan);
            }
            // a card error ends the build with what it found
            if s.scan_slice(&mut d).unwrap_or(true) {
                s.build = Build::Done;
            }
        }
    }
}
