//! One pass of the main loop in parts ([`App::step`] calls them in order): the seconds gone, the alert said, the
//! sensors found while searching, the banner's time, the screen's own redraw. Each was a block of `step`.

use crate::*;

impl App {
    /// The RTC went on (to `sec` in its minute, [`App::rtc_s`] in all): every second gone is ticked, however long
    /// the pass (an eMMC or radio wait, a stall), so no ride time is lost. A clock set is no second gone: its setter
    /// moves `counted_s` with it ([`App::phone_time`], the only one); a step back, or forward by more than
    /// [`CLOCK_STEP_S`], of any other cause counts one.
    ///
    /// Which second is what: the first ticked in a pass the RTC moved in, and any of the last [`REPEAT_S`], takes
    /// the pass's readings (`second`, as before 2026-11-02); an older one of a long stall is a gap (`gap_second`:
    /// no readings, the timer only, a record of invalid values).
    ///
    /// Bounded: [`CATCH_UP_PER_PASS`] a pass, and none after one that leaves a checkpoint due (its records written by
    /// this pass's [`App::step_recorder`] before more go into the buffer), so a long catch-up runs over passes. While
    /// a recording copy's buffer is down to its reserve (a card that cannot be written: `takes_backlog`) only the
    /// pass's new second is ticked; the backlog waits in `owed_s` rather than be dropped, and a pause or an end
    /// credits it to the timer at once (`start_pause`). Seconds still owed at a switch-off (about the last second; a
    /// backlog only right after a long stall or a long card failure) are not drained: lost.
    pub(crate) fn catch_up_seconds(&mut self, sec: u8) {
        let mut fresh = false;
        if self.rtc_s != self.rec.counted_s {
            let gone = self.rtc_s.saturating_sub(self.rec.counted_s);
            let gone = if (1..=i64::from(CLOCK_STEP_S)).contains(&gone) { gone } else { 1 };
            self.rec.owed_s = self.rec.owed_s.saturating_add(u32::try_from(gone).unwrap_or(1));
            self.rec.counted_s = self.rtc_s;
            self.value_due = true; // the pages show this second's readings
            self.seconds = sec; // the time the recorder reads goes with the minute already read
            fresh = true;
        }
        let held = !(self.rec.clean.takes_backlog() && self.rec.full.takes_backlog());
        let n = if held { u32::from(fresh) } else { CATCH_UP_PER_PASS };
        for k in 0..self.rec.owed_s.min(n) {
            // each with its own time for the recorder: the ones still owed after it (the second review, R5)
            self.rec.owed_s = self.rec.owed_s.saturating_sub(1);
            self.rec.lag_s = self.rec.owed_s;
            self.rec.gap = !(fresh && k == 0) && self.rec.lag_s >= REPEAT_S;
            self.second();
            if self.rec.clean.due() || self.rec.full.due() {
                break;
            }
        }
        (self.rec.lag_s, self.rec.gap) = (0, false);
    }

    /// The next alert due is said and beeped, unless the banner up is another alert's.
    pub(crate) fn say_alert(&mut self) {
        // one at a time: the next waits for the banner of the one before to go
        if !(self.notes.ms.is_some() && alerts::Alert::is_text(self.notes.text.as_str()))
            && let Some(a) = self.alert.pop()
        {
            self.say_as(a.text(), if a.alarm() { Banner::Alarm } else { Banner::Ride });
            self.tones.play(&BEEP_ALERT);
        }
    }

    /// Channels that sent data while searching: the chip says which sensor each found, and it is paired.
    pub(crate) fn pair_asked(&mut self, b: &mut impl Board) {
        for kind in Kind::ALL {
            if self.radio.ask & 1 << kind.channel() != 0
                && let Ok(Some(id)) = nrf::ant_channel_id(b, &mut self.rx, kind.channel())
            {
                *self.radio.sensors.at_mut(kind) = Sensor::Paired(id);
                self.decoders.forget_ids(kind); // another sensor's percent and maker, if it had sent them
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
            self.notes.ms = Some(ms.saturating_add(pass_ms)).filter(|&ms| ms < self.notes.kind.ms());
            if self.notes.ms.is_none() {
                self.unbanner(b);
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
