//! Fault-injection fuzz of a ride download, phone side: the file client against the fake unit over a link that loses,
//! doubles, delays and cuts notifications, and a driver that reacts as the app does (RideBrowser.swift `data()`:
//! Partial resumes at once, Busy resumes after a wait, silence resumes, Failed stops). It guards the 2026-11-10 bug
//! (a ride saved with 342 bytes missing and 342 doubled: an answer ended unchecked and the resume read on from its
//! bytes) and the fixes (unchecked bytes dropped on resume, END count asked again `REDO_MAX` times, whole-file CRC).
//!
//! The faults are applied to the frames of each answer, after the fake unit made them with the firmware's encoder:
//! a lost or doubled notification, an answer cut silently (its tail sometimes arriving late, with the old seq), cut
//! by BUSY with the true count, a stale frame of an older seq, a loss plus a double of equal length in one answer (the
//! count matches, only the file's CRC sees it). The unit answers either to the end (as the firmware does today) or in
//! windows (END OK with the count every N bytes, as a planned firmware change would).

use crate::demo::{BIG_CHUNK, FakeUnit, synthetic_ride};
use crate::files::*;
use bryton_rustfw::filesvc::{FRAME_BIG, Frame, Status};

/// The fuzz scale, as `src/sim/fuzz.rs`: `FUZZ_SCALE` in the environment (default 1; `./check.sh --quick` sets 0.25).
fn fuzz_scale() -> f32 {
    std::env::var("FUZZ_SCALE").ok().and_then(|v| v.parse().ok()).filter(|s| *s > 0.0 && *s <= 1.0).unwrap_or(1.0)
}

/// xorshift32: deterministic, so a failing seed reproduces.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n.max(1)
    }
    /// True once in `n` (never for 0).
    fn one_in(&mut self, n: usize) -> bool {
        n > 0 && self.below(n) == 0
    }
}

/// What a seed does to the link.
struct Params {
    /// Chunk payload bytes.
    chunk: usize,
    /// The unit ends an answer after about this many bytes (None: it streams to the file's end).
    window: Option<usize>,
    /// A frame is lost, and one doubled, once in this many (0: a clean link).
    rate: usize,
}

/// An answer's frames as the client receives them.
struct Delivered {
    /// Encoded frames, each with the bytes it brings to the current answer (0 for a stale or END frame).
    frames: Vec<(Vec<u8>, usize)>,
    /// The re-read budget this answer adds: what a fault in it may legitimately make the client read again.
    budget: usize,
}

fn enc(f: Frame) -> Vec<u8> {
    let mut b = [0; FRAME_BIG];
    let n = f.encode(&mut b);
    b.get(..n).unwrap_or(&[]).to_vec()
}

/// The faulty link applied to the unit's answer `raw` to a READ; `late` holds frames of earlier answers still on the
/// way (delivered first, with their old seq). `size` is the file's, for the budget.
fn link(rng: &mut Rng, p: &Params, raw: &[Vec<u8>], late: &mut Vec<Vec<u8>>, size: usize) -> Delivered {
    let (mut seq, mut chunks): (u8, Vec<Vec<u8>>) = (0, Vec::new());
    for f in raw {
        match Frame::parse(f) {
            Some(Frame::Chunk { seq: s, data }) => {
                seq = s;
                chunks.push(data.to_vec());
            }
            Some(Frame::End { seq: s, .. }) => seq = s,
            _ => {}
        }
    }
    // the unit's own answer: whole, or one window; END OK with the bytes it sent
    if let Some(w) = p.window {
        let mut sum = 0;
        let n = chunks
            .iter()
            .take_while(|c| {
                let fits = sum == 0 || sum + c.len() <= w;
                if fits {
                    sum += c.len();
                }
                fits
            })
            .count();
        chunks.truncate(n);
    }
    let sent: usize = chunks.iter().map(Vec::len).sum();
    let cap = p.window.unwrap_or(size) + BIG_CHUNK;
    let mut out: Vec<(Vec<u8>, usize)> = late.drain(..).map(|f| (f, 0)).collect();
    let mut end = Some((Status::Ok, sent));
    let mut budget = 0;
    if p.rate > 0 {
        // cut silently (maybe the tail arrives late), or by BUSY with the true count
        let k = rng.below(chunks.len() + 1);
        match rng.below(6) {
            0 => {
                if rng.one_in(2) {
                    *late = chunks.get(k..).unwrap_or(&[]).iter().map(|d| enc(Frame::Chunk { seq, data: d })).collect();
                }
                chunks.truncate(k);
                end = None;
                budget += cap;
            }
            1 => {
                let kept: usize = chunks.iter().take(k).map(Vec::len).sum();
                chunks.truncate(k);
                end = Some((Status::Busy, kept));
            }
            _ => {}
        }
        let (mut lost, mut doubled) = (0, 0);
        let mut seen = Vec::new();
        // a loss and a double of equal length in one answer: the count matches
        let swap = rng.one_in(6) && chunks.len() > 4;
        let (a, b) = (rng.below(chunks.len()), rng.below(chunks.len()));
        for (i, c) in chunks.iter().enumerate() {
            let lose = (swap && i == a) || rng.one_in(p.rate);
            let double = (swap && i == b && i != a) || rng.one_in(p.rate);
            if lose {
                lost += c.len();
            } else {
                seen.push(c.clone());
                if double {
                    doubled += c.len();
                    seen.push(c.clone());
                }
            }
        }
        chunks = seen;
        if lost > 0 || doubled > 0 {
            // a hole or a double that only the whole-file CRC finds costs the whole file once; the count, one answer
            budget += if lost == doubled { size } else { cap };
        }
    }
    for c in &chunks {
        out.push((enc(Frame::Chunk { seq, data: c }), c.len()));
    }
    if let Some((status, count)) = end {
        out.push((enc(Frame::End { seq, status, count: u32::try_from(count).unwrap_or(0) }), 0));
    }
    if p.rate > 0 && rng.one_in(3) {
        let at = rng.below(out.len() + 1);
        let old = seq.wrapping_sub(1);
        let stale = if rng.one_in(3) {
            enc(Frame::End { seq: old, status: Status::Ok, count: 0 })
        } else {
            enc(Frame::Chunk { seq: old, data: &[0xEE; 18] })
        };
        out.insert(at, (stale, 0));
    }
    Delivered { frames: out, budget }
}

/// What the seeds reached and what they cost.
#[derive(Default)]
struct Tally {
    count_caught: u32,
    crc_caught: u32,
    silence: u32,
    busy: u32,
    redo: u32,
    finished: u32,
    failed: u32,
    too_long: u32,
    /// Bytes read again by finished downloads: (windowed, bytes).
    reread: Vec<(bool, usize)>,
}

const NAME: &str = "261101100000";
/// Answers a seed may take before the test calls it a loop.
const STEPS: usize = 2000;

/// One download of one seed, driven as the app does. Asserts the invariants.
fn download(seed: u32, rides: &[Vec<u8>], t: &mut Tally) {
    let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
    for _ in 0..4 {
        rng.next();
    }
    let p = Params {
        chunk: [18, 100, BIG_CHUNK][(seed / 10 % 3) as usize],
        window: (seed % 2 == 1).then(|| [600, 1500, 3000][(seed / 2 % 3) as usize]),
        rate: [0, 300, 100, 30, 10][(seed / 6 % 5) as usize],
    };
    let ride = &rides[(seed / 30 % 3) as usize];
    let mut unit = FakeUnit::new(vec![(*b"261101100000", ride.clone())]);
    unit.faults.chunk = p.chunk;
    let c = FileClient::new();
    let mut now = 0u64;
    let cmd = c.list_command(now);
    for f in unit.handle(&cmd) {
        c.on_data(f, now);
    }
    let size = ride.len();
    let (mut late, mut budget, mut delivered) = (Vec::new(), 0, 0);
    let mut cmd = c.read_command(NAME.into(), now).expect("listed");
    for _ in 0..STEPS {
        let d = link(&mut rng, &p, &unit.handle(&cmd), &mut late, size);
        budget += d.budget;
        let mut terminal = None;
        for (f, bytes) in d.frames {
            now += 10;
            let before = c.progress().received;
            let e = c.on_data(f, now);
            let after = c.progress().received;
            assert!(after as usize <= size, "seed {seed}: received {after} over the size {size}");
            if terminal.is_none() {
                delivered += bytes;
            }
            if e == FileEvent::Partial && after < before {
                t.redo += 1;
                if before as usize == size {
                    t.crc_caught += 1;
                } else {
                    t.count_caught += 1;
                }
            }
            if matches!(e, FileEvent::Partial | FileEvent::Busy | FileEvent::Finished | FileEvent::Failed { .. }) {
                terminal.get_or_insert(e);
            }
        }
        match terminal {
            Some(FileEvent::Finished) => {
                assert!(c.file() == *ride, "seed {seed}: Finished with other bytes");
                t.finished += 1;
                let reread = delivered.saturating_sub(size);
                t.reread.push((p.window.is_some(), reread));
                if p.window.is_some() {
                    assert!(reread <= budget, "seed {seed}: {reread} bytes read again, a fault explains {budget}");
                }
                return;
            }
            Some(FileEvent::Failed { error }) => {
                assert!(p.rate > 0, "seed {seed}: a clean link failed: {error:?}");
                (t.failed, t.too_long) = (t.failed + 1, t.too_long + u32::from(error == FileError::TooLong));
                assert!(c.file().is_empty(), "seed {seed}: bytes given out by a failed download");
                return;
            }
            Some(FileEvent::Busy) => {
                now += 3000;
                t.busy += 1;
            }
            Some(_) => {}
            None => {
                now += SILENCE_MS;
                assert!(c.silent(now), "seed {seed}: a cut answer is not silence");
                t.silence += 1;
            }
        }
        cmd = c.resume_command(now).expect("a download under way resumes");
    }
    panic!("seed {seed}: no end after {STEPS} answers");
}

#[test]
fn a_ride_survives_a_faulty_link_or_fails_whole() {
    let rides: Vec<Vec<u8>> = [20, 60, 150].iter().map(|s| synthetic_ride(9, *s)).collect();
    let seeds = ((600.0 * fuzz_scale()) as u32).max(1);
    std::println!("FUZZ_SCALE {}", fuzz_scale());
    let mut t = Tally::default();
    for seed in 1..=seeds {
        download(seed, &rides, &mut t);
    }
    let (mut one, mut win): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    for (w, n) in &t.reread {
        if *w {
            win.push(*n);
        } else {
            one.push(*n);
        }
    }
    let line =
        |v: &[usize]| format!("mean {} max {}", v.iter().sum::<usize>() / v.len().max(1), v.iter().max().unwrap_or(&0));
    std::println!(
        "files_fuzz: {seeds} seeds, finished {} failed {} (too long {}), count {} crc {} redo {} silence {} busy {}; \
         bytes read again: one answer {}, windowed {}",
        t.finished,
        t.failed,
        t.too_long,
        t.count_caught,
        t.crc_caught,
        t.redo,
        t.silence,
        t.busy,
        line(&one),
        line(&win)
    );
    if seeds >= 100 {
        for (n, v) in [
            ("count", t.count_caught),
            ("crc", t.crc_caught),
            ("silence", t.silence),
            ("busy", t.busy),
            ("redo", t.redo),
            ("finished", t.finished),
            ("failed", t.failed),
        ] {
            assert!(v > 0, "the fuzz never reached {n}");
        }
    }
}
