//! More tests of the write client: dates, stale answers, refusals mid-write, the write length, and a fuzz.

use crate::demo::FakeUnit;
use crate::files::*;
use crate::write_tests::{list_workouts, pump};
use bryton_rustfw::filesvc::{FRAME_BIG, Frame, Status};

fn end_frame(seq: u8, status: Status, count: u32) -> Vec<u8> {
    let mut b = [0; FRAME_BIG];
    let n = Frame::End { seq, status, count }.encode(&mut b);
    b[..n].to_vec()
}

#[test]
fn a_bad_date_is_refused() {
    let c = FileClient::new();
    for day in ["20261308", "20260008", "20261100", "20261132", "2026110a", "202611", "261108000"] {
        assert_eq!(c.write_command(day.into(), vec![1], 0), None, "{day}");
    }
    assert!(c.write_command("20261231".into(), vec![1], 0).is_some());
}

#[test]
fn a_stale_end_is_ignored_even_across_the_seq_wrap() {
    let w = crate::workout::demo_workouts()[0].1.clone();
    let (c, mut u, mut t) = (FileClient::new(), FakeUnit::new(Vec::new()), 0);
    for _ in 0..255 {
        c.list_command(t); // an old LIST (seq now 255) whose END arrives late
    }
    let cmd = c.write_command("20261108".into(), w, t).unwrap();
    assert_eq!(cmd[1], 0, "the seq wrapped");
    assert_eq!(c.on_data(end_frame(255, Status::Busy, 0), t), FileEvent::Ignored);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::WriteReady);
    // an OK END with the right seq in Sending changes nothing
    assert_eq!(c.on_data(end_frame(0, Status::Ok, 0), t), FileEvent::Ignored);
    assert!(c.write_next_command(20, t).is_some());
}

#[test]
fn a_refused_data_frame_fails_the_write() {
    let w = crate::workout::demo_workouts()[0].1.clone();
    let (c, mut u, mut t) = (FileClient::new(), FakeUnit::new(Vec::new()), 0);
    u.faults.refuse_data = Some(Status::Busy);
    let cmd = c.write_command("20261108".into(), w, t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::WriteReady);
    let cmd = c.write_next_command(20, t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::Busy });
    assert_eq!(c.write_next_command(20, t), None);
}

#[test]
fn every_frame_fits_the_write_length() {
    let w = crate::workout::demo_workouts()[0].1.clone();
    for max_write in 0..=300u32 {
        let (c, mut u, mut t) = (FileClient::new(), FakeUnit::new(Vec::new()), 0);
        let cmd = c.write_command("20261108".into(), w.clone(), t).unwrap();
        assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::WriteReady);
        let limit = max_write.clamp(20, 244) as usize;
        while let Some(cmd) = c.write_next_command(max_write, t) {
            assert!(cmd.len() <= limit, "{max_write}: {}", cmd.len());
            pump(&c, &mut u, &cmd, &mut t);
        }
        assert_eq!(u.workouts.len(), 1, "written whole at {max_write}");
    }
}

#[test]
fn a_delete_needs_the_last_list_in() {
    let (c, mut u, mut t) = (FileClient::new(), FakeUnit::demo(), 0);
    assert!(!list_workouts(&c, &mut u, &mut t).is_empty());
    let cmd = c.list_in_command(UnitFolder::Workout, t); // a LIST_IN pending: its list is cleared
    assert_eq!(c.delete_command("2026110800".into(), t), None);
    pump(&c, &mut u, &cmd, &mut t);
    assert!(c.delete_command("2026110800".into(), t).is_some());
    c.list_command(t); // the rides' list under way
    assert_eq!(c.delete_command("2026110800".into(), t), None);
}

#[test]
fn abort_drops_late_answers() {
    let c = FileClient::new();
    let cmd = c.write_command("20261108".into(), vec![1, 2, 3], 0).unwrap();
    c.abort();
    assert_eq!(c.on_data(end_frame(cmd[1], Status::Ok, 0), 0), FileEvent::Ignored);
    assert_eq!(c.write_next_command(20, 0), None);
}

/// Writes, deletes and lists interleaved with random answers over a client in every state.
#[test]
fn fuzz_write_paths() {
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let w = crate::workout::demo_workouts()[0].1.clone();
    let (mut ready, mut written, mut deleted, mut failed) = (0, 0, 0, 0);
    let mut count = |e: FileEvent| match e {
        FileEvent::WriteReady => ready += 1,
        FileEvent::Written => written += 1,
        FileEvent::Deleted => deleted += 1,
        FileEvent::Failed { .. } => failed += 1,
        _ => {}
    };
    for _ in 0..300 {
        let (c, mut u, mut t) = (FileClient::new(), FakeUnit::demo(), 0u64);
        let mut seq = 0u8;
        for _ in 0..60 {
            t += next() % 2000;
            let r = next();
            let mut cmd = Vec::new();
            match r % 8 {
                0 => {
                    let day = match (r >> 8) % 3 {
                        0 => "20261108".to_string(),
                        1 => format!("2026{:02}{:02}", (r >> 16) % 14, (r >> 24) % 33),
                        _ => "2026110x".to_string(),
                    };
                    let len = (r >> 32) as usize % 9001;
                    let bytes = if (r >> 12) % 2 == 0 { w.clone() } else { vec![(r >> 40) as u8; len] };
                    cmd = c.write_command(day, bytes, t).unwrap_or_default();
                }
                1 | 2 => cmd = c.write_next_command((next() % 300) as u32, t).unwrap_or_default(),
                3 => {
                    let status = Status::from_u8((next() % 8) as u8).unwrap_or(Status::Ok);
                    let s = if next() % 2 == 0 { seq } else { next() as u8 };
                    count(c.on_data(end_frame(s, status, (next() % 3) as u32), t));
                }
                4 => cmd = c.list_in_command(UnitFolder::Workout, t),
                5 => cmd = c.read_in_command(format!("202611{:02}00", 8 + (next() % 4)), t).unwrap_or_default(),
                6 => {
                    let key = if next() % 2 == 0 { "2026110800" } else { "2026119900" };
                    cmd = c.delete_command(key.into(), t).unwrap_or_default();
                }
                _ => cmd = c.list_command(t),
            }
            u.faults.busy = next() % 16 == 0;
            u.faults.full = next() % 32 == 0;
            if cmd.len() > 1 {
                seq = cmd[1];
                for f in u.handle(&cmd) {
                    assert!(f.len() <= FRAME_BIG);
                    count(c.on_data(f, t));
                }
            }
            let _ = (c.progress(), c.silent(t));
        }
    }
    assert!(ready > 0 && written > 0 && deleted > 0 && failed > 0, "{ready} {written} {deleted} {failed}");
}

/// Sends course `bytes` as `name` in Control writes of at most `max_write` bytes: the last event.
fn send_course(c: &FileClient, u: &mut FakeUnit, name: &str, bytes: &[u8], max_write: u32, t: &mut u64) -> FileEvent {
    let cmd = c.course_write_command(name.into(), bytes.to_vec(), max_write, *t).expect("a valid course write");
    assert!(cmd.len() <= max_write as usize, "{} bytes in a {max_write}-byte write", cmd.len());
    let mut last = pump(c, u, &cmd, t);
    if last != FileEvent::WriteReady {
        return last;
    }
    while let Some(cmd) = c.write_next_command(max_write, *t) {
        last = pump(c, u, &cmd, t);
    }
    last
}

/// A course goes to folder 2 at either write length (its name cut to fit the 20-byte write), is listed, read back
/// and deleted; the same bytes again are written once, other bytes under the name refused; names the unit refuses,
/// an empty or too big file give no command.
#[test]
fn a_course_round_trips_at_both_write_lengths() {
    let course = crate::demo::synthetic_ride(9, 600); // any bytes: the fake unit does not parse them
    let (c, mut u, mut t) = (FileClient::new(), FakeUnit::new(Vec::new()), 0);
    assert_eq!(send_course(&c, &mut u, "Col du Galibier", &course, 20, &mut t), FileEvent::Written);
    assert_eq!(send_course(&c, &mut u, "Col du Galibier by the north", &course, 244, &mut t), FileEvent::Written);
    let names: Vec<_> = u.courses.iter().map(|e| e.0.clone()).collect();
    assert_eq!(names, ["Col du G.fit", "Col du Galibier by t.fit"], "cut to 8 at 20 bytes, to 20 at most");
    assert_eq!(send_course(&c, &mut u, "Col du G", &course, 20, &mut t), FileEvent::Written, "again: once");
    assert_eq!(u.courses.len(), 2);
    let other = [&course[..], &[0]].concat();
    let refused = send_course(&c, &mut u, "COL DU G", &other, 20, &mut t);
    assert_eq!(refused, FileEvent::Failed { error: FileError::BadRequest }, "another file of that name");
    let cmd = c.list_in_command(UnitFolder::Courses, t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    let keys: Vec<_> = c.files_in().into_iter().map(|e| e.name).collect();
    assert_eq!(keys, ["00", "01"]);
    let cmd = c.read_in_command("01".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), course);
    let cmd = c.delete_command("00".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Deleted);
    assert_eq!(u.courses.len(), 1);
    for bad in ["", " ", "../x", "a.fit", "CON", "caf\u{e9}", "a\tb"] {
        assert_eq!(c.course_write_command(bad.into(), course.clone(), 244, t), None, "{bad:?}");
    }
    assert_eq!(c.course_write_command("x".into(), Vec::new(), 244, t), None, "empty");
    assert_eq!(c.course_write_command("x".into(), vec![0; (1 << 20) + 1], 244, t), None, "over 1 MB");
}
