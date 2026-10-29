//! One pass of the main loop in parts ([`App::step`] calls them in order): the seconds gone, the alert said, the
//! sensors found while searching, the banner's time, the screen's own redraw. Each was a block of `step`.

use crate::*;

impl App {
    /// The RTC's seconds went on to `sec`: every second gone, not one a pass, so a pass that took longer (an eMMC or
    /// radio wait) loses no ride time.
    pub(crate) fn catch_up_seconds(&mut self, sec: u8) {
        if sec != self.seconds {
            // every second gone, not one a pass: a pass that took longer (an eMMC or radio wait) loses no ride time;
            // more than MAX_CATCH_UP is the clock set, not riding
            let gone = (sec + 60 - self.seconds) % 60;
            self.value_due = true; // the pages show this second's readings
            self.seconds = sec; // the time the recorder reads goes with the minute already read
            // each with its own time for the recorder: the first was `n - 1` seconds ago (the second review, R5)
            let n = gone.clamp(1, MAX_CATCH_UP);
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
            self.say_as(
                a.text(),
                if a == alerts::Alert::HrHigh || a == alerts::Alert::CadenceLow { Banner::Alarm } else { Banner::Ride },
            );
            self.tones.play(&BEEP_ALERT);
        }
    }

    /// Channels that sent data while searching: the chip says which sensor each found, and it is paired.
    pub(crate) fn pair_asked(&mut self, b: &mut impl Board) {
        for kind in Kind::ALL {
            if self.radio.ask & 1 << kind.channel() != 0
                && let Ok(Some(id)) = nrf::ant_channel_id(b, &mut self.rx, kind.channel())
            {
                self.radio.sensors[kind as usize] = Sensor::Paired(id);
                self.keep_pairing(b, kind, id);
                let mut l = Line::<20>::new();
                l.push_str(kind.name());
                l.push_str(" PAIRED");
                self.notice(l.as_str());
            }
        }
        self.radio.ask = 0;
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
            self.notes.ms = Some(ms.saturating_add(pass_ms)).filter(|&ms| ms < BANNER_MS);
            if self.notes.ms.is_none() {
                self.unbanner(b);
            }
        }
    }

    /// What the screen on needs drawn again this pass (the page's boxes that changed, a list, the card's map).
    pub(crate) fn redraw_screen(&mut self, b: &mut impl Board) {
        match self.screen {
            Screen::Main | Screen::Ride(_) | Screen::Edit(..) => self.update_page(b),
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
                ui::draw_list(&mut Panel(b, self.area(), self.look()), &self.radio.nearby);
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
                | Menu::System
                | Menu::BuiltIn
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
