//! What is on screen: [`App::show`] draws a screen whole when it opens; [`App::update_page`] then redraws only the
//! page boxes whose content changed (AGENTS.md, redraw only what changes); the banner over it; the ride card; the
//! basemap under the first map field; and the screenshot rows ([`App::render_row`]).

use crate::*;

/// How long a banner stays up, unless an input dismisses it first.
pub const BANNER_MS: u32 = 4000;

/// How long a lap's summary stays up.
pub const LAP_BANNER_MS: u32 = 8000;

/// How long the ready card stays up (app/start_cues.rs).
pub const READY_BANNER_MS: u32 = 3000;

/// A banner's kind, lowest first: a higher one is not covered by a lower one ([`App::say_as`]). An incoming call is
/// above them all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Banner {
    /// What a key just did, a notice.
    Info,
    /// A lap's summary: stays [`LAP_BANNER_MS`], over what a key said, under a turn, a workout step or an alarm.
    Summary,
    /// The ready card at a ride's start: stays [`READY_BANNER_MS`], over what a key said, under everything the ride says.
    Ready,
    /// A turn ahead.
    Cue,
    /// What the ride says: a workout step, a reminder, a climb.
    Ride,
    /// A turn now.
    Now,
    /// An alarm (heart rate high, off course, the battery low, a ride not saving): white on red.
    Alarm,
}

impl Banner {
    /// How long this kind stays up.
    pub(crate) fn ms(self) -> u32 {
        match self {
            Banner::Summary => LAP_BANNER_MS,
            Banner::Ready => READY_BANNER_MS,
            _ => BANNER_MS,
        }
    }

    /// A turn cue, ahead or now: good for its moment only.
    pub(crate) fn is_cue(self) -> bool {
        matches!(self, Banner::Cue | Banner::Now)
    }
}

/// What a page box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Widget {
    /// The TODAY card: the workout planned for today, or REST DAY.
    Today,
    /// The date line.
    Date,
    /// The latest notice; nothing is drawn when there is none (Ferris lives in ABOUT).
    Notices,
    /// A ride field in its box: a value, the map or the graph.
    Field(Field),
}

/// A page box as drawn: each pass compares it with what the box would show now and redraws only the difference
/// (the digits, the characters, the map's new points, the graph's columns that changed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// Nothing that changes.
    #[default]
    Fixed,
    /// A fingerprint of what the box shows, redrawn whole when it changes (the date, the notices).
    Print(u32),
    /// A value, its colour and its zone bar's (BLACK: none).
    Value(Line<8>, u16, u16),
    /// A map.
    Map(map::Shown),
    /// A graph.
    Graph(GraphShown),
    /// The course profile.
    Profile(ProfileShown),
    /// The workout graph.
    Workout(WorkoutShown),
    /// The workout gauge.
    Gauge(GaugeShown),
    /// The CLIMB page's head (the segment's name and delta, or the climb's title).
    Head(ui::segment::HeadShown),
}

/// Boxes on a page at most.
pub(crate) const BOXES: usize = 10; // the fields overview: date, notices, 4 chosen fields, 4 of the table

impl App {
    /// Memory for the basemap (at least [`tiles::MEM_WORDS`] words); without it the map has no basemap.
    pub fn set_map_memory(&mut self, mem: &'static mut [u16]) {
        self.basemap.set_memory(mem);
    }

    /// The last basemap view change's cost.
    #[cfg(test)]
    pub fn basemap_stats(&self) -> tiles::Stats {
        self.basemap.stats
    }

    /// Ride page `n`'s first map field: the one with the basemap.
    pub(crate) fn first_map(&self) -> Option<usize> {
        self.widgets().position(|(_, w)| w == Widget::Field(Field::Map))
    }

    /// Draws the basemap of `view` in `area` into its buffer, unless it holds it already.
    pub(crate) fn render_basemap(&mut self, b: &mut impl Board, area: map::Rect, view: Option<map::View>) {
        if let (Some(v), Some(o), false) = (view, self.track.origin(), self.emmc.lent()) {
            let t = b.cycles();
            self.basemap.render(&mut emmc::Volume(&mut self.emmc, b), area, v, o);
            self.timed(b, crate::prof::Sec::Tiles, t);
        }
    }

    /// What is on screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// A ride runs (or its card is on screen): what a reset or switching off would lose (main.rs does not reset on a
    /// USB unplug then).
    pub fn busy(&self) -> bool {
        self.ride.state() != ride::State::Idle || self.screen == Screen::Summary
    }

    /// The card of the ride (the one running, or the last one): its numbers and title, as a past ride's
    /// ([`history::Ride::stats`]).
    pub(crate) fn ride_card(&self) -> ui::card::Card {
        let (r, p) = (&self.ride, &self.power);
        ui::card::Card::of(
            &ui::card::Stats {
                start: (self.ride_started.date, self.ride_started.time),
                timer_s: r.timer_s(),
                dist_m: r.dist_m(),
                avg_speed_x10: r.avg_speed_x10(),
                ascent_m: Some(r.ascent_m()),
                np: p.np(),
                avg_power: r.avg_power(),
                avg_hr: r.avg_hr(),
                max_hr: r.max_hr(),
                tss: self.tss_now(),
                kj: Some(p.kj()),
            },
            self.settings.imperial,
        )
    }

    /// The card screen: the track in [`ui::card::MAP`] over the basemap, the whole ride's elevation, the numbers, as
    /// they were when the card was shown ([`App::summary`]).
    pub(crate) fn draw_summary(&self, c: &mut impl Canvas) {
        let Some((m, g)) = self.summary else { return };
        let (_, _, cols, h) = ui::card::ELEV;
        let base = Some(&self.basemap);
        ui::card::draw(c, &self.ride_card(), (&self.track, m.view, base), &|i| self.graph_bar(g, cols, h, i));
    }

    /// The card's map on screen (the ride just ended, a past ride): its view and its track's origin.
    fn card_view(&self) -> Option<(map::View, (i32, i32))> {
        match self.screen {
            Screen::Summary => self.summary.and_then(|(m, _)| m.view).zip(self.track.origin()),
            Screen::Past => {
                let p = &self.past.track;
                map::View::fit_whole(p, map::area(ui::card::MAP)).zip(p.origin())
            }
            _ => None,
        }
    }

    /// Draws the whole screen below the bar from the state (the panel was cleared, or a screenshot row).
    pub(crate) fn draw_screen(&self, c: &mut impl Canvas) {
        match self.screen {
            Screen::Main | Screen::Ride(_) | Screen::Edit(..) => {
                let (_, cy, _, ch) = c.clip();
                for (i, (bx, w)) in self.widgets().enumerate() {
                    // a box wholly outside the clip (a banner's redraw over the page's lower half) would be computed
                    // (every graph bar) then discarded by the panel: not drawn
                    if matches!(w, Widget::Field(_)) && (bx.1 >= cy + ch || bx.1 + bx.3 <= cy) {
                        continue;
                    }
                    self.draw_widget(c, i, bx, w);
                }
                self.draw_edit_frame(c);
            }
            Screen::Menu(menu, sel) => {
                let mut title = Line::<16>::new();
                match menu {
                    Menu::Page(OVERVIEW) => title.push_str("OVERVIEW"),
                    Menu::Steps if !self.plan.workout.name().is_empty() => title.push_str(self.plan.workout.name()),
                    Menu::Edit(k) => {
                        title.push_str("PAGE ");
                        title.push_u32(self.edit.map_or(0, |e| e.page) as u32 + 1);
                        title.push_str(" BOX ");
                        title.push_u32(k as u32 + 1);
                    }
                    Menu::Page(n) => self.page_label(n, &mut title),
                    _ => title.push_str(menu_title(menu)),
                }
                ui::draw_title(c, title.as_str());
                self.draw_menu_rows(c, menu, sel);
            }
            Screen::Summary => self.draw_summary(c),
            Screen::End(sel) | Screen::Discard(sel) => {
                ui::draw_end(c, matches!(self.screen, Screen::Discard(_)), self.end_stats().as_str(), sel)
            }
            Screen::Past => {
                let (p, h) = (&self.past, ui::card::ELEV.3);
                let view = map::View::fit_whole(&p.track, map::area(ui::card::MAP));
                ui::card::draw(c, &p.card, (&p.track, view, Some(&self.basemap)), &|i| p.elev(i, h));
                ui::card::draw_delete(c);
            }
            Screen::Media => {
                ui::media::draw(c, &self.media_now());
            }
            Screen::Quick => self.draw_quick(c),
            Screen::Trainer => self.draw_trainer(c),
            Screen::SegmentResult => self.draw_segment_result(c),
            Screen::RampResult => self.draw_ramp(c),
            Screen::PowerCal => self.draw_powercal(c),
            Screen::Notif(k, top) => self.draw_notif(c, k, top),
            Screen::About => ui::draw_about(c, env!("CARGO_PKG_VERSION"), env!("GIT_DESCRIBE"), env!("GIT_DATE")),
            Screen::Nearby => {
                ui::draw_back(c); // the title itself is drawn again as the count changes, the "<" once
                if self.ble.is_some() {
                    ui::draw_nearby(c, self.radio.nearby.count(), self.radio.scanning);
                }
                ui::draw_list(c, &self.radio.nearby, None);
            }
        }
        if self.notes.ms.is_some() && self.notes.drawn {
            ui::draw_banner(c, self.notes.text.as_str(), self.notes.kind == Banner::Alarm);
        }
        if self.phone.ringing {
            ui::draw_call(c, self.phone.call.as_ref().map_or("", |(_, who)| who.as_str()));
        }
    }

    /// The boxes of the page on screen and what each holds (none on a menu or a list). The status strip is not
    /// one.
    pub(crate) fn widgets(&self) -> impl Iterator<Item = ((u32, u32, u32, u32), Widget)> + Clone + use<> {
        let mut all = [((0, 0, 0, 0), Widget::Today); BOXES];
        let table = self.screen == Screen::Main && self.theme.overview == ui::theme::Overview::Fields;
        let (page, fixed): (_, &[Widget]) = match self.screen {
            Screen::Main if table => (OVERVIEW, &[Widget::Date, Widget::Notices]),
            Screen::Main => (OVERVIEW, &[Widget::Today, Widget::Date, Widget::Notices]),
            Screen::Ride(n) => (n, &[]),
            Screen::Edit(..) => (EDITED, &[]),
            _ => return all.into_iter().take(0),
        };
        let more = self.more_shown() == Some(page);
        let p = match page {
            COURSE => page::Page::from_cfg(COURSE_PAGE).unwrap_or(BLANK),
            CLIMB => page::Page::from_cfg(CLIMB_PAGE).unwrap_or(BLANK),
            WORKOUT => page::Page::from_cfg(WORKOUT_PAGE).unwrap_or(BLANK),
            EDITED => self.page_ref(EDITED).copied().unwrap_or(BLANK),
            n if more => self.more_page(n),
            n => self.ride_page_ref(n).copied().unwrap_or(BLANK),
        };
        let mut n = 0;
        for (slot, &w) in all.iter_mut().zip(fixed) {
            slot.1 = w;
            n += 1;
        }
        let taps = self.plot_taps.get(page).filter(|_| !more); // a tap's plot is per box of the normal page
        for (i, &f) in p.fields.iter().enumerate() {
            // a plot chosen by a tap, while the box still holds a graph
            let tap = taps.and_then(|t| t.get(i).copied().flatten()).filter(|_| graph_plot(f).is_some());
            let f = tap.unwrap_or(f);
            let bx = if page == OVERVIEW { ui::overview_box(i) } else { p.cell_box(i).map(Into::into) };
            let (Some(bx), Some(slot)) = (bx, all.get_mut(n)) else { break };
            *slot = (bx, Widget::Field(f));
            n += 1;
        }
        for (i, f) in ui::TABLE.into_iter().enumerate().filter(|_| table) {
            let (Some(bx), Some(slot)) = (ui::table_box(i), all.get_mut(n)) else { break };
            *slot = (bx, Widget::Field(f));
            n += 1;
        }
        all.into_iter().take(n)
    }

    /// What box `bx` holding `w` would show now.
    pub(crate) fn now_drawn(&self, bx: (u32, u32, u32, u32), w: Widget) -> Drawn {
        match w {
            Widget::Today => Drawn::Print(self.today_print()),
            Widget::Date => {
                Drawn::Print(u32::from(self.date.0) << 16 | u32::from(self.date.1) << 8 | u32::from(self.date.2))
            }
            Widget::Notices => {
                Drawn::Print(self.notes.list.iter().fold(0x811c_9dc5, |h, n| fnv(h, n.as_str().as_bytes())))
            }
            Widget::Field(Field::Map) => Drawn::Map(map::Shown::of(
                &self.track,
                map::area(bx),
                self.route().as_ref(),
                self.zoom(),
                self.page_follow(),
            )),
            Widget::Field(f @ (Field::Profile | Field::ClimbProfile)) => {
                Drawn::Profile(self.profile_now(ui::plot_area(bx).2, f == Field::ClimbProfile))
            }
            Widget::Field(Field::WorkoutGraph) => Drawn::Workout(self.workout_now(ui::plot_area(bx).2)),
            Widget::Field(Field::SegHead) => {
                Drawn::Head(self.head_text().map_or_else(Default::default, |h| h.head().shown()))
            }
            Widget::Field(Field::WktPower) => Drawn::Gauge(self.gauge_now(ui::gauge_cols(bx))),
            Widget::Field(Field::ClimbPace) => Drawn::Gauge(self.pace_now(ui::gauge_cols(bx))),
            Widget::Field(f) => match graph_plot(f) {
                Some(plot) => Drawn::Graph(self.graph_now(plot, ui::graph_area(bx).2)),
                None => Drawn::Value(self.field_text(f), self.field_color(f), self.field_bar(f)),
            },
        }
    }

    /// Box `i` (`bx`, holding `w`) whole, as drawn ([`App::drawn`]): after the page was cleared, or a screenshot row.
    pub(crate) fn draw_widget(&self, c: &mut impl Canvas, i: usize, bx: (u32, u32, u32, u32), w: Widget) {
        let d = self.drawn.get(i).copied().unwrap_or_default();
        match (w, d) {
            (Widget::Today, _) => self.draw_today(c),
            // under a call's two-line banner the date row would show its lower half: left out, the box cleared
            (Widget::Date, _) if self.phone.ringing => {}
            (Widget::Date, _) => ui::draw_date(c, date_text(self.date).as_str()),
            (Widget::Notices, _) => {
                let mut n: [&str; ui::NOTICES] = [""; ui::NOTICES];
                for (slot, line) in n.iter_mut().zip(&self.notes.list) {
                    *slot = line.as_str();
                }
                let shown = self.notes.list.iter().take_while(|l| !l.as_str().is_empty()).count().min(ui::NOTICES);
                ui::draw_notices(c, n.get(..shown).unwrap_or(&[]));
            }
            (Widget::Field(f), d) => {
                ui::draw_field_frame_bar(
                    c,
                    bx,
                    self.avg_field(f),
                    if let Drawn::Value(_, _, bar) = d { bar } else { ui::BLACK },
                    (self.settings.imperial, self.settings.temp_f),
                );
                match d {
                    Drawn::Map(m) => {
                        let base = (self.first_map() == Some(i)).then_some(&self.basemap);
                        map::draw(
                            c,
                            map::area(bx),
                            &self.track,
                            m.view,
                            base,
                            self.route().as_ref(),
                            m.zoomed() || m.route_drawn(),
                        );
                    }
                    Drawn::Profile(p) => self.draw_profile(c, bx, p),
                    Drawn::Workout(w) => self.draw_workout_graph(c, bx, w),
                    Drawn::Gauge(g) => self.draw_gauge(c, bx, g),
                    Drawn::Head(_) => {
                        if let Some(h) = self.head_text() {
                            ui::segment::draw_head(c, bx, &h.head());
                        }
                    }
                    Drawn::Graph(g) => {
                        self.draw_graph_axes(c, bx, g);
                        let a = ui::graph_area(bx);
                        ui::draw_graph(c, a, &|k| self.graph_bar(g, a.2, a.3, k));
                    }
                    Drawn::Value(text, fg, _) => ui::draw_field_value(c, bx, text.as_str(), fg),
                    _ => {}
                }
            }
        }
    }

    /// A page's map follows the rider at the default level (no zoom set by hand): outdoors, and unless the crosshair
    /// showed the whole route. Indoors, the fit.
    pub(crate) fn page_follow(&self) -> bool {
        !self.indoors() && !self.map_whole
    }

    /// `f`, but a 3 / 10 / 30 s power field as the window chosen by a tap (its label).
    pub(crate) fn avg_field(&self, f: Field) -> Field {
        match f {
            // the editor shows the field the rider picked
            Field::Power3s | Field::Power10s | Field::Power30s if !matches!(self.screen, Screen::Edit(..)) => {
                [Field::Power3s, Field::Power10s, Field::Power30s][self.settings.wkt_avg.min(2)]
            }
            f => f,
        }
    }

    /// A tap on a power average field of the page: the window goes to the next (3, 10, 30 s, round), kept, and
    /// those boxes are drawn again, each in its own box.
    pub(crate) fn cycle_wkt_avg(&mut self, b: &mut impl Board) {
        self.settings.wkt_avg = (self.settings.wkt_avg + 1) % 3;
        self.save_misc(b);
        for (i, (bx, w)) in self.widgets().enumerate() {
            if !matches!(w, Widget::Field(Field::Power3s | Field::Power10s | Field::Power30s)) {
                continue;
            }
            let (x, y, bw, bh) = bx;
            let mut p = Panel(b, self.area(), self.look());
            p.fill(x, y, bw, bh, ui::BLACK); // the label may be shorter now
            let now = self.now_drawn(bx, w);
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
            self.draw_widget(&mut Panel(b, self.area(), self.look()), i, bx, w);
        }
        self.update_page(b); // the gauge and the pacer follow
    }

    /// Whether (`x`, `y`) is in a power average field of the page.
    pub(crate) fn wkt_avg_at(&self, x: u16, y: u16) -> bool {
        self.widgets().any(|(bx, w)| {
            matches!(w, Widget::Field(Field::Power3s | Field::Power10s | Field::Power30s)) && inside(bx, x, y)
        })
    }

    /// The map's zoom set by hand, if any.
    pub(crate) fn zoom(&self) -> Option<map::Zoom> {
        self.map_zoom.map(|level| map::Zoom { level, centre: self.map_centre })
    }

    /// The page's (first) map: its area and how it is drawn.
    pub(crate) fn page_map(&self) -> Option<(map::Rect, map::Shown)> {
        let i = self.first_map()?;
        match (self.widgets().nth(i), self.drawn.get(i)) {
            (Some((bx, _)), Some(Drawn::Map(m))) => Some((map::area(bx), *m)),
            _ => None,
        }
    }

    /// A tap at (`x`, `y`) on the page's map's buttons: the zoom it asks for (None: the fit); None elsewhere. "+"
    /// and "-" keep the centre panned to; the crosshair goes back to the fit.
    pub(crate) fn zoom_at(&self, x: u16, y: u16) -> Option<Option<map::Zoom>> {
        let (area, m) = self.page_map()?;
        let (level, centre) = match map::button_at(area, &m, i32::from(x), i32::from(y))? {
            map::Button::Centre => (None, None),
            b => (map::zoom(&m, b == map::Button::In), self.map_centre),
        };
        Some(level.map(|level| map::Zoom { level, centre }))
    }

    /// A drag from (`x`, `y`) pans the page's map: it starts on a map zoomed in by hand.
    pub(crate) fn pans_at(&self, x: u16, y: u16) -> bool {
        let (x, y) = (i32::from(x), i32::from(y));
        matches!(self.screen, Screen::Ride(_))
            && self
                .page_map()
                .is_some_and(|(a, m)| m.zoomed() && (a.0..a.0 + a.2).contains(&x) && (a.1..a.1 + a.3).contains(&y))
    }

    /// The page's map's centre after a drag of (`dx`, `dy`) pixels ([`map::pan`]); None when it does not pan.
    pub(crate) fn pan(&self, dx: i16, dy: i16) -> Option<(i32, i32)> {
        let (area, m) = self.page_map()?;
        map::pan(&self.track, area, self.route().as_ref(), &m, (i32::from(dx), i32::from(dy)))
    }

    /// The map in box `i` as drawn (tests).
    #[cfg(test)]
    pub(crate) fn map_view(&self, i: usize) -> Option<map::View> {
        match self.drawn.get(i) {
            Some(Drawn::Map(m)) => m.view,
            _ => None,
        }
    }

    /// The rows the page may be drawn in (top, bottom): under the banner while one (or a call) is up, above a call's
    /// buttons.
    pub(crate) fn area(&self) -> (u32, u32) {
        let top = if self.phone.ringing || self.notes.ms.is_some() { ui::BANNER_H } else { 0 };
        (top, if self.phone.ringing { ui::CALL_Y } else { ui::H })
    }

    /// The banner is gone: what is under it (the strip, the page's top) drawn again, from what the screen shows.
    pub(crate) fn unbanner(&mut self, b: &mut impl Board) {
        self.notes.ms = None;
        self.redraw_top(b);
        self.notes.show_waiting();
    }

    /// The banner's rows drawn again from what the screen shows: the strip, the page's top, the banner if one is up.
    pub(crate) fn redraw_top(&mut self, b: &mut impl Board) {
        let mut p = Panel(b, (0, ui::BANNER_H), self.look());
        p.fill(0, 0, ui::W, ui::BANNER_H, ui::BLACK);
        if !self.hint_shown {
            self.strip_shown = self.strip; // not drawn while covered (update_strip)
            self.plan.top = self.ahead();
        }
        self.draw_top(&mut p);
        self.draw_screen(&mut p);
    }

    /// Adds a notice (newest first), stamped with the time: what the unit itself has to say (a fix, a sensor
    /// paired, the battery low); phone notifications will come here too.
    pub(crate) fn notice(&mut self, text: &str) {
        self.notice_as(text, Banner::Info);
    }

    /// A notice that is an alarm (the battery low, a ride not saving): its banner white on red.
    pub(crate) fn alarm_notice(&mut self, text: &str) {
        self.notice_as(text, Banner::Alarm);
    }

    /// A notice, its banner of `kind`.
    fn notice_as(&mut self, text: &str, kind: Banner) {
        self.value_due = true; // the overview's notices box follows
        let mut l = Line::new();
        l.push_str(hm_text(self.strip.time).as_str());
        l.push_str(" ");
        l.push_str(text);
        self.notes.push(l);
        // over the overview the notice is in its own box already; the banner without the time (the strip has it)
        // and not over a ride's card, whose title the banner would cover (an alarm still must be seen)
        if self.screen != Screen::Main && (self.screen != Screen::Summary || kind == Banner::Alarm) {
            self.say_as(text, kind);
        }
    }

    /// Shows `text` in the banner for [`BANNER_MS`] (what a key just did, a notice).
    pub(crate) fn say(&mut self, text: &str) {
        self.say_as(text, Banner::Info);
    }

    /// Shows `text` in a red banner: an alarm (heart rate high, off course).
    pub(crate) fn alarm(&mut self, text: &str) {
        self.say_as(text, Banner::Alarm);
    }

    /// Shows `text` as a banner of `kind`. One up of a higher kind stays and this one waits; one up of a lower kind
    /// waits for this one; one of the same kind is replaced (2026-10-25 review: a turn said again after the next
    /// one, LAP twice shown 8 s). One waits at most (the higher kind kept, else the newer) and shows when the
    /// banner up goes ([`App::unbanner`]): an alarm over a workout step, the step after it. A cue is good for its
    /// moment only: one waiting, or one up pushed off, goes when another cue is said. An incoming call is above
    /// them all: a banner waits under it ([`App::area`]).
    pub(crate) fn say_as(&mut self, text: &str, kind: Banner) {
        self.notes.say(text, kind);
    }

    /// Clears the page area and draws the current screen.
    pub(crate) fn show(&mut self, b: &mut impl Board) {
        if matches!(self.screen, Screen::Ride(_)) {
            self.more_on = self.more_shown(); // MORE is up only over its own page; a menu over it keeps it
        }
        self.strip.more = self.more_shown().is_some();
        // the overview's card waits for the start-up delay; a page with a workout field reads the card at first use
        // not on the fields overview: its table's Workout field must not read the card at start (CALENDAR_MS)
        let table = self.screen == Screen::Main && self.theme.overview == ui::theme::Overview::Fields;
        let field = self.widgets().any(|(_, w)| w == Widget::Field(Field::Workout)) && !table;
        if field || self.screen == Screen::Main {
            self.refresh_calendar(b, field);
        }
        self.refresh_done();
        for (i, (bx, w)) in self.widgets().enumerate() {
            let now = self.now_drawn(bx, w);
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
        if self.screen == Screen::Summary {
            let (_, _, cols, _) = ui::card::ELEV;
            let m = map::Shown::of(&self.track, map::area(ui::card::MAP), None, None, false);
            self.summary = Some((m, self.graph_summary(cols)));
        }
        if let (Some((v, o)), false) = (self.card_view(), self.emmc.lent()) {
            // a card's map over the basemap too (the owner, 2026-10-24); the page's is drawn again when it shows
            let t = b.cycles();
            self.basemap.render(&mut emmc::Volume(&mut self.emmc, b), map::area(ui::card::MAP), v, o);
            self.timed(b, crate::prof::Sec::Tiles, t);
        }
        if let Some(i) = self.first_map()
            && let (Some((bx, _)), Some(Drawn::Map(m))) = (self.widgets().nth(i), self.drawn.get(i))
        {
            let view = m.view;
            self.render_basemap(b, map::area(bx), view);
        }
        let mut p = Panel(b, self.area(), self.look());
        match self.screen {
            // a menu's rows paint their whole boxes: clearing under them first doubled a menu's cost (2026-10-26)
            Screen::Menu(..) => ui::clear_menu(&mut p),
            // so does the TODAY card (ui::draw_today): a fifth of the overview's (25 288 of 134 745 px)
            Screen::Main if self.theme.overview == ui::theme::Overview::Classic => {
                ui::clear_page_but(&mut p, ui::TODAY_BOX)
            }
            _ => ui::clear_page(&mut p),
        }
        self.draw_screen(&mut p);
        let hint = matches!(self.screen, Screen::Edit(..));
        if hint != self.hint_shown {
            // into the editor or out of it: its hint in the strip's place, or the strip back
            self.hint_shown = hint;
            self.strip_shown = self.strip;
            self.draw_top(&mut Panel(b, self.area(), self.look()));
        }
        if self.screen == Screen::Nearby {
            self.list_shown = ui::list_prints(&self.radio.nearby); // drawn whole by draw_screen
        }
        if let Screen::Menu(menu, sel) = self.screen {
            let first = menu::first_shown(sel, self.menu_rows(), self.rows(menu));
            self.menu_shown = core::array::from_fn(|i| self.menu_row_print(menu, sel, first, i));
            self.menu_head = core::array::from_fn(|i| self.menu_print(menu, sel, first, i, false));
        }
        if self.screen == Screen::Media {
            self.phone.shown = ui::media::shown(&self.media_now());
        }
        if self.screen == Screen::Quick {
            self.quick_drawn();
        }
        if self.screen == Screen::Trainer {
            self.trainer_printed();
        }
        if self.strip.more != self.strip_shown.more {
            self.update_strip(b); // the MORE marker
        }
    }

    /// Redraws what changed in the page's boxes since they were drawn; in the editor, the frame again over the
    /// selected box when it changed.
    pub(crate) fn update_page(&mut self, b: &mut impl Board) {
        self.refresh_done();
        if self.screen == Screen::Main {
            self.refresh_calendar(b, false); // after the start, midnight, or new zones: the card follows
        }
        // Everything but the map follows the readings of the second (the pages show one reading a second, bars and
        // the workout move by the second); between seconds only an input (it acts at once) or the map needs a look.
        let due = core::mem::take(&mut self.value_due) || self.light.idle_ms < 1000;
        let widgets = self.widgets(); // built once a pass
        let first = widgets.clone().position(|(_, w)| w == Widget::Field(Field::Map));
        let selected = if let Screen::Edit(_, k) = self.screen { Some(k) } else { None };
        for (i, (bx, w)) in widgets.enumerate() {
            let Some(&old) = self.drawn.get(i) else { break };
            if !due && !matches!(old, Drawn::Map(_)) {
                continue;
            }
            let now = match (w, old) {
                (_, Drawn::Map(mut m)) => {
                    let area = map::area(bx);
                    let (zoom, follow) = (self.zoom(), self.page_follow());
                    let refit = first == Some(i) && {
                        let route = self.route();
                        map::refit(area, &self.track, &m, route.as_ref(), zoom, follow)
                    };
                    if refit {
                        let route = self.route();
                        let view = map::Shown::of(&self.track, area, route.as_ref(), zoom, follow).view;
                        self.render_basemap(b, area, view);
                    }
                    let (base, route) = ((first == Some(i)).then_some(&self.basemap), self.route());
                    let mut p = Panel(b, self.area(), self.look());
                    map::update(&mut p, area, &self.track, &mut m, base, route.as_ref(), zoom, follow);
                    Drawn::Map(m)
                }
                (_, Drawn::Graph(g)) => self.update_graph(b, g, bx),
                (_, Drawn::Profile(p)) => self.update_profile(b, p, bx),
                (_, Drawn::Workout(w)) => self.update_workout_graph(b, w, bx),
                (_, Drawn::Gauge(g)) => self.update_gauge(b, g, bx),
                (_, Drawn::Head(was)) => match self.head_text() {
                    Some(h) => {
                        let shown =
                            ui::segment::update_head(&mut Panel(b, self.area(), self.look()), bx, &was, &h.head());
                        Drawn::Head(shown)
                    }
                    None => Drawn::Head(was), // the segment just ended: the page leaves at this pass
                },
                (Widget::Field(f), Drawn::Value(was, was_fg, was_bar)) => {
                    let (text, fg, bar) = (self.field_text(f), self.field_color(f), self.field_bar(f));
                    if bar != was_bar {
                        ui::draw_zone_bar(&mut Panel(b, self.area(), self.look()), bx, bar); // a zone changed: the bar only
                    }
                    if (was, was_fg) != (text, fg) {
                        let old = if was_fg == fg { was.as_str() } else { "" }; // a new colour: all of it
                        ui::draw_field_value_changes(
                            &mut Panel(b, self.area(), self.look()),
                            bx,
                            old,
                            text.as_str(),
                            fg,
                        );
                    }
                    Drawn::Value(text, fg, bar)
                }
                _ => {
                    let now = self.now_drawn(bx, w);
                    if now != old {
                        if let Some(d) = self.drawn.get_mut(i) {
                            *d = now;
                        }
                        self.draw_widget(&mut Panel(b, self.area(), self.look()), i, bx, w);
                    }
                    now
                }
            };
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
            if selected == Some(i) && now != old {
                ui::draw_cell_frame(&mut Panel(b, self.area(), self.look()), bx, ui::ACCENT);
            }
        }
    }

    /// Row `y` of the screen as the app drew it: the screenshot source.
    pub fn render_row(&self, y: u32) -> Row {
        let mut row = Row::new(y);
        row.look = self.look();
        row.px = [row.look.ink(ui::BLACK); ui::W as usize]; // the gaps nothing draws are the background
        self.draw_top(&mut row);
        self.draw_screen(&mut row);
        row
    }
}
