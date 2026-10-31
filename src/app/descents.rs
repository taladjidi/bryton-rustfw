//! Descents ahead: the next one of the course followed, else of the road ahead (`climbs_ahead`), for the DESCENT
//! field and the STEEP DESCENT alert (a banner and a beep [`App::STEEP_AHEAD_M`] before a descent whose steepest
//! 50 m passes the setting, once per descent).

use super::courses::{push_dist, push_grade};
use crate::course::Descent;
use crate::*;

impl App {
    /// The alert comes this far before the descent, m.
    const STEEP_AHEAD_M: u32 = 300;
    /// Two sightings of a descent start within this many metres of the ride's odometer are the same descent (the
    /// road ahead is traced again as the rider goes).
    const SAME_DESCENT_M: i32 = 400;

    /// The descents of the course followed, else of the road ahead, and the rider's place on them, m.
    fn descents_here(&self) -> Option<(impl Iterator<Item = Descent> + '_, u32)> {
        // two iterator types: the course's, else the road ahead's (a place on neither gives nothing)
        let (c, a) = (self.on_course(), self.ahead.as_ref().map(|(a, _)| &**a).filter(|a| a.on_road()));
        let d = c.map(|(_, at)| at.along_m).or(a.map(|a| a.along_m()))?;
        let (ci, ai) = (c.map(|(c, _)| c.descents()), c.is_none().then_some(a).flatten().map(|a| a.descents()));
        Some((ci.into_iter().flatten().chain(ai.into_iter().flatten()), d))
    }

    /// The DESCENT field: the distance to the next descent (what is left of the one running) and its steepest
    /// grade; "--" without one.
    pub(crate) fn descent_text(&self) -> Line<8> {
        let mut l = Line::new();
        let next = self.descents_here().and_then(|(mut ds, d)| Some((ds.find(|k| k.end_m() > d)?, d)));
        match next {
            Some((k, d)) => {
                let m = if d >= k.start_m { k.end_m() - d } else { k.start_m - d };
                push_dist(&mut l, m);
                l.push_str("|");
                push_grade(&mut l, k.max_x10 as i32);
            }
            None => l.push_str("--"),
        }
        l
    }

    /// Once a second: says "STEEP 12% IN 300 M" when the next descent steeper than the setting is within
    /// [`Self::STEEP_AHEAD_M`], once.
    pub(crate) fn descent_second(&mut self) {
        let pct = STEEP_DESCENT_PCT.get(self.settings.steep_descent).copied().unwrap_or(0);
        if self.ride.state() == ride::State::Idle {
            self.descent_said = None;
        }
        if pct == 0 || self.ride.state() != ride::State::Running {
            return;
        }
        let found = self
            .descents_here()
            .and_then(|(mut ds, d)| Some((ds.find(|k| k.start_m > d && k.max_x10 >= u32::from(pct) * 10)?, d)));
        let Some((k, d)) = found else { return };
        let to = k.start_m - d;
        // where it starts on the ride's odometer: the same wherever the road is traced from
        let at = self.ride.dist_m() as i32 + to as i32;
        if to > Self::STEEP_AHEAD_M || self.descent_said.is_some_and(|s| (s - at).abs() < Self::SAME_DESCENT_M) {
            return;
        }
        self.descent_said = Some(at);
        let mut l = Line::<20>::new();
        l.push_str("STEEP ");
        l.push_u32((k.max_x10 + 5) / 10);
        l.push_str("% IN ");
        l.push_u32((to + 25) / 50 * 50);
        l.push_str(" M");
        self.light.idle_ms = 0;
        self.say_as(l.as_str(), Banner::Ride);
        self.tones.play(&Self::CUE_TONES);
    }
}
