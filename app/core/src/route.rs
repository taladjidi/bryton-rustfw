//! Routes: a GPX file read into a course the unit follows, shown on the phone and sent as a FIT course.
//!
//! The firmware's own code does the reading, so the phone and the unit agree on what a route is:
//! - **GPX:** `storage::course_file::Gpx` (track and route points with their `ele`, the turn words of points and of
//!   waypoints; see that file's doc for the subset read). The route's name is the first `<name>` of the file.
//! - **Length and climbs:** `course::step_mm` (the unit's flat-plane distance, a jump of over 1 km where a segment
//!   starts is not ridden) and `Course::climbs_of` (Garmin's ClimbPro rule: 500 m at 3 %), on the thinned profile.
//!   The ascent counts rises past a 3 m hysteresis, as the unit's.
//! - **Thinning:** more than the unit's 4096 points (`course::POINTS`, which thins the same way: evenly along the
//!   way) are cut by distance, the first and last kept.
//! - **FIT course** (Garmin's FIT SDK, `Cookbook/CourseEncode/Program.cs` in github.com/garmin/fit-csharp-sdk and the
//!   profile's message numbers, read 2026-11-08): `file_id` (type 6, course), `course` (sport cycling, name), a
//!   `lap` (start and end position, distance, time), a timer `event` start, a `record` per point (position,
//!   altitude as (m + 500) x 5, distance in 1/100 m, and a timestamp at 20 km/h since the file has no clock), a
//!   `course_point` per cue (type from the profile's `course_point`: 6 left, 7 right, 19 and 21 slight, 20 and 22
//!   sharp, 23 u-turn, 8 straight, 1 summit, 3 water, 4 food, 5 danger, 9 first aid, 0 generic), and the stop
//!   `event`. The bytes depend only on the route, so sending it twice is the same file to the unit.
//!   A file with no `ele` has the altitude invalid (all ones).
//! - **Checked:** every file is read back by the firmware's FIT reader (`fitread::Reader::with_position`, what
//!   `course_file::is_course` runs on the card) before it is given out; `tools/fit_check.py --course` reads one with
//!   fitdecode and Garmin's SDK (`cargo test the_encoded_route_is_written_for_fit_check`).
//!
//! A route planner produces a [`RouteView`] the same way: points in, [`route_from_points`]; a FIT course from another
//! tool comes in through [`route_from_fit`].

use crate::draft::DraftError;
use bryton_rustfw::course::{Course, Item, JUMP_M, POINTS, Turn, step_mm};
use bryton_rustfw::filesvc::CourseName;
use bryton_rustfw::fit::crc;
use bryton_rustfw::fitread::{Msg, Reader};
use bryton_rustfw::storage::course_file::Gpx;
use bryton_rustfw::storage::workout_write::COURSE_MAX;

/// The largest GPX read, bytes.
const READ_MAX: usize = 8 << 20;
/// Cues kept (course points).
const CUES_MAX: usize = 200;
/// The unit's name length.
const NAME_MAX: usize = 20;
/// FIT time of the first record (arbitrary: a course has no clock).
const BASE_TIME: u32 = 1_100_000_000;

/// A point of a route, for the map and the profile.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RoutePoint {
    /// Degrees.
    pub lat: f64,
    /// Degrees.
    pub lon: f64,
    /// From the start, m.
    pub dist_m: u32,
    /// Altitude, m (None for a route with no elevation).
    pub alt_m: Option<f64>,
}

/// A climb of a route (the unit's definition).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RouteClimb {
    /// From the start, m.
    pub start_m: u32,
    /// Length, m.
    pub len_m: u32,
    /// Gain, m.
    pub gain_m: u32,
    /// Average grade, % x 10.
    pub grade_x10: u32,
}

/// A route as the phone shows it and the unit follows it.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct RouteView {
    /// Its name, in the unit's alphabet (1 to 20 letters, digits, spaces, `-`, `_`).
    pub name: String,
    /// Length, m.
    pub distance_m: u32,
    /// Ascent, m.
    pub ascent_m: u32,
    /// Descent, m.
    pub descent_m: u32,
    /// The points carry an altitude.
    pub has_alt: bool,
    /// At most 4096 points, evenly spaced along the way once thinned.
    pub points: Vec<RoutePoint>,
    /// The climbs (empty without altitude).
    pub climbs: Vec<RouteClimb>,
    /// Turns and waypoints sent as course points.
    pub cue_count: u32,
    /// The file had more points than the unit keeps, so it was thinned.
    pub thinned: bool,
    /// What was left out or chosen, in words (a second group of points, separate tracks); empty when nothing.
    pub notes: Vec<String>,
    /// The FIT course to send.
    pub fit: Vec<u8>,
}

/// A point on its way in: degrees x 1e-7, altitude in dm, a new segment starts here.
#[derive(Clone, Copy, Debug)]
struct P {
    lat: i32,
    lon: i32,
    alt: Option<i32>,
    split: bool,
}

/// A cue on its way in: position, FIT `course_point` type, distance when the file states it.
#[derive(Clone, Copy, Debug)]
struct Cue {
    lat: i32,
    lon: i32,
    kind: u8,
    dist_m: Option<u32>,
    /// The point it is on (a track point's own turn word), so its distance is that point's: an out-and-back keeps
    /// each pass's cue.
    idx: Option<usize>,
}

fn fit_type(t: Turn) -> u8 {
    match t {
        Turn::Left => 6,
        Turn::Right => 7,
        Turn::SlightLeft => 19,
        Turn::SlightRight => 21,
        Turn::SharpLeft => 20,
        Turn::SharpRight => 22,
        Turn::UTurn => 23,
        Turn::Straight => 8,
        Turn::Summit => 1,
        Turn::Water => 3,
        Turn::Food => 4,
        Turn::Danger => 5,
        Turn::FirstAid => 9,
        _ => 0,
    }
}

fn refuse<T>(message: &str) -> Result<T, DraftError> {
    Err(DraftError::Invalid { message: message.to_owned() })
}

/// A letter without its accent, as plain ASCII ("é" is "e", "ß" is "ss", "œ" is "oe"); None for any other character.
fn fold(c: char) -> Option<&'static str> {
    let lower = c.to_lowercase().next()?;
    let f = match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    };
    Some(f)
}

/// A name in the unit's alphabet from any text: accents folded ("été" is "ete", "Côte d'Azur" is "Cote dAzur"),
/// apostrophes dropped, other characters spaces, runs of spaces one, cut to 20; None when nothing the unit takes is
/// left (empty, or a Windows device name).
#[uniffi::export]
pub fn route_name(text: String) -> Option<String> {
    let mut mapped = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            mapped.push(c);
        } else if let Some(f) = fold(c) {
            // a capital keeps its case ("Ç" is "C", "Œ" is "Oe")
            let mut it = f.chars();
            if let Some(first) = it.next() {
                mapped.push(if c.is_uppercase() { first.to_ascii_uppercase() } else { first });
                mapped.extend(it);
            }
        } else if !matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '`') {
            mapped.push(' ');
        }
    }
    let words: Vec<&str> = mapped.split(' ').filter(|w| !w.is_empty()).collect();
    let cut: String = words.join(" ").chars().take(NAME_MAX).collect();
    let name = cut.trim().to_owned();
    CourseName::new(name.as_bytes()).map(|_| name)
}

/// Whether the unit would take these for one course: the same name once cut, any case, `_` for a space (its COURSES
/// menu shows them alike).
#[uniffi::export]
pub fn route_clash(a: String, b: String) -> bool {
    let key = |s: &str| s.to_ascii_lowercase().replace('_', " ");
    key(&a) == key(&b)
}

/// The check the unit lists a course with: its file name as listed (`<name>.fit`) and size (src/link/filesvc.rs
/// `name_check`), so the phone finds the course it sent in the unit's list.
#[uniffi::export]
pub fn course_check(name: String, size: u32) -> u16 {
    bryton_rustfw::filesvc::name_check(format!("{name}.fit").as_bytes(), size)
}

/// The text of an XML element: CDATA unwrapped, the five entities and numeric references decoded.
fn xml_text(s: &str) -> String {
    let s = s.replace("<![CDATA[", "").replace("]]>", "");
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find('&') {
        out += rest.get(..i).unwrap_or("");
        let tail = rest.get(i..).unwrap_or("");
        let end = tail.find(';').filter(|&e| e <= 10);
        let ent = end.and_then(|e| tail.get(1..e));
        let ch = ent.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let n = e.strip_prefix('#')?;
                let v = match n.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(v)
            }
        });
        match (ch, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = tail.get(e + 1..).unwrap_or("");
            }
            _ => {
                out.push('&');
                rest = tail.get(1..).unwrap_or("");
            }
        }
    }
    out + rest
}

/// The text of the first `<name>` in `s`, None when there is none or it is not closed.
fn name_in(s: &str) -> Option<String> {
    let from = s.find("<name>")? + 6;
    let rest = s.get(from..)?;
    Some(xml_text(rest.get(..rest.find("</name>")?)?).trim().to_owned()).filter(|n| !n.is_empty())
}

/// The route's name: `<metadata><name>`, else the `<name>` of the first `<trk>` or `<rte>` (before its points); the
/// names of waypoints and of the author are not it.
fn gpx_name(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes.get(..bytes.len().min(1 << 20))?).into_owned();
    if let (Some(a), Some(b)) = (text.find("<metadata"), text.find("</metadata>")) {
        let meta = text.get(a..b).unwrap_or("");
        // out of the author's and copyright's elements
        let mut flat = String::new();
        let mut rest = meta;
        while let Some(i) = rest.find("<author") {
            flat += rest.get(..i).unwrap_or("");
            rest = rest.get(i..).and_then(|r| r.find("</author>").and_then(|e| r.get(e + 9..))).unwrap_or("");
        }
        flat += rest;
        if let Some(n) = name_in(&flat) {
            return Some(n);
        }
    }
    let start = ["<trk>", "<rte>", "<trk ", "<rte "].iter().filter_map(|t| text.find(t)).min()?;
    let body = text.get(start..)?;
    let end = ["<trkseg", "<trkpt", "<rtept", "</trk>", "</rte>"].iter().filter_map(|t| body.find(t)).min()?;
    name_in(body.get(..end)?)
}

/// A UTF-16 file (byte order mark) as UTF-8; anything else as it is.
fn utf8(bytes: Vec<u8>) -> Vec<u8> {
    let (big, rest) = match bytes.as_slice() {
        [0xff, 0xfe, rest @ ..] => (false, rest),
        [0xfe, 0xff, rest @ ..] => (true, rest),
        _ => return bytes,
    };
    let units = rest
        .chunks_exact(2)
        .map(|c| if big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
    String::from_utf16_lossy(&units.collect::<Vec<_>>()).into_bytes()
}

/// `bytes` with every `from` written as `to` (the same length): a file read as if its `rtept` (or `trkpt`) were
/// another tag, since the firmware's reader does not say which it read.
fn rename(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut i = 0;
    while let Some(w) = bytes.get(i..i + from.len()) {
        if w == from {
            if let Some(slot) = out.get_mut(i..i + to.len()) {
                slot.copy_from_slice(to);
            }
            i += from.len();
        } else {
            i += 1;
        }
    }
    out
}

/// The points and cues the firmware's reader finds in GPX bytes.
fn read_gpx(bytes: &[u8]) -> (Vec<P>, Vec<Cue>) {
    let (mut pts, mut cues) = (Vec::new(), Vec::new());
    let mut gpx = Gpx::new();
    for chunk in bytes.chunks(4096) {
        gpx.push_slice(chunk, &mut |item| match item {
            Item::Point(p) => pts.push(P { lat: p.lat, lon: p.lon, alt: p.alt_dm, split: p.split }),
            Item::Cue(lat, lon, t, dist_m) => cues.push(Cue { lat, lon, kind: fit_type(t), dist_m, idx: None }),
            Item::CueHere(t) => {
                if let Some((i, p)) = pts.len().checked_sub(1).and_then(|i| Some((i, pts.get(i)?))) {
                    cues.push(Cue { lat: p.lat, lon: p.lon, kind: fit_type(t), dist_m: None, idx: Some(i) });
                }
            }
        });
    }
    (pts, cues)
}

/// Reads a GPX file into a route named after the file's own name (or `fallback_name`). A UTF-16 file is read as
/// UTF-8. A file with a track and a route reads the one with more points (the track on a tie): planners often write
/// the same road as both.
#[uniffi::export]
pub fn import_route(bytes: Vec<u8>, fallback_name: String) -> Result<RouteView, DraftError> {
    if bytes.len() > READ_MAX {
        return refuse("The GPX file is too big for a route (8 MB at most).");
    }
    let bytes = utf8(bytes);
    let (trk, rte) = (read_gpx(&rename(&bytes, b"rtept", b"xtept")), read_gpx(&rename(&bytes, b"trkpt", b"xrkpt")));
    let mut notes = Vec::new();
    let apart = |a: &[P], b: &[P]| match (a.first(), b.first()) {
        (Some(x), Some(y)) => step_mm((x.lat, x.lon), (y.lat, y.lon)) > 200_000,
        _ => false,
    };
    if apart(&trk.0, &rte.0) {
        notes.push("The file holds a track and a different route: the one with more points is used.".to_owned());
    }
    let (pts, cues) = if rte.0.len() > trk.0.len() { rte } else { trk };
    let name = gpx_name(&bytes).and_then(route_name).or_else(|| route_name(fallback_name)).unwrap_or("Route".into());
    build(name, pts, cues, notes)
}

/// A route from points a planner made (`dist_m` is ignored: it is worked out).
#[uniffi::export]
pub fn route_from_points(name: String, points: Vec<RoutePoint>) -> Result<RouteView, DraftError> {
    let e7 = |d: f64| (d * 1e7).round().clamp(-1.8e9, 1.8e9) as i32;
    let pts = points
        .iter()
        .filter(|p| p.lat.abs() <= 90.0 && p.lon.abs() <= 180.0)
        .map(|p| P {
            lat: e7(p.lat),
            lon: e7(p.lon),
            alt: p.alt_m.filter(|a| a.abs() < 1e5).map(|a| (a * 10.0).round() as i32),
            split: false,
        })
        .collect();
    build(route_name(name).unwrap_or("Route".into()), pts, Vec::new(), Vec::new())
}

/// A route from a FIT course (one this encoder made, or another tool's), named `name`.
#[uniffi::export]
pub fn route_from_fit(bytes: Vec<u8>, name: String) -> Result<RouteView, DraftError> {
    let Some((pts, cues)) = read_back(&bytes) else { return refuse("This file is not a FIT course.") };
    build(route_name(name).unwrap_or("Route".into()), pts, cues, Vec::new())
}

/// The firmware's FIT reader on a course: its points and cues, None when the file is not whole.
fn read_back(fit: &[u8]) -> Option<(Vec<P>, Vec<Cue>)> {
    let (mut pts, mut cues) = (Vec::new(), Vec::new());
    let mut r = Reader::with_position();
    r.push_slice(fit, |m| match m {
        Msg::Record(x) => {
            if let Some((la, lo)) = x.pos.filter(|&(la, lo)| from_semi(la).abs() <= 900_000_000 && lo != i32::MAX) {
                pts.push(P { lat: from_semi(la), lon: from_semi(lo), alt: x.alt_dm, split: false });
            }
        }
        Msg::CoursePoint { pos: Some((la, lo)), kind, dist_cm } => {
            let (lat, lon) = (from_semi(la), from_semi(lo));
            cues.push(Cue { lat, lon, kind: kind.unwrap_or(0), dist_m: dist_cm.map(|c| c / 100), idx: None });
        }
        _ => {}
    });
    (r.result() == Some(Ok(()))).then_some((pts, cues))
}

/// Semicircles to degrees x 1e-7.
fn from_semi(semi: i32) -> i32 {
    (i64::from(semi) * 1_800_000_000 / (1 << 31)) as i32
}

/// Degrees x 1e-7 to semicircles.
fn to_semi(e7: i32) -> i32 {
    // 180 degrees is 2^31, one past the type; i32::MAX is FIT's "invalid", so stop one short
    i32::try_from(i64::from(e7) * (1 << 31) / 1_800_000_000).unwrap_or(i32::MAX - 1)
}

/// Distance from the start at each point, m: the unit's flat-plane steps, a jump of over 1 km where a segment
/// starts left out.
fn walk(pts: &[P]) -> Vec<u32> {
    let mut mm = 0u64;
    let mut out = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        if let Some(a) = i.checked_sub(1).and_then(|k| pts.get(k)) {
            let step = step_mm((a.lat, a.lon), (p.lat, p.lon));
            if !(p.split && step > JUMP_M * 1000) {
                mm = mm.saturating_add(step);
            }
        }
        out.push(u32::try_from(mm / 1000).unwrap_or(u32::MAX));
    }
    out
}

/// The indices kept: all of them up to the unit's 4096, else one about every total / 4094 m, the first and last
/// always.
fn thin(cum: &[u32]) -> Vec<usize> {
    let n = cum.len();
    if n <= POINTS {
        return (0..n).collect();
    }
    let total = cum.last().copied().unwrap_or(0);
    let step = total.div_ceil(POINTS as u32 - 2).max(1);
    let mut keep = vec![0];
    let mut last = 0;
    for (i, &d) in cum.iter().enumerate().take(n - 1).skip(1) {
        if d - last >= step {
            keep.push(i);
            last = d;
        }
    }
    keep.push(n - 1);
    keep
}

/// Rises and falls past a 3 m hysteresis, m.
fn ascent_descent(alt: &[i32]) -> (u32, u32) {
    let Some(&first) = alt.first() else { return (0, 0) };
    let (mut base, mut up, mut down) = (first, 0i64, 0i64);
    for &a in alt {
        if a - base >= 30 {
            up += i64::from(a - base);
            base = a;
        } else if base - a >= 30 {
            down += i64::from(base - a);
            base = a;
        }
    }
    (u32::try_from(up / 10).unwrap_or(u32::MAX), u32::try_from(down / 10).unwrap_or(u32::MAX))
}

/// The altitude (dm) at distance `d` on the thinned points, interpolated.
fn alt_at(kept: &[(P, u32)], d: u32) -> Option<i32> {
    let k = kept.partition_point(|x| x.1 <= d).clamp(1, kept.len().saturating_sub(1).max(1));
    let (a, b) = (kept.get(k - 1)?, kept.get(k)?);
    let (da, db) = (i64::from(a.1), i64::from(b.1));
    let (aa, ab) = (i64::from(a.0.alt?), i64::from(b.0.alt?));
    let t = (i64::from(d) - da).clamp(0, (db - da).max(0));
    i32::try_from(if db > da { aa + (ab - aa) * t / (db - da) } else { ab }).ok()
}

fn build(name: String, mut pts: Vec<P>, mut cues: Vec<Cue>, mut notes: Vec<String>) -> Result<RouteView, DraftError> {
    // separate tracks (a jump of over 1 km where a segment starts) are not one ride: the first is kept
    let gap = (1..pts.len()).find(|&i| {
        let (a, b) = (pts.get(i - 1), pts.get(i));
        matches!((a, b), (Some(a), Some(b)) if b.split && step_mm((a.lat, a.lon), (b.lat, b.lon)) > JUMP_M * 1000)
    });
    if let Some(i) = gap {
        pts.truncate(i);
        cues.retain(|c| c.idx.is_none_or(|k| k < i));
        notes.push("The file holds several separate tracks: only the first one is kept.".to_owned());
    }
    if pts.len() < 2 {
        return refuse("The file holds no route: it needs at least two track or route points.");
    }
    // FIT's altitude field spans -500 m to 12 600 m; anything else is a wrong number, left out as missing
    for p in &mut pts {
        p.alt = p.alt.filter(|a| (-5000..=126_000).contains(a));
    }
    let has_alt = pts.iter().any(|p| p.alt.is_some());
    if has_alt {
        // carry the last altitude forward (the first known one backward): a gap in `ele` is not a cliff
        let mut at = pts.iter().find_map(|p| p.alt).unwrap_or(0);
        for p in &mut pts {
            at = *p.alt.get_or_insert(at);
        }
    }
    let cum = walk(&pts);
    let (ascent_m, descent_m) = ascent_descent(&pts.iter().filter_map(|p| p.alt).collect::<Vec<_>>());
    let keep = thin(&cum);
    let thinned = keep.len() < pts.len();
    let kept: Vec<(P, u32)> = keep.iter().filter_map(|&i| Some((*pts.get(i)?, *cum.get(i)?))).collect();
    let distance_m = kept.last().map_or(0, |k| k.1);

    let climbs = if has_alt {
        let (found, n) = Course::climbs_of(distance_m, |d| alt_at(&kept, d));
        let one = |c: &bryton_rustfw::course::Climb| RouteClimb {
            start_m: c.start_m,
            len_m: c.len_m,
            gain_m: c.gain_m,
            grade_x10: c.grade_x10(),
        };
        found.iter().take(n).map(one).collect()
    } else {
        Vec::new()
    };

    // a cue is on its own point, or states its distance, or is placed on the nearest point; a waypoint over 500 m
    // from the route (a far landmark) is not a cue of it
    cues.retain_mut(|c| {
        if let Some(d) = c.idx.and_then(|i| cum.get(i)) {
            c.dist_m = Some(*d);
            return true;
        }
        let near = kept.iter().map(|p| (step_mm((c.lat, c.lon), (p.0.lat, p.0.lon)), p.1)).min();
        c.dist_m = c.dist_m.or(near.map(|n| n.1));
        near.is_some_and(|n| n.0 <= 500_000)
    });
    cues.truncate(CUES_MAX);
    cues.sort_by_key(|c| c.dist_m);

    let fit = encode(&name, &kept, &cues, has_alt);
    if fit.len() > COURSE_MAX as usize {
        return refuse("The route is bigger than the unit takes (1 MB).");
    }
    // what the unit will read: the same points and cues, or the encoder is wrong
    match read_back(&fit) {
        Some((p, c)) if p.len() == kept.len() && c.len() == cues.len() => {}
        _ => return refuse("The route could not be written as a course the unit reads."),
    }
    let points = kept
        .iter()
        .map(|(p, d)| RoutePoint {
            lat: f64::from(p.lat) / 1e7,
            lon: f64::from(p.lon) / 1e7,
            dist_m: *d,
            alt_m: p.alt.filter(|_| has_alt).map(|a| f64::from(a) / 10.0),
        })
        .collect();
    let cue_count = u32::try_from(cues.len()).unwrap_or(0);
    Ok(RouteView { name, distance_m, ascent_m, descent_m, has_alt, points, climbs, cue_count, thinned, notes, fit })
}

/// The Demo's GPX file: a made-up loop of about 25 km around the Eiffel Tower's quarter with two hills (invented
/// altitudes, a ring with a wavy edge: no real road), a few turn waypoints. For the Simulator and the screenshots.
#[uniffi::export]
pub fn demo_route_gpx() -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"demo\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n\
         <metadata><name>Eiffel loop</name></metadata>\n",
    );
    let at = |a: f64| {
        let r = 0.034 + 0.006 * (3.0 * a).sin();
        let hill = |c: f64, h: f64| h * (-(((a - c) / 0.4).powi(2))).exp();
        (48.8584 + r * a.sin(), 2.2945 + 1.5 * r * a.cos(), 38.0 + hill(1.3, 140.0) + hill(4.2, 120.0))
    };
    for (k, word) in [(40, "Left"), (130, "Right"), (230, "Slight Left"), (310, "Right")] {
        let (lat, lon, _) = at(k as f64 / 360.0 * std::f64::consts::TAU);
        s += &format!("<wpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><name>{word}</name></wpt>\n");
    }
    s += "<trk><name>Eiffel loop</name><trkseg>\n";
    for k in 0..=360 {
        let (lat, lon, ele) = at(k as f64 / 360.0 * std::f64::consts::TAU);
        s += &format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{ele:.1}</ele></trkpt>\n");
    }
    (s + "</trkseg></trk></gpx>\n").into_bytes()
}

/// A FIT definition message: local type, global number, fields (number, size, base type).
fn define(d: &mut Vec<u8>, local: u8, global: u16, fields: &[(u8, u8, u8)]) {
    d.extend([0x40 | local, 0, 0]); // header, reserved, little endian
    d.extend(global.to_le_bytes());
    d.push(fields.len() as u8);
    for &(n, s, t) in fields {
        d.extend([n, s, t]);
    }
}

const ENUM: u8 = 0x00;
const U16: u8 = 0x84;
const S32: u8 = 0x85;
const U32: u8 = 0x86;
const STR: u8 = 0x07;

fn encode(name: &str, kept: &[(P, u32)], cues: &[Cue], has_alt: bool) -> Vec<u8> {
    // 0.18 s per metre (20 km/h)
    let at = |m: u32| BASE_TIME.saturating_add((u64::from(m) * 18 / 100) as u32);
    let end = |k: Option<&(P, u32)>| k.map_or((0, 0), |k| (k.0.lat, k.0.lon));
    let ((lat0, lon0), (lat1, lon1)) = (end(kept.first()), end(kept.last()));
    let total = kept.last().map_or(0, |k| k.1);
    let (t0, t1) = (at(0), at(total));
    let mut d = Vec::new();
    // file_id: type 6 (course), manufacturer 255 (development), product 0, time_created
    define(&mut d, 0, 0, &[(0, 1, ENUM), (1, 2, U16), (2, 2, U16), (4, 4, U32)]);
    d.extend([0, 6]);
    d.extend(255u16.to_le_bytes());
    d.extend(0u16.to_le_bytes());
    d.extend(t0.to_le_bytes());
    // course: sport 2 (cycling), name
    define(&mut d, 1, 31, &[(4, 1, ENUM), (5, 24, STR)]);
    d.extend([1, 2]);
    let mut n = [0u8; 24];
    for (slot, b) in n.iter_mut().zip(name.bytes().take(NAME_MAX)) {
        *slot = b;
    }
    d.extend(n);
    // lap: timestamp, start_time, start and end position, total_elapsed_time, total_timer_time (ms), total_distance (cm)
    let lap = [(253, 4, U32), (2, 4, U32), (3, 4, S32), (4, 4, S32), (5, 4, S32), (6, 4, S32)];
    define(&mut d, 2, 19, &[lap[0], lap[1], lap[2], lap[3], lap[4], lap[5], (7, 4, U32), (8, 4, U32), (9, 4, U32)]);
    d.push(2);
    d.extend(t1.to_le_bytes());
    d.extend(t0.to_le_bytes());
    for v in [lat0, lon0, lat1, lon1] {
        d.extend(to_semi(v).to_le_bytes());
    }
    for v in [(t1 - t0).saturating_mul(1000), (t1 - t0).saturating_mul(1000), total.saturating_mul(100)] {
        d.extend(v.to_le_bytes());
    }
    // event: timestamp, event 0 (timer), event_type 0 (start); the stop (9, stop_disable_all) comes last
    define(&mut d, 3, 21, &[(253, 4, U32), (0, 1, ENUM), (1, 1, ENUM)]);
    d.push(3);
    d.extend(t0.to_le_bytes());
    d.extend([0, 0]);
    // record: timestamp, position, altitude, distance
    define(&mut d, 4, 20, &[(253, 4, U32), (0, 4, S32), (1, 4, S32), (2, 2, U16), (5, 4, U32)]);
    for (p, m) in kept {
        d.push(4);
        d.extend(at(*m).to_le_bytes());
        d.extend(to_semi(p.lat).to_le_bytes());
        d.extend(to_semi(p.lon).to_le_bytes());
        let alt = match p.alt {
            Some(a) if has_alt => u16::try_from((i64::from(a) + 5000) / 2).unwrap_or(0).min(0xfffe),
            _ => 0xffff,
        };
        d.extend(alt.to_le_bytes());
        d.extend(m.saturating_mul(100).to_le_bytes());
    }
    // course_point: message_index, timestamp, position, distance, type
    if !cues.is_empty() {
        define(&mut d, 5, 32, &[(254, 2, U16), (1, 4, U32), (2, 4, S32), (3, 4, S32), (4, 4, U32), (5, 1, ENUM)]);
    }
    for (i, c) in cues.iter().enumerate() {
        let m = c.dist_m.unwrap_or(0);
        d.push(5);
        d.extend((i as u16).to_le_bytes());
        d.extend(at(m).to_le_bytes());
        d.extend(to_semi(c.lat).to_le_bytes());
        d.extend(to_semi(c.lon).to_le_bytes());
        d.extend(m.saturating_mul(100).to_le_bytes());
        d.push(c.kind);
    }
    d.push(3);
    d.extend(t1.to_le_bytes());
    d.extend([0, 9]);
    let mut f = vec![14, 0x20, 0x5c, 0x08];
    f.extend((d.len() as u32).to_le_bytes());
    f.extend(b".FIT");
    f.extend(crc(0, &f).to_le_bytes());
    f.extend(d);
    f.extend(crc(0, &f).to_le_bytes());
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loop of `n` points around a centre, climbing 500 m over its first half (4.5 % for 11 km).
    fn gpx_loop(n: usize, ele: bool, extra: &str) -> String {
        let mut s =
            String::from("<?xml version=\"1.0\"?><gpx version=\"1.1\"><metadata><name>Loop de test!</name></metadata>");
        s += extra;
        s += "<trk><trkseg>";
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let (lat, lon) = (48.85 + 0.03 * a.sin(), 2.29 + 0.045 * a.cos());
            let e = 100.0 + 250.0 * (1.0 - a.cos());
            if ele {
                s += &format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{e:.1}</ele></trkpt>");
            } else {
                s += &format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"/>");
            }
        }
        s + "</trkseg></trk></gpx>"
    }

    #[test]
    fn a_gpx_becomes_a_course_the_units_reader_follows() {
        let wpt = "<wpt lat=\"48.88\" lon=\"2.29\"><name>Left</name></wpt>";
        let r = import_route(gpx_loop(400, true, wpt).into_bytes(), "file".into()).unwrap();
        assert_eq!(r.name, "Loop de test");
        assert_eq!(r.points.len(), 400);
        // a loop of 0.03 deg x 0.045 deg radii: about 22 km
        assert!((20_000..25_000).contains(&r.distance_m), "{}", r.distance_m);
        assert!(
            (490..=510).contains(&r.ascent_m) && (490..=510).contains(&r.descent_m),
            "{} {}",
            r.ascent_m,
            r.descent_m
        );
        assert_eq!(r.cue_count, 1);
        assert!(!r.climbs.is_empty() && r.climbs[0].grade_x10 >= 30, "{:?}", r.climbs);
        // it reads back whole, and what it reads is what the view says
        let (pts, cues) = read_back(&r.fit).unwrap();
        assert_eq!((pts.len(), cues.len()), (400, 1));
        assert_eq!(cues[0].kind, 6);
        let b = route_from_fit(r.fit.clone(), r.name.clone()).unwrap();
        assert_eq!((b.distance_m, b.ascent_m, b.points.len()), (r.distance_m, r.ascent_m, 400));
        // semicircles are finer than degrees x 1e-7: the second trip may move a point by a hair, not a metre
        assert!(b.distance_m.abs_diff(r.distance_m) <= 1 && b.ascent_m.abs_diff(r.ascent_m) <= 1);
        assert_eq!(import_route(gpx_loop(400, true, "").into_bytes(), "x".into()).unwrap().fit, {
            import_route(gpx_loop(400, true, "").into_bytes(), "y".into()).unwrap().fit
        });
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::write(dir.join("route.fit"), &r.fit).unwrap();
    }

    #[test]
    fn the_demo_route_has_two_climbs_and_four_cues() {
        let r = import_route(demo_route_gpx(), "x".into()).unwrap();
        assert_eq!((r.name.as_str(), r.points.len(), r.cue_count), ("Eiffel loop", 361, 4));
        assert!((20_000..30_000).contains(&r.distance_m), "{}", r.distance_m);
        assert_eq!(r.climbs.len(), 2, "{:?}", r.climbs);
    }

    /// The phone finds the course it sent in the unit's list by its check (the fake unit lists as the real one).
    #[test]
    fn a_sent_course_is_found_in_the_list_by_its_check() {
        use crate::files::{FileClient, FileEvent, UnitFolder};
        let (c, unit) = (FileClient::new(), crate::demo::DemoUnit::new());
        let r = import_route(demo_route_gpx(), "x".into()).unwrap();
        let feed = |cmd: Vec<u8>| -> Vec<FileEvent> { unit.handle(cmd).into_iter().map(|f| c.on_data(f, 1)).collect() };
        feed(c.course_write_command(r.name.clone(), r.fit.clone(), 244, 1).unwrap());
        while let Some(cmd) = c.write_next_command(244, 1) {
            feed(cmd);
        }
        assert!(feed(c.list_in_command(UnitFolder::Courses, 1)).contains(&FileEvent::Listed));
        let size = u32::try_from(r.fit.len()).unwrap();
        assert_eq!(
            (c.files_in(UnitFolder::Courses)[0].size, c.files_in_checks(UnitFolder::Courses)[0]),
            (size, course_check(r.name, size))
        );
    }

    #[test]
    fn a_route_without_elevation_has_no_climbs() {
        let r = import_route(gpx_loop(50, false, "").into_bytes(), "x".into()).unwrap();
        assert!(!r.has_alt && r.climbs.is_empty() && r.ascent_m == 0);
        assert!(r.points.iter().all(|p| p.alt_m.is_none()));
    }

    #[test]
    fn too_many_points_are_thinned_to_the_units_limit() {
        let r = import_route(gpx_loop(20_000, true, "").into_bytes(), "x".into()).unwrap();
        assert!(r.thinned && r.points.len() <= POINTS && r.points.len() > 3000, "{}", r.points.len());
        assert!((20_000..25_000).contains(&r.distance_m));
        assert!(read_back(&r.fit).is_some());
    }

    #[test]
    fn refusals_are_in_words() {
        for bad in ["", "<gpx/>", "<gpx><trk><trkseg><trkpt lat=\"1\" lon=\"1\"/></trkseg></trk></gpx>"] {
            assert!(import_route(bad.as_bytes().to_vec(), "x".into()).is_err());
        }
        assert!(route_from_fit(vec![1, 2, 3], "x".into()).is_err());
    }

    /// Writes an encoded route for `tools/fit_check.py --course` (fitdecode and Garmin's SDK).
    #[test]
    fn the_encoded_route_is_written_for_fit_check() {
        let extra = "<wpt lat=\"48.88\" lon=\"2.29\"><type>Right</type></wpt><wpt lat=\"48.85\" lon=\"2.335\"><sym>Water</sym></wpt>";
        let r = import_route(gpx_loop(600, true, extra).into_bytes(), "x".into()).unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::write(dir.join("course.fit"), &r.fit).unwrap();
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    /// `FUZZ_SCALE` as the firmware's fuzz reads it (the quick gate sets 0.25).
    fn scale() -> f64 {
        std::env::var("FUZZ_SCALE").ok().and_then(|v| v.parse().ok()).filter(|s| *s > 0.0 && *s <= 1.0).unwrap_or(1.0)
    }

    /// A random GPX of tracks, routes and waypoints with bad numbers, junk and missing `ele`; every 20th seed has a
    /// file with more points than the unit keeps.
    fn random_gpx(seed: u64) -> String {
        let mut g = Lcg(seed + 1);
        let mut s = String::from("<gpx>");
        if g.next(3) == 0 {
            s += "<name>x</name><junk><a><b>";
        }
        for _ in 0..1 + g.next(3) {
            s += ["<trk><trkseg>", "<rte>", "<trk><name>t</name><trkseg>"][g.next(3) as usize];
            let n = if seed.is_multiple_of(20) { 15_000 } else { [3, 20, 120][g.next(3) as usize] + g.next(30) };
            for _ in 0..n {
                let (la, lo) = match g.next(12) {
                    0 => ("abc".to_string(), "1".to_string()),
                    1 => ("1e5".to_string(), "2".to_string()),
                    2 => ("99999".to_string(), "2".to_string()),
                    3 => ("-0.0".to_string(), "180".to_string()),
                    _ => (
                        format!("{}.{:05}", 40 + g.next(10), g.next(99999)),
                        format!("{}.{:05}", 1 + g.next(5), g.next(99999)),
                    ),
                };
                let ele = match g.next(6) {
                    0 => String::new(),
                    1 => "<ele>NaN</ele>".into(),
                    2 => "<ele></ele>".into(),
                    3 => format!("<ele>{}</ele>", g.next(99_999_999_999)),
                    _ => format!("<ele>{}.{}</ele>", g.next(2000), g.next(9)),
                };
                let cue = if g.next(40) == 0 { "<type>left</type>" } else { "" };
                let junk = if g.next(30) == 0 { "<extensions><x:y><z/></x:y></extensions>" } else { "" };
                let tag = ["trkpt", "rtept", "wpt"][g.next(3) as usize];
                s += &format!("<{tag} lat=\"{la}\" lon=\"{lo}\">{ele}{cue}{junk}</{tag}>");
            }
            s += ["</trkseg></trk>", "</rte>", ""][g.next(3) as usize];
        }
        s
    }

    /// Structured GPX, valid and not, in UTF-8, with a BOM and in UTF-16: nothing panics, an accepted one reads back
    /// whole within the unit's limits, and the seeds reach the encoder, the cues and the thinning (counts), or the
    /// test would prove nothing about them.
    #[test]
    fn random_gpx_never_panics_and_accepted_ones_encode() {
        let (mut ok, mut cued, mut thinned, mut noted, mut utf16) = (0, 0, 0, 0, 0);
        let mut why = std::collections::BTreeMap::new();
        for seed in 0..(120.0 * scale()) as u64 {
            let s = random_gpx(seed);
            let bytes = match seed % 5 {
                1 => [&[0xef, 0xbb, 0xbf][..], s.as_bytes()].concat(),
                2 => [&[0xff, 0xfe][..], &s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>()].concat(),
                3 => [&[0xfe, 0xff][..], &s.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<_>>()].concat(),
                _ => s.into_bytes(),
            };
            let got = import_route(bytes, "f".into());
            if let Err(e) = &got {
                *why.entry(e.to_string()).or_insert(0) += 1;
            }
            if let Ok(r) = got {
                ok += 1;
                utf16 += usize::from(seed % 5 >= 2);
                cued += usize::from(r.cue_count > 0);
                thinned += usize::from(r.thinned);
                noted += usize::from(!r.notes.is_empty());
                assert!(r.points.len() >= 2 && r.points.len() <= POINTS && r.fit.len() <= COURSE_MAX as usize);
                assert!(read_back(&r.fit).is_some());
                assert!(r.points.windows(2).all(|w| w[0].dist_m <= w[1].dist_m));
            }
        }
        assert!(ok > 25 && utf16 > 0 && noted > 0, "{ok} {utf16} {noted} {why:?}");
        assert!(scale() < 0.5 || (cued > 0 && thinned > 0), "{cued} {thinned}");
    }

    /// A valid course, truncated, mutated and with coordinates out of range: refused or read within bounds.
    #[test]
    fn route_from_fit_survives_damaged_files() {
        let wpt = "<wpt lat=\"48.88\" lon=\"2.29\"><name>Left</name></wpt>";
        let good = import_route(gpx_loop(300, true, wpt).into_bytes(), "x".into()).unwrap().fit;
        let (mut ok, mut refused) = (0, 0);
        let check = |fit: Vec<u8>, ok: &mut u32, refused: &mut u32| match route_from_fit(fit, "f".into()) {
            Ok(r) => {
                *ok += 1;
                assert!(r.points.iter().all(|p| p.lat.abs() <= 90.0 && p.lon.abs() <= 180.0));
                assert!(r.points.len() >= 2 && read_back(&r.fit).is_some());
            }
            Err(_) => *refused += 1,
        };
        for seed in 0..(400.0 * scale()) as u64 {
            let mut g = Lcg(seed + 7);
            let mut f = good.clone();
            match g.next(3) {
                0 => f.truncate(g.next(f.len() as u64) as usize),
                1 => {
                    for _ in 0..1 + g.next(4) {
                        let i = g.next(f.len() as u64) as usize;
                        f[i] ^= 1 << g.next(8);
                    }
                }
                _ => {
                    // a latitude set out of range, the CRC made right again: only the range is wrong
                    let at = 14 + 13 + 5 + 24 + 30 + 70 + g.next(200) as usize;
                    if let Some(w) = f.get_mut(at..at + 4) {
                        w.copy_from_slice(&0x7000_0000u32.to_le_bytes());
                    }
                    let n = f.len();
                    let c = crc(0, &f[..n - 2]).to_le_bytes();
                    f[n - 2..].copy_from_slice(&c);
                }
            }
            check(f, &mut ok, &mut refused);
        }
        check(good, &mut ok, &mut refused);
        assert!(ok > 0 && refused > 0, "{ok} {refused}");
    }

    /// Names with Unicode, entities, CDATA and unterminated elements: None or a name the unit takes.
    #[test]
    fn names_from_odd_text_are_the_units_or_none() {
        let pieces = [
            "é",
            "Ç",
            "œ",
            "ß",
            "'",
            "’",
            "日本",
            "&amp;",
            "&#233;",
            "&#x41;",
            "&bogus;",
            "<![CDATA[x]]>",
            "  ",
            "_",
            "-",
            "a",
            "Z",
            "9",
            "CON",
            "\u{0}",
            "😀",
        ];
        let mut g = Lcg(5);
        for _ in 0..(2000.0 * scale()) as usize {
            let text: String = (0..g.next(14)).map(|_| pieces[g.next(pieces.len() as u64) as usize]).collect();
            let closed = format!("<gpx><metadata><name>{text}</name></metadata></gpx>");
            let open = format!("<gpx><metadata><name>{text}");
            for t in [closed, open] {
                let n = gpx_name(t.as_bytes()).and_then(route_name);
                for n in [n, route_name(text.clone())].into_iter().flatten() {
                    assert!(CourseName::new(n.as_bytes()).is_some(), "{n:?}");
                }
            }
        }
    }

    #[test]
    fn a_huge_file_is_refused_or_read_without_trouble() {
        assert!(import_route(vec![b'<'; READ_MAX + 1], "x".into()).is_err());
        let r = import_route(gpx_loop(90_000, true, "").into_bytes(), "x".into()).unwrap();
        assert!(r.thinned && r.points.len() <= POINTS);
    }

    #[test]
    fn a_track_and_a_route_of_the_same_road_are_read_once() {
        let a = gpx_loop(300, true, "");
        let body = a.split("<trk>").nth(1).unwrap().split("</trk>").next().unwrap();
        let rte = body.replace("trkseg", "x").replace("trkpt", "rtept");
        let both = a.replace("</gpx>", &format!("<rte>{rte}</rte></gpx>"));
        let one = import_route(a.into_bytes(), "x".into()).unwrap();
        let two = import_route(both.into_bytes(), "x".into()).unwrap();
        assert_eq!((two.points.len(), two.distance_m), (one.points.len(), one.distance_m));
        assert!(two.notes.is_empty(), "the same road is no news: {:?}", two.notes);
        // the group with more points wins, and it is said when they are two different places
        let pts = |c: &str, n: usize, lat: f64| {
            (0..n).map(|k| format!("<{c} lat=\"{}\" lon=\"5.0\"/>", lat + 0.01 * k as f64)).collect::<String>()
        };
        let s = format!(
            "<gpx><trk><trkseg>{}</trkseg></trk><rte>{}</rte></gpx>",
            pts("trkpt", 2, 48.0),
            pts("rtept", 4, 43.0)
        );
        let r = import_route(s.into_bytes(), "x".into()).unwrap();
        assert_eq!((r.points.len(), r.notes.len()), (4, 1));
    }

    #[test]
    fn separate_tracks_keep_the_first_and_say_so() {
        let seg = |lat: f64| {
            let b = lat + 0.01;
            format!("<trk><trkseg><trkpt lat=\"{lat}\" lon=\"2.0\"/><trkpt lat=\"{b}\" lon=\"2.0\"/></trkseg></trk>")
        };
        let r = import_route(format!("<gpx>{}{}</gpx>", seg(48.0), seg(49.0)).into_bytes(), "x".into()).unwrap();
        assert_eq!((r.points.len(), r.notes.len()), (2, 1));
        // a gap under 1 km inside one file is the same ride
        let near = import_route(format!("<gpx>{}{}</gpx>", seg(48.0), seg(48.0125)).into_bytes(), "x".into()).unwrap();
        assert_eq!((near.points.len(), near.notes.len()), (4, 0));
    }

    #[test]
    fn names_fold_accents_and_come_from_the_right_element() {
        assert_eq!(route_name("été".into()).as_deref(), Some("ete"));
        assert_eq!(route_name("Côte d'Azur".into()).as_deref(), Some("Cote dAzur"));
        assert_eq!(route_name("Œuvre Straße Ça".into()).as_deref(), Some("Oeuvre Strasse Ca"));
        assert_eq!(route_name("Col de la Croix-Haute (2)".into()).as_deref(), Some("Col de la Croix-Haut"));
        assert_eq!(route_name("CON".into()), None);
        assert_eq!(route_name("!!!".into()), None);
        let gpx = "<gpx><wpt lat=\"1\" lon=\"1\"><name>Cafe</name></wpt><metadata><author><name>Me</name></author><name>Tour &amp; Cie &#233;t&#xE9;</name></metadata><trk><name>Trk</name></trk></gpx>";
        assert_eq!(gpx_name(gpx.as_bytes()).as_deref(), Some("Tour & Cie été"));
        let no_meta = "<gpx><wpt lat=\"1\" lon=\"1\"><name>Cafe</name></wpt><trk><name><![CDATA[Col du Galibier]]></name><trkseg/></trk></gpx>";
        assert_eq!(gpx_name(no_meta.as_bytes()).as_deref(), Some("Col du Galibier"));
        assert_eq!(gpx_name(b"<gpx><wpt><name>Cafe</name></wpt><trk><trkseg/></trk></gpx>"), None);
        assert_eq!(gpx_name(b"<gpx><metadata><name>open"), None);
    }

    #[test]
    fn a_cue_on_an_out_and_back_keeps_its_own_pass() {
        // out along a line and back over it, a turn word on the same place on each pass
        let pt = |k: i32, w: &str| format!("<trkpt lat=\"{}\" lon=\"2.0\">{w}</trkpt>", 48.0 + 0.001 * f64::from(k));
        let mut s = String::from("<gpx><trk><trkseg>");
        for k in 0..=10 {
            s += &pt(k, if k == 5 { "<type>left</type>" } else { "" });
        }
        for k in (0..10).rev() {
            s += &pt(k, if k == 5 { "<type>right</type>" } else { "" });
        }
        let r = import_route((s + "</trkseg></trk></gpx>").into_bytes(), "x".into()).unwrap();
        let (_, cues) = read_back(&r.fit).unwrap();
        let d: Vec<u32> = cues.iter().filter_map(|c| c.dist_m).collect();
        assert_eq!(cues.iter().map(|c| c.kind).collect::<Vec<_>>(), [6, 7]);
        assert!(d[1] > d[0] + 100, "the way back is a second pass: {d:?}");
    }

    #[test]
    fn clashes_match_the_units_menu() {
        assert!(route_clash("Col_du Lac".into(), "col du lac".into()));
        assert!(!route_clash("Col du Lac".into(), "Col du Lad".into()));
    }
}
