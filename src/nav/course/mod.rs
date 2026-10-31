//! A course to follow: its points (from a file, storage/course_file.rs), where the rider is on it, and its climbs. Pure
//! logic, no heap; the big arrays live in one static block that main.rs hands over ([`crate::storage::course_file`]'s
//! `Courses`, in AXI SRAM: about 70 KB, too big for the app).
//!
//! - **Points:** at most [`POINTS`], one every `step` metres along the way; when full, every other point goes and the
//!   step doubles (as map.rs's track), so a course of any length fits, evenly spaced: 4096 points are one every 50 m
//!   over 200 km. Distance thinning, not Douglas-Peucker (Douglas and Peucker 1973; Visvalingam and Whyatt 1993):
//!   both need the whole line in memory, and a course is streamed from its file. Each point keeps its distance from
//!   the start (summed over every point of the file, not only the kept ones), its altitude, and the ascent so far.
//! - **Gaps and length ([`Odometer`]):** a jump of over [`JUMP_M`] where a GPX track segment starts is not ridden,
//!   counted, drawn or followed; a FIT course's own distance is its length when it agrees with the points'.
//! - **Plane:** metres east and north of the first point, as the map's ([`crate::map::project`]); a point farther
//!   than [`FAR_M`] is left out (it keeps the search's arithmetic in i64).
//! - **Where the rider is ([`Course::follow`], once a second):** the nearest segment in a window around the last one
//!   ([`BEHIND`] back, [`AHEAD`] ahead): a few dozen segments a second, never the whole course. Lost (farther than
//!   [`LOST_M`], or not placed yet), a wider search every [`SEARCH_S`] seconds, bounded: of the chunks of 64 points
//!   whose boxes are about as near as the nearest, the [`SEARCH_CHUNKS`] nearest along the course to where the rider
//!   was (on a lap course, the lap being ridden). Where two passes are about as near (an out-and-back, a loop, a
//!   crossing), the first one not behind the rider wins: the way the cursor goes picks the leg.
//! - **Off course:** farther than [`OFF_M`] for [`OFF_S`] seconds, once the course was joined; back within
//!   [`BACK_M`]. On a course thinned past a point every 50 m, the threshold is half the step plus [`BACK_M`]: a chord
//!   cuts a bend by up to half the step. Each said once ([`Event`]).
//! - **Ascent:** altitude rises counted past a 3 m hysteresis (small wiggles are noise).
//! - **Climbs (`climbs`):** Garmin's ClimbPro definition (www8.garmin.com/manuals/webhelp, "Using ClimbPro", read
//!   2026-10-24): at least 500 m long at an average grade of at least 3 %, which is a climb score (length in metres
//!   x grade in %) of at least 1500. Found on the profile in 50 m steps: a climb starts at a step of 2 % or more and
//!   goes on while it reaches new tops, across a dip of at most 20 m below its top and 500 m without a new one (the
//!   merge across short dips), and ends at its top. These thresholds are this firmware's choice (inferred: Garmin
//!   does not publish its own; stock's Climb Challenge, seen in its code, also starts a climb at 2 %).
//!
//! - **Cues ([`cues`]):** turn-by-turn cues, from the file or found on the course's shape, and the next one said.
//!
//! Nothing here panics or loops on any input: every index is checked, every loop is bounded by [`POINTS`],
//! [`CLIMBS`] or a window (the file readers are fuzzed in src/sim/fuzz.rs).

use crate::map::{Route, cos_q16, project};

pub mod cues;
pub use cues::*;

/// Points kept at most.
pub const POINTS: usize = 4096;
const CHUNK: usize = Route::CHUNK;
/// Chunks of points (a box each) at most.
pub const CHUNKS: usize = POINTS / CHUNK;
/// Climbs kept at most.
pub const CLIMBS: usize = 32;
/// Spacing of the points until the course first fills, metres.
const FIRST_STEP_M: u32 = 10;
/// Points farther than this from the first (metres east or north) are left out.
pub const FAR_M: i32 = 2_000_000;
/// No altitude.
const NO_ALT: u16 = u16::MAX;
/// Ascent hysteresis, decimetres.
const ASC_DM: i32 = 30;
/// Climbs: the profile in steps of this many metres; a step climbs at this grade (% x 10) or more; a climb ends
/// this far (m) after its top without a new one, or this far (dm) below it.
const BIN_M: u32 = 50;
const UP_X10: i32 = 20;
const GAP_M: u32 = 500;
const DIP_DM: i32 = 200;
/// Climbs: profile steps at most (8 a kept point).
const BINS: u32 = POINTS as u32 * 8;
/// A climb is at least this long, m, at this average grade, % x 10 (Garmin's ClimbPro).
pub const MIN_LEN_M: u32 = 500;
/// See [`MIN_LEN_M`].
pub const MIN_GRADE_X10: u32 = 30;
/// The grade ahead is over this far, m.
pub const AHEAD_M: u32 = 200;
/// The window of segments searched each second: this many back and ahead of the last one.
pub const BEHIND: usize = 8;
/// See [`BEHIND`].
pub const AHEAD: usize = 40;
/// Off course past this far, m, for [`OFF_S`] seconds; back within [`BACK_M`].
pub const OFF_M: u32 = 50;
/// See [`OFF_M`].
pub const OFF_S: u32 = 5;
/// See [`OFF_M`].
pub const BACK_M: u32 = 25;
/// Farther than this, m, the window is lost: a wider search every [`SEARCH_S`] seconds over [`SEARCH_CHUNKS`]
/// chunks.
pub const LOST_M: u32 = 100;
/// See [`LOST_M`].
pub const SEARCH_S: u32 = 5;
/// See [`LOST_M`].
pub const SEARCH_CHUNKS: usize = 4;
/// Two passes within this of each other, m, are as near: the first one not behind wins.
const TIE_M: u32 = 20;

/// Decimetres to FIT's altitude ((m + 500) x 5), as kept.
fn enc(dm: i32) -> u16 {
    (dm.saturating_add(5000) / 2).clamp(0, 0xfffe) as u16
}

/// Kept altitude to decimetres.
fn dm(v: u16) -> i32 {
    i32::from(v) * 2 - 5000
}

/// The length of a vector, m, each part clamped to 2^31 so the squares cannot overflow.
fn hypot(dx: i64, dy: i64) -> u32 {
    let sq = |v: i64| v.unsigned_abs().min(1 << 31).pow(2);
    (sq(dx) + sq(dy)).isqrt().min(u64::from(u32::MAX)) as u32
}

/// The length of the step from `a` to `b` (degrees x 1e-7), mm, on the local plane at `a` (as the map's).
///
/// ```
/// use bryton_rustfw::course::step_mm;
/// assert_eq!(step_mm((0, 0), (8983, 0)), 99_998); // 8983e-7 degrees of latitude: 100 m
/// ```
pub fn step_mm(a: (i32, i32), b: (i32, i32)) -> u64 {
    let cos = cos_q16(a.0);
    let clamp = |v: i64| v.clamp(-(1 << 31), 1 << 31);
    let dy = clamp((i64::from(b.0) - i64::from(a.0)) * 11_132 / 1000);
    let dx = clamp((i64::from(b.1) - i64::from(a.1)) * 11_132 * cos / 65_536 / 1000);
    (dx * dx).unsigned_abs().saturating_add((dy * dy).unsigned_abs()).isqrt()
}

/// How far `p` is from box `b` (min x, min y, max x, max y), m; 0 inside.
fn box_m(b: &(i32, i32, i32, i32), p: (i32, i32)) -> u64 {
    let dx = (i64::from(b.0) - i64::from(p.0)).max(i64::from(p.0) - i64::from(b.2)).max(0);
    let dy = (i64::from(b.1) - i64::from(p.1)).max(i64::from(p.1) - i64::from(b.3)).max(0);
    (dx * dx + dy * dy).unsigned_abs().isqrt()
}

/// Bit `i` of a gap set: no segment into point `i`.
pub fn is_gap(gaps: &[u32], i: usize) -> bool {
    gaps.get(i / 32).is_some_and(|w| w & (1 << (i % 32)) != 0)
}

/// A point of a course file as its reader gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pt {
    /// Latitude, degrees x 1e-7.
    pub lat: i32,
    /// Longitude, degrees x 1e-7.
    pub lon: i32,
    /// Altitude, dm.
    pub alt_dm: Option<i32>,
    /// A new track or route segment starts here (GPX `trkseg`, `trk`, `rte`).
    pub split: bool,
    /// The file's own distance from its start at this point, m (a FIT `record`'s distance).
    pub dist_m: Option<u32>,
}

/// The length of a course as its points come: the sum of the steps between them, less the gaps; and the file's
/// own length when it states one. Used by the model and by the list's measure, so both say the same.
///
/// - **Gaps:** a GPX file of several tracks or segments (`trk`, `trkseg`) may join rides done apart; a jump of more
///   than [`JUMP_M`] where a segment starts is not ridden, not counted and not drawn. Inside one segment points
///   are joined whatever their spacing: a planner's straight road can be one point at each end, and a GPS that
///   lost its fix in a tunnel still rode the straight line (better counted than lost).
/// - **Stated length:** a FIT course's records carry their distance along the course (Garmin's course cookbook
///   writes it, github.com/garmin/fit-csharp-sdk `Cookbook/CourseEncode`), what the planning app showed. It is
///   taken when it is within [`AGREE_PER_MILLE`] of the summed points: the sum here is on a flat local plane and
///   misses the corners the thinning cuts, so the app's figure is the better one; farther apart, the file's figure
///   is not about these points (a truncated read past [`crate::storage::course_file::MAX_BYTES`], a distance from
///   another start, a broken field), and the sum, which is what the rider is placed on, stays. Bryton's routes
///   state a length too (message 254, storage/course_file.rs) within 0.25 % of the sum: not read, it would change
///   nothing. GPX 1.1 states none (its schema has no length; extensions differ per app).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Odometer {
    last: Option<(i32, i32)>,
    mm: u64,
    n: u32,
    /// The file's distance at its first and last point, m.
    stated: Option<(u32, u32)>,
}

/// A jump longer than this where a track segment starts is a gap, m.
pub const JUMP_M: u64 = 1000;
/// The file's own length is taken within this of the summed points, per mille.
pub const AGREE_PER_MILLE: u64 = 20;

impl Odometer {
    /// Nothing yet.
    pub const fn new() -> Self {
        Odometer { last: None, mm: 0, n: 0, stated: None }
    }

    /// The next point: true if a gap comes before it (then not counted).
    ///
    /// ```
    /// use bryton_rustfw::course::{Odometer, Pt};
    /// let mut o = Odometer::new();
    /// let north = |m: i32, split| Pt { lat: m * 8983 / 100, split, ..Pt::default() }; // 8983e-7 degrees: 100 m
    /// assert!(!o.step(north(0, false)) && !o.step(north(500, false)));
    /// assert!(o.step(north(5000, true)), "4.5 km where a segment starts: a gap");
    /// assert!(!o.step(north(5100, false)) && !o.step(north(9000, false)), "3.9 km inside a segment: ridden");
    /// assert!(!o.step(north(9500, true)), "500 m where a segment starts: ridden");
    /// assert!(o.len_m().unwrap().abs_diff(500 + 100 + 3900 + 500) <= 1);
    /// ```
    pub fn step(&mut self, pt: Pt) -> bool {
        let p = (pt.lat, pt.lon);
        let mut gap = false;
        if let Some(q) = self.last {
            let s = step_mm(q, p);
            gap = pt.split && s > JUMP_M * 1000;
            if !gap {
                self.mm = self.mm.saturating_add(s);
            }
        }
        if let Some(d) = pt.dist_m {
            self.stated = Some((self.stated.map_or(d, |s| s.0), d));
        }
        (self.last, self.n) = (Some(p), self.n.saturating_add(1));
        gap
    }

    /// The points' summed length, m.
    pub fn run_m(&self) -> u32 {
        (self.mm / 1000).min(u64::from(u32::MAX)) as u32
    }

    /// The course's length, m: the file's own when it agrees with the points' within [`AGREE_PER_MILLE`], else
    /// theirs; None under two points.
    pub fn len_m(&self) -> Option<u32> {
        let sum = self.run_m();
        let stated = self
            .stated
            .and_then(|(a, b)| b.checked_sub(a))
            .filter(|&s| u64::from(s.abs_diff(sum)) * 1000 <= u64::from(sum) * AGREE_PER_MILLE && sum > 0);
        (self.n >= 2).then_some(stated.unwrap_or(sum))
    }
}

/// Descents kept of a profile, at most.
pub const DESCENTS: usize = 8;
/// A descent is at least this long, m, and this steep on average, % x 10 (shorter than a climb: a steep drop is
/// what the STEEP DESCENT alert is for).
const DESC_MIN_LEN_M: u32 = 300;
const DESC_MIN_GRADE_X10: u32 = 30;
/// The window the steepest grade is taken over, m, and how far it slides each step.
const DESC_WINDOW_M: u32 = 50;
const DESC_STEP_M: u32 = 25;

/// A descent of a profile: found as a climb of the upside-down profile, with its steepest 50 m.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Descent {
    /// Where it starts, m from the profile's start.
    pub start_m: u32,
    /// Its length, m.
    pub len_m: u32,
    /// Its drop, m.
    pub drop_m: u32,
    /// Its steepest grade over 50 m, % x 10.
    pub max_x10: u32,
}

impl Descent {
    /// No descent (the empty slot).
    pub const NONE: Descent = Descent { start_m: 0, len_m: 0, drop_m: 0, max_x10: 0 };

    /// Where it ends, m.
    pub fn end_m(&self) -> u32 {
        self.start_m.saturating_add(self.len_m)
    }

    /// Its average grade, % x 10.
    pub fn grade_x10(&self) -> u32 {
        (self.drop_m * 1000).checked_div(self.len_m).unwrap_or(0)
    }

    /// The descents of any profile `total` m long whose altitude (dm) at a distance (m) is `alt_at`: the climbs of
    /// the profile upside down ([`Course::runs_of`]), each with its steepest [`DESC_WINDOW_M`] m. A course's own and
    /// the road ahead's (`climbs_ahead`).
    pub(crate) fn of(total: u32, alt_at: impl Fn(u32) -> Option<i32>) -> ([Descent; DESCENTS], usize) {
        let (runs, n) = Course::runs_of(total, |d| alt_at(d).map(|a| -a), DESC_MIN_LEN_M, DESC_MIN_GRADE_X10);
        let (mut out, mut k) = ([Descent::NONE; DESCENTS], 0);
        for r in runs.iter().take(n) {
            let (mut max, mut d) = (0i32, r.start_m);
            while d < r.end_m() {
                let to = (d + DESC_WINDOW_M).min(r.end_m());
                if let (Some(a), Some(b)) = (alt_at(d), alt_at(to))
                    && to > d
                {
                    max = max.max((a - b) * 100 / (to - d) as i32);
                }
                d += DESC_STEP_M;
            }
            if let Some(slot) = out.get_mut(k) {
                *slot =
                    Descent { start_m: r.start_m, len_m: r.len_m, drop_m: r.gain_m, max_x10: max.clamp(0, 999) as u32 };
                k += 1;
            }
        }
        (out, k)
    }
}

/// A climb on the course.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Climb {
    /// Where it starts, m from the course's start.
    pub start_m: u32,
    /// Its length, m.
    pub len_m: u32,
    /// Its gain, m.
    pub gain_m: u32,
}

impl Climb {
    /// The climb's score, length (m) x average grade (%): the grade is gain / length, so this is 100 x the gain (m).
    /// Category scheme (inferred, from search results citing cyclingmagazine.ca and cycling forums, not checked against
    /// ASO): score 8000 is category 4, 16000 category 3, 32000 category 2, 64000 category 1, 80000 HC.
    pub fn score(&self) -> u32 {
        self.gain_m.saturating_mul(100)
    }

    /// MEDIUM+ (category 3 or harder): a score of 16000 or more (CLIMBS: the climbs ahead keep only these).
    pub fn medium_plus(&self) -> bool {
        self.score() >= 16_000
    }

    /// Where it ends, m from the start.
    pub fn end_m(&self) -> u32 {
        self.start_m.saturating_add(self.len_m)
    }

    /// Its average grade, % x 10.
    pub fn grade_x10(&self) -> u32 {
        (self.gain_m * 1000).checked_div(self.len_m).unwrap_or(0)
    }
}

/// What following the course has to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Off the course (after [`OFF_S`] s beyond [`OFF_M`]).
    Off,
    /// Back on it.
    Back,
    /// A climb starts.
    Climb(Climb),
    /// A cue ahead, and how far, m (0: at the turn) ([`cues`]).
    Cue(Cue, u32),
}

/// What a course file gives, in order: a point, or a cue at a position (degrees x 1e-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// A point of the course.
    Point(Pt),
    /// A cue (FIT `course_point`, a GPX turn word), with the file's distance of it if stated (m).
    Cue(i32, i32, Turn, Option<u32>),
    /// A cue on the point given last (a GPX track point's own turn word).
    CueHere(Turn),
}

/// Where the rider is on the course.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracker {
    /// The segment the rider is on (from point `seg` to `seg + 1`).
    seg: usize,
    /// Distance along the course, m.
    pub along_m: u32,
    /// Distance from the course, m; None before the first fix.
    pub off_m: Option<u32>,
    off_s: u32,
    lost_s: u32,
    /// The course was joined (within [`BACK_M`]) at least once: only then is going off it said.
    joined: bool,
    warned: bool,
    /// Climbs said so far (the next one to say).
    said: usize,
    /// Segments and chunk boxes looked at so far: the cost (src/sim/ checks a budget).
    pub examined: u32,
    /// The next cue (its index), what of it was said (0 nothing, 1 the warning, 2 the turn), the speed along the
    /// course, m/s.
    cue: usize,
    cue_said: u8,
    speed: u32,
    /// Fixes in a row placed back before the last cue passed ([`cues::BACK_FIXES`]).
    back_s: u8,
    /// The last fix on the course's plane (metres), for [`Course::to_start`].
    here: (i32, i32),
}

impl Tracker {
    /// Not placed yet.
    pub const fn new() -> Self {
        Tracker {
            seg: 0,
            along_m: 0,
            off_m: None,
            off_s: 0,
            lost_s: 0,
            joined: false,
            warned: false,
            said: 0,
            examined: 0,
            cue: 0,
            cue_said: 0,
            speed: 0,
            back_s: 0,
            here: (0, 0),
        }
    }

    /// The course was joined (within [`BACK_M`]) at least once.
    pub fn joined(&self) -> bool {
        self.joined
    }

    /// Off course was said and the rider is not back yet.
    pub fn off_course(&self) -> bool {
        self.warned
    }
}

/// The nearest point found: its segment, its distance along the course and from it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub(crate) seg: usize,
    pub(crate) along_m: u32,
    pub(crate) off_m: u32,
}

/// The nearest point of the step from `a` to `b` (plane metres, at distances `da` and `db` along the track) to `p`:
/// squared distance and distance along. Shared with the segment matcher (`segment_match`).
pub(crate) fn seg_hit(a: (i32, i32), b: (i32, i32), (da, db): (u32, u32), p: (i32, i32)) -> (i64, u32) {
    let (abx, aby) = (i64::from(b.0) - i64::from(a.0), i64::from(b.1) - i64::from(a.1));
    let (apx, apy) = (i64::from(p.0) - i64::from(a.0), i64::from(p.1) - i64::from(a.1));
    let l2 = abx * abx + aby * aby;
    // where along the segment, 0 to 65536 (every value here is within 4e6 m: no overflow)
    let t = if l2 == 0 { 0 } else { ((apx * abx + apy * aby).clamp(0, l2) << 16) / l2 };
    let (cx, cy) = (apx - ((abx * t) >> 16), apy - ((aby * t) >> 16));
    let along = i64::from(da) + ((i64::from(db.saturating_sub(da)) * t) >> 16);
    (cx * cx + cy * cy, along.clamp(0, i64::from(u32::MAX)) as u32)
}

/// The nearest segment among `ranges`, `on(i)` giving segment `i`'s [`seg_hit`] to the fix. The segments about as
/// near as the nearest ([`TIE_M`]) make runs of consecutive segments, one per pass of the track near the fix; each
/// run's nearest segment stands for it, and the pass nearest to `along` (where the rider was) wins.
pub(crate) fn nearest_by(
    on: impl Fn(usize) -> Option<(i64, u32)>,
    ranges: &[core::ops::Range<usize>],
    along: u32,
    n: &mut u32,
) -> Option<Hit> {
    let segs = || ranges.iter().flat_map(|r| r.clone()).filter_map(|i| Some((i, on(i)?)));
    let mut best_d2 = i64::MAX;
    for (_, (d2, _)) in segs() {
        *n = n.saturating_add(1);
        best_d2 = best_d2.min(d2);
    }
    let off = best_d2.unsigned_abs().isqrt();
    let tie = (off + u64::from(TIE_M)).saturating_mul(off + u64::from(TIE_M));
    let mut pick: Option<Hit> = None;
    let mut close = |run: Option<(usize, i64, Hit)>| {
        if let Some((_, _, h)) = run
            && pick.is_none_or(|k| h.along_m.abs_diff(along) < k.along_m.abs_diff(along))
        {
            pick = Some(h);
        }
    };
    let mut run: Option<(usize, i64, Hit)> = None;
    for (i, (d2, a)) in segs() {
        let h = Hit { seg: i, along_m: a, off_m: d2.unsigned_abs().isqrt() as u32 };
        run = match run {
            _ if d2.unsigned_abs() > tie => {
                close(run);
                None
            }
            Some((last, rd2, rh)) if last + 1 == i => Some(if d2 < rd2 { (i, d2, h) } else { (i, rd2, rh) }),
            _ => {
                close(run);
                Some((i, d2, h))
            }
        };
    }
    close(run);
    pick
}

/// A course: its points, their distances, altitudes and ascent, the chunks' boxes and the climbs.
pub struct Course {
    /// The first point (degrees x 1e-7) and cos of its latitude x 65536: the plane's origin.
    origin: Option<(i32, i32, i64)>,
    xy: [(i32, i32); POINTS],
    dist: [u32; POINTS],
    alt: [u16; POINTS],
    /// Ascent from the start, m.
    asc: [u16; POINTS],
    len: usize,
    step_m: u32,
    boxes: [(i32, i32, i32, i32); CHUNKS],
    n_boxes: usize,
    climbs: [Climb; CLIMBS],
    n_climbs: usize,
    has_alt: bool,
    /// Lowest and highest altitude, dm.
    range: (i32, i32),
    id: u32,
    /// Bit `i`: no segment from point `i - 1` to point `i` (a gap between two track segments, [`JUMP_M`]).
    gaps: [u32; POINTS / 32],
    cues: Cues,
    // while reading: the distance so far, the last point not kept yet
    odo: Odometer,
    pending: Option<((i32, i32), u32, u16)>,
    descents: [Climb; DESCENTS],
    /// Each descent's steepest grade (their `max_x10`): plain arrays, as an array of structs here grew the course
    /// memory's start-up frame past its budget (tools/stack_check.py).
    desc_max: [u16; DESCENTS],
    n_descents: usize,
}

impl Default for Course {
    fn default() -> Self {
        Self::new()
    }
}

impl Course {
    /// No course.
    pub const fn new() -> Self {
        Course {
            origin: None,
            xy: [(0, 0); POINTS],
            dist: [0; POINTS],
            alt: [0; POINTS],
            asc: [0; POINTS],
            len: 0,
            step_m: FIRST_STEP_M,
            boxes: [(0, 0, 0, 0); CHUNKS],
            n_boxes: 0,
            climbs: [Climb { start_m: 0, len_m: 0, gain_m: 0 }; CLIMBS],
            n_climbs: 0,
            has_alt: false,
            range: (0, 0),
            id: 0,
            gaps: [0; POINTS / 32],
            cues: Cues::new(),
            odo: Odometer::new(),
            pending: None,
            descents: [Climb { start_m: 0, len_m: 0, gain_m: 0 }; DESCENTS],
            desc_max: [0; DESCENTS],
            n_descents: 0,
        }
    }

    /// Empties it for a new course (its id is kept, and changes at [`Course::finish`]).
    pub fn clear(&mut self) {
        (self.origin, self.len, self.step_m, self.n_boxes, self.n_climbs) = (None, 0, FIRST_STEP_M, 0, 0);
        self.n_descents = 0;
        (self.has_alt, self.odo, self.pending, self.gaps) = (false, Odometer::new(), None, [0; POINTS / 32]);
        self.cues = Cues::new();
    }

    /// The next thing the file gives.
    pub fn take(&mut self, item: Item) {
        match item {
            Item::Point(pt) => self.add(pt),
            Item::Cue(lat, lon, turn, dist_m) => self.add_cue(lat, lon, turn, dist_m),
            Item::CueHere(turn) => self.add_cue_here(turn),
        }
    }

    /// The next point of the file: latitude and longitude in degrees x 1e-7, altitude in decimetres. Points off the
    /// globe or farther than [`FAR_M`] from the first are left out.
    pub fn push(&mut self, lat: i32, lon: i32, alt_dm: Option<i32>) {
        self.add(Pt { lat, lon, alt_dm, split: false, dist_m: None });
    }

    /// The next point of the file, with what the file says of it besides: a new track segment starts at it, its
    /// distance from the start (see [`Odometer`]).
    pub fn add(&mut self, pt: Pt) {
        let Pt { lat, lon, alt_dm, .. } = pt;
        if lat.unsigned_abs() > 900_000_000 || lon.unsigned_abs() > 1_800_000_000 {
            return;
        }
        let o = *self.origin.get_or_insert((lat, lon, cos_q16(lat)));
        let p = project(o, lat, lon);
        if p.0.abs() > FAR_M || p.1.abs() > FAR_M {
            return;
        }
        let gap = self.odo.step(pt);
        let (d, a) = (self.odo.run_m(), alt_dm.map_or(NO_ALT, enc));
        if gap {
            // both ends of the gap kept: the last point before it, and this one, marked
            if let Some((q, dq, aq)) = self.pending.take() {
                self.put(q, dq, aq);
            }
            self.put(p, d, a);
            self.mark_gap(self.len.saturating_sub(1));
            return;
        }
        let keep = match self.len.checked_sub(1).and_then(|i| self.dist.get(i)) {
            Some(&prev) => d >= prev.saturating_add(self.step_m),
            None => true,
        };
        if keep {
            self.put(p, d, a);
            self.pending = None;
        } else {
            self.pending = Some((p, d, a));
        }
    }

    fn mark_gap(&mut self, i: usize) {
        if let Some(w) = self.gaps.get_mut(i / 32) {
            *w |= 1 << (i % 32);
        }
    }

    /// No segment from point `i - 1` to point `i`: a gap.
    fn gap_at(&self, i: usize) -> bool {
        is_gap(&self.gaps, i)
    }

    /// Keeps a point; when full, every other point goes first and the step doubles.
    fn put(&mut self, p: (i32, i32), d: u32, a: u16) {
        if self.len >= POINTS {
            for i in 0..POINTS / 2 {
                self.xy[i] = self.xy[2 * i];
                self.dist[i] = self.dist[2 * i];
                self.alt[i] = self.alt[2 * i];
                // a gap into point 2i - 1 or 2i is one into the point kept, 2i (bits read are never below i)
                let gap = i > 0 && (self.gap_at(2 * i - 1) || self.gap_at(2 * i));
                let w = &mut self.gaps[i / 32];
                *w = (*w & !(1 << (i % 32))) | (u32::from(gap) << (i % 32));
            }
            self.gaps[POINTS / 64..].fill(0);
            self.len = POINTS / 2;
            self.step_m = self.step_m.saturating_mul(2);
        }
        if let (Some(x), Some(dd), Some(aa)) =
            (self.xy.get_mut(self.len), self.dist.get_mut(self.len), self.alt.get_mut(self.len))
        {
            (*x, *dd, *aa) = (p, d, a);
            self.len += 1;
        }
    }

    /// The file has ended: its last point kept, the gaps in the altitude filled, then the ascent, the boxes and the
    /// climbs worked out; a new id.
    pub fn finish(&mut self) {
        if let Some((p, d, a)) = self.pending.take() {
            self.put(p, d, a);
        }
        let n = self.len.min(POINTS);
        // the file's own length when it agrees with the points' (see Odometer::len_m): every distance scaled to it
        let (sum, len) = (self.odo.run_m(), self.odo.len_m().unwrap_or(0));
        if len != sum && sum > 0 {
            for d in self.dist.iter_mut().take(n) {
                *d = (u64::from(*d) * u64::from(len) / u64::from(sum)).min(u64::from(u32::MAX)) as u32;
            }
        }
        self.derive(n);
        self.finish_cues();
        self.id = self.id.wrapping_add(1);
    }

    /// The profile, boxes and climbs of the first `n` points: from their altitudes (gaps filled) and positions.
    fn derive(&mut self, n: usize) {
        let alts = self.alt.get_mut(..n).unwrap_or(&mut []);
        let first = alts.iter().copied().find(|&a| a != NO_ALT);
        self.has_alt = first.is_some();
        let mut prev = first.unwrap_or(enc(0));
        for a in alts.iter_mut() {
            if *a == NO_ALT {
                *a = prev;
            }
            prev = *a;
        }
        let (mut low, mut total, mut range) = (dm(prev), 0i32, (i32::MAX, i32::MIN));
        if let Some(&a0) = alts.first() {
            low = dm(a0);
        }
        for (a, asc) in alts.iter().zip(self.asc.iter_mut()) {
            let v = dm(*a);
            range = (range.0.min(v), range.1.max(v));
            if v > low + ASC_DM {
                total = total.saturating_add(v - low);
                low = v;
            } else if v < low {
                low = v;
            }
            *asc = (total / 10).clamp(0, i32::from(u16::MAX)) as u16;
        }
        self.range = if n == 0 { (0, 0) } else { range };
        // a box per chunk of segments: points c * CHUNK to (c + 1) * CHUNK, the last one shared with the next
        self.n_boxes = if n == 0 { 0 } else { (n - 1).div_ceil(CHUNK).max(1) };
        let pts = self.xy.get(..n).unwrap_or(&[]);
        for (c, b) in self.boxes.iter_mut().take(self.n_boxes).enumerate() {
            let part = pts.get(c * CHUNK..((c + 1) * CHUNK + 1).min(n)).unwrap_or(&[]);
            *b = part.iter().fold((i32::MAX, i32::MAX, i32::MIN, i32::MIN), |b, p| {
                (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
            });
        }
        (self.climbs, self.n_climbs) = self.find_climbs();
        let (all, n) = Descent::of(self.len_m(), |d| self.alt_at(d));
        self.n_descents = n.min(DESCENTS);
        for ((c, m), d) in self.descents.iter_mut().zip(self.desc_max.iter_mut()).zip(all.iter().take(n)) {
            (*c, *m) = (Climb { start_m: d.start_m, len_m: d.len_m, gain_m: d.drop_m }, d.max_x10 as u16);
        }
    }

    /// The course followed the other way: points, distances, altitudes, gaps, ascent, boxes and climbs of the
    /// reversed profile, and the turns found again on the reversed shape (a right turn is a left one). The file's own
    /// cues are dropped (their turns would be the wrong way round). The plane's origin stays where it was.
    #[inline(never)]
    pub fn reverse(&mut self) {
        let n = self.len.min(POINTS);
        let total = self.len_m();
        if let Some(s) = self.xy.get_mut(..n) {
            s.reverse();
        }
        if let Some(s) = self.alt.get_mut(..n) {
            s.reverse();
        }
        if let Some(s) = self.dist.get_mut(..n) {
            s.reverse();
            for d in s {
                *d = total.saturating_sub(*d);
            }
        }
        // no segment between new points i - 1 and i = none between old points n - i and n - 1 - i
        let old = core::mem::replace(&mut self.gaps, [0; POINTS / 32]);
        for i in 1..n {
            if is_gap(&old, n - i) {
                self.mark_gap(i);
            }
        }
        let had_alt = self.has_alt;
        self.derive(n);
        self.has_alt = had_alt;
        self.recue();
        self.id = self.id.wrapping_add(1);
    }

    /// The course from a track: `pts` (metres east and north of `origin`, degrees x 1e-7), in the order to follow
    /// them, thinned to [`POINTS`] as a file's are. No altitudes, no cues of a file: the shape's turns.
    #[inline(never)]
    pub fn from_plane(&mut self, origin: (i32, i32), pts: impl Iterator<Item = (i32, i32)>) {
        self.clear();
        self.origin = Some((origin.0, origin.1, cos_q16(origin.0)));
        let (mut d, mut prev, mut widest) = (0u32, None::<(i32, i32)>, 0u32);
        for p in pts {
            if let Some(q) = prev {
                let gap = hypot(i64::from(p.0) - i64::from(q.0), i64::from(p.1) - i64::from(q.1));
                (d, widest) = (d.saturating_add(gap), widest.max(gap));
            }
            // every point is kept (put thins by halving when full, the last one is pushed after it)
            self.put(p, d, NO_ALT);
            prev = Some(p);
        }
        // a sparse track: the matcher's chord tolerance and the cues' arms follow the spacing ([`Course::follow`])
        self.step_m = self.step_m.max(widest);
        self.finish();
    }

    /// While the rider has not joined the course: the bearing (degrees clockwise from north) and the distance (m)
    /// of its start from the last fix, in a straight line (routing is not here). None once joined.
    pub fn to_start(&self, t: &Tracker) -> Option<(u32, u32)> {
        let s = self.xy.first().filter(|_| self.len >= 2 && t.off_m.is_some() && !t.joined)?;
        let (dx, dy) = (i64::from(s.0) - i64::from(t.here.0), i64::from(s.1) - i64::from(t.here.1));
        let m = hypot(dx, dy);
        Some(((-angle_deg((0, 1), (dx, dy))).rem_euclid(360) as u32, m))
    }

    /// Where (`lat`, `lon`) (degrees x 1e-7) is on this course's plane, metres east and north of its first point.
    pub fn plane_of(&self, lat: i32, lon: i32) -> Option<(i32, i32)> {
        self.origin.map(|o| project(o, lat, lon))
    }

    /// The degrees x 1e-7 (latitude, longitude) of a point of this course's plane.
    pub fn lat_lon(&self, p: (i32, i32)) -> Option<(i32, i32)> {
        let (lat, lon, cos) = self.origin?;
        let dlat = i64::from(p.1) * 1_000_000 / 11_132;
        let dlon = i64::from(p.0) * 1_000_000 * 65_536 / (11_132 * cos.max(1));
        Some((
            (i64::from(lat) + dlat).clamp(-900_000_000, 900_000_000) as i32,
            (i64::from(lon) + dlon).clamp(-1_800_000_000, 1_800_000_000) as i32,
        ))
    }

    /// How near the course passes `p` (plane metres) if within `max_m`: its distance and the distance along the
    /// course there (the first pass, on a lap course).
    pub fn near(&self, p: (i32, i32), max_m: u32) -> Option<(u32, u32)> {
        let segs = self.len.saturating_sub(1);
        let mut best: Option<(u32, u32)> = None;
        for (c, b) in self.boxes.iter().take(self.n_boxes).enumerate() {
            if box_m(b, p) > u64::from(max_m) {
                continue;
            }
            let r = (c * CHUNK).min(segs)..((c + 1) * CHUNK).min(segs);
            let hit = self.nearest(p, &[r], 0, &mut 0).filter(|h| h.off_m <= max_m);
            if let Some(h) = hit.filter(|h| best.is_none_or(|(o, _)| h.off_m < o)) {
                best = Some((h.off_m, h.along_m));
            }
        }
        best
    }

    /// The points kept, metres east and north of the first.
    pub fn points(&self) -> &[(i32, i32)] {
        self.xy.get(..self.len).unwrap_or(&[])
    }

    /// Its length, m.
    pub fn len_m(&self) -> u32 {
        self.len.checked_sub(1).and_then(|i| self.dist.get(i)).copied().unwrap_or(0)
    }

    /// Its climbs, in order.
    pub fn climbs(&self) -> &[Climb] {
        self.climbs.get(..self.n_climbs).unwrap_or(&[])
    }

    /// Its descents, in order.
    pub fn descents(&self) -> impl Iterator<Item = Descent> + '_ {
        let (c, m) = (self.descents.get(..self.n_descents), self.desc_max.get(..self.n_descents));
        c.unwrap_or(&[]).iter().zip(m.unwrap_or(&[])).map(|(c, &m)| Descent {
            start_m: c.start_m,
            len_m: c.len_m,
            drop_m: c.gain_m,
            max_x10: u32::from(m),
        })
    }

    /// It has altitudes.
    pub fn has_alt(&self) -> bool {
        self.has_alt
    }

    /// Lowest and highest altitude, dm.
    pub fn range_dm(&self) -> (i32, i32) {
        self.range
    }

    /// Changes with each course read.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The course on the map of a track whose plane has its origin at `track` (degrees x 1e-7); None with fewer than
    /// two points.
    pub fn route(&self, track: (i32, i32)) -> Option<Route<'_>> {
        let (lat, lon, _) = self.origin.filter(|_| self.len >= 2)?;
        let off = project((track.0, track.1, cos_q16(track.0)), lat, lon);
        let boxes = self.boxes.get(..self.n_boxes).unwrap_or(&[]);
        let gaps = &self.gaps;
        Some(Route { pts: self.points(), boxes, gaps, off, id: self.id })
    }

    /// The altitude at `d` m from the start, dm, between the two points around it; None without altitudes.
    pub fn alt_at(&self, d: u32) -> Option<i32> {
        let n = self.len.min(POINTS);
        let dist = self.dist.get(..n).unwrap_or(&[]);
        let i = dist.partition_point(|&x| x <= d);
        let at = |k: usize| self.alt.get(k).map(|&a| dm(a));
        let (Some(&d0), Some(a0)) = (dist.get(i.saturating_sub(1)), at(i.saturating_sub(1))) else { return None };
        let v = match (dist.get(i), at(i)) {
            (Some(&d1), Some(a1)) if d1 > d0 && i > 0 => {
                a0 + ((i64::from(a1 - a0) * i64::from(d.saturating_sub(d0))) / i64::from(d1 - d0)) as i32
            }
            _ => a0,
        };
        self.has_alt.then_some(v)
    }

    /// The grade of the [`AHEAD_M`] after `d`, % x 10 (shorter near the end; None within 20 m of it).
    pub fn grade_ahead(&self, d: u32) -> Option<i32> {
        let to = d.saturating_add(AHEAD_M).min(self.len_m());
        let len = to.checked_sub(d).filter(|&l| l >= 20)?;
        Some((self.alt_at(to)? - self.alt_at(d)?) * 100 / len as i32)
    }

    /// The ascent left from where `t` is to the end, m.
    pub fn ascent_left(&self, t: &Tracker) -> u32 {
        let at = |i: usize| u32::from(self.asc.get(i).copied().unwrap_or(0));
        at(self.len.saturating_sub(1)).saturating_sub(at(t.seg))
    }

    /// The climb at `d` m from the start, or the next one: its index, and whether `d` is on it.
    pub fn climb_at(&self, d: u32) -> Option<(usize, bool)> {
        let k = self.climbs().iter().position(|c| c.end_m() > d)?;
        Some((k, self.climbs().get(k).is_some_and(|c| d >= c.start_m)))
    }

    /// Profile column `k` of `cols` from `start` m over `span` m: its altitude (dm, at its middle), its grade (% x 10,
    /// over the column) and whether it is on a climb; None without altitudes.
    pub fn column(&self, start: u32, span: u32, cols: u32, k: u32) -> Option<(i32, i32, bool)> {
        let at = |i: u32| start.saturating_add((u64::from(span) * u64::from(i) / u64::from(cols.max(1))) as u32);
        let (d0, d1) = (at(k), at(k + 1));
        let mid = d0 + (d1 - d0) / 2;
        // over 100 m at least: a narrow column's few decimetres would flicker between two colours
        let half = ((d1 - d0) / 2).max(50);
        let (g0, g1) = (mid.saturating_sub(half), mid.saturating_add(half).min(self.len_m()));
        let grade = match g1.saturating_sub(g0) {
            0 => 0,
            len => (self.alt_at(g1)? - self.alt_at(g0)?) * 100 / len.min(i32::MAX as u32) as i32,
        };
        let climb = self.climbs().iter().any(|c| mid >= c.start_m && mid < c.end_m());
        Some((self.alt_at(mid)?, grade, climb))
    }

    /// Segment `i`'s nearest point to `p`: squared distance and distance along the course.
    fn on_segment(&self, i: usize, p: (i32, i32)) -> Option<(i64, u32)> {
        if self.gap_at(i + 1) {
            return None; // not ridden
        }
        let (&a, &b) = (self.xy.get(i)?, self.xy.get(i + 1)?);
        let (&da, &db) = (self.dist.get(i)?, self.dist.get(i + 1)?);
        Some(seg_hit(a, b, (da, db), p))
    }

    /// The nearest segment to `p` among `ranges` ([`nearest_by`]).
    fn nearest(&self, p: (i32, i32), ranges: &[core::ops::Range<usize>], along: u32, n: &mut u32) -> Option<Hit> {
        nearest_by(|i| self.on_segment(i, p), ranges, along, n)
    }

    /// The nearest segment among [`SEARCH_CHUNKS`] chunks: of the chunks whose boxes are about as near to `p` as the
    /// nearest box (within [`LOST_M`]), the ones nearest along the course to `along`, where the rider was. On a lap
    /// course every lap's box holds `p`: the nearest box alone would always pick the first laps (review 2).
    fn search(&self, p: (i32, i32), along: u32, n: &mut u32) -> Option<Hit> {
        let boxes = self.boxes.get(..self.n_boxes).unwrap_or(&[]);
        let box_m = |b: &(i32, i32, i32, i32)| box_m(b, p);
        let near = boxes.iter().map(box_m).min().unwrap_or(u64::MAX).saturating_add(u64::from(LOST_M));
        let segs = self.len.saturating_sub(1);
        let span = |c: usize| (c * CHUNK).min(segs)..((c + 1) * CHUNK).min(segs);
        let at = |i: usize| self.dist.get(i).copied().unwrap_or(0);
        let mut pick = [(u32::MAX, 0usize); SEARCH_CHUNKS];
        for (c, b) in boxes.iter().enumerate() {
            *n = n.saturating_add(1);
            if box_m(b) > near {
                continue;
            }
            let r = span(c);
            let gap = at(r.start).saturating_sub(along).max(along.saturating_sub(at(r.end)));
            if let Some(worst) = pick.iter_mut().max_by_key(|x| x.0).filter(|w| gap < w.0) {
                *worst = (gap, c);
            }
        }
        let ranges = pick.map(|(gap, c)| if gap == u32::MAX { 0..0 } else { span(c) });
        self.nearest(p, &ranges, along, n)
    }

    /// The rider's fix (degrees x 1e-7), once a second: where `t` is now, and what to say (one thing at a time).
    pub fn follow(&self, t: &mut Tracker, lat: i32, lon: i32) -> Option<Event> {
        let was = t.off_m.map(|_| t.along_m);
        let o = self.origin.filter(|_| self.len >= 2)?;
        let p = project(o, lat, lon);
        t.here = p;
        let segs = self.len - 1;
        let window = t.seg.saturating_sub(BEHIND)..(t.seg + AHEAD).min(segs);
        let mut hit = self.nearest(p, &[window], t.along_m, &mut t.examined);
        let lost = t.off_m.is_none() || hit.is_none_or(|h| h.off_m > LOST_M);
        if lost && t.lost_s.is_multiple_of(SEARCH_S) {
            let wide = self.search(p, t.along_m, &mut t.examined);
            if let Some(w) = wide.filter(|w| hit.is_none_or(|h| w.off_m.saturating_add(TIE_M) < h.off_m)) {
                hit = Some(w);
            }
        }
        t.lost_s = if lost { t.lost_s.saturating_add(1) } else { 0 };
        let h = hit?;
        (t.seg, t.along_m, t.off_m) = (h.seg, h.along_m, Some(h.off_m));
        t.joined |= h.off_m <= BACK_M;
        // a chord between two kept points cuts a bend by up to half the step: thinned past 100 m, that alone would
        // pass OFF_M
        let off_limit = OFF_M.max(self.step_m / 2 + BACK_M);
        t.off_s = if h.off_m > off_limit { t.off_s.saturating_add(1) } else { 0 };
        if t.joined && !t.warned && t.off_s >= OFF_S {
            t.warned = true;
            return Some(Event::Off);
        }
        if t.warned && h.off_m <= BACK_M {
            t.warned = false;
            return Some(Event::Back);
        }
        if let Some(e) = self.cue_step(t, was) {
            return Some(e);
        }
        // climbs gone by (the rider joined later on) are not said; the one being entered is, once
        let climbs = self.climbs();
        t.said = t.said.max(climbs.iter().position(|c| c.end_m() > h.along_m).unwrap_or(climbs.len()));
        let c = *climbs.get(t.said)?;
        (h.along_m >= c.start_m && h.off_m <= OFF_M).then(|| {
            t.said += 1;
            Event::Climb(c)
        })
    }

    /// The climbs, worked out from the profile in [`BIN_M`] steps: a step climbs at [`UP_X10`] or more; a climb
    /// starts at a climbing step and goes on while new tops come, within [`GAP_M`] and [`DIP_DM`] of the last one;
    /// it is kept from its start to its top if it is a climb ([`MIN_LEN_M`], [`MIN_GRADE_X10`]).
    // ponytail: made for the smooth profiles of planned courses (the apps take altitudes from a terrain model); on a
    // noisy recorded track a climb may start early. Smooth first (a moving average) if that shows.
    fn find_climbs(&self) -> ([Climb; CLIMBS], usize) {
        Self::climbs_of(self.len_m(), |d| self.alt_at(d))
    }

    /// The climbs of any profile `total` m long whose altitude (dm) at a distance (m) is `alt_at`, by the same
    /// definition: a course's own, and the road ahead's (`climbs_ahead`).
    pub(crate) fn climbs_of(total: u32, alt_at: impl Fn(u32) -> Option<i32>) -> ([Climb; CLIMBS], usize) {
        Self::runs_of(total, alt_at, MIN_LEN_M, MIN_GRADE_X10)
    }

    /// [`Course::climbs_of`] with its own minimum length (m) and grade (% x 10): a descent is a climb of the profile
    /// turned upside down (`gain_m` is then its drop).
    pub(crate) fn runs_of(
        total: u32,
        alt_at: impl Fn(u32) -> Option<i32>,
        min_len: u32,
        min_grade: u32,
    ) -> ([Climb; CLIMBS], usize) {
        let (mut out, mut n) = ([Climb::default(); CLIMBS], 0);
        let mut accept = |from: u32, top: u32| {
            let len = top.saturating_sub(from);
            let gain = alt_at(top).zip(alt_at(from)).map_or(0, |(t, f)| (t - f).max(0) as u32);
            if len >= min_len
                && gain * 100 / len >= min_grade
                && let Some(slot) = out.get_mut(n)
            {
                *slot = Climb { start_m: from, len_m: len, gain_m: gain / 10 };
                n += 1;
            }
        };
        // wider steps past BINS of them (over 1600 km): a crafted file's length alone cannot make the loop long
        let bin = BIN_M.max(total.div_ceil(BINS));
        // the climb being followed: its start, its top (m, dm)
        let mut cur: Option<(u32, u32, i32)> = None;
        for b in 0..total.div_ceil(bin) {
            let (d0, d1) = (b.saturating_mul(bin), (b + 1).saturating_mul(bin).min(total));
            let (Some(a0), Some(a1)) = (alt_at(d0), alt_at(d1)) else { break };
            let up = (a1 - a0) * 100 / (d1 - d0).max(1) as i32 >= UP_X10;
            cur = match cur {
                None if up => Some((d0, d1, a1)),
                None => None,
                Some((s, _, top)) if a1 > top + 5 => Some((s, d1, a1)),
                Some((s, at, top)) if d1 - at > GAP_M || top - a1 > DIP_DM => {
                    accept(s, at);
                    up.then_some((d0, d1, a1))
                }
                keep => keep,
            };
        }
        if let Some((s, at, _)) = cur {
            accept(s, at);
        }
        (out, n)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// Degrees x 1e-7 per metre north (as map.rs's 0.011132 m per unit).
    const NORTH: f64 = 1.0 / 0.011_132;

    /// A course from points in metres east and north of 0 N 0 E, with altitudes in metres.
    pub fn course(pts: &[(f64, f64, f64)]) -> Course {
        let mut c = Course::new();
        for &(x, y, z) in pts {
            c.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some((z * 10.0).round() as i32));
        }
        c.finish();
        c
    }

    /// Points every `step` m along straight lines between `corners` (x, y, altitude).
    pub fn lines(corners: &[(f64, f64, f64)], step: f64) -> Vec<(f64, f64, f64)> {
        let mut v = std::vec![corners[0]];
        for w in corners.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            let n = (len / step).ceil().max(1.0) as usize;
            for k in 1..=n {
                let t = k as f64 / n as f64;
                v.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t));
            }
        }
        v
    }

    fn fix(x: f64, y: f64) -> (i32, i32) {
        ((y * NORTH) as i32, (x * NORTH) as i32)
    }

    /// Descents: a long gentle one (4 km at 4 %) and a short steep one (300 m at 12 %) are both found, with their
    /// length, drop and steepest 50 m; a climb is not a descent, and the climbs are the ones they were.
    #[test]
    fn descents_gentle_and_steep() {
        let c = course(&lines(
            &[
                (0.0, 0.0, 300.0),
                (4000.0, 0.0, 140.0),
                (5000.0, 0.0, 140.0),
                (5300.0, 0.0, 104.0),
                (6300.0, 0.0, 104.0),
                (7300.0, 0.0, 164.0),
            ],
            10.0,
        ));
        let d: std::vec::Vec<_> = c.descents().collect();
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(d[0].start_m <= 50 && d[0].len_m.abs_diff(4000) <= 100 && d[0].drop_m.abs_diff(160) <= 5, "{:?}", d[0]);
        assert!((38..=45).contains(&d[0].max_x10) && (38..=42).contains(&d[0].grade_x10()), "gentle: {:?}", d[0]);
        assert!(d[1].start_m.abs_diff(5000) <= 50 && d[1].len_m.abs_diff(300) <= 50, "{:?}", d[1]);
        assert!((115..=125).contains(&d[1].max_x10), "steep: {:?}", d[1]);
        assert_eq!(c.climbs().len(), 1, "the 60 m rise is the only climb");
        assert!(c.climbs()[0].start_m.abs_diff(6300) <= 50);
        assert!(course(&lines(&[(0.0, 0.0, 100.0), (2000.0, 0.0, 160.0)], 10.0)).descents().next().is_none());
    }

    /// The steepest 50 m of a descent that is gentle on average: a 1 km drop at 3 % with a 100 m wall at 15 %.
    #[test]
    fn descent_max_grade_is_the_steepest_window() {
        let c =
            course(&lines(&[(0.0, 0.0, 200.0), (450.0, 0.0, 190.0), (550.0, 0.0, 175.0), (1000.0, 0.0, 165.0)], 5.0));
        let d: std::vec::Vec<_> = c.descents().collect();
        assert_eq!(d.len(), 1, "{d:?}");
        assert!((140..=160).contains(&d[0].max_x10) && d[0].grade_x10() < 40, "{:?}", d[0]);
    }

    /// Reversed: the same length, the ends swapped, the ascent and climbs of the profile read downhill, and the
    /// turns found again on the reversed shape: an L whose corner turns left is, the other way, a right turn.
    #[test]
    fn reversed_course() {
        let mut c = course(&lines(&[(0.0, 0.0, 0.0), (3000.0, 0.0, 100.0), (3000.0, 3000.0, 100.0)], 10.0));
        assert_eq!(c.cues().iter().map(|q| q.turn).collect::<Vec<_>>(), [Turn::Left]);
        let (len, first, last, id) = (c.len_m(), c.points()[0], *c.points().last().unwrap(), c.id());
        assert!(c.ascent_left(&Tracker::new()) >= 95 && !c.climbs().is_empty());
        c.reverse();
        assert_eq!((c.len_m(), c.points()[0], *c.points().last().unwrap()), (len, last, first));
        assert_ne!(c.id(), id, "the map fits it again");
        assert_eq!(c.cues().iter().map(|q| q.turn).collect::<Vec<_>>(), [Turn::Right]);
        let q = c.cues()[0].at_m;
        assert!((2990..=3010).contains(&q), "the corner is 3 km from the new start: {q}");
        assert_eq!(c.ascent_left(&Tracker::new()), 0, "downhill all the way");
        assert!(c.climbs().is_empty(), "the climb is a descent now");
        assert!(c.dist[..c.points().len()].windows(2).all(|w| w[0] <= w[1]), "distances rise");
        // twice: the original again
        c.reverse();
        assert_eq!(c.cues().iter().map(|q| q.turn).collect::<Vec<_>>(), [Turn::Left]);
        assert_eq!((c.points()[0], c.len_m()), (first, len));
    }

    /// The way back: a track in the order of its end to its start, thinned to the memory, ending at the start.
    #[test]
    fn course_from_a_track_is_bounded() {
        let track: Vec<(i32, i32)> = (0..512).map(|k| (k, 0)).collect(); // the map's own bound
        let mut c = Course::new();
        c.from_plane((0, 0), track.iter().rev().copied());
        assert_eq!(c.points().len(), 512);
        assert_eq!((c.points()[0], c.len_m()), (track[track.len() - 1], track.len() as u32 - 1));
        assert_eq!(c.points().last(), Some(&(0, 0)), "ends at the start");
        // more than the memory holds: thinned, the ends kept
        let big: Vec<(i32, i32)> = (0..POINTS as i32 * 3).map(|k| (k, 0)).collect();
        c.from_plane((0, 0), big.iter().rev().copied());
        assert!(c.points().len() <= POINTS && c.points().len() >= POINTS / 2, "{}", c.points().len());
        assert_eq!((c.points()[0], c.points().last()), (big[big.len() - 1], Some(&(0, 0))));
        c.from_plane((0, 0), core::iter::empty());
        assert!(c.points().is_empty() && c.len_m() == 0);
    }

    /// A sparse track (512 points over 100 km, 195 m apart) followed from its start is never off course.
    #[test]
    fn a_sparse_way_back_is_followed() {
        let track: Vec<(i32, i32)> = (0..512).map(|k| (k * 195, 0)).collect();
        let mut c = Course::new();
        c.from_plane((0, 0), track.iter().copied());
        let mut t = Tracker::new();
        for k in 0..99_600 / 8 {
            let lon = (f64::from(k * 8) / 0.011_132) as i32;
            assert_ne!(c.follow(&mut t, 0, lon), Some(Event::Off), "at {} m", k * 8);
        }
        // a fix far away: no overflow
        let mut t = Tracker::new();
        let _ = c.follow(&mut t, 899_000_000, 1_799_000_000);
        let _ = c.to_start(&t);
    }

    /// Reversed with gaps and without altitudes: the gaps stay between the same points, no altitude appears, and
    /// reversing twice is the course again.
    #[test]
    fn reverse_keeps_gaps_and_no_altitude() {
        let mut c = Course::new();
        for k in 0..40 {
            let x = if k < 20 { k * 10 } else { 5000 + k * 10 }; // a jump at point 20
            c.add(Pt { lat: 0, lon: (f64::from(x) / 0.011_132) as i32, alt_dm: None, split: k == 20, dist_m: None });
        }
        c.finish();
        let n = c.points().len();
        let gaps: Vec<_> = (1..n).filter(|&i| c.gap_at(i)).collect();
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let (pts, dist) = (c.points().to_vec(), c.dist[..n].to_vec());
        c.reverse();
        assert!(!c.has_alt());
        assert_eq!((1..n).filter(|&i| c.gap_at(i)).collect::<Vec<_>>(), [n - gaps[0]]);
        c.reverse();
        assert_eq!((c.points(), &c.dist[..n]), (&pts[..], &dist[..]));
        assert_eq!((1..n).filter(|&i| c.gap_at(i)).collect::<Vec<_>>(), gaps);
    }

    /// Not on the course yet: the bearing and distance of its start, until the course is joined.
    #[test]
    fn straight_to_the_start() {
        let c = course(&lines(&[(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0)], 10.0));
        let mut t = Tracker::new();
        assert_eq!(c.to_start(&t), None, "no fix yet");
        let (lat, lon) = fix(-300.0, -300.0); // south west of the start, the course going east
        c.follow(&mut t, lat, lon);
        let (b, m) = c.to_start(&t).unwrap();
        assert!((44..=46).contains(&b) && (420..=428).contains(&m), "{b} {m}");
        let (lat, lon) = fix(200.0, 10.0);
        c.follow(&mut t, lat, lon);
        assert_eq!(c.to_start(&t), None, "joined");
    }

    /// Distance, the plane, the ascent: 3 km east then 4 km north, rising 100 m on the second leg.
    #[test]
    fn distances_and_ascent() {
        let c = course(&lines(&[(0.0, 0.0, 50.0), (3000.0, 0.0, 50.0), (3000.0, 4000.0, 150.0)], 5.0));
        assert!((6990..=7010).contains(&c.len_m()), "{}", c.len_m());
        let last = *c.points().last().unwrap();
        assert!((last.0 - 3000).abs() <= 3 && (last.1 - 4000).abs() <= 3, "{last:?}");
        assert!((690..=705).contains(&c.points().len()), "a point every 10 m: {}", c.points().len());
        assert!((97..=100).contains(&c.asc[c.len - 1]), "the 3 m hysteresis: {}", c.asc[c.len - 1]);
        assert_eq!(c.alt_at(1500), Some(500));
        assert!(c.alt_at(5000).unwrap().abs_diff(1000) <= 2, "halfway up the second leg: 100 m");
        assert!(c.grade_ahead(4000).unwrap().abs_diff(25) <= 1, "2.5 %");
        assert_eq!(c.range_dm(), (500, 1500));
    }

    /// Points as a file's reader gives them: the first one starting a segment when `split`, the file's own
    /// distance from `stated` (its first point's, its last's) when given.
    fn add(c: &mut Course, pts: &[(f64, f64, f64)], split: bool, stated: Option<(f64, f64)>) {
        for (k, &(x, y, z)) in pts.iter().enumerate() {
            let dist_m = stated.map(|(a, b)| (a + (b - a) * k as f64 / (pts.len() - 1) as f64).round() as u32);
            let (lat, lon) = ((y * NORTH).round() as i32, (x * NORTH).round() as i32);
            c.add(Pt { lat, lon, alt_dm: Some((z * 10.0).round() as i32), split: split && k == 0, dist_m });
        }
    }

    /// The segments that are gaps.
    fn gaps(c: &Course) -> Vec<usize> {
        (1..c.points().len()).filter(|&i| c.gap_at(i)).collect()
    }

    /// Two rides 8 km apart in one GPX (two segments): the gap is not ridden, not counted, not drawn, and a rider
    /// in its middle is not on the course; the same jump inside one segment is ridden. Thinned past POINTS, the gap
    /// stays between the same two places.
    #[test]
    fn gaps_between_segments() {
        let (a, b) = (
            lines(&[(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0)], 5.0),
            lines(&[(10_000.0, 0.0, 0.0), (12_000.0, 0.0, 0.0)], 5.0),
        );
        let mut c = Course::new();
        add(&mut c, &a, true, None);
        add(&mut c, &b, true, None);
        c.finish();
        assert!(c.len_m().abs_diff(4000) <= 2, "the two rides only: {}", c.len_m());
        let g = gaps(&c);
        assert_eq!(g.len(), 1, "{g:?}");
        let (p, q) = (c.points()[g[0] - 1], c.points()[g[0]]);
        assert!((p.0 - 2000).abs() <= 2 && (q.0 - 10_000).abs() <= 2, "{p:?} to {q:?}");
        assert!(crate::course::is_gap(c.route((0, 0)).unwrap().gaps, g[0]), "not drawn");
        let mut t = Tracker::new();
        let (lat, lon) = fix(6000.0, 0.0);
        c.follow(&mut t, lat, lon);
        assert!(t.off_m.unwrap() >= 3990, "the gap's middle is 4 km from the course: {:?}", t.off_m);
        // the same jump inside one segment: ridden
        let mut c = Course::new();
        add(&mut c, &a, true, None);
        add(&mut c, &b, false, None);
        c.finish();
        assert!(c.len_m().abs_diff(12_000) <= 3 && gaps(&c).is_empty(), "{}", c.len_m());
        // thinned: one gap, between the same places
        let a = lines(&[(0.0, 0.0, 0.0), (30_000.0, 0.0, 0.0)], 5.0);
        let b = lines(&[(50_000.0, 0.0, 0.0), (80_000.0, 0.0, 0.0)], 5.0);
        let mut c = Course::new();
        add(&mut c, &a, true, None);
        add(&mut c, &b, true, None);
        c.finish();
        assert!(c.step_m >= 20, "thinned: {}", c.step_m);
        assert!(c.len_m().abs_diff(60_000) <= 10, "{}", c.len_m());
        let g = gaps(&c);
        assert_eq!(g.len(), 1, "{g:?}");
        let (p, q) = (c.points()[g[0] - 1], c.points()[g[0]]);
        let s = 2 * c.step_m as i32; // a gap's end may go at a thinning, its neighbour kept
        assert!((30_000 - s..=30_000).contains(&p.0) && (50_000..=50_000 + s).contains(&q.0), "{p:?} {q:?}");
    }

    /// A FIT course's own distance (its records'): taken within 2 % of the summed points, every distance scaled to
    /// it (the top at the end still); else the sum. Counted from the file's first point.
    #[test]
    fn stated_length() {
        let pts = lines(&[(0.0, 0.0, 0.0), (0.0, 5000.0, 100.0)], 5.0);
        for (stated, want) in [(5050.0, 5050), (4920.0, 4920), (6000.0, 5000), (4800.0, 5000)] {
            let mut c = Course::new();
            add(&mut c, &pts, false, Some((1000.0, 1000.0 + stated)));
            c.finish();
            assert!(c.len_m().abs_diff(want) <= 1, "stated {stated}: {}", c.len_m());
            assert!(c.alt_at(c.len_m()).unwrap().abs_diff(1000) <= 1, "stated {stated}");
        }
    }

    /// A long course is thinned: never more than POINTS points, evenly spaced, both ends kept.
    #[test]
    fn thins_when_full() {
        let c = course(&lines(&[(0.0, 0.0, 0.0), (0.0, 150_000.0, 0.0)], 5.0));
        assert!(c.points().len() <= POINTS && c.points().len() > POINTS / 2, "{}", c.points().len());
        assert_eq!(c.points()[0], (0, 0));
        assert!((c.points().last().unwrap().1 - 150_000).abs() < 20);
        assert!(c.dist[..c.len - 1].windows(2).all(|w| w[1] - w[0] >= c.step_m && w[1] - w[0] < c.step_m + 20));
        assert!((149_990..=150_010).contains(&c.len_m()), "the distance of every point given: {}", c.len_m());
    }

    /// Climbs worked out by hand: 1 km flat at 100 m, 2 km up to 220 m (6 %), a 200 m dip of 5 m, 800 m up 40 m
    /// (5 %), 1 km flat, then 400 m up 30 m (too short), 1 km flat, 1 km up 20 m (2 %: too gentle). The first two
    /// rises are one climb across the dip: from 1000 m to 4000 m, 155 m, 5.1 %.
    #[test]
    fn climbs_by_hand() {
        let c = course(&lines(
            &[
                (0.0, 0.0, 100.0),
                (1000.0, 0.0, 100.0),
                (3000.0, 0.0, 220.0),
                (3200.0, 0.0, 215.0),
                (4000.0, 0.0, 255.0),
                (5000.0, 0.0, 255.0),
                (5400.0, 0.0, 285.0),
                (6400.0, 0.0, 285.0),
                (7400.0, 0.0, 305.0),
            ],
            5.0,
        ));
        let cl = c.climbs();
        assert_eq!(cl.len(), 1, "{cl:?}");
        let k = cl[0];
        assert!((990..=1040).contains(&k.start_m), "{k:?}");
        assert!((3950..=4000).contains(&k.end_m()), "{k:?}");
        assert!((150..=155).contains(&k.gain_m) && (50..=53).contains(&k.grade_x10()), "{k:?}");
        assert_eq!(c.climb_at(500), Some((0, false)));
        assert_eq!(c.climb_at(2000), Some((0, true)));
        assert_eq!(c.climb_at(4500), None);
        // two climbs apart: a 50 m drop between them ends the first
        let c = course(&lines(&[(0.0, 0.0, 0.0), (1000.0, 0.0, 60.0), (2000.0, 0.0, 10.0), (3000.0, 0.0, 80.0)], 5.0));
        let g: Vec<_> = c.climbs().iter().map(|k| (k.start_m, k.len_m, k.gain_m)).collect();
        assert_eq!(g.len(), 2, "{g:?}");
        assert!(g[0].0 <= 5 && (990..=1000).contains(&g[0].1) && (59..=60).contains(&g[0].2), "{g:?}");
        assert!((2000..=2010).contains(&g[1].0) && (69..=70).contains(&g[1].2), "{g:?}");
    }

    /// Following a course: along and off, a fix beside it, the window moving with the rider.
    #[test]
    fn follows() {
        let c = course(&lines(&[(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0), (2000.0, 2000.0, 0.0)], 10.0));
        let mut t = Tracker::default();
        for x in (0..1950).step_by(8) {
            let (lat, lon) = fix(f64::from(x), 30.0);
            assert_eq!(c.follow(&mut t, lat, lon), None);
            assert!(t.along_m.abs_diff(x as u32) <= 3 && t.off_m.unwrap().abs_diff(30) <= 1, "{x}: {t:?}");
        }
        let (lat, lon) = fix(2030.0, 1000.0);
        c.follow(&mut t, lat, lon);
        assert!(t.along_m.abs_diff(3000) <= 2 && t.off_m == Some(30), "found again after a jump: {t:?}");
    }

    /// An out-and-back: going out, the outbound leg; past the turn, the return leg, though both are as near.
    #[test]
    fn out_and_back_takes_the_way_the_rider_goes() {
        let c = course(&lines(&[(0.0, 0.0, 0.0), (3000.0, 0.0, 0.0), (3000.0, 5.0, 0.0), (0.0, 5.0, 0.0)], 10.0));
        let mut t = Tracker::default();
        let mut x = 0.0;
        while x < 3000.0 {
            let (lat, lon) = fix(x, 2.0);
            c.follow(&mut t, lat, lon);
            assert!(x > 2985.0 || t.along_m.abs_diff(x as u32) <= 3, "out at {x}: {t:?}");
            x += 8.0;
        }
        while x > 0.0 {
            x -= 8.0;
            let (lat, lon) = fix(x, 3.0);
            c.follow(&mut t, lat, lon);
            assert!(x > 2985.0 || t.along_m.abs_diff(6005 - x as u32) <= 3, "back at {x}: {t:?}");
        }
        // joining in the middle: the first pass
        let mut t = Tracker::default();
        let (lat, lon) = fix(1500.0, 2.0);
        c.follow(&mut t, lat, lon);
        assert!(t.along_m.abs_diff(1500) <= 3, "{t:?}");
    }

    /// A loop that starts and ends at the same place: at the start, the start; near the end, the end.
    #[test]
    fn loop_course() {
        let c = course(&lines(
            &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 1000.0, 0.0), (0.0, 1000.0, 0.0), (0.0, 0.0, 0.0)],
            10.0,
        ));
        let mut t = Tracker::default();
        let (lat, lon) = fix(5.0, 0.0);
        c.follow(&mut t, lat, lon);
        assert!(t.along_m <= 10, "{t:?}");
        for k in 0..500 {
            let d = 8.0 * f64::from(k);
            let (x, y) = match d {
                d if d < 1000.0 => (d, 0.0),
                d if d < 2000.0 => (1000.0, d - 1000.0),
                d if d < 3000.0 => (3000.0 - d, 1000.0),
                d => (0.0, 4000.0 - d),
            };
            let (lat, lon) = fix(x, y);
            c.follow(&mut t, lat, lon);
        }
        assert!(t.along_m >= 3980, "the end, not the start: {t:?}");
    }

    /// A course of 5 laps of 8 km: placed on lap 5, then a gap in the fixes (a tunnel) puts the rider 800 m on, out
    /// of the window: the search finds lap 5 again, not one of the laps before (review 2: lap 4 won).
    #[test]
    fn lap_course_found_on_the_right_lap() {
        let sq = [(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0), (2000.0, 2000.0, 0.0), (0.0, 2000.0, 0.0)];
        let mut corners = Vec::new();
        for _ in 0..5 {
            corners.extend_from_slice(&sq);
        }
        corners.push(sq[0]);
        let c = course(&lines(&corners, 10.0));
        assert!((39_990..=40_010).contains(&c.len_m()), "{}", c.len_m());
        let mut t = Tracker::default();
        let mut x = 0.0;
        while x < 8000.0 * 4.0 + 200.0 {
            let d = x % 8000.0;
            let (px, py) = match d {
                d if d < 2000.0 => (d, 0.0),
                d if d < 4000.0 => (2000.0, d - 2000.0),
                d if d < 6000.0 => (6000.0 - d, 2000.0),
                d => (0.0, 8000.0 - d),
            };
            let (lat, lon) = fix(px, py);
            c.follow(&mut t, lat, lon);
            x += 8.0;
        }
        assert!(t.along_m.abs_diff(32_200) <= 15, "on lap 5: {t:?}");
        let (lat, lon) = fix(1000.0, 0.0);
        for _ in 0..SEARCH_S {
            c.follow(&mut t, lat, lon);
        }
        assert!(t.along_m.abs_diff(33_000) <= 10, "still on lap 5: {t:?}");
    }

    /// Off course after OFF_S seconds beyond OFF_M, said once; back within BACK_M; a climb said at its start; the
    /// window keeps the cost to a few dozen segments a second.
    #[test]
    fn off_course_and_climbs() {
        let c = course(&lines(&[(0.0, 0.0, 100.0), (1000.0, 0.0, 100.0), (2000.0, 0.0, 160.0)], 10.0));
        let mut t = Tracker::default();
        let mut said = Vec::new();
        let mut x = 0.0;
        let mut seconds = 0u32;
        while x < 2000.0 {
            let y = if (300.0..500.0).contains(&x) { 120.0 } else { 0.0 };
            let (lat, lon) = fix(x, y);
            if let Some(e) = c.follow(&mut t, lat, lon) {
                said.push((x as u32, e));
            }
            x += 8.0;
            seconds += 1;
        }
        let kinds: Vec<_> = said.iter().map(|(_, e)| core::mem::discriminant(e)).collect();
        assert_eq!(kinds.len(), 3, "{said:?}");
        assert_eq!(said[0], (336, Event::Off), "the 5th second beyond 50 m");
        assert_eq!(said[1], (504, Event::Back));
        assert!(matches!(said[2], (1000..=1016, Event::Climb(Climb { gain_m: 55..=60, .. }))), "{said:?}");
        assert!(t.examined / seconds < 120, "{} a second", t.examined / seconds);
    }

    /// A crafted file (review 2: a 30 KB GPX): points 1900 km apart, back and forth, make a course of millions of
    /// km; the climbs are still worked out, and the length is right (a bin per 50 m would be 86 million).
    #[test]
    fn climbs_bounded_on_a_crafted_course() {
        let mut c = Course::new();
        for k in 0..3000 {
            c.push(if k % 2 == 0 { 0 } else { 170_000_000 }, 0, Some(k % 7 * 1000));
        }
        c.finish();
        assert!(c.len_m() > 4_000_000_000, "{}", c.len_m());
        assert!(
            c.points().len() <= 3000 && c.climbs().len() <= 3000,
            "{} points, {} climbs",
            c.points().len(),
            c.climbs().len()
        );
    }

    /// A 400 km course is thinned to a point every 160 m, so its chords cut a spur by up to 80 m: riding the spur
    /// itself is not off course (the threshold grows with the step). Spurs of several depths: the kept points fall
    /// at different places on each.
    #[test]
    fn thinned_course_no_false_off_course() {
        let mut worst = 0;
        for depth in (60..=240).step_by(15) {
            let deep = f64::from(depth);
            let c = course(&lines(
                &[
                    (0.0, 0.0, 0.0),
                    (0.0, 200_000.0, 0.0),
                    (deep, 200_000.0, 0.0),
                    (0.0, 200_010.0, 0.0),
                    (0.0, 400_000.0, 0.0),
                ],
                5.0,
            ));
            assert_eq!(c.step_m, 160);
            let mut t = Tracker::default();
            let mut way: Vec<(f64, f64)> = (0..250).map(|k| (0.0, 198_000.0 + 8.0 * f64::from(k))).collect();
            way.extend((0..=depth / 8).map(|k| (8.0 * f64::from(k), 200_000.0)));
            way.extend((0..=depth / 8).rev().map(|k| (8.0 * f64::from(k), 200_010.0)));
            way.extend((0..250).map(|k| (0.0, 200_010.0 + 8.0 * f64::from(k))));
            for (x, y) in way {
                let (lat, lon) = fix(x, y);
                assert_ne!(c.follow(&mut t, lat, lon), Some(Event::Off), "{depth} m spur, at ({x}, {y}): {t:?}");
                worst = worst.max(t.off_m.unwrap());
            }
            assert!(t.along_m.abs_diff(202_000 + 2 * depth) <= 200, "{t:?}");
        }
        assert!(worst > OFF_M, "the chords cut the spurs: {worst} m at most");
    }

    /// Bad input: points off the globe or too far are left out; an empty or one-point course follows nothing.
    #[test]
    fn bad_points() {
        let mut c = Course::new();
        c.push(i32::MAX, 0, None);
        c.push(0, 0, None);
        c.push(0, 1_799_999_999, None); // half the world away
        c.finish();
        assert_eq!((c.points().len(), c.has_alt(), c.alt_at(0)), (1, false, None));
        assert_eq!(c.follow(&mut Tracker::default(), 0, 0), None);
        assert!(c.route((0, 0)).is_none());
        let mut c = Course::new();
        c.finish();
        assert_eq!((c.len_m(), c.climb_at(0), c.column(0, 100, 10, 3)), (0, None, None));
    }
}

#[cfg(test)]
mod climb_score_tests {
    use super::Climb;

    /// The score is length x grade = 100 x gain; MEDIUM+ from 16000 (2 km at 8 %), not at 15900.
    #[test]
    fn medium_plus_from_a_score_of_16000() {
        let c = |len_m, gain_m| Climb { start_m: 0, len_m, gain_m };
        assert_eq!(c(2000, 160).score(), 16_000);
        assert!(c(2000, 160).medium_plus() && !c(2000, 159).medium_plus());
        assert!(!c(1000, 80).medium_plus(), "1 km at 8 % is category 4 (8000)");
        assert_eq!(c(0, 0).score(), 0);
    }
}
