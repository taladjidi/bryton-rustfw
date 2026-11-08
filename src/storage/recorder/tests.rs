//! The recorder's tests: the card model, a power cut after every sector, the resume and the save.

extern crate std;
use super::*;
use crate::fat::Disk;
use crate::fat::write::tests::{base_stock, fill, free_counts, fsck, fsck_cut, fsck_torn, stock, stock_kept};
use crate::fit::RECORD_LEN;
use crate::fitread::{Msg, Reader};
use crate::ride::Profile;
use std::vec::Vec;

/// One test of a cut family: `cut_part!(name_part_3, power_cut(3))` is `#[test] fn name_part_3() { power_cut(3) }`.
/// The families have [`CUT_PARTS`] tests each, so the slowest were cut from 10 to 14 s to 2 to 5 s (the wall-clock
/// floor of `cargo test-host`); every cut still runs, in exactly one part.
macro_rules! cut_part {
    ($(#[$m:meta])* $name:ident, $body:expr) => {
        $(#[$m])*
        #[test]
        fn $name() {
            $body;
        }
    };
}

fn two_copies_cut(part: u64) {
    for torn in [false, true] {
        two_copies_cut_torn(torn, part);
    }
}

/// The card: the volume in memory, and a power cut after `budget` sectors written (the sector that would be
/// next and every one after it never lands, within a multi-sector write too); then it never answers again.
/// `torn`: the sector the cut falls in is left as garbage (a card without reliable write); with `prefix` Some(k)
/// instead, its first k bytes new and the rest old: a sector every entry of which a reader may accept (the review of
/// 2026-11-01, finding 3).
/// `fails_at` Some(n): the write that lands sector n (counted as `written`) lands whole and still returns an error,
/// once (a CRC or busy timeout after the card programmed it: finding 11).
struct Card {
    img: Vec<u8>,
    budget: Option<u64>,
    written: u64,
    dead: bool,
    torn: bool,
    prefix: Option<usize>,
    fails_at: Option<u64>,
    cut_at: u32,
    /// The ride second `ride` was at (the cut's, once the card died).
    now: u32,
    /// Sectors read.
    reads: u64,
}

impl Disk for Card {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        if self.dead {
            return Err(Error::Io);
        }
        self.reads += (buf.len() / SECTOR) as u64;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.img.get(lba as usize * SECTOR + i).copied().unwrap_or(0);
        }
        Ok(())
    }
}

impl DiskWrite for Card {
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
        for (k, s) in buf.chunks_exact(SECTOR).enumerate() {
            let at = (lba as usize + k) * SECTOR;
            if self.img.len() < at + SECTOR {
                self.img.resize(at + SECTOR, 0);
            }
            if self.dead || self.budget.is_some_and(|b| self.written >= b) {
                if !self.dead {
                    self.cut_at = (at / SECTOR) as u32;
                }
                match (self.torn && !self.dead, self.prefix) {
                    // on a 4-byte edge: a FAT entry's, and inside a directory entry too (its name new, its cluster
                    // old: a folder's entry on cluster 0, which the reader refuses as Corrupt, never the root)
                    (true, Some(k)) => self.img[at..at + k].copy_from_slice(&s[..k]),
                    (true, None) => {
                        // neither the old nor the new bytes: garbage
                        for (i, b) in self.img[at..at + SECTOR].iter_mut().enumerate() {
                            *b = (i as u8).wrapping_mul(97) ^ 0x5a ^ (at / SECTOR) as u8;
                        }
                    }
                    (false, _) => {}
                }
                self.dead = true;
                return Err(Error::Io);
            }
            self.img[at..at + SECTOR].copy_from_slice(s);
            self.written += 1;
        }
        if self.fails_at.is_some_and(|n| n < self.written && n + (buf.len() / SECTOR) as u64 >= self.written) {
            self.fails_at = None;
            return Err(Error::Io);
        }
        Ok(())
    }
}

const T0: u32 = 1_161_676_800; // 2026-10-23 08:00 UTC
const SECS: u32 = 900;
const PAUSE: u32 = 300; // paused from here (the second itself recorded) ...
const RESUME: u32 = 361; // ... to here
const LAP: u32 = 500;
const NAME: [u8; 12] = *b"261023100000";

fn local(t: u32) -> Local {
    (2026, 10, 23, 10 + (t / 3600) as u8, (t / 60 % 60) as u8, (t % 60) as u8)
}

std::thread_local! {
    /// The test ride's profile ([`Sample::profile`], [`Sample::by_hand`]): set by the indoor and MTB variants of a
    /// test, on its own thread.
    static PROFILE: core::cell::Cell<(Profile, bool)> = const { core::cell::Cell::new((Profile::Road, false)) };
}

fn sample(t: u32) -> Sample {
    Sample {
        time: T0 + t,
        pos: Some((5_123_000 + 300 * t as i32, 4_234_000 - 200 * t as i32)),
        alt_mm: Some(35_000 + 100 * (t % 50) as i32),
        speed_mm_s: Some(7000 + 10 * (t % 100)),
        hr: (t >= 10).then_some(120 + (t % 40) as u8),
        cadence: (t >= 30).then_some(85 + (t % 10) as u8),
        power: (t >= 30).then_some(200 + (t % 60) as u16),
        temp_c: Some(12),
        alpha1_milli: (t >= 60).then_some(500 + (t % 700) as u16),
        rmssd_ms: (t >= 60).then_some(20 + (t % 40) as u16),
        artifacts_pct: (t >= 60).then_some((t % 9) as u8),
        ascent_m: Some(t / 10),
        trainer: crate::fit::tests::trainer(t),
        profile: PROFILE.get().0,
        by_hand: PROFILE.get().1,
        auto_paused: auto_paused(t),
    }
}

/// Second `t` of the test ride is auto-paused (4 s in every 20, in each lap, before and after any cut): recorded,
/// out of its lap's averages (fit.rs "Laps").
fn auto_paused(t: u32) -> bool {
    t % 20 < 4
}

/// The strap's beat in second `t` of the test ride, ms: 250 + t, so a file's beats tell which were lost or
/// written twice, and they add up to less than the ride's time, as a strap's do.
fn beats(t: u32) -> [u16; 1] {
    [250 + t as u16]
}

/// The beats (`hrv` messages) of the saved file `name`, the invalid slots left out.
fn saved_beats(img: &[u8], name: &str) -> Vec<u64> {
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let v = crate::fat::tests::read_all(&mut fat, &mut d, name).unwrap();
    let m = crate::fit::tests::decode(&v).unwrap();
    m.iter().filter(|x| x.0 == 78).flat_map(|x| x.1.iter().map(|f| f.1)).filter(|&v| v != 0xffff).collect()
}

fn words(t: u32) -> [u32; RIDE_WORDS] {
    let mut w = [0; RIDE_WORDS];
    w[0] = t;
    w
}

fn recorded(t: u32) -> bool {
    !(PAUSE + 1..RESUME).contains(&t)
}

/// The ride from second `from` (0: Start) to the end and its save, as the app drives the recorder: the last
/// outcome, None once the card died.
fn ride(r: &mut Recorder, d: &mut Card, from: u32) -> Option<Outcome> {
    ride_all(core::slice::from_mut(r), d, from).map(|o| o[0])
}

/// [`ride`] with several recorders, each event to each in turn and their card work in that order (the app
/// drives the clean copy's, then the full copy's): the last outcome of each.
fn ride_all(rs: &mut [Recorder], d: &mut Card, from: u32) -> Option<Vec<Outcome>> {
    ride_all_with(rs, d, from, false)
}

/// [`ride_all`], the ride discarded at its end if `discard` (the end-ride screen's DISCARD).
fn ride_all_with(rs: &mut [Recorder], d: &mut Card, from: u32, discard: bool) -> Option<Vec<Outcome>> {
    let mut last = std::vec![Outcome::Nothing; rs.len()];
    let mut io = |rs: &mut [Recorder], d: &mut Card, t: u32| {
        pass(rs, d, t, &mut last);
        !d.dead
    };
    for t in from..SECS {
        for r in rs.iter_mut() {
            match t {
                0 => r.start(T0 - 1, NAME, false),
                _ if t == from => r.resume(T0 + t), // after a reset: the ride was paused
                RESUME => r.resume(T0 + t),
                _ => {}
            }
            if recorded(t) {
                r.second(&sample(t), words(t));
            }
            // the beats come paused or not; a full copy resumed already has those up to its own checkpoint
            // (on the unit, beats after a reset are new ones: the test's clock goes back to the clean copy's)
            if T0 + t > r.skip_to {
                for b in beats(t) {
                    r.beat(b);
                }
            }
            match t {
                PAUSE => r.pause(T0 + t, words(t)),
                LAP => r.lap(T0 + t),
                _ => {}
            }
        }
        if !io(rs, d, t) {
            return None;
        }
    }
    for r in rs.iter_mut() {
        r.end(T0 + SECS, words(SECS));
        if discard {
            r.discard();
        }
    }
    for _ in 0..3 {
        if !io(rs, d, SECS) {
            return None;
        }
    }
    Some(last)
}

/// One pass of card work as `App::step_recorder` does it: the clean copy's (`rs[0]`), then the full copy's.
fn pass(rs: &mut [Recorder], d: &mut Card, t: u32, last: &mut [Outcome]) {
    d.now = t;
    if let [clean, full] = rs {
        clean.keep_up(full);
        full.keep_up(clean);
    }
    if rs[0].due() {
        last[0] = rs[0].io(d, local(t));
    }
    if let ([clean], [full, ..]) = rs.split_at_mut(1)
        && full.due()
    {
        last[1] = full.io_after(clean, d, local(t));
    }
}

/// The saved ride's record times and whether it has its session; Err if it is not a whole FIT file.
fn saved(img: &[u8], name: &str) -> Result<(Vec<u32>, bool), std::string::String> {
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, name).map_err(|e| std::format!("{e:?}"))?;
    let mut v = std::vec![0u8; f.size as usize];
    fat.read_at(&mut d, &f, 0, &mut v).unwrap();
    let (mut times, mut session, mut rd) = (Vec::new(), false, Reader::new());
    rd.push_slice(&v, |m| match m {
        Msg::Record(r) => times.push(r.time),
        Msg::Session(_) => session = true,
        Msg::CoursePoint { .. } | Msg::Lap(_) => {}
    });
    match rd.result() {
        Some(Ok(())) => Ok((times, session)),
        e => Err(std::format!("{e:?}")),
    }
}

/// Each lap of the saved file `name` carries the average and maximum heart rate (fields 15, 16) and power (19, 20)
/// of the file's own records in it (from the lap before's time, excluded, to its own) that were not auto-paused
/// ([`auto_paused`]), FIT's invalid when none had one: after a cut and a resume too, the lap's sums before the cut
/// kept in the recovery record. The laps.
fn laps_match_records(img: &[u8], name: &str, what: &str) -> usize {
    use crate::fit::tests::field;
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let v = crate::fat::tests::read_all(&mut fat, &mut d, name).unwrap();
    let m = crate::fit::tests::decode(&v).unwrap();
    let mut from = 0;
    let laps: Vec<_> = m.iter().filter(|x| x.0 == 19).collect();
    for lap in &laps {
        let to = field(lap, 253).unwrap();
        let recs: Vec<_> = m
            .iter()
            .filter(|x| x.0 == 20)
            .filter(|x| {
                field(x, 253).is_some_and(|t| (from + 1..=to).contains(&t) && !auto_paused((t - T0 as u64) as u32))
            })
            .collect();
        for (f, avg, max, none) in [(3, 15, 16, 0xff), (7, 19, 20, 0xffff)] {
            let x: Vec<u64> = recs.iter().filter_map(|r| field(r, f)).collect();
            let want = if x.is_empty() {
                (none, none)
            } else {
                (x.iter().sum::<u64>() / x.len() as u64, *x.iter().max().unwrap())
            };
            assert_eq!((field(lap, avg), field(lap, max)), (Some(want.0), Some(want.1)), "{what}: lap to {to}, {f}");
        }
        from = to;
    }
    timer_follows_events(&m, what);
    laps.len()
}

/// The file's timer events (event timer) alternate start, stop (an auto-pause's or the Start key's), and each lap's
/// and the session's total_timer_time is its time less the stops in it (fit.rs, "The timer"): after any cut and
/// resume too.
pub(crate) fn timer_follows_events(m: &[crate::fit::tests::Msg], what: &str) {
    use crate::fit::tests::field;
    let ev: Vec<(u64, u64)> = m
        .iter()
        .filter(|x| x.0 == 21 && field(x, 0) == Some(0))
        .map(|x| (field(x, 253).unwrap(), field(x, 1).unwrap()))
        .collect();
    assert!(ev.iter().enumerate().all(|(k, e)| e.1 == if k % 2 == 0 { 0 } else { 4 }), "{what}: {ev:?}");
    // the stops: from each stop to the start after it (or on)
    let stops: Vec<(u64, u64)> =
        (1..ev.len()).step_by(2).map(|k| (ev[k].0, ev.get(k + 1).map_or(u64::MAX, |e| e.0))).collect();
    for x in m.iter().filter(|x| x.0 == 19 || x.0 == 18) {
        let (from, to) = (field(x, 2).unwrap(), field(x, 253).unwrap());
        let stopped: u64 = stops.iter().map(|&(a, b)| b.min(to).saturating_sub(a.max(from))).sum();
        assert_eq!(field(x, 8), Some((to - from - stopped) * 1000), "{what}: message {} to {to}: {ev:?}", x.0);
    }
}

const SAVED: &str = "Activities/261023100000.fit";
const SAVED_FULL: &str = "Rustfw/Activities/261023100000.fit";

/// Stock's files (everything outside `\Rustfw` on the test volume, tools/fat_image.py: its ride in progress
/// `\Activities\tmp.fit`, its recovery file `\System\tmp0.fit`, its settings) as they were: bytes, entries, chains
/// (the review of 2026-11-01, finding 4). `torn`: sectors a cut tore, whose entries are left out.
fn stock_same(img: &[u8], torn: &[u32], what: &str) {
    stock_kept(base_stock(), img, torn, &[]).unwrap_or_else(|e| panic!("{what}: {e}"));
}

fn check_clean(img: &[u8], what: &str) -> u32 {
    let lost = fsck(img).unwrap_or_else(|e| panic!("{what}: {e}"));
    let (free, info) = free_counts(img);
    assert_eq!(free, info, "{what}: FSInfo's free count");
    lost
}

/// A ride recorded and saved: in `\Activities` with stock's name, every second in it, nothing left in
/// `\Rustfw` but the records, a clean volume.
#[test]
fn records_and_saves_a_ride() {
    let img = crate::fat::tests::image();
    let mut d = card(img);
    let mut r = Recorder::new();
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(NAME)));
    assert_eq!(r.phase(), Phase::Done);
    let (times, session) = saved(&d.img, SAVED).unwrap();
    let want: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    assert_eq!(times, want);
    assert!(session);
    assert_eq!(check_clean(&d.img, "after the save"), 0);
    // for tools/fit_check.py (fitdecode and Garmin's SDK): target/fit/recorded.fit
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let f = fat.open(&mut ram, SAVED).unwrap();
    let mut v = std::vec![0u8; f.size as usize];
    fat.read_at(&mut ram, &f, 0, &mut v).unwrap();
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("recorded.fit"), &v).unwrap();
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    assert_eq!(fat.open(&mut ram, "Rustfw/Rides/261023100000.fit"), Err(Error::NotFound));
    assert_eq!(fat.open(&mut ram, "Rustfw/ride.rec").map(|f| f.size), Ok(3072));
    std::println!("{} sectors written for a {SECS} s ride, {} errors, {} dropped", d.written, r.errors, r.dropped);
    assert_eq!((r.errors, r.dropped), (0, 0));
    // a second ride with the same name: the folders are there, and it is saved a second later
    let mut r = Recorder::new();
    let mut d2 = card(d.img.clone());
    assert_eq!(ride(&mut r, &mut d2, 0), Some(Outcome::Saved(*b"261023100001")));
    assert!(saved(&d2.img, "Activities/261023100001.fit").is_ok());
    assert!(saved(&d2.img, SAVED).is_ok(), "the first one is still there");
    assert!(d2.written < d.written, "no folders to make: {} >= {}", d2.written, d.written);
    assert_eq!(check_clean(&d2.img, "two rides"), 0);
}

/// The saved ride's session sub_sport (FIT profile: session field 6), if it has one.
fn sub_sport(img: &[u8], name: &str) -> Option<u64> {
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).ok()?;
    let f = fat.open(&mut d, name).ok()?;
    let mut v = std::vec![0u8; f.size as usize];
    fat.read_at(&mut d, &f, 0, &mut v).ok()?;
    let m = crate::fit::tests::decode(&v).ok()?;
    m.iter().find(|x| x.0 == 18).and_then(|s| crate::fit::tests::field(s, 6))
}

// The golden rule: the power cut after every sector of a whole ride, its start (folders made) and its save
// included. After each, the boot's recovery gives back the ride at its last checkpoint (at most a minute
// lost), it goes on and is saved, and the volume is clean (FATs equal, FSInfo exact, no chain into a free
// cluster, no cross-link; a lost cluster at worst).
cut_part!(power_cut_at_every_sector_write_part_0, power_cut_at_every_sector(Profile::Road, false, 0));
cut_part!(power_cut_at_every_sector_write_part_1, power_cut_at_every_sector(Profile::Road, false, 1));
cut_part!(power_cut_at_every_sector_write_part_2, power_cut_at_every_sector(Profile::Road, false, 2));
cut_part!(power_cut_at_every_sector_write_part_3, power_cut_at_every_sector(Profile::Road, false, 3));
cut_part!(power_cut_at_every_sector_write_part_4, power_cut_at_every_sector(Profile::Road, false, 4));
cut_part!(power_cut_at_every_sector_write_part_5, power_cut_at_every_sector(Profile::Road, false, 5));
cut_part!(power_cut_at_every_sector_write_part_6, power_cut_at_every_sector(Profile::Road, false, 6));
cut_part!(power_cut_at_every_sector_write_part_7, power_cut_at_every_sector(Profile::Road, false, 7));

/// The heart rate and power of the saved file `name`'s laps (fields 15, 16, 19, 20).
fn lap_values(img: &[u8], name: &str) -> Vec<[Option<u64>; 4]> {
    let mut d = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let v = crate::fat::tests::read_all(&mut fat, &mut d, name).unwrap();
    let m = crate::fit::tests::decode(&v).unwrap();
    m.iter().filter(|x| x.0 == 19).map(|l| [15, 16, 19, 20].map(|n| crate::fit::tests::field(l, n))).collect()
}

/// A reset in the second lap, whose auto-paused seconds fall before the cut and after the resume ([`auto_paused`]):
/// the resumed ride's laps carry the same heart rate and power as the ride never cut (the seconds after the last
/// checkpoint ridden again, the lap's sums before it from the recovery record).
#[test]
fn a_resume_mid_lap_keeps_its_averages() {
    let img = crate::fat::tests::image();
    let mut whole = card(img.clone());
    ride(&mut Recorder::new(), &mut whole, 0).unwrap();
    let want = lap_values(&whole.img, SAVED);
    assert_eq!(want.len(), 2);
    let mut tried = 0;
    for cut in (0..whole.written).step_by(7) {
        let mut d = Card { budget: Some(cut), ..card(img.clone()) };
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none());
        let mut d = card(d.img);
        let mut r = Recorder::new();
        let Outcome::Resumed(w, _) = r.recover(&mut d, local(SECS + 5)) else { continue };
        // the checkpoint well into the second lap, auto-paused seconds on both sides of it
        if !(LAP + 30..SECS - 30).contains(&w[0]) {
            continue;
        }
        assert!(ride(&mut r, &mut d, w[0] + 1).is_some());
        assert_eq!(lap_values(&d.img, SAVED), want, "cut {cut}, resumed at {}", w[0]);
        tried += 1;
    }
    assert!(tried > 3, "{tried} resumes in the second lap");
}

// The same for an indoor ride: after every cut it comes back indoor (the recovery record keeps the profile)
// and is saved with sub_sport indoor_cycling (6), at boot too (ended from the record alone).
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_0, power_cut_at_every_sector(Profile::Indoor, false, 0));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_1, power_cut_at_every_sector(Profile::Indoor, false, 1));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_2, power_cut_at_every_sector(Profile::Indoor, false, 2));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_3, power_cut_at_every_sector(Profile::Indoor, false, 3));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_4, power_cut_at_every_sector(Profile::Indoor, false, 4));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_5, power_cut_at_every_sector(Profile::Indoor, false, 5));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_6, power_cut_at_every_sector(Profile::Indoor, false, 6));
cut_part!(an_indoor_ride_cut_at_every_sector_stays_indoor_part_7, power_cut_at_every_sector(Profile::Indoor, false, 7));

// The same for an MTB ride: after every cut it comes back MTB and is saved with sub_sport mountain (8).
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_0, power_cut_at_every_sector(Profile::Mtb, true, 0));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_1, power_cut_at_every_sector(Profile::Mtb, true, 1));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_2, power_cut_at_every_sector(Profile::Mtb, true, 2));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_3, power_cut_at_every_sector(Profile::Mtb, true, 3));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_4, power_cut_at_every_sector(Profile::Mtb, true, 4));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_5, power_cut_at_every_sector(Profile::Mtb, true, 5));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_6, power_cut_at_every_sector(Profile::Mtb, true, 6));
cut_part!(an_mtb_ride_cut_at_every_sector_stays_mtb_part_7, power_cut_at_every_sector(Profile::Mtb, true, 7));

// The same for an indoor ride set by hand (no trainer: a dumb trainer, rollers): it comes back by hand too, so
// the app does not give it back to the automatic choice (which turns it ROAD without a trainer).
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_0,
    power_cut_at_every_sector(Profile::Indoor, true, 0)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_1,
    power_cut_at_every_sector(Profile::Indoor, true, 1)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_2,
    power_cut_at_every_sector(Profile::Indoor, true, 2)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_3,
    power_cut_at_every_sector(Profile::Indoor, true, 3)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_4,
    power_cut_at_every_sector(Profile::Indoor, true, 4)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_5,
    power_cut_at_every_sector(Profile::Indoor, true, 5)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_6,
    power_cut_at_every_sector(Profile::Indoor, true, 6)
);
cut_part!(
    an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand_part_7,
    power_cut_at_every_sector(Profile::Indoor, true, 7)
);

fn power_cut_at_every_sector(profile: Profile, by_hand: bool, part: u64) {
    PROFILE.set((profile, by_hand));
    let sub = Some(match profile {
        Profile::Road => 7,
        Profile::Indoor => 6,
        Profile::Mtb => 8,
        // GRAVEL, CX and COMMUTE are kept as ROAD bits by the recorder (fit.rs to_record): not run here
        Profile::Gravel | Profile::Cx | Profile::Commute => unreachable!("kept as ROAD"),
    });
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let total = full.written;
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut resumed, mut saved_at_boot, mut nothing, mut max_lost, mut twice_cuts) = (0, 0, 0, 0, 0);
    assert_eq!(laps_match_records(&full.img, SAVED, "uncut"), 2);
    let mut mid_lap = 0; // resumed in the second lap, its sums from the record
    for cut in (part..total).step_by(CUT_PARTS as usize) {
        let what = std::format!("cut {cut} of {total}");
        let mut d = Card { budget: Some(cut), ..card(img.clone()) };
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}: the card died");
        // the volume as the cut left it: readable, the FATs at most a sector apart, a chain at most one cluster past
        // its size (a cut between a checkpoint's data and its size: the boot's recovery trims it)
        let c = fsck_cut(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(c.apart <= 1, "{what}: {} FAT sectors apart", c.apart);
        stock_same(&d.img, &[], &what);
        // the ride's chain as the recovery finds it: its record's cluster (not its name: none before the create)
        let mine = newest_rec(&d.img).map_or(0, |r| r.cluster);
        assert!(
            c.past.iter().all(|&(first, n)| first == mine && n <= 1),
            "{what}: chains past their size {:?}",
            c.past
        );
        // reboot; and the reboot cut too, at each of its own writes, then again: the same outcome
        let cut_img = d.img;
        let mut d = card(cut_img.clone());
        let mut r = Recorder::new();
        let outcome = r.recover(&mut d, local(SECS + 5));
        for k in 0..d.written {
            let mut twice = Card { budget: Some(k), ..card(cut_img.clone()) };
            Recorder::new().recover(&mut twice, local(SECS + 5));
            let mut again = card(twice.img);
            let o = Recorder::new().recover(&mut again, local(SECS + 6));
            stock_same(&again.img, &[], &what);
            // a cut save may be finished by the first reboot already: then nothing is left to do
            let same = o == outcome || (matches!(outcome, Outcome::Saved(_)) && o == Outcome::Nothing);
            assert!(same, "{what}, reboot cut at {k}: {o:?}, not {outcome:?}");
            twice_cuts += 1;
        }
        match outcome {
            Outcome::Resumed(w, name) => {
                assert_eq!(name, NAME);
                assert_eq!((r.profile(), r.by_hand()), (profile, by_hand), "{what}: profile back");
                resumed += 1;
                mid_lap += usize::from(w[0] > LAP && best_rec(&cut_img).lap[1] > 0);
                let from = w[0] + 1;
                let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                assert!(ride(&mut r, &mut d, from).is_some(), "{what}");
                let (times, session) = saved(&d.img, SAVED).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(session, "{what}");
                assert_eq!(sub_sport(&d.img, SAVED), sub, "{what}");
                assert!(times.windows(2).all(|p| p[0] < p[1]), "{what}: times in order");
                let lost = all.len() - times.len();
                assert!(lost <= CHECKPOINT_S as usize, "{what}: {lost} s lost");
                assert!(times.iter().all(|t| all.contains(t)), "{what}");
                assert_eq!(laps_match_records(&d.img, SAVED, &what), 2, "{what}");
            }
            Outcome::Saved(name) => {
                assert_eq!(name, NAME);
                saved_at_boot += 1;
                let (times, session) = saved(&d.img, SAVED).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(session && times == all, "{what}: the whole ride");
                assert_eq!(laps_match_records(&d.img, SAVED, &what), 2, "{what}");
                assert_eq!(sub_sport(&d.img, SAVED), sub, "{what}");
            }
            Outcome::Nothing => {
                nothing += 1;
                // cut before the first checkpoint's record: nothing was promised, and nothing is saved; the
                // next ride's start puts the volume right (a cut while making the folders leaves no record)
                assert!(saved(&d.img, SAVED).is_err(), "{what}");
                // (a second later if the cut left a half-made file of that name)
                let Some(Outcome::Saved(n)) = ride(&mut r, &mut d, 0) else { panic!("{what}: the next ride") };
                let path = std::format!("Activities/{}.fit", core::str::from_utf8(&n).unwrap());
                assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}");
            }
            Outcome::Failed(_) | Outcome::Kept(_) | Outcome::Stopped(_) | Outcome::Discarded(_) => {
                panic!("{what}: recovery failed: {outcome:?}")
            }
        }
        assert_eq!(r.phase(), Phase::Done, "{what}");
        stock_same(&d.img, &[], &what);
        max_lost = max_lost.max(check_clean(&d.img, &what));
    }
    std::println!(
        "{total} cut points: {resumed} resumed, {saved_at_boot} saved at boot, {nothing} before the first \
         checkpoint; lost clusters at most {max_lost}; {twice_cuts} cuts of a reboot"
    );
    assert!(resumed > 50 / CUT_PARTS as usize);
    // the cuts after the last checkpoint, whose save the boot finishes (found by printing them, as AT_BOOT_CUTS): the
    // part that owns each must meet it, so the whole run asserts what the single test did (`saved_at_boot > 0`)
    // they are the ride's last four writes, from the uncut run's count: a change of the writes that moves them fails here
    let owned = (total.saturating_sub(4)..total).filter(|c| c % CUT_PARTS == part).count();
    assert_eq!(
        saved_at_boot, owned,
        "part {part}: {saved_at_boot} cuts saved at boot, the last four of {total} writes expected to ({owned} here): \
         did the recorder's write count or its save order change?"
    );
    assert!(mid_lap > 5 / CUT_PARTS as usize, "{mid_lap} resumes in the second lap");
    assert!(max_lost <= 2);
}

// The golden rule on a card without reliable write: the power cut at every sector of a whole ride, the sector
// being written left as garbage (the ride file's sector 0 and last sector, its directory sector, a FAT
// sector, FSInfo, a record). The ride is resumed (at most a minute lost) or saved from its record, never
// lost once its first checkpoint is on the card; the next ride is recorded and saved; the volume is clean.
// A torn directory sector of `\Activities` or the root (stock's entries) is the card's reliable write's to
// prevent (emmc.rs): here it only must not stop the recorder.
//
// After each cut, the reboot is cut (torn too) at each of its own writes, then booted again: the same outcome.
// A resumed ride is cut a second time once in each of its checkpoints (the review of 2026-11-01, finding 10):
// resumed or saved again, at most a minute lost. Each test runs every [`CUT_PARTS`]th cut.
cut_part!(power_cut_tearing_the_sector_at_every_write_part_0, torn_cuts(false, 0));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_1, torn_cuts(false, 1));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_2, torn_cuts(false, 2));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_3, torn_cuts(false, 3));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_4, torn_cuts(false, 4));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_5, torn_cuts(false, 5));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_6, torn_cuts(false, 6));
cut_part!(power_cut_tearing_the_sector_at_every_write_part_7, torn_cuts(false, 7));

// The same, the torn sector's first k bytes new and the rest old (k on a 4-byte edge, a FAT entry's, every 8th on a
// 32-byte one, a directory entry's; from 4 to 508 over the cuts): a sector a reader accepts, which garbage never
// is (the review of 2026-11-01, finding 3: a grow's two FAT entries in one write would pass every other test).
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_0, torn_cuts(true, 0));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_1, torn_cuts(true, 1));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_2, torn_cuts(true, 2));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_3, torn_cuts(true, 3));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_4, torn_cuts(true, 4));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_5, torn_cuts(true, 5));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_6, torn_cuts(true, 6));
cut_part!(power_cut_tearing_a_prefix_at_every_write_part_7, torn_cuts(true, 7));

/// The torn card cut after `budget` sectors: garbage, or (`prefix`) the first k bytes new, k from `seed`.
fn torn_card(img: Vec<u8>, budget: u64, prefix: bool, seed: u64) -> Card {
    let k = prefix.then_some(4 * (1 + (seed % 127) as usize));
    Card { budget: Some(budget), torn: true, prefix: k, ..card(img) }
}

/// The save's last four writes (step 4's; step 5's two, `Fat::link`'s entries written deleted, then made live; the
/// record "done"), each torn after every byte of its sector (a card without reliable write that programs a sector in
/// order), then booted: saved once under its name, every second there, the volume clean (no orphan long name), FSInfo
/// right, a second boot with nothing to do. Step 4's write marks the long name's pieces deleted below the 8.3 entry:
/// a tear between left the 8.3 entry live under its alias alone, which the save looked up by name and so linked a
/// second time into `\Activities` (found 2026-11-03: cluster 120 cross-linked). Step 5 written live in one write
/// could leave orphan long name pieces, an empty file of the ride's name, a link 0 bytes long, or one on its
/// cluster's high word alone (another file's chain past 0xffff clusters). `give_up`: a data sector damaged before the
/// boot, so its read-back fails and the save keeps the ride where it is (`keep_in_rides`). The volumes of each case
/// go to target/fat for fsck_msdos.
fn torn_save(back: u64, give_up: bool) {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let (total, at) = (full.written, full.written - back);
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (first, size) = {
        let mut ram = crate::fat::tests::Ram(full.img.clone(), 0);
        let f = Fat::mount(&mut ram).unwrap().open(&mut ram, SAVED).unwrap();
        (f.cluster, f.size)
    };
    let data = first_lba(&full.img, SAVED) * SECTOR + 2 * SECTOR; // its sector 2: neither sector 0 nor a record's
    let mut alias_alone = 0;
    for keep in 1..SECTOR {
        let what = std::format!("the save's write {at} of {total} torn after {keep} bytes, given up {give_up}");
        let mut d = Card { budget: Some(at), torn: true, prefix: Some(keep), ..card(img.clone()) };
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}");
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let rides = fat.open(&mut ram, "Rustfw/Rides").unwrap();
        let by_name = fat.find(&mut ram, rides, "261023100000.fit").is_ok();
        let alone = !by_name && fat.locate_chain(&mut ram, rides, first).is_ok();
        let mut img2 = d.img;
        if give_up {
            img2[data..data + 16].fill(0x42);
        }
        let mut d = card(img2);
        let o = Recorder::new().recover(&mut d, local(SECS + 5));
        let (folder, other) = match o {
            Outcome::Saved(NAME) => ("Activities", "Rustfw/Rides"),
            Outcome::Kept(NAME) if give_up => ("Rustfw/Rides", "Activities"),
            o => panic!("{what}: {o:?}"),
        };
        let path = std::format!("{folder}/261023100000.fit");
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        assert_eq!(fat.open(&mut ram, &path).map(|f| (f.cluster, f.size)), Ok((first, size)), "{what}");
        if !give_up {
            assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}");
        }
        // one live entry on the chain in the volume (fsck: never cross-linked), the other folder's gone
        let other = fat.open(&mut ram, other).unwrap();
        assert!(fat.locate_chain(&mut ram, other, first).is_err(), "{what}: named twice");
        assert_eq!(fsck(&d.img), Ok(0), "{what}");
        assert_eq!(free_counts(&d.img).0, free_counts(&d.img).1, "{what}: FSInfo's free count");
        stock_same(&d.img, &[], &what);
        assert_eq!(Recorder::new().recover(&mut d, local(SECS + 6)), Outcome::Nothing, "{what}: twice");
        let tag = if give_up { "_given_up" } else { "" };
        if alone {
            alias_alone += 1;
            crate::fat::tests::dump(&std::format!("save_torn_after_its_long_name{tag}"), &d.img);
        }
        // inside step 5's 8.3 entry (at 160 of the sector: its cluster's words at 180 and 186, its size at 188)
        if back < 4 && keep == 187 {
            crate::fat::tests::dump(&std::format!("save_write_{back}_from_the_end_torn{tag}"), &d.img);
        }
    }
    // the long name's two pieces at bytes 64 and 96 of the sector, the 8.3 entry's first byte at 128
    assert!(back != 4 || alias_alone >= 60, "{alias_alone} tears left the 8.3 entry alone");
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_step_4() {
    torn_save(4, false);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_its_link() {
    torn_save(3, false);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_its_link_made_live() {
    torn_save(2, false);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_its_record() {
    torn_save(1, false);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_step_4_given_up() {
    torn_save(4, true);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_its_link_given_up() {
    torn_save(3, true);
}

#[test]
fn power_cut_tearing_the_save_at_every_byte_of_its_link_made_live_given_up() {
    torn_save(2, true);
}

/// The last writes of a resumed ride each cut by [`torn_cuts`]: its save (about 10: the end's checkpoint, about 7
/// with its data, then steps 4 to 6) and a few before. Cost (2026-11-03): the 16 tests' second cuts from about 190
/// to 450 each, 25 to 48 s on one thread, 5.6 to 7.1 s of wall time on the gate's.
const LAST_WRITES: u64 = 12;

fn torn_cuts(prefix: bool, part: u64) {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let total = full.written;
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut promised, mut resumed, mut saved_at_boot, mut max_lost, mut torn_root) = (false, 0, 0, 0, 0);
    let (mut twice_cuts, mut second_cuts, mut let_go) = (0, 0, 0);
    let cluster = Fat::mount(&mut crate::fat::tests::Ram(img.clone(), 0)).unwrap().cluster_bytes() as usize;
    // the root directory's first cluster (the boot sector's fields: fatgen103)
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
    let root_lba = u(14, 2) + u(16, 1) * u(36, 4) + (u(44, 4) - 2) * u(13, 1);
    let root = root_lba..root_lba + u(13, 1);
    // \Rustfw's directory, whose folders and records are made at the first ride's start, before any record
    let mine = {
        let mut ram = crate::fat::tests::Ram(full.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let c = fat.open(&mut ram, DIR).unwrap().cluster;
        let lba = fat.lba(c);
        lba..lba + fat.cluster_bytes() / SECTOR as u32
    };
    // the saved ride: in order, every second the ride's, at most a minute lost of the `had` recorded at the cut
    let check = |img: &[u8], had: usize, what: &str| {
        let Ok((times, _)) = saved(img, SAVED) else { return false };
        assert!(times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)), "{what}");
        let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
        assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {} s lost", had - kept);
        true
    };
    let from_of = |w: [u32; RIDE_WORDS]| {
        let from = w[0] + 1;
        if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from }
    };
    // not every fourth cut: a checkpoint's writes repeat every 8, and its directory sector's cuts (saved at boot)
    // would all fall in one part
    for cut in (0..total).filter(|c| (c + c / 8) % CUT_PARTS == part) {
        let mut d = torn_card(img.clone(), cut, prefix, cut);
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none());
        let what = std::format!("prefix {prefix}, cut {cut} of {total}, sector {} torn", d.cut_at);
        let cut_lba = d.cut_at;
        let had = all.iter().filter(|&&a| a <= T0 + d.now).count(); // the seconds recorded when the power went
        let cut_img = d.img;
        stock_same(&cut_img, &[cut_lba], &what);
        let mut d = card(cut_img.clone());
        let mut r = Recorder::new();
        let outcome = r.recover(&mut d, local(SECS + 5));
        // a ride saved at boot from its record alone (its entry torn) whose repair is torn too, the known limits of
        // `Recorder::still_ours` and `mark_checked`: under 3 sectors (the repair writes sector 1), or in one cluster
        // with its sector 0 torn (as if another small file had taken the freed cluster since): let go
        let at_boot = crate::storage::trash::tests::file(&d.img, SAVED).map(|f| f.len());
        let tiny = at_boot.is_some_and(|n| n < 2 * SECTOR + 2);
        let one_cluster = at_boot.is_some_and(|n| n <= cluster).then(|| first_lba(&d.img, SAVED) as u32);
        // the reboot torn at each of its own writes, then booted again: the same outcome
        for k in 0..d.written {
            let mut twice = torn_card(cut_img.clone(), k, prefix, cut + k);
            Recorder::new().recover(&mut twice, local(SECS + 5));
            let torn2 = twice.cut_at;
            let mut again = card(twice.img);
            let o = Recorder::new().recover(&mut again, local(SECS + 6));
            stock_same(&again.img, &[cut_lba, torn2], &what);
            // a cut save may be finished by the first reboot already: then nothing is left to do; and a resume torn
            // in the ride's directory sector (its size) leaves the record alone naming it: saved from there
            let same = o == outcome
                || matches!(
                    (outcome, o),
                    (Outcome::Saved(_), Outcome::Nothing) | (Outcome::Resumed(..), Outcome::Saved(_))
                );
            assert!(same, "{what}, reboot torn at {k}: {o:?}, not {outcome:?}");
            if !matches!(o, Outcome::Resumed(..) | Outcome::Nothing) || matches!(outcome, Outcome::Saved(_)) {
                let gone = o == Outcome::Nothing && (tiny || one_cluster == Some(twice.cut_at));
                let_go += usize::from(gone);
                assert!(check(&again.img, had, &what) || gone, "{what}, reboot torn at {k}: not saved");
            }
            twice_cuts += 1;
        }
        let booted = d.written;
        match outcome {
            Outcome::Resumed(w, _) => {
                resumed += 1;
                assert_eq!(ride(&mut r, &mut d, from_of(w)), Some(Outcome::Saved(NAME)), "{what}");
            }
            Outcome::Saved(n) => {
                assert_eq!(n, NAME, "{what}");
                saved_at_boot += 1;
            }
            Outcome::Nothing => assert!(!promised, "{what}: a ride with a checkpoint lost"),
            o => panic!("{what}: {o:?}"),
        }
        promised |= check(&d.img, had, &what);
        // the resumed ride cut again, torn, once in each of its checkpoints (the phase in it moving with the cut),
        // and at every one of its last [`LAST_WRITES`] (its last checkpoint and its save, whose torn step 4 once
        // left two entries on the chain: 2026-11-03, `power_cut_tearing_the_save_at_every_byte_of_step_4`)
        if let Outcome::Resumed(w, _) = outcome {
            let n = d.written - booted;
            let checkpoints = u64::from((SECS - from_of(w)) / CHECKPOINT_S + 1);
            let step = (n / checkpoints).max(1);
            let last = n.saturating_sub(LAST_WRITES);
            for at in (cut * 7 % step..last).step_by(step as usize).chain(last..n) {
                let what = std::format!("{what}, resumed and torn again at {at} of {n}");
                let mut d2 = card(cut_img.clone());
                let mut r2 = Recorder::new();
                assert_eq!(r2.recover(&mut d2, local(SECS + 5)), outcome, "{what}");
                let mut d2 = torn_card(d2.img, at, prefix, cut + at);
                assert!(ride(&mut r2, &mut d2, from_of(w)).is_none(), "{what}: the card died");
                let had2 = all.iter().filter(|&&a| a <= T0 + d2.now).count();
                let torn2 = d2.cut_at;
                let mut d3 = card(d2.img);
                let mut r3 = Recorder::new();
                match r3.recover(&mut d3, local(SECS + 7)) {
                    Outcome::Resumed(w3, _) => {
                        assert_eq!(ride(&mut r3, &mut d3, from_of(w3)), Some(Outcome::Saved(NAME)), "{what}");
                    }
                    Outcome::Saved(n) => assert_eq!(n, NAME, "{what}"),
                    o => panic!("{what}: {o:?}, the ride lost"),
                }
                assert!(check(&d3.img, had2, &what), "{what}: not saved");
                stock_same(&d3.img, &[cut_lba, torn2], &what);
                fsck_torn(&d3.img).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(free_counts(&d3.img).0, free_counts(&d3.img).1, "{what}: FSInfo's free count");
                second_cuts += 1;
            }
        }
        let Some(Outcome::Saved(n)) = ride(&mut Recorder::new(), &mut d, 0) else { panic!("{what}: the next ride") };
        let path = std::format!("Activities/{}.fit", core::str::from_utf8(&n).unwrap());
        assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}: the next ride");
        // (a folder whose entry the first start's cut tore inside it, on cluster 0: made again by the next Start)
        let lost = fsck_torn(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(free_counts(&d.img).0, free_counts(&d.img).1, "{what}: FSInfo's free count");
        stock_same(&d.img, &[cut_lba], &what);
        if lost > 2 {
            // only a torn sector of the root directory (making \Rustfw or \Activities), or of \Rustfw's while
            // its folders are made, before the first record (no ride yet): its other entries lost
            assert!(root.contains(&cut_lba) || mine.contains(&cut_lba) && !promised, "{what}: {lost} clusters lost");
            torn_root += 1;
        } else {
            max_lost = max_lost.max(lost);
        }
    }
    std::println!(
        "prefix {prefix}, part {part}: {total} torn cuts: {resumed} resumed, {saved_at_boot} saved at boot; lost \
         clusters at most {max_lost}; {torn_root} in the root directory; {twice_cuts} torn reboots ({let_go} let \
         go), {second_cuts} second cuts"
    );
    // a prefix tear of the ride's directory sector keeps its entry, old or new: saved at boot is the garbage's
    assert!(resumed > 50 / CUT_PARTS && (prefix || saved_at_boot > 10 / CUT_PARTS), "{resumed}, {saved_at_boot}");
    assert!(second_cuts > resumed, "{second_cuts} second cuts");
    assert!(max_lost <= 2);
}

/// The review of 2026-11-01, finding 11: a write that lands whole and still fails (a CRC or busy timeout after the
/// card programmed it), at each sector of a whole ride, its start (folders made) and its save included; the card well
/// after it. The ride is saved whole once (the write tried again), the volume clean, a reboot finds nothing to do;
/// at worst an empty file is left in `\Rustfw\Rides` (a create whose entry landed: the retry takes the next name).
#[test]
fn a_write_that_lands_and_still_fails_at_every_sector() {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut orphans, mut renamed, mut dropped, mut lost_one) = (0, 0, 0, 0);
    for n in 0..full.written {
        let what = std::format!("the write of sector {n} of {} failed", full.written);
        let mut d = Card { fails_at: Some(n), ..card(img.clone()) };
        let mut r = Recorder::new();
        let mut out = std::vec![ride(&mut r, &mut d, 0).unwrap()];
        out.extend(io_all(&mut r, &mut d, SECS + 1));
        assert_eq!(d.fails_at, None, "{what}: never written");
        let names: Vec<_> =
            out.iter().filter_map(|o| if let Outcome::Saved(n) = o { Some(*n) } else { None }).collect();
        let [name] = names[..] else { panic!("{what}: {out:?}") };
        renamed += usize::from(name != NAME);
        let path = std::format!("Activities/{}.fit", core::str::from_utf8(&name).unwrap());
        let times = saved(&d.img, &path).unwrap_or_else(|e| panic!("{what}: {e}")).0;
        // the seconds lost: only those dropped while the recorder waited to try again (`RETRY`), as after any
        // failed write
        assert!(times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)), "{what}");
        let missing: Vec<u32> = all.iter().filter(|t| !times.contains(t)).map(|t| t - T0).collect();
        assert!(
            r.dropped <= RETRY && all.len() - times.len() <= r.dropped as usize,
            "{what}: {} dropped, {} missing {:?}..{:?}, {out:?} errors {}",
            r.dropped,
            missing.len(),
            missing.first(),
            missing.last(),
            r.errors
        );
        dropped = dropped.max(r.dropped);
        assert_eq!(r.phase(), Phase::Done, "{what}");
        // a cluster lost at worst: a grow whose FAT write landed, seen failed, the retry taking the next one (FSInfo
        // then one over: the writer never counted it taken)
        let lost = fsck(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        stock_same(&d.img, &[], &what);
        let (free, info) = free_counts(&d.img);
        assert!(lost <= 1 && free <= info && info - free <= lost, "{what}: {lost} lost, FSInfo {info}, {free} free");
        lost_one += lost;
        // what is left in \Rustfw\Rides: empty files only
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let rides = fat.open(&mut ram, "Rustfw/Rides").unwrap();
        let mut c = fat.cursor(rides).unwrap();
        while let Some(e) = fat.list(&mut ram, &mut c).unwrap() {
            if !e.file.dir {
                assert_eq!(e.file.size, 0, "{what}: {:?} left", core::str::from_utf8(e.name()));
                orphans += 1;
            }
        }
        assert_eq!(Recorder::new().recover(&mut d, local(SECS + 5)), Outcome::Nothing, "{what}: the reboot");
    }
    std::println!(
        "{} failed writes: {orphans} empty files left in \\Rustfw\\Rides, {renamed} saved a second later, at most \
         {dropped} s dropped, {lost_one} lost clusters",
        full.written
    );
}

fn card(img: Vec<u8>) -> Card {
    let (budget, prefix, fails_at) = (None, None, None);
    Card { img, budget, written: 0, dead: false, torn: false, prefix, fails_at, cut_at: 0, now: 0, reads: 0 }
}

/// The first sector of file `path` on the volume `img`.
fn first_lba(img: &[u8], path: &str) -> usize {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let c = fat.open(&mut ram, path).unwrap().cluster;
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
    (u(14, 2) + u(16, 1) * u(36, 4) + (c - 2) * u(13, 1)) as usize
}

/// `secs` seconds of ride on `d`, its checkpoints done, not ended (the unit then dies).
fn open_ride(d: &mut Card, secs: u32) -> Recorder {
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in 0..secs {
        r.second(&sample(t), words(t));
        if r.due() {
            assert_eq!(r.io(d, local(t)), Outcome::Nothing);
        }
    }
    r
}

/// B1b-2's review (2): a `ride.rec` of size 0 on a cluster (an older build's create cut before its sectors were
/// written, the cluster an old ride.rec's, its records still there) holds no record: the boot resumes nothing.
#[test]
fn a_ride_rec_without_its_size_holds_no_record() {
    let mut d = card(crate::fat::tests::image());
    let _ = open_ride(&mut d, 200);
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let dir = fat.open(&mut ram, DIR).unwrap();
    let h = fat.locate(&mut ram, dir, Kind::Clean.rec()).unwrap();
    let at = h.entry_lba as usize * SECTOR + usize::from(h.slot) * 32 + 28;
    let mut sized = card(d.img.clone());
    assert!(matches!(Recorder::new().recover(&mut sized, local(201)), Outcome::Resumed(..)), "sized: resumed");
    d.img[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(Recorder::new().recover(&mut d, local(201)), Outcome::Nothing);
}

/// Calls io (as the main loop would, `RETRY` waits included) until it has nothing left to do: what it said.
fn io_all(r: &mut Recorder, d: &mut impl DiskWrite, t: u32) -> Vec<Outcome> {
    let mut out = Vec::new();
    for _ in 0..100 * RETRY {
        if r.due() {
            match r.io(d, local(t)) {
                Outcome::Nothing => {}
                o => out.push(o),
            }
        }
    }
    out
}

/// The second review's R1: one damaged byte in the ride file (a bit error; a sector torn on a card without
/// reliable write) used to fail the save's read-back forever, and every later Start with it. The ride is kept
/// as it is in `\Rustfw\Rides`, said once, and the next ride is recorded and saved; a reboot finds nothing to do.
#[test]
fn a_damaged_ride_is_kept_and_the_next_one_recorded() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let lba = first_lba(&d.img, "Rustfw/Rides/261023100000.fit");
    d.img[lba * SECTOR + 100] ^= 0x55;
    r.end(T0 + 200, words(200));
    assert_eq!(io_all(&mut r, &mut d, 201), [Outcome::Kept(NAME)]);
    assert_eq!((r.phase(), r.failing()), (Phase::Done, false));
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    assert!(fat.open(&mut ram, "Rustfw/Rides/261023100000.fit").is_ok_and(|f| f.size > 5000), "kept");
    assert_eq!(fat.open(&mut ram, SAVED), Err(Error::NotFound));
    assert_eq!(Recorder::new().recover(&mut d, local(300)), Outcome::Nothing);
    let mut r = Recorder::new();
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(*b"261023100001")), "a second later: the name kept");
    assert_eq!(saved(&d.img, "Activities/261023100001.fit").map(|s| s.1), Ok(true));
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// A card that fails every read of one sector of the ride file at the save: tried again [`GIVE_UP`] times (a
/// minute), then the ride is kept and the next one is recorded and saved.
#[test]
fn a_save_that_keeps_failing_is_given_up() {
    struct Bad(Card, usize);
    impl Disk for Bad {
        fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
            let n = buf.len() / SECTOR;
            if (lba as usize..lba as usize + n).contains(&self.1) {
                return Err(Error::Io);
            }
            self.0.read(lba, buf)
        }
    }
    impl DiskWrite for Bad {
        fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
            self.0.write(lba, buf)
        }
    }
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let lba = first_lba(&d.img, "Rustfw/Rides/261023100000.fit") + 1;
    let mut d = Bad(d, lba);
    r.end(T0 + 200, words(200));
    let out = io_all(&mut r, &mut d, 201);
    assert_eq!(out.len(), GIVE_UP as usize, "{out:?}");
    assert!(out[..out.len() - 1].iter().all(|o| *o == Outcome::Failed(Error::Io)));
    assert_eq!((out[out.len() - 1], r.phase()), (Outcome::Kept(NAME), Phase::Done));
    r.start(T0 + 1000, *b"261023110000", false);
    r.second(&sample(1001), words(1));
    r.end(T0 + 1002, words(2));
    assert_eq!(io_all(&mut r, &mut d, 1003), [Outcome::Saved(*b"261023110000")]);
}

/// A card whose writes in a sector range fail as `Corrupt` while on (`.2`): what the card says of a write it
/// refuses for good, as a checkpoint's size into a damaged directory.
struct Refuse(Card, core::ops::Range<u32>, bool);

impl Disk for Refuse {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.0.read(lba, buf)
    }
}

impl DiskWrite for Refuse {
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
        if self.2 && self.1.contains(&lba) {
            return Err(Error::Corrupt); // the ride's directory: its size
        }
        self.0.write(lba, buf)
    }
}

/// A ride whose checkpoint fails as `Corrupt` at the size, after its data sectors, when the last checkpoint's
/// trailer straddled two sectors (its length % 512 == 1): the recorder stopped, its save as it is next (the card's
/// refusal over), and the seconds on the card.
fn stopped_on_two_sectors() -> (Recorder, Refuse, u32) {
    let d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    let mut d = Refuse(d, 0..0, false);
    // a checkpoint every second, a gear change (odd size) now and then, until the file is 512 n + 1 long
    let mut t = 0;
    while r.on_card % SECTOR as u32 != 1 {
        assert!(t < 3000, "no length 512 n + 1");
        r.second(&sample(t), words(t));
        if t % 3 == 0 {
            r.gear(T0 + t, false, 1, (t % 11) as u8 + 1);
        }
        r.checkpoint_now();
        assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing, "t {t}");
        t += 1;
    }
    let dir = first_lba(&d.0.img, "Rustfw/Rides") as u32;
    (d.1, d.2) = (dir..dir + 64, true);
    r.second(&sample(t), words(t)); // over the trailer's second byte, in the next sector
    r.checkpoint_now();
    assert_eq!(r.io(&mut d, local(t)), Outcome::Stopped(Error::Corrupt));
    d.2 = false; // the save's own writes go through
    (r, d, t)
}

/// A checkpoint that fails as `Corrupt` at the size, after its data sectors, when the last checkpoint's trailer
/// straddled two sectors (its length % 512 == 1): the next checkpoint's data went over the trailer's second byte,
/// and the save as it is writes it again ([`trailer_end`]), so the ride is saved, not kept.
#[test]
fn a_ride_stopped_with_its_trailer_across_two_sectors_is_saved() {
    let (mut r, mut d, t) = stopped_on_two_sectors();
    let saved_len = r.on_card;
    assert_eq!(io_all(&mut r, &mut d, t), [Outcome::Saved(NAME)]);
    let (times, _) = saved(&d.0.img, SAVED).unwrap();
    assert_eq!((times.len() as u32, saved_len % SECTOR as u32), (t, 1));
}

/// The second review's R2: Start pressed while the boot's recovery was still pending (the card failed at boot)
/// used to let `recover` take over the new ride's writer: both were lost. One writer, one ride: `recover` does
/// nothing once a ride started, and the start finishes the old ride's save first.
#[test]
fn start_while_the_boot_recovery_is_pending() {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    // a cut during the save: the next boot would finish it
    let cut_img = (0..full.written)
        .rev()
        .map(|cut| {
            let mut d = Card { budget: Some(cut), ..card(img.clone()) };
            let _ = ride(&mut Recorder::new(), &mut d, 0);
            d.img
        })
        .find(|i| matches!(Recorder::new().recover(&mut card(i.clone()), local(SECS + 5)), Outcome::Saved(_)))
        .unwrap();
    let mut d = card(cut_img);
    let mut r = Recorder::new();
    let name2 = *b"261023120000";
    r.start(T0 + 5000, name2, true);
    r.second(&sample(5000), words(0));
    assert_eq!(r.recover(&mut d, local(5000)), Outcome::Nothing);
    assert_eq!(r.phase(), Phase::Starting);
    let mut out = Vec::new();
    for t in 1..200 {
        r.second(&sample(5000 + t), words(t));
        if r.due() && r.io(&mut d, local(5000 + t)) != Outcome::Nothing {
            out.push(t);
        }
    }
    r.end(T0 + 5200, words(200));
    assert_eq!(out, [1], "the old ride saved at the first io");
    assert_eq!(io_all(&mut r, &mut d, 5201), [Outcome::Saved(name2)]);
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    assert_eq!(saved(&d.img, SAVED).map(|s| s.0 == all), Ok(true), "the old ride whole");
    assert_eq!(saved(&d.img, "Activities/261023120000.fit").map(|s| s.0.len()), Ok(200), "the new ride");
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The second review's R6: a ride left open whose boot marker was lost (a flat battery) used to be resumed by
/// the next Start, days later, in place of the new ride. Now it is resumed only if it stopped less than
/// [`RESUME_S`] ago (or the marker said so); else it is ended at its last record and saved, and the new ride
/// starts.
#[test]
fn an_old_ride_left_open_is_saved_and_a_new_one_started() {
    let mut d = card(crate::fat::tests::image());
    let _ = open_ride(&mut d, 200);
    let img = d.img;
    let name2 = *b"261025100000";
    for (later, marker, resumed) in [(2 * 86_400, false, false), (600, false, true), (2 * 86_400, true, true)] {
        let what = std::format!("{later} s later, marker {marker}");
        let mut d = card(img.clone());
        let mut r = Recorder::new();
        r.start(T0 + 200 + later, name2, marker);
        r.second(&sample(200 + later), words(0));
        let out = io_all(&mut r, &mut d, 200);
        assert_eq!(r.phase(), Phase::Recording, "{what}");
        if resumed {
            assert!(matches!(out[..], [Outcome::Resumed(_, NAME)]), "{what}: {out:?}");
            continue;
        }
        assert_eq!(out, [Outcome::Saved(NAME)], "{what}");
        let (times, session) = saved(&d.img, SAVED).unwrap();
        assert!(session && (140..=200).contains(&times.len()), "{what}: {} records", times.len());
        r.end(T0 + 202 + later, words(2));
        assert_eq!(io_all(&mut r, &mut d, 300), [Outcome::Saved(name2)], "{what}");
        assert_eq!(check_clean(&d.img, &what), 0);
    }
}

fn pair() -> [Recorder; 2] {
    [Recorder::new(), Recorder::of(Kind::Full)]
}

/// The two copies of a ride: the clean one in `\Activities` as with one recorder (no developer data), the full
/// one in `\Rustfw\Activities` with the same records and `Alpha1` / `RMSSD` in each; the cost of the second.
#[test]
fn two_copies_cost() {
    let img = crate::fat::tests::image();
    let mut one = card(img.clone());
    ride(&mut Recorder::new(), &mut one, 0).unwrap();
    let mut d = card(img);
    let mut rs = pair();
    assert_eq!(ride_all(&mut rs, &mut d, 0), Some(std::vec![Outcome::Saved(NAME); 2]));
    let read = |img: &[u8], path: &str| {
        let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        crate::fat::tests::read_all(&mut fat, &mut ram, path).unwrap()
    };
    let clean = read(&d.img, SAVED);
    assert_eq!(clean, read(&one.img, SAVED), "the clean copy as with one recorder");
    assert_eq!(clean[1], 0x10);
    let full = read(&d.img, SAVED_FULL);
    assert_eq!(full[1], 0x20);
    let (times, session) = saved(&d.img, SAVED_FULL).unwrap();
    assert_eq!((times, session), saved(&d.img, SAVED).unwrap());
    assert!(full.windows(6).any(|w| w == b"Alpha1"));
    // every beat, in order, five to a message: none in the clean copy
    let fed: Vec<u64> = (0..SECS).flat_map(beats).map(u64::from).collect();
    assert_eq!(saved_beats(&d.img, SAVED_FULL), fed);
    assert!(saved_beats(&d.img, SAVED).is_empty());
    assert_eq!(rs[1].beats_dropped, 0);
    assert_eq!(check_clean(&d.img, "two copies"), 0);
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("recorded_full.fit"), &full).unwrap();
    std::println!(
        "{SECS} s ride: {} sectors written with one copy, {} with two ({} B and {} B files)",
        one.written,
        d.written,
        clean.len(),
        full.len()
    );
    // a second ride the same second: both copies a second later, still one name
    let mut d2 = card(d.img.clone());
    let n2 = *b"261023100001";
    assert_eq!(ride_all(&mut pair(), &mut d2, 0), Some(std::vec![Outcome::Saved(n2); 2]));
    assert!(saved(&d2.img, "Rustfw/Activities/261023100001.fit").is_ok());
    // the cost once the folders are there: a second ride with one copy, and with two
    let mut one2 = card(one.img.clone());
    ride(&mut Recorder::new(), &mut one2, 0).unwrap();
    // the other way (docs/sync.md): the full copy alone, the clean one made from it at the save (its sectors)
    let mut full2 = card(d.img.clone());
    ride(&mut Recorder::of(Kind::Full), &mut full2, 0).unwrap();
    std::println!(
        "the next {SECS} s ride: {} sectors with one copy, {} with two, {} with the full one only (+{} to write \
         the clean file at the save)",
        one2.written,
        d2.written,
        full2.written,
        clean.len().div_ceil(SECTOR)
    );
    assert!(d2.written < 3 * one2.written);
}

// The golden rule with two copies: the power cut (the sector left whole, or torn) after every sector of a ride
// recorded by both. The clean copy comes back as with one recorder (resumed or saved at boot, at most a minute
// lost); the full one is resumed or saved too, or, cut before its first checkpoint, never promised; a full
// one back without its clean one is ended and saved (the app's rule). The next ride gets both, the volume
// stays clean.
//
// Four tests run in parallel, each every fourth cut (`part` 0 to 3 of [`CUT_PARTS`]).
cut_part!(two_copies_power_cut_at_every_sector_part_0, two_copies_cut(0));
cut_part!(two_copies_power_cut_at_every_sector_part_1, two_copies_cut(1));
cut_part!(two_copies_power_cut_at_every_sector_part_2, two_copies_cut(2));
cut_part!(two_copies_power_cut_at_every_sector_part_3, two_copies_cut(3));
cut_part!(two_copies_power_cut_at_every_sector_part_4, two_copies_cut(4));
cut_part!(two_copies_power_cut_at_every_sector_part_5, two_copies_cut(5));
cut_part!(two_copies_power_cut_at_every_sector_part_6, two_copies_cut(6));
cut_part!(two_copies_power_cut_at_every_sector_part_7, two_copies_cut(7));

/// The cuts are shared among this many tests (cut `c` belongs to part `c % CUT_PARTS`), so each sees the whole
/// ride's range of cut points, and the four run in parallel.
const CUT_PARTS: u64 = 8;

fn two_copies_cut_torn(torn: bool, part: u64) {
    let img = crate::fat::tests::image();
    let mut whole = card(img.clone());
    ride_all(&mut pair(), &mut whole, 0).unwrap();
    let total = whole.written;
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut both, mut clean_only, mut none, mut max_lost) = (0, 0, 0, 0);
    for cut in (part..total).step_by(CUT_PARTS as usize) {
        let mut d = Card { budget: Some(cut), torn, ..card(img.clone()) };
        assert!(ride_all(&mut pair(), &mut d, 0).is_none());
        let what = std::format!("torn {torn}, cut {cut} of {total} (sector {})", d.cut_at);
        let had = all.iter().filter(|&&a| a <= T0 + d.now).count();
        let d_now = d.now;
        let torn_at: &[u32] = if torn { &[d.cut_at] } else { &[] };
        stock_same(&d.img, torn_at, &what);
        let mut d = card(d.img);
        let mut rs = pair();
        let o = [rs[0].recover(&mut d, local(SECS + 5)), rs[1].recover(&mut d, local(SECS + 5))];
        if matches!(o[1], Outcome::Resumed(..)) {
            let [clean, full] = &mut rs;
            full.resumed_after(clean); // as App::step_recorder
        }
        match o[0] {
            Outcome::Resumed(w, _) => {
                let from = w[0] + 1;
                let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                let out = ride_all(&mut rs, &mut d, from).unwrap_or_else(|| panic!("{what}"));
                assert_eq!(out[0], Outcome::Saved(NAME), "{what}");
            }
            Outcome::Saved(n) => assert_eq!(n, NAME, "{what}"),
            Outcome::Nothing => {}
            o => panic!("{what}: {o:?}"),
        }
        io_all(&mut rs[1], &mut d, SECS + 10);
        assert_eq!([rs[0].phase(), rs[1].phase()], [Phase::Done; 2], "{what}");
        for (path, kind) in [(SAVED, "clean"), (SAVED_FULL, "full")] {
            if let Ok((times, _)) = saved(&d.img, path) {
                assert!(
                    times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)),
                    "{what}: {kind} {:?}",
                    times.iter().map(|t| t - T0).collect::<Vec<_>>()
                );
                let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
                assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {kind}: {} s lost", had - kept);
            }
        }
        if saved(&d.img, SAVED_FULL).is_ok() {
            // the beats: in order, none twice (values rise), at most a checkpoint's worth lost before the cut
            // (a minute of records, or paused a minute of beats; and the four of an unfinished message),
            // whatever the resume
            // the trainer's fields described before the first record that has them, whatever the resume
            let f = crate::storage::trash::tests::file(&d.img, SAVED_FULL).unwrap();
            let m = crate::fit::tests::decode(&f).unwrap_or_else(|e| panic!("{what}: {e}"));
            let desc = m.iter().position(|x| x.0 == 206 && crate::fit::tests::field(x, 1) == Some(3));
            let first = m.iter().position(|x| x.0 == 20 && crate::fit::tests::field(x, 233).is_some());
            assert!(first.is_none() || (desc.is_some() && desc < first), "{what}: target_w described after use");
            let rr = saved_beats(&d.img, SAVED_FULL);
            assert!(rr.windows(2).all(|p| p[0] < p[1]), "{what}: beats out of order or twice");
            let lost: Vec<u32> = (0..=d_now.min(SECS - 1))
                .filter(|&t| !rr.contains(&u64::from(beats(t)[0])))
                .map(|t| u32::from(beats(t)[0]))
                .collect();
            let ms: u32 = lost.iter().sum();
            assert!(
                lost.len() <= CHECKPOINT_S as usize + 4 || ms <= CHECKPOINT_S * 1000 + 4 * 1150,
                "{what}: {} beats lost, {ms} ms",
                lost.len()
            );
        }
        match (saved(&d.img, SAVED).is_ok(), saved(&d.img, SAVED_FULL).is_ok()) {
            (true, true) => both += 1,
            (true, false) => clean_only += 1,
            (false, true) => panic!("{what}: a full copy without its clean one"),
            _ => none += 1,
        }
        // the next ride: both copies
        let out = ride_all(&mut pair(), &mut d, 0).unwrap_or_else(|| panic!("{what}: the next ride"));
        assert!(matches!(out[..], [Outcome::Saved(a), Outcome::Saved(b)] if a == b), "{what}: {out:?}");
        stock_same(&d.img, torn_at, &what);
        let lost = if torn { fsck_torn(&d.img) } else { fsck(&d.img) }.unwrap_or_else(|e| panic!("{what}: {e}"));
        if !torn {
            max_lost = max_lost.max(lost);
        }
    }
    std::println!(
        "two copies, torn {torn}: {total} cuts: both saved {both}, clean only {clean_only}, neither {none} \
         (cut before the first checkpoint); lost clusters at most {max_lost}"
    );
    assert!(both > total as usize / 2 / CUT_PARTS as usize);
    assert!(max_lost <= 4);
}

const DISCARDED: [&str; 2] = ["Rustfw/Discarded/261023100000.fit", "Rustfw/Discarded/Full/261023100000.fit"];
const RIDING: [&str; 2] = ["Rustfw/Rides/261023100000.fit", "Rustfw/Full/261023100000.fit"];

/// A ride discarded: both copies saved whole into `\Rustfw\Discarded` (the full one into `Full` in it) under its
/// name, nothing in `\Activities` or `\Rustfw\Activities`, nothing left in `\Rustfw`'s ride folders, a clean
/// volume; the next ride is saved as ever, and one discarded under a name taken there a second later, both
/// copies under the same one.
#[test]
fn a_discarded_ride_is_moved_whole() {
    let mut d = card(crate::fat::tests::image());
    let out = ride_all_with(&mut pair(), &mut d, 0, true).unwrap();
    assert_eq!(out, [Outcome::Discarded(NAME); 2]);
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    for p in DISCARDED {
        assert_eq!(saved(&d.img, p).map(|s| s.0), Ok(all.clone()), "{p}");
    }
    for p in [SAVED, SAVED_FULL, RIDING[0], RIDING[1]] {
        assert!(saved(&d.img, p).is_err(), "{p}");
    }
    assert_eq!(check_clean(&d.img, "after the discard"), 0);
    // the next ride, saved: in \Activities as ever
    assert_eq!(ride_all(&mut pair(), &mut d, 0).unwrap(), [Outcome::Saved(*b"261023100001"); 2]);
    // a third, discarded: its name was free in both saved folders, and is in both discarded ones
    let out = ride_all_with(&mut pair(), &mut d, 0, true).unwrap();
    assert_eq!(out, [Outcome::Discarded(*b"261023100002"); 2], "a name free in every folder");
    assert!(saved(&d.img, "Rustfw/Discarded/Full/261023100002.fit").is_ok());
    assert_eq!(check_clean(&d.img, "three rides"), 0);
    // a discard asked once the save has begun (a full card stopped the ride) changes nothing: it is saved
    let mut r = Recorder::new();
    (r.phase, r.ended) = (Phase::Saving, true);
    assert!(!r.discard());
}

/// Folder `path`'s entry in its parent set to cluster 0, as a mkdir torn inside its entry leaves it (its name new,
/// its cluster old). Its old chain stays taken (lost).
fn tear_folder_entry(img: &mut [u8], path: &str) {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let child = fat.open(&mut ram, path).unwrap().cluster;
    let parent = match path.rsplit_once('/') {
        Some((p, _)) => fat.open(&mut ram, p).unwrap().cluster,
        None => fat.root().cluster,
    };
    let dir = fat.lba(parent) as usize * SECTOR;
    let cluster = |e: &[u8]| u32::from(e[20]) << 16 | u32::from(e[21]) << 24 | u32::from(e[26]) | u32::from(e[27]) << 8;
    let at = (dir..dir + fat.cluster_bytes() as usize)
        .step_by(32)
        .find(|&a| img[a + 11] & 0x10 != 0 && cluster(&img[a..a + 32]) == child && ![b'.', 0xe5].contains(&img[a]))
        .unwrap();
    for i in [20, 21, 26, 27] {
        img[at + i] = 0;
    }
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    assert_eq!(fat.open(&mut ram, path).map(|_| ()), Err(Error::Corrupt), "{path}: the reader refuses it");
}

/// The root directory's first cluster's bytes on `img` (the boot sector's fields: fatgen103).
fn root_bytes(img: &[u8]) -> Vec<u8> {
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b));
    let root = (u(14, 2) + u(16, 1) * u(36, 4) + (u(44, 4) - 2) * u(13, 1)) * SECTOR;
    img[root..root + u(13, 1) * SECTOR].to_vec()
}

/// The review of 2026-11-01: a folder entry torn inside it (on cluster 0) is refused as Corrupt by the reader,
/// never taken for the root (".." on cluster 0 still is). `\Rustfw\Discarded\Full`'s: the next Start makes it again
/// (the torn entry marked deleted), both copies of a discarded ride go there, both records done, the root directory
/// never written.
#[test]
fn a_folder_entry_on_cluster_0_is_never_the_root() {
    let mut d = card(crate::fat::tests::image());
    ride_all(&mut pair(), &mut d, 0).unwrap(); // the folders made
    tear_folder_entry(&mut d.img, DISCARDED_PATHS[1]);
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    assert!(fat.open(&mut ram, "Rustfw/..").is_ok_and(|f| f.cluster == fat.root().cluster));
    let before = root_bytes(&d.img);
    let mut rs = pair();
    let out = ride_all_with(&mut rs, &mut d, 0, true).unwrap();
    let name = *b"261023100001";
    assert_eq!(out, [Outcome::Discarded(name); 2]);
    assert_eq!([rs[0].phase(), rs[1].phase()], [Phase::Done; 2], "not stuck");
    assert!(root_bytes(&d.img) == before, "the root directory written");
    assert!(saved(&d.img, "Rustfw/Discarded/Full/261023100001.fit").is_ok(), "in the folder made again");
    assert!(check_clean(&d.img, "the folder made again") <= 1, "its old cluster lost at most");
}

/// The same, `\Rustfw\Discarded` torn while a ride is recorded (its Start found it whole): the discard keeps the
/// ride where it was recorded (`Kept`, its record done, never retried forever), the root directory never written,
/// and the next ride's Start makes the folder again.
#[test]
fn a_discard_into_a_folder_on_cluster_0_keeps_the_ride() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    tear_folder_entry(&mut d.img, DISCARDED_PATHS[0]);
    let before = root_bytes(&d.img);
    r.end(T0 + 200, words(200));
    assert!(r.discard());
    assert_eq!(io_all(&mut r, &mut d, 201), [Outcome::Kept(NAME)]);
    assert_eq!(r.phase(), Phase::Done, "not stuck");
    kept_in_rides(&d.img, "the discard");
    assert!(root_bytes(&d.img) == before, "the root directory written");
    assert!(matches!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(_))), "the next ride");
    assert!(fsck(&d.img).is_ok());
}

/// The review of 2026-11-01: a folder of this firmware's that the recorder needs (`\Rustfw`, `\Rustfw\Rides`) with
/// its entry torn inside it (on cluster 0): made again by the next Start (the torn entry marked deleted, never
/// zeroed), so the ride is recorded and saved; the volume clean, the torn folder's own cluster lost at most.
#[test]
fn a_needed_folder_on_cluster_0_is_made_again() {
    for path in [DIR, "Rustfw/Rides"] {
        let mut ram = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let mut dir = fat.root();
        for name in path.split('/') {
            dir = fat.ensure_dir(&mut ram, dir, name, (0, 0)).unwrap();
        }
        let mut d = card(ram.0);
        tear_folder_entry(&mut d.img, path);
        let mut r = Recorder::new();
        assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(NAME)), "{path}");
        assert_eq!(r.phase(), Phase::Done, "{path}");
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        assert_eq!(saved(&d.img, SAVED).map(|s| s.0 == all), Ok(true), "{path}");
        let lost = check_clean(&d.img, path);
        assert!(lost <= 1, "{path}: {lost} lost");
    }
}

/// Stock's `\Activities` with its entry torn inside it (on cluster 0) is never mended (it would orphan stock's
/// rides): at the save of a ride under way the ride is kept where it was recorded (`Kept`, its record done, never
/// retried forever), the root directory and that entry untouched; the next ride is recorded, and kept the same way.
#[test]
fn stocks_activities_on_cluster_0_is_never_mended() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    tear_folder_entry(&mut d.img, ACTIVITIES);
    let before = root_bytes(&d.img);
    r.end(T0 + 200, words(200));
    assert_eq!(io_all(&mut r, &mut d, 201), [Outcome::Kept(NAME)]);
    assert_eq!(r.phase(), Phase::Done, "not stuck");
    kept_in_rides(&d.img, "kept");
    assert!(root_bytes(&d.img) == before, "the root directory written");
    let mut r = Recorder::new();
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Kept(*b"261023100001")), "the next ride");
    assert_eq!(r.phase(), Phase::Done);
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    assert_eq!(saved(&d.img, "Rustfw/Rides/261023100001.fit").map(|s| s.0 == all), Ok(true));
    assert!(root_bytes(&d.img) == before, "the root directory written");
}

/// B1b-1's re-check (c): while stock's `\Activities` stays torn (on cluster 0, never mended), a Start's setup never
/// finds the volume whole; it synced both FATs (read whole) at every Start, now at the first of a boot only.
#[test]
fn a_torn_activities_syncs_the_fats_once_a_boot() {
    let mut d = card(crate::fat::tests::image());
    ride(&mut Recorder::new(), &mut d, 0).unwrap(); // the folders and the app's files made
    tear_folder_entry(&mut d.img, ACTIVITIES);
    let fat = Fat::mount(&mut crate::fat::tests::Ram(d.img.clone(), 0)).unwrap().fat_sectors() as u64;
    let reads = |r: &mut Recorder, d: &mut Card| {
        let from = d.reads;
        assert!(matches!(ride(r, d, 0), Some(Outcome::Kept(_))), "kept where it was recorded");
        d.reads - from
    };
    let mut r = Recorder::new();
    let (first, second) = (reads(&mut r, &mut d), reads(&mut r, &mut d));
    let next_boot = reads(&mut Recorder::new(), &mut d);
    assert!(first >= second + 2 * fat, "synced at the boot's first Start: {first} sectors read, then {second}");
    assert!(next_boot >= second + 2 * fat, "and at the next boot's: {next_boot}");
}

/// B1b-1's re-check (d): a FILE named `Activities` in the root (a computer's, never removed) is let through as a
/// torn `\Activities` is: every Start used to fail (`ensure_dir` Refused); the ride is recorded and kept where it was
/// recorded, the file and the root directory untouched.
#[test]
fn a_file_named_activities_never_stops_a_ride() {
    let mut d = card(crate::fat::tests::image());
    ride(&mut Recorder::new(), &mut d, 0).unwrap(); // the folders made: the next Start writes no root entry
    // \Activities' entry made a file's, sized to its chain (fsck stays clean)
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let acts = fat.open(&mut ram, ACTIVITIES).unwrap().cluster;
    let root = fat.lba(fat.root().cluster) as usize * SECTOR;
    let cluster = |e: &[u8]| u32::from(e[20]) | u32::from(e[21]) << 8 | u32::from(e[26]) << 16 | u32::from(e[27]) << 24;
    let at = (root..root + fat.cluster_bytes() as usize)
        .step_by(32)
        .find(|&a| d.img[a + 11] == 0x10 && cluster(&d.img[a..a + 32]).rotate_left(16) == acts && d.img[a] != 0xe5)
        .unwrap();
    d.img[at + 11] = 0x20;
    d.img[at + 28..at + 32].copy_from_slice(&fat.cluster_bytes().to_le_bytes());
    assert!(fat.open(&mut crate::fat::tests::Ram(d.img.clone(), 0), ACTIVITIES).is_ok_and(|f| !f.dir));
    let lost = fsck(&d.img).unwrap(); // the rides that were in it, named by nothing now
    let before = root_bytes(&d.img);
    let mut r = Recorder::new();
    // (its name free again: the first ride saved in the folder now named by nothing)
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Kept(NAME)));
    assert_eq!(r.phase(), Phase::Done, "not stuck");
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    assert_eq!(saved(&d.img, "Rustfw/Rides/261023100000.fit").map(|s| s.0 == all), Ok(true));
    assert!(root_bytes(&d.img) == before, "the root directory written");
    assert_eq!(check_clean(&d.img, "kept"), lost);
}

// The golden rule through the discard: the power cut (the sector left whole, or torn) after every sector of a
// ride recorded by both copies and discarded. After the next start each copy is in exactly one place, its ride
// folder (resumed, paused, then discarded again) or `\Rustfw\Discarded`, never in `\Activities` or
// `\Rustfw\Activities`, with at most a minute lost; the volume stays clean. One exception, on a card without
// reliable write only: a torn sector that takes the ride's entry before the end's record (which is the first to
// say "discard") has the ride saved whole from its record at the start (`recover_from`, as for any ride whose
// entry a cut tore), in `\Activities`: in one place still, never lost (HISTORY's DELETE moves it after).
//
// Four tests run in parallel, each every fourth cut ([`CUT_PARTS`]).
cut_part!(a_discard_cut_at_every_sector_part_0, a_discard_cut(0));
cut_part!(a_discard_cut_at_every_sector_part_1, a_discard_cut(1));
cut_part!(a_discard_cut_at_every_sector_part_2, a_discard_cut(2));
cut_part!(a_discard_cut_at_every_sector_part_3, a_discard_cut(3));
cut_part!(a_discard_cut_at_every_sector_part_4, a_discard_cut(4));
cut_part!(a_discard_cut_at_every_sector_part_5, a_discard_cut(5));
cut_part!(a_discard_cut_at_every_sector_part_6, a_discard_cut(6));
cut_part!(a_discard_cut_at_every_sector_part_7, a_discard_cut(7));

/// The cuts of the discard's ride at which the next start finds it already discarded (both torn modes).
const AT_BOOT_CUTS: [u64; 4] = [967, 968, 969, 970];

fn a_discard_cut(part: u64) {
    for torn in [false, true] {
        let img = crate::fat::tests::image();
        let mut whole = card(img.clone());
        ride_all_with(&mut pair(), &mut whole, 0, true).unwrap();
        let total = whole.written;
        let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
        let (mut resumed, mut at_boot, mut none, mut max_lost, mut torn_saved) = (0, 0, 0, 0, 0);
        for cut in (part..total).step_by(CUT_PARTS as usize) {
            let mut d = Card { budget: Some(cut), torn, ..card(img.clone()) };
            assert!(ride_all_with(&mut pair(), &mut d, 0, true).is_none());
            let what = std::format!("torn {torn}, cut {cut} of {total} (sector {})", d.cut_at);
            let had = all.iter().filter(|&&a| a <= T0 + d.now).count();
            let never_saved = |img: &[u8], when: &str| {
                for p in [SAVED, SAVED_FULL] {
                    assert!(saved(img, p).is_err(), "{what}, {when}: {p} saved");
                }
            };
            never_saved(&d.img, "the cut");
            let torn_at: &[u32] = if torn { &[d.cut_at] } else { &[] };
            stock_same(&d.img, torn_at, &what);
            let mut d = card(d.img);
            let mut rs = pair();
            let o = [rs[0].recover(&mut d, local(SECS + 5)), rs[1].recover(&mut d, local(SECS + 5))];
            if matches!(o[1], Outcome::Resumed(..)) {
                let [clean, full] = &mut rs;
                full.resumed_after(clean); // as App::step_recorder
            }
            // each copy in one place after the start: its ride folder (resumed), or discarded
            for k in 0..2 {
                let places = [RIDING[k], DISCARDED[k]].iter().filter(|p| saved(&d.img, p).is_ok()).count();
                assert!(places <= 1, "{what}: copy {k} in {places} places");
            }
            if torn && o.iter().any(|o| matches!(o, Outcome::Saved(_))) {
                // the exception above: each copy in one place all the same
                for (k, p) in [SAVED, SAVED_FULL].iter().enumerate() {
                    let at = [RIDING[k], DISCARDED[k], p].iter().filter(|p| saved(&d.img, p).is_ok()).count();
                    assert!(at <= 1, "{what}: copy {k} in {at} places");
                }
                torn_saved += 1;
                continue;
            }
            never_saved(&d.img, "the start");
            match o[0] {
                Outcome::Resumed(w, _) => {
                    resumed += 1;
                    assert!(!rs[0].discard && !rs[1].discard, "{what}: asked again");
                    let from = w[0] + 1;
                    let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                    let out = ride_all_with(&mut rs, &mut d, from, true).unwrap_or_else(|| panic!("{what}"));
                    assert_eq!(out[0], Outcome::Discarded(NAME), "{what}");
                }
                Outcome::Discarded(n) => {
                    at_boot += 1;
                    assert_eq!(n, NAME, "{what}");
                }
                Outcome::Nothing => none += 1,
                o => panic!("{what}: {o:?}"),
            }
            io_all(&mut rs[1], &mut d, SECS + 10);
            assert_eq!([rs[0].phase(), rs[1].phase()], [Phase::Done; 2], "{what}");
            never_saved(&d.img, "the discard");
            stock_same(&d.img, torn_at, &what);
            for (k, p) in DISCARDED.iter().enumerate() {
                // cut before the first record: nothing promised (its first second may stay in the ride folder,
                // as with a save: never resumed, never in \Activities)
                let promised = o[k] != Outcome::Nothing;
                // a torn sector that took the folder's entry: the discard makes no folder, the ride is kept
                let folder = Fat::mount(&mut d).and_then(|mut f| f.open(&mut d, DISCARDED_PATHS[k])).is_ok();
                if promised && torn && !folder {
                    assert!(saved(&d.img, RIDING[k]).is_ok() && saved(&d.img, p).is_err(), "{what}: {p}: kept");
                    continue;
                }
                assert!(!promised || saved(&d.img, RIDING[k]).is_err(), "{what}: {p}: still riding");
                if let Ok((times, _)) = saved(&d.img, p) {
                    assert!(times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)));
                    let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
                    assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {p}: {} s lost", had - kept);
                }
            }
            assert!(
                saved(&d.img, DISCARDED[0]).is_ok() || saved(&d.img, DISCARDED[1]).is_err(),
                "{what}: a full copy without its clean one"
            );
            let lost = if torn { fsck_torn(&d.img) } else { fsck(&d.img) }.unwrap_or_else(|e| panic!("{what}: {e}"));
            if !torn {
                max_lost = max_lost.max(lost);
            }
        }
        std::println!(
            "discard, torn {torn}: {total} cuts: {resumed} resumed then discarded, {at_boot} discarded at the \
             start, {none} before the first checkpoint, {torn_saved} saved (an entry torn); lost clusters at most \
             {max_lost}"
        );
        assert!(resumed > 50 / CUT_PARTS as usize);
        // the cuts that hit the discard at the start are few (the last writes of the ride's discard, found by
        // printing them): the part that owns each one must meet it, so the whole run asserts what the single
        // test did (`at_boot > 0`). A change of the writes moves them: update the list from the printed counts.
        let owned = AT_BOOT_CUTS.iter().filter(|&&c| c % CUT_PARTS == part).count();
        assert_eq!(at_boot, owned, "discarded at the start: cuts {AT_BOOT_CUTS:?} of {total}");
        assert!(max_lost <= 4);
    }
}

/// A ride resumed after a cut in its discard's first checkpoint (a ride ended before any checkpoint, its record
/// "recording" with the discard's flag) and then saved goes to `\Activities`: the discard is asked again.
#[test]
fn a_ride_resumed_from_a_discard_and_saved_is_saved() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in 0..5 {
        r.second(&sample(t), words(t));
    }
    r.end(T0 + 5, words(5));
    r.discard();
    // the file made and its first checkpoint, "recording": then the cut, before the "saving" one
    let mut fat = Fat::mount(&mut d).unwrap();
    let dir = r.setup(&mut fat, &mut d, (0, 0)).unwrap();
    r.read_records(&mut fat, &mut d).unwrap();
    r.write_record(&mut fat, &mut d, Phase::Starting, 0).unwrap();
    r.file = Some(fat.create(&mut d, dir, "261023100000.fit", (0, 0)).unwrap());
    r.checkpoint(&mut fat, &mut d, (0, 0), Phase::Recording).unwrap();
    let mut r = Recorder::new();
    assert!(matches!(r.recover(&mut d, local(10)), Outcome::Resumed(..)));
    assert!(!r.discard);
    r.end(T0 + 6, words(6));
    assert_eq!(io_all(&mut r, &mut d, 10), [Outcome::Saved(NAME)]);
    assert!(saved(&d.img, SAVED).is_ok());
}

/// A full copy that fails (its file damaged: the save's read-back finds it) is kept as it is and said; the
/// clean one is saved all the same, and the next ride gets both.
#[test]
fn a_failing_full_copy_never_stops_the_clean_one() {
    let mut d = card(crate::fat::tests::image());
    let mut rs = pair();
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    for t in 0..200 {
        for r in rs.iter_mut() {
            r.second(&sample(t), words(t));
            if r.due() {
                assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
            }
        }
    }
    let lba = first_lba(&d.img, "Rustfw/Full/261023100000.fit");
    d.img[lba * SECTOR + 100] ^= 0x55;
    for r in rs.iter_mut() {
        r.end(T0 + 200, words(200));
    }
    assert_eq!(io_all(&mut rs[0], &mut d, 201), [Outcome::Saved(NAME)]);
    assert_eq!(io_all(&mut rs[1], &mut d, 201), [Outcome::Kept(NAME)]);
    assert_eq!(saved(&d.img, SAVED).map(|s| (s.0.len(), s.1)), Ok((200, true)));
    assert!(saved(&d.img, SAVED_FULL).is_err());
    let out = ride_all(&mut pair(), &mut d, 0).unwrap();
    assert_eq!(out, [Outcome::Saved(*b"261023100001"); 2]);
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// Every second of ride from `from` to `to` (excluded) to each recorder, a pass of card work after each.
fn seconds(rs: &mut [Recorder], d: &mut Card, from: u32, to: u32, last: &mut [Outcome]) {
    for t in from..to {
        for r in rs.iter_mut() {
            r.second(&sample(t), words(t));
        }
        pass(rs, d, t, last);
    }
}

/// Seen on the unit (2026-10-24): after a reset mid-ride the clean copy had 22 s fewer than the full one. The
/// full copy's records are bigger, so its buffer ran low ([`LOW`]) and it checkpointed before its minute was up;
/// the clean one waited for its minute. The two checkpoint together now: at any cut the clean copy holds every
/// second the full one does.
#[test]
fn the_clean_copy_is_never_behind_the_full_one() {
    let mut d = card(crate::fat::tests::image());
    let (mut rs, mut last, mut at) = (pair(), [Outcome::Nothing; 2], [0u32; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    for t in 0..SECS {
        seconds(&mut rs, &mut d, t, t + 1, &mut last);
        for (a, r) in at.iter_mut().zip(&rs) {
            if r.since == 0 {
                *a = t; // checkpointed in this pass
            }
        }
        assert!(at[0] >= at[1], "second {t}: the clean copy's checkpoint at {}, the full one's at {}", at[0], at[1]);
    }
    assert_eq!((rs[0].dropped, rs[1].dropped), (0, 0));
}

/// The full copy's folder damaged (its entry in `\Rustfw` points out of the volume): the clean copy still
/// records and saves the next ride.
#[test]
fn a_damaged_full_folder_never_stops_the_clean_copy() {
    let mut d = card(crate::fat::tests::image());
    ride_all(&mut pair(), &mut d, 0).unwrap();
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let c = Fat::mount(&mut ram).unwrap().open(&mut ram, FULL_PATH).unwrap().cluster;
    let lba = first_lba(&d.img, DIR);
    let cluster = |e: &[u8]| u32::from_le_bytes([e[26], e[27], e[20], e[21]]);
    let at = (lba * SECTOR..(lba + 4) * SECTOR) // its entry after the app's files' (`make_kept`)
        .step_by(32)
        .find(|&at| d.img[at] != b'.' && d.img[at + 11] & 0x10 != 0 && cluster(&d.img[at..at + 32]) == c)
        .unwrap();
    d.img[at + 20..at + 22].copy_from_slice(&[0xff, 0x0f]);
    d.img[at + 26..at + 28].copy_from_slice(&[0xf0, 0xff]);
    let out = ride_all(&mut pair(), &mut d, 0).unwrap();
    assert_eq!(out[0], Outcome::Saved(*b"261023100001"));
    let (times, session) = saved(&d.img, "Activities/261023100001.fit").unwrap();
    assert_eq!((times.len(), session), (SECS as usize - 60, true));
}

/// A full copy left open whose clean copy was saved (a cut between the two saves), then a new ride with the
/// boot's marker lost: both copies start the new ride; the old full copy is saved as it was, never carried on
/// with the new ride's seconds (a full copy goes on only with the clean copy's ride).
#[test]
fn the_full_copy_goes_on_only_with_the_clean_copys_ride() {
    let mut d = card(crate::fat::tests::image());
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, &mut d, 0, 200, &mut last);
    for r in rs.iter_mut() {
        r.end(T0 + 200, words(200));
    }
    assert_eq!(io_all(&mut rs[0], &mut d, 200), [Outcome::Saved(NAME)]);
    // the cut: the full copy's save never ran. A new ride ten minutes on
    let new = *b"261023101000";
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 + 599, new, false);
    }
    seconds(&mut rs, &mut d, 600, 700, &mut last);
    for r in rs.iter_mut() {
        r.end(T0 + 700, words(700));
    }
    for _ in 0..3 {
        pass(&mut rs, &mut d, 700, &mut last);
    }
    assert_eq!(last, [Outcome::Saved(new); 2]);
    let (old, _) = saved(&d.img, SAVED_FULL).unwrap();
    assert!(old.iter().all(|&t| t < T0 + 200), "the old full copy as it was: {:?}", old.last());
    let clean = saved(&d.img, "Activities/261023101000.fit").unwrap();
    assert_eq!(saved(&d.img, "Rustfw/Activities/261023101000.fit"), Ok(clean));
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// A card nearly full: what is left is the clean copy's. With room for about one and a half clean rides, the
/// full copy is not made and the clean one is recorded whole and saved.
#[test]
fn a_nearly_full_card_is_left_to_the_clean_copy() {
    let img = crate::fat::tests::image();
    let mut one = card(img.clone());
    ride(&mut Recorder::new(), &mut one, 0).unwrap();
    let keep = (free_counts(&img).0 - free_counts(&one.img).0) * 3 / 2;
    let mut img = img;
    fill(&mut img, keep, true);
    let mut d = card(img);
    let out = ride_all(&mut pair(), &mut d, 0).unwrap();
    assert_eq!(out[0], Outcome::Saved(NAME));
    let (times, session) = saved(&d.img, SAVED).unwrap();
    assert_eq!((times.len(), session), (SECS as usize - 60, true), "the clean copy whole");
    assert!(saved(&d.img, SAVED_FULL).is_err());
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The seconds of a saved ride, checked to be `T0 + 0, T0 + 1, ...` (a ride of [`seconds`] only), cut short
/// (the card refused a checkpoint) but not empty, and without its session (never ended on the card).
fn cut_short(img: &[u8], path: &str) -> usize {
    let (times, session) = saved(img, path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let want: Vec<u32> = (0..times.len() as u32).map(|t| T0 + t).collect();
    assert!(times == want && !times.is_empty() && times.len() < SECS as usize && !session, "{path}: {times:?}");
    times.len()
}

/// A card one cluster short of a ride (the review of 2026-10-25: the refused checkpoint was tried again for
/// ever, the ride never saved and the card never lent): the recording stops at the refused checkpoint and the
/// ride is saved as its last checkpoint left it, a whole FIT file, its folder left empty, a clean volume.
#[test]
fn a_card_one_cluster_short_saves_what_fit() {
    let img = crate::fat::tests::image();
    let mut one = card(img.clone());
    ride(&mut Recorder::new(), &mut one, 0).unwrap();
    let need = free_counts(&img).0 - free_counts(&one.img).0;
    let mut img = img;
    fill(&mut img, need - 1, true);
    let mut d = card(img);
    let mut r = Recorder::new();
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(NAME)));
    assert_eq!((r.phase(), r.errors), (Phase::Done, 1), "stopped at the first refusal, never tried again");
    let (times, session) = saved(&d.img, SAVED).unwrap();
    let want: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    assert!(!session && !times.is_empty() && want.starts_with(&times), "{} of {}", times.len(), want.len());
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    assert_eq!(fat.open(&mut ram, "Rustfw/Rides/261023100000.fit"), Err(Error::NotFound));
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The card filled mid-ride (a computer's copy, say), with both copies: the clean one is saved as far as its
/// last checkpoint; the full one stops at its reserve when FSInfo's count says the card is full, and when that
/// count is stale (it does not see the card fill) the card's refusal stops it and it is saved likewise.
#[test]
fn a_card_full_mid_ride_saves_both_copies_as_far_as_they_fit() {
    for tell in [true, false] {
        let mut d = card(crate::fat::tests::image());
        let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
        for r in rs.iter_mut() {
            r.start(T0 - 1, NAME, false);
        }
        seconds(&mut rs, &mut d, 0, 400, &mut last);
        fill(&mut d.img, 0, tell);
        seconds(&mut rs, &mut d, 400, SECS, &mut last);
        for r in rs.iter_mut() {
            r.end(T0 + SECS, words(SECS));
        }
        for _ in 0..3 {
            pass(&mut rs, &mut d, SECS, &mut last);
        }
        assert_eq!(rs.each_ref().map(|r| r.phase()), [Phase::Done; 2], "tell {tell}");
        assert_eq!(last[0], Outcome::Saved(NAME), "tell {tell}");
        assert!(cut_short(&d.img, SAVED) >= 300);
        if tell {
            assert_eq!(last[1], Outcome::Kept(NAME));
            assert!(saved(&d.img, SAVED_FULL).is_err());
            assert_eq!(check_clean(&d.img, "told"), 0);
        } else {
            assert_eq!(last[1], Outcome::Saved(NAME));
            assert!(cut_short(&d.img, SAVED_FULL) >= 300);
            assert_eq!(fsck(&d.img), Ok(0), "FSInfo's count stale, on purpose");
        }
    }
}

/// A reset on a full card: the boot reopens the ride at its last checkpoint (written again in place: no new
/// cluster), the ride goes on, and the next checkpoint's refusal saves it as far as it fit.
#[test]
fn a_ride_resumed_on_a_full_card_is_saved() {
    let mut d = card(crate::fat::tests::image());
    let (mut r, mut last) = (Recorder::new(), [Outcome::Nothing]);
    r.start(T0 - 1, NAME, false);
    seconds(core::slice::from_mut(&mut r), &mut d, 0, 270, &mut last);
    fill(&mut d.img, 0, true);
    let mut r = Recorder::new(); // the reset: what was in RAM is gone
    assert!(matches!(r.recover(&mut d, local(270)), Outcome::Resumed(..)));
    r.resume(T0 + 280);
    seconds(core::slice::from_mut(&mut r), &mut d, 280, SECS, &mut last);
    r.end(T0 + SECS, words(SECS));
    for _ in 0..3 {
        pass(core::slice::from_mut(&mut r), &mut d, SECS, &mut last);
    }
    assert_eq!((r.phase(), last[0]), (Phase::Done, Outcome::Saved(NAME)));
    let (times, session) = saved(&d.img, SAVED).unwrap();
    assert!(!session && times.len() >= 200 && times.iter().all(|&t| t < T0 + SECS), "{times:?}");
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// A damaged record is never taken.
#[test]
fn a_damaged_record_is_refused() {
    let r = |seq, phase| {
        let ride = [seq; RIDE_WORDS];
        Rec {
            seq,
            phase,
            name: NAME,
            cluster: 7,
            fit: [0; RECORD_LEN],
            ride,
            discard: false,
            checked: None,
            lap: [seq; crate::fit::LAP_WORDS],
            more: [seq + 1; crate::ride::MORE_WORDS],
            timer: seq | 1 << 31,
            by_hand: false,
        }
    };
    let s = r(5, Phase::Recording).to_sector();
    assert_eq!(Rec::from_sector(&s), Some(r(5, Phase::Recording)));
    // the discard's flag, byte 22; 0 in the records of the builds before it
    let d = Rec { discard: true, ..r(6, Phase::Saving) };
    assert_eq!((d.to_sector()[22], s[22]), (1, 0));
    // the chain's mark, byte 23; 0 in the records of the builds before it, which ignore it
    let c = Rec { checked: Some(0xbeef), ..r(7, Phase::Recording) };
    assert_eq!((c.to_sector()[23], s[23]), (1, 0));
    assert_eq!(Rec::from_sector(&c.to_sector()), Some(c));
    assert_eq!(Rec::from_sector(&d.to_sector()), Some(d));
    // paused with Start, byte 30; 0 in the records of the builds before it (read as not paused by hand)
    let h = Rec { by_hand: true, ..r(8, Phase::Recording) };
    assert_eq!((h.to_sector()[30], s[30]), (1, 0));
    assert_eq!(Rec::from_sector(&h.to_sector()), Some(h));
    // the lap sums, bytes 192..228: zero in the records of the builds before them, read as no sums (no new tag)
    let mut old = s;
    old[192..228].fill(0);
    let crc = crate::fit::crc(0, &old[..SECTOR - 2]);
    old[SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(Rec::from_sector(&old).map(|r| r.lap), Some([0; crate::fit::LAP_WORDS]));
    // the totals beyond the ride words, bytes 228..280 and the ride's word 15 (188..192): zero in the records of the
    // builds before them, read as none (no new tag); nothing after them
    assert_eq!(s[228..280], [6u8, 0, 0, 0].repeat(13)[..]);
    // the FIT timer's word, bytes 280..284 (zero in the records of the builds before it: `Writer::with_timer_word`
    // then keeps its guess); nothing after it
    assert_eq!(s[280..284], (5u32 | 1 << 31).to_le_bytes());
    assert!(s[284..SECTOR - 2].iter().all(|&b| b == 0));
    let mut old = s;
    old[188..192].fill(0);
    old[228..280].fill(0);
    let crc = crate::fit::crc(0, &old[..SECTOR - 2]);
    old[SECTOR - 2..].copy_from_slice(&crc.to_le_bytes());
    let back = Rec::from_sector(&old).unwrap();
    assert_eq!((back.more, back.ride[15]), ([0; crate::ride::MORE_WORDS], 0));
    for at in [0, 4, 8, 9, 24, 40, 100, 511] {
        let mut bad = s;
        bad[at] ^= 0x10;
        assert_eq!(Rec::from_sector(&bad), None, "byte {at}");
    }
    // sealed whole, but another tag, or another count of ride words (another layout): no record either
    for at in [0, 21] {
        let mut other = s;
        other[at] ^= 0x10;
        crate::storage::seal16(&mut other);
        assert_eq!(Rec::from_sector(&other), None, "sealed, byte {at}");
    }
}

/// The `Discarded` entry in `\Rustfw`'s first directory sector.
fn discarded_entry(img: &[u8]) -> usize {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let c = Fat::mount(&mut ram).unwrap().open(&mut ram, DISCARDED_PATHS[0]).unwrap().cluster;
    let lba = first_lba(img, DIR);
    let cluster = |e: &[u8]| u32::from_le_bytes([e[26], e[27], e[20], e[21]]);
    (lba * SECTOR..(lba + 1) * SECTOR)
        .step_by(32)
        .find(|&at| img[at] != b'.' && img[at + 11] & 0x10 != 0 && cluster(&img[at..at + 32]) == c)
        .unwrap()
}

/// REVIEW: the folder's entry (and its long name pieces) marked deleted.
/// Removes `\Rustfw\Discarded` (and its long name) as a computer or an older build might leave it.
fn drop_discarded(img: &mut [u8]) {
    let mut at = discarded_entry(img);
    img[at] = 0xe5;
    while at >= 32 && img[at - 32 + 11] == 0x0f {
        at -= 32;
        img[at] = 0xe5;
    }
}

/// REVIEW: a file named Discarded in \Rustfw (or a damaged Discarded folder): does the clean copy still record?
/// The discard's folders are optional: damaged, or a file in their name, the next ride is saved all the same.
#[test]
fn a_bad_discarded_folder_never_stops_a_ride() {
    for damaged in [true, false] {
        let mut d = card(crate::fat::tests::image());
        ride_all(&mut pair(), &mut d, 0).unwrap();
        if damaged {
            let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
            let c = Fat::mount(&mut ram).unwrap().open(&mut ram, DISCARDED_PATHS[0]).unwrap().cluster;
            let lba = first_lba(&d.img, DIR);
            let cluster = |e: &[u8]| u32::from_le_bytes([e[26], e[27], e[20], e[21]]);
            let at = (lba * SECTOR..(lba + 1) * SECTOR)
                .step_by(32)
                .find(|&at| d.img[at] != b'.' && d.img[at + 11] & 0x10 != 0 && cluster(&d.img[at..at + 32]) == c)
                .unwrap();
            d.img[at + 20..at + 22].copy_from_slice(&[0xff, 0x0f]);
            d.img[at + 26..at + 28].copy_from_slice(&[0xf0, 0xff]);
        } else {
            // as a computer might leave it: the folder gone, a file in its name
            drop_discarded(&mut d.img);
            let mut fat = Fat::mount(&mut d).unwrap();
            let dir = fat.open(&mut d, DIR).unwrap();
            fat.create(&mut d, dir, "Discarded", (0, 0)).unwrap();
        }
        let out = ride_all(&mut pair(), &mut d, 0);
        assert_eq!(out.unwrap()[0], Outcome::Saved(*b"261023100001"), "damaged={damaged}");
    }
}

// REVIEW: a ride resumed at boot (setup never ran for it) on a volume without the Discarded folders (a ride open
// at the update), discarded, the power cut (torn) at every sector: is the ride ever lost?
// A discard never makes a folder: a ride resumed with the folders missing, then discarded, is in one place
// whatever the cut.
cut_part!(a_resumed_ride_discarded_without_folders_survives_every_cut_untorn, resumed_discard_cuts(false));
cut_part!(a_resumed_ride_discarded_without_folders_survives_every_cut_torn, resumed_discard_cuts(true));

fn resumed_discard_cuts(torn: bool) {
    let img = crate::fat::tests::image();
    // mid-ride cut
    let mut whole = card(img.clone());
    ride_all(&mut pair(), &mut whole, 0).unwrap();
    let mut d = Card { budget: Some(whole.written / 2), ..card(img.clone()) };
    assert!(ride_all(&mut pair(), &mut d, 0).is_none());
    let mut d = card(d.img);
    // the Discarded folder (and Full in it) gone: as an older build left the volume
    drop_discarded(&mut d.img);
    assert!(Fat::mount(&mut d).unwrap().open(&mut d, DISCARDED_PATHS[0]).is_err());
    let base = d.img.clone();
    let run = |budget: Option<u64>, torn: bool| -> (Card, Vec<Outcome>) {
        let mut d = Card { budget, torn, ..card(base.clone()) };
        let mut rs = pair();
        let o = [rs[0].recover(&mut d, local(SECS)), rs[1].recover(&mut d, local(SECS))];
        if matches!(o[1], Outcome::Resumed(..)) {
            let [c, f] = &mut rs;
            f.resumed_after(c);
        }
        let mut out = o.to_vec();
        if let Outcome::Resumed(w, _) = o[0] {
            let from = w[0] + 1;
            let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
            if let Some(v) = ride_all_with(&mut rs, &mut d, from, true) {
                out.extend(v);
            }
        }
        (d, out)
    };
    let (d0, _) = run(None, false);
    let total = d0.written;
    let mut bad = Vec::new();
    {
        for cut in 0..total {
            let (d, _) = run(Some(cut), torn);
            let at = d.cut_at;
            // reboot and finish
            let mut d = card(d.img);
            let mut rs = pair();
            let o = [rs[0].recover(&mut d, local(SECS + 5)), rs[1].recover(&mut d, local(SECS + 5))];
            if matches!(o[1], Outcome::Resumed(..)) {
                let [c, f] = &mut rs;
                f.resumed_after(c);
            }
            if matches!(o[0], Outcome::Resumed(..)) {
                rs[0].end(T0 + SECS, words(SECS));
                rs[0].discard();
                io_all(&mut rs[0], &mut d, SECS + 10);
            }
            io_all(&mut rs[1], &mut d, SECS + 10);
            for (k, p) in [SAVED, SAVED_FULL].iter().enumerate() {
                let n = [RIDING[k], DISCARDED[k], p].iter().filter(|q| saved(&d.img, q).is_ok()).count();
                if n != 1 {
                    bad.push(std::format!("torn {torn} cut {cut} (lba {at}): copy {k} in {n} places, boot {o:?}"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} bad cuts of {}: {bad:?}", bad.len(), total);
}

/// `\Activities` left with `left` free slots in its directory's last sector: deleted entries (0xe5) written from
/// its end marker on (in its chain's last cluster) up to there, as many saves and deletes leave it.
fn crowd(img: &mut [u8], left: usize) {
    let u =
        |img: &[u8], at: usize, n: usize| img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b));
    let (spc, fat0, fats, fat_len) = (u(img, 13, 1), u(img, 14, 2), u(img, 16, 1), u(img, 36, 4));
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut c = Fat::mount(&mut ram).unwrap().open(&mut ram, ACTIVITIES).unwrap().cluster as usize;
    while let n @ 2..0x0fff_fff0 = u(img, fat0 * SECTOR + 4 * c, 4) & 0x0fff_ffff {
        c = n;
    }
    let at = (fat0 + fats * fat_len + (c - 2) * spc) * SECTOR;
    let end = at + spc * SECTOR - 32 * left;
    let mut e = at;
    while img[e] != 0 {
        e += 32;
    }
    assert!(e < end, "room to crowd");
    for k in (e..end).step_by(32) {
        img[k] = 0xe5;
    }
}

/// The ride's file is named by an entry: saved whole in `\Activities`, or where it was recorded; at most a minute
/// lost (its last checkpoint).
fn named(img: &[u8], what: &str) {
    let (times, _) = saved(img, SAVED)
        .or_else(|_| saved(img, "Rustfw/Rides/261023100000.fit"))
        .unwrap_or_else(|e| panic!("{what}: the ride named by no entry ({e})"));
    assert!(times.len() >= 400 - CHECKPOINT_S as usize, "{what}: {} s", times.len());
}

/// The review of 2026-10-26 (high): a save whose `\Activities` must grow for the new entry (under 3 free slots in
/// its last sector) on a full card removed the ride's entry (step 4), could not link the new one (5: no cluster),
/// then wrote the record "done": the chain named by nothing, the ride lost. The room is made before 4 now: on a
/// full card the ride is kept where it was, with a cluster free it is saved into the grown folder. The power cut
/// at every sector of the end and the save, then a reboot: the ride is always named, never more than a minute
/// short, the volume clean.
#[test]
fn a_save_into_a_full_folder_on_a_full_card_keeps_the_ride() {
    let run = |keep: u32, cut: Option<u64>| -> (Card, Outcome, u64) {
        let mut d = card(crate::fat::tests::image());
        let (mut r, mut last) = (Recorder::new(), [Outcome::Nothing]);
        r.start(T0 - 1, NAME, false);
        seconds(core::slice::from_mut(&mut r), &mut d, 0, 400, &mut last);
        crowd(&mut d.img, 2);
        fill(&mut d.img, keep, true);
        let from = d.written;
        d.budget = cut.map(|k| from + k);
        r.end(T0 + 400, words(400));
        for _ in 0..3 {
            pass(core::slice::from_mut(&mut r), &mut d, 400, &mut last);
        }
        let n = d.written - from;
        (d, last[0], n)
    };
    let mut outcomes = Vec::new();
    for keep in [0, 1, 2, 3] {
        let (d, o, total) = run(keep, None);
        let what = std::format!("{keep} clusters free");
        outcomes.push(o);
        assert!(matches!(o, Outcome::Saved(NAME) | Outcome::Kept(NAME)), "{what}: {o:?}");
        named(&d.img, &what);
        assert_eq!(fsck(&d.img).map_err(|e| std::format!("{what}: {e}")), Ok(0));
        for cut in 0..total {
            let what = std::format!("{keep} clusters free, cut {cut} of {total}");
            let (d, _, _) = run(keep, Some(cut));
            let mut d = card(d.img);
            let mut r = Recorder::new();
            if let Outcome::Resumed(..) = r.recover(&mut d, local(500)) {
                r.end(T0 + 400, words(400));
                io_all(&mut r, &mut d, 500);
            }
            assert_eq!(r.phase(), Phase::Done, "{what}");
            named(&d.img, &what);
            if let Err(e) = fsck(&d.img) {
                panic!("{what}: {e}");
            }
        }
    }
    assert_eq!(outcomes[0], Outcome::Kept(NAME), "a full card: kept where it was");
    assert!(outcomes.contains(&Outcome::Saved(NAME)), "saved into the grown folder: {outcomes:?}");
}

/// A card whose writes fail in some sector ranges (`bad`, while `on`) and whose reads fail in others (`unread`).
struct Bad {
    card: Card,
    bad: Vec<core::ops::Range<u32>>,
    unread: Vec<core::ops::Range<u32>>,
    on: bool,
}

impl Disk for Bad {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        if self.on && self.unread.iter().any(|r| r.start < lba + n && lba < r.end) {
            return Err(Error::Io);
        }
        self.card.read(lba, buf)
    }
}

impl DiskWrite for Bad {
    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
        let n = (buf.len() / SECTOR) as u32;
        if self.on && self.bad.iter().any(|r| r.start < lba + n && lba < r.end) {
            return Err(Error::Io);
        }
        self.card.write(lba, buf)
    }
}

/// A folder's first cluster's sectors (64: more than the test volume's cluster).
fn dir_lbas(img: &[u8], path: &str) -> core::ops::Range<u32> {
    let lba = first_lba(img, path) as u32;
    lba..lba + 64
}

/// The ride whole where it was recorded.
fn kept_in_rides(img: &[u8], what: &str) {
    let (times, session) = saved(img, "Rustfw/Rides/261023100000.fit").unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(session && times.len() == 200, "{what}: {} s", times.len());
    assert!(saved(img, SAVED).is_err(), "{what}: one entry");
}

/// The second half of that finding: a save given up ([`GIVE_UP`] card errors) between its steps 4 and 5 (the
/// writes of `\Activities`' directory failing) wrote "done" over the record, the only thing that still named the
/// ride. Now the ride is named again where it was recorded (the room made in step 3), its record done, "Kept":
/// the next ride records as ever.
#[test]
fn a_save_failing_between_steps_4_and_5_keeps_the_ride_in_its_folder() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    let out = io_all(&mut r, &mut d, 201);
    assert_eq!(out.len(), GIVE_UP as usize, "{out:?}");
    assert_eq!((out[out.len() - 1], r.phase(), r.stuck()), (Outcome::Kept(NAME), Phase::Done, false));
    kept_in_rides(&d.card.img, "kept");
    assert_eq!(check_clean(&d.card.img, "kept"), 0);
    d.on = false;
    const NEXT: [u8; 12] = *b"261023110000";
    r.start(T0 + 1000, NEXT, false);
    r.second(&sample(1001), words(1));
    r.end(T0 + 1002, words(2));
    assert_eq!(io_all(&mut r, &mut d, 1003), [Outcome::Saved(NEXT)]);
    kept_in_rides(&d.card.img, "after the next ride");
}

/// The re-review of 4748752 (high): a cut between the save's steps 4 and 5, then a boot whose reads of the ride
/// fail ([`GIVE_UP`] times at its step 2): the boot gave up in memory only, and the next Start's records went
/// over the only record that named the ride. The give-up names it again in its folder first now.
#[test]
fn a_save_cut_after_step_4_then_failing_at_boot_keeps_the_ride() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let ride = first_lba(&d.img, "Rustfw/Rides/261023100000.fit") as u32;
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    r.io(&mut d, local(201)); // steps 1 to 4 done, 5 failed: the unit off now
    assert_eq!(r.fails(), 1);
    let mut d = Bad { card: card(d.card.img), bad: Vec::new(), unread: std::vec![ride + 1..ride + 2], on: true };
    let mut r = Recorder::new();
    assert_eq!(r.recover(&mut d, local(300)), Outcome::Failed(Error::Io));
    let out = io_all(&mut r, &mut d, 301);
    assert_eq!(out.last(), Some(&Outcome::Kept(NAME)), "{out:?}");
    // the next ride, its records where the old one's was: the old ride named all the same
    const NEXT: [u8; 12] = *b"261023110000";
    r.start(T0 + 1000, NEXT, false);
    r.second(&sample(1001), words(1));
    r.end(T0 + 1002, words(2));
    assert_eq!(io_all(&mut r, &mut d, 1003), [Outcome::Saved(NEXT)]);
    kept_in_rides(&d.card.img, "after the next ride");
    assert_eq!(check_clean(&d.card.img, "after"), 0);
}

/// A save that can neither finish nor name the ride again (`\Activities`' and `\Rustfw\Rides`' directories both
/// failing): stuck, never "done", its record never overwritten, tried again with a back-off, by the same run, a
/// boot (the marker kept) and a Start (the marker lost); a Start meanwhile waits behind it (`held`), and once
/// the card is well the old ride is saved and the new one recorded, its timer stopped from its start until then.
#[test]
fn a_stuck_save_holds_the_next_ride_until_the_card_is_well() {
    const NEXT: [u8; 12] = *b"261023110000";
    let stuck = |o: &[Outcome]| !o.is_empty() && o.iter().all(|o| *o == Outcome::Failed(Error::Io));
    // the old ride saved first, then the new one recorded and saved
    let finish = |r: &mut Recorder, d: &mut Bad, what: &str| {
        d.on = false; // the card well again
        assert_eq!(io_all(r, d, 1001), [Outcome::Saved(NAME)], "{what}: the old save finished first");
        assert_eq!(r.phase(), Phase::Recording, "{what}: then the new ride, its file made");
        r.second(&sample(1001), words(1));
        r.end(T0 + 1002, words(2));
        assert_eq!(io_all(r, d, 1003), [Outcome::Saved(NEXT)], "{what}");
        assert_eq!(saved(&d.card.img, SAVED).map(|s| s.1), Ok(true), "{what}");
        let (times, session) = saved(&d.card.img, "Activities/261023110000.fit").unwrap();
        assert!(session && times == [T0 + 1001], "{what}: {times:?}");
        // its timer stopped from its start until the first second recorded: when it was not recorded
        let mut ram = crate::fat::tests::Ram(d.card.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let v = crate::fat::tests::read_all(&mut fat, &mut ram, "Activities/261023110000.fit").unwrap();
        let events: Vec<(Option<u64>, Option<u64>)> = crate::fit::tests::decode(&v)
            .unwrap()
            .iter()
            .filter(|x| x.0 == 21)
            .map(|x| (crate::fit::tests::field(x, 253), crate::fit::tests::field(x, 1)))
            .collect();
        let at = |t: u32| Some(u64::from(T0 + t));
        assert_eq!(events[..3], [(at(1000), Some(0)), (at(1000), Some(4)), (at(1001), Some(0))], "{what}");
        assert_eq!(check_clean(&d.card.img, what), 0);
    };
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES), dir_lbas(&d.img, "Rustfw/Rides")];
    let mut d = Bad { card: d, bad: bad[..1].to_vec(), unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    assert_eq!(r.io(&mut d, local(201)), Outcome::Failed(Error::Io), "the end on the card, step 4 done, 5 not");
    d.bad = bad.clone(); // and the rides folder failing from here
    let mut out = std::vec![Outcome::Failed(Error::Io)];
    out.extend(io_all(&mut r, &mut d, 201));
    assert!(stuck(&out) && out.len() > GIVE_UP as usize, "{out:?}");
    assert!(out.len() < 100 / 4, "backed off: {} tries", out.len());
    assert_eq!((r.phase(), r.stuck()), (Phase::Saving, true), "never given up");
    let left = d.card.img.clone(); // the ride named by its record alone

    // the same run: a Start waits behind the save, which goes on being tried
    r.start(T0 + 1000, NEXT, false);
    assert!(r.starting() && r.held());
    assert!(stuck(&io_all(&mut r, &mut d, 1001)));
    finish(&mut r, &mut d, "same run");

    // a boot (the marker kept): the save tried again by io, a Start waits
    let mut d = Bad { card: card(left.clone()), bad: bad.clone(), unread: Vec::new(), on: true };
    let mut r = Recorder::new();
    assert_eq!(r.recover(&mut d, local(300)), Outcome::Failed(Error::Io));
    assert_eq!(r.phase(), Phase::Saving);
    assert!(stuck(&io_all(&mut r, &mut d, 301)));
    r.start(T0 + 1000, NEXT, false);
    assert!(r.held());
    finish(&mut r, &mut d, "boot");

    // the marker lost: a Start finds the save in the record, gives it its retries, then waits behind it
    let mut d = Bad { card: card(left), bad, unread: Vec::new(), on: true };
    let mut r = Recorder::new();
    r.start(T0 + 1000, NEXT, false);
    assert!(stuck(&io_all(&mut r, &mut d, 1001)));
    assert_eq!(r.phase(), Phase::Saving);
    assert!(r.held(), "the new ride not recorded: said");
    finish(&mut r, &mut d, "start");
}

/// The re-review of 4748752: the full copy's ended ride waited in RAM for the clean one's save (its end, up to a
/// minute of it, lost to a power cut meanwhile). Its end goes on the card ("saving") at once now; only its save
/// waits.
#[test]
fn the_full_copys_end_is_on_the_card_while_the_clean_one_saves() {
    // `held`: a Start right after the end, held behind the save (the review of 2026-10-31, finding 3: io_after)
    for held in [false, true] {
        let mut d = card(crate::fat::tests::image());
        let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
        for r in rs.iter_mut() {
            r.start(T0 - 1, NAME, false);
        }
        seconds(&mut rs, &mut d, 0, 90, &mut last);
        let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)]; // the clean copy's save retried
        let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
        for r in rs.iter_mut() {
            r.end(T0 + 90, words(90));
        }
        let [clean, full] = &mut rs;
        if held {
            clean.start(T0 + 91, *b"261023100131", false);
            full.start_after(clean, T0 + 91, *b"261023100131", false);
            assert!(clean.pending() && full.pending());
        }
        assert_eq!(clean.io(&mut d, local(90)), Outcome::Failed(Error::Io));
        assert_eq!(full.io_after(clean, &mut d, local(90)), Outcome::Nothing);
        assert_eq!((clean.phase(), full.phase()), (Phase::Saving, Phase::Saving), "both ends on the card, {held}");
        // a power cut now: the full copy is saved whole at the boot
        let mut f = Recorder::of(Kind::Full);
        let mut c = card(d.card.img.clone());
        assert_eq!(f.recover(&mut c, local(100)), Outcome::Saved(NAME));
        let (times, session) = saved(&c.img, SAVED_FULL).unwrap();
        assert!(session && times.len() == 90, "{held}: {}", times.len());
    }
}

/// A Start after a full card stopped the ride (its save as it is under way, the ride not ended) was ignored; it
/// is held as any Start during a save.
#[test]
fn a_start_during_a_save_as_it_is_is_held() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    (r.phase, r.stopped) = (Phase::Saving, true); // as io leaves it at a refused checkpoint
    r.start(T0 + 300, *b"261023110000", false);
    assert!(r.pending());
}

/// The review of 2026-10-26: a Start while the last ride was still being saved was ignored ("NOT RECORDING"),
/// and the whole new ride went unrecorded. It starts once the save is done now, both copies, each saved after.
#[test]
fn a_start_during_the_last_save_is_recorded() {
    let mut d = card(crate::fat::tests::image());
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, &mut d, 0, 200, &mut last);
    const NEXT: [u8; 12] = *b"261023110000";
    for r in rs.iter_mut() {
        r.end(T0 + 200, words(200));
        r.start(T0 + 3599, NEXT, false); // before the pass that saves the last one
        assert!(r.starting());
    }
    seconds(&mut rs, &mut d, 3600, 3800, &mut last);
    for r in rs.iter_mut() {
        r.end(T0 + 3800, words(3800));
    }
    for _ in 0..3 {
        pass(&mut rs, &mut d, 3800, &mut last);
    }
    assert_eq!(last, [Outcome::Saved(NEXT); 2]);
    for p in [SAVED, SAVED_FULL] {
        assert_eq!(saved(&d.img, p).map(|s| (s.0.len(), s.1)), Ok((200, true)), "{p}");
    }
    for p in ["Activities/261023110000.fit", "Rustfw/Activities/261023110000.fit"] {
        let (times, session) = saved(&d.img, p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(session && times.len() >= 198, "{p}: {} s", times.len());
    }
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The review of 2026-10-26: each copy picked its saved name against its own folder only, so a name taken in
/// `\Activities` during the ride (stock's, a computer's) gave the two copies different names, which the sync and
/// HISTORY's DELETE pair by. The full copy takes the name the clean one was saved under now. (One taken in
/// `\Rustfw\Activities` still parts them: only this firmware writes there, never during a ride.)
#[test]
fn both_copies_keep_one_name() {
    let mut d = card(crate::fat::tests::image());
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, &mut d, 0, 100, &mut last);
    let mut fat = Fat::mount(&mut d).unwrap();
    let dir = fat.open(&mut d, ACTIVITIES).unwrap();
    fat.create(&mut d, dir, "261023100000.fit", (0, 0)).unwrap();
    seconds(&mut rs, &mut d, 100, 200, &mut last);
    for r in rs.iter_mut() {
        r.end(T0 + 200, words(200));
    }
    for _ in 0..3 {
        pass(&mut rs, &mut d, 200, &mut last);
    }
    const ONE_ON: [u8; 12] = *b"261023100001";
    assert_eq!(last, [Outcome::Saved(ONE_ON); 2]);
    for p in ["Activities/261023100001.fit", "Rustfw/Activities/261023100001.fit"] {
        assert_eq!(saved(&d.img, p).map(|s| s.1), Ok(true), "{p}");
    }
}

/// The full copy's reserve ([`FULL_RESERVE`]) with FSInfo's free count unknown (0xffffffff, as a formatter may
/// leave it): it was skipped, the full copy then taking the clean one's last clusters. The count is made now.
#[test]
fn a_nearly_full_card_with_no_free_count_is_left_to_the_clean_copy() {
    let img = crate::fat::tests::image();
    let mut one = card(img.clone());
    ride(&mut Recorder::new(), &mut one, 0).unwrap();
    let keep = (free_counts(&img).0 - free_counts(&one.img).0) * 3 / 2;
    let mut d = card(img);
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, &mut d, 0, 100, &mut last); // the folders made (which counts the free clusters)
    fill(&mut d.img, keep, false);
    let fsinfo = (usize::from(d.img[48]) | usize::from(d.img[49]) << 8) * SECTOR;
    d.img[fsinfo + 488..fsinfo + 492].copy_from_slice(&u32::MAX.to_le_bytes());
    seconds(&mut rs, &mut d, 100, SECS, &mut last);
    for r in rs.iter_mut() {
        r.end(T0 + SECS, words(SECS));
    }
    for _ in 0..3 {
        pass(&mut rs, &mut d, SECS, &mut last);
    }
    assert_eq!(last, [Outcome::Saved(NAME), Outcome::Kept(NAME)]);
    assert!(saved(&d.img, SAVED).is_ok_and(|s| s.1), "the clean copy whole");
    assert!(saved(&d.img, SAVED_FULL).is_err());
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The final review (medium): a cut after the save's step 4, then a boot whose card cannot be mounted (a flaky
/// card, a slow identification) [`GIVE_UP`] times: the boot's recovery concluded `Kept` without having read the
/// record (the boot's marker cleared, the drive lent writable over a nameless chain). It never concludes now
/// while the record is unread: failing, stuck past its retries (the drive lent read only), until the card mounts;
/// then the save is finished.
#[test]
fn a_boot_that_cannot_read_the_record_never_concludes() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    r.io(&mut d, local(201)); // steps 1 to 4 done, 5 failed: the unit off now
    let mut d = Bad { card: card(d.card.img), bad: Vec::new(), unread: std::vec![0..1], on: true };
    let mut r = Recorder::new();
    let mut out = Vec::new();
    for _ in 0..100 * RETRY {
        match r.recover(&mut d, local(300)) {
            Outcome::Nothing => {}
            o => out.push(o),
        }
    }
    assert!(out.len() > GIVE_UP as usize && out.iter().all(|o| *o == Outcome::Failed(Error::Io)), "{out:?}");
    assert!(r.stuck() && r.waiting(), "stuck, tried again (backed off)");
    d.on = false; // the card mounts
    let mut o = Outcome::Nothing;
    for _ in 0..64 * RETRY {
        o = r.recover(&mut d, local(400));
        if o != Outcome::Nothing {
            break;
        }
    }
    assert_eq!((o, r.stuck()), (Outcome::Saved(NAME), false));
    assert_eq!(saved(&d.card.img, SAVED).map(|s| s.1), Ok(true));
}

/// The final review: a ride left open whose entry in `\Rustfw\Rides` is gone (a torn directory sector), its
/// recovery failing (here the record's copy of its last sector unreadable): it was given up as a ride left open
/// (`Kept`, its record overwritten by the next ride's), its chain then named by nothing. It is a save from the
/// moment its entry is found gone: given up, it is named again in its folder first.
#[test]
fn a_ride_left_open_without_its_entry_is_never_lost() {
    let mut d = card(crate::fat::tests::image());
    let _ = open_ride(&mut d, 200); // the unit off with the ride open
    let mut fat = Fat::mount(&mut d).unwrap();
    let rides = fat.open(&mut d, "Rustfw/Rides").unwrap();
    let h = fat.locate(&mut d, rides, "261023100000.fit").unwrap();
    fat.remove(&mut d, &h).unwrap(); // its entry gone, as a torn sector leaves it
    let rec = first_lba(&d.img, "Rustfw/ride.rec") as u32;
    let unread = std::vec![rec + 1..rec + 2, rec + 4..rec + 5];
    let mut d = Bad { card: d, bad: Vec::new(), unread, on: true };
    const NEXT: [u8; 12] = *b"261023110000";
    let mut r = Recorder::new();
    r.start(T0 + 1000, NEXT, false);
    let out = io_all(&mut r, &mut d, 1001);
    assert_eq!(out.last(), Some(&Outcome::Kept(NAME)), "{out:?}");
    d.on = false;
    r.second(&sample(1001), words(1));
    r.end(T0 + 1002, words(2));
    assert_eq!(io_all(&mut r, &mut d, 1003), [Outcome::Saved(NEXT)]);
    let (times, _) = saved(&d.card.img, "Rustfw/Rides/261023100000.fit").expect("named in its folder");
    assert!(times.len() >= 200 - CHECKPOINT_S as usize, "{}", times.len());
    assert_eq!(check_clean(&d.card.img, "after"), 0);
}

/// The final review: while the clean copy holds a ride behind its last save, the full copy started it all the
/// same (a full copy of a ride said "NOT RECORDING", saved and synced). It is held with the clean one now, and
/// starts once the clean one has.
#[test]
fn the_full_copy_is_held_with_the_clean_one() {
    let mut d = card(crate::fat::tests::image());
    let mut clean = Recorder::new();
    clean.pending = Some((T0, NAME, false)); // held behind a stuck save
    let mut full = Recorder::of(Kind::Full);
    full.start_after(&clean, T0, NAME, false);
    assert_eq!((full.phase(), full.pending()), (Phase::Done, true), "held");
    assert_eq!(full.io_after(&clean, &mut d, local(0)), Outcome::Nothing);
    assert_eq!(full.phase(), Phase::Done, "still held");
    clean.pending = None;
    clean.start(T0, NAME, false); // the stuck save done: the clean copy starts
    assert_eq!(clean.io(&mut d, local(1)), Outcome::Nothing);
    assert!(full.due());
    full.io_after(&clean, &mut d, local(1));
    assert_eq!((full.phase(), full.pending()), (Phase::Recording, false), "started after the clean one");
}

/// A save stuck after its step 4 (`\Activities`' directory failing to write, then `\Rustfw\Rides`' failing to
/// write, or to read if `unread`), a Start held behind it: the recorder and the card, ready for GIVE UP SAVE.
fn stuck_save(unread: bool) -> (Recorder, Bad) {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let (acts, rides) = (dir_lbas(&d.img, ACTIVITIES), std::vec![dir_lbas(&d.img, "Rustfw/Rides")]);
    let mut d = Bad { card: d, bad: std::vec![acts.clone()], unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    assert_eq!(r.io(&mut d, local(201)), Outcome::Failed(Error::Io), "step 4 done, 5 not");
    if unread {
        d.unread = rides;
    } else {
        d.bad.extend(rides);
    }
    io_all(&mut r, &mut d, 201);
    assert_eq!((r.phase(), r.stuck()), (Phase::Saving, true));
    (r, d)
}

/// After GIVE UP SAVE: the held ride starts, is recorded and saved (the card well again).
fn next_ride_records(r: &mut Recorder, d: &mut Bad, what: &str) {
    const NEXT: [u8; 12] = *b"261023110000";
    assert_eq!(r.phase(), Phase::Starting, "{what}: the held start started");
    d.on = false;
    assert!(io_all(r, d, 1001).is_empty(), "{what}");
    r.second(&sample(1001), words(1));
    r.end(T0 + 1002, words(2));
    assert_eq!(io_all(r, d, 1003), [Outcome::Saved(NEXT)], "{what}");
    assert_eq!(saved(&d.card.img, "Activities/261023110000.fit").map(|s| s.1), Ok(true), "{what}");
}

/// GIVE UP SAVE, its three outcomes. Only while stuck; the rides folder writable again: named there
/// (`Kept`); still failing to write but readable: named in `\Rustfw\Rescue`, the volume clean; unreadable: the
/// record "done" alone, the chain lost (fsck: a lost chain of the ride's clusters, which a computer's fsck may
/// turn into FILE0000.CHK; no cross-link, nothing in any folder named twice). Each time the held start starts.
#[test]
fn give_up_save_names_the_ride_where_it_can() {
    let mut idle = Recorder::new();
    assert_eq!(idle.give_up_save(&mut card(crate::fat::tests::image()), local(0)), None, "not stuck: nothing");
    let start = |r: &mut Recorder| {
        r.start(T0 + 1000, *b"261023110000", false);
        assert!(r.held());
    };

    // the rides folder well again: named there
    let (mut r, mut d) = stuck_save(false);
    start(&mut r);
    d.bad.truncate(1); // \Activities still failing
    assert_eq!(r.give_up_save(&mut d, local(300)), Some(GaveUp::Named(Outcome::Kept(NAME))));
    assert!(!r.stuck());
    kept_in_rides(&d.card.img, "rides");
    assert_eq!(check_clean(&d.card.img, "rides"), 0);
    next_ride_records(&mut r, &mut d, "rides");

    // its directory still failing to write: named in \Rustfw\Rescue, the record done first
    let (mut r, mut d) = stuck_save(false);
    start(&mut r);
    assert_eq!(r.give_up_save(&mut d, local(300)), Some(GaveUp::Rescue(NAME)));
    let (times, session) = saved(&d.card.img, "Rustfw/Rescue/261023100000.fit").unwrap();
    assert!(session && times.len() == 200, "whole: {} s", times.len());
    assert!(saved(&d.card.img, "Rustfw/Rides/261023100000.fit").is_err() && saved(&d.card.img, SAVED).is_err());
    assert_eq!(check_clean(&d.card.img, "rescue"), 0, "named once, nothing lost");
    next_ride_records(&mut r, &mut d, "rescue");
    assert_eq!(check_clean(&d.card.img, "rescue, then a ride"), 0);

    // its directory unreadable: nowhere to name it, the record "done", the chain lost
    let (mut r, mut d) = stuck_save(true);
    start(&mut r);
    assert_eq!(r.give_up_save(&mut d, local(300)), Some(GaveUp::Lost));
    let lost = check_clean(&d.card.img, "lost");
    assert!(lost >= 1, "the ride's chain lost (fsck: {lost} clusters), never cross-linked");
    let mut again = Recorder::new();
    assert_eq!(again.recover(&mut d, local(301)), Outcome::Nothing, "the record done: a boot does nothing");
    next_ride_records(&mut r, &mut d, "lost");
    assert_eq!(check_clean(&d.card.img, "lost, then a ride"), lost, "the same chain lost, no more");
}

/// A card behind (no io): room left for a plain record but not for the trainer's descriptions. The second is still
/// recorded, without the trainer's fields, instead of dropped whole (the review of 842e73f).
#[test]
fn a_short_buffer_drops_the_trainer_fields_not_the_second() {
    let mut r = Recorder::of(Kind::Full);
    r.start(T0, *b"261023100000", false);
    let plain = |t: u32| Sample { time: T0 + t, hr: Some(150), ..Default::default() };
    let mut t = 1;
    while r.room() >= RECORD_MAX + 300 + Writer::END_BYTES {
        r.second(&plain(t), [0; RIDE_WORDS]);
        t += 1;
    }
    let s = Sample { trainer: crate::fit::tests::trainer(200), ..plain(t) };
    let need = r.writer.trainer_bytes(&s);
    assert!(need > 300, "the descriptions and the device still to write: {need}");
    assert!(r.room() >= RECORD_MAX + Writer::END_BYTES && r.room() < RECORD_MAX + need + Writer::END_BYTES);
    let (dropped, len) = (r.dropped, r.writer.data_len());
    r.second(&s, [0; RIDE_WORDS]);
    assert_eq!(r.dropped, dropped, "the second is not dropped");
    assert!(r.writer.data_len() > len, "it is in the file");
    assert_eq!(r.writer.trainer_bytes(&s), need, "without the trainer's descriptions");
}

/// A computer frees the chain at `first` (both FATs, FSInfo's count kept exact) and points FSInfo's next free
/// cluster at it, so that the next file made takes its clusters: the bytes the chain held.
fn free_chain(img: &mut [u8], first: u32) -> usize {
    let u =
        |img: &[u8], at: usize, n: usize| img[at..at + n].iter().rev().fold(0usize, |v, &b| v << 8 | usize::from(b));
    let (fat0, len, fats, fsinfo) =
        (u(img, 14, 2) * SECTOR, u(img, 36, 4) * SECTOR, u(img, 16, 1), u(img, 48, 2) * SECTOR);
    let (mut c, mut n) = (first as usize, 0);
    while (2..0x0fff_fff8).contains(&c) && n < 1 << 20 {
        let next = u(img, fat0 + 4 * c, 4) & 0x0fff_ffff;
        for k in 0..fats {
            img[fat0 + k * len + 4 * c..][..4].fill(0);
        }
        (c, n) = (next, n + 1);
    }
    let free = u(img, fsinfo + 488, 4) + n;
    img[fsinfo + 488..fsinfo + 492].copy_from_slice(&(free as u32).to_le_bytes());
    img[fsinfo + 492..fsinfo + 496].copy_from_slice(&first.to_le_bytes());
    n * u(img, 13, 1) * SECTOR
}

/// The computer removes the ride's entry in `\Rustfw\Rides` (marked deleted, as `Fat::remove` does).
fn remove_ride_entry(img: &mut Vec<u8>) {
    let mut ram = crate::fat::tests::Ram(img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let dir = fat.open(&mut ram, "Rustfw/Rides").unwrap();
    let h = fat.locate(&mut ram, dir, "261023100000.fit").unwrap();
    fat.remove(&mut ram, &h).unwrap();
    *img = ram.0;
}

const OTHER: &str = "OTHER.BIN";

/// The computer then copies a file of `len` bytes (a pattern) to the root: it takes the freed clusters.
fn other_file(img: &mut Vec<u8>, first: u32, len: usize) -> Vec<u8> {
    let mut ram = crate::fat::tests::Ram(img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let root = fat.root();
    let mut h = fat.create(&mut ram, root, OTHER, (0, 0)).unwrap();
    let v: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
    fat.write_sectors(&mut ram, &mut h, 0, &v).unwrap();
    fat.set_size(&mut ram, &mut h, len as u32, (0, 0)).unwrap();
    assert_eq!(h.first, first, "the ride's clusters reused");
    *img = ram.0;
    v
}

/// The other file whole, nothing named on the ride's chain, the volume clean (no lost cluster: the chain freed or
/// the other file's).
fn let_go(img: &[u8], other: &Option<Vec<u8>>, what: &str) {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    if let Some(v) = other {
        assert_eq!(crate::fat::tests::read_all(&mut fat, &mut ram, OTHER).as_ref(), Ok(v), "{what}: the other file");
    }
    for p in ["Rustfw/Rides/261023100000.fit", SAVED, "Rustfw/Rescue/261023100000.fit"] {
        assert_eq!(fat.open(&mut ram, p).err(), Some(Error::NotFound), "{what}: {p}");
    }
    assert_eq!(check_clean(img, what), 0);
}

/// The review of 2026-10-29, S1: a ride left open (the unit off), then removed on a computer, its chain freed or
/// given to another file. The boot's recovery finds it named by its record alone: nothing written on that chain,
/// nothing linked onto it, the record done (a second boot does nothing); the power cut at each of that recovery's
/// writes, plain and torn, ends the same; the next ride is recorded and saved, the volume clean.
#[test]
fn a_ride_left_open_and_removed_by_a_computer_is_let_go() {
    for reuse in [false, true] {
        let mut d = card(crate::fat::tests::image());
        let first = open_ride(&mut d, 200).file.unwrap().first;
        let mut img = d.img;
        remove_ride_entry(&mut img);
        let len = free_chain(&mut img, first);
        let other = reuse.then(|| other_file(&mut img, first, len));
        let what = if reuse { "reused" } else { "freed" };
        let_go(&img, &other, what);
        let mut d = card(img.clone());
        assert_eq!(Recorder::new().recover(&mut d, local(300)), Outcome::Nothing, "{what}");
        let_go(&d.img, &other, what);
        assert_eq!(Recorder::new().recover(&mut d, local(301)), Outcome::Nothing, "{what}: the record done");
        let total = d.written;
        assert!(total > 0, "{what}: the record written");
        for cut in 0..total {
            for torn in [false, true] {
                let what = std::format!("{what}, cut {cut} of {total}, torn {torn}");
                let mut c = Card { budget: Some(cut), torn, ..card(img.clone()) };
                Recorder::new().recover(&mut c, local(300));
                let mut c = card(c.img);
                assert_eq!(Recorder::new().recover(&mut c, local(301)), Outcome::Nothing, "{what}");
                let_go(&c.img, &other, &what);
            }
        }
        assert!(matches!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(_))), "{what}: the next ride");
        if let Some(v) = &other {
            let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
            let mut fat = Fat::mount(&mut ram).unwrap();
            assert_eq!(crate::fat::tests::read_all(&mut fat, &mut ram, OTHER).as_ref(), Ok(v), "{what}: after");
        }
        assert_eq!(check_clean(&d.img, what), 0);
    }
}

/// The review of 2026-10-29, S2: a save stuck between its steps 4 and 5 (the ride named by its record alone),
/// then its chain freed by a computer. Neither GIVE UP SAVE (its rides folder, `\Rustfw\Rescue`) nor a boot's
/// save links an entry onto the free clusters (the next ride's `create` would take them: a cross-link); the record
/// ends done, the volume clean; the power cut at each of the boot's writes ends the same.
#[test]
fn a_stuck_save_whose_chain_a_computer_freed_is_never_linked() {
    // GIVE UP SAVE, the rides folder writable again: lost, the held start starts
    let (mut r, mut d) = stuck_save(false);
    let first = r.file.unwrap().first;
    free_chain(&mut d.card.img, first);
    r.start(T0 + 1000, *b"261023110000", false);
    d.bad.truncate(1);
    assert_eq!(r.give_up_save(&mut d, local(300)), Some(GaveUp::Lost));
    let_go(&d.card.img, &None, "give up");
    assert_eq!(Recorder::new().recover(&mut d, local(301)), Outcome::Nothing, "the record done");
    next_ride_records(&mut r, &mut d, "give up");
    assert_eq!(check_clean(&d.card.img, "give up, then a ride"), 0);

    // the rescue itself: never an entry on the freed chain
    let (mut r, mut d) = stuck_save(false);
    free_chain(&mut d.card.img, first);
    let mut fat = Fat::mount(&mut d).unwrap();
    assert_eq!(r.rescue(&mut fat, &mut d, (0, 0)), Err(Error::Corrupt));
    let_go(&d.card.img, &None, "rescue");

    // a boot: the save tried from the record, the chain not the ride's: the record done
    let (_, d) = stuck_save(false);
    let mut img = d.card.img;
    free_chain(&mut img, first);
    let mut d = card(img.clone());
    assert_eq!(Recorder::new().recover(&mut d, local(300)), Outcome::Nothing);
    let_go(&d.img, &None, "boot");
    let total = d.written;
    for cut in 0..total {
        for torn in [false, true] {
            let what = std::format!("boot, cut {cut} of {total}, torn {torn}");
            let mut c = Card { budget: Some(cut), torn, ..card(img.clone()) };
            Recorder::new().recover(&mut c, local(300));
            let mut c = card(c.img);
            assert_eq!(Recorder::new().recover(&mut c, local(301)), Outcome::Nothing, "{what}");
            let_go(&c.img, &None, &what);
        }
    }
    assert!(matches!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(_))), "the next ride");
    assert_eq!(check_clean(&d.img, "boot, then a ride"), 0);
}

/// The review of the S1 fix, finding 1: a ride left open whose entry is gone (a torn folder sector) but whose chain
/// is whole. The boot's recovery checks the chain, marks its record (`Rec::checked`), repairs sector 0 and the last
/// sector from the record's copies, and saves. Cut at every sector of that, torn, then booted again: the ride is
/// saved whole, a torn repair redone (unmarked, the torn sector 0 failed the check: the ride was let go). A ride
/// over one cluster: within one, sector 0 is compared even when marked, so a torn one lets it go (the third review).
#[test]
fn a_ride_without_its_entry_is_saved_whatever_cut_its_repair() {
    let (saved_whole, let_go) = cut_the_repair(800);
    assert!(saved_whole > 0 && let_go == 0);
}

/// The same within one cluster (200 s): a cut tearing the repair's sector 0 lets the ride go (marked, sector 0 is
/// still compared there: mod.rs, `still_ours`), named nowhere, its chain lost, the volume otherwise clean; any
/// other cut, the ride saved whole.
#[test]
fn a_one_cluster_ride_whose_repair_tears_sector_0_is_let_go() {
    let (saved_whole, let_go) = cut_the_repair(200);
    assert!(saved_whole > 0 && let_go > 0, "{saved_whole} saved, {let_go} let go");
}

/// A ride of `secs` seconds left open, its entry gone, its boot's recovery cut at every sector (plain and torn),
/// then booted again: how many cuts ended with the ride saved whole, and how many let it go (only a torn sector 0
/// within one cluster may).
fn cut_the_repair(secs: u32) -> (u32, u32) {
    let mut d = card(crate::fat::tests::image());
    let _ = open_ride(&mut d, secs);
    let sector0 = first_lba(&d.img, "Rustfw/Rides/261023100000.fit") as u32;
    let one_cluster = {
        let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        fat.open(&mut ram, "Rustfw/Rides/261023100000.fit").unwrap().size <= fat.cluster_bytes()
    };
    assert_eq!(one_cluster, secs < 600, "the ride's length: one cluster or more");
    let mut img = d.img;
    remove_ride_entry(&mut img);
    let mut whole = card(img.clone());
    assert_eq!(Recorder::new().recover(&mut whole, local(secs + 100)), Outcome::Saved(NAME));
    let (want, _) = saved(&whole.img, SAVED).unwrap();
    assert!(want.len() >= (secs - CHECKPOINT_S) as usize);
    let (total, mut torn0, mut saved_whole, mut let_go) = (whole.written, 0, 0, 0);
    for cut in 0..total {
        for torn in [false, true] {
            let what = std::format!("cut {cut} of {total}, torn {torn}");
            let mut c = Card { budget: Some(cut), torn, ..card(img.clone()) };
            Recorder::new().recover(&mut c, local(300));
            let tore0 = torn && c.cut_at == sector0;
            torn0 += usize::from(tore0);
            let mut c = card(c.img);
            let o = Recorder::new().recover(&mut c, local(301));
            if one_cluster && tore0 {
                assert_eq!(o, Outcome::Nothing, "{what}");
                let mut ram = crate::fat::tests::Ram(c.img.clone(), 0);
                let mut fat = Fat::mount(&mut ram).unwrap();
                for p in ["Rustfw/Rides/261023100000.fit", SAVED, "Rustfw/Rescue/261023100000.fit"] {
                    assert_eq!(fat.open(&mut ram, p).err(), Some(Error::NotFound), "{what}: {p}");
                }
                assert_eq!(best_rec(&c.img).phase, Phase::Done, "{what}");
                let_go += 1;
            } else {
                assert!(matches!(o, Outcome::Saved(NAME) | Outcome::Nothing), "{what}: {o:?}");
                assert_eq!(saved(&c.img, SAVED).map(|s| s.0), Ok(want.clone()), "{what}: saved whole");
                saved_whole += 1;
            }
            fsck_torn(&c.img).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(free_counts(&c.img).0, free_counts(&c.img).1, "{what}: FSInfo's free count");
        }
    }
    assert!(torn0 > 0, "a cut tore the repair's sector 0");
    (saved_whole, let_go)
}

/// The review of the S1 fix, finding 4: a read error inside the chain's check (the ride's sector 0 unreadable) is
/// a card error, not a verdict: `Failed`, the record not done, and the ride saved once the card reads again.
#[test]
fn a_read_error_while_checking_the_chain_keeps_the_record() {
    let mut d = card(crate::fat::tests::image());
    let _ = open_ride(&mut d, 200);
    let sector0 = first_lba(&d.img, "Rustfw/Rides/261023100000.fit") as u32;
    let mut img = d.img;
    remove_ride_entry(&mut img);
    let mut d = Bad { card: card(img), bad: Vec::new(), unread: std::vec![sector0..sector0 + 1], on: true };
    assert_eq!(Recorder::new().recover(&mut d, local(300)), Outcome::Failed(Error::Io));
    d.on = false;
    assert_eq!(Recorder::new().recover(&mut d, local(301)), Outcome::Saved(NAME), "the record not done");
    assert!(saved(&d.card.img, SAVED).is_ok_and(|s| s.0.len() >= 200 - CHECKPOINT_S as usize));
    assert_eq!(check_clean(&d.card.img, "after"), 0);
}

/// The newer valid record in `ride.rec` on `img`.
fn best_rec(img: &[u8]) -> Rec {
    newest_rec(img).unwrap()
}

/// The newer valid record of the two copies in `ride.rec` on `img`, as `Recorder::recover` reads them; None
/// before the first (no `ride.rec`, or neither copy whole).
fn newest_rec(img: &[u8]) -> Option<Rec> {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).ok()?;
    let v = crate::fat::tests::read_all(&mut fat, &mut ram, "Rustfw/ride.rec").ok()?;
    let rec = |k: usize| Rec::from_sector(v.get((3 * k + 2) * SECTOR..(3 * k + 3) * SECTOR)?.try_into().ok()?);
    [rec(0), rec(1)].into_iter().flatten().max_by_key(|r| r.seq)
}

/// The second review of the S1 fix, finding 1: a record marked checked (the boot cut right after the mark, before
/// its repair), then the card in other hands (a computer, stock): the ride's chain freed, or given to another file.
/// The mark does not make the chain the ride's: sector 1's CRC, which the repair never writes, no longer matches,
/// so nothing is written on the chain or linked onto it, the other file whole, the ride let go, the volume clean.
#[test]
fn a_marked_record_whose_chain_a_computer_freed_or_reused_is_let_go() {
    for reuse in [false, true] {
        let (mut img, first) = marked_and_cut(200);
        let len = free_chain(&mut img, first);
        let other = reuse.then(|| other_file(&mut img, first, len));
        let what = if reuse { "reused" } else { "freed" };
        let mut d = card(img);
        assert_eq!(Recorder::new().recover(&mut d, local(301)), Outcome::Nothing, "{what}");
        let_go(&d.img, &other, what);
        assert_eq!(best_rec(&d.img).phase, Phase::Done, "{what}");
    }
}

/// A ride of `secs` seconds left open, its entry removed, the boot cut at its first write after the mark (copies
/// and record, one write): the image, and the ride's first cluster.
fn marked_and_cut(secs: u32) -> (Vec<u8>, u32) {
    let mut d = card(crate::fat::tests::image());
    let first = open_ride(&mut d, secs).file.unwrap().first;
    let mut img = d.img;
    remove_ride_entry(&mut img);
    let img = (0..20)
        .map(|b| {
            let mut c = Card { budget: Some(b), ..card(img.clone()) };
            Recorder::new().recover(&mut c, local(300));
            c.img
        })
        .find(|i| best_rec(i).checked.is_some())
        .expect("a boot cut after the mark");
    assert_eq!(best_rec(&img).phase, Phase::Recording);
    (img, first)
}

/// The third review, finding 1: a ride within one cluster, marked, the boot cut before its repair; its cluster
/// then given to a file of one sector (an .ini, a .sum), which changes sector 0 alone: sector 1 still holds the
/// ride's stale bytes and passes the mark's CRC. Sector 0 is compared too for a one-cluster chain: the ride let go,
/// the small file whole, nothing linked onto it.
#[test]
fn a_marked_one_cluster_ride_whose_cluster_a_small_file_took_is_let_go() {
    let (mut img, first) = marked_and_cut(70);
    let cluster = {
        let mut ram = crate::fat::tests::Ram(img.clone(), 0);
        Fat::mount(&mut ram).unwrap().cluster_bytes() as usize
    };
    assert_eq!(free_chain(&mut img, first), cluster, "the ride within one cluster");
    let other = Some(other_file(&mut img, first, SECTOR));
    let mut d = card(img);
    assert_eq!(Recorder::new().recover(&mut d, local(301)), Outcome::Nothing);
    let_go(&d.img, &other, "a small file");
    assert_eq!(best_rec(&d.img).phase, Phase::Done);
}

/// The test card (`fat::tests::image`) as a ride's start leaves it: `\Rustfw` and the app's files ([`KEPT`], the
/// folders), zeroed.
#[cfg(test)] // the module is (mod.rs); said here for tools/lint.py, which reads one file at a time
pub(crate) fn kept_card() -> Vec<u8> {
    let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    fat.ensure_dir(&mut d, fat.root(), DIR, (0, 0)).unwrap();
    make_kept(&mut fat, &mut d, (0, 0)).unwrap();
    kept_made(&d.0, "the kept card");
    d.0
}

/// Each of [`KEPT`] in `\Rustfw` at its size, `\Rustfw\Deleted\Full` and `\Rustfw\Log`.
fn kept_made(img: &[u8], what: &str) {
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    for (name, size) in KEPT {
        let v = crate::fat::tests::read_all(&mut fat, &mut ram, &std::format!("Rustfw/{name}"));
        assert_eq!(v.map(|v| v.len()), Ok(size as usize), "{what}: {name}");
    }
    assert!(fat.open(&mut ram, "Rustfw/Deleted/Full").is_ok_and(|f| f.dir), "{what}: Deleted/Full");
    assert!(fat.open(&mut ram, "Rustfw/Log").is_ok_and(|f| f.dir), "{what}: Log");
}

/// A card from before [`KEPT`] was made at a ride's start (the owner's): those files, `\Rustfw\Deleted` and
/// `\Rustfw\Log` gone, their entries marked deleted and their chains freed.
fn unmake_kept(img: &mut Vec<u8>) {
    let mut ram = crate::fat::tests::Ram(img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let dir = fat.open(&mut ram, DIR).unwrap();
    let mut chains = Vec::new();
    for (name, _) in KEPT {
        let h = fat.locate(&mut ram, dir, name).unwrap();
        fat.remove(&mut ram, &h).unwrap();
        chains.push(h.first);
    }
    chains.push(fat.open(&mut ram, "Rustfw/Deleted/Full").unwrap().cluster);
    for folder in ["Rustfw/Deleted", "Rustfw/Log"] {
        // the folder's 8.3 entry found by its cluster (`locate` opens files only)
        let f = fat.open(&mut ram, folder).unwrap();
        chains.push(f.cluster);
        let lba = fat.lba(dir.cluster);
        let (entry_lba, slot) = (lba..lba + fat.cluster_bytes() / SECTOR_U32)
            .flat_map(|lba| (0..16u8).map(move |k| (lba, k)))
            .find(|&(lba, k)| {
                let e = &ram.0[lba as usize * SECTOR + 32 * usize::from(k)..][..32];
                e[11] == 0x10 && [e[26], e[27], e[20], e[21]] == f.cluster.to_le_bytes()
            })
            .unwrap();
        fat.remove(&mut ram, &Handle { entry_lba, slot, dir: dir.cluster, ..Handle::default() }).unwrap();
    }
    *img = ram.0;
    for c in chains {
        free_chain(img, c);
    }
}

/// The review of 2026-10-29, S7: the app's files in `\Rustfw` are made zeroed at the first ride's start, each
/// reader takes them as "none", and on a card that has them neither the set-up nor `make_kept` writes anything.
#[test]
fn the_apps_files_are_made_at_a_rides_start_and_once() {
    let mut d = card(crate::fat::tests::image());
    assert_eq!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(NAME)));
    kept_made(&d.img, "after a ride");
    assert_eq!(check_clean(&d.img, "after a ride"), 0);
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    for (name, _) in KEPT {
        let v = crate::fat::tests::read_all(&mut fat, &mut ram, &std::format!("Rustfw/{name}")).unwrap();
        assert!(v.iter().all(|&b| b == 0), "{name}: zeros");
    }
    let (two, one) = ([0u8; 1024], [0u8; SECTOR]);
    assert!(crate::page::Saved::newest(&two).is_none() && crate::SettingsCopy::newest(&two).is_none());
    assert!(crate::phone::Stored::from_sector(&one).is_none() && crate::altitude::Cal::from_sector(&one).is_none());
    assert!(crate::sensors::from_sector(&one).is_none());
    assert!(crate::bikes::Bikes::newest(&two).is_none() && crate::places::Places::newest(&two).is_none());
    d.written = 0;
    assert_eq!(crate::trash::finish(&mut d, (0, 0)), Ok(false), "no move pending");
    // the owner's card once it has them: nothing written
    let mut r = Recorder::new();
    let mut fat = Fat::mount(&mut d).unwrap();
    r.setup(&mut fat, &mut d, (0, 0)).unwrap();
    make_kept(&mut fat, &mut d, (0, 0)).unwrap();
    assert_eq!(d.written, 0, "nothing written");
}

/// The review of 2026-10-31 (storage, 8): a file of [`KEPT`] longer than listed (a newer build's, an older build
/// flashed after it) is left as it is: not cut back, nothing written, no FAT sync at each ride's start.
#[test]
fn a_kept_file_longer_than_listed_is_left_alone() {
    let mut d = card(kept_card());
    let mut fat = Fat::mount(&mut d).unwrap();
    let dir = fat.open(&mut d, DIR).unwrap();
    let (name, size) = KEPT[0];
    let mut h = fat.locate(&mut d, dir, name).unwrap();
    let longer = size + SECTOR_U32;
    fat.write_sectors(&mut d, &mut h, size / SECTOR_U32, &[7; SECTOR]).unwrap();
    fat.set_size(&mut d, &mut h, longer, (0, 0)).unwrap();
    d.written = 0;
    make_kept(&mut fat, &mut d, (0, 0)).unwrap();
    assert_eq!(d.written, 0, "nothing written");
    assert_eq!(fat.locate(&mut d, dir, name).map(|h| h.size), Ok(longer), "{name} kept whole");
}

/// The review of 2026-10-29, S7: on a card from before (a saved ride, `\Rustfw` and its records, none of the app's
/// files), the next ride's start makes them; a power cut at every sector of that start (plain and torn) leaves the
/// volume clean, the saved ride whole, and the ride after it finishes the job and is saved. Entries are lost only
/// by a torn sector of `\Rustfw`'s directory: the one write there, at a ride's start with nothing open.
#[test]
fn making_the_apps_files_survives_a_cut_at_every_sector() {
    let mut d = card(crate::fat::tests::image());
    assert_eq!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(NAME)));
    let mut img = d.img;
    unmake_kept(&mut img);
    assert_eq!(check_clean(&img, "a card from before"), 0);
    let mine = {
        let mut ram = crate::fat::tests::Ram(img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let c = fat.open(&mut ram, DIR).unwrap().cluster;
        let lba = fat.lba(c);
        lba..lba + fat.cluster_bytes() / SECTOR_U32
    };
    let start = |c: &mut Card| {
        let mut r = Recorder::new();
        r.start(T0 + 5000, *b"261023112000", false);
        assert!(r.due());
        r.io(c, local(5000))
    };
    let mut whole = card(img.clone());
    assert_eq!(start(&mut whole), Outcome::Nothing);
    kept_made(&whole.img, "whole");
    let (total, mut torn_mine) = (whole.written, 0);
    for cut in 0..total {
        for torn in [false, true] {
            let mut c = Card { budget: Some(cut), torn, ..card(img.clone()) };
            start(&mut c);
            let what = std::format!("cut {cut} of {total}, torn {torn}, sector {}", c.cut_at);
            let tore_mine = torn && mine.contains(&c.cut_at);
            torn_mine += usize::from(tore_mine);
            let mut c = card(c.img);
            assert!(matches!(ride(&mut Recorder::new(), &mut c, 0), Some(Outcome::Saved(_))), "{what}");
            kept_made(&c.img, &what);
            assert!(saved(&c.img, SAVED).is_ok(), "{what}: the saved ride");
            let lost = fsck_torn(&c.img).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(free_counts(&c.img).0, free_counts(&c.img).1, "{what}: FSInfo's free count");
            assert!(lost <= 2 || tore_mine, "{what}: {lost} clusters lost");
        }
    }
    std::println!("{total} writes cut, {torn_mine} tearing \\Rustfw's directory");
    assert_eq!((total, torn_mine), (174, 20), "a write added or dropped");
}

/// The review of S7: a `make_kept` that fails (a name in [`KEPT`] taken by a folder) is tried once a boot, never
/// again at each start: the error counted once, the rides recorded and saved all the same.
#[test]
fn a_failing_make_kept_is_tried_once_a_boot() {
    let mut ram = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let dir = fat.ensure_dir(&mut ram, fat.root(), DIR, (0, 0)).unwrap();
    fat.ensure_dir(&mut ram, dir, KEPT[0].0, (0, 0)).unwrap();
    let mut d = card(ram.0);
    let mut r = Recorder::new();
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(NAME)));
    assert_eq!(r.errors, 1, "the first start");
    assert_eq!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(*b"261023100001")));
    assert_eq!(r.errors, 1, "not tried again");
    let mut r = Recorder::new();
    assert!(matches!(ride(&mut r, &mut d, 0), Some(Outcome::Saved(_))));
    assert_eq!(r.errors, 1, "tried again after a boot");
    assert_eq!(check_clean(&d.img, "after"), 0);
}

/// The review of 2026-10-29, S5: a checkpoint that grows the ride's chain into a new cluster, then fails (its new
/// cluster's data, sector 0, the size or the record), used to forget the grown chain: the retry took another
/// cluster and relinked the last one to it, the first one left taken, named by nothing (16 KB per failure). The
/// retry goes on from the grown chain: the ride recorded on and saved whole, no cluster lost, FSInfo exact. A
/// failure inside the grow's own FAT writes (either copy, twice in a row too) may leave the cluster it was taking
/// lost (never taken up: `Fat::grow`): at most one lost cluster per failed grow, never a cross-link, FSInfo's count
/// as many high until the next boot's FAT sync makes it exact.
#[test]
fn a_checkpoint_failing_after_growing_loses_at_most_one_cluster() {
    const SECS: u32 = 900;
    // the second whose checkpoint takes the second cluster, and that cluster, on a card that never fails
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    let mut grows = None;
    for t in 0..SECS {
        r.second(&sample(t), words(t));
        if r.due() {
            assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
            if grows.is_none() && r.file.is_some_and(|h| h.clusters == 2) {
                grows = r.file.map(|h| (t, h.last));
            }
        }
    }
    let (at, new) = grows.expect("a checkpoint grows the chain");
    let img = d.img;
    let lba = |img: &[u8], path: &str| first_lba(img, path) as u32;
    let new_lba = {
        let mut ram = crate::fat::tests::Ram(img.clone(), 0);
        Fat::mount(&mut ram).unwrap().lba(new)
    };
    let fresh = crate::fat::tests::image();
    // the ride's folders and record are made at the first checkpoint: their places taken from the ride done
    let rides = dir_lbas(&img, "Rustfw/Rides");
    let rec = lba(&img, "Rustfw/ride.rec");
    let sector0 = lba(&img, "Rustfw/Rides/261023100000.fit");
    // the FAT sector holding the new cluster's entry, in each copy (the boot sector's fields: fatgen103)
    let u = |at: usize, n: usize| img[at..at + n].iter().rev().fold(0u32, |v, &b| v << 8 | u32::from(b));
    let fat_sector = |k: u32| u(14, 2) + k * u(36, 4) + new / (SECTOR as u32 / 4);
    let (fat1, fat2) = (fat_sector(0), fat_sector(1));
    for (what, bad, fails) in [
        ("the first FAT", fat1..fat1 + 1, 1),
        ("the second FAT (the first one written)", fat2..fat2 + 1, 1),
        ("the second FAT, twice in a row", fat2..fat2 + 1, 2),
        ("the new cluster's data", new_lba..new_lba + 1, 1),
        ("sector 0", sector0..sector0 + 1, 1),
        ("the size", rides, 1),
        ("the record", rec..rec + 6, 1),
    ] {
        let mut d = Bad { card: card(fresh.clone()), bad: std::vec![bad], unread: Vec::new(), on: false };
        let mut r = Recorder::new();
        r.start(T0 - 1, NAME, false);
        let mut failed = 0;
        for t in 0..SECS {
            r.second(&sample(t), words(t));
            if r.due() {
                d.on = t == at; // the growing checkpoint fails (`fails` times in a row, its retries)
                for _ in 0..10 * RETRY {
                    match r.io(&mut d, local(t)) {
                        Outcome::Nothing => {}
                        Outcome::Failed(Error::Io) if t == at => failed += 1,
                        o => panic!("{what}: {o:?} at {t}"),
                    }
                    if !r.waiting() && (t != at || failed == fails) {
                        break;
                    }
                }
                d.on = false;
                // its retry, after the wait, within the second (the main loop's passes): nothing dropped
                assert!(io_all(&mut r, &mut d, t).is_empty(), "{what}");
            }
        }
        assert_eq!((failed, r.dropped), (fails, 0), "{what}: the checkpoint at {at} failed");
        r.end(T0 + SECS, words(SECS));
        assert_eq!(io_all(&mut r, &mut d, SECS + 1), [Outcome::Saved(NAME)], "{what}");
        assert_eq!(saved(&d.card.img, SAVED).map(|s| s.0.len()), Ok(SECS as usize), "{what}: whole");
        let lost = fsck(&d.card.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        let (free, info) = free_counts(&d.card.img);
        let in_fat = if what.contains("FAT") { fails } else { 0 }; // one cluster per failed grow at most
        assert!(lost <= in_fat, "{what}: {lost} clusters lost");
        assert!(free <= info && info - free <= in_fat, "{what}: FSInfo {info}, {free} free");
        assert_eq!(Recorder::new().recover(&mut d.card, local(SECS + 5)), Outcome::Nothing, "{what}");
        assert_eq!(check_clean(&d.card.img, what), lost, "{what}: after the boot's sync");
    }
}

/// The ride whole (`want`, its record times) in exactly one of the places it may be named: saved, kept where it was
/// recorded, or rescued.
fn whole_once(img: &[u8], want: &[u32], what: &str) {
    let at: Vec<&str> = [SAVED, "Rustfw/Rides/261023100000.fit", "Rustfw/Rescue/261023100000.fit"]
        .into_iter()
        .filter(|p| saved(img, p).is_ok_and(|s| s.0 == want))
        .collect();
    assert_eq!(at.len(), 1, "{what}: the ride whole in {at:?}");
}

/// The boot after a cut, the card well again: a ride it reopens is ended at its last record and saved, as the
/// owner would; a rescue's record left "saving" is done (`Kept`).
fn boot_and_settle(d: &mut Card, what: &str) {
    let mut r = Recorder::new();
    match r.recover(d, local(SECS + 5)) {
        Outcome::Resumed(w, name) => {
            r.end(r.writer.last_time(), w);
            assert_eq!(io_all(&mut r, d, SECS + 6), [Outcome::Saved(name)], "{what}");
        }
        Outcome::Saved(_) | Outcome::Nothing | Outcome::Kept(_) => {}
        o => panic!("{what}: the boot: {o:?}"),
    }
}

/// The review of 2026-10-29, S6: the writes of a path cut at every sector, plain, torn as garbage and torn with a prefix new (finding 3 of the review of 2026-11-01). `setup` brings a
/// recorder and its card (`inner`) to the path, `act` runs it. After the cut, the boot ([`boot_and_settle`]): the
/// ride whole in one place, as the path without a cut leaves it at `want_at`; the volume clean (no cross-link, no
/// chain into a free cluster, FSInfo exact). A torn sector of `\Rustfw`'s directory (a folder made there: the
/// rescue's) is left out, its loss known (`ride.rec`'s entry with it: the review of 2026-10-29, S7). The cut points,
/// the torn ones left out, and the most clusters a cut left lost.
fn cut_every_sector<D: DiskWrite>(
    setup: impl Fn() -> (Recorder, D),
    inner: fn(&mut D) -> &mut Card,
    act: impl Fn(&mut Recorder, &mut D),
    want_at: &str,
) -> (u64, u32, u32) {
    let (mut r, mut d) = setup();
    let st = stock(&inner(&mut d).img); // stock's files as the path finds them
    let mine = {
        let mut ram = crate::fat::tests::Ram(inner(&mut d).img.clone(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let c = fat.open(&mut ram, DIR).unwrap().cluster;
        let lba = fat.lba(c);
        lba..lba + fat.cluster_bytes() / SECTOR as u32
    };
    let from = inner(&mut d).written;
    act(&mut r, &mut d);
    let total = inner(&mut d).written - from;
    let mut b = card(inner(&mut d).img.clone());
    boot_and_settle(&mut b, "no cut");
    let want = saved(&b.img, want_at).unwrap_or_else(|e| panic!("no cut: {e}")).0;
    let (mut lost, mut s7) = (0, 0);
    for cut in 0..total {
        // plain, torn as garbage, torn with a prefix new (`Card::prefix`, its k moving with the cut)
        for (torn, prefix) in [(false, None), (true, None), (true, Some(4 * (1 + cut as usize % 127)))] {
            let what = std::format!("cut {cut} of {total}, torn {torn}, prefix {prefix:?}");
            let (mut r, mut d) = setup();
            let c = inner(&mut d);
            (c.budget, c.torn, c.prefix) = (Some(c.written + cut), torn, prefix);
            act(&mut r, &mut d);
            let at = inner(&mut d).cut_at;
            let torn_at: &[u32] = if torn { &[at] } else { &[] };
            stock_kept(&st, &inner(&mut d).img, torn_at, &[]).unwrap_or_else(|e| panic!("{what}: {e}"));
            if torn && prefix.is_none() && mine.contains(&at) {
                s7 += 1;
                continue;
            }
            let mut b = card(inner(&mut d).img.clone());
            boot_and_settle(&mut b, &what);
            stock_kept(&st, &b.img, torn_at, &[]).unwrap_or_else(|e| panic!("{what}, booted: {e}"));
            whole_once(&b.img, &want, &what);
            let l = if torn { fsck_torn(&b.img) } else { fsck(&b.img) }.unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(free_counts(&b.img).0, free_counts(&b.img).1, "{what}: FSInfo's free count");
            lost = lost.max(l);
        }
    }
    std::println!(
        "{total} cut points, plain, torn and prefix torn: lost clusters at most {lost}; {s7} torn in \\Rustfw left out"
    );
    (total, s7, lost)
}

/// S6 (a): the save of a ride the card stopped ([`Recorder::save_as_is`], [`trailer_end`]: its last sector and
/// sector 0 from the record, then the save) cut at every sector: saved whole, or resumed from its record at the
/// boot and saved.
#[test]
fn a_stopped_rides_save_cut_at_every_sector() {
    let setup = || {
        let (r, d, _) = stopped_on_two_sectors();
        (r, d)
    };
    let act = |r: &mut Recorder, d: &mut Refuse| {
        io_all(r, d, 3000);
    };
    let (total, s7, lost) = cut_every_sector(setup, |d| &mut d.0, act, SAVED);
    assert_eq!((total, s7), (7, 0), "a write added or dropped");
    assert!(lost <= 1, "{lost} lost");
}

/// S6 (b): GIVE UP SAVE's rescue (`\Rustfw\Rescue`: the link, then the record "done") cut at every sector: the
/// ride rescued (its record done by the boot when the cut fell between), or saved by the boot from its record
/// "saving".
#[test]
fn give_up_saves_rescue_cut_at_every_sector() {
    let setup = || {
        let (mut r, d) = stuck_save(false);
        r.start(T0 + 1000, *b"261023110000", false);
        (r, d)
    };
    let act = |r: &mut Recorder, d: &mut Bad| {
        r.give_up_save(d, local(300));
    };
    let (total, s7, lost) = cut_every_sector(setup, |d| &mut d.card, act, "Rustfw/Rescue/261023100000.fit");
    // the rescue folder's entry in \Rustfw: one sector written twice (deleted, then made live), its tears S7's
    assert_eq!((total, s7), (41, 2), "a write added or dropped");
    assert!(lost <= 1, "{lost} lost");
}

/// S6 (c): a Start finding the last ride unfinished (left open two days ago: ended and saved; or its save cut:
/// finished), then its own Starting record, its file and first checkpoint, cut at every sector: the old ride saved
/// whole, by the Start or by the boot after the cut.
#[test]
fn a_start_after_an_unfinished_ride_cut_at_every_sector() {
    let img = crate::fat::tests::image();
    let mut open = card(img.clone());
    let _ = open_ride(&mut open, 200);
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let save_cut = (0..full.written)
        .rev()
        .map(|cut| {
            let mut d = Card { budget: Some(cut), ..card(img.clone()) };
            let _ = ride(&mut Recorder::new(), &mut d, 0);
            d.img
        })
        .find(|i| best_rec(i).phase == Phase::Saving)
        .unwrap();
    let cases = [("left open", open.img, 2 * 86_400, false, 28), ("save cut", save_cut, 5000, true, 12)];
    for (what, old, later, marker, cuts) in cases {
        let setup = || {
            let mut r = Recorder::new();
            r.start(T0 + 200 + later, *b"261025100000", marker);
            r.second(&sample(200 + later), words(0));
            (r, card(old.clone()))
        };
        let act = |r: &mut Recorder, d: &mut Card| {
            io_all(r, d, 200 + later);
        };
        let (total, s7, lost) = cut_every_sector(setup, |d| d, act, SAVED);
        assert_eq!((total, s7), (cuts, 0), "{what}: a write added or dropped");
        assert!(lost <= 1, "{what}: {lost} lost");
    }
}

/// The review of S5 and S6, finding 1: a rescue folder that cannot be read (a failing card's, the one GIVE UP SAVE
/// was for) never stops a later ride's save: only a chain named nowhere else is looked for there.
#[test]
fn a_rescue_folder_that_cannot_be_read_never_stops_a_save() {
    let mut ram = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let mut dir = fat.root();
    for name in RESCUE_PATHS[0].split('/') {
        dir = fat.ensure_dir(&mut ram, dir, name, (0, 0)).unwrap();
    }
    let unread = std::vec![dir_lbas(&ram.0, RESCUE_PATHS[0])];
    let mut d = Bad { card: card(ram.0), bad: Vec::new(), unread, on: true };
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in 0..200 {
        r.second(&sample(t), words(t));
        if r.due() {
            assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
        }
    }
    r.end(T0 + 200, words(200));
    assert_eq!(io_all(&mut r, &mut d, 201), [Outcome::Saved(NAME)]);
    assert_eq!(check_clean(&d.card.img, "saved"), 0);
}

/// The same review, finding 4: a rescue under a second later (its name taken in `\Rustfw\Rescue`), its record
/// "done" failing: the boot finds it there and says, and records, the name it has, not the ride's first one.
#[test]
fn a_rescue_left_saving_is_done_under_its_own_name() {
    let (mut r, mut d) = stuck_save(false);
    let mut ram = crate::fat::tests::Ram(d.card.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let mut dir = fat.root();
    for name in RESCUE_PATHS[0].split('/') {
        dir = fat.ensure_dir(&mut ram, dir, name, (0, 0)).unwrap();
    }
    fat.create(&mut ram, dir, "261023100000.fit", (0, 0)).unwrap(); // an empty file: no cluster
    d.card.img = ram.0;
    let rec = first_lba(&d.card.img, "Rustfw/ride.rec") as u32;
    d.bad.push(rec..rec + 6); // the record "done" fails
    const LATER: [u8; 12] = *b"261023100001";
    assert_eq!(r.give_up_save(&mut d, local(300)), Some(GaveUp::Rescue(LATER)));
    assert_eq!(best_rec(&d.card.img).phase, Phase::Saving, "the record not done");
    let mut b = card(d.card.img);
    assert_eq!(Recorder::new().recover(&mut b, local(301)), Outcome::Kept(LATER));
    assert_eq!((best_rec(&b.img).phase, best_rec(&b.img).name), (Phase::Done, LATER));
    let (times, _) = saved(&b.img, "Rustfw/Rescue/261023100001.fit").unwrap();
    assert_eq!(times.len(), 200);
    assert!(saved(&b.img, SAVED).is_err(), "never a second entry");
    assert_eq!(check_clean(&b.img, "after"), 0);
}

// The review of 2026-10-31, finding 3: the recorder's rules that no test guarded (cargo-mutants).

/// A clean and a full copy of a ride on `d`, their files made at second 0 (both recording, nothing due).
fn recording_pair(d: &mut Card) -> [Recorder; 2] {
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, d, 0, 1, &mut last);
    assert!(rs.iter().all(|r| r.phase() == Phase::Recording && !r.due()), "both recording");
    rs
}

/// `keep_up`: the full copy wanting a checkpoint (its buffer low before the minute, a minute of beats, switching off)
/// makes the clean one checkpoint in the same pass; not when the full copy wants none, nor while it waits after a
/// failure, nor for a minute of beats alone while paused (the clean copy has nothing new then), unless the full
/// copy's buffer is low too.
#[test]
fn the_full_copys_checkpoint_takes_the_clean_one_along() {
    let fresh = || card(crate::fat::tests::image());
    let [mut clean, full] = recording_pair(&mut fresh());
    clean.keep_up(&full);
    assert!(!clean.due(), "the full copy wants nothing");

    // the full copy's buffer low (its records are bigger) before the clean one's minute
    let [mut clean, mut full] = recording_pair(&mut fresh());
    let mut t = 100;
    while !full.due() {
        full.second(&sample(t), words(t));
        clean.second(&sample(t), words(t));
        t += 1;
    }
    assert!(t < 100 + CHECKPOINT_S && !clean.due(), "second {t}: the full copy's buffer first");
    clean.keep_up(&full);
    assert!(clean.due(), "together: the full copy's buffer low");

    // a minute of beats while riding
    let [mut clean, mut full] = recording_pair(&mut fresh());
    for _ in 0..40 {
        full.beat(1500);
    }
    assert!(full.due() && !clean.due());
    clean.keep_up(&full);
    assert!(clean.due(), "together: a minute of beats while riding");

    // paused: switching off with some beats, a minute of beats alone, a minute of beats with the buffer low
    let paused = || {
        let mut d = fresh();
        let mut rs = recording_pair(&mut d);
        for r in rs.iter_mut() {
            r.pause(T0 + 1, words(1));
        }
        pass(&mut rs, &mut d, 1, &mut [Outcome::Nothing; 2]);
        assert!(!rs[0].due() && !rs[1].due(), "the pause checkpointed");
        rs
    };
    let [mut clean, mut full] = paused();
    for _ in 0..10 {
        full.beat(1000);
    }
    full.checkpoint_now();
    clean.keep_up(&full);
    assert!(clean.due(), "together: switching off, ten seconds of beats");
    let [mut clean, mut full] = paused();
    for _ in 0..40 {
        full.beat(1500);
    }
    assert!(full.due());
    clean.keep_up(&full);
    assert!(!clean.due(), "a minute of beats alone while paused: the clean copy has nothing new");
    let [mut clean, mut full] = paused();
    while full.room() >= LOW + Writer::END_BYTES {
        full.beat(3000);
    }
    clean.keep_up(&full);
    assert!(clean.due(), "together: paused, but the full copy's buffer low");

    // the full copy waiting after a failure (its card dead): the clean one goes on alone
    let (mut d, mut dead) = (fresh(), Card { dead: true, ..fresh() });
    let [mut clean, mut full] = pair();
    clean.start(T0 - 1, NAME, false);
    full.start(T0 - 1, NAME, false);
    assert_eq!(clean.io(&mut d, local(0)), Outcome::Nothing);
    assert_eq!(full.io(&mut dead, local(0)), Outcome::Failed(Error::Io));
    assert!(full.failing() && !clean.due());
    clean.keep_up(&full);
    assert!(!clean.due(), "never held to a failing full copy");
}

/// GIVE UP SAVE while stuck on a record the boot could not read (no save known to be under way): nothing written,
/// the record left as it was, so the next boot still finishes the save.
#[test]
fn give_up_save_outside_a_save_writes_nothing() {
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 200);
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
    r.end(T0 + 200, words(200));
    r.io(&mut d, local(201)); // steps 1 to 4 done, 5 failed: the unit off now
    let mut d = Bad { card: card(d.card.img), bad: Vec::new(), unread: std::vec![0..1], on: true };
    let mut r = Recorder::new();
    for _ in 0..100 * RETRY {
        r.recover(&mut d, local(300));
    }
    assert!(r.stuck() && r.phase() != Phase::Saving, "stuck, the record unread");
    d.on = false; // the card mounts
    let (img, written) = (d.card.img.clone(), d.card.written);
    assert_eq!(r.give_up_save(&mut d, local(400)), Some(GaveUp::Lost));
    assert!(d.card.written == written && d.card.img == img, "nothing written");
    assert!(!r.stuck());
    assert_eq!(Recorder::new().recover(&mut d, local(401)), Outcome::Saved(NAME), "the record as it was");
    assert_eq!(saved(&d.card.img, SAVED).map(|s| s.1), Ok(true));
}

/// The buffer as the FIT writer's sink (`Ram`): a write within the header, or within the buffer up to its last
/// byte, lands where it says; one past the buffer's end, or before the buffer but past the header, is refused and
/// changes nothing.
#[test]
fn the_ram_sink_refuses_what_it_cannot_hold() {
    let (mut tail, mut len, mut head) = ([0u8; TAIL], 0u32, [0u8; 14]);
    let at = 1000u32;
    let last = at + TAIL as u32;
    let mut ram = Ram { tail: &mut tail, tail_off: at, len: &mut len, head: &mut head };
    assert_eq!(ram.write_at(0, &[1; 14]), Ok(()), "the whole header");
    assert_eq!(ram.write_at(2, &[2; 4]), Ok(()), "within the header");
    assert_eq!(ram.write_at(10, &[3; 8]), Err(fit::Error), "past the header, before the buffer");
    assert_eq!(ram.write_at(at - 1, &[4; 2]), Err(fit::Error), "across the buffer's start");
    assert_eq!(ram.write_at(at, &[5; 3]), Ok(()), "the buffer's start");
    assert_eq!(ram.write_at(last - 4, &[6, 7, 8, 9]), Ok(()), "up to the buffer's last byte");
    assert_eq!(ram.write_at(last - 3, &[1; 4]), Err(fit::Error), "one byte past the buffer");
    assert_eq!(ram.write_at(last, &[1]), Err(fit::Error), "after the buffer");
    assert_eq!(head, [1, 1, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1]);
    assert_eq!(tail[..4], [5, 5, 5, 0]);
    assert_eq!(tail[TAIL - 5..], [0, 6, 7, 8, 9]);
    assert!(tail[4..TAIL - 5].iter().all(|&b| b == 0), "nothing else written");
    // the buffer at the file's start: the header in both
    let (mut tail, mut head) = ([0u8; TAIL], [0u8; 14]);
    let mut ram = Ram { tail: &mut tail, tail_off: 0, len: &mut len, head: &mut head };
    assert_eq!(ram.write_at(12, &[7; 4]), Ok(()));
    assert_eq!(head[12..], [7, 7]);
    assert_eq!(tail[11..17], [0, 7, 7, 7, 7, 0]);
}

/// `set_profile`: a change while the ride is recorded is the file's (its end says it, no second needed after it);
/// once the ride is ended, or with no ride, a change does nothing.
#[test]
fn the_profile_is_set_only_while_recording() {
    let mut idle = Recorder::new();
    idle.set_profile(Profile::Mtb, true);
    assert_eq!((idle.profile(), idle.by_hand()), (Profile::Road, false), "no ride: nothing");
    let mut d = card(crate::fat::tests::image());
    let mut r = open_ride(&mut d, 100);
    r.set_profile(Profile::Indoor, true);
    assert_eq!((r.profile(), r.by_hand()), (Profile::Indoor, true), "while recording");
    r.end(T0 + 100, words(100));
    r.set_profile(Profile::Mtb, false);
    assert_eq!((r.profile(), r.by_hand()), (Profile::Indoor, true), "ended: as its end says");
    assert_eq!(io_all(&mut r, &mut d, 101), [Outcome::Saved(NAME)]);
    assert_eq!(sub_sport(&d.img, SAVED), Some(6), "indoor_cycling");
}

/// `put`: nothing goes into the file of a ride once it is ended (a second, a lap, a pause after its end), nor without
/// a ride; a card that never answers fills the buffer, the seconds after are dropped, and the end still fits: the
/// ride saved with its session, its first seconds whole.
#[test]
fn nothing_after_the_end_and_the_end_always_fits() {
    let mut idle = Recorder::new();
    idle.second(&sample(0), words(0));
    idle.lap(T0);
    assert!(!idle.due() && !idle.starting() && idle.phase() == Phase::Done, "no ride: nothing");
    let laps = |img: &[u8]| {
        let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let m = crate::fit::tests::decode(&crate::fat::tests::read_all(&mut fat, &mut ram, SAVED).unwrap()).unwrap();
        m.iter().filter(|x| x.0 == 19).count()
    };
    // room in the buffer: what comes after the end is not in the file
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in 0..5 {
        r.second(&sample(t), words(t));
    }
    r.end(T0 + 5, words(5));
    r.second(&sample(6), words(6));
    r.lap(T0 + 7);
    r.gear(T0 + 7, false, 1, 2);
    assert_eq!(io_all(&mut r, &mut d, 8), [Outcome::Saved(NAME)]);
    assert_eq!(saved(&d.img, SAVED), Ok(((0..5).map(|t| T0 + t).collect(), true)));
    assert_eq!(laps(&d.img), 1, "the end's lap only");
    // a card that never answered
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in 0..300 {
        r.second(&sample(t), words(t)); // no card work: the buffer fills
    }
    r.end(T0 + 300, words(300));
    r.second(&sample(301), words(301));
    r.lap(T0 + 302);
    r.pause(T0 + 303, words(303));
    assert_eq!(io_all(&mut r, &mut d, 304), [Outcome::Saved(NAME)]);
    let (times, session) = saved(&d.img, SAVED).unwrap();
    assert!(session, "the end fits");
    let n = times.len() as u32;
    assert!(n > 30 && n < 300, "the buffer full: {n} s");
    assert_eq!(times, (0..n).map(|t| T0 + t).collect::<Vec<_>>(), "the first seconds, nothing after the end");
    assert_eq!(laps(&d.img), 1, "the end's lap only");
}

/// `put`: a checkpoint is due from the second the buffer runs low (under [`LOW`] and the end's bytes), before its
/// minute, and not a second before.
#[test]
fn a_checkpoint_is_due_once_the_buffer_runs_low() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::of(Kind::Full);
    r.start(T0 - 1, NAME, false);
    r.second(&sample(0), words(0));
    assert_eq!(r.io(&mut d, local(0)), Outcome::Nothing);
    assert!(!r.due());
    let mut t = 100;
    loop {
        let room = r.room();
        r.second(&sample(t), words(t));
        if r.due() {
            assert!(room >= LOW + Writer::END_BYTES && r.room() < LOW + Writer::END_BYTES, "{room}, {}", r.room());
            break;
        }
        t += 1;
    }
    assert!(t - 100 + 1 < CHECKPOINT_S, "before its minute: {} s", t - 100 + 1);
}

/// `starting`: from the Start until the file is made, and while a Start waits behind the last ride's save.
#[test]
fn starting_until_the_file_is_made_or_while_held() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    assert!(!r.starting(), "no ride");
    r.start(T0 - 1, NAME, false);
    assert!(r.starting(), "started");
    r.second(&sample(0), words(0));
    assert_eq!(r.io(&mut d, local(0)), Outcome::Nothing);
    assert!(!r.starting(), "the file made");
    r.end(T0 + 1, words(1));
    r.start(T0 + 2, *b"261023100002", false);
    assert!(r.starting() && r.pending(), "held behind the save");
    assert_eq!(io_all(&mut r, &mut d, 3), [Outcome::Saved(NAME)]);
    assert!(!r.starting() && r.phase() == Phase::Recording, "the held one started, its file made");
}

/// `second`: one record a second, each later than the last: seconds caught up after a long pass (one clock) take
/// the next seconds; a gap in the clock stays a gap.
#[test]
fn each_record_is_later_than_the_last() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    for t in [0, 1, 1, 1, 2, 10, 11] {
        r.second(&sample(t), words(t));
    }
    r.end(T0 + 12, words(12));
    assert_eq!(io_all(&mut r, &mut d, 12), [Outcome::Saved(NAME)]);
    let want: Vec<u32> = [0, 1, 2, 3, 4, 10, 11].iter().map(|t| T0 + t).collect();
    assert_eq!(saved(&d.img, SAVED).map(|s| s.0), Ok(want));
}

/// [`RECORD_MAX`]: the longest record (the first, its definition with every field and the developer fields, the
/// trailer after it) takes exactly that, and a buffer that has just run low (under [`LOW`]) still has room for it.
#[test]
fn the_longest_record_fits_in_record_max() {
    let mut r = Recorder::of(Kind::Full);
    r.start(T0 - 1, NAME, false);
    let before = r.writer.data_len();
    r.second(&Sample { trainer: None, ..sample(100) }, words(100));
    let used = (r.writer.data_len() - before) as usize + 2; // the trailer (CRC) after it
    assert_eq!(used, RECORD_MAX, "the longest record");
    const { assert!(RECORD_MAX < LOW, "room for a record once the buffer runs low") };
    assert_eq!(r.dropped, 0);
}

/// `io_after`: nothing of the full copy until the clean one has made its file (a ride left open is carried on only
/// when it is the clean one's): the clean copy's card work failing at the start, the full copy writes nothing and
/// waits; the card well again, both are recorded and saved.
#[test]
fn the_full_copy_waits_for_the_clean_ones_file() {
    const NEXT: [u8; 12] = *b"261023110000";
    let mut d = card(crate::fat::tests::image());
    ride_all(&mut pair(), &mut d, 0).unwrap(); // the folders made
    let bad = std::vec![dir_lbas(&d.img, "Rustfw/Rides")];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: true };
    let mut rs = pair();
    for r in rs.iter_mut() {
        r.start(T0 + 999, NEXT, false);
    }
    let [clean, full] = &mut rs;
    assert_eq!(clean.io(&mut d, local(1000)), Outcome::Failed(Error::Io));
    let (img, written) = (d.card.img.clone(), d.card.written);
    assert_eq!(full.io_after(clean, &mut d, local(1000)), Outcome::Nothing);
    assert!(full.phase() == Phase::Starting && d.card.written == written && d.card.img == img, "the full copy waits");
    d.on = false;
    let mut last = [Outcome::Nothing; 2];
    for t in 1000..1400 {
        let [clean, full] = &mut rs;
        if t < 1300 {
            clean.second(&sample(t), words(t));
            full.second(&sample(t), words(t));
        } else if t == 1300 {
            clean.end(T0 + t, words(t));
            full.end(T0 + t, words(t));
        }
        clean.keep_up(full);
        full.keep_up(clean);
        if clean.due() {
            last[0] = clean.io(&mut d, local(t));
        }
        if full.due() {
            last[1] = full.io_after(clean, &mut d, local(t));
        }
    }
    assert_eq!(last, [Outcome::Saved(NEXT); 2]);
    // the seconds while the clean copy waited out its failure, past what the buffers hold, dropped
    let (clean, _) = saved(&d.card.img, "Activities/261023110000.fit").unwrap();
    let (full, _) = saved(&d.card.img, "Rustfw/Activities/261023110000.fit").unwrap();
    assert!(clean[0] == T0 + 1000 && full[0] == clean[0] && full.iter().all(|t| clean.contains(t)));
}

/// `second`: a full copy resumed after a reset holds its seconds up to its last checkpoint already; the clock may
/// hand some of them again: never written twice.
#[test]
fn a_resumed_full_copy_never_writes_a_second_twice() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::of(Kind::Full);
    r.start(T0 - 1, NAME, false);
    for t in 0..100 {
        r.second(&sample(t), words(t));
        if r.due() {
            assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
        }
    }
    r.checkpoint_now();
    assert_eq!(r.io(&mut d, local(100)), Outcome::Nothing); // all 100 on the card: the unit off
    let mut r = Recorder::of(Kind::Full);
    assert!(matches!(r.recover(&mut d, local(105)), Outcome::Resumed(..)));
    r.resume(T0 + 100);
    for t in 95..120 {
        r.second(&sample(t), words(t));
    }
    assert_eq!(r.dropped, 0, "skipped, not refused by the writer and counted as dropped");
    r.end(T0 + 120, words(120));
    assert_eq!(io_all(&mut r, &mut d, 121), [Outcome::Saved(NAME)]);
    let want: Vec<u32> = (0..120).map(|t| T0 + t).collect();
    assert_eq!(saved(&d.img, SAVED_FULL).map(|s| s.0), Ok(want));
}

/// `second`: a second given while the ride is paused leaves its timer stopped (only a Start held behind a save starts
/// its timer at its first second): the file's events as without that second.
#[test]
fn a_second_while_paused_keeps_the_timer_stopped() {
    let events = |paused_second: bool| {
        let mut d = card(crate::fat::tests::image());
        let mut r = Recorder::new();
        r.start(T0 - 1, NAME, false);
        for t in 0..5 {
            r.second(&sample(t), words(t));
        }
        r.pause(T0 + 5, words(5));
        if paused_second {
            r.second(&sample(6), words(6));
        }
        r.end(T0 + 8, words(8));
        assert_eq!(io_all(&mut r, &mut d, 9), [Outcome::Saved(NAME)]);
        let mut ram = crate::fat::tests::Ram(d.img, 0);
        let mut fat = Fat::mount(&mut ram).unwrap();
        let m = crate::fit::tests::decode(&crate::fat::tests::read_all(&mut fat, &mut ram, SAVED).unwrap()).unwrap();
        m.into_iter().filter(|x| x.0 == 21).collect::<Vec<_>>()
    };
    let without = events(false);
    assert!(without.len() >= 2, "the start and the pause: {without:?}");
    assert_eq!(events(true), without);
}

/// `io_after`: the full copy takes the name the clean one's save of the same ride took, never an earlier ride's: a
/// ride saved a second later (its name taken in `\Activities`), then the next ride's clean save failing and kept in
/// its rides folder: that ride's full copy is saved under its own name.
#[test]
fn the_full_copy_never_takes_an_earlier_rides_saved_name() {
    let mut d = card(crate::fat::tests::image());
    let (mut rs, mut last) = (pair(), [Outcome::Nothing; 2]);
    for r in rs.iter_mut() {
        r.start(T0 - 1, NAME, false);
    }
    seconds(&mut rs, &mut d, 0, 100, &mut last);
    let mut fat = Fat::mount(&mut d).unwrap();
    let dir = fat.open(&mut d, ACTIVITIES).unwrap();
    fat.create(&mut d, dir, "261023100000.fit", (0, 0)).unwrap();
    for r in rs.iter_mut() {
        r.end(T0 + 100, words(100));
    }
    for _ in 0..3 {
        pass(&mut rs, &mut d, 100, &mut last);
    }
    const ONE_ON: [u8; 12] = *b"261023100001";
    assert_eq!(last, [Outcome::Saved(ONE_ON); 2], "the earlier ride a second later");
    // the next ride: its clean copy's save fails (`\Activities` refused) and is kept in `\Rustfw\Rides`
    const NEXT: [u8; 12] = *b"261023110000";
    let bad = std::vec![dir_lbas(&d.img, ACTIVITIES)];
    let mut d = Bad { card: d, bad, unread: Vec::new(), on: false };
    let mut out = [Vec::new(), Vec::new()];
    for t in 1000..3000 {
        let [clean, full] = &mut rs;
        if t == 1000 {
            clean.start(T0 + t - 1, NEXT, false);
            full.start_after(clean, T0 + t - 1, NEXT, false);
        }
        if t < 1100 {
            clean.second(&sample(t), words(t));
            full.second(&sample(t), words(t));
        } else if t == 1100 {
            clean.end(T0 + t, words(t));
            full.end(T0 + t, words(t));
            d.on = true;
        }
        clean.keep_up(full);
        full.keep_up(clean);
        if clean.due() {
            out[0].push(clean.io(&mut d, local(t)));
        }
        if full.due() {
            out[1].push(full.io_after(clean, &mut d, local(t)));
        }
    }
    assert_eq!(out[0].last(), Some(&Outcome::Kept(NEXT)), "{:?}", out[0]);
    assert_eq!(out[1].last(), Some(&Outcome::Saved(NEXT)), "its own name: {:?}", out[1]);
    assert!(saved(&d.card.img, "Rustfw/Activities/261023110000.fit").is_ok_and(|s| s.1));
    assert!(saved(&d.card.img, "Rustfw/Activities/261023100002.fit").is_err(), "never after the earlier name");
}

/// `second`: a checkpoint at least every minute of ride, however small the records (the buffer far from low).
#[test]
fn a_checkpoint_every_minute_whatever_the_buffer() {
    let mut d = card(crate::fat::tests::image());
    let mut r = Recorder::new();
    r.start(T0 - 1, NAME, false);
    let plain = |t: u32| Sample { time: T0 + t, ..Default::default() };
    r.second(&plain(0), words(0));
    assert_eq!(r.io(&mut d, local(0)), Outcome::Nothing);
    let mut t = 0;
    for _ in 0..3 {
        for k in 1..=CHECKPOINT_S {
            assert!(!r.due(), "second {t}");
            t += 1;
            r.second(&plain(t), words(t));
            assert_eq!(r.due(), k == CHECKPOINT_S, "second {t}");
        }
        assert_eq!(r.io(&mut d, local(t)), Outcome::Nothing);
    }
    assert!(r.room() > TAIL / 2, "the buffer far from low");
}

/// GIVE UP SAVE's rescue takes a cluster for `\Rustfw\Rescue` only with [`FULL_RESERVE`] free, counted from the FAT
/// itself (FSInfo's count may be stale after the card's errors): exactly the reserve free is enough, a cluster less
/// is not (the ride's chain then left lost, never cross-linked), and a stale FSInfo saying plenty is not believed.
#[test]
fn the_rescue_keeps_the_next_rides_reserve() {
    let reserve = |d: &Bad| {
        let mut ram = crate::fat::tests::Ram(d.card.img.clone(), 0);
        (FULL_RESERVE / u64::from(Fat::mount(&mut ram).unwrap().cluster_bytes())) as u32
    };
    for (short, tell, want) in [(0, true, GaveUp::Rescue(NAME)), (1, true, GaveUp::Lost), (1, false, GaveUp::Lost)] {
        let (mut r, mut d) = stuck_save(false);
        let free = reserve(&d) - short;
        fill(&mut d.card.img, free, tell);
        assert_eq!(r.give_up_save(&mut d, local(300)), Some(want), "{free} clusters free, FSInfo told {tell}");
        let rescued = saved(&d.card.img, "Rustfw/Rescue/261023100000.fit").is_ok();
        assert_eq!(rescued, want == GaveUp::Rescue(NAME), "{free} free, told {tell}");
        fsck(&d.card.img).unwrap();
    }
}

#[test]
fn clusters_for_a_size() {
    let fat = Fat::mount(&mut crate::fat::tests::Ram(crate::fat::tests::image(), 0)).unwrap();
    let c = fat.cluster_bytes();
    assert_eq!(fat.clusters_for(0), 1, "an empty file still has its cluster");
    assert_eq!((fat.clusters_for(c), fat.clusters_for(c + 1)), (1, 2));
}

/// One second of the totals test ride: 30 km/h, 600 W for the first 30 s of every 5 min (W' drains), 150 W after
/// it, 250 W (CP) the rest; on the barometer (reference 1), climbing from 100 m to 300 m over 10 min and back.
fn totals_input(t: u32) -> crate::ride::Input {
    let power = match t % 300 {
        0..30 => 600,
        30..120 => 150,
        _ => 250,
    };
    let up = t % 1200;
    let alt = 100_000 + 333 * if up < 600 { up } else { 1200 - up } as i32;
    crate::ride::Input { speed_x10: Some(300), power: Some(power), alt_mm: Some(alt), alt_ref: 1, ..Default::default() }
}

/// The ride and its power from second `from + 1` to `to` (CP 250 W, W' 20 kJ), as the app's pass does.
fn totals_ride(r: &mut crate::ride::Ride, p: &mut crate::power::Power, from: u32, to: u32) {
    for t in from + 1..=to {
        let i = totals_input(t);
        r.second(&i);
        p.second(i.power, r, 250, 20_000);
    }
}

/// The ride and power a restart at a checkpoint gives back: their words through the recovery record's sector, read
/// as the app's resume reads them (riding.rs `Resumed`), then Start.
fn restarted(r: &crate::ride::Ride, p: &crate::power::Power) -> (crate::ride::Ride, crate::power::Power) {
    let rec = Rec {
        seq: 1,
        phase: Phase::Recording,
        name: NAME,
        cluster: 7,
        fit: [0; RECORD_LEN],
        ride: r.to_words(),
        discard: false,
        checked: None,
        lap: [0; crate::fit::LAP_WORDS],
        more: r.more_words(p),
        timer: 0,
        by_hand: true,
    };
    let back = Rec::from_sector(&rec.to_sector()).unwrap();
    let mut r = crate::ride::Ride::resumed(&back.ride, &back.more, back.by_hand);
    let mut p2 = crate::power::Power::new();
    p2.resume(&crate::ride::power_words(&back.more));
    r.start_pause();
    (r, p2)
}

/// Finding 65 (review 2026-11-01): a 2 h ride restarted at every checkpoint (each minute, the cut right after a
/// record, so no second is ridden twice: 119 restarts) ends with the totals of the ride never cut. Exact: kJ, max
/// power, the W' balance at the end and at its lowest, the max altitude, the timer. What a resume starts again costs
/// a little per restart: the NP window refills (its first 29 s add no NP sample: 119 restarts drop half the samples
/// here, so NP within 4 % and TSS, s x NP^2, within 8 %; one restart: the next test); the ascent's low point and the
/// descent's high point restart (at most the 3 m hysteresis and a second's 0.33 m each); the 20 m climbing window
/// refills (60 s at 4 %: 500 m of uphill or downhill each).
#[test]
fn totals_go_on_across_a_restart_at_every_checkpoint() {
    use crate::{power::Power, ride::Ride};
    const N: u32 = 7200;
    let (mut whole, mut wp) = (Ride::new(), Power::new());
    whole.start_pause();
    let mut low_whole = i32::MAX;
    for t in 0..N {
        totals_ride(&mut whole, &mut wp, t, t + 1);
        low_whole = low_whole.min(wp.wbal_j().unwrap());
    }
    let (mut r, mut p) = (Ride::new(), Power::new());
    r.start_pause();
    let mut low = i32::MAX;
    for t in 0..N {
        if t > 0 && t % 60 == 0 {
            (r, p) = restarted(&r, &p);
        }
        totals_ride(&mut r, &mut p, t, t + 1);
        low = low.min(p.wbal_j().unwrap());
    }
    assert!(low_whole < 10_000, "W' drained: {low_whole} J");
    assert_eq!((p.kj(), p.max(), p.wbal_j(), low), (wp.kj(), wp.max(), wp.wbal_j(), low_whole));
    assert_eq!((r.timer_s(), r.max_alt_m(), whole.max_alt_m()), (N, Some(299), Some(299)));
    for (got, want) in [(r.descent_m(), whole.descent_m()), (r.ascent_m(), whole.ascent_m())] {
        assert!(want >= 1190 && got <= want && got + 119 * 4 >= want, "{got} m of {want}");
    }
    let (np, np_whole) = (u32::from(p.np().unwrap()), u32::from(wp.np().unwrap()));
    assert!(np.abs_diff(np_whole) * 25 <= np_whole, "NP {np} of {np_whole}");
    let (tss, tss_whole) = (p.tss(250).unwrap(), wp.tss(250).unwrap());
    assert!(tss.abs_diff(tss_whole) * 25 <= tss_whole * 2, "TSS {tss} of {tss_whole}");
    for (got, want) in [(r.uphill_m(), whole.uphill_m()), (r.downhill_m(), whole.downhill_m())] {
        assert!(want > 20_000 && got <= want && got + 119 * 500 >= want, "{got} m of {want}");
    }
}

/// The same restart at one checkpoint at a time, every minute of a 1 h ride: NP within 2 % (29 samples of 3571
/// missing, the 600 W efforts weighing most in the 4th power: 3 W at worst here), TSS within 3 %.
/// And a record of a build before the totals (bytes 228..280 and word 15 zero) resumes as before them: the totals
/// count from the resume, W' full.
#[test]
fn totals_go_on_across_one_restart_and_an_older_record_starts_them_again() {
    use crate::{power::Power, ride::Ride};
    const N: u32 = 3600;
    let (mut whole, mut wp) = (Ride::new(), Power::new());
    whole.start_pause();
    totals_ride(&mut whole, &mut wp, 0, N);
    for c in (60..N).step_by(60) {
        let (mut r, mut p) = (Ride::new(), Power::new());
        r.start_pause();
        totals_ride(&mut r, &mut p, 0, c);
        (r, p) = restarted(&r, &p);
        totals_ride(&mut r, &mut p, c, N);
        assert_eq!((p.kj(), p.max(), p.wbal_j()), (wp.kj(), wp.max(), wp.wbal_j()), "restart at {c}");
        assert_eq!(r.max_alt_m(), whole.max_alt_m(), "restart at {c}");
        assert!(r.descent_m() <= whole.descent_m() && r.descent_m() + 4 >= whole.descent_m(), "restart at {c}");
        let (np, np_whole) = (u32::from(p.np().unwrap()), u32::from(wp.np().unwrap()));
        assert!(np.abs_diff(np_whole) * 50 <= np_whole, "restart at {c}: NP {np} of {np_whole}");
        let (tss, tss_whole) = (p.tss(250).unwrap(), wp.tss(250).unwrap());
        assert!(tss.abs_diff(tss_whole) * 100 <= tss_whole * 3, "restart at {c}: TSS {tss} of {tss_whole}");
    }
    // the older record: what the resume did before the totals were kept
    let (mut r, mut p) = (Ride::new(), Power::new());
    r.start_pause();
    totals_ride(&mut r, &mut p, 0, 1800);
    let mut old = r.to_words();
    old[15] = 0;
    let mut back = Ride::resumed(&old, &[0; crate::ride::MORE_WORDS], true);
    let mut p2 = p;
    p2.resume(&[0; crate::power::POWER_WORDS]);
    let mut fresh = p;
    fresh.new_ride();
    assert_eq!(p2, fresh, "zeros: a new ride's power, the rolling averages kept");
    assert_eq!((p2.kj(), p2.max(), p2.np(), p2.wbal_j()), (0, None, None, None));
    assert_eq!((back.descent_m(), back.uphill_m(), back.max_alt_m(), back.timer_s()), (0, 0, None, 1800));
    back.start_pause();
    totals_ride(&mut back, &mut p2, 1799, 1829); // seconds 1800 to 1829
    // 30 s at 600 W from the resume (its 5 min effort), 350 W over CP: from full
    let from_resume = (p2.kj(), p2.max(), p2.wbal_j(), back.max_alt_m());
    assert_eq!(from_resume, (18, Some(600), Some(20_000 - 30 * 350), Some(299)), "from the resume");
}

/// The second review of 2026-11-03, 1: a save whose link (`Fat::link`) a cut left whole and deleted between its two
/// writes, then its chain freed by a computer (or given to another file); and a save done but for its record, then
/// the saved ride deleted by the rider (on a computer or in stock: its entry marked deleted, its chain freed). The
/// boot's save finds the chain no longer the ride's: nothing revived onto it (a cross-link, or a ride the rider
/// deleted brought back), the record done, `Nothing`; a second boot does nothing; the next ride is saved.
#[test]
fn a_link_cut_or_deleted_then_freed_is_never_revived() {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let total = full.written;
    let first = {
        let mut ram = crate::fat::tests::Ram(full.img.clone(), 0);
        Fat::mount(&mut ram).unwrap().open(&mut ram, SAVED).unwrap().cluster
    };
    for (case, cut) in [("cut inside the link", total - 2), ("deleted by the rider", total - 1)] {
        for reuse in [false, true] {
            let what = std::format!("{case}, {}", if reuse { "reused" } else { "freed" });
            let mut d = Card { budget: Some(cut), ..card(img.clone()) };
            assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}");
            let mut img = d.img;
            let mut ram = crate::fat::tests::Ram(img.clone(), 0);
            let mut fat = Fat::mount(&mut ram).unwrap();
            let acts = fat.open(&mut ram, "Activities").unwrap();
            match fat.locate(&mut ram, acts, "261023100000.fit") {
                Ok(h) => {
                    assert_eq!(case, "deleted by the rider", "{what}: linked");
                    fat.remove(&mut ram, &h).unwrap();
                    img = ram.0;
                }
                Err(e) => assert_eq!((case, e), ("cut inside the link", Error::NotFound), "{what}"),
            }
            let len = free_chain(&mut img, first);
            let other = reuse.then(|| other_file(&mut img, first, len));
            let mut d = card(img);
            assert_eq!(Recorder::new().recover(&mut d, local(SECS + 5)), Outcome::Nothing, "{what}");
            let_go(&d.img, &other, &what);
            assert_eq!(Recorder::new().recover(&mut d, local(SECS + 6)), Outcome::Nothing, "{what}: twice");
            assert!(matches!(ride(&mut Recorder::new(), &mut d, 0), Some(Outcome::Saved(_))), "{what}: next ride");
            assert_eq!(check_clean(&d.img, &what), 0);
        }
    }
}

/// The second review of 2026-11-03, 3: a save given up whose ride's entry in `\Rustfw\Rides` a torn step 4 left
/// under its alias alone, its deleted long name pieces then damaged (not its name any more): never relinked as a
/// name they do not spell, the ride kept under the name its entry has (its alias), and said so (`Kept`).
#[test]
fn a_ride_kept_under_its_alias_says_that_name() {
    let img = crate::fat::tests::image();
    let mut full = card(img.clone());
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let first = {
        let mut ram = crate::fat::tests::Ram(full.img.clone(), 0);
        Fat::mount(&mut ram).unwrap().open(&mut ram, SAVED).unwrap().cluster
    };
    // step 4's write (the fourth from the end) torn after the pieces (bytes 64 and 96), before the 8.3 entry (128)
    let mut d = Card { budget: Some(full.written - 4), torn: true, prefix: Some(100), ..card(img) };
    assert!(ride(&mut Recorder::new(), &mut d, 0).is_none());
    let mut ram = crate::fat::tests::Ram(d.img, 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let rides = fat.open(&mut ram, "Rustfw/Rides").unwrap();
    let h = fat.locate_chain(&mut ram, rides, first).unwrap();
    assert!(fat.find(&mut ram, rides, "261023100000.fit").is_err(), "the alias alone");
    let mut img = ram.0;
    let piece = h.entry_lba as usize * SECTOR + usize::from(h.slot) * 32 - 32;
    img[piece + 1] ^= 1; // the first piece's first character: not the ride's name
    let data = first_lba(&full.img, SAVED) * SECTOR + 2 * SECTOR;
    img[data..data + 16].fill(0x42); // its read-back fails: kept where it is
    let mut d = card(img);
    let Outcome::Kept(n) = Recorder::new().recover(&mut d, local(SECS + 5)) else { panic!("not kept") };
    assert_ne!(n, NAME);
    let path = std::format!("Rustfw/Rides/{}", core::str::from_utf8(&n).unwrap());
    let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    assert_eq!(fat.open(&mut ram, &path).map(|f| f.cluster), Ok(first), "{path}");
    assert_eq!(check_clean(&d.img, "kept under its alias"), 0);
    assert_eq!(Recorder::new().recover(&mut d, local(SECS + 6)), Outcome::Nothing);
}
