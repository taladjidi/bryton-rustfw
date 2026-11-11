//! One pass of the main loop in parts ([`App::step`] calls them in order): the seconds gone, the alert said, the
//! sensors found while searching, the banner's time, the screen's own redraw. Each was a block of `step`.

use crate::*;

impl App {
    /// The RTC went on (to `sec` in its minute, [`App::rtc_s`] in all): every second gone counts, however long the
    /// pass (an eMMC or radio wait, a stall), so no ride time is lost (the owner, 2026-11-02: no cap). The last
    /// [`REPEAT_S`] are ticked one by one with the pass's readings, as before; the older ones of a longer stall go on
    /// the timer at once ([`ride::Ride::gap_seconds`], as the ride was moving or not before the stall), with no
    /// records: the file's times jump over them (a reader takes records at any interval). A clock set is no second
    /// gone: its setter moves `counted_s` with it ([`App::phone_time`], the only one); a step back, or forward by
    /// more than [`CLOCK_STEP_S`], counts one.
    ///
    /// What follows from the seconds put on the timer at once: the eat and drink reminders crossed are said
    /// (`alerts::Alerts::second`); a workout's step clock and the ramp test follow ridden seconds only, so a stall does
    /// not move them on; a stall longer than a time auto-lap makes one long lap; the FIT file holds up to 4 s x the
    /// speed more distance than the screen for each stall over [`REPEAT_S`] (the writer's 5 s cap on the time between
    /// two records, against the 5 s ticked), accepted.
    pub(crate) fn catch_up_seconds(&mut self, sec: u8) {
        if self.rtc_s != self.rec.counted_s {
            let gone = self.rtc_s.saturating_sub(self.rec.counted_s);
            let gone = if (1..=i64::from(CLOCK_STEP_S)).contains(&gone) { gone as u32 } else { 1 };
            self.rec.counted_s = self.rtc_s;
            self.value_due = true; // the pages show this second's readings
            self.seconds = sec; // the time the recorder reads goes with the minute already read
            let n = u8::try_from(gone).unwrap_or(REPEAT_S).min(REPEAT_S);
            if gone > u32::from(n) {
                self.ride.gap_seconds(gone - u32::from(n));
                self.rec.clean.checkpoint_soon(); // the timer's new seconds on the card now, not a minute of records on
                self.rec.full.checkpoint_soon();
            }
            // each with its own time for the recorder: the first was `n - 1` seconds ago (the second review, R5)
            for k in 0..n {
                self.rec.lag_s = n - 1 - k;
                self.second();
            }
            self.rec.lag_s = 0;
        }
    }

    /// The next alert due is said and beeped, unless the banner up is another alert's.
    pub(crate) fn say_alert(&mut self) {
        // one at a time: the next waits for the banner of the one before to go
        if !(self.notes.ms.is_some() && alerts::Alert::is_text(self.notes.text.as_str()))
            && let Some(a) = self.alert.pop()
        {
            self.say_as(a.text(), if a.alarm() { Banner::Alarm } else { Banner::Warn });
            self.tones.play(&BEEP_ALERT);
        }
    }

    /// Channels that sent data while searching: the chip says which sensor each found, and it is paired.
    pub(crate) fn pair_asked(&mut self, b: &mut impl Board) {
        for kind in Kind::ALL {
            if self.radio.ask & 1 << kind.channel() != 0
                && let Ok(Some(id)) = nrf::ant_channel_id(b, &mut self.rx, kind.channel())
                && self.radio.found.at(kind) != Some(id)
            {
                *self.radio.found.at_mut(kind) = Some(id);
                let mut l = Line::<40>::new();
                l.push_str(kind.short());
                l.push_str(" ");
                l.push_u32(u32::from(id.device));
                l.push_str(" FOUND\nSTART: PAIR"); // the longest, SPD+CAD 65535, with the notes' time: 37 of 40
                self.notice(l.as_str());
            }
        }
        self.radio.ask = 0;
    }

    /// Tests: the rider's confirm of the device a search heard, if one was.
    #[cfg(test)]
    pub(crate) fn confirm_found(&mut self, b: &mut impl Board, kind: Kind) {
        if let Some(id) = self.radio.found.at(kind) {
            self.pair_found(b, kind, id);
        }
    }

    /// The rider confirmed the device a search heard: pair it.
    pub(crate) fn pair_found(&mut self, b: &mut impl Board, kind: Kind, id: nrf::SensorId) {
        *self.radio.found.at_mut(kind) = None;
        *self.radio.sensors.at_mut(kind) = Sensor::Paired(id);
        self.decoders.forget_ids(kind); // another sensor's percent and maker, if it had sent them
        self.keep_pairing(b, kind, id);
        let mut l = Line::<20>::new();
        l.push_str(kind.name());
        l.push_str(" PAIRED");
        self.notice(l.as_str());
    }

    /// The banner's time goes on by `pass_ms`; it is drawn once, and the screen under it comes back when it goes.
    pub(crate) fn run_banner(&mut self, b: &mut impl Board, pass_ms: u32) {
        // under an incoming call a banner waits (the call says who calls there, end_ring shows it)
        if let Some(ms) = self.notes.ms.filter(|_| !self.phone.ringing) {
            if !core::mem::replace(&mut self.notes.drawn, true) {
                ui::draw_banner(
                    &mut Panel(b, (0, ui::H), self.look()),
                    self.notes.text.as_str(),
                    self.notes.kind == Banner::Alarm,
                );
            }
            self.notes.ms = Some(ms.saturating_add(pass_ms)).filter(|&ms| ms < self.notes.kind.ms());
            if self.notes.ms.is_none() {
                // a row's "SURE?" goes with its banner: a stray Start later must not act with no prompt shown
                let armed = self.menu.confirm.is_some_and(|c| c != PAST_DELETE);
                if armed {
                    self.menu.confirm = None;
                }
                self.unbanner(b);
                if armed && let Screen::Menu(m, sel) = self.screen {
                    self.update_menu_rows(b, m, sel); // its "SURE?" goes
                }
            }
        }
    }

    /// What the screen on needs drawn again this pass (the page's boxes that changed, a list, the card's map).
    pub(crate) fn redraw_screen(&mut self, b: &mut impl Board) {
        self.track_ride_menu(b);
        match self.screen {
            Screen::Main | Screen::Ride(_) | Screen::Edit(..) | Screen::Status => self.update_page(b),
            Screen::Summary => {
                // the elevation stays as it was; the map keeps up with points still coming (the unit moving on)
                let (area, look) = (self.area(), self.look());
                if let Some((m, _)) = &mut self.summary {
                    let (base, view) = (Some(&self.basemap), m.view);
                    map::update(
                        &mut Panel(b, area, look),
                        map::area(ui::card::MAP),
                        &self.track,
                        m,
                        base,
                        None,
                        None,
                        false,
                    );
                    if m.view != view {
                        self.show(b); // refitted: the basemap of the new view under it (else black; review)
                    }
                }
            }
            Screen::Nearby if self.list_dirty && self.redraw_ms >= LIST_REDRAW_MS => {
                ui::draw_list(
                    &mut Panel(b, self.area(), self.look()),
                    &self.radio.nearby,
                    Some(&mut self.menu.list_shown),
                );
                self.list_dirty = false;
                self.redraw_ms = 0;
            }
            Screen::Menu(
                menu @ (Menu::Main
                | Menu::Sensors
                | Menu::Gps
                | Menu::Stock
                | Menu::History
                | Menu::Courses
                | Menu::Segments
                | Menu::Climbs
                | Menu::Months
                | Menu::System
                | Menu::BuiltIn
                | Menu::Storage
                | Menu::Ride(_)
                | Menu::Phone
                | Menu::BtTrainers),
                sel,
            ) if self.redraw_ms >= LIST_REDRAW_MS => {
                self.update_menu_rows(b, menu, sel);
                self.redraw_ms = 0;
            }
            _ => {}
        }
    }
}
