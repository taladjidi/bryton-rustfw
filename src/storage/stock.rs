//! What Bryton's stock firmware left on the shared volume (AGENTS.md, "Share the storage with stock"), read only:
//! - **its rides** (`\Activities\YYMMDDhhmmss.fit`, ghidra/STORAGE_AND_MAPS.md 2.1): [`Scan`] goes through them
//!   a bounded number of bytes per call, so the app spreads it over its passes and stays responsive; it gives the
//!   power curve of the last [`WINDOW_DAYS`] days, CP and W' ([`crate::power_curve`]) and the distance and time
//!   of this week, this year and all time ([`Summary`]), and the ride history's index ([`Scan::index`]);
//! - **its settings** (`\System\*.ini`): [`import`] maps what has an equivalent here ([`Imported`]).
//!
//! A ride counts only if its file is whole (header, CRC) and has its session: `tmp.fit` (stock's ride in
//! progress) and the GNSS logs (`.ubx`) do not have a ride's name and are never opened. The ride's day is its
//! name's (local time, as stock names them). Nothing is ever written: the FAT reader has no write path.

use crate::fat::{self, Cursor, Disk, Fat, File};
use crate::fitread::{Msg, Reader, Session};
use crate::history::{Acc, Index, Start};
use crate::ini::{get, number};
use crate::nrf::SensorId;
use crate::power_curve::{Curve, Rolling};
use crate::ui::Field;
use crate::{PAGES, PageCfg, ui};

/// The power curve covers the rides of this many days back.
pub const WINDOW_DAYS: u16 = 90;
/// Bytes read from the card per disk read: 16 sectors in one multi-block command.
pub const IO: usize = 8192;

/// Distance and time of some rides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Rides.
    pub rides: u16,
    /// Distance, m.
    pub dist_m: u32,
    /// Timer (moving) time, s.
    pub time_s: u32,
}

impl Totals {
    fn add(&mut self, s: &Session) {
        self.rides = self.rides.saturating_add(1);
        self.dist_m = self.dist_m.saturating_add(s.dist_cm.unwrap_or(0) / 100);
        self.time_s = self.time_s.saturating_add(s.timer_ms.unwrap_or(0) / 1000);
    }
}

/// What a scan found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// The best powers of the rides in the window.
    pub curve: Curve,
    /// This week (from Monday), this calendar year, all time.
    pub week: Totals,
    /// See `week`.
    pub year: Totals,
    /// See `week`.
    pub all: Totals,
    /// Rides in the window with power.
    pub with_power: u16,
    /// Files with a ride's name that were not whole rides (cut, bad CRC, no session, unreadable).
    pub skipped: u16,
    /// Bytes read.
    pub bytes: u32,
}

/// Where a scan is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Never started.
    Idle,
    /// Going; [`Scan::step`] again.
    Running,
    /// Every ride read.
    Done,
    /// The volume or the folder could not be read.
    Failed(fat::Error),
}

/// The day of a ride from its file name, `YYMMDDhhmmss.fit` (stock's, 2-digit year from 2000), as days since
/// 1970; None for any other name.
///
/// ```
/// use bryton_rustfw::stock::ride_day;
/// assert_eq!(ride_day(b"261023081500.FIT"), Some(20_749)); // 2026-10-23
/// assert_eq!([ride_day(b"tmp.fit"), ride_day(b"20261023081500.ubx"), ride_day(b"261323081500.fit")], [None; 3]);
/// ```
pub fn ride_day(name: &[u8]) -> Option<u16> {
    let (y, m, d, _, _) = ride_start(name)?;
    u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).ok()
}

/// The start of a ride from its file name, `YYMMDDhhmmss.fit` (local time); None for any other name.
///
/// ```
/// use bryton_rustfw::stock::ride_start;
/// assert_eq!(ride_start(b"261023081500.fit"), Some((2026, 10, 23, 8, 15)));
/// ```
pub fn ride_start(name: &[u8]) -> Option<Start> {
    let (digits, ext) = (name.get(..12)?, name.get(12..)?);
    if !ext.eq_ignore_ascii_case(b".fit") || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let two = |i: usize| digits.get(i..i + 2).map_or(0, |d| (d[0] - b'0') * 10 + (d[1] - b'0'));
    let (y, m, d) = (2000 + u16::from(two(0)), two(2), two(4));
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || two(6) > 23 || two(8) > 59 || two(10) > 59 {
        return None;
    }
    Some((y, m, d, two(6), two(8)))
}

/// A scan of stock's rides, a bounded amount per [`Scan::step`].
pub struct Scan {
    status: Status,
    fat: Option<Fat>,
    dir: Option<Cursor>,
    /// The ride being read: its file, how far, its day, its start.
    file: Option<(File, u32, u16, Start)>,
    reader: Reader,
    rolling: Rolling,
    /// Its session and totals so far.
    acc: Acc,
    /// Today, the first day of this week and of this year, days since 1970.
    today: u16,
    week: u16,
    year: u16,
    /// Files listed so far (shown as progress).
    pub listed: u16,
    /// What was found so far.
    pub out: Summary,
    /// The rides read whole, newest first (the ride history).
    pub index: Index,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    /// No scan yet.
    pub const fn new() -> Self {
        Scan {
            status: Status::Idle,
            fat: None,
            dir: None,
            file: None,
            reader: Reader::new(),
            rolling: Rolling::new(),
            acc: Acc::new(),
            today: 0,
            week: 0,
            year: 0,
            listed: 0,
            out: Summary {
                curve: Curve { watts: [0; crate::power_curve::N], day: [0; crate::power_curve::N] },
                week: Totals { rides: 0, dist_m: 0, time_s: 0 },
                year: Totals { rides: 0, dist_m: 0, time_s: 0 },
                all: Totals { rides: 0, dist_m: 0, time_s: 0 },
                with_power: 0,
                skipped: 0,
                bytes: 0,
            },
            index: Index::new(),
        }
    }

    /// Bytes of rides read since the scan started.
    pub fn read_bytes(&self) -> u32 {
        self.out.bytes
    }

    /// Where it is.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Starts again from the first ride, with today's date (y, m, d) for the window and the totals.
    pub fn start(&mut self, (y, m, d): (u16, u8, u8)) {
        let day = |y, m, d| u16::try_from(crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400).unwrap_or(0);
        self.today = day(y, m, d);
        self.week = self.today - (self.today + 3) % 7; // 1970-01-01 was a Thursday: (days + 3) % 7 is 0 on Monday
        self.year = day(y, 1, 1);
        (self.status, self.fat, self.dir, self.file, self.listed) = (Status::Running, None, None, None, 0);
        self.out = Summary::default();
        self.index.clear();
    }

    /// Goes on for about `budget` bytes read (a directory entry counts 32); false once it is done or failed.
    pub fn step(&mut self, d: &mut impl Disk, budget: u32) -> bool {
        let mut spent = 0u32;
        while self.status == Status::Running && spent < budget {
            if let Err(e) = self.advance(d, &mut spent) {
                self.status = Status::Failed(e);
            }
        }
        self.status == Status::Running
    }

    #[inline(never)] // its read buffer lives in its own frame, not in the caller's (the main loop's)
    fn advance(&mut self, d: &mut impl Disk, spent: &mut u32) -> Result<(), fat::Error> {
        let fat = match &mut self.fat {
            Some(f) => f,
            None => {
                *spent += fat::SECTOR as u32;
                self.fat.insert(Fat::mount(d)?)
            }
        };
        let dir = match &mut self.dir {
            Some(c) => c,
            None => {
                let f = fat.open(d, "Activities")?;
                *spent += fat::SECTOR as u32;
                self.dir.insert(fat.cursor(f)?)
            }
        };
        let Some((file, offset, day, start)) = self.file else {
            *spent += 32;
            match fat.list(d, dir)? {
                None => self.status = Status::Done,
                Some(e) => {
                    if let (false, Some(day), Some(start)) = (e.file.dir, ride_day(e.name()), ride_start(e.name())) {
                        self.listed = self.listed.saturating_add(1);
                        self.file = Some((e.file, 0, day, start));
                        (self.reader, self.acc) = (Reader::new(), Acc::new());
                        self.rolling.reset();
                    }
                }
            }
            return Ok(());
        };
        let mut io = [0u8; IO]; // on the stack for this call only: the scan stays small between passes
        let n = match fat.read_at(d, &file, offset, &mut io) {
            Ok(n) => n,
            Err(_) => {
                self.end_file(false); // one bad file: skipped, the scan goes on
                return Ok(());
            }
        };
        *spent += n as u32;
        self.out.bytes = self.out.bytes.saturating_add(n as u32);
        let in_window = self.today.saturating_sub(day) < WINDOW_DAYS;
        let (rolling, acc) = (&mut self.rolling, &mut self.acc);
        self.reader.push_slice(io.get(..n).unwrap_or(&[]), |m| {
            if let (Msg::Record(r), true) = (m, in_window) {
                rolling.sample(r.time, r.power);
            }
            acc.take(&m);
        });
        match self.reader.result() {
            Some(r) => self.end_file(r.is_ok()),
            None if n == 0 => self.end_file(false), // the file ended before the FIT data did
            None => self.file = Some((file, offset + n as u32, day, start)),
        }
        Ok(())
    }

    /// The ride read: counted if it was whole, with a session.
    fn end_file(&mut self, whole: bool) {
        let Some((file, _, day, start)) = self.file.take() else { return };
        let o = &mut self.out;
        match self.acc.session() {
            Some(s) if whole => {
                if let Some(r) = self.acc.ride(file, start) {
                    self.index.add(r);
                }
                o.all.add(&s);
                if day >= self.year {
                    o.year.add(&s);
                }
                if day >= self.week {
                    o.week.add(&s);
                }
                if self.rolling.best[0] > 0 {
                    o.with_power = o.with_power.saturating_add(1);
                    o.curve.merge(&self.rolling.best, day);
                }
            }
            _ => o.skipped = o.skipped.saturating_add(1),
        }
    }

    /// Reads stock's settings files ([`import`]), each into a buffer of [`IO`] bytes on the stack: `\System\DeviceData.ini` for the bike
    /// in use and the backlight, then `UserProfile.ini`, `Sensor.ini` and that bike's `Bike<n>Profile.ini`. A missing file gives
    /// nothing from it.
    #[inline(never)] // as `advance`
    pub fn import(&self, d: &mut impl Disk) -> Result<Imported, fat::Error> {
        let mut io = [0u8; IO];
        let mut fat = Fat::mount(d)?;
        let mut imp = Imported::default();
        let read = |fat: &mut Fat, d: &mut _, path: &str, io: &mut [u8; IO]| -> usize {
            fat.open(d, path).and_then(|f| fat.read_at(d, &f, 0, io)).unwrap_or(0)
        };
        let n = read(&mut fat, d, "System/DeviceData.ini", &mut io);
        let bike = imp.device(io.get(..n).unwrap_or(&[]));
        let n = read(&mut fat, d, "System/UserProfile.ini", &mut io);
        imp.user(io.get(..n).unwrap_or(&[]));
        let n = read(&mut fat, d, "System/Sensor.ini", &mut io);
        imp.sensors(io.get(..n).unwrap_or(&[]));
        let mut path = *b"System/Bike1Profile.ini";
        path[11] = b'1' + bike;
        let n = read(&mut fat, d, core::str::from_utf8(&path).unwrap_or(""), &mut io);
        imp.bike(io.get(..n).unwrap_or(&[]));
        Ok(imp)
    }
}

/// Stock's settings that have an equivalent here. None: not found, or out of the range this firmware takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// FTP, W (UserProfile.ini `[FTP] BaseVale`, stock's spelling).
    pub ftp: Option<u16>,
    /// Max heart rate, bpm (`[MHR] BaseVale`).
    pub max_hr: Option<u16>,
    /// Weight, 0.1 kg (`[UserProfile] Weight_kg`).
    pub weight_dg: Option<u16>,
    /// Critical power, W (`[UserProfile] CP`).
    pub cp: Option<u16>,
    /// W', J (`[UserProfile] Wbalance`: inferred to be W' in J from its values and stock's W' balance field).
    pub w_prime: Option<u32>,
    /// Wheel circumference, mm (Sensor.ini, a speed sensor's `Wheel`; unit inferred).
    pub wheel_mm: Option<u16>,
    /// ANT+ sensors per [`crate::sensors::Kind::ALL`] (Sensor.ini).
    pub sensors: [Option<SensorId>; 5],
    /// Ride pages (the bike's `[Page1]` to `[Page4]`, the enabled ones first).
    pub pages: [Option<PageCfg>; PAGES],
    /// Eat and drink reminders, minutes of ride time, and the heart rate alert, bpm (the bike's `[Reminder<n>]`).
    pub eat_min: Option<u16>,
    /// See `eat_min`.
    pub drink_min: Option<u16>,
    /// See `eat_min`.
    pub hr_alert: Option<u16>,
    /// The backlight follows the ambient light (DeviceData.ini `[DeviceSys] brightnessAuto=1`).
    pub light_auto: bool,
}

/// Stock's field ids (`GridN=` in Bike`<n>`Profile.ini) and the field here that shows the same value, or the
/// closest. The ids are the indexes of stock's field title table (GetDataFieldTitle 0xd0150ef8, English titles
/// read from its string blocks: ghidra/STORAGE_AND_MAPS.md 3.2; bryton-S510-mods docs/how-it-works.md "Data
/// fields" confirms 27, 28, 29). A ring or bar of a value maps to the value. 12 is "Non" in stock: the patched stock
/// firmware of bryton-S510-mods shows DFA a1 there (P9), so a page that has it was made on it (its W' balance,
/// P10, was field 180). Ids not here (altitude, zones, lap averages, pedal dynamics...) have no equivalent yet.
pub const STOCK_FIELDS: [(u8, Field); 52] = {
    use Field::*;
    [
        (0, Kcal),         // "Calories"
        (1, Temp),         // "Temp."
        (2, Speed),        // "Speed"
        (3, AvgSpeed),     // "Avg Speed"
        (4, MaxSpeed),     // "Max Speed"
        (5, Dist),         // "Distance"
        (7, Time),         // "Time" (of day, inferred: "Ride Time" and "Trip Time" are the timers)
        (8, Timer),        // "Ride Time"
        (12, Dfa),         // "Non"; P9's DFA a1
        (16, Ascent),      // "Alt. Gain"
        (18, Grade),       // "Gradient"
        (21, Hr),          // "Heart Rate"
        (22, AvgHr),       // "Avg HR"
        (23, MaxHr),       // "Max HR"
        (24, Cadence),     // "Cadence"
        (25, AvgCadence),  // "Avg CAD"
        (27, Power30s),    // "30s PW"
        (28, Power3s),     // "3s PW"
        (29, Power),       // "Power"
        (30, MaxPower),    // "Max Power"
        (31, AvgPower),    // "Avg Power"
        (44, Kcal),        // "Calories"
        (48, LapDist),     // "LapDistance"
        (50, LapTime),     // "LapTime"
        (58, TotalKm),     // "ODO"
        (65, Wkg),         // "w/kg" (3 s power here)
        (67, If),          // "IF"
        (68, Np),          // "NP"
        (69, Tss),         // "TSS"
        (70, Balance),     // "CurPB-LR" (pedal balance)
        (78, Power10s),    // "10s Power"
        (80, Kj),          // "Total Work"
        (92, ToGo),        // "To Dest."
        (95, Map),         // "Turn Info": the map in a data page (ghidra recovered_src/ui/page_switch.c)
        (96, Speed),       // "SPD Ring"
        (97, Speed),       // "SPD Bar"
        (98, Cadence),     // "CAD Ring"
        (99, Cadence),     // "CAD Bar"
        (100, Hr),         // "HR Ring"
        (101, Hr),         // "HR Bar"
        (102, Power),      // "PW Ring"
        (103, Power),      // "PW Bar"
        (104, Power3s),    // "3s PW Ring"
        (105, Power10s),   // "10s PW Ring"
        (106, Power30s),   // "30s PW Ring"
        (120, Vam),        // "VAM"
        (141, StepTarget), // "Target PW"
        (142, StepTarget), // "Target Cad"
        (143, StepTarget), // "Target HR"
        (144, StepTime),   // "Step Rem."
        (147, StepTime),   // "Step Rem."
        (180, Wbal),       // "W' Bal" (ghidra recovered_src/ui/data_fields.c)
    ]
};

fn stock_field(id: u32) -> Option<Field> {
    STOCK_FIELDS.iter().find(|(i, _)| u32::from(*i) == id).map(|&(_, f)| f)
}

/// `prefix`, `suffix`, then the decimal `n` (a section or key name such as `HRSensor1`, `Grid10`), in `buf`.
fn named<'a>(buf: &'a mut [u8; 16], prefix: &str, suffix: &str, n: u32) -> &'a str {
    let mut l = crate::text::Line::<16>::new();
    l.push_str(prefix);
    l.push_str(suffix);
    l.push_u32(n);
    let b = l.as_str().as_bytes();
    let len = b.len().min(16);
    buf[..len].copy_from_slice(&b[..len]);
    core::str::from_utf8(&buf[..len]).unwrap_or("")
}

impl Imported {
    /// From UserProfile.ini.
    pub fn user(&mut self, t: &[u8]) {
        // to u16 only once in range: `as` first would turn 65736 into 200 (the review, 2026-10-24)
        let in_range = |v: u32, r: core::ops::RangeInclusive<u16>| u16::try_from(v).ok().filter(|v| r.contains(v));
        let ftp = number(t, "FTP", "BaseVale", 0).and_then(|v| in_range(v, crate::menu::FTP_RANGE));
        self.ftp = ftp.map(|v| (v + 2) / 5 * 5); // the menu steps by 5
        self.max_hr = number(t, "MHR", "BaseVale", 0).and_then(|v| in_range(v, crate::menu::MAX_HR_RANGE));
        self.weight_dg =
            number(t, "UserProfile", "Weight_kg", 1).filter(|v| (200..=2500).contains(v)).map(|v| v as u16);
        self.cp = number(t, "UserProfile", "CP", 0).filter(|v| (50..=1000).contains(v)).map(|v| v as u16);
        self.w_prime = number(t, "UserProfile", "Wbalance", 0).filter(|v| (1000..=100_000).contains(v));
    }

    /// From Sensor.ini: per kind, the first active ANT sensor (`HWType=0`, inferred: the BLE ones carry a `Mac`)
    /// of the sections `HRSensor<n>`, `SPDSensor<n>`, `CADSensor<n>`, `COMSensor<n>` (combined speed and cadence,
    /// inferred from the name) and `PWRSensor<n>`; and a speed sensor's wheel.
    pub fn sensors(&mut self, t: &[u8]) {
        const PREFIX: [&str; 5] = ["HR", "SPD", "CAD", "COM", "PWR"];
        for (k, (prefix, slot)) in PREFIX.iter().zip(&mut self.sensors).enumerate() {
            for n in 1..=4u32 {
                let mut buf = [0u8; 16];
                let section = named(&mut buf, prefix, "Sensor", n);
                let id = number(t, section, "ID", 0).filter(|&id| id > 0 && id < 1 << 20);
                let ant = number(t, section, "HWType", 0) == Some(0) && number(t, section, "active", 0) == Some(1);
                if let (Some(id), true, None) = (id, ant, *slot) {
                    // a 20-bit device number: the top 4 bits go in the transmission type's upper nibble
                    *slot = Some(SensorId { device: id as u16, trans: ((id >> 16) as u8) << 4 });
                }
                let wheel = number(t, section, "Wheel", 0).filter(|w| (1000..=3000).contains(w));
                if (k == 1 || k == 3) && self.wheel_mm.is_none() {
                    self.wheel_mm = wheel.map(|w| w as u16);
                }
            }
        }
    }

    /// From DeviceData.ini: the backlight, and the bike in use (`[DeviceSys] ActBike`, 0 for Bike1Profile.ini:
    /// Cfg_SetActiveBike writes the 0-based index there, ghidra notes/agent_a1.md), returned.
    pub fn device(&mut self, t: &[u8]) -> u8 {
        // brightnessAuto: written with brightness and AutoBrnVal by Cfg_DevDataApplySettingById (agent_a2.md); 1 on,
        // inferred. The manual `brightness` is not taken: its scale is not checked.
        self.light_auto = number(t, "DeviceSys", "brightnessAuto", 0) == Some(1);
        number(t, "DeviceSys", "ActBike", 0).unwrap_or(0).min(2) as u8
    }

    /// From Bike`<n>`Profile.ini: the pages ([`Imported::pages`]) and the reminders ([`Imported::reminders`]).
    pub fn bike(&mut self, t: &[u8]) {
        self.pages(t);
        self.reminders(t);
    }

    /// `[Page1]` to `[Page4]`, stock's data pages (the others have a `name` and are lap, map, climb... pages),
    /// the enabled ones in order: `isEnabled` 1 or 2 (stock's FUN_d015f248 turns 2 into 1, and a data page shows
    /// when it is 1: ApplicationglobalSetting_IsPageAvailable 0xd01535e0), each with the fields of its
    /// `Grid<type>` line (`type`: the field count; Page_LoadBikeProfilePage reads one list per count, so the others
    /// are what the page would show with another count). The fields with an equivalent here, in stock's order and
    /// each once, at most 6, give the layout of that many fields; a page with none is left out.
    pub fn pages(&mut self, t: &[u8]) {
        let mut out = 0;
        for n in 1..=4u32 {
            let mut buf = [0u8; 16];
            let section = named(&mut buf, "Page", "", n);
            let count = number(t, section, "type", 0).unwrap_or(0);
            if number(t, section, "isEnabled", 0).unwrap_or(0) == 0 || get(t, section, "name").is_some() {
                continue;
            }
            let mut buf = [0u8; 16];
            let Some(list) = get(t, section, named(&mut buf, "Grid", "", count)) else { continue };
            let mut page = PageCfg { layout: 0, fields: [Field::Time; 6] };
            let mut k = 0;
            for id in list.split(|&b| b == b',').filter_map(|v| crate::ini::decimal(v, 0)) {
                let Some(f) = stock_field(id) else { continue };
                if page.fields.get(..k).is_some_and(|had| had.contains(&f)) {
                    continue; // a ring and the number of one value
                }
                if let Some(slot) = page.fields.get_mut(k) {
                    *slot = f;
                    k += 1;
                }
            }
            page.layout = ui::LAYOUT_ORDER.iter().copied().find(|&l| ui::field_count(l) == k).unwrap_or(0);
            if let (true, Some(slot)) = (page.layout != 0, self.pages.get_mut(out)) {
                *slot = Some(page);
                out += 1;
            }
        }
    }

    /// `[Reminder1]` to `[Reminder6]` (ghidra recovered_src/ride/reminders.c): the ones on (`status=1`) that have
    /// an equivalent here. Every so many minutes of trip time (`WhichReminder=0`, `ReminderFreq=2` repeats,
    /// `Timer` in s): `ReminderType` 0 Drink, 1 Eat; heart rate over (`WhichReminder=5`, `HR`). Rounded to the
    /// menu's steps of 5.
    pub fn reminders(&mut self, t: &[u8]) {
        let fives = |v: u32, r: core::ops::RangeInclusive<u16>| {
            u16::try_from(v.saturating_add(2) / 5 * 5).ok().map(|v| v.clamp(*r.start(), *r.end()))
        };
        for n in 1..=6u32 {
            let mut buf = [0u8; 16];
            let section = named(&mut buf, "Reminder", "", n);
            let key = |k| number(t, section, k, 0);
            if key("status") != Some(1) {
                continue;
            }
            match (key("WhichReminder"), key("ReminderFreq"), key("ReminderType")) {
                (Some(0), Some(2), Some(ty @ 0..=1)) => {
                    let min = key("Timer").map(|s| s / 60).filter(|&m| m > 0).and_then(|m| fives(m, 5..=120));
                    let slot = if ty == 0 { &mut self.drink_min } else { &mut self.eat_min };
                    *slot = slot.or(min);
                }
                (Some(5), _, _) => {
                    let hr = key("HR").filter(|&h| h > 0).and_then(|h| fives(h, 100..=230));
                    self.hr_alert = self.hr_alert.or(hr);
                }
                _ => {}
            }
        }
    }
}

/// Stock's settings from the three files' text ([`Imported::user`], [`Imported::sensors`], [`Imported::bike`]).
pub fn import(user: &[u8], sensor: &[u8], bike: &[u8]) -> Imported {
    let mut i = Imported::default();
    i.user(user);
    i.sensors(sensor);
    i.bike(bike);
    i
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::fat::tests::{Ram, image};
    use crate::power_curve::fit_cp;

    const TODAY: (u16, u8, u8) = (2026, 10, 23);

    fn day(y: u16, m: u8, d: u8) -> u16 {
        (crate::epoch_s((y, m, d, 0, 0, 0)) / 86_400) as u16
    }

    /// The made-up rides of tools/fat_image.py, against values worked out by hand there.
    #[test]
    fn scans_the_synthetic_rides() {
        let mut d = Ram(image(), 0);
        let mut s = Scan::new();
        assert_eq!(s.status(), Status::Idle);
        s.start(TODAY);
        let mut passes = 0;
        while s.step(&mut d, 4096) {
            passes += 1;
            assert!(passes < 1000);
        }
        assert_eq!(s.status(), Status::Done);
        assert!(passes > 5, "spread over passes: {passes}");
        let o = s.out;
        assert_eq!((s.listed, o.skipped, o.with_power), (6, 2, 2), "6 ride names, a bad CRC and a cut file");
        assert_eq!(o.all, Totals { rides: 4, dist_m: 37_400, time_s: 4200 });
        assert_eq!(o.year, Totals { rides: 3, dist_m: 31_400, time_s: 3600 });
        assert_eq!(o.week, Totals { rides: 2, dist_m: 19_400, time_s: 2400 });
        let (a, b) = (day(2026, 10, 20), day(2026, 10, 1));
        assert_eq!(o.curve.watts, [400, 400, 400, 400, 400, 400, 350, 310, 300, 275, 265, 260, 0]);
        assert_eq!(o.curve.day, [a, a, a, a, a, a, a, a, b, a, a, a, 0], "480 s from the ride with the stop");
        assert!(fit_cp(&o.curve).is_some());
        // the same in one go
        let mut one = Scan::new();
        one.start(TODAY);
        while one.step(&mut d, u32::MAX) {}
        assert_eq!(one.out, o);
    }

    #[test]
    fn a_volume_without_rides() {
        let mut s = Scan::new();
        s.start(TODAY);
        while s.step(&mut Ram(std::vec::Vec::new(), 0), 1 << 20) {}
        assert_eq!(s.status(), Status::Failed(fat::Error::NotFat32));
    }

    /// Made-up settings files in stock's layout (the keys and sections of a unit's files, none of its values).
    #[test]
    fn imports_stock_settings() {
        let user = b"[Info]\nVer=1000\n\n[UserProfile]\nWbalance=21000\nCP=255\nWeight_kg=72.50000\n\n[FTP]\n\
            Zone1L=0\nBaseVale=263\n\n[MHR]\nZone7H=65535\nBaseVale=188\n\n[LTHR]\nBaseVale=170\n\0";
        let sensor = b"[Sensor]\r\nHR_Num=2\r\n[HRSensor1]\r\nID=0\r\nHWType=0\r\nactive=1\r\n[HRSensor2]\r\n\
            ID=1234\r\nHWType=0\r\nactive=1\r\n[SPDSensor1]\r\nWheel=2105.000000\r\nID=70000\r\nHWType=0\r\n\
            active=1\r\n[PWRSensor1]\r\nID=4321\r\nHWType=1\r\nactive=1\r\n";
        // Page1 shown (isEnabled 2), its Grid3 the one shown; Page2 off; Page3 with more fields than a layout here,
        // a ring and its value, and ids without an equivalent (14 altitude, 32 zone); Page4 only those; Page5 a lap
        // page. Reminders: drink every 45 min (on), eat (off), HR over 172, a clock alarm (no equivalent).
        let bike = b"[Info]\nVer=6001\n[Page1]\nGrid2=96,5\nGrid3=2,21,7\ntype=3\nisEnabled=2\n\n[Page2]\n\
            Grid4=3,18,7,0\ntype=4\nisEnabled=0\n[Page3]\nGrid10=103,29,14,12,180,32,13,68,8,5\ntype=10\n\
            gridtype=1\nisEnabled=1\n[Page4]\nGrid2=14,32\ntype=2\nisEnabled=1\n[Page5]\nname=1012\ntype=6\n\
            isEnabled=1\nGrid6=2,2,2,2,2,2\n[Reminder1]\nTimer=2700\nWhichReminder=0\nReminderFreq=2\n\
            ReminderType=0\nstatus=1\n[Reminder2]\nTimer=3600\nWhichReminder=0\nReminderFreq=2\nReminderType=1\n\
            status=0\n[Reminder3]\nHR=172\nWhichReminder=5\nReminderFreq=1\nReminderType=5\nstatus=1\n\
            [Reminder4]\nAlerm_Hour=7\nWhichReminder=1\nReminderFreq=1\nReminderType=3\nstatus=1\n";
        let i = import(user, sensor, bike);
        let got = (i.ftp, i.max_hr, i.weight_dg, i.cp, i.w_prime);
        assert_eq!(got, (Some(265), Some(188), Some(725), Some(255), Some(21_000)));
        assert_eq!(i.wheel_mm, Some(2105));
        assert_eq!(i.sensors[0], Some(SensorId { device: 1234, trans: 0 }), "the first with an ID");
        assert_eq!(i.sensors[1], Some(SensorId { device: 4464, trans: 0x10 }), "20-bit device number 70000");
        assert_eq!(i.sensors[4], None, "a BLE power meter");
        use Field::*;
        let p0 = i.pages[0].unwrap();
        assert_eq!((ui::field_count(p0.layout), &p0.fields[..3]), (3, &[Speed, Hr, Time][..]));
        let p1 = i.pages[1].unwrap();
        let want = [Power, Dfa, Wbal, Np, Timer, Dist];
        assert_eq!((ui::field_count(p1.layout), &p1.fields[..]), (6, &want[..]), "each once, the first 6 mapped");
        assert_eq!(i.pages[2], None, "page 4 has nothing mapped; page 5 is not a data page");
        assert_eq!((i.drink_min, i.eat_min, i.hr_alert), (Some(45), None, Some(170)), "HR to the menu's 5s");
        let mut d = Imported::default();
        assert_eq!((d.device(b"[DeviceSys]\nbrightnessAuto=1\nActBike=2\n"), d.light_auto), (2, true));
        assert_eq!((d.device(b"[DeviceSys]\nbrightnessAuto=0\n"), d.light_auto), (0, false));
        assert_eq!(import(b"", b"\0\0", b"[Page1"), Imported::default());
    }

    /// The owner's drive backups, when they are there (never in the repository): every ride read, aggregates
    /// printed only. `cargo test-host stock_backups -- --ignored --nocapture`, with BRYTON_DRIVE pointing at a
    /// copy of the drive's root.
    #[test]
    #[ignore]
    fn stock_backups() {
        let Ok(root) = std::env::var("BRYTON_DRIVE") else {
            std::println!("BRYTON_DRIVE not set: skipped");
            return;
        };
        let (mut rides, mut power, mut bad, mut bytes, mut same_max) = (0, 0, 0, 0, 0);
        for e in std::fs::read_dir(std::format!("{root}/Activities")).unwrap().flatten() {
            let name = e.file_name().into_string().unwrap();
            if ride_day(name.as_bytes()).is_none() {
                continue;
            }
            let f = std::fs::read(e.path()).unwrap();
            bytes += f.len();
            let (mut r, mut roll, mut session) = (Reader::new(), Rolling::new(), None);
            for &b in &f {
                match r.push(b) {
                    Some(Msg::Record(x)) => roll.sample(x.time, x.power),
                    Some(Msg::Session(s)) => session = Some(s),
                    _ => {}
                }
            }
            match (r.result(), session) {
                (Some(Ok(())), Some(s)) => {
                    rides += 1;
                    power += usize::from(roll.best[0] > 0);
                    // the 1 s best against the session's own maximum: a check of the reader
                    same_max += usize::from(roll.best[0] > 0 && s.max_power == Some(roll.best[0]));
                }
                _ => bad += 1,
            }
        }
        std::println!(
            "{rides} rides ({power} with power, {same_max} whose 1 s best is the session's maximum), {bad} not whole, {} MB",
            bytes >> 20
        );
        let sys = |f: &str| std::fs::read(std::format!("{root}/System/{f}")).unwrap_or_default();
        let mut i = Imported::default();
        let bike = std::format!("Bike{}Profile.ini", 1 + i.device(&sys("DeviceData.ini")));
        let bike = sys(&bike);
        i.user(&sys("UserProfile.ini"));
        i.sensors(&sys("Sensor.ini"));
        i.bike(&bike);
        let found = [i.ftp.is_some(), i.max_hr.is_some(), i.weight_dg.is_some(), i.cp.is_some(), i.w_prime.is_some()];
        let sensors = i.sensors.iter().filter(|s| s.is_some()).count();
        let pages = i.pages.iter().filter(|p| p.is_some()).count();
        std::println!("settings found (ftp, max hr, weight, cp, w'): {found:?}; {sensors} sensors, {pages} pages");
        let alerts = [i.eat_min.is_some(), i.drink_min.is_some(), i.hr_alert.is_some(), i.light_auto];
        std::println!("reminders and backlight found (eat, drink, hr, auto light): {alerts:?}");
        // how much of the data pages shown maps: counts and stock's generic field ids only, never the owner's values
        let (mut shown, mut mapped, mut unmapped) = (0, 0, std::collections::BTreeSet::new());
        for n in 1..=4 {
            let section = std::format!("Page{n}");
            let count = number(&bike, &section, "type", 0).unwrap_or(0);
            if number(&bike, &section, "isEnabled", 0).unwrap_or(0) == 0 || get(&bike, &section, "name").is_some() {
                continue;
            }
            let list = get(&bike, &section, &std::format!("Grid{count}")).unwrap_or(&[]);
            for id in list.split(|&b| b == b',').filter_map(|v| crate::ini::decimal(v, 0)) {
                shown += 1;
                match stock_field(id) {
                    Some(_) => mapped += 1,
                    None => _ = unmapped.insert(id),
                }
            }
        }
        std::println!(
            "data pages shown: {mapped} of {shown} fields have an equivalent; stock ids without: {unmapped:?}"
        );
        assert!(rides > 0);
    }
}
