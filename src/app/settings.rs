//! The settings and where they are kept: [`Settings`] (wheel, backlight, heart rate, power, alerts) and the ride
//! pages' [`PageCfg`], packed into RTC backup registers (which survive standby; BOARD.md 8: none used by stock). The
//! app's backup registers are all named here, so two features never take the same one; the paired sensors' are
//! [`sensors::Kind::backup`].
//!
//! **The file and the registers.** The settings are kept in `\Rustfw\settings.bin` (app/settings_file.rs), which
//! wins over the registers once it is read; a new setting goes there only. Registers 11, 16 and 28 and AUTO OFF's
//! bits of 18 stay, written at every change ([`App::save_misc`]) with what they held before, because:
//! - the file is read a few seconds after the start (the card is not touched at boot), and the first screen needs
//!   its backlight level and time-out (11) and its font (16) before; 11's tag also tells the first start that this
//!   firmware ran here ([`App::first_start`]);
//! - the file is written between rides only: a change made during a ride, with the USB drive lent, or by a build that
//!   may not write the card lives in the registers until then, and survives a reset or a switch-off there (they
//!   win over the file where they differ from what it was written with);
//! - the words are full, so a register freed would give a single word back, and older builds read these.
//!
//! A setting added from now on lives in the file only: lost to a reset only if it was changed while the file had to
//! wait.

use crate::*;
use page::Page;

/// A ride page: its layout (an id in [`ui::LAYOUTS`], 0: off) and what each field shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCfg {
    /// Layout id ([`ui::layout`]).
    pub layout: u8,
    /// The fields, the first `layout` of them used.
    pub fields: [Field; 6],
}

impl PageCfg {
    /// As kept in two backup registers, behind tags 0xd and 0xe in their top nibbles: the layout in bits 24..27 of
    /// the first, then a byte per field, three in each word. (Builds of 2026-10-23 used tag 0xb in both, from register
    /// 19: [`PageCfg::from_words_b`].)
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, ui::Field};
    /// let p = PageCfg { layout: 6, fields: [Field::Power, Field::Hr, Field::Temp, Field::Map, Field::Dist, Field::Time] };
    /// assert_eq!(PageCfg::from_words(p.to_words()), Some(p));
    /// assert_eq!(PageCfg::from_words([0, 0]), None);
    /// ```
    pub fn to_words(self) -> [u32; 2] {
        let bytes = |f: &[Field]| f.iter().enumerate().fold(0, |w, (i, &f)| w | (f as u32) << (8 * i));
        let (a, b) = self.fields.split_at(3);
        [(0xd << 28) | (u32::from(self.layout) << 24) | bytes(a), (0xe << 28) | bytes(b)]
    }

    /// The page kept in `w`, if [`PageCfg::to_words`] wrote it.
    pub fn from_words(w: [u32; 2]) -> Option<Self> {
        Self::tagged(w, [0xd, 0xe])
    }

    /// A page as builds of 2026-10-23 kept it (tag 0xb in both words).
    pub fn from_words_b(w: [u32; 2]) -> Option<Self> {
        Self::tagged(w, [0xb, 0xb])
    }

    fn tagged(w: [u32; 2], tags: [u32; 2]) -> Option<Self> {
        let layout = (w[0] >> 24 & 0xf) as u8;
        if w[0] >> 28 != tags[0] || w[1] >> 28 != tags[1] || ui::layout(layout).is_none() {
            return None;
        }
        Some(PageCfg { layout, fields: core::array::from_fn(|i| Field::from_index(w[i / 3] >> (8 * (i % 3)) & 0xff)) })
    }

    /// A page as earlier builds kept it in one register: tag 0xa, the layout, 4 bits per field.
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, ui::Field};
    /// let p = PageCfg::from_word(0xa300_0021).unwrap();
    /// assert_eq!((p.layout, &p.fields[..3]), (3, &[Field::Hr, Field::Speed, Field::Time][..]));
    /// ```
    pub fn from_word(w: u32) -> Option<Self> {
        let layout = (w >> 24 & 0xf) as u8;
        if w >> 28 != 0xa || ui::layout(layout).is_none() {
            return None;
        }
        Some(PageCfg { layout, fields: core::array::from_fn(|i| Field::from_index(w >> (4 * i) & 0xf)) })
    }
}

/// Ride pages at most (UI review 2, 5.2 point 11: six is six presses round with the Page key). They are kept in
/// `\Rustfw\pages.bin` (app/editor.rs), the first [`REG_PAGES`] also in backup registers.
pub const PAGES: usize = 6;

/// The ride pages the backup registers keep: the fallback when the file is missing or bad, and what earlier builds
/// (and read-only ones, which do not write the card) read.
pub const REG_PAGES: usize = 3;

/// The overview's fields are kept as page [`OVERVIEW`] (four fields, never off), after the ride pages.
pub const OVERVIEW: usize = PAGES;

/// [`Screen::Ride`]'s number for the course page (its fields fixed: the profile, the distances, time and speed),
/// after the ride pages in their round while a course is followed.
pub const COURSE: usize = PAGES + 1;

/// [`Screen::Ride`]'s number for the workout page (fixed like the course page), first in the round while a workout is
/// loaded.
pub const WORKOUT: usize = PAGES + 2;

/// [`Screen::Ride`]'s number for the climb page, in the round right after the page the rider was on when a climb's
/// warning fired, until its top is passed (`App::on_climb`); not stored, the rider's sets are unchanged.
pub const CLIMB: usize = PAGES + 3;

// The RTC backup registers (32 words, kept in standby and through a reset; BOARD.md 8 has stock's side), all named
// here, each word behind a tag so an empty or foreign one reads as unset:
//
//   reg      owner                                                  name
//   0        diagnostics: USB drive at a drop (0xd1), standby (0x5b) USB_LAST_BKP = STANDBY_BKP
//   1..4     stock (clock set, clock, USB mode)                     -
//   5..8, 10 paired ANT+ sensors (tag 0xa5)                         sensors::Kind::backup
//   9, 12,13 stock (12: its Nordic image; 13: last battery mV)      -
//   11       wheel, backlight, max HR, FTP (also in settings.bin)   WHEEL_BKP
//   14       paired shifting system (0x61 / 0x62)                   sensors::Kind::backup
//   15       stock reads it at boot                                 -
//   16       weights and W' (0xa; also in settings.bin)             POWER_BKP
//   17, 18   GNSS aiding                                            AID_BKP
//   19       stock reads it at boot (once ours: set back to 0)      STOCK_BKP19
//   20..27   ride pages and the overview (0xd / 0xe)                PAGE_BKP
//   28       alerts and the keys' actions (0x9; also settings.bin)  ALERTS_BKP
//   29       fault marker (0xfa)                                    FAULT_BKP
//   30       a ride open on the card                                RIDE_BKP
//   31       diagnostics: USB drops (0x0b), reset flags (0x5c)      USB_DROPS_BKP = STANDBY_FLAGS_BKP
//
// None is free, so the two diagnostics share 0 and 31: the last writer wins and its tag says whose word it is.

/// RTC backup register counting the USB drops the link recovered from (the board's SOF watch): tag 0x0b, the count
/// in the low 16 bits; shared with [`STANDBY_FLAGS_BKP`] (its tag 0x5c restarts the count).
pub const USB_DROPS_BKP: usize = 31;

/// RTC backup register with what the USB drive saw last at a drop ([`App::usb_dropped`]): tag 0xd1; shared with
/// [`STANDBY_BKP`].
pub const USB_LAST_BKP: usize = 0;

/// The standby breadcrumbs (main.rs `standby`: the power button did not start the unit after switching off,
/// 2026-10-25): tag 0x5b in bits 24..31, the boots since standby was entered in bits 16..23, the times WFI returned
/// in standby in bits 0..15. Shared with [`USB_LAST_BKP`].
pub const STANDBY_BKP: usize = USB_LAST_BKP;

/// The reset and standby flags at the first boot after standby ([`standby_flags_word`]); shared with
/// [`USB_DROPS_BKP`].
pub const STANDBY_FLAGS_BKP: usize = USB_DROPS_BKP;

/// [`STANDBY_FLAGS_BKP`]'s word: tag 0x5c in bits 24..31, RCC_RSR's flags (bits 17..30) in bits 10..23 and
/// PWR_CPUCR's low 10 bits (its stop and standby flags) in bits 0..9. Tagged, so the USB drop count never takes it
/// for its own (raw, the flags' top byte could read as its tag 0x0b).
///
/// ```
/// let w = bryton_rustfw::standby_flags_word(0x0b02_0000, 0x1c0);
/// assert_eq!((w >> 24, w >> 10 & 0x3fff, w & 0x3ff), (0x5c, 0x0b02_0000 >> 17, 0x1c0));
/// ```
pub fn standby_flags_word(rsr: u32, cpucr: u32) -> u32 {
    0x5c00_0000 | (rsr >> 7 & 0x00ff_fc00) | (cpucr & 0x3ff)
}

/// The page the editor works on (its copy, kept or dropped when it is left): the field picker's page number then.
pub const EDITED: usize = PAGES + 2;

/// The backup registers of the first ride pages and the overview, two each ([`PageCfg::to_words`]).
/// Registers 20 to 31 are not referenced by stock (R039; ghidra/HARDWARE.md, RTC and backup registers), 19 is: its
/// boot reads it (AGENTS.md, "Share the storage with stock").
pub(crate) const PAGE_BKP: [[usize; 2]; REG_PAGES + 1] = [[20, 21], [22, 23], [24, 25], [26, 27]];

/// Set in the first ride page's second word (register 21, bits 24..27, which [`PageCfg::from_words`] and every
/// earlier build leave alone) while the registers hold page changes `\Rustfw\pages.bin` does not have yet: a
/// change made during a ride, in the editor or with the drive lent, then a switch-off or a reset before the file
/// could be written (app/editor.rs).
pub(crate) const PAGES_NEWER: u32 = 1 << 24;

/// Where builds of 2026-10-23 kept them (from 19, which stock reads: moved, and 19 given back).
pub(crate) const PAGE_BKP_B: [[usize; 2]; REG_PAGES + 1] = [[19, 20], [21, 22], [23, 24], [25, 26]];

/// The backup register stock reads at boot (with 15: whether to keep its last battery voltage); stock's own first
/// boot writes 0 there.
pub(crate) const STOCK_BKP19: usize = 19;

/// Where earlier builds kept the ride pages, one register each ([`PageCfg::from_word`]): read when the new ones
/// are empty. Their page 3, in 16, is left out: 16 is [`POWER_BKP`] now, whose words carry the same tag.
pub(crate) const OLD_PAGE_BKP: [usize; 2] = [14, 15];

/// The backup register of the wheel size, the backlight, max heart rate and FTP: see [`Settings`].
pub(crate) const WHEEL_BKP: usize = 11;

/// The backup register of the rider's weight and W': see [`Settings::power_word`]. Not 12, which looks unused but
/// holds stock's Nordic image number (BOARD.md 8, 11): 16 is ours. The first builds kept page 3 there, but it is
/// never read as a page now ([`OLD_PAGE_BKP`]): the power word's tag is 0xa too, [`PageCfg::from_word`]'s, and its
/// bit 24 (the smooth font) would read as a layout.
pub(crate) const POWER_BKP: usize = 16;

/// The backup register of the fault marker (hw.rs default_handler): 0xfa in the top byte, the faults in a row in
/// the low byte; said at start, cleared after [`FAULT_CLEAR_MS`] of running.
pub const FAULT_BKP: usize = 29;

/// Running this long after a fault clears its count (a fault after it is the first in a row again).
pub const FAULT_CLEAR_MS: u32 = 60_000;

/// The backup register of the alerts ([`Settings::alerts_word`]); not referenced by stock (BOARD.md 8).
pub(crate) const ALERTS_BKP: usize = 28;

/// The backup register that says a ride is open on the card (storage/recorder/mod.rs): [`RIDE_OPEN`] from its start until it
/// is saved, so the boot looks for it; not referenced by stock (BOARD.md 8). Lost with the backup domain (a flat
/// battery): the next Start then finds the ride from its record.
pub(crate) const RIDE_BKP: usize = 30;

/// See [`RIDE_BKP`].
pub(crate) const RIDE_OPEN: u32 = 0x5243_0001;

/// The small settings kept in RTC backup register 11, as one word behind a tag byte:
/// - tag 0xa7 (this layout; 0xa8 and 0xa9 the same with the ride profile fixed, [`Settings::profile`]; 0xaa to 0xac the
///   same three with climbs ahead off, [`Settings::ahead`]): bits 0..2 the wheel's index in [`WHEELS`], 3..5 the backlight level (an index in
///   [`LIGHT_LEVELS`], or [`LIGHT_AUTO`]), 6..7 its time-out (an index in [`LIGHT_TIMEOUTS_S`]), 8..15 max heart
///   rate less 100, 16..22 FTP in 5 W (at most 120), 23 the touch panel off ([`Settings::touch`]: 0 in words written
///   before it, on);
/// - tag 0xa6 (earlier builds): the wheel's index in bits 0..2, max heart rate and FTP as 4-bit codes in 4..7 and
///   8..11 ([`code_hr`], [`code_ftp`]), the backlight level in 16..18 and its time-out in 20..21; read only;
/// - tag 0xa5 (written by earlier builds): the wheel in mm in bits 0..11, the backlight level in 16..19 and its
///   time-out in 20..23; read, never written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Wheel circumference, mm (one of [`WHEELS`]).
    pub wheel_mm: u16,
    /// Max heart rate, bpm (in [`menu::MAX_HR_RANGE`]).
    pub max_hr: u16,
    /// FTP, W (in [`menu::FTP_RANGE`], a multiple of 5).
    pub ftp: u16,
    /// Backlight level: an index in [`LIGHT_LEVELS`], or [`LIGHT_AUTO`].
    pub light_level: usize,
    /// Backlight time-out: an index in [`LIGHT_TIMEOUTS_S`].
    pub light_timeout: usize,
    /// AUTO OFF: an index in [`AUTO_OFF_MIN`] (idle on the overview, not riding, no cable).
    pub auto_off: usize,
    /// Rider weight, kg (in [`menu::WEIGHT_RANGE`]); kept in register 16 ([`Settings::power_word`]).
    pub weight_kg: u16,
    /// W', J (in [`menu::WPRIME_RANGE`], a multiple of 500); kept in register 16.
    pub wprime_j: u16,
    /// Bike weight with what it carries, in 100 g (in [`menu::BIKE_RANGE`]); kept in register 16; the climb pacer adds
    /// it to the rider's, and the trainer is told it.
    pub bike_hg: u16,
    /// Eat and drink reminders, minutes of ride time (0: off), and the heart rate alert, bpm (0: off); kept in
    /// register 28 ([`Settings::alerts_word`]).
    pub eat_min: u16,
    /// See `eat_min`.
    pub drink_min: u16,
    /// See `eat_min`.
    pub hr_alert: u16,
    /// The power average window every 3 / 10 / 30 s field, the workout gauge and the climb pacer show (a tap on one of the
    /// fields cycles it): an index in [`WKT_AVGS`]; kept in register 16.
    pub wkt_avg: usize,
    /// What each shifter button press does, by [`sensors::Press`]: 3 bits each, the single presses in register 16
    /// (bits 18..23), the long ones in register 28 (bits 5..7 and 13..15), the bits those words left free. 0, in
    /// words written before: nothing.
    // split over two words because no backup register is free (BOARD.md 8); in settings.bin, four bytes in a row
    pub remote: [menu::Remote; 4],
    /// The touch panel acts (the TOUCH setting); off, it is not read for gestures and only the keys act. Kept in
    /// register 11, bit 23.
    pub touch: bool,
    /// The altitude being edited (ALTITUDE in SYSTEM > BUILT IN), m + [`ALT_MENU_OFFSET`]. Not kept: the
    /// calibration is (altitude.rs).
    pub alt_m: u16,
    /// The ride profile the rider fixed (BIKE > PROFILE); None: AUTO, indoor when a trainer is heard at Start, else
    /// road; MTB: the same with MTB for road (app/profile.rs). Kept in register 11's tag: 0xa7 for AUTO (the
    /// tag every build knows), 0xa8 ROAD, 0xa9 INDOOR (an earlier build reads those two as an empty word: defaults);
    /// MTB is 0xa7 there with register 16's bit 27 ([`Settings::with_power_word`]), so a build before MTB reads AUTO.
    pub profile: Option<ride::Profile>,
    /// Climbs found on the road ahead when no course is followed (BIKE > CLIMBS AHEAD, `climbs_ahead`). Kept in
    /// register 11's tag: 0xa7 to 0xa9 on, 0xaa to 0xac off (the same three profiles; an earlier build reads those
    /// as an empty word: defaults).
    pub ahead: bool,
    /// The smooth font for text (the FONT setting, SMOOTH) instead of the pixel font; kept in register 16, bit 24.
    pub smooth: bool,
    /// GPS POWER (BIKE): kept in register 16, bits 25 (SAVER) and 26 (FULL); neither is AUTO.
    pub gps_power: GpsPower,
}

/// GPS POWER: the receiver's power mode ([`crate::gnss::config`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpsPower {
    /// Cyclic tracking while riding fast and straight, continuous otherwise ([`crate::gnss::Adaptive`]): the default.
    #[default]
    Auto,
    /// Continuous tracking always.
    Full,
    /// Cyclic tracking always (u-blox PSMCT): about 5 mA less, at some cost in accuracy (docs/power.md).
    Saver,
}

/// The power windows the workout gauge offers, as its menu row and its title show them: 3, 10 and 30 s averages
/// (power.rs [`power::Power::rolling`] 0, 1, 2), then the power each second. 3 s first: the default.
pub const WKT_AVGS: [&str; 3] = ["3S", "10S", "30S"];

/// The power window kept in 2 bits: 3, 10, 30 s as 0, 1, 2; an old 3 (NOW, each second) reads as 3 s.
fn window(bits: u32) -> usize {
    (bits & 3) as usize % 3
}

impl Settings {
    /// The settings in `w` (defaults for an empty or foreign word).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, WHEELS};
    /// let m = Settings { wheel_mm: WHEELS[3], max_hr: 187, ftp: 600, light_level: 4, light_timeout: 3, touch: false, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(m.to_word()), m);
    /// // words from earlier builds: 2136 mm, AUTO, 60 s; then max HR code 7 (225), FTP code 11 (100)
    /// let old = Settings::from_word(0xa5 << 24 | 3 << 20 | 4 << 16 | 2136);
    /// assert_eq!((old.wheel_mm, old.light_level, old.light_timeout, old.max_hr), (2136, 4, 3, 190));
    /// let old = Settings::from_word(0xa6 << 24 | 11 << 8 | 7 << 4 | 3);
    /// assert_eq!((old.wheel_mm, old.max_hr, old.ftp), (WHEELS[3], 225, 100));
    /// ```
    pub fn from_word(w: u32) -> Settings {
        let wheel = |mm: u16| if WHEELS.contains(&mm) { mm } else { WHEELS[1] };
        let level = |l: u32| (l as usize) % (LIGHT_LEVELS.len() + 1);
        let index = |i: u32| WHEELS.get(i as usize).copied().unwrap_or(WHEELS[1]);
        // what a word that carries none of these keeps, and every tag starts from; the tags set what they hold
        let base = Settings {
            wheel_mm: WHEELS[1],
            max_hr: ride::MAX_HR,
            ftp: ride::FTP,
            // a fresh unit (no saved word): AUTO, off after 30 s, about 60 mA less than 100 % never off
            // (docs/power.md, the owner's choice of 2026-10-27); a saved word keeps its own
            light_level: LIGHT_AUTO,
            light_timeout: 2,
            auto_off: 1,
            weight_kg: power::WEIGHT_KG,
            wprime_j: power::WPRIME_J,
            bike_hg: power::BIKE_HG,
            eat_min: 0,
            drink_min: 0,
            hr_alert: 0,
            wkt_avg: 0,
            touch: true,
            remote: [menu::Remote::None; 4],
            alt_m: 500,
            profile: None,
            ahead: true,
            smooth: false,
            gps_power: GpsPower::Auto,
        };
        match w >> 24 {
            0xa7..=0xac => Settings {
                profile: match (w >> 24) % 3 {
                    // 0xa8, 0xab: ROAD; 0xa9, 0xac: INDOOR; 0xa7, 0xaa: AUTO
                    0 => Some(ride::Profile::Road),
                    1 => Some(ride::Profile::Indoor),
                    _ => None,
                },
                ahead: w >> 24 < 0xaa,
                wheel_mm: index(w & 7),
                max_hr: (100 + (w >> 8 & 0xff) as u16).clamp(*menu::MAX_HR_RANGE.start(), *menu::MAX_HR_RANGE.end()),
                ftp: (5 * (w >> 16 & 0x7f) as u16).clamp(*menu::FTP_RANGE.start(), *menu::FTP_RANGE.end()),
                touch: w >> 23 & 1 == 0,
                light_level: level(w >> 3 & 7),
                light_timeout: (w >> 6 & 3) as usize,
                ..base
            },
            0xa6 => Settings {
                wheel_mm: index(w & 7),
                max_hr: code_hr(w >> 4 & 0xf),
                ftp: code_ftp(w >> 8 & 0xf),
                light_level: level(w >> 16 & 7),
                light_timeout: (w >> 20 & 3) as usize,
                ..base
            },
            0xa5 => Settings {
                wheel_mm: wheel((w & 0xfff) as u16),
                light_level: level(w >> 16 & 0xf),
                light_timeout: (w >> 20 & 0xf) as usize % LIGHT_TIMEOUTS_S.len(),
                ..base
            },
            _ => base,
        }
    }

    /// The alerts and the keys' actions kept in register 28: tag 0x9 in bits 28..31, eat and drink reminders in 5 min
    /// steps in bits 0..4 and 8..12, the long presses' actions in 5..7 and 13..15, the heart rate alert in 5 bpm steps
    /// in 16..21 (0: off for each), the single presses' actions in 22..24 and 25..27. Earlier builds' tag 0xc2 (top
    /// byte) is read too: the heart rate in bpm in 16..23, the single presses then in register 16.
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let s = Settings { eat_min: 45, drink_min: 20, hr_alert: 175, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(0).with_alerts_word(s.alerts_word()), s);
    /// assert_eq!(Settings::from_word(0).with_alerts_word(0).eat_min, 0, "off without the tag");
    /// ```
    pub fn alerts_word(self) -> u32 {
        let [l1, r1, l, r] = self.remote.map(|a| a as u32);
        (0x9 << 28)
            | r1 << 25
            | l1 << 22
            | (u32::from(self.hr_alert / 5) & 0x3f) << 16
            | r << 13
            | (u32::from(self.drink_min / 5) & 0x1f) << 8
            | l << 5
            | (u32::from(self.eat_min / 5) & 0x1f)
    }

    /// Self with the alerts of register 28 ([`Settings::alerts_word`]), and the long presses' actions.
    ///
    /// ```
    /// use bryton_rustfw::{Settings, menu::Remote};
    /// let s = Settings { eat_min: 120, remote: [Remote::Lap, Remote::Play, Remote::PrevTrack, Remote::Start], ..Settings::from_word(0) };
    /// let back = Settings::from_word(0).with_power_word(s.power_word()).with_alerts_word(s.alerts_word());
    /// assert_eq!((back.remote, back.eat_min), (s.remote, 120));
    /// ```
    pub fn with_alerts_word(self, w: u32) -> Settings {
        let [s0, s1, ..] = self.remote;
        let min = |v: u32| ((v & 0x1f) as u16 * 5).min(120);
        let long = [menu::Remote::from_bits(w >> 5), menu::Remote::from_bits(w >> 13)];
        match w >> 28 {
            0x9 => Settings {
                eat_min: min(w),
                drink_min: min(w >> 8),
                hr_alert: ((w >> 16 & 0x3f) as u16 * 5).min(230),
                remote: [menu::Remote::from_bits(w >> 22), menu::Remote::from_bits(w >> 25), long[0], long[1]],
                ..self
            },
            // earlier builds (tag 0xc2): the heart rate in bpm, the single presses in register 16
            0xc if w >> 24 == 0xc2 => Settings {
                eat_min: min(w),
                drink_min: min(w >> 8),
                hr_alert: (w >> 16 & 0xff) as u16,
                remote: [s0, s1, long[0], long[1]],
                ..self
            },
            _ => {
                let remote = [s0, s1, menu::Remote::None, menu::Remote::None];
                Settings { eat_min: 0, drink_min: 0, hr_alert: 0, remote, ..self }
            }
        }
    }

    /// These settings with the weight, W', the bike's weight, the workout gauge's window and the single presses' actions
    /// kept in `w` (RTC backup register 16). Tag 0xa in bits 28..31 (this layout), then the rider's weight less 30 kg in
    /// 0..6, W' in 0.5 kJ in 7..13, the gauge's window in 14..15, the bike's weight in 100 g in 16..23 (0:
    /// [`power::BIKE_HG`]), the smooth font in bit 24 (the FONT setting; clear in earlier words: PIXEL), GPS POWER in
    /// bits 25..26 (1 SAVER, 2 FULL, 0 AUTO; earlier words had only bit 25, SAVER, clear for their default FULL, now
    /// read as AUTO), BIKE > PROFILE MTB in bit 27 (earlier builds take bits 24..27 as part of the tag 0xa0 to 0xaf
    /// and ignore it); the single presses' actions are in register 28 ([`Settings::alerts_word`]). Read only,
    /// from earlier builds: tag 0xb in bits 28..31, the same up to the window, the bike's kg in 16..20, the single
    /// presses' actions in 21..23 and 24..26; tag 0xc1 in bits 24..31, the weight in kg in 0..7, W' in 0.5 kJ in
    /// 8..15, the window in 16..17, the actions in 18..23, no bike weight.
    /// Without either tag, the defaults ([`power::WEIGHT_KG`], [`power::WPRIME_J`], [`power::BIKE_HG`], 3 s).
    ///
    /// ```
    /// use bryton_rustfw::{GpsPower, Settings};
    /// let s = Settings { weight_kg: 68, wprime_j: 23_500, wkt_avg: 2, bike_hg: 65, gps_power: GpsPower::Full, ..Settings::from_word(0) };
    /// assert_eq!(Settings::from_word(0).with_power_word(s.power_word()), s);
    /// assert_eq!(Settings::from_word(0).with_power_word(0).wprime_j, 20_000);
    /// // MTB: register 11 says AUTO (what a build before MTB reads), register 16 says MTB
    /// let m = Settings { profile: Some(bryton_rustfw::ride::Profile::Mtb), ..Settings::from_word(0) };
    /// assert_eq!(m.to_word() >> 24, 0xa7);
    /// assert_eq!(Settings::from_word(m.to_word()).with_power_word(m.power_word()), m);
    /// // an earlier build's word: its weight and W', the bike at the default
    /// let old = Settings::from_word(0).with_power_word(0xc1_03_2e_46);
    /// assert_eq!((old.weight_kg, old.wprime_j, old.bike_hg, old.wkt_avg), (70, 23_000, 90, 0), "an old NOW reads 3 s");
    /// ```
    pub fn with_power_word(self, w: u32) -> Settings {
        let [s0, s1, s2, s3] = self.remote;
        let (kg, wp, bike) = (menu::WEIGHT_RANGE, menu::WPRIME_RANGE, menu::BIKE_RANGE);
        match w >> 24 {
            0xa0..=0xaf => Settings {
                weight_kg: ((w & 0x7f) as u16 + 30).clamp(*kg.start(), *kg.end()),
                wprime_j: (500 * (w >> 7 & 0x7f) as u16).clamp(*wp.start(), *wp.end()),
                wkt_avg: window(w >> 14),
                smooth: w >> 24 & 1 != 0,
                gps_power: match w >> 25 & 3 {
                    1 => GpsPower::Saver,
                    2 => GpsPower::Full,
                    _ => GpsPower::Auto, // and words before AUTO, whose FULL was the default: the new default
                },
                profile: if w >> 27 & 1 != 0 { Some(ride::Profile::Mtb) } else { self.profile },
                bike_hg: match (w >> 16 & 0xff) as u16 {
                    0 => power::BIKE_HG,
                    b => b.clamp(*bike.start(), *bike.end()),
                },
                remote: [s0, s1, s2, s3],
                ..self
            },
            0xb0..=0xbf => Settings {
                weight_kg: ((w & 0x7f) as u16 + 30).clamp(*kg.start(), *kg.end()),
                wprime_j: (500 * (w >> 7 & 0x7f) as u16).clamp(*wp.start(), *wp.end()),
                wkt_avg: window(w >> 14),
                bike_hg: match (w >> 16 & 0x1f) as u16 {
                    0 => power::BIKE_HG,
                    b => (b * 10).clamp(*bike.start(), *bike.end()),
                },
                remote: [menu::Remote::from_bits(w >> 21), menu::Remote::from_bits(w >> 24), s2, s3],
                ..self
            },
            0xc1 => Settings {
                weight_kg: ((w & 0xff) as u16).clamp(*kg.start(), *kg.end()),
                wprime_j: (500 * (w >> 8 & 0xff) as u16).clamp(*wp.start(), *wp.end()),
                wkt_avg: window(w >> 16),
                bike_hg: power::BIKE_HG,
                remote: [menu::Remote::from_bits(w >> 18), menu::Remote::from_bits(w >> 21), s2, s3],
                ..self
            },
            _ => {
                let remote = [menu::Remote::None, menu::Remote::None, s2, s3];
                let (weight_kg, wprime_j, bike_hg) = (power::WEIGHT_KG, power::WPRIME_J, power::BIKE_HG);
                Settings {
                    weight_kg,
                    wprime_j,
                    bike_hg,
                    wkt_avg: 0,
                    remote,
                    smooth: false,
                    gps_power: GpsPower::Auto,
                    ..self
                }
            }
        }
    }

    /// Self with AUTO OFF from register 18 ([`AUTO_OFF_MIN`]): anything out of range, or unset, is the default.
    pub fn with_auto_off_word(self, w: u32) -> Settings {
        // only a word the aiding wrote (its tag 0xa or 0x5): an older build rewriting the register leaves zeros
        match (w >> 22 & 7) as usize {
            _ if !matches!(w >> 28, 0xa | 0x5) => self,
            i @ 1..=5 => Settings { auto_off: i - 1, ..self },
            _ => self,
        }
    }

    /// The weights, W', the gauge's window and the single presses' actions as kept in register 16
    /// ([`Settings::with_power_word`]).
    pub fn power_word(self) -> u32 {
        (0xa << 28)
            | (match self.gps_power {
                GpsPower::Auto => 0,
                GpsPower::Saver => 1,
                GpsPower::Full => 2,
            }) << 25
            | u32::from(self.profile == Some(ride::Profile::Mtb)) << 27
            | u32::from(self.smooth) << 24
            | (u32::from(self.bike_hg) & 0xff) << 16
            | (self.wkt_avg as u32 & 3) << 14
            | (u32::from(self.wprime_j / 500) & 0x7f) << 7
            | u32::from(self.weight_kg.saturating_sub(30)) & 0x7f
    }

    /// As kept (tag 0xa7, or 0xa8 / 0xa9 for a profile fixed by hand).
    pub fn to_word(self) -> u32 {
        let wheel = WHEELS.iter().position(|&w| w == self.wheel_mm).unwrap_or(1) as u32;
        (match self.profile {
            None | Some(ride::Profile::Mtb) => 0xa7, // MTB in register 16
            Some(ride::Profile::Road) => 0xa8,
            Some(ride::Profile::Indoor) => 0xa9,
        } + 3 * u32::from(!self.ahead)) << 24
            | u32::from(!self.touch) << 23 // ponytail: the bit FTP / 5 (120 at most) leaves free in a full word
            | (u32::from(self.ftp / 5) & 0x7f) << 16
            | (u32::from(self.max_hr.saturating_sub(100)) & 0xff) << 8
            | (self.light_timeout as u32 & 3) << 6
            | (self.light_level as u32 & 7) << 3
            | wheel
    }
}

impl Settings {
    /// Setting `s`'s value as its menu row shows it.
    pub fn text(&self, s: Setting, l: &mut Line<12>) {
        match s {
            Setting::Wheel => l.push_u32(u32::from(self.wheel_mm)),
            Setting::Light => match LIGHT_LEVELS.get(self.light_level) {
                Some(&v) => l.push_u32(u32::from(v)),
                None => l.push_str("AUTO"),
            },
            Setting::Touch => l.push_str(if self.touch { "ON" } else { "OFF" }),
            Setting::ClimbsAhead => l.push_str(if self.ahead { "ON" } else { "OFF" }),
            Setting::Font => l.push_str(if self.smooth { "SMOOTH" } else { "PIXEL" }),
            Setting::GpsPower => l.push_str(match self.gps_power {
                GpsPower::Auto => "AUTO",
                GpsPower::Full => "FULL",
                GpsPower::Saver => "SAVER",
            }),
            Setting::AutoOff => match AUTO_OFF_MIN.get(self.auto_off) {
                Some(&m) if m > 0 => {
                    l.push_u32(m);
                    l.push_str(" MIN");
                }
                _ => l.push_str("OFF"),
            },
            Setting::LightOff => match LIGHT_TIMEOUTS_S.get(self.light_timeout) {
                Some(0) | None => l.push_str("NEVER"),
                Some(&t) => {
                    l.push_u32(t);
                    l.push_str(" S");
                }
            },
            Setting::MaxHr => l.push_u32(u32::from(self.max_hr)),
            Setting::Ftp => l.push_u32(u32::from(self.ftp)),
            Setting::Weight => l.push_u32(u32::from(self.weight_kg)),
            Setting::BikeWeight => {
                l.push_u32(u32::from(self.bike_hg / 10));
                l.push_str(".");
                l.push_u32(u32::from(self.bike_hg % 10));
            }
            Setting::WPrime => {
                l.push_u32(u32::from(self.wprime_j / 1000));
                l.push_str(".");
                l.push_u32(u32::from(self.wprime_j % 1000 / 100));
            }
            Setting::Eat | Setting::Drink | Setting::HrAlert => {
                let v = match s {
                    Setting::Eat => self.eat_min,
                    Setting::Drink => self.drink_min,
                    _ => self.hr_alert,
                };
                if v == 0 { l.push_str("OFF") } else { l.push_u32(u32::from(v)) }
            }
            Setting::Altitude => {
                let m = i32::from(self.alt_m) - ALT_MENU_OFFSET;
                if m < 0 {
                    l.push_str("-");
                }
                l.push_u32(m.unsigned_abs());
            }
            Setting::BikeProfile => l.push_str(match self.profile {
                None => "AUTO",
                Some(ride::Profile::Road) => "ROAD",
                Some(ride::Profile::Indoor) => "INDOOR",
                Some(ride::Profile::Mtb) => "MTB",
            }),
            Setting::Remote(n) => l.push_str(self.remote.get(usize::from(n)).map_or("", |a| a.name())),
        }
    }

    /// A choice to its next value (round), as Select does.
    ///
    /// ```
    /// use bryton_rustfw::{Settings, LIGHT_AUTO, LIGHT_LEVELS, menu::Setting};
    /// let mut s = Settings::from_word(0); // a fresh unit: AUTO
    /// for _ in 0..LIGHT_LEVELS.len() + 1 {
    ///     s.cycle(Setting::Light);
    /// }
    /// assert_eq!(s.light_level, LIGHT_AUTO, "round");
    /// ```
    pub fn cycle(&mut self, s: Setting) {
        match s {
            Setting::Wheel => {
                let i = WHEELS.iter().position(|&w| w == self.wheel_mm).map_or(0, |i| (i + 1) % WHEELS.len());
                self.wheel_mm = WHEELS.get(i).copied().unwrap_or(WHEELS[1]);
            }
            Setting::Light => self.light_level = (self.light_level + 1) % (LIGHT_LEVELS.len() + 1),
            Setting::LightOff => self.light_timeout = (self.light_timeout + 1) % LIGHT_TIMEOUTS_S.len(),
            Setting::AutoOff => self.auto_off = (self.auto_off + 1) % AUTO_OFF_MIN.len(),
            Setting::Touch => self.touch = !self.touch,
            Setting::ClimbsAhead => self.ahead = !self.ahead,
            Setting::Font => self.smooth = !self.smooth,
            Setting::GpsPower => {
                self.gps_power = match self.gps_power {
                    GpsPower::Auto => GpsPower::Full,
                    GpsPower::Full => GpsPower::Saver,
                    GpsPower::Saver => GpsPower::Auto,
                }
            }
            Setting::BikeProfile => {
                self.profile = match self.profile {
                    None => Some(ride::Profile::Road),
                    Some(ride::Profile::Road) => Some(ride::Profile::Indoor),
                    Some(ride::Profile::Indoor) => Some(ride::Profile::Mtb),
                    Some(ride::Profile::Mtb) => None,
                }
            }
            Setting::Remote(n) => {
                if let Some(a) = self.remote.get_mut(usize::from(n)) {
                    *a = menu::Remote::from_bits(*a as u32 + 1);
                }
            }
            _ => {} // numbers: Setting::number
        }
    }

    /// A number one step up or down, held in its range ([`Setting::number`]).
    ///
    /// ```
    /// use bryton_rustfw::{Settings, menu::Setting};
    /// let mut s = Settings::from_word(0);
    /// s.step(Setting::Ftp, true);
    /// s.step(Setting::MaxHr, false);
    /// assert_eq!((s.ftp, s.max_hr), (205, 189));
    /// ```
    pub fn step(&mut self, s: Setting, up: bool) {
        let (Some((range, step)), Some(v)) = (s.number(), self.number_mut(s)) else { return };
        *v = if up { v.saturating_add(step) } else { v.saturating_sub(step) }.clamp(*range.start(), *range.end());
    }

    pub(crate) fn number_mut(&mut self, s: Setting) -> Option<&mut u16> {
        match s {
            Setting::MaxHr => Some(&mut self.max_hr),
            Setting::Ftp => Some(&mut self.ftp),
            Setting::Weight => Some(&mut self.weight_kg),
            Setting::BikeWeight => Some(&mut self.bike_hg),
            Setting::WPrime => Some(&mut self.wprime_j),
            Setting::Eat => Some(&mut self.eat_min),
            Setting::Drink => Some(&mut self.drink_min),
            Setting::HrAlert => Some(&mut self.hr_alert),
            Setting::Altitude => Some(&mut self.alt_m),
            _ => None,
        }
    }
}

/// Max heart rate from its 4-bit code in a tag 0xa6 word: 150 to 225 in steps of 5, 0 being stock's default, 190
/// ([`ride::MAX_HR`]).
///
/// ```
/// use bryton_rustfw::code_hr;
/// assert_eq!([0, 7, 8, 15].map(code_hr), [190, 225, 150, 185]);
/// ```
pub fn code_hr(bits: u32) -> u16 {
    150 + 5 * ((bits + 8) % 16) as u16
}

/// FTP from its 4-bit code in a tag 0xa6 word: 100 to 400 W in steps of 20, 0 being [`ride::FTP`] (200 W).
///
/// ```
/// use bryton_rustfw::code_ftp;
/// assert_eq!([0, 10, 11, 15].map(code_ftp), [200, 400, 100, 180]);
/// ```
pub fn code_ftp(bits: u32) -> u16 {
    100 + 20 * ((bits + 5) % 16) as u16
}

/// Backlight levels offered, percent (stock: 1 to 100). One more choice follows them: [`LIGHT_AUTO`].
pub const LIGHT_LEVELS: [u8; 4] = [100, 60, 30, 10];

/// The backlight setting that follows the ambient light ([`ambient::Auto`]): the index after [`LIGHT_LEVELS`].
pub const LIGHT_AUTO: usize = LIGHT_LEVELS.len();

/// Backlight time-outs offered, seconds without a key press (0: never off).
pub const LIGHT_TIMEOUTS_S: [u32; 4] = [0, 10, 30, 60];

/// AUTO OFF choices, minutes of quiet before the unit switches itself off (0: never). Kept in bits 22..24 of
/// register 18 (`AID_BKP`'s second word, whose bits 22..27 the aiding never uses and older builds ignore) as
/// index + 1; 0 there (a fresh unit, an older build's word) reads as the default, 5 min (owner's choice,
/// 2026-10-28: 12.2 h idle on the overview cost about 41 % of the battery).
pub const AUTO_OFF_MIN: [u32; 5] = [0, 5, 10, 30, 60];

/// Wheel circumferences offered (mm): 700 x 23, 25, 28, 32, 35, 38 C and 650B x 47 (ANT+ / tyre makers' tables).
pub const WHEELS: [u16; 7] = [2096, 2105, 2136, 2155, 2168, 2180, 2079];

/// The ride pages before any are set (or imported from stock), after what Garmin's, Wahoo's and Hammerhead's bike
/// computers ship (2026-10-24: Wahoo's default stack is a data page, an elevation page and a map page with an
/// elevation profile and a few fields; Garmin's map page, when navigating, distance and time to the destination):
/// the ride's essentials, the climbing, the map whole; the overview's totals last.
pub(crate) const DEFAULT_PAGES: [PageCfg; REG_PAGES + 1] = {
    use Field::*;
    [
        PageCfg { layout: 6, fields: [Speed, Timer, Dist, AvgSpeed, Hr, Power] },
        PageCfg { layout: 5, fields: [AltGraph, Ascent, Grade, Vam, Cadence, Time] },
        PageCfg { layout: 1, fields: [Map, Time, Time, Time, Time, Time] },
        PageCfg { layout: 4, fields: [WeekKm, WeekH, TotalKm, Temp, Time, Time] },
    ]
};

/// The course page, an extra page while a course is followed ([`COURSE`]; Garmin and Wahoo add theirs the same
/// way): the course's profile large, the next turn across the width, then the owner's list: the distance to go, the
/// climb pacer ([`Field::ClimbPace`]), the moving time and the average speed. Not configurable.
pub(crate) const COURSE_PAGE: PageCfg = {
    use Field::*;
    PageCfg { layout: 12, fields: [Profile, NextTurn, ToGo, ClimbPace, Timer, AvgSpeed] }
};

/// The climb page ([`CLIMB`]): the climb's profile (60 %, with its gain, length and grade), under it the grade under
/// the wheel and the pacer's power (the ascent where no course is followed). Not configurable.
pub(crate) const CLIMB_PAGE: PageCfg = {
    use Field::*;
    PageCfg { layout: 15, fields: [ClimbProfile, Grade, ClimbPace, Time, Time, Time] }
};

/// The workout page, first in the round while a workout runs ([`WORKOUT`]): the reading the step targets in its gauge
/// (digits white, the marker says above / in / below), the whole workout's graph with the rider's place, the step's
/// time left and what the trainer is told in ERG (the adjust small), the next step with its length, and the cadence
/// (its edge bar red under 60 rpm in ERG: the resistance climbs when the rider slows). Not configurable.
pub(crate) const WORKOUT_PAGE: PageCfg = {
    use Field::*;
    PageCfg { layout: 12, fields: [WktPower, WorkoutGraph, StepTime, ErgTarget, NextStep, Cadence] }
};

/// The backup registers of the GNSS aiding (the RTC's offset from UTC, the last position).
pub(crate) const AID_BKP: [usize; 2] = [17, 18];

/// The backup registers of ride page (or overview) `n`, if it has some.
fn page_regs(n: usize) -> Option<[usize; 2]> {
    PAGE_BKP.get(if n == OVERVIEW { REG_PAGES } else { n }).copied().filter(|_| n < REG_PAGES || n == OVERVIEW)
}

/// Writes ride page (or overview) `n` to its backup registers, if it has some ([`Page::to_cfg`]).
pub(crate) fn save_page(b: &mut impl Board, n: usize, page: &Page) {
    save_cfg(b, n, page.to_cfg());
}

/// Writes `c` as ride page (or overview) `n`'s backup registers (layout 0: switched off, for a page not there).
pub(crate) fn save_cfg(b: &mut impl Board, n: usize, c: PageCfg) {
    if let Some(regs) = page_regs(n) {
        for (r, w) in regs.iter().zip(c.to_words()) {
            b.backup_write(*r, w);
        }
    }
}

/// The ride pages and the overview the backup registers keep (each format of earlier builds read too, a default
/// for one that holds none), the pages switched off left out (one at least); how many ride pages.
pub(crate) fn pages_from_registers(b: &mut impl Board) -> ([Page; PAGES + 1], usize) {
    let first = Page::from_cfg(DEFAULT_PAGES[0]).unwrap_or(BLANK);
    let mut pages = [first; PAGES + 1];
    let mut n = 0;
    for i in 0..REG_PAGES {
        if let Some(p) = Page::from_cfg(register_page(b, i)) {
            pages[n] = p;
            n += 1;
        }
    }
    pages[OVERVIEW] = Page::from_cfg(register_page(b, REG_PAGES)).unwrap_or(pages[OVERVIEW]);
    (pages, n.max(1))
}

/// Page `i` of the backup registers (`REG_PAGES`: the overview) in any format earlier builds kept, else its default.
fn register_page(b: &mut impl Board, i: usize) -> PageCfg {
    let now = PAGE_BKP[i].map(|r| b.backup_read(r));
    let then = PAGE_BKP_B[i].map(|r| b.backup_read(r));
    let first = OLD_PAGE_BKP.get(i).map(|&r| b.backup_read(r));
    PageCfg::from_words(now)
        .or_else(|| PageCfg::from_words_b(then))
        .or_else(|| first.and_then(PageCfg::from_word))
        .unwrap_or(DEFAULT_PAGES[i])
}

/// The backup registers' words of the first ride pages, as [`save_page`] writes them (a page not there: off).
pub(crate) fn register_words(pages: &[Page], count: usize) -> [u32; 2 * REG_PAGES] {
    let mut w = [0; 2 * REG_PAGES];
    for (i, out) in w.chunks_exact_mut(2).enumerate() {
        let off = PageCfg { layout: 0, fields: [Field::Time; 6] };
        let c = if i < count { pages.get(i).map_or(off, Page::to_cfg) } else { off };
        out.copy_from_slice(&c.to_words());
    }
    w
}

/// A page of one cell showing the time: what a page that cannot be read becomes.
pub const BLANK: Page = Page { rows: [(1, 1), (0, 0), (0, 0), (0, 0), (0, 0)], fields: [Field::Time; page::CELLS] };

impl Settings {
    /// The words `App::save_misc` writes to registers 11, 16 and 28, then AUTO OFF's bits of register 18 (as
    /// [`with_auto_off_word`](Settings::with_auto_off_word) reads them, shifted down): what a copy of
    /// `\Rustfw\settings.bin` keeps to know whether the registers changed since it was written.
    pub fn register_words(self) -> [u32; 4] {
        [self.to_word(), self.power_word(), self.alerts_word(), self.auto_off as u32 + 1]
    }
}

impl App {
    /// A setting changed: written to its backup registers at once, and to `\Rustfw\settings.bin` between rides
    /// (app/settings_file.rs).
    pub(crate) fn save_misc(&mut self, b: &mut impl Board) {
        self.write_settings_registers(b);
        self.settings_store.dirty |= self.card_writable;
        self.settings_store.lost = false; // the rider's change: the registers hold it, a late read must not undo it
    }

    /// Writes the settings to their backup registers.
    pub(crate) fn write_settings_registers(&self, b: &mut impl Board) {
        b.backup_write(WHEEL_BKP, self.settings.to_word());
        b.backup_write(POWER_BKP, self.settings.power_word());
        b.backup_write(ALERTS_BKP, self.settings.alerts_word());
        self.write_aid(b);
    }

    /// The aiding's two words (they change with a fix) with AUTO OFF in the second's bits 22..24.
    pub(crate) fn write_aid(&self, b: &mut impl Board) {
        let [w0, w1] = self.gps.aid.to_words();
        b.backup_write(AID_BKP[0], w0);
        b.backup_write(AID_BKP[1], w1 | (self.settings.auto_off as u32 + 1) << 22);
    }

    /// The ride pages (the first [`App::page_count`]), then the overview's fields ([`OVERVIEW`]).
    pub fn pages(&self) -> &[Page; PAGES + 1] {
        &self.pages
    }

    /// How many ride pages there are.
    pub fn page_count(&self) -> usize {
        self.page_count
    }
}
