//! The graph fields (power, heart rate, W' balance, DFA alpha1, altitude over the last minutes or the whole ride)
//! the course profile field and the workout graph: what each shows ([`GraphShown`], [`ProfileShown`],
//! [`WorkoutShown`]), its bars and scale, and the update that redraws only the columns that changed.

use crate::*;

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
}

/// What the workout graph shows: the workout (its file, None: none), the zones' FTP and max heart rate, the rider's
/// column ([`workout::column`]). A change of anything but the column redraws it whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkoutShown {
    file: Option<fat::File>,
    zones: (u16, u16),
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
        let per = g.n.div_ceil(cols.max(1)).max(1);
        let (sum, count) = (i * per..((i + 1) * per).min(g.n))
            .map(|k| u32::from(if g.dist { h.dist_bar(k) } else { h.whole_bar(k, g.plot) }))
            .filter(|&v| v != 0)
            .fold((0, 0), |(s, n), v| (s + v, n + 1));
        if count == 0 { 0 } else { (sum / count) as u16 }
    }

    /// Column `i` of a graph of `cols` columns `h` high showing `g`: the bar's height and colour (a bar of a reading
    /// is at least 1 high).
    pub(crate) fn graph_bar(&self, g: GraphShown, cols: u32, h: u32, i: u32) -> (u32, u16) {
        use ride::Plot;
        let v = self.graph_value(g, cols, i);
        if v == 0 {
            return (0, ui::BLACK);
        }
        let color = match g.plot {
            Plot::Power => ride::power_color(ride::power_zone(v, self.settings.ftp)),
            Plot::Hr => ride::hr_color(ride::hr_zone(v, self.settings.max_hr)),
            Plot::Wbal => power::wbal_color(i32::from(v) * 10, self.settings.wprime_j),
            // above the aerobic threshold's 0.75, between it and 0.5, below (Rogers et al.)
            Plot::Dfa if v >= 75 => ui::GREEN,
            Plot::Dfa if v >= 50 => ui::ACCENT,
            Plot::Dfa => ui::RED,
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
            Plot::Wbal => (0, u32::from(self.settings.wprime_j) / 10),
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
        let (x, y) = (u32::from(x), u32::from(y));
        self.widgets().enumerate().find_map(|(i, ((bx, by, bw, bh), w))| match w {
            Widget::Field(f) if graph_plot(f).is_some() && (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y) => {
                Some((i, f))
            }
            _ => None,
        })
    }

    /// A graph in box `bx`, drawn as `old`, after a new bar: each column changes by the difference between its old
    /// bar and its new one (the chart scrolls left by `STEP` bars), all of it if the graph changed metric.
    pub(crate) fn update_graph(&mut self, b: &mut impl Board, old: GraphShown, bx: (u32, u32, u32, u32)) -> Drawn {
        // Bars scroll in this many at a time (10 s): nearly every column changes on a scroll, so this sends a fifth
        // of the panel windows of a scroll per bar, and fewer pixels (sim graph_field: 1812 px/s in 33 windows/s,
        // against 1987 in 113). The whole ride grows to the right instead: a new bar changes one column.
        const STEP: u32 = 5;
        let a = ui::graph_area(bx);
        let now = self.graph_now(old.plot, a.2);
        let step = if now.whole { 1 } else { STEP };
        let mut p = Panel(b, self.area());
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
    pub(crate) fn profile_now(&self, cols: u32) -> ProfileShown {
        let none = ProfileShown { id: 0, start: 0, span: 0, rider: None };
        let Some(c) = self.courses.as_deref().filter(|c| c.on.is_some() && c.course.has_alt()) else { return none };
        let len = c.course.len_m();
        let at = c.at.off_m.map(|_| c.at.along_m.min(len));
        let step = (len / 8).max(1);
        let start = at.map_or(0, |d| (d / step * step).min(len.saturating_sub(1)));
        let span = (len - start).max(1);
        let rider =
            at.map(|d| ((u64::from(d - start) * u64::from(cols) / u64::from(span)) as u32).min(cols.max(1) - 1));
        ProfileShown { id: c.course.id(), start, span, rider }
    }

    /// Profile column `k` of `p` in a plot `a`: its bar's height and colour (the rider's column white, the ridden
    /// part dark, the climbs by grade, the rest grey).
    pub(crate) fn profile_bar(&self, p: ProfileShown, a: (u32, u32, u32, u32), k: u32) -> (u32, u16) {
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

    /// The profile field in box `bx`, whole.
    pub(crate) fn draw_profile(&self, c: &mut impl Canvas, bx: (u32, u32, u32, u32), p: ProfileShown) {
        ui::draw_plot_title(c, bx, if p.id == 0 { "NO COURSE" } else { "PROFILE" });
        let a = ui::plot_area(bx);
        ui::draw_graph(c, a, &|k| self.profile_bar(p, a, k));
    }

    /// The profile field drawn as `old`: whole when it shows another part of the course, else only the columns
    /// between the rider's old place and the new one.
    pub(crate) fn update_profile(&mut self, b: &mut impl Board, old: ProfileShown, bx: (u32, u32, u32, u32)) -> Drawn {
        let a = ui::plot_area(bx);
        let now = self.profile_now(a.2);
        let mut p = Panel(b, self.area());
        if (ProfileShown { rider: old.rider, ..now }) != old {
            p.fill(bx.0, bx.1, bx.2.saturating_sub(1), ui::plot_area(bx).1 - bx.1, ui::BLACK);
            self.draw_profile(&mut p, bx, now);
        } else if now.rider != old.rider {
            for k in moved(old.rider, now.rider).filter(|&k| k < a.2) {
                ui::draw_graph_column(&mut p, a, k, self.profile_bar(old, a, k), self.profile_bar(now, a, k));
            }
        }
        Drawn::Profile(now)
    }

    /// What a workout graph `cols` wide shows now: the workout opened, the rider's column while it is played.
    pub(crate) fn workout_now(&self, cols: u32) -> WorkoutShown {
        let steps = self.plan.steps();
        let total = workout::graph_total_s(steps);
        let rider = self.player.filter(|_| total > 0).map(|p| match p.graph_at(steps, self.ride.dist_m()) {
            at if at >= total => cols, // done: all of it dark
            at => (u64::from(at) * u64::from(cols) / u64::from(total)) as u32,
        });
        let zones = (self.settings.ftp, self.settings.max_hr);
        WorkoutShown { file: self.plan_file.filter(|_| total > 0), zones, rider }
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
        &mut self,
        b: &mut impl Board,
        old: WorkoutShown,
        bx: (u32, u32, u32, u32),
    ) -> Drawn {
        let a = ui::plot_area(bx);
        let now = self.workout_now(a.2);
        let mut p = Panel(b, self.area());
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
