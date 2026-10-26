//! The ride profile ([`ride::Profile`]): ROAD or INDOOR, chosen when the ride (or a workout) starts from the trainer
//! heard, fixed by hand (BIKE > PROFILE, the ride menu's INDOOR row), and given up again if the trainer goes
//! silent. What it changes: the round skips the map and next-turn pages indoors, the GNSS receiver is off during an
//! indoor ride (gps.rs), and the ride file's sub_sport (indoor_cycling or road, fit.rs), which the recovery record
//! keeps with "by hand", so a ride resumed after a reset comes back with its profile, and one set by hand stays so
//! (riding.rs).
//!
//! **Pages per profile** (UI review 3, 2.3): an INDOOR ride takes its pages from the indoor set
//! ([`page::Set::indoor_default`]: the trainer, the power numbers, the graphs), a ROAD ride from the road set; the
//! overview is shared. The editor and PAGES work on one set ([`App::view_indoor`]): the profile's while a ride is
//! open, else the one PAGES > SET names. The road set is the one the backup registers keep (three pages); the indoor
//! set lives in `\Rustfw\pages.bin` only (page.rs), so a change to it made while the file must wait is lost to a
//! switch-off.

use crate::*;

/// Seconds without the trainer, indoor and automatic, before the ride goes back to ROAD: the one costly mistake is
/// an outdoor ride recorded as indoor because the unit sat near a powered trainer.
pub const TRAINER_QUIET_S: u16 = 120;

impl App {
    /// The set PAGES and the editor show: the profile's while a ride is open, else the one chosen in PAGES.
    pub(crate) fn view_indoor(&self) -> bool {
        if self.ride.state() == ride::State::Idle { self.pages_indoor } else { self.profile == ride::Profile::Indoor }
    }

    /// A set's pages, how many are used, and their MORE variants: the indoor one or the road one.
    pub(crate) fn set_ref(&self, indoor: bool) -> (&[page::Page], usize, &[Option<page::Page>; PAGES]) {
        if indoor {
            (&self.indoor.pages, self.indoor.count, &self.indoor.more)
        } else {
            (self.pages.get(..PAGES).unwrap_or(&[]), self.page_count, &self.more)
        }
    }

    /// [`App::set_ref`], to change.
    pub(crate) fn set_mut(
        &mut self,
        indoor: bool,
    ) -> (&mut [page::Page], &mut usize, &mut [Option<page::Page>; PAGES]) {
        if indoor {
            (&mut self.indoor.pages, &mut self.indoor.count, &mut self.indoor.more)
        } else {
            (self.pages.get_mut(..PAGES).unwrap_or(&mut []), &mut self.page_count, &mut self.more)
        }
    }

    /// How many ride pages the set in force (for the ride on screen) has.
    pub(crate) fn ride_count(&self) -> usize {
        self.set_ref(self.indoors()).1
    }

    /// Ride page `n` of the set in force; the overview.
    pub(crate) fn ride_page_ref(&self, n: usize) -> Option<&page::Page> {
        match n {
            OVERVIEW => self.pages.get(OVERVIEW),
            n => {
                let (pages, count, _) = self.set_ref(self.indoors());
                pages.get(n).filter(|_| n < count)
            }
        }
    }

    /// Each pass: when the profile in force changed (a trainer heard or lost, by hand), the ride pages on screen
    /// come from the other set: a page the set lacks goes to the first one it has, and the page is drawn anew.
    /// Held while the editor is open (it works on a copy of one set).
    pub(crate) fn settle_set(&mut self, b: &mut impl Board) {
        let now = self.indoors();
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

    /// The profile Start would take now: the one fixed in BIKE, else indoor when a trainer is heard.
    pub(crate) fn chosen_profile(&self) -> ride::Profile {
        match self.settings.profile {
            Some(p) => p,
            None if self.trainer_heard() => ride::Profile::Indoor,
            None => ride::Profile::Road,
        }
    }

    /// The ride is indoors: the ride's profile while one runs, else what Start would choose (so the round already
    /// skips the map before the ride starts).
    pub(crate) fn indoors(&self) -> bool {
        match self.ride.state() {
            ride::State::Idle => self.chosen_profile() == ride::Profile::Indoor,
            _ => self.profile == ride::Profile::Indoor,
        }
    }

    /// At Start and at a workout's start: takes the chosen profile unless the rider set this ride's by hand.
    pub(crate) fn choose_profile(&mut self) {
        self.trainer_quiet_s = 0;
        let p = self.chosen_profile();
        if self.profile_by_hand || p == self.profile {
            return;
        }
        self.profile = p;
        self.say(if p == ride::Profile::Indoor { "INDOOR RIDE" } else { "ROAD RIDE" });
    }

    /// The ride menu's INDOOR row: the other profile, by hand, for the rest of this ride.
    pub(crate) fn toggle_indoor(&mut self) {
        self.profile = if self.indoors() { ride::Profile::Road } else { ride::Profile::Indoor };
        self.profile_by_hand = true;
        self.trainer_quiet_s = 0;
    }

    /// Once a second: an automatic indoor ride whose trainer has been silent for [`TRAINER_QUIET_S`] goes back to ROAD
    /// (a profile fixed in BIKE or by hand stays: the rider said so).
    pub(crate) fn profile_second(&mut self) {
        if self.ride.state() == ride::State::Idle
            || self.profile != ride::Profile::Indoor
            || self.profile_by_hand
            || self.settings.profile.is_some()
            || self.edit.is_some()
        {
            return;
        }
        if self.trainer_heard() {
            self.trainer_quiet_s = 0;
        } else {
            self.trainer_quiet_s = self.trainer_quiet_s.saturating_add(1);
            if self.trainer_quiet_s >= TRAINER_QUIET_S {
                self.profile = ride::Profile::Road;
                self.say("TRAINER LOST: ROAD RIDE");
            }
        }
    }

    /// Page `n` is one an indoor ride has no use for: it holds the map or the next turn.
    pub(crate) fn indoor_blank(&self, n: usize) -> bool {
        self.ride_page_ref(n)
            .is_some_and(|p| p.fields.iter().take(p.cells()).any(|&f| matches!(f, Field::Map | Field::NextTurn)))
    }
}
