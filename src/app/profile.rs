//! The ride profile ([`ride::Profile`]): ROAD, INDOOR or MTB, chosen when the ride (or a workout) starts from the
//! trainer heard, fixed by hand (BIKE > PROFILE, the ride menu's INDOOR row), and given up again if the trainer goes
//! silent. BIKE > PROFILE MTB is the outdoor choice that stays until changed; a trainer heard still makes the ride
//! INDOOR while it holds (the owner, 2026-10-27), and the ride goes back to MTB when it goes silent. What it
//! changes: the round skips the map and next-turn pages indoors, the GNSS receiver is off during an indoor ride and
//! tracks continuously during an MTB one (gps.rs), the auto-pause (ride.rs), and the ride file's sub_sport
//! (indoor_cycling, mountain or road, fit.rs), which the recovery record keeps with "by hand", so a ride resumed
//! after a reset comes back with its profile, and one set by hand stays so (riding.rs).
//!
//! **Pages per profile** (UI review 3, 2.3): an INDOOR ride takes its pages from the indoor set
//! ([`page::Set::indoor_default`]: the trainer, the power numbers, the graphs), an MTB ride from the MTB set
//! ([`page::Set::mtb_default`]: big fields, at most [`page::MTB_PAGES`] pages), a ROAD ride from the road set; the
//! overview is shared. The editor and PAGES work on one set ([`App::view_set`]): the profile's while a ride is
//! open, else the one PAGES > ROAD, INDOOR or MTB PAGES opened. The road set is the one the backup registers keep
//! (three pages); the indoor and MTB sets live in `\Rustfw\pages.bin` only (page.rs), so a change to them made
//! while the file must wait is lost to a switch-off.

use crate::*;

/// Seconds without the trainer, indoor and automatic, before the ride goes back to ROAD: the one costly mistake is
/// an outdoor ride recorded as indoor because the unit sat near a powered trainer.
pub const TRAINER_QUIET_S: u16 = 120;

/// The climb the CLIMB page shows and where the rider is on the trace or course.
#[derive(Clone, Copy, Debug)]
pub struct ClimbNow {
    pub(crate) climb: course::Climb,
    pub(crate) along: u32,
    /// Of the followed course (else the road ahead's).
    pub(crate) course: bool,
    /// The live segment being ridden stands for the climb: its id.
    pub(crate) seg: Option<u32>,
    /// The segment's target now, m along it.
    pub(crate) ghost: Option<u32>,
}

impl App {
    /// The set PAGES and the editor show: the profile's while a ride is open, else the one chosen in PAGES.
    pub(crate) fn view_set(&self) -> ride::Profile {
        if self.ride.state() == ride::State::Idle { self.pages_set } else { self.profile }
    }

    /// A set's pages, how many are used, and their MORE variants: the profile's (ROAD, INDOOR or MTB).
    pub(crate) fn set_ref(&self, p: ride::Profile) -> (&[page::Page], usize, &[Option<page::Page>; PAGES]) {
        match p {
            ride::Profile::Indoor => (&self.indoor.pages, self.indoor.count, &self.indoor.more),
            ride::Profile::Mtb => (&self.mtb.pages, self.mtb.count, &self.mtb.more),
            ride::Profile::Road => (self.pages.get(..PAGES).unwrap_or(&[]), self.page_count, &self.more),
        }
    }

    /// [`App::set_ref`], to change.
    pub(crate) fn set_mut(
        &mut self,
        p: ride::Profile,
    ) -> (&mut [page::Page], &mut usize, &mut [Option<page::Page>; PAGES]) {
        match p {
            ride::Profile::Indoor => (&mut self.indoor.pages, &mut self.indoor.count, &mut self.indoor.more),
            ride::Profile::Mtb => (&mut self.mtb.pages, &mut self.mtb.count, &mut self.mtb.more),
            ride::Profile::Road => {
                (self.pages.get_mut(..PAGES).unwrap_or(&mut []), &mut self.page_count, &mut self.more)
            }
        }
    }

    /// Most pages a profile's set may hold: the MTB part of `pages.bin` has room for [`page::MTB_PAGES`].
    pub(crate) fn set_cap(p: ride::Profile) -> usize {
        if p == ride::Profile::Mtb { page::MTB_PAGES } else { PAGES }
    }

    /// How many ride pages the set in force (for the ride on screen) has.
    pub(crate) fn ride_count(&self) -> usize {
        self.set_ref(self.ride_profile()).1
    }

    /// Ride page `n` of the set in force; the overview.
    pub(crate) fn ride_page_ref(&self, n: usize) -> Option<&page::Page> {
        match n {
            OVERVIEW => self.pages.get(OVERVIEW),
            n => {
                let (pages, count, _) = self.set_ref(self.ride_profile());
                pages.get(n).filter(|_| n < count)
            }
        }
    }

    /// A climb is ahead or under way: from the moment its warning fires (a followed course: the rider on the climb;
    /// the road ahead: [`climbs_ahead::ANNOUNCE_M`] before its start) until its top is passed.
    pub(crate) fn on_climb(&self) -> bool {
        if self.ride.state() == ride::State::Idle {
            return false;
        }
        if let Some((c, at)) = self.on_course() {
            return c.climb_at(at.along_m).is_some_and(|(_, on)| on);
        }
        let ahead = self.ahead.as_ref().map(|(a, _)| &**a).filter(|a| a.on_road() && !self.indoors());
        ahead.and_then(|a| {
            a.climb_at(a.along_m()).map(|(k, on)| on || a.along_m() + climbs_ahead::ANNOUNCE_M >= k.start_m)
        }) == Some(true)
    }

    /// What the CLIMB page shows: the live segment being ridden (its whole length is the "climb", from 0), else
    /// [`App::climb_view`].
    pub(crate) fn climb_now(&self) -> Option<ClimbNow> {
        if let Some(s) = self.segment_now() {
            let gain_m = self.segments()?.seg().header.elevation_m;
            let climb = course::Climb { start_m: 0, len_m: s.along_m.saturating_add(s.left_m), gain_m };
            return Some(ClimbNow { climb, along: s.along_m, course: false, seg: Some(s.id), ghost: s.ghost_m });
        }
        self.climb_view()
    }

    /// The climb in view: the followed course's (the one the rider is on, else the next), else the road ahead's;
    /// where the rider is on it (m from the trace's or course's start).
    pub(crate) fn climb_view(&self) -> Option<ClimbNow> {
        if let Some((c, at)) = self.on_course() {
            let climb = *c.climbs().get(c.climb_at(at.along_m)?.0)?;
            return Some(ClimbNow { climb, along: at.along_m, course: true, seg: None, ghost: None });
        }
        let a = self.ahead.as_ref().map(|(a, _)| &**a).filter(|a| a.on_road() && !self.indoors())?;
        let climb = a.climb_at(a.along_m())?.0;
        Some(ClimbNow { climb, along: a.along_m(), course: false, seg: None, ghost: None })
    }

    /// The altitude at `d` m of the climb in hand, dm.
    pub(crate) fn climb_alt(&self, n: &ClimbNow, d: u32) -> Option<i32> {
        match (n.seg, n.course) {
            (Some(_), _) => self.segments()?.alt_at(d),
            (None, true) => self.courses.as_deref()?.course.alt_at(d),
            (None, false) => self.ahead.as_ref()?.0.alt_at(d),
        }
    }

    /// The time on the segment or the climb (ClimbTime) and the distance left to its end (ClimbLeft).
    pub(crate) fn climb_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let Some(n) = self.climb_now() else {
            l.push_str("--");
            return l;
        };
        if f == Field::ClimbLeft {
            let (v, unit) = ui::segment::left_text(n.climb.end_m().saturating_sub(n.along));
            l.push_str(v.as_str());
            l.push_str("|");
            l.push_str(unit);
        } else {
            let s = match self.segment_now() {
                Some(s) => s.elapsed_s,
                None => self.climb.t0.map_or(0, |t| self.ride.timer_s().saturating_sub(t)),
            };
            l.push_str(ui::segment::time_text(s).as_str());
        }
        l
    }

    /// One of the rider's ride pages already shows the climb: no second page for a climb alone (a segment's page
    /// always opens: no rider page shows its delta).
    pub(crate) fn has_climb_page(&self) -> bool {
        (0..self.ride_count()).any(|n| {
            self.ride_page_ref(n).is_some_and(|p| {
                p.fields
                    .iter()
                    .take(p.cells())
                    .any(|f| matches!(f, Field::Climb | Field::ClimbGain | Field::ClimbPace | Field::ClimbProfile))
            })
        })
    }

    /// Each pass: the climb page enters the round after the page on screen when the warning fires, and leaves it
    /// at the top (shown then, the round goes on to the page after its home).
    pub(crate) fn settle_climb(&mut self, b: &mut impl Board) {
        let on = self.page_on(CLIMB);
        // the anchor is the page shown when the page joins (any of the round); one that leaves the round (a set
        // switch, a page deleted, an indoor skip) is replaced by the page shown
        if on && (!self.climb.on || !self.page_on(self.climb.after)) {
            self.climb.after =
                if let Screen::Ride(n) = self.screen { if n != CLIMB { n } else { self.climb.after } } else { 0 };
            if let (false, Screen::Ride(n)) = (self.page_on(self.climb.after), self.ride_page(0, 1)) {
                self.climb.after = n;
            }
        }
        self.open_climb(on, b);
        if on == self.climb.on {
            return;
        }
        self.climb.on = on;
        if !on && self.screen == Screen::Ride(CLIMB) {
            self.screen = self.ride_page(place(self.climb.after) + 1, 1);
            self.show(b);
        }
    }

    /// The CLIMB page opens once per climb, when the rider is seen to reach its start (an edge: first seen inside, as
    /// after a resume or a late trace, only presets the latch), and once per live segment, when it starts
    /// (`climb.seg`, set by app/segment_ui.rs). Kept per climb (`climb.2`: its start, done): done when the page
    /// opened, or when it cannot for this climb (a menu or other screen is up, the page is not in the round); a
    /// finger or key down, or a pause, retries at the next pass. The latch lives until another climb (a later
    /// start, the rider outside it) or none is in view; the rider stepping back over the start does not reopen it,
    /// and a trace made again inside the climb moves its start without a new climb.
    fn open_climb(&mut self, on: bool, b: &mut impl Board) {
        let can = on && matches!(self.screen, Screen::Ride(p) if p != CLIMB);
        let ready = self.ride.state() == ride::State::Running && !self.touch.down() && !self.keys.any_down();
        let mut open = false;
        if self.climb.seg {
            if !can {
                self.climb.seg = false; // not for this segment
            } else if ready {
                (self.climb.seg, open) = (false, true);
            }
        }
        match self.climb_view() {
            None => (self.climb.handled, self.climb.t0) = (None, None),
            Some(n) => {
                let (start, inside) = (n.climb.start_m, n.along >= n.climb.start_m);
                if self.climb.handled.map(|h| h.0) != Some(start) {
                    self.climb.t0 = None;
                }
                self.climb.t0 = match (inside, self.climb.t0) {
                    (true, None) => Some(self.ride.timer_s()),
                    (true, t) => t,
                    (false, _) => None,
                };
                let mut done = match self.climb.handled {
                    Some((k, d)) if k == start => d,
                    Some((_, d)) if inside => d,
                    _ => inside,
                };
                if inside && !done {
                    if !can {
                        done = true;
                    } else if ready {
                        (open, done) = (true, true);
                    }
                }
                self.climb.handled = Some((start, done));
            }
        }
        if open {
            self.screen = Screen::Ride(CLIMB);
            self.show(b);
        }
    }

    /// Each pass: when the profile in force changed (a trainer heard or lost, by hand), the ride pages on screen
    /// come from the other set: a page the set lacks goes to the first one it has, and the page is drawn anew.
    /// Held while the editor is open (it works on a copy of one set).
    pub(crate) fn settle_set(&mut self, b: &mut impl Board) {
        let now = self.ride_profile();
        if now == self.set_shown || self.edit.is_some() {
            return;
        }
        self.set_shown = now;
        self.plot_taps = [[None; page::CELLS]; PAGES + 1];
        let fix = |app: &Self, s: Screen| match s {
            Screen::Ride(n) if n < PAGES && !app.page_on(n) => app.ride_page(0, 1),
            s => s,
        };
        self.screen = fix(self, self.screen);
        for k in 0..self.depth.min(self.under.len()) {
            self.under[k] = fix(self, self.under[k]);
        }
        if matches!(self.screen, Screen::Ride(_)) {
            self.show(b);
        }
    }

    /// The profile Start would take now: ROAD or INDOOR fixed in BIKE, else indoor when a trainer is heard, else
    /// MTB if BIKE says so, else ROAD.
    pub(crate) fn chosen_profile(&self) -> ride::Profile {
        match self.settings.profile {
            Some(p) if p != ride::Profile::Mtb => p,
            _ if self.trainer_heard() => ride::Profile::Indoor,
            p => p.unwrap_or_default(),
        }
    }

    /// The ride's profile while one runs, else what Start would choose (so the round already skips the map before
    /// the ride starts).
    pub(crate) fn ride_profile(&self) -> ride::Profile {
        match self.ride.state() {
            ride::State::Idle => self.chosen_profile(),
            _ => self.profile,
        }
    }

    /// The ride is indoors ([`App::ride_profile`]).
    pub(crate) fn indoors(&self) -> bool {
        self.ride_profile() == ride::Profile::Indoor
    }

    /// What a ride outdoors is: MTB when BIKE > PROFILE says so, else ROAD.
    fn outdoor(&self) -> ride::Profile {
        if self.settings.profile == Some(ride::Profile::Mtb) { ride::Profile::Mtb } else { ride::Profile::Road }
    }

    /// At Start and at a workout's start: takes the chosen profile unless the rider set this ride's by hand.
    pub(crate) fn choose_profile(&mut self) {
        self.tlink.quiet_s = 0;
        let p = self.chosen_profile();
        if self.profile_by_hand || p == self.profile {
            return;
        }
        self.profile = p;
        self.say(match p {
            ride::Profile::Road => "ROAD RIDE",
            ride::Profile::Indoor => "INDOOR RIDE",
            ride::Profile::Mtb => "MTB RIDE",
        });
    }

    /// The ride menu's INDOOR row: indoors or out (ROAD or MTB: [`App::outdoor`]), by hand, for the rest of this ride.
    pub(crate) fn toggle_indoor(&mut self) {
        self.profile = if self.indoors() { self.outdoor() } else { ride::Profile::Indoor };
        self.profile_by_hand = true;
        self.tlink.quiet_s = 0;
    }

    /// Once a second: an automatic indoor ride whose trainer has been silent for [`TRAINER_QUIET_S`] goes back to ROAD,
    /// or MTB when BIKE says so (ROAD or INDOOR fixed in BIKE, or a profile set by hand, stays: the rider said so).
    pub(crate) fn profile_second(&mut self) {
        if self.ride.state() == ride::State::Idle
            || self.profile != ride::Profile::Indoor
            || self.profile_by_hand
            || matches!(self.settings.profile, Some(ride::Profile::Road | ride::Profile::Indoor))
            || self.edit.is_some()
        {
            return;
        }
        if self.trainer_heard() {
            self.tlink.quiet_s = 0;
        } else {
            self.tlink.quiet_s = self.tlink.quiet_s.saturating_add(1);
            if self.tlink.quiet_s >= TRAINER_QUIET_S {
                self.profile = self.outdoor();
                self.say(if self.profile == ride::Profile::Mtb {
                    "TRAINER LOST: MTB RIDE"
                } else {
                    "TRAINER LOST: ROAD RIDE"
                });
            }
        }
    }

    /// Page `n` is one an indoor ride has no use for: it holds the map or the next turn.
    pub(crate) fn indoor_blank(&self, n: usize) -> bool {
        self.ride_page_ref(n)
            .is_some_and(|p| p.fields.iter().take(p.cells()).any(|&f| matches!(f, Field::Map | Field::NextTurn)))
    }
}
