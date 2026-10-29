//! Ride pages: a layout of 1 to 6 fields between the strip and the bottom of the panel.
//! Field boxes, values, layouts, frames, zone bars and the graph fields' axes, stats and columns.

use super::*;

/// What a ride page field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Time of day, HH:MM.
    Time,
    /// Heart rate.
    Hr,
    /// Speed, km/h with one decimal.
    Speed,
    /// Cadence.
    Cadence,
    /// Power.
    Power,
    /// The map of the ride (map.rs): its box holds the map instead of a value.
    Map,
    /// The ride's moving time, h:mm:ss.
    Timer,
    /// The ride's distance, km (2 decimals under 100 km, then 1).
    Dist,
    /// Average moving speed, km/h.
    AvgSpeed,
    /// Top speed, km/h.
    MaxSpeed,
    /// Average heart rate.
    AvgHr,
    /// Average power.
    AvgPower,
    /// This lap's moving time.
    LapTime,
    /// This lap's distance, km.
    LapDist,
    /// Altitude gained, m (GPS).
    Ascent,
    /// The last minutes of power, coloured by zone.
    Graph,
    /// The barometer's temperature, C.
    Temp,
    /// The last minutes of heart rate, coloured by zone.
    HrGraph,
    /// DFA alpha1 of the last 2 minutes of beats (hrv.rs): about 0.75 at the aerobic threshold.
    Dfa,
    /// RMSSD of the last minute of beats, ms (hrv.rs).
    Rmssd,
    // power.rs and the ride's derived metrics
    /// 3 s average power.
    Power3s,
    /// 10 s average power.
    Power10s,
    /// 30 s average power.
    Power30s,
    /// Normalized power.
    Np,
    /// Intensity factor (NP / FTP).
    If,
    /// Training stress score.
    Tss,
    /// Work, kJ.
    Kj,
    /// Energy, kcal.
    Kcal,
    /// 3 s power per kg.
    Wkg,
    /// W' balance, kJ.
    Wbal,
    /// The ride's max power.
    MaxPower,
    /// The ride's max heart rate.
    MaxHr,
    /// Average cadence (zeros left out).
    AvgCadence,
    /// Left / right pedal balance, percent.
    Balance,
    /// VAM, m/h.
    Vam,
    /// Grade, %.
    Grade,
    // the graphs of the other plots (ride::Plot): a tap on a graph goes to the next one
    /// The last minutes (or the ride) of W' balance.
    WbalGraph,
    /// Of DFA alpha1.
    DfaGraph,
    /// Of altitude: the elevation profile.
    AltGraph,
    // totals of the rides on the card (stock's and, once saved, this firmware's) and the ride running
    /// This week's distance, km (from Monday).
    WeekKm,
    /// This week's riding time, h.
    WeekH,
    /// This year's distance, km.
    YearKm,
    /// Every ride's distance, km (the odometer).
    TotalKm,
    // the workout player (workout.rs)
    /// During a workout, the step running of all ("3/14"); otherwise today's planned workout, h:mm.
    Workout,
    /// The step's target range, green in it, orange under, red over (3 s power, heart rate, cadence).
    StepTarget,
    /// What is left of the step: time, km, or LAP.
    StepTime,
    /// The next step's target (its middle) and its length as a small tail ("150|0:30"; END on the last step).
    NextStep,
    // the course followed (course.rs)
    /// What is left of the course, km.
    ToGo,
    /// How far the course is, m (red once off course was said).
    OffCourse,
    /// The climb the rider is on (what is left, and the grade of the next 200 m: "0.8K 9%") or the next one (its
    /// length and average grade: "1.2K 6%"), coloured by grade.
    Climb,
    /// The climb's gain left, m (on it), or the next climb's gain.
    ClimbGain,
    /// The course's ascent left, m.
    UpToGo,
    /// The course's profile from about where the rider is to its end: the rider's place, the ridden part dark, the
    /// climbs coloured by grade.
    Profile,
    /// The workout's steps over its whole length, each as high as its target and in its zone colour; the rider's
    /// place, the part done dark.
    WorkoutGraph,
    /// During a workout, the reading the step targets (power over the window chosen in the menu, or heart rate or
    /// cadence) big, a gauge around the target band with a marker for the reading, and the band in numbers;
    /// otherwise the power alone.
    WktPower,
    /// What plays on the phone (AMS): the elapsed time of the track, m:ss (its title is on the media screen and in the
    /// banner when it changes).
    Music,
    // electronic shifting (sensors.rs)
    /// The gear, front-rear ("2-11"), from 1.
    Gear,
    /// Gear changes in this ride.
    Shifts,
    /// The shifting system's battery: percent (Di2), or its status (ANT+: NEW, GOOD, OK, LOW, CRIT).
    ShiftBattery,
    /// The course's next cue: its arrow and how far ("350M", "1.2K"); "--" off course (course/cues.rs).
    NextTurn,
    // the trainer (trainer.rs)
    /// The trainer's mode in force: ERG, RES or SIM.
    TrainerMode,
    /// Its target: watts (ERG), percent (RES) or grade (SIM).
    TrainerTarget,
    /// What the trainer is told now in ERG, watts, the rider's adjust included ("225|+15"); "--" outside ERG or without
    /// a trainer (the plan's band is [`Field::StepTarget`]).
    ErgTarget,
    /// The time left in the whole workout (the steps after the one running, and what is left of it).
    WorkoutLeft,
    /// On a climb of the followed course (or within 5 km of one), the power to ride now for the fastest climb with the
    /// W' left ([`crate::pacing`]), in the workout gauge: the 3 s power against a band around the target; "TOP" and the
    /// time to the top in its title.
    ClimbPace,
    /// The climb the rider is on (else the next), from its start to its top: each column coloured by its grade,
    /// the rider's place white, the climb's gain, length and average grade over it (the CLIMB page).
    ClimbProfile,
    // the ride's totals, laps and slopes (ride.rs). Appended: a page keeps a field's index in ALL. None of these is
    // kept over a resume (the recovery record has no room for them): they restart from zero then.
    /// The ride's elapsed time, h:mm:ss: auto-pauses count, a Start pause does not.
    TripTime,
    /// Laps taken.
    Laps,
    /// The last lap's moving time.
    LastLap,
    /// This lap's average power, W.
    LapAvgPower,
    /// The last lap's average power, W.
    LastLapAvgPower,
    /// This lap's normalized power, W.
    LapNp,
    /// The top cadence, rpm.
    MaxCadence,
    /// The highest altitude, m.
    MaxAlt,
    /// Altitude lost, m (the ascent's hysteresis).
    Descent,
    /// This lap's ascent, m.
    LapAscent,
    /// This lap's descent, m.
    LapDescent,
    /// Distance ridden uphill (grade of +1 % or more), km.
    UphillKm,
    /// Distance ridden downhill (grade of -1 % or less), km.
    DownhillKm,
}

impl Field {
    /// Every field, in the order the page menu cycles through them; the index is kept in a byte
    /// ([`crate::PageCfg`]), so new ones go at the end.
    pub const ALL: [Field; 79] = {
        use Field::*;
        [
            Time,
            Hr,
            Speed,
            Cadence,
            Power,
            Map,
            Timer,
            Dist,
            AvgSpeed,
            MaxSpeed,
            AvgHr,
            AvgPower,
            LapTime,
            LapDist,
            Ascent,
            Graph,
            Temp,
            HrGraph,
            Dfa,
            Rmssd, // power.rs and the ride's derived metrics:
            Power3s,
            Power10s,
            Power30s,
            Np,
            If,
            Tss,
            Kj,
            Kcal,
            Wkg,
            Wbal,
            MaxPower,
            MaxHr,
            AvgCadence,
            Balance,
            Vam,
            Grade,
            WbalGraph,
            DfaGraph,
            AltGraph,
            WeekKm,
            WeekH,
            YearKm,
            TotalKm, // the workout player:
            Workout,
            StepTarget,
            StepTime,
            NextStep, // the course followed:
            ToGo,
            OffCourse,
            Climb,
            ClimbGain,
            UpToGo,
            Profile, // the workout player again:
            WorkoutGraph,
            WktPower, // the phone:
            Music,    // electronic shifting:
            Gear,
            Shifts,
            ShiftBattery, // the course followed again:
            NextTurn,     // the trainer:
            TrainerMode,
            TrainerTarget,
            ErgTarget,
            WorkoutLeft,
            ClimbPace,
            ClimbProfile,
            TripTime,
            Laps,
            LastLap,
            LapAvgPower,
            LastLapAvgPower,
            LapNp,
            MaxCadence,
            MaxAlt,
            Descent,
            LapAscent,
            LapDescent,
            UphillKm,
            DownhillKm,
        ]
    };

    /// Its label, top left of its box.
    pub fn label(self) -> &'static str {
        const LABELS: [&str; Field::ALL.len()] = [
            "TIME",
            "HR",
            "KM/H",
            "RPM",
            "WATTS",
            "MAP",
            "TIMER",
            "KM",
            "AVG KM/H",
            "MAX KM/H",
            "AVG HR",
            "AVG W",
            "LAP TIME",
            "LAP KM",
            "ASCENT M",
            "W GRAPH",
            "TEMP C",
            "HR GRAPH",
            "DFA A1",
            "RMSSD",
            // power.rs and the ride's derived metrics
            "3S W",
            "10S W",
            "30S W",
            "NP",
            "IF",
            "TSS",
            "KJ",
            "KCAL",
            "W/KG",
            "W'BAL KJ",
            "MAX W",
            "MAX HR",
            "AVG RPM",
            "L/R",
            "VAM M/H",
            "GRADE %",
            "W' GRAPH",
            "DFA GRAPH",
            "ALT GRAPH",
            "WEEK KM",
            "WEEK H",
            "YEAR KM",
            "TOTAL KM",
            // the workout player
            "WORKOUT",
            "TARGET",
            "STEP LEFT",
            "NEXT",
            // the course followed
            "TO GO KM",
            "OFF CRS M",
            "CLIMB",
            "CLIMB M",
            "UP LEFT M",
            "PROFILE",
            "WKT GRAPH",
            "WKT POWER",
            "MUSIC",
            // electronic shifting
            "GEAR",
            "SHIFTS",
            "SHIFT BAT",
            "NEXT TURN",
            // the trainer
            "MODE",
            "TRG",
            "ERG W",
            "WKT LEFT",
            "TOP",
            "CLIMB",
            "TRIP TIME",
            "LAPS",
            "LAST LAP",
            "LAP AVG W",
            "LAST W",
            "LAP NP",
            "MAX RPM",
            "MAX ALT M",
            "DESCENT M",
            "LAP ASC M",
            "LAP DSC M",
            "UP KM",
            "DOWN KM",
        ];
        LABELS[self as usize]
    }

    /// The widest value it shows (a time: without its small seconds): the tests draw it in every box of every layout
    /// (the scale follows the value, not this; one longer than [`VALUE_CHARS`] gets a smaller one).
    #[cfg(test)]
    pub(super) fn widest(self) -> &'static str {
        const WIDEST: [&str; Field::ALL.len()] = [
            "88:88", "888", "88.8", "888", "8888", "", "88:88", "88.88", "88.8", "88.8", "888", "8888", "88:88",
            "88.88", "8888", "", "-88.8", "", "8.88", "888", // power.rs and the ride's derived metrics:
            "8888", "8888", "8888", "8888", "8.88", "888", "8888", "8888", "88.8", "-88.8", "8888", "888", "888",
            "88/88", "-8888", "-88.8", "", "", "", "8888", "888.8", "88888", "888888", // the workout player:
            "88/88", "888-888", "88:88", "888-888", // the course followed:
            "888.8", "8888", "8.8K 88%", "8888", "8888", "", "", "8888",  // the phone:
            "88:88", // electronic shifting:
            "8-88", "8888", "GOOD",   // the course followed again:
            "888.8K", // the trainer:
            "ERG", "-10.0 %", // the widest target: a grade (a power is "2000 W")
            "888|+88", "88:88|88", "", "", // the workout and ERG again, the climb pacer, the climb profile
            "88:88", "888", "88:88", "888", "888", "888", "888", "8888", "8888", "8888", "8888", "88.88", "88.88",
        ];
        WIDEST[self as usize]
    }

    /// Its box holds a picture (map, graph), not a value.
    pub fn is_picture(self) -> bool {
        matches!(
            self,
            Field::Map
                | Field::Graph
                | Field::HrGraph
                | Field::WbalGraph
                | Field::DfaGraph
                | Field::AltGraph
                | Field::Profile
                | Field::WorkoutGraph
                | Field::WktPower
                | Field::ClimbPace
                | Field::ClimbProfile
        )
    }

    /// A field of the workout player (the step, its target, what is left, the next step, the graph, the gauge).
    pub fn is_workout(self) -> bool {
        matches!(
            self,
            Field::Workout
                | Field::StepTarget
                | Field::StepTime
                | Field::NextStep
                | Field::WorkoutLeft
                | Field::WorkoutGraph
                | Field::WktPower
        )
    }

    /// Its group in [`FIELD_GROUPS`] (every field is in one: a test checks).
    pub fn group(self) -> usize {
        FIELD_GROUPS.iter().position(|(_, g)| g.contains(&self)).unwrap_or(0)
    }

    /// From its index in [`Field::ALL`] (anything else: time).
    pub fn from_index(i: u32) -> Field {
        Self::ALL.get(i as usize).copied().unwrap_or(Field::Time)
    }
}

/// The fields by kind, for the picker: a page's box is a group then a field, a few rows each, not 55 in a row.
pub const FIELD_GROUPS: [(&str, &[Field]); 11] = {
    use Field::*;
    [
        (
            "RIDE",
            &[
                Time, Timer, TripTime, Laps, LastLap, Speed, Dist, AvgSpeed, MaxSpeed, MaxCadence, Cadence, AvgCadence,
                LapTime, LapDist, Kcal, Temp, Map,
            ],
        ),
        (
            "POWER",
            &[
                Power,
                Power3s,
                Power10s,
                Power30s,
                AvgPower,
                MaxPower,
                Np,
                If,
                Tss,
                Kj,
                Wkg,
                Wbal,
                Balance,
                LapAvgPower,
                LastLapAvgPower,
                LapNp,
            ],
        ),
        ("HEART", &[Hr, AvgHr, MaxHr, Dfa, Rmssd]),
        ("CLIMB", &[Ascent, Vam, Grade, MaxAlt, Descent, LapAscent, LapDescent, UphillKm, DownhillKm]),
        ("GRAPHS", &[Graph, HrGraph, WbalGraph, DfaGraph, AltGraph]),
        ("WORKOUT", &[Workout, StepTarget, StepTime, WorkoutLeft, NextStep, WorkoutGraph, WktPower]),
        ("COURSE", &[ToGo, NextTurn, OffCourse, Climb, ClimbGain, UpToGo, Profile, ClimbPace, ClimbProfile]),
        ("TOTALS", &[WeekKm, WeekH, YearKm, TotalKm]),
        ("PHONE", &[Music]),
        ("SHIFTING", &[Gear, Shifts, ShiftBattery]),
        ("TRAINER", &[TrainerMode, TrainerTarget, ErgTarget]),
    ]
};

/// A page layout: rows top to bottom, each (its share of the height, boxes side by side), and its name in the menu.
pub type Layout = (&'static [(u8, u8)], &'static str);

/// The layouts by the id a page keeps ([`crate::PageCfg::layout`]); 0 is a page switched off. Ids 1, 2, 3, 4 and 6
/// are the layouts of earlier builds, whose id was their field count, so pages kept then read the same.
pub const LAYOUTS: [Layout; 16] = [
    (&[], "OFF"),
    (&[(1, 1)], "1"),
    (&[(1, 1), (1, 1)], "2"),
    (&[(1, 1), (1, 2)], "1+2"),
    (&[(1, 2), (1, 2)], "2X2"),
    (&[(2, 1), (1, 2), (1, 2)], "1+2X2"),
    (&[(1, 2), (1, 2), (1, 2)], "2X3"),
    (&[(1, 2), (1, 1)], "2+1"),
    (&[(1, 1), (1, 1), (1, 1)], "3 ROWS"),
    (&[(2, 1), (1, 1), (1, 1)], "BIG+2"),
    (&[(2, 1), (1, 1), (1, 2)], "BIG+1+2"),
    (&[(1, 2), (1, 1), (1, 2)], "2+1+2"),
    (&[(2, 1), (1, 1), (1, 2), (1, 2)], "BIG+1+2X2"),
    // ids 13 and 14 are this build's (stock's pages of 7 and 8 fields); earlier builds refuse them, so the backup
    // registers never keep them (crate::page::Page::to_cfg)
    (&[(1, 1), (1, 2), (1, 2), (1, 2)], "1+2X3"),
    (&[(1, 2), (1, 2), (1, 2), (1, 2)], "2X4"),
    // id 15: the CLIMB page's (not in the menu's order, never stored): the profile 60 %, two values under it
    (&[(3, 1), (2, 2)], "CLIMB"),
];

/// The order the page menu goes through the layouts: by field count (none is off: a page is deleted instead).
pub const LAYOUT_ORDER: [u8; 14] = [1, 2, 8, 3, 7, 9, 4, 10, 5, 11, 6, 12, 13, 14];

/// Layout `id`, if there is one.
pub fn layout(id: u8) -> Option<Layout> {
    LAYOUTS.get(usize::from(id)).copied()
}

/// How many fields layout `id` shows.
///
/// ```
/// use bryton_rustfw::ui::field_count;
/// assert_eq!([0, 1, 4, 5, 6, 10, 99].map(field_count), [0, 1, 4, 5, 6, 4, 0]);
/// ```
pub fn field_count(id: u8) -> usize {
    layout(id).map_or(0, |(rows, _)| rows.iter().map(|&(_, n)| usize::from(n)).sum())
}

/// Top of the ride fields.
pub const RIDE_Y: u32 = PAGE_Y;
/// Height of the ride fields: everything under the strip.
pub const RIDE_H: u32 = H - RIDE_Y;

/// Field box `i` (x, y, w, h) of layout `id` in a field area `h` high ([`rows_box`]). None past the last.
///
/// ```
/// use bryton_rustfw::ui::{field_box, RIDE_Y};
/// assert_eq!(field_box(5, 300, 0), Some((0, RIDE_Y, 240, 150)), "big over 2 x 2: the big box");
/// assert_eq!(field_box(5, 300, 4), Some((120, RIDE_Y + 225, 120, 75)));
/// assert_eq!(field_box(5, 300, 5), None);
/// ```
pub fn field_box(id: u8, h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    rows_box(layout(id)?.0, h, i)
}

/// Box `i` of rows `rows` (top to bottom, each its share of the height and its boxes side by side) in a field area
/// `h` high, row by row, left to right; the last row and the last box of a row take what division leaves. None past
/// the last. The one geometry of the ride pages, preset or edited ([`crate::page::Page`]).
pub fn rows_box(rows: &[(u8, u8)], h: u32, i: usize) -> Option<(u32, u32, u32, u32)> {
    let total: u32 = rows.iter().map(|&(share, _)| u32::from(share)).sum();
    let (mut y, mut k) = (0, i);
    for (r, &(share, n)) in rows.iter().enumerate() {
        let rh = if r + 1 == rows.len() { h - y } else { h * u32::from(share) / total };
        let n = usize::from(n);
        if k < n {
            let bw = W / n as u32;
            let w = if k + 1 == n { W - bw * k as u32 } else { bw };
            return Some((bw * k as u32, RIDE_Y + y, w, rh));
        }
        (y, k) = (y + rh, k - n);
    }
    None
}

/// Height of a field's label line.
pub(super) const FIELD_LABEL_H: u32 = 22;

/// The characters a value box is sized for: "88.88", "12:34", "-88.8". A box's values all have one scale, so the
/// eye finds the same digit height in every box of a size (the UI review, 2026-10-25: a timer at 2x beside a
/// distance at 4x and a heart rate at 6x).
pub(super) const VALUE_CHARS: u32 = 5;

/// A value's ink stays this far from its box's edges (and its rules): two values side by side are then 2 x 3 + 1 px
/// apart, more than twice the gap between two characters of one value at 3x (UI review 2, P2: at 2 px, "12:00" and
/// "5.76" in a row read "12:005.76"). A half-width box holds five characters at 3x, no longer 4x.
pub(super) const VALUE_MARGIN: u32 = 3;

/// The smallest value box an edit may leave (w, h): five characters at 3x under the label (UI review 2, 5.2 point 2;
/// at 2x a value is no longer read at a glance).
pub const VALUE_MIN: (u32, u32) =
    (ink_cells(VALUE_CHARS, 3) + 2 * VALUE_MARGIN, PIXEL.height as u32 * 3 + FIELD_LABEL_H + 1);

/// In a field's value, what follows this is drawn small and grey after the rest: the seconds of a time over an
/// hour ("1:23|45"), so the hours and minutes keep the box's size.
pub const SMALL_TAIL: char = '|';

/// The small tail's style ([`SMALL_TAIL`]).
const TAIL: Style = Style { fg: GREY, ..LABEL };

/// The width of `s` in the pixel font less its last cell's blank column: its ink for laying out, the same for every
/// text of a length (so a value's characters keep their cells as they change).
pub(super) fn ink(style: &Style, s: &str) -> u32 {
    style.width(s).saturating_sub(if s.is_empty() { 0 } else { style.scale })
}

/// The ink width of `n` cells of the pixel font at `scale` (the last cell's blank column left out).
pub(super) const fn ink_cells(n: u32, scale: u32) -> u32 {
    (n * CELL - 1) * scale
}

/// The value scale of a box `w` x `h`: the largest at which five characters ("88.88", "12:34") fit its width and the
/// height under the label; 2 at least.
///
/// ```
/// use bryton_rustfw::ui::{box_scale, field_box, RIDE_H};
/// let half = field_box(6, RIDE_H, 0).unwrap(); // 2 x 3: 120 x 98
/// assert_eq!((box_scale(half.2, half.3), box_scale(240, 296)), (3, 8));
/// ```
pub fn box_scale(w: u32, h: u32) -> u32 {
    let fits =
        |s: u32| ink_cells(VALUE_CHARS, s) + 2 * VALUE_MARGIN <= w && PIXEL.height as u32 * s + FIELD_LABEL_H < h;
    (2..=8).rev().find(|&s| fits(s)).unwrap_or(2)
}

/// The scale of a value whose big part is `big` in a box `w` x `h`: the box's ([`box_scale`]), or, for a value
/// longer than [`VALUE_CHARS`] that does not fit it ("1.6K 6%", "200-220"), the largest under it at which it does.
/// By its length only, so a value that keeps its length keeps its scale and its characters their cells.
pub(super) fn value_scale(big: &str, w: u32, h: u32) -> u32 {
    let n = big.len() as u32;
    let mut scale = box_scale(w, h);
    while scale > 1 && ink_cells(n.max(1), scale) + 2 * VALUE_MARGIN > w {
        scale -= 1;
    }
    scale
}

/// Where a value goes in box `bx`: its style, the big part's left edge and top, the big part and the small tail
/// (empty when there is none or it would not fit: the tail goes first), and the value band (left, right).
type ValueAt<'a> = (Style, u32, u32, &'a str, &'a str, (u32, u32));

pub(super) fn value_at<'a>((x, y, w, h): (u32, u32, u32, u32), value: &'a str, fg: u16) -> ValueAt<'a> {
    let (mut big, tail) = value.split_once(SMALL_TAIL).unwrap_or((value, ""));
    let style = Style { fg, scale: value_scale(big, w, h), ..LABEL };
    let top = y + FIELD_LABEL_H + h.saturating_sub(1 + FIELD_LABEL_H).saturating_sub(style.height()) / 2;
    let band = (x + 1, x + w - 1);
    let room = band.1 - band.0;
    while ink(&style, big) > room {
        big = big.get(..big.len().saturating_sub(1)).unwrap_or(""); // too long even at 1x: cut, whole characters
    }
    let with_tail = style.width(big) + ink(&TAIL, tail);
    let (tail, ink) = if tail.is_empty() || with_tail > room { ("", ink(&style, big)) } else { (tail, with_tail) };
    (style, band.0 + room.saturating_sub(ink) / 2, top, big, tail, band)
}

/// Draws `s` in `style` from `x`, character by character, only those that differ from `old` (all: None), each in its
/// cell, nothing at or right of `right`.
fn draw_cells(c: &mut impl Canvas, style: &Style, x: u32, y: u32, right: u32, s: &str, old: Option<&str>) {
    let cell = CELL * style.scale;
    for (i, ch) in s.char_indices() {
        let cx = x + i as u32 * cell;
        match old.map(|o| o.get(i..).and_then(|t| t.chars().next())) {
            _ if cx >= right => {}
            None => {
                text::draw_clipped(c, style, cx, y, right, s.get(i..i + ch.len_utf8()).unwrap_or(" "));
            }
            Some(Some(was)) if was == ch => {}
            Some(was) => text::redraw_char(c, style, (cx, y, right), was.unwrap_or(' '), ch),
        }
    }
}

/// Height of a zone bar along the top of a box (above the label, a pixel apart from it: the labels of the value fields
/// start 9 px down). 8 px: 4 was too thin to see on the bike (owner, 2026-10-26).
pub const ZONE_BAR_H: u32 = 8;

/// The zone bar of box `bx` in `color` (BLACK clears it): the box's full width less its right rule.
pub fn draw_zone_bar(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), color: u16) {
    c.fill(x, y, w - u32::from(x + w < W), ZONE_BAR_H, color);
}

/// A field's frame with its zone bar (`bar`; BLACK: none), for the plain power and heart rate fields.
pub fn draw_field_frame_bar(c: &mut impl Canvas, bx: (u32, u32, u32, u32), field: Field, bar: u16) {
    draw_field_frame(c, bx, field);
    if bar != BLACK {
        draw_zone_bar(c, bx, bar);
    }
}

/// A field's frame: the lines between boxes (right and bottom edges, inside the box) and its label (none on a map).
pub fn draw_field_frame(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), field: Field) {
    if x + w < W {
        c.fill(x + w - 1, y, 1, h, GREY);
    }
    if y + h < H - 2 {
        c.fill(x, y + h - 1, w, 1, GREY);
    }
    if !field.is_picture() {
        text::draw(c, &Style { fg: GREY, ..LABEL }, x + 6, y + ZONE_BAR_H + 1, Align::Left, field.label());
    }
}

/// Width of the page editor's frame round the selected cell (UI review 2, 5.2 point 5).
pub const FRAME: u32 = 3;

/// The inside of a field box: without its rules (right and bottom edges, [`draw_field_frame`]).
pub fn box_inside((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x, y, w - u32::from(x + w < W), h - u32::from(y + h < H - 2))
}

/// The page editor's frame round box `bx`, inside its rules, in `color`.
pub fn draw_cell_frame(c: &mut impl Canvas, bx: (u32, u32, u32, u32), color: u16) {
    let (x, y, w, h) = box_inside(bx);
    c.fill(x, y, w, FRAME, color);
    c.fill(x, y + h - FRAME, w, FRAME, color);
    c.fill(x, y + FRAME, FRAME, h - 2 * FRAME, color);
    c.fill(x + w - FRAME, y + FRAME, FRAME, h - 2 * FRAME, color);
}

/// The page editor's hint in the strip's place, over its orange line: what the keys do.
pub fn draw_edit_hint(c: &mut impl Canvas, text: &str) {
    c.fill(0, 0, W, STRIP_H, BLACK);
    text::draw_in(c, &Style { fg: ACCENT, ..LABEL }, 0, STRIP_H - LABEL.height(), W, Align::Centre, text);
    c.fill(0, STRIP_H, W, 2, ACCENT);
}

/// A split's icon at the right of menu row `i` (the editor's SPLIT rows, no dash in the text): a box 24 x 16 cut
/// side by side (`stacked` false) or one over the other, in the row's ink.
pub fn draw_split_icon(c: &mut impl Canvas, i: usize, stacked: bool, selected: bool) {
    let (iw, ih) = (24, 16);
    let ink = if selected { ON_ACCENT } else { WHITE };
    let x = NEARBY.0 + NEARBY.2 - iw;
    let y = LIST_Y + i as u32 * MENU_ROW + (MENU_ROW - 1 - ih) / 2;
    c.fill(x, y, iw, 2, ink);
    c.fill(x, y + ih - 2, iw, 2, ink);
    c.fill(x, y, 2, ih, ink);
    c.fill(x + iw - 2, y, 2, ih, ink);
    if stacked {
        c.fill(x, y + ih / 2 - 1, iw, 2, ink);
    } else {
        c.fill(x + iw / 2 - 1, y, 2, ih, ink);
    }
}

/// A field's value in `fg`, centred by its ink under its label, in a fixed band (what was there before is cleared):
/// at the box's scale, a [`SMALL_TAIL`] small and grey after it, bottoms aligned.
pub fn draw_field_value(c: &mut impl Canvas, bx: (u32, u32, u32, u32), value: &str, fg: u16) {
    let (style, left, top, big, tail, (l, r)) = value_at(bx, value, fg);
    let bh = style.height();
    c.fill(l, top, left.saturating_sub(l), bh, BLACK);
    let mut end = text::draw_clipped(c, &style, left, top, r, big);
    if !tail.is_empty() {
        // the ink's bottom rows level: 7 font rows of each
        let ty = top + 7 * style.scale - 7 * TAIL.scale;
        let tw = TAIL.width(tail).min(r.saturating_sub(end));
        c.fill(end, top, tw, ty - top, BLACK);
        let t_end = text::draw_clipped(c, &TAIL, end, ty, r, tail);
        c.fill(end, ty + TAIL.height(), tw, (top + bh).saturating_sub(ty + TAIL.height()), BLACK);
        end = t_end;
    }
    c.fill(end, top, r.saturating_sub(end), bh, BLACK);
}

/// A field's value that was `old`: only the characters that changed when the length and the place of the small tail
/// are the same (they keep their cells), else the whole value band, and the rows of the old one's band outside it
/// (a value drawn smaller: "OPEN" then "200-220", 2026-10-25 review).
pub fn draw_field_value_changes(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: &str, new: &str, fg: u16) {
    let (style, left, top, big, tail, (l, r)) = value_at(bx, new, fg);
    let (old_style, _, old_top, old_big, old_tail, _) = value_at(bx, old, fg);
    if old_big.len() != big.len() || old_tail.len() != tail.len() || old.is_empty() {
        let (end, old_end) = (top + style.height(), old_top + old_style.height());
        c.fill(l, old_top, r - l, top.saturating_sub(old_top), BLACK);
        c.fill(l, end, r - l, old_end.saturating_sub(end), BLACK);
        return draw_field_value(c, bx, new, fg);
    }
    draw_cells(c, &style, left, top, r, big, Some(old_big));
    let ty = top + 7 * style.scale - 7 * TAIL.scale;
    draw_cells(c, &TAIL, left + style.width(big), ty, r, tail, Some(old_tail));
}

/// A graph's axis text and figures: the label font (2x), grey; no 1x text where a rider reads (UI review).
const AXIS: Style = Style { fg: GREY, ..LABEL };
/// Room left of the plot for its scale (four characters: "0.75", "2400"), and under it for the time.
const AXIS_W: u32 = 4 * CELL * 2 + 3;
const AXIS_H: u32 = 22;
/// The label lines of a graph with axes: the title and a figure, then the second figure.
const GRAPH_HEAD_H: u32 = 2 * 20 + 4;

/// A graph box this big has its axes and figures drawn (smaller ones show the title and the bars alone): the full
/// width, so the 2x scale leaves the plot most of the box.
fn has_axes(w: u32, h: u32) -> bool {
    w >= 200 && h >= 120
}

/// The graph's plot in field box `bx`: left, top, columns, height. One bar per pixel column, side by side, so the
/// bars make a filled area as stock's graph does; the newest bar is the rightmost column. Room is left for the
/// axes ([`draw_graph_axes`]) and the figures ([`draw_graph_stats`]) when the box has it.
pub fn graph_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let (left, head, below) = if has_axes(w, h) { (AXIS_W, GRAPH_HEAD_H, AXIS_H) } else { (0, FIELD_LABEL_H, 0) };
    (x + 4 + left, y + head, w.saturating_sub(8 + left), h.saturating_sub(head + 4 + below))
}

/// What a graph plots, in its label line (`title`: "POWER W", "HR"), and, when the box has room, its axes: the
/// values at the bottom, the middle and the top of the plot (`labels`) on its left, and the time it spans under it,
/// from `from` ("-7 MIN", "START") on the left to "NOW" on the right. Drawn with the box; nothing here changes as the
/// bars move.
pub fn draw_graph_axes(c: &mut impl Canvas, bx: (u32, u32, u32, u32), title: &str, labels: [&str; 3], from: &str) {
    let (x, y, w, h) = bx;
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, w - 12, Align::Left, title);
    if !has_axes(w, h) {
        return;
    }
    let a = graph_area(bx);
    let yw = AXIS_W - 3;
    let [low, mid, high] = labels;
    text::draw_in(c, &AXIS, x + 2, a.1, yw, Align::Right, high);
    text::draw_in(c, &AXIS, x + 2, a.1 + a.3 / 2 - AXIS.height() / 2, yw, Align::Right, mid);
    text::draw_in(c, &AXIS, x + 2, a.1 + a.3 - AXIS.height(), yw, Align::Right, low);
    let ty = a.1 + a.3 + 2;
    text::draw_in(c, &AXIS, a.0, ty, a.2 / 2, Align::Left, from);
    text::draw_in(c, &AXIS, a.0 + a.2 / 2, ty, a.2 - a.2 / 2, Align::Right, "NOW");
}

/// Room at a graph's top right for its figures ([`draw_graph_stats`]): "AVG 1234".
const STATS_W: u32 = 8 * CELL * 2;

/// Two figures of the ride (`lines`: what each was, "" when not drawn yet, and is: "AVG 182", "NP 195") at the right
/// of a graph's label lines, one above the other, each in its own box (None: left as drawn), only its changed
/// characters when its length stays; only in a box with axes, which has the two lines.
pub fn draw_graph_stats(
    c: &mut impl Canvas,
    (x, y, w, h): (u32, u32, u32, u32),
    title: &str,
    lines: [Option<(&str, &str)>; 2],
) {
    if !has_axes(w, h) || LABEL.width(title) + STATS_W + 16 > w {
        return;
    }
    let style = Style { fg: WHITE, ..AXIS };
    for (k, (was, l)) in (0..).zip(lines).filter_map(|(k, l)| Some((k, l?))) {
        let (right, ly) = (x + w - 6, y + 4 + k * AXIS.height());
        if was.len() == l.len() && style.width(l) <= STATS_W {
            draw_changed_chars(c, &style, right - style.width(l), ly, was, l);
        } else {
            text::draw_in(c, &style, right - STATS_W, ly, STATS_W, Align::Right, l);
        }
    }
}

/// A progress graph's column `k` (the workout graph's, the climb's): `bar` (height, colour) as it is, the part done
/// (columns before the rider's) dark, the rider's column white and `full` high.
pub fn progress_column(rider: Option<u32>, k: u32, full: u32, bar: (u32, u16)) -> (u32, u16) {
    match rider {
        Some(r) if r == k => (full, WHITE),
        Some(r) if k < r => (bar.0, DARK),
        _ => bar,
    }
}

/// The climb's plot in field box `bx`: all of the box but a margin (its figures are written over it).
pub fn climb_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x + 4, y + 3, w.saturating_sub(8), h.saturating_sub(7))
}

/// A plot without axes (the course profile) in field box `bx`: left, top, columns, height.
pub fn plot_area((x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (x + 4, y + FIELD_LABEL_H, w.saturating_sub(8), h.saturating_sub(FIELD_LABEL_H + 4))
}

/// A plot's title in its label line.
pub fn draw_plot_title(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), title: &str) {
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, x + 6, y + 4, w.saturating_sub(12), Align::Left, title);
}

/// Graph column `i` that showed a bar `old` (height, colour) now showing `new`: only the band between the two
/// tops when the colour stays (most columns as the chart scrolls: neighbouring bars are close once smoothed), else
/// the new bar whole.
pub fn draw_graph_column(c: &mut impl Canvas, area: (u32, u32, u32, u32), i: u32, old: (u32, u16), new: (u32, u16)) {
    let (x, bottom) = (area.0 + i, area.1 + area.3);
    let ((h0, c0), (h1, c1)) = ((old.0.min(area.3), old.1), (new.0.min(area.3), new.1));
    if c0 == c1 || h0 == 0 {
        if h1 > h0 {
            c.fill(x, bottom - h1, 1, h1 - h0, c1);
        }
    } else {
        c.fill(x, bottom - h1, 1, h1, c1);
    }
    if h0 > h1 {
        c.fill(x, bottom - h0, 1, h0 - h1, BLACK);
    }
}

/// The whole graph: column `i` gets `bar(i)` (height, colour), the rest of the column black.
pub fn draw_graph(c: &mut impl Canvas, area: (u32, u32, u32, u32), bar: &dyn Fn(u32) -> (u32, u16)) {
    for i in 0..area.2 {
        draw_graph_column(c, area, i, (area.3, BLACK), bar(i));
    }
}
