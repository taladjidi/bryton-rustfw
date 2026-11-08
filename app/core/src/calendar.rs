//! Intervals.icu both ways beyond the profile: the calendar's planned workouts (to the unit) and the upload answer.
//!
//! Sources (looked up 2026-11-08 in the OpenAPI spec https://intervals.icu/api/v1/docs):
//! - `GET /api/v1/athlete/{id}/events{format}` (summary "List events"): query `oldest`, `newest` (local dates
//!   `YYYY-MM-DD`), `category` (array). An `Event` has `id` (int32), `start_date_local`, `category` (WORKOUT, RACE_A, NOTE,
//!   ...), `name`, `updated`. The empty `{format}` giving the plain JSON list is inferred (the spec documents only
//!   `.csv` as the alternative; the forum's API guide https://forum.intervals.icu/t/api-access-to-intervals-icu/609 uses
//!   `/events?oldest=&newest=`).
//! - `GET /api/v1/athlete/{id}/events/{eventId}/download{ext}` ("Download a planned workout in zwo, mrc, erg or fit
//!   format"): the app asks for `.zwo` and imports it with [`crate::import`] then [`crate::draft::encode_workout`],
//!   so the unit's limits apply to what it sends. `ext` with its dot is inferred from the path template.
//! - `POST /api/v1/athlete/{id}/activities` (multipart form field `file`; "Activities are de-duped using a hash of the
//!   file. Response code is 201 if at least one new activity was created and 200 otherwise (all dups)"); its
//!   `UploadResponse` has `id` and `activities[].id` (strings). That a duplicate answer may carry no activity id is
//!   inferred.
//!
//! Nothing here touches the network: Swift fetches, this parses, selects and decides.

use crate::draft::{DraftError, encode_workout, invalid};
use crate::import::import_workout;

/// A planned workout of the calendar, as much as the app needs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarEvent {
    /// Intervals.icu's event id.
    pub id: i64,
    /// The local day, `YYYYMMDD` (the unit's folder name).
    pub day: String,
    /// Its name, up to 60 characters; "Workout" when missing.
    pub name: String,
    /// The event's `updated` stamp, as text; a changed workout has a new one.
    pub updated: String,
}

/// A workout this app put on the unit: the event, the day, the stamp it had, and the CRC-32 of the file sent (which
/// is also the unit's file name).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Sent {
    /// The event.
    pub event_id: i64,
    /// `YYYYMMDD`.
    pub day: String,
    /// The event's stamp when it was sent.
    pub updated: String,
    /// CRC-32 of the unit file.
    pub hash: u32,
}

/// What to do on the unit after a calendar read.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Settle {
    /// Event ids to send (their converted files are the caller's).
    pub send: Vec<i64>,
    /// Files this app sent that are no longer wanted (moved, changed, deleted): delete them from the unit.
    pub delete: Vec<Sent>,
    /// Records whose file is right but whose stamp changed: store these instead of the old ones.
    pub refresh: Vec<Sent>,
}

/// A ZWO converted to the unit's workout file.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PlannedFile {
    /// The FIT workout file.
    pub bytes: Vec<u8>,
    /// Its CRC-32.
    pub hash: u32,
}

/// `YYYYMMDD` from the start of a `YYYY-MM-DD...` stamp, or None when it is not a date.
fn day_of(stamp: &str) -> Option<String> {
    let b = stamp.as_bytes();
    let digits = [0, 1, 2, 3, 5, 6, 8, 9];
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' || !digits.iter().all(|&i| b[i].is_ascii_digit()) {
        return None;
    }
    // ASCII digits and dashes: the slices below are on character boundaries
    let (y, m, d) = (&stamp[..4], &stamp[5..7], &stamp[8..10]);
    let (mi, di) = (m.parse::<u32>().ok()?, d.parse::<u32>().ok()?);
    ((1..=12).contains(&mi) && (1..=31).contains(&di)).then(|| format!("{y}{m}{d}"))
}

/// The WORKOUT events of `GET .../events` whose local day is from `first` to `last` (`YYYYMMDD`, both included), in
/// the order given. An event with no integer id, no date or another category is left out. None when the body is not
/// a JSON array.
#[uniffi::export]
pub fn calendar_parse(json: Vec<u8>, first: String, last: String) -> Option<Vec<CalendarEvent>> {
    let v: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let mut out = Vec::new();
    for e in v.as_array()? {
        let id = e.get("id").and_then(|i| i.as_i64());
        let day = e.get("start_date_local").and_then(|s| s.as_str()).and_then(day_of);
        let (Some(id), Some(day)) = (id, day) else { continue };
        if e.get("category").and_then(|c| c.as_str()) != Some("WORKOUT") || day < first || day > last {
            continue;
        }
        let name: String = e.get("name").and_then(|n| n.as_str()).unwrap_or("").chars().take(60).collect();
        let updated = e.get("updated").and_then(|u| u.as_str()).unwrap_or("").chars().take(40).collect();
        out.push(CalendarEvent {
            id,
            day,
            name: if name.trim().is_empty() { "Workout".into() } else { name },
            updated,
        });
    }
    Some(out)
}

/// The events to download and convert: those with no record of the same id, day and stamp.
#[uniffi::export]
pub fn calendar_to_fetch(sent: Vec<Sent>, events: Vec<CalendarEvent>) -> Vec<CalendarEvent> {
    let same = |s: &Sent, e: &CalendarEvent| s.event_id == e.id && s.day == e.day && s.updated == e.updated;
    events.into_iter().filter(|e| !sent.iter().any(|s| same(s, e))).collect()
}

/// Decides the unit's side. `events` are the ids the calendar lists in the window, `converted` the fetched ones that
/// became a file (their `Sent` record, as it will be stored), `refused` the fetched ones the unit cannot hold.
/// A fetched event with a record of another file or day replaces it (delete, then send); one whose file is the same
/// only gets its stamp refreshed. A record for a day from `today` on whose event left the window, was deleted or
/// became unsendable is deleted from the unit; earlier days are never touched (a past workout stays where it is).
/// Shortcut: two events with the same file on one day share it on the unit (it dedupes by CRC), so deleting one
/// takes the other's; rare, and the next change of that event sends it again.
#[uniffi::export]
pub fn calendar_settle(
    sent: Vec<Sent>,
    events: Vec<i64>,
    converted: Vec<Sent>,
    refused: Vec<i64>,
    today: String,
) -> Settle {
    let mut out = Settle { send: Vec::new(), delete: Vec::new(), refresh: Vec::new() };
    for c in &converted {
        match sent.iter().find(|s| s.event_id == c.event_id) {
            Some(s) if s.day == c.day && s.hash == c.hash => out.refresh.push(c.clone()),
            old => {
                out.delete.extend(old.filter(|s| s.day >= today).cloned());
                out.send.push(c.event_id);
            }
        }
    }
    for s in &sent {
        let handled = converted.iter().any(|c| c.event_id == s.event_id);
        let gone = !events.contains(&s.event_id) || refused.contains(&s.event_id);
        if !handled && gone && s.day >= today {
            out.delete.push(s.clone());
        }
    }
    out
}

/// A planned workout's ZWO as the unit's file, or why the unit cannot hold it (in words).
#[uniffi::export]
pub fn plan_workout(zwo: Vec<u8>, fallback_name: String) -> Result<PlannedFile, DraftError> {
    let bytes = encode_workout(import_workout(zwo, fallback_name)?)?;
    if bytes.len() > 8192 {
        return invalid("The unit keeps workout files of 8 KB at most");
    }
    let hash = bryton_rustfw::inflate::crc32(bytes.iter().copied());
    Ok(PlannedFile { bytes, hash })
}

/// CRC-32 of a file, as the unit names it (to find a file this app sent in the unit's list).
#[uniffi::export]
pub fn file_crc(bytes: Vec<u8>) -> u32 {
    bryton_rustfw::inflate::crc32(bytes.into_iter())
}

/// What an upload's answer says.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Uploaded {
    /// The activity's id, empty when the answer gave none.
    pub activity_id: String,
    /// Intervals.icu already had the file (status 200: "all dups").
    pub duplicate: bool,
}

/// The answer of `POST .../activities`: Some for 201 (created) and 200 (a duplicate, which counts as uploaded), None
/// for anything else.
#[uniffi::export]
pub fn upload_outcome(status: u16, json: Vec<u8>) -> Option<Uploaded> {
    if status != 200 && status != 201 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&json).unwrap_or_default();
    let first = v.get("activities").and_then(|a| a.as_array()).and_then(|a| a.first());
    let id = match first.and_then(|a| a.get("id")) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    // the id goes into a URL: letters and digits only
    let ok = id.len() <= 20 && id.chars().all(|c| c.is_ascii_alphanumeric());
    Some(Uploaded { activity_id: if ok { id } else { String::new() }, duplicate: status == 200 })
}

#[cfg(test)]
mod tests {
    use super::*;

    // synthetic; the shape follows the spec's Event
    const EVENTS: &str = r#"[
      {"id":1,"start_date_local":"2026-11-09T00:00:00","category":"WORKOUT","name":"Sweet spot","updated":"2026-11-08T10:00:00.000+00:00"},
      {"id":2,"start_date_local":"2026-11-10T00:00:00","category":"NOTE","name":"A note"},
      {"id":3,"start_date_local":"2026-11-30T00:00:00","category":"WORKOUT","name":"Too far"},
      {"id":4,"start_date_local":"2026-11-07T00:00:00","category":"WORKOUT","name":"Past"},
      {"id":"5","start_date_local":"2026-11-09T00:00:00","category":"WORKOUT"},
      {"id":6,"start_date_local":"soon","category":"WORKOUT"},
      {"id":7,"start_date_local":"2026-11-16","category":"WORKOUT","name":""}, 8, null, {"id":9,"category":"WORKOUT"}]"#;

    fn parse(first: &str, last: &str) -> Vec<CalendarEvent> {
        calendar_parse(EVENTS.as_bytes().to_vec(), first.into(), last.into()).unwrap()
    }

    #[test]
    fn selects_workouts_in_the_window() {
        let e = parse("20261108", "20261115");
        assert_eq!(e.iter().map(|e| e.id).collect::<Vec<_>>(), [1]);
        assert_eq!((e[0].day.as_str(), e[0].name.as_str()), ("20261109", "Sweet spot"));
        // the window's ends are included; a bare date and an empty name work; the wrong types are dropped
        let all = parse("20261107", "20261116");
        assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 4, 7]);
        assert_eq!(all[2].name, "Workout");
        assert_eq!(calendar_parse(b"{}".to_vec(), "0".into(), "9".into()), None);
        assert_eq!(calendar_parse(b"".to_vec(), "0".into(), "9".into()), None);
    }

    fn rec(id: i64, day: &str, upd: &str, hash: u32) -> Sent {
        Sent { event_id: id, day: day.into(), updated: upd.into(), hash }
    }

    #[test]
    fn a_rerun_sends_nothing_and_changes_are_followed() {
        let ev = parse("20261108", "20261115");
        let sent = vec![rec(1, "20261109", "2026-11-08T10:00:00.000+00:00", 11)];
        assert!(calendar_to_fetch(sent.clone(), ev.clone()).is_empty());
        let s = calendar_settle(sent.clone(), vec![1], vec![], vec![], "20261108".into());
        assert_eq!((s.send.len(), s.delete.len(), s.refresh.len()), (0, 0, 0));
        // restamped, same file: only the record changes
        let re = rec(1, "20261109", "new", 11);
        let restamped = CalendarEvent { updated: "new".into(), ..ev[0].clone() };
        assert_eq!(calendar_to_fetch(sent.clone(), vec![restamped]).len(), 1);
        let s = calendar_settle(sent.clone(), vec![1], vec![re.clone()], vec![], "20261108".into());
        assert_eq!((s.send.len(), s.delete.len(), s.refresh), (0, 0, vec![re]));
        // moved or edited: the old file goes, the new one is sent
        for c in [rec(1, "20261110", "new", 11), rec(1, "20261109", "new", 12)] {
            let s = calendar_settle(sent.clone(), vec![1], vec![c], vec![], "20261108".into());
            assert_eq!((s.send, s.delete, s.refresh.len()), (vec![1], sent.clone(), 0));
        }
        // deleted on Intervals, left the window, or no longer holdable: deleted from the unit, but not a past day
        let s = calendar_settle(sent.clone(), vec![], vec![], vec![], "20261108".into());
        assert_eq!(s.delete, sent);
        let s = calendar_settle(sent.clone(), vec![1], vec![], vec![1], "20261108".into());
        assert_eq!(s.delete, sent);
        let s = calendar_settle(sent, vec![], vec![], vec![], "20261110".into());
        assert!(s.delete.is_empty());
    }

    // the shape of Intervals.icu's ZWO export (synthetic)
    const ZWO: &str = r#"<workout_file><author>Intervals.icu</author><name>Sweet spot 3x10 Ünï</name><sportType>bike</sportType>
      <workout><Warmup Duration="600" PowerLow="0.50" PowerHigh="0.75"/><SteadyState Duration="300" Power="0.60"/>
      <IntervalsT Repeat="3" OnDuration="600" OnPower="0.90" OffDuration="300" OffPower="0.50"/>
      <Cooldown Duration="600" PowerLow="0.60" PowerHigh="0.40"/></workout></workout_file>"#;

    #[test]
    fn a_zwo_becomes_a_unit_file_within_its_limits() {
        let f = plan_workout(ZWO.as_bytes().to_vec(), "Event".into()).unwrap();
        assert_eq!(f.hash, file_crc(f.bytes.clone()));
        let w = crate::workout::decode_workout(f.bytes.clone(), 250, 190).unwrap();
        assert!(w.name.len() <= 15 && w.name.is_ascii(), "{}", w.name);
        assert_eq!(w.total_s, 600 + 300 + 3 * 900 + 600);
        // the same input twice is the same file (the unit and the sent set dedupe on it)
        assert_eq!(plan_workout(ZWO.as_bytes().to_vec(), "Event".into()).unwrap(), f);
        // over the unit's steps: refused in words, not truncated
        let step = "<SteadyState Duration=\"10\" Power=\"0.5\"/>";
        let many = format!("<workout_file><workout>{}</workout></workout_file>", step.repeat(100));
        let e = plan_workout(many.into_bytes(), "Big".into()).unwrap_err();
        assert!(e.to_string().contains("64"), "{e}");
        assert!(plan_workout(b"not a workout".to_vec(), "x".into()).is_err());
    }

    #[test]
    fn upload_answers() {
        let created =
            br#"{"icu_athlete_id":"i1","id":"x","activities":[{"id":"i987","icu_athlete_id":"i1"}]}"#.to_vec();
        let new = Uploaded { activity_id: "i987".into(), duplicate: false };
        assert_eq!(upload_outcome(201, created.clone()), Some(new.clone()));
        // a duplicate answers 200, with or without an id; both count as uploaded
        assert_eq!(upload_outcome(200, created), Some(Uploaded { duplicate: true, ..new }));
        assert_eq!(upload_outcome(200, b"{}".to_vec()), Some(Uploaded { activity_id: String::new(), duplicate: true }));
        assert_eq!(upload_outcome(200, b"garbage".to_vec()).map(|u| u.activity_id), Some(String::new()));
        // an id that is not safe in a URL is dropped, other statuses are not an upload
        let evil = br#"{"activities":[{"id":"../x?y"}]}"#.to_vec();
        assert_eq!(upload_outcome(201, evil).map(|u| u.activity_id), Some(String::new()));
        for st in [0u16, 204, 302, 401, 403, 409, 422, 429, 500] {
            assert_eq!(upload_outcome(st, b"{}".to_vec()), None);
        }
    }

    #[test]
    fn fuzz_events_and_settle_never_panic() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let ids = ["1", "2", "3", "4", "5", "-3", "\"4\"", "null"];
        let days = [
            "\"2026-11-09T07:00:00\"",
            "\"2026-13-09\"",
            "\"2026-11-0x\"",
            "\"2026-11-12T00:00:00\"",
            "5",
            "null",
            "\"2026-11-10\"",
            "\"2026-11-3\u{e9}\"",
            "\"2026-11-11\"",
        ];
        let cats = ["\"WORKOUT\"", "\"WORKOUT\"", "\"WORKOUT\"", "\"NOTE\"", "null"];
        let (mut kept, mut dropped, mut sends) = (0, 0, 0);
        for _ in 0..3000 {
            let mut parts = Vec::new();
            for _ in 0..(next() % 6) {
                let (i, d, c) = (ids[(next() % 8) as usize], days[(next() % 9) as usize], cats[(next() % 5) as usize]);
                parts.push(format!(
                    "{{\"id\":{i},\"start_date_local\":{d},\"category\":{c},\"name\":{d},\"updated\":{i}}}"
                ));
            }
            let mut b = format!("[{}]", parts.join(",")).into_bytes();
            if next() % 6 == 0 {
                let n = (next() % (b.len() as u64 + 1)) as usize;
                b.truncate(n);
            }
            let Some(ev) = calendar_parse(b, "20261101".into(), "20261130".into()) else {
                dropped += 1;
                continue;
            };
            kept += ev.len();
            let sent: Vec<Sent> = ev.iter().step_by(2).map(|e| rec(e.id, &e.day, "old", (next() % 3) as u32)).collect();
            let conv: Vec<Sent> = ev.iter().map(|e| rec(e.id, &e.day, &e.updated, (next() % 3) as u32)).collect();
            assert!(calendar_to_fetch(sent.clone(), ev.clone()).len() <= ev.len());
            let s = calendar_settle(sent, ev.iter().map(|e| e.id).collect(), conv, vec![], "20261105".into());
            sends += s.send.len();
        }
        assert!(kept > 1000 && dropped > 100 && sends > 100, "{kept} {dropped} {sends}");
    }
}
