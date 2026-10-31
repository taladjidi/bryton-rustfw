//! CLIMBS DONE (roadmap 3c): the climbs of the followed course, or of the road ahead, that the rider rode up to the top,
//! newest first, in a bounded list ([`MAX`]) with length, gain, grade, time, VAM and the average power or heart rate.
//! Seen once a second from [`App::climb_view`] (never a live segment's); a climb joined past its first
//! [`JOIN_M`] metres (after a resume) is not counted, its time would be wrong. The list lives in RAM only: it starts
//! empty with each ride and after a reset-resume (keeping it is a recorder change, not made here). Nothing here
//! touches the card. A banner says the last one ("CLIMB 2.3K 14:05 VAM 850"); the list is the ride menu's CLIMBS DONE.

use crate::course::Climb;
use crate::*;

/// Climbs kept; the oldest goes when a new one is done.
pub(crate) const MAX: usize = 20;
/// A climb is joined only in its first metres.
const JOIN_M: u32 = 100;
/// The top counts as reached this close to the end when the climb leaves the view (a second is up to 15 m).
const END_SLACK_M: u32 = 60;

/// A climb done, packed (10 bytes); 0 for the average of a reading never heard.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct Done {
    pub(crate) len_m: u16,
    pub(crate) gain_m: u16,
    pub(crate) time_s: u16,
    pub(crate) avg_w: u16,
    pub(crate) avg_hr: u8,
}

impl Done {
    /// Metres climbed per hour; 0 for no time.
    pub(crate) fn vam(&self) -> u32 {
        (u32::from(self.gain_m) * 3600).checked_div(u32::from(self.time_s)).unwrap_or(0)
    }

    /// The average grade, whole percent.
    pub(crate) fn grade(&self) -> u32 {
        let l = u32::from(self.len_m);
        (u32::from(self.gain_m) * 100 + l / 2).checked_div(l).unwrap_or(0)
    }
}

/// The climb being ridden.
#[derive(Clone, Copy)]
struct Cur {
    k: Climb,
    t0: u32,
    last_t: u32,
    last_along: u32,
    sum_w: u32,
    n_w: u32,
    sum_hr: u32,
    n_hr: u32,
}

impl Cur {
    fn done(&self) -> Done {
        let sat = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
        Done {
            len_m: sat(self.k.len_m),
            gain_m: sat(self.k.gain_m),
            time_s: sat(self.last_t.saturating_sub(self.t0)),
            avg_w: sat(self.sum_w.checked_div(self.n_w).unwrap_or(0)),
            avg_hr: u8::try_from(self.sum_hr.checked_div(self.n_hr).unwrap_or(0)).unwrap_or(u8::MAX),
        }
    }
}

/// The list and the climb under way.
pub(crate) struct ClimbsDone {
    list: [Done; MAX],
    n: usize,
    cur: Option<Cur>,
}

impl ClimbsDone {
    /// Nothing done.
    pub(crate) fn new() -> Self {
        ClimbsDone { list: [Done::default(); MAX], n: 0, cur: None }
    }

    /// A new ride.
    pub(crate) fn clear(&mut self) {
        (self.n, self.cur) = (0, None);
    }

    /// Climbs in the list.
    pub(crate) fn len(&self) -> usize {
        self.n
    }

    /// Climb `k`, newest first.
    pub(crate) fn get(&self, k: usize) -> Option<&Done> {
        self.list.get(..self.n)?.get(k)
    }

    fn push(&mut self, d: Done) {
        self.list.rotate_right(1);
        if let Some(f) = self.list.first_mut() {
            *f = d;
        }
        self.n = (self.n + 1).min(MAX);
    }

    /// One second: the climb in view and the rider's metres along (None: none in view), the ride's timer, the
    /// readings. Returns the climb that was just completed (its top reached, or left in view close to it).
    pub(crate) fn see(&mut self, view: Option<(Climb, u32)>, t: u32, w: Option<u16>, hr: Option<u16>) -> Option<Done> {
        let mut out = None;
        if let Some(c) = self.cur
            && view.is_none_or(|(k, _)| k.start_m != c.k.start_m)
        {
            if c.last_along + END_SLACK_M >= c.k.end_m() {
                out = Some(c.done());
            }
            self.cur = None;
        }
        if let Some((k, along)) = view
            && along >= k.start_m
        {
            if self.cur.is_none() && along <= k.start_m + JOIN_M && along < k.end_m() {
                self.cur = Some(Cur { k, t0: t, last_t: t, last_along: along, sum_w: 0, n_w: 0, sum_hr: 0, n_hr: 0 });
            }
            if let Some(c) = &mut self.cur {
                (c.last_t, c.last_along) = (t, along);
                if let Some(w) = w {
                    (c.sum_w, c.n_w) = (c.sum_w + u32::from(w), c.n_w + 1);
                }
                if let Some(h) = hr {
                    (c.sum_hr, c.n_hr) = (c.sum_hr + u32::from(h), c.n_hr + 1);
                }
                if along >= c.k.end_m() {
                    out = Some(c.done());
                    self.cur = None;
                }
            }
        }
        if let Some(d) = out {
            self.push(d);
        }
        out
    }
}

impl App {
    /// Once a second (app/riding.rs `second`): the list takes the climb in view; a new ride clears it.
    pub(crate) fn climbs_second(&mut self) {
        if self.ride.state() == ride::State::Idle {
            self.climbs_done.clear();
            return;
        }
        let view = self.climb_view().map(|n| (n.climb, n.along));
        let (t, w, hr) = (self.ride.timer_s(), self.live.shown(Metric::Power), self.live.shown(Metric::Hr));
        if let Some(d) = self.climbs_done.see(view, t, w, hr) {
            let mut l = Line::<40>::new();
            l.push_str("CLIMB ");
            ui::card::push_km(&mut l, u32::from(d.len_m));
            l.push_str("K ");
            l.push_str(ui::segment::time_text(u32::from(d.time_s)).as_str());
            l.push_str(" VAM ");
            l.push_u32(d.vam());
            self.say_as(l.as_str(), Banner::Summary);
        }
    }

    /// Row `k` of CLIMBS DONE: "2.3K 6% 14:05", then "850 210W" (VAM, average power; "B" for the heart rate when no
    /// power was heard).
    pub(crate) fn climb_done_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<()> {
        let d = self.climbs_done.get(k)?;
        ui::card::push_km(label, u32::from(d.len_m));
        label.push_str("K ");
        label.push_u32(d.grade());
        label.push_str("% ");
        label.push_str(ui::segment::time_text(u32::from(d.time_s)).as_str());
        value.push_u32(d.vam());
        if d.avg_w > 0 {
            value.push_str(" ");
            value.push_u32(u32::from(d.avg_w));
            value.push_str("W");
        } else if d.avg_hr > 0 {
            value.push_str(" ");
            value.push_u32(u32::from(d.avg_hr));
            value.push_str("B");
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: Climb = Climb { start_m: 1000, len_m: 1000, gain_m: 80 };

    #[test]
    fn a_climb_ridden_to_the_top() {
        let mut c = ClimbsDone::new();
        // 1 km in 300 s from t=100, 200 W and 150 bpm all along
        for s in 0..=300u32 {
            let along = 1000 + s * 1000 / 300;
            let d = c.see(Some((K, along)), 100 + s, Some(200), Some(150));
            assert_eq!(d.is_some(), s == 300);
        }
        let d = *c.get(0).unwrap();
        assert_eq!((d.len_m, d.gain_m, d.time_s, d.avg_w, d.avg_hr), (1000, 80, 300, 200, 150));
        assert_eq!((d.vam(), d.grade()), (960, 8));
        assert!(c.see(None, 500, None, None).is_none(), "done once");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_climb_left_near_the_top_counts_and_one_left_early_does_not() {
        let mut c = ClimbsDone::new();
        c.see(Some((K, 1000)), 0, None, None);
        c.see(Some((K, 1950)), 90, None, None);
        assert!(c.see(None, 91, None, None).is_some(), "left within the slack");
        assert_eq!(c.get(0).unwrap().avg_w, 0, "no power heard");
        c.see(Some((K, 1000)), 100, None, None);
        c.see(Some((K, 1500)), 150, None, None);
        assert!(c.see(None, 151, None, None).is_none(), "turned off half way");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_climb_joined_late_is_not_counted_and_the_list_is_bounded() {
        let mut c = ClimbsDone::new();
        c.see(Some((K, 1500)), 0, None, None);
        assert!(c.see(Some((K, 2000)), 50, None, None).is_none());
        assert_eq!(c.len(), 0);
        for i in 0..MAX as u32 + 5 {
            let k = Climb { start_m: 10_000 * (i + 1), len_m: 500, gain_m: 30 + i };
            c.see(Some((k, k.start_m)), i * 1000, None, None);
            c.see(Some((k, k.end_m())), i * 1000 + 100, None, None);
        }
        assert_eq!(c.len(), MAX);
        assert_eq!(c.get(0).unwrap().gain_m, 30 + MAX as u16 + 4, "newest first");
        assert_eq!(c.get(MAX - 1).unwrap().gain_m, 35);
        assert!(c.get(MAX).is_none());
    }
}
