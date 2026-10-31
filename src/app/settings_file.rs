//! `\Rustfw\settings.bin`: where the [`Settings`] are kept since the backup registers ran out (roadmap T0). Read
//! when the pages' file is (never at boot), written between rides only, the way `\Rustfw\pages.bin` is
//! (app/editor.rs: the recorder is the card's one writer while a ride is open), in this firmware's own folder (stock
//! never looks there).
//!
//! **The format.** 1024 bytes: two copies of one 512-byte sector each ([`SettingsCopy`]). The newest whole copy (a
//! good magic, version and CRC; the higher sequence number) wins, and a write goes over the other one, so a write
//! cut at any point leaves the newest copy before it whole. A copy, little endian:
//!
//! ```text
//! 0    "RSET"          magic
//! 4    version (u16)   1; a copy of another version is not read (a layout that cannot be appended to gets a new one)
//! 6    length (u16)    bytes of settings at 28 (172 now, with STEEP DESCENT at 171, GNSS SYSTEMS at 170, LEVEL STEP and SLOPE SCALE at 168, 169, MAP at 167, TIME ZONE and SUMMER TIME at 165, 166, CLIMBS, CLIMB PAGE and DATE FORMAT at 152..154, SUNSET ALERT at 155, QUICK STATUS at 158 and its six fields at 159..164, the Di2 teeth at 156 and 157, START REMINDER and READY CHECK at 150..152, ZONES BY, the custom flag, ten zone bounds (u16 each), SEX, BIRTH YEAR (u16) and HEIGHT (a byte) and MAX HR FROM AGE (a byte) at 123..150; 123 before, with SOUND, UNITS, TEMPERATURE, AUTO PAUSE at 99..102 and the trips and odometer at 103..123; 99 before: 31 in the first build, THEME's 12 bytes, POWER MATCH's byte, the 11 alert limits, then LTHR's 2 bytes, POWER SMOOTH's byte, then AUTO's byte and the DAY and NIGHT THEME names, then AUTO LAP's mode, km and min bytes, then LIVE SEGMENTS (data 97) and TARGET (98) appended; at most 480); a later build appends its settings
//!                      and raises it: a reader takes those it knows, the defaults for those past the length
//! 8    seq (u32)       higher is newer
//! 12   words (4 x u32) the settings' backup registers as written with this copy (which wins at a start, below)
//! 28   settings        in [`NUMBERS`]' order: the wheel (mm), then the numbers (u16 each); then the choices, a
//!                      byte each ([`SettingsCopy::to_sector`]), then THEME's name (12 bytes, NUL padded; a file
//!                      only, no register), then POWER MATCH (a byte, file only), then the 11 alert limits ([`alerts::LIMITS`], u16 each,
//!                      0 = OFF; a file without them reads OFF); a value out of its range reads as its default
//!                      then LTHR (u16), POWER SMOOTH (a byte), AUTO (a byte, 1 on) and DAY THEME and NIGHT THEME (12 bytes
//!                      each, as THEME's); a file without them reads AUTO off, OUTDOOR by day and DARK by night
//! 508  CRC-32 of 0..508
//! ```
//!
//! **Which wins at a start.** The registers give the settings at once (settings.rs: which stay, and why); the file is
//! read a few seconds later, then:
//! - the registers held none of this firmware's settings at the start (a flat backup battery; the first start's
//!   import of stock's then put them there): the file's, and the registers written from it;
//! - else, register by register ([`Settings::with_registers`]): one that differs from the word the newest copy was
//!   written with (a change made while the file had to wait, during a ride or with the drive lent, then a reset or a
//!   switch-off; one made before the file was read; or another build's) gives the fields it carries, and the file
//!   is written again with them; every other field is the file's, including those a changed register does not carry
//!   (an earlier build's tag, or register 18 rewritten by a build without AUTO OFF);
//! - the registers are then written from the result where they differ from it (by a build that may write the card).
//!
//! A read that fails for any reason but "no file" is tried again: it never counts as no file, which would write a
//! first copy beside a newer one.
//!
//! No file (the first start of a build with it), or no whole copy: the registers' settings are written as its first
//! copy, once (the migration). A build that may not write the card reads the file but never writes it.

use crate::text::Face;
use crate::*;
use menu::Setting;

/// The file's name, in this firmware's folder ([`crate::recorder::DIR`]).
pub const SETTINGS_FILE: &str = "settings.bin";

const MAGIC: &[u8; 4] = b"RSET";
const VERSION: u16 = 1;
const WORDS_AT: usize = 12;
const DATA_AT: usize = 28;
const CRC_AT: usize = 508;

/// The numbers kept, in the file's order (after the wheel): never reordered, a new one appended at the end of the
/// layout (after the choices).
pub const NUMBERS: [Setting; 8] = [
    Setting::MaxHr,
    Setting::Ftp,
    Setting::Weight,
    Setting::BikeWeight,
    Setting::WPrime,
    Setting::Eat,
    Setting::Drink,
    Setting::HrAlert,
];

/// BIKE > PROFILE as its byte is the index.
const PROFILES: [Option<ride::Profile>; 7] = [
    None,
    Some(ride::Profile::Road),
    Some(ride::Profile::Indoor),
    Some(ride::Profile::Mtb),
    Some(ride::Profile::Gravel),
    Some(ride::Profile::Cx),
    Some(ride::Profile::Commute),
];

/// GPS POWER as its byte is the index.
const GPS_POWERS: [GpsPower; 3] = [GpsPower::Auto, GpsPower::Full, GpsPower::Saver];

/// One copy of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsCopy {
    /// Higher is newer.
    pub seq: u32,
    /// The registers as written with it ([`Settings::register_words`]).
    pub words: [u32; 4],
    /// The settings (`alt_m` is not kept: the default here).
    pub settings: Settings,
}

impl SettingsCopy {
    /// The sector (module doc).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, SettingsCopy};
    /// let s = Settings { ftp: 265, face: bryton_rustfw::text::Face::Barlow, ..Settings::from_word(0) };
    /// let c = SettingsCopy { seq: 3, words: s.register_words(), settings: s };
    /// let mut sector = c.to_sector();
    /// assert_eq!(SettingsCopy::from_sector(&sector), Some(c));
    /// sector[40] ^= 1;
    /// assert_eq!(SettingsCopy::from_sector(&sector), None, "damaged");
    /// ```
    pub fn to_sector(&self) -> [u8; 512] {
        let s = self.settings;
        let mut out = [0u8; 512];
        let mut data = [0u8; 192];
        let mut n = 0;
        let mut put = |bytes: &[u8]| {
            for &b in bytes {
                if let Some(d) = data.get_mut(n) {
                    *d = b;
                    n += 1;
                }
            }
        };
        put(&s.wheel_mm.to_le_bytes());
        for k in NUMBERS {
            let mut t = s;
            put(&t.number_mut(k).map_or(0, |v| *v).to_le_bytes());
        }
        let [r0, r1, r2, r3] = s.remote.map(|r| r as u8);
        let profile = PROFILES.iter().position(|&p| p == s.profile).unwrap_or(0) as u8;
        let gps = GPS_POWERS.iter().position(|&g| g == s.gps_power).unwrap_or(0) as u8;
        let (light, off, auto, avg) = (s.light_level as u8, s.light_timeout as u8, s.auto_off as u8, s.wkt_avg as u8);
        put(&[light, off, auto, avg, r0, r1, r2, r3]);
        put(&[u8::from(s.touch), profile, u8::from(s.ahead), s.face as u8, gps]);
        put(&s.theme);
        put(&[u8::from(s.power_match)]);
        for v in s.limits {
            put(&v.to_le_bytes());
        }
        put(&s.lthr.to_le_bytes());
        put(&[s.power_smooth as u8]);
        put(&[u8::from(s.auto_theme)]);
        put(&s.day_theme);
        put(&s.night_theme);
        put(&[s.auto_lap as u8, s.lap_km as u8, s.lap_min as u8]);
        put(&[u8::from(s.live_segments), s.seg_target as u8]);
        put(&[u8::from(s.sound), u8::from(s.imperial), u8::from(s.temp_f), s.auto_pause as u8]);
        for v in [s.trip_m[0], s.trip_s[0], s.trip_m[1], s.trip_s[1], s.odo_m] {
            put(&v.to_le_bytes());
        }
        put(&[u8::from(s.zones_lthr), u8::from(s.zone_custom)]);
        for v in s.zone_up {
            put(&v.to_le_bytes());
        }
        put(&[u8::from(s.female)]);
        put(&s.birth_year.to_le_bytes());
        put(&[s.height_cm as u8, u8::from(s.max_hr_age)]);
        put(&[u8::from(s.start_reminder), u8::from(s.ready_check)]);
        put(&[u8::from(s.climbs_medium), u8::from(s.climb_page), s.date_fmt as u8]);
        put(&[s.sunset_alert as u8]);
        put(&[s.chainrings as u8, s.cassette as u8]);
        put(&[u8::from(s.quick_status)]);
        put(&s.quick_page.fields.map(|f| f as u8)[..6]);
        put(&[s.tz as u8, s.dst as u8]);
        put(&[u8::from(s.map_up)]);
        put(&[s.level_step as u8, s.slope_scale as u8]);
        put(&[s.gnss_systems as u8]);
        put(&[s.steep_descent as u8]);
        out[..4].copy_from_slice(MAGIC);
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&(n as u16).to_le_bytes());
        out[8..12].copy_from_slice(&self.seq.to_le_bytes());
        for (o, w) in out[WORDS_AT..DATA_AT].chunks_exact_mut(4).zip(self.words) {
            o.copy_from_slice(&w.to_le_bytes());
        }
        out[DATA_AT..DATA_AT + n].copy_from_slice(&data[..n]);
        let crc = inflate::crc32(out[..CRC_AT].iter().copied());
        out[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// From a sector [`SettingsCopy::to_sector`] wrote; None if it is not one (never written, damaged, another
    /// version). A short one (an earlier build's) gives the defaults for the settings past its end, one out of its
    /// range the default for that setting.
    pub fn from_sector(sec: &[u8; 512]) -> Option<SettingsCopy> {
        let le16 = |at: usize| Some(u16::from_le_bytes([*sec.get(at)?, *sec.get(at + 1)?]));
        let le32 = |at: usize| Some(u32::from_le_bytes(sec.get(at..at + 4)?.try_into().ok()?));
        let len = usize::from(le16(6)?);
        if sec.get(..4)? != MAGIC
            || le16(4)? != VERSION
            || le32(CRC_AT)? != inflate::crc32(sec[..CRC_AT].iter().copied())
        {
            return None;
        }
        let mut data = sec.get(DATA_AT..CRC_AT)?.get(..len)?.iter().copied();
        let d = Settings::from_word(0);
        let mut s = d;
        let mut u16_next = || Some(u16::from_le_bytes([data.next()?, data.next()?]));
        if let Some(mm) = u16_next().filter(|mm| WHEELS.contains(mm)) {
            s.wheel_mm = mm;
        }
        for k in NUMBERS {
            if let (Some(v), Some((range, _)), Some(slot)) = (u16_next(), k.number(), s.number_mut(k))
                && range.contains(&v)
            {
                *slot = v;
            }
        }
        // the choices: a byte each, an index below `n`
        let mut byte = |n: usize| data.next().map(usize::from).filter(|&v| v < n);
        s.light_level = byte(LIGHT_LEVELS.len() + 1).unwrap_or(d.light_level);
        s.light_timeout = byte(LIGHT_TIMEOUTS_S.len()).unwrap_or(d.light_timeout);
        s.auto_off = byte(AUTO_OFF_MIN.len()).unwrap_or(d.auto_off);
        s.wkt_avg = byte(WKT_AVGS.len()).unwrap_or(d.wkt_avg);
        for (r, def) in s.remote.iter_mut().zip(d.remote) {
            *r = byte(8).map_or(def, |v| menu::Remote::from_bits(v as u32));
        }
        s.touch = byte(2).map_or(d.touch, |v| v == 1);
        s.profile = byte(PROFILES.len()).and_then(|v| PROFILES.get(v).copied()).unwrap_or(d.profile);
        s.ahead = byte(2).map_or(d.ahead, |v| v == 1);
        s.face = byte(Face::ALL.len()).and_then(|v| Face::ALL.get(v).copied()).unwrap_or(d.face);
        s.gps_power = byte(GPS_POWERS.len()).and_then(|v| GPS_POWERS.get(v).copied()).unwrap_or(d.gps_power);
        // THEME (appended after the first build's 31 bytes): a name of letters, digits, - and _, else DARK
        let mut name = [0u8; 12];
        for n in name.iter_mut() {
            *n = data.next().unwrap_or(0);
        }
        if theme_name_ok(&name) {
            s.theme = normal(name);
        }
        // POWER MATCH (appended after THEME): a byte, 0 or 1; a file without it reads ON
        s.power_match = data.next().map_or(d.power_match, |v| if v < 2 { v == 1 } else { d.power_match });
        // the alert limits (appended after POWER MATCH): an older file ends before them, they read OFF
        for n in 0..alerts::LIMITS {
            let k = Setting::Limit(n as u8);
            if let (Some(v), Some((range, _)), Some(slot)) =
                (data.next().zip(data.next()).map(|(a, b)| u16::from_le_bytes([a, b])), k.number(), s.number_mut(k))
                && range.contains(&v)
            {
                *slot = v;
            }
        }
        // LTHR (2 bytes) and POWER SMOOTH (a byte), appended after the limits: a file without them reads 0 and the live
        // reading; an LTHR out of range reads unset
        // (both bytes or none: a file cut inside LTHR reads the default)
        let lthr = data.next().zip(data.next()).map_or(d.lthr, |(a, b)| u16::from_le_bytes([a, b]));
        s.lthr = Some(lthr).filter(|&v| v == 0 || menu::LTHR_RANGE.contains(&v)).unwrap_or(d.lthr);
        s.power_smooth = data.next().map(usize::from).filter(|&v| v < POWER_SMOOTH_S.len()).unwrap_or(d.power_smooth);
        // AUTO, DAY THEME, NIGHT THEME (appended after POWER SMOOTH): a file without them reads AUTO off, OUTDOOR by day
        // and DARK by night; a name out of its form reads as its default
        s.auto_theme = data.next().map_or(d.auto_theme, |v| if v < 2 { v == 1 } else { d.auto_theme });
        s.day_theme = theme_name_at(&mut data).map(normal).unwrap_or(d.day_theme);
        s.night_theme = theme_name_at(&mut data).map(normal).unwrap_or(d.night_theme);
        // AUTO LAP, LAP EVERY km and min (appended after the themes): a file without them reads OFF, 5 km, 10 min; a
        // byte out of its range reads its default
        s.auto_lap = data.next().map(usize::from).filter(|&v| v < 3).unwrap_or(d.auto_lap);
        s.lap_km = data.next().map(u16::from).filter(|v| (1..=50).contains(v)).unwrap_or(d.lap_km);
        s.lap_min = data.next().map(u16::from).filter(|v| (1..=60).contains(v)).unwrap_or(d.lap_min);
        // LIVE SEGMENTS and TARGET (appended after AUTO LAP): a file without them reads ON and AUTO; a byte out of its
        // range reads its default
        s.live_segments = data.next().map_or(d.live_segments, |v| if v < 2 { v == 1 } else { d.live_segments });
        s.seg_target = data.next().map(usize::from).filter(|&v| v < SEGMENT_TARGETS.len()).unwrap_or(d.seg_target);
        // SOUND, UNITS, TEMPERATURE, AUTO PAUSE (appended after TARGET): a file without them reads ON, METRIC, C and the
        // profile's rule; a byte out of its range reads its default
        let flag = |v: Option<u8>, def: bool| v.map_or(def, |v| if v < 2 { v == 1 } else { def });
        s.sound = flag(data.next(), d.sound);
        s.imperial = flag(data.next(), d.imperial);
        s.temp_f = flag(data.next(), d.temp_f);
        s.auto_pause = data.next().map(usize::from).filter(|&v| v < AUTO_PAUSE_KMH.len()).unwrap_or(d.auto_pause);
        // the trips and the odometer (u32 each, after AUTO PAUSE): a file cut inside one reads 0 for it and the rest
        let mut u32_next = || {
            let b = [data.next()?, data.next()?, data.next()?, data.next()?];
            Some(u32::from_le_bytes(b))
        };
        (s.trip_m[0], s.trip_s[0]) = (u32_next().unwrap_or(0), u32_next().unwrap_or(0));
        (s.trip_m[1], s.trip_s[1]) = (u32_next().unwrap_or(0), u32_next().unwrap_or(0));
        s.odo_m = u32_next().unwrap_or(0);
        // ZONES BY, the custom flag and the ten bounds, then SEX, BIRTH YEAR, HEIGHT (appended after the odometer): a
        // file without them reads MAX HR, the computed zones, male, none set. Bounds that are not whole, or do not
        // rise, read as no custom zones; a year or height out of range reads unset.
        s.zones_lthr = flag(data.next(), d.zones_lthr);
        let custom = flag(data.next(), false);
        let mut up = [0u16; 10];
        let whole =
            up.iter_mut().all(|v| data.next().zip(data.next()).map(|(a, b)| *v = u16::from_le_bytes([a, b])).is_some());
        if whole {
            s.zone_up = up;
            s.zone_custom =
                custom && Settings::zone_bounds_ok(&up) && crate::ride::Zones::custom(s.ftp, s.max_hr, up).is_some();
        }
        s.female = flag(data.next(), false);
        let year = data.next().zip(data.next()).map_or(0, |(a, b)| u16::from_le_bytes([a, b]));
        s.birth_year = Some(year).filter(|y| (1920..=2020).contains(y)).unwrap_or(0);
        s.height_cm = data.next().map(u16::from).filter(|h| (100..=230).contains(h)).unwrap_or(0);
        // MAX HR from age (a byte after HEIGHT): a file without it reads off
        s.max_hr_age = flag(data.next(), false);
        // START REMINDER and READY CHECK (after MAX HR from age): a file without them reads both ON
        s.start_reminder = flag(data.next(), d.start_reminder);
        s.ready_check = flag(data.next(), d.ready_check);
        // CLIMBS, CLIMB PAGE and DATE FORMAT (after READY CHECK): a file without them reads ALL, the climb page on and
        // D/M/Y; a byte out of range reads D/M/Y
        s.climbs_medium = flag(data.next(), d.climbs_medium);
        s.climb_page = flag(data.next(), d.climb_page);
        s.date_fmt = data.next().and_then(crate::DateFormat::from_byte).unwrap_or(d.date_fmt);
        // SUNSET ALERT (after DATE FORMAT): a file without it reads OFF; a byte out of its range reads OFF
        s.sunset_alert = data.next().map(usize::from).filter(|&v| v < SUNSET_LEAD_MIN.len()).unwrap_or(d.sunset_alert);
        // CHAINRINGS and CASSETTE (after SUNSET ALERT): a file without them, or a byte out of range, reads OFF
        s.chainrings = data.next().map(usize::from).filter(|&v| v < CHAINRINGS.len()).unwrap_or(d.chainrings);
        s.cassette = data.next().map(usize::from).filter(|&v| v < CASSETTES.len()).unwrap_or(d.cassette);
        // QUICK STATUS and its six fields (after SUNSET ALERT): a file without them reads ON and the default fields; a
        // file cut inside the fields, or a byte past the last field, keeps the default for that field
        s.quick_status = flag(data.next(), d.quick_status);
        let mut q = QUICK_FIELDS;
        for f in q.iter_mut() {
            *f = data.next().and_then(|v| Field::ALL.get(usize::from(v)).copied()).unwrap_or(*f);
        }
        s.quick_page = quick_page(q);
        // TIME ZONE and SUMMER TIME (after the QUICK STATUS fields): a file without them reads CLOCK and OFF; a byte out of
        // its range reads its default
        s.tz = data.next().map(u16::from).filter(|&v| v <= TZ_MAX).unwrap_or(d.tz);
        s.dst = data.next().and_then(Dst::from_byte).unwrap_or(d.dst);
        s.map_up = flag(data.next(), d.map_up); // MAP (DISPLAY): NORTH UP in a file without the byte
        s.level_step = data.next().map(usize::from).filter(|&v| v < LEVEL_STEPS.len()).unwrap_or(d.level_step);
        s.slope_scale = data.next().map(usize::from).filter(|&v| v < SLOPE_SCALES.len()).unwrap_or(d.slope_scale);
        // GNSS SYSTEMS (after SLOPE SCALE): a file without it, or a byte out of range, reads the receiver's default
        s.gnss_systems = data.next().map(usize::from).filter(|&v| v < gnss::SYSTEMS.len()).unwrap_or(d.gnss_systems);
        // STEEP DESCENT (after GNSS SYSTEMS): a file without it, or a byte out of range, reads OFF
        s.steep_descent =
            data.next().map(usize::from).filter(|&v| v < STEEP_DESCENT_PCT.len()).unwrap_or(d.steep_descent);
        let words = [le32(WORDS_AT)?, le32(WORDS_AT + 4)?, le32(WORDS_AT + 8)?, le32(WORDS_AT + 12)?];
        Some(SettingsCopy { seq: le32(8)?, words, settings: s })
    }

    /// The newer of the file's two copies that check out (its 1024 bytes), if any.
    pub fn newest(file: &[u8; 1024]) -> Option<SettingsCopy> {
        let copy = |k: usize| SettingsCopy::from_sector(file.get(512 * k..512 * (k + 1))?.try_into().ok()?);
        match (copy(0), copy(1)) {
            (Some(a), Some(b)) => Some(if b.seq > a.seq { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

/// The profile after a register change: a register that says ROAD does not undo GRAVEL, CX or COMMUTE from the file
/// (the register holds ROAD for them). shortcut: an explicit GRAVEL to ROAD change before a reset is lost, until the
/// register gets a bit.
fn keep_new_profile(file: Option<ride::Profile>, reg: Option<ride::Profile>) -> Option<ride::Profile> {
    use ride::Profile::{Commute, Cx, Gravel, Road};
    if reg == Some(Road) && matches!(file, Some(Gravel | Cx | Commute)) { file } else { reg }
}

/// A theme name is letters, digits, - and _, then NUL padding (an empty name is DARK).
fn theme_name_ok(name: &[u8; 12]) -> bool {
    let n = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    name.get(..n).is_some_and(|s| s.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        && name.get(n..).is_some_and(|s| s.iter().all(|&b| b == 0))
}

/// A theme name read from the file as its canonical bytes: DARK (or any case of it) is the empty name.
fn normal(name: [u8; 12]) -> [u8; 12] {
    Settings::name_bytes(Settings::name_of(&name))
}

/// The next 12 bytes of `data` as a theme name; None if the data ends first or the name is not one.
fn theme_name_at(data: &mut impl Iterator<Item = u8>) -> Option<[u8; 12]> {
    let mut name = [0u8; 12];
    for n in name.iter_mut() {
        *n = data.next()?;
    }
    theme_name_ok(&name).then_some(name)
}

/// Failed reads of the file after which the passes stop trying.
const LOAD_TRIES: u8 = 3;

/// `\Rustfw\settings.bin`, read and to be written.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SettingsStore {
    /// Read, or known not to be there (no file, no volume).
    pub(crate) loaded: bool,
    /// Reads that failed (I/O).
    pub(crate) tries: u8,
    /// The settings changed since the file was written (the registers have them already: [`App::save_misc`]).
    pub(crate) dirty: bool,
    /// The newest copy's number.
    pub(crate) seq: u32,
    /// At the start, register 11 held no settings of this firmware's: the file wins.
    pub(crate) lost: bool,
    /// The registers at the start ([`registers`]) and the words of the settings read from them
    /// ([`Settings::register_words`]): until the file is read, a register is written only where a change made it
    /// differ from the latter, else left as it was (its fields then come from the file, not from defaults the
    /// registers' tags lacked).
    pub(crate) boot: [u32; 4],
    pub(crate) boot_words: [u32; 4],
    /// THEME was picked (the file may not be read yet): the file's name of it does not win.
    pub(crate) theme_set: bool,
    /// DAY THEME or NIGHT THEME was picked: the file's names do not win (as THEME's).
    pub(crate) names_set: bool,
    /// The file was read whole: written over in place (as `PageStore::sound`).
    pub(crate) sound: bool,
    /// FAT sectors compared so far.
    pub(crate) sync: u32,
    /// The totals (m, s) of the ride the recorder is saving, set when it is ended, and of the one ended behind it (a
    /// ride held while the last one's save is pending): each is added to the trips once, when its own save is done.
    pub(crate) saving: Option<(u32, u32)>,
    /// See `saving`.
    pub(crate) queued: Option<(u32, u32)>,
    /// Totals of saved rides not yet added to the trips: they wait for the file to be read ([`App::pay_trips`]).
    pub(crate) due: (u32, u32),
}

impl SettingsStore {
    /// A ride was ended (SAVE): its totals wait for its save.
    pub(crate) fn ended(&mut self, m: u32, s: u32) {
        match &mut self.saving {
            None => self.saving = Some((m, s)),
            Some(_) => {
                let q = self.queued.get_or_insert((0, 0));
                (q.0, q.1) = (q.0.saturating_add(m), q.1.saturating_add(s));
            }
        }
    }

    /// The ride being saved is done: `saved` (a `Saved` outcome) makes its totals due, else (kept, discarded) they are
    /// dropped. The ride queued behind it is the one being saved now. A save with no totals (a ride the boot's
    /// recovery saved without the rider ending it) adds nothing to the trips.
    pub(crate) fn done(&mut self, saved: bool) {
        if let Some((m, s)) = self.saving.take().filter(|_| saved) {
            (self.due.0, self.due.1) = (self.due.0.saturating_add(m), self.due.1.saturating_add(s));
        }
        self.saving = self.queued.take();
    }

    /// The totals of rides ended and not yet in the trips (saving, queued, due): a trip field adds them so its value
    /// does not dip between the end of a ride and its save.
    pub(crate) fn unpaid(&self) -> (u32, u32) {
        let (a, b) = (self.saving.unwrap_or((0, 0)), self.queued.unwrap_or((0, 0)));
        (a.0.saturating_add(b.0).saturating_add(self.due.0), a.1.saturating_add(b.1).saturating_add(self.due.1))
    }

    /// As at a start: the registers read.
    pub(crate) fn at_boot(b: &mut impl Board, read: Settings) -> Self {
        let boot = registers(b);
        let lost = !matches!(boot[0] >> 24, 0xa5..=0xac);
        SettingsStore { lost, boot, boot_words: read.register_words(), ..Default::default() }
    }

    /// Register `k`'s word ([`Settings::register_words`]' order) is written: the file read, the registers lost at
    /// the start (this firmware's to set again), or a change made it differ from the start's.
    pub(crate) fn writes(&self, k: usize, word: u32) -> bool {
        self.loaded || !matches!(self.boot[0] >> 24, 0xa5..=0xac) || self.boot_words.get(k) != Some(&word)
    }
}

/// The settings' registers as a copy's words compare with them: 11, 16 and 28 whole, then AUTO OFF's index + 1 from
/// register 18 (0 where its tag says nothing of this build's wrote it, [`Settings::with_auto_off_word`]).
fn registers(b: &mut impl Board) -> [u32; 4] {
    let aid = b.backup_read(AID_BKP[1]);
    let off = if matches!(aid >> 28, 0xa | 0x5) { aid >> 22 & 7 } else { 0 };
    [b.backup_read(WHEEL_BKP), b.backup_read(POWER_BKP), b.backup_read(ALERTS_BKP), off]
}

impl Settings {
    /// These settings (a copy's, written with the registers' `words`) with the fields of each register in `regs`
    /// ([`Settings::register_words`]' order) that changed since taken from it; the fields of the others, and those a
    /// changed register does not carry (its tag says which), stay. Which registers gave theirs.
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let file = Settings { ftp: 265, weight_kg: 68, auto_off: 3, ..Settings::from_word(0) };
    /// let words = file.register_words();
    /// let now = Settings { weight_kg: 70, ..file }; // register 16 changed alone
    /// let regs = [words[0], now.power_word(), words[2], 0]; // register 18 without AUTO OFF's bits
    /// let (s, took) = file.with_registers(words, regs);
    /// assert_eq!((s, took), (now, [false, true, false, false]));
    /// ```
    pub fn with_registers(self, words: [u32; 4], regs: [u32; 4]) -> (Settings, [bool; 4]) {
        let r = Settings::from_word(regs[0]).with_power_word(regs[1]).with_alerts_word(regs[2]);
        let mut s = self;
        let changed = |k: usize| words.get(k) != regs.get(k);
        let tag = |k: usize| regs.get(k).map_or(0, |w| w >> 24);
        let mut took = [false; 4];
        if changed(0) && matches!(tag(0), 0xa5..=0xac) {
            took[0] = true;
            (s.wheel_mm, s.light_level, s.light_timeout) = (r.wheel_mm, r.light_level, r.light_timeout);
            if tag(0) >= 0xa6 {
                (s.max_hr, s.ftp) = (r.max_hr, r.ftp);
            }
            if tag(0) >= 0xa7 {
                (s.touch, s.ahead) = (r.touch, r.ahead);
                s.profile = keep_new_profile(s.profile, r.profile);
            }
        }
        if changed(1) && matches!(tag(1), 0xa0..=0xbf | 0xc1) {
            took[1] = true;
            (s.weight_kg, s.wprime_j, s.wkt_avg, s.bike_hg) = (r.weight_kg, r.wprime_j, r.wkt_avg, r.bike_hg);
            if tag(1) <= 0xaf {
                (s.face, s.gps_power) = (r.face, r.gps_power);
                s.profile = keep_new_profile(s.profile, r.profile);
            } else {
                (s.remote[0], s.remote[1]) = (r.remote[0], r.remote[1]); // earlier builds' single presses
            }
        }
        if changed(2) && (tag(2) >> 4 == 0x9 || tag(2) == 0xc2) {
            took[2] = true;
            (s.eat_min, s.drink_min, s.hr_alert) = (r.eat_min, r.drink_min, r.hr_alert);
            (s.remote[2], s.remote[3]) = (r.remote[2], r.remote[3]);
            if tag(2) >> 4 == 0x9 {
                (s.remote[0], s.remote[1]) = (r.remote[0], r.remote[1]);
            }
        }
        if changed(3)
            && let Some(i) = regs.get(3).and_then(|&v| v.checked_sub(1)).filter(|&i| i < 5)
        {
            took[3] = true;
            s.auto_off = i as usize;
        }
        (s, took)
    }
}

impl App {
    /// The settings' file read when the pages' is ([`PAGES_LOAD_MS`] after the start, or at the first screen other
    /// than the overview, or at a change; again after a failed read, [`LOAD_TRIES`] times), written when the settings
    /// changed and no ride is open.
    #[inline(never)]
    pub(crate) fn step_settings(&mut self, b: &mut impl Board) {
        if self.emmc.lent() {
            return;
        }
        // when pages.bin is read (the card not touched at boot), or at once for a change
        let due = self.screen != Screen::Main || self.gps.ms >= PAGES_LOAD_MS || self.settings_store.dirty;
        let st = self.settings_store;
        if !st.loaded && (st.tries < LOAD_TRIES || st.dirty) && due {
            self.load_settings(b);
        }
        self.pay_trips();
        let st = self.settings_store;
        if !(self.card_writable && st.loaded && st.dirty && self.ride_settled()) {
            return;
        }
        if !st.sound {
            match Self::sync_fats_step(&mut self.emmc, b, &mut self.settings_store.sync) {
                Some(false) => return, // the FAT copies compared over the next passes first
                Some(true) => self.fats_equal(),
                None => {}
            }
        }
        self.save_settings(b);
    }

    /// A saved ride's totals go to TRIP 1, TRIP 2 and the ODOMETER, once the file is read (its values are the base) and
    /// never during a ride: the settings are only written to the card between rides. A discarded ride never gets here.
    fn pay_trips(&mut self) {
        let st = &mut self.settings_store;
        if !(st.due != (0, 0) && st.loaded) {
            return;
        }
        let (m, s) = core::mem::take(&mut st.due);
        if m != 0 || s != 0 {
            st.dirty |= self.card_writable;
            self.settings.add_ride(m, s);
        }
    }

    /// Reads `\Rustfw\settings.bin` (module doc: which wins).
    #[inline(never)] // its 1 KB buffer in its own frame
    pub(crate) fn load_settings(&mut self, b: &mut impl Board) {
        let mut file = [0u8; 1024];
        let read = self.read_copies(b, SETTINGS_FILE, &mut file);
        let regs = registers(b);
        let st = &mut self.settings_store;
        if read.is_err() && read != Err(fat::Error::NotFound) {
            // tried again (a write now would not know the newest copy's number, and "no file" would write a first
            // copy beside a newer one), at the next change once the passes gave up: the registers keep the change
            // meanwhile, and win over the file once it is read
            st.tries = st.tries.saturating_add(1);
            st.dirty = false;
            return;
        }
        st.loaded = true;
        st.sound = read == Ok(1024) && !self.fats_suspect;
        let Some(c) = read.ok().filter(|&n| n == 1024).and_then(|_| SettingsCopy::newest(&file)) else {
            st.dirty = self.card_writable; // the migration: the registers' settings as the first copy
            if self.card_writable && self.settings.register_words() != regs {
                self.write_settings_registers(b); // an earlier build's words: this build's, as the copy has them
            }
            return;
        };
        st.seq = c.seq;
        // the registers lost: the file's whole; else each register that changed since the copy gives its fields
        let (merged, took) = if st.lost { (c.settings, [false; 4]) } else { c.settings.with_registers(c.words, regs) };
        let was = self.settings;
        // a theme picked before the file was read stays and goes to the file (the registers carry no theme)
        // the registers carry no theme: THEME, AUTO and the day and night themes are the file's, or those picked before it
        let pick = if st.theme_set { was } else { merged };
        let names = if st.names_set { was } else { merged };
        let now = Settings { day_theme: names.day_theme, night_theme: names.night_theme, ..pick };
        let themes = |s: &Settings| (s.theme, s.auto_theme, s.day_theme, s.night_theme);
        // the file gets the registers' fields, or words that say what the registers now hold, or the theme picked
        st.dirty = self.card_writable
            && (took.contains(&true) || merged.register_words() != c.words || themes(&now) != themes(&c.settings));
        self.settings = Settings {
            alt_m: was.alt_m,
            theme: pick.theme,
            auto_theme: pick.auto_theme,
            day_theme: now.day_theme,
            night_theme: now.night_theme,
            ..merged
        };
        if (st.lost || self.card_writable) && self.settings.register_words() != regs {
            // put right from the result (lost, or a register some fields missed); a build that may not write the
            // card leaves them as another build wrote them, but not stock's import over lost ones (the next
            // writable start would take it over the file)
            self.write_settings_registers(b);
        }
        if self.settings != was {
            self.update_light(b);
            self.gps.reconfig = true;
            if (self.settings.theme, self.settings.auto_theme, self.settings.day_theme, self.settings.night_theme)
                != (was.theme, was.auto_theme, was.day_theme, was.night_theme)
            {
                self.load_day_night(b);
                self.apply_theme(b, false);
            } else if self.settings.face != was.face {
                self.font_changed(b);
            }
        }
    }

    /// Writes the settings over the file's older copy (the file created with both if needed).
    fn save_settings(&mut self, b: &mut impl Board) {
        self.settings_store.dirty = false; // once: a failure is said, and tried again at the next change
        let st = self.settings_store;
        let c = SettingsCopy {
            seq: st.seq.wrapping_add(1),
            words: self.settings.register_words(),
            settings: self.settings,
        };
        // no volume (a card stock never formatted, the tests' blank one): the registers only, nothing to say
        match self.write_copy(b, SETTINGS_FILE, c.seq % 2, &c.to_sector()) {
            None => {}
            Some(Ok(())) => (self.settings_store.seq, self.settings_store.sound) = (c.seq, true),
            Some(Err(_)) => {
                self.fats_unsure(); // compared again first
                self.notice("SETTINGS NOT SAVED");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some() -> Settings {
        Settings {
            wheel_mm: WHEELS[4],
            max_hr: 177,
            ftp: 265,
            weight_kg: 68,
            wprime_j: 23_500,
            bike_hg: 85,
            eat_min: 45,
            drink_min: 20,
            hr_alert: 175,
            light_level: 1,
            light_timeout: 3,
            auto_off: 4,
            wkt_avg: 2,
            remote: [menu::Remote::Lap, menu::Remote::Play, menu::Remote::PrevTrack, menu::Remote::Start],
            touch: false,
            profile: Some(ride::Profile::Mtb),
            ahead: false,
            face: Face::Barlow,
            gps_power: GpsPower::Saver,
            power_match: false,
            lthr: 158,
            power_smooth: 2,
            limits: [400, 90, 120, 60, 45, 15, 100, 10, 300, 500, 75],
            ..Settings::from_word(0)
        }
    }

    fn file(a: Option<SettingsCopy>, b: Option<SettingsCopy>) -> [u8; 1024] {
        let mut f = [0u8; 1024];
        for (k, c) in [a, b].into_iter().enumerate() {
            if let Some(c) = c {
                f[512 * k..512 * (k + 1)].copy_from_slice(&c.to_sector());
            }
        }
        f
    }

    #[test]
    fn round_trip_and_newest_wins() {
        let s = some();
        let c = SettingsCopy { seq: 7, words: s.register_words(), settings: s };
        assert_eq!(SettingsCopy::from_sector(&c.to_sector()), Some(c));
        let d = SettingsCopy { seq: 8, settings: Settings::from_word(0), ..c };
        assert_eq!(SettingsCopy::newest(&file(Some(c), Some(d))), Some(d));
        assert_eq!(SettingsCopy::newest(&file(Some(d), Some(c))), Some(d), "either slot");
        let mut f = file(Some(c), Some(d));
        f[512 + 100] ^= 0x10;
        assert_eq!(SettingsCopy::newest(&f), Some(c), "the newer damaged: the older");
        f[100] ^= 0x10;
        assert_eq!(SettingsCopy::newest(&f), None, "both damaged");
        assert_eq!(SettingsCopy::newest(&[0; 1024]), None);
    }

    /// A copy of an earlier build (shorter) gives the defaults past its end; one of a later build (longer) the
    /// settings this build knows; a value out of range its default; another version nothing.
    #[test]
    fn short_long_and_foreign_copies() {
        let s = some();
        let full = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        let len = usize::from(u16::from_le_bytes([full[6], full[7]]));
        assert_eq!(len, 172);
        let with = |f: &dyn Fn(&mut [u8; 512])| {
            let mut sec = full;
            f(&mut sec);
            let crc = inflate::crc32(sec[..CRC_AT].iter().copied());
            sec[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
            SettingsCopy::from_sector(&sec).map(|c| c.settings)
        };
        // the first 18 bytes only (the wheel and the numbers): the choices are the defaults
        let short = with(&|sec| sec[6] = 18).unwrap();
        let d = Settings::from_word(0);
        assert_eq!((short.ftp, short.hr_alert, short.face, short.profile), (265, 175, d.face, d.profile));
        let none = with(&|sec| sec[6] = 0).unwrap();
        assert_eq!(none, d, "nothing: the defaults");
        let long = with(&|sec| {
            sec[6] = 72;
            sec[DATA_AT + 69..DATA_AT + 72].fill(0xee);
        });
        assert_eq!(long, Some(s), "a later build's settings past the ones known are left");
        let old = with(&|sec| sec[6] = 43).unwrap();
        assert!(old.power_match && !s.power_match, "a file from before POWER MATCH reads ON, the rest as it was");
        assert_eq!(
            Settings { power_match: false, ..old },
            Settings { limits: [0; alerts::LIMITS], lthr: 0, power_smooth: 0, ..s }
        );
        let older = with(&|sec| sec[6] = 44).unwrap();
        assert_eq!(
            (older.limits, older.power_match, older.lthr),
            ([0; alerts::LIMITS], false, 0),
            "a file from before the limits reads them OFF"
        );
        assert_eq!(
            Settings { limits: s.limits, lthr: 0, power_smooth: 0, ..older },
            Settings { lthr: 0, power_smooth: 0, ..s }
        );
        let wild_limit =
            with(&|sec| sec[DATA_AT + 44..DATA_AT + 46].copy_from_slice(&60_000u16.to_le_bytes())).unwrap();
        assert_eq!((wild_limit.limits[0], wild_limit.limits[1]), (0, s.limits[1]), "out of range: OFF");
        let v67 = with(&|sec| sec[6] = 67).unwrap(); // LTHR cut after one byte
        assert_eq!((v67.limits, v67.lthr, v67.power_smooth), (s.limits, 0, 0), "a cut LTHR reads the default");
        let v66 = with(&|sec| sec[6] = 66).unwrap();
        assert_eq!(
            (v66.limits, v66.lthr, v66.power_smooth),
            (s.limits, 0, 0),
            "a file from before LTHR and POWER SMOOTH reads them unset and on the live reading"
        );
        let wild = with(&|sec| sec[DATA_AT + 43] = 7).unwrap();
        assert!(wild.power_match, "a byte out of range reads the default, ON");
        // LTHR at 66 (2 bytes), POWER SMOOTH at 68: out of range reads the defaults
        let wild2 = with(&|sec| {
            sec[DATA_AT + 66..DATA_AT + 68].fill(0xff); // an LTHR out of range
            sec[DATA_AT + 68] = 9; // a POWER SMOOTH out of range
        })
        .unwrap();
        assert_eq!((wild2.lthr, wild2.power_smooth), (0, 0), "out of range: the defaults");
        let odd = with(&|sec| {
            sec[DATA_AT..DATA_AT + 2].copy_from_slice(&1234u16.to_le_bytes()); // no such wheel
            sec[DATA_AT + 4..DATA_AT + 6].copy_from_slice(&9999u16.to_le_bytes()); // FTP out of range
            sec[DATA_AT + 18] = 9; // the backlight
            sec[DATA_AT + 27] = 7; // the profile
        })
        .unwrap();
        assert_eq!(
            (odd.wheel_mm, odd.ftp, odd.light_level, odd.profile),
            (d.wheel_mm, d.ftp, d.light_level, d.profile)
        );
        assert_eq!((odd.max_hr, odd.face), (s.max_hr, s.face), "the others kept");
        assert_eq!(with(&|sec| sec[4] = 2), None, "another version");
        assert_eq!(with(&|sec| sec[6..8].copy_from_slice(&481u16.to_le_bytes())), None, "longer than a copy holds");
    }
}

#[cfg(test)]
mod day_night_file_tests {
    use super::*;

    /// FONT's choice is an index in the settings file: PIXEL and BARLOW, and any other index the default.
    #[test]
    fn font_face_round_trips_and_older_files_read_as_before() {
        let copy = |face| SettingsCopy {
            seq: 3,
            words: Settings::from_word(0).register_words(),
            settings: Settings { face, ..Settings::from_word(0) },
        };
        let at = (0..508).find(|&i| copy(Face::Pixel).to_sector()[i] != copy(Face::Barlow).to_sector()[i]);
        let at = at.expect("the face is one byte of the sector");
        for face in Face::ALL {
            assert_eq!(SettingsCopy::from_sector(&copy(face).to_sector()).map(|c| c.settings.face), Some(face));
        }
        for (index, face) in [(0u8, Face::Pixel), (1, Face::Barlow)] {
            let mut sec = copy(Face::Pixel).to_sector();
            sec[at] = index;
            recrc(&mut sec);
            assert_eq!(SettingsCopy::from_sector(&sec).map(|c| c.settings.face), Some(face), "index {index}");
        }
        let mut sec = copy(Face::Pixel).to_sector();
        sec[at] = 7;
        recrc(&mut sec);
        assert_eq!(
            SettingsCopy::from_sector(&sec).map(|c| c.settings.face),
            Some(Face::Pixel),
            "no such face: the default"
        );
    }

    /// ZONES BY, the custom zones, SEX, BIRTH YEAR and HEIGHT are kept at data 123..150 and read back; a file of 44 to
    /// 123 bytes reads the computed zones, MAX HR, male and none set; bounds that do not rise read as the computed zones.
    #[test]
    fn zones_sex_year_and_height_round_trip_and_older_files_read_defaults() {
        let up = [100, 120, 140, 160, 90, 130, 170, 210, 250, 300];
        let s = Settings {
            zones_lthr: true,
            zone_custom: true,
            zone_up: up,
            female: true,
            birth_year: 1984,
            height_cm: 178,
            lthr: 165,
            ..Settings::from_word(0)
        };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(u16::from_le_bytes([sec[6], sec[7]]), 172);
        assert_eq!(sec[28 + 123..28 + 125], [1, 1]);
        let back = SettingsCopy::from_sector(&sec).unwrap().settings;
        assert_eq!((back.zones().upper(), back.zones_lthr, back.female), (up, true, true));
        assert_eq!((back.birth_year, back.height_cm), (1984, 178));
        for len in [44u16, 66, 99, 123] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            let o = SettingsCopy::from_sector(&old).unwrap().settings;
            assert_eq!((o.zone_custom, o.zones_lthr, o.female, o.birth_year, o.height_cm), (false, false, false, 0, 0));
            let want = ride::Zones::computed(o.ftp, o.max_hr, o.lthr, ride::ZonesBy::MaxHr);
            assert_eq!(o.zones(), want, "a {len}-byte file");
        }
        let mut bad = sec;
        bad[28 + 127..28 + 129].copy_from_slice(&[5, 0]); // zone 3 ends under zone 2
        recrc(&mut bad);
        let o = SettingsCopy::from_sector(&bad).unwrap().settings;
        assert!(!o.zone_custom, "bounds that do not rise: the computed zones");
        for (at, v) in [(0usize, 30u16), (3, 300), (9, 2500)] {
            // heart rate under 40 or over 250, power over 2000, still rising
            let mut wild = sec;
            let mut b = up;
            b[at] = v;
            for (k, x) in b.iter().enumerate() {
                wild[28 + 125 + 2 * k..28 + 127 + 2 * k].copy_from_slice(&x.to_le_bytes());
            }
            recrc(&mut wild);
            let o = SettingsCopy::from_sector(&wild).unwrap().settings;
            assert!(!o.zone_custom, "bound {at} = {v} is out of what ZONES offers");
        }
        assert!(SettingsCopy::from_sector(&sec).unwrap().settings.zone_custom, "the good ones are read");
        assert!(o.zones_lthr && o.female, "the rest kept");
    }

    /// A register that changed alone (the light) says ROAD for a GRAVEL file: the file's profile stays.
    #[test]
    fn a_register_change_keeps_a_gravel_profile_from_the_file() {
        let file = Settings { profile: Some(ride::Profile::Gravel), light_level: 1, ..Settings::from_word(0) };
        let words = file.register_words();
        let now = Settings { light_level: 3, ..file };
        let mut regs = words;
        regs[0] = Settings { profile: Some(ride::Profile::Road), ..now }.to_word();
        let (s, took) = file.with_registers(words, regs);
        assert!(took[0]);
        assert_eq!((s.light_level, s.profile), (3, Some(ride::Profile::Gravel)));
    }

    /// The profile byte (data 27) is the index in [`PROFILES`]: the first four keep their numbers (a file written before
    /// GRAVEL, CX and COMMUTE reads the same profile), the new ones follow, and each round-trips.
    #[test]
    fn the_profile_byte_keeps_old_indexes_and_reads_the_new_profiles() {
        use ride::Profile::{Commute, Cx, Gravel, Indoor, Mtb, Road};
        let want = [None, Some(Road), Some(Indoor), Some(Mtb), Some(Gravel), Some(Cx), Some(Commute)];
        for (i, p) in want.into_iter().enumerate() {
            let s = Settings { profile: p, ..Settings::from_word(0) };
            let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
            assert_eq!(sec[28 + 27] as usize, i, "{p:?}");
            assert_eq!(SettingsCopy::from_sector(&sec).map(|c| c.settings.profile), Some(p));
        }
    }

    /// AUTO LAP, LAP EVERY km and min are kept at data 94, 95, 96 and read back; a file of 44, 66, 69 or 94 bytes reads
    /// OFF, 5 km and 10 min; a byte out of range reads its default.
    #[test]
    fn auto_lap_round_trips_and_older_files_read_it_off() {
        let s = Settings { auto_lap: 2, lap_km: 12, lap_min: 45, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(u16::from_le_bytes([sec[6], sec[7]]), 172);
        assert_eq!(sec[28 + 94..28 + 97], [2, 12, 45]);
        let read = |sec: &[u8; 512]| {
            SettingsCopy::from_sector(sec).map(|c| (c.settings.auto_lap, c.settings.lap_km, c.settings.lap_min))
        };
        assert_eq!(read(&sec), Some((2, 12, 45)));
        for len in [44u16, 66, 69, 94] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some((0, 5, 10)), "a {len}-byte file");
        }
        let mut bad = sec;
        bad[28 + 94..28 + 97].copy_from_slice(&[9, 0, 61]);
        recrc(&mut bad);
        assert_eq!(read(&bad), Some((0, 5, 10)));
    }

    /// LIVE SEGMENTS and TARGET are kept at data 97 and 98 and read back; a file of 44, 66, 69, 94 or 97 bytes reads ON and
    /// AUTO; a byte out of range reads its default.
    #[test]
    fn live_segments_round_trip_and_older_files_read_on_and_auto() {
        let s = Settings { live_segments: false, seg_target: 3, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(u16::from_le_bytes([sec[6], sec[7]]), 172);
        assert_eq!(sec[28 + 97..28 + 99], [0, 3]);
        let read =
            |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| (c.settings.live_segments, c.settings.seg_target));
        assert_eq!(read(&sec), Some((false, 3)));
        for len in [44u16, 66, 69, 94, 97] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some((true, 0)), "a {len}-byte file");
        }
        let mut bad = sec;
        bad[28 + 97..28 + 99].copy_from_slice(&[7, 9]);
        recrc(&mut bad);
        assert_eq!(read(&bad), Some((true, 0)));
    }

    fn recrc(sec: &mut [u8; 512]) {
        let crc = crate::inflate::crc32(sec[..508].iter().copied());
        sec[508..].copy_from_slice(&crc.to_le_bytes());
    }

    /// AUTO, DAY THEME and NIGHT THEME are kept after POWER SMOOTH (data 69 on), and read back; a file of 44, 66 or 69
    /// bytes (an earlier build) reads AUTO off, OUTDOOR by day and DARK by night, its other settings as they were.
    #[test]
    fn auto_and_the_day_and_night_themes_round_trip_and_older_files_read_them_off() {
        let s = Settings {
            auto_theme: true,
            day_theme: Settings::name_bytes("LIGHT"),
            night_theme: Settings::name_bytes("SUNNY"),
            theme: Settings::name_bytes("OUTDOOR"),
            ..Settings::from_word(0)
        };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(u16::from_le_bytes([sec[6], sec[7]]), 172);
        assert_eq!(sec[28 + 69], 1, "AUTO at data 69");
        assert_eq!(sec[28 + 70..28 + 82], Settings::name_bytes("LIGHT"), "DAY THEME at 70");
        assert_eq!(sec[28 + 82..28 + 94], Settings::name_bytes("SUNNY"), "NIGHT THEME at 82");
        let back = SettingsCopy::from_sector(&sec).map(|c| c.settings);
        assert_eq!(
            back.map(|b| (b.auto_theme, b.day_theme, b.night_theme, b.theme)),
            Some((true, s.day_theme, s.night_theme, s.theme))
        );
        for len in [44u16, 66, 69] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            let o = SettingsCopy::from_sector(&old).map(|c| c.settings);
            let got = o.map(|o| (o.auto_theme, o.day_theme, o.night_theme, o.theme));
            assert_eq!(got, Some((false, Settings::name_bytes("OUTDOOR"), [0; 12], s.theme)), "a {len}-byte file");
        }
        // a bad AUTO byte reads off; a damaged DAY THEME reads OUTDOOR
        let mut bad = sec;
        bad[28 + 69] = 7;
        bad[28 + 70] = b'/';
        recrc(&mut bad);
        let b = SettingsCopy::from_sector(&bad).map(|c| c.settings);
        assert_eq!(b.map(|b| (b.auto_theme, b.day_theme)), Some((false, Settings::name_bytes("OUTDOOR"))));
    }

    /// SOUND, UNITS, TEMPERATURE, AUTO PAUSE (data 99..102) and the trips and the odometer (103..123) are kept and read
    /// back; a file of 44, 66, 69, 94, 97 or 99 bytes reads ON, METRIC, C, AUTO and zeros; a byte out of range reads its
    /// default; a file cut inside the trips reads 0 for the cut one and those after it.
    #[test]
    fn units_sound_auto_pause_and_trips_round_trip_and_older_files_read_defaults() {
        let s = Settings {
            sound: false,
            imperial: true,
            temp_f: true,
            auto_pause: 4,
            trip_m: [1234, 5_000_000],
            trip_s: [99, 70_000],
            odo_m: 987_654,
            ..Settings::from_word(0)
        };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(u16::from_le_bytes([sec[6], sec[7]]), 172);
        assert_eq!(sec[28 + 99..28 + 103], [0, 1, 1, 4]);
        let read = |sec: &[u8; 512]| {
            SettingsCopy::from_sector(sec).map(|c| {
                let s = c.settings;
                ((s.sound, s.imperial, s.temp_f, s.auto_pause), (s.trip_m, s.trip_s, s.odo_m))
            })
        };
        assert_eq!(read(&sec), Some(((false, true, true, 4), ([1234, 5_000_000], [99, 70_000], 987_654))));
        for len in [44u16, 66, 69, 94, 97, 99] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some(((true, false, false, 0), ([0; 2], [0; 2], 0))), "a {len}-byte file");
        }
        let mut cut = sec;
        cut[6..8].copy_from_slice(&(103u16 + 4 + 4 + 2).to_le_bytes()); // TRIP 1, its time, two bytes of TRIP 2
        recrc(&mut cut);
        assert_eq!(read(&cut).map(|r| r.1), Some(([1234, 0], [99, 0], 0)));
        let mut bad = sec;
        bad[28 + 99..28 + 103].copy_from_slice(&[2, 7, 255, 9]);
        recrc(&mut bad);
        assert_eq!(read(&bad).map(|r| r.0), Some((true, false, false, 0)));
    }

    /// CLIMBS, CLIMB PAGE and DATE FORMAT (data 152 to 154) are kept and read back; a file that stops before one reads
    /// its default (ALL, on, D/M/Y), a byte out of range reads D/M/Y.
    #[test]
    fn climbs_and_date_format_round_trip_and_older_files_read_defaults() {
        let s = Settings {
            climbs_medium: true,
            climb_page: false,
            date_fmt: crate::DateFormat::Ymd,
            ..Settings::from_word(0)
        };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 152..28 + 155], [1, 0, 2]);
        let read = |sec: &[u8; 512]| {
            SettingsCopy::from_sector(sec)
                .map(|c| (c.settings.climbs_medium, c.settings.climb_page, c.settings.date_fmt))
        };
        assert_eq!(read(&sec), Some((true, false, crate::DateFormat::Ymd)));
        for (len, want) in [
            (152u16, (false, true, crate::DateFormat::Dmy)),
            (153, (true, true, crate::DateFormat::Dmy)),
            (154, (true, false, crate::DateFormat::Dmy)),
        ] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some(want), "a {len}-byte file");
        }
        let mut bad = sec;
        bad[28 + 154] = 9;
        recrc(&mut bad);
        assert_eq!(read(&bad), Some((true, false, crate::DateFormat::Dmy)));
    }

    /// START REMINDER and READY CHECK (data 150, 151) are kept and read back; a file of 44 to 150 bytes reads both ON,
    /// a byte out of range reads ON.
    #[test]
    fn start_reminder_and_ready_check_round_trip_and_older_files_read_on() {
        let s = Settings { start_reminder: false, ready_check: false, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 150..28 + 152], [0, 0]);
        let read = |sec: &[u8; 512]| {
            SettingsCopy::from_sector(sec).map(|c| (c.settings.start_reminder, c.settings.ready_check))
        };
        assert_eq!(read(&sec), Some((false, false)));
        for len in [44u16, 99, 123, 149, 150, 151] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            // 151 holds the first byte only
            assert_eq!(read(&old), Some(if len == 151 { (false, true) } else { (true, true) }), "a {len}-byte file");
        }
        let mut bad = sec;
        bad[28 + 150..28 + 152].copy_from_slice(&[2, 255]);
        recrc(&mut bad);
        assert_eq!(read(&bad), Some((true, true)));
    }

    /// SUNSET ALERT (after DATE FORMAT, byte 155): kept and read back; a file without it (152, 154, 155 bytes) reads OFF,
    /// a byte out of range reads OFF.
    #[test]
    fn sunset_alert_round_trips_and_older_files_read_off() {
        let s = Settings { sunset_alert: 2, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 155], 2);
        let read = |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| c.settings.sunset_alert);
        assert_eq!(read(&sec), Some(2));
        for len in [152u16, 154, 155] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            let crc = inflate::crc32(old[..CRC_AT].iter().copied());
            old[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
            assert_eq!(read(&old), Some(0), "a {len}-byte file");
        }
        let mut wild = sec;
        wild[28 + 155] = 9;
        let crc = inflate::crc32(wild[..CRC_AT].iter().copied());
        wild[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(read(&wild), Some(0), "a byte out of range reads OFF");
    }

    /// TIME ZONE and SUMMER TIME (after SUNSET ALERT, bytes [`TZ_AT`] and the next): kept and read back; a file
    /// that stops before them reads CLOCK and OFF, a byte out of range its default.
    #[test]
    fn time_zone_and_dst_round_trip_and_older_files_read_clock_and_off() {
        let s = Settings { tz: TZ_UTC + 4, dst: Dst::Eu, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + TZ_AT..28 + TZ_AT + 2], [53, 1]);
        let read = |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| (c.settings.tz, c.settings.dst));
        assert_eq!(read(&sec), Some((53, Dst::Eu)));
        for (len, want) in [(TZ_AT, (0, Dst::Off)), (TZ_AT + 1, (53, Dst::Off)), (152, (0, Dst::Off))] {
            let mut old = sec;
            old[6..8].copy_from_slice(&(len as u16).to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some(want), "a {len}-byte file");
        }
        let mut wild = sec;
        wild[28 + TZ_AT..28 + TZ_AT + 2].copy_from_slice(&[TZ_MAX as u8 + 1, 3]);
        recrc(&mut wild);
        assert_eq!(read(&wild), Some((0, Dst::Off)), "bytes out of range read CLOCK and OFF");
    }

    /// MAP (byte [`MAP_AT`], after SUMMER TIME): kept and read back; a file that stops before it reads NORTH UP.
    #[test]
    fn map_orientation_round_trips_and_older_files_read_north_up() {
        let s = Settings { map_up: true, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + MAP_AT], 1);
        let read = |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| c.settings.map_up);
        assert_eq!(read(&sec), Some(true));
        for len in [MAP_AT, 152, 166] {
            let mut old = sec;
            old[6..8].copy_from_slice(&(len as u16).to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some(false), "a {len}-byte file");
        }
    }

    /// Where MAP is in the settings data.
    const MAP_AT: usize = 167;

    /// LEVEL STEP and SLOPE SCALE (bytes 168, 169): kept; older files and wild bytes read the defaults.
    #[test]
    fn trainer_settings_round_trip_and_older_files_read_defaults() {
        let s = Settings { level_step: 2, slope_scale: 2, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!((sec[28 + 168], sec[28 + 169]), (2, 2));
        let read =
            |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| (c.settings.level_step, c.settings.slope_scale));
        assert_eq!(read(&sec), Some((2, 2)));
        for len in [168u16, 169] {
            let mut old = sec;
            old[6..8].copy_from_slice(&len.to_le_bytes());
            recrc(&mut old);
            assert_eq!(read(&old), Some(if len == 169 { (2, 0) } else { (0, 0) }), "a {len}-byte file");
        }
        let mut wild = sec;
        wild[28 + 168] = 99;
        recrc(&mut wild);
        assert_eq!(read(&wild), Some((0, 2)), "a byte out of range");
    }

    /// STEEP DESCENT (byte 171): kept; older files (171 bytes or fewer) and a wild byte read OFF.
    #[test]
    fn steep_descent_round_trips_and_older_files_read_off() {
        let s = Settings { steep_descent: 3, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 171], 3);
        let read = |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| c.settings.steep_descent);
        assert_eq!(read(&sec), Some(3));
        let mut old = sec;
        old[6..8].copy_from_slice(&171u16.to_le_bytes());
        recrc(&mut old);
        assert_eq!(read(&old), Some(0), "a 171-byte file");
        let mut wild = sec;
        wild[28 + 171] = 99;
        recrc(&mut wild);
        assert_eq!(read(&wild), Some(0), "a byte out of range");
    }

    /// Where TIME ZONE is in the settings data.
    const TZ_AT: usize = 165;

    /// QUICK STATUS (after SUNSET ALERT, bytes 158 to 164): kept and read back; a file without it (or cut inside the
    /// fields) reads ON and the default fields; a byte out of range keeps that field's default.
    #[test]
    fn quick_status_round_trips_and_older_files_read_defaults() {
        let mut f = QUICK_FIELDS;
        f[1] = Field::Cadence;
        let s = Settings { quick_status: false, quick_page: quick_page(f), ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!((sec[28 + 158], sec[28 + 160]), (0, Field::Cadence as u8));
        let read =
            |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| (c.settings.quick_status, c.settings.quick_page));
        assert_eq!(read(&sec), Some((false, quick_page(f))));
        let d = Settings::from_word(0);
        let cut = |sec: &[u8; 512], len: u16| {
            let mut c = *sec;
            c[6..8].copy_from_slice(&len.to_le_bytes());
            let crc = inflate::crc32(c[..CRC_AT].iter().copied());
            c[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
            c
        };
        for len in [155u16, 156, 158] {
            assert_eq!(read(&cut(&sec, len)), Some((true, d.quick_page)), "a {len}-byte file");
        }
        // cut inside the fields: the flag is read, the fields past the cut are the defaults
        assert_eq!(read(&cut(&sec, 160)).map(|r| r.0), Some(false), "a 160-byte file");
        let mut wild = sec;
        wild[28 + 159] = 250;
        let crc = inflate::crc32(wild[..CRC_AT].iter().copied());
        wild[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(read(&wild).map(|r| r.1.fields[0]), Some(QUICK_FIELDS[0]), "a byte out of range");
    }

    /// CHAINRINGS and CASSETTE (bytes 156, 157): kept; older files (156, 157 bytes) and out of range bytes read OFF.
    #[test]
    fn gear_presets_round_trip_and_older_files_read_off() {
        let s = Settings { chainrings: 2, cassette: 5, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 156..28 + 158], [2, 5]);
        let read =
            |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| (c.settings.chainrings, c.settings.cassette));
        assert_eq!(read(&sec), Some((2, 5)));
        let fix = |sec: &mut [u8; 512], len: u16| {
            sec[6..8].copy_from_slice(&len.to_le_bytes());
            let crc = inflate::crc32(sec[..CRC_AT].iter().copied());
            sec[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        };
        for (len, want) in [(155u16, (0, 0)), (156, (0, 0)), (157, (2, 0))] {
            let mut old = sec;
            fix(&mut old, len);
            assert_eq!(read(&old), Some(want), "a {len}-byte file");
        }
        let mut wild = sec;
        wild[28 + 156] = 99;
        wild[28 + 157] = 99;
        fix(&mut wild, 158);
        assert_eq!(read(&wild), Some((0, 0)));
    }

    /// GNSS SYSTEMS (byte 170, after SLOPE SCALE): kept; an older file (170 bytes), or a byte out of range, reads the
    /// receiver's default.
    #[test]
    fn gnss_systems_round_trip_and_older_files_read_the_default() {
        let s = Settings { gnss_systems: 3, ..Settings::from_word(0) };
        let sec = SettingsCopy { seq: 1, words: [0; 4], settings: s }.to_sector();
        assert_eq!(sec[28 + 170], 3);
        let read = |sec: &[u8; 512]| SettingsCopy::from_sector(sec).map(|c| c.settings.gnss_systems);
        assert_eq!(read(&sec), Some(3));
        let mut old = sec;
        old[6..8].copy_from_slice(&170u16.to_le_bytes());
        recrc(&mut old);
        assert_eq!(read(&old), Some(0), "a 171-byte file");
        let mut wild = sec;
        wild[28 + 170] = 9;
        recrc(&mut wild);
        assert_eq!(read(&wild), Some(0), "a byte out of range");
    }
}
