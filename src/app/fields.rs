//! A ride field's text and colour ([`App::field_text`], [`App::field_color`]), and the fixed-point and time
//! formatting they use: integers into a stack [`Line`], no floats (float formatting costs a few KB of image).

use crate::*;

/// Cadence under this, rpm, in ERG turns the cadence box's edge bar red.
pub(crate) const ERG_LOW_RPM: u16 = 60;

/// `v / 10^shift` with `decimals` decimals (truncated): `push_fixed(l, 51234561, 7, 5)` gives "5.12345".
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

/// Hours and minutes as "09:41".
pub(crate) fn hm_text((h, m): (u8, u8)) -> Line<6> {
    let mut l = Line::new();
    for d in [h / 10, h % 10, 10, m / 10, m % 10] {
        l.push_str(if d == 10 { ":" } else { ui::DIGITS[usize::from(d % 10)] });
    }
    l
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

/// Speed from which the course over ground is shown as a heading (mm/s): 5 km/h, below it the heading is noise.
const HEADING_MIN_MM_S: u32 = 1_389;

/// The compass points, N first (a heading's point is [`point_of`]).
pub(crate) const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

/// A heading (degrees x 1e-5) as one of 8 compass points, N first; each point covers 45 degrees centred on it, so
/// 359.9 is N and 22.5 is NE.
pub(crate) fn point_of(h: i32) -> u8 {
    (((h.rem_euclid(36_000_000) + 2_250_000) / 4_500_000) % 8) as u8
}

/// The point shown for heading `h`, given the point on the screen: it stays until the heading is more than 30 degrees
/// from its centre, so a heading near a sector's edge does not flicker between two points.
pub(crate) fn held_point(h: i32, shown: Option<u8>) -> u8 {
    match shown {
        Some(i) => {
            let d = (h.rem_euclid(36_000_000) - i32::from(i) * 4_500_000).rem_euclid(36_000_000);
            if d.min(36_000_000 - d) <= 3_000_000 { i } else { point_of(h) }
        }
        None => point_of(h),
    }
}

/// Minutes from `now` to the sunset, which is unwrapped (see [`crate::sun::rise_set`]); None once it has set.
pub(crate) fn minutes_to_sunset(now: u32, sun: Option<(u32, u32)>) -> Option<u32> {
    sun.map(|(_, s)| s).filter(|&s| s >= now).map(|s| s - now)
}

/// Seconds to the course's end at the average moving speed (km/h x 10), None under 5 km/h: a slow ride's average
/// says little about the next hour.
pub(crate) fn time_to_dest_s(left_m: u32, avg_x10: Option<u16>) -> Option<u32> {
    let v = avg_x10.filter(|&v| v >= 50)?;
    Some((u64::from(left_m) * 36 / u64::from(v)).min(u64::from(u32::MAX)) as u32)
}

/// Minutes of the day as "HH:MM" (the sun's times, the arrival).
pub(crate) fn push_day_time<const N: usize>(l: &mut Line<N>, mins: u32) {
    let (h, m) = (mins / 60 % 24, mins % 60);
    for d in [h / 10, h % 10, 10, m / 10, m % 10] {
        l.push_str(if d == 10 { ":" } else { ui::DIGITS[(d % 10) as usize] });
    }
}

/// `v` as a percent of `of` (None for 0), at most 999: a 4-digit value would not fit its box.
fn pct_of(v: u16, of: u16) -> Option<u32> {
    (of > 0).then(|| (u32::from(v) * 100 / u32::from(of)).min(999))
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
    /// The big power value now: the live reading, or the average of the POWER SMOOTH window (1 s is the reading).
    pub(crate) fn power_now(&self) -> Option<u16> {
        match POWER_SMOOTH_S.get(self.settings.power_smooth).copied().unwrap_or(1) {
            0 | 1 => self.live.shown(Metric::Power),
            s => self.power.average(s as usize),
        }
    }

    /// The zone the bar of the held zone `i` (0 power, 4 heart rate, see [`App::field_bar`]) shows now, None for none.
    pub(crate) fn zone_now(&self, i: usize) -> Option<u8> {
        self.zones.get(i).map(ride::ZoneHold::shown).filter(|&z| z > 0)
    }

    /// The sun's rise and set for the minute, worked out once a minute (not per pass). Needs a fix and a learnt zone
    /// (the RTC is local time: the offset comes from the phone or the fix); without them nothing is kept, so the first
    /// fix shows its times at once. In polar day or night the None is kept for the minute.
    pub(crate) fn refresh_sun(&mut self) {
        let Some((p, o)) = self.gps.pvt.filter(|p| p.good()).zip(self.gps.aid.offset_15min) else {
            self.sun = None;
            return;
        };
        let key = (self.date, self.strip.time);
        if self.sun.is_some_and(|(k, _)| k == key) {
            return;
        }
        let (y, m, d) = self.date;
        let times = crate::sun::rise_set(p.lat, p.lon, days_from_civil(y, m, d), i32::from(o) * 15);
        self.sun = Some((key, times));
    }

    /// The heading's point on the screen (held, see [`held_point`]): None without a fix or under 5 km/h.
    pub(crate) fn refresh_heading(&mut self) {
        let fresh = self.gps.pvt.filter(|p| p.good() && p.speed_mm_s >= HEADING_MIN_MM_S);
        self.heading = fresh.map(|p| held_point(p.heading, self.heading));
    }

    /// The sun's (rise, set) in local minutes of the day, as [`App::refresh_sun`] kept them.
    pub(crate) fn sun_times(&self) -> Option<(u32, u32)> {
        self.sun.and_then(|(_, t)| t)
    }

    /// The text of ride field `f` now ("--" without a value).
    pub(crate) fn field_text(&self, f: Field) -> Line<8> {
        let mut l = Line::new();
        let (r, live) = (&self.ride, |m| self.live.shown(m).map(u32::from)); // one reading a second
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
            Field::Timer | Field::LapTime | Field::TripTime => {
                let s = match f {
                    Field::Timer => r.timer_s(),
                    Field::TripTime => r.elapsed_s(),
                    _ => r.lap_s(),
                };
                push_duration(&mut l, s, true);
                return l;
            }
            Field::LastLap => {
                match r.last_lap_s() {
                    Some(s) => push_duration(&mut l, s, true),
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::UphillKm | Field::DownhillKm => {
                push_km(&mut l, if f == Field::UphillKm { r.uphill_m() } else { r.downhill_m() });
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
            | Field::ClimbProfile
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
            Field::Workout | Field::StepTarget | Field::StepTime | Field::NextStep | Field::WorkoutLeft => {
                return self.workout_text(f);
            }
            Field::WktPower => return self.gauge_now(0).value,
            Field::ClimbPace => return self.pace_now(0).value,
            Field::TrainerMode | Field::TrainerTarget => return self.trainer_field(f),
            Field::ErgTarget => return self.erg_field(),
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
            Field::Heading => {
                match self.heading.and_then(|i| POINTS.get(usize::from(i))) {
                    Some(point) => l.push_str(point),
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Sunrise | Field::Sunset | Field::ToSunset => {
                let now = u32::from(self.strip.time.0) * 60 + u32::from(self.strip.time.1);
                match (f, self.sun_times()) {
                    (Field::Sunrise, Some((r, _))) => push_day_time(&mut l, r),
                    (Field::Sunset, Some((_, s))) => push_day_time(&mut l, s),
                    (Field::ToSunset, sun) => match minutes_to_sunset(now, sun) {
                        Some(m) => push_hm(&mut l, m * 60),
                        None => l.push_str("--"),
                    },
                    _ => l.push_str("--"),
                }
                return l;
            }
            Field::TimeToDest | Field::Eta => {
                let left = self.on_course().map(|(c, at)| c.len_m().saturating_sub(at.along_m));
                match (f, left.and_then(|m| time_to_dest_s(m, r.avg_speed_x10()))) {
                    (Field::TimeToDest, Some(s)) => push_hm(&mut l, s),
                    (Field::Eta, Some(s)) => {
                        let now = u32::from(self.strip.time.0) * 60 + u32::from(self.strip.time.1);
                        push_day_time(&mut l, (now * 60 + s) / 60);
                    }
                    _ => l.push_str("--"),
                }
                return l;
            }
            Field::UnitBatt => {
                match self.strip.battery {
                    Some(p) => {
                        l.push_u32(u32::from(p));
                        l.push_str("%");
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::PowerBatt | Field::HrBatt => {
                let kind = if f == Field::PowerBatt { sensors::Kind::Power } else { sensors::Kind::Hr };
                match self.decoders.battery_status(kind) {
                    Some(s) => l.push_str(
                        ["?", "NEW", "GOOD", "OK", "LOW", "CRIT"].get(usize::from(s)).copied().unwrap_or("?"),
                    ),
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::Shifts => (Some(u32::from(self.radio.shifts)), false),
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
            Field::Power => (self.power_now().map(u32::from), false),
            Field::PctMaxHr => (self.live.shown(Metric::Hr).and_then(|b| pct_of(b, self.settings.max_hr)), false),
            Field::PctLthr => (self.live.shown(Metric::Hr).and_then(|b| pct_of(b, self.settings.lthr)), false),
            // the zones the bars show (the held zone, [`App::zone_now`]), so the number, the colour and the time agree
            Field::HrZone => (self.zone_now(4).map(u32::from), false),
            Field::PowerZone => (self.zone_now(0).map(u32::from), false),
            Field::PctFtp => (p.rolling(0).and_then(|w| pct_of(w, ftp)), false),
            Field::PctFtpAvg => (r.avg_power().and_then(|w| pct_of(w, ftp)), false),
            Field::PctFtpLap => (r.lap_avg_power().and_then(|w| pct_of(w, ftp)), false),
            Field::PctFtp10s => (p.rolling(1).and_then(|w| pct_of(w, ftp)), false),
            Field::PctFtp30s => (p.rolling(2).and_then(|w| pct_of(w, ftp)), false),
            Field::HrZoneTime | Field::PowerZoneTime => {
                let secs = if f == Field::HrZoneTime {
                    self.zone_now(4).and_then(|z| self.zone_s.hr_s(z))
                } else {
                    self.zone_now(0).and_then(|z| self.zone_s.power_s(z))
                };
                match secs {
                    Some(s) => push_duration(&mut l, s, true),
                    None => l.push_str("--"),
                }
                return l;
            }
            Field::AvgSpeed => (r.avg_speed_x10().map(u32::from), true),
            Field::MaxSpeed => (Some(u32::from(r.max_speed_x10())), true),
            Field::AvgHr => (r.avg_hr().map(u32::from), false),
            Field::AvgPower => (r.avg_power().map(u32::from), false),
            Field::Ascent => (Some(r.ascent_m()), false),
            Field::Descent => (Some(r.descent_m()), false),
            Field::LapAscent => (Some(r.lap_ascent_m()), false),
            Field::LapDescent => (Some(r.lap_descent_m()), false),
            Field::Laps => (Some(r.laps()), false),
            Field::LapAvgPower => (r.lap_avg_power().map(u32::from), false),
            Field::LastLapAvgPower => (r.last_lap_avg_power().map(u32::from), false),
            Field::LapNp => (r.lap_np().map(u32::from), false),
            Field::MaxCadence => (r.max_cadence().map(u32::from), false),
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
            // the three show the window a tap chose (settings.wkt_avg)
            Field::Power3s | Field::Power10s | Field::Power30s => {
                (p.rolling(self.settings.wkt_avg).map(u32::from), false)
            }
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
            Field::BalanceAvg => {
                match r.balance_right() {
                    Some(right) => {
                        l.push_u32(u32::from(100u16.saturating_sub(right)));
                        l.push_str("/");
                        l.push_u32(u32::from(right));
                    }
                    None => l.push_str("--"),
                }
                return l;
            }
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
            Field::Wbal | Field::Vam | Field::Grade | Field::MaxAlt => {
                let v = match f {
                    Field::Wbal => p.wbal_j().map(|j| j / 100), // kJ x 10
                    Field::Vam => r.vam(),
                    Field::MaxAlt => r.max_alt_m(),
                    _ => r.grade_x10(),
                };
                if v.is_some_and(|v| v < 0) {
                    l.push_str("-");
                }
                (v.map(i32::unsigned_abs), !matches!(f, Field::Vam | Field::MaxAlt))
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

    /// The edge bar of field `f` along the box's top, BLACK for none. One rule: the digits are white and the state is
    /// the bar's: the power fields (live, 3, 10, 30 s, NP, max) and the heart rate (live, max) in their zone, W' balance as
    /// it drains, the step target by how the reading sits against the band, CLIMB by its grade. Each bar zones exactly
    /// the number in its box, with a margin on the value against flicker at a zone's edge ([`ride::ZoneHold`]).
    pub(crate) fn field_bar(&self, f: Field) -> u16 {
        let zone = |z: u8, power: bool| match (z, power) {
            (0, _) => ui::BLACK,
            (z, true) => ride::power_color(z),
            (z, false) => ride::hr_color(z),
        };
        let held = |i: usize| self.zones.get(i).map_or(0, ride::ZoneHold::shown);
        let (p, ftp) = (&self.power, self.settings.ftp);
        match f {
            Field::Power => zone(held(0), true),
            Field::Power3s | Field::Power10s | Field::Power30s => zone(held(1 + self.settings.wkt_avg), true),
            Field::Hr => zone(held(4), false),
            Field::Np => p.np().map_or(ui::BLACK, |w| ride::power_zone_color(w, ftp)),
            Field::MaxPower => p.max().map_or(ui::BLACK, |w| ride::power_zone_color(w, ftp)),
            Field::MaxHr => self.ride.max_hr().map_or(ui::BLACK, |v| ride::hr_zone_color(v, self.settings.max_hr)),
            Field::Wbal => p.wbal_j().map_or(ui::BLACK, |j| power::wbal_color(j, self.cp_model().1)),
            // in ERG the trainer sets the watts: what the rider controls is cadence, and under 60 rpm the resistance
            // climbs (an alarm, so red)
            Field::Cadence
                if self.ride.state() != ride::State::Idle
                    && self.erg_on()
                    && self.live.get(Metric::Cadence).is_some_and(|c| c < ERG_LOW_RPM) =>
            {
                ui::RED
            }
            Field::StepTarget => self.target_bar(),
            Field::Climb => self.course_bar(f),
            _ => ui::BLACK,
        }
    }

    /// The colour of ride field `f`'s value: white, except the alarms in red (a shifting battery low, off course): the
    /// state is the edge bar's ([`App::field_bar`]).
    pub(crate) fn field_color(&self, f: Field) -> u16 {
        // a shifting battery low (20 % or less, or the ANT+ status low or critical): red, to charge before the ride
        let low = |s: u16| if s >= sensors::SHIFT_STATUS { s >= sensors::SHIFT_STATUS + 4 } else { s <= 20 };
        if f == Field::ShiftBattery && self.live.get(Metric::ShiftBattery).is_some_and(low) {
            return ui::RED;
        }
        self.course_color(f).unwrap_or(ui::WHITE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_field_ids_keep_their_fields() {
        // a page keeps each field as its index in ALL: the ids of the fields before this build's stay put
        assert_eq!(
            [Field::from_index(0), Field::from_index(14), Field::from_index(65)],
            [Field::Time, Field::Ascent, Field::ClimbProfile]
        );
        assert_eq!(Field::from_index(66), Field::TripTime);
        assert_eq!([Field::from_index(79), Field::from_index(90)], [Field::PctMaxHr, Field::BalanceAvg]);
        assert_eq!([Field::from_index(91), Field::from_index(99)], [Field::Heading, Field::HrBatt]);
        assert_eq!(Field::ALL.len(), 100);
    }

    #[test]
    fn heading_time_and_clock_of_the_sun() {
        // the compass wraps at 359.9 degrees, the 45-degree sectors centred on their points
        assert_eq!(
            [35_999_999, 0, 2_249_999, 2_250_000, -1].map(|h| POINTS[usize::from(point_of(h))]),
            ["N", "N", "N", "NE", "N"]
        );
        // a heading swinging 22 to 24 degrees keeps N; 31 degrees off its centre it moves on to NE
        let mut shown = None;
        for h in [0, 2_200_000, 2_400_000, 2_300_000, 2_200_000, 2_400_000] {
            shown = Some(held_point(h, shown));
            assert_eq!(shown, Some(0), "held at {h}");
        }
        assert_eq!(held_point(3_100_000, shown), 1);
        // no course speed: "--" (None); under 5 km/h average: None; else distance / speed
        assert_eq!(
            [time_to_dest_s(10_000, None), time_to_dest_s(10_000, Some(49)), time_to_dest_s(10_000, Some(200))],
            [None, None, Some(1800)]
        );
        // the clock of arrival wraps past midnight
        let mut l = Line::<8>::new();
        push_day_time(&mut l, (23 * 60 + 50 + 30) % 1440);
        assert_eq!(l.as_str(), "00:20");
        // Reykjavik at 23:00 on the June solstice: the sunset is 00:04 the next day (1444), so 1:04 left
        assert_eq!(minutes_to_sunset(23 * 60, Some((175, 1444))), Some(64));
        assert_eq!(minutes_to_sunset(23 * 60, Some((100, 1200))), None, "set for the day");
        assert_eq!(minutes_to_sunset(23 * 60, None), None);
    }

    #[test]
    fn percents_of_a_value() {
        // a floor, 0 for no value (no FTP, no LTHR), and the 999 cap of a box
        assert_eq!(
            [pct_of(151, 200), pct_of(150, 200), pct_of(1000, 100), pct_of(5, 0)],
            [Some(75), Some(75), Some(999), None]
        );
    }

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
