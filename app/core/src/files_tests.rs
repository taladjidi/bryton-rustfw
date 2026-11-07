//! Tests of the file client against the fake unit, which answers with the firmware's own encoder.

use crate::demo::{FakeUnit, synthetic_ride};
use crate::files::*;

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
    let mut u = FakeUnit::new(vec![(*b"261101090000", ride.clone())]);
    u.faults.chunk = chunk;
    let c = FileClient::new();
    let mut t = 0;
    let cmd = c.list_command(t);
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Listed);
    assert_eq!(c.entries(), vec![RideEntry { name: "261101090000".into(), size: ride.len() as u32 }]);
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
    for chunk in [1, 7, 18, 1000] {
        let (c, mut u, ride) = setup(chunk);
        let mut t = 100;
        let cmd = c.read_command("261101090000".into(), t).unwrap();
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
    let (c, mut u, ride) = setup(18);
    let mut t = 100;
    u.faults.drop_after = Some(1000);
    let cmd = c.read_command("261101090000".into(), t).unwrap();
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
    let (c, mut u, ride) = setup(18);
    let mut t = 100;
    u.faults.drop_after = Some(500);
    let cmd = c.read_command("261101090000".into(), t).unwrap();
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
    let (c, mut u, _) = setup(18);
    let mut t = 100;
    u.faults.short_by = 5;
    let cmd = c.read_command("261101090000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::Short });
    assert!(c.file().is_empty());
    u.faults.short_by = 0;
    u.faults.count_off = 1;
    let cmd = c.read_command("261101090000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::CountMismatch });
}

#[test]
fn errors_of_the_unit_and_unknown_rides() {
    let (c, mut u, _) = setup(18);
    let mut t = 100;
    assert_eq!(c.read_command("000000000000".into(), t), None);
    u.rides.clear(); // deleted since LIST
    let cmd = c.read_command("261101090000".into(), t).unwrap();
    assert_eq!(pump(&c, &mut u, &cmd, &mut t), FileEvent::Failed { error: FileError::NotFound });
    assert_eq!(c.resume_command(t), None);
}

#[test]
fn unknown_stale_and_surplus_frames_are_ignored() {
    let (c, mut u, ride) = setup(18);
    let t = 100;
    let cmd = c.read_command("261101090000".into(), t).unwrap();
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
    let (c, _, ride) = setup(18);
    let cmd = c.read_command("261101090000".into(), 0).unwrap();
    let big = vec![0u8; ride.len() + 1];
    let mut last = FileEvent::Ignored;
    for ch in big.chunks(18) {
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
        let (c, mut u, _) = setup(18);
        let mut t = 0u64;
        let mut cmd = c.read_command("261101090000".into(), t).unwrap();
        for _ in 0..200 {
            t += next() % 3000;
            let (r, seq) = (next(), cmd[1]);
            let f: Vec<u8> = match r % 7 {
                0 => (0..(r >> 8) % 24).map(|_| next() as u8).collect(),
                1 => [&[0x81, seq][..], b"123456789012", &[5, 0, 0, 0]].concat(),
                2 => {
                    let mut f = vec![0x82, seq];
                    f.extend((0..1 + (r >> 8) % 18).map(|_| next() as u8));
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
                _ => (0..18).map(|_| next() as u8).collect(),
            };
            match c.on_data(f, t) {
                FileEvent::Entry => entry += 1,
                FileEvent::Listed => listed += 1,
                FileEvent::Progress => progress += 1,
                FileEvent::Finished => done += 1,
                FileEvent::Busy => busy += 1,
                FileEvent::Failed { .. } => fail += 1,
                FileEvent::Ignored => {}
            }
            let _ = (c.progress(), c.file(), c.entries());
        }
    }
    let all = format!("{entry} {listed} {progress} {done} {fail} {busy}");
    assert!(entry > 0 && progress > 0 && fail > 0 && busy > 0, "{all}");
}
