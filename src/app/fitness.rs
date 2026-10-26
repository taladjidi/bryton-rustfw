//! FITNESS's estimates and the model a ride takes: CP and W' from the 90-day power curve ([`power_curve::fit_cp`]),
//! the heart rate variability thresholds from the recent full copies ([`crate::hrvt`], read when FITNESS first
//! shows: [`App::step_thresholds`]), the estimated FTP, and the APPLY rows that copy them into the settings.
//!
//! **The ride's model** ([`App::model_now`]): the fit's CP and W' when it is trusted ([`power_curve::Cp::trusted`]:
//! at least 3 bests of 2 to 30 minutes on the model, within 5 %, from the last 90 days), else the settings' FTP and
//! W'. Taken at a ride's start (and at its resume after a restart) and kept until it ends
//! ([`App::cp_model`]): the live W' balance and the climb pacer use that pair for the whole ride, even if the ride
//! scan ends or the curve changes meanwhile.
//!
//! **EST FTP** ([`App::est_ftp`]): CP when its fit is trusted; else HRVT2 when [`hrvt::MIN_RIDES`] recent rides gave
//! one. CP wins: it comes from maximal efforts, the rider's own performance, where HRVT2 is a physiological marker
//! within some 22 W of the second ventilatory threshold (hrvt.rs), and that threshold (the respiratory compensation
//! point) sits at CP's oxygen uptake (Keir et al. 2015, "Exercise intensity thresholds: identifying the boundaries
//! of sustainable performance", Med Sci Sports Exerc 47: 1932-1940) but is not interchangeable with it in watts
//! (Galan-Rioja, Gonzalez-Mohino, Poole and Gonzalez-Rave 2020, "Relative proximity of critical power and
//! metabolic/ventilatory thresholds: systematic review and meta-analysis", Sports Med 50: 1771-1783, doi
//! 10.1007/s40279-020-01314-8; both checked by their abstracts only).

use crate::*;

impl App {
    /// Gives the reading of the recent rides its memory (main.rs, AXI SRAM): without, the HRVT rows show "--".
    pub fn set_threshold_memory(&mut self, m: &'static mut thresholds::Scan) {
        self.thresholds = Some(m);
    }

    /// The fit of the last 90 days' power curve, once the ride scan (or its cache) has the curve.
    pub(crate) fn cp_fit(&self) -> Option<power_curve::Cp> {
        if self.stock.ready() { power_curve::fit_cp(&self.stock.out.curve) } else { None }
    }

    /// The CP (W) and W' (J, at most 65 kJ) a ride started now takes, and whether they are the fit's (module doc).
    pub(crate) fn model_now(&self) -> (u16, u16, bool) {
        match self.cp_fit().filter(power_curve::Cp::trusted) {
            Some(c) => (c.cp, c.w_prime.min(u32::from(u16::MAX)) as u16, true),
            None => (self.settings.ftp, self.settings.wprime_j, false),
        }
    }

    /// The estimated FTP and where it comes from (module doc).
    pub(crate) fn est_ftp(&self) -> Option<(u16, &'static str)> {
        let cp = self.cp_fit().filter(power_curve::Cp::trusted).map(|c| (c.ftp(), " CP"));
        let hrv = || self.thresholds.as_deref()?.out.vt2_trusted().map(|w| (w, " HRV"));
        cp.or_else(hrv)
    }

    /// The value of a FITNESS row this file owns (nothing for the others: [`App::stock_value`]).
    pub(crate) fn fitness_value(&self, r: menu::StockRow, v: &mut Line<12>) {
        use menu::StockRow as S;
        let watts = |v: &mut Line<12>, w: Option<u16>| match w {
            Some(w) => {
                v.push_u32(u32::from(w));
                v.push_str(" W");
            }
            None => v.push_str("--"),
        };
        let scan = self.thresholds.as_deref();
        let reading = scan.is_some_and(|s| s.status() == thresholds::Status::Running);
        let none = hrvt::Recent::new();
        let out = scan.map_or(&none, |s| &s.out);
        match r {
            S::EstFtp => match self.est_ftp() {
                Some((w, from)) => {
                    watts(v, Some(w));
                    v.push_str(from);
                }
                None => v.push_str("--"),
            },
            S::Model => {
                let from_fit = match self.ride_model.filter(|_| self.ride.state() != ride::State::Idle) {
                    Some((.., fit)) => fit,
                    None => self.model_now().2,
                };
                v.push_str(if from_fit { "CP FIT" } else { "SETTINGS" });
            }
            S::ApplyFtp => watts(v, self.est_ftp().map(|(w, _)| round_ftp(w))),
            S::ApplyWPrime => match self.cp_fit().filter(power_curve::Cp::trusted) {
                Some(c) => {
                    push_fixed(v, i32::from(round_wprime(c.w_prime)), 3, 1);
                    v.push_str(" KJ");
                }
                None => v.push_str("--"),
            },
            _ if reading && matches!(r, S::Hrvt1 | S::Hrvt2 | S::HrvRides) => v.push_str("..."),
            S::Hrvt1 => watts(v, out.vt1().0),
            S::Hrvt2 => watts(v, out.vt2().0),
            S::HrvRides => {
                v.push_u32(u32::from(out.vt1().1));
                v.push_str(" OF ");
                v.push_u32(u32::from(out.read));
            }
            _ => {}
        }
    }

    /// Select on APPLY FTP or APPLY W' (asked twice, as IMPORT): the estimate into the setting, saved.
    pub(crate) fn apply_estimate(&mut self, b: &mut impl Board, menu: Menu, sel: usize, r: menu::StockRow) {
        let ftp = matches!(r, menu::StockRow::ApplyFtp);
        let value = if ftp {
            self.est_ftp().map(|(w, _)| round_ftp(w))
        } else {
            self.cp_fit().filter(power_curve::Cp::trusted).map(|c| round_wprime(c.w_prime))
        };
        let Some(value) = value else { return self.notice("NO ESTIMATE YET") };
        let mut what = Line::<24>::new();
        what.push_str(if ftp { "SET FTP " } else { "SET W' " });
        if ftp {
            what.push_u32(u32::from(value));
            what.push_str(" W");
        } else {
            what.push_u32(u32::from(value) / 1000);
            what.push_str(".");
            what.push_u32(u32::from(value) % 1000 / 100);
            what.push_str(" KJ");
        }
        if !self.confirmed(menu, sel, what.as_str()) {
            return;
        }
        if ftp {
            self.settings.ftp = value;
        } else {
            self.settings.wprime_j = value;
        }
        self.save_misc(b);
        self.notice(if ftp { "FTP SET" } else { "W' SET" });
    }

    /// The recent full copies are read for the thresholds once FITNESS shows (once a power-up, again after a ride
    /// is saved), [`App::SCAN_IDLE`] bytes a pass ([`App::SCAN_RIDING`] during a ride), then nothing.
    #[inline(never)]
    pub(crate) fn step_thresholds(&mut self, b: &mut impl Board) {
        let Some(scan) = self.thresholds.as_deref_mut() else { return };
        if scan.status() == thresholds::Status::Idle && matches!(self.screen, Screen::Menu(Menu::Stock, _)) {
            let (y, m, d) = self.date;
            scan.start(u16::try_from(epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0));
        }
        if scan.status() != thresholds::Status::Running {
            return;
        }
        let bytes = if self.ride.state() == ride::State::Idle { Self::SCAN_IDLE } else { Self::SCAN_RIDING };
        scan.step(&mut emmc::Volume(&mut self.emmc, b), bytes);
    }
}

/// FTP to the setting's 5 W steps, in its range.
fn round_ftp(w: u16) -> u16 {
    let (lo, hi) = (*menu::FTP_RANGE.start(), *menu::FTP_RANGE.end());
    ((w.saturating_add(2)) / 5 * 5).clamp(lo, hi)
}

/// W' to the setting's 0.5 kJ steps, in its range.
fn round_wprime(j: u32) -> u16 {
    let (lo, hi) = (*menu::WPRIME_RANGE.start(), *menu::WPRIME_RANGE.end());
    ((j.saturating_add(250) / 500 * 500).min(u32::from(hi)) as u16).clamp(lo, hi)
}
