//! Two cues around a ride's start, neither of which starts, stops or touches the recording by itself:
//! - the START REMINDER: with no ride, the bike moving for 10 s (GPS over 8 km/h, the wheel sensor turning, or power
//!   above 0) says START THE RIDE? with a beep, at most every 5 minutes; Start then starts the ride as always;
//! - the READY CHECK: a new ride's Start shows a two-line card for 3 s ([`Banner::Ready`]): GPS and battery, then the
//!   paired sensors heard or the missing ones. The ride starts at the press (the recorder is fed in the same call as
//!   before); the card is a banner over it, any input skips it, and Start on it only skips it (the ride runs already).
//!   Nothing here can fail or wait: no card is also no delay.

use crate::*;

/// The bike must move this many seconds in a row for the reminder.
const MOVE_S: u8 = 10;
/// The reminder is said again no sooner than this many seconds after the last.
const REMIND_EVERY_S: u16 = 300;
/// GPS speed above this is moving (8 km/h), mm/s.
const MOVING_MM_S: u32 = 2223;
/// The card warns of a battery at or below this percent.
const LOW_PERCENT: u8 = 15;

/// What the reminder counts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StartCues {
    /// Seconds the bike has moved in a row (without a ride).
    moving_s: u8,
    /// Seconds since the last reminder (saturating: the first one is due at once).
    since_s: u16,
}

impl StartCues {
    /// No movement seen, a reminder due.
    pub(crate) fn new() -> Self {
        StartCues { moving_s: 0, since_s: REMIND_EVERY_S }
    }
}

impl App {
    /// The bike moves: GPS over 8 km/h, a wheel sensor heard turning, or power above 0.
    fn bike_moving(&self) -> bool {
        let gps = self.gps.pvt.is_some_and(|p| p.good() && p.speed_mm_s > MOVING_MM_S);
        let wheel = [Kind::Speed, Kind::SpeedCadence].iter().any(|&k| self.sensor_heard(k))
            && self.live.get(Metric::Speed).is_some_and(|v| v > 0);
        gps || wheel || self.live.get(Metric::Power).is_some_and(|v| v > 0)
    }

    /// The sensor of `kind` is paired and was heard in the last [`sensors::STALE_MS`].
    fn sensor_heard(&self, kind: Kind) -> bool {
        let i = kind as usize;
        matches!(self.radio.sensors.get(i), Some(Sensor::Paired(_)))
            && self.radio.heard_ms.get(i).is_some_and(|&ms| ms < sensors::STALE_MS)
    }

    /// Once a second: counts the movement, and says START THE RIDE? when it held for 10 s (see the module doc).
    pub(crate) fn start_reminder_second(&mut self) {
        let c = &mut self.cues;
        c.since_s = c.since_s.saturating_add(1);
        let quiet = self.ride.state() != ride::State::Idle
            || !self.settings.start_reminder
            || self.strip.charging
            || !matches!(self.screen, Screen::Main | Screen::Ride(_));
        if quiet || !self.bike_moving() {
            self.cues.moving_s = 0;
            return;
        }
        let c = &mut self.cues;
        c.moving_s = c.moving_s.saturating_add(1);
        if c.moving_s >= MOVE_S && c.since_s >= REMIND_EVERY_S {
            c.since_s = 0;
            self.say_as("START THE RIDE?", Banner::Ride);
            self.tones.play(&BEEP_ALERT); // dropped by SOUND OFF
        }
    }

    /// A new ride's ready card, if READY CHECK is on: true when it was said (it replaces RIDE STARTED).
    pub(crate) fn ready_card(&mut self) -> bool {
        if !self.settings.ready_check {
            return false;
        }
        let mut l = Line::<40>::new();
        l.push_str(if self.gps.pvt.is_some_and(|p| p.good()) { "GPS YES  " } else { "GPS NO  " });
        let low = match self.strip.battery {
            Some(p) => {
                l.push_str("BAT ");
                l.push_u32(u32::from(p));
                l.push_str("%\n");
                p <= LOW_PERCENT
            }
            None => {
                l.push_str("BAT --\n");
                false
            }
        };
        // the paired sensors, heard or missing (the trainer has its own screens; the shifting systems only when heard:
        // they may stay silent at a standstill); the second line is the warning, names cut with "+N" to fit
        let counted = |k: &&Kind| {
            let paired = matches!(self.radio.sensors.get(**k as usize), Some(Sensor::Paired(_)));
            paired && **k != Kind::Trainer && (!matches!(**k, Kind::Shifting | Kind::Di2) || self.sensor_heard(**k))
        };
        let mut names = [""; Kind::ALL.len()];
        let mut miss = 0;
        for k in Kind::ALL.iter().filter(counted).filter(|k| !self.sensor_heard(**k)) {
            names[miss] = k.short();
            miss += 1;
        }
        let mut total = miss;
        if miss == 0 {
            for k in Kind::ALL.iter().filter(counted) {
                names[total] = k.short();
                total += 1;
            }
        }
        let mut l2 = Line::<40>::new();
        if low {
            l2.push_str("LOW BATT");
        }
        if total == 0 {
            l2.push_str(if low { "" } else { "NO SENSORS PAIRED" });
        } else {
            if miss > 0 {
                l2.push_str(if low { " NO" } else { "NO" });
            }
            for (i, n) in names.iter().take(total).enumerate() {
                let sep = if l2.as_str().is_empty() { "" } else { " " };
                let left = total - i - 1;
                let tail = if left > 0 {
                    " +9"
                } else if miss == 0 {
                    " OK"
                } else {
                    ""
                };
                let mut t = l2;
                t.push_str(sep);
                t.push_str(n);
                t.push_str(tail);
                if ui::LABEL.width(t.as_str()) > ui::W - 12 {
                    l2.push_str(" +");
                    l2.push_u32((total - i) as u32);
                    break;
                }
                l2.push_str(sep);
                l2.push_str(n);
                if left == 0 && miss == 0 {
                    l2.push_str(" OK");
                }
            }
        }
        l.push_str(l2.as_str());
        self.say_as(l.as_str(), Banner::Ready);
        true
    }
}
