//! Reminders and alerts during a ride, fed one second at a time: eat and drink every so many minutes of ride time
//! (stock has six reminder kinds by time, distance, energy or heart rate; Garmin and Wahoo eat / drink by time), and
//! heart rate over a limit. Pure logic: the app says each one in a banner and beeps.
//!
//! The heart-rate alert is not a nag: it fires after [`HR_OVER_S`] seconds over the limit, then again only once the
//! heart rate has come back [`HR_REARM_BPM`] under it, and at most every [`HR_EVERY_S`].

/// Seconds over the heart-rate limit before the alert.
pub const HR_OVER_S: u32 = 10;
/// Below the limit by this much re-arms the alert.
pub const HR_REARM_BPM: u16 = 5;
/// At most one heart-rate alert this often.
pub const HR_EVERY_S: u32 = 60;

/// Cadence under this, rpm, in ERG is too slow: the trainer keeps the watts, so the resistance climbs.
pub const CADENCE_LOW_RPM: u16 = 50;
/// Seconds under it before the warning.
pub const CADENCE_LOW_S: u32 = 5;
/// At most one cadence warning this often.
pub const CADENCE_EVERY_S: u32 = 60;

/// What to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    /// Time to eat.
    Eat,
    /// Time to drink.
    Drink,
    /// Heart rate over the limit.
    HrHigh,
    /// Cadence under [`CADENCE_LOW_RPM`] in ERG.
    CadenceLow,
}

impl Alert {
    /// Whether `text` is one of the banners of an alert.
    pub fn is_text(text: &str) -> bool {
        [Alert::Eat, Alert::Drink, Alert::HrHigh, Alert::CadenceLow].iter().any(|a| a.text() == text)
    }

    /// The banner's text.
    pub fn text(self) -> &'static str {
        match self {
            Alert::Eat => "EAT REMINDER",
            Alert::Drink => "DRINK REMINDER",
            Alert::HrHigh => "HEART RATE HIGH",
            Alert::CadenceLow => "CADENCE LOW",
        }
    }
}

/// Alerts due and not said yet: one bit each, so two in the same second (or a catch-up of several) are not lost to
/// the one the banner shows; said one at a time, the heart rate first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Queue(u8);

impl Queue {
    /// Adds `a` (nothing for `None`).
    pub fn push(&mut self, a: Option<Alert>) {
        self.0 |= a.map_or(0, |a| 1 << a as u8);
    }

    /// Takes the next to say: the heart rate, the cadence, eat, drink.
    pub fn pop(&mut self) -> Option<Alert> {
        let a = [Alert::HrHigh, Alert::CadenceLow, Alert::Eat, Alert::Drink]
            .into_iter()
            .find(|&a| self.0 & 1 << a as u8 != 0)?;
        self.0 &= !(1 << a as u8);
        Some(a)
    }
}

/// The reminders' and alerts' state for one ride.
#[derive(Clone, Copy, Debug, Default)]
pub struct Alerts {
    over_s: u32,
    armed: bool,
    since_hr_s: u32,
    /// The timer at the last call (a reminder is due when the timer reaches its minute, not while it stands still).
    last_timer_s: u32,
    /// Reminders due and not said yet (one a second): bit 0 eat, bit 1 drink.
    pending: u8,
    /// Seconds in a row under [`CADENCE_LOW_RPM`] in ERG, and since the last warning.
    slow_s: u32,
    since_cadence_s: u32,
}

impl Alerts {
    /// A new ride: everything armed.
    pub fn new() -> Self {
        Alerts {
            over_s: 0,
            armed: true,
            since_hr_s: HR_EVERY_S,
            last_timer_s: 0,
            pending: 0,
            slow_s: 0,
            since_cadence_s: CADENCE_EVERY_S,
        }
    }

    /// One second of a moving ride: the cadence under [`CADENCE_LOW_RPM`] for [`CADENCE_LOW_S`] seconds with `erg` on
    /// (the trainer holds the watts) gives [`Alert::CadenceLow`], at most once in [`CADENCE_EVERY_S`]. No reading is
    /// not slow (no sensor), and a pause or no ERG starts the count again.
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts};
    /// let mut a = Alerts::new();
    /// let fired: Vec<_> = (1..=130).filter_map(|t| a.cadence(Some(40), true).map(|x| (t, x))).collect();
    /// assert_eq!(fired, [(5, Alert::CadenceLow), (65, Alert::CadenceLow), (125, Alert::CadenceLow)]);
    /// assert_eq!((1..=100).find_map(|_| a.cadence(Some(40), false)), None, "not in ERG");
    /// ```
    pub fn cadence(&mut self, rpm: Option<u16>, erg: bool) -> Option<Alert> {
        self.since_cadence_s = self.since_cadence_s.saturating_add(1);
        if erg && rpm.is_some_and(|c| c < CADENCE_LOW_RPM) {
            self.slow_s = self.slow_s.saturating_add(1);
        } else {
            self.slow_s = 0;
        }
        if self.slow_s >= CADENCE_LOW_S && self.since_cadence_s >= CADENCE_EVERY_S {
            self.since_cadence_s = 0;
            return Some(Alert::CadenceLow);
        }
        None
    }

    /// One second of a moving ride: `timer_s` the ride's timer after it, `hr` the heart rate; the settings `eat_min`
    /// and `drink_min` (0: off) and `hr_high` (0: off). At most one alert a second: the heart rate first, then eat,
    /// then drink.
    ///
    /// ```
    /// use bryton_rustfw::alerts::{Alert, Alerts};
    /// let mut a = Alerts::new();
    /// let fired: Vec<_> = (1..=3600).filter_map(|t| a.second(t, None, 45, 20, 0).map(|x| (t, x))).collect();
    /// assert_eq!(fired[..3], [(1200, Alert::Drink), (2400, Alert::Drink), (2700, Alert::Eat)]);
    /// ```
    pub fn second(
        &mut self,
        timer_s: u32,
        hr: Option<u16>,
        eat_min: u16,
        drink_min: u16,
        hr_high: u16,
    ) -> Option<Alert> {
        self.since_hr_s = self.since_hr_s.saturating_add(1);
        match hr {
            Some(h) if hr_high > 0 && h > hr_high => self.over_s += 1,
            Some(h) if h + HR_REARM_BPM <= hr_high => (self.over_s, self.armed) = (0, true),
            _ => self.over_s = 0,
        }
        // the reminders due are kept first: one said a second, the heart rate's alert ahead of them
        let moved = timer_s != core::mem::replace(&mut self.last_timer_s, timer_s);
        let due = |min: u16| moved && min > 0 && timer_s > 0 && timer_s.is_multiple_of(u32::from(min) * 60);
        self.pending |= u8::from(due(eat_min)) | u8::from(due(drink_min)) << 1;
        if self.armed && self.over_s >= HR_OVER_S && self.since_hr_s >= HR_EVERY_S {
            (self.armed, self.since_hr_s) = (false, 0);
            return Some(Alert::HrHigh);
        }
        let (bit, alert) = match self.pending {
            0 => return None,
            p if p & 1 != 0 => (1, Alert::Eat),
            _ => (2, Alert::Drink),
        };
        self.pending &= !bit;
        Some(alert)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn heart_rate_alert_does_not_nag() {
        let mut a = Alerts::new();
        let mut fired = 0;
        // 15 s at 180 over a 170 limit: one alert, after 10 s
        for t in 1..=15 {
            if a.second(t, Some(180), 0, 0, 170).is_some() {
                assert_eq!(t, HR_OVER_S);
                fired += 1;
            }
        }
        // hovering just under the limit, not 5 under: no new alert when it goes over again
        for t in 16..=30 {
            assert_eq!(a.second(t, Some(168), 0, 0, 170), None);
        }
        for t in 31..=100 {
            fired += usize::from(a.second(t, Some(175), 0, 0, 170).is_some());
        }
        assert_eq!(fired, 1, "not re-armed");
        // well under, then over again after a minute: a second alert
        a.second(101, Some(150), 0, 0, 170);
        for t in 102..=200 {
            fired += usize::from(a.second(t, Some(175), 0, 0, 170).is_some());
        }
        assert_eq!(fired, 2);
        // off
        let mut b = Alerts::new();
        assert!((1..100).all(|t| b.second(t, Some(200), 0, 0, 0).is_none()));
    }

    #[test]
    fn the_queue_keeps_what_is_due_together() {
        let mut q = Queue::default();
        q.push(Some(Alert::Drink));
        q.push(Some(Alert::CadenceLow));
        q.push(Some(Alert::HrHigh));
        q.push(None);
        q.push(Some(Alert::Drink));
        assert_eq!(
            [q.pop(), q.pop(), q.pop(), q.pop()],
            [Some(Alert::HrHigh), Some(Alert::CadenceLow), Some(Alert::Drink), None]
        );
    }

    #[test]
    fn cadence_warning_waits_five_seconds_and_does_not_nag() {
        let mut a = Alerts::new();
        // pedalling fine, then slow: the count starts when it goes slow, 4 s are not enough
        assert!((1..=10).all(|_| a.cadence(Some(85), true).is_none()));
        assert!((1..=4).all(|_| a.cadence(Some(45), true).is_none()));
        assert_eq!(a.cadence(Some(80), true), None, "back up: the count starts again");
        assert!((1..=4).all(|_| a.cadence(Some(0), true).is_none()));
        assert_eq!(a.cadence(Some(0), true), Some(Alert::CadenceLow), "5 s at 0 rpm");
        // still slow: not again before a minute
        assert!((1..60).all(|_| a.cadence(Some(0), true).is_none()));
        assert_eq!(a.cadence(Some(0), true), Some(Alert::CadenceLow));
        // no reading, or ERG off: nothing
        let mut b = Alerts::new();
        assert!((1..=100).all(|_| b.cadence(None, true).is_none() && b.cadence(Some(0), false).is_none()));
    }

    #[test]
    fn eat_then_drink_when_both_are_due_and_nothing_while_paused() {
        let mut a = Alerts::new();
        assert_eq!(a.second(0, None, 30, 20, 0), None, "not at the start");
        assert_eq!(a.second(3600, None, 30, 20, 0), Some(Alert::Eat));
        assert_eq!(a.second(3600, None, 30, 20, 0), Some(Alert::Drink), "the next second");
        assert_eq!(a.second(3600, None, 30, 20, 0), None, "paused on the minute: once only");
    }

    /// The heart-rate alert and an eat minute (and a drink minute) in the same second: none is lost (review 2).
    #[test]
    fn heart_rate_alert_keeps_the_reminders_due() {
        let mut a = Alerts::new();
        let fired: std::vec::Vec<_> =
            (1..=20).filter_map(|t| a.second(1190 + t, Some(180), 20, 20, 170).map(|x| (1190 + t, x))).collect();
        assert_eq!(fired, [(1200, Alert::HrHigh), (1201, Alert::Eat), (1202, Alert::Drink)]);
    }
}
