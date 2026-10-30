//! SYSTEM > SCREENSHOT: the menus are left and the screen under them (the overview or the ride page they were opened
//! over) is saved at the next pass as `\Screenshot\YYYYMMDDhhmmss.bmp` (storage/screenshot.rs: stock's folder and
//! name, the format, the write order). The pixels are [`App::render_row`]'s, the dev link's screenshot source, so the
//! file equals the panel.
//!
//! A menu row, not a key combination: stock's trigger is not known (docs/s810.md), and a chord could fire by accident
//! during a ride, or eat a Start. Only with no ride open or waiting to be saved (the recorder is the card's one writer
//! while a ride is open, AGENTS.md), the card writable and not lent; a full card or a write error is a notice, the
//! recorder never touched.

use crate::storage::screenshot;
use crate::*;

impl App {
    /// SYSTEM > SCREENSHOT: the menus left, the screen under them saved at the next pass; else why not, said.
    pub(crate) fn screenshot(&mut self) {
        let why = match () {
            _ if self.emmc.lent() => "USB DRIVE ON",
            _ if !self.card_writable => "CARD READ ONLY",
            _ if !self.ride_settled() => "NOT DURING A RIDE",
            _ => {
                self.leave_menus();
                self.shot_due = true;
                return;
            }
        };
        self.say(why);
    }

    /// The screenshot asked for, once the screen under the menus is drawn: saved, or why not as a notice.
    pub(crate) fn step_screenshot(&mut self, b: &mut impl Board) {
        if !core::mem::take(&mut self.shot_due) || self.emmc.lent() || !self.card_writable || !self.ride_settled() {
            return;
        }
        let local = self.rec_time().0;
        match self.write_shot(b, screenshot::name(local).as_str(), fat::write::dos_time(local)) {
            Ok(()) => self.notice("SCREENSHOT SAVED"),
            Err(fat::Error::Refused) => self.notice("CARD FULL: NO SCREENSHOT"), // or the name taken, this second
            Err(_) => {
                self.fats_unsure(); // the next writes compare the FAT copies first
                self.notice("SCREENSHOT NOT SAVED");
            }
        }
    }

    /// Writes the screen as file `name` (storage/screenshot.rs: the chain first, the entry last).
    #[inline(never)] // its 4 KB buffer in its own frame
    pub(crate) fn write_shot(&mut self, b: &mut impl Board, name: &str, t: (u16, u16)) -> Result<(), fat::Error> {
        let mut fat = fat::Fat::mount(&mut emmc::Volume(&mut self.emmc, b))?;
        let mut shot = screenshot::Shot::begin(&mut fat, &mut emmc::Volume(&mut self.emmc, b), name, t)?;
        let mut buf = [0u8; screenshot::CHUNK * 512];
        let mut at = 0u32;
        while at < screenshot::SIZE {
            let sectors = ((screenshot::SIZE - at).div_ceil(512) as usize).min(screenshot::CHUNK);
            screenshot::fill(at as usize, &mut buf, |y| self.render_row(y).px);
            let bytes = buf.get(..sectors * 512).unwrap_or(&buf);
            if let Err(e) = shot.write(&mut fat, &mut emmc::Volume(&mut self.emmc, b), at / 512, bytes) {
                shot.abandon(&mut fat, &mut emmc::Volume(&mut self.emmc, b));
                return Err(e);
            }
            at += buf.len() as u32;
        }
        // a link that fails leaves the chain lost, never trimmed: its entry may be on the card already
        shot.finish(&mut fat, &mut emmc::Volume(&mut self.emmc, b), name, t)
    }
}
