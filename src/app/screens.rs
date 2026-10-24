//! What is on screen: [`App::show`] draws a screen whole when it opens; [`App::update_page`] then redraws only the
//! page boxes whose content changed (AGENTS.md, redraw only what changes); the banner over it; the ride card; the
//! basemap under the first map field; and the screenshot rows ([`App::render_row`]).

use crate::*;

/// How long a banner stays up, unless an input dismisses it first.
pub const BANNER_MS: u32 = 4000;

/// What a page box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Widget {
    /// The big time, each digit in its own cell.
    Clock,
    /// The date line.
    Date,
    /// The latest notices, or Ferris when there are none.
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
    /// The clock's digits: H H M M.
    Digits([u8; 4]),
    /// A fingerprint of what the box shows, redrawn whole when it changes (the date, the notices).
    Print(u32),
    /// A value and its colour.
    Value(Line<8>, u16),
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
}

/// Box `i` of a page with this many fields (the ride pages' and the overview's grids).
pub(crate) type BoxAt = fn(u8, usize) -> Option<(u32, u32, u32, u32)>;

/// Boxes on a page at most.
pub(crate) const BOXES: usize = 8;

impl App {
    /// Memory for the basemap (at least [`tiles::MEM_WORDS`] words); without it the map has no basemap.
    pub fn set_map_memory(&mut self, mem: &'static mut [u16]) {
        self.basemap.set_memory(mem);
    }

    /// The last basemap view change's cost.
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
            self.basemap.render(&mut emmc::Volume(&mut self.emmc, b), area, v, o);
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
        ui::card::Card::of(&ui::card::Stats {
            start: self.ride_started,
            timer_s: r.timer_s(),
            dist_m: r.dist_m(),
            avg_speed_x10: r.avg_speed_x10(),
            ascent_m: Some(r.ascent_m()),
            np: p.np(),
            avg_power: r.avg_power(),
            avg_hr: r.avg_hr(),
            max_hr: r.max_hr(),
            tss: p.tss(self.settings.ftp),
            kj: Some(p.kj()),
        })
    }

    /// The card screen: the track in [`ui::card::MAP`], the whole ride's elevation, the numbers, as they were when
    /// the card was shown ([`App::summary`]).
    pub(crate) fn draw_summary(&self, c: &mut impl Canvas) {
        let Some((m, g)) = self.summary else { return };
        let (_, _, cols, h) = ui::card::ELEV;
        ui::card::draw(c, &self.ride_card(), (&self.track, m.view), &|i| self.graph_bar(g, cols, h, i));
    }

    /// Draws the whole screen below the bar from the state (the panel was cleared, or a screenshot row).
    pub(crate) fn draw_screen(&self, c: &mut impl Canvas) {
        match self.screen {
            Screen::Main | Screen::Ride(_) => {
                for (i, (bx, w)) in self.widgets().enumerate() {
                    self.draw_widget(c, i, bx, w);
                }
            }
            Screen::Menu(menu, sel) => {
                let mut title = Line::<16>::new();
                match menu {
                    Menu::Page(OVERVIEW) => title.push_str("OVERVIEW"),
                    Menu::Steps if !self.plan.name().is_empty() => title.push_str(self.plan.name()),
                    Menu::Page(n) => {
                        title.push_str(menu_title(menu));
                        title.push_u32(n as u32 + 1);
                    }
                    _ => title.push_str(menu_title(menu)),
                }
                ui::draw_title(c, title.as_str());
                self.draw_menu_rows(c, menu, sel);
            }
            Screen::Summary => self.draw_summary(c),
            Screen::Past => {
                let (p, h) = (&self.past, ui::card::ELEV.3);
                let view = map::View::fit_whole(&p.track, map::area(ui::card::MAP));
                ui::card::draw(c, &p.card, (&p.track, view), &|i| p.elev(i, h));
            }
            Screen::Media => {
                ui::media::draw(c, &self.media_now());
            }
            Screen::Nearby => {
                if self.ble.is_some() {
                    ui::draw_nearby(c, self.nearby.count(), self.scanning);
                }
                ui::draw_list(c, &self.nearby);
            }
        }
        if self.banner_ms.is_some() && self.banner_drawn {
            ui::draw_banner(c, self.banner.as_str());
        }
        if self.phone.ringing {
            let who = self.phone.call.as_ref().map_or("", |(_, who)| who.as_str());
            ui::draw_call(c, who, self.phone.talking.map(|ms| ms / 1000));
        }
    }

    /// The boxes of the page on screen and what each holds (none on a menu or a list). The clock and Ferris place
    /// themselves.
    pub(crate) fn widgets(&self) -> impl Iterator<Item = ((u32, u32, u32, u32), Widget)> + use<> {
        let mut all = [((0, 0, 0, 0), Widget::Clock); BOXES];
        let (page, at, fixed): (_, BoxAt, &[Widget]) = match self.screen {
            Screen::Main => (OVERVIEW, |_, i| ui::overview_box(i), &[Widget::Clock, Widget::Date, Widget::Notices]),
            Screen::Ride(n) => (n, |layout, i| ui::field_box(layout, ui::RIDE_H, i), &[]),
            _ => return all.into_iter().take(0),
        };
        let p = self.pages.get(page).copied().unwrap_or(DEFAULT_PAGES[0]);
        let mut n = 0;
        for (slot, &w) in all.iter_mut().zip(fixed) {
            slot.1 = w;
            n += 1;
        }
        for (i, &f) in p.fields.iter().enumerate() {
            let (Some(bx), Some(slot)) = (at(p.layout, i), all.get_mut(n)) else { break };
            *slot = (bx, Widget::Field(f));
            n += 1;
        }
        all.into_iter().take(n)
    }

    /// What box `bx` holding `w` would show now.
    pub(crate) fn now_drawn(&self, bx: (u32, u32, u32, u32), w: Widget) -> Drawn {
        match w {
            Widget::Clock => Drawn::Digits(self.main_digits()),
            Widget::Date => {
                Drawn::Print(u32::from(self.date.0) << 16 | u32::from(self.date.1) << 8 | u32::from(self.date.2))
            }
            Widget::Notices => {
                Drawn::Print(self.notices.iter().fold(0x811c_9dc5, |h, n| fnv(h, n.as_str().as_bytes())))
            }
            Widget::Field(Field::Map) => Drawn::Map(map::Shown::of(&self.track, map::area(bx), self.route().as_ref())),
            Widget::Field(Field::Profile) => Drawn::Profile(self.profile_now(ui::plot_area(bx).2)),
            Widget::Field(Field::WorkoutGraph) => Drawn::Workout(self.workout_now(ui::plot_area(bx).2)),
            Widget::Field(Field::WktPower) => Drawn::Gauge(self.gauge_now(ui::gauge_cols(bx))),
            Widget::Field(f) => match graph_plot(f) {
                Some(plot) => Drawn::Graph(self.graph_now(plot, ui::graph_area(bx).2)),
                None => Drawn::Value(self.field_text(f), self.field_color(f)),
            },
        }
    }

    /// Box `i` (`bx`, holding `w`) whole, as drawn ([`App::drawn`]): after the page was cleared, or a screenshot row.
    pub(crate) fn draw_widget(&self, c: &mut impl Canvas, i: usize, bx: (u32, u32, u32, u32), w: Widget) {
        let d = self.drawn.get(i).copied().unwrap_or_default();
        match (w, d) {
            (Widget::Clock, Drawn::Digits(digits)) => ui::draw_main_time(c, digits),
            (Widget::Date, _) => ui::draw_date(c, date_text(self.date).as_str()),
            (Widget::Notices, _) => {
                let mut n: [&str; ui::NOTICES] = [""; ui::NOTICES];
                for (slot, line) in n.iter_mut().zip(&self.notices) {
                    *slot = line.as_str();
                }
                ui::draw_notices(c, &n[..self.notices.iter().take_while(|l| !l.as_str().is_empty()).count()]);
            }
            (Widget::Field(f), d) => {
                ui::draw_field_frame(c, bx, f);
                match d {
                    Drawn::Map(m) => {
                        let base = (self.first_map() == Some(i)).then_some(&self.basemap);
                        map::draw(c, map::area(bx), &self.track, m.view, base, self.route().as_ref());
                    }
                    Drawn::Profile(p) => self.draw_profile(c, bx, p),
                    Drawn::Workout(w) => self.draw_workout_graph(c, bx, w),
                    Drawn::Gauge(g) => self.draw_gauge(c, bx, g),
                    Drawn::Graph(g) => {
                        self.draw_graph_axes(c, bx, g);
                        let a = ui::graph_area(bx);
                        ui::draw_graph(c, a, &|k| self.graph_bar(g, a.2, a.3, k));
                    }
                    Drawn::Value(text, fg) => ui::draw_field_value(c, bx, f, text.as_str(), fg),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The map in box `i` as drawn (tests).
    #[cfg(test)]
    pub(crate) fn map_view(&self, i: usize) -> Option<map::View> {
        match self.drawn.get(i) {
            Some(Drawn::Map(m)) => m.view,
            _ => None,
        }
    }

    /// The overview's digits: H H M M.
    pub(crate) fn main_digits(&self) -> [u8; 4] {
        let (h, m) = self.strip.time;
        [h / 10, h % 10, m / 10, m % 10]
    }

    /// How far down the page may be drawn: above the banner while one is up.
    pub(crate) fn bottom(&self) -> u32 {
        if self.phone.ringing {
            ui::CALL_Y
        } else if self.banner_ms.is_some() {
            ui::BANNER_Y
        } else {
            ui::H
        }
    }

    /// The banner is gone: what is under it drawn again, from what the screen shows.
    pub(crate) fn unbanner(&mut self, b: &mut impl Board) {
        self.banner_ms = None;
        let mut p = Panel(b, (ui::BANNER_Y, ui::H));
        p.fill(0, ui::BANNER_Y, ui::W, ui::H - ui::BANNER_Y, ui::BLACK);
        self.draw_screen(&mut p);
    }

    /// Adds a notice (newest first), stamped with the time: what the unit itself has to say (a fix, a sensor
    /// paired, the battery low); phone notifications will come here too.
    pub(crate) fn notice(&mut self, text: &str) {
        self.notices.rotate_right(1);
        let (h, m) = self.strip.time;
        let mut l = Line::new();
        for d in [h / 10, h % 10, 10, m / 10, m % 10] {
            l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
        }
        l.push_str(" ");
        l.push_str(text);
        self.notices[0] = l;
        // over the overview the notice is in its own box already
        if self.screen != Screen::Main {
            self.say(l.as_str());
        }
    }

    /// Shows `text` in the banner for [`BANNER_MS`] (what a key just did, a notice).
    pub(crate) fn say(&mut self, text: &str) {
        self.banner.clear();
        self.banner.push_str(text);
        (self.banner_ms, self.banner_drawn) = (Some(0), false);
    }

    /// Clears the page area and draws the current screen.
    pub(crate) fn show(&mut self, b: &mut impl Board) {
        if self.calendar_day.is_none() && self.widgets().any(|(_, w)| w == Widget::Field(Field::Workout)) {
            self.load_calendar(b); // the overview's planned workout: the card read at first use
        }
        for (i, (bx, w)) in self.widgets().enumerate() {
            let now = self.now_drawn(bx, w);
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
        if self.screen == Screen::Summary {
            let (_, _, cols, _) = ui::card::ELEV;
            let m = map::Shown::of(&self.track, map::area(ui::card::MAP), None);
            self.summary = Some((m, self.graph_shown(ride::Plot::Alt, true, cols)));
        }
        if let Some(i) = self.first_map()
            && let (Some((bx, _)), Some(Drawn::Map(m))) = (self.widgets().nth(i), self.drawn.get(i))
        {
            let view = m.view;
            self.render_basemap(b, map::area(bx), view);
        }
        let mut p = Panel(b, (0, self.bottom()));
        ui::clear_page(&mut p);
        self.draw_screen(&mut p);
        if let Screen::Menu(menu, sel) = self.screen {
            self.menu_shown = core::array::from_fn(|i| self.menu_row_print(menu, sel, i));
        }
        if self.screen == Screen::Media {
            self.phone.shown = ui::media::shown(&self.media_now());
        }
    }

    /// Redraws what changed in the page's boxes since they were drawn.
    pub(crate) fn update_page(&mut self, b: &mut impl Board) {
        let first = self.first_map();
        for (i, (bx, w)) in self.widgets().enumerate() {
            let Some(&old) = self.drawn.get(i) else { break };
            let now = match (w, old) {
                (_, Drawn::Map(mut m)) => {
                    let area = map::area(bx);
                    if first == Some(i) && map::refit(area, &self.track, &m, self.route().as_ref()) {
                        let view = map::Shown::of(&self.track, area, self.route().as_ref()).view;
                        self.render_basemap(b, area, view);
                    }
                    let (base, route) = ((first == Some(i)).then_some(&self.basemap), self.route());
                    map::update(&mut Panel(b, (0, self.bottom())), area, &self.track, &mut m, base, route.as_ref());
                    Drawn::Map(m)
                }
                (_, Drawn::Graph(g)) => self.update_graph(b, g, bx),
                (_, Drawn::Profile(p)) => self.update_profile(b, p, bx),
                (_, Drawn::Workout(w)) => self.update_workout_graph(b, w, bx),
                (_, Drawn::Gauge(g)) => self.update_gauge(b, g, bx),
                (Widget::Clock, Drawn::Digits(was)) => {
                    let now = self.main_digits();
                    for (k, (d, was)) in now.into_iter().zip(was).enumerate() {
                        if d != was {
                            ui::draw_main_digit(&mut Panel(b, (0, self.bottom())), k, d);
                        }
                    }
                    Drawn::Digits(now)
                }
                (Widget::Field(f), Drawn::Value(was, was_fg)) => {
                    let (text, fg) = (self.field_text(f), self.field_color(f));
                    if (was, was_fg) != (text, fg) {
                        let old = if was_fg == fg { was.as_str() } else { "" }; // a new colour: all of it
                        ui::draw_field_value_changes(&mut Panel(b, (0, self.bottom())), bx, f, old, text.as_str(), fg);
                    }
                    Drawn::Value(text, fg)
                }
                _ => {
                    let now = self.now_drawn(bx, w);
                    if now != old {
                        if let Some(d) = self.drawn.get_mut(i) {
                            *d = now;
                        }
                        self.draw_widget(&mut Panel(b, (0, self.bottom())), i, bx, w);
                    }
                    now
                }
            };
            if let Some(d) = self.drawn.get_mut(i) {
                *d = now;
            }
        }
    }

    /// Row `y` of the screen as the app drew it: the screenshot source.
    pub fn render_row(&self, y: u32) -> Row {
        let mut row = Row::new(y);
        ui::draw_strip(&mut row, &self.strip_shown);
        self.draw_screen(&mut row);
        row
    }
}
