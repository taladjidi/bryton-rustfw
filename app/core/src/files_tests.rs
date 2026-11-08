//! Tests of the file client against the fake unit, which answers with the firmware's own encoder.

use crate::demo::BIG_CHUNK;
use crate::demo::{FakeUnit, synthetic_ride};
use crate::files::*;
use bryton_rustfw::filesvc::{FRAME_BIG, FRAME_MAX};

/// Writes `cmd` to the unit and feeds its frames back, 10 ms apart; the last event that is not Progress.
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

fn setup(chunk: usize) -> (FileClient, FakeUnit, Vec<u8>) {
    let ride = synthetic_ride(9, 300);
    let mut u = FakeUnit::new(vec![(*b"261101100000", ride.clone())]);
    u.faults.chunk = chunk;
    let c = FileClient::new();
    let mut t = 0;
    let cmd = c.list_command(t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    assert_eq!(c.entries(), vec![RideEntry { name: "261101100000".into(), size: ride.len() as u32 }]);
    (c, u, ride)
}

#[test]
fn the_synthetic_ride_is_a_fit_file() {
    let r = synthetic_ride(9, 60);
    assert!(r.len() > 100);
    assert_eq!(&r[8..12], b".FIT");
}

#[test]
fn whole_read_with_any_chunk_size() {
    for chunk in [1, 7, 18, BIG_CHUNK, 1000] {
        let (c, mut u, ride) = setup(chunk);
        let mut t = 100;
        let cmd = c.read_command("261101100000".into(), t).unwrap();
        assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
        assert_eq!(c.file(), ride);
        let p = c.progress();
        assert_eq!((p.received, p.total), (ride.len() as u32, ride.len() as u32));
        assert!(p.bytes_per_s > 0);
        assert_eq!(c.resume_command(t), None, "a finished file is not asked again");
    }
}

#[test]
fn drop_then_resume_from_the_bytes_kept() {
    let (c, mut u, ride) = setup(BIG_CHUNK);
    let mut t = 100;
    u.faults.drop_after = Some(1000);
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Ignored);
    assert!(c.silent(t + SILENCE_MS) && !c.silent(t + SILENCE_MS - 20));
    assert_eq!(c.progress().received, 1000);
    let cmd = c.resume_command(t + SILENCE_MS).unwrap();
    assert_eq!(&cmd[2..6], &1000u32.to_le_bytes(), "READ from the bytes kept");
    t += SILENCE_MS;
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), ride);
}

#[test]
fn busy_mid_read_then_resume() {
    let (c, mut u, ride) = setup(BIG_CHUNK);
    let mut t = 100;
    u.faults.drop_after = Some(500);
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    pump(&c, &mut u, &cmd, &mut t);
    u.faults.busy = true;
    let cmd = c.resume_command(t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Busy);
    assert_eq!(c.progress().received, 500, "the bytes are kept");
    u.faults.busy = false;
    let cmd = c.resume_command(t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), ride);
}

#[test]
fn short_file_and_wrong_count_are_errors() {
    let (c, mut u, _) = setup(BIG_CHUNK);
    let mut t = 100;
    u.faults.short_by = 5;
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    // the answer ended early but brought bytes: resumable, and a resume that brings none is a short file
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Partial);
    let cmd = c.resume_command(t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::Short });
    assert!(c.file().is_empty());
    u.faults.short_by = 0;
    u.faults.count_off = 1;
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::CountMismatch });
}

#[test]
fn an_early_end_resumes_to_a_whole_file() {
    let (c, mut u, ride) = setup(BIG_CHUNK);
    let mut t = 100;
    u.faults.short_by = 5;
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Partial);
    assert_eq!(c.progress().received as usize, ride.len() - 5);
    u.faults.short_by = 0;
    let cmd = c.resume_command(t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), ride);
}

#[test]
fn errors_of_the_unit_and_unknown_rides() {
    let (c, mut u, _) = setup(BIG_CHUNK);
    let mut t = 100;
    assert_eq!(c.read_command("000000000000".into(), t), None);
    u.rides.clear(); // deleted since LIST
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::NotFound });
    assert_eq!(c.resume_command(t), None);
}

#[test]
fn unknown_stale_and_surplus_frames_are_ignored() {
    let (c, mut u, ride) = setup(BIG_CHUNK);
    let t = 100;
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    let mut frames = u.handle(&cmd);
    assert_eq!(c.on_data(vec![0x55, 1, 2], t), FileEvent::Ignored);
    assert_eq!(c.on_data(vec![], t), FileEvent::Ignored);
    // a chunk of an earlier command (other seq)
    assert_eq!(c.on_data(vec![0x82, cmd[1].wrapping_add(9), 1, 2, 3], t), FileEvent::Ignored);
    assert_eq!(c.progress().received, 0);
    let end = frames.pop().unwrap();
    for f in frames {
        c.on_data(f, t);
    }
    assert_eq!(c.on_data(end.clone(), t), FileEvent::Finished);
    assert_eq!(c.on_data(end, t), FileEvent::Ignored, "an END after the file is whole");
    assert_eq!(c.file(), ride);
}

#[test]
fn more_bytes_than_listed_is_refused() {
    let (c, _, ride) = setup(BIG_CHUNK);
    let cmd = c.read_command("261101100000".into(), 0).unwrap();
    let big = vec![0u8; ride.len() + 1];
    let mut last = FileEvent::Ignored;
    for ch in big.chunks(BIG_CHUNK) {
        let mut f = vec![0x82, cmd[1]];
        f.extend_from_slice(ch);
        last = c.on_data(f, 1);
        if matches!(last, FileEvent::Failed { .. }) {
            break;
        }
    }
    assert_eq!(last, FileEvent::Failed { error: FileError::TooLong });
}

/// Random and well-formed frames over a client in every state: no panic, and the decode paths are reached.
#[test]
fn fuzz_frames_never_panic_and_reach_every_path() {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (mut progress, mut done, mut fail, mut busy, mut entry, mut listed) = (0, 0, 0, 0, 0, 0);
    for _ in 0..300 {
        let (c, mut u, _) = setup(BIG_CHUNK);
        let mut t = 0u64;
        let mut cmd = c.read_command("261101100000".into(), t).unwrap();
        for _ in 0..200 {
            t += next() % 3000;
            let (r, seq) = (next(), cmd[1]);
            let f: Vec<u8> = match r % 7 {
                0 => (0..(r >> 8) % (FRAME_MAX as u64 + 4)).map(|_| next() as u8).collect(),
                1 => [&[0x81, seq][..], b"123456789012", &[5, 0, 0, 0]].concat(),
                2 => {
                    let mut f = vec![0x82, seq];
                    f.extend((0..1 + (r >> 8) % BIG_CHUNK as u64).map(|_| next() as u8));
                    f
                }
                3 => vec![0x83, seq, (r >> 8) as u8 % 6, (r >> 16) as u8, (r >> 24) as u8 % 2, 0, 0],
                4 => {
                    cmd = c.resume_command(t).unwrap_or(cmd.clone());
                    let _ = c.silent(t);
                    u.handle(&cmd).into_iter().next().unwrap_or_default()
                }
                5 => {
                    cmd = c.list_command(t);
                    u.handle(&cmd).into_iter().next().unwrap_or_default()
                }
                _ => (0..FRAME_MAX).map(|_| next() as u8).collect(),
            };
            match c.on_data(f, t) {
                FileEvent::Entry => entry += 1,
                FileEvent::Listed => listed += 1,
                FileEvent::Progress => progress += 1,
                FileEvent::Finished => done += 1,
                FileEvent::Busy | FileEvent::Partial => busy += 1,
                FileEvent::Failed { .. } => fail += 1,
                _ => {}
            }
            let _ = (c.progress(), c.file(), c.entries());
        }
    }
    let all = format!("{entry} {listed} {progress} {done} {fail} {busy}");
    assert!(entry > 0 && progress > 0 && fail > 0 && busy > 0, "{all}");
}

#[test]
fn a_chunk_longer_than_a_frame_is_ignored() {
    let (c, _, _) = setup(BIG_CHUNK);
    let cmd = c.read_command("261101100000".into(), 0).unwrap();
    let mut f = vec![0x82, cmd[1]];
    f.extend(std::iter::repeat_n(7u8, FRAME_BIG - 1)); // FRAME_BIG + 1 bytes in all (the largest MTU less 3)
    assert_eq!(c.on_data(f, 1), FileEvent::Ignored);
    assert_eq!(c.progress().received, 0);
}

#[test]
fn workouts_are_listed_and_read_in_their_folder_beside_the_rides() {
    let mut u = FakeUnit::demo();
    let c = FileClient::new();
    let mut t = 0;
    let cmd = c.list_command(t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    let cmd = c.list_in_command(UnitFolder::Workout, t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    assert_eq!(c.entries().len(), 3, "the rides stay listed");
    let files = c.files_in();
    assert_eq!(files.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["2026110800", "2026110900", "2026111100"]);
    let cmd = c.read_in_command("2026110900".into(), t).expect("listed");
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), u.workouts[1].1);
    assert!(crate::workout::decode_workout(c.file(), 250, 190).is_some());
    assert_eq!(c.read_in_command("2026110901".into(), t), None, "not in the list");
    // a dropped READ_IN resumes as a READ_IN, from the bytes kept
    u.faults.drop_after = Some(100);
    let cmd = c.read_in_command("2026110800".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Ignored);
    let cmd = c.resume_command(t).unwrap();
    assert_eq!(cmd[0], 5, "READ_IN");
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
    assert_eq!(c.file(), u.workouts[0].1);
    // BUSY ends a folder list as it ends a ride list
    u.faults.busy = true;
    let cmd = c.list_in_command(UnitFolder::Workout, t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Busy);
    // and a ride still downloads after a workout was read
    u.faults.busy = false;
    let cmd = c.list_command(t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    assert_eq!(cmd[0], 2, "READ");
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Finished);
}

#[test]
fn a_chunk_that_arrives_after_a_cancel_is_ignored() {
    let (c, mut u, ride) = setup(20);
    let mut t = 1000;
    let cmd = c.read_command("261101100000".into(), t).unwrap();
    let frames = u.handle(&cmd);
    for f in frames.iter().take(3) {
        t += 10;
        assert_ne!(c.on_data(f.clone(), t), FileEvent::Ignored);
    }
    c.cancel_command();
    assert!(frames.len() > 10 && ride.len() > 100);
    // the rest was already in flight: nothing counts, nothing finishes, nothing can be resumed
    for f in frames.iter().skip(3) {
        t += 10;
        assert_eq!(c.on_data(f.clone(), t), FileEvent::Ignored);
    }
    assert!(c.file().is_empty() && c.resume_command(t).is_none());
}
