//! A planned workout for the screen (roadmap 9, the workouts step): the unit's FIT workout file decoded by the
//! firmware's own parser (`storage::workout_file`, so the app reads what the unit reads, repeats already written out),
//! turned into plain records for Swift, with the power targets in watts for the rider's FTP and a TSS / IF estimate.
//! Also the synthetic workouts of the Demo unit, written here as FIT files (no real plan, no personal data).

use bryton_rustfw::fit::crc;
use bryton_rustfw::ride::Zones;
use bryton_rustfw::storage::workout_file::{self as wf, Kind, Length};

/// What a step's target is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TargetKind {
    /// No target.
    Open,
    /// Power.
    Power,
    /// Heart rate.
    HeartRate,
    /// Cadence.
    Cadence,
}

/// One step of a workout.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StepView {
    /// Its length, s (0 when it ends by distance or by the Lap key).
    pub seconds: u32,
    /// Its length, m (0 when it is timed or open).
    pub distance_m: u32,
    /// What the target is on.
    pub kind: TargetKind,
    /// The target's low end as the file gives it: % of FTP (`percent`), W, % of max heart rate, bpm or rpm.
    pub low: u16,
    /// The target's high end, as `low`.
    pub high: u16,
    /// `low` and `high` are percentages (of FTP, or of the maximum heart rate).
    pub percent: bool,
    /// The power zone (1 to 7) the file asked for, 0 for a range.
    pub zone: u8,
    /// A power target in watts for the FTP given to [`decode_workout`] (low, high); 0 for another kind.
    pub watts_low: u16,
    /// See `watts_low`.
    pub watts_high: u16,
    /// The power zone (1 to 7) of the target's middle for that FTP; 0 for another kind.
    pub power_zone: u8,
    /// FIT intensity: 0 active, 1 rest, 2 warm-up, 3 cool-down, 4 recovery, 5 interval, 6 other.
    pub intensity: u8,
}

/// A decoded workout.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkoutView {
    /// Its name (capitals; the unit keeps 15 characters).
    pub name: String,
    /// The time of its timed steps, s.
    pub total_s: u32,
    /// Its steps in order, repeats written out.
    pub steps: Vec<StepView>,
    /// Steps were left out (more than the unit keeps).
    pub cut: bool,
    /// Intensity factor times 100 (normalised as a 4th-power mean over time); 0 unless every step is a timed power
    /// step, so the figure covers the whole workout.
    pub intensity_x100: u32,
    /// Training stress score estimate (hours x IF squared x 100); 0 as `intensity_x100`.
    pub tss: u32,
}

/// Decodes a workout file for a rider with this FTP (W) and maximum heart rate (bpm); None if it is no workout (bad
/// framing, CRC, type).
#[uniffi::export]
pub fn decode_workout(bytes: Vec<u8>, ftp: u16, max_hr: u16) -> Option<WorkoutView> {
    let w = wf::parse(&bytes).ok()?;
    let z = Zones::standard(ftp.max(1), max_hr.max(1));
    let edges = *z.power_edges();
    let (mut steps, mut secs, mut pow4, mut other) = (Vec::new(), 0u64, 0f64, false);
    for s in w.steps() {
        let t = s.target;
        let (seconds, distance_m) = match s.len {
            Length::Time(t) => (t, 0),
            Length::Distance(m) => (0, m),
            Length::Open => (0, 0),
        };
        let kind = match t.kind {
            Kind::Open => TargetKind::Open,
            Kind::Power => TargetKind::Power,
            Kind::Hr => TargetKind::HeartRate,
            Kind::Cadence => TargetKind::Cadence,
        };
        let range = if kind == TargetKind::Power { t.range(&z) } else { None };
        let (watts_low, watts_high) = range.unwrap_or((0, 0));
        let mid = watts_low.midpoint(watts_high);
        let power_zone = range.map_or(0, |_| 1 + edges.iter().filter(|&&e| mid >= e).count() as u8);
        if range.is_some() && seconds > 0 {
            secs += u64::from(seconds);
            pow4 += f64::from(seconds) * (f64::from(mid) / f64::from(ftp.max(1))).powi(4);
        } else {
            other = true; // a step the estimate does not cover: distance, Lap key, heart rate, cadence, open
        }
        steps.push(StepView {
            seconds,
            distance_m,
            kind,
            low: t.lo,
            high: t.hi,
            percent: t.pct,
            zone: t.zone,
            watts_low,
            watts_high,
            power_zone,
            intensity: s.intensity,
        });
    }
    let (intensity_x100, tss) = if secs == 0 || other {
        (0, 0)
    } else {
        let intf = (pow4 / secs as f64).powf(0.25);
        ((intf * 100.0).round() as u32, (secs as f64 / 3600.0 * intf * intf * 100.0).round() as u32)
    };
    Some(WorkoutView { name: w.name().to_owned(), total_s: w.total_s(), steps, cut: w.cut, intensity_x100, tss })
}

/// A step for [`fit_workout`]: duration type and value (time: ms), target type and value, low, high, intensity.
pub type Raw = (u8, u32, u8, u32, u32, u32, u8);

/// A FIT workout file laid out as the firmware's reader expects (file_id type 5, workout, one workout_step message
/// per step); the format is in `storage/workout_file.rs`.
pub fn fit_workout(name: &str, steps: &[Raw]) -> Vec<u8> {
    let mut d = vec![0x40, 0, 0, 0, 0, 1, 0, 1, 0, 0, 5]; // file_id: type 5
    let mut n = [0u8; 16];
    let b = name.as_bytes();
    n[..b.len().min(15)].copy_from_slice(&b[..b.len().min(15)]);
    d.extend([0x41, 0, 0, 26, 0, 2, 8, 16, 7, 6, 2, 0x84, 1]);
    d.extend(n);
    d.extend((steps.len() as u16).to_le_bytes());
    d.extend([0x42, 0, 0, 27, 0, 9, 254, 2, 0x84, 1, 1, 0, 2, 4, 0x86, 3, 1, 0, 4, 4, 0x86, 5, 4, 0x86]);
    d.extend([6, 4, 0x86, 7, 1, 0, 0, 3, 7]);
    for (i, &(dt, dv, tt, tv, lo, hi, int)) in steps.iter().enumerate() {
        d.push(0x02);
        d.extend((i as u16).to_le_bytes());
        d.push(dt);
        d.extend(dv.to_le_bytes());
        d.push(tt);
        d.extend(tv.to_le_bytes());
        d.extend(lo.to_le_bytes());
        d.extend(hi.to_le_bytes());
        d.push(int);
        d.extend(b"s\0\0");
    }
    let mut f = vec![14, 0x20, 0x5c, 0x08];
    f.extend((d.len() as u32).to_le_bytes());
    f.extend(b".FIT");
    f.extend(crc(0, &f).to_le_bytes());
    f.extend(d);
    f.extend(crc(0, &f).to_le_bytes());
    f
}

const MIN: u32 = 60_000;

/// A timed step with a power target in % of FTP (FIT workout_power: 0 to 1000 is a percentage).
fn power(secs: u32, lo: u32, hi: u32, intensity: u8) -> Raw {
    (0, secs * 1000, 4, 0, lo, hi, intensity)
}

/// The Demo unit's workouts: `(key, file)`, keys as the unit gives them (day, then the file's number).
pub fn demo_workouts() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut sweet = vec![power(600, 50, 65, 2)];
    for _ in 0..3 {
        sweet.extend([power(600, 88, 94, 5), power(300, 50, 55, 4)]);
    }
    sweet.push(power(600, 45, 55, 3));
    let mut vo2 = vec![power(900, 50, 70, 2), power(60, 90, 105, 0)];
    for _ in 0..5 {
        vo2.extend([power(180, 112, 120, 5), power(180, 45, 50, 4)]);
    }
    vo2.push(power(600, 40, 55, 3));
    // heart rate in bpm is the file's bpm + 100; cadence in rpm
    let endurance = vec![
        power(900, 45, 60, 2),
        (0, 70 * MIN, 1, 0, 100 + 125, 100 + 140, 0),
        (0, 10 * MIN, 3, 0, 95, 105, 0),
        (0, 5 * MIN, 2, 0, 0, 0, 3),
    ];
    vec![
        (b"2026110800".to_vec(), fit_workout("Sweet spot 3x10", &sweet)),
        (b"2026110900".to_vec(), fit_workout("VO2 5x3", &vo2)),
        (b"2026111100".to_vec(), fit_workout("Endurance HR", &endurance)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_workouts_decode_with_the_firmware_parser() {
        let all = demo_workouts();
        let w = decode_workout(all[0].1.clone(), 200, 190).expect("sweet spot");
        assert_eq!((w.name.as_str(), w.steps.len(), w.total_s), ("SWEET SPOT 3X10", 8, 3900));
        let s = &w.steps[1];
        assert_eq!(
            (s.kind, s.percent, s.low, s.high, s.watts_low, s.watts_high),
            (TargetKind::Power, true, 88, 94, 176, 188)
        );
        assert_eq!(s.power_zone, 4, "91 % of FTP: sweet spot is tempo to threshold");
        // IF between the hardest step (94 %) and the easiest (50 %), TSS a few dozen for an hour of it
        assert!((70..=90).contains(&w.intensity_x100), "{}", w.intensity_x100);
        assert!((50..=110).contains(&w.tss), "{}", w.tss);
        let hr = decode_workout(all[2].1.clone(), 200, 190).unwrap();
        assert_eq!(hr.steps[1].kind, TargetKind::HeartRate);
        assert_eq!((hr.steps[1].low, hr.steps[1].high, hr.steps[1].percent), (125, 140, false));
        assert_eq!(hr.steps[2].kind, TargetKind::Cadence);
        assert_eq!(hr.steps[3].kind, TargetKind::Open);
        assert_eq!(hr.steps[1].watts_low, 0);
        // the same file at a higher FTP asks for more watts
        let z = decode_workout(all[0].1.clone(), 300, 190).unwrap();
        assert_eq!(z.steps[1].watts_low, 264);
    }

    #[test]
    fn the_estimate_covers_only_timed_power_steps() {
        let whole = fit_workout("A", &[power(600, 80, 90, 0), power(600, 50, 60, 0)]);
        let w = decode_workout(whole, 200, 190).unwrap();
        assert!(w.tss > 0 && w.intensity_x100 > 0);
        // a distance step, a Lap step or a heart-rate step leaves the workout half covered: no figure
        for extra in [(1, 5000, 4, 0, 80, 90, 0), (5, 0, 4, 0, 80, 90, 0), (0, 600_000, 1, 0, 125, 140, 0)] {
            let b = fit_workout("A", &[power(600, 80, 90, 0), extra]);
            let w = decode_workout(b, 200, 190).unwrap();
            assert_eq!((w.tss, w.intensity_x100), (0, 0), "{extra:?}");
        }
    }

    /// The max heart rate of the rider scales a percentage target, the FTP the power one.
    #[test]
    fn heart_rate_percentages_follow_the_given_maximum() {
        let b = fit_workout("H", &[(0, 600_000, 1, 0, 70, 80, 0)]);
        let z = |hr| decode_workout(b.clone(), 200, hr).unwrap().steps[0].clone();
        assert_eq!((z(190).kind, z(190).percent), (TargetKind::HeartRate, true));
        assert_eq!((z(190).low, z(170).low), (70, 70), "the file's own numbers are not changed");
    }

    /// Re-seals both CRCs of a FIT file after its body was changed.
    fn reseal(b: &mut [u8]) {
        let n = b.len();
        let h = crc(0, &b[..12]).to_le_bytes();
        b[12..14].copy_from_slice(&h);
        let c = crc(0, &b[..n - 2]).to_le_bytes();
        b[n - 2..].copy_from_slice(&c);
    }

    /// Structured workouts (random step types, sizes, repeats, long names) and good files with their body mutated
    /// and the CRCs sealed again: nothing panics, and both a decode and a refusal happen over the seeds.
    #[test]
    fn mutated_and_random_workouts_never_panic() {
        let (mut ok, mut no) = (0, 0);
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u32
        };
        let good = demo_workouts().remove(1).1;
        for _ in 0..3000 {
            let b = if next().is_multiple_of(2) {
                let n = next() as usize % 40;
                let steps: Vec<Raw> = (0..n)
                    .map(|_| match next() % 4 {
                        // a repeat step: back to step 0, a few times
                        0 => (6, next() % 3, 2, next() % 4, next() % 90, next() % 90, (next() % 8) as u8),
                        _ => (
                            (next() % 7) as u8,
                            next(),
                            (next() % 6) as u8,
                            next(),
                            next() % 3000,
                            next() % 3000,
                            (next() % 8) as u8,
                        ),
                    })
                    .collect();
                let name = "N".repeat(next() as usize % 40);
                fit_workout(&name, &steps)
            } else {
                let mut b = good.clone();
                for _ in 0..1 + next() % 4 {
                    let at = 14 + next() as usize % (b.len() - 16);
                    b[at] = next() as u8;
                }
                reseal(&mut b);
                b
            };
            match decode_workout(b, (next() % 700) as u16, (next() % 260) as u16) {
                Some(w) => {
                    assert!(w.steps.len() <= 4096 && w.name.chars().count() <= 15 + 16, "{w:?}");
                    ok += 1;
                }
                None => no += 1,
            }
        }
        assert!(ok > 200 && no > 200, "ok {ok}, refused {no}");
    }

    #[test]
    fn bad_bytes_are_no_workout() {
        let good = demo_workouts().remove(1).1;
        assert!(decode_workout(good.clone(), 250, 190).is_some());
        let mut seen = 0;
        for cut in 0..good.len() {
            seen += usize::from(decode_workout(good[..cut].to_vec(), 250, 190).is_some());
        }
        for i in 0..good.len() {
            let mut b = good.clone();
            b[i] ^= 0x55;
            seen += usize::from(decode_workout(b, 250, 190).is_some());
        }
        assert_eq!(seen, 0, "a cut or a flipped byte breaks the CRC");
        assert_eq!(decode_workout(Vec::new(), 250, 190), None);
        assert!(decode_workout(good, 0, 190).is_some(), "FTP 0 is taken as 1, no division by zero");
    }
}
