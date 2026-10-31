//! The fault log after a fault reset (storage/faultlog.rs: the marker, the line, the file): the marker found at the
//! start is kept, and its line appended to `\Rustfw\Log\faults.log` once, when the card is quiet: no ride open or
//! waiting to be resumed or saved (a ride resumed after the fault is saved first: the recorder is the card's one
//! writer while a ride is open), the drive not lent, [`PAGES_LOAD_MS`] after the start; never in the fault handler.
//! Then the marker is marked logged, so a reset before its minute is up (when the app clears it) does not log it
//! twice. An append that fails is said once and tried again a minute later, [`RETRIES`] times a boot at most, the FAT
//! copies compared first (and at the next start, the marker not being marked logged). A build that may not write the
//! card never writes it. Without the folder `\Rustfw\Log` (made at a ride's start: storage/recorder/mod.rs
//! `make_kept`) nothing is written: looked for again a minute later, said never, no try counted.

use crate::storage::faultlog;
use crate::*;

/// A failed append is tried again this long after.
const RETRY_MS: u32 = 60_000;

/// Tries again after a failure, a boot: each counts the FAT copies first (about 15 MB read on the unit's card), so a
/// card that keeps failing is not scanned every minute.
const RETRIES: u8 = 3;

impl App {
    /// At the start: a fault marker whose line is not in the log yet is kept, with the time now (the fault's, give or
    /// take the restart).
    pub(crate) fn fault_found(&mut self, word: u32) {
        if word >> 24 == faultlog::TAG && word & faultlog::LOGGED == 0 {
            self.fault_due = Some((word, self.rec_time().0, PAGES_LOAD_MS, 0));
        }
    }

    /// The kept marker's line appended once the card is quiet and its time has come (module doc).
    #[inline(never)]
    pub(crate) fn step_fault_log(&mut self, b: &mut impl Board) {
        let Some((word, at, after_ms, tries)) = self.fault_due else { return };
        if !self.card_writable || self.emmc.lent() || !self.ride_settled() || self.up_ms < after_ms {
            return;
        }
        self.fault_due = None;
        let line = faultlog::line(word, at, env!("GIT_DESCRIBE"));
        let t = fat::write::dos_time(self.rec_time().0);
        // after a failed write (this one's or another's) the FAT copies are compared first, as the other writers do
        let suspect = self.fats_suspect;
        let mut d = emmc::Volume(&mut self.emmc, b);
        let r = fat::Fat::mount(&mut d).and_then(|mut fat| {
            if suspect {
                fat.sync_fats(&mut d)?;
            }
            let top = fat.open(&mut d, recorder::DIR)?;
            faultlog::append(&mut fat, &mut d, top, line.as_str().as_bytes(), t)
        });
        match r {
            Ok(()) => {
                if suspect {
                    self.fats_equal();
                }
                let cur = b.backup_read(FAULT_BKP); // the count byte may have been cleared at the minute meanwhile
                if cur >> 8 == word >> 8 {
                    b.backup_write(FAULT_BKP, if self.up_ms >= FAULT_CLEAR_MS { 0 } else { cur | faultlog::LOGGED });
                }
            }
            Err(fat::Error::NotFat32) => self.fault_given_up(b, word), // no volume (a card stock never formatted)
            // no folder yet: never made here (module doc), looked for again later; the line kept in memory. Only after
            // the sync succeeded: the FAT copies are equal, so the next look reads no FAT (15 MB) again
            Err(fat::Error::NotFound) => {
                if suspect {
                    self.fats_equal();
                }
                self.fault_due = Some((word, at, self.up_ms.saturating_add(RETRY_MS), tries));
            }
            Err(_) => {
                self.fats_unsure(); // the next writes compare the FAT copies first
                if tries == 0 {
                    self.notice("FAULT LOG NOT SAVED"); // once
                }
                if tries < RETRIES {
                    self.fault_due = Some((word, at, self.up_ms.saturating_add(RETRY_MS), tries + 1));
                } else {
                    self.fault_given_up(b, word);
                }
            }
        }
    }

    /// No line will be written this boot: past the minute the marker goes, or every boot would find it again.
    fn fault_given_up(&self, b: &mut impl Board, word: u32) {
        if self.up_ms >= FAULT_CLEAR_MS && b.backup_read(FAULT_BKP) >> 8 == word >> 8 {
            b.backup_write(FAULT_BKP, 0);
        }
    }
}
