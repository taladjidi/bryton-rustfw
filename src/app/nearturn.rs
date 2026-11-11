//! MAP AT TURN: following a course on a page with no map, the map page comes up by itself when a turn is within
//! [`NEAR_M`], stays until the turn is passed and [`HOLD_MS`] more, then the rider's page comes back (UI research,
//! pattern 5: Wahoo does not open the map, Garmin users ask for it). Two full page draws, one at each switch.

use crate::*;

/// A turn this close (m) brings the map.
const NEAR_M: u32 = 150;
/// The map stays this long after the turn is passed, ms.
const HOLD_MS: u32 = 4000;

/// The map page opened for a turn.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NearTurn {
    /// The page to go back to while the map page is up for a turn.
    pub(crate) back: Option<usize>,
    /// The turn (its place on the course, m) the map was opened for: once is enough, a rider who pages away is left be.
    cue: Option<u32>,
    /// The map page the turn opened: another map page the rider pages to is paging away.
    shown: Option<usize>,
    /// Since the turn was passed (or lost), ms.
    hold_ms: u32,
}

impl NearTurn {
    /// No map opened.
    pub(crate) const fn new() -> Self {
        NearTurn { back: None, cue: None, shown: None, hold_ms: 0 }
    }
}

impl App {
    /// Whether ride page `n` holds a map field.
    fn page_has_map(&self, n: usize) -> bool {
        let p = match n {
            COURSE => page::Page::from_cfg(COURSE_PAGE),
            CLIMB | WORKOUT => return false,
            n => self.ride_page_ref(n).copied(),
        };
        p.is_some_and(|p| p.fields.iter().take(p.cells()).any(|&f| f == Field::Map))
    }

    /// Once a pass: opens the map page near a turn, and gives the rider's page back after it. Never from a menu or
    /// another screen, the workout or climb page, a page with a MORE variant up, nor under the workout step's
    /// countdown in the strip (a turn within 100 m outranks it there anyway).
    pub(crate) fn settle_near_turn(&mut self, b: &mut impl Board, dt: u32) {
        if self.ride.state() == ride::State::Idle {
            self.near = NearTurn::new(); // a new ride: its turns are new
        }
        // the hold follows the turn even while the rider is stopped at the junction; only opening needs a running ride
        let near = self.on_course().and_then(|(c, t)| c.next_cue(t)).map(|(cue, d)| (cue.at_m, d));
        let may_open = self.settings.map_turn && self.ride.state() == ride::State::Running;
        let Screen::Ride(n) = self.screen else {
            self.near.back = None; // a menu or the overview: the rider went elsewhere
            return;
        };
        if let Some(back) = self.near.back {
            if !self.page_has_map(n) || n == back || Some(n) != self.near.shown {
                self.near.back = None; // paged away by the rider
            } else if near.is_some_and(|(_, d)| d <= NEAR_M * 4 / 3) {
                self.near.hold_ms = 0;
            } else {
                self.near.hold_ms = self.near.hold_ms.saturating_add(dt);
                if self.near.hold_ms >= HOLD_MS {
                    self.near.back = None;
                    self.screen = if self.page_on(back) { Screen::Ride(back) } else { self.ride_page(0, 1) };
                    self.show(b);
                }
            }
            return;
        }
        let Some((at, _)) = near.filter(|&(at, d)| may_open && d <= NEAR_M && self.near.cue != Some(at)) else {
            return;
        };
        let step = matches!(self.plan.top, Some(a) if a.kind == super::workouts::AheadKind::Step);
        if n == WORKOUT || n == CLIMB || self.page_has_map(n) || self.more_on.is_some() || self.hint_shown || step {
            return;
        }
        let target =
            (1..ROUND).map(|k| page_at((place(n) + k) % ROUND)).find(|&m| self.page_on(m) && self.page_has_map(m));
        if let Some(m) = target {
            (self.near.back, self.near.cue, self.near.shown, self.near.hold_ms) = (Some(n), Some(at), Some(m), 0);
            self.screen = Screen::Ride(m);
            self.show(b);
        }
    }
}
