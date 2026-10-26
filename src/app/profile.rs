//! The ride profile ([`ride::Profile`]): ROAD or INDOOR, chosen when the ride (or a workout) starts from the trainer
//! heard, fixed by hand (BIKE > PROFILE, the ride menu's INDOOR row), and given up again if the trainer goes
//! silent. What it changes so far: the round skips the map and next-turn pages indoors. (The GPS power-off, the
//! pages per profile and the FIT sub_sport come later: UI review 3, section 4.2.)

use crate::*;

/// Seconds without the trainer, indoor and automatic, before the ride goes back to ROAD: the one costly mistake is
/// an outdoor ride recorded as indoor because the unit sat near a powered trainer.
pub const TRAINER_QUIET_S: u16 = 120;

impl App {
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
        self.pages
            .get(n)
            .is_some_and(|p| p.fields.iter().take(p.cells()).any(|&f| matches!(f, Field::Map | Field::NextTurn)))
    }
}
