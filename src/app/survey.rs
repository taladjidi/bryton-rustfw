//! STORAGE (SYSTEM > STORAGE): the card's survey (storage/volume.rs) read a pass at a time while the screen is open
//! and no ride runs or waits to be saved; the oldest ride opens on its HISTORY card, where DELETE is.

use crate::*;

impl App {
    /// FAT sectors the survey reads a pass (16 KB: about a millisecond on the card, as the scan's passes).
    pub(crate) const SURVEY_SECTORS: u32 = 32;

    /// The survey goes on while STORAGE is open and no ride runs or waits to be saved (the caller checks the card).
    #[inline(never)]
    pub(crate) fn step_survey(&mut self, b: &mut impl Board) {
        if self.survey.done() || !matches!(self.screen, Screen::Menu(Menu::Storage, _)) || !self.ride_settled() {
            return;
        }
        self.survey.step(&mut emmc::Volume(&mut self.emmc, b), Self::SURVEY_SECTORS);
    }

    /// STORAGE's oldest ride `k` opens as HISTORY's: read over the next passes (App::step_past).
    pub(crate) fn read_old_ride(&mut self, k: usize) {
        let Some(e) = self.survey.oldest(k) else { return };
        let Some(start) = stock::ride_start(e.name()) else { return };
        let r = history::Ride { file: e.file, start, ..history::NONE };
        self.past.start(&r, self.settings.ftp);
        self.say("READING RIDE");
    }
}
