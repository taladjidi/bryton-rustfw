//! The ride summary after SAVE: the save state on the card's title line (read from the recorders' phase and
//! outcome, never a timer), the second card of totals (Page) and the laps (Lap), read back from the saved file as a
//! past ride's. It only reads: nothing here touches the recorder or the card's writes.

use crate::*;
use ui::card::Save;

/// `summary_page` values: the card and the totals.
pub(crate) const PAGE_CARD: u8 = 0;
pub(crate) const PAGE_TOTALS: u8 = 1;

impl App {
    /// What the title line says: the recorders' outcomes wrote `save` ([`App::save_settle`], `step_recorder`); the full
    /// copy still being written shows as COPY... after SAVED.
    pub(crate) fn save_now(&self) -> Save {
        match self.save {
            Save::Saved if self.rec.full.phase() != recorder::Phase::Done => Save::Copying,
            s => s,
        }
    }

    /// A save that ends with nothing said is not a save: its recorder is done or stuck and no `Saved` came.
    pub(crate) fn save_settle(&mut self) {
        let done = self.rec.clean.phase() == recorder::Phase::Done;
        if self.save == Save::Saving && (done || self.rec.clean.stuck() || self.rec.clean.failing()) {
            self.save = Save::Failed("NOT SAVED: CARD");
        }
    }

    /// The summary is up and the save state changed since it was drawn: the title line alone is drawn again (not
    /// under a banner, which covers it: the screen's next full draw has the state).
    pub(crate) fn save_redraw(&mut self, b: &mut impl Board) {
        let now = self.save_now();
        if self.screen != Screen::Summary || now == self.save_drawn || self.notes.ms.is_some() {
            return;
        }
        self.save_drawn = now;
        let date = self.ride_card().title;
        ui::card::draw_title_line(&mut panel::Panel(b, self.area(), self.look()), date.as_str(), Some(now));
    }

    /// The second card: what the ride has that the first leaves out, from numbers it already keeps.
    pub(crate) fn ride_totals(&self) -> ui::card::Totals {
        let (r, imp) = (&self.ride, self.settings.imperial);
        let num = |v: Option<u32>| {
            let mut l = Line::<10>::new();
            match v {
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

    /// Page on the summary: the card and the totals in turn.
    pub(crate) fn summary_flip(&mut self, b: &mut impl Board) {
        self.summary_page = if self.summary_page == PAGE_TOTALS { PAGE_CARD } else { PAGE_TOTALS };
        self.show(b);
    }

    /// Lap on the summary: the laps of the ride, read from its saved file as a past ride's. Only once it is saved and
    /// the card is free; they open from [`App::laps_step`] when the history has the ride (one scan, not one a press).
    pub(crate) fn summary_laps(&mut self) {
        if self.save_now() != Save::Saved || !self.ride_settled() {
            return self.say(if matches!(self.save, Save::Failed(_)) { "NOT ON THE CARD" } else { "SAVING: WAIT" });
        }
        self.laps_wanted = true;
        self.laps_step();
    }

    /// The laps asked for: read once the history's index has the ride; if it has not, its scan is started (once).
    pub(crate) fn laps_step(&mut self) {
        if !self.laps_wanted {
            return;
        }
        if self.screen != Screen::Summary {
            self.laps_wanted = false;
            return;
        }
        let ((y, mo, d), (h, mi)) = (self.ride_started.date, self.ride_started.time);
        if let Some(&r) = (0..).map_while(|i| self.stock.index.get(i)).find(|r| r.start == (y, mo, d, h, mi)) {
            self.laps_wanted = false;
            self.past.start(&r, self.settings.ftp, self.settings.imperial); // App::step_past opens the list
            self.say("READING RIDE");
        } else if self.stock.status() == stock::Status::Idle {
            self.rescan();
        }
    }
}
