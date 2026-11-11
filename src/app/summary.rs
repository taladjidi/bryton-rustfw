//! The ride summary after SAVE: the save state on the card's title line (read from the recorders' phase and
//! outcome, never a timer), the second card of totals (Page) and the laps (Lap), read back from the saved file as a
//! past ride's. It only reads: nothing here touches the recorder or the card's writes.

use crate::*;
use ui::card::Save;

/// `summary_page` values: the card, the totals, and the laps list opened from it.
pub(crate) const PAGE_CARD: u8 = 0;
pub(crate) const PAGE_TOTALS: u8 = 1;
pub(crate) const PAGE_LAPS: u8 = 2;

impl App {
    /// The ride just ended is not saved (a recorder kept it unsaved, or a build that cannot write the card), is being
    /// saved (a recorder still holds it: written, read back, the full copy), or is saved.
    pub(crate) fn save_now(&self) -> Save {
        if self.save_failed || !self.card_writable {
            Save::Failed("NOT SAVED: CARD")
        } else if self.rec.clean.phase() != recorder::Phase::Done || self.rec.full.phase() != recorder::Phase::Done {
            Save::Saving
        } else {
            Save::Saved
        }
    }

    /// The summary is up and the save state changed since it was drawn: that line alone is drawn again.
    pub(crate) fn save_redraw(&mut self, b: &mut impl Board) {
        let now = self.save_now();
        if self.screen != Screen::Summary || now == self.save_drawn {
            return;
        }
        self.save_drawn = now;
        ui::card::draw_save(&mut panel::Panel(b, self.area(), self.look()), now);
    }

    /// The second card: what the ride has that the first leaves out, from numbers it already keeps.
    pub(crate) fn ride_totals(&self) -> ui::card::Totals {
        let (r, imp) = (&self.ride, self.settings.imperial);
        let num = |v: Option<u32>| {
            let mut l = Line::<10>::new();
            match v.filter(|&v| v > 0) {
                Some(v) => l.push_u32(v),
                None => l.push_str("--"),
            }
            l
        };
        let mut elapsed = Line::<10>::new();
        push_hm(&mut elapsed, r.elapsed_s());
        let mut top = Line::<10>::new();
        let v = u32::from(r.max_speed_x10());
        let v = if imp { mph_x10(v) } else { v };
        top.push_u32(v / 10);
        top.push_str(".");
        top.push_u32(v % 10);
        let best = self.ride_best();
        let at = |s: u16| {
            let i = power_curve::DURATIONS.iter().position(|&d| d == s)?;
            best.get(i).map(|&w| u32::from(w))
        };
        ui::card::Totals {
            rows: [
                ("ELAPSED H:MM", elapsed),
                (if imp { "MAX MPH" } else { "MAX KM/H" }, top),
                ("AVG CADENCE RPM", num(r.avg_cadence().map(u32::from))),
                ("ENERGY KCAL", num(Some(self.power.kcal()))),
                ("MAX POWER W", num(self.power.max().map(u32::from))),
                ("BEST 5 MIN W", num(at(300))),
                ("BEST 20 MIN W", num(at(1200))),
                ("LAPS", num(Some(r.laps().saturating_add(1)))),
            ],
        }
    }

    /// Back from the laps: to the summary when it opened them, else the screen below.
    pub(crate) fn leave_laps(&mut self) {
        if self.summary_page == PAGE_LAPS {
            (self.summary_page, self.screen) = (PAGE_CARD, Screen::Summary);
        } else {
            self.close();
        }
    }

    /// Page on the summary: the card and the totals in turn.
    pub(crate) fn summary_flip(&mut self, b: &mut impl Board) {
        self.summary_page = if self.summary_page == PAGE_TOTALS { PAGE_CARD } else { PAGE_TOTALS };
        self.show(b);
    }

    /// Lap on the summary: the laps of the ride, read from its saved file as a past ride's (the history's entry for its
    /// start); asked before the save is done or the history has read it, it says so.
    pub(crate) fn summary_laps(&mut self) {
        if self.save_now() != Save::Saved {
            return self.say("SAVING: WAIT");
        }
        let ((y, mo, d), (h, mi)) = (self.ride_started.date, self.ride_started.time);
        match (0..).map_while(|i| self.stock.index.get(i)).find(|r| r.start == (y, mo, d, h, mi)) {
            Some(&r) => {
                self.past.start(&r, self.settings.ftp, self.settings.imperial); // App::step_past opens the list
                self.say("READING RIDE");
            }
            None => {
                self.rescan();
                self.say("HISTORY READING: TRY AGAIN");
            }
        }
    }
}
