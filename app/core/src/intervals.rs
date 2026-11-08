//! The rider's training profile from Intervals.icu, and its merge into the unit's.
//!
//! Sources (looked up 2026-11-08):
//! - OpenAPI spec https://intervals.icu/api/v1/docs : `GET /api/v1/athlete/{id}` ("Get the athlete with
//!   sportSettings") returns the athlete (`name`, `weight` kg as a float) and `sportSettings`, each with `types` (e.g.
//!   "Ride"), `ftp`, `lthr`, `max_hr`, `w_prime` (integers). Auth: HTTP basic, user `API_KEY`, password the key from
//!   Settings (security scheme `APIKey`).
//! - The forum's API access guide, https://forum.intervals.icu/t/api-access-to-intervals-icu/609 : the athlete id is
//!   `0` for the key's own athlete, else `i<number>` (inferred from memory of that page, the spec's id is a string).
//! - The spec types `weight` as a float and `ftp`, `lthr`, `max_hr`, `w_prime` as int32 but states no units and
//!   marks no field required: kg and J are inferred (the Intervals.icu UI shows kg and J), and so are the id forms
//!   (the spec's `id` parameter has no description). Every field is optional: the parser keeps a number from 0.5 to
//!   100000 and drops anything else (a string, a null, a negative, a huge value) as "not given"; the unit's menu
//!   ranges are applied later, by the merge. Nothing here touches the network: Swift fetches.

use crate::files::UnitProfile;
use bryton_rustfw::menu;

/// What Intervals.icu gave for the Ride sport settings; each field is None when absent or unusable.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct IntervalsProfile {
    /// The athlete's name, for the Test button.
    pub name: Option<String>,
    /// FTP, W.
    pub ftp: Option<u32>,
    /// Maximum heart rate, bpm.
    pub max_hr: Option<u32>,
    /// Lactate threshold heart rate, bpm.
    pub lthr: Option<u32>,
    /// Weight, kg (rounded).
    pub weight_kg: Option<u32>,
    /// W', J.
    pub wprime_j: Option<u32>,
}

/// Why a response could not be used, in words for the rider.
#[derive(Debug, PartialEq, Eq, uniffi::Error)]
pub enum IntervalsParseError {
    /// The answer is not the athlete JSON.
    NotAthlete,
    /// The athlete has no sport settings for Ride.
    NoRideSettings,
}

impl core::fmt::Display for IntervalsParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotAthlete => "Intervals.icu answered with something that is not an athlete.",
            Self::NoRideSettings => "This Intervals.icu account has no sport settings for Ride. Add them in Settings.",
        })
    }
}

/// Above any sane FTP, heart rate, weight or W' (the largest menu range ends at 40000).
const CAP: f64 = 100_000.0;

/// A finite number from 0.5 to [`CAP`], rounded.
fn num(v: Option<&serde_json::Value>) -> Option<u32> {
    let f = v?.as_f64()?;
    (0.5..=CAP).contains(&f).then(|| f.round() as u32)
}

/// Bike sport types an account may keep its settings under, best first.
const RIDE_TYPES: [&str; 5] = ["Ride", "VirtualRide", "GravelRide", "MountainBikeRide", "EBikeRide"];

/// Parses the body of `GET /api/v1/athlete/{id}`.
#[uniffi::export]
pub fn intervals_parse(json: Vec<u8>) -> Result<IntervalsProfile, IntervalsParseError> {
    let v: serde_json::Value = serde_json::from_slice(&json).map_err(|_| IntervalsParseError::NotAthlete)?;
    let obj = v.as_object().ok_or(IntervalsParseError::NotAthlete)?;
    let all = obj.get("sportSettings").and_then(|s| s.as_array()).ok_or(IntervalsParseError::NoRideSettings)?;
    let has = |s: &serde_json::Value, ty: &str| {
        s.get("types").and_then(|t| t.as_array()).is_some_and(|t| t.iter().any(|x| x.as_str() == Some(ty)))
    };
    let ride =
        RIDE_TYPES.iter().find_map(|ty| all.iter().find(|s| has(s, ty))).ok_or(IntervalsParseError::NoRideSettings)?;
    let name = obj.get("name").and_then(|n| n.as_str()).map(|n| n.chars().take(60).collect());
    Ok(IntervalsProfile {
        name,
        ftp: num(ride.get("ftp")),
        max_hr: num(ride.get("max_hr")),
        lthr: num(ride.get("lthr")),
        weight_kg: num(obj.get("weight")),
        wprime_j: num(ride.get("w_prime")),
    })
}

/// The unit's profile with Intervals' values laid over it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProfileMerge {
    /// What to send: the unit's fields, with each in-range Intervals field replacing its own.
    pub profile: UnitProfile,
    /// The merge differs from the unit's profile (something to send).
    pub changed: bool,
    /// Fields Intervals gave that the unit's menu cannot take, with the range, for the rider.
    pub skipped: Vec<String>,
}

fn pick(
    skipped: &mut Vec<String>,
    label: &str,
    unit: &str,
    range: &core::ops::RangeInclusive<u16>,
    given: Option<u32>,
    old: u16,
) -> u16 {
    match given {
        None => old,
        Some(g) => match u16::try_from(g) {
            Ok(v) if range.contains(&v) => v,
            _ => {
                skipped.push(format!(
                    "{label} {g} {unit} is outside the unit's {} to {} {unit}",
                    range.start(),
                    range.end()
                ));
                old
            }
        },
    }
}

/// Lays Intervals' values over the unit's profile (zones choice and sex are never touched).
#[uniffi::export]
pub fn merge_unit_profile(unit: UnitProfile, iv: IntervalsProfile) -> ProfileMerge {
    let mut skipped = Vec::new();
    let s = &mut skipped;
    let b = |v: u16| u8::try_from(v).unwrap_or(u8::MAX);
    let profile = UnitProfile {
        ftp: pick(s, "FTP", "W", &menu::FTP_RANGE, iv.ftp, unit.ftp),
        max_hr: b(pick(s, "Max HR", "bpm", &menu::MAX_HR_RANGE, iv.max_hr, u16::from(unit.max_hr))),
        lthr: b(pick(s, "LTHR", "bpm", &menu::LTHR_RANGE, iv.lthr, u16::from(unit.lthr))),
        weight_kg: b(pick(s, "Weight", "kg", &menu::WEIGHT_RANGE, iv.weight_kg, u16::from(unit.weight_kg))),
        wprime_j: pick(s, "W'", "J", &menu::WPRIME_RANGE, iv.wprime_j, unit.wprime_j),
        ..unit
    };
    ProfileMerge { changed: profile != unit, profile, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::FakeUnit;
    use crate::files::{FileClient, FileEvent};

    const SAMPLE: &str = r#"{"id":"i1","name":"Test Rider","weight":71.6,"sex":"M","sportSettings":[
        {"id":1,"types":["Run"],"ftp":null,"lthr":170},
        {"id":2,"types":["Ride","VirtualRide"],"ftp":265,"indoor_ftp":260,"lthr":171,"max_hr":188,"w_prime":21500}]}"#;

    fn unit() -> UnitProfile {
        FakeUnit::demo().profile.into()
    }

    #[test]
    fn parses_a_real_shaped_payload() {
        let p = intervals_parse(SAMPLE.as_bytes().to_vec()).unwrap();
        assert_eq!(
            p,
            IntervalsProfile {
                name: Some("Test Rider".into()),
                ftp: Some(265),
                max_hr: Some(188),
                lthr: Some(171),
                weight_kg: Some(72),
                wprime_j: Some(21500)
            }
        );
    }

    #[test]
    fn missing_wrong_and_huge_fields() {
        let j =
            r#"{"sportSettings":[{"types":["Ride"],"ftp":"265","lthr":-4,"max_hr":1e30,"w_prime":null}],"weight":"x"}"#;
        let p = intervals_parse(j.as_bytes().to_vec()).unwrap();
        assert_eq!((p.ftp, p.lthr, p.max_hr, p.wprime_j, p.weight_kg, p.name), (None, None, None, None, None, None));
        assert_eq!(intervals_parse(b"[]".to_vec()), Err(IntervalsParseError::NotAthlete));
        let run = b"{\"sportSettings\":[{\"types\":[\"Run\"]}]}".to_vec();
        assert_eq!(intervals_parse(run), Err(IntervalsParseError::NoRideSettings));
        assert_eq!(intervals_parse(b"{}".to_vec()), Err(IntervalsParseError::NoRideSettings));
    }

    #[test]
    fn hostile_documents() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        let deep_obj = format!("{{\"a\":{deep},\"sportSettings\":[]}}");
        for bad in [deep.as_bytes(), deep_obj.as_bytes(), b"\xff\xfe{\"name\":\"\xc3\"}", b"42", b"\"x\"", b"null", b""]
        {
            assert_eq!(intervals_parse(bad.to_vec()), Err(IntervalsParseError::NotAthlete), "{bad:?}");
        }
        // several entries: Ride wins over VirtualRide and Run, whatever the order
        let j = r#"{"sportSettings":[{"types":["Run"],"ftp":1},{"types":["VirtualRide"],"ftp":200},{"types":["Ride"],"ftp":250}]}"#;
        assert_eq!(intervals_parse(j.as_bytes().to_vec()).unwrap().ftp, Some(250));
        let j = r#"{"sportSettings":[{"types":["Run"],"ftp":1},{"types":["GravelRide"],"ftp":230}]}"#;
        assert_eq!(intervals_parse(j.as_bytes().to_vec()).unwrap().ftp, Some(230));
    }

    #[test]
    fn merge_keeps_unit_fields_and_drops_out_of_range() {
        let u = unit();
        let iv = IntervalsProfile { ftp: Some(300), max_hr: Some(400), weight_kg: Some(99_999), ..Default::default() };
        let m = merge_unit_profile(u, iv);
        assert_eq!(m.profile, UnitProfile { ftp: 300, ..u });
        assert!(m.changed);
        assert_eq!(m.skipped.len(), 2);
        assert!(m.skipped[0].contains("Max HR 400 bpm"));
        let same = merge_unit_profile(u, IntervalsProfile { ftp: Some(u32::from(u.ftp)), ..Default::default() });
        assert!(!same.changed && same.skipped.is_empty());
    }

    #[test]
    fn a_fake_unit_round_trip() {
        let mut u = FakeUnit::demo();
        let c = FileClient::new();
        let mut t = 0;
        let run = |u: &mut FakeUnit, cmd: Vec<u8>, t: &mut u64| {
            let mut last = FileEvent::Ignored;
            for f in u.handle(&cmd) {
                *t += 10;
                last = c.on_data(f, *t);
            }
            last
        };
        assert_eq!(run(&mut u, c.profile_get_command(t), &mut t), FileEvent::Profile);
        let iv = intervals_parse(SAMPLE.as_bytes().to_vec()).unwrap();
        let m = merge_unit_profile(c.profile().unwrap(), iv.clone());
        assert_eq!(run(&mut u, c.profile_set_command(m.profile, t), &mut t), FileEvent::ProfileSet);
        assert_eq!((u.profile.ftp, u.profile.max_hr, u.profile.lthr, u.profile.weight_kg), (265, 188, 171, 72));
        // an unchanged set is answered OK with nothing written, and the merge sees nothing to send
        assert!(!merge_unit_profile(u.profile.into(), iv).changed);
    }

    #[test]
    fn fuzz_structured_json_never_panics_and_reaches_ok() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let vals = ["265", "-1", "0", "1e400", "99999999999999999999", "2.5", "null", "\"7\"", "[]", "{}", "true"];
        let (mut ok, mut no_ride, mut bad) = (0, 0, 0);
        for _ in 0..3000 {
            let mut v = || vals[(next() % vals.len() as u64) as usize];
            let (a, b, c, d, e) = (v(), v(), v(), v(), v());
            let types = ["[\"Ride\"]", "[\"Run\"]", "\"Ride\"", "[]", "null", "[1,\"Ride\"]"][(next() % 6) as usize];
            let j = format!(
                "{{\"name\":{a},\"weight\":{b},\"sportSettings\":[{{\"types\":{types},\"ftp\":{c},\"lthr\":{d},\"max_hr\":{e},\"w_prime\":{a}}}]}}"
            );
            let mut bytes = j.into_bytes();
            if next() % 8 == 0 {
                let i = (next() % bytes.len() as u64) as usize;
                bytes.truncate(i);
            }
            match intervals_parse(bytes) {
                Ok(p) => {
                    ok += 1;
                    let m = merge_unit_profile(unit(), p);
                    assert!(menu::FTP_RANGE.contains(&m.profile.ftp));
                }
                Err(IntervalsParseError::NoRideSettings) => no_ride += 1,
                Err(IntervalsParseError::NotAthlete) => bad += 1,
            }
        }
        assert!(ok > 300 && no_ride > 300 && bad > 100, "{ok} {no_ride} {bad}");
    }
}
