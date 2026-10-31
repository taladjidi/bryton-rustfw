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
    /// (2026-10-24, `tools/dev.py bench scan`): 2.1 to 2.5 MB/s read and parsed (the eMMC alone 10.5 MB/s), so 96 KB keeps
    /// a pass near 40 ms and the menu answering (70 rides, 12 MB: about 15 s).
    pub(crate) const SCAN_IDLE: u32 = 96 * 1024;

    pub(crate) const SCAN_RIDING: u32 = 16 * 1024;

    /// Work a pass gives the scan at most, us: a pass then stays near [`POLL_MS`] + 15 ms, so keys, touch and the
    /// screen keep up while the rides are read (a pass of 96 KB took about 40 ms on the unit).
    pub(crate) const SCAN_IDLE_US: u32 = 15_000;

    pub(crate) const SCAN_RIDING_US: u32 = 4_000;

    /// FAT sectors of each copy compared a step before the scan's first cache write ([`stock::Cache::prepare`]),
    /// steps taken until the pass has spent [`App::SCAN_IDLE_US`]: 16 KB read a step, about 1.6 ms at the card's
    /// 10.5 MB/s, so the unit's 29,808 sectors (15 MB with both copies) take some 160 passes (about 10 s, inferred).
    pub(crate) const SYNC_SECTORS: u32 = 16;

    /// A stock data row's value.
    pub(crate) fn stock_value(&self, r: menu::StockRow, v: &mut Line<12>) {
        use menu::StockRow as S;
        let o = &self.stock.out;
        let cp = self.cp_fit();
        match (r, self.stock.status()) {
            (S::EstFtp | S::Model | S::ApplyFtp | S::ApplyWPrime | S::Hrvt1 | S::Hrvt2 | S::HrvRides, _) => {
                self.fitness_value(r, v)
            }
            (S::Scan, stock::Status::Idle) => v.push_str("GO"),
            (S::Scan, stock::Status::Running) => v.push_u32(u32::from(self.stock.listed)),
            (S::Scan, stock::Status::Done) => v.push_u32(u32::from(o.all.rides)),
            (S::Scan, _) if self.stock.ready() => v.push_u32(u32::from(o.all.rides)),
            (S::Scan, stock::Status::Failed(_)) => v.push_str("NONE"),
            (S::Import, _) => v.push_str("FROM STOCK"),
            (S::Curve, _) => v.push_str("SHOW"),
            _ if !self.stock.ready() => v.push_str("--"),
            (S::Cp | S::WPrime | S::Fit, _) if cp.is_none() => v.push_str("--"),
            (S::Cp, _) => {
                v.push_u32(cp.map_or(0, |c| u32::from(c.cp)));
                v.push_str(" W");
            }
            (S::WPrime, _) => {
                push_fixed(v, cp.map_or(0, |c| c.w_prime) as i32, 3, 1);
                v.push_str(" KJ");
            }
            (S::Fit, _) => {
                // "4/7 PTS 2%": bests on the model of those of 2 to 30 minutes, the largest gap
                let c = cp.map_or((0, 0, 0), |c| (c.points, c.of, c.err_pct));
                v.push_u32(u32::from(c.0));
                v.push_str("/");
                v.push_u32(u32::from(c.1));
                v.push_str(" PTS ");
                v.push_u32(u32::from(c.2));
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

    /// The scan of stock's rides (re)started: a delete a cut left is tried again at its first pass.
    pub(crate) fn rescan(&mut self) {
        self.trash_tried = false;
        self.stock.start(self.date);
    }

    /// For the dev link's `bench scan`: the scan of stock's rides (started if it is not running) goes on for about
    /// `bytes`; how many it read.
    pub fn bench_scan(&mut self, b: &mut impl Board, bytes: u32) -> u32 {
        if self.stock.status() != stock::Status::Running {
            self.rescan();
        }
        let before = self.stock.read_bytes();
        self.stock.step(&mut emmc::Volume(&mut self.emmc, b), bytes);
        self.stock.read_bytes().wrapping_sub(before)
    }

    /// The dev link's `bench map`: draws the basemap of the full-page map at zoom level `level`, at the rider's
    /// position moved by (`dx`, `dy`) m (a view not drawn yet: a cold re-centre), and gives what it read and drew;
    /// None without a position or a card. The page's own basemap is drawn again at its next view.
    pub fn bench_map(&mut self, b: &mut impl Board, level: u32, dx: i32, dy: i32) -> Option<tiles::Stats> {
        let area = map::area(ui::field_box(1, ui::RIDE_H, 0)?);
        let level = level.clamp(map::MIN_LEVEL, map::MAX_LEVEL);
        let mut v = map::View::follow(&self.track, area, map::mpp16(level))?;
        (v.cx, v.cy) = (v.cx.saturating_add(dx), v.cy.saturating_add(dy));
        let o = self.track.origin()?;
        if self.emmc.lent() {
            return None;
        }
        self.basemap.render(&mut emmc::Volume(&mut self.emmc, b), area, v, o);
        Some(self.basemap.stats)
    }

    /// Gives the scan of stock's rides its cache (main.rs: 192 KB in AXI SRAM): read at the first pass, so the
    /// overview's totals show at once, then only new or changed rides are read.
    pub fn set_scan_cache(&mut self, c: &'static mut stock::Cache) {
        self.stock.set_cache(c);
    }

    /// The scan of stock's rides goes on, if it runs: [`IO`](stock::IO) bytes at a time until the pass has spent
    /// [`App::SCAN_IDLE_US`] of work ([`Board::awake_us`]) or [`App::SCAN_IDLE`] bytes (less during a ride), so a
    /// pass stays short whatever the card and the files cost. Its cache is read at the first pass, kept up to date
    /// on a new day, and written once the scan has changed it, only between rides, in the same time a pass.
    #[inline(never)]
    pub(crate) fn step_stock(&mut self, b: &mut impl Board) {
        self.stock.load(&mut emmc::Volume(&mut self.emmc, b), self.date);
        self.stock.new_day(self.date);
        // read once by itself, a while after start-up and not during a ride, for the totals on the overview
        if self.stock.status() == stock::Status::Idle
            && self.gps.ms >= STOCK_SCAN_MS
            && self.ride.state() == ride::State::Idle
        {
            self.rescan();
        }
        let running = self.stock.status() == stock::Status::Running;
        self.trash_tried &= running; // a new scan, a new try
        if running && !self.trash_tried && self.trash_check && self.ride_settled() {
            // a delete a cut left (trash.rs): finished before the scan reads the rides (it then drops the ride); not
            // at boot (the card is not touched then), not during a ride (it makes the FATs equal first). Once a
            // scan: a lasting error is not tried every pass. A card error: kept, tried again at the next scan and
            // before the drive is lent, read only meanwhile (drive_on; the review of 2026-10-29, S4); a damaged
            // volume: dropped, as drive_on does
            self.trash_tried = true;
            let t = fat::write::dos_time(self.rec_time().0);
            self.trash_check = trash::finish(&mut emmc::Volume(&mut self.emmc, b), t) == Err(fat::Error::Io);
        }
        let t0 = b.awake_us();
        if self.stock.status() == stock::Status::Running {
            let idle = self.ride.state() == ride::State::Idle;
            let (bytes, us) =
                if idle { (Self::SCAN_IDLE, Self::SCAN_IDLE_US) } else { (Self::SCAN_RIDING, Self::SCAN_RIDING_US) };
            // ponytail: one FAT sector read again a pass while the scan runs (about 50 us), rather than tracking
            // every write of the recorder and the settings
            self.stock.forget_fat();
            let b0 = self.stock.read_bytes();
            while self.stock.step(&mut emmc::Volume(&mut self.emmc, b), stock::IO as u32)
                && self.stock.read_bytes().wrapping_sub(b0) < bytes
                && b.awake_us().wrapping_sub(t0) < us
            {}
        }
        // the cache written, its FAT comparison first spread over passes in the same time budget (a read-only build
        // skips it: its write would be refused by emmc::Card::write after those reads, for nothing)
        let t = fat::write::dos_time(self.rec_time().0);
        while self.stock.unsaved()
            && self.card_writable
            && self.ride_settled()
            && b.awake_us().wrapping_sub(t0) < Self::SCAN_IDLE_US
        {
            // failed: read again next start (no longer unsaved)
            let _ = self.stock.save(&mut emmc::Volume(&mut self.emmc, b), t, Self::SYNC_SECTORS);
        }
    }

    /// No ride running, and none waiting to be resumed or saved: the card may be used for something else.
    pub(crate) fn ride_settled(&self) -> bool {
        self.ride.state() == ride::State::Idle
            && self.rec.clean.phase() == recorder::Phase::Done
            && self.rec.full.phase() == recorder::Phase::Done
            && !self.rec.resume_pending
            && !self.rec.full_pending
    }

    /// The past ride asked for in HISTORY goes on being read, [`App::SCAN_IDLE`] a pass (someone waits for it);
    /// its card opens once read, if the list still shows (leaving it stops the reading).
    #[inline(never)]
    pub(crate) fn step_past(&mut self, b: &mut impl Board) {
        if !self.past.reading() {
            return;
        }
        if !matches!(self.screen, Screen::Menu(Menu::History | Menu::Storage, _)) {
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

    /// DELETE on a past ride's card (a tap on it or Start held asks, Start or a tap again does: [`PAST_DELETE`]): the ride moved into
    /// `\Rustfw\Deleted` ([`trash::delete`]: kept, never erased), its full copy too; the history, the totals and the
    /// power curve without it at once; back to the list. Never while a ride runs or waits to be saved or resumed.
    pub(crate) fn delete_past(&mut self, b: &mut impl Board) {
        if !self.ride_settled() {
            return self.notice("NOT DURING A RIDE");
        }
        if !self.card_writable || self.emmc.lent() {
            return self.notice("CARD BUSY");
        }
        let (menu, sel) = PAST_DELETE;
        if !self.confirmed(menu, sel, "DELETE RIDE") {
            return;
        }
        let r = self.past.ride;
        let t = fat::write::dos_time(self.rec_time().0);
        match trash::delete(&mut emmc::Volume(&mut self.emmc, b), &r.file, t) {
            Ok(()) => {
                self.stock.forget(&r.file, r.start, self.date);
                self.notice("RIDE DELETED");
                self.close();
                self.survey.start(); // the count and the oldest rides changed (STORAGE, if it is under this card)
                self.clamp_menu(); // the list one ride shorter: the selection on the next one, or the last
            }
            Err(_) => {
                self.trash_check = true; // a move the failure left midway, finished at the next scan or lending
                self.alarm_notice("DELETE FAILED");
            }
        }
    }

    /// At the first start (no backup register holds anything of this firmware's: settings, a paired sensor, a
    /// page), stock's settings are taken as [`App::import_stock`] does from the menu, so the unit starts as stock had
    /// it; the settings are then saved, so this happens once, even when the card has no stock data. Said only when
    /// something was taken. Never with a ride to resume (its sensors and zones stay). A flat backup battery loses
    /// this firmware's settings, so the next start is a first start again: stock's are better than the defaults then.
    #[inline(never)] // its card reading in its own frame, not begin's
    pub(crate) fn first_start(&mut self, b: &mut impl Board) {
        // [`WHEEL_BKP`] alone is not enough: it is written only when a setting is changed, and a unit can have its
        // sensors paired and pages set without that (review of ecb8b98: stock's then replaced them)
        let own = matches!(b.backup_read(WHEEL_BKP) >> 24, 0xa5..=0xac)
            || Kind::ALL.iter().any(|k| k.from_word(b.backup_read(k.backup())).is_some())
            || PAGE_BKP.iter().any(|r| PageCfg::from_words([b.backup_read(r[0]), b.backup_read(r[1])]).is_some());
        // the registers written below are not the rider's change: settings.bin, if there, still wins over them
        let lost = self.settings_store.lost;
        if own || self.rec.resume_pending {
            if !own {
                self.save_misc(b); // the ride's settings kept as they are: no import at the next start either
            }
        } else {
            self.import_stock(b, true);
            self.save_misc(b);
        }
        self.settings_store.lost = lost;
    }

    /// Applies stock's settings ([`stock::Scan::import`]) where this firmware has them, and says how many (`quiet`:
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
        if let Some(p0) = i.alt_p0_dhpa
            && self.alt.import_stock(p0)
        {
            n += 1; // written to its file between rides (not trusted: its age is unknown)
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
        if let Some(r) = i.remote {
            (s.remote, n) = (r, n + 1);
        }
        self.save_misc(b);
        for (kind, id) in Kind::ALL.into_iter().zip(i.sensors) {
            if let Some(id) = id {
                self.keep_pairing(b, kind, id); // paired at the next start if ANT is not up
                if self.ant {
                    self.set_sensor(b, kind, Sensor::Paired(id));
                }
                n += 1;
            }
        }
        // stock's pages in the first places, as many as it had (the others kept after them); not at a first start
        // over pages of this firmware's in \Rustfw\pages.bin (the backup registers lost: a flat backup battery)
        self.ensure_pages(b);
        let mut pages = 0;
        let ours = quiet && self.store.seq != 0;
        // the file not read (lent, a read error): its pages unknown, so stock's are not put over them
        let known = self.store.loaded;
        for (k, p) in i.pages.iter().enumerate().filter(|_| !ours && known) {
            if let (Some(p), Some(page)) = (p, self.pages.get_mut(k)) {
                *page = *p;
                pages = k + 1;
                n += 1;
            }
        }
        if pages > 0 {
            self.page_count = self.page_count.max(pages);
            self.pages_changed(b);
            if !self.card_writable {
                self.save_page_registers(b);
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
