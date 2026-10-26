//! Climbs on the road ahead, without a course: stock's "Climb Challenge 2.0" (ghidra/recovered_src/ride/route_climb.c in
//! the bryton repo) on the routing graph of its map files ([`crate::route`]). Pure logic against a [`Graph`]; the
//! files come through [`Maps`], one tile a call.
//!
//! - **Snap** ([`Ahead::work`]): the nearest edge within [`SNAP_M`] of the fix, over every edge of the tiles
//!   loaded, plus `angle / 180 x 50` m when the road's direction differs from the heading by more than 30 degrees
//!   (stock's rule, RouteClimb_SnapToRoad 0xd028d508; it applies it when the speed is over 2, here always: the rider
//!   moves). The tiles around the fix are loaded first, one a call.
//! - **Trace**: from the snapped point, forward by the heading, node by node for [`TRACE_M`] metres or [`PATH`]
//!   nodes. At each node (RouteClimb_TraceAhead 0xd028de28): the neighbour on the same way as the road being
//!   followed with the smallest turn, else one of the same road class (the edge attribute byte) with the smallest
//!   turn, else the smallest turn of any; never back, never onto a node already passed, never a turn over 110
//!   degrees; the trace ends there if none is left (a dead end, a hairpin). A neighbour in a tile not loaded is
//!   asked for, one tile a call ([`Ahead::work`]), then the trace goes on. Ways are compared by a 16-bit fold of
//!   their ids, the first two of a node.
//! - **Smoothing** (RouteClimb_SmoothElevation 0xd028e130): the map's elevations are whole metres, hence grades of
//!   +-7 % between nodes 14 m apart. Stock runs a 9-tap quadratic Savitzky-Golay filter, weights -21, 14, 39, 54, 59,
//!   54, 39, 14, -21 over 231 (Savitzky and Golay 1964; Abraham Savitzky's table for 9 points), whose taps are
//!   `k = nodes x 70 m / length` nodes apart (about 70 m of road, at least 1): a window of about 560 m. The first and
//!   last `4k` nodes keep their raw value, and a trace under 630 m is not smoothed. Here the result is kept in
//!   decimetres (stock rounds to metres).
//! - **Climbs: this firmware's definition, not stock's.** Stock's detector (route_climb.c: fitted segments, climb
//!   above 2 % for 300 m, glued across 2 km gaps, average above 3 %, gain over 12 m) is a different rule from the
//!   one the owner's courses use ([`crate::course`], Garmin's ClimbPro: 500 m at 3 %, found on the profile in 50 m
//!   steps). A climb that is announced on a course and not on the road ahead (or the other way round) would be two
//!   rules for one hill, so the profile of the trace goes through the same finder (`Course::climbs_of`); stock's
//!   thresholds stay in the notes for a unit comparison with a ride of its own.
//! - **When:** the app runs it outdoors with no course followed, moving with a fix ([`Ahead::second`]); it traces
//!   again when the rider leaves the traced road ([`OFF_S`] seconds farther than [`OFF_PATH_M`]) or nears the end
//!   of a trace that stopped at its length ([`NEAR_END_M`]), at most every [`RETRACE_S`] seconds, and the work is
//!   spread over the main loop's passes (a slice of a tile read, inflated or decoded is the longest step).
//! - **Output:** the climbs on the trace (`start_m` from where the trace starts, [`Ahead::along_m`] the rider's
//!   place on it), the profile ([`Ahead::alt_at`], [`Ahead::grade_ahead`], as a [`Course`]'s), and the announcement
//!   of the next climb as the rider reaches it ([`ANNOUNCE_M`] before its start, or inside it), once.
//!
//! Nothing here panics or loops on any input: every index is checked, the trace ends at [`PATH`] points, the
//! snap is a pass over the graph's edges (sim/fuzz.rs feeds it random graphs and damaged files).

use crate::course::{CLIMBS, Climb, Course, step_mm};
use crate::fat::{Disk, Fat};
use crate::inflate::Scratch;
use crate::map::cos_q16;
use crate::route::{Graph, Loaded, Maps, NO_ELEV, NODES, Slice, TileState, tile_of};

/// The nearest road within this of the fix is the rider's, m.
pub const SNAP_M: u32 = 150;
/// The trace goes on this far, m.
pub const TRACE_M: u32 = 4000;
/// Points on a trace at most.
pub const PATH: usize = 400;
/// A turn over this many degrees ends a trace (cosine: -0.342, in thousandths).
const TURN_COS_MILLI: i64 = -342;
/// A trace shorter than this is not smoothed, m.
pub const SMOOTH_MIN_M: u32 = 630;
/// The filter's taps are about this far apart, m.
pub const SMOOTH_TAP_M: u32 = 70;
/// Slower than this, mm/s, the rider is not moving: no new trace, no announcement.
pub const MIN_SPEED_MM_S: u32 = 2000;
/// Farther than this from the trace, m, for [`OFF_S`] seconds in a row, the rider has left the road traced.
pub const OFF_PATH_M: u32 = 40;
/// See [`OFF_PATH_M`].
pub const OFF_S: u8 = 3;
/// A trace again no sooner than this many seconds after the last one began.
pub const RETRACE_S: u32 = 5;
/// After a failed try (no map, no road), the next one waits this many seconds.
pub const RETRY_S: u32 = 30;
/// A trace that stopped at its length is made again when this little of it is left ahead, m.
pub const NEAR_END_M: u32 = 1500;
/// A climb is announced when the rider is this close to its start, m.
pub const ANNOUNCE_M: u32 = 50;
/// A climb whose top is within this of the one announced is the same climb, m.
const SAME_CLIMB_M: u64 = 900;
/// Climbs kept of a trace.
const KEPT: usize = 4;
/// Tile loads that emptied the graph, at most, in one trace: then it gives up.
const MAX_CLEARS: u8 = 2;
/// The window of segments searched around the last one each second.
const WINDOW: (usize, usize) = (3, 40);

/// The fix a trace starts from, and the rider's pace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fix {
    /// Latitude, degrees x 1e-7.
    pub lat: i32,
    /// Longitude, degrees x 1e-7.
    pub lon: i32,
    /// Heading, degrees x 1e-5.
    pub heading_e5: i32,
    /// Speed, mm/s.
    pub speed_mm_s: u32,
}

/// A point of the trace.
#[derive(Clone, Copy, Default)]
struct P {
    lat: i32,
    lon: i32,
    /// The map's elevation, m ([`NO_ELEV`] until filled).
    elev: i16,
    /// Smoothed, dm.
    smooth: i16,
    /// Metres from the start of the trace.
    along: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Snap,
    Trace,
}

/// Why a trace ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// At its length or its point limit: the road goes on.
    Length,
    /// No road ahead that turns less than 110 degrees (a dead end, a hairpin), or a tile that is not there.
    Stop,
}

/// Speed index 1 of an edge's attribute byte (factor 0.3 in stock's table): inferred to be paths and steps.
const SLOW: u8 = 1;

/// An edge stock routes: the high nibble of its attribute indexes the speed table (STORAGE_AND_MAPS.md 4.7), which
/// has -1 (not routable) at 0 and 6 and 15 entries (0 to 14).
fn routable(attr: u8) -> bool {
    !matches!(attr >> 4, 0 | 6 | 15)
}

/// The edge a trace takes at a node: its rank (road class, cosine of the turn in thousandths), the neighbour's
/// place, the edge's class byte and the neighbour's place in the graph.
type Pick = ((u8, i64), (i32, i32), u8, usize);

/// What the next step needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Done(End),
    Need((u32, u32)),
}

/// The road ahead: the graph, the trace and its climbs. About 110 KB: main.rs keeps it in AXI SRAM.
pub struct Ahead {
    graph: Graph,
    maps: Maps,
    path: [P; PATH],
    n: usize,
    visited: [u32; NODES.div_ceil(32)],
    phase: Phase,
    fix: Fix,
    /// Seconds counted by [`Ahead::second`], and the second a trace last began and the next try may begin.
    now_s: u32,
    began_s: u32,
    retry_s: u32,
    clears: u8,
    // the trace being made: the node to reach, the one before, the way and class being followed, the length
    cur: (i32, i32),
    prev: (i32, i32),
    way: u16,
    attr: u8,
    dist_mm: u64,
    // the trace made
    have: bool,
    end: End,
    climbs: [Climb; KEPT],
    n_climbs: usize,
    along_m: u32,
    seg: usize,
    off_s: u8,
    said: bool,
    /// The rider is inside the climb said (a new trace of it is the same climb, whatever its top).
    in_said: bool,
    said_top: (i32, i32),
    /// The graph was emptied during this trace: its node numbers mean nothing, the path is searched instead.
    cleared: bool,
    /// Traces made and tiles asked for in all (the cost, sim.rs).
    pub traces: u32,
}

impl Default for Ahead {
    fn default() -> Self {
        Self::new()
    }
}

/// Metres from `units` of 1e-7 degrees of latitude (0.011132 m each, as map.rs).
fn units_m(units: u64) -> u64 {
    units * 11_132 / 1_000_000
}

/// The vector from `a` to `b` on the plane at cosine `cos` (Q16): east and north in units of 1e-7 degrees of
/// latitude, clamped (a node farther than 23 km is of no use to a trace, and the arithmetic stays in an i64).
fn vec2(a: (i32, i32), b: (i32, i32), cos: i64) -> (i64, i64) {
    let lim = 1i64 << 21; // 23 km: far enough for any road near the rider, and the products below fit an i64
    let dx = ((i64::from(b.1) - i64::from(a.1)) * cos) >> 16;
    (dx.clamp(-lim, lim), (i64::from(b.0) - i64::from(a.0)).clamp(-lim, lim))
}

fn norm(v: (i64, i64)) -> u64 {
    (v.0 * v.0 + v.1 * v.1).unsigned_abs().isqrt()
}

/// The penalty for a road whose direction is `cos_milli` (thousandths, 0 to 1000) off the heading: stock's
/// `angle / 180 x 50` m past 30 degrees; the angle from a table of arccosines every 0.1.
fn turn_penalty_m(cos_milli: i64) -> u64 {
    const ACOS: [u16; 10] = [90, 84, 78, 72, 66, 60, 53, 46, 37, 26];
    if cos_milli >= 866 {
        return 0;
    }
    let c = cos_milli.clamp(0, 1000) as usize;
    let (i, f) = (c / 100, (c % 100) as u32);
    let a0 = u32::from(ACOS.get(i).copied().unwrap_or(0));
    let a1 = u32::from(ACOS.get(i + 1).copied().unwrap_or(0));
    let angle = (a0 * (100 - f) + a1 * f) / 100;
    u64::from(angle * 50 / 180)
}

/// Sine of an angle in degrees (-360 to 360), by the series of tiles.rs, which is accurate within a quarter turn.
fn sin_deg(d: f64) -> f64 {
    let d = if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    };
    let a = d.abs();
    let s = crate::tiles::sin((if a > 90.0 { 180.0 - a } else { a }) * core::f64::consts::PI / 180.0);
    if d < 0.0 { -s } else { s }
}

/// A heading in degrees x 1e-5 (clockwise from north) as (east, north) x 16384.
fn heading_vec(h_e5: i32) -> (i64, i64) {
    let deg = f64::from(h_e5.rem_euclid(36_000_000)) / 1e5;
    ((sin_deg(deg) * 16384.0) as i64, (sin_deg(90.0 - deg) * 16384.0) as i64)
}

impl Ahead {
    /// Nothing loaded, nothing traced.
    pub const fn new() -> Ahead {
        Ahead {
            graph: Graph::new(),
            maps: Maps::new(),
            path: [P { lat: 0, lon: 0, elev: 0, smooth: 0, along: 0 }; PATH],
            n: 0,
            visited: [0; NODES.div_ceil(32)],
            phase: Phase::Idle,
            fix: Fix { lat: 0, lon: 0, heading_e5: 0, speed_mm_s: 0 },
            now_s: 0,
            began_s: 0,
            retry_s: 0,
            clears: 0,
            cur: (0, 0),
            prev: (0, 0),
            way: 0,
            attr: 0,
            dist_mm: 0,
            have: false,
            end: End::Length,
            climbs: [Climb { start_m: 0, len_m: 0, gain_m: 0 }; KEPT],
            n_climbs: 0,
            along_m: 0,
            seg: 0,
            off_s: 0,
            said: false,
            in_said: false,
            said_top: (0, 0),
            cleared: false,
            traces: 0,
        }
    }

    /// Forgets the trace and what it found, and the climb said (a new ride); the tiles held stay.
    pub fn reset(&mut self) {
        self.maps.abort(&mut self.graph);
        (self.phase, self.n, self.have, self.n_climbs, self.said) = (Phase::Idle, 0, false, 0, false);
        self.in_said = false;
        (self.off_s, self.seg, self.along_m, self.retry_s) = (0, 0, 0, 0);
    }

    /// The graph (tiles held, for the cost).
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Tiles read and bytes read from the card, in all.
    pub fn io(&self) -> (u32, u32) {
        (self.maps.tiles, self.maps.bytes)
    }

    /// A trace is made and the rider is on it.
    pub fn on_road(&self) -> bool {
        self.have && self.off_s < OFF_S
    }

    /// A trace is being made (tiles being read).
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// A tile is half loaded (its next slice is the next unit of work).
    pub fn loading(&self) -> bool {
        self.maps.pending().is_some()
    }

    /// The trace's length, m.
    pub fn len_m(&self) -> u32 {
        self.path.get(..self.n).and_then(<[P]>::last).map_or(0, |p| u32::from(p.along))
    }

    /// Why the trace ended.
    pub fn end(&self) -> End {
        self.end
    }

    /// The rider's place on the trace, m from its start.
    pub fn along_m(&self) -> u32 {
        self.along_m
    }

    /// The climbs of the trace (at most four), from its start.
    pub fn climbs(&self) -> &[Climb] {
        self.climbs.get(..self.n_climbs).unwrap_or(&[])
    }

    /// The next climb at `d` m on the trace (the one running, else the one ahead), and whether `d` is on it.
    pub fn climb_at(&self, d: u32) -> Option<(Climb, bool)> {
        let c = *self.climbs().iter().find(|c| c.end_m() > d)?;
        Some((c, d >= c.start_m))
    }

    /// The altitude `d` m along the trace, dm (smoothed), between the two points around it.
    pub fn alt_at(&self, d: u32) -> Option<i32> {
        let path = self.path.get(..self.n).filter(|p| !p.is_empty())?;
        let i = path.partition_point(|p| u32::from(p.along) <= d);
        let a = path.get(i.saturating_sub(1))?;
        Some(match path.get(i).filter(|_| i > 0) {
            Some(b) if b.along > a.along => {
                let (da, span) = (i64::from(d.saturating_sub(u32::from(a.along))), i64::from(b.along - a.along));
                i32::from(a.smooth) + ((i64::from(b.smooth) - i64::from(a.smooth)) * da / span) as i32
            }
            _ => i32::from(a.smooth),
        })
    }

    /// The grade of the 200 m after `d`, % x 10 (shorter near the end; None within 20 m of it), as a course's.
    pub fn grade_ahead(&self, d: u32) -> Option<i32> {
        let to = d.saturating_add(crate::course::AHEAD_M).min(self.len_m());
        let len = to.checked_sub(d).filter(|&l| l >= 20)?;
        Some((self.alt_at(to)? - self.alt_at(d)?) * 100 / len as i32)
    }

    /// The position of the trace `d` m from its start.
    fn point_at(&self, d: u32) -> Option<(i32, i32)> {
        let path = self.path.get(..self.n)?;
        let i = path.partition_point(|p| u32::from(p.along) < d).min(path.len().saturating_sub(1));
        path.get(i).map(|p| (p.lat, p.lon))
    }

    // -------------------------------------------------------------------------------------------------------
    // Once a second
    // -------------------------------------------------------------------------------------------------------

    /// The rider's second: `active` (outdoors, riding, no course, the setting on) and the fix when there is a good
    /// one. Places the rider on the trace, decides whether to trace again, and returns the climb to announce.
    pub fn second(&mut self, active: bool, fix: Option<Fix>) -> Option<Climb> {
        self.now_s = self.now_s.saturating_add(1);
        if !active {
            if self.have || self.phase != Phase::Idle {
                self.reset();
            }
            return None;
        }
        let moving = fix.filter(|f| f.speed_mm_s >= MIN_SPEED_MM_S)?;
        self.fix = moving;
        if self.have {
            self.locate(moving);
        }
        let since = self.now_s.saturating_sub(self.began_s);
        let stale = self.have && self.off_s >= OFF_S;
        let short = self.have && self.end == End::Length && self.len_m().saturating_sub(self.along_m) < NEAR_END_M;
        let due = if self.have { (stale || short) && since >= RETRACE_S } else { self.now_s >= self.retry_s };
        if self.phase == Phase::Idle && due {
            (self.phase, self.began_s, self.clears) = (Phase::Snap, self.now_s, 0);
        }
        self.announce()
    }

    /// Places the rider on the trace: the nearest segment around the last one.
    fn locate(&mut self, f: Fix) {
        let cos = cos_q16(f.lat);
        let here = (f.lat, f.lon);
        let last = self.n.saturating_sub(1);
        let (from, to) = (self.seg.saturating_sub(WINDOW.0), (self.seg + WINDOW.1).min(last));
        let mut best: Option<(u64, usize, u32)> = None;
        let (hx, hy) = heading_vec(f.heading_e5);
        for i in from..to {
            let (Some(a), Some(b)) = (self.path.get(i), self.path.get(i + 1)) else { break };
            let (va, vb) = (vec2(here, (a.lat, a.lon), cos), vec2(here, (b.lat, b.lon), cos));
            let (foot, t) = foot_of(va, vb);
            let d = norm(foot);
            if best.is_none_or(|(bd, ..)| d < bd) {
                let along = u32::from(a.along) + ((u32::from(b.along.saturating_sub(a.along)) as u64 * t) >> 16) as u32;
                best = Some((d, i, along));
            }
        }
        // going against the trace (a U-turn): off it too, through the same count; [`Ahead::second`] has the speed floor
        let against = |i: usize| match (self.path.get(i), self.path.get(i + 1)) {
            (Some(a), Some(b)) => {
                let v = vec2((a.lat, a.lon), (b.lat, b.lon), cos);
                v.0 * hx + v.1 * hy < 0
            }
            _ => false,
        };
        match best {
            Some((d, i, along)) if units_m(d) <= u64::from(OFF_PATH_M) && !against(i) => {
                (self.seg, self.along_m, self.off_s) = (i, along, 0);
            }
            Some((_, i, along)) if against(i) => {
                (self.seg, self.along_m, self.off_s) = (i, along, self.off_s.saturating_add(1));
            }
            Some(_) | None => self.off_s = self.off_s.saturating_add(1),
        }
    }

    /// The climb to announce now, if any: the next one on the trace once the rider is [`ANNOUNCE_M`] from its start
    /// or in it; once, whatever the retraces make of its length.
    fn announce(&mut self) -> Option<Climb> {
        if !self.on_road() {
            return None;
        }
        let Some((c, on)) = self.climb_at(self.along_m) else {
            self.in_said = false;
            return None;
        };
        let top = self.point_at(c.end_m())?;
        // the same climb: its top is where the one said had it, or the rider is inside the one said (a longer trace
        // of a long climb moves its top, and its start is then the rider's place: it reads as what is left)
        if self.said && (step_mm(self.said_top, top) < SAME_CLIMB_M * 1000 || (self.in_said && on)) {
            if on {
                (self.said_top, self.in_said) = (top, true);
            }
            return None;
        }
        if !on {
            self.in_said = false;
        }
        if self.along_m.saturating_add(ANNOUNCE_M) < c.start_m {
            return None;
        }
        (self.said, self.said_top, self.in_said) = (true, top, true);
        Some(c)
    }

    // -------------------------------------------------------------------------------------------------------
    // The work: tiles, snap, trace, smooth, climbs
    // -------------------------------------------------------------------------------------------------------

    /// One step of the trace being made: a tile read, or the snap, or the trace up to the next tile it needs, or
    /// the smoothing and the climbs. True while there is more to do (call again if the pass has time); false when
    /// idle. Never more than a slice of a tile ([`Slice::PASS`]) a call: a tile takes several, the next call goes on.
    pub fn work(&mut self, d: &mut impl Disk, mem: &mut [u16]) -> bool {
        if self.phase == Phase::Idle {
            return false;
        }
        let want = match self.phase {
            _ if self.maps.pending().is_some() => self.maps.pending(),
            Phase::Snap => self.snap_tiles(),
            _ => match self.trace() {
                Step::Need(t) => Some(t),
                Step::Done(end) => {
                    self.finish(end);
                    return false;
                }
            },
        };
        if let Some(t) = want {
            self.load(d, mem, t);
            return self.phase != Phase::Idle;
        }
        // all the tiles around the fix are known: snap
        match self.snap() {
            true => self.phase = Phase::Trace,
            false => self.fail(),
        }
        self.phase != Phase::Idle
    }

    /// The first tile around the fix (within [`SNAP_M`]) that is not known yet.
    fn snap_tiles(&self) -> Option<(u32, u32)> {
        let f = self.fix;
        let dlat = (u64::from(SNAP_M) * 1_000_000 / 11_132) as i32;
        let dlon = (i64::from(dlat) * 65_536 / cos_q16(f.lat).max(1)).min(i64::from(i32::MAX / 2)) as i32;
        let corners = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        let at = |(a, b): (i32, i32)| tile_of(f.lat.saturating_add(a * dlat), f.lon.saturating_add(b * dlon));
        // the centre first: without it there is nothing to snap to
        let centre = tile_of(f.lat, f.lon);
        if self.graph.state(centre.0, centre.1) == TileState::Unknown {
            return Some(centre);
        }
        corners.map(at).into_iter().find(|&t| self.graph.state(t.0, t.1) == TileState::Unknown)
    }

    /// Reads tile `t` into the graph. A graph that is full is emptied (the trace made so far is kept: it holds
    /// positions) and the tile asked for again, [`MAX_CLEARS`] times; a card error gives up for [`RETRY_S`].
    fn load(&mut self, d: &mut impl Disk, mem: &mut [u16], t: (u32, u32)) {
        let Ok(mut fat) = Fat::mount(d) else { return self.fail() };
        match self.maps.step(&mut fat, d, &mut self.graph, &mut Scratch(mem), t, Slice::PASS) {
            Ok(None | Some(Loaded::Yes | Loaded::No)) => {}
            Ok(Some(Loaded::Full)) if self.clears < MAX_CLEARS => {
                self.clears += 1;
                self.graph.clear();
                self.cleared = true;
            }
            Ok(Some(Loaded::Full)) => self.fail(),
            Err(_) => {
                self.maps.forget();
                self.fail();
            }
        }
    }

    /// Gives up this try: no trace, the next one after [`RETRY_S`].
    fn fail(&mut self) {
        self.maps.abort(&mut self.graph);
        (self.phase, self.have, self.n, self.n_climbs) = (Phase::Idle, false, 0, 0);
        self.retry_s = self.now_s.saturating_add(RETRY_S);
    }

    /// Snaps to the nearest road and starts the trace there; false when there is none within [`SNAP_M`].
    fn snap(&mut self) -> bool {
        let f = self.fix;
        let cos = cos_q16(f.lat);
        let here = (f.lat, f.lon);
        let (hx, hy) = heading_vec(f.heading_e5);
        let mut best: Option<(u64, usize, (i32, i32), u64)> = None; // score (units), node, the far end, t
        for (i, a) in self.graph.nodes().iter().enumerate() {
            let va = vec2(here, (a.lat, a.lon), cos);
            for (to, attr) in self.graph.edges(i) {
                if !routable(attr) {
                    continue;
                }
                let vb = vec2(here, to, cos);
                let (foot, t) = foot_of(va, vb);
                let d = norm(foot);
                if units_m(d) > u64::from(SNAP_M) {
                    continue;
                }
                let ab = (vb.0 - va.0, vb.1 - va.1);
                let len = norm(ab).max(1) as i64;
                let along = ((ab.0 * hx + ab.1 * hy).abs() * 1000) / (len * 16384);
                // a metre is 89.8 units
                let slow = if attr >> 4 == SLOW { u64::from(SNAP_M) * 1_000_000 / 11_132 } else { 0 };
                let score = d + turn_penalty_m(along) * 1_000_000 / 11_132 + slow;
                if best.is_none_or(|b| score < b.0) {
                    best = Some((score, i, to, t));
                }
            }
        }
        let Some((_, ai, bpos, t)) = best else { return false };
        let Some(a) = self.graph.node(ai).copied() else { return false };
        let b = self.graph.find(bpos.0, bpos.1).and_then(|j| self.graph.node(j)).copied();
        let attr = self.graph.edges(ai).find(|&(p, _)| p == bpos).map_or(0, |e| e.1);
        // forward: the end the heading points at
        let ab = vec2((a.lat, a.lon), bpos, cos);
        let forward_b = ab.0 * hx + ab.1 * hy >= 0;
        let (from, to) = if forward_b { ((a.lat, a.lon), bpos) } else { (bpos, (a.lat, a.lon)) };
        // the snapped point on the segment, and its elevation
        let foot = (
            (i64::from(a.lat) + (i64::from(bpos.0) - i64::from(a.lat)) * t as i64 / 65_536) as i32,
            (i64::from(a.lon) + (i64::from(bpos.1) - i64::from(a.lon)) * t as i64 / 65_536) as i32,
        );
        let elev = match b.filter(|b| b.elev != NO_ELEV && a.elev != NO_ELEV) {
            Some(b) => (i64::from(a.elev) + (i64::from(b.elev) - i64::from(a.elev)) * t as i64 / 65_536) as i16,
            None => a.elev,
        };
        self.n = 0;
        self.visited = [0; NODES.div_ceil(32)];
        self.cleared = false;
        self.push(foot, elev, 0);
        self.dist_mm = 0;
        self.prev = from;
        self.cur = to;
        self.attr = attr;
        self.way = b.and_then(|b| a.common_way(&b)).unwrap_or(0);
        true
    }

    fn push(&mut self, (lat, lon): (i32, i32), elev: i16, along: u16) {
        if let Some(p) = self.path.get_mut(self.n) {
            *p = P { lat, lon, elev, smooth: 0, along };
            self.n += 1;
        }
    }

    /// The node `i` at `pos` is on the trace already. After the graph was emptied its numbers mean nothing, and the
    /// trace is searched for the place (a few hundred points, once per step: only then).
    fn seen(&self, i: usize, pos: (i32, i32)) -> bool {
        if self.cleared {
            return self.path.get(..self.n).is_some_and(|p| p.iter().any(|q| (q.lat, q.lon) == pos));
        }
        self.visited.get(i / 32).is_some_and(|w| w >> (i % 32) & 1 == 1)
    }

    /// The trace, from `cur` on, up to a node whose tile is needed, or its end.
    fn trace(&mut self) -> Step {
        loop {
            if self.n >= PATH || self.dist_mm >= u64::from(TRACE_M) * 1000 {
                return Step::Done(End::Length);
            }
            let cur = self.cur;
            let Some(ci) = self.graph.find(cur.0, cur.1) else {
                let t = tile_of(cur.0, cur.1);
                return if self.graph.state(t.0, t.1) == TileState::Unknown {
                    Step::Need(t)
                } else {
                    Step::Done(End::Stop)
                };
            };
            // every neighbour must be known (or known to be nowhere) before choosing among them
            for (pos, _) in self.graph.edges(ci) {
                if self.graph.find(pos.0, pos.1).is_none() {
                    let t = tile_of(pos.0, pos.1);
                    if self.graph.state(t.0, t.1) == TileState::Unknown {
                        return Step::Need(t);
                    }
                }
            }
            let Some(node) = self.graph.node(ci).copied() else { return Step::Done(End::Stop) };
            let last = self.path.get(self.n.saturating_sub(1)).map_or(cur, |p| (p.lat, p.lon));
            self.dist_mm = self.dist_mm.saturating_add(step_mm(last, cur));
            if let Some(w) = self.visited.get_mut(ci / 32) {
                *w |= 1 << (ci % 32);
            }
            self.push(cur, node.elev, (self.dist_mm / 1000).min(u64::from(u16::MAX)) as u16);
            // the choice: (class, cosine of the turn, thousandths), the best
            let cos = cos_q16(cur.0);
            let u = vec2(self.prev, cur, cos);
            let mut pick: Option<Pick> = None;
            let on_slow = self.attr >> 4 == SLOW;
            for (pos, attr) in self.graph.edges(ci) {
                // stock marks an edge not routable by its speed index (0, 6, over 14: -1 in the table at 0xd0293898);
                // index 1 (factor 0.3) is inferred to be paths and steps: only on the way already on one, and last
                let slow = attr >> 4 == SLOW;
                if pos == self.prev || !routable(attr) || (slow && !on_slow) {
                    continue;
                }
                let Some(ni) = self.graph.find(pos.0, pos.1) else { continue };
                let Some(next) = self.graph.node(ni) else { continue };
                if self.seen(ni, pos) {
                    continue;
                }
                let v = vec2(cur, pos, cos);
                let (lu, lv) = (norm(u) as i64, norm(v) as i64);
                if lu == 0 || lv == 0 {
                    continue;
                }
                let c = (u.0 * v.0 + u.1 * v.1) * 1000 / (lu * lv);
                if c < TURN_COS_MILLI {
                    continue;
                }
                let class = if next.on_way(self.way) { 2 } else { u8::from(attr == self.attr) } + 4 * u8::from(!slow);
                if pick.is_none_or(|(k, ..)| (class, c) > k) {
                    pick = Some(((class, c), pos, attr, ni));
                }
            }
            let Some((_, pos, attr, ni)) = pick else { return Step::Done(End::Stop) };
            if let Some(next) = self.graph.node(ni)
                && !next.on_way(self.way)
            {
                self.way = node.common_way(next).unwrap_or(0);
            }
            (self.prev, self.cur, self.attr) = (cur, pos, attr);
        }
    }

    /// The trace is made: elevations filled, smoothed, the climbs found.
    fn finish(&mut self, end: End) {
        self.phase = Phase::Idle;
        self.traces = self.traces.wrapping_add(1);
        if !self.fill() {
            return self.fail();
        }
        self.smooth();
        let total = self.len_m();
        let (all, n) = Course::climbs_of(total, |d| self.alt_at(d));
        for (slot, c) in self.climbs.iter_mut().zip(all.iter().take(n.min(CLIMBS))) {
            *slot = *c;
        }
        (self.n_climbs, self.end, self.have) = (n.min(KEPT), end, true);
        (self.seg, self.along_m, self.off_s) = (0, 0, 0);
    }

    /// Nodes with no elevation take the one before (the first ones, the first known); false when none has one.
    fn fill(&mut self) -> bool {
        let Some(path) = self.path.get_mut(..self.n) else { return false };
        let Some(first) = path.iter().map(|p| p.elev).find(|&e| e != NO_ELEV) else { return false };
        let mut last = first;
        for p in path.iter_mut() {
            if p.elev == NO_ELEV {
                p.elev = last;
            }
            last = p.elev;
        }
        true
    }

    /// Stock's filter over the elevations (module doc), into `smooth`, dm.
    fn smooth(&mut self) {
        const W: [i32; 9] = [-21, 14, 39, 54, 59, 54, 39, 14, -21];
        let n = self.n;
        let len = self.len_m();
        let count = n.saturating_sub(1);
        let k = if len >= SMOOTH_MIN_M {
            ((count as u64 * u64::from(SMOOTH_TAP_M)) / u64::from(len)).max(1) as usize
        } else {
            0
        };
        let raw: [i16; PATH] = core::array::from_fn(|i| self.path.get(i).map_or(0, |p| p.elev));
        for (i, p) in self.path.iter_mut().take(n).enumerate() {
            p.smooth = raw.get(i).copied().unwrap_or(0).saturating_mul(10);
            if k == 0 || i < 4 * k || i + 4 * k > count {
                continue;
            }
            let mut acc = 0i32;
            for (j, w) in W.iter().enumerate() {
                // j - 4 taps either side, k nodes apart: in range by the test above
                if let Some(&e) = raw.get(i + j * k - 4 * k) {
                    acc += w * i32::from(e);
                }
            }
            // dm: acc / 231 m x 10, rounded half away from zero
            let v = (acc * 10 * 2 + 231 * acc.signum()) / (231 * 2);
            p.smooth = v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
    }
}

/// The point of segment `a`-`b` nearest the origin, and where along it, Q16 (0: `a`).
fn foot_of(a: (i64, i64), b: (i64, i64)) -> ((i64, i64), u64) {
    let ab = (b.0 - a.0, b.1 - a.1);
    let l2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if l2 == 0 { 0 } else { ((-(a.0 * ab.0 + a.1 * ab.1)).clamp(0, l2) << 16) / l2 };
    ((a.0 + ((ab.0 * t) >> 16), a.1 + ((ab.1 * t) >> 16)), t as u64)
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat;
    use crate::route::tests::metres;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// Memory for a test app (leaked: it lives as long as the test, as main.rs's static), and the map tiles' buffer.
    pub fn scratch() -> &'static mut [u16] {
        std::boxed::Box::leak(std::vec![0u16; 48 * 1024].into_boxed_slice())
    }

    pub fn memory() -> &'static mut Ahead {
        std::boxed::Box::leak(std::boxed::Box::new(Ahead::new()))
    }

    /// A disk that is never to be read: the graphs of these tests are built whole.
    pub struct NoDisk;
    impl Disk for NoDisk {
        fn read(&mut self, _: u32, _: &mut [u8]) -> Result<(), fat::Error> {
            panic!("a tile was asked for");
        }
    }

    pub type Pts<'a> = &'a [(f64, f64, i16)];

    /// A graph of roads, each (way id, class byte, points in metres east and north of the origin with their
    /// elevations): consecutive points are linked, both ways; points at the same place are one node. Tiles with
    /// nodes are loaded, the ones around them marked absent, so nothing is read.
    pub fn net(a: &mut Ahead, roads: &[(u16, u8, Pts)]) {
        let key = |x: f64, y: f64| metres(x, y);
        type Node = (i16, [u16; 2], Vec<((i32, i32), u8)>);
        let mut nodes: BTreeMap<(i32, i32), Node> = BTreeMap::new();
        for &(way, attr, pts) in roads {
            for (k, &(x, y, e)) in pts.iter().enumerate() {
                let n = nodes.entry(key(x, y)).or_insert((e, [0; 2], Vec::new()));
                if !n.1.contains(&way) {
                    let slot = if n.1[0] == 0 { 0 } else { 1 };
                    n.1[slot] = way;
                }
                if k > 0 {
                    let (px, py, _) = pts[k - 1];
                    n.2.push((key(px, py), attr));
                }
                if k + 1 < pts.len() {
                    let (nx, ny, _) = pts[k + 1];
                    n.2.push((key(nx, ny), attr));
                }
            }
        }
        let mut tiles: BTreeMap<(u32, u32), Vec<(i32, i32)>> = BTreeMap::new();
        for p in nodes.keys() {
            tiles.entry(tile_of(p.0, p.1)).or_default().push(*p);
        }
        for (&(x, y), ps) in &tiles {
            a.graph.begin_tile(x, y).unwrap();
            for p in ps {
                let (e, ways, edges) = &nodes[p];
                let edges: Vec<_> = edges.iter().map(|&((la, lo), at)| (la, lo, at)).collect();
                a.graph.add_node((p.0, p.1, *e), *ways, &edges).unwrap();
            }
            a.graph.end_tile(true);
        }
        for &(x, y) in tiles.keys() {
            for (dx, dy) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1), (0, 2), (1, 2), (2, 2)] {
                let t = (x + dx - 1, y + dy - 1);
                if a.graph.state(t.0, t.1) == TileState::Unknown {
                    a.graph.mark_absent(t.0, t.1);
                }
            }
        }
    }

    /// A straight road east from `x0` to `x1` every 25 m at `y`, its elevation by `elev(x)`.
    pub fn east(x0: f64, x1: f64, y: f64, elev: impl Fn(f64) -> f64) -> Vec<(f64, f64, i16)> {
        let n = ((x1 - x0) / 25.0).round() as i32;
        (0..=n).map(|k| (x0 + 25.0 * f64::from(k), y, elev(x0 + 25.0 * f64::from(k)).round() as i16)).collect()
    }

    /// The fix of a rider at (`x`, `y`) heading `deg` at 8 m/s.
    pub fn fix(x: f64, y: f64, deg: f64) -> Fix {
        let (lat, lon) = metres(x, y);
        Fix { lat, lon, heading_e5: (deg * 1e5) as i32, speed_mm_s: 8000 }
    }

    /// Traces from `f` on a graph that is whole; true when a trace was made.
    fn run(a: &mut Ahead, f: Fix) -> bool {
        a.fix = f;
        a.phase = Phase::Snap;
        a.clears = 0;
        let mut guard = 0;
        let mem = scratch();
        while a.work(&mut NoDisk, mem) {
            guard += 1;
            assert!(guard < 50);
        }
        a.have
    }

    /// The trace's points in metres east and north of the origin.
    fn points(a: &Ahead) -> Vec<(f64, f64)> {
        let (lat0, lon0) = metres(0.0, 0.0);
        let cos = 40_295.0 / 65_536.0;
        a.path[..a.n]
            .iter()
            .map(|p| (f64::from(p.lon - lon0) * 0.011_132 * cos, f64::from(p.lat - lat0) * 0.011_132))
            .collect()
    }

    /// Stock's filter by hand: 100 nodes 10 m apart (k = 7), a step of 100 m in the middle. At node 45 the taps
    /// (17, 24, 31, 38, 45, 52, 59, 66, 73) see 0 five times and 100 four times: 100 x (54 + 39 + 14 - 21) / 231 =
    /// 37.23 m. Node 28 is the first window: only its last tap (56) is up, 100 x -21 / 231 = -9.09 m. The first 28
    /// and the last 28 nodes keep their raw values; a trace under 630 m is not smoothed at all.
    #[test]
    fn smoothing_by_hand() {
        let mut a = Ahead::new();
        for i in 0..100 {
            a.path[i] = P { lat: 0, lon: 0, elev: if i < 50 { 0 } else { 100 }, smooth: 0, along: i as u16 * 10 };
        }
        a.n = 100;
        a.smooth();
        let s = |i: usize| a.path[i].smooth;
        assert_eq!((s(45), s(28), s(27), s(10), s(70), s(71), s(72), s(99)), (372, -91, 0, 0, 1030, 1091, 1000, 1000));
        // the filter keeps a straight slope as it is
        for i in 0..100 {
            a.path[i].elev = (i * 2) as i16;
        }
        a.smooth();
        assert!((28..71).all(|i| a.path[i].smooth.abs_diff(i as i16 * 20) <= 1), "a line stays a line");
        // 50 nodes 10 m apart are 490 m: raw
        a.n = 50;
        a.path[49].along = 490;
        a.smooth();
        assert!((0..50).all(|i| a.path[i].smooth == a.path[i].elev * 10));
    }

    /// At a junction the trace keeps to the road it is on (the same way) although the other road goes straighter:
    /// the main road bends 40 degrees, a branch of another way goes on straight.
    #[test]
    fn junction_keeps_the_way() {
        let a = &mut Ahead::new();
        let (c, s) = (40f64.to_radians().cos(), 40f64.to_radians().sin());
        let bend: Vec<_> = (0..=160).map(|k| (500.0 + 25.0 * f64::from(k) * c, 25.0 * f64::from(k) * s, 100)).collect();
        net(
            a,
            &[
                (1, 0xee, &east(0.0, 500.0, 0.0, |_| 100.0)),
                (1, 0xee, &bend),
                (2, 0xee, &east(500.0, 1400.0, 0.0, |_| 100.0)),
            ],
        );
        assert!(run(a, fix(100.0, 3.0, 90.0)));
        let p = points(a);
        let last = p.last().unwrap();
        assert!(last.1 > 1500.0 && last.0 > 2000.0, "along the bend, not the branch: {last:?}");
        assert!(p.iter().skip(25).all(|q| q.1 > -1.0), "{p:?}");
        assert_eq!(a.end(), End::Length);
        assert!((3950..=4100).contains(&a.len_m()), "{} m", a.len_m());
        // from the branch's own start, the branch is the way: the same junction, the other side
        let b = &mut Ahead::new();
        net(
            b,
            &[
                (1, 0xee, &east(0.0, 500.0, 0.0, |_| 100.0)),
                (1, 0xee, &bend),
                (2, 0xee, &east(500.0, 1400.0, 0.0, |_| 100.0)),
            ],
        );
        assert!(run(b, fix(900.0, 0.0, 90.0)));
        assert_eq!((b.end(), points(b).last().unwrap().1.abs() < 1.0), (End::Stop, true), "the branch's end");
    }

    /// With no common way (each stretch its own way), the road of the same class wins over a straighter one of
    /// another class; with the same class, the smallest turn.
    #[test]
    fn class_rule_then_smallest_turn() {
        let (c, s) = (40f64.to_radians().cos(), 40f64.to_radians().sin());
        let bend: Vec<_> = (0..=40).map(|k| (500.0 + 25.0 * f64::from(k) * c, 25.0 * f64::from(k) * s, 100)).collect();
        for (straight_class, want_straight) in [(0x99u8, false), (0xee, true)] {
            let a = &mut Ahead::new();
            net(
                a,
                &[
                    (1, 0xee, &east(0.0, 500.0, 0.0, |_| 100.0)),
                    (2, 0xee, &bend),
                    (3, straight_class, &east(500.0, 1500.0, 0.0, |_| 100.0)),
                ],
            );
            assert!(run(a, fix(100.0, 0.0, 90.0)));
            let last = *points(a).last().unwrap();
            assert_eq!(last.1.abs() < 1.0, want_straight, "class {straight_class:x}: {last:?}");
        }
    }

    /// A dead end ends the trace there; so does a hairpin (the only way on turns 170 degrees); a ring road is
    /// ridden once, never round and round.
    #[test]
    fn dead_end_hairpin_and_ring() {
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &east(0.0, 600.0, 0.0, |_| 100.0))]);
        assert!(run(a, fix(100.0, 0.0, 90.0)));
        assert_eq!((a.end(), a.len_m().abs_diff(500) <= 30), (End::Stop, true), "{} m", a.len_m());
        // east 400 m, then back west at an angle of 10 degrees off the way it came
        let back: Vec<_> = (0..=20)
            .map(|k| {
                (
                    400.0 - 25.0 * f64::from(k) * 10f64.to_radians().cos(),
                    25.0 * f64::from(k) * 10f64.to_radians().sin(),
                    100,
                )
            })
            .collect();
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &east(0.0, 400.0, 0.0, |_| 100.0)), (1, 0xee, &back)]);
        assert!(run(a, fix(100.0, 0.0, 90.0)));
        assert_eq!(
            (a.end(), a.len_m().abs_diff(300) <= 30),
            (End::Stop, true),
            "the U-turn is refused: {} m",
            a.len_m()
        );
        // a ring of 24 corners, radius 100 m: about 628 m round, entered and ridden once
        let ring: Vec<_> = (0..=24)
            .map(|k| {
                let t = f64::from(k % 24) * core::f64::consts::TAU / 24.0;
                (100.0 * t.cos(), 100.0 * t.sin(), 100)
            })
            .collect();
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &ring)]);
        assert!(run(a, fix(100.0, 20.0, 0.0)));
        assert!(a.n <= 26 && a.len_m() < 700, "{} points, {} m", a.n, a.len_m());
    }

    /// The nearest road is not always the rider's: heading east, a road along the heading 6 m away wins over one
    /// across it at 4 m (stock adds angle / 180 x 50 m past 30 degrees); heading north the other. Nothing within
    /// 150 m: no trace.
    #[test]
    fn snap_by_heading() {
        let cross = |a: &mut Ahead| {
            net(
                a,
                &[
                    (1, 0xee, &east(-500.0, 500.0, 0.0, |_| 100.0)),
                    (2, 0xee, &[(0.0, -500.0, 100), (0.0, 0.0, 100), (0.0, 500.0, 100)]),
                ],
            );
        };
        let a = &mut Ahead::new();
        cross(a);
        assert!(run(a, fix(-4.0, -6.0, 90.0)));
        let p = points(a);
        assert!(p.iter().all(|q| q.1.abs() < 1.0 || q.1.abs() < 7.0 && q.0 < 1.0), "east along y = 0: {:?}", &p[..3]);
        assert!(p.last().unwrap().0 > 400.0);
        let b = &mut Ahead::new();
        cross(b);
        assert!(run(b, fix(-4.0, -6.0, 0.0)));
        assert!(points(b).last().unwrap().1 > 400.0, "north along x = 0");
        let c = &mut Ahead::new();
        cross(c);
        assert!(!run(c, fix(-300.0, -200.0, 90.0)), "250 m from the nearest road");
        assert!(!c.have && !c.busy());
        assert!(c.retry_s > c.now_s, "it waits before it tries again");
    }

    /// 2 km at 6 % from 1 km: a climb of about that, found on the smoothed profile; the grade ahead and the
    /// altitude follow the trace. A bump of 300 m, a long false flat of 2 % and a descent are not climbs.
    #[test]
    fn climbs_on_the_trace() {
        let profile = |x: f64| match x {
            x if x < 1000.0 => 100.0,
            x if x < 3000.0 => 100.0 + 0.06 * (x - 1000.0),
            _ => 220.0,
        };
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &east(0.0, 4500.0, 0.0, profile))]);
        assert!(run(a, fix(100.0, 0.0, 90.0)));
        let c = a.climbs()[0];
        std::println!("climb on the trace: {c:?}");
        // the trace starts at x = 100 m: the climb starts about 900 m on, a little earlier for the smoothing
        assert!(
            (700..=1000).contains(&c.start_m) && (1900..=2500).contains(&c.len_m) && (115..=125).contains(&c.gain_m),
            "{c:?}"
        );
        assert_eq!(a.climbs().len(), 1);
        assert_eq!(a.alt_at(0), Some(1000), "100.0 m in dm");
        assert!(a.alt_at(2900).unwrap().abs_diff(2200) <= 30, "{:?}", a.alt_at(2900));
        let g = a.grade_ahead(1500).unwrap();
        assert!((55..=65).contains(&g), "{g}");
        assert_eq!(a.climb_at(0).map(|c| c.1), Some(false));
        assert_eq!(a.climb_at(c.start_m + 100).map(|c| c.1), Some(true));
        // not climbs
        for (name, f) in [
            ("a bump", (|x: f64| if (1000.0..1300.0).contains(&x) { 130.0 } else { 100.0 }) as fn(f64) -> f64),
            ("2 percent", |x: f64| 100.0 + 0.02 * x.min(3000.0)),
            ("a descent", |x: f64| 200.0 - 0.06 * x.min(2000.0)),
        ] {
            let b = &mut Ahead::new();
            net(b, &[(1, 0xee, &east(0.0, 4500.0, 0.0, f))]);
            assert!(run(b, fix(100.0, 0.0, 90.0)));
            assert!(b.climbs().is_empty(), "{name}: {:?}", b.climbs());
        }
    }

    /// Nodes with no elevation take the one before; a road with none has no trace.
    #[test]
    fn missing_elevations() {
        let a = &mut Ahead::new();
        let mut pts = east(0.0, 2000.0, 0.0, |x| 100.0 + x * 0.05);
        for p in pts.iter_mut().skip(10).take(5) {
            p.2 = NO_ELEV;
        }
        pts[0].2 = NO_ELEV;
        net(a, &[(1, 0xee, &pts)]);
        assert!(run(a, fix(60.0, 0.0, 90.0)));
        assert!(a.path[..a.n].iter().all(|p| p.elev != NO_ELEV));
        let b = &mut Ahead::new();
        let none: Vec<_> = east(0.0, 2000.0, 0.0, |_| 0.0).into_iter().map(|(x, y, _)| (x, y, NO_ELEV)).collect();
        net(b, &[(1, 0xee, &none)]);
        assert!(!run(b, fix(60.0, 0.0, 90.0)));
    }

    /// The seconds: the rider is placed on the trace as they ride it, the climb is said once as they reach it, off
    /// the road for 3 seconds starts another trace (not before 5 seconds after the last), and near the end of a
    /// trace that stopped at its length too; not moving, nothing changes; not active, everything is forgotten.
    #[test]
    fn riding_the_trace() {
        let profile = |x: f64| {
            if x < 1000.0 {
                100.0
            } else if x < 3000.0 {
                100.0 + 0.06 * (x - 1000.0)
            } else {
                220.0
            }
        };
        let (a, mem) = (memory(), scratch());
        net(a, &[(1, 0xee, &east(0.0, 9000.0, 0.0, profile))]);
        let mut said = Vec::new();
        // the first seconds start the trace
        assert_eq!(a.second(true, Some(fix(100.0, 0.0, 90.0))), None);
        assert_eq!(a.phase, Phase::Snap);
        while a.work(&mut NoDisk, mem) {}
        assert!(a.have && a.end() == End::Length);
        let start = a.climbs()[0].start_m;
        // ride 8 m/s along it
        let mut x = 100.0;
        for t in 0..400 {
            x += 8.0;
            if let Some(c) = a.second(true, Some(fix(x, 0.0, 90.0))) {
                said.push((t, x, c));
            }
            while a.work(&mut NoDisk, mem) {}
            assert!(a.on_road(), "at {x}");
        }
        assert_eq!(said.len(), 1, "said once: {said:?}");
        let (_, at, c) = said[0];
        let due = 100.0 + f64::from(start) - f64::from(ANNOUNCE_M);
        assert!(at >= due && at <= due + 16.0, "at x = {at}, due at {due}: climb {c:?}, trace start 100");
        assert!(a.traces >= 2, "the trace was made again near its end: {}", a.traces);
        // not moving: nothing changes
        let along = a.along_m();
        assert_eq!(a.second(true, Some(Fix { speed_mm_s: 500, ..fix(x + 500.0, 0.0, 90.0) })), None);
        assert_eq!(a.along_m(), along);
        // 100 m to the side for three seconds: off the road; a new trace waits for its 5 seconds
        let began = a.began_s;
        a.began_s = a.now_s;
        for _ in 0..3 {
            a.second(true, Some(fix(x, 100.0, 90.0)));
        }
        assert!(!a.on_road() && a.phase == Phase::Idle, "five seconds have not passed since the last trace");
        a.second(true, Some(fix(x, 100.0, 90.0)));
        a.second(true, Some(fix(x, 100.0, 90.0)));
        a.second(true, Some(fix(x, 100.0, 90.0)));
        assert_eq!(a.phase, Phase::Snap);
        let _ = began;
        // inactive: forgotten
        a.second(false, None);
        assert!(!a.have && a.n == 0 && a.phase == Phase::Idle);
    }

    /// A climb already said is not said again by the next trace, even if its top moved a little; the next climb is.
    #[test]
    fn a_climb_is_said_once() {
        let hills = |x: f64| match x {
            x if x < 600.0 => 100.0,
            x if x < 2600.0 => 100.0 + 0.06 * (x - 600.0),
            x if x < 3400.0 => 220.0,
            x if x < 5400.0 => 220.0 + 0.05 * (x - 3400.0),
            _ => 320.0,
        };
        let (a, mem) = (memory(), scratch());
        net(a, &[(1, 0xee, &east(0.0, 7000.0, 0.0, hills))]);
        let mut said = Vec::new();
        let mut x = 100.0;
        for _ in 0..800 {
            x += 8.0;
            if let Some(c) = a.second(true, Some(fix(x, 0.0, 90.0))) {
                said.push((x, c));
            }
            while a.work(&mut NoDisk, mem) {}
        }
        assert_eq!(said.len(), 2, "two climbs: {said:?}");
        assert!(said[0].0 < said[1].0 && said[1].0 > 2800.0, "{said:?}");
    }

    /// A climb longer than a trace: the next trace, made inside it, is the same climb (its start is the rider's
    /// place, its top is farther on): not said again.
    #[test]
    fn a_long_climb_is_said_once() {
        let (a, mem) = (memory(), scratch());
        net(a, &[(1, 0xee, &east(0.0, 9000.0, 0.0, |x| 100.0 + 0.05 * (x - 600.0).clamp(0.0, 7000.0)))]);
        let (mut said, mut x) = (0, 100.0);
        for _ in 0..900 {
            x += 8.0;
            said += u32::from(a.second(true, Some(fix(x, 0.0, 90.0))).is_some());
            while a.work(&mut NoDisk, mem) {}
        }
        assert!(a.traces >= 3, "traced again inside the climb: {}", a.traces);
        assert_eq!(said, 1);
    }

    /// A U-turn: the rider is on the road traced but going back along it: off the trace after 3 seconds, and a new
    /// one begins (5 seconds after the last). A wobble at a standstill is not a U-turn (the speed floor).
    #[test]
    fn a_u_turn_is_off_the_trace() {
        let (a, mem) = (memory(), scratch());
        net(a, &[(1, 0xee, &east(0.0, 6000.0, 0.0, |_| 100.0))]);
        a.second(true, Some(fix(500.0, 0.0, 90.0)));
        while a.work(&mut NoDisk, mem) {}
        for k in 0..3 {
            a.second(true, Some(fix(540.0 + f64::from(k), 0.0, 90.0)));
        }
        assert!(a.on_road());
        // the same place, heading west: the second 3 is off, and the trace is made again
        for _ in 0..2 {
            a.second(true, Some(fix(550.0, 0.0, 270.0)));
        }
        assert!(a.on_road() && a.phase == Phase::Idle);
        a.second(true, Some(fix(550.0, 0.0, 270.0)));
        assert!(!a.on_road());
        for _ in 0..5 {
            a.second(true, Some(fix(550.0, 0.0, 270.0)));
        }
        assert_eq!(a.phase, Phase::Snap);
        while a.work(&mut NoDisk, mem) {}
        assert!(a.on_road() && points(a).last().unwrap().0 < 550.0, "traced west now");
        // slower than the floor: heading means nothing
        let b = &mut Ahead::new();
        net(b, &[(1, 0xee, &east(0.0, 6000.0, 0.0, |_| 100.0))]);
        assert!(run(b, fix(500.0, 0.0, 90.0)));
        for _ in 0..10 {
            b.second(true, Some(Fix { speed_mm_s: 500, ..fix(520.0, 0.0, 270.0) }));
        }
        assert!(b.on_road() && b.phase == Phase::Idle);
    }

    /// Edges stock does not route (speed index 0, 6 or over 14) are not taken, nor snapped to; the slow class (index
    /// 1, inferred paths) only when the rider is on one already.
    #[test]
    fn unroutable_and_slow_edges() {
        let (c, s) = (40f64.to_radians().cos(), 40f64.to_radians().sin());
        let bend: Vec<_> = (0..=40).map(|k| (500.0 + 25.0 * f64::from(k) * c, 25.0 * f64::from(k) * s, 100)).collect();
        // no class or way in common: the smallest turn would take the straight road, unless it is filtered out
        for (attr, straight_taken) in [(0x0eu8, false), (0x6e, false), (0xfe, false), (0x1e, false), (0x4e, true)] {
            let a = &mut Ahead::new();
            net(
                a,
                &[
                    (1, 0x2e, &east(0.0, 500.0, 0.0, |_| 100.0)),
                    (2, 0x3e, &bend),
                    (3, attr, &east(500.0, 1500.0, 0.0, |_| 100.0)),
                ],
            );
            assert!(run(a, fix(100.0, 0.0, 90.0)));
            let straight = points(a).last().unwrap().1.abs() < 1.0;
            assert_eq!(straight, straight_taken, "class {attr:x}");
        }
        // snapping: the nearer road is a path (0x0e): the road 8 m farther is the rider's
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &east(-500.0, 500.0, 8.0, |_| 100.0)), (2, 0x0e, &east(-500.0, 500.0, 0.0, |_| 100.0))]);
        assert!(run(a, fix(0.0, 1.0, 90.0)));
        assert!(points(a).iter().all(|p| (p.1 - 8.0).abs() < 1.0), "on the road at y = 8");
        // on a slow edge already, the trace goes on along it
        let a = &mut Ahead::new();
        net(a, &[(1, 0x1e, &east(0.0, 3000.0, 0.0, |_| 100.0))]);
        assert!(run(a, fix(100.0, 0.0, 90.0)) && a.len_m() > 2000);
    }

    /// After the graph is emptied mid-trace its node numbers mean nothing: a place already on the trace is found
    /// by the trace itself.
    #[test]
    fn passed_places_after_a_clear() {
        let a = &mut Ahead::new();
        net(a, &[(1, 0xee, &east(0.0, 1000.0, 0.0, |_| 100.0))]);
        assert!(run(a, fix(100.0, 0.0, 90.0)));
        let p = (a.path[3].lat, a.path[3].lon);
        let i = a.graph.find(p.0, p.1).unwrap();
        assert!(a.seen(i, p));
        a.cleared = true;
        a.visited = [0; NODES.div_ceil(32)];
        assert!(a.seen(0, p) && !a.seen(0, (p.0 + 5, p.1)));
    }

    /// What one trace costs on the computer (the unit is a few tens of times slower: inferred from 400 MHz against
    /// 3 GHz and an in-order core): a tile read, inflated and decoded; the snap; the trace; the smoothing and the
    /// climbs. Printed with `--nocapture`; the bounds are loose.
    #[test]
    fn cost_of_a_trace() {
        use crate::fat::tests::Ram;
        use crate::route::tests::{FIXTURE_TILES, image_with_map};
        use std::time::Instant;
        let mut d = Ram(image_with_map(), 0);
        let mem = scratch();
        let mut a = std::boxed::Box::new(Ahead::new());
        // tile by tile, as the ride asks for them
        let t0 = Instant::now();
        let mut fat = Fat::mount(&mut d).unwrap();
        for t in FIXTURE_TILES {
            a.maps.load(&mut fat, &mut d, &mut a.graph, &mut Scratch(&mut *mem), t).unwrap();
        }
        let load = t0.elapsed() / 8;
        // the trace with everything loaded: snap, trace, smooth, climbs (graph kept: the run asks for nothing)
        let n = 200;
        let t0 = Instant::now();
        for _ in 0..n {
            a.fix = fix(1000.0, 0.0, 90.0);
            a.phase = Phase::Snap;
            while a.work(&mut d, mem) {}
        }
        let trace = t0.elapsed() / n;
        let (nodes, edges) = (a.graph.len(), (0..a.graph.len()).map(|i| a.graph.edges(i).count()).sum::<usize>());
        std::println!(
            "trace cost on the computer: {load:?} a tile read and decoded, {trace:?} a trace of {} points ({nodes} nodes and {edges} edges snapped over), {} bytes read",
            a.n,
            a.maps.bytes
        );
        assert!(a.have && load.as_millis() < 50 && trace.as_millis() < 50);
    }

    /// Traces on the roads of a real file (a z9 `.dat` from stock's maps): riders at random nodes, the tiles read as
    /// the trace needs them. Ignored: real maps stay outside the repository (AGENTS.md); point BRYTON_ROUTE_DAT at
    /// the file (`cargo test-host real_file_traces -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn real_file_traces() {
        use crate::fat::tests::Ram;
        use std::time::Instant;
        let Ok(path) = std::env::var("BRYTON_ROUTE_DAT") else {
            std::println!("BRYTON_ROUTE_DAT not set: skipped");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let name = std::string::String::from(std::path::Path::new(&path).file_name().unwrap().to_str().unwrap());
        let mut d = Ram(crate::route::tests::image_with_file(&name, &bytes), 0);
        let z: Vec<u32> = name.trim_end_matches(".dat").split('-').skip(1).map(|s| s.parse::<u32>().unwrap()).collect();
        let mut mem = std::vec![0u16; crate::route::SCRATCH_WORDS]; // the unit's own size
        let mut a = std::boxed::Box::new(Ahead::new());
        // riders: the centre of random tiles' first node, found by loading the tile alone
        let mut x = 12345u32;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let (mut made, mut climbs, mut lens, mut tiles, mut time) =
            (0u32, 0u32, Vec::new(), 0u32, std::time::Duration::ZERO);
        let mut grades: Vec<(u32, u32, u32)> = Vec::new();
        let mut sample = 0u32;
        for _ in 0..200 {
            let t = ((z[0] << 5) + rnd() % 32, (z[1] << 5) + rnd() % 32);
            let mut g = crate::route::Graph::new();
            let mut fat = Fat::mount(&mut d).unwrap();
            let Ok(crate::route::Loaded::Yes) =
                crate::route::Maps::new().load(&mut fat, &mut d, &mut g, &mut Scratch(&mut mem), t)
            else {
                continue;
            };
            let Some(n) = g.nodes().get(rnd() as usize % g.len().max(1)).copied() else { continue };
            let Some(((la, lo), _)) = g.edges(g.find(n.lat, n.lon).unwrap()).next() else { continue };
            let cos = cos_q16(n.lat);
            let (dx, dy) = vec2((n.lat, n.lon), (la, lo), cos);
            let heading =
                (f64::from(dx as i32).atan2(f64::from(dy as i32)).to_degrees().rem_euclid(360.0) * 1e5) as i32;
            *a = Ahead::new();
            let t0 = Instant::now();
            a.second(true, Some(Fix { lat: n.lat, lon: n.lon, heading_e5: heading, speed_mm_s: 8000 }));
            let mut guard = 0;
            while a.work(&mut d, &mut mem) && guard < 200 {
                guard += 1;
            }
            time += t0.elapsed();
            sample += 1;
            if a.have {
                made += 1;
                climbs += a.climbs().len() as u32;
                grades.extend(a.climbs().iter().map(|c| (c.grade_x10(), c.len_m, c.gain_m)));
                lens.push(a.len_m());
                tiles += a.io().0;
            }
        }
        lens.sort_unstable();
        std::println!(
            "{made} traces of {sample} riders, median {} m, {climbs} climbs, {:.1} tiles read a trace, {:?} a trace",
            lens.get(lens.len() / 2).copied().unwrap_or(0),
            f64::from(tiles) / f64::from(made.max(1)),
            time / sample.max(1)
        );
        grades.sort_unstable();
        std::println!(
            "climb grades x10 (grade, length, gain): min {:?} median {:?} max {:?}",
            grades.first(),
            grades.get(grades.len() / 2),
            grades.last()
        );
        assert!(made > 100);
    }
}
