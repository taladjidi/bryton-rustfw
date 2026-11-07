//! Workout files from other apps, read into a [`WorkoutDraft`] for the editor: Zwift's ZWO (XML) and the ERG / MRC
//! text files of CompuTrainer-style trainers and TrainerRoad, Golden Cheetah and the like.
//!
//! Sources: the ZWO tags and attributes from github.com/h4l/zwift-workout-file-reference
//! (`zwift_workout_file_tag_reference.md`, read 2026-11-07); ERG / MRC from the format notes of Golden Cheetah
//! (github.com/GoldenCheetah/GoldenCheetah, `src/FileIO/ErgFile.cpp`: `[COURSE HEADER]`, `MINUTES WATTS` or `MINUTES
//! PERCENT`, then `[COURSE DATA]` lines of minutes and a value, each line a point of a line chart; read 2026-11-07).
//! ZWO is read by hand (a tag scanner for this subset: no XML crate, no entities beyond the five of the name).
//!
//! What becomes what:
//! - `Warmup`, `Cooldown`, `Ramp`: one step whose target is the range low to high, in % of FTP. The unit shows a band,
//!   not a slope; sub-steps would spend the 64 items for a smooth look the unit does not draw. Warm-up and cool-down
//!   get that intensity, a ramp is active.
//! - `SteadyState`: one step with the power as a point range; `FreeRide`, `MaxEffort`: a timed step with no target.
//! - `IntervalsT`: on and off steps (active, recovery) and a repeat of 2 items; repeated once, no repeat item.
//! - ERG / MRC: each pair of points is a step: equal values a flat step (equal neighbours merged), different values
//!   a range low to high; zero-length pairs (the stairs of a point list) are skipped; a value under 1 is no target.
//!   ERG values are watts, MRC values % of FTP.
//! - Left out: text events, cadence, flat-road and other attributes, a ZWO's description and tags.
//!
//! A file with more steps than the unit takes still opens: [`crate::draft::encode_workout`] says so when saving.

use crate::draft::{DraftError, DraftStep, MAX_NAME, WorkoutDraft, invalid};
use crate::workout::TargetKind;

/// A name for the file: printable ASCII only, trimmed and cut to what the unit holds; `fallback` when nothing is left.
fn clean_name(name: &str, fallback: &str) -> String {
    let pick = |s: &str| s.chars().filter(|c| (' '..='~').contains(c)).take(MAX_NAME).collect::<String>();
    [name, fallback].iter().map(|s| pick(s).trim().to_owned()).find(|s| !s.is_empty()).unwrap_or("Workout".to_owned())
}

/// Reads a ZWO, ERG or MRC file (told apart by content); `fallback_name` names a file with no name of its own.
#[uniffi::export]
pub fn import_workout(bytes: Vec<u8>, fallback_name: String) -> Result<WorkoutDraft, DraftError> {
    let text = String::from_utf8_lossy(&bytes);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    let (name, steps) = if t.starts_with('<') {
        zwo(t)?
    } else if t.to_ascii_uppercase().contains("[COURSE") {
        erg(t)?
    } else {
        return invalid("Not a ZWO, ERG or MRC workout");
    };
    if steps.is_empty() {
        return invalid("The file has no steps");
    }
    Ok(WorkoutDraft { name: clean_name(&name, &fallback_name), steps })
}

fn timed(seconds: f64, lo: f64, hi: f64, intensity: u8) -> DraftStep {
    let pct = |v: f64| (v * 100.0).round().clamp(0.0, 1000.0) as u16;
    DraftStep::power_pct(seconds.round().clamp(1.0, 86_400.0) as u32, pct(lo.min(hi)), pct(lo.max(hi)), intensity)
}

/// A tag: lower-case name, attributes (lower-case keys), whether it closes, the text up to the next tag.
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    close: bool,
    text: String,
}

impl Tag {
    fn num(&self, key: &str) -> Result<Option<f64>, DraftError> {
        let Some((_, v)) = self.attrs.iter().find(|(k, _)| k == key) else { return Ok(None) };
        match v.trim().parse::<f64>() {
            Ok(x) if x.is_finite() && (0.0..=1e6).contains(&x) => Ok(Some(x)),
            _ => invalid(format!("{}: {key}=\"{v}\" is not a number", self.name)),
        }
    }

    fn need(&self, key: &str) -> Result<f64, DraftError> {
        self.num(key)?.map_or_else(|| invalid(format!("{} has no {key}", self.name)), Ok)
    }
}

/// The tags of an XML text; comments, `<?..?>` and `<!..>` are skipped.
fn tags(src: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        if let Some(c) = rest.strip_prefix("!--") {
            rest = c.split_once("-->").map_or("", |(_, r)| r);
            continue;
        }
        let Some((inside, after)) = rest.split_once('>') else { break };
        let (text, _) = after.split_once('<').unwrap_or((after, ""));
        rest = after;
        if inside.starts_with(['?', '!']) {
            continue;
        }
        let close = inside.starts_with('/');
        let body = inside.trim_start_matches('/').trim_end_matches('/');
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let mut attrs = Vec::new();
        let mut a = &body[name_end..];
        while let Some((key, val)) = a.split_once('=') {
            let (Some(q), key) = (val.trim_start().chars().next().filter(|c| matches!(c, '"' | '\'')), key.trim())
            else {
                break;
            };
            let val = val.trim_start();
            let Some((v, r)) = val[1..].split_once(q) else { break };
            attrs.push((key.to_ascii_lowercase(), v.to_owned()));
            a = r;
        }
        out.push(Tag { name: body[..name_end].to_ascii_lowercase(), attrs, close, text: text.trim().to_owned() });
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn zwo(src: &str) -> Result<(String, Vec<DraftStep>), DraftError> {
    let (mut name, mut steps) = (String::new(), Vec::new());
    for t in tags(src).iter().filter(|t| !t.close) {
        match t.name.as_str() {
            "name" => name = unescape(&t.text),
            "warmup" | "cooldown" | "ramp" | "rampup" | "rampdown" => {
                let int = match t.name.as_str() {
                    "warmup" | "rampup" => 2,
                    "cooldown" | "rampdown" => 3,
                    _ => 0,
                };
                steps.push(timed(t.need("duration")?, t.need("powerlow")?, t.need("powerhigh")?, int));
            }
            "steadystate" | "solidstate" => {
                let p = t.need("power")?;
                steps.push(timed(t.need("duration")?, p, p, 0));
            }
            "freeride" | "maxeffort" => {
                let mut s = DraftStep::power_pct(t.need("duration")?.round().clamp(1.0, 86_400.0) as u32, 0, 0, 0);
                (s.kind, s.percent) = (TargetKind::Open, false);
                steps.push(s);
            }
            "intervalst" => {
                let times = t.num("repeat")?.unwrap_or(1.0).round().clamp(1.0, 1000.0) as u16;
                let (on, off) = (t.need("onduration")?, t.need("offduration")?);
                steps.push(timed(on, t.need("onpower")?, t.need("onpower")?, 0));
                steps.push(timed(off, t.need("offpower")?, t.need("offpower")?, 4));
                if times > 1 {
                    steps.push(DraftStep::repeat(2, times));
                }
            }
            _ => {}
        }
    }
    Ok((name, steps))
}

fn erg(src: &str) -> Result<(String, Vec<DraftStep>), DraftError> {
    let (mut name, mut watts, mut in_data) = (String::new(), false, false);
    let mut points: Vec<(f64, f64)> = Vec::new();
    for line in src.lines().map(str::trim) {
        let up = line.to_ascii_uppercase();
        if up.starts_with("[END COURSE DATA") {
            break;
        } else if up.starts_with("[COURSE DATA") {
            in_data = true;
        } else if in_data {
            let mut n = line.split(|c: char| c.is_whitespace() || c == ',' || c == ';').filter(|s| !s.is_empty());
            let (Some(a), Some(b)) = (n.next(), n.next()) else { continue };
            let (Ok(m), Ok(v)) = (a.parse::<f64>(), b.parse::<f64>()) else {
                return invalid(format!("Not a point of the workout: {line}"));
            };
            if !(m.is_finite() && v.is_finite() && m >= 0.0 && (0.0..=1e5).contains(&v))
                || points.last().is_some_and(|p| m < p.0)
            {
                return invalid(format!("Points must go forward in time: {line}"));
            }
            points.push((m, v));
        } else if up.contains("MINUTES") {
            watts = up.contains("WATTS");
        } else if let Some((k, v)) = line.split_once('=') {
            // FILE NAME names it best; DESCRIPTION when there is none
            if matches!(k.trim().to_ascii_uppercase().as_str(), "FILE NAME" | "DESCRIPTION")
                && (name.is_empty() || k.trim().eq_ignore_ascii_case("FILE NAME"))
            {
                name = v.trim().to_owned();
            }
        }
    }
    let target = |s: &mut DraftStep, lo: f64, hi: f64| {
        let v = |x: f64| x.round().clamp(0.0, 65_535.0) as u16;
        if hi < 1.0 {
            return; // a rest: no target
        }
        (s.kind, s.percent, s.low, s.high) =
            (TargetKind::Power, !watts, v(lo.min(hi)).max(u16::from(watts)), v(lo.max(hi)));
    };
    let mut steps: Vec<DraftStep> = Vec::new();
    let mut flat: Vec<bool> = Vec::new(); // whether each step is flat, to merge equal neighbours
    for w in points.windows(2) {
        let ((m0, v0), (m1, v1)) = (w[0], w[1]);
        let secs = (m1 * 60.0).round() - (m0 * 60.0).round();
        if secs < 1.0 {
            continue;
        }
        let mut s = DraftStep::power_pct(secs as u32, 0, 0, 0);
        (s.kind, s.percent) = (TargetKind::Open, false);
        target(&mut s, v0, v1);
        let is_flat = v0 == v1;
        match (steps.last_mut(), flat.last()) {
            (Some(p), Some(true))
                if is_flat && (p.kind, p.low, p.high, p.percent) == (s.kind, s.low, s.high, s.percent) =>
            {
                p.amount += s.amount;
            }
            _ => {
                steps.push(s);
                flat.push(is_flat);
            }
        }
    }
    Ok((name, steps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::{draft_preview, encode_workout};

    fn open(xml: &str) -> Result<WorkoutDraft, DraftError> {
        import_workout(xml.as_bytes().to_vec(), "file".into())
    }

    const ZWO: &str = r#"<?xml version="1.0"?>
<workout_file><author>Nobody</author><name>Test &amp; Go</name><description>x</description>
<sportType>bike</sportType><tags><tag name="x"/></tags>
<workout>
  <!-- a comment <SteadyState Duration="9" Power="9"/> -->
  <Warmup Duration="600" PowerLow="0.40" PowerHigh="0.70"/>
  <SteadyState Duration="300" Power="0.9" Cadence="90"><textevent timeoffset="0" message="go"/></SteadyState>
  <IntervalsT Repeat="4" OnDuration="60" OffDuration="120" OnPower="1.2" OffPower="0.5"/>
  <Ramp Duration="180" PowerLow="0.5" PowerHigh="0.9"/>
  <FreeRide Duration="120" FlatRoad="1"/>
  <Cooldown Duration="300" PowerLow="0.7" PowerHigh="0.3"/>
</workout></workout_file>"#;

    #[test]
    fn a_zwo_becomes_steps_and_encodes() {
        let d = open(ZWO).unwrap();
        assert_eq!(d.name, "Test & Go");
        assert_eq!(d.steps.len(), 1 + 1 + 3 + 1 + 1 + 1);
        assert_eq!((d.steps[0].low, d.steps[0].high, d.steps[0].intensity), (40, 70, 2));
        assert_eq!((d.steps[1].low, d.steps[1].high), (90, 90));
        assert_eq!((d.steps[4].repeat_back, d.steps[4].repeat_times), (2, 4));
        assert_eq!(d.steps[3].intensity, 4);
        assert_eq!(d.steps[6].kind, TargetKind::Open);
        assert_eq!((d.steps[7].low, d.steps[7].high, d.steps[7].intensity), (30, 70, 3), "cool-down: low to high");
        let v = draft_preview(d, 250, 190).unwrap();
        assert_eq!(v.total_s, 600 + 300 + 4 * 180 + 180 + 120 + 300);
        assert_eq!(v.steps.len(), 1 + 1 + 8 + 3);
    }

    #[test]
    fn mrc_and_erg_points_become_steps() {
        let mrc = "[COURSE HEADER]\nVERSION = 2\nUNITS = ENGLISH\nDESCRIPTION = Stairs\nFILE NAME = Climb prep\nMINUTES PERCENT\n[END COURSE HEADER]\n[COURSE DATA]\n0 50\n5 50\n5 80\n10 80\n10 80\n15 100\n[END COURSE DATA]\n";
        let d = open(mrc).unwrap();
        assert_eq!(d.name, "Climb prep");
        assert_eq!(d.steps.len(), 3, "{:?}", d.steps);
        assert_eq!((d.steps[0].amount, d.steps[0].low, d.steps[0].percent), (300, 50, true));
        assert_eq!((d.steps[1].amount, d.steps[1].low, d.steps[1].high), (300, 80, 80));
        assert_eq!((d.steps[2].amount, d.steps[2].low, d.steps[2].high), (300, 80, 100), "a ramp is a range");
        let erg = "[COURSE HEADER]\nMINUTES WATTS\n[END COURSE HEADER]\n[COURSE DATA]\n0.0\t120\n2.5\t120\n2.5\t0\n3\t0\n3\t250\n4\t250\n[END COURSE DATA]";
        let e = open(erg).unwrap();
        assert_eq!(e.name, "file");
        assert_eq!(e.steps.len(), 3);
        assert_eq!((e.steps[0].amount, e.steps[0].low, e.steps[0].percent), (150, 120, false));
        assert_eq!(e.steps[1].kind, TargetKind::Open, "0 W is a rest");
        assert!(encode_workout(e).is_ok());
    }

    #[test]
    fn bad_files_are_refused_and_names_are_cleaned() {
        for bad in [
            "",
            "hello",
            "<a/>",
            "<workout_file><workout><SteadyState Duration=\"x\" Power=\"1\"/></workout>",
            "<w><SteadyState Power=\"1\"/>",
            "[COURSE DATA]\n5 100\n1 50\n[END COURSE DATA]",
            "[COURSE DATA]\nfoo bar\n[END COURSE DATA]",
            "[COURSE HEADER]\n[END COURSE HEADER]",
        ] {
            assert!(open(bad).is_err(), "{bad:?}");
        }
        let long = "<w><name>\u{c9}t\u{e9} 12345678901234567890</name><SteadyState Duration=\"60\" Power=\"0.5\"/></w>";
        let d = open(long).unwrap();
        assert_eq!(d.name, "t 1234567890123", "accents dropped, 15 characters");
        assert!(encode_workout(d).is_ok());
        let d = import_workout(b"<w><SteadyState Duration=\"60\" Power=\"0.5\"/></w>".to_vec(), "Caf\u{e9}".into())
            .unwrap();
        assert_eq!(d.name, "Caf");
    }

    /// Structured and mutated files: nothing panics, and parsed ones reach the encoder.
    #[test]
    fn mutated_files_never_panic() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as usize
        };
        let seeds = [ZWO, "[COURSE DATA]\n0 50\n5 50\n5 80\n10 100\n[END COURSE DATA]"];
        let (mut parsed, mut refused) = (0, 0);
        for _ in 0..3000 {
            let mut b = seeds[next() % 2].as_bytes().to_vec();
            for _ in 0..next() % 6 {
                let at = next() % b.len();
                match next() % 3 {
                    0 => b[at] = next() as u8,
                    1 => b.truncate(at.max(1)),
                    _ => b.insert(at, b"<>\"=/ 0-.9"[next() % 10]),
                }
            }
            match import_workout(b, "f".into()) {
                Ok(d) => {
                    let _ = encode_workout(d);
                    parsed += 1;
                }
                Err(_) => refused += 1,
            }
        }
        assert!(parsed > 300 && refused > 300, "parsed {parsed}, refused {refused}");
    }
}
