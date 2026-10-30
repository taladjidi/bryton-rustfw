//! The graph fields (power, heart rate, W' balance, DFA alpha1, altitude over the last minutes or the whole ride)
//! the course profile field and the workout graph: what each shows ([`GraphShown`], [`ProfileShown`],
//! [`WorkoutShown`]), its bars and scale, and the update that redraws only the columns that changed.

use crate::*;

/// A box: left, top, width, height.
type Box4 = (u32, u32, u32, u32);

/// What a graph field shows on screen: its plot, over the last minutes or the whole ride, the bars up to `n`
/// ([`ride::History::len`] or [`ride::History::whole_len`]), the whole ride's halvings and the scale (low, high, in
/// the plot's units), and the ride's figures at its top right ([`App::graph_stats`]). A change of anything but `n`
/// or `stats` redraws the graph whole; of `stats`, only their box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphShown {
    plot: ride::Plot,
    whole: bool,
    /// Over the distance, not the time (the summary's altitude).
    dist: bool,
    n: u32,
    halved: u32,
    scale: (u32, u32),
    stats: [Option<u16>; 2],
}

/// What the course profile field shows: the course (its id, 0: none), from `start` m over `span` m, the rider's
/// column. A change of anything but the column redraws it whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProfileShown {
    id: u32,
    start: u32,
    span: u32,
    rider: Option<u32>,
    /// The target's column on a live segment (a grey mark two columns wide).
    ghost: Option<u32>,
    /// The climb's profile (the CLIMB page): `start` and `span` are the climb's (or the live segment's: `id` is
    /// 3 and its id), `id` is 1 (road ahead) or 2 (course).
    climb: bool,
}

/// What the workout graph shows: the workout (its file, None: none), the zones' FTP and max heart rate, the rider's
/// column ([`workout::column`]). A change of anything but the column redraws it whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkoutShown {
    file: Option<fat::File>,
    zones: ride::Zones,
    pub(crate) rider: Option<u32>,
}

/// The columns to redraw when a rider's column moves from `old` to `new` (both, and those between: done or not).
fn moved(old: Option<u32>, new: Option<u32>) -> core::ops::Range<u32> {
    match (old, new) {
        (Some(x), Some(y)) => x.min(y)..x.max(y).saturating_add(1),
        (Some(x), None) | (None, Some(x)) => x..x.saturating_add(1),
        (None, None) => 0..0,
    }
}

/// The plot a graph field shows, None for a field that is no graph.
pub(crate) fn graph_plot(f: Field) -> Option<ride::Plot> {
    use ride::Plot;
    Some(match f {
        Field::Graph => Plot::Power,
        Field::HrGraph => Plot::Hr,
        Field::WbalGraph => Plot::Wbal,
        Field::DfaGraph => Plot::Dfa,
        Field::AltGraph => Plot::Alt,
        _ => return None,
    })
}

/// The graph field showing `plot`.
pub(crate) fn graph_field(plot: ride::Plot) -> Field {
    use ride::Plot;
    match plot {
        Plot::Power => Field::Graph,
        Plot::Hr => Field::HrGraph,
        Plot::Wbal => Field::WbalGraph,
        Plot::Dfa => Field::DfaGraph,
        Plot::Alt => Field::AltGraph,
    }
}

/// A graph's title, in its label line.
fn graph_title(plot: ride::Plot) -> &'static str {
    use ride::Plot;
    match plot {
        Plot::Power => "POWER W",
        Plot::Hr => "HR",
        Plot::Wbal => "W'BAL KJ",
        Plot::Dfa => "DFA A1",
        Plot::Alt => "ELEV M",
    }
}

impl ProfileShown {
    /// The rider's column (tests).
    #[cfg(test)]
    pub(crate) fn rider(&self) -> Option<u32> {
        self.rider
    }

    /// The target's column (tests).
    #[cfg(test)]
    pub(crate) fn ghost(&self) -> Option<u32> {
        self.ghost
    }

    /// The part shown: from m (tests).
    #[cfg(test)]
    pub(crate) fn start(&self) -> u32 {
        self.start
    }

    /// The part shown: over m (tests).
    #[cfg(test)]
    pub(crate) fn span(&self) -> u32 {
        self.span
    }
}

impl App {
    /// What a graph of `plot` with `cols` columns shows now.
    pub(crate) fn graph_now(&self, plot: ride::Plot, cols: u32) -> GraphShown {
        self.graph_shown(plot, self.graph_whole, cols)
    }

    /// What a graph of `plot` over the last minutes or the `whole` ride, `cols` wide, shows now.
    pub(crate) fn graph_shown(&self, plot: ride::Plot, whole: bool, cols: u32) -> GraphShown {
        let h = &self.history;
        let (n, halved) = if whole { (h.whole_len(), h.whole_halved()) } else { (h.len(), 0) };
        let mut g = GraphShown { plot, whole, dist: false, n, halved, scale: (0, 1), stats: self.graph_stats(plot) };
        g.scale = self.graph_scale(g, cols);
        g
    }

    /// What the summary card's altitude shows: the whole ride, with distance on the x axis.
    pub(crate) fn graph_summary(&self, cols: u32) -> GraphShown {
        let mut g = self.graph_shown(ride::Plot::Alt, true, cols);
        (g.dist, g.n, g.halved) = (true, self.history.dist_len(), 0);
        g.scale = self.graph_scale(g, cols);
        g
    }

    /// The ride's figures a graph shows at its top right, as stock does: average power and NP over a power graph,
    /// average and max heart rate over a heart rate graph.
    pub(crate) fn graph_stats(&self, plot: ride::Plot) -> [Option<u16>; 2] {
        match plot {
            ride::Plot::Power => [self.ride.avg_power(), self.power.np()],
            ride::Plot::Hr => [self.ride.avg_hr(), self.ride.max_hr()],
            _ => [None; 2],
        }
    }

    /// Draws `g`'s figures ([`App::graph_stats`]) in box `bx`, only those that differ from `old`'s (all: None).
    fn draw_graph_stats(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), g: GraphShown, old: Option<GraphShown>) {
        let names = match g.plot {
            ride::Plot::Power => ["AVG", "NP"],
            ride::Plot::Hr => ["AVG", "MAX"],
            _ => return,
        };
        let text = |stats: [Option<u16>; 2]| {
            core::array::from_fn::<_, 2, _>(|k| {
                let mut l = Line::<10>::new();
                l.push_str(names[k]);
                l.push_str(" ");
                match stats[k] {
                    Some(v) => l.push_u32(u32::from(v)),
                    None => l.push_str("--"),
                }
                l
            })
        };
        let (lines, was) = (text(g.stats), old.map(|o| text(o.stats)));
        // each figure that changed, with what it was (its characters redrawn alone when the length stays)
        let line = |k: usize| {
            let was = was.as_ref().map(|w| w[k].as_str());
            (old.map(|o| o.stats[k]) != Some(g.stats[k])).then(|| (was.unwrap_or(""), lines[k].as_str()))
        };
        ui::draw_graph_stats(c, bx, graph_title(g.plot), [line(0), line(1)]);
    }

    /// Column `i` of `cols` of a graph showing `g`: its value (0: none). Over the whole ride, a column is the mean of
    /// the bars it covers when there are more bars than columns; over the last minutes, the newest bar is the
    /// rightmost column.
    pub(crate) fn graph_value(&self, g: GraphShown, cols: u32, i: u32) -> u16 {
        let h = &self.history;
        if !g.whole {
            return (g.n + i).checked_sub(cols).map_or(0, |k| h.bar(k, g.plot));
        }
        // the distance profile fits the width (as the summary card's does): column i takes columns lo..hi of the
        // profile, one at least when it has fewer; the time graphs fill from the left and grow
        let (lo, hi) = if g.dist {
            let lo = i * g.n / cols.max(1);
            (lo, ((i + 1) * g.n / cols.max(1)).max(lo + 1).min(g.n))
        } else {
            let per = g.n.div_ceil(cols.max(1)).max(1);
            (i * per, ((i + 1) * per).min(g.n))
        };
        let (sum, count) = (lo..hi)
            .map(|k| u32::from(if g.dist { h.dist_bar(k) } else { h.whole_bar(k, g.plot) }))
            .filter(|&v| v != 0)
            .fold((0, 0), |(s, n), v| (s + v, n + 1));
        if count == 0 { 0 } else { (sum / count) as u16 }
    }

    /// Column `i` of a graph of `cols` columns `h` high showing `g`: the bar's height and colour (a bar of a reading
    /// is at least 1 high).
    pub(crate) fn graph_bar(&self, g: GraphShown, cols: u32, h: u32, i: u32) -> (u32, u16) {
        use ride::Plot;
        #[cfg(test)]
        self.bars.set(self.bars.get() + 1);
        let v = self.graph_value(g, cols, i);
        if v == 0 {
            return (0, ui::BLACK);
        }
        let color = match g.plot {
            Plot::Power => self.settings.zones().power_color(v),
            Plot::Hr => self.settings.zones().hr_color(v),
            Plot::Wbal => power::wbal_color(i32::from(v) * 10, self.cp_model().1),
            Plot::Dfa => ride::dfa_color(v),
            Plot::Alt => ui::GREY,
        };
        let (low, high) = g.scale;
        let height = u32::from(v).saturating_sub(low) * h / high.saturating_sub(low).max(1);
        (height.clamp(1, h), color)
    }

    /// A graph's scale, bottom and top, in its plot's units: power from 0 to twice the FTP, heart rate from half the
    /// maximum to it, W' balance from 0 to W', DFA alpha1 from 0 to 1.5; altitude around what the graph shows (in
    /// 20 m steps, 60 m at least, so it is not redrawn for every metre).
    pub(crate) fn graph_scale(&self, g: GraphShown, cols: u32) -> (u32, u32) {
        use ride::Plot;
        let (max, ftp) = (u32::from(self.settings.max_hr), u32::from(self.settings.ftp));
        match g.plot {
            Plot::Power => (0, 2 * ftp),
            Plot::Hr => (max / 2, max),
            Plot::Wbal => (0, u32::from(self.cp_model().1) / 10),
            Plot::Dfa => (0, 150),
            Plot::Alt => ride::alt_scale((0..cols).map(|i| u32::from(self.graph_value(g, cols, i)))),
        }
    }

    /// A graph's label, scale and time span (its axes) in box `bx`.
    pub(crate) fn draw_graph_axes(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), g: GraphShown) {
        use ride::Plot;
        let (low, high) = g.scale;
        let label = |v: u32| {
            let mut l = Line::<8>::new();
            match g.plot {
                Plot::Wbal => l.push_u32(v / 100), // kJ
                Plot::Dfa => push_fixed_u(&mut l, v, 2),
                Plot::Alt => l.push_u32(v.saturating_sub(ride::ALT_OFFSET as u32)),
                _ => l.push_u32(v),
            }
            l
        };
        let (lo, mid, hi) = (label(low), label(low.midpoint(high)), label(high));
        let title = graph_title(g.plot);
        let mut from = Line::<12>::new();
        if g.whole {
            from.push_str("START");
        } else {
            from.push_str("-");
            from.push_u32(ui::graph_area(bx).2 * ride::BAR_S / 60);
            from.push_str(" MIN");
        }
        ui::draw_graph_axes(c, bx, title, [lo.as_str(), mid.as_str(), hi.as_str()], from.as_str());
        self.draw_graph_stats(c, bx, g, None);
    }

    /// The graph field at (`x`, `y`) on the page shown, if there is one: its box index and field.
    pub(crate) fn graph_at(&self, x: u16, y: u16) -> Option<(usize, Field)> {
        self.widgets().enumerate().find_map(|(i, (r, w))| match w {
            Widget::Field(f) if graph_plot(f).is_some() && ui::Rect::from(r).contains(x, y) => Some((i, f)),
            _ => None,
        })
    }

    /// A graph in box `bx`, drawn as `old`, after a new bar: each column changes by the difference between its old
    /// bar and its new one (the chart scrolls left by `STEP` bars), all of it if the graph changed metric.
    pub(crate) fn update_graph(&self, b: &mut impl Board, old: GraphShown, bx: (u32, u32, u32, u32)) -> Drawn {
        // Bars scroll in this many at a time (10 s): nearly every column changes on a scroll, so this sends a fifth
        // of the panel windows of a scroll per bar, and fewer pixels (sim graph_field: 1812 px/s in 33 windows/s,
        // against 1987 in 113). The whole ride grows to the right instead: a new bar changes one column.
        const STEP: u32 = 5;
        let a = ui::graph_area(bx);
        let now = self.graph_now(old.plot, a.2);
        let step = if now.whole { 1 } else { STEP };
        let mut p = Panel(b, self.area(), self.look());
        let same = GraphShown { n: old.n, stats: old.stats, ..now } == old;
        if same && now.stats != old.stats {
            self.draw_graph_stats(&mut p, bx, now, Some(old));
        }
        if same && now.n.wrapping_sub(old.n) < step {
            return Drawn::Graph(GraphShown { n: old.n, ..now });
        }
        if !same {
            // another scale, a halved whole ride, or another plot or span: all of it
            self.draw_graph_axes(&mut p, bx, now);
            ui::draw_graph(&mut p, a, &|k| self.graph_bar(now, a.2, a.3, k));
        } else {
            for k in 0..a.2 {
                let (was, is) = (self.graph_bar(old, a.2, a.3, k), self.graph_bar(now, a.2, a.3, k));
                ui::draw_graph_column(&mut p, a, k, was, is);
            }
        }
        Drawn::Graph(now)
    }

    /// What a profile `cols` wide shows now: from the eighth of the course the rider is in to the end (so it is
    /// redrawn whole eight times over a course, and otherwise moves by a column now and then).
    pub(crate) fn profile_now(&self, cols: u32, climb: bool) -> ProfileShown {
        let none = ProfileShown { id: 0, start: 0, span: 0, rider: None, ghost: None, climb };
        if climb {
            let Some(n) = self.climb_now() else { return none };
            let (start, span) = (n.climb.start_m, n.climb.len_m.max(1));
            let rider = n
                .along
                .checked_sub(start)
                .filter(|&d| d < span)
                .map(|d| ((u64::from(d) * u64::from(cols) / u64::from(span)) as u32).min(cols.max(1) - 1));
            let col = |d: u32| (u64::from(d) * u64::from(cols) / u64::from(span)) as u32;
            let ghost = n.ghost.filter(|&g| g < span).map(|g| col(g).min(cols.max(1) - 1));
            let id = n.seg.map_or(1 + u32::from(n.course), |s| 3u32.saturating_add(s));
            return ProfileShown { id, start, span, rider, ghost, climb };
        }
        let Some(c) = self.courses.as_deref().filter(|c| c.on.is_some() && c.course.has_alt()) else { return none };
        let len = c.course.len_m();
        let at = c.at.off_m.map(|_| c.at.along_m.min(len));
        let step = (len / 8).max(1);
        let start = at.map_or(0, |d| (d / step * step).min(len.saturating_sub(1)));
        let span = (len - start).max(1);
        let rider =
            at.map(|d| ((u64::from(d - start) * u64::from(cols) / u64::from(span)) as u32).min(cols.max(1) - 1));
        ProfileShown { id: c.course.id(), start, span, rider, ghost: None, climb }
    }

    /// Profile column `k` of `p` in a plot `a`: its bar's height and colour (the rider's column white, the ridden
    /// part dark, the climbs by grade, the rest grey).
    pub(crate) fn profile_bar(&self, p: ProfileShown, a: (u32, u32, u32, u32), k: u32) -> (u32, u16) {
        if p.climb {
            return self.climb_bar(p, a, k);
        }
        let Some(c) = self.courses.as_deref().filter(|_| p.id != 0) else { return (0, ui::BLACK) };
        if p.rider == Some(k) {
            return (a.3, ui::WHITE);
        }
        let Some((alt, grade, climb)) = c.course.column(p.start, p.span, a.2, k) else { return (0, ui::BLACK) };
        // the whole course's range, 30 m at least, so a flat course stays flat
        let (lo, hi) = c.course.range_dm();
        let hi = hi.max(lo + 300);
        let h = ((i64::from(alt - lo) * i64::from(a.3)) / i64::from(hi - lo)).clamp(1, i64::from(a.3)) as u32;
        let color = match (p.rider.is_some_and(|r| k < r), climb) {
            (true, _) => ui::DARK,
            (false, true) => ui::grade_color(grade),
            (false, false) => ui::GREY,
        };
        (h, color)
    }

    /// Column `k` of the climb's profile: as high as its altitude in the climb's range (30 m at least), coloured by
    /// the grade over it (100 m at least, inside the climb); the progress as the workout graph's: done dark, the rider's
    /// column white.
    fn climb_bar(&self, p: ProfileShown, a: (u32, u32, u32, u32), k: u32) -> (u32, u16) {
        let Some(n) = self.climb_now().filter(|_| p.id != 0) else { return (0, ui::BLACK) };
        if p.rider == Some(k) {
            return (a.3, ui::WHITE);
        }
        if p.ghost.is_some_and(|g| k == g || k == g + 1) {
            return (a.3, ui::GREY);
        }
        if n.seg.is_some() && self.climb_alt(&n, p.start).is_none() {
            // a segment without a profile: the bar only, the part done dark
            return ui::progress_column(p.rider, k, a.3, (a.3 / 5, ui::GREY));
        }
        let end = p.start.saturating_add(p.span);
        let at = |i: u32| p.start + (u64::from(p.span) * u64::from(i) / u64::from(a.2.max(1))) as u32;
        let (d0, d1) = (at(k), at(k + 1));
        let mid = d0 + (d1 - d0) / 2;
        let half = ((d1 - d0) / 2).max(50);
        let (g0, g1) = (mid.saturating_sub(half).max(p.start), mid.saturating_add(half).min(end));
        let alt = |d| self.climb_alt(&n, d);
        let (Some(lo), Some(hi), Some(h0), Some(h1), Some(m)) = (alt(p.start), alt(end), alt(g0), alt(g1), alt(mid))
        else {
            return (0, ui::BLACK);
        };
        let grade = (h1 - h0) * 100 / g1.saturating_sub(g0).clamp(1, i32::MAX as u32) as i32;
        let hi = hi.max(lo + 300);
        let h = ((i64::from(m - lo) * i64::from(a.3)) / i64::from(hi - lo)).clamp(1, i64::from(a.3)) as u32;
        ui::progress_column(p.rider, k, a.3, (h, ui::grade_color(grade)))
    }

    /// The climb's figures written over its plot: the gain (top left) and the length and average grade (top right,
    /// as the banner says them), each in a black box the size of its text, so it reads over any colour. None
    /// without a climb.
    pub(crate) fn climb_text_boxes(&self, a: Box4) -> Option<[(Box4, Line<16>); 2]> {
        let k = self.climb_now()?.climb;
        let (mut l, mut r) = (Line::<16>::new(), Line::<16>::new());
        l.push_str("+");
        l.push_u32(k.gain_m);
        l.push_str(" M");
        ui::card::push_km(&mut r, k.len_m);
        r.push_str(" KM ");
        r.push_u32(k.grade_x10().div_ceil(10).max(1));
        r.push_str("%");
        let h = ui::LABEL.height();
        let wl = (ui::LABEL.width(l.as_str()) + 6).min(a.2);
        let mut wr = (ui::LABEL.width(r.as_str()) + 6).min(a.2);
        if wl + wr > a.2 {
            (wr, r) = (0, Line::new()); // a narrow field: the gain only
        }
        Some([((a.0, a.1, wl, h), l), ((a.0 + a.2 - wr, a.1, wr, h), r)])
    }

    /// The climb's figures drawn again where they cover columns `cols` of the plot `a` (after those were redrawn).
    fn draw_climb_text(&self, c: &mut impl Canvas, a: (u32, u32, u32, u32), cols: core::ops::Range<u32>) {
        for ((x, y, w, _), s) in self.climb_text_boxes(a).into_iter().flatten() {
            let first = x - a.0;
            if w > 0 && first < cols.end && cols.start < first + w {
                ui::text::draw_in(c, &ui::LABEL, x, y, w, ui::text::Align::Centre, s.as_str());
            }
        }
    }

    /// The profile field in box `bx`, whole.
    pub(crate) fn draw_profile(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), p: ProfileShown) {
        let a = if p.climb { ui::climb_area(bx) } else { ui::plot_area(bx) };
        if !p.climb {
            ui::draw_plot_title(c, bx, if p.id == 0 { "NO COURSE" } else { "PROFILE" });
        }
        ui::draw_graph(c, a, &|k| self.profile_bar(p, a, k));
        if p.climb {
            self.draw_climb_text(c, a, 0..a.2);
        }
    }

    /// The profile field drawn as `old`: whole when it shows another part of the course, else only the columns
    /// between the rider's old place and the new one.
    pub(crate) fn update_profile(&self, b: &mut impl Board, old: ProfileShown, bx: (u32, u32, u32, u32)) -> Drawn {
        let a = if old.climb { ui::climb_area(bx) } else { ui::plot_area(bx) };
        let now = self.profile_now(a.2, old.climb);
        let mut p = Panel(b, self.area(), self.look());
        if (ProfileShown { rider: old.rider, ghost: old.ghost, ..now }) != old {
            p.fill(bx.0, bx.1, bx.2.saturating_sub(1), a.1 - bx.1, ui::BLACK);
            self.draw_profile(&mut p, bx, now);
        } else if (now.rider, now.ghost) != (old.rider, old.ghost) {
            // the columns of the rider's move and of the target's (its mark is two wide), as one run
            let (r, g) = (moved(old.rider, now.rider), moved(old.ghost, now.ghost));
            let g = g.start..g.end.saturating_add(u32::from(!g.is_empty()));
            for k in r.clone().chain(g.clone()).filter(|&k| k < a.2) {
                ui::draw_graph_column(&mut p, a, k, self.profile_bar(old, a, k), self.profile_bar(now, a, k));
            }
            if now.climb {
                self.draw_climb_text(&mut p, a, r);
                self.draw_climb_text(&mut p, a, g);
            }
        }
        Drawn::Profile(now)
    }

    /// What a workout graph `cols` wide shows now: the workout opened, the rider's column while it is played.
    pub(crate) fn workout_now(&self, cols: u32) -> WorkoutShown {
        let steps = self.plan.steps();
        let total = workout::graph_total_s(steps);
        let rider = self.plan.player.filter(|_| total > 0).map(|p| match p.graph_at(steps, self.ride.dist_m()) {
            at if at >= total => cols, // done: all of it dark
            at => (u64::from(at) * u64::from(cols) / u64::from(total)) as u32,
        });
        let zones = self.settings.zones();
        WorkoutShown { file: self.plan.file.filter(|_| total > 0), zones, rider }
    }

    /// The workout graph in box `bx`, whole.
    pub(crate) fn draw_workout_graph(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), w: WorkoutShown) {
        ui::draw_plot_title(c, bx, if w.file.is_none() { "NO WORKOUT" } else { "WORKOUT" });
        let a = ui::plot_area(bx);
        ui::draw_graph(c, a, &|k| self.workout_bar(w, a, k));
    }

    /// Workout graph column `k` of `w` in a plot `a`.
    fn workout_bar(&self, w: WorkoutShown, a: (u32, u32, u32, u32), k: u32) -> (u32, u16) {
        if w.file.is_none() {
            return (0, ui::BLACK);
        }
        workout::column(self.plan.steps(), w.zones, (a.2, a.3), w.rider, k)
    }

    /// The workout graph drawn as `old`: whole for another workout or other zones, else only the columns between the
    /// rider's old place and the new one (one or two a step of the plan's width: most seconds, none).
    pub(crate) fn update_workout_graph(
        &self,
        b: &mut impl Board,
        old: WorkoutShown,
        bx: (u32, u32, u32, u32),
    ) -> Drawn {
        let a = ui::plot_area(bx);
        let now = self.workout_now(a.2);
        let mut p = Panel(b, self.area(), self.look());
        if (WorkoutShown { rider: old.rider, ..now }) != old {
            p.fill(bx.0, bx.1, bx.2.saturating_sub(1), a.1 - bx.1, ui::BLACK);
            self.draw_workout_graph(&mut p, bx, now);
        } else if now.rider != old.rider {
            for k in moved(old.rider, now.rider).filter(|&k| k < a.2) {
                ui::draw_graph_column(&mut p, a, k, self.workout_bar(old, a, k), self.workout_bar(now, a, k));
            }
        }
        Drawn::Workout(now)
    }
}
