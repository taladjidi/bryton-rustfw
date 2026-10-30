//! The fault log after a fault reset (storage/faultlog.rs: the marker, the line, the file): the marker found at the
//! start is kept, and its line appended to `\Rustfw\Log\faults.log` once, when the card is quiet: no ride open or
//! waiting to be resumed or saved (a ride resumed after the fault is saved first: the recorder is the card's one
//! writer while a ride is open), the drive not lent, [`PAGES_LOAD_MS`] after the start; never in the fault handler.
//! Then the marker is marked logged, so a reset before its minute is up (when the app clears it) does not log it
//! twice. An append that fails is said once and tried again a minute later, as long as the unit runs (and at the next
//! start, the marker not being marked logged). A build that may not write the card never writes it.

use crate::storage::faultlog;
use crate::*;

/// A failed append is tried again this long after.
const RETRY_MS: u32 = 60_000;

impl App {
    /// At the start: a fault marker whose line is not in the log yet is kept, with the time now (the fault's, give or
    /// take the restart).
    pub(crate) fn fault_found(&mut self, word: u32) {
        if word >> 24 == faultlog::TAG && word & faultlog::LOGGED == 0 {
            self.fault_due = Some((word, self.rec_time().0, PAGES_LOAD_MS));
        }
    }

    /// The kept marker's line appended once the card is quiet and its time has come (module doc).
    #[inline(never)]
    pub(crate) fn step_fault_log(&mut self, b: &mut impl Board) {
        let Some((word, at, after_ms)) = self.fault_due else { return };
        if !self.card_writable || self.emmc.lent() || !self.ride_settled() || self.up_ms < after_ms {
            return;
        }
        self.fault_due = None;
        let line = faultlog::line(word, at, env!("GIT_DESCRIBE"));
        let t = fat::write::dos_time(self.rec_time().0);
        let mut d = emmc::Volume(&mut self.emmc, b);
        let r = fat::Fat::mount(&mut d).and_then(|mut fat| {
            let top = fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)?;
            faultlog::append(&mut fat, &mut d, top, line.as_str().as_bytes(), t)
        });
        match r {
            Ok(()) => {
                if b.backup_read(FAULT_BKP) == word {
                    b.backup_write(FAULT_BKP, word | faultlog::LOGGED);
                }
            }
            Err(fat::Error::NotFat32) => {} // no volume (a card stock never formatted): nothing to say
            Err(_) => {
                self.fats_unsure(); // the next writes compare the FAT copies first
                if after_ms == PAGES_LOAD_MS {
                    self.notice("FAULT LOG NOT SAVED"); // once
                }
                self.fault_due = Some((word, at, self.up_ms.saturating_add(RETRY_MS)));
            }
        }
    }
}
