//! The climb pacer's glue ([`crate::pacing`]): the climb of the followed course the rider is on, or within
//! [`AHEAD_M`] of, cut into sections; the plan made again once a section (and, before the climb, every 30 s as W'
//! recovers) from the W' balance then; and the gauge field ([`Field::ClimbPace`]).
//!
//! The whole W' balance goes to the climb in hand, even if a bigger one follows on the course: the pacer does not look
//! past the top. The one CP / W' model ([`App::cp_model`]) drains the balance and sizes the plan.

use crate::*;

/// How far before a climb its plan shows, m.
const AHEAD_M: u32 = 5000;
/// Section length wanted, m (a plan is at most [`pacing::MAX`] sections: a longer climb has longer ones).
const SECTION_M: u32 = 200;
/// The band around the target power, percent each side (the gauge's colour says in, nearly, or out of it).
const BAND_PCT: u16 = 5;

/// The plan in force, and what it was made for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaceState {
    /// What the plan is for: the climb's start (m), the section the rider is in, and the 30 s before the climb.
    pub(crate) key: Option<(u32, u8, u16)>,
    /// The plan from where the rider was to the top (its first power is the target now).
    pub(crate) plan: pacing::Plan,
    /// The rider is on the climb (else it is ahead: no target yet, the time is the climb's).
    on: bool,
    /// The ride's timer when the plan was made, s: the time to the top counts down from it.
    made_s: u32,
}

impl PaceState {
    pub(crate) const fn new() -> Self {
        PaceState { key: None, plan: pacing::Plan::EMPTY, on: false, made_s: 0 }
    }
}

impl App {
    /// The critical power (W) and W' (J, at most 65 kJ) in force: the fit of the rider's bests when the scan has one,
    /// else the settings' FTP and W'. W' balance and the pacer both use it, so the budget is the balance of the model
    /// that drained it.
    pub(crate) fn cp_model(&self) -> (u16, u16) {
        let fit = if self.stock.ready() { power_curve::fit_cp(&self.stock.out.curve) } else { None };
        fit.map_or((self.settings.ftp, self.settings.wprime_j), |c| (c.cp, c.w_prime.min(u32::from(u16::MAX)) as u16))
    }

    /// Once a second: makes the plan again when the rider changes section (or 30 s pass before the climb); nothing
    /// otherwise, so the solver (a few ms) runs once per section.
    #[inline(never)]
    pub(crate) fn pace_second(&mut self) {
        let timer = self.ride.timer_s();
        let mut secs = [pacing::Sec::default(); pacing::MAX];
        let mut n = 0;
        let mut on = false;
        let key = 'k: {
            let Some((c, at)) = self.on_course() else { break 'k None };
            let d = at.along_m;
            let Some(cl) = c.climb_at(d).and_then(|(k, _)| c.climbs().get(k).copied()) else { break 'k None };
            on = d >= cl.start_m;
            if !on && cl.start_m - d > AHEAD_M {
                break 'k None;
            }
            let count = cl.len_m.div_ceil(SECTION_M).clamp(1, pacing::MAX as u32);
            let len = (cl.len_m / count).max(1);
            let idx = if on { ((d - cl.start_m) / len).min(count - 1) } else { 0 };
            let key = (cl.start_m, idx as u8, if on { 0 } else { (timer / 30) as u16 });
            if self.pace.key == Some(key) {
                return;
            }
            // the first section starts where the rider is, the others on the climb's grid
            let first = if on { d } else { cl.start_m };
            for (i, s) in secs.iter_mut().enumerate().take(count as usize).skip(idx as usize) {
                let from = if i as u32 == idx { first } else { cl.start_m + i as u32 * len };
                let to = if i as u32 + 1 == count { cl.end_m() } else { cl.start_m + (i as u32 + 1) * len };
                let (Some(a), Some(b)) = (c.alt_at(from), c.alt_at(to)) else { break 'k None };
                let grade = (b - a) * 100 / to.saturating_sub(from).max(1) as i32; // dm over m: % x 10
                *s = pacing::Sec {
                    len_m: to.saturating_sub(from).min(65_535) as u16,
                    grade_x10: grade.clamp(-1000, 1000) as i16,
                };
                n += 1;
            }
            Some(key)
        };
        self.pace.key = key;
        self.pace.on = on;
        self.pace.made_s = timer;
        self.pace.plan = match key {
            Some(_) => pacing::plan(secs.get(..n).unwrap_or(&[]), &self.pace_rider()),
            None => pacing::Plan::EMPTY,
        };
    }

    /// The rider for the plan: [`App::cp_model`], the weights, and the W' balance now as the budget.
    pub(crate) fn pace_rider(&self) -> pacing::Rider {
        let (cp, wprime) = self.cp_model();
        let w_full = f32::from(wprime);
        let budget = self.power.wbal_j().map_or(w_full, |j| j.clamp(0, i32::from(wprime)) as f32);
        let mass = f32::from(self.settings.weight_kg) + f32::from(self.settings.bike_hg) / 10.0;
        pacing::Rider { cp: f32::from(cp), w_full, budget, mass }
    }

    /// What the pace gauge in a box with `cols` bar columns shows now: on the climb, the power over the window set in
    /// the menu against a band around the plan's power for this section; the time to the top in the title, counting
    /// down between plans. Ahead of the climb only the time (the climb's): no band, so the effort on the way is not
    /// judged against the climb's first target.
    pub(crate) fn pace_now(&self, cols: u32) -> GaugeShown {
        let avg = self.settings.wkt_avg;
        let plan = &self.pace.plan;
        let target = plan.watts.first().copied().filter(|&w| w > 0 && plan.n > 0 && self.pace.on);
        let band = target.map(|w| (w - w * BAND_PCT / 100, w.saturating_add(w * BAND_PCT / 100)));
        let reading = if avg < 3 { self.power.rolling(avg) } else { self.live.get(Metric::Power) };
        let mut value = Line::new();
        match reading {
            Some(v) => value.push_u32(u32::from(v)),
            None => value.push_str("--"),
        }
        let color = match (reading, band) {
            (Some(v), Some(b)) => workout::gauge_color(v, b),
            (Some(w), None) => ride::power_color(ride::power_zone(w, self.settings.ftp)),
            (None, _) => ui::WHITE,
        };
        let marker = reading.zip(band).map(|(v, b)| workout::gauge_x(v, b, cols));
        let ridden = if self.pace.on { self.ride.timer_s().saturating_sub(self.pace.made_s) } else { 0 };
        let pace = Some(plan.total_s.saturating_sub(ridden).min(u32::from(u16::MAX)) as u16);
        GaugeShown {
            kind: storage::workout::Kind::Power,
            avg,
            band,
            step: target.is_some(),
            pace,
            value,
            color,
            marker,
        }
    }
}
