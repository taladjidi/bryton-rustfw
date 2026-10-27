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
//! ([`page::Set::indoor_default`]: the trainer, the power numbers, the graphs), a ROAD ride from the road set; the
//! overview is shared. The editor and PAGES work on one set ([`App::view_indoor`]): the profile's while a ride is
//! open, else the one PAGES > ROAD PAGES or INDOOR PAGES opened. The road set is the one the backup registers keep (three pages); the indoor
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
