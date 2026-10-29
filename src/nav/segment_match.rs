//! The live segment matcher (docs/segments.md, "Detection" and "Ghost"; step S2): pure logic, fed one GNSS fix a second,
//! one segment at a time. Nothing on the unit calls it yet (S3 wires it).
//!
//! Two phases, one small state machine ([`Matcher`], under 100 bytes):
//! - **idle**: [`Matcher::step_idle`] looks at the headers the index holds ([`Header`]: the first points of each
//!   segment are enough): [`Event::Approach`] once within [`APPROACH_M`] of a start, [`Event::Start`] when the rider
//!   passes within [`START_M`] of the first point heading within [`HEADING_DEG`] of the segment's first bearing, the
//!   start time interpolated between the two fixes around the closest approach. The first header that starts wins;
//!   while a segment is active, idle steps are ignored (stock does the same: inferred).
//! - **active**: [`Matcher::step_active`] with the parsed [`Segment`]: the projection on its track by a moving window,
//!   **the same code as the course follower's** (`course::nearest_by` and `course::seg_hit`, shared
//!   by generalising `Course::nearest` over a closure); [`Event::Lost`] after [`LOST_S`] s more than [`OFF_M`] off the
//!   track or back more than [`OFF_M`] along it (a U-turn), or when paused over [`PAUSE_S`] s; [`Event::Finish`] when
//!   the rider crosses the last point's perpendicular within [`START_M`] of it, the time interpolated at the crossing,
//!   with the delta against each kept target.
//!
//! **Ghost:** [`target_s`] is a kept target's time at a distance, interpolated between points; [`delta_s`] is the
//! elapsed time minus it (negative: ahead). A target known only by its total time has times on a straight line, so it
//! works the same.
//!
//! Decisions (S2): the index's "bounding box" is that of the header's start pairs ([`near`]), since the header holds no
//! other point; a start needs 2 m of movement between fixes (a stopped rider has no heading); the rider must cross the
//! end's perpendicular to finish (stopping 3 m short never finishes); time is whole seconds of the ride clock, the
//! start and finish interpolated to the millisecond, the elapsed time rounded.
//!
//! Nothing here panics: reads use `get`, and the module denies `clippy::indexing_slicing` outside the tests.

#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::cast_possible_truncation))]

use crate::course::{AHEAD, BEHIND, nearest_by, seg_hit, step_mm};
use crate::map::{cos_q16, project};
use crate::segment::{Header, KEPT, Segment};

/// A banner is due within this of a start, m.
pub const APPROACH_M: u32 = 300;
/// The rider starts within this of the first point, m (stock: under 30 m / 20 m on the projection), and finishes
/// within this of the last.
pub const START_M: u32 = 25;
/// The start heading may differ from the first bearing by this, degrees (60: cos = 1/2, tested without trigonometry).
pub const HEADING_DEG: u32 = 60;
/// More than this off the track, or this far back along it, m, is off.
pub const OFF_M: u32 = 50;
/// Off for this long, s, and the segment is lost.
pub const LOST_S: u32 = 10;
/// A pause (no movement, or no fix) over this, s, drops the segment.
pub const PAUSE_S: u32 = 300;
/// Less movement than this, m, is standing still.
const STILL_M: u64 = 3;

/// What the matcher has to say (one thing at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Within [`APPROACH_M`] of the start of segment `id`, said once.
    Approach(u32),
    /// Segment `id` started: the caller loads it ([`Segment::parse`]) and feeds [`Matcher::step_active`].
    Start(u32),
    /// Segment `id` was lost (off the track, a U-turn, a pause, or not the segment loaded): dropped.
    Lost(u32),
    /// Segment `id` finished in `elapsed_s`; `deltas[c]` is the elapsed time minus the time of the kept target in
    /// column `c` of [`Segment::kept`] (negative: ahead), None for a column with no target.
    Finish {
        /// The segment.
        id: u32,
        /// Elapsed time, s.
        elapsed_s: u32,
        /// Against each kept target, s.
        deltas: [Option<i32>; KEPT],
    },
}

/// The segment being ridden.
#[derive(Clone, Copy, Debug)]
struct Active {
    id: u32,
    /// The window's segment (from point `seg` to `seg + 1`).
    seg: u16,
    /// Distance along the segment, m, and the furthest so far.
    along_m: u32,
    best_m: u32,
    start_ms: u64,
    last_t: u32,
    /// Seconds off the track so far.
    off_s: u32,
    /// Seconds standing still so far, and where it started.
    still_s: u32,
    still_at: (i32, i32),
    /// The last fix, for the finish's crossing.
    prev: (i32, i32),
}

/// The matcher: the last fix, the segment approached last, the active segment.
#[derive(Clone, Copy, Debug)]
pub struct Matcher {
    prev: Option<(i32, i32, u32)>,
    approached: u32,
    active: Option<Active>,
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Plane millimetres east and north of `o` (degrees x 1e-7 and the cosine of its latitude), as [`step_mm`]'s plane.
fn mm(o: (i32, i32, i64), lat: i32, lon: i32) -> (i64, i64) {
    let dy = (i64::from(lat) - i64::from(o.0)) * 11_132 / 1000;
    let dx = (i64::from(lon) - i64::from(o.1)) * 11_132 * o.2 / 65_536 / 1000;
    (dx, dy)
}

/// A start pair that is in use (stored as (0, 0) when unused).
fn used(p: &&(i32, i32)) -> bool {
    **p != (0, 0)
}

/// Whether (`lat`, `lon`) is within [`APPROACH_M`] of the box of the header's start pairs: the index's first filter.
///
/// ```
/// use bryton_rustfw::segment::Segment;
/// use bryton_rustfw::segment_match::near;
/// let mut s = Segment::new();
/// s.header.start[0] = (0, 100_000_000);
/// assert!(near(&s.header, 0, 100_000_000 + 20_000)); // 220 m east
/// assert!(!near(&s.header, 0, 100_000_000 + 40_000)); // 440 m east
/// ```
pub fn near(h: &Header, lat: i32, lon: i32) -> bool {
    let mut it = h.start.iter().filter(used);
    let Some(&first) = it.next() else { return false };
    let (lo, hi) =
        it.fold((first, first), |(lo, hi), &(a, b)| ((lo.0.min(a), lo.1.min(b)), (hi.0.max(a), hi.1.max(b))));
    // 1 m is 89.8e-7 degrees of latitude; a degree of longitude is shorter by the cosine
    let dlat = i64::from(APPROACH_M) * 1_000_000 / 11_132;
    let dlon = dlat * 65_536 / cos_q16(lat).max(1000);
    let (la, lo_) = (i64::from(lat), i64::from(lon));
    la >= i64::from(lo.0) - dlat
        && la <= i64::from(hi.0) + dlat
        && lo_ >= i64::from(lo.1) - dlon
        && lo_ <= i64::from(hi.1) + dlon
}

/// A kept target's time at distance `d` m along the segment, s, interpolated between the points around it; None for
/// a segment with no points or a column that does not exist.
pub fn target_s(seg: &Segment, col: usize, d: u32) -> Option<u32> {
    let n = seg.npoints.min(seg.dist_m.len());
    let dist = seg.dist_m.get(..n)?;
    let at = |i: usize| Some((*dist.get(i)?, u32::from(*seg.times.get(i)?.get(col)?)));
    let i = dist.partition_point(|&x| x <= d);
    let (d1, v1) = at(i.min(n.checked_sub(1)?))?;
    let (d0, v0) = at(i.saturating_sub(1))?;
    Some(lerp(d, d0, d1, v0, v1))
}

/// `v0` at `d0` to `v1` at `d1`, at `d` (clamped to the ends; `v0` if the span is empty).
///
/// ```
/// use bryton_rustfw::segment_match::lerp;
/// assert_eq!(lerp(25, 0, 50, 0, 10), 5);
/// assert_eq!(lerp(99, 0, 50, 0, 10), 10);
/// assert_eq!(lerp(7, 5, 5, 3, 9), 3);
/// assert_eq!(lerp(25, 0, 50, 10, 0), 5);
/// ```
pub fn lerp(d: u32, d0: u32, d1: u32, v0: u32, v1: u32) -> u32 {
    if d1 <= d0 || d <= d0 {
        return v0;
    }
    let f = u64::from(d.min(d1) - d0);
    let (a, b, span) = (u64::from(v0), u64::from(v1), u64::from(d1 - d0));
    let v = if b >= a { a + (b - a) * f / span } else { a - (a - b) * f / span };
    u32::try_from(v).unwrap_or(v1) // within [v0, v1] already
}

/// The ghost's delta, s: `elapsed_s` minus the target's time at `d` m (negative: ahead).
///
/// ```
/// use bryton_rustfw::segment::Segment;
/// use bryton_rustfw::segment_match::delta_s;
/// assert_eq!(delta_s(&Segment::new(), 0, 10, 5), None); // no points
/// ```
pub fn delta_s(seg: &Segment, col: usize, d: u32, elapsed_s: u32) -> Option<i32> {
    let t = target_s(seg, col, d)?;
    i32::try_from(i64::from(elapsed_s) - i64::from(t)).ok()
}

fn ms_to_s(ms: u64) -> u32 {
    u32::try_from((ms + 500) / 1000).unwrap_or(u32::MAX)
}

/// The time (ms of the ride clock) a rider going from fix `a` to fix `b` starts segment `h`, if that pass starts it:
/// closest approach to the first point within [`START_M`], moving at least 2 m, heading within [`HEADING_DEG`] of the
/// first bearing (the first point to the next one in use).
fn start_ms(h: &Header, a: (i32, i32, u32), b: (i32, i32, u32)) -> Option<u64> {
    let first = *h.start.iter().find(used)?;
    let next = h.start.iter().find(|p| used(p) && **p != first)?;
    let o = (first.0, first.1, cos_q16(first.0));
    let v = mm(o, next.0, next.1);
    let (pa, pb) = (mm(o, a.0, a.1), mm(o, b.0, b.1));
    let m = (pb.0 - pa.0, pb.1 - pa.1);
    let l2 = i128::from(m.0) * i128::from(m.0) + i128::from(m.1) * i128::from(m.1);
    if l2 < 4_000_000 {
        return None; // under 2 m: standing still has no heading
    }
    // the closest approach of the pass to the first point (the origin), 0..=1024 along it
    let dot0 = i128::from(pa.0) * i128::from(m.0) + i128::from(pa.1) * i128::from(m.1);
    let u = (-dot0 * 1024 / l2).clamp(0, 1024);
    if u == 1024 {
        return None; // still approaching: the next pass has the closest approach in it
    }
    let c = (i128::from(pa.0) + i128::from(m.0) * u / 1024, i128::from(pa.1) + i128::from(m.1) * u / 1024);
    if c.0 * c.0 + c.1 * c.1 > i128::from(START_M * 1000).pow(2) {
        return None;
    }
    // cos 60 = 1/2: dot > 0 and 4 dot^2 >= |m|^2 |v|^2
    let dot = i128::from(m.0) * i128::from(v.0) + i128::from(m.1) * i128::from(v.1);
    let lv2 = i128::from(v.0) * i128::from(v.0) + i128::from(v.1) * i128::from(v.1);
    if dot <= 0 || 4 * dot * dot < l2 * lv2 {
        return None;
    }
    let span = u64::from(b.2.saturating_sub(a.2)) * 1000;
    Some(u64::from(a.2) * 1000 + span * u64::try_from(u).unwrap_or(0) / 1024)
}

impl Matcher {
    /// Nothing seen yet.
    pub const fn new() -> Self {
        Matcher { prev: None, approached: 0, active: None }
    }

    /// The id of the segment being ridden.
    pub fn active_id(&self) -> Option<u32> {
        self.active.map(|a| a.id)
    }

    /// Distance along the active segment, m.
    pub fn along_m(&self) -> Option<u32> {
        self.active.map(|a| a.along_m)
    }

    /// Seconds since the active segment started, at ride clock `now` (s).
    pub fn elapsed_s(&self, now: u32) -> Option<u32> {
        self.active.map(|a| ms_to_s((u64::from(now) * 1000).saturating_sub(a.start_ms)))
    }

    /// Drops the active segment and the last fix (the ride ended, the card was ejected).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// One fix (degrees x 1e-7, ride clock `t` s) while no segment is active: [`Event::Start`] of the first header
    /// that starts, else [`Event::Approach`] of the first one newly near. Ignored while a segment is active.
    pub fn step_idle<'a>(
        &mut self,
        headers: impl IntoIterator<Item = &'a Header>,
        lat: i32,
        lon: i32,
        t: u32,
    ) -> Option<Event> {
        if self.active.is_some() {
            return None;
        }
        let prev = self.prev.replace((lat, lon, t));
        let (mut approach, mut any) = (None, false);
        for h in headers.into_iter().filter(|h| near(h, lat, lon)) {
            if let Some(ms) = prev.and_then(|p| start_ms(h, p, (lat, lon, t))) {
                let still_at = (lat, lon);
                let (id, last_t, prev) = (h.id, t, still_at);
                let a = Active {
                    id,
                    seg: 0,
                    along_m: 0,
                    best_m: 0,
                    start_ms: ms,
                    last_t,
                    off_s: 0,
                    still_s: 0,
                    still_at,
                    prev,
                };
                self.active = Some(a);
                self.approached = id; // no banner after a lost segment, near its start
                return Some(Event::Start(id));
            }
            let Some(first) = h.start.iter().find(used) else { continue };
            if step_mm((lat, lon), *first) <= u64::from(APPROACH_M) * 1000 {
                any = true;
                if approach.is_none() && self.approached == 0 {
                    approach = Some(h.id);
                }
            }
        }
        if !any {
            self.approached = 0; // away from all of them: the next pass may say it again
        }
        let id = approach?;
        self.approached = id;
        Some(Event::Approach(id))
    }

    /// One fix while a segment is active; `seg` is the parsed file of [`Matcher::active_id`] (another one drops it).
    pub fn step_active(&mut self, seg: &Segment, lat: i32, lon: i32, t: u32) -> Option<Event> {
        let mut a = self.active?;
        let ev = Self::advance(&mut a, seg, lat, lon, t);
        match ev {
            Some(_) => *self = Matcher { prev: None, active: None, ..*self },
            None => self.active = Some(a),
        }
        ev
    }

    fn advance(a: &mut Active, seg: &Segment, lat: i32, lon: i32, t: u32) -> Option<Event> {
        let lost = Some(Event::Lost(a.id));
        let last = seg.npoints.checked_sub(1)?;
        let (Some(p0), Some(end), Some(before)) = (seg.pt(0), seg.pt(last), seg.pt(last.saturating_sub(1))) else {
            return lost;
        };
        let dt = t.saturating_sub(a.last_t);
        a.last_t = t;
        if seg.header.id != a.id || dt > PAUSE_S {
            return lost;
        }
        if step_mm(a.still_at, (lat, lon)) < STILL_M * 1000 {
            a.still_s = a.still_s.saturating_add(dt);
            if a.still_s > PAUSE_S {
                return lost;
            }
        } else {
            (a.still_s, a.still_at) = (0, (lat, lon));
        }
        let o = (p0.lat, p0.lon, cos_q16(p0.lat));
        let xy = |i: usize| seg.pt(i).map(|q| project(o, q.lat, q.lon));
        let dist = |i: usize| seg.dist_m.get(i).copied();
        let p = project(o, lat, lon);
        let from = usize::from(a.seg);
        let window = from.saturating_sub(BEHIND)..(from + AHEAD).min(last);
        let on = |i: usize| Some(seg_hit(xy(i)?, xy(i + 1)?, (dist(i)?, dist(i + 1)?), p));
        let hit = nearest_by(on, &[window], a.along_m, &mut 0);
        let prev = core::mem::replace(&mut a.prev, (lat, lon));
        // off the track, or a U-turn (the way back is on the track, but behind the furthest point)
        let Some(h) = hit.filter(|h| h.off_m <= OFF_M && h.along_m.saturating_add(OFF_M) >= a.best_m) else {
            a.off_s = a.off_s.saturating_add(dt);
            return (a.off_s >= LOST_S).then_some(Event::Lost(a.id));
        };
        a.off_s = 0;
        a.seg = u16::try_from(h.seg).unwrap_or(u16::MAX);
        a.along_m = h.along_m;
        a.best_m = a.best_m.max(h.along_m);
        let len = dist(last)?;
        if a.best_m.saturating_add(OFF_M) < len {
            return None;
        }
        // finish: past the last point's perpendicular, within START_M of it
        let (e, b) = (mm(o, end.lat, end.lon), mm(o, before.lat, before.lon));
        let dir = (i128::from(e.0 - b.0), i128::from(e.1 - b.1));
        let side = |f: (i32, i32)| {
            let q = mm(o, f.0, f.1);
            i128::from(q.0 - e.0) * dir.0 + i128::from(q.1 - e.1) * dir.1 // negative before the end
        };
        let (s0, s1) = (side(prev), side((lat, lon)));
        if s1 < 0 || step_mm((lat, lon), (end.lat, end.lon)) > u64::from(START_M) * 1000 {
            return None;
        }
        // the crossing, between the previous fix and this one
        let f = if s0 < 0 { u64::try_from(-s0 * 1024 / (s1 - s0).max(1)).unwrap_or(1024).min(1024) } else { 0 };
        let at_ms = u64::from(t.saturating_sub(dt)) * 1000 + u64::from(dt) * 1000 * f / 1024;
        let elapsed_s = ms_to_s(at_ms.saturating_sub(a.start_ms));
        let mut deltas = [None; KEPT];
        for (c, d) in deltas.iter_mut().enumerate() {
            if seg.kept.get(c).is_some_and(Option::is_some) {
                *d = delta_s(seg, c, len, elapsed_s);
            }
        }
        Some(Event::Finish { id: a.id, elapsed_s, deltas })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::course::{Course, Tracker};
    use crate::segment::tests::synth;
    use std::vec::Vec;

    const CODES: [[u8; 2]; 5] = [*b"07", *b"05", *b"06", *b"12", *b"13"];

    /// A parsed synthetic segment: `np` points 50 m apart east from 0 N 10 E, 10 s apart (5 m/s), the PR's time
    /// `10 i`, the KOM's `10 i + 1`, the QOM's `10 i + 2`, the recent's `10 i + 3`.
    pub(crate) fn seg(np: usize) -> Segment {
        let mut s = Segment::new();
        s.parse(&synth(np, &CODES)).unwrap();
        s
    }

    /// A fix `east` m east of the segment's start and `north` m north, degrees x 1e-7.
    pub(crate) fn at(east: f64, north: f64) -> (i32, i32) {
        ((north / 0.011132).round() as i32, 100_000_000 + (east / 0.011132).round() as i32)
    }

    /// Runs `fixes` (one a second from t = 0) through the matcher the way S3 will; the events, in order.
    pub(crate) fn drive(m: &mut Matcher, s: &Segment, fixes: &[(i32, i32)]) -> Vec<Event> {
        let mut out = Vec::new();
        for (t, &(lat, lon)) in fixes.iter().enumerate() {
            let t = t as u32;
            let ev = if m.active_id().is_some() {
                m.step_active(s, lat, lon, t)
            } else {
                m.step_idle([&s.header], lat, lon, t)
            };
            out.extend(ev);
        }
        out
    }

    /// 5 m/s east from `from` m to `to` m, level.
    fn line(from: f64, to: f64, speed: f64) -> Vec<(i32, i32)> {
        let n = ((to - from) / speed) as usize;
        (0..=n).map(|k| at(from + speed * k as f64, 0.0)).collect()
    }

    fn finish(ev: &[Event]) -> Option<(u32, [Option<i32>; KEPT])> {
        ev.iter().find_map(|e| match e {
            Event::Finish { elapsed_s, deltas, .. } => Some((*elapsed_s, *deltas)),
            _ => None,
        })
    }

    const ID: u32 = 4_000_000_001;

    #[test]
    fn clean_pass_against_itself() {
        let s = seg(20); // 950 m
        let ev = drive(&mut Matcher::new(), &s, &line(-250.0, 1000.0, 5.0));
        assert_eq!(ev.len(), 3, "{ev:?}");
        assert_eq!((ev[0], ev[1]), (Event::Approach(ID), Event::Start(ID)));
        let (el, d) = finish(&ev).unwrap();
        assert!((189..=191).contains(&el), "{el}");
        // column 0 is the PR (10 s per point: 190 s): ahead by the target's own offset, 0 for the PR
        assert!(d[0].unwrap().abs() <= 1, "{d:?}");
        assert!(d[1].unwrap() < 0 && d[2].unwrap() < d[1].unwrap() + 1 && d[3].is_some(), "{d:?}");
    }

    #[test]
    fn start_missed_by_30_m() {
        let s = seg(20);
        let fixes: Vec<_> = line(-250.0, 1000.0, 5.0).iter().map(|&(_, lon)| (at(0.0, 30.0).0, lon)).collect();
        let ev = drive(&mut Matcher::new(), &s, &fixes);
        assert!(!ev.iter().any(|e| matches!(e, Event::Start(_) | Event::Finish { .. })), "{ev:?}");
        // and wrong way round at the start line is no start either
        let back: Vec<_> = line(-250.0, 100.0, 5.0).into_iter().rev().collect();
        assert!(!drive(&mut Matcher::new(), &s, &back).iter().any(|e| matches!(e, Event::Start(_))));
    }

    #[test]
    fn u_turn_is_lost() {
        let s = seg(20);
        let mut f = line(-50.0, 400.0, 5.0);
        f.extend(line(0.0, 400.0, 5.0).iter().rev().map(|&(a, b)| (a, b)));
        let ev = drive(&mut Matcher::new(), &s, &f);
        assert_eq!(ev.last(), Some(&Event::Lost(ID)), "{ev:?}");
        assert!(finish(&ev).is_none());
    }

    #[test]
    fn stop_over_5_minutes_is_lost() {
        let s = seg(20);
        let mut f = line(-50.0, 250.0, 5.0);
        let stop = *f.last().unwrap();
        f.extend(std::iter::repeat_n(stop, 320));
        let ev = drive(&mut Matcher::new(), &s, &f);
        assert_eq!(ev.last(), Some(&Event::Lost(ID)), "{ev:?}");
        // a short stop is not
        let mut f = line(-50.0, 250.0, 5.0);
        f.extend(std::iter::repeat_n(stop, 200));
        f.extend(line(255.0, 1000.0, 5.0));
        assert!(finish(&drive(&mut Matcher::new(), &s, &f)).is_some());
    }

    #[test]
    fn lost_and_found() {
        let s = seg(20);
        let mut f = line(-50.0, 300.0, 5.0);
        f.extend((1..=6).map(|k| at(300.0 + 5.0 * k as f64, 80.0))); // 6 s, 80 m off
        f.extend(line(335.0, 1000.0, 5.0));
        let ev = drive(&mut Matcher::new(), &s, &f);
        assert!(finish(&ev).is_some() && !ev.contains(&Event::Lost(ID)), "{ev:?}");
        // 12 s off loses it
        let mut f = line(-50.0, 300.0, 5.0);
        f.extend((1..=12).map(|k| at(300.0 + 5.0 * k as f64, 80.0)));
        f.extend(line(360.0, 1000.0, 5.0));
        let ev = drive(&mut Matcher::new(), &s, &f);
        assert!(ev.contains(&Event::Lost(ID)) && finish(&ev).is_none(), "{ev:?}");
    }

    #[test]
    fn first_started_wins() {
        let a = seg(20);
        let mut b = seg(20);
        b.header.id = 7;
        let mut m = Matcher::new();
        let mut ev = Vec::new();
        for (t, &(lat, lon)) in line(-250.0, 100.0, 5.0).iter().enumerate() {
            ev.extend(m.step_idle([&a.header, &b.header], lat, lon, t as u32));
        }
        assert_eq!(ev, [Event::Approach(ID), Event::Start(ID)]);
        assert_eq!(m.active_id(), Some(ID));
        // a segment that is not the one loaded drops it
        assert_eq!(m.step_active(&b, 0, 100_000_000, 99), Some(Event::Lost(ID)));
        assert_eq!(m.active_id(), None);
    }

    #[test]
    fn along_a_course_both_follow() {
        let s = seg(20);
        let mut c = Course::new();
        for i in 0..40 {
            let (lat, lon) = at(-500.0 + 50.0 * f64::from(i), 0.0);
            c.push(lat, lon, None);
        }
        c.finish();
        let mut t = Tracker::new();
        let mut m = Matcher::new();
        let mut ev = Vec::new();
        for (k, &(lat, lon)) in line(-400.0, 1000.0, 5.0).iter().enumerate() {
            c.follow(&mut t, lat, lon);
            let k = k as u32;
            ev.extend(if m.active_id().is_some() {
                m.step_active(&s, lat, lon, k)
            } else {
                m.step_idle([&s.header], lat, lon, k)
            });
        }
        assert!(finish(&ev).is_some(), "{ev:?}");
        assert!(t.along_m > 1300 && t.off_m.unwrap() < 5, "{} {:?}", t.along_m, t.off_m);
    }

    #[test]
    fn gnss_noise_does_not_lose_it() {
        let s = seg(20);
        let mut x = 12345u32;
        let mut noise = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            f64::from(x % 2001) / 100.0 - 10.0
        };
        let f: Vec<_> = (0..=250).map(|k| at(-250.0 + 5.0 * f64::from(k) + noise() * 0.3, noise())).collect();
        let ev = drive(&mut Matcher::new(), &s, &f);
        assert!(!ev.contains(&Event::Lost(ID)), "{ev:?}");
        // noisy 5 m/s east: the start and the finish are found, the time within a few seconds
        let (el, _) = finish(&ev).expect("finish");
        assert!((184..=196).contains(&el), "{el}");
    }

    #[test]
    fn ghost_sign() {
        let s = seg(20);
        let fast = finish(&drive(&mut Matcher::new(), &s, &line(-250.0, 1000.0, 6.0))).unwrap();
        let slow = finish(&drive(&mut Matcher::new(), &s, &line(-250.0, 1000.0, 4.0))).unwrap();
        assert!(fast.1[0].unwrap() < -20, "{fast:?}"); // about 158 s against 190
        assert!(slow.1[0].unwrap() > 20, "{slow:?}"); // about 238 s
        // mid-segment: 475 m at 150 s against the PR's 95 s... and the time of a distance
        assert_eq!(target_s(&s, 0, 475), Some(95));
        assert_eq!(delta_s(&s, 0, 475, 100), Some(5));
        assert_eq!(delta_s(&s, 0, 475, 90), Some(-5));
        assert_eq!(target_s(&s, 0, 100_000), Some(190));
        assert_eq!(target_s(&s, 9, 10), None);
    }

    #[test]
    fn size_and_reset() {
        assert!(core::mem::size_of::<Matcher>() <= 100, "{}", core::mem::size_of::<Matcher>());
        let mut m = Matcher::new();
        let s = seg(20);
        drive(&mut m, &s, &line(-250.0, 100.0, 5.0));
        assert!(m.active_id().is_some() && m.along_m().is_some());
        m.reset();
        assert_eq!((m.active_id(), m.elapsed_s(5)), (None, None));
    }
}
