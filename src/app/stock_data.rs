//! What stock left on the card, read in the background: the scan of its rides (a bounded amount per pass, for the
//! totals, the history and the power curve) and the import of its settings (wheel, heart rate, FTP).

use crate::*;

/// Stock's rides are read by themselves this long after start-up (for the totals), unless a ride runs.
pub const STOCK_SCAN_MS: u32 = 30_000;

impl App {
    // -----------------------------------------------------------------------------------------------------------
    // What stock left on the card (storage/stock.rs): the stock data menu
    // -----------------------------------------------------------------------------------------------------------

    /// Bytes of stock's rides read per pass: more while no ride runs, little during a ride. Seen on the unit
    /// (2026-10-24, `dev.py bench scan`): 2.1 to 2.5 MB/s read and parsed (the eMMC alone 10.5 MB/s), so 96 KB keeps
    /// a pass near 40 ms and the menu answering (70 rides, 12 MB: about 15 s).
    pub(crate) const SCAN_IDLE: u32 = 96 * 1024;

    pub(crate) const SCAN_RIDING: u32 = 16 * 1024;

    /// A stock data row's value.
    pub(crate) fn stock_value(&self, r: menu::StockRow, v: &mut Line<12>) {
        use menu::StockRow as S;
        let o = &self.stock.out;
        let cp = power_curve::fit_cp(&o.curve);
        match (r, self.stock.status()) {
            (S::Scan, stock::Status::Idle) => v.push_str("GO"),
            (S::Scan, stock::Status::Running) => v.push_u32(u32::from(self.stock.listed)),
            (S::Scan, stock::Status::Done) => v.push_u32(u32::from(o.all.rides)),
            (S::Scan, stock::Status::Failed(_)) => v.push_str("NONE"),
            (S::Import, _) => v.push_str("SETTINGS"),
            (_, s) if s != stock::Status::Done => v.push_str("--"),
            (S::Cp | S::WPrime | S::Fit, _) if cp.is_none() => v.push_str("--"),
            (S::Cp, _) => v.push_u32(cp.map_or(0, |c| u32::from(c.cp))),
            (S::WPrime, _) => push_fixed(v, cp.map_or(0, |c| c.w_prime) as i32, 3, 1),
            (S::Fit, _) => {
                let c = cp.map_or((0, 0), |c| (c.points, c.err_pct));
                v.push_u32(u32::from(c.0));
                v.push_str(" PTS ");
                v.push_u32(u32::from(c.1));
                v.push_str("%");
            }
            (S::WeekKm, _) => v.push_u32(o.week.dist_m / 1000),
            (S::WeekH, _) => push_fixed(v, (o.week.time_s / 360) as i32, 1, 1),
            (S::YearKm, _) => v.push_u32(o.year.dist_m / 1000),
            (S::YearH, _) => push_fixed(v, (o.year.time_s / 360) as i32, 1, 1),
            (S::AllKm, _) => v.push_u32(o.all.dist_m / 1000),
            (S::AllH, _) => push_fixed(v, (o.all.time_s / 360) as i32, 1, 1),
        }
    }

    /// For the dev link's `bench scan`: the scan of stock's rides (started if it is not running) goes on for about
    /// `bytes`; how many it read.
    pub fn bench_scan(&mut self, b: &mut impl Board, bytes: u32) -> u32 {
        if self.stock.status() != stock::Status::Running {
            self.stock.start(self.date);
        }
        let before = self.stock.read_bytes();
        self.stock.step(&mut emmc::Volume(&mut self.emmc, b), bytes);
        self.stock.read_bytes().wrapping_sub(before)
    }

    /// The scan of stock's rides goes on, if it runs.
    pub(crate) fn step_stock(&mut self, b: &mut impl Board) {
        // read once by itself, a while after start-up and not during a ride, for the totals on the overview
        if self.stock.status() == stock::Status::Idle
            && self.gnss_ms >= STOCK_SCAN_MS
            && self.ride.state() == ride::State::Idle
        {
            self.stock.start(self.date);
        }
        if self.stock.status() == stock::Status::Running {
            let budget = if self.ride.state() == ride::State::Idle { Self::SCAN_IDLE } else { Self::SCAN_RIDING };
            self.stock.step(&mut emmc::Volume(&mut self.emmc, b), budget);
        }
    }

    /// The past ride asked for in HISTORY goes on being read, [`App::SCAN_IDLE`] a pass (someone waits for it);
    /// its card opens once read, if the list still shows (leaving it stops the reading).
    #[inline(never)]
    pub(crate) fn step_past(&mut self, b: &mut impl Board) {
        if !self.past.reading() {
            return;
        }
        if !matches!(self.screen, Screen::Menu(Menu::History, _)) {
            return self.past.stop();
        }
        match self.past.step(&mut emmc::Volume(&mut self.emmc, b), Self::SCAN_IDLE) {
            Some(Ok(())) => {
                self.open(Screen::Past);
                self.show(b);
            }
            Some(Err(_)) => self.notice("BAD RIDE FILE"),
            None => {}
        }
    }

    /// At the first start (no settings of this firmware's own in the backup registers: [`WHEEL_BKP`] has none of
    /// its tags), stock's settings are taken as [`App::import_stock`] does from the menu, so the unit starts as
    /// stock had it; the settings are then saved, so this happens once, even when the card has no stock data.
    /// Said only when something was taken. A flat backup battery loses this firmware's settings, so the next start
    /// is a first start again: stock's are better than the defaults then too.
    #[inline(never)] // its card reading in its own frame, not begin's
    pub(crate) fn first_start(&mut self, b: &mut impl Board) {
        if matches!(b.backup_read(WHEEL_BKP) >> 24, 0xa5..=0xa7) {
            return;
        }
        self.import_stock(b, true);
        self.save_misc(b);
    }

    /// Applies stock's settings ([`stock::import`]) where this firmware has them, and says how many (`quiet`:
    /// nothing said when there was nothing).
    pub(crate) fn import_stock(&mut self, b: &mut impl Board, quiet: bool) {
        let Ok(i) = self.stock.import(&mut emmc::Volume(&mut self.emmc, b)) else {
            if !quiet {
                self.notice("NO STOCK DATA");
            }
            return;
        };
        let mut n = 0;
        let s = &mut self.settings;
        let numbers = [
            (i.ftp, &mut s.ftp),
            (i.max_hr, &mut s.max_hr),
            (i.eat_min, &mut s.eat_min),
            (i.drink_min, &mut s.drink_min),
            (i.hr_alert, &mut s.hr_alert),
        ];
        for (want, have) in numbers {
            if let Some(v) = want {
                (*have, n) = (v, n + 1);
            }
        }
        if i.light_auto {
            (s.light_level, n) = (LIGHT_AUTO, n + 1);
        }
        if let Some(mm) = i.wheel_mm {
            // the nearest wheel offered
            s.wheel_mm = WHEELS.iter().copied().min_by_key(|w| w.abs_diff(mm)).unwrap_or(s.wheel_mm);
            n += 1;
        }
        if let Some(dg) = i.weight_dg {
            let (lo, hi) = (*menu::WEIGHT_RANGE.start(), *menu::WEIGHT_RANGE.end());
            (s.weight_kg, n) = (((dg + 5) / 10).clamp(lo, hi), n + 1);
        }
        if let Some(j) = i.w_prime {
            // to the menu's 0.5 kJ steps; CP is this firmware's FTP (W' balance uses it), so stock's CP is not taken
            let (lo, hi) = (*menu::WPRIME_RANGE.start(), *menu::WPRIME_RANGE.end());
            let j = ((j + 250) / 500 * 500).min(u32::from(hi)) as u16;
            (s.wprime_j, n) = (j.clamp(lo, hi), n + 1);
        }
        self.save_misc(b);
        for (kind, id) in Kind::ALL.into_iter().zip(i.sensors) {
            if let Some(id) = id {
                b.backup_write(kind.backup(), id.to_word()); // paired at the next start if ANT is not up
                if self.ant {
                    self.set_sensor(b, kind, Sensor::Paired(id));
                }
                n += 1;
            }
        }
        for (k, p) in i.pages.iter().enumerate() {
            if let (Some(p), Some(page)) = (p, self.pages.get_mut(k)) {
                *page = *p;
                save_page(b, k, p);
                n += 1;
            }
        }
        if quiet && n == 0 {
            return;
        }
        let mut l = Line::<20>::new();
        l.push_str("IMPORTED ");
        l.push_u32(n);
        self.notice(l.as_str());
    }
}
