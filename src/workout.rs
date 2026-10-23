//! The workout player: which step of a planned workout ([`crate::storage::workout`]) is running and how much of
//! it is left. Pure logic, fed one second at a time: the step's clock moves only when the ride's timer moved (a
//! pause or an auto-pause stops both), a distance step ends on the ride's distance, an open step on Lap
//! ([`Player::skip`]). Each change comes out as an [`Event`] for the app to show and sound: a step started, the
//! last 3 seconds of a timed step (3-2-1 beeps, as bryton-S510-mods' patch P1 does), the workout done.

use crate::storage::workout::{Length, Step};

/// Seconds before a timed step ends that each beep (3, 2, 1).
pub const COUNTDOWN_S: u32 = 3;

/// What changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Step `n` (from 0) started.
    Step(usize),
    /// A timed step has this many seconds left (3, 2, 1).
    Countdown(u32),
    /// The last step ended.
    Done,
}

/// What is left of the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Seconds.
    Time(u32),
    /// Metres.
    Distance(u32),
    /// Until Lap.
    Lap,
    /// The workout is done.
    Done,
}

/// Where a workout is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    /// The step running (past the last: done).
    step: usize,
    /// Timer seconds into it.
    elapsed_s: u32,
    /// The ride's distance when it started, m.
    from_m: u32,
}

impl Player {
    /// Starts at step `at` of `steps`, with the ride at `dist_m`: the player and its first event.
    pub fn start(steps: &[Step], at: usize, dist_m: u32) -> (Player, Event) {
        let p = Player { step: at, elapsed_s: 0, from_m: dist_m };
        (p, if at < steps.len() { Event::Step(at) } else { Event::Done })
    }

    /// The step running; None once done.
    pub fn step(&self, steps: &[Step]) -> Option<usize> {
        (self.step < steps.len()).then_some(self.step)
    }

    /// One second: `ticked` if the ride's timer moved, the ride now at `dist_m`.
    pub fn second(&mut self, steps: &[Step], ticked: bool, dist_m: u32) -> Option<Event> {
        let s = steps.get(self.step)?;
        if !ticked {
            return None;
        }
        self.elapsed_s = self.elapsed_s.saturating_add(1);
        match s.len {
            Length::Time(t) if self.elapsed_s >= t => Some(self.skip(steps, dist_m)),
            Length::Time(t) if t - self.elapsed_s <= COUNTDOWN_S => Some(Event::Countdown(t - self.elapsed_s)),
            Length::Distance(m) if dist_m.saturating_sub(self.from_m) >= m => Some(self.skip(steps, dist_m)),
            _ => None,
        }
    }

    /// The next step (Lap, or the step's end), the ride at `dist_m`.
    pub fn skip(&mut self, steps: &[Step], dist_m: u32) -> Event {
        self.step = (self.step + 1).min(steps.len());
        (self.elapsed_s, self.from_m) = (0, dist_m);
        if self.step < steps.len() { Event::Step(self.step) } else { Event::Done }
    }

    /// What is left of the step, the ride at `dist_m`.
    pub fn left(&self, steps: &[Step], dist_m: u32) -> Left {
        match steps.get(self.step).map(|s| s.len) {
            None => Left::Done,
            Some(Length::Time(t)) => Left::Time(t.saturating_sub(self.elapsed_s)),
            Some(Length::Distance(m)) => Left::Distance(m.saturating_sub(dist_m.saturating_sub(self.from_m))),
            Some(Length::Open) => Left::Lap,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::storage::workout::Target;
    use std::vec::Vec;

    fn st(len: Length) -> Step {
        Step { len, target: Target::OPEN, intensity: 0 }
    }

    /// Runs `secs` seconds, the timer moving when `moving(s)`, 5 m a moving second: the events with their second.
    fn run(p: &mut Player, steps: &[Step], secs: u32, moving: impl Fn(u32) -> bool) -> Vec<(u32, Event)> {
        let mut dist = 0;
        (1..=secs)
            .filter_map(|s| {
                dist += if moving(s) { 5 } else { 0 };
                p.second(steps, moving(s), dist).map(|e| (s, e))
            })
            .collect()
    }

    #[test]
    fn timed_steps_count_down_and_follow() {
        let steps = [st(Length::Time(5)), st(Length::Time(2)), st(Length::Time(4))];
        let (mut p, first) = Player::start(&steps, 0, 0);
        assert_eq!(first, Event::Step(0));
        let ev = run(&mut p, &steps, 12, |_| true);
        use Event::*;
        // 5 s: beeps at 3, 2, 1 left, then the next; a 2 s step only has 1 left to beep; the last one ends the workout
        let want = [(2, Countdown(3)), (3, Countdown(2)), (4, Countdown(1)), (5, Step(1))];
        assert_eq!(ev[..4], want);
        assert_eq!(
            ev[4..],
            [(6, Countdown(1)), (7, Step(2)), (8, Countdown(3)), (9, Countdown(2)), (10, Countdown(1)), (11, Done)]
        );
        assert_eq!((p.step(&steps), p.left(&steps, 0)), (None, Left::Done));
        assert_eq!(p.second(&steps, true, 0), None, "done stays done");
    }

    #[test]
    fn pauses_with_the_ride_timer() {
        let steps = [st(Length::Time(10)), st(Length::Time(10))];
        let (mut p, _) = Player::start(&steps, 0, 0);
        // stopped for seconds 4 to 8: the step ends 5 s late
        let ev = run(&mut p, &steps, 20, |s| !(4..=8).contains(&s));
        assert_eq!(ev.iter().find(|e| e.1 == Event::Step(1)).map(|e| e.0), Some(15));
        assert_eq!(p.left(&steps, 0), Left::Time(5));
    }

    #[test]
    fn open_steps_end_on_lap_distance_on_the_distance() {
        let steps = [st(Length::Open), st(Length::Distance(20)), st(Length::Time(3))];
        let (mut p, _) = Player::start(&steps, 0, 100);
        assert!(run(&mut p, &steps, 30, |_| true).is_empty(), "an open step waits for Lap");
        assert_eq!(p.left(&steps, 0), Left::Lap);
        assert_eq!(p.skip(&steps, 100), Event::Step(1));
        assert_eq!(p.left(&steps, 110), Left::Distance(10));
        let ev: Vec<_> = [105, 110, 115, 120].into_iter().filter_map(|d| p.second(&steps, true, d)).collect();
        assert_eq!(ev, [Event::Step(2)], "20 m from 100 m: at 120");
        assert_eq!(p.left(&steps, 120), Left::Time(3));
    }

    #[test]
    fn lap_skips_and_the_last_step_ends_it() {
        let steps = [st(Length::Time(60)), st(Length::Time(60))];
        let (mut p, _) = Player::start(&steps, 1, 0);
        assert_eq!(p.step(&steps), Some(1), "started from a later step");
        assert_eq!(p.skip(&steps, 0), Event::Done);
        assert_eq!(p.skip(&steps, 0), Event::Done, "and stays so");
        assert_eq!(Player::start(&steps, 5, 0).1, Event::Done);
        assert_eq!(Player::start(&[], 0, 0).1, Event::Done);
    }
}
