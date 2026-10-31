//! Favourite places in the app (places.rs has the list and the file's format): SAVE PLACE, the PLACES menu (each
//! place's distance and bearing from here, its altitude typed by hand, DELETE), the barometer calibrated at a place,
//! and the file `\Rustfw\places.bin`.
//!
//! **The file** is read when the pages' is ([`PAGES_LOAD_MS`], or at once when a menu wants it), also while a ride is
//! open (a read); written the way `settings.bin` is (app/settings_file.rs): only when no ride is open nor waits to be
//! resumed or saved (`ride_settled`: the recorder is the card's one writer then), by a build that may write the card,
//! its two copies in this firmware's own folder, the newer over the older, the FAT copies compared first after a
//! write that failed. A place saved during a ride is kept in memory and written once the ride is saved (a reset in
//! between loses it, never the ride). A read that fails for any reason but "no file" is tried again, and no place can
//! be saved until it succeeds: a first copy is never written beside a newer one.
//!
//! **The calibration.** Each second with a fix (horizontal accuracy 10 m or better) and the speed under 2 km/h within
//! [`places::NEAR_M`] of a place with an altitude, the barometer is calibrated to it ([`altitude::By::Place`]), once
//! a visit (until the rider is [`LEFT_M`] away), and again when a ride's start asks for a calibration (it then
//! replaces the receiver's 30 s average). Not while FROM GPS averages: the rider asked for that one.

use crate::*;

/// The file's name, in this firmware's folder ([`crate::recorder::DIR`]).
pub const PLACES_FILE: &str = "places.bin";

/// Reads tried before giving up until a menu asks again.
const LOAD_TRIES: u8 = 3;

/// After [`LOAD_TRIES`] failed, one more read this often, s (a card that answers again: the places come back without a
/// restart, and with them the calibration at a place).
const RETRY_S: u8 = 60;

/// Stopped: under 2 km/h (mm/s).
const STOPPED_MM_S: u32 = 556;

/// Horizontal accuracy a fix needs to calibrate at a place, mm.
const H_ACC_MM: u32 = 10_000;

/// A place is left (its calibration may be made again at the next visit) this far from it, m.
pub const LEFT_M: u32 = 2 * places::NEAR_M;

/// The places and their file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlaceStore {
    /// The places saved.
    pub(crate) list: places::Places,
    /// The file was read (or is not there).
    pub(crate) loaded: bool,
    tries: u8,
    /// Seconds since the reads gave up: one more read a minute ([`RETRY_S`]).
    idle_s: u8,
    /// A change waits to be written.
    pub(crate) dirty: bool,
    /// The FAT copies are known equal (else compared before the write, from `sync`).
    pub(crate) sound: bool,
    pub(crate) sync: u32,
    /// The newest copy's sequence number.
    seq: u32,
    /// The place the barometer was calibrated at, until the rider leaves it.
    at: Option<u8>,
}

impl PlaceStore {
    /// Nothing read.
    pub(crate) const fn new() -> Self {
        PlaceStore {
            list: places::Places::new(),
            loaded: false,
            tries: 0,
            idle_s: 0,
            dirty: false,
            sound: false,
            sync: 0,
            seq: 0,
            at: None,
        }
    }
}

impl App {
    /// The file read when due, written when a change waits and no ride is open.
    #[inline(never)]
    pub(crate) fn step_places(&mut self, b: &mut impl Board) {
        if self.emmc.lent() {
            return;
        }
        let st = self.places;
        if !st.loaded && st.tries < LOAD_TRIES && (self.screen != Screen::Main || self.gps.ms >= PAGES_LOAD_MS) {
            self.load_places(b);
        }
        if !(self.card_writable && self.places.loaded && self.places.dirty && self.ride_settled()) {
            return;
        }
        if !self.places.sound {
            match Self::sync_fats_step(&mut self.emmc, b, &mut self.places.sync) {
                Some(false) => return, // the FAT copies compared over the next passes first
                Some(true) => self.fats_equal(),
                None => {}
            }
        }
        self.save_places(b);
    }

    /// Reads `\Rustfw\places.bin`: its newest whole copy, or none saved when there is no file or no whole copy.
    #[inline(never)] // its 1 KB buffer in its own frame
    fn load_places(&mut self, b: &mut impl Board) {
        let mut file = [0u8; 1024];
        let read = self.read_copies(b, PLACES_FILE, &mut file);
        if read.is_err() && read != Err(fat::Error::NotFound) {
            self.places.tries = self.places.tries.saturating_add(1);
            return;
        }
        let sound = read == Ok(1024) && !self.fats_suspect;
        let st = &mut self.places;
        (st.loaded, st.sound) = (true, sound);
        if let Some((seq, list)) = read.ok().filter(|&n| n == 1024).and_then(|_| places::Places::newest(&file)) {
            (st.seq, st.list) = (seq, list);
        }
    }

    /// Writes the places over the file's older copy (the file created with both if needed).
    fn save_places(&mut self, b: &mut impl Board) {
        self.places.dirty = false; // once: a failure is said, and tried again at the next change
        let seq = self.places.seq.wrapping_add(1);
        let sector = self.places.list.to_sector(seq);
        match self.write_copy(b, PLACES_FILE, seq % 2, &sector) {
            None => {}
            Some(Ok(())) => (self.places.seq, self.places.sound) = (seq, true),
            Some(Err(_)) => {
                self.fats_unsure(); // compared again first
                self.notice("PLACES NOT SAVED");
            }
        }
    }

    /// The places may be changed: the file read first (now, if it was not); else why not, said.
    pub(crate) fn places_ready(&mut self, b: &mut impl Board) -> bool {
        if !self.emmc.lent() && !self.places.loaded {
            self.load_places(b);
        }
        if !self.places.loaded {
            self.say(if self.emmc.lent() { "USB DRIVE ON" } else { "PLACES NOT READ" });
        }
        self.places.loaded
    }

    /// SAVE PLACE: here, with the barometer's altitude when its calibration is fresh, else the receiver's when good
    /// enough to calibrate by, else none (typed later in the place's menu).
    pub(crate) fn save_place(&mut self, b: &mut impl Board) {
        if !self.places_ready(b) {
            return;
        }
        let Some(p) = self.gps.pvt.filter(|p| p.good()) else { return self.say("NO GPS FIX") };
        let now = self.now_s();
        let baro = self.alt.alt_mm().filter(|_| self.alt.cal().is_some_and(|c| c.fresh(now)));
        let Some(k) = self.places.list.add(p.lat, p.lon, baro.or(self.alt_for_cal())) else {
            return self.say("PLACES FULL");
        };
        self.places.dirty = self.card_writable;
        // its altitude the barometer's: nothing to calibrate; else (the receiver's, or none) the barometer takes it here
        self.places.at = baro.and(u8::try_from(k).ok());
        let mut l = Line::<16>::new();
        if let Some(place) = self.places.list.get(k) {
            place.name(&mut l);
        }
        let mut t = Line::<40>::new();
        t.push_str(l.as_str());
        t.push_str(" SAVED");
        self.say(t.as_str());
    }

    /// DELETE in a place's menu (confirmed).
    pub(crate) fn delete_place(&mut self, k: usize) {
        self.places.list.remove(k);
        (self.places.dirty, self.places.at) = (self.card_writable, None);
        self.say("PLACE DELETED");
    }

    /// Once a second: stopped at a place with an altitude, the barometer reads it (module doc).
    pub(crate) fn places_second(&mut self) {
        let st = &mut self.places;
        if !st.loaded && st.tries >= LOAD_TRIES {
            st.idle_s = st.idle_s.saturating_add(1);
            if st.idle_s >= RETRY_S {
                (st.tries, st.idle_s) = (LOAD_TRIES - 1, 0); // step_places reads once more
            }
        }
        let Some(p) = self.gps.pvt.filter(|p| p.good() && p.h_acc_mm <= H_ACC_MM) else { return };
        match self.places.list.nearest_with_alt(p.lat, p.lon) {
            Some((k, d)) if d <= places::NEAR_M => {
                let asked = self.alt.progress().is_some(); // a ride's start (FROM GPS is the rider's own)
                if (self.places.at == u8::try_from(k).ok() && !asked)
                    || self.alt.manual()
                    || p.speed_mm_s >= STOPPED_MM_S
                {
                    return;
                }
                let Some(place) = self.places.list.get(k).copied() else { return };
                let Some(alt) = place.alt_mm() else { return };
                if self.alt.calibrate_by(alt, self.now_s(), altitude::By::Place) {
                    self.places.at = u8::try_from(k).ok();
                    let mut t = Line::<40>::new();
                    t.push_str("ALTITUDE SET AT\n");
                    let mut l = Line::<16>::new();
                    place.name(&mut l);
                    t.push_str(l.as_str());
                    self.notice(t.as_str());
                }
            }
            Some((_, d)) if d <= LEFT_M => {}
            _ => self.places.at = None,
        }
    }

    /// Row `k` of PLACES: its name, and how far and which way from here ("1.2 KM NE"); "--" with no fix.
    pub(crate) fn place_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<()> {
        let place = self.places.list.get(k)?;
        place.name(label);
        let Some(p) = self.gps.pvt.filter(|p| p.good()) else {
            value.push_str("--");
            return Some(());
        };
        let (m, deg) = places::dist_bearing((p.lat, p.lon), (place.lat, place.lon));
        if m < 1000 {
            value.push_u32(m);
            value.push_str(" M ");
        } else {
            ui::card::push_km(value, m.saturating_add(50).min(999_999)); // to the nearest 100 m
            value.push_str(" KM ");
        }
        value.push_str(POINTS.get(usize::from(point_of(i32::from(deg) * 100_000))).copied().unwrap_or(""));
        Some(())
    }

    /// A place's ALTITUDE row when it is not being edited: its altitude, or "--" when none is known.
    pub(crate) fn place_alt_row(&self, k: usize, value: &mut Line<12>) {
        match self.places.list.get(k).and_then(places::Place::alt_mm) {
            Some(mm) => {
                let m = mm.div_euclid(1000);
                if m < 0 {
                    value.push_str("-");
                }
                value.push_u32(m.unsigned_abs());
                value.push_str(" M");
            }
            None => value.push_str("--"),
        }
    }

    /// Select on a place's ALTITUDE: the edit opens at its altitude, else at the barometer's, else at 0 m.
    pub(crate) fn place_alt_edit_start(&mut self, k: usize) {
        let known = self.places.list.get(k).and_then(places::Place::alt_mm);
        let mm = known.or(self.alt.alt_mm()).unwrap_or(0);
        let m = ((mm + 2500).div_euclid(5000) * 5 + ALT_MENU_OFFSET).clamp(0, 9500) as u16; // to the nearest 5 m
        // with none known, confirming the altitude shown sets it
        (self.settings.alt_m, self.alt_store.edited_from) = (m, if known.is_some() { m } else { u16::MAX });
    }

    /// The edit of a place's altitude is confirmed (Back leaves it as it was): kept, and the barometer calibrated to
    /// it at the next second if the rider is there.
    pub(crate) fn place_alt_typed(&mut self, k: usize) {
        if self.settings.alt_m == self.alt_store.edited_from || k >= self.places.list.len() {
            return;
        }
        let mm = (i32::from(self.settings.alt_m) - ALT_MENU_OFFSET) * 1000;
        self.places.list.set_alt(k, Some(mm));
        (self.places.dirty, self.places.at) = (self.card_writable, None);
        self.say("PLACE ALTITUDE SET");
    }
}
