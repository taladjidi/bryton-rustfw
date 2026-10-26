//! The recent full copies read for the heart rate variability thresholds ([`crate::hrvt`]): each ride of the last
//! [`hrvt::DAYS`] days in `\Rustfw\Activities` (the full copies, with `Alpha1`: recorder.rs `Kind::Full`) through
//! the FIT reader with alpha1 ([`Reader::with_alpha1`]) into a [`hrvt::RideFit`], its thresholds into a
//! [`hrvt::Recent`]. A bounded amount a call ([`Scan::step`]), so the main loop spreads it over its passes as the
//! ride scan does (stock.rs); read only. Nothing is kept on the card: it is read again at each power-up (or after
//! a ride is saved) when FITNESS first asks.

use crate::fat::{self, Cursor, Disk, Fat, File, Seek};
use crate::fitread::{Msg, Reader};
use crate::hrvt;
use crate::stock::{IO, ride_day};

/// Where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not started.
    Idle,
    /// Reading.
    Running,
    /// Every recent ride read.
    Done,
    /// The volume or the folder could not be read (none yet: no full copy).
    Failed,
}

/// The reading of the recent full copies.
pub struct Scan {
    status: Status,
    /// Today, days since 1970.
    today: u16,
    dir: Option<Cursor>,
    /// The ride being read, the bytes of it read, where its chain is.
    file: Option<(File, u32, Seek)>,
    reader: Reader,
    ride: hrvt::RideFit,
    /// What the rides read so far give.
    pub out: hrvt::Recent,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    /// Not started.
    pub const fn new() -> Self {
        Scan {
            status: Status::Idle,
            today: 0,
            dir: None,
            file: None,
            reader: Reader::with_alpha1(),
            ride: hrvt::RideFit::new(),
            out: hrvt::Recent::new(),
        }
    }

    /// Where it is.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Starts again from the first ride, `today` (days since 1970) setting the window.
    pub fn start(&mut self, today: u16) {
        *self = Scan { status: Status::Running, today, ..Scan::new() };
    }

    /// Back to not started (a ride was saved: read again when next asked).
    pub fn reset(&mut self) {
        *self = Scan::new();
    }

    /// Reads on for about `budget` bytes; true while it goes on.
    #[inline(never)] // its read buffer lives in its own frame, not in the caller's (the main loop's)
    pub fn step(&mut self, d: &mut impl Disk, budget: u32) -> bool {
        if self.status != Status::Running {
            return false;
        }
        let Ok(mut fat) = Fat::mount(d) else {
            self.status = Status::Failed;
            return false;
        };
        let mut io = [0u8; IO];
        let mut spent = 0u32;
        while self.status == Status::Running && spent < budget {
            spent = spent.saturating_add(self.advance(d, &mut fat, &mut io));
        }
        self.status == Status::Running
    }

    /// One step: a directory entry, or a buffer of the ride being read; the bytes it cost.
    fn advance(&mut self, d: &mut impl Disk, fat: &mut Fat, io: &mut [u8; IO]) -> u32 {
        let dir = match &mut self.dir {
            Some(c) => c,
            None => match fat.open(d, crate::recorder::FULL_PATH).and_then(|f| fat.cursor(f)) {
                Ok(c) => self.dir.insert(c),
                Err(_) => {
                    self.status = Status::Failed;
                    return 0;
                }
            },
        };
        let Some((file, offset, seek)) = &mut self.file else {
            match fat.list(d, dir) {
                Ok(None) => self.status = Status::Done,
                Err(_) => self.status = Status::Failed,
                Ok(Some(e)) => {
                    let recent = ride_day(e.name()).is_some_and(|day| self.today.saturating_sub(day) < hrvt::DAYS);
                    if !e.file.dir && recent {
                        self.file = Some((e.file, 0, Seek::default()));
                        (self.reader, self.ride) = (Reader::with_alpha1(), hrvt::RideFit::new());
                    }
                }
            }
            return fat::SECTOR as u32 / 16; // a 32-byte entry
        };
        let n = fat.read_from(d, file, *offset, io, seek).unwrap_or(0);
        *offset = offset.saturating_add(n as u32);
        let ride = &mut self.ride;
        self.reader.push_slice(io.get(..n).unwrap_or(&[]), |m| {
            if let Msg::Record(r) = m {
                ride.sample(r.time, r.power, r.alpha1_milli);
            }
        });
        // the end of the file, its end of data or a bad byte: the ride as far as it was read (a cut copy counts)
        if n == 0 || self.reader.result().is_some() {
            self.out.add(self.ride.thresholds());
            self.file = None;
        }
        (n as u32).max(1)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, image};
    use crate::fit::{Sample, Writer, fit_time};
    use std::vec::Vec;

    /// A full copy (developer fields) of `secs` s from FIT time `t0`: power `p(t)`, alpha1 by the rider's line
    /// (0.75 at 210 W, 0.5 at 265 W) on the 2-minute mean, or none.
    pub(crate) fn full_copy(t0: u32, secs: u32, p: impl Fn(u32) -> u16, alpha1: bool) -> Vec<u8> {
        let mut f = Vec::new();
        let mut w = Writer::begin_with(&mut f, t0, true).unwrap();
        for t in 0..secs {
            let lo = t.saturating_sub(119);
            let mean = (lo..=t).map(|s| f32::from(p(s))).sum::<f32>() / (t - lo + 1) as f32;
            let a1 = (0.75 - (mean - 210.0) * 0.25 / 55.0).clamp(0.2, 1.6);
            let s = Sample {
                time: t0 + 1 + t,
                power: Some(p(t)),
                hr: Some(140),
                alpha1_milli: alpha1.then_some((a1 * 1000.0) as u16),
                ..Default::default()
            };
            w.record(&mut f, &s).unwrap();
        }
        w.finish(&mut f, t0 + secs + 1).unwrap();
        f
    }

    /// The synthetic volume with `files` in `\Rustfw\Activities`.
    pub(crate) fn volume(files: &[(&str, Vec<u8>)]) -> Ram {
        let mut d = Ram(image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let t = (0, 0);
        let rustfw = fat.ensure_dir(&mut d, fat.root(), "Rustfw", t).unwrap();
        let dir = fat.ensure_dir(&mut d, rustfw, "Activities", t).unwrap();
        for (name, data) in files {
            let mut h = fat.create(&mut d, dir, name, t).unwrap();
            let mut padded = data.clone();
            padded.resize(data.len().next_multiple_of(fat::SECTOR), 0);
            fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
            fat.set_size(&mut d, &mut h, data.len() as u32, t).unwrap();
        }
        d
    }

    /// Recent full copies are read, a bounded amount a call: the ride of steps gives both thresholds, the one
    /// without alpha1 (a ride with no strap) nothing, the old one is not read.
    #[test]
    fn recent_rides() {
        let steps = |t: u32| (140 + t / 600 * 20) as u16;
        let day = |y, m, d| u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap();
        let mut d = volume(&[
            ("261020100000.fit", full_copy(fit_time((2026, 10, 20, 10, 0, 0)), 4800, steps, true)),
            ("261022100000.fit", full_copy(fit_time((2026, 10, 22, 10, 0, 0)), 1800, steps, false)),
            ("260801100000.fit", full_copy(fit_time((2026, 8, 1, 10, 0, 0)), 4800, steps, true)),
        ]);
        let mut s = Scan::new();
        assert!(!s.step(&mut d, 1 << 20), "not started");
        s.start(day(2026, 10, 26));
        let mut calls = 0;
        while s.step(&mut d, 16 * 1024) {
            calls += 1;
            assert!(calls < 1000);
        }
        assert!(calls > 3, "spread over calls: {calls}");
        assert_eq!(s.status(), Status::Done);
        let ((vt1, n1), (vt2, n2)) = (s.out.vt1(), s.out.vt2());
        assert_eq!((n1, n2, s.out.read), (1, 1, 2), "two recent rides read, one gave both");
        assert!(vt1.unwrap().abs_diff(210) <= 6 && vt2.unwrap().abs_diff(265) <= 6, "{vt1:?} {vt2:?}");
        assert_eq!(s.out.vt2_trusted(), None, "one ride: not trusted");
    }

    /// No `\Rustfw\Activities`: failed, nothing shown.
    #[test]
    fn no_folder() {
        let mut d = Ram(image(), 0);
        let mut s = Scan::new();
        s.start(20_000);
        assert!(!s.step(&mut d, 1 << 20));
        assert_eq!((s.status(), s.out.read), (Status::Failed, 0));
    }

    /// The owner's own full copies, never in the repository: `HRVT_FILES` names them (colon separated); prints
    /// each ride's fit. `HRVT_FILES=a.fit:b.fit cargo test-host real_rides -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_rides() {
        let files = std::env::var("HRVT_FILES").unwrap_or_default();
        for path in files.split(':').filter(|p| !p.is_empty()) {
            let data = std::fs::read(path).unwrap();
            let (mut r, mut fit, mut with) = (Reader::with_alpha1(), hrvt::RideFit::new(), 0);
            r.push_slice(&data, |m| {
                if let Msg::Record(x) = m {
                    with += usize::from(x.alpha1_milli.is_some());
                    fit.sample(x.time, x.power, x.alpha1_milli);
                }
            });
            std::println!("{path}: {with} with alpha1, {:?}, fit {:?}, {:?}", r.result(), fit.fit(), fit.thresholds());
        }
    }
}
