//! The climb pacer's glue ([`crate::pacing`]): the climb of the followed course the rider is on, or within
//! [`AHEAD_M`] of, cut into sections; the plan made again once a section (and, before the climb, every 30 s as W'
//! recovers) from the W' balance then; and the gauge field ([`Field::ClimbPace`]).

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
    key: Option<(u32, u8, u16)>,
    /// The plan from that section to the top (its first power is the target now).
    pub(crate) plan: pacing::Plan,
}

impl PaceState {
    pub(crate) const fn new() -> Self {
        PaceState { key: None, plan: pacing::Plan::EMPTY }
    }
}

impl App {
    /// Once a second: makes the plan again when the rider changes section (or 30 s pass before the climb); nothing
    /// otherwise, so the solver (a few ms) runs once per section.
    #[inline(never)]
    pub(crate) fn pace_second(&mut self) {
        let timer = self.ride.timer_s();
        let mut secs = [pacing::Sec::default(); pacing::MAX];
        let mut n = 0;
        let key = 'k: {
            let Some((c, at)) = self.on_course() else { break 'k None };
            let d = at.along_m;
            let Some(cl) = c.climb_at(d).and_then(|(k, _)| c.climbs().get(k).copied()) else { break 'k None };
            let on = d >= cl.start_m;
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
            for (i, s) in secs.iter_mut().enumerate().take(count as usize).skip(idx as usize) {
                let from = cl.start_m + i as u32 * len;
                let to = if i as u32 + 1 == count { cl.end_m() } else { from + len };
                let (Some(a), Some(b)) = (c.alt_at(from), c.alt_at(to)) else { break 'k None };
                let grade = (b - a) * 100 / to.saturating_sub(from).max(1) as i32; // dm over m: % x 10
                *s = pacing::Sec { len_m: (to - from).min(65_535) as u16, grade_x10: grade.clamp(-1000, 1000) as i16 };
                n += 1;
            }
            Some(key)
        };
        self.pace.key = key;
        self.pace.plan = match key {
            Some(_) => pacing::plan(secs.get(..n).unwrap_or(&[]), &self.pace_rider()),
            None => pacing::Plan::EMPTY,
        };
    }

    /// The rider for the plan: CP and W' fitted from the rider's rides when the scan has them, else the settings' FTP
    /// and W'; the budget is the W' balance now.
    fn pace_rider(&self) -> pacing::Rider {
        let s = &self.settings;
        let fit = if self.stock.ready() { power_curve::fit_cp(&self.stock.out.curve) } else { None };
        let (cp, w_full) =
            fit.map_or((f32::from(s.ftp), f32::from(s.wprime_j)), |c| (f32::from(c.cp), c.w_prime as f32));
        let left = self.power.wbal_j().map_or(1.0, |j| (j.max(0) as f32 / f32::from(s.wprime_j.max(1))).min(1.0));
        pacing::Rider { cp, w_full, budget: w_full * left, mass: f32::from(s.weight_kg) + pacing::BIKE_KG }
    }

    /// What the pace gauge in a box with `cols` bar columns shows now: the power over the window set in the menu
    /// against a band around the plan's power for this section; the predicted time to the top in the title.
    pub(crate) fn pace_now(&self, cols: u32) -> GaugeShown {
        let avg = self.settings.wkt_avg;
        let target = self.pace.plan.watts.first().copied().filter(|&w| w > 0 && self.pace.plan.n > 0);
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
        let pace = Some(self.pace.plan.total_s.min(u32::from(u16::MAX)) as u16);
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
