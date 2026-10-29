//! The recorder's tests: the card model, a power cut after every sector, the resume and the save.

extern crate std;
use super::*;
use crate::fat::Disk;
use crate::fat::write::tests::{fill, free_counts, fsck, fsck_cut, fsck_torn};
use crate::fit::RECORD_LEN;
use crate::fitread::{Msg, Reader};
use crate::ride::Profile;
use std::vec::Vec;

/// The card: the volume in memory, and a power cut after `budget` sectors written (the sector that would be
/// next and every one after it never lands, within a multi-sector write too); then it never answers again.
/// `torn`: the sector the cut falls in is left half new, half garbage (a card without reliable write).
struct Card {
    img: Vec<u8>,
    budget: Option<u64>,
    written: u64,
    dead: bool,
    torn: bool,
    cut_at: u32,
    /// The ride second `ride` was at (the cut's, once the card died).
    now: u32,
}

impl Disk for Card {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        if self.dead {
            return Err(Error::Io);
        }
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
                if self.torn && !self.dead {
                    let _ = s; // neither the old nor the new bytes: garbage
                    for (i, b) in self.img[at..at + SECTOR].iter_mut().enumerate() {
                        *b = (i as u8).wrapping_mul(97) ^ 0x5a ^ (at / SECTOR) as u8;
                    }
                }
                self.dead = true;
                return Err(Error::Io);
            }
            self.img[at..at + SECTOR].copy_from_slice(s);
            self.written += 1;
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
    }
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
        Msg::CoursePoint { .. } => {}
    });
    match rd.result() {
        Some(Ok(())) => Ok((times, session)),
        e => Err(std::format!("{e:?}")),
    }
}

const SAVED: &str = "Activities/261023100000.fit";
const SAVED_FULL: &str = "Rustfw/Activities/261023100000.fit";

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
    let mut d = Card { img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
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
    let mut d2 = Card { img: d.img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
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

/// The golden rule: the power cut after every sector of a whole ride, its start (folders made) and its save
/// included. After each, the boot's recovery gives back the ride at its last checkpoint (at most a minute
/// lost), it goes on and is saved, and the volume is clean (FATs equal, FSInfo exact, no chain into a free
/// cluster, no cross-link; a lost cluster at worst).
#[test]
fn power_cut_at_every_sector_write() {
    power_cut_at_every_sector(Profile::Road, false);
}

/// The same for an indoor ride: after every cut it comes back indoor (the recovery record keeps the profile)
/// and is saved with sub_sport indoor_cycling (6), at boot too (ended from the record alone).
#[test]
fn an_indoor_ride_cut_at_every_sector_stays_indoor() {
    power_cut_at_every_sector(Profile::Indoor, false);
}

/// The same for an MTB ride: after every cut it comes back MTB and is saved with sub_sport mountain (8).
#[test]
fn an_mtb_ride_cut_at_every_sector_stays_mtb() {
    power_cut_at_every_sector(Profile::Mtb, true);
}

/// The same for an indoor ride set by hand (no trainer: a dumb trainer, rollers): it comes back by hand too, so
/// the app does not give it back to the automatic choice (which turns it ROAD without a trainer).
#[test]
fn an_indoor_ride_by_hand_cut_at_every_sector_stays_by_hand() {
    power_cut_at_every_sector(Profile::Indoor, true);
}

fn power_cut_at_every_sector(profile: Profile, by_hand: bool) {
    PROFILE.set((profile, by_hand));
    let sub = Some(match profile {
        Profile::Road => 7,
        Profile::Indoor => 6,
        Profile::Mtb => 8,
    });
    let img = crate::fat::tests::image();
    let mut full = Card { img: img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let total = full.written;
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut resumed, mut saved_at_boot, mut nothing, mut max_lost, mut twice_cuts) = (0, 0, 0, 0, 0);
    for cut in 0..total {
        let what = std::format!("cut {cut} of {total}");
        let mut d =
            Card { img: img.clone(), budget: Some(cut), written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none(), "{what}: the card died");
        // the volume as the cut left it: readable, the FATs at most a sector apart, a chain at most one cluster past
        // its size (a cut between a checkpoint's data and its size: the boot's recovery trims it)
        let c = fsck_cut(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(c.apart <= 1, "{what}: {} FAT sectors apart", c.apart);
        let mine = {
            let mut ram = crate::fat::tests::Ram(d.img.clone(), 0);
            let mut fat = Fat::mount(&mut ram).unwrap();
            fat.open(&mut ram, "Rustfw/Rides/261023100000.fit").map_or(0, |f| f.cluster)
        };
        assert!(
            c.past.iter().all(|&(first, n)| first == mine && n <= 1),
            "{what}: chains past their size {:?}",
            c.past
        );
        // reboot; and the reboot cut too, at each of its own writes, then again: the same outcome
        let cut_img = d.img;
        let mut d =
            Card { img: cut_img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        let mut r = Recorder::new();
        let outcome = r.recover(&mut d, local(SECS + 5));
        for k in 0..d.written {
            let mut twice =
                Card { img: cut_img.clone(), budget: Some(k), written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
            Recorder::new().recover(&mut twice, local(SECS + 5));
            let mut again =
                Card { img: twice.img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
            let o = Recorder::new().recover(&mut again, local(SECS + 6));
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
            }
            Outcome::Saved(name) => {
                assert_eq!(name, NAME);
                saved_at_boot += 1;
                let (times, session) = saved(&d.img, SAVED).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(session && times == all, "{what}: the whole ride");
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
        max_lost = max_lost.max(check_clean(&d.img, &what));
    }
    std::println!(
        "{total} cut points: {resumed} resumed, {saved_at_boot} saved at boot, {nothing} before the first \
         checkpoint; lost clusters at most {max_lost}; {twice_cuts} cuts of a reboot"
    );
    assert!(resumed > 50 && saved_at_boot > 0);
    assert!(max_lost <= 2);
}

/// The golden rule on a card without reliable write: the power cut at every sector of a whole ride, the sector
/// being written left as garbage (the ride file's sector 0 and last sector, its directory sector, a FAT
/// sector, FSInfo, a record). The ride is resumed (at most a minute lost) or saved from its record, never
/// lost once its first checkpoint is on the card; the next ride is recorded and saved; the volume is clean.
/// A torn directory sector of `\Activities` or the root (stock's entries) is the card's reliable write's to
/// prevent (emmc.rs): here it only must not stop the recorder.
#[test]
fn power_cut_tearing_the_sector_at_every_write() {
    let img = crate::fat::tests::image();
    let mut full = Card { img: img.clone(), budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
    ride(&mut Recorder::new(), &mut full, 0).unwrap();
    let total = full.written;
    let all: Vec<u32> = (0..SECS).filter(|&t| recorded(t)).map(|t| T0 + t).collect();
    let (mut promised, mut resumed, mut saved_at_boot, mut max_lost, mut torn_root) = (false, 0, 0, 0, 0);
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
    for cut in 0..total {
        let mut d =
            Card { img: img.clone(), budget: Some(cut), written: 0, dead: false, torn: true, cut_at: 0, now: 0 };
        assert!(ride(&mut Recorder::new(), &mut d, 0).is_none());
        let what = std::format!("cut {cut} of {total}, sector {} torn", d.cut_at);
        let cut_lba = d.cut_at;
        let had = all.iter().filter(|&&a| a <= T0 + d.now).count(); // the seconds recorded when the power went
        let mut d = Card { img: d.img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 };
        let mut r = Recorder::new();
        match r.recover(&mut d, local(SECS + 5)) {
            Outcome::Resumed(w, _) => {
                resumed += 1;
                let from = w[0] + 1;
                let from = if (PAUSE + 1..RESUME).contains(&from) { RESUME } else { from };
                assert_eq!(ride(&mut r, &mut d, from), Some(Outcome::Saved(NAME)), "{what}");
            }
            Outcome::Saved(n) => {
                assert_eq!(n, NAME, "{what}");
                saved_at_boot += 1;
            }
            Outcome::Nothing => assert!(!promised, "{what}: a ride with a checkpoint lost"),
            o => panic!("{what}: {o:?}"),
        }
        if let Ok((times, _)) = saved(&d.img, SAVED) {
            promised = true;
            assert!(times.windows(2).all(|p| p[0] < p[1]) && times.iter().all(|t| all.contains(t)), "{what}");
            let kept = times.iter().filter(|&&t| t <= all[had.max(1) - 1]).count();
            assert!(had - kept <= CHECKPOINT_S as usize, "{what}: {} s lost", had - kept);
        }
        let Some(Outcome::Saved(n)) = ride(&mut Recorder::new(), &mut d, 0) else { panic!("{what}: the next ride") };
        let path = std::format!("Activities/{}.fit", core::str::from_utf8(&n).unwrap());
        assert_eq!(saved(&d.img, &path).map(|s| s.0 == all), Ok(true), "{what}: the next ride");
        let lost = fsck_torn(&d.img).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(free_counts(&d.img).0, free_counts(&d.img).1, "{what}: FSInfo's free count");
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
        "{total} torn cuts: {resumed} resumed, {saved_at_boot} saved at boot; lost clusters at most {max_lost}; \
         {torn_root} in the root directory"
    );
    assert!(resumed > 50 && saved_at_boot > 10);
    assert!(max_lost <= 2);
}

fn card(img: Vec<u8>) -> Card {
    Card { img, budget: None, written: 0, dead: false, torn: false, cut_at: 0, now: 0 }
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

/// The golden rule with two copies: the power cut (the sector left whole, or torn) after every sector of a ride
/// recorded by both. The clean copy comes back as with one recorder (resumed or saved at boot, at most a minute
/// lost); the full one is resumed or saved too, or, cut before its first checkpoint, never promised; a full
/// one back without its clean one is ended and saved (the app's rule). The next ride gets both, the volume
/// stays clean.
///
/// Four tests run in parallel, each every fourth cut (`part` 0 to 3 of [`CUT_PARTS`]).
#[test]
fn two_copies_power_cut_at_every_sector_part_0() {
    two_copies_cut(0);
}
#[test]
fn two_copies_power_cut_at_every_sector_part_1() {
    two_copies_cut(1);
}
#[test]
fn two_copies_power_cut_at_every_sector_part_2() {
    two_copies_cut(2);
}
#[test]
fn two_copies_power_cut_at_every_sector_part_3() {
    two_copies_cut(3);
}

/// The cuts are shared among this many tests (cut `c` belongs to part `c % CUT_PARTS`), so each sees the whole
/// ride's range of cut points, and the four run in parallel.
const CUT_PARTS: u64 = 4;

fn two_copies_cut(part: u64) {
    for torn in [false, true] {
        two_copies_cut_torn(torn, part);
    }
}

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

/// The golden rule through the discard: the power cut (the sector left whole, or torn) after every sector of a
/// ride recorded by both copies and discarded. After the next start each copy is in exactly one place, its ride
/// folder (resumed, paused, then discarded again) or `\Rustfw\Discarded`, never in `\Activities` or
/// `\Rustfw\Activities`, with at most a minute lost; the volume stays clean. One exception, on a card without
/// reliable write only: a torn sector that takes the ride's entry before the end's record (which is the first to
/// say "discard") has the ride saved whole from its record at the start (`recover_from`, as for any ride whose
/// entry a cut tore), in `\Activities`: in one place still, never lost (HISTORY's DELETE moves it after).
///
/// Four tests run in parallel, each every fourth cut ([`CUT_PARTS`]).
#[test]
fn a_discard_cut_at_every_sector_part_0() {
    a_discard_cut(0);
}
#[test]
fn a_discard_cut_at_every_sector_part_1() {
    a_discard_cut(1);
}
#[test]
fn a_discard_cut_at_every_sector_part_2() {
    a_discard_cut(2);
}
#[test]
fn a_discard_cut_at_every_sector_part_3() {
    a_discard_cut(3);
}

/// The cuts of the discard's ride at which the next start finds it already discarded (both torn modes).
const AT_BOOT_CUTS: [u64; 3] = [719, 720, 721];

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
    let at = (lba * SECTOR..(lba + 1) * SECTOR)
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
        Rec { seq, phase, name: NAME, cluster: 7, fit: [0; RECORD_LEN], ride, discard: false, checked: None }
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
    for at in [0, 4, 8, 9, 24, 40, 100, 511] {
        let mut bad = s;
        bad[at] ^= 0x10;
        assert_eq!(Rec::from_sector(&bad), None, "byte {at}");
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

/// REVIEW: a ride resumed at boot (setup never ran for it) on a volume without the Discarded folders (a ride open
/// at the update), discarded, the power cut (torn) at every sector: is the ride ever lost?
/// A discard never makes a folder: a ride resumed with the folders missing, then discarded, is in one place
/// whatever the cut.
#[test]
fn a_resumed_ride_discarded_without_folders_survives_every_cut() {
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
    for torn in [false, true] {
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
    assert!(bad.is_empty(), "{} bad cuts of {}: {bad:?}", bad.len(), 2 * total);
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
    assert_eq!(rs[0].io(&mut d, local(90)), Outcome::Failed(Error::Io));
    let [clean, full] = &mut rs;
    assert_eq!(full.io_after(clean, &mut d, local(90)), Outcome::Nothing);
    assert_eq!((clean.phase(), full.phase()), (Phase::Saving, Phase::Saving), "both ends on the card");
    // a power cut now: the full copy is saved whole at the boot
    let mut f = Recorder::of(Kind::Full);
    let mut c = card(d.card.img.clone());
    assert_eq!(f.recover(&mut c, local(100)), Outcome::Saved(NAME));
    let (times, session) = saved(&c.img, SAVED_FULL).unwrap();
    assert!(session && times.len() == 90, "{}", times.len());
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
    let mut ram = crate::fat::tests::Ram(img.to_vec(), 0);
    let mut fat = Fat::mount(&mut ram).unwrap();
    let v = crate::fat::tests::read_all(&mut fat, &mut ram, "Rustfw/ride.rec").unwrap();
    let rec = |k: usize| Rec::from_sector(v[(3 * k + 2) * SECTOR..(3 * k + 3) * SECTOR].try_into().unwrap());
    [rec(0), rec(1)].into_iter().flatten().max_by_key(|r| r.seq).unwrap()
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

/// The review of 2026-10-29, S5: a checkpoint that grows the ride's chain into a new cluster, then fails once (its
/// new cluster's data, sector 0, the size or the record), used to forget the grown chain: the retry took another
/// cluster and relinked the last one to it, the first one left taken, named by nothing (16 KB per failure, counted
/// used by FSInfo). The retry goes on from the grown chain: the ride saved whole, no cluster lost, FSInfo exact.
/// The second FAT's sector failing twice in a row (the third review, finding 1): the retry's take-up fails too, and
/// the one after it still takes the cluster up, the ride recorded on.
#[test]
fn a_checkpoint_failing_once_after_growing_loses_no_cluster() {
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
        assert_eq!(check_clean(&d.card.img, what), 0, "{what}: no cluster lost");
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

/// The review of 2026-10-29, S6: the writes of a path cut at every sector, plain and torn. `setup` brings a
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
        for torn in [false, true] {
            let what = std::format!("cut {cut} of {total}, torn {torn}");
            let (mut r, mut d) = setup();
            let c = inner(&mut d);
            (c.budget, c.torn) = (Some(c.written + cut), torn);
            act(&mut r, &mut d);
            if torn && mine.contains(&inner(&mut d).cut_at) {
                s7 += 1;
                continue;
            }
            let mut b = card(inner(&mut d).img.clone());
            boot_and_settle(&mut b, &what);
            whole_once(&b.img, &want, &what);
            let l = if torn { fsck_torn(&b.img) } else { fsck(&b.img) }.unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(free_counts(&b.img).0, free_counts(&b.img).1, "{what}: FSInfo's free count");
            lost = lost.max(l);
        }
    }
    std::println!("{total} cut points, plain and torn: lost clusters at most {lost}; {s7} torn in \\Rustfw left out");
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
    assert_eq!((total, s7), (6, 0), "a write added or dropped");
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
    // the rescue folder's entry in \Rustfw: one sector, its tear S7's
    assert_eq!((total, s7), (39, 1), "a write added or dropped");
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
    let cases = [("left open", open.img, 2 * 86_400, false, 27), ("save cut", save_cut, 5000, true, 12)];
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
