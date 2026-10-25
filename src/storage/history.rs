//! The ride history: stock's rides (`\Activities\YYMMDDhhmmss.fit`) as a list, newest first, and the card of any
//! one of them, the same card the app shows when a ride ends ([`crate::ui::card`]).
//!
//! - **The index** ([`Index`]): filled by the scan of stock's rides ([`crate::stock::Scan`]), which reads every
//!   ride anyway; per ride a [`Ride`] of about 50 bytes (its file, start and totals), the newest [`RIDES`] kept in
//!   a fixed array, sorted on insertion. No second pass over the card.
//! - **A past ride's card** ([`Past::start`], then [`Past::step`] once a main loop pass, a bounded amount each: a
//!   long ride is about a second of reading, never in one pass): its file read once, in [`IO`]-byte reads, through a FIT reader that
//!   picks the position and altitude ([`Reader::with_position`]): the track ([`crate::map::Track`], thinned as a
//!   live one), the elevation profile (the altitudes, decimated to [`SAMPLES`], then one column an equal share of the distance),
//!   and the numbers: the session's (time, distance, ascent, average power and heart rate) and the records' (NP
//!   with [`crate::power`]'s method, a record a second; maximum heart rate; work). TSS with the FTP set now.
//!
//! The file content is untrusted (fuzzed in sim.rs `fuzz`): every loop is bounded by the file's size (at most
//! [`MAX_BYTES`]) or a fixed array, nothing indexes unchecked. Read only, as all of the FAT reader; a ride is
//! deleted (moved out of sight, kept) by [`crate::trash`].

use crate::fat::{self, Disk, Fat, File};
use crate::fitread::{Msg, Reader, Session};
use crate::map::Track;
use crate::power::{self, Power};
use crate::ride::{ALT_OFFSET, alt_scale};
use crate::ui::card::{self, Card, Stats};

/// Rides the index keeps (the newest).
pub const RIDES: usize = 128;
/// Bytes read from the card per disk read (stock.rs's scan reads as much).
pub const IO: usize = crate::stock::IO;
/// A file longer than this is read only this far: about 2 s on the unit over the passes (a ten-hour ride is about
/// 1.5 MB).
pub const MAX_BYTES: u32 = 4 << 20;
/// Altitudes kept for the profile at most; when full, every other one goes (as the track's points).
pub const SAMPLES: usize = 512;
/// Columns of the profile.
pub const COLS: usize = card::ELEV.2 as usize;

/// When a ride started, local time as stock names its files: year, month, day, hours, minutes.
pub type Start = (u16, u8, u8, u8, u8);

/// A past ride: its file, its start, and its totals as the list and its card show them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ride {
    /// The file.
    pub file: File,
    /// The start (from the file's name).
    pub start: Start,
    /// Moving time, s.
    pub timer_s: u32,
    /// Distance, m.
    pub dist_m: u32,
    /// Ascent, m.
    pub ascent_m: Option<u16>,
    /// Average power, W (the session's, else the records').
    pub avg_power: Option<u16>,
    /// Normalized power, W, from the records.
    pub np: Option<u16>,
    /// Records with power (seconds, for TSS).
    pub power_s: u32,
    /// Work, kJ.
    pub kj: u32,
    /// Average heart rate (the session's), bpm.
    pub avg_hr: Option<u8>,
    /// Maximum heart rate (the records'), bpm.
    pub max_hr: Option<u8>,
}

/// No ride: a ride's fields all empty.
pub(crate) const NONE: Ride = Ride {
    file: File { cluster: 0, size: 0, dir: false },
    start: (0, 0, 0, 0, 0),
    timer_s: 0,
    dist_m: 0,
    ascent_m: None,
    avg_power: None,
    np: None,
    power_s: 0,
    kj: 0,
    avg_hr: None,
    max_hr: None,
};

impl Ride {
    /// Training stress score at `ftp`.
    pub fn tss(&self, ftp: u16) -> Option<u32> {
        self.np.map(|np| power::tss(self.power_s, np, ftp))
    }

    /// The numbers its card shows, TSS at `ftp`.
    pub fn stats(&self, ftp: u16) -> Stats {
        let (y, mo, d, h, mi) = self.start;
        let speed = (self.timer_s > 0).then(|| (u64::from(self.dist_m) * 36 / u64::from(self.timer_s)) as u16);
        Stats {
            start: ((y, mo, d), (h, mi)),
            timer_s: self.timer_s,
            dist_m: self.dist_m,
            avg_speed_x10: speed,
            ascent_m: self.ascent_m.map(u32::from),
            np: self.np,
            avg_power: self.avg_power,
            avg_hr: self.avg_hr.map(u16::from),
            max_hr: self.max_hr.map(u16::from),
            tss: self.tss(ftp),
            kj: Some(self.kj),
        }
    }
}

/// A ride's totals as its messages go by: the session, and from the records NP, work and the maximum heart rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct Acc {
    power: Power,
    power_s: u32,
    max_hr: u8,
    session: Option<Session>,
}

impl Acc {
    /// Nothing yet.
    pub const fn new() -> Self {
        Acc { power: Power::new(), power_s: 0, max_hr: 0, session: None }
    }

    /// A message of the file: a record counts as a moving second (ponytail: a record a second, as stock records;
    /// a file with sparser records gives a lower NP and TSS).
    pub fn take(&mut self, m: &Msg) {
        match m {
            Msg::Record(r) => {
                self.power.sample(r.power, true);
                self.power_s = self.power_s.saturating_add(u32::from(r.power.is_some()));
                self.max_hr = self.max_hr.max(r.hr.unwrap_or(0));
            }
            Msg::Session(s) => self.session = self.session.or(Some(*s)),
            Msg::CoursePoint { .. } => {}
        }
    }

    /// The session, once read.
    pub fn session(&self) -> Option<Session> {
        self.session
    }

    /// The ride in `file`, started at `start`; None without a session (not a whole ride).
    pub fn ride(&self, file: File, start: Start) -> Option<Ride> {
        let s = self.session?;
        Some(Ride {
            file,
            start,
            timer_s: s.timer_ms.unwrap_or(0) / 1000,
            dist_m: s.dist_cm.unwrap_or(0) / 100,
            ascent_m: s.ascent_m,
            avg_power: s.avg_power.or(self.power.avg()),
            np: self.power.np(),
            power_s: self.power_s,
            kj: self.power.kj(),
            avg_hr: s.avg_hr,
            max_hr: (self.max_hr > 0).then_some(self.max_hr),
        })
    }
}

/// The newest [`RIDES`] rides, newest first.
pub struct Index {
    rides: [Ride; RIDES],
    len: usize,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Index {
    /// No rides.
    pub const fn new() -> Self {
        Index { rides: [NONE; RIDES], len: 0 }
    }

    /// Forgets them all.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Rides kept.
    pub fn len(&self) -> usize {
        self.len
    }

    /// No rides kept.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Ride `i`, newest first.
    pub fn get(&self, i: usize) -> Option<&Ride> {
        self.rides.get(..self.len)?.get(i)
    }

    /// Adds `r` in its place (newest first); when full, the oldest goes (`r` itself if it is older than all).
    pub fn add(&mut self, r: Ride) {
        let kept = self.rides.get(..self.len).unwrap_or(&[]);
        let at = kept.iter().position(|k| k.start < r.start).unwrap_or(self.len);
        if at >= RIDES {
            return;
        }
        let end = self.len.min(RIDES - 1);
        self.rides.copy_within(at..end, at + 1);
        if let Some(slot) = self.rides.get_mut(at) {
            *slot = r;
        }
        self.len = (self.len + 1).min(RIDES);
    }
}

/// FIT semicircles to degrees x 1e-7 (2^31 semicircles are 180 degrees).
pub(crate) fn degrees_e7(sc: i32) -> i32 {
    ((i64::from(sc) * 1_800_000_000) >> 31) as i32
}

/// The card of a past ride: its track, its profile, its numbers; and the ride being read for it.
pub struct Past {
    /// The track (metres from its first point).
    pub track: Track,
    /// The title and the numbers.
    pub card: Card,
    /// The profile: a column's altitude, m + [`ALT_OFFSET`] (0: none).
    elev: [u16; COLS],
    scale: (u32, u32),
    /// Bytes of the file read for it.
    pub bytes: u32,
    samples: Samples,
    /// The ride being read ([`Past::step`]): it, the FTP for its TSS, its reader and totals so far.
    reading: Option<(Ride, u16, Reader, Acc)>,
    /// The ride last asked for (the one the card shows once read): what DELETE moves.
    pub ride: Ride,
}

impl Default for Past {
    fn default() -> Self {
        Self::new()
    }
}

impl Past {
    /// No ride.
    pub const fn new() -> Self {
        Past {
            track: Track::new(),
            card: Card::EMPTY,
            elev: [0; COLS],
            scale: (0, 1),
            bytes: 0,
            samples: Samples::new(),
            reading: None,
            ride: NONE,
        }
    }

    /// Starts reading `ride`'s file into the card, TSS at `ftp`: over the next [`Past::step`]s.
    pub fn start(&mut self, ride: &Ride, ftp: u16) {
        (self.track, self.samples, self.bytes, self.ride) = (Track::new(), Samples::new(), 0, *ride);
        self.reading = Some((*ride, ftp, Reader::with_position(), Acc::new()));
    }

    /// A ride is being read.
    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Stops reading (the card is not wanted any more).
    pub fn stop(&mut self) {
        self.reading = None;
    }

    /// Reads on for about `budget` bytes (the whole file, or [`MAX_BYTES`] of it, over the calls); Some once the
    /// card is done or the file could not be read. A file that has changed or gone bad since the index gives what
    /// could be read, with the index's numbers when it has no session.
    #[inline(never)] // its read buffer lives in its own frame, not in the caller's (the main loop's)
    pub fn step(&mut self, d: &mut impl Disk, budget: u32) -> Option<Result<(), fat::Error>> {
        let (ride, ftp, reader, acc) = self.reading.as_mut()?;
        let mut fat = match Fat::mount(d) {
            Ok(f) => f,
            Err(e) => {
                self.reading = None;
                return Some(Err(e));
            }
        };
        let mut io = [0u8; IO];
        let (size, mut spent) = (ride.file.size.min(MAX_BYTES), 0u32);
        let mut done = false;
        while !done && self.bytes < size && spent < budget {
            let n = match fat.read_at(d, &ride.file, self.bytes, &mut io) {
                Ok(n) => n,
                Err(e) => {
                    self.reading = None;
                    return Some(Err(e));
                }
            };
            (self.bytes, spent) = (self.bytes.saturating_add(n as u32), spent.saturating_add(n as u32));
            let (track, samples) = (&mut self.track, &mut self.samples);
            reader.push_slice(io.get(..n).unwrap_or(&[]), |m| {
                acc.take(&m);
                let Msg::Record(r) = m else { return };
                if let Some((lat, lon)) = r.pos.filter(|p| p.0.unsigned_abs() <= 1 << 30) {
                    track.add(degrees_e7(lat), degrees_e7(lon), 0);
                }
                // a record without a distance stands where the last one did
                if let Some(cm) = r.dist_cm {
                    samples.dist = cm;
                }
                if let Some(dm) = r.alt_dm {
                    samples.push((dm / 10 + ALT_OFFSET).clamp(1, i32::from(u16::MAX)) as u16);
                }
            });
            done = n == 0 || reader.result().is_some();
        }
        if !done && self.bytes < size {
            return None;
        }
        self.track.end();
        let got = acc.ride(ride.file, ride.start).unwrap_or(*ride);
        self.card = Card::of(&got.stats(*ftp));
        self.samples.columns(&mut self.elev);
        self.scale = alt_scale(self.elev.iter().map(|&v| u32::from(v)));
        self.reading = None;
        Some(Ok(()))
    }

    /// Column `i` of the profile `h` high: the bar's height and colour (as the live ride's altitude graph).
    pub fn elev(&self, i: u32, h: u32) -> (u32, u16) {
        let v = u32::from(self.elev.get(i as usize).copied().unwrap_or(0));
        if v == 0 {
            return (0, crate::ui::BLACK);
        }
        let (low, high) = self.scale;
        let height = v.saturating_sub(low) * h / high.saturating_sub(low).max(1);
        (height.clamp(1, h), crate::ui::GREY)
    }
}

/// Altitudes, one every `every` given, thinned to every other one when full.
struct Samples {
    v: [u16; SAMPLES],
    /// The distance (1/100 m) of each, and the last one read.
    d: [u32; SAMPLES],
    dist: u32,
    len: usize,
    every: u32,
    k: u32,
}

impl Samples {
    const fn new() -> Self {
        Samples { v: [0; SAMPLES], d: [0; SAMPLES], dist: 0, len: 0, every: 1, k: 0 }
    }

    fn push(&mut self, a: u16) {
        let k = self.k;
        self.k = k.wrapping_add(1);
        if !k.is_multiple_of(self.every) {
            return;
        }
        if self.len == SAMPLES {
            for i in 0..SAMPLES / 2 {
                self.v[i] = self.v[2 * i];
                self.d[i] = self.d[2 * i];
            }
            self.len = SAMPLES / 2;
            self.every = self.every.saturating_mul(2);
        }
        if let Some(slot) = self.v.get_mut(self.len) {
            *slot = a;
            if let Some(d) = self.d.get_mut(self.len) {
                *d = self.dist;
            }
            self.len += 1;
        }
    }
}

impl Samples {
    /// The profile: each column the mean altitude over an equal share of the distance (a column nothing fell in
    /// takes the one before). Without a distance (all at 0), an equal share of the samples.
    fn columns(&self, out: &mut [u16; COLS]) {
        let kept = self.v.get(..self.len).unwrap_or(&[]);
        let total = self.d.get(..self.len).and_then(|d| d.iter().max()).copied().unwrap_or(0);
        if total == 0 {
            for (i, col) in out.iter_mut().enumerate() {
                // the samples column i stands for (at least one: a short ride is stretched)
                let (lo, hi) = (i * kept.len() / COLS, (i + 1) * kept.len() / COLS);
                let part = kept.get(lo..hi.max(lo + 1)).unwrap_or(&[]);
                let sum: u32 = part.iter().map(|&v| u32::from(v)).sum();
                *col = sum.checked_div(part.len() as u32).unwrap_or(0) as u16;
            }
            return;
        }
        let mut acc = [(0u32, 0u32); COLS];
        for (&a, &d) in kept.iter().zip(&self.d) {
            let i = (u64::from(d) * COLS as u64 / (u64::from(total) + 1)) as usize;
            if let Some(c) = acc.get_mut(i) {
                *c = (c.0 + u32::from(a), c.1 + 1);
            }
        }
        let mut last = 0;
        for (col, (sum, n)) in out.iter_mut().zip(acc) {
            last = sum.checked_div(n).map_or(last, |v| v as u16);
            *col = last;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn ride(y: u16, d: u8) -> Ride {
        Ride { start: (y, 10, d, 8, 0), ..NONE }
    }

    /// Newest first whatever the order they come in; full, the oldest goes.
    #[test]
    fn index_keeps_the_newest() {
        let mut ix = Index::new();
        for d in [3, 1, 2] {
            ix.add(ride(2026, d));
        }
        let days: std::vec::Vec<u8> = (0..ix.len()).map(|i| ix.get(i).unwrap().start.2).collect();
        assert_eq!(days, [3, 2, 1]);
        ix.clear();
        for k in 0..RIDES as u16 + 10 {
            ix.add(ride(1900 + k, 1));
        }
        ix.add(ride(1800, 1)); // older than all: not kept
        assert_eq!(ix.len(), RIDES);
        assert_eq!((ix.get(0).unwrap().start.0, ix.get(RIDES - 1).unwrap().start.0), (2037, 1910));
        assert_eq!(ix.get(RIDES), None);
    }

    /// The altitudes thin evenly, a ride of any length keeps between half and all of [`SAMPLES`].
    #[test]
    fn samples_thin() {
        let mut s = Samples::new();
        for a in 0..10_000u16 {
            s.push(a);
        }
        assert!(s.len > SAMPLES / 2 && s.len <= SAMPLES, "{}", s.len);
        assert_eq!(s.v[0], 0);
        let step = s.v[1] - s.v[0];
        assert!(s.v[..s.len].windows(2).all(|w| w[1] - w[0] == step), "evenly spaced");
        assert_eq!(degrees_e7(1 << 30), 900_000_000);
        assert_eq!(degrees_e7(-(1 << 29)), -450_000_000);
    }

    /// Columns cover equal distance: a climb in the first tenth of the distance is the first tenth of the columns
    /// even if the rider stood still for most of the samples.
    #[test]
    fn profile_follows_distance() {
        let mut s = Samples::new();
        for k in 0..100u32 {
            s.dist = k * 1000; // 10 m a sample, climbing 1 m each
            s.push(1000 + k as u16);
        }
        s.dist = 100_000;
        for _ in 0..400 {
            s.push(1100); // a long stop at the top
        }
        let mut out = [0; COLS];
        s.columns(&mut out);
        assert!(out[0] < 1010 && out[COLS - 1] == 1100, "{out:?}");
        assert!(out[COLS / 2] >= 1050 || out[COLS / 2] == 1100, "the climb fills the distance, not the time");
        let mut flat = Samples::new();
        for k in 0..100u16 {
            flat.push(1000 + k); // no distance at all
        }
        flat.columns(&mut out);
        assert!(out[0] < 1010 && out[COLS - 1] > 1090);
    }
}
