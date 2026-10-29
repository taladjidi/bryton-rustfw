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
//! 6    length (u16)    bytes of settings at 28 (31 in version 1, at most 480); a later build appends its settings
//!                      and raises it: a reader takes those it knows, the defaults for those past the length
//! 8    seq (u32)       higher is newer
//! 12   words (4 x u32) the settings' backup registers as written with this copy (which wins at a start, below)
//! 28   settings        in [`NUMBERS`]' order: the wheel (mm), then the numbers (u16 each); then the choices, a
//!                      byte each ([`SettingsCopy::to_sector`]); a value out of its range reads as its default
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
const PROFILES: [Option<ride::Profile>; 4] =
    [None, Some(ride::Profile::Road), Some(ride::Profile::Indoor), Some(ride::Profile::Mtb)];

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
    /// let s = Settings { ftp: 265, smooth: true, ..Settings::from_word(0) };
    /// let c = SettingsCopy { seq: 3, words: s.register_words(), settings: s };
    /// let mut sector = c.to_sector();
    /// assert_eq!(SettingsCopy::from_sector(&sector), Some(c));
    /// sector[40] ^= 1;
    /// assert_eq!(SettingsCopy::from_sector(&sector), None, "damaged");
    /// ```
    pub fn to_sector(&self) -> [u8; 512] {
        let s = self.settings;
        let mut out = [0u8; 512];
        let mut data = [0u8; 64];
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
        put(&[u8::from(s.touch), profile, u8::from(s.ahead), u8::from(s.smooth), gps]);
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
        s.smooth = byte(2).map_or(d.smooth, |v| v == 1);
        s.gps_power = byte(GPS_POWERS.len()).and_then(|v| GPS_POWERS.get(v).copied()).unwrap_or(d.gps_power);
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
    /// The file was read whole: written over in place (as `PageStore::sound`).
    pub(crate) sound: bool,
    /// FAT sectors compared so far.
    pub(crate) sync: u32,
}

impl SettingsStore {
    /// As at a start: the registers read.
    pub(crate) fn at_boot(b: &mut impl Board) -> Self {
        let lost = !matches!(b.backup_read(WHEEL_BKP) >> 24, 0xa5..=0xac);
        SettingsStore { lost, ..Default::default() }
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
                (s.touch, s.ahead, s.profile) = (r.touch, r.ahead, r.profile);
            }
        }
        if changed(1) && matches!(tag(1), 0xa0..=0xbf | 0xc1) {
            took[1] = true;
            (s.weight_kg, s.wprime_j, s.wkt_avg, s.bike_hg) = (r.weight_kg, r.wprime_j, r.wkt_avg, r.bike_hg);
            if tag(1) <= 0xaf {
                (s.smooth, s.gps_power, s.profile) = (r.smooth, r.gps_power, r.profile);
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

    /// Reads `\Rustfw\settings.bin` (module doc: which wins).
    #[inline(never)] // its 1 KB buffer in its own frame
    fn load_settings(&mut self, b: &mut impl Board) {
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
        st.sound = read == Ok(1024);
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
        // the file gets the registers' fields, or words that say what the registers now hold
        st.dirty = self.card_writable && (took.contains(&true) || merged.register_words() != c.words);
        let was = self.settings;
        self.settings = Settings { alt_m: was.alt_m, ..merged };
        if self.card_writable && self.settings.register_words() != regs {
            // put right from the result (lost, or a register some fields missed); a build that may not write the
            // card leaves them as another build wrote them
            self.write_settings_registers(b);
        }
        if self.settings != was {
            self.update_light(b);
            self.gps.reconfig = true;
            if self.settings.smooth != was.smooth {
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
            smooth: true,
            gps_power: GpsPower::Saver,
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
        assert_eq!(len, 31);
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
        assert_eq!((short.ftp, short.hr_alert, short.smooth, short.profile), (265, 175, d.smooth, d.profile));
        let none = with(&|sec| sec[6] = 0).unwrap();
        assert_eq!(none, d, "nothing: the defaults");
        let long = with(&|sec| {
            sec[6] = 40;
            sec[DATA_AT + 31..DATA_AT + 40].fill(0xee);
        });
        assert_eq!(long, Some(s), "a later build's settings past the ones known are left");
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
        assert_eq!((odd.max_hr, odd.smooth), (s.max_hr, s.smooth), "the others kept");
        assert_eq!(with(&|sec| sec[4] = 2), None, "another version");
        assert_eq!(with(&|sec| sec[6..8].copy_from_slice(&481u16.to_le_bytes())), None, "longer than a copy holds");
    }
}
