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
    /// The unit already held this file for that day before this app sent it (a rider's own copy): it is never deleted.
    pub pre: bool,
}

/// A fetched event that became a file: its record as it will be stored, and the bytes to send.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Planned {
    /// The record.
    pub record: Sent,
    /// The unit's workout file.
    pub bytes: Vec<u8>,
}

/// One change of the unit's calendar: send `bytes` for `record`, or (no bytes) delete the file of `record`.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarJob {
    /// The record sent, or the one to delete.
    pub record: Sent,
    /// The file to send; None for a delete.
    pub bytes: Option<Vec<u8>>,
    /// A delete that waits for this event's send: if the send fails, the old file stays.
    pub after: Option<i64>,
    /// A delete of an event that left the calendar (to be confirmed by a second read before it runs).
    pub gone: bool,
}

/// What to do on the unit after a calendar read.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Settle {
    /// Sends first, then deletes.
    pub jobs: Vec<CalendarJob>,
    /// Records whose file is right but whose stamp changed: store these instead of the old ones.
    pub refresh: Vec<Sent>,
    /// Records to drop with nothing to do on the unit (a rider's own file, never deleted).
    pub forget: Vec<Sent>,
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
/// became a file, `refused` the ids the unit cannot hold.
/// - A fetched event with no record of the same file and day is sent, and its other records (an older file or day) are
///   deleted only after that send succeeded (`after`), so a move or edit never loses both files.
/// - One whose file is the same only gets its stamp refreshed.
/// - A record for a day from `today` on whose event left the window or was deleted (`gone`), or became unsendable, is
///   deleted from the unit; earlier days are never touched.
/// - A record with `pre` is a file the rider had: it is forgotten, never deleted.
///
/// Shortcut: two events with the same file on one day share it on the unit (it dedupes by CRC), so deleting one
/// takes the other's; rare, and the next change of that event sends it again.
#[uniffi::export]
pub fn calendar_settle(
    sent: Vec<Sent>,
    events: Vec<i64>,
    converted: Vec<Planned>,
    refused: Vec<i64>,
    today: String,
) -> Settle {
    let mut out = Settle { jobs: Vec::new(), refresh: Vec::new(), forget: Vec::new() };
    let mut deletes = Vec::new();
    let mut stale = |s: &Sent, after: Option<i64>, gone: bool, out: &mut Settle| {
        if s.day < today {
            return;
        }
        if s.pre {
            out.forget.push(s.clone());
        } else {
            deletes.push(CalendarJob { record: s.clone(), bytes: None, after, gone });
        }
    };
    let mut sends = Vec::new();
    for c in &converted {
        let id = c.record.event_id;
        let mine: Vec<&Sent> = sent.iter().filter(|s| s.event_id == id).collect();
        let same = mine.iter().find(|s| s.day == c.record.day && s.hash == c.record.hash);
        match same {
            Some(s) => out.refresh.push(Sent { pre: s.pre, ..c.record.clone() }),
            None => sends.push(CalendarJob {
                record: c.record.clone(),
                bytes: Some(c.bytes.clone()),
                after: None,
                gone: false,
            }),
        }
        for old in mine.iter().filter(|s| !(s.day == c.record.day && s.hash == c.record.hash)) {
            stale(old, same.is_none().then_some(id), false, &mut out);
        }
    }
    for s in &sent {
        let handled = converted.iter().any(|c| c.record.event_id == s.event_id);
        let listed = events.contains(&s.event_id);
        if !handled && (!listed || refused.contains(&s.event_id)) {
            stale(s, None, !listed, &mut out);
        }
    }
    out.jobs = sends;
    out.jobs.extend(deletes);
    out
}

/// Keeps the "gone" deletes only when a second read of the calendar agrees: not when either list is empty while
/// records exist (a short or failed answer must not wipe the unit), and not for an event that reappeared.
/// `again` is None when the second read failed. Other jobs pass.
#[uniffi::export]
pub fn calendar_keep_gone(
    jobs: Vec<CalendarJob>,
    first: Vec<i64>,
    again: Option<Vec<i64>>,
    records: u32,
) -> Vec<CalendarJob> {
    let trusted = |l: &[i64]| !l.is_empty() || records == 0;
    let agree = again.as_ref().is_some_and(|a| trusted(a) && trusted(&first));
    jobs.into_iter()
        .filter(|j| !j.gone || (agree && !again.as_ref().is_some_and(|a| a.contains(&j.record.event_id))))
        .collect()
}

/// Why a calendar job failed on the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum JobError {
    /// NOT FOUND.
    NotFound,
    /// MISMATCH: the unit received other bytes, or the list changed since the key was read.
    Mismatch,
    /// BUSY: a ride, or the card lent.
    Busy,
    /// The link dropped or the unit went quiet.
    Link,
    /// No `\Workout\calendar` on the unit.
    NoCalendar,
    /// A build that does not write the card.
    ReadOnly,
    /// Anything else (day full, bad request, card): this job cannot be done now.
    Refused,
}

/// What to do after a job failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum JobNext {
    /// The job is done (a delete of a file that is not there).
    Done,
    /// Read the list again and run the same job once more.
    Retry,
    /// Only this job is dropped; the others run. A send is shown "not sent" with the reason.
    Drop,
    /// The unit or the link is not available now: stop the pass, ask again soon.
    Stop,
    /// The unit cannot take calendar writes (no calendar, read only): stop, and ask again in an hour.
    StopHard,
}

/// The rule for a failed job. Only NOT FOUND finishes a delete; a MISMATCH is the list or bytes being off, so it is
/// tried once more after a new list (`retried`), then dropped.
#[uniffi::export]
pub fn calendar_job_error(is_send: bool, e: JobError, retried: bool) -> JobNext {
    match e {
        JobError::NotFound if !is_send => JobNext::Done,
        JobError::Mismatch if !retried => JobNext::Retry,
        JobError::Busy | JobError::Link => JobNext::Stop,
        JobError::NoCalendar | JobError::ReadOnly => JobNext::StopHard,
        _ => JobNext::Drop,
    }
}

/// The queue after `failed` ended with `next`: everything for Stop and StopHard; for Drop the deletes that waited for
/// that event's send (the old file stays); the same queue otherwise.
#[uniffi::export]
pub fn calendar_after(queue: Vec<CalendarJob>, failed: CalendarJob, next: JobNext) -> Vec<CalendarJob> {
    match next {
        JobNext::Stop | JobNext::StopHard => Vec::new(),
        JobNext::Drop if failed.bytes.is_some() => {
            queue.into_iter().filter(|j| j.after != Some(failed.record.event_id)).collect()
        }
        _ => queue,
    }
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

/// The multipart/form-data body of an upload: one field `file`, the name cut to letters and digits plus `.fit`.
#[uniffi::export]
pub fn upload_body(boundary: String, name: String, bytes: Vec<u8>) -> Vec<u8> {
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).take(40).collect();
    let mut out = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}.fit\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(&bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
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
        Sent { event_id: id, day: day.into(), updated: upd.into(), hash, pre: false }
    }

    fn plan(r: Sent) -> Planned {
        Planned { record: r, bytes: vec![1, 2, 3] }
    }

    fn settle(sent: &[Sent], events: &[i64], conv: Vec<Planned>, refused: &[i64], today: &str) -> Settle {
        calendar_settle(sent.to_vec(), events.to_vec(), conv, refused.to_vec(), today.into())
    }

    const STAMP: &str = "2026-11-08T10:00:00.000+00:00";

    #[test]
    fn a_rerun_sends_nothing_and_changes_are_followed() {
        let ev = parse("20261108", "20261115");
        let sent = vec![rec(1, "20261109", STAMP, 11)];
        assert!(calendar_to_fetch(sent.clone(), ev.clone()).is_empty());
        let s = settle(&sent, &[1], vec![], &[], "20261108");
        assert_eq!((s.jobs.len(), s.refresh.len(), s.forget.len()), (0, 0, 0));
        // restamped, same file: only the record changes
        let re = rec(1, "20261109", "new", 11);
        let restamped = CalendarEvent { updated: "new".into(), ..ev[0].clone() };
        assert_eq!(calendar_to_fetch(sent.clone(), vec![restamped]).len(), 1);
        let s = settle(&sent, &[1], vec![plan(re.clone())], &[], "20261108");
        assert_eq!((s.jobs.len(), s.refresh), (0, vec![re]));
        // deleted on Intervals, left the window, or no longer holdable: deleted from the unit, but not a past day
        let s = settle(&sent, &[], vec![], &[], "20261108");
        assert_eq!(s.jobs, vec![CalendarJob { record: sent[0].clone(), bytes: None, after: None, gone: true }]);
        let s = settle(&sent, &[1], vec![], &[1], "20261108");
        assert_eq!(s.jobs.len(), 1);
        assert!(!s.jobs[0].gone, "an event that is still listed needs no second read");
        assert!(settle(&sent, &[], vec![], &[], "20261110").jobs.is_empty());
    }

    #[test]
    fn a_move_sends_first_and_deletes_the_old_file_only_after() {
        let old = rec(1, "20261109", STAMP, 11);
        for new in [rec(1, "20261110", "new", 11), rec(1, "20261109", "new", 12)] {
            let s = settle(std::slice::from_ref(&old), &[1], vec![plan(new.clone())], &[], "20261108");
            assert_eq!(s.jobs.len(), 2);
            assert_eq!((s.jobs[0].record.clone(), s.jobs[0].bytes.is_some()), (new.clone(), true));
            assert_eq!(
                (s.jobs[1].record.clone(), s.jobs[1].bytes.clone(), s.jobs[1].after),
                (old.clone(), None, Some(1))
            );
            // the send failed alone: the delete that waited for it goes, the rest of the queue stays
            let mut q = s.jobs.clone();
            q.push(CalendarJob { record: rec(2, "20261109", "x", 5), bytes: None, after: None, gone: false });
            let after = calendar_after(q[1..].to_vec(), q[0].clone(), JobNext::Drop);
            assert_eq!(after.len(), 1);
            assert_eq!(after[0].record.event_id, 2);
            // a stop empties it
            assert!(calendar_after(q[1..].to_vec(), q[0].clone(), JobNext::Stop).is_empty());
            assert!(calendar_after(q[1..].to_vec(), q[0].clone(), JobNext::StopHard).is_empty());
        }
        // both records of an event (an earlier delete failed): the one that matches is kept, the other deleted
        let (a, b) = (rec(1, "20261109", STAMP, 11), rec(1, "20261110", STAMP, 12));
        let s = settle(&[a.clone(), b.clone()], &[1], vec![plan(rec(1, "20261110", "new", 12))], &[], "20261108");
        assert_eq!(s.refresh.len(), 1);
        assert_eq!(s.jobs.iter().map(|j| j.record.clone()).collect::<Vec<_>>(), vec![a]);
        assert_eq!(s.jobs[0].after, None);
    }

    #[test]
    fn a_riders_own_file_is_never_deleted() {
        let mine = Sent { pre: true, ..rec(1, "20261109", STAMP, 11) };
        let s = settle(std::slice::from_ref(&mine), &[], vec![], &[], "20261108");
        assert_eq!((s.jobs.len(), s.forget), (0, vec![mine.clone()]));
        // a move leaves the rider's old copy alone too, and a refresh keeps the flag
        let s = settle(std::slice::from_ref(&mine), &[1], vec![plan(rec(1, "20261110", "n", 11))], &[], "20261108");
        assert_eq!((s.jobs.len(), s.forget.len()), (1, 1));
        let s = settle(std::slice::from_ref(&mine), &[1], vec![plan(rec(1, "20261109", "n", 11))], &[], "20261108");
        assert!(s.refresh[0].pre);
    }

    #[test]
    fn failures_by_kind() {
        use JobError::*;
        // a delete is done only for NOT FOUND; MISMATCH reads the list again once, then is dropped
        assert_eq!(calendar_job_error(false, NotFound, false), JobNext::Done);
        assert_eq!(calendar_job_error(false, Mismatch, false), JobNext::Retry);
        assert_eq!(calendar_job_error(false, Mismatch, true), JobNext::Drop);
        assert_eq!(calendar_job_error(true, Mismatch, false), JobNext::Retry);
        // a send that fails for its own reasons drops only itself
        assert_eq!(calendar_job_error(true, Refused, false), JobNext::Drop);
        assert_eq!(calendar_job_error(true, NotFound, false), JobNext::Drop);
        // the unit or the link is out: stop; no calendar or read only: stop for longer
        for send in [true, false] {
            assert_eq!(calendar_job_error(send, Busy, false), JobNext::Stop);
            assert_eq!(calendar_job_error(send, Link, true), JobNext::Stop);
            assert_eq!(calendar_job_error(send, NoCalendar, false), JobNext::StopHard);
            assert_eq!(calendar_job_error(send, ReadOnly, false), JobNext::StopHard);
        }
        // a retry and a done leave the queue as it is
        let j = CalendarJob { record: rec(1, "20261109", "u", 1), bytes: None, after: None, gone: false };
        assert_eq!(calendar_after(vec![j.clone()], j.clone(), JobNext::Retry).len(), 1);
        assert_eq!(calendar_after(vec![j.clone()], j, JobNext::Done).len(), 1);
    }

    #[test]
    fn a_short_calendar_deletes_nothing() {
        let gone = CalendarJob { record: rec(1, "20261109", "u", 1), bytes: None, after: None, gone: true };
        let other = CalendarJob { gone: false, ..gone.clone() };
        let keep = |first: &[i64], again: Option<&[i64]>, records| {
            calendar_keep_gone(vec![gone.clone(), other.clone()], first.to_vec(), again.map(|a| a.to_vec()), records)
                .len()
        };
        assert_eq!(keep(&[2], Some(&[2]), 3), 2, "two reads agree: the delete runs");
        assert_eq!(keep(&[2], Some(&[1, 2]), 3), 1, "the event is back: not deleted");
        assert_eq!(keep(&[2], None, 3), 1, "the second read failed");
        assert_eq!(keep(&[], Some(&[]), 3), 1, "an empty calendar with records is not believed");
        assert_eq!(keep(&[2], Some(&[]), 3), 1);
        assert_eq!(keep(&[], Some(&[]), 0), 2, "no records, nothing at risk");
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
    fn the_upload_body_is_a_multipart_file() {
        let b = upload_body("XB".into(), "26110\r\n9\"0700".into(), vec![0, 255, b'\r', b'\n']);
        let head = "--XB\r\nContent-Disposition: form-data; name=\"file\"; filename=\"2611090700.fit\"\r\n\
                    Content-Type: application/octet-stream\r\n\r\n";
        assert!(b.starts_with(head.as_bytes()), "{:?}", String::from_utf8_lossy(&b));
        let tail = [&[0u8, 255, b'\r', b'\n'][..], b"\r\n--XB--\r\n"].concat();
        assert!(b.ends_with(&tail));
        assert_eq!(b.len(), head.len() + tail.len());
    }

    #[test]
    fn fuzz_events_settle_zwo_and_uploads_never_panic() {
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
            let pre = |n: u64| n.is_multiple_of(5);
            let sent: Vec<Sent> = ev
                .iter()
                .step_by(2)
                .map(|e| Sent { pre: pre(next()), ..rec(e.id, &e.day, "old", (next() % 3) as u32) })
                .collect();
            let conv: Vec<Planned> =
                ev.iter().map(|e| plan(rec(e.id, &e.day, &e.updated, (next() % 3) as u32))).collect();
            assert!(calendar_to_fetch(sent.clone(), ev.clone()).len() <= ev.len());
            let s = settle(&sent, &ev.iter().map(|e| e.id).collect::<Vec<_>>(), conv, &[], "20261105");
            sends += s.jobs.iter().filter(|j| j.bytes.is_some()).count();
            // every delete that waits for a send has that send in the queue, before it
            for (i, j) in s.jobs.iter().enumerate().filter(|(_, j)| j.after.is_some()) {
                assert!(s.jobs[..i].iter().any(|k| k.bytes.is_some() && Some(k.record.event_id) == j.after));
            }
            // a rider's own file is never a delete job
            assert!(s.jobs.iter().all(|j| j.bytes.is_some() || !sent.iter().any(|o| o.pre && *o == j.record)));
        }
        assert!(kept > 1000 && dropped > 100 && sends > 100, "{kept} {dropped} {sends}");

        // untrusted ZWO: tags and attributes from a structured set, extreme values, cut short sometimes
        let shapes: [(&str, &[&str]); 8] = [
            ("Warmup", &["Duration", "PowerLow", "PowerHigh"]),
            ("Cooldown", &["Duration", "PowerLow", "PowerHigh"]),
            ("SteadyState", &["Duration", "Power"]),
            ("IntervalsT", &["Repeat", "OnDuration", "OnPower", "OffDuration", "OffPower"]),
            ("Ramp", &["Duration", "PowerLow", "PowerHigh"]),
            ("FreeRide", &["Duration"]),
            ("MaxEffort", &["Duration"]),
            ("Bogus", &["Duration"]),
        ];
        let good = ["60", "0.5", "1.2", "0.8", "120", "300", "600", "3", "0.0", "1"];
        let bad = ["-1", "1e30", "NaN", "x", "99999999"];
        let (mut ok, mut err) = (0, 0);
        for _ in 0..3000 {
            let mut body = String::new();
            for _ in 0..(next() % 8) {
                let (t, at) = shapes[(next() % 8) as usize];
                body += &format!("<{t}");
                for a in at {
                    let v = if next() % 12 == 0 { bad[(next() % 5) as usize] } else { good[(next() % 10) as usize] };
                    body += &format!(" {a}=\"{v}\"");
                }
                body += "/>";
            }
            let mut z =
                format!("<workout_file><name>N\u{e9}{}</name><workout>{body}</workout></workout_file>", next() % 9)
                    .into_bytes();
            if next() % 8 == 0 {
                let n = (next() % (z.len() as u64 + 1)) as usize;
                z.truncate(n);
            }
            match plan_workout(z, "Event".into()) {
                Ok(f) => {
                    ok += 1;
                    assert!(f.bytes.len() <= 8192 && crate::workout::decode_workout(f.bytes, 250, 190).is_some());
                }
                Err(_) => err += 1,
            }
        }
        assert!(ok > 100 && err > 100, "{ok} {err}");

        // upload answers: structured JSON, statuses around 200
        let idv = ["\"i1\"", "7", "null", "[]", "\"../x\"", "\"\"", "\"abc123\"", "{}"];
        let (mut with_id, mut without) = (0, 0);
        for _ in 0..2000 {
            let (a, b) = (idv[(next() % 8) as usize], idv[(next() % 8) as usize]);
            let mut j = format!("{{\"id\":{a},\"activities\":[{{\"id\":{b}}},{{\"id\":{a}}}]}}").into_bytes();
            if next() % 6 == 0 {
                let n = (next() % (j.len() as u64 + 1)) as usize;
                j.truncate(n);
            }
            let st = [200u16, 201, 202, 400, 429, 500][(next() % 6) as usize];
            match upload_outcome(st, j) {
                Some(u) => {
                    assert!(st == 200 || st == 201);
                    if u.activity_id.is_empty() {
                        without += 1;
                    } else {
                        assert!(u.activity_id.chars().all(|c| c.is_ascii_alphanumeric()));
                        with_id += 1;
                    }
                }
                None => assert!(st != 200 && st != 201),
            }
        }
        assert!(with_id > 100 && without > 100, "{with_id} {without}");
    }
}
