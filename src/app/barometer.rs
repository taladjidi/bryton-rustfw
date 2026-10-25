//! The barometric altitude in the app: the readings (a second while riding or averaging, [`BATTERY_MS`] otherwise),
//! the altitude the ride takes ([`App::alt_now`]), the calibration's menu rows and its file `\Rustfw\altcal.bin`
//! (the logic and the choices are in altitude.rs).
//!
//! **Rate.** The LPS22HH converts on request (one-shot, power-down in between: ambient.rs). A ride asks once a
//! second: two bus transactions, about 0.4 ms of the loop's time a second (a write of 3 bytes, a status read, a
//! 5-byte read at the I2C4 timing of BOARD.md 16: inferred, not measured on the unit) and a conversion of tens of ms
//! on the chip (datasheet: microamps, inferred), against the 4 s of smoothing and the 5 s climb samples that use it.
//! Faster would add nothing the filter keeps. Idle: every [`BATTERY_MS`], as before (the menu's pressure and
//! temperature rows).
//!
//! **The file** is written between rides only (`ride_settled`: the recorder is the card's one writer while a ride
//! is open); read once after [`ALT_LOAD_MS`], also while a ride is open (a read: a resumed ride gets its calibration
//! back, so its altitude does not change source), and never in a build that may not write the card. A write that fails
//! says so and is not retried until the next calibration; a ride never waits for it.

use crate::*;

/// The calibration file is read this long after the start.
pub const ALT_LOAD_MS: u32 = 3500;

/// `\Rustfw\altcal.bin`.
pub const ALT_FILE: &str = "altcal.bin";

/// Altitudes the menu offers, m (the Dead Sea's shore to above Everest), kept as `alt_m` = metres + this.
pub const ALT_MENU_OFFSET: i32 = 500;

/// Reading state of the calibration file.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AltStore {
    /// Read, or known not to be there.
    pub(crate) loaded: bool,
    /// The altitude the edit opened at (menu units): a calibration only when it was changed.
    pub(crate) edited_from: u16,
}

impl App {
    /// Seconds since 1970 on the unit's clock (what a calibration's age is counted in).
    pub(crate) fn now_s(&self) -> u32 {
        epoch_s(self.rec_time().0).clamp(0, i64::from(u32::MAX)) as u32
    }

    /// The receiver's altitude when it is good enough to calibrate by: a valid 3D fix with six satellites or more
    /// (a 2D fix holds its last altitude).
    fn alt_for_cal(&self) -> Option<i32> {
        self.pvt.filter(|p| p.fix_ok && p.fix == 3 && p.sats >= 6).map(|p| p.alt_mm)
    }

    /// The altitude of the ride this second and its reference ([`altitude::Altimeter::source`]): the ride, its
    /// graphs and the file's records all take it from here.
    pub(crate) fn alt_now(&self) -> (Option<i32>, u8) {
        self.alt.source(self.pvt.filter(|p| p.good()).map(|p| p.alt_mm))
    }

    /// Once a second while a ride is open: the altitude of the ride goes on from its last value when the source
    /// changes (altitude.rs `follow`).
    pub(crate) fn alt_follow(&mut self) {
        let gps = self.pvt.filter(|p| p.good()).map(|p| p.alt_mm);
        self.alt.follow(gps);
    }

    /// The barometer, once a pass: its reading when due (a second while riding or averaging), then the file.
    pub(crate) fn step_altitude(&mut self, b: &mut impl Board) {
        if self.chips.baro {
            let busy = self.ride.state() == ride::State::Running || self.alt.progress().is_some();
            self.baro_ms = self.baro_ms.saturating_add(self.dt_ms);
            if self.baro_ms >= if busy { BARO_RIDE_MS } else { BATTERY_MS } {
                self.baro_ms = 0;
                // the conversion started one period ago; the next one now (one-shot: the chip sleeps in between)
                let read = ambient::baro_read_raw(b);
                self.baro = read.map(|(p, t)| ((p * 10 + 2048) / 4096, t));
                self.alt.feed(read.map(|(p, _)| p));
                ambient::baro_start(b);
            }
        }
        self.alt_storage(b);
    }

    /// One second of the average toward a calibration from the receiver, and its end said.
    pub(crate) fn alt_second(&mut self) {
        if self.alt.progress().is_none() {
            return;
        }
        let (manual, now) = (self.alt.manual(), self.now_s());
        self.alt.second(self.alt_for_cal(), now);
        if manual && self.alt.progress().is_none() {
            let set = self.alt.cal().is_some_and(|c| c.by == altitude::By::Gps && c.when_s == now);
            self.notice(if set { "ALTITUDE SET FROM GPS" } else { "NO ALTITUDE SET" });
        }
    }

    /// The altitude as the menu row edits it: metres + [`ALT_MENU_OFFSET`], in steps of 5.
    fn alt_menu(&self) -> Option<u16> {
        let m = (self.alt.alt_mm()? + 2500).div_euclid(5000) * 5 + ALT_MENU_OFFSET; // to the nearest 5 m
        Some(m.clamp(0, 9500) as u16)
    }

    /// Select on the ALTITUDE row opens the edit at the altitude the barometer reads.
    pub(crate) fn alt_edit_start(&mut self) -> bool {
        let Some(m) = self.alt_menu() else {
            self.say("NO BAROMETER");
            return false;
        };
        (self.settings.alt_m, self.alt_store.edited_from) = (m, m);
        true
    }

    /// The edit is confirmed (Select or a tap; Back leaves things as they were): the barometer reads the typed
    /// altitude from now on, if it was changed.
    pub(crate) fn alt_typed(&mut self) {
        if self.settings.alt_m == self.alt_store.edited_from {
            return;
        }
        let m = (i32::from(self.settings.alt_m) - ALT_MENU_OFFSET) * 1000;
        let now = self.now_s();
        let set = self.alt.calibrate(m, now);
        self.say(if set { "ALTITUDE SET" } else { "NOT SET" });
    }

    /// Select on FROM GPS.
    pub(crate) fn alt_from_gps(&mut self) {
        if !self.alt.reads() {
            return self.say("NO BAROMETER");
        }
        self.alt.from_gps();
        self.say("AVERAGING THE GPS\nALTITUDE: 30 S");
    }

    /// The ALTITUDE row's value when it is not being edited.
    pub(crate) fn alt_row(&self, value: &mut Line<12>) {
        match self.alt.alt_mm() {
            Some(mm) => {
                let m = (mm + 500).div_euclid(1000);
                if m < 0 {
                    value.push_str("-");
                }
                value.push_u32(m.unsigned_abs());
                value.push_str(" M");
            }
            None => value.push_str(if self.chips.baro { "--" } else { "NONE" }),
        }
    }

    /// The FROM GPS row's value: the seconds averaged, or what is missing.
    pub(crate) fn alt_gps_row(&self, value: &mut Line<12>) {
        match (self.alt.progress(), self.alt.reads(), self.alt_for_cal()) {
            (Some(n), ..) => {
                value.push_u32(n);
                value.push_str("/");
                value.push_u32(altitude::WINDOW_S);
            }
            (None, false, _) => value.push_str(if self.chips.baro { "--" } else { "NONE" }),
            (None, true, None) => value.push_str("NO FIX"),
            (None, true, Some(_)) => {}
        }
    }

    /// The calibration file: read once, written when the calibration changed, between rides.
    #[inline(never)]
    fn alt_storage(&mut self, b: &mut impl Board) {
        if !self.card_writable || self.emmc.lent() || self.gnss_ms < ALT_LOAD_MS {
            return;
        }
        // the read is allowed while a ride is open (a resumed ride has its calibration back); the write is not
        if !self.alt_store.loaded {
            self.alt_store.loaded = true;
            let mut s = [0u8; 512];
            let mut d = emmc::Volume(&mut self.emmc, b);
            let read = fat::Fat::mount(&mut d).and_then(|mut fat| {
                let dir = fat.open(&mut d, recorder::DIR)?;
                let f = fat.open_in(&mut d, dir, ALT_FILE)?;
                fat.read_at(&mut d, &f, 0, &mut s)
            });
            if let Ok(512) = read
                && let Some(cal) = altitude::Cal::from_sector(&s)
            {
                let now = self.now_s();
                self.alt.load(cal, now);
            }
        }
        if !self.ride_settled() {
            return;
        }
        let Some(cal) = self.alt.take_dirty() else { return };
        let (local, _) = self.rec_time();
        let sector = cal.to_sector();
        let mut d = emmc::Volume(&mut self.emmc, b);
        let r = fat::Fat::mount(&mut d).and_then(|mut fat| {
            let t = fat::write::dos_time(local);
            let dir = fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)?;
            let mut h = match fat.locate(&mut d, dir, ALT_FILE) {
                Err(fat::Error::NotFound) => fat.create(&mut d, dir, ALT_FILE, t)?,
                other => other?,
            };
            fat.write_sectors(&mut d, &mut h, 0, &sector)?;
            if h.size != 512 {
                fat.set_size(&mut d, &mut h, 512, t)?;
            }
            Ok(())
        });
        if r.is_err() {
            self.notice("ALTITUDE NOT SAVED");
        }
    }
}
