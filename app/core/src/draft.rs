//! The workout editor's side of the core (roadmap 9, workouts): a [`WorkoutDraft`] the phone edits, encoded as the FIT
//! workout file the unit's parser reads ([`encode_workout`], the writer is `workout::fit_workout`, the reader
//! `storage/workout_file.rs`), previewed through that same parser ([`draft_preview`]), and made back from a decoded
//! workout to edit a copy ([`draft_from_view`]).
//!
//! Sources: Garmin FIT SDK Profile (developer.fit.garmin.com/fit/download/, `Profile.xlsx`, messages `workout` 26 and
//! `workout_step` 27; types `wkt_step_duration`, `wkt_step_target`, `workout_power`, `workout_hr`, `intensity`),
//! read 2026-11-07, and the field list in `storage/workout_file.rs`:
//! - `duration_type` 0 time (ms), 1 distance (cm), 5 open (until the Lap key), 6 repeat_until_steps_cmplt
//!   (`duration_value` the step to repeat from, `target_value` the number of times in all);
//! - `target_type` 2 open, 1 heart rate, 3 cadence, 4 power; power custom values 0 to 1000 are % FTP, above it
//!   watts + 1000; heart rate 0 to 100 is % of max, above it bpm + 100.
//!
//! A FIT workout has no separate "Lap key" duration: "until Lap" and "open" are the same step (duration_type 5).
//! A repeat is an item of the list that says "the last `repeat_back` items, `repeat_times` times in all"; repeats nest
//! (a repeat may be inside the repeated items), as the file's own repeat steps do.

use crate::workout::{StepView, TargetKind, WorkoutView, decode_workout, fit_workout};

/// Items of a draft at most, repeat steps included (the firmware's reader takes 64 file steps, `RAW` there).
pub const MAX_ITEMS: usize = 64;
/// Steps once repeats are written out (`workout_file::STEPS`).
pub const MAX_STEPS: usize = bryton_rustfw::storage::workout_file::STEPS;
/// Characters of a name the file holds (a 16 byte string with its NUL).
pub const MAX_NAME: usize = bryton_rustfw::storage::workout_file::NAME - 1;
/// The longest step by time, s.
pub const MAX_SECONDS: u32 = 86_400;
/// The longest step by distance, m.
pub const MAX_METERS: u32 = 1_000_000;

/// What ends a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DurationKind {
    /// A time, `amount` s.
    Time,
    /// A distance, `amount` m.
    Distance,
    /// The Lap key (FIT "open").
    Lap,
}

/// One item of a draft: a step, or (`repeat_back` above 0) a repeat of the items before it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DraftStep {
    /// 0 for a step; otherwise this item repeats the `repeat_back` items before it.
    pub repeat_back: u8,
    /// The repeat's count in all (2 or more), when `repeat_back` is above 0.
    pub repeat_times: u16,
    /// What ends the step.
    pub duration: DurationKind,
    /// Seconds or metres, as `duration` says (ignored for the Lap key).
    pub amount: u32,
    /// What the target is on.
    pub kind: TargetKind,
    /// The target's low end: % of FTP or W (power), % of max heart rate or bpm, rpm.
    pub low: u16,
    /// The target's high end, as `low`.
    pub high: u16,
    /// `low` and `high` are percentages (power or heart rate only).
    pub percent: bool,
    /// FIT intensity: 0 active, 1 rest, 2 warm-up, 3 cool-down, 4 recovery, 5 interval, 6 other.
    pub intensity: u8,
}

impl DraftStep {
    /// A timed step with a power range in % of FTP.
    pub fn power_pct(seconds: u32, low: u16, high: u16, intensity: u8) -> Self {
        DraftStep {
            repeat_back: 0,
            repeat_times: 0,
            duration: DurationKind::Time,
            amount: seconds,
            kind: TargetKind::Power,
            low,
            high,
            percent: true,
            intensity,
        }
    }

    /// A repeat of the `back` items before it, `times` in all.
    pub fn repeat(back: u8, times: u16) -> Self {
        DraftStep {
            repeat_back: back,
            repeat_times: times,
            duration: DurationKind::Lap,
            amount: 0,
            kind: TargetKind::Open,
            low: 0,
            high: 0,
            percent: false,
            intensity: 0,
        }
    }
}

/// A workout being edited.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkoutDraft {
    /// Its name: printable ASCII, [`MAX_NAME`] characters at most (the unit shows capitals).
    pub name: String,
    /// Its items in order.
    pub steps: Vec<DraftStep>,
}

/// Why a draft or an import was refused.
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum DraftError {
    /// The message says what and where, for the rider.
    Invalid {
        /// What is wrong.
        message: String,
    },
}

impl core::fmt::Display for DraftError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let DraftError::Invalid { message } = self;
        f.write_str(message)
    }
}

/// A refusal with this message.
pub(crate) fn invalid<T>(message: impl Into<String>) -> Result<T, DraftError> {
    Err(DraftError::Invalid { message: message.into() })
}

/// The file's step for item `i` (0-based); a repeat checks its reach against `i`.
fn raw(i: usize, s: &DraftStep) -> Result<crate::workout::Raw, DraftError> {
    let n = i + 1;
    if s.repeat_back > 0 {
        let (back, times) = (usize::from(s.repeat_back), u32::from(s.repeat_times));
        if back > i {
            return invalid(format!("Item {n}: repeats {back} items, only {i} come before it"));
        }
        if !(2..=1000).contains(&times) {
            return invalid(format!("Item {n}: a repeat is 2 to 1000 times"));
        }
        return Ok((6, (i - back) as u32, 2, times, 0, 0, 0));
    }
    let (dt, dv) = match s.duration {
        DurationKind::Time if (1..=MAX_SECONDS).contains(&s.amount) => (0, s.amount * 1000),
        DurationKind::Time => return invalid(format!("Item {n}: a time is 1 s to 24 h")),
        DurationKind::Distance if (1..=MAX_METERS).contains(&s.amount) => (1, s.amount * 100),
        DurationKind::Distance => return invalid(format!("Item {n}: a distance is 1 m to 1000 km")),
        DurationKind::Lap => (5, 0),
    };
    let (lo, hi) = (u32::from(s.low), u32::from(s.high));
    if s.kind != TargetKind::Open && !(lo <= hi && hi >= 1) {
        return invalid(format!("Item {n}: the target's low end is above its high end, or it is empty"));
    }
    // (type, low, high) as the file has them; the limits keep a % from reading as W or bpm, and a W or bpm from a %
    let (tt, lo, hi) = match (s.kind, s.percent) {
        (TargetKind::Open, _) => (2, 0, 0),
        (TargetKind::Power, true) if hi <= 1000 => (4, lo, hi),
        (TargetKind::Power, false) if lo >= 1 && hi <= 2500 => (4, lo + 1000, hi + 1000),
        (TargetKind::Power, _) => return invalid(format!("Item {n}: power is up to 1000 % of FTP or 2500 W")),
        (TargetKind::HeartRate, true) if hi <= 100 => (1, lo, hi),
        (TargetKind::HeartRate, false) if lo >= 1 && hi <= 250 => (1, lo + 100, hi + 100),
        (TargetKind::HeartRate, _) => return invalid(format!("Item {n}: heart rate is up to 100 % or 250 bpm")),
        (TargetKind::Cadence, _) if hi <= 250 => (3, lo, hi),
        (TargetKind::Cadence, _) => return invalid(format!("Item {n}: cadence is up to 250 rpm")),
    };
    if s.intensity > 6 {
        return invalid(format!("Item {n}: unknown intensity"));
    }
    Ok((dt, dv, tt, 0, lo, hi, s.intensity))
}

/// Encodes a draft as the FIT workout file the unit reads.
///
/// Refuses what the unit cannot take: a name that is not printable ASCII or is over 15 characters, no step, more
/// than [`MAX_ITEMS`] items or [`MAX_STEPS`] steps once the repeats are written out, a value out of range.
#[uniffi::export]
pub fn encode_workout(draft: WorkoutDraft) -> Result<Vec<u8>, DraftError> {
    if draft.name.trim().is_empty() {
        return invalid("Give it a name");
    }
    if draft.name.chars().count() > MAX_NAME || !draft.name.bytes().all(|c| (0x20..0x7f).contains(&c)) {
        return invalid(format!("The name is up to {MAX_NAME} letters, digits and signs, no accents"));
    }
    if draft.steps.len() > MAX_ITEMS {
        return invalid(format!("At most {MAX_ITEMS} items (steps and repeats), this has {}", draft.steps.len()));
    }
    let mut steps = Vec::with_capacity(draft.steps.len());
    let mut written: Vec<usize> = Vec::with_capacity(draft.steps.len()); // steps each item adds, repeats written out
    for (i, s) in draft.steps.iter().enumerate() {
        steps.push(raw(i, s)?);
        let add = if s.repeat_back > 0 {
            let region: usize = written.get(i - usize::from(s.repeat_back)..).unwrap_or(&[]).iter().sum();
            region * (usize::from(s.repeat_times) - 1)
        } else {
            1
        };
        written.push(add.min(10_000));
    }
    let total: usize = written.iter().sum();
    if total == 0 {
        return invalid("Add at least one step");
    }
    if total > MAX_STEPS {
        return invalid(format!("{total} steps once repeats are written out; the unit keeps {MAX_STEPS}"));
    }
    let file = fit_workout(&draft.name, &steps);
    if file.len() > bryton_rustfw::storage::workout_file::FILE_MAX {
        return invalid("The file is too big for the unit");
    }
    Ok(file)
}

/// What the unit would show for a draft: it is encoded, then read back by the firmware's parser, so the chart, the
/// steps and the IF / TSS estimate are exactly the unit's.
#[uniffi::export]
pub fn draft_preview(draft: WorkoutDraft, ftp: u16, max_hr: u16) -> Result<WorkoutView, DraftError> {
    let bytes = encode_workout(draft)?;
    decode_workout(bytes, ftp, max_hr).map_or_else(|| invalid("The unit's reader refused the file"), Ok)
}

/// A draft from a decoded workout, to edit a copy: the file has its repeats written out, so runs of equal steps
/// (a block of one to 32 steps, three times or more, or a longer block twice) are folded back into a repeat, and
/// the result is folded again until nothing changes, so nested repeats come back nested. A copy can still pass
/// [`MAX_ITEMS`] (a stock file of up to 96 steps with no equal runs): the editor then asks to remove some.
#[uniffi::export]
pub fn draft_from_view(view: WorkoutView) -> WorkoutDraft {
    let mut steps: Vec<DraftStep> = view.steps.iter().map(step_of).collect();
    loop {
        let folded = fold(&steps);
        if folded.len() == steps.len() {
            break;
        }
        steps = folded;
    }
    WorkoutDraft { name: view.name, steps }
}

/// One pass of folding runs into repeats.
fn fold(steps: &[DraftStep]) -> Vec<DraftStep> {
    // A run can end only where no later repeat reaches back across the end, and its block must hold the whole
    // reach of its own repeats: equal items mean the same only then.
    let mut cut = vec![true; steps.len() + 1];
    for (q, s) in steps.iter().enumerate() {
        if s.repeat_back > 0 {
            let first = q.saturating_sub(usize::from(s.repeat_back)) + 1;
            cut.iter_mut().take(q + 1).skip(first).for_each(|c| *c = false);
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        // (steps saved, block length, times) of the best fold starting here
        let mut best = (0, 1, 1);
        for l in 1..=32.min((steps.len() - i) / 2) {
            if steps[i..i + l].iter().enumerate().any(|(o, s)| usize::from(s.repeat_back) > o) {
                continue;
            }
            let mut k = 1;
            while steps.get(i + k * l..i + (k + 1) * l) == steps.get(i..i + l) {
                k += 1;
            }
            while k >= 2 && !cut[i + k * l] {
                k -= 1;
            }
            if k >= 2 && l * (k - 1) > best.0 + 1 {
                best = (l * (k - 1) - 1, l, k);
            }
        }
        let (saved, l, k) = best;
        if saved > 0 {
            out.extend_from_slice(&steps[i..i + l]);
            out.push(DraftStep::repeat(l as u8, k as u16));
            i += l * k;
        } else {
            out.push(steps[i].clone());
            i += 1;
        }
    }
    out
}

fn step_of(s: &StepView) -> DraftStep {
    let (duration, amount) = match (s.seconds, s.distance_m) {
        (0, 0) => (DurationKind::Lap, 0),
        (0, m) => (DurationKind::Distance, m),
        (t, _) => (DurationKind::Time, t),
    };
    let open = s.kind == TargetKind::Open;
    DraftStep {
        repeat_back: 0,
        repeat_times: 0,
        duration,
        amount,
        kind: s.kind,
        low: if open { 0 } else { s.low },
        high: if open { 0 } else { s.high },
        percent: s.percent && matches!(s.kind, TargetKind::Power | TargetKind::HeartRate),
        intensity: s.intensity.min(6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(duration: DurationKind, amount: u32, kind: TargetKind, low: u16, high: u16, percent: bool) -> DraftStep {
        DraftStep { repeat_back: 0, repeat_times: 0, duration, amount, kind, low, high, percent, intensity: 0 }
    }

    fn draft(name: &str, steps: Vec<DraftStep>) -> WorkoutDraft {
        WorkoutDraft { name: name.to_owned(), steps }
    }

    /// The steps a draft means once its repeats are written out, by the format's own rule.
    fn expand(items: &[DraftStep]) -> Vec<DraftStep> {
        let (mut out, mut start): (Vec<DraftStep>, Vec<usize>) = (Vec::new(), Vec::new());
        for (i, s) in items.iter().enumerate() {
            start.push(out.len());
            if s.repeat_back > 0 {
                let (from, to) = (start[i - usize::from(s.repeat_back)], out.len());
                for _ in 1..s.repeat_times {
                    out.extend_from_within(from..to);
                }
            } else {
                out.push(s.clone());
            }
        }
        out
    }

    /// Checks that `d` encodes and the firmware's parser gives back exactly its steps.
    fn round_trip(d: &WorkoutDraft) -> WorkoutView {
        let bytes = encode_workout(d.clone()).expect("encodes");
        let v = decode_workout(bytes, 250, 190).expect("decodes");
        assert_eq!(v.name, d.name.to_ascii_uppercase());
        let want = expand(&d.steps);
        assert_eq!(v.steps.len(), want.len(), "{d:?}");
        for (g, w) in v.steps.iter().zip(&want) {
            let open = w.kind == TargetKind::Open;
            assert_eq!(g.kind, w.kind);
            assert_eq!((g.low, g.high), if open { (0, 0) } else { (w.low, w.high) });
            assert_eq!(g.percent, w.percent && !open && w.kind != TargetKind::Cadence, "{w:?}");
            assert_eq!(g.intensity, w.intensity);
            let (secs, m) = match w.duration {
                DurationKind::Time => (w.amount, 0),
                DurationKind::Distance => (0, w.amount),
                DurationKind::Lap => (0, 0),
            };
            assert_eq!((g.seconds, g.distance_m), (secs, m));
        }
        v
    }

    #[test]
    fn every_target_and_duration_round_trips() {
        use DurationKind::*;
        use TargetKind::*;
        let mut steps = Vec::new();
        for (d, a) in [(Time, 90), (Distance, 2500), (Lap, 0)] {
            steps.extend([
                step(d, a, Power, 88, 94, true),
                step(d, a, Power, 180, 220, false),
                step(d, a, Power, 60, 60, true),
                step(d, a, HeartRate, 70, 80, true),
                step(d, a, HeartRate, 130, 145, false),
                step(d, a, Cadence, 85, 95, false),
                step(d, a, Open, 0, 0, false),
            ]);
        }
        steps[0].intensity = 2;
        steps[1].intensity = 3;
        let v = round_trip(&draft("Every kind", steps));
        assert_eq!((v.steps[0].watts_low, v.steps[0].watts_high), (220, 235), "88 to 94 % of 250 W");
        assert_eq!((v.steps[1].watts_low, v.steps[1].watts_high), (180, 220), "watts are not scaled by the FTP");
    }

    #[test]
    fn repeats_nest_and_follow_the_formats_rule() {
        // warm-up, (on, off) x 3, a rest, all of that x 2, cool-down
        let steps = vec![
            DraftStep::power_pct(300, 50, 65, 2),
            DraftStep::power_pct(60, 110, 120, 5),
            DraftStep::power_pct(30, 40, 50, 4),
            DraftStep::repeat(2, 3),
            DraftStep::power_pct(120, 45, 55, 4),
            DraftStep::repeat(5, 2),
            DraftStep::power_pct(300, 40, 50, 3),
        ];
        let v = round_trip(&draft("Nested", steps));
        assert_eq!(v.steps.len(), 2 * (1 + 6 + 1) + 1, "the outer repeat takes the warm-up too");
        assert_eq!(v.total_s, 2 * (300 + 3 * 90 + 120) + 300);
    }

    #[test]
    fn the_units_limits_are_refused_with_a_message() {
        let ok = vec![DraftStep::power_pct(60, 50, 60, 0)];
        let bad = |d: WorkoutDraft| match encode_workout(d) {
            Err(DraftError::Invalid { message }) => message,
            Ok(_) => panic!("accepted"),
        };
        assert!(bad(draft("A very long workout name", ok.clone())).contains("name"));
        assert!(bad(draft("Caf\u{e9}", ok.clone())).contains("name"));
        assert!(bad(draft("A", vec![])).contains("at least one"));
        assert!(bad(draft("   ", ok.clone())).contains("name"));
        assert!(bad(draft("A", vec![DraftStep::repeat(1, 2)])).contains("Item 1"));
        assert!(bad(draft("A", vec![ok[0].clone(), DraftStep::repeat(1, 1)])).contains("2 to 1000"));
        assert!(bad(draft("A", vec![ok[0].clone(); 65])).contains("64"));
        let r = vec![ok[0].clone(), DraftStep::repeat(1, 98)];
        assert!(bad(draft("A", r)).contains("96"));
        for (lo, hi, pct, kind) in [
            (90, 80, true, TargetKind::Power),
            (0, 0, true, TargetKind::Power),
            (0, 1001, true, TargetKind::Power),
            (0, 100, false, TargetKind::Power),
            (60, 101, true, TargetKind::HeartRate),
            (20, 251, false, TargetKind::HeartRate),
            (0, 251, false, TargetKind::Cadence),
        ] {
            let mut s = ok[0].clone();
            (s.low, s.high, s.percent, s.kind) = (lo, hi, pct, kind);
            bad(draft("A", vec![s]));
        }
        let mut s = ok[0].clone();
        s.amount = MAX_SECONDS + 1;
        bad(draft("A", vec![s.clone()]));
        s.duration = DurationKind::Distance;
        s.amount = MAX_METERS + 1;
        bad(draft("A", vec![s]));
        // 64 items and 96 steps are accepted, and the reader keeps them all
        let v = round_trip(&draft("A", vec![ok[0].clone(); 64]));
        assert!(!v.cut);
        let r = vec![ok[0].clone(), DraftStep::repeat(1, 96)];
        assert_eq!(round_trip(&draft("A", r)).steps.len(), 96);
    }

    #[test]
    fn copies_fold_their_repeats_back() {
        let on = DraftStep::power_pct(60, 110, 120, 5);
        let off = DraftStep::power_pct(30, 40, 50, 4);
        let d = draft("Fold", vec![on.clone(), off, DraftStep::repeat(2, 5), DraftStep::power_pct(60, 40, 50, 3)]);
        let v = draft_preview(d, 250, 190).unwrap();
        let back = draft_from_view(v.clone());
        assert_eq!(back.steps.len(), 4, "{back:?}");
        assert_eq!(draft_preview(back, 250, 190).unwrap().steps, v.steps);
        // two equal steps stay two: a repeat would save nothing
        let two = draft_from_view(draft_preview(draft("T", vec![on.clone(), on]), 250, 190).unwrap());
        assert_eq!(two.steps.len(), 2);
    }

    /// Copies of the demo workouts and of a nested one come back with the same steps, and smaller.
    #[test]
    fn copies_round_trip_with_nesting() {
        let (on, off) = (DraftStep::power_pct(60, 110, 120, 5), DraftStep::power_pct(30, 40, 50, 4));
        let mut dist = step(DurationKind::Distance, 2500, TargetKind::HeartRate, 130, 145, false);
        dist.intensity = 2;
        let lap = step(DurationKind::Lap, 0, TargetKind::Open, 0, 0, false);
        let nested = draft(
            "Nested",
            vec![
                DraftStep::power_pct(300, 50, 65, 2),
                on,
                off,
                DraftStep::repeat(2, 3),
                DraftStep::power_pct(120, 45, 55, 4),
                DraftStep::repeat(5, 2),
                dist,
                lap,
                DraftStep::power_pct(300, 40, 50, 3),
            ],
        );
        let mut views = vec![draft_preview(nested, 250, 190).unwrap()];
        views.extend(crate::workout::demo_workouts().into_iter().map(|(_, f)| decode_workout(f, 250, 190).unwrap()));
        assert_eq!(views.len(), 4);
        for v in views {
            let back = draft_from_view(v.clone());
            assert!(
                back.steps.len() <= v.steps.len() && (v.name != "NESTED" || back.steps.len() < 14),
                "{}: {} items",
                v.name,
                back.steps.len()
            );
            assert_eq!(draft_preview(back, 250, 190).unwrap().steps, v.steps, "{}", v.name);
        }
        let v = draft_preview(draft("N", vec![DraftStep::power_pct(60, 50, 60, 0)]), 250, 190).unwrap();
        let two = draft_from_view(v);
        assert!(two.steps.iter().all(|s| s.repeat_back == 0));
    }

    /// Writes an encoded workout for `tools/fit_check.py --workout` (the independent FIT decoders).
    #[test]
    fn the_encoded_workout_is_written_for_fit_check() {
        let d = draft(
            "Sweet spot",
            vec![
                DraftStep::power_pct(600, 50, 65, 2),
                DraftStep::power_pct(60, 90, 95, 5),
                DraftStep::power_pct(30, 40, 50, 4),
                DraftStep::repeat(2, 3),
            ],
        );
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::write(dir.join("workout.fit"), encode_workout(d).unwrap()).unwrap();
    }

    /// Random drafts, valid and not: nothing panics, and an accepted draft reads back exactly.
    #[test]
    fn random_drafts_never_panic_and_accepted_ones_round_trip() {
        let (mut ok, mut no) = (0, 0);
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as u32
        };
        for _ in 0..4000 {
            let n = next() as usize % 20;
            let mut steps = Vec::new();
            for i in 0..n {
                steps.push(if i > 0 && next() % 5 == 0 {
                    DraftStep::repeat((next() % 4) as u8, (next() % 6) as u16)
                } else {
                    let kind = [TargetKind::Open, TargetKind::Power, TargetKind::HeartRate, TargetKind::Cadence]
                        [next() as usize % 4];
                    let a = (next() % 300) as u16;
                    DraftStep {
                        repeat_back: 0,
                        repeat_times: 0,
                        duration: [DurationKind::Time, DurationKind::Distance, DurationKind::Lap][next() as usize % 3],
                        amount: next() % 4000,
                        kind,
                        low: a,
                        high: a + (next() % 60) as u16,
                        percent: next() % 2 == 0,
                        intensity: (next() % 8) as u8,
                    }
                });
            }
            let name: String = (0..next() % 20).map(|_| (0x20 + next() % 0x5f) as u8 as char).collect();
            let d = draft(&name, steps);
            match encode_workout(d.clone()) {
                Ok(_) => {
                    round_trip(&d);
                    ok += 1;
                }
                Err(_) => no += 1,
            }
        }
        assert!(ok > 300 && no > 300, "ok {ok}, refused {no}");
    }
}
