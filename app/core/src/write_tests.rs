//! Tests of sending and deleting a workout through the file client against the fake unit.

use crate::demo::FakeUnit;
use crate::files::*;

/// Writes `cmd` to the unit and feeds its frames back; the last event that is not Progress.
fn pump(c: &FileClient, u: &mut FakeUnit, cmd: &[u8], t: &mut u64) -> FileEvent {
    let mut last = FileEvent::Ignored;
    for f in u.handle(cmd) {
        *t += 10;
        let e = c.on_data(f, *t);
        if e != FileEvent::Progress {
            last = e;
        }
    }
    last
}

/// Sends `bytes` for `day` with Control writes of at most `max_write` bytes; the last event and the data frames sent.
fn send(c: &FileClient, u: &mut FakeUnit, day: &str, bytes: &[u8], max_write: u32, t: &mut u64) -> (FileEvent, usize) {
    let cmd = c.write_command(day.into(), bytes.to_vec(), *t).expect("a valid write");
    let e = pump(c, u, &cmd, t);
    if e != FileEvent::WriteReady {
        return (e, 0);
    }
    let (mut last, mut frames) = (FileEvent::Ignored, 0);
    while let Some(cmd) = c.write_next_command(max_write, *t) {
        assert!(cmd.len() <= max_write as usize);
        frames += usize::from(cmd[0] == 7);
        last = pump(c, u, &cmd, t);
        if matches!(last, FileEvent::Failed { .. }) {
            break;
        }
    }
    (last, frames)
}

fn list_workouts(c: &FileClient, u: &mut FakeUnit, t: &mut u64) -> Vec<String> {
    let cmd = c.list_in_command(UnitFolder::Workout, *t);
    assert_eq!(pump(c, u, &cmd, t), FileEvent::Listed);
    c.files_in().into_iter().map(|e| e.name).collect()
}

#[test]
fn a_workout_round_trips_at_both_mtus_and_is_written_once() {
    let w = crate::workout::demo_workouts()[0].1.clone();
    for (max_write, per_frame) in [(20u32, 14usize), (244, 238)] {
        let mut u = FakeUnit::new(Vec::new());
        let (c, mut t) = (FileClient::new(), 0);
        let (e, frames) = send(&c, &mut u, "20261108", &w, max_write, &mut t);
        assert_eq!(e, FileEvent::Written);
        assert_eq!(frames, w.len().div_ceil(per_frame), "data split at {max_write}");
        assert_eq!(list_workouts(&c, &mut u, &mut t), ["2026110800"]);
        let cmd = c.read_in_command("2026110800".into(), t).unwrap();
        assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
        assert_eq!(c.file(), w, "byte identical");
        // the same file again (its answer lost): written once
        assert_eq!(send(&c, &mut u, "20261108", &w, max_write, &mut t).0, FileEvent::Written);
        assert_eq!(list_workouts(&c, &mut u, &mut t).len(), 1);
        let cmd = c.delete_command("2026110800".into(), t).unwrap();
        assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Deleted);
        assert!(list_workouts(&c, &mut u, &mut t).is_empty());
        assert_eq!(c.delete_command("2026110800".into(), t), None, "no longer listed");
    }
}

#[test]
fn each_refusal_is_its_error() {
    let w = crate::workout::demo_workouts()[0].1.clone();
    let fail = |error| FileEvent::Failed { error };
    let mut t = 0;
    let c = FileClient::new();
    let mut u = FakeUnit::demo();
    u.faults.read_only = true;
    assert_eq!(send(&c, &mut u, "20261108", &w, 20, &mut t).0, fail(FileError::ReadOnly));
    u.faults.read_only = false;
    u.faults.full = true;
    assert_eq!(send(&c, &mut u, "20261108", &w, 20, &mut t).0, fail(FileError::Full));
    u.faults.full = false;
    u.faults.no_calendar = true;
    assert_eq!(send(&c, &mut u, "20261108", &w, 20, &mut t).0, fail(FileError::NoCalendar));
    u.faults.no_calendar = false;
    u.faults.mismatch = true;
    assert_eq!(send(&c, &mut u, "20261108", &w, 20, &mut t).0, fail(FileError::Mismatch));
    u.faults.mismatch = false;
    u.faults.busy = true;
    assert_eq!(send(&c, &mut u, "20261108", &w, 20, &mut t).0, fail(FileError::Busy));
    assert_eq!(c.write_next_command(20, t), None, "a failed write hands out no data");
    // DELETE: busy, read only, and a key changed since the list
    u.faults.busy = false;
    let before = list_workouts(&c, &mut u, &mut t);
    u.faults.busy = true;
    let cmd = c.delete_command(before[0].clone(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), fail(FileError::Busy));
    u.faults.busy = false;
    u.faults.read_only = true;
    let cmd = c.delete_command(before[0].clone(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), fail(FileError::ReadOnly));
    u.faults.read_only = false;
    u.workouts.remove(0);
    let cmd = c.delete_command(before[0].clone(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), fail(FileError::NotFound));
    // bad arguments never reach the wire
    assert_eq!(c.write_command("2026-11-08".into(), w.clone(), t), None);
    assert_eq!(c.write_command("20261108".into(), Vec::new(), t), None);
    assert_eq!(c.write_command("20261108".into(), vec![0; 8193], t), None);
}

#[test]
fn a_write_awaiting_its_answer_goes_silent() {
    let (c, t) = (FileClient::new(), 100);
    c.write_command("20261108".into(), vec![1, 2, 3], t).unwrap();
    assert!(c.silent(t + SILENCE_MS) && !c.silent(t + 10));
    assert_eq!(c.progress().total, 3);
}
