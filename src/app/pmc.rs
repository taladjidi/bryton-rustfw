//! Training load in the app (training/load.rs does the sums, ui/load.rs draws the chart): the scan's rides give each
//! ride a TSS, the days a CTL, ATL and TSB, the months their totals.
//!
//! - **When it is built:** once the scan's results are whole ([`stock::Scan::ready`]), and again whenever they change (a
//!   ride saved and scanned, a ride deleted), the day turns, or FTP, LTHR, max heart rate or sex change (they set a
//!   ride's TSS). Never touches the card; the scan's cache is not changed (a TSS is worked out here, not stored: FTP
//!   moves).
//! - **The fields** CTL, ATL and TSB ([`Field::Ctl`]) add the ride on its way ([`App::tss_now`]) to today, so they move as
//!   the ride goes; TSB is yesterday's, as TrainingPeaks has it.
//! - **TRAINING LOAD** (FITNESS): the chart of 90 days. Next / Prev (Page / Lap) move the cursor a day, a tap on the plot
//!   picks one, Back leaves. The plot is drawn once when it opens (and again when the scan finishes under it).
//! - **MONTHS** (FITNESS): a row per month, newest first; Select changes what every row shows (km, hours, rides, ascent,
//!   TSS).
//! - The chart says PARTIAL (for "90 D") when the index was full and reaches back less than the window, and HR EST when a
//!   ride of the window was scored from its average heart rate.
//! - What the scan does not give: heart rate per second (a ride without power gets the hrTSS of its average, low for a
//!   variable ride), and rides past the newest 128 (the index's size).

use crate::load::{self, SHOWN};
use crate::*;

/// What the month rows can show.
pub(crate) const STATS: u8 = 5;

/// The built load, what it was built from, and which stat the month rows show.
pub(crate) struct State {
    pub(crate) load: load::Load,
    /// The index's revision, the day, FTP, LTHR, max heart rate and sex it was built from.
    key: (u32, u16, [u16; 3], bool),
    /// 0 km, 1 hours, 2 rides, 3 ascent, 4 TSS.
    pub(crate) stat: u8,
}

impl State {
    pub(crate) const fn new() -> Self {
        State { load: load::Load::new(), key: (0, 0, [0; 3], false), stat: 0 }
    }
}

impl App {
    /// Today, days since 1970.
    fn pmc_today(&self) -> u16 {
        let (y, m, d) = self.date;
        u16::try_from(epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0)
    }

    /// The TSS of the ride on its way, not in the scan yet.
    pub(crate) fn pmc_extra(&self) -> u32 {
        let s = &self.settings;
        match self.ride.state() {
            ride::State::Idle => 0,
            // no FTP: power::tss would divide by 1, so the ride's score is its heart rate's, as the rides' are
            _ if s.ftp == 0 => self.hrtss.score(s.lthr, s.max_hr, s.female).unwrap_or(0),
            _ => self.tss_now().unwrap_or(0),
        }
    }

    /// Once a pass: builds the load when the scan's results are new.
    pub(crate) fn step_pmc(&mut self, b: &mut impl Board) {
        if !self.stock.ready() {
            return;
        }
        let s = &self.settings;
        let key = (self.stock.index.rev, self.pmc_today(), [s.ftp, s.lthr, s.max_hr], s.female);
        if key == self.pmc.key && self.pmc.load.is_built() {
            return;
        }
        let (ftp, lthr, max_hr, female) = (s.ftp, s.lthr, s.max_hr, s.female);
        self.pmc.key = key;
        self.pmc.load.build(key.1, &self.stock.index, |r| load::ride_tss_est(r, ftp, lthr, max_hr, female));
        if matches!(self.screen, Screen::Load(_) | Screen::Menu(Menu::Months, _)) {
            self.show(b); // the scan finished under the chart, or the day turned
        }
    }

    /// Opens the chart on today.
    pub(crate) fn open_load(&mut self) {
        self.open(Screen::Load(SHOWN - 1));
    }

    /// The date of day `k` of the chart.
    fn load_date(&self, k: usize) -> (u16, u8, u8) {
        let (y, m, d) = self.date;
        civil_from_days(days_from_civil(y, m, d) - (SHOWN - 1 - k.min(SHOWN - 1)) as i32)
    }

    /// The whole screen.
    pub(crate) fn draw_load(&self, c: &mut impl Canvas, cursor: usize) {
        let days = self.pmc.load.days(self.pmc_extra());
        let (partial, hr_est) = (self.pmc.load.partial(), self.pmc.load.hr_est());
        ui::load::draw_screen(
            c,
            &days,
            cursor,
            (self.pmc.load.is_built(), partial, hr_est),
            date_text(self.load_date(cursor)).as_str(),
        );
    }

    /// An input on the chart: Next and Prev move the cursor a day, a tap picks one, Back leaves.
    pub(crate) fn input_load(&mut self, b: &mut impl Board, i: Input) {
        let Screen::Load(k) = self.screen else { return };
        let riding = self.ride.state() != ride::State::Idle;
        let k = match i {
            _ if matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right))) => return self.close_load(b),
            Input::Touch(Gesture::Tap(x, y)) if inside(ui::BACK_TAP, x, y) => return self.close_load(b),
            Input::Select if riding => return self.start_key(b),
            Input::Prev if riding => return self.lap(),
            Input::Next => (k + 1) % SHOWN,
            Input::Prev => (k + SHOWN - 1) % SHOWN,
            Input::Touch(Gesture::Tap(x, y)) if u32::from(y) >= ui::load::PLOT.1 => ui::load::day_at(x),
            _ => return,
        };
        if k != self.load_cursor() {
            self.screen = Screen::Load(k);
            let days = self.pmc.load.days(self.pmc_extra());
            let mut p = panel::Panel(b, self.area(), self.look());
            ui::load::draw_cursor(&mut p, &days, k, date_text(self.load_date(k)).as_str());
        }
    }

    fn load_cursor(&self) -> usize {
        if let Screen::Load(k) = self.screen { k } else { usize::MAX }
    }

    fn close_load(&mut self, b: &mut impl Board) {
        self.close();
        self.show(b);
    }

    /// A MONTHS row: "OCT 26" and the month's total of the stat the rows show.
    pub(crate) fn month_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<()> {
        let m = *self.pmc.load.months().get(k)?;
        let (y, mo) = (m.ym / 12, usize::from(m.ym % 12));
        label.push_str(MONTHS.get(mo).copied().unwrap_or(""));
        label.push_str(" ");
        label.push_u32(u32::from(y % 100));
        let imp = self.settings.imperial;
        match self.pmc.stat {
            0 => {
                value.push_u32(dist_x100(u32::from(m.dist_100m) * 100, imp) / 100);
                value.push_str(if imp { " MI" } else { " KM" });
            }
            1 => {
                value.push_u32(u32::from(m.minutes) / 60);
                value.push_str(".");
                value.push_u32(u32::from(m.minutes) % 60 * 10 / 60);
                value.push_str(" H");
            }
            2 => {
                value.push_u32(u32::from(m.rides));
                value.push_str(" RIDES");
            }
            3 => {
                value.push_u32(if imp { feet(i32::from(m.ascent_m)) as u32 } else { u32::from(m.ascent_m) });
                value.push_str(if imp { " FT" } else { " M" });
            }
            _ => {
                value.push_u32(u32::from(m.tss));
                value.push_str(" TSS");
            }
        }
        Some(())
    }
}
