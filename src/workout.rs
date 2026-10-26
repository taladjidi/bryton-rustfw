//! The workout player: which step of a planned workout ([`crate::storage::workout`]) is running and how much of
//! it is left. Pure logic, fed one second at a time: the step's clock moves only when the ride's timer moved (a
//! pause or an auto-pause stops both), a distance step ends on the ride's distance, an open step on Lap
//! ([`Player::skip`]). Each change comes out as an [`Event`] for the app to show and sound: a step started, the
//! last 2 seconds of a timed step (beep, beep, then the step's longer beep: the owner's choice), the workout done.

use crate::storage::workout::{Kind, Length, Step};
use crate::{ride, ui};

/// Seconds before a timed step ends that each beep (2, 1), then the next step's longer beep.
pub const COUNTDOWN_S: u32 = 2;
/// An open step's width on the workout graph, s: it lasts until Lap, so its length is unknown.
// ponytail: a fixed guess; the time actually spent in it would need the graph to grow as it goes
pub const OPEN_S: u32 = 300;
/// The speed a distance step is drawn at on the workout graph, m/s (about 30 km/h).
const GRAPH_MPS: u32 = 8;

/// What changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Step `n` (from 0) started.
    Step(usize),
    /// A timed step has this many seconds left (2, 1).
    Countdown(u32),
    /// The last step ended.
    Done,
}

/// What is left of the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Seconds.
    Time(u32),
    /// Metres.
    Distance(u32),
    /// Until Lap.
    Lap,
    /// The workout is done.
    Done,
}

/// Where a workout is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    /// The step running (past the last: done).
    step: usize,
    /// Timer seconds into it.
    elapsed_s: u32,
    /// The ride's distance when it started, m.
    from_m: u32,
}

impl Player {
    /// Starts at step `at` of `steps`, with the ride at `dist_m`: the player and its first event.
    pub fn start(steps: &[Step], at: usize, dist_m: u32) -> (Player, Event) {
        let p = Player { step: at, elapsed_s: 0, from_m: dist_m };
        (p, if at < steps.len() { Event::Step(at) } else { Event::Done })
    }

    /// The step running; None once done.
    pub fn step(&self, steps: &[Step]) -> Option<usize> {
        (self.step < steps.len()).then_some(self.step)
    }

    /// One second: `ticked` if the ride's timer moved, the ride now at `dist_m`.
    pub fn second(&mut self, steps: &[Step], ticked: bool, dist_m: u32) -> Option<Event> {
        let s = steps.get(self.step)?;
        if !ticked {
            return None;
        }
        self.elapsed_s = self.elapsed_s.saturating_add(1);
        match s.len {
            Length::Time(t) if self.elapsed_s >= t => Some(self.skip(steps, dist_m)),
            Length::Time(t) if t - self.elapsed_s <= COUNTDOWN_S => Some(Event::Countdown(t - self.elapsed_s)),
            Length::Distance(m) if dist_m.saturating_sub(self.from_m) >= m => Some(self.skip(steps, dist_m)),
            _ => None,
        }
    }

    /// The next step (Lap, or the step's end), the ride at `dist_m`.
    pub fn skip(&mut self, steps: &[Step], dist_m: u32) -> Event {
        self.step = (self.step + 1).min(steps.len());
        (self.elapsed_s, self.from_m) = (0, dist_m);
        if self.step < steps.len() { Event::Step(self.step) } else { Event::Done }
    }

    /// Where the rider is on the workout graph, s from its start ([`graph_s`]): the steps done and the part of the
    /// step running (an open step's no further than its end); the whole graph once done.
    pub fn graph_at(&self, steps: &[Step], dist_m: u32) -> u32 {
        let done = steps.get(..self.step).unwrap_or(steps).iter().map(graph_s).fold(0, u32::saturating_add);
        let part = steps.get(self.step).map_or(0, |s| {
            let p = match s.len {
                Length::Distance(_) => dist_m.saturating_sub(self.from_m) / GRAPH_MPS,
                _ => self.elapsed_s,
            };
            p.min(graph_s(s) - 1)
        });
        done.saturating_add(part)
    }

    /// What is left of the step, the ride at `dist_m`.
    pub fn left(&self, steps: &[Step], dist_m: u32) -> Left {
        match steps.get(self.step).map(|s| s.len) {
            None => Left::Done,
            Some(Length::Time(t)) => Left::Time(t.saturating_sub(self.elapsed_s)),
            Some(Length::Distance(m)) => Left::Distance(m.saturating_sub(dist_m.saturating_sub(self.from_m))),
            Some(Length::Open) => Left::Lap,
        }
    }
}

/// A step's width on the workout graph, s: its time, a distance at about 30 km/h, an open step [`OPEN_S`]; 1 s at
/// least.
pub fn graph_s(s: &Step) -> u32 {
    match s.len {
        Length::Time(t) => t,
        Length::Distance(m) => m / GRAPH_MPS,
        Length::Open => OPEN_S,
    }
    .max(1)
}

/// The workout graph's span, s.
pub fn graph_total_s(steps: &[Step]) -> u32 {
    steps.iter().map(graph_s).fold(0, u32::saturating_add)
}

/// A step's level on the workout graph (per mille of its height) and its colour, for this FTP and max heart rate:
/// power from 0 to 150 % of FTP (zone 7 at the top), heart rate from half the max to it, in their zone colours; a
/// step without a power or heart-rate target a quarter high, grey when open, white for cadence (no zones).
pub fn level(s: &Step, ftp: u16, max_hr: u16) -> (u32, u16) {
    let per_mille = |v: u32, lo: u32, hi: u32| v.saturating_sub(lo) * 1000 / hi.saturating_sub(lo).max(1);
    let (ftp32, max32) = (u32::from(ftp), u32::from(max_hr));
    match (s.target.kind, s.target.range(ftp, max_hr)) {
        (Kind::Power, Some((lo, hi))) => {
            let mid = lo.midpoint(hi);
            (per_mille(mid.into(), 0, ftp32 * 3 / 2), ride::power_color(ride::power_zone(mid, ftp)))
        }
        (Kind::Hr, Some((lo, hi))) => {
            let mid = lo.midpoint(hi);
            (per_mille(mid.into(), max32 / 2, max32), ride::hr_color(ride::hr_zone(mid, max_hr)))
        }
        (Kind::Open, _) => (250, ui::GREY),
        _ => (250, ui::WHITE),
    }
}

/// Column `k` of a workout graph `cols` wide and `h` high over the whole of `steps` (zones from `ftp`, `max_hr`):
/// its bar's height and colour ([`level`]); the rider's column (`rider`) white, the part done dark. `rider` is
/// `cols` once the workout is done.
///
/// ```
/// use bryton_rustfw::storage::workout::{Kind, Length, Step, Target};
/// use bryton_rustfw::{ride, ui, workout::column};
/// let target = |pct| Target { kind: Kind::Power, lo: pct, hi: pct, pct: true };
/// let steps = [50, 160].map(|pct| Step { len: Length::Time(60), target: target(pct), intensity: 0 });
/// assert_eq!(column(&steps, (200, 190), (4, 100), Some(1), 3), (100, ride::ZONE_COLORS[6]), "160 %: the top, Z7");
/// assert_eq!(column(&steps, (200, 190), (4, 100), Some(1), 1).1, ui::WHITE, "the rider");
/// assert_eq!(column(&steps, (200, 190), (4, 100), Some(1), 0), (33, ui::DARK), "done");
/// ```
pub fn column(
    steps: &[Step],
    (ftp, max_hr): (u16, u16),
    (cols, h): (u32, u32),
    rider: Option<u32>,
    k: u32,
) -> (u32, u16) {
    if rider == Some(k) {
        return (h, ui::WHITE);
    }
    let total = graph_total_s(steps);
    let t = (u64::from(k) * u64::from(total) / u64::from(cols.max(1))) as u32;
    let mut end = 0u32;
    let Some(s) = steps.iter().find(|s| {
        end = end.saturating_add(graph_s(s));
        t < end
    }) else {
        return (0, ui::BLACK);
    };
    let (level, color) = level(s, ftp, max_hr);
    let height = (level * h / 1000).clamp(1, h.max(1));
    (height, if rider.is_some_and(|r| k < r) { ui::DARK } else { color })
}

/// What the workout gauge field measures and its target band, during step `s` (None: no workout running), for this
/// FTP and max heart rate: the step's own kind and range; power without a band for an open step or no workout.
pub fn gauge_target(s: Option<&Step>, ftp: u16, max_hr: u16) -> (Kind, Option<(u16, u16)>) {
    match s.map(|s| (s.target.kind, s.target.range(ftp, max_hr))) {
        Some((kind, Some((lo, hi)))) if kind != Kind::Open => (kind, Some((lo.min(hi), lo.max(hi)))),
        _ => (Kind::Power, None),
    }
}

/// The gauge's scale for a band (`lo`, `hi`) of `kind`: every zone, so the band shows where it sits among them (the
/// owner's ask, 2026-10-26). Power from 40 % to 165 % of `ftp` (Coggan's Z1 under 56 %, Z7 over 150 %), heart rate
/// from 50 % to 100 % of `max_hr`, each widened to keep 10 % around a band that falls outside; cadence (no zones) 30 %
/// under `lo` to 30 % over `hi`. Fixed for a step: the zones never slide.
///
/// ```
/// use bryton_rustfw::{storage::workout::Kind, workout::gauge_scale};
/// assert_eq!(gauge_scale(Kind::Power, (200, 220), 250, 190), (100, 412));
/// assert_eq!(gauge_scale(Kind::Power, (400, 420), 250, 190), (100, 462), "widened for a band off the zones");
/// assert_eq!(gauge_scale(Kind::Hr, (150, 160), 250, 190), (95, 190));
/// assert_eq!(gauge_scale(Kind::Cadence, (150, 160), 250, 190), (105, 208));
/// ```
pub fn gauge_scale(kind: Kind, (lo, hi): (u16, u16), ftp: u16, max_hr: u16) -> (u32, u32) {
    let (lo, hi) = (u32::from(lo), u32::from(hi));
    let (a, b) = match kind {
        Kind::Power => (u32::from(ftp) * 40 / 100, u32::from(ftp) * 165 / 100),
        Kind::Hr => (u32::from(max_hr) / 2, u32::from(max_hr)),
        _ => (lo * 7 / 10, hi * 13 / 10),
    };
    let a = a.min(lo * 9 / 10);
    (a, b.max(hi * 11 / 10).max(a + 1))
}

/// The column of reading `v` on a gauge `cols` wide over `scale` ([`gauge_scale`]), held at its ends.
///
/// ```
/// use bryton_rustfw::workout::gauge_x;
/// assert_eq!([0, 140, 200, 220, 286, 999].map(|w| gauge_x(w, (140, 286), 146)), [0, 0, 60, 80, 145, 145]);
/// ```
pub fn gauge_x(v: u16, (a, b): (u32, u32), cols: u32) -> u32 {
    let x = u64::from(u32::from(v).saturating_sub(a)) * u64::from(cols) / u64::from(b.saturating_sub(a).max(1));
    (x.min(u64::from(cols.saturating_sub(1)))) as u32
}

/// The zones a gauge shows behind its band: for each of up to seven zones, the column where it starts on a gauge
/// `cols` wide around `band` ([`gauge_scale`]) and its colour; a zone above the scale starts at `u32::MAX`, one below
/// it at column 0 (the last zone starting at or before a column is that column's). Power gets Coggan's seven zones of
/// `ftp`, heart rate the five of `max_hr` ([`ride::power_zone`], [`ride::hr_zone`]: the same edges, so the marker's
/// column is in the zone the reading is in), cadence one grey. Fixed for a step: the scale does not slide.
///
/// ```
/// use bryton_rustfw::{ride::ZONE_COLORS as Z, storage::workout::Kind, workout::gauge_zones};
/// // 200-220 W, FTP 200: the scale is 80 to 330 W on 250 columns, a watt a column
/// let z = gauge_zones(Kind::Power, (200, 220), 250, 200, 190);
/// assert_eq!(z[1], (32, Z[1]), "zone 2 from 112 W");
/// assert_eq!(z[2], (72, Z[2]), "zone 3 from 152 W");
/// assert_eq!(z[4], (132, Z[4]), "zone 5 from 212 W");
/// assert_eq!(z[6], (222, Z[6]), "zone 7 from 302 W, on the scale too");
/// ```
pub fn gauge_zones(kind: Kind, band: (u16, u16), cols: u32, ftp: u16, max_hr: u16) -> [(u32, u16); 7] {
    let (a, b) = gauge_scale(kind, band, ftp, max_hr);
    let (pcts, base): (&[u32], u32) = match kind {
        Kind::Power => (&[56, 76, 91, 106, 121, 151], u32::from(ftp.max(1))),
        Kind::Hr => (&[60, 70, 80, 90], u32::from(max_hr.max(1))),
        _ => (&[], 1),
    };
    let col_of = |w: u32| match w {
        w if w <= a => 0,
        w if w >= b => u32::MAX,
        w => (u64::from(w - a) * u64::from(cols) / u64::from(b - a)) as u32,
    };
    let mut z = [(u32::MAX, ui::DARK); 7];
    z[0] = (0, if pcts.is_empty() { ui::DARK } else { zone_color(kind, 1) });
    for (k, pct) in pcts.iter().enumerate() {
        z[k + 1] = (col_of((pct * base).div_ceil(100)), zone_color(kind, k as u8 + 2));
    }
    z
}

/// Zone `z` of a reading's kind as a colour.
fn zone_color(kind: Kind, z: u8) -> u16 {
    if kind == Kind::Hr { ride::hr_color(z) } else { ride::power_color(z) }
}

/// How far a reading `v` is from `band`, as a colour: the zone palette's green in it, its yellow within 5 % of it
/// (nearly there), its blue further under and its red further over. The same colours the zones give easy and hard
/// efforts ([`ride::ZONE_COLORS`]), so the gauge reads as the power and heart-rate fields do.
///
/// ```
/// use bryton_rustfw::{ride::ZONE_COLORS as Z, workout::gauge_color};
/// let c = [180, 195, 200, 220, 230, 240].map(|w| gauge_color(w, (200, 220)));
/// assert_eq!(c, [Z[0], Z[2], Z[1], Z[1], Z[2], Z[4]]);
/// ```
pub fn gauge_color(v: u16, (lo, hi): (u16, u16)) -> u16 {
    let (v, lo, hi) = (u32::from(v), u32::from(lo), u32::from(hi));
    if v < lo * 95 / 100 {
        ride::ZONE_COLORS[0]
    } else if v < lo {
        ride::ZONE_COLORS[2]
    } else if v <= hi {
        ride::ZONE_COLORS[1]
    } else if v <= hi * 105 / 100 {
        ride::ZONE_COLORS[2]
    } else {
        ride::ZONE_COLORS[4]
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::storage::workout::Target;
    use std::vec::Vec;

    fn st(len: Length) -> Step {
        Step { len, target: Target::OPEN, intensity: 0 }
    }

    /// Runs `secs` seconds, the timer moving when `moving(s)`, 5 m a moving second: the events with their second.
    fn run(p: &mut Player, steps: &[Step], secs: u32, moving: impl Fn(u32) -> bool) -> Vec<(u32, Event)> {
        let mut dist = 0;
        (1..=secs)
            .filter_map(|s| {
                dist += if moving(s) { 5 } else { 0 };
                p.second(steps, moving(s), dist).map(|e| (s, e))
            })
            .collect()
    }

    #[test]
    fn timed_steps_count_down_and_follow() {
        let steps = [st(Length::Time(5)), st(Length::Time(2)), st(Length::Time(4))];
        let (mut p, first) = Player::start(&steps, 0, 0);
        assert_eq!(first, Event::Step(0));
        let ev = run(&mut p, &steps, 12, |_| true);
        use Event::*;
        // 5 s: beeps at 2 and 1 left, then the next; a 2 s step only has 1 left to beep; the last one ends the workout
        let want = [(3, Countdown(2)), (4, Countdown(1)), (5, Step(1))];
        assert_eq!(ev[..3], want);
        assert_eq!(ev[3..], [(6, Countdown(1)), (7, Step(2)), (9, Countdown(2)), (10, Countdown(1)), (11, Done)]);
        assert_eq!((p.step(&steps), p.left(&steps, 0)), (None, Left::Done));
        assert_eq!(p.second(&steps, true, 0), None, "done stays done");
    }

    #[test]
    fn pauses_with_the_ride_timer() {
        let steps = [st(Length::Time(10)), st(Length::Time(10))];
        let (mut p, _) = Player::start(&steps, 0, 0);
        // stopped for seconds 4 to 8: the step ends 5 s late
        let ev = run(&mut p, &steps, 20, |s| !(4..=8).contains(&s));
        assert_eq!(ev.iter().find(|e| e.1 == Event::Step(1)).map(|e| e.0), Some(15));
        assert_eq!(p.left(&steps, 0), Left::Time(5));
    }

    #[test]
    fn open_steps_end_on_lap_distance_on_the_distance() {
        let steps = [st(Length::Open), st(Length::Distance(20)), st(Length::Time(3))];
        let (mut p, _) = Player::start(&steps, 0, 100);
        assert!(run(&mut p, &steps, 30, |_| true).is_empty(), "an open step waits for Lap");
        assert_eq!(p.left(&steps, 0), Left::Lap);
        assert_eq!(p.skip(&steps, 100), Event::Step(1));
        assert_eq!(p.left(&steps, 110), Left::Distance(10));
        let ev: Vec<_> = [105, 110, 115, 120].into_iter().filter_map(|d| p.second(&steps, true, d)).collect();
        assert_eq!(ev, [Event::Step(2)], "20 m from 100 m: at 120");
        assert_eq!(p.left(&steps, 120), Left::Time(3));
    }

    #[test]
    fn lap_skips_and_the_last_step_ends_it() {
        let steps = [st(Length::Time(60)), st(Length::Time(60))];
        let (mut p, _) = Player::start(&steps, 1, 0);
        assert_eq!(p.step(&steps), Some(1), "started from a later step");
        assert_eq!(p.skip(&steps, 0), Event::Done);
        assert_eq!(p.skip(&steps, 0), Event::Done, "and stays so");
        assert_eq!(Player::start(&steps, 5, 0).1, Event::Done);
        assert_eq!(Player::start(&[], 0, 0).1, Event::Done);
    }

    #[test]
    fn graph_columns_and_the_riders_place() {
        let t = |kind, lo, hi| Target { kind, lo, hi, pct: true };
        let steps = [
            Step { len: Length::Time(300), target: t(Kind::Power, 60, 70), intensity: 2 },
            Step { len: Length::Distance(2400), target: t(Kind::Hr, 85, 95), intensity: 0 },
            Step { len: Length::Open, target: Target::OPEN, intensity: 3 },
        ];
        // 300 s, 2400 m at 8 m/s: 300 s, open: 300 s; 9 columns of 100 s each
        assert_eq!(graph_total_s(&steps), 900);
        let col = |rider, k| column(&steps, (200, 180), (9, 60), rider, k);
        let bars: Vec<_> = (0..9).map(|k| col(None, k)).collect();
        let z2 = ride::power_color(2); // 130 W of 200
        let hr = ride::hr_color(ride::hr_zone(162, 180));
        assert_eq!(bars[..3], [(25, z2); 3], "130 W of 0 to 300 W: 60 x 0.433");
        assert_eq!(bars[3..6], [(48, hr); 3], "162 bpm between 90 and 180: 60 x 0.8");
        assert_eq!(bars[6..], [(15, ui::GREY); 3], "open: a quarter");
        assert_eq!(col(None, 9), (0, ui::BLACK), "past the end");
        assert_eq!((col(Some(4), 3), col(Some(4), 4), col(Some(4), 5)), ((48, ui::DARK), (60, ui::WHITE), (48, hr)));
        assert!((0..9).all(|k| col(Some(9), k).1 == ui::DARK), "done: all of it dark, no marker");

        let (mut p, _) = Player::start(&steps, 0, 1000);
        run(&mut p, &steps, 150, |_| true);
        assert_eq!(p.graph_at(&steps, 1000), 150);
        p.skip(&steps, 1000);
        assert_eq!(p.graph_at(&steps, 1800), 400, "800 m into the distance step: 100 s");
        p.skip(&steps, 3400);
        run(&mut p, &steps, 1000, |_| true);
        assert_eq!(p.graph_at(&steps, 0), 899, "an open step held to its end");
        p.skip(&steps, 0);
        assert_eq!(p.graph_at(&steps, 0), 900, "done");
    }

    /// The zones behind the gauge follow the FTP (and the maximum heart rate): every watt of the scale is in the zone
    /// `ride::power_zone` gives it, whatever the FTP, and every zone is on it.
    #[test]
    fn gauge_zones_follow_ftp_and_max_hr() {
        let band = (200, 220);
        for ftp in [120, 200, 250, 310] {
            let (a, b) = gauge_scale(Kind::Power, band, ftp, 190);
            let cols = (b - a) * 2; // a watt is two columns: no two edges share one
            let z = gauge_zones(Kind::Power, band, cols, ftp, 190);
            assert!(z.iter().all(|z| z.0 != u32::MAX), "all seven zones on the scale at FTP {ftp}");
            for v in a..b {
                let col = gauge_x(v as u16, (a, b), cols);
                let zone = z.iter().rev().find(|z| z.0 <= col).unwrap().1;
                assert_eq!(zone, ride::power_color(ride::power_zone(v as u16, ftp)), "{v} W at FTP {ftp}");
            }
        }
        for max in [170, 190] {
            let (a, b) = gauge_scale(Kind::Hr, (150, 160), 200, max);
            let z = gauge_zones(Kind::Hr, (150, 160), (b - a) * 2, 200, max);
            for v in a..b {
                let col = gauge_x(v as u16, (a, b), (b - a) * 2);
                let zone = z.iter().rev().find(|z| z.0 <= col).unwrap().1;
                assert_eq!(zone, ride::hr_color(ride::hr_zone(v as u16, max)), "{v} bpm at {max}");
            }
            assert_eq!(z[5].0, u32::MAX, "only five heart-rate zones");
        }
        assert_eq!(gauge_zones(Kind::Cadence, (85, 95), 100, 200, 190)[1].0, u32::MAX, "cadence: one grey");
    }

    #[test]
    fn gauge_follows_the_steps_target() {
        let t = |kind, lo, hi, pct| Target { kind, lo, hi, pct };
        let step = |target| Step { len: Length::Time(60), target, intensity: 0 };
        let (power, hr) = (step(t(Kind::Power, 100, 110, true)), step(t(Kind::Hr, 150, 160, false)));
        assert_eq!(gauge_target(Some(&power), 200, 190), (Kind::Power, Some((200, 220))), "% of FTP to W");
        assert_eq!(gauge_target(Some(&hr), 200, 190), (Kind::Hr, Some((150, 160))), "a heart-rate step: bpm");
        assert_eq!(gauge_target(Some(&step(Target::OPEN)), 200, 190), (Kind::Power, None), "open: power, no band");
        assert_eq!(gauge_target(None, 200, 190), (Kind::Power, None), "no workout");
        let swapped = step(t(Kind::Power, 220, 200, false));
        assert_eq!(gauge_target(Some(&swapped), 200, 190).1, Some((200, 220)), "a band written high to low");

        // cadence 150-160: 105 to 208 over 103 columns, a column a unit
        assert_eq!(gauge_scale(Kind::Cadence, (150, 160), 200, 190), (105, 208));
        assert_eq!([100, 150, 160, 250].map(|v| gauge_x(v, (105, 208), 103)), [0, 45, 55, 102], "held at the ends");
        let c = [140, 145, 155, 167, 170].map(|v| gauge_color(v, (150, 160)));
        let z = ride::ZONE_COLORS;
        assert_eq!(c, [z[0], z[2], z[1], z[2], z[4]], "5 % off: 142 and 168 bpm");
        assert_eq!(gauge_x(5, gauge_scale(Kind::Cadence, (0, 0), 0, 0), 50), 49, "a band of 0: no division by zero");
        assert_eq!(gauge_x(0, (140, 286), 0), 0, "no columns");
    }
}
