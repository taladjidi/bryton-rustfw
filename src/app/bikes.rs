//! The three bikes in the app (bikes.rs has the list, the counting and the file's format): BIKE > BIKES (ACTIVE BIKE,
//! each bike's odometer and parts, SERVICE EVERY, RESET), the active bike's gear kept as the settings', each saved
//! ride counted once, a notice when a part passes its interval, and the file `\Rustfw\bikes.bin`.
//!
//! **Memory.** The store lives beside the app in AXI SRAM ([`App::set_bike_memory`]: the app is at its size budget);
//! without it (the tests that do not give one) there are no bikes and BIKES is empty.
//!
//! **The file** is read once the settings' is (its migration takes the settings' single bike and ODOMETER), written
//! the way `places.bin` is (app/places.rs): only when no ride is open nor waits to be resumed or saved
//! ([`App::ride_settled`]), by a build that may write the card, its two copies in this firmware's own folder, the
//! FAT copies compared first after a write that failed. A read that fails for any reason but "no file" is tried
//! again, and nothing is changed or counted until it succeeds: a first copy is never written beside a newer one.
//!
//! **The migration** (no file, or no whole copy): BIKE 1 gets the settings' wheel, weight, chainrings and cassette and
//! its ODOMETER; BIKE 2 and 3 the defaults. `settings.bin` and the backup registers are left as they are (they keep
//! the active bike's gear, so an older build and stock see the bike they knew; stock never reads `\Rustfw\`).
//!
//! **The active bike's gear is the settings'.** WHEEL, BIKE WEIGHT, CHAINRINGS and CASSETTE edit the settings as
//! before; the active bike takes them at the next pass. ACTIVE BIKE puts the chosen bike's gear into the settings.
//! At the read the file's gear wins over the settings' (written first, below, it is the newer of the two after a
//! cut between the writes; a change only the registers kept is undone with the switch it went with).
//! `bikes.bin` is written before `settings.bin` (the settings wait while it is due), so a power cut between the two
//! writes leaves the newer in the bikes' file, which wins.
//!
//! **The rides** are counted from the ride scan's list when a scan ends ([`bikes::Bikes::count`], idempotent: the
//! mark and the sums in one copy). The scan runs again after each save (app/riding.rs), so a ride is counted a few
//! seconds after its save, and after a power cut, at the next start's scan.

use crate::*;

/// The file's name, in this firmware's folder ([`crate::recorder::DIR`]).
pub const BIKES_FILE: &str = "bikes.bin";

/// Reads tried before giving up until a menu asks again.
const LOAD_TRIES: u8 = 3;

/// The bikes and their file.
#[derive(Clone, Copy, Debug)]
pub struct BikeStore {
    /// The bikes.
    pub(crate) list: bikes::Bikes,
    /// The file was read (or is not there).
    pub(crate) loaded: bool,
    tries: u8,
    /// A change waits to be written.
    pub(crate) dirty: bool,
    /// The FAT copies are known equal (else compared before the write, from `sync`).
    pub(crate) sound: bool,
    pub(crate) sync: u32,
    /// The newest copy's sequence number.
    seq: u32,
    /// The scan's end was counted (cleared while a scan runs).
    pub(crate) scan_counted: bool,
}

impl Default for BikeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BikeStore {
    /// Nothing read.
    pub const fn new() -> Self {
        BikeStore {
            list: bikes::Bikes::new(),
            loaded: false,
            tries: 0,
            dirty: false,
            sound: false,
            sync: 0,
            seq: 0,
            scan_counted: false,
        }
    }
}

/// A bike's name.
pub(crate) fn bike_name(k: usize) -> &'static str {
    ["BIKE 1", "BIKE 2", "BIKE 3"].get(k).copied().unwrap_or("BIKE")
}

impl App {
    /// Gives the app the bikes' store (main.rs: in AXI SRAM).
    pub fn set_bike_memory(&mut self, m: &'static mut BikeStore) {
        self.bikes = Some(m);
    }

    /// The bikes, once their file is read.
    pub(crate) fn bikes(&self) -> Option<&bikes::Bikes> {
        self.bikes.as_deref().filter(|s| s.loaded).map(|s| &s.list)
    }

    /// A change to the bikes waits to be written (the settings wait for it: module doc).
    pub(crate) fn bikes_due(&self) -> bool {
        self.bikes.as_deref().is_some_and(|s| s.loaded && s.dirty)
    }

    /// The file read when the settings' is, the active bike kept as the settings, the rides counted at a scan's end,
    /// the file written when a change waits and no ride is open.
    #[inline(never)]
    pub(crate) fn step_bikes(&mut self, b: &mut impl Board) {
        let Some(st) = self.bikes.as_deref().copied() else { return };
        if self.emmc.lent() {
            return;
        }
        if !st.loaded && st.tries < LOAD_TRIES && self.settings_store.loaded {
            self.load_bikes(b);
        }
        let done = self.stock.status() == stock::Status::Done;
        let (gear, writable) = (bikes::Gear::of(&self.settings), self.card_writable);
        let index = &self.stock.index;
        let Some(st) = self.bikes.as_deref_mut().filter(|s| s.loaded) else { return };
        if st.list.active().gear != gear {
            st.list.active_mut().gear = gear; // an edit in BIKE
            st.dirty |= writable;
        }
        let mut due = None;
        if done && !st.scan_counted {
            let rides = (0..history::RIDES).map_while(|i| index.get(i)).map(|r| (r.start, r.dist_m));
            if st.list.count(rides) {
                st.dirty |= writable;
                due = st.list.due();
            }
        }
        st.scan_counted = done;
        let st = *st;
        if let Some(p) = due {
            self.part_notice(&st.list, p);
        }
        if !(writable && st.dirty && self.ride_settled()) {
            return;
        }
        if !st.sound {
            let Some(at) = self.bikes.as_deref_mut().map(|s| &mut s.sync) else { return };
            match Self::sync_fats_step(&mut self.emmc, b, at) {
                Some(false) => return, // the FAT copies compared over the next passes first
                Some(true) => self.fats_equal(),
                None => {}
            }
        }
        self.save_bikes(b);
    }

    /// "CHAIN 3000 KM: SERVICE": part `p` of the active bike is past its interval.
    fn part_notice(&mut self, list: &bikes::Bikes, p: usize) {
        let mut t = Line::<40>::new();
        t.push_str(bikes::PART_NAMES.get(p).copied().unwrap_or(""));
        t.push_str(" ");
        t.push_u32(u32::from(list.active().parts.get(p).map_or(0, |p| p.every_km)));
        t.push_str(" KM: SERVICE");
        self.notice(t.as_str());
    }

    /// Reads `\Rustfw\bikes.bin`: its newest whole copy (its active bike's gear into the settings), or the migration
    /// from the settings when there is no file or no whole copy.
    #[inline(never)] // its 1 KB buffer in its own frame
    fn load_bikes(&mut self, b: &mut impl Board) {
        let mut file = [0u8; 1024];
        let read = self.read_copies(b, BIKES_FILE, &mut file);
        let sound = read == Ok(1024) && !self.fats_suspect;
        let gear = bikes::Gear::of(&self.settings);
        let (odo, writable) = (self.settings.odo_m, self.card_writable);
        // the rides from a minute before now on are the bikes' (one started as the file was read counts); none
        // without a clock (the first scan's end sets the mark: the rides then on the card are not counted)
        let (y, mo, d, h, mi, _) = from_epoch_s(epoch_s(self.rec_time().0) - 60);
        let mark = if self.date.0 >= 2020 { (y, mo, d, h, mi) } else { (0, 0, 0, 0, 0) };
        let Some(st) = self.bikes.as_deref_mut() else { return };
        if read.is_err() && read != Err(fat::Error::NotFound) {
            st.tries = st.tries.saturating_add(1);
            return;
        }
        (st.loaded, st.sound) = (true, sound);
        match read.ok().filter(|&n| n == 1024).and_then(|_| bikes::Bikes::newest(&file)) {
            Some((seq, list)) => (st.seq, st.list) = (seq, list),
            None => (st.list, st.dirty) = (bikes::Bikes::migrate(gear, odo, mark), writable),
        }
        let list = st.list;
        if list.active().gear != gear {
            list.active().gear.apply(&mut self.settings);
            self.save_misc(b);
        }
        if let Some(p) = list.due() {
            self.part_notice(&list, p);
        }
    }

    /// Writes the bikes over the file's older copy (the file created with both if needed).
    fn save_bikes(&mut self, b: &mut impl Board) {
        let Some(st) = self.bikes.as_deref_mut() else { return };
        st.dirty = false; // once: a failure is said, and tried again at the next change
        let seq = st.seq.wrapping_add(1);
        let sector = st.list.to_sector(seq);
        match self.write_copy(b, BIKES_FILE, seq % 2, &sector) {
            None => {}
            Some(Ok(())) => {
                if let Some(st) = self.bikes.as_deref_mut() {
                    (st.seq, st.sound) = (seq, true);
                }
            }
            Some(Err(_)) => {
                self.fats_unsure(); // compared again first
                self.notice("BIKES NOT SAVED");
            }
        }
    }

    /// The bikes may be changed: the file read first (now, if it was not); else why not, said.
    fn bikes_ready(&mut self, b: &mut impl Board) -> bool {
        let Some(st) = self.bikes.as_deref() else { return false };
        if self.ride.state() != ride::State::Idle {
            self.say("END THE RIDE FIRST");
            return false;
        }
        if !self.emmc.lent() && !st.loaded && self.settings_store.loaded {
            self.load_bikes(b);
        }
        let loaded = self.bikes().is_some();
        if !loaded {
            self.say(if self.emmc.lent() { "USB DRIVE ON" } else { "BIKES NOT READ" });
        }
        loaded
    }

    /// The bikes changed by the rider: written at the next pass.
    fn bikes_changed(&mut self) {
        let writable = self.card_writable;
        if let Some(st) = self.bikes.as_deref_mut() {
            st.dirty |= writable;
        }
    }

    /// ACTIVE BIKE: the next bike, its gear into the settings (the old one's kept in its own).
    pub(crate) fn next_bike(&mut self, b: &mut impl Board) {
        if !self.bikes_ready(b) {
            return;
        }
        let gear = bikes::Gear::of(&self.settings);
        let Some(st) = self.bikes.as_deref_mut() else { return };
        st.list.active_mut().gear = gear;
        st.list.active = (st.list.active + 1) % bikes::BIKES as u8;
        let (now, k) = (st.list.active().gear, usize::from(st.list.active));
        self.bikes_changed();
        now.apply(&mut self.settings);
        self.save_misc(b);
        self.say(bike_name(k));
    }

    /// RESET on part `p` of bike `k` (confirmed): back to 0 km.
    pub(crate) fn reset_part(&mut self, b: &mut impl Board, k: usize, p: usize) {
        if !self.bikes_ready(b) {
            return;
        }
        if let Some(part) = self.bikes.as_deref_mut().and_then(|s| s.list.list.get_mut(k)?.parts.get_mut(p)) {
            part.m = 0;
        }
        self.bikes_changed();
        self.say("PART RESET");
    }

    /// SERVICE EVERY on part `p` of bike `k`: the next interval.
    pub(crate) fn next_every(&mut self, b: &mut impl Board, k: usize, p: usize) {
        if !self.bikes_ready(b) {
            return;
        }
        if let Some(part) = self.bikes.as_deref_mut().and_then(|s| s.list.list.get_mut(k)?.parts.get_mut(p)) {
            let i = bikes::EVERY_KM.iter().position(|&e| e == part.every_km).map_or(0, |i| i + 1);
            part.every_km = bikes::EVERY_KM.get(i).copied().unwrap_or(0);
        }
        self.bikes_changed();
    }

    /// Row `i` of BIKES, of a bike's menu or of a part's.
    pub(crate) fn bike_rows(&self, menu: Menu, i: usize) -> Option<menu::Row> {
        use menu::Row;
        self.bikes.as_ref()?;
        match menu {
            Menu::Bikes if i == 0 => Some(Row::ActiveBike),
            Menu::Bikes => (i <= bikes::BIKES).then_some(Row::Open(Menu::BikeParts(i - 1))),
            Menu::BikeParts(k) if i == 0 => Some(Row::BikeOdo(k)),
            Menu::BikeParts(k) => (i <= bikes::PARTS).then_some(Row::Part(k, i - 1)),
            Menu::Part(k, p) => [Row::PartEvery(k, p), Row::PartReset(k, p)].get(i).copied(),
            _ => None,
        }
    }

    /// A bike row's value: the active bike, an odometer, a part's distance (green: none due; red: past its interval)
    /// or interval; "--" until the file is read.
    pub(crate) fn bike_value(&self, row: menu::Row, value: &mut Line<12>) -> u16 {
        use menu::Row;
        let Some(list) = self.bikes() else {
            value.push_str("--");
            return ui::WHITE;
        };
        let unit = if self.settings.imperial { " MI" } else { " KM" };
        let part = |k: usize, p: usize| list.list.get(k).and_then(|b| b.parts.get(p)).copied().unwrap_or_default();
        match row {
            Row::ActiveBike => value.push_u32(u32::from(list.active) + 1), // "BIKE 2" does not fit beside the label
            Row::Open(Menu::BikeParts(k)) | Row::BikeOdo(k) => {
                super::fields::push_trip(value, list.list.get(k).map_or(0, |b| b.odo_m), self.settings.imperial);
                value.push_str(unit);
            }
            Row::Part(k, p) | Row::PartReset(k, p) => {
                // whole km, as SERVICE EVERY (a label and "12345 KM" fit a row)
                value.push_u32(part(k, p).m / 1000);
                value.push_str(" KM");
                if part(k, p).due() {
                    return ui::RED;
                }
            }
            Row::PartEvery(k, p) => match part(k, p).every_km {
                0 => value.push_str("OFF"),
                km => {
                    value.push_u32(u32::from(km));
                    value.push_str(" KM");
                }
            },
            _ => {}
        }
        ui::WHITE
    }
}
