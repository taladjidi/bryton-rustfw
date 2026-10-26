//! Turn-by-turn cues of a course ("left in 200 m"), as Garmin's, Wahoo's and Hammerhead's bike computers give them:
//! the cues a file states, else turns found on the course's shape; the next one from the rider's place along the
//! course, said twice (a warning, then at the turn). Pure logic, no heap: at most [`CUES`] cues, kept with the course.
//!
//! - **From the file:** a FIT course's `course_point` messages (message 32: 2 position_lat, 3 position_long in
//!   semicircles, 4 distance, 5 type, 6 name; the `course_point` type's values in [`Turn::from_fit`]: Garmin's FIT
//!   SDK profile 21.218, github.com/garmin/fit-python-sdk `garmin_fit_sdk/profile.py`, read 2026-10-25); GPX points
//!   whose `<type>`, `<sym>` or `<name>` is a turn word ([`Turn::from_text`]). GPX 1.1 has no turn element: the
//!   words are the planners' conventions, BRouter's (github.com/abrensch/brouter `brouter-core/.../Formatter.java`,
//!   `getSymbolString` for its "gpsies" style: `<wpt>` with `<sym>` and `<type>` "Left", "TSLR", "TU";
//!   `getLocusSymbolString` for its Locus style: `<sym>` "left_slight", "u-turn" on the track point; roundabouts
//!   "RNDB2", "RNLB2" and "roundabout_e2" with their exit; read 2026-10-25) and RideWithGPS's (`<name>` "Left", "Right" on its cue points, as its users describe the export:
//!   inferred, no documentation found). Komoot's GPX has no cues. Each cue is placed on the course by its position
//!   ([`CUE_NEAR_M`] at most from it; else dropped, a point of interest off the way): a track point's own cue on
//!   that point's pass ([`Course::add_cue_here`]); with the file's distance of it (a FIT course point's, scaled as
//!   the points', [`super::Odometer`]), the pass nearest that distance (among the chunks nearest along it, as the
//!   rider's wide search); without (GPX waypoints), the first pass at or after the previous cue's, so on a loop or
//!   an out-and-back with cues in order each lands on its own leg (two waypoints at one place of an out-and-back
//!   with no cue between them both go on the way out: GPX says no more), [`PLACE_CHUNKS`] chunks looked into at
//!   most (a waypoint in the middle of a lap course is in every lap's box). Names are not kept (the banner has room for the arrow, the word and the
//!   distance).
//! - **Computed** when the file states no turn (a GPX track, stock's routes). Without a road map this firmware
//!   cannot know where the road forks (Garmin, Wahoo and Hammerhead compute a course's turns against their maps'
//!   road graphs: a turn is where the course leaves the road it is on); it reads the shape, by these rules (the
//!   thresholds are this firmware's choice, tuned on the shapes in the tests):
//!   - **Where it turns:** points whose neighbours within [`ARM_M`] (or a kept step) turn [`TURN_DEG`] or more,
//!     either way, all told, and those within [`MERGE_M`] of each other, are one group (a corner drawn with
//!     several points, a roundabout's way in, ring and way out). Its turn is what its points add up to from an
//!     arm before it to an arm after, not modulo 360; in it, the runs turning one way ([`RUN_DEG`] or more).
//!   - **A roundabout:** three runs, out one way, round the other, out the first way (whichever side the traffic
//!     keeps to), way in and way out within [`RING_M`], the middle one a ring: [`RING_DEG`] or more, half again
//!     as much as the way in and the way out, spread over [`RING_SPAN_M`] or more, no point of it turning more
//!     than [`RING_PEAK_DEG`] (a junction's corner turns at one point, and more): one cue, its exit
//!     ([`Turn::Roundabout`], "EXIT 2"), at the way in. The exit counts quarter turns round the ring from the way
//!     in: right is 1, straight on 2, left 3, back 4 (with the ring's direction). That assumes four roads a quarter
//!     turn apart (inferred: a map would count them); the arrow and "EXIT" still say it is a roundabout. Straight
//!     on is said too: at a roundabout the rider must pick an exit, as Garmin says "take the 2nd exit". The
//!     same three runs without a ring: less back in the middle than either way (a first exit whose ring the
//!     points barely show, a kink between two corners), the turn they make; back to the heading (under
//!     [`TURN_DEG`] all told: a chicane, an island, a cycle path's offset), nothing; else, a jog, a turn each.
//!     The limit: at 10 m a point, a ring of 12 m and a corner cut over two points look alike, so a turn with
//!     kinks either side whose corner falls between two points may still read as a roundabout.
//!   - **Else a turn,** one per run if it turns both ways (a junction then another): slight under 60 degrees,
//!     sharp from 110, a U-turn from 159 (BRouter's classes, `VoiceHint.java` `calcCommand`, where it knows no
//!     other way at the junction), at its sharpest point; under [`TURN_DEG`] none.
//!   - **A road that winds says nothing:** a turn spread over more than [`BEND_M`] per 90 degrees (or a kept step)
//!     is a bend of the road, not a junction's corner (a radius over about 20 m); a U-turn whose way out does not
//!     come back along its way in (within [`UTURN_M`]) is a hairpin of the road. The price: a junction taken in
//!     one wide sweep, or a roundabout's first exit whose ring the points barely show, says nothing either. The
//!     cues are only as good as the course's points: a course thinned past 25 m a point turns at its kept points
//!     (the cue up to half a step from the corner; a roundabout reads as a turn).
//! - **Thinned past [`CUES`]:** evenly, as the points are: when full, every other cue of the lowest rank there is
//!   goes (points of interest, then slight turns, then turns), and from then on one in two of that rank is kept;
//!   so a long course keeps cues from its start to its end. A rider past the last one is told so
//!   ([`Course::cues_done`]: the field's END).
//! - **Said ([`super::Event::Cue`]):** the next cue ahead of the rider (passed once [`PASSED_M`] behind) at
//!   [`WARN_M`] or [`WARN_S`] seconds ahead at the speed along the course, whichever is farther, then at [`NOW_M`]
//!   or [`NOW_S`] seconds. Nothing is said off course (off course said, or farther than [`super::OFF_M`]), nor before
//!   the course is joined: a rider elsewhere is not told a turn of a road they are not on. Placed back over a
//!   cue passed (a lap course found on an earlier lap), the cues are said again from there once that holds for
//!   [`BACK_FIXES`] fixes: a single fix astray says nothing twice.
//!
//! Every loop is bounded by [`CUES`], [`super::POINTS`] or an arm's few points.

use super::{Course, Event, OFF_M, POINTS, Tracker};

/// Cues kept at most.
pub const CUES: usize = 256;
/// Computed turns: the heading before and after a point over this far, m.
pub const ARM_M: u32 = 25;
/// Computed turns: a turn from this angle, degrees.
pub const TURN_DEG: i32 = 45;
/// A turn under this is slight, degrees (BRouter).
pub const SLIGHT_DEG: i32 = 60;
/// A turn from this is sharp, degrees (BRouter).
pub const SHARP_DEG: i32 = 110;
/// A turn from this is a U-turn, degrees (BRouter).
pub const UTURN_DEG: i32 = 159;
/// Computed turns: points turning within this of each other are one turn, m.
pub const MERGE_M: u32 = 40;
/// Computed turns: a run of points turning one way less than this is a wobble, not a turn, degrees.
pub const RUN_DEG: i32 = 15;
/// Computed turns: a roundabout's ring turns this much at least, degrees (a straight swerve's middle corner
/// can turn 35 to 60).
pub const RING_DEG: i32 = 45;
/// Computed turns: a roundabout's ring turns over this far at least (two points or more at the course's 10 m
/// spacing; a junction's corner turns at one), m.
pub const RING_SPAN_M: u32 = 10;
/// Computed turns: no point of a roundabout's ring turns more than this, degrees: a 10 m step round the
/// smallest ring tested (12 m radius) turns 48, 64 with the way in's share; a junction's corner turns more, cut
/// over two points as well (70 for a right turn with 20 degree kinks).
pub const RING_PEAK_DEG: i32 = 65;
/// Computed turns: a roundabout's way in and way out turn this close to each other at most (across a ring of a
/// radius up to 30 m, and 20 m of approach each side), m.
pub const RING_M: u32 = 110;
/// Computed turns: a point turning less than this is not where a run's turn is, degrees.
pub const BEND_MIN_DEG: i32 = 5;
/// Computed turns: a turn spread over more than this per 90 degrees is a bend of the road, not a junction, m.
pub const BEND_M: u32 = 30;
/// Computed turns: a U-turn whose way out comes back within this of the way in turns back on the road; farther,
/// it is a hairpin of the road, m.
pub const UTURN_M: u32 = 10;
/// Placing a file's cue without a distance: chunks near it looked into at most.
pub const PLACE_CHUNKS: u32 = 8;
/// Placed back before a cue passed: the cues are taken again from there after this many fixes there.
pub const BACK_FIXES: u8 = 2;
/// A file's cue farther than this from the course is not on it, m.
pub const CUE_NEAR_M: u32 = 50;
/// A cue this far behind the rider is passed, m.
pub const PASSED_M: u32 = 10;
/// The warning: this far ahead, m, or this many seconds at the rider's speed if farther.
pub const WARN_M: u32 = 200;
/// See [`WARN_M`].
pub const WARN_S: u32 = 20;
/// At the turn: this far ahead, m, or this many seconds if farther.
pub const NOW_M: u32 = 30;
/// See [`NOW_M`].
pub const NOW_S: u32 = 4;
/// Speeds along the course above this, m/s, are a jump (a new placing), not riding.
const MAX_SPEED: u32 = 30;
/// Placed back by more than this, m, the cues are taken again from there (a lap course found on another lap).
const JUMP_BACK_M: u32 = 100;
/// Not placed yet.
const UNPLACED: u32 = u32::MAX;

/// What a cue says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Turn left.
    Left,
    /// Turn right.
    Right,
    /// Slight left (or keep left at a fork).
    SlightLeft,
    /// Slight right.
    SlightRight,
    /// Sharp left.
    SharpLeft,
    /// Sharp right.
    SharpRight,
    /// Turn back.
    UTurn,
    /// Straight on.
    Straight,
    /// A summit (or a categorised climb's top).
    Summit,
    /// Water.
    Water,
    /// Food.
    Food,
    /// Danger.
    Danger,
    /// First aid.
    FirstAid,
    /// Any other point of the course.
    Point,
    /// A roundabout, and which exit to take (1: the first).
    Roundabout(u8),
}

impl Turn {
    /// FIT's `course_point` type (FIT SDK profile): its turns, forks as slight turns, a climb category's top as a
    /// summit, the points this firmware names, any other as a point.
    ///
    /// ```
    /// use bryton_rustfw::course::Turn;
    /// assert_eq!([6, 7, 19, 22, 23, 16, 1, 13, 3, 200].map(Turn::from_fit), [
    ///     Turn::Left, Turn::Right, Turn::SlightLeft, Turn::SharpRight, Turn::UTurn, Turn::SlightLeft, Turn::Summit,
    ///     Turn::Summit, Turn::Water, Turn::Point,
    /// ]);
    /// ```
    pub fn from_fit(t: u8) -> Turn {
        match t {
            1 | 10..=14 => Turn::Summit,
            3 => Turn::Water,
            4 => Turn::Food,
            5 => Turn::Danger,
            6 => Turn::Left,
            7 => Turn::Right,
            8 | 18 => Turn::Straight,
            9 => Turn::FirstAid,
            16 | 19 => Turn::SlightLeft,
            17 | 21 => Turn::SlightRight,
            20 => Turn::SharpLeft,
            22 => Turn::SharpRight,
            23 => Turn::UTurn,
            _ => Turn::Point,
        }
    }

    /// A GPX cue's word (any case; spaces, `_` and `-` ignored): the planners' turn words (see the module doc),
    /// FIT's type names, and the points this firmware names; None for anything else (a waypoint's own name).
    ///
    /// ```
    /// use bryton_rustfw::course::Turn;
    /// assert_eq!(Turn::from_text(b"Slight Left"), Some(Turn::SlightLeft));
    /// assert_eq!(Turn::from_text(b"TSHR"), Some(Turn::SharpRight));
    /// assert_eq!(Turn::from_text(b"u-turn_left"), Some(Turn::UTurn));
    /// assert_eq!(Turn::from_text(b"RNDB2"), Some(Turn::Roundabout(2)));
    /// assert_eq!(Turn::from_text(b"roundabout_e3"), Some(Turn::Roundabout(3)));
    /// assert_eq!(Turn::from_text(b"Cafe du Port"), None);
    /// ```
    pub fn from_text(s: &[u8]) -> Option<Turn> {
        let mut w = [0u8; 16];
        let mut n = 0;
        for &b in s.iter().filter(|b| !matches!(b, b' ' | b'_' | b'-' | b'\t' | b'\r' | b'\n')) {
            *w.get_mut(n)? = b.to_ascii_lowercase();
            n += 1;
        }
        let w = w.get(..n)?;
        // BRouter's roundabouts: "RNDB3" (counter-clockwise) and "RNLB3" (clockwise) in its gpsies style,
        // "roundabout_e3" in its Locus style (Formatter.java `getSymbolString`, `getLocusSymbolString`)
        for pre in [&b"rndb"[..], b"rnlb", b"roundaboute"] {
            if let Some(&[d @ b'1'..=b'9']) = w.strip_prefix(pre) {
                return Some(Turn::Roundabout(d - b'0'));
            }
        }
        Some(match w {
            b"left" | b"tl" | b"turnleft" => Turn::Left,
            b"right" | b"tr" | b"turnright" => Turn::Right,
            b"slightleft" | b"leftslight" | b"tsll" | b"kl" | b"keepleft" | b"stayleft" | b"bearleft" | b"leftfork" => {
                Turn::SlightLeft
            }
            b"slightright" | b"rightslight" | b"tslr" | b"kr" | b"keepright" | b"stayright" | b"bearright"
            | b"rightfork" => Turn::SlightRight,
            b"sharpleft" | b"leftsharp" | b"tshl" => Turn::SharpLeft,
            b"sharpright" | b"rightsharp" | b"tshr" => Turn::SharpRight,
            b"uturn" | b"tu" | b"tlu" | b"tru" | b"uturnleft" | b"uturnright" => Turn::UTurn,
            b"straight" | b"continue" => Turn::Straight,
            b"summit" => Turn::Summit,
            b"water" => Turn::Water,
            b"food" => Turn::Food,
            b"danger" => Turn::Danger,
            b"firstaid" => Turn::FirstAid,
            _ => return None,
        })
    }

    /// A turn (not a point of interest).
    pub fn is_turn(self) -> bool {
        self.rank() > 0
    }

    /// What is kept first when the cues are too many: turns, then slight turns and straight on, then points.
    fn rank(self) -> u8 {
        match self {
            Turn::Left | Turn::Right | Turn::SharpLeft | Turn::SharpRight | Turn::UTurn | Turn::Roundabout(_) => 2,
            Turn::SlightLeft | Turn::SlightRight | Turn::Straight => 1,
            _ => 0,
        }
    }

    /// The turn of a heading change of `deg` degrees (positive: left), classed as BRouter does (module doc); None
    /// under [`TURN_DEG`].
    ///
    /// ```
    /// use bryton_rustfw::course::Turn;
    /// assert_eq!([30, 50, 90, -90, -130, 175, -180].map(Turn::of_angle), [
    ///     None, Some(Turn::SlightLeft), Some(Turn::Left), Some(Turn::Right), Some(Turn::SharpRight),
    ///     Some(Turn::UTurn), Some(Turn::UTurn),
    /// ]);
    /// ```
    pub fn of_angle(deg: i32) -> Option<Turn> {
        let left = deg > 0;
        Some(match deg.unsigned_abs() as i32 {
            a if a < TURN_DEG => return None,
            a if a < SLIGHT_DEG => [Turn::SlightRight, Turn::SlightLeft][usize::from(left)],
            a if a < SHARP_DEG => [Turn::Right, Turn::Left][usize::from(left)],
            a if a < UTURN_DEG => [Turn::SharpRight, Turn::SharpLeft][usize::from(left)],
            _ => Turn::UTurn,
        })
    }

    /// Its word in the banner.
    pub fn word(self) -> &'static str {
        match self {
            Turn::Left => "LEFT",
            Turn::Right => "RIGHT",
            Turn::SlightLeft => "SLIGHT LEFT",
            Turn::SlightRight => "SLIGHT RIGHT",
            Turn::SharpLeft => "SHARP LEFT",
            Turn::SharpRight => "SHARP RIGHT",
            Turn::UTurn => "U-TURN",
            Turn::Straight => "STRAIGHT",
            Turn::Summit => "SUMMIT",
            Turn::Water => "WATER",
            Turn::Food => "FOOD",
            Turn::Danger => "DANGER",
            Turn::FirstAid => "FIRST AID",
            Turn::Point => "POINT",
            Turn::Roundabout(n) => {
                const EXITS: [&str; 9] =
                    ["EXIT 1", "EXIT 2", "EXIT 3", "EXIT 4", "EXIT 5", "EXIT 6", "EXIT 7", "EXIT 8", "EXIT 9"];
                EXITS.get(usize::from(n.clamp(1, 9)) - 1).copied().unwrap_or("EXIT")
            }
        }
    }

    /// Its arrow: a character of the pixel font (tools/pixel_font.py, codes 1 to 10), so it is drawn and redrawn as
    /// text.
    pub fn glyph(self) -> &'static str {
        match self {
            Turn::Left => "\x01",
            Turn::Right => "\x02",
            Turn::SlightLeft => "\x03",
            Turn::SlightRight => "\x04",
            Turn::SharpLeft => "\x05",
            Turn::SharpRight => "\x06",
            Turn::UTurn => "\x07",
            Turn::Straight => "\x08",
            Turn::Roundabout(_) => "\x0a",
            _ => "\x09",
        }
    }
}

/// A cue: where it is along the course, and what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// Distance from the course's start, m.
    pub at_m: u32,
    /// What it says.
    pub turn: Turn,
    /// From a file, until placed: its position, degrees x 1e-7.
    pos: (i32, i32),
    /// Until placed: `at_m` is the course's own distance of the point it sits on (a GPX track point's cue), not
    /// the file's.
    run: bool,
}

impl Cue {
    // all zeros (Left is 0): the course memory is built on the stack, and zeros cost no copy there (stack_check.py)
    const NONE: Cue = Cue { at_m: 0, turn: Turn::Left, pos: (0, 0), run: false };
}

/// The signed angle from direction `a` to direction `b`, degrees (positive: counter-clockwise, a left turn on the
/// east-north plane), -180 to 180; 0 if either is nought. Integer atan2: the arctangent of the smaller over the
/// larger component by `45 x - x (x - 1) (14.02 + 3.79 x)` degrees (Rajan, Wang, Inkol and Joyal 2006, "Efficient
/// approximations for the arctangent function", IEEE Signal Processing Magazine: within 0.1 degree).
///
/// ```
/// use bryton_rustfw::course::angle_deg;
/// assert_eq!(angle_deg((10, 0), (0, 10)), 90);
/// assert_eq!(angle_deg((10, 0), (-10, -10)), -135);
/// assert_eq!(angle_deg((10, 0), (-10, 0)), 180);
/// assert_eq!(angle_deg((10, 0), (10, 3)), 17);
/// ```
pub fn angle_deg(a: (i64, i64), b: (i64, i64)) -> i32 {
    let dot = a.0.saturating_mul(b.0).saturating_add(a.1.saturating_mul(b.1));
    let cross = a.0.saturating_mul(b.1).saturating_sub(a.1.saturating_mul(b.0));
    let (x, y) = (dot.unsigned_abs(), cross.unsigned_abs());
    if x == 0 && y == 0 {
        return 0;
    }
    // the arctangent of r (0 to 1, x 65536), degrees x 65536
    // in i64: r (r - 1) is under 2^32, the bracket under 2^27 (no i128: its routines cost the image a few KB)
    let atan = |r: u64| -> u64 {
        let r = r as i64;
        let v = 45 * r - r * (r - 65_536) / 65_536 * (14_02 * 65_536 + 379 * r) / (100 * 65_536);
        v.max(0) as u64
    };
    // p <= q: both scaled down to 46 bits first, so p x 65536 fits
    let ratio = |p: u64, q: u64| {
        let sh = (64 - q.leading_zeros()).saturating_sub(46);
        ((p >> sh) * 65_536 / (q >> sh).max(1)).min(65_536)
    };
    let t = if y <= x { atan(ratio(y, x)) } else { 90 * 65_536 - atan(ratio(x, y)) };
    let t = if dot < 0 { 180 * 65_536 - t } else { t };
    let deg = ((t + 32_768) / 65_536) as i32;
    if cross < 0 { -deg } else { deg }
}

/// The cues of a course: those of its file, else those of its shape.
pub(super) struct Cues {
    c: [Cue; CUES],
    n: usize,
    /// Per rank ([`Turn::rank`]): the cues of that rank seen so far, and its thinning: one in 2^`shift` kept.
    seen: [u32; 3],
    shift: [u8; 3],
}

impl Cues {
    pub(super) const fn new() -> Self {
        Cues { c: [Cue::NONE; CUES], n: 0, seen: [0; 3], shift: [0; 3] }
    }

    fn all(&self) -> &[Cue] {
        self.c.get(..self.n).unwrap_or(&[])
    }

    /// Keeps `cue`, unless its rank is thinned to one in 2^k and it is not that one. When full, every other cue of
    /// the lowest rank (at most `cue`'s) with two or more goes and that rank's thinning doubles, as the points'
    /// ([`Course::finish`]): the cues kept stay spread along the whole course, the turns before the points.
    fn keep(&mut self, cue: Cue) {
        let r = usize::from(cue.turn.rank()).min(2);
        let Some(seen) = self.seen.get_mut(r) else { return };
        let k = *seen;
        *seen = seen.saturating_add(1);
        // bounded: each pass frees a slot or returns
        for _ in 0..=CUES {
            let shift = self.shift.get(r).copied().unwrap_or(31);
            if k & ((1u32 << shift.min(31)) - 1) != 0 {
                return; // thinned, as those of its rank kept before it
            }
            if let Some(slot) = self.c.get_mut(self.n) {
                *slot = cue;
                self.n += 1;
                return;
            }
            let count = |q: usize| self.all().iter().filter(|c| usize::from(c.turn.rank()) == q).count();
            let Some(q) = (0..=r).find(|&q| count(q) >= 2) else { return };
            self.halve(q);
        }
    }

    /// Every other cue of rank `q` dropped (the first kept), the others kept in order.
    fn halve(&mut self, q: usize) {
        let (mut w, mut j) = (0, 0usize);
        for i in 0..self.n.min(CUES) {
            let c = self.c[i];
            if usize::from(c.turn.rank()) == q {
                j += 1;
                if j % 2 == 0 {
                    continue;
                }
            }
            self.c[w] = c;
            w += 1;
        }
        self.n = w;
        if let Some(s) = self.shift.get_mut(q) {
            *s = (*s + 1).min(31);
        }
    }
}

impl Course {
    /// A cue of the file at a position (degrees x 1e-7), with the file's distance of it if it states one (m, as
    /// its points'), placed on the course when it is read whole.
    pub fn add_cue(&mut self, lat: i32, lon: i32, turn: Turn, dist_m: Option<u32>) {
        if lat.unsigned_abs() <= 900_000_000 && lon.unsigned_abs() <= 1_800_000_000 {
            let at_m = dist_m.map_or(UNPLACED, |d| d.min(UNPLACED - 1));
            self.cues.keep(Cue { at_m, turn, pos: (lat, lon), run: false });
        }
    }

    /// A cue of the file on the point it gave last (a GPX track point's own turn word): placed on that very pass
    /// of the course, not on another one through the same place.
    pub fn add_cue_here(&mut self, turn: Turn) {
        if let Some(pos) = self.odo.last {
            self.cues.keep(Cue { at_m: self.odo.run_m().min(UNPLACED - 1), turn, pos, run: true });
        }
    }

    /// The cues, in order along the course.
    pub fn cues(&self) -> &[Cue] {
        self.cues.all()
    }

    /// The file's cues placed on the course (those off it dropped), the shape's turns found if the file states
    /// none, all in order (at [`Course::finish`]).
    pub(super) fn finish_cues(&mut self) {
        let (mut from, mut n) = (0usize, 0usize);
        for i in 0..self.cues.n.min(CUES) {
            let c = self.cues.c[i];
            let p = self.origin.map(|o| super::project(o, c.pos.0, c.pos.1));
            let hint = match (c.at_m, c.run) {
                (UNPLACED, _) => None,
                (d, true) => Some(self.run_along(d)),
                (d, false) => Some(self.along_of(d)),
            };
            let hit = p.and_then(|p| hint.and_then(|h| self.place_near(p, h)).or_else(|| self.place(p, from, &mut 0)));
            if let Some((seg, at_m)) = hit {
                self.cues.c[n] = Cue { at_m, run: false, ..c };
                (n, from) = (n + 1, seg);
            }
        }
        self.cues.n = n;
        if !self.cues().iter().any(|c| c.turn.is_turn()) {
            self.find_turns();
        }
        let n = self.cues.n.min(CUES);
        // in order: an insertion sort (the file's cues come nearly in order; the shape's turns are, after them)
        for i in 1..n {
            let mut j = i;
            while j > 0 && self.cues.c[j - 1].at_m > self.cues.c[j].at_m {
                self.cues.c.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    /// The course's distance of a file's distance `d` (m): from the file's first point, scaled as the points were
    /// ([`Course::finish`]).
    fn along_of(&self, d: u32) -> u32 {
        let (first, last) = self.odo.stated.unwrap_or((0, 0));
        let d = u64::from(d.saturating_sub(first));
        match u64::from(last.saturating_sub(first)) {
            0 => d.min(u64::from(u32::MAX)) as u32,
            stated => (d * u64::from(self.len_m()) / stated).min(u64::from(u32::MAX)) as u32,
        }
    }

    /// The course's distance of a point's summed distance `d` (m), scaled as the points were ([`Course::finish`]).
    fn run_along(&self, d: u32) -> u32 {
        let (sum, len) = (self.odo.run_m(), self.odo.len_m().unwrap_or(0));
        match sum {
            0 => d,
            _ => (u64::from(d) * u64::from(len) / u64::from(sum)).min(u64::from(u32::MAX)) as u32,
        }
    }

    /// Where on the course a file's cue at `p` is, by its distance `along` (m): of the passes within
    /// [`CUE_NEAR_M`], the one nearest along (an out-and-back's two legs, told apart). Looks into the
    /// [`super::SEARCH_CHUNKS`] chunks nearest along, as the rider's wide search does: a lap course's other laps
    /// cost nothing.
    fn place_near(&self, p: (i32, i32), along: u32) -> Option<(usize, u32)> {
        let h = self.search(p, along, &mut 0).filter(|h| h.off_m <= CUE_NEAR_M)?;
        Some((h.seg, h.along_m))
    }

    /// Where on the course a file's cue at `p` (on the plane) is: the first pass within [`CUE_NEAR_M`] from
    /// segment `from` on, else from the start; its segment and distance along. Chunks whose boxes are farther are
    /// not looked into, and at most [`PLACE_CHUNKS`] are: a cue near no pass (a point of interest in the middle of
    /// a lap course, inside every lap's box) is dropped at that cost. `n` counts the segments looked at.
    fn place(&self, p: (i32, i32), from: usize, n: &mut u32) -> Option<(usize, u32)> {
        let segs = self.len.saturating_sub(1);
        let (near, chunks) = (u64::from(CUE_NEAR_M), self.n_boxes.min(super::CHUNKS));
        let first = (from / super::CHUNK).min(chunks);
        let mut budget = PLACE_CHUNKS;
        for (lo, hi, start) in [(first, chunks, from), (0, (first + 1).min(chunks), 0)] {
            for c in lo..hi {
                if super::box_m(&self.boxes[c], p) > near {
                    continue;
                }
                if budget == 0 {
                    return None;
                }
                budget -= 1;
                let start = (c * super::CHUNK).max(start).min(segs);
                let along = self.dist.get(start).copied().unwrap_or(0);
                let hit = |end: usize, n: &mut u32| {
                    let r = start..(end * super::CHUNK).min(segs);
                    self.nearest(p, &[r], along, n).filter(|h| h.off_m <= CUE_NEAR_M)
                };
                if hit(c + 1, n).is_some() {
                    // this chunk and the next: a turn on a chunk's last point has its way out in the next one
                    return hit(c + 2, n).map(|h| (h.seg, h.along_m));
                }
            }
        }
        None
    }

    /// Point `i`'s neighbour at least `arm` m back (`dir` -1) or ahead (1) along the course, not across a gap.
    fn arm(&self, i: usize, dir: isize, arm: u32) -> Option<usize> {
        let n = self.len.min(POINTS);
        let at = |k: usize| self.dist.get(k).copied();
        let mut j = i;
        for _ in 0..64 {
            let k = j.checked_add_signed(dir).filter(|&k| k < n)?;
            if self.gap_at(if dir < 0 { j } else { k }) {
                return None;
            }
            j = k;
            if at(i)?.abs_diff(at(j)?) >= arm {
                return Some(j);
            }
        }
        None
    }

    /// The heading change at point `i` alone, from the segment into it to the segment out of it, degrees; 0 at
    /// either end or a gap.
    fn bend(&self, i: usize) -> i32 {
        let p = |k: usize| self.points().get(k).map(|&(x, y)| (i64::from(x), i64::from(y)));
        let Some(h) = i.checked_sub(1) else { return 0 };
        match (p(h), p(i), p(i + 1)) {
            (Some(a), Some(b), Some(c)) if !self.gap_at(i) && !self.gap_at(i + 1) => {
                angle_deg((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1))
            }
            _ => 0,
        }
    }

    /// Point `i` turns: the points within `arm` of it turn [`TURN_DEG`] or more, either way, all told (a
    /// roundabout's way in turns one way and its ring the other: their sum would cancel), each point turning less
    /// than [`BEND_MIN_DEG`] left out (a road's slow curve, a recorded track's wobble).
    fn turning(&self, i: usize, arm: u32) -> bool {
        let at = |k: usize| self.dist.get(..self.len).and_then(|d| d.get(k)).copied();
        let Some(d) = at(i) else { return false };
        let mut all = 0i32;
        let mut add = |k: usize| {
            let v = self.bend(k).abs();
            if v >= BEND_MIN_DEG {
                all = all.saturating_add(v);
            }
        };
        add(i);
        for dir in [-1isize, 1] {
            let mut j = i;
            for _ in 0..64 {
                let Some(k) = j.checked_add_signed(dir).filter(|&k| at(k).is_some_and(|dk| dk.abs_diff(d) <= arm))
                else {
                    break;
                };
                (j, ()) = (k, add(k));
            }
        }
        all >= TURN_DEG
    }

    /// The turns of the course's shape (module doc), as cues.
    fn find_turns(&mut self) {
        let n = self.len.min(POINTS);
        let arm = ARM_M.max(self.step_m);
        let merge = MERGE_M.max(self.step_m);
        // the points turning, either way, within `merge` of each other: their first and last
        // and the points looked into so far (a group's runs start after the last one's)
        let (mut group, mut done): (Option<(usize, usize)>, usize) = (None, 0);
        for i in 1..n.saturating_sub(1) {
            let d = self.dist.get(i).copied().unwrap_or(0);
            let near = |last: usize| d.saturating_sub(self.dist.get(last).copied().unwrap_or(0)) <= merge;
            // in a group, a point turning a run's worth on its own goes with it (a roundabout's way out, cut short
            // by the thinning)
            let turning = self.turning(i, arm) || group.is_some_and(|g| near(g.1) && self.bend(i).abs() >= RUN_DEG);
            group = match (group, turning) {
                (Some((first, last)), true) if near(last) => Some((first, i)),
                (Some(g), false) if near(g.1) => Some(g),
                (g, turning) => {
                    if let Some(g) = g {
                        done = self.group_turns(g.0, g.1, arm, done);
                    }
                    turning.then_some((i, i))
                }
            };
        }
        if let Some(g) = group {
            self.group_turns(g.0, g.1, arm, done);
        }
    }

    /// The cues of a group of turning points, `first` to `last` (module doc): a roundabout, a turn, or a turn per
    /// run one way, the bends and hairpins of the road left out.
    /// The points from an arm before `first` to an arm after `last` are looked into, from `from` on; the last one
    /// looked into is returned.
    fn group_turns(&mut self, first: usize, last: usize, arm: u32, from: usize) -> usize {
        // at the course's ends or a gap, as far as it goes
        let (b, f) = (self.arm(first, -1, arm).unwrap_or(first).max(from), self.arm(last, 1, arm).unwrap_or(last));
        // the runs one way (the first three kept), and the whole turn: what the points add up to, not modulo 360
        let (mut three, mut count) = ([Run::NONE; 3], 0usize);
        let mut runs = Runs::new(b, f);
        while let Some(r) = runs.next(self) {
            if let Some(slot) = three.get_mut(count) {
                *slot = r;
            }
            count += 1;
        }
        let total = runs.all;
        let [r0, r1, r2] = three;
        let apart = |a: usize, b: usize| match (self.xy.get(a), self.xy.get(b)) {
            (Some(p), Some(q)) => (i64::from(p.0) - i64::from(q.0)).pow(2) + (i64::from(p.1) - i64::from(q.1)).pow(2),
            _ => i64::MAX,
        };
        // one way, the other, the first way again, the outer turns close: a roundabout, or a junction's corners
        let three_ways = count == 3
            && (r0.sum > 0) == (r2.sum > 0)
            && (r0.sum > 0) != (r1.sum > 0)
            && apart(r0.peak, r2.peak) <= i64::from(RING_M).pow(2);
        let (ring, outer) = (r1.sum.abs(), r0.sum.abs().max(r2.sum.abs()));
        // a ring: its turn spread over more than one point, none turning as much as a corner, half again as
        // much as the way in and the way out (2026-10-25 review: corners of a jog, a kinked turn, a swerve)
        let is_ring = r1.span.1.saturating_sub(r1.span.0) >= RING_SPAN_M
            && r1.peak_deg.abs() <= RING_PEAK_DEG
            && ring >= RING_DEG
            && 2 * ring >= 3 * outer;
        if three_ways && is_ring {
            // a roundabout: in one way, round the other, out the first way; the exit by how far round it went
            let round = if r1.sum > 0 { total } else { -total };
            let exit = (180 + round + 45).div_euclid(90).clamp(1, 4) as u8;
            let at_m = self.dist.get(r0.peak).copied().unwrap_or(0);
            self.cues.keep(Cue { at_m, turn: Turn::Roundabout(exit), pos: (0, 0), run: false });
        } else if three_ways && ring < r0.sum.abs().min(r2.sum.abs()) {
            // less back between two turns one way (a roundabout's first exit, its ring barely shown; a kink
            // between two corners): the turn they make, at the first
            self.corner(total, r0, b, f);
        } else if three_ways && total.abs() < TURN_DEG {
            // back to its heading (a chicane, an island, a cycle path's offset): a swerve, nothing to say
        } else if count == 1 {
            self.corner(total, r0, b, f);
        } else {
            // turns one way then the other (a junction then another, a road winding): each run on its own
            let mut runs = Runs::new(b, f);
            while let Some(r) = runs.next(self) {
                self.corner(r.sum, r, b, f);
            }
        }
        f.saturating_add(1)
    }

    /// The cue of a run turning `deg` degrees (unwrapped) one way, between points `b` and `f`: none for a bend of
    /// the road (slower than 90 degrees per [`BEND_M`], or per step of a thinned course) or a hairpin (a U-turn whose way out does not come back
    /// along the way in, within [`UTURN_M`]).
    fn corner(&mut self, deg: i32, r: Run, b: usize, f: usize) {
        let Some(turn) = Turn::of_angle((deg + 180).rem_euclid(360) - 180) else { return };
        let width = u64::from(r.span.1.saturating_sub(r.span.0));
        // a corner between two points kept is cut over a step: spread over that much at least
        if width * 90 > u64::from(deg.unsigned_abs()) * u64::from(BEND_M.max(self.step_m)) {
            return;
        }
        if turn == Turn::UTurn {
            let lim = i64::from(UTURN_M) * i64::from(UTURN_M);
            let back = self
                .xy
                .get(f)
                .is_some_and(|&p| (b..r.peak).filter_map(|i| self.on_segment(i, p)).any(|(d2, _)| d2 <= lim));
            if !back {
                return;
            }
        }
        let at_m = self.dist.get(r.peak).copied().unwrap_or(0);
        self.cues.keep(Cue { at_m, turn, pos: (0, 0), run: false });
    }
}

/// A run of points turning one way: its turn (degrees, positive left), its sharpest point and that point's turn,
/// and where its points turning [`BEND_MIN_DEG`] or more start and end (m; `(u32::MAX, 0)` without any).
#[derive(Clone, Copy, Debug)]
struct Run {
    sum: i32,
    peak: usize,
    peak_deg: i32,
    span: (u32, u32),
}

impl Run {
    const NONE: Run = Run { sum: 0, peak: 0, peak_deg: 0, span: (u32::MAX, 0) };

    /// Both runs as one.
    fn join(self, o: Run) -> Run {
        let (peak, peak_deg) =
            if o.peak_deg.abs() > self.peak_deg.abs() { (o.peak, o.peak_deg) } else { (self.peak, self.peak_deg) };
        Run {
            sum: self.sum.saturating_add(o.sum),
            peak,
            peak_deg,
            span: (self.span.0.min(o.span.0), self.span.1.max(o.span.1)),
        }
    }
}

/// The runs of the points from `from` to `to` turning one way, in order: those turning less than [`RUN_DEG`] left
/// out (a wobble), and the runs either side of one joined.
struct Runs {
    i: usize,
    f: usize,
    /// The turn of all the points so far, degrees.
    all: i32,
    raw: Option<Run>,
    held: Option<Run>,
}

impl Runs {
    fn new(from: usize, to: usize) -> Self {
        Runs { i: from, f: to.saturating_add(1), all: 0, raw: None, held: None }
    }

    /// The next run; bounded by the points left.
    fn next(&mut self, c: &Course) -> Option<Run> {
        while self.i < self.f.min(c.len).min(POINTS) {
            let i = self.i;
            self.i += 1;
            let v = c.bend(i);
            self.all = self.all.saturating_add(v);
            if v == 0 {
                continue;
            }
            let at = c.dist.get(i).copied().unwrap_or(0);
            let span = if v.abs() >= BEND_MIN_DEG { (at, at) } else { (u32::MAX, 0) };
            let one = Run { sum: v, peak: i, peak_deg: v, span };
            match self.raw {
                Some(r) if (r.sum > 0) == (v > 0) => self.raw = Some(r.join(one)),
                _ => {
                    let ended = self.raw.replace(one);
                    if let Some(out) = self.end(ended) {
                        return Some(out);
                    }
                }
            }
        }
        let ended = self.raw.take();
        self.end(ended).or_else(|| self.held.take())
    }

    /// A run has ended: joined to the one held if it turns the same way, else the one held is out.
    fn end(&mut self, r: Option<Run>) -> Option<Run> {
        let r = r.filter(|r| r.sum.abs() >= RUN_DEG)?;
        match self.held.take() {
            Some(h) if (h.sum > 0) == (r.sum > 0) => {
                self.held = Some(h.join(r));
                None
            }
            h => {
                self.held = Some(r);
                h
            }
        }
    }
}

impl Course {
    /// The next cue ahead of the rider and how far it is, m; None off course or without one.
    pub fn next_cue(&self, t: &Tracker) -> Option<(Cue, u32)> {
        if t.warned || t.off_m.is_none_or(|m| m > OFF_M) {
            return None;
        }
        let c = *self.cues().get(t.cue)?;
        Some((c, c.at_m.saturating_sub(t.along_m)))
    }

    /// On the course and past its last cue: no turn left before the end.
    pub fn cues_done(&self, t: &Tracker) -> bool {
        !t.warned && t.off_m.is_some_and(|m| m <= OFF_M) && t.cue >= self.cues().len()
    }

    /// Once a second after the rider was placed (`was`: the distance along before): the next cue moved on, and
    /// the warning or the turn said if due. Placed back before a cue passed ([`JUMP_BACK_M`] behind it), the cues
    /// are taken again from there only once that holds for [`BACK_FIXES`] fixes (a lap course found on an earlier
    /// lap): a single fix astray says nothing twice.
    pub(super) fn cue_step(&self, t: &mut Tracker, was: Option<u32>) -> Option<Event> {
        let along = t.along_m;
        let moved = was.map_or(0, |w| along.saturating_sub(w));
        if moved <= MAX_SPEED {
            t.speed = moved;
        }
        let k = self.cues().partition_point(|c| c.at_m.saturating_add(PASSED_M) <= along);
        let passed = t.cue.checked_sub(1).and_then(|i| self.cues().get(i)).map_or(0, |c| c.at_m + PASSED_M);
        if k > t.cue {
            (t.cue, t.cue_said, t.back_s) = (k, 0, 0);
        } else if k < t.cue && along.saturating_add(JUMP_BACK_M) < passed {
            t.back_s = t.back_s.saturating_add(1);
            if t.back_s >= BACK_FIXES {
                (t.cue, t.cue_said, t.back_s) = (k, 0, 0);
            }
        } else {
            t.back_s = 0;
        }
        if !t.joined {
            return None;
        }
        let (c, d) = self.next_cue(t)?;
        let (warn, now) = (WARN_M.max(WARN_S * t.speed), NOW_M.max(NOW_S * t.speed));
        let say = if d <= now { 2 } else { u8::from(d <= warn) };
        (say > t.cue_said).then(|| {
            t.cue_said = say;
            Event::Cue(c, if say == 2 { 0 } else { d })
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::course::tests::{course, lines};
    use std::vec::Vec;

    /// Degrees x 1e-7 per metre, as course.rs's tests.
    const NORTH: f64 = 1.0 / 0.011_132;

    fn at(x: f64, y: f64) -> (i32, i32) {
        ((y * NORTH).round() as i32, (x * NORTH).round() as i32)
    }

    fn turns(c: &Course) -> Vec<(u32, Turn)> {
        c.cues().iter().map(|c| (c.at_m, c.turn)).collect()
    }

    /// Each found turn within `tol` m of where it is wanted, of the kind wanted.
    fn check(got: &[(u32, Turn)], want: &[(u32, Turn)], tol: u32) {
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(g.1 == w.1 && g.0.abs_diff(w.0) <= tol, "{g:?} for {w:?}: {got:?}");
        }
    }

    /// The zigzag of tools/fat_image.py's `Zigzag.gpx` and `Turns.fit` (sim.rs rides it): a left and a right corner,
    /// 1.5 km of winding road (15 m either side, 300 m waves), a sharp right, a slight left, a right, a hairpin.
    pub(crate) fn zigzag() -> Vec<(f64, f64, f64)> {
        let mut pts = lines(&[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 600.0, 0.0), (1800.0, 600.0, 0.0)], 10.0);
        for k in 1..=150 {
            let x = 1800.0 + 10.0 * f64::from(k);
            pts.push((x, 600.0 + 15.0 * (core::f64::consts::TAU * (x - 1800.0) / 300.0).sin(), 0.0));
        }
        let corners = [(3300.0, 600.0, 0.0), (2800.0, 100.0, 0.0), (2800.0, -500.0, 0.0), (2350.0, -700.0, 0.0)];
        pts.extend_from_slice(&lines(&corners, 10.0)[1..]);
        let hairpin = [(2350.0, -700.0, 0.0), (1900.0, -900.0, 0.0), (1900.0, -906.0, 0.0), (2350.0, -706.0, 0.0)];
        pts.extend_from_slice(&lines(&hairpin, 3.0)[1..]);
        pts
    }

    /// The zigzag's turns, along the course: where (m) and which.
    pub(crate) fn zigzag_turns(c: &Course) -> [(u32, Turn); 6] {
        let wind = c.len_m() - (1000 + 600 + 800 + 707 + 600 + 492 + 492 + 6 + 492); // the winding stretch's length
        let s = 2400 + wind;
        [
            (1000, Turn::Left),
            (1600, Turn::Right),
            (s, Turn::SharpRight),
            (s + 707, Turn::SlightLeft),
            (s + 1307, Turn::Right),
            (s + 1307 + 984, Turn::UTurn),
        ]
    }

    /// Turns on a course's shape: the zigzag's corners, each where it is and of its kind; the winding stretch none.
    #[test]
    fn turns_of_the_shape() {
        let c = course(&zigzag());
        check(&turns(&c), &zigzag_turns(&c), 12);
        // a course of 2 points, or of none: no turn, no panic
        assert!(course(&[(0.0, 0.0, 0.0), (100.0, 0.0, 0.0)]).cues().is_empty());
        assert!(course(&[]).cues().is_empty());
    }

    /// A course thinned to a point every 80 m (330 km) still finds its corners, a step from them at most.
    #[test]
    fn turns_on_a_thinned_course() {
        let c = course(&lines(
            &[(0.0, 0.0, 0.0), (0.0, 150_000.0, 0.0), (5000.0, 150_000.0, 0.0), (5000.0, 0.0, 0.0)],
            10.0,
        ));
        assert_eq!(c.step_m, 80);
        check(&turns(&c), &[(150_000, Turn::Right), (155_000, Turn::Right)], 80);
    }

    /// A file's cues are placed by their position, each after the one before: on an out-and-back the same place
    /// is two cues, one on each leg. A cue 80 m off is dropped. With turns from the file, none are computed (the
    /// corner at 1 km has no cue).
    #[test]
    fn file_cues_on_their_legs() {
        let mut c = Course::new();
        for (x, y, t) in [(500.0, 3.0, Turn::Water), (2000.0, 2.0, Turn::UTurn), (500.0, 7.0, Turn::Food)] {
            let (lat, lon) = at(x, y);
            c.add_cue(lat, lon, t, None);
        }
        let (lat, lon) = at(1500.0, 80.0);
        c.add_cue(lat, lon, Turn::Left, None);
        let (lat, lon) = at(5.0, 400.0);
        c.add_cue(lat, lon, Turn::SharpLeft, None);
        for (x, y, z) in
            lines(&[(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0), (2000.0, 5.0, 0.0), (0.0, 5.0, 0.0), (0.0, 500.0, 0.0)], 10.0)
        {
            c.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some((z * 10.0) as i32));
        }
        c.finish();
        check(&turns(&c), &[(500, Turn::Water), (2002, Turn::UTurn), (3505, Turn::Food), (4400, Turn::SharpLeft)], 3);
    }

    /// Past CUES, the less important go first, and those kept stay spread along the course: 300 points of
    /// interest then 20 turns keep the 20 turns, and every other point (one each 20 m, from the first to the last).
    #[test]
    fn thinned_cues_keep_the_turns() {
        let mut c = Course::new();
        for k in 0..320 {
            let (lat, lon) = at(f64::from(k) * 10.0, 0.0);
            c.add_cue(lat, lon, if k >= 300 { Turn::Right } else { Turn::Water }, None);
        }
        for (x, y, z) in lines(&[(0.0, 0.0, 0.0), (4000.0, 0.0, 0.0)], 10.0) {
            c.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some(z as i32));
        }
        c.finish();
        assert_eq!(c.cues().iter().filter(|c| c.turn == Turn::Right).count(), 20);
        assert!(c.cues().windows(2).all(|w| w[0].at_m <= w[1].at_m), "in order");
        let water: Vec<_> = c.cues().iter().filter(|c| c.turn == Turn::Water).map(|c| c.at_m).collect();
        assert_eq!(water.len(), 150, "{water:?}");
        assert!(water[0] == 0 && water[149] >= 2970, "{water:?}");
        assert!(water.windows(2).all(|w| (19..=21).contains(&(w[1] - w[0]))), "{water:?}");
    }

    /// Riding `c` along `way` (a point a second): what is said, and where (m along).
    fn ride(c: &Course, way: &[(f64, f64)]) -> Vec<(u32, Event)> {
        let mut t = Tracker::new();
        let mut said = Vec::new();
        for &(x, y) in way {
            let (lat, lon) = at(x, y);
            if let Some(e @ Event::Cue(..)) = c.follow(&mut t, lat, lon) {
                said.push((t.along_m, e));
            }
        }
        said
    }

    /// The zigzag ridden at 8 m/s: each turn said at 200 m and at 32 m (4 s), once each, in order.
    #[test]
    fn said_before_and_at_each_turn() {
        let pts = zigzag();
        let c = course(&pts);
        let way: Vec<_> = lines(&pts, 8.0).iter().map(|p| (p.0, p.1)).collect();
        let said = ride(&c, &way);
        let want = zigzag_turns(&c);
        assert_eq!(said.len(), 2 * want.len(), "{said:?}");
        for (k, w) in want.iter().enumerate() {
            let (a, b) = (said[2 * k], said[2 * k + 1]);
            let Event::Cue(cue, d) = a.1 else { panic!() };
            assert!(cue.turn == w.1 && (192..=200).contains(&d), "{k}: {a:?}");
            assert!(w.0.abs_diff(a.0) <= 215, "{k}: {a:?}");
            assert_eq!(b.1, Event::Cue(cue, 0), "{k}");
            assert!(cue.at_m - b.0 <= 33, "{k}: {b:?}");
        }
    }

    /// At 15 m/s the warning comes 20 s ahead (300 m), the turn 4 s (60 m).
    #[test]
    fn faster_earlier() {
        let c = course(&lines(&[(0.0, 0.0, 0.0), (2000.0, 0.0, 0.0), (2000.0, 1000.0, 0.0)], 10.0));
        let way: Vec<_> = (0..200).map(|k| (f64::from(k) * 15.0, 0.0)).collect();
        let said = ride(&c, &way);
        assert!(matches!(said[0].1, Event::Cue(_, 286..=300)), "{said:?}");
        assert!((1940..=1955).contains(&said[1].0), "{said:?}");
    }

    /// Off the course nothing is said: a rider 150 m beside it, going the same way past where it turns left, hears
    /// nothing; back on it after the corner, the next turn is said, at 200 m and at the turn.
    #[test]
    fn nothing_said_off_course() {
        let c =
            course(&lines(&[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 1000.0, 0.0), (2000.0, 1000.0, 0.0)], 10.0));
        let mut way: Vec<_> = (0..60).map(|k| (f64::from(k) * 8.0, 0.0)).collect();
        way.extend((60..150).map(|k| (f64::from(k) * 8.0, -150.0)));
        way.extend((0..60).map(|k| (1200.0 - 25.0 * f64::from(k) / 6.0, -150.0 + f64::from(k) * 8.0)));
        way.extend((0..100).map(|k| (1000.0, 330.0 + f64::from(k) * 8.0)));
        let (mut t, mut said) = (Tracker::new(), Vec::new());
        for &(x, y) in &way {
            let (lat, lon) = at(x, y);
            if let Some(Event::Cue(cue, d)) = c.follow(&mut t, lat, lon) {
                assert!(t.off_m.unwrap() <= OFF_M, "said off course at ({x}, {y}): {t:?}");
                said.push((cue.turn, d));
            }
        }
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said.iter().all(|s| s.0 == Turn::Right) && said[1].1 == 0, "the right turn only: {said:?}");
    }

    /// An out-and-back: going out, the turn of the way out; coming back past the same place, the way back's.
    #[test]
    fn out_and_back_says_its_leg() {
        let shape = [(0.0, 300.0, 0.0), (0.0, 0.0, 0.0), (2000.0, 0.0, 0.0), (2000.0, 5.0, 0.0), (0.0, 5.0, 0.0)];
        // a file's cue on each leg at the same place, and no turn: the shape's turns are found too
        let mut pts = lines(&shape, 10.0);
        pts.push((0.0, 400.0, 0.0));
        let mut f = Course::new();
        // as a FIT course states them: with their distances (from 100 m, the file's first point's)
        for (x, y, t, d) in [(1000.0, 1.0, Turn::Water, 1400), (1000.0, 4.0, Turn::Food, 3405)] {
            let (lat, lon) = at(x, y);
            f.add_cue(lat, lon, t, Some(d));
        }
        let mut d = 100.0;
        for (k, &(x, y, z)) in pts.iter().enumerate() {
            if k > 0 {
                d += ((x - pts[k - 1].0).powi(2) + (y - pts[k - 1].1).powi(2)).sqrt();
            }
            let (lat, lon) = at(x, y);
            f.add(crate::course::Pt { lat, lon, alt_dm: Some(z as i32), split: false, dist_m: Some(d as u32) });
        }
        f.finish();
        let mut way: Vec<_> = (0..38).map(|k| (0.0, 300.0 - f64::from(k) * 8.0)).collect();
        way.extend((0..250).map(|k| (f64::from(k) * 8.0, 1.0)));
        way.extend((0..250).map(|k| (2000.0 - f64::from(k) * 8.0, 4.0)));
        way.extend((0..60).map(|k| (0.0, 5.0 + f64::from(k) * 8.0)));
        let said: Vec<_> = ride(&f, &way).into_iter().filter(|s| matches!(s.1, Event::Cue(_, 0))).collect();
        let kinds: Vec<_> =
            said.iter().map(|s| if let Event::Cue(c, _) = s.1 { c.turn } else { Turn::Point }).collect();
        assert_eq!(kinds, [Turn::Left, Turn::Water, Turn::UTurn, Turn::Food, Turn::Right], "{said:?}");
        assert!(said[3].0.abs_diff(3305) <= 35, "the way back's: {said:?}");
    }

    /// The cost at the course's read, worst cases: 40 km of 10 m points turning every 150 m (266 corners: the
    /// turns found and thinned to CUES), and the same with a file's cue at each corner (placed by position, the
    /// forward search). Printed, and bounded loosely.
    #[test]
    fn cost() {
        let mut corners = std::vec![(0.0, 0.0, 0.0)];
        for k in 1..=266 {
            let (x, y, _) = corners[k - 1];
            corners.push(if k % 2 == 1 { (x + 150.0, y, 0.0) } else { (x, y + 150.0, 0.0) });
        }
        let pts = lines(&corners, 10.0);
        let t0 = std::time::Instant::now();
        let c = course(&pts);
        let found = t0.elapsed();
        assert!((CUES / 2..=CUES).contains(&c.cues().len()), "{}", c.cues().len());
        let mut f = Course::new();
        for &(x, y, _) in &corners[1..] {
            let (lat, lon) = at(x, y);
            f.add_cue(lat, lon, Turn::Left, None);
        }
        for &(x, y, z) in &pts {
            f.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some(z as i32));
        }
        let t0 = std::time::Instant::now();
        f.finish();
        let placed = t0.elapsed();
        assert!((CUES / 2..=CUES).contains(&f.cues().len()), "{}", f.cues().len());
        std::println!(
            "a course of {} points: read with its {} turns found {found:?}, with {} cues placed {placed:?}",
            c.points().len(),
            c.cues().len(),
            f.cues().len()
        );
        assert!(found.as_millis() < 100 && placed.as_millis() < 100);
    }

    /// An arc of radius `r` about (`cx`, `cy`) from angle `a0` to `a1` (radians), a point every `step` m or less.
    fn arc(cx: f64, cy: f64, r: f64, a0: f64, a1: f64, step: f64) -> Vec<(f64, f64, f64)> {
        let n = ((a1 - a0).abs() * r / step).ceil().max(1.0) as usize;
        (0..=n)
            .map(|k| {
                let a = a0 + (a1 - a0) * k as f64 / n as f64;
                (cx + r * a.cos(), cy + r * a.sin(), 0.0)
            })
            .collect()
    }

    /// Through a roundabout of radius `r` whose west point is 500 m east of the start, come from the west: out by
    /// exit `exit` (1 to 4, the roads a quarter turn apart), round it counter-clockwise (`ccw`, driving on the
    /// right) or clockwise. `flare` 0: the roads straight into the ring (a corner each way); else they meet it
    /// `flare` degrees round from their own direction, from 20 m off (the review's shapes, 2026-10-25).
    fn roundabout(r: f64, exit: u32, ccw: bool, flare: f64) -> Vec<(f64, f64, f64)> {
        use core::f64::consts::PI;
        let s = if ccw { 1.0 } else { -1.0 };
        let (cx, off) = (500.0 + r, if flare == 0.0 { 0.0 } else { 20.0 });
        let e = f64::from(exit) * 90.0;
        let skip = usize::from(flare == 0.0); // the ring starts where the way in ends, the way out where it ends
        let mut pts = lines(&[(0.0, 0.0, 0.0), (500.0 - off, 0.0, 0.0)], 10.0);
        let ring = arc(cx, 0.0, r, PI + s * flare.to_radians(), PI + s * (e - flare).to_radians(), 3.0);
        pts.extend_from_slice(&ring[skip..]);
        let a = PI + s * e.to_radians();
        let out = [
            (cx + (r + off) * a.cos(), (r + off) * a.sin(), 0.0),
            (cx + (r + 500.0) * a.cos(), (r + 500.0) * a.sin(), 0.0),
        ];
        pts.extend_from_slice(&lines(&out, 10.0)[skip..]);
        pts
    }

    /// A roundabout is one cue, its exit, said at its way in, whichever way round it goes and however its roads
    /// meet it: radii 12 to 30 m, every exit, both directions, roads straight into the ring or flared. A first
    /// exit taken in one smooth sweep (flared roads: the ring itself barely ridden) may read as a bend and say
    /// nothing, never the other way (2026-10-25 review: straight on said SLIGHT LEFT or LEFT, a U-turn SHARP RIGHT).
    #[test]
    fn roundabouts_by_their_exit() {
        for r in [12.0, 15.0, 20.0, 30.0] {
            for flare in [0.0, 25.0] {
                for ccw in [true, false] {
                    for exit in 1..=4u32 {
                        let c = course(&roundabout(r, exit, ccw, flare));
                        let got = turns(&c);
                        let first = if ccw { Turn::Right } else { Turn::Left };
                        let ok = match got.as_slice() {
                            [] => exit == 1 && flare != 0.0,
                            &[(at, t)] => {
                                (470..=530).contains(&at)
                                    && (t == Turn::Roundabout(exit as u8) || exit == 1 && t == first)
                            }
                            _ => false,
                        };
                        assert!(ok, "r {r} flare {flare} ccw {ccw} exit {exit}: {got:?}");
                    }
                }
            }
        }
        // its word and its arrow
        assert_eq!((Turn::Roundabout(2).word(), Turn::Roundabout(2).glyph()), ("EXIT 2", "\x0a"));
    }

    /// Only a ring is a roundabout (2026-10-25 review of main 34ed689: a right-left-right jog of corners 15 to
    /// 60 m apart said one EXIT 1 and its 2nd and 3rd turns not at all, a right turn with 15 to 30 degree left
    /// kinks before and after said EXIT 3, a straight swerve EXIT 2): a jog is a turn at each corner, a kinked
    /// turn is said its way, a swerve under 45 degrees all told says nothing.
    #[test]
    fn junctions_are_not_roundabouts() {
        for gap in [15.0, 20.0, 25.0, 30.0, 40.0, 60.0] {
            for s in [1.0, -1.0] {
                let mut r = Road::new();
                r.straight(500.0).corner(s * 90.0).straight(gap).corner(-s * 90.0).straight(gap).corner(s * 90.0);
                r.straight(500.0);
                let c = course(&r.pts);
                let (a, b) = if s > 0.0 { (Turn::Left, Turn::Right) } else { (Turn::Right, Turn::Left) };
                let g = gap as u32;
                // a corner cut between two points is placed up to a point's step away
                check(&turns(&c), &[(500, a), (500 + g, b), (500 + 2 * g, a)], 10);
            }
        }
        let left = [Turn::SlightLeft, Turn::Left, Turn::SharpLeft];
        let right = [Turn::SlightRight, Turn::Right, Turn::SharpRight];
        for kink in [15.0, 20.0, 30.0] {
            for d in [10.0, 20.0] {
                for s in [1.0, -1.0] {
                    // a turn `s` with kinks the other way before and after (a cycle path's offset at the junction)
                    let mut r = Road::new();
                    r.straight(500.0).corner(-s * kink).straight(d).corner(s * (90.0 + 2.0 * kink)).straight(d);
                    r.corner(-s * kink).straight(500.0);
                    let got = turns(&course(&r.pts));
                    let way = if s > 0.0 { &left } else { &right };
                    let ok = got.len() == 1 && way.contains(&got[0].1) && (495..=535).contains(&got[0].0);
                    assert!(ok, "turn {s} kinks {kink} at {d} m: {got:?}");
                    // a straight swerve: out, back twice as much, out again
                    let mut r = Road::new();
                    r.straight(500.0).corner(-s * kink).straight(d).corner(s * 2.0 * kink).straight(d);
                    r.corner(-s * kink).straight(500.0);
                    let got = turns(&course(&r.pts));
                    assert!(got.is_empty(), "swerve {s} {kink} at {d} m: {got:?}");
                }
            }
        }
    }

    /// A turtle drawing a road: a point every 10 m on a straight, every 3 m on a bend.
    struct Road {
        pts: Vec<(f64, f64, f64)>,
        h: f64,
    }

    impl Road {
        fn new() -> Self {
            Road { pts: std::vec![(0.0, 0.0, 0.0)], h: 0.0 }
        }

        fn at(&self) -> (f64, f64) {
            let p = self.pts[self.pts.len() - 1];
            (p.0, p.1)
        }

        /// Ahead `m` metres.
        fn straight(&mut self, m: f64) -> &mut Self {
            let (x, y) = self.at();
            let to = (x + m * self.h.cos(), y + m * self.h.sin(), 0.0);
            self.pts.extend_from_slice(&lines(&[(x, y, 0.0), to], 10.0)[1..]);
            self
        }

        /// A bend of radius `r`, `deg` degrees (positive: left).
        fn bend(&mut self, r: f64, deg: f64) -> &mut Self {
            let (x, y) = self.at();
            let s = deg.signum();
            let (cx, cy) = (x - s * r * self.h.sin(), y + s * r * self.h.cos());
            let a0 = self.h - s * core::f64::consts::FRAC_PI_2;
            let a1 = a0 + deg.to_radians();
            self.pts.extend_from_slice(&arc(cx, cy, r, a0, a1, 3.0)[1..]);
            self.h += deg.to_radians();
            self
        }

        /// A junction's corner: `deg` degrees at once.
        fn corner(&mut self, deg: f64) -> &mut Self {
            self.h += deg.to_radians();
            self
        }
    }

    /// A road that winds says nothing; a junction after it still does (2026-10-25 review: 8 m hairpins each a
    /// U-TURN, 30 m bends each a turn, with beeps). A switchback climb (12 hairpins of 8 m and 15 m), a descent
    /// of 20 bends of 30 m and 20 of 60 m, alternate ways, then a right turn at a junction: only that is said.
    #[test]
    fn winding_roads_say_nothing() {
        let mut road = Road::new();
        for k in 0..12 {
            let r = if k < 6 { 8.0 } else { 15.0 };
            road.straight(250.0).bend(r, if k % 2 == 0 { 180.0 } else { -180.0 });
        }
        road.straight(300.0);
        for r in [30.0, 60.0] {
            for k in 0..20 {
                road.straight(120.0).bend(r, if k % 2 == 0 { 90.0 } else { -90.0 });
            }
        }
        road.straight(300.0).corner(-90.0).straight(500.0);
        let c = course(&road.pts);
        let junction = c.len_m() - 500;
        check(&turns(&c), &[(junction, Turn::Right)], 10);
        // ridden: the junction's two beeps, nothing else
        let way: Vec<_> = lines(&road.pts, 8.0).iter().map(|p| (p.0, p.1)).collect();
        let said = ride(&c, &way);
        assert_eq!(said.len(), 2, "{said:?}");
    }

    /// Past CUES the cues are thinned evenly along the whole course, not the first ones kept (2026-10-25 review:
    /// the last third of a long course had none): 600 corners 150 m apart, found on the shape or from a file,
    /// keep one in 4 or more, from the start to the end. A rider past the last one is told the cues are done.
    #[test]
    fn cues_thinned_evenly() {
        let mut corners = std::vec![(0.0, 0.0, 0.0)];
        for k in 1..=600 {
            let (x, y, _) = corners[k - 1];
            corners.push(if k % 2 == 1 { (x + 150.0, y, 0.0) } else { (x, y + 150.0, 0.0) });
        }
        let pts = lines(&corners, 10.0);
        let mut f = Course::new();
        for &(x, y, _) in &corners[1..] {
            let (lat, lon) = at(x, y);
            f.add_cue(lat, lon, Turn::Left, None);
        }
        for &(x, y, z) in &pts {
            f.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some(z as i32));
        }
        f.finish();
        for c in [course(&pts), f] {
            let ats: Vec<_> = c.cues().iter().map(|q| q.at_m).collect();
            let len = c.len_m();
            assert!((CUES / 2..=CUES).contains(&ats.len()), "{}", ats.len());
            assert!(ats[0] <= 4 * 150 && len - ats[ats.len() - 1] <= 5 * 150, "{ats:?}");
            assert!(ats.windows(2).all(|w| w[1] - w[0] <= 6 * 150), "{ats:?}");
            // past the last cue: done, and no next cue
            let mut t = Tracker::new();
            let end = *corners.last().unwrap();
            for d in [-40.0, -32.0, -24.0] {
                let (lat, lon) = at(end.0, end.1 + d);
                c.follow(&mut t, lat, lon);
            }
            assert!(c.cues_done(&t) && c.next_cue(&t).is_none(), "{t:?}");
        }
        // on its way, they are not
        let c = course(&pts);
        let mut t = Tracker::new();
        let (lat, lon) = at(10.0, 0.0);
        c.follow(&mut t, lat, lon);
        assert!(!c.cues_done(&t) && c.next_cue(&t).is_some());
    }

    /// One fix astray says nothing twice (2026-10-25 review: a fix 140 m back said the warning again): a warning
    /// said, one fix 140 m back, then on: not said again; one fix back over the cue passed before, then on: not
    /// that one either. Placed back for good (a lap course found on an earlier lap), the cues are said again.
    #[test]
    fn one_fix_astray_says_nothing_twice() {
        let c =
            course(&lines(&[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 1000.0, 0.0), (2000.0, 1000.0, 0.0)], 10.0));
        assert_eq!(c.cues().iter().map(|q| q.turn).collect::<Vec<_>>(), [Turn::Left, Turn::Right]);
        let east = |a: u32, b: u32| (a..=b).step_by(8).map(|x| (f64::from(x), 0.0)).collect::<Vec<_>>();
        let north = |a: u32, b: u32| (a..=b).step_by(8).map(|y| (1000.0, f64::from(y))).collect::<Vec<_>>();
        let kinds = |said: &[(u32, Event)]| -> Vec<(Turn, bool)> {
            said.iter()
                .map(|s| if let Event::Cue(c, d) = s.1 { (c.turn, d == 0) } else { (Turn::Point, false) })
                .collect()
        };
        let mut way = east(0, 880);
        way.push((740.0, 0.0));
        way.extend(east(888, 1000));
        way.extend(north(8, 850));
        way.push((900.0, 0.0));
        way.extend(north(858, 1000));
        way.extend((1008..1100).step_by(8).map(|x| (f64::from(x), 1000.0)));
        let said = ride(&c, &way);
        let once = [(Turn::Left, false), (Turn::Left, true), (Turn::Right, false), (Turn::Right, true)];
        assert_eq!(kinds(&said), once, "{said:?}");
        // back for good: from the corner's way out to 400 m before it, then on again
        let mut way = east(0, 1000);
        way.extend(north(8, 300));
        way.extend(east(600, 1000));
        way.extend(north(8, 300));
        let said = ride(&c, &way);
        assert_eq!(kinds(&said), [once[0], once[1], once[0], once[1]], "{said:?}");
    }

    /// Placing a file's cue without a distance looks into PLACE_CHUNKS chunks at most (2026-10-25 review: a waypoint
    /// inside a lap course, in every lap's box and near no pass, cost two chunks per lap, twice): 20 laps of a 200 m
    /// square, a waypoint in its middle (in every chunk's box) is dropped at that cost; one on the course is placed
    /// on the lap after the cue before, still.
    #[test]
    fn placing_a_cue_is_bounded() {
        let mut corners = std::vec![];
        for _ in 0..20 {
            corners.extend_from_slice(&[(0.0, 0.0, 0.0), (200.0, 0.0, 0.0), (200.0, 200.0, 0.0), (0.0, 200.0, 0.0)]);
        }
        corners.push((0.0, 0.0, 0.0));
        let c = course(&lines(&corners, 10.0));
        let o = c.origin.unwrap();
        let on = |x: f64, y: f64| {
            let (lat, lon) = at(x, y);
            crate::course::project(o, lat, lon)
        };
        let mut n = 0;
        assert_eq!(c.place(on(100.0, 100.0), 0, &mut n), None);
        // a third of the course at most (before: every chunk, and again from the start)
        let most = (PLACE_CHUNKS * crate::course::CHUNK as u32).min(c.points().len() as u32 / 3);
        assert!(n <= most, "{n} segments looked at");
        let mut n = 0;
        let lap10 = 10 * 80; // 80 points a lap
        let (_, at_m) = c.place(on(200.0, 3.0), lap10, &mut n).unwrap();
        assert!(at_m.abs_diff(10 * 800 + 203) <= 2, "{at_m}");
        // the time to place 256 waypoints in the middle, printed
        let mut f = Course::new();
        for k in 0..256 {
            let (lat, lon) = at(92.0 + f64::from(k % 16), 92.0 + f64::from(k / 16));
            f.add_cue(lat, lon, Turn::Left, None);
        }
        for (x, y, z) in lines(&corners, 10.0) {
            f.push((y * NORTH).round() as i32, (x * NORTH).round() as i32, Some(z as i32));
        }
        let t0 = std::time::Instant::now();
        f.finish();
        std::println!("256 waypoints inside a 20-lap course placed (none) in {:?}", t0.elapsed());
    }

    /// A GPX track point's own cue (BRouter's Locus style) goes on that point's pass (2026-10-25 review: placed
    /// by its position, it landed on the first pass through the same place): on an out-and-back, a cue on the
    /// way back is said on the way back.
    #[test]
    fn track_point_cue_on_its_own_pass() {
        let mut gpx = std::string::String::from("<gpx><trk><trkseg>");
        let pts = lines(&[(0.0, 0.0, 0.0), (0.0, 1000.0, 0.0), (4.0, 1000.0, 0.0), (4.0, 0.0, 0.0)], 10.0);
        for (k, &(x, y, _)) in pts.iter().enumerate() {
            let (lat, lon) = at(x, y);
            // the way back's point 500 m from the start: a right turn there (a side road), not on the way out
            let sym = if k > 100 && y == 500.0 { "<sym>right</sym>" } else { "" };
            gpx.push_str(&std::format!(
                "<trkpt lat=\"{}.{:07}\" lon=\"{}.{:07}\">{sym}</trkpt>",
                lat / 10_000_000,
                lat % 10_000_000,
                lon / 10_000_000,
                lon % 10_000_000
            ));
        }
        gpx.push_str("</trkseg></trk></gpx>");
        let mut c = Course::new();
        let mut g = crate::storage::course_file::Gpx::new();
        g.push_slice(gpx.as_bytes(), &mut |it| c.take(it));
        c.finish();
        check(&turns(&c), &[(1504, Turn::Right)], 3);
    }
}
