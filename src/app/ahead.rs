//! Climbs on the road ahead with no course followed: the memory (from main.rs), the second's step (the rider placed on
//! the trace, a new trace when needed, the banner and beep at a climb's start, as a course's), the work spread over
//! the main loop's passes (tiles of stock's map files read, snap, trace), and the climb fields fed from the trace
//! when no course is followed (`climbs_ahead`; BIKE > CLIMBS AHEAD turns it off).

use crate::*;

impl App {
    /// Work per pass at most, in microseconds of CPU ([`Board::awake_us`]): a tile read, inflated and decoded is the
    /// unit, so one may go over it; the next waits for the next pass.
    const AHEAD_US: u32 = 6_000;
    /// Units of work per pass at most (where the CPU time is not measured, as in the simulation).
    const AHEAD_STEPS: u32 = 6;

    /// The last tile of the road ahead read, inflated and decoded (all its slices), the longest so far, and the
    /// longest single call on a tile, in microseconds of CPU ([`Board::awake_us`]; `tools/dev.py info`).
    pub fn ahead_tile_us(&self) -> (u32, u32, u32) {
        (self.ahead_us.last, self.ahead_us.max, self.ahead_us.slice)
    }

    /// Gives the road ahead its memory (main.rs: about 110 KB in AXI SRAM) and the buffer map tiles are inflated
    /// into (64 KB or more of the map's SDRAM); without them there are no climbs ahead.
    pub fn set_ahead_memory(&mut self, mem: &'static mut climbs_ahead::Ahead, scratch: &'static mut [u16]) {
        self.ahead = Some((mem, scratch));
    }

    /// Climbs ahead are on: switched on, a ride (paused too: the trace and the climb said are kept, the work waits),
    /// outdoors, no course followed, the card not lent.
    fn ahead_on(&self) -> bool {
        self.settings.ahead
            && self.ride.state() != ride::State::Idle
            && !self.indoors()
            && self.courses.as_deref().is_none_or(|c| c.on.is_none())
            && !self.emmc.lent()
    }

    /// The ride is an MTB ride: climbs ahead follow paths and tracks.
    pub(crate) fn mtb(&self) -> bool {
        self.ride_profile() == ride::Profile::Mtb
    }

    /// Once a second: the rider on the trace, a new trace when needed, a climb to say.
    pub(crate) fn ahead_second(&mut self) {
        let on = self.ahead_on();
        let fix = self.gps.pvt.filter(|p| p.good()).map(|p| climbs_ahead::Fix {
            lat: p.lat,
            lon: p.lon,
            heading_e5: p.heading,
            speed_mm_s: p.speed_mm_s,
        });
        let mtb = self.mtb();
        let medium = self.settings.climbs_medium;
        let Some((a, _)) = self.ahead.as_mut() else { return };
        a.set_trails(mtb);
        a.set_medium(medium);
        if let Some(c) = a.second(on, fix) {
            self.course_event(course::Event::Climb(c));
        }
    }

    /// The trace goes on, [`Self::AHEAD_STEPS`] units of work a pass at most or [`Self::AHEAD_US`] of CPU time: a
    /// tile, the snap, the trace up to the next tile it needs.
    #[inline(never)]
    pub(crate) fn step_ahead(&mut self, b: &mut impl Board) {
        if self.ride.state() != ride::State::Running {
            return; // paused: what is half done waits
        }
        let Some((a, mem)) = self.ahead.as_mut().filter(|(a, _)| a.busy()) else { return };
        let t0 = b.awake_us();
        for _ in 0..Self::AHEAD_STEPS {
            let (tiles, loading, t1) = (a.io().0, a.loading(), b.awake_us());
            let more = a.work(&mut emmc::Volume(&mut self.emmc, b), mem);
            let spent = b.awake_us().wrapping_sub(t1);
            if loading || a.loading() || a.io().0 != tiles {
                // a slice of a tile: the longest one, and the tile's whole (tools/dev.py info says how long)
                let (total, max, slice) =
                    (self.ahead_acc.saturating_add(spent), self.ahead_us.max, self.ahead_us.slice);
                self.ahead_us.slice = slice.max(spent);
                self.ahead_acc = total;
                if !a.loading() {
                    self.ahead_us = AheadUs { last: total, max: max.max(total), slice: self.ahead_us.slice };
                    self.ahead_acc = 0;
                }
            }
            if !more || b.awake_us().wrapping_sub(t0) >= Self::AHEAD_US {
                break;
            }
        }
    }

    /// The climb fields when no course is followed: the same text as a course's, from the road ahead; "--" while there
    /// is no climb ahead on it.
    pub(crate) fn ahead_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let ahead = self.ahead.as_ref().map(|(a, _)| &**a).filter(|a| a.on_road());
        let here = ahead.and_then(|a| Some((a, a.climb_at(a.along_m())?)));
        match (f, here) {
            // on a climb: what is left of it and the grade ahead; before it: its length and grade
            (Field::Climb, Some((a, (k, on)))) => {
                let d = a.along_m();
                let (m, g) = match on {
                    true => (k.end_m().saturating_sub(d), a.grade_ahead(d).unwrap_or(0)),
                    false => (k.len_m, k.grade_x10() as i32),
                };
                if m < 10_000 {
                    ui::card::push_km(&mut l, m);
                } else {
                    l.push_u32(m / 1000);
                }
                l.push_str("K|");
                push_grade(&mut l, g);
            }
            (Field::ClimbGain, Some((a, (k, true)))) => {
                let d = a.along_m();
                let top = a.alt_at(k.end_m()).zip(a.alt_at(d)).map_or(0, |(t, h)| (t - h).max(0) / 10);
                l.push_u32(top as u32);
            }
            (Field::ClimbGain, Some((_, (k, false)))) => l.push_u32(k.gain_m),
            _ => l.push_str("--"),
        }
        l
    }

    /// CLIMB's edge bar from the road ahead, by the grade of the climb running or the next; BLACK elsewhere.
    pub(crate) fn ahead_bar(&self, f: Field) -> u16 {
        let Some(a) = self.ahead.as_ref().map(|(a, _)| &**a).filter(|a| f == Field::Climb && a.on_road()) else {
            return ui::BLACK;
        };
        let d = a.along_m();
        let grade = a.climb_at(d).and_then(|(k, on)| match on {
            true => a.grade_ahead(d),
            false => Some(k.grade_x10() as i32),
        });
        grade.map_or(ui::BLACK, ui::grade_color)
    }
}
