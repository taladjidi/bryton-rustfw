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
pub const EDITED: usize = PAGES + 4;

// Each screen's page is its own index: a clash sends one screen's boxes to another's (the editor drew the workout page's).
const _: () = assert!(
    OVERVIEW != COURSE
        && OVERVIEW != WORKOUT
        && OVERVIEW != CLIMB
        && OVERVIEW != EDITED
        && COURSE != WORKOUT
        && COURSE != CLIMB
        && COURSE != EDITED
        && WORKOUT != CLIMB
        && WORKOUT != EDITED
        && CLIMB != EDITED
);

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
    /// The other limits and reminders ([`alerts::LIMITS`], 0: off); kept in the settings file only.
    pub limits: [u16; alerts::LIMITS],
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
    /// THEME (DISPLAY): the name of the colour theme, NUL padded; empty is DARK. A built-in's name, or a file's in
    /// `\Rustfw\Themes` without `.ini` ([`crate::theme`]). Kept in settings.bin only (the registers have no room).
    pub theme: [u8; 12],
    /// POWER MATCH (BIKE): ERG targets corrected so the power meter reads them ([`crate::power_match`]). Kept in
    /// settings.bin only, after THEME; ON in a file that predates it.
    pub power_match: bool,
    /// LTHR (ME): the lactate threshold heart rate, bpm; 0 until it is set (the LTHR fields show "--"). Kept in
    /// settings.bin only, after the alert limits (byte 66); 0 in a file that predates it.
    pub lthr: u16,
    /// POWER SMOOTH (BIKE): an index into [`POWER_SMOOTH_S`], the big power value's window. Kept in settings.bin only,
    /// after LTHR; 0 (the live reading) in a file that predates it.
    pub power_smooth: usize,
    /// AUTO (THEME): the colour theme follows the ambient light, DAY THEME by day and NIGHT THEME by night
    /// ([`crate::theme::DayNight`]). Kept in settings.bin only, after POWER SMOOTH; off in a file that predates it.
    pub auto_theme: bool,
    /// DAY THEME and NIGHT THEME (DISPLAY): the themes AUTO uses, named as THEME is. Kept in settings.bin only.
    pub day_theme: [u8; 12],
    /// NIGHT THEME: the theme AUTO uses in the dark.
    pub night_theme: [u8; 12],
    /// AUTO LAP (BIKE): 0 OFF, 1 BY DISTANCE, 2 BY TIME. Kept in settings.bin only (data 94, with the two after it); OFF
    /// in a file that predates it.
    pub auto_lap: usize,
    /// LAP EVERY, km (1 to 50) for BY DISTANCE; 5 in a file that predates it.
    pub lap_km: u16,
    /// LAP EVERY, minutes of moving time (1 to 60) for BY TIME; 10 in a file that predates it.
    pub lap_min: u16,
    /// LIVE SEGMENTS (SEGMENTS menu): the matcher runs during a ride. Kept in settings.bin only (data 97, with the byte
    /// after it); ON in a file that predates it.
    pub live_segments: bool,
    /// TARGET (SEGMENTS menu): the index in [`SEGMENT_TARGETS`]; 0 (AUTO) in a file that predates it.
    pub seg_target: usize,
    /// SOUND (DISPLAY): the buzzer plays the alerts, the course and workout tones and the start / pause beeps (the
    /// power-on and power-off melodies stay). Kept in settings.bin only (data 99); ON in a file that predates it.
    pub sound: bool,
    /// START REMINDER (BIKE): with no ride, the bike moving for 10 s says START THE RIDE? (data 150); ON in a file that
    /// predates it.
    pub start_reminder: bool,
    /// READY CHECK (BIKE): a new ride's start shows the GPS, sensors and battery for 3 s (data 151); ON in a file that
    /// predates it.
    pub ready_check: bool,
    /// CLIMBS (BIKE): the climbs ahead are MEDIUM+ (category 3 and harder, a score of 16000 or more) or all of them
    /// (data 152); ALL in a file that predates it.
    pub climbs_medium: bool,
    /// CLIMB PAGE (BIKE): the climb page is in the paging when on (data 153); ON in a file that predates it.
    pub climb_page: bool,
    /// DATE FORMAT (DISPLAY): how every date is drawn (data 154); D/M/Y in a file that predates it.
    pub date_fmt: DateFormat,
    /// UNITS (DISPLAY): the fields show miles, mph and feet (data 100); the FIT file stays metric. METRIC in a file
    /// that predates it.
    pub imperial: bool,
    /// TEMPERATURE (DISPLAY): the temperature field in F (data 101); C in a file that predates it.
    pub temp_f: bool,
    /// AUTO PAUSE (BIKE): an index in [`AUTO_PAUSE_KMH`] (data 102); 0, the profile's own rule, in a file that predates
    /// it.
    pub auto_pause: usize,
    /// TRIP 1 and TRIP 2: the distance (m) of the rides saved since each was reset (data 103 and 111, u32; 0 in a file
    /// that predates them), with the moving time (s) beside it (data 107 and 115).
    pub trip_m: [u32; 2],
    /// The time of each trip, s of moving time.
    pub trip_s: [u32; 2],
    /// ODOMETER: the distance (m) of every ride saved on this unit since this build had it (data 119, u32); never reset.
    pub odo_m: u32,
    /// ZONES BY (ME): the heart-rate zones come from the LTHR, not the maximum (data 123; MAX HR in a file that predates it).
    pub zones_lthr: bool,
    /// ZONES (ME): the rider's own bounds in `zone_up` are in use (data 124); the computed zones when not.
    pub zone_custom: bool,
    /// The upper bound of heart-rate zones 1 to 4 (bpm) then power zones 1 to 6 (W), u16 each (data 125..145); read
    /// only while `zone_custom` (use [`Settings::zones`]).
    pub zone_up: [u16; 10],
    /// SEX (ME): female (data 145); TRIMP's constant for hrTSS ([`crate::hrtss`]).
    pub female: bool,
    /// BIRTH YEAR (ME): 0 until set (data 146, u16).
    pub birth_year: u16,
    /// HEIGHT (ME), cm: 0 until set (data 148, a byte).
    pub height_cm: u16,
    /// MAX HR was worked out from BORN (data 149, a byte); a manual MAX HR edit clears it.
    pub max_hr_age: bool,
}

/// AUTO PAUSE's choices as the ride gets them ([`ride::Input::pause_kmh`]): 0 the profile's own rule, 255 never, else
/// the km/h under which a ride is paused.
pub const AUTO_PAUSE_KMH: [u8; 6] = [0, 255, 2, 3, 5, 8];

impl Settings {
    /// A saved ride's distance (m) and moving time (s) added to both trips and the odometer (saturating).
    pub fn add_ride(&mut self, m: u32, s: u32) {
        for (tm, ts) in self.trip_m.iter_mut().zip(self.trip_s.iter_mut()) {
            (*tm, *ts) = (tm.saturating_add(m), ts.saturating_add(s));
        }
        self.odo_m = self.odo_m.saturating_add(m);
    }

    /// Trip `n` (0 or 1) back to zero.
    pub fn reset_trip(&mut self, n: usize) {
        if let (Some(m), Some(s)) = (self.trip_m.get_mut(n), self.trip_s.get_mut(n)) {
            (*m, *s) = (0, 0);
        }
    }
}

/// TARGET's choices: AUTO is the PR if the file has one, else the KOM.
pub const SEGMENT_TARGETS: [&str; 5] = ["AUTO", "PR", "KOM", "QOM", "RECENT"];

impl Settings {
    /// The auto lap's limits for [`ride::Ride::lap_reached`]: metres and seconds, 0 for none.
    pub fn auto_lap_limits(&self) -> (u32, u32) {
        match self.auto_lap {
            1 => (u32::from(self.lap_km) * 1000, 0),
            2 => (0, u32::from(self.lap_min) * 60),
            _ => (0, 0),
        }
    }
}

/// The big power value's window, seconds, as POWER SMOOTH offers them: the live reading, then 3, 5 and 10 s.
pub const POWER_SMOOTH_S: [u32; 4] = [1, 3, 5, 10];

impl Settings {
    /// The theme's name as the menu shows it.
    pub fn theme_name(&self) -> &str {
        Self::name_of(&self.theme)
    }

    /// A theme name (THEME, DAY THEME or NIGHT THEME) as the menu shows it: DARK for the empty one.
    pub fn name_of(name: &[u8; 12]) -> &str {
        let n = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        match name.get(..n).and_then(|b| core::str::from_utf8(b).ok()) {
            Some(s) if !s.is_empty() => s,
            _ => "DARK",
        }
    }

    /// A theme name as its bytes (up to 12 characters; DARK is the empty name).
    pub fn name_bytes(name: &str) -> [u8; 12] {
        let mut out = [0u8; 12];
        if !name.eq_ignore_ascii_case("DARK") {
            for (t, b) in out.iter_mut().zip(name.bytes()) {
                *t = b;
            }
        }
        out
    }

    /// Sets THEME by name (up to 12 characters; DARK is the empty name).
    pub fn set_theme_name(&mut self, name: &str) {
        self.theme = Self::name_bytes(name);
    }
}

/// DATE FORMAT: how the dates are drawn (`fields::date_text`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DateFormat {
    /// 23/10/2026: the default.
    #[default]
    Dmy,
    /// 10/23/2026.
    Mdy,
    /// 2026-10-23.
    Ymd,
}

impl DateFormat {
    /// The next choice (the list wraps).
    pub fn next(self) -> Self {
        match self {
            DateFormat::Dmy => DateFormat::Mdy,
            DateFormat::Mdy => DateFormat::Ymd,
            DateFormat::Ymd => DateFormat::Dmy,
        }
    }

    /// The choice a settings byte names (its position in the list), None for any other byte.
    pub fn from_byte(b: u8) -> Option<Self> {
        [DateFormat::Dmy, DateFormat::Mdy, DateFormat::Ymd].get(usize::from(b)).copied()
    }
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
            limits: [0; alerts::LIMITS],
            wkt_avg: 0,
            touch: true,
            remote: [menu::Remote::None; 4],
            alt_m: 500,
            profile: None,
            ahead: true,
            smooth: false,
            gps_power: GpsPower::Auto,
            theme: [0; 12],
            power_match: true,
            lthr: 0,
            power_smooth: 0,
            auto_theme: false,
            day_theme: Settings::name_bytes("OUTDOOR"),
            night_theme: [0; 12],
            auto_lap: 0,
            lap_km: 5,
            lap_min: 10,
            live_segments: true,
            seg_target: 0,
            sound: true,
            start_reminder: true,
            ready_check: true,
            climbs_medium: false,
            climb_page: true,
            date_fmt: DateFormat::Dmy,
            imperial: false,
            temp_f: false,
            auto_pause: 0,
            trip_m: [0; 2],
            trip_s: [0; 2],
            odo_m: 0,
            zones_lthr: false,
            zone_custom: false,
            zone_up: [0; 10],
            female: false,
            birth_year: 0,
            height_cm: 0,
            max_hr_age: false,
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
                wprime_j: u16::try_from(w >> 8 & 0xff).unwrap_or(0).saturating_mul(500).clamp(*wp.start(), *wp.end()),
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
            Setting::PowerMatch => l.push_str(if self.power_match { "ON" } else { "OFF" }),
            Setting::Lthr if self.lthr == 0 => l.push_str("--"),
            Setting::Lthr => l.push_u32(u32::from(self.lthr)),
            Setting::ZonesBy => l.push_str(if self.zones_lthr { "LTHR" } else { "MAX HR" }),
            Setting::Zone(n) => l.push_u32(u32::from(self.zones().upper().get(usize::from(n)).copied().unwrap_or(0))),
            Setting::ZonesReset => l.push_str(if self.zone_custom { "CUSTOM" } else { "DEFAULT" }),
            Setting::Sex => l.push_str(if self.female { "FEMALE" } else { "MALE" }),
            Setting::BirthYear | Setting::Height if self.number_get(s) == 0 => l.push_str("--"),
            Setting::BirthYear | Setting::Height => l.push_u32(u32::from(self.number_get(s))),
            Setting::PowerSmooth => {
                l.push_u32(POWER_SMOOTH_S.get(self.power_smooth).copied().unwrap_or(1));
                l.push_str(" S");
            }
            Setting::AutoLap => {
                l.push_str(["OFF", "BY DISTANCE", "BY TIME"].get(self.auto_lap).copied().unwrap_or("OFF"))
            }
            Setting::LiveSegments => l.push_str(if self.live_segments { "ON" } else { "OFF" }),
            Setting::SegTarget => l.push_str(SEGMENT_TARGETS.get(self.seg_target).copied().unwrap_or("AUTO")),
            Setting::Sound => l.push_str(if self.sound { "ON" } else { "OFF" }),
            Setting::StartReminder => l.push_str(if self.start_reminder { "ON" } else { "OFF" }),
            Setting::ReadyCheck => l.push_str(if self.ready_check { "ON" } else { "OFF" }),
            Setting::ClimbsMedium => l.push_str(if self.climbs_medium { "MEDIUM+" } else { "ALL" }),
            Setting::ClimbPage => l.push_str(if self.climb_page { "ON" } else { "OFF" }),
            Setting::DateFormat => l.push_str(match self.date_fmt {
                DateFormat::Dmy => "D/M/Y",
                DateFormat::Mdy => "M/D/Y",
                DateFormat::Ymd => "Y-M-D",
            }),
            Setting::Units => l.push_str(if self.imperial { "IMPERIAL" } else { "METRIC" }),
            Setting::Temp => l.push_str(if self.temp_f { "F" } else { "C" }),
            Setting::AutoPause => match AUTO_PAUSE_KMH.get(self.auto_pause) {
                Some(0) | None => l.push_str("AUTO"), // the profile's own rule
                Some(255) => l.push_str("OFF"),
                Some(&k) => {
                    l.push_u32(u32::from(k));
                    l.push_str(" KM/H");
                }
            },
            Setting::Trip(n) => {
                let m = self.trip_m.get(usize::from(n)).copied().unwrap_or(0);
                super::fields::push_trip(l, m, self.imperial);
                l.push_str(if self.imperial { " MI" } else { " KM" });
            }
            Setting::Odometer => {
                super::fields::push_trip(l, self.odo_m, self.imperial);
                l.push_str(if self.imperial { " MI" } else { " KM" });
            }
            Setting::LapKm => l.push_u32(u32::from(self.lap_km)),
            Setting::LapMin => l.push_u32(u32::from(self.lap_min)),
            Setting::Font => l.push_str(if self.smooth { "SMOOTH" } else { "PIXEL" }),
            Setting::Theme if self.auto_theme => l.push_str("AUTO"),
            Setting::Theme => l.push_str(self.theme_name()),
            Setting::DayTheme => l.push_str(Self::name_of(&self.day_theme)),
            Setting::NightTheme => l.push_str(Self::name_of(&self.night_theme)),
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
            Setting::Limit(n) => match self.limits.get(usize::from(n)) {
                Some(&v) if v > 0 && n == 10 => {
                    // DFA a1 in hundredths
                    l.push_u32(u32::from(v / 100));
                    l.push_str(".");
                    l.push_u32(u32::from(v / 10 % 10));
                    l.push_u32(u32::from(v % 10));
                }
                // the speed limits are kept in km/h and shown in mph when UNITS is IMPERIAL
                Some(&v) if v > 0 && matches!(n, 4 | 5) && self.imperial => {
                    l.push_u32((u32::from(v) * 6214 + 5000) / 10_000)
                }
                Some(&v) if v > 0 => l.push_u32(u32::from(v)),
                _ => l.push_str("OFF"),
            },
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
            Setting::PowerMatch => self.power_match = !self.power_match,
            Setting::PowerSmooth => self.power_smooth = (self.power_smooth + 1) % POWER_SMOOTH_S.len(),
            Setting::AutoLap => self.auto_lap = (self.auto_lap + 1) % 3,
            Setting::LiveSegments => self.live_segments = !self.live_segments,
            Setting::SegTarget => self.seg_target = (self.seg_target + 1) % SEGMENT_TARGETS.len(),
            Setting::Sound => self.sound = !self.sound,
            Setting::StartReminder => self.start_reminder = !self.start_reminder,
            Setting::ClimbsMedium => self.climbs_medium = !self.climbs_medium,
            Setting::ClimbPage => self.climb_page = !self.climb_page,
            Setting::DateFormat => self.date_fmt = self.date_fmt.next(),
            Setting::ReadyCheck => self.ready_check = !self.ready_check,
            Setting::ZonesBy => self.zones_lthr = !self.zones_lthr,
            Setting::ZonesReset => self.zone_custom = false,
            Setting::Sex => self.female = !self.female,
            Setting::Units => self.imperial = !self.imperial,
            Setting::Temp => self.temp_f = !self.temp_f,
            Setting::AutoPause => self.auto_pause = (self.auto_pause + 1) % AUTO_PAUSE_KMH.len(),
            Setting::Font => self.smooth = !self.smooth,
            // the built-ins only; the card's files are added by the app (App::next_theme)
            Setting::Theme | Setting::DayTheme | Setting::NightTheme => {
                let slot = match s {
                    Setting::DayTheme => &mut self.day_theme,
                    Setting::NightTheme => &mut self.night_theme,
                    _ => &mut self.theme,
                };
                let next = if Self::name_of(slot).eq_ignore_ascii_case("DARK") { "OUTDOOR" } else { "DARK" };
                *slot = Self::name_bytes(next);
            }
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
        if let Setting::Zone(n) = s {
            return self.step_zone(usize::from(n), up);
        }
        // a birth year or a height not set yet starts from a typical one, not from the bottom of its range
        if let (Setting::BirthYear | Setting::Height, Some(v)) = (s, self.number_mut(s))
            && *v == 0
        {
            *v = if s == Setting::Height { 170 } else { 1985 };
            return;
        }
        let (Some((range, step)), Some(v)) = (s.number(), self.number_mut(s)) else { return };
        *v = if up { v.saturating_add(step) } else { v.saturating_sub(step) }.clamp(*range.start(), *range.end());
    }

    /// The maximum heart rate from the age (220 minus the years, Fox and Haskell's rule of thumb, clamped to
    /// [`menu::MAX_HR_RANGE`]) in `year`, while MAX HR is the unset default ([`ride::MAX_HR`]) or was itself worked out
    /// from the age (`max_hr_age`, so a changed BORN recomputes it): a maximum the rider set or measured is never
    /// replaced. None when `year` is not a real clock year (under 2024: the clock is not set) or the age is under 5;
    /// else whether MAX HR changed.
    ///
    /// ```
    /// use bryton_rustfw::Settings;
    /// let mut s = Settings { birth_year: 1986, ..Settings::from_word(0) };
    /// assert_eq!(s.max_hr_from_age(2026), Some(true));
    /// assert_eq!((s.max_hr, s.max_hr_age), (180, true), "220 - 40");
    /// s.birth_year = 1996;
    /// assert_eq!(s.max_hr_from_age(2026), Some(true), "recomputed while it came from the age");
    /// assert_eq!(s.max_hr, 190);
    /// s.max_hr = 183;
    /// s.max_hr_age = false;
    /// assert_eq!(s.max_hr_from_age(2026), Some(false), "a maximum that was set is kept");
    /// assert_eq!(s.max_hr_from_age(1970), None, "the clock is not set");
    /// ```
    pub fn max_hr_from_age(&mut self, year: u16) -> Option<bool> {
        let age = year.checked_sub(self.birth_year).filter(|&a| year >= 2024 && a >= 5 && self.birth_year > 0)?;
        if !self.max_hr_age && self.max_hr != ride::MAX_HR {
            return Some(false);
        }
        let hr = 220u16.saturating_sub(age).clamp(*menu::MAX_HR_RANGE.start(), *menu::MAX_HR_RANGE.end());
        let changed = hr != self.max_hr;
        (self.max_hr, self.max_hr_age) = (hr, true);
        Some(changed)
    }

    /// The value of a number setting, 0 for none.
    fn number_get(&self, s: Setting) -> u16 {
        let mut c = *self;
        c.number_mut(s).map_or(0, |v| *v)
    }

    /// The zones every consumer reads: the rider's own bounds (ME > ZONES) while `zone_custom`, else the computed ones
    /// for ZONES BY.
    pub fn zones(&self) -> ride::Zones {
        let by = if self.zones_lthr { ride::ZonesBy::Lthr } else { ride::ZonesBy::MaxHr };
        let computed = ride::Zones::computed(self.ftp, self.max_hr, self.lthr, by);
        let custom = Some(self.zone_up).filter(|u| self.zone_custom && Self::zone_bounds_ok(u));
        custom.and_then(|u| ride::Zones::custom(self.ftp, self.max_hr, u)).unwrap_or(computed)
    }

    /// Whether `up` (the ten bounds, bpm then W) are in what ZONES offers: heart rate [`menu::ZONE_HR_MIN`] to
    /// [`menu::ZONE_HR_MAX`], power up to [`menu::ZONE_PW_MAX`]. A file's bounds out of it read as the computed zones.
    pub(crate) fn zone_bounds_ok(up: &[u16; 10]) -> bool {
        up[..4].iter().all(|v| (menu::ZONE_HR_MIN..=menu::ZONE_HR_MAX).contains(v))
            && up[4..].iter().all(|&v| v <= menu::ZONE_PW_MAX)
    }

    /// Zone bound `n` (0 to 3 heart rate, 4 to 9 power) one step up or down, between its neighbours. The first edit
    /// takes the computed zones as the rider's own.
    fn step_zone(&mut self, n: usize, up: bool) {
        let mut z = self.zones().upper();
        let (first, last, min, max, step) =
            if n < 4 { (0, 3, menu::ZONE_HR_MIN, menu::ZONE_HR_MAX, 1) } else { (4, 9, 1, menu::ZONE_PW_MAX, 5) };
        let lo = if n == first { min } else { z.get(n - 1).map_or(min, |v| v + 1) };
        let hi = if n == last { max } else { z.get(n + 1).map_or(max, |v| v.saturating_sub(1)) };
        let Some(v) = z.get_mut(n) else { return };
        *v = if up { v.saturating_add(step) } else { v.saturating_sub(step) }.clamp(lo, hi.max(lo));
        if ride::Zones::custom(self.ftp, self.max_hr, z).is_some() && z != self.zones().upper() {
            (self.zone_custom, self.zone_up) = (true, z);
        }
    }

    pub(crate) fn number_mut(&mut self, s: Setting) -> Option<&mut u16> {
        match s {
            Setting::MaxHr => Some(&mut self.max_hr),
            Setting::Ftp => Some(&mut self.ftp),
            Setting::Lthr => Some(&mut self.lthr),
            Setting::BirthYear => Some(&mut self.birth_year),
            Setting::Height => Some(&mut self.height_cm),
            Setting::Weight => Some(&mut self.weight_kg),
            Setting::BikeWeight => Some(&mut self.bike_hg),
            Setting::WPrime => Some(&mut self.wprime_j),
            Setting::Eat => Some(&mut self.eat_min),
            Setting::Drink => Some(&mut self.drink_min),
            Setting::HrAlert => Some(&mut self.hr_alert),
            Setting::Limit(n) => self.limits.get_mut(usize::from(n)),
            Setting::Altitude => Some(&mut self.alt_m),
            Setting::LapKm => Some(&mut self.lap_km),
            Setting::LapMin => Some(&mut self.lap_min),
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

/// The climb page ([`CLIMB`]), shown while a live segment is ridden or a climb is on: the head (the segment's name,
/// the delta against its target and the target; on a climb, its title and the time to the top), the profile of the
/// segment or the climb with the rider's place (and the target's, on a segment), the pacer's gauge, then the time on
/// it and the distance left. Not configurable.
pub(crate) const CLIMB_PAGE: PageCfg = {
    use Field::*;
    PageCfg { layout: 16, fields: [SegHead, ClimbProfile, ClimbPace, ClimbTime, ClimbLeft, Time] }
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

    /// Writes the settings to their backup registers (before the file is read, those no change touched as they were
    /// at the start: `SettingsStore::writes`; a change undone puts the start's word back).
    pub(crate) fn write_settings_registers(&self, b: &mut impl Board) {
        let st = &self.settings_store;
        let w = self.settings.register_words();
        for (k, (r, word)) in [WHEEL_BKP, POWER_BKP, ALERTS_BKP].into_iter().zip(w).enumerate() {
            b.backup_write(r, if st.writes(k, word) { word } else { st.boot.get(k).copied().unwrap_or(word) });
        }
        self.write_aid(b);
    }

    /// The aiding's two words (they change with a fix) with AUTO OFF in the second's bits 22..24.
    pub(crate) fn write_aid(&self, b: &mut impl Board) {
        let [w0, w1] = self.gps.aid.to_words();
        b.backup_write(AID_BKP[0], w0);
        let off = self.settings.auto_off as u32 + 1;
        // before the file is read and AUTO OFF unchanged: the bits as they were (none from a build without it)
        let st = &self.settings_store;
        let off = if st.writes(3, off) { off } else { st.boot.get(3).copied().unwrap_or(0) };
        b.backup_write(AID_BKP[1], w1 | off << 22);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_word_wprime_top_byte_clamps_without_overflow() {
        // the W' byte is 0..255 (0.5 kJ each): 255 x 500 J overflows a u16, so it clamps to the range's top
        let s = Settings::from_word(0).with_power_word(0xc1_00_ff_00); // W' byte in bits 8..15
        assert_eq!(s.wprime_j, *menu::WPRIME_RANGE.end());
    }
}
