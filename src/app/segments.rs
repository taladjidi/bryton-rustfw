//! Live segments on the unit (docs/segments.md, "On the unit"; step S3): the index of the segment files, the
//! matcher fed one fix a second during a ride, the segment loaded when it starts, and what the screens (S4) read.
//! Nothing is drawn here; the card is written only between rides, after a ride is saved (the efforts, S5, below).
//!
//! **Memory** (all in the one static [`Segments`], main.rs `make_segments`, AXI SRAM, link.x `.axibss`): the index
//! of at most [`MAX_INDEX`] segments as [`Entry`] (32 B each: 4 KB), the one active [`Segment`] (about 8.6 KB), the
//! efforts waiting for the save ([`EFFORTS`] x 820 B: 3.2 KB), the matcher and some counters. No buffer of the whole
//! file: [`Segment::parse_with`] reads the file 1 KB at a time, so the load needs about 3 KB of stack, in its own
//! function. The screens are app/segment_ui.rs.
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
//!
//! **Efforts** (docs/segments.md, "Efforts"; step S5). While a segment is ridden, the time at each of its points is
//! kept as the rider passes it (interpolated between the fixes around it), in the next free one of [`EFFORTS`] slots;
//! at the finish the effort (id, start date, time, the times) is kept, or counted and dropped when the slots are full
//! or it took over 65535 s, or the ride waits behind the last one's save (not recorded yet) ([`Segments::unkept`]).
//! Lost: nothing kept. Nothing is written during a ride: the
//! recorder is the card's one writer while one is open. The efforts are written only once the ride is **saved**:
//! the clean copy's save done (its file read back whole and named in `\Activities`: `recorder::Outcome::Saved`,
//! app/riding.rs `step_recorder`, not the boot's recovery of an earlier ride) marks them, and the writes wait for
//! `App::ride_settled` (both copies' saves done, no ride open, none waiting to be resumed), one effort a pass, on a
//! build that may write the card. A ride discarded, kept in `\Rustfw\Rides` (its save failed) or given up has its
//! efforts dropped then, unwritten. A reset loses the efforts not written yet (RAM), never the ride.
//!
//! Each effort: `\Rustfw\Segments\<id>.pr` ([`crate::segment::PrCopy`]: two copies, the settings file's scheme,
//! `editor::write_copy_in`) when it beats the PR (the newest whole copy's for the same points, else the PR the
//! segment had when it was ridden: the source's), then a line `id,date,elapsed_s` appended to
//! `\Rustfw\Segments\efforts.csv` (made with a header line if missing; the date is the effort's start, UTC, ISO 8601).
//! Never in `\Rustfw` itself (its directory sector holds `ride.rec`'s entry: recorder/mod.rs), and the folder is
//! never made here: a segment was loaded from it. A write that fails (a card full, an error) is counted
//! ([`Segments::unwritten`]) and the effort dropped, never tried again: the next ride starts as ever. A cut during it
//! leaves the `.pr`'s old copy or its new one whole (the newest whole copy wins), and the csv's lines as they were or
//! with the new one (its size is written last); the ride's files and records are not touched. The index build finds
//! a `.pr` or the csv a cut left unfinished (its size, or a chain longer than it needs) and has the next settings or
//! pages write compare the FATs first (`App::fats_unsure`): a FAT sector may then be in FAT 1 only.
//!
//! **The PR file is the PR target** at the next load ([`Segments::load`]): its times replace the PR column (or fill
//! it, the segment having none), when its id and point count are the segment's; anything else in it is ignored.

use crate::fat::{self, Disk, DiskWrite, Fat};
use crate::segment::{Effort, HEADER, Header, MAX_BYTES, POINTS, PR_FILE, PrCopy, Segment, pr_copy};
use crate::segment_match::{Entry, Event, Matcher, delta_s, lerp};
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
/// Efforts kept at most until they are written (a ride rarely finishes more; the rest are counted).
pub const EFFORTS: usize = 4;
/// The efforts' file, in [`DIR`].
pub const CSV: &str = "efforts.csv";
const CSV_HEADER: &str = "id,date,elapsed_s\n";

/// A finished effort waiting for the ride's save (all zeros: none).
#[derive(Clone, Copy, Debug)]
pub struct Kept {
    /// The segment.
    pub id: u32,
    /// FIT time of its start.
    pub date: u32,
    /// Its time, s (at most 65535).
    pub elapsed_s: u32,
    /// The PR's time the segment had when ridden, s (0: none).
    pr_s: u32,
    /// Points of the segment.
    npoints: u16,
    /// The time at each point, s.
    times: [u16; POINTS],
}

impl Kept {
    const ZERO: Kept = Kept { id: 0, date: 0, elapsed_s: 0, pr_s: 0, npoints: 0, times: [0; POINTS] };
}

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
    /// Efforts finished: `efforts[..kept]`, the first `saved` of them of a saved ride (written next).
    efforts: [Kept; EFFORTS],
    kept: usize,
    saved: usize,
    /// The effort being ridden: its next point, and (m along, s) at the last fix.
    next_pt: usize,
    last: (u32, u32),
    /// Efforts finished but not kept (the slots full, too long).
    pub unkept: u16,
    /// Efforts whose write failed (dropped).
    pub unwritten: u16,
    /// Efforts written (a csv line each).
    pub written: u16,
    /// PR files written.
    pub prs: u16,
    /// Loads that took the rider's PR file as the PR target.
    pub pr_files: u16,
    /// The index build found a file of the efforts a cut left unfinished (app: the FATs compared before a write).
    suspect: bool,
    /// See [`Segments::new_pr`].
    new_pr: bool,
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
            efforts: [Kept::ZERO; EFFORTS],
            kept: 0,
            saved: 0,
            next_pt: 0,
            last: (0, 0),
            unkept: 0,
            unwritten: 0,
            written: 0,
            prs: 0,
            pr_files: 0,
            suspect: false,
            new_pr: false,
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

    /// The header of segment `id`'s file (the banner's name, the list's distance and gain): the volume mounted, the folder looked up, the file's first sector read (3 KB in the sim's pass).
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

    /// Efforts finished and not written yet (of a saved ride, or of the one open).
    pub fn efforts(&self) -> usize {
        self.kept
    }

    /// One fix (degrees x 1e-7, clock `t` s, FIT time `date`); a finish keeps its effort only if `record` (the ride is
    /// being recorded, not waiting behind the last one's save: its efforts would be marked by that save).
    fn fix(&mut self, lat: i32, lon: i32, t: u32, date: u32, record: bool) {
        let loaded = self.seg.npoints > 0;
        let ev = match self.matcher.active_id() {
            Some(id) if loaded && self.seg.header.id == id => {
                let ev = self.matcher.step_active(&self.seg, lat, lon, t);
                if ev.is_none() {
                    self.split(t);
                }
                ev
            }
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
        if let Event::Finish { id, elapsed_s, .. } = ev {
            self.new_pr = false; // until the effort is kept and beats the PR
            if record {
                self.keep_effort(id, elapsed_s, date.saturating_sub(elapsed_s));
            } else {
                self.unkept = self.unkept.saturating_add(1);
            }
        }
        if let Event::Start(id) = ev {
            (self.next_pt, self.last) = (1, (0, 0));
            self.waited = 0;
            // the file loaded last is this one: nothing to read (the index is fixed during a ride, so is the file)
            self.want = (!(loaded && self.seg.header.id == id)).then_some(id);
        }
    }

    /// The times of the points passed since the last fix, into the free slot (none free: not kept, counted at the
    /// finish).
    fn split(&mut self, t: u32) {
        let (Some(along), Some(el)) = (self.matcher.along_m(), self.matcher.elapsed_s(t)) else { return };
        let (a0, e0) = self.last;
        let Some(e) = self.efforts.get_mut(self.kept) else { return };
        if along <= a0 {
            return; // no further than the last fix: nothing passed
        }
        let last = self.seg.npoints.saturating_sub(1);
        while self.next_pt < last {
            let Some(&d) = self.seg.dist_m.get(self.next_pt).filter(|&&d| d <= along) else { break };
            if let Some(c) = e.times.get_mut(self.next_pt) {
                *c = u16::try_from(lerp(d, a0, along, e0, el)).unwrap_or(u16::MAX);
            }
            self.next_pt += 1;
        }
        self.last = (along, el);
    }

    /// The effort on the segment loaded, finished in `elapsed_s`, started at FIT time `date`: kept, its points not
    /// passed at a fix filled between the last fix and the finish, its times made never to fall.
    fn keep_effort(&mut self, id: u32, elapsed_s: u32, date: u32) {
        let np = self.seg.npoints;
        let last = np.saturating_sub(1);
        let pr_s = match self.seg.kept.first() {
            Some(Some(Effort::Pr)) => self.seg.times.get(last).and_then(|row| row.first()).map_or(0, |&v| u32::from(v)),
            _ => 0,
        };
        let (a0, e0) = self.last;
        let ok = self.seg.header.id == id && np >= 2;
        let (Some(&len), Ok(total), true, true) =
            (self.seg.dist_m.get(last), u16::try_from(elapsed_s), self.kept < EFFORTS, ok)
        else {
            self.unkept = self.unkept.saturating_add(1);
            return;
        };
        // the result card's NEW PR, by the rule the PR file is written with: faster than the PR loaded and than the
        // efforts kept on these points before it (written first)
        let before = self.efforts.get(..self.kept).unwrap_or(&[]);
        let best = before.iter().filter(|k| k.id == id && usize::from(k.npoints) == np).map(|k| k.elapsed_s).min();
        let best = [best, (pr_s > 0).then_some(pr_s)].into_iter().flatten().min();
        let new_pr = best.is_none_or(|b| elapsed_s < b);
        let Some(e) = self.efforts.get_mut(self.kept) else { return };
        let (Some(times), Some(dist)) = (e.times.get_mut(..np), self.seg.dist_m.get(..np)) else { return };
        let mut prev = 0u16;
        for (i, (t, &d)) in times.iter_mut().zip(dist).enumerate() {
            if i >= self.next_pt {
                *t = u16::try_from(lerp(d, a0, len, e0, elapsed_s)).unwrap_or(total);
            }
            *t = match i {
                0 => 0,
                _ if i == last => total,
                _ => (*t).clamp(prev, total),
            };
            prev = *t;
        }
        (e.id, e.date, e.elapsed_s, e.pr_s) = (id, date, elapsed_s, pr_s);
        e.npoints = u16::try_from(np).unwrap_or(0);
        self.kept += 1;
        self.new_pr = new_pr;
    }

    /// Whether the last finish was kept and beats the PR (the result card's NEW PR; the PR file follows the same rule).
    pub(crate) fn new_pr(&self) -> bool {
        self.new_pr
    }

    /// The ride was saved: its efforts are written once the card is free.
    pub(crate) fn ride_saved(&mut self) {
        self.saved = self.kept;
    }

    /// The oldest effort written ([`Segments::write_effort`]) or given up: the others move up.
    fn pop_effort(&mut self) {
        self.efforts.rotate_left(1);
        if let Some(last) = self.efforts.last_mut() {
            *last = Kept::ZERO;
        }
        self.kept = self.kept.saturating_sub(1);
        self.saved = self.saved.saturating_sub(1);
    }

    /// The oldest effort to the card: its `.pr` if it beats the PR ([`Segments::prs`] counted once it is written),
    /// then its csv line.
    #[inline(never)] // its 2 KB buffer in a frame of its own
    fn write_effort(&mut self, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<(), fat::Error> {
        let e = self.efforts.first().copied().ok_or(fat::Error::NotFound)?;
        let mut fat = Fat::mount(d)?;
        let dir = fat.open(d, DIR)?; // never made here: the segment was loaded from it
        let mut name = Line::<16>::new();
        name.push_u32(e.id);
        name.push_str(".pr");
        let np = usize::from(e.npoints);
        let mut file = [0u8; PR_FILE];
        // the PR now: the newest whole copy, its time only if for these points; its number whatever it holds, so the
        // new copy is the newest (a failed read writes nothing: it may hold the newer)
        let (seq, old) = match fat.open_in(d, dir, name.as_str()) {
            Ok(f) if !f.dir && f.size as usize == PR_FILE => {
                if fat.read_at(d, &f, 0, &mut file)? != PR_FILE {
                    return Err(fat::Error::Io);
                }
                let newest = PrCopy::newest(&file);
                let fits = newest.filter(|p| p.id == e.id && p.npoints() == np).map(|p| p.total_s);
                (newest.map_or(1, |p| p.seq.wrapping_add(1)), fits)
            }
            Ok(_) | Err(fat::Error::NotFound) => (1, None),
            Err(err) => return Err(err),
        };
        let best = [old, (e.pr_s > 0).then_some(e.pr_s)].into_iter().flatten().min();
        if best.is_none_or(|b| e.elapsed_s < b) {
            let copy = pr_copy(seq, e.id, e.date, e.times.get(..np).unwrap_or(&[])).ok_or(fat::Error::Corrupt)?;
            crate::app::editor::write_copy_in(&mut fat, d, dir, name.as_str(), seq % 2, &copy, t)?;
            self.prs = self.prs.saturating_add(1);
            if self.seg.header.id == e.id {
                self.seg.npoints = 0; // loaded again at its next start, with this PR
            }
        }
        let mut line = Line::<48>::new();
        line.push_u32(e.id);
        line.push_str(",");
        push_iso(&mut line, e.date);
        line.push_str(",");
        line.push_u32(e.elapsed_s);
        line.push_str("\n");
        append_line(&mut fat, d, dir, line.as_str().as_bytes(), t)
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
            let ends = |x: &[u8]| n.get(n.len().saturating_sub(x.len())..).is_some_and(|y| y.eq_ignore_ascii_case(x));
            if !e.file.dir && (ends(b".pr") || n.eq_ignore_ascii_case(CSV.as_bytes())) && !self.suspect {
                // a chain that cannot be read counts as unfinished, and the build goes on
                self.suspect = !whole(&mut fat, d, &e.file, ends(b".pr")).unwrap_or(false);
            }
            if e.file.dir || !ends(b".seg") {
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
        if self.seg.header.id != id {
            return Err(());
        }
        // the rider's own PR, if its file is whole and for these points (else the source's PR as it is)
        name.clear();
        name.push_u32(id);
        name.push_str(".pr");
        let mut file = [0u8; PR_FILE];
        let np = self.seg.npoints;
        let whole = match fat.open_in(d, dir, name.as_str()) {
            Ok(f) if !f.dir && f.size as usize == PR_FILE => fat.read_at(d, &f, 0, &mut file).ok() == Some(PR_FILE),
            _ => false,
        };
        if let Some(p) = whole.then(|| PrCopy::newest(&file)).flatten().filter(|p| p.id == id && p.npoints() == np) {
            for (row, v) in self.seg.times.iter_mut().zip(p.iter()) {
                if let Some(c) = row.first_mut() {
                    *c = v;
                }
            }
            if let Some(k) = self.seg.kept.first_mut() {
                *k = Some(Effort::Pr);
            }
            self.pr_files = self.pr_files.saturating_add(1);
        }
        Ok(())
    }
}

/// Whether a file of the efforts (a `.pr` if `pr`, else efforts.csv) is as a finished write leaves it: a `.pr` of
/// [`PR_FILE`] bytes, a csv not empty, and a chain no longer than its size needs. Else a write was cut (the file made,
/// or a cluster taken, the size not written yet), maybe between the two FATs' writes of a cluster.
fn whole(fat: &mut Fat, d: &mut impl Disk, f: &fat::File, pr: bool) -> Result<bool, fat::Error> {
    let size_ok = if pr { f.size as usize == PR_FILE } else { f.size > 0 };
    if !size_ok || f.cluster < 2 {
        return Ok(false);
    }
    let need = f.size.div_ceil(fat.cluster_bytes()).max(1);
    Ok(fat.chain(d, f.cluster)?.clusters == need)
}

/// FIT time `fit` as ISO 8601 UTC (`2026-10-30T09:12:05Z`).
fn push_iso(l: &mut Line<48>, fit: u32) {
    let (y, mo, d, h, mi, s) = from_epoch_s(i64::from(fit) + 631_065_600);
    l.push_u32(u32::from(y));
    for (sep, v) in [("-", mo), ("-", d), ("T", h), (":", mi), (":", s)] {
        l.push_str(sep);
        if v < 10 {
            l.push_str("0");
        }
        l.push_u32(u32::from(v));
    }
    l.push_str("Z");
}

/// `line` appended to [`CSV`] in `dir` (made, with its header line, if missing): the last sector written again with
/// it, then the size, so a cut before the size leaves the file as it was. Header and line fit in two sectors.
pub(crate) fn append_line(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    dir: fat::File,
    line: &[u8],
    t: (u16, u16),
) -> Result<(), fat::Error> {
    let mut h = match fat.locate(d, dir, CSV) {
        Err(fat::Error::NotFound) => fat.create(d, dir, CSV, t)?,
        other => other?,
    };
    let (sector, at) = (h.size / 512, h.size as usize % 512);
    let mut buf = [0u8; 1024];
    if at > 0 {
        let first = buf.first_chunk_mut::<512>().ok_or(fat::Error::Corrupt)?;
        fat.read_sector(d, &h, sector, first)?;
    }
    let head = if h.size == 0 { CSV_HEADER.as_bytes() } else { &[] };
    let end = at + head.len() + line.len();
    let (a, b) = buf.get_mut(at..end).ok_or(fat::Error::Corrupt)?.split_at_mut(head.len());
    a.copy_from_slice(head);
    b.copy_from_slice(line);
    fat.write_sectors(d, &mut h, sector, buf.get(..end.div_ceil(512) * 512).ok_or(fat::Error::Corrupt)?)?;
    let grown = u32::try_from(end - at).map_err(|_| fat::Error::Corrupt)?;
    let size = h.size.checked_add(grown).ok_or(fat::Error::Corrupt)?;
    fat.set_size(d, &mut h, size, t)
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
        let (date, record) = (self.rec_time().1, !self.rec.clean.pending());
        let state = self.ride.state();
        let Some(s) = self.segments.as_deref_mut() else { return };
        s.target = target;
        if state == ride::State::Idle || !live {
            s.stop(); // stopped or saved: a new ride starts with none; LIVE SEGMENTS off drops the one ridden
        } else if let (ride::State::Running, Some(p)) = (state, fix) {
            s.fix(p.lat, p.lon, clock, date, record);
        }
    }

    /// The pass loop's part: an effort of a saved ride written, else the file of a segment that started, else a
    /// slice of the index build. The card is ours.
    pub(crate) fn step_segments(&mut self, b: &mut impl Board) {
        let idle = self.ride.state() == ride::State::Idle;
        let due = self.up_ms >= START_AT_MS;
        let settled = self.ride_settled();
        let (writable, t) = (self.card_writable, fat::write::dos_time(self.rec_time().0));
        let Some(s) = self.segments.as_deref_mut() else { return };
        let mut d = emmc::Volume(&mut self.emmc, b);
        if settled {
            s.kept = s.saved; // a ride ended without a save (discarded, kept in Rides, given up): its efforts go
        }
        let mut failed = false;
        if settled && s.saved > 0 {
            // one effort a pass, after the ride's save; a failure drops it (counted, never tried again)
            match writable.then(|| s.write_effort(&mut d, t)) {
                Some(Ok(())) => s.written = s.written.saturating_add(1),
                Some(Err(_)) => (s.unwritten, failed) = (s.unwritten.saturating_add(1), true),
                None => {}
            }
            s.pop_effort();
        } else if let Some(id) = s.want.take() {
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
        // a write failed now, or one was cut before this start: a FAT sector may have reached the first copy only
        if failed || self.segments.as_deref_mut().is_some_and(|s| core::mem::take(&mut s.suspect)) {
            self.fats_unsure();
        }
    }

    /// The ride was saved (the clean copy's save done): the efforts on it are written once the card is free.
    pub(crate) fn segments_ride_saved(&mut self) {
        if let Some(s) = self.segments.as_deref_mut() {
            s.ride_saved();
        }
    }
}
