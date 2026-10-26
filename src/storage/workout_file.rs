//! Planned workouts as stock keeps them on the shared volume, read only: `\Workout\calendar\YYYYMMDD\<name>.fit`
//! (one folder per day, synced from Bryton's app), each a FIT workout file ([`parse`]) beside a `<name>.json` that
//! says the same in other words (not read: the .fit has everything the screens show). [`calendar`] lists today's
//! and the next days'. Stock's other copies (`calendar_temp\tempN`, `saved_temp`, `Workout_temp`) are left alone.
//!
//! A workout file is small (a few hundred bytes to a few KB), so it is read whole ([`FILE_MAX`]) and decoded from
//! the buffer: the streaming reader (fitread.rs) keeps numbers only, and a workout has a name.
//!
//! Format (FIT profile 21.218, `garmin_fit_sdk/profile.py` of github.com/garmin/fit-python-sdk, and the FIT SDK's
//! workout cookbook, github.com/garmin/fit-csharp-sdk `Cookbook/WorkoutEncode/Program.cs`, both read 2026-10-24):
//! - the FIT framing as in fitread.rs (header, definitions, data messages, compressed timestamp headers, developer
//!   fields skipped, the file CRC);
//! - `file_id` (0): field 0 `type`, 5 for a workout file;
//! - `workout` (26): 8 `wkt_name` (string), 6 `num_valid_steps`, 4 `sport`;
//! - `workout_step` (27): 254 `message_index`, 0 `wkt_step_name`, 1 `duration_type`, 2 `duration_value`,
//!   3 `target_type`, 4 `target_value`, 5 `custom_target_value_low`, 6 `custom_target_value_high`, 7 `intensity`;
//! - `duration_type` (wkt_step_duration): 0 time (`duration_value` in ms), 1 distance (cm), 5 open (until Lap),
//!   6 repeat_until_steps_cmplt (`duration_value` the step to repeat from, `target_value` the number of times, all
//!   of them: the cookbook's "5 x 800 m" is `repetitions: 5`), 28 repetition_time (ms);
//! - `target_type` (wkt_step_target): 0 speed, 1 heart_rate, 2 open, 3 cadence, 4 power, 7 to 10 power over 3 s,
//!   10 s, 30 s, a lap, 13 heart rate over a lap. With custom values: power (workout_power) 0 to 1000 is % FTP,
//!   above it watts + 1000; heart rate (workout_hr) 0 to 100 is % of max, above it bpm + 100; cadence in rpm.
//!   Without (low and high 0), `target_value` is a zone: taken here as this firmware's zones (ride.rs);
//! - `intensity`: 0 active, 1 rest, 2 warmup, 3 cooldown, 4 recovery, 5 interval, 6 other.
//!
//! **Bryton's own encoding** (read from the files its app synced to a unit, 2026-10-18): every step is time
//! based, repeats are already written out, `target_type` is 245 (not in the profile) and the custom values are
//! 100000 + % FTP, low and high (a ramp is written as its range). That the value is % FTP is inferred: in every
//! file, value - 100000 is the percentage the .json beside it gives for that step ("ramp 42-71%" for 100042 and
//! 100071, say); `wkt_name` is cut at 15 characters (the .json has the whole name).
//!
//! Nothing here panics or loops on any input (fuzzed in sim.rs `fuzz`): every index is checked, every loop is
//! bounded by the buffer or [`STEPS`].

use crate::fat::{self, Disk, Fat, File};
use crate::fit::crc;

/// Steps of a workout kept at most, repeats written out; a longer one is cut ([`Workout::cut`]).
pub const STEPS: usize = 96;
/// Steps as the file lists them (repeat steps included) taken at most.
const RAW: usize = 64;
/// Bytes of a workout file read at most: a file longer than this is not shown.
pub const FILE_MAX: usize = 8192;
/// Days of planned workouts listed at most.
pub const DAYS: usize = 8;
/// Characters of a workout's name kept.
pub const NAME: usize = 16;
/// Columns of a planned workout's profile ([`Planned::prof`]): 4 px each on the overview's card, 2 px in the list.
pub const PROF: usize = 56;
/// Height of a profile column at most, the scale of [`Planned::prof`]'s heights (a nibble: 1 to 15).
pub const PROF_H: u32 = 15;

/// The colours a profile column takes: the seven zones' ([`crate::ride::ZONE_COLORS`]), grey for an open step, white
/// for the rest ([`crate::workout::level`]).
fn shades() -> [u16; 9] {
    let z = crate::ride::ZONE_COLORS;
    [z[0], z[1], z[2], z[3], z[4], z[5], z[6], crate::ui::GREY, crate::ui::WHITE]
}

/// A profile column as one byte: the height (1 to [`PROF_H`], 0 for none) over the colour's number.
fn pack_bar((h, color): (u32, u16)) -> u8 {
    let shade = shades().iter().position(|&c| c == color).unwrap_or(8) as u8;
    (h.min(PROF_H) as u8) << 4 | shade
}

/// A profile column's height (of [`PROF_H`]; 0: none) and colour.
pub fn unpack_bar(b: u8) -> (u8, u16) {
    (b >> 4, shades().get(usize::from(b & 15)).copied().unwrap_or(crate::ui::WHITE))
}

/// What ends a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// A time, s.
    Time(u32),
    /// A distance, m.
    Distance(u32),
    /// The Lap key (and any end this firmware does not follow: heart rate, calories, power).
    Open,
}

/// What a target is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No target.
    Open,
    /// Power.
    Power,
    /// Heart rate.
    Hr,
    /// Cadence.
    Cadence,
}

/// A step's target: a range, in % (of FTP, of max heart rate) or in W / bpm / rpm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// What it is on.
    pub kind: Kind,
    /// Low end.
    pub lo: u16,
    /// High end.
    pub hi: u16,
    /// `lo` and `hi` are % of FTP (power) or of max heart rate.
    pub pct: bool,
}

impl Target {
    /// No target.
    pub const OPEN: Target = Target { kind: Kind::Open, lo: 0, hi: 0, pct: false };

    /// The range in W, bpm or rpm for this FTP and max heart rate; None for no target.
    ///
    /// ```
    /// use bryton_rustfw::storage::workout_file::{Kind, Target};
    /// let t = Target { kind: Kind::Power, lo: 88, hi: 93, pct: true };
    /// assert_eq!(t.range(250, 190), Some((220, 233)));
    /// assert_eq!(Target::OPEN.range(250, 190), None);
    /// ```
    pub fn range(self, ftp: u16, max_hr: u16) -> Option<(u16, u16)> {
        let of = |v: u16, base: u16| (u32::from(v) * u32::from(base) + 50) / 100;
        let to = |v: u32| v.min(u32::from(u16::MAX)) as u16;
        match (self.kind, self.pct) {
            (Kind::Open, _) => None,
            (Kind::Power, true) => Some((to(of(self.lo, ftp)), to(of(self.hi, ftp)))),
            (Kind::Hr, true) => Some((to(of(self.lo, max_hr)), to(of(self.hi, max_hr)))),
            _ => Some((self.lo, self.hi)),
        }
    }
}

/// One step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// What ends it.
    pub len: Length,
    /// Its target.
    pub target: Target,
    /// FIT intensity (0 active, 1 rest, 2 warmup, 3 cooldown, 4 recovery, 5 interval, 6 other).
    pub intensity: u8,
}

const OPEN_STEP: Step = Step { len: Length::Open, target: Target::OPEN, intensity: 0 };

/// A workout: its name and its steps, repeats written out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Workout {
    name: [u8; NAME],
    name_len: u8,
    steps: [Step; STEPS],
    n: u8,
    /// Steps were left out: more than [`STEPS`] once repeated, or more than the reader takes.
    pub cut: bool,
}

impl Default for Workout {
    fn default() -> Self {
        Self::new()
    }
}

impl Workout {
    /// No steps.
    pub const fn new() -> Self {
        Workout { name: [0; NAME], name_len: 0, steps: [OPEN_STEP; STEPS], n: 0, cut: false }
    }

    /// Its name in capitals (the pixel font's), other characters as '?'.
    pub fn name(&self) -> &str {
        let b = self.name.get(..usize::from(self.name_len)).unwrap_or(&[]);
        core::str::from_utf8(b).unwrap_or("")
    }

    /// Its steps, in order.
    pub fn steps(&self) -> &[Step] {
        self.steps.get(..usize::from(self.n)).unwrap_or(&[])
    }

    /// The time of its timed steps, s (open and distance steps count nothing).
    pub fn total_s(&self) -> u32 {
        self.steps().iter().map(|s| if let Length::Time(t) = s.len { t } else { 0 }).fold(0, u32::saturating_add)
    }

    /// A workout made in the firmware (the ramp test, ramp.rs): `name` cut to [`NAME`] characters, `steps` cut to
    /// [`STEPS`].
    pub fn built(name: &str, steps: &[Step]) -> Self {
        let mut w = Workout::new();
        for (d, b) in w.name.iter_mut().zip(name.bytes()) {
            *d = b;
            w.name_len += 1;
        }
        steps.iter().for_each(|&s| w.push(s));
        w
    }

    fn push(&mut self, s: Step) {
        match self.steps.get_mut(usize::from(self.n)) {
            Some(slot) => {
                *slot = s;
                self.n += 1;
            }
            None => self.cut = true,
        }
    }
}

/// Why a file is not a workout this reader takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a FIT header.
    Header,
    /// Shorter than its header says, or a message running past the data.
    Truncated,
    /// The file CRC is wrong.
    Crc,
    /// A data message on a local type with no definition.
    Undefined,
    /// Its file_id is not a workout's (type 5), or it has none.
    NotWorkout,
}

/// Fields of a definition kept: the ones read here.
const PICKED: usize = 9;

#[derive(Clone, Copy, Default)]
struct Def {
    global: u16,
    big: bool,
    size: usize,
    /// (field number, offset, size)
    picked: [(u8, u16, u8); PICKED],
    n: u8,
    defined: bool,
}

/// A step as the file has it (None: the field is absent or invalid).
#[derive(Clone, Copy, Default)]
struct Raw {
    dt: Option<u32>,
    dv: Option<u32>,
    tt: Option<u32>,
    tv: Option<u32>,
    lo: Option<u32>,
    hi: Option<u32>,
    intensity: Option<u32>,
}

/// The fields read, by message: file_id type; workout name; workout_step's.
fn wanted(global: u16, num: u8) -> bool {
    matches!((global, num), (0, 0) | (26, 8) | (27, 1..=7))
}

/// An unsigned value of 1, 2 or 4 bytes; None if it is invalid (all ones) or another size.
fn uint(b: &[u8], big: bool) -> Option<u32> {
    if !matches!(b.len(), 1 | 2 | 4) {
        return None;
    }
    let v = b.iter().enumerate().fold(0u32, |v, (i, &x)| match big {
        true => v << 8 | u32::from(x),
        false => v | u32::from(x) << (8 * i),
    });
    let invalid = if b.len() == 4 { u32::MAX } else { (1 << (8 * b.len())) - 1 };
    (v != invalid).then_some(v)
}

/// Decodes a whole workout file.
pub fn parse(file: &[u8]) -> Result<Workout, Error> {
    let hs = usize::from(*file.first().ok_or(Error::Header)?);
    let head = file.get(..hs).filter(|_| hs == 12 || hs == 14).ok_or(Error::Header)?;
    if head.get(8..12) != Some(b".FIT") {
        return Err(Error::Header);
    }
    let size = head.get(4..8).and_then(|b| uint(b, false)).ok_or(Error::Header)? as usize;
    let end = hs.checked_add(size).ok_or(Error::Truncated)?;
    let whole = file.get(..end.checked_add(2).ok_or(Error::Truncated)?).ok_or(Error::Truncated)?;
    if crc(0, whole) != 0 {
        return Err(Error::Crc);
    }
    let data = whole.get(..end).ok_or(Error::Truncated)?;
    let mut defs = [Def::default(); 16];
    let mut raw = [Raw::default(); RAW];
    let (mut n_raw, mut is_workout, mut w) = (0usize, false, Workout::new());
    let mut p = hs;
    let byte = |p: usize| data.get(p).copied().ok_or(Error::Truncated);
    while p < end {
        let h = byte(p)?;
        p += 1;
        let local = if h & 0x80 != 0 {
            usize::from(h >> 5 & 3) // compressed timestamp header: a data message
        } else if h & 0x40 != 0 {
            // definition: reserved, architecture, global (2), field count, then 3 bytes a field
            let big = byte(p + 1)? != 0;
            let g = [byte(p + 2)?, byte(p + 3)?];
            let mut d =
                Def { global: if big { u16::from_be_bytes(g) } else { u16::from_le_bytes(g) }, big, ..Def::default() };
            let fields = usize::from(byte(p + 4)?);
            p += 5;
            for _ in 0..fields {
                let (num, sz) = (byte(p)?, byte(p + 1)?);
                if wanted(d.global, num)
                    && let (Ok(off), Some(slot)) = (u16::try_from(d.size), d.picked.get_mut(usize::from(d.n)))
                {
                    *slot = (num, off, sz);
                    d.n += 1;
                }
                d.size += usize::from(sz);
                p += 3;
            }
            if h & 0x20 != 0 {
                let dev = usize::from(byte(p)?);
                p += 1;
                for _ in 0..dev {
                    d.size += usize::from(byte(p + 1)?); // (number, size, developer data index)
                    p += 3;
                }
            }
            if p > end {
                return Err(Error::Truncated);
            }
            if let Some(slot) = defs.get_mut(usize::from(h & 0xf)) {
                *slot = Def { defined: true, ..d };
            }
            continue;
        } else {
            usize::from(h & 0xf)
        };
        let def = defs.get(local).filter(|d| d.defined).ok_or(Error::Undefined)?;
        let msg = data.get(p..p + def.size).ok_or(Error::Truncated)?;
        p += def.size;
        let field = |num: u8| {
            let &(_, off, sz) = def.picked.iter().take(usize::from(def.n)).find(|f| f.0 == num)?;
            msg.get(usize::from(off)..usize::from(off) + usize::from(sz))
        };
        let value = |num: u8| field(num).and_then(|b| uint(b, def.big));
        match def.global {
            0 => is_workout |= value(0) == Some(5),
            26 => {
                let name = field(8).unwrap_or(&[]);
                let (mut k, mut i) = (0u8, 0);
                // ASCII kept (capitals, the font's), a UTF-8 sequence as one '?', up to the NUL
                while let (Some(&c), Some(slot)) = (name.get(i), w.name.get_mut(usize::from(k))) {
                    i += 1;
                    match c {
                        0 => break,
                        0x80..=0xbf => continue,
                        0x20..=0x7e => *slot = c.to_ascii_uppercase(),
                        _ => *slot = b'?',
                    }
                    k += 1;
                }
                w.name_len = k;
            }
            27 => match raw.get_mut(n_raw) {
                Some(r) => {
                    *r = Raw {
                        dt: value(1),
                        dv: value(2),
                        tt: value(3),
                        tv: value(4),
                        lo: value(5),
                        hi: value(6),
                        intensity: value(7),
                    };
                    n_raw += 1;
                }
                None => w.cut = true,
            },
            _ => {}
        }
    }
    if !is_workout {
        return Err(Error::NotWorkout);
    }
    expand(&mut w, raw.get(..n_raw).unwrap_or(&[]));
    Ok(w)
}

/// Writes out `raw` into `w`: each repeat step copies the steps written since the step it repeats from.
fn expand(w: &mut Workout, raw: &[Raw]) {
    let mut start = [0u8; RAW]; // where each file step begins in `w`
    for (i, r) in raw.iter().enumerate() {
        if let Some(s) = start.get_mut(i) {
            *s = w.n;
        }
        match r.dt {
            Some(6) => {
                // repeat_until_steps_cmplt: from step dv, tv times in all (done once already)
                let from = r.dv.and_then(|f| usize::try_from(f).ok()).filter(|&f| f < i);
                let (Some(from), Some(times)) = (from, r.tv) else { continue };
                let (s, e) = (usize::from(start.get(from).copied().unwrap_or(w.n)), usize::from(w.n));
                for _ in 1..times.min(STEPS as u32 + 1) {
                    for k in s..e {
                        let step = w.steps.get(k).copied().unwrap_or(OPEN_STEP);
                        w.push(step);
                    }
                    if w.cut {
                        return;
                    }
                }
            }
            // ponytail: repeat until a time, distance, heart rate or power: done once
            Some(7..=13) => {}
            _ => w.push(step(r)),
        }
    }
}

/// Power zones 1 to 7 as % FTP (ride.rs `power_zone`), heart rate zones 1 to 5 as % max (`hr_zone`): zone z is
/// from entry z - 1 to entry z.
const POWER_ZONES: [u16; 8] = zone_table([0, 0, 0, 0, 0, 0, 0, 200], &crate::ride::POWER_ZONE_PCT);
const HR_ZONES: [u16; 6] = zone_table([50, 0, 0, 0, 0, 100], &crate::ride::HR_ZONE_PCT);

/// `ends` with the zone edges `pct` written between its first and last entries.
const fn zone_table<const N: usize>(mut ends: [u16; N], pct: &[u32]) -> [u16; N] {
    let mut i = 0;
    while i < pct.len() {
        ends[i + 1] = pct[i] as u16;
        i += 1;
    }
    ends
}

fn step(r: &Raw) -> Step {
    let len = match (r.dt, r.dv) {
        (Some(0 | 28), Some(ms)) => Length::Time(ms.saturating_add(500) / 1000),
        (Some(1), Some(cm)) => Length::Distance(cm / 100),
        _ => Length::Open,
    };
    let u = |v: u32| v.min(u32::from(u16::MAX)) as u16;
    let custom = match (r.lo, r.hi) {
        (Some(lo), Some(hi)) if lo != 0 || hi != 0 => Some((lo.min(hi), lo.max(hi))),
        (Some(v), None) | (None, Some(v)) if v != 0 => Some((v, v)),
        _ => None,
    };
    let zone = |table: &[u16], kind| {
        let z = usize::try_from(r.tv?).ok()?;
        let (lo, hi) = (*table.get(z.checked_sub(1)?)?, *table.get(z)?);
        Some(Target { kind, lo, hi: hi.saturating_sub(1).max(lo), pct: true })
    };
    // % below the offset, else the unit plus the offset
    let offset = |kind, (lo, hi): (u32, u32), off: u32| {
        let pct = hi <= off;
        let v = |x: u32| u(if pct || x <= off { x } else { x - off });
        Target { kind, lo: v(lo), hi: v(hi), pct }
    };
    let target = match (r.tt, custom) {
        (Some(4 | 7..=10), Some(c)) => Some(offset(Kind::Power, c, 1000)),
        (Some(4 | 7..=10), None) => zone(&POWER_ZONES, Kind::Power),
        (Some(1 | 13), Some(c)) => Some(offset(Kind::Hr, c, 100)),
        (Some(1 | 13), None) => zone(&HR_ZONES, Kind::Hr),
        (Some(3), Some((lo, hi))) => Some(Target { kind: Kind::Cadence, lo: u(lo), hi: u(hi), pct: false }),
        // Bryton's: 100000 + % FTP (inferred, see the module doc)
        (Some(245), Some((lo, hi))) if lo >= 100_000 => {
            Some(Target { kind: Kind::Power, lo: u(lo - 100_000), hi: u(hi - 100_000), pct: true })
        }
        _ => None,
    };
    Step { len, target: target.unwrap_or(Target::OPEN), intensity: r.intensity.map_or(0, |i| i.min(255) as u8) }
}

/// A planned workout: its day, its file, its timed length and step count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    /// Its day (y, m, d).
    pub day: (u16, u8, u8),
    /// The .fit file.
    pub file: File,
    /// [`Workout::total_s`].
    pub total_s: u32,
    /// Steps, repeats written out.
    pub steps: u8,
    /// Its name, as [`Workout::name`].
    pub name: crate::text::Line<NAME>,
    /// Its profile, kept so the overview and the list draw without reading the card: a byte a column, its height
    /// (1 to [`PROF_H`]; 0 past the end) and its zone colour ([`unpack_bar`]), for the FTP and maximum heart rate
    /// it was read with ([`crate::workout::column`]). [`PROF`] bytes an entry, [`DAYS`] entries: 448 bytes.
    pub prof: [u8; PROF],
}

impl Planned {
    /// The entry for workout `w` of `file` on `day`, its profile coloured for these zones (FTP, maximum heart rate).
    pub fn of(day: (u16, u8, u8), file: File, w: &Workout, zones: (u16, u16)) -> Self {
        let mut name = crate::text::Line::new();
        name.push_str(w.name());
        let prof = core::array::from_fn(|k| {
            pack_bar(crate::workout::column(w.steps(), zones, (PROF as u32, PROF_H), None, k as u32))
        });
        Planned { day, file, total_s: w.total_s(), steps: w.n, name, prof }
    }
}

/// The workouts of today and the next days, earliest first ([`calendar`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    days: [Option<Planned>; DAYS],
}

impl Calendar {
    /// Earliest first.
    pub fn iter(&self) -> impl Iterator<Item = &Planned> {
        self.days.iter().map_while(Option::as_ref)
    }

    /// Workout `i`, earliest first.
    pub fn get(&self, i: usize) -> Option<&Planned> {
        self.days.get(i).and_then(Option::as_ref)
    }

    /// Adds `p` in day order (after those of its day), dropping the latest when full.
    pub(crate) fn add(&mut self, p: Planned) {
        let at = self.days.iter().position(|d| d.is_none_or(|d| d.day > p.day)).unwrap_or(DAYS);
        if at < DAYS {
            self.days[at..].rotate_right(1);
            self.days[at] = Some(p);
        }
    }
}

/// A day folder's date, `YYYYMMDD`.
///
/// ```
/// use bryton_rustfw::storage::workout_file::folder_day;
/// assert_eq!(folder_day(b"20261024"), Some((2026, 10, 24)));
/// assert_eq!([folder_day(b"2026102"), folder_day(b"20261324"), folder_day(b"2026102x")], [None; 3]);
/// ```
pub fn folder_day(name: &[u8]) -> Option<(u16, u8, u8)> {
    if name.len() != 8 || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = |r: core::ops::Range<usize>| {
        name.get(r).map_or(0, |d| d.iter().fold(0u16, |v, &c| v * 10 + u16::from(c - b'0')))
    };
    let (y, m, d) = (n(0..4), n(4..6) as u8, n(6..8) as u8);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// Reads the workout file `f` whole (at most [`FILE_MAX`] bytes) and decodes it; None if it is longer, cannot be
/// read or is no workout.
#[inline(never)] // its buffer lives in its own frame, only while it runs
pub fn load(fat: &mut Fat, d: &mut impl Disk, f: &File) -> Option<Workout> {
    if f.size as usize > FILE_MAX {
        return None; // not read at all
    }
    let mut buf = [0u8; FILE_MAX];
    let n = fat.read_at(d, f, 0, &mut buf).ok()?;
    parse(buf.get(..n)?).ok()
}

/// Workout files read at most for a calendar: twice [`DAYS`], so a few that are no workout still leave a full one.
pub const LOADS: usize = 2 * DAYS;

/// The workouts planned from `today` on (`\Workout\calendar\YYYYMMDD\*.fit`), the earliest [`DAYS`]; files that are
/// no workout are left out. Bounded whatever the card holds (review 2: every future file was read): the day
/// folders are listed first (32 bytes each), then only the earliest [`DAYS`] are looked into, earliest first, and at
/// most [`LOADS`] files of [`FILE_MAX`] are read: about 130 KB, some 50 ms on the unit.
pub fn calendar(d: &mut impl Disk, today: (u16, u8, u8), zones: (u16, u16)) -> Result<Calendar, fat::Error> {
    let mut fat = Fat::mount(d)?;
    let dir = fat.open(d, "Workout/calendar")?;
    let mut days = fat.cursor(dir)?;
    // the earliest DAYS day folders from today on, earliest first
    type Folder = Option<((u16, u8, u8), File)>;
    let mut first: [Folder; DAYS] = [None; DAYS];
    while let Some(e) = fat.list(d, &mut days)? {
        let Some(day) = folder_day(e.name()).filter(|&day| e.file.dir && day >= today) else { continue };
        let at = first.iter().position(|f| f.is_none_or(|(d, _)| d > day)).unwrap_or(DAYS);
        if let Some(tail) = first.get_mut(at..).filter(|t| !t.is_empty()) {
            tail.rotate_right(1);
            if let Some(slot) = tail.first_mut() {
                *slot = Some((day, e.file));
            }
        }
    }
    let (mut cal, mut loads) = (Calendar::default(), 0);
    for (day, folder) in first.into_iter().flatten() {
        let mut files = fat.cursor(folder)?;
        while let Some(f) = fat.list(d, &mut files)? {
            let name = f.name();
            let fit = name.len() > 4 && name.get(name.len() - 4..).is_some_and(|x| x.eq_ignore_ascii_case(b".fit"));
            if f.file.dir || !fit || f.file.size as usize > FILE_MAX || loads >= LOADS {
                continue;
            }
            loads += 1;
            if let Some(w) = load(&mut fat, d, &f.file) {
                cal.add(Planned::of(day, f.file, &w, zones));
            }
        }
    }
    Ok(cal)
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// A workout step for [`file`]: duration type and value, target type, value, low and high, intensity.
    pub type S = (u8, u32, u8, u32, u32, u32, u8);

    /// A FIT workout file as an app would write it: file_id (type `kind`), workout (`name`), the steps.
    pub fn file(kind: u8, name: &str, steps: &[S]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend([0x40, 0, 0, 0, 0, 1, 0, 1, 0, 0, kind]); // file_id: type
        let mut n = [0u8; 16];
        n[..name.len().min(15)].copy_from_slice(&name.as_bytes()[..name.len().min(15)]);
        d.extend([0x41, 0, 0, 26, 0, 2, 8, 16, 7, 6, 2, 0x84, 1]);
        d.extend(n);
        d.extend((steps.len() as u16).to_le_bytes());
        // workout_step, as Bryton's app lays it out, plus a developer field to skip
        d.extend([0x62, 0, 0, 27, 0, 9, 254, 2, 0x84, 1, 1, 0, 2, 4, 0x86, 3, 1, 0, 4, 4, 0x86, 5, 4, 0x86]);
        d.extend([6, 4, 0x86, 7, 1, 0, 0, 3, 7, 1, 0, 2, 0]);
        for (i, &(dt, dv, tt, tv, lo, hi, int)) in steps.iter().enumerate() {
            d.push(0x02);
            d.extend((i as u16).to_le_bytes());
            d.push(dt);
            d.extend(dv.to_le_bytes());
            d.push(tt);
            d.extend(tv.to_le_bytes());
            d.extend(lo.to_le_bytes());
            d.extend(hi.to_le_bytes());
            d.push(int);
            d.extend(b"ab\0");
            d.extend([9, 9]);
        }
        let mut f = std::vec![14, 0x20, 0x5c, 0x08];
        f.extend((d.len() as u32).to_le_bytes());
        f.extend(b".FIT");
        let h = crc(0, &f);
        f.extend(h.to_le_bytes());
        f.extend(d);
        let c = crc(0, &f);
        f.extend(c.to_le_bytes());
        f
    }

    const MIN: u32 = 60_000;

    /// A made-up interval session: warm-up ramp, 3 x (2 min at 105-110 %, 1 min easy) as a repeat, a lap-ended
    /// cool-down.
    pub fn intervals() -> Vec<u8> {
        file(
            5,
            "3x2 vo2 max",
            &[
                (0, 5 * MIN, 4, 0, 50, 75, 2),
                (0, 2 * MIN, 4, 0, 105, 110, 0),
                (0, MIN, 4, 0, 1150, 1150, 1),
                (6, 1, 2, 3, 0, 0, 0),
                (5, 0, 1, 0, 0, 0, 3),
            ],
        )
    }

    #[test]
    fn reads_steps_and_writes_out_repeats() {
        let w = parse(&intervals()).unwrap();
        assert_eq!(w.name(), "3X2 VO2 MAX");
        let s = w.steps();
        assert_eq!(s.len(), 1 + 3 * 2 + 1);
        assert_eq!(
            s[0],
            Step {
                len: Length::Time(300),
                target: Target { kind: Kind::Power, lo: 50, hi: 75, pct: true },
                intensity: 2
            }
        );
        for k in 0..3 {
            assert_eq!(s[1 + 2 * k].target, Target { kind: Kind::Power, lo: 105, hi: 110, pct: true });
            assert_eq!(s[2 + 2 * k].target, Target { kind: Kind::Power, lo: 150, hi: 150, pct: false }, "W + 1000");
            assert_eq!(s[2 + 2 * k].len, Length::Time(60));
        }
        assert_eq!((s[7].len, s[7].target, s[7].intensity), (Length::Open, Target::OPEN, 3));
        assert_eq!(w.total_s(), 300 + 3 * 180);
        assert!(!w.cut);
    }

    #[test]
    fn targets_by_kind() {
        let one = |st: S| parse(&file(5, "x", &[st])).unwrap().steps()[0];
        let t = |kind, lo, hi, pct| Target { kind, lo, hi, pct };
        assert_eq!(one((0, 1000, 1, 0, 235, 255, 0)).target, t(Kind::Hr, 135, 155, false), "bpm + 100");
        assert_eq!(one((0, 1000, 1, 0, 70, 80, 0)).target, t(Kind::Hr, 70, 80, true), "% max");
        assert_eq!(one((0, 1000, 1, 2, 0, 0, 0)).target, t(Kind::Hr, 60, 69, true), "zone 2");
        assert_eq!(one((0, 1000, 4, 4, 0, 0, 0)).target, t(Kind::Power, 91, 105, true), "zone 4");
        assert_eq!(one((0, 1000, 4, 9, 0, 0, 0)).target, Target::OPEN, "no zone 9");
        assert_eq!(one((0, 1000, 3, 0, 85, 95, 0)).target, t(Kind::Cadence, 85, 95, false));
        assert_eq!(one((0, 1000, 0, 0, 5556, 6944, 0)).target, Target::OPEN, "speed: not followed");
        assert_eq!(one((0, 1000, 245, 0, 100_045, 100_080, 2)).target, t(Kind::Power, 45, 80, true), "Bryton's");
        assert_eq!(one((1, 150_000, 2, 0, 0, 0, 0)).len, Length::Distance(1500));
        assert_eq!(one((2, 140, 2, 0, 0, 0, 0)).len, Length::Open, "heart rate below: ends on Lap");
        assert_eq!(t(Kind::Hr, 70, 80, true).range(250, 190), Some((133, 152)));
        assert_eq!(t(Kind::Cadence, 85, 95, false).range(250, 190), Some((85, 95)));
    }

    #[test]
    fn nested_repeats_and_the_cap() {
        // 2 x (A, 3 x (B, C))
        let a = (0, 1000, 2, 0, 0, 0, 0);
        let b = (0, 2000, 2, 0, 0, 0, 0);
        let w = parse(&file(5, "n", &[a, b, b, (6, 1, 2, 3, 0, 0, 0), (6, 0, 2, 2, 0, 0, 0)])).unwrap();
        let secs: Vec<_> = w.steps().iter().map(|s| s.len).collect();
        assert_eq!(secs.len(), 2 * (1 + 3 * 2));
        assert_eq!(w.total_s(), 2 * (1 + 3 * 4));
        // a repeat forwards, or of itself: ignored; a huge count: cut at STEPS
        let w = parse(&file(5, "n", &[a, (6, 5, 2, 3, 0, 0, 0), (6, 1, 2, 3, 0, 0, 0)])).unwrap();
        assert_eq!(w.steps().len(), 1);
        let w = parse(&file(5, "n", &[a, (6, 0, 2, 1_000_000, 0, 0, 0)])).unwrap();
        assert_eq!((w.steps().len(), w.cut), (STEPS, true));
    }

    #[test]
    fn refuses_what_is_not_a_whole_workout() {
        let f = intervals();
        assert_eq!(parse(&f[..f.len() - 1]), Err(Error::Truncated));
        let mut bad = f.clone();
        bad[30] ^= 1;
        assert_eq!(parse(&bad), Err(Error::Crc));
        assert_eq!(parse(&file(4, "ride", &[])), Err(Error::NotWorkout));
        assert_eq!(parse(b"hello"), Err(Error::Header));
        assert_eq!(parse(&[]), Err(Error::Header));
    }

    #[test]
    fn names_in_capitals_utf8_as_one_mark() {
        let w = parse(&file(5, "Z2 \u{2014} 3\u{d7}", &[])).unwrap();
        assert_eq!(w.name(), "Z2 ? 3?");
    }

    /// The synthetic card (tools/fat_image.py): today's and the next days' workouts, not the past one, not the
    /// broken one (bad CRC), the non-ASCII long name read through its file.
    #[test]
    fn calendar_of_the_synthetic_card() {
        let mut d = crate::fat::tests::Ram(crate::fat::tests::image(), 0);
        let c = calendar(&mut d, (2026, 10, 23), (200, 190)).unwrap();
        let got: Vec<_> = c.iter().map(|p| (p.day, p.total_s, p.steps)).collect();
        assert_eq!(got, [((2026, 10, 23), 20 + 2 * 45, 6), ((2026, 10, 25), 5400, 1)]);
        let mut fat = Fat::mount(&mut d).unwrap();
        let w = load(&mut fat, &mut d, &c.get(1).unwrap().file).unwrap();
        assert_eq!(w.name(), "LONG RIDE ? E", "15 bytes, the 3 of the dash as one mark");
        assert_eq!(w.steps()[0].target, Target { kind: Kind::Power, lo: 65, hi: 75, pct: true });
        assert_eq!(calendar(&mut d, (2026, 10, 26), (200, 190)).unwrap(), Calendar::default(), "none planned");
        assert!(calendar(&mut crate::fat::tests::Ram(Vec::new(), 0), (2026, 10, 23), (200, 190)).is_err(), "no volume");
    }

    #[test]
    fn calendar_keeps_the_earliest_days() {
        let f = File { cluster: 0, size: 0, dir: false };
        let mut c = Calendar::default();
        for d in [30u8, 25, 27, 25, 21, 22, 23, 24, 26, 28] {
            c.add(Planned {
                day: (2026, 10, d),
                file: f,
                total_s: u32::from(d),
                steps: 1,
                name: Default::default(),
                prof: [0; PROF],
            });
        }
        let days: Vec<_> = c.iter().map(|p| p.day.2).collect();
        assert_eq!(days, [21, 22, 23, 24, 25, 25, 26, 27]);
    }
}
