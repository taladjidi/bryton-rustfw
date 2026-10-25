//! A ride field's text and colour ([`App::field_text`], [`App::field_color`]), and the fixed-point and time
//! formatting they use: integers into a stack [`Line`], no floats (float formatting costs a few KB of image).

use crate::*;

/// `v / 10^shift` with `decimals` decimals (truncated): `push_fixed(l, 488566100, 7, 5)` gives "48.85661".
pub(crate) fn push_fixed(l: &mut Line<12>, v: i32, shift: u32, decimals: u32) {
    if v < 0 {
        l.push_str("-");
    }
    let v = v.unsigned_abs();
    let unit = 10u32.pow(shift);
    l.push_u32(v / unit);
    if decimals > 0 {
        l.push_str(".");
        let frac = v % unit / 10u32.pow(shift - decimals);
        let digits = 10u32.pow(decimals - 1);
        let mut d = digits;
        while d > 0 {
            l.push_str(ui::DIGITS[(frac / d % 10) as usize]);
            d /= 10;
        }
    }
}

/// `v / 10^decimals` with its decimals ("0.75").
pub(crate) fn push_fixed_u(l: &mut Line<8>, v: u32, decimals: u32) {
    let unit = 10u32.pow(decimals);
    l.push_u32(v / unit);
    if !v.is_multiple_of(unit) {
        l.push_str(".");
        let mut d = unit / 10;
        while d > 0 {
            l.push_str(ui::DIGITS[(v / d % 10) as usize]);
            d /= 10;
        }
    }
}

/// Seconds as "h:mm".
pub(crate) fn push_hm<const N: usize>(l: &mut Line<N>, s: u32) {
    l.push_u32(s / 3600);
    l.push_str(":");
    l.push_str(ui::DIGITS[(s / 600 % 6) as usize]);
    l.push_str(ui::DIGITS[(s / 60 % 10) as usize]);
}

/// Seconds as "m:ss", or "h:mm:ss" from an hour.
pub(crate) fn push_clock<const N: usize>(l: &mut Line<N>, s: u32) {
    if s >= 3600 {
        l.push_u32(s / 3600);
        l.push_str(":");
        l.push_str(ui::DIGITS[(s / 600 % 6) as usize]);
        l.push_str(ui::DIGITS[(s / 60 % 10) as usize]);
    } else {
        l.push_u32(s / 60);
    }
    l.push_str(":");
    l.push_str(ui::DIGITS[(s / 10 % 6) as usize]);
    l.push_str(ui::DIGITS[(s % 10) as usize]);
}

/// Seconds as a ride field shows a duration: under an hour "MM:SS" ("M:SS" without `pad`), then "H:MM" with the
/// seconds as a small tail ("1:23|45", [`ui::SMALL_TAIL`]), so a timer's digits are as big as a speed's.
pub(crate) fn push_duration<const N: usize>(l: &mut Line<N>, s: u32, pad: bool) {
    let two = |l: &mut Line<N>, v: u32| {
        l.push_str(ui::DIGITS[(v / 10 % 10) as usize]);
        l.push_str(ui::DIGITS[(v % 10) as usize]);
    };
    if s >= 3600 {
        l.push_u32(s / 3600);
        l.push_str(":");
        two(l, s / 60 % 60);
        l.push_str("|");
    } else {
        if pad {
            two(l, s / 60);
        } else {
            l.push_u32(s / 60);
        }
        l.push_str(":");
    }
    two(l, s % 60);
}

/// Metres as km with 2 decimals.
pub(crate) fn push_km<const N: usize>(l: &mut Line<N>, m: u32) {
    l.push_u32(m / 1000);
    l.push_str(".");
    l.push_str(ui::DIGITS[(m / 100 % 10) as usize]);
    l.push_str(ui::DIGITS[(m / 10 % 10) as usize]);
}

/// One step of FNV-1a over `bytes` (fingerprints of what a box shows).
pub(crate) fn fnv(h: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(h, |h, &x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// Months as the screens write them.
pub(crate) const MONTHS: [&str; 12] =
    ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// "FRI 23 OCT".
pub(crate) fn date_text((y, m, d): (u16, u8, u8)) -> Line<12> {
    const DAYS: [&str; 7] = ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"]; // 1970-01-01 was a Thursday
    let mut l = Line::new();
    l.push_str(DAYS[days_from_civil(y, m, d).rem_euclid(7) as usize]);
    l.push_str(" ");
    l.push_u32(u32::from(d));
    l.push_str(" ");
    l.push_str(MONTHS.get(usize::from(m).wrapping_sub(1)).copied().unwrap_or(""));
    l
}

impl App {
    /// The text of ride field `f` now ("--" without a value).
    pub(crate) fn field_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let (r, live) = (&self.ride, |m| self.live.get(m).map(u32::from));
        let (p, ftp) = (&self.power, self.settings.ftp);
        // the value, and whether it is in tenths
        let (v, tenths) = match f {
            Field::Time => {
                let (h, m) = self.strip.time;
                for d in [h / 10, h % 10, 10, m / 10, m % 10] {
                    l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
                }
                return l;
            }
            Field::Timer | Field::LapTime => {
                push_duration(&mut l, if f == Field::Timer { r.timer_s() } else { r.lap_s() }, true);
                return l;
            }
            Field::Dist | Field::LapDist => {
                let m = if f == Field::Dist { r.dist_m() } else { r.lap_dist_m() };
                // as stock: 2 decimals under 100 km, then 1
                let (unit, digits) = if m < 100_000 { (10, 2) } else { (100, 1) };
                l.push_u32(m / 1000);
                l.push_str(".");
                let frac = m % 1000 / unit;
                for d in [frac / 10, frac % 10].get(2 - digits..).unwrap_or(&[]) {
                    l.push_str(ui::DIGITS[*d as usize % 10]);
                }
                return l;
            }
            Field::Map
            | Field::Graph
            | Field::HrGraph
            | Field::WbalGraph
            | Field::DfaGraph
            | Field::AltGraph
            | Field::Profile
            | Field::WorkoutGraph => {
                return l;
            }
            Field::WeekKm | Field::WeekH | Field::YearKm | Field::TotalKm => {
                // the rides on the card (once read: see step_stock) and the one running
                if !self.stock.ready() {
                    l.push_str("--");
                    return l;
                }
                let o = &self.stock.out;
                let (live_m, live_s) = if r.state() == ride::State::Idle { (0, 0) } else { (r.dist_m(), r.timer_s()) };
                match f {
                    Field::WeekH => push_fixed_u(&mut l, (o.week.time_s + live_s) / 360, 1),
                    Field::WeekKm => l.push_u32((o.week.dist_m + live_m) / 1000),
                    Field::YearKm => l.push_u32((o.year.dist_m + live_m) / 1000),
                    _ => l.push_u32((o.all.dist_m + live_m) / 1000),
                }
                return l;
            }
            Field::Workout | Field::StepTarget | Field::StepTime | Field::NextStep => return self.workout_text(f),
            Field::WktPower => return self.gauge_now(0).value,
            Field::Music if self.phone.now.title.as_str().is_empty() => (None, false),
            Field::Music => {
                push_duration(&mut l, self.phone.now.elapsed(), false);
                return l;
            }
            Field::ToGo | Field::OffCourse | Field::Climb | Field::ClimbGain | Field::UpToGo | Field::NextTurn => {
                return self.course_text(f);
            }
            Field::Gear => {
                match self.live.get(Metric::Gear) {
                    // a 1x system (no front, 0): the rear alone, "11" (sensors.rs)
                    Some(g) => {
                        if g >> 8 != 0 {
                            l.push_u32(u32::from(g >> 8));
                            l.push_str("-");
                        }
                        l.push_u32(u32::from(g & 0xff));
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Shifts => (Some(u32::from(self.shifts)), false),
            Field::ShiftBattery => {
                match self.live.get(Metric::ShiftBattery) {
                    Some(s) if s >= sensors::SHIFT_STATUS => {
                        let status = usize::from(s - sensors::SHIFT_STATUS);
                        l.push_str(["?", "NEW", "GOOD", "OK", "LOW", "CRIT"].get(status).copied().unwrap_or("?"));
                    }
                    Some(p) => {
                        l.push_u32(u32::from(p));
                        l.push_str("%");
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Temp => {
                let t = self.baro.map(|(_, t)| i32::from(t) / 10); // C x 100 to tenths
                if t.is_some_and(|t| t < 0) {
                    l.push_str("-");
                }
                (t.map(i32::unsigned_abs), true)
            }
            Field::Hr => (live(Metric::Hr), false),
            Field::Speed => (live(Metric::Speed), true),
            Field::Cadence => (live(Metric::Cadence), false),
            Field::Power => (live(Metric::Power), false),
            Field::AvgSpeed => (r.avg_speed_x10().map(u32::from), true),
            Field::MaxSpeed => (Some(u32::from(r.max_speed_x10())), true),
            Field::AvgHr => (r.avg_hr().map(u32::from), false),
            Field::AvgPower => (r.avg_power().map(u32::from), false),
            Field::Ascent => (Some(r.ascent_m()), false),
            // "--" once the strap is silent, else 2 decimals
            Field::Dfa => match self.hrv.alpha1().filter(|_| self.live.get(Metric::Hr).is_some()) {
                Some(a) => {
                    let v = (a * 100.0 + 0.5).clamp(0.0, 999.0) as u32;
                    l.push_u32(v / 100);
                    l.push_str(".");
                    l.push_str(ui::DIGITS[(v / 10 % 10) as usize]);
                    l.push_str(ui::DIGITS[(v % 10) as usize]);
                    return l;
                }
                None => (None, false),
            },
            Field::Rmssd => (self.hrv.rmssd().map(u32::from).filter(|_| self.live.get(Metric::Hr).is_some()), false),
            // power.rs and the ride's derived metrics
            Field::Power3s => (p.rolling(0).map(u32::from), false),
            Field::Power10s => (p.rolling(1).map(u32::from), false),
            Field::Power30s => (p.rolling(2).map(u32::from), false),
            Field::Np => (p.np().map(u32::from), false),
            Field::If => {
                match p.if_x100(ftp) {
                    Some(v) => {
                        l.push_u32(v / 100);
                        l.push_str(".");
                        l.push_str(ui::DIGITS[(v / 10 % 10) as usize]);
                        l.push_str(ui::DIGITS[(v % 10) as usize]);
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Tss => (p.tss(ftp), false),
            Field::Kj => (Some(p.kj()), false),
            Field::Kcal => (Some(p.kcal()), false),
            Field::Wkg => (p.rolling(0).map(|w| power::wkg_x10(w, self.settings.weight_kg)), true),
            Field::MaxPower => (p.max().map(u32::from), false),
            Field::MaxHr => (r.max_hr().map(u32::from), false),
            Field::AvgCadence => (r.avg_cadence().map(u32::from), false),
            Field::Balance => {
                match self.live.get(Metric::Balance) {
                    Some(right) => {
                        l.push_u32(u32::from(100u16.saturating_sub(right)));
                        l.push_str("/");
                        l.push_u32(u32::from(right));
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Wbal | Field::Vam | Field::Grade => {
                let v = match f {
                    Field::Wbal => p.wbal_j().map(|j| j / 100), // kJ x 10
                    Field::Vam => r.vam(),
                    _ => r.grade_x10(),
                };
                if v.is_some_and(|v| v < 0) {
                    l.push_str("-");
                }
                (v.map(i32::unsigned_abs), f != Field::Vam)
            }
        };
        match v {
            None => l.push_str("--"),
            Some(v) if tenths => {
                l.push_u32(v / 10);
                l.push_str(".");
                l.push_u32(v % 10);
            }
            Some(v) => l.push_u32(v),
        }
        l
    }

    /// The colour of field `f`'s value and of its box's background: the plain power fields (live, 3, 10, 30 s) and the
    /// live heart rate fill their box with their zone's colour, the value in black or white over it (the owner: the
    /// zone at a glance; zone 1's grey on its own was nearly white); the rest on black ([`App::field_color`]).
    pub(crate) fn field_ink(&self, f: Field) -> (u16, u16) {
        let zone = match f {
            // the live power's box takes the 3 s average's zone: second-to-second power crosses a zone's edge all
            // the time, and each change fills the whole box again (18,000 px/s on the sim's noisy power, against
            // 6,500 without the colour); its digits stay live
            Field::Power if self.live.get(Metric::Power).is_some() => Some(Field::Power3s),
            Field::Power3s | Field::Power10s | Field::Power30s | Field::Hr => Some(f),
            _ => None,
        };
        match zone.map(|z| self.field_color(z)) {
            Some(bg) if bg != ui::WHITE => (ui::ink_on(bg), bg),
            _ => (self.field_color(f), ui::BLACK),
        }
    }

    /// The colour of ride field `f`'s value: heart rate and power (live, rolling, NP, max) in their zone's colour,
    /// W' balance as it drains ([`power::wbal_color`]), the rest white.
    pub(crate) fn field_color(&self, f: Field) -> u16 {
        if f == Field::StepTarget {
            return self.target_color();
        }
        if f == Field::WktPower {
            return self.gauge_now(0).color;
        }
        if let Some(c) = self.course_color(f) {
            return c;
        }
        // a shifting battery low (20 % or less, or the ANT+ status low or critical): red, to charge before the ride
        let low = |s: u16| if s >= sensors::SHIFT_STATUS { s >= sensors::SHIFT_STATUS + 4 } else { s <= 20 };
        if f == Field::ShiftBattery && self.live.get(Metric::ShiftBattery).is_some_and(low) {
            return ui::RED;
        }
        let p = &self.power;
        let power = match f {
            Field::Power => self.live.get(Metric::Power),
            Field::Power3s => p.rolling(0),
            Field::Power10s => p.rolling(1),
            Field::Power30s => p.rolling(2),
            Field::Np => p.np(),
            Field::MaxPower => p.max(),
            _ => None,
        };
        let hr = match f {
            Field::Hr => self.live.get(Metric::Hr),
            Field::MaxHr => self.ride.max_hr(),
            _ => None,
        };
        match (power, hr, p.wbal_j()) {
            (Some(w), _, _) => ride::power_color(ride::power_zone(w, self.settings.ftp)),
            (_, Some(v), _) => ride::hr_color(ride::hr_zone(v, self.settings.max_hr)),
            (_, _, Some(j)) if f == Field::Wbal => power::wbal_color(j, self.settings.wprime_j),
            _ => ui::WHITE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let d = |s, pad| {
            let mut l = Line::<8>::new();
            push_duration(&mut l, s, pad);
            l
        };
        assert_eq!([d(0, true).as_str(), d(65, true).as_str(), d(65, false).as_str()], ["00:00", "01:05", "1:05"]);
        assert_eq!(
            [d(3599, true).as_str(), d(3600, true).as_str(), d(45_296, false).as_str()],
            ["59:59", "1:00|00", "12:34|56"]
        );
    }
}
