//! Menus as data: what the user asks for ([`Input`], from a key now and from a touch gesture later), what each menu
//! row is ([`Row`]) and the settings rows change ([`Setting`]). The app draws and acts on a row by its kind, never
//! by its position, so a menu grows by adding a row to its table.

use crate::{Key, Menu, Press, power, ride, touch::Gesture};

/// What the user asked for. Screens act on these, never on keys, so a key, a touch gesture or the dev link reach the
/// same code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Page: the next page, or down a menu.
    Next,
    /// Lap: up a menu (on a ride page, a lap).
    Prev,
    /// Start: select a row (on a ride page, start, pause or resume).
    Select,
    /// Back: out of a menu (on a ride page, the overview; on the overview, nothing).
    Back,
    /// Page held: the menu (the ride menu while a ride records or is paused).
    Menu,
    /// Start held: end a paused ride.
    End,
    /// Back held: power off.
    Off,
    /// Lap held: play or pause the phone's music (app/phone.rs), from any page.
    Media,
    /// Back and Lap held together: the touch lock on or off, on every screen.
    TouchLock,
    /// Page or Lap still held: [`Input::Next`] or [`Input::Prev`] again (a list scrolls, a value steps).
    NextHeld,
    /// See [`Input::NextHeld`].
    PrevHeld,
    /// A gesture on the touch panel: on a page, swipe left the next page, swipe right the previous one, hold the
    /// menu (not while a ride records); in a menu, a tap acts on a row (two taps while the ride moves: select, then act), the "<" is Back,
    /// swipe up or down turns a screen of rows, swipe right is Back; while a number is edited, swipe up or down steps
    /// it, a tap keeps it. None with TOUCH off.
    Touch(Gesture),
}

impl Input {
    /// The input a key press stands for, if any.
    ///
    /// ```
    /// use bryton_rustfw::{Key, Press, menu::Input};
    /// assert_eq!(Input::of(Press::Short(Key::Page)), Some(Input::Next));
    /// assert_eq!(Input::of(Press::Long(Key::Back)), Some(Input::Off));
    /// assert_eq!(Input::of(Press::Repeat(Key::Start)), None);
    /// ```
    pub fn of(p: Press) -> Option<Input> {
        Some(match p {
            Press::Short(Key::Page) => Input::Next,
            Press::Short(Key::Lap) => Input::Prev,
            Press::Short(Key::Start) => Input::Select,
            Press::Short(Key::Back) => Input::Back,
            Press::Long(Key::Page) => Input::Menu,
            Press::Long(Key::Start) => Input::End,
            Press::Long(Key::Back) => Input::Off,
            Press::Long(Key::Lap) => Input::Media,
            Press::Chord => Input::TouchLock,
            Press::Repeat(Key::Page) => Input::NextHeld,
            Press::Repeat(Key::Lap) => Input::PrevHeld,
            _ => return None,
        })
    }
}

/// A setting a menu row shows and changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Wheel circumference (a choice of [`crate::WHEELS`]).
    Wheel,
    /// Backlight level (a choice of [`crate::LIGHT_LEVELS`] or AUTO).
    Light,
    /// Backlight time-out (a choice of [`crate::LIGHT_TIMEOUTS_S`]).
    LightOff,
    /// AUTO OFF (a choice of [`crate::AUTO_OFF_MIN`]).
    AutoOff,
    /// The touch panel on or off (keys always work).
    Touch,
    /// Max heart rate (a number).
    MaxHr,
    /// FTP (a number), also the CP of W' balance.
    Ftp,
    /// Lactate threshold heart rate (a number, bpm; 0 until set, the LTHR fields show "--").
    Lthr,
    /// ZONES BY: the heart-rate zones from the MAX HR or the LTHR (a choice).
    ZonesBy,
    /// Bound `n` of the zones table (0 to 3 the heart-rate zones 1 to 4, 4 to 9 the power zones 1 to 6): the highest
    /// bpm or W in the zone, a number edited in place (app/settings.rs `step_zone`).
    Zone(u8),
    /// RESET TO DEFAULT: Select goes back to the computed zones; the value says which are in use.
    ZonesReset,
    /// SEX: male or female (TRIMP's constant for hrTSS).
    Sex,
    /// BIRTH YEAR: 0 until set; sets the maximum heart rate (220 minus the age) while that is still the default.
    BirthYear,
    /// HEIGHT, cm: 0 until set (kept for formulas that need it; none does yet).
    Height,
    // power.rs
    /// Rider weight (a number), for W/kg.
    Weight,
    /// W' for W' balance (a number, J, shown in kJ).
    WPrime,
    /// Bike weight (a number), for the climb pacer.
    BikeWeight,
    // alerts.rs (0: off)
    /// Eat reminder, every so many minutes of ride time.
    Eat,
    /// Drink reminder, the same.
    Drink,
    /// Heart rate alert over this, bpm.
    HrAlert,
    /// Limit `n` of [`crate::alerts::LIMITS`]: power, cadence, speed, heart rate low, reminders by distance, kcal and kJ,
    /// the DFA a1 alert (0: off).
    Limit(u8),
    /// What a shifter button press does ([`Remote`]), for press `n` ([`crate::sensors::Press`]).
    Remote(u8),
    /// The barometric altitude (a number, m): typing it calibrates the barometer to it (altitude.rs).
    Altitude,
    /// The ride profile: AUTO (indoor when a trainer is heard at Start), ROAD, INDOOR or MTB (indoor too when a
    /// trainer is heard) ([`crate::ride::Profile`]).
    BikeProfile,
    /// Climbs found on the road ahead when no course is followed, on or off (`climbs_ahead`).
    ClimbsAhead,
    /// CLIMBS (BIKE): the climbs ahead are ALL or MEDIUM+ (category 3 and harder).
    ClimbsMedium,
    /// CLIMB PAGE (BIKE): the climb page is on or off (off, it is skipped in the paging).
    ClimbPage,
    /// QUICK STATUS (PAGES): a tap on a plain field of a ride page opens the overlay, on or off.
    QuickStatus,
    /// DATE FORMAT (DISPLAY): D/M/Y, M/D/Y or Y-M-D.
    DateFormat,
    /// ZONE (DISPLAY), the time zone: CLOCK (the RTC shown as is) or a zone, UTC - 12:00 to + 14:00 in quarter hours (a number,
    /// `tz`; app/clock.rs).
    TimeZone,
    /// MAP (DISPLAY): NORTH UP or HEADING UP.
    MapUp,
    /// SUMMER TIME (DISPLAY): OFF, EU or US, with a TIME ZONE ([`crate::Dst`]).
    DaylightSaving,
    /// The font of every text: PIXEL (dot matrix, the default) or BARLOW ([`crate::fonts::smooth`]).
    Font,
    /// The colour theme: AUTO (day or night by the ambient light), DARK, OUTDOOR or a file of `\Rustfw\Themes`
    /// ([`crate::theme`]).
    Theme,
    /// The theme AUTO uses by day, and by night (the same choices as THEME, without AUTO).
    DayTheme,
    /// The theme AUTO uses by night.
    NightTheme,
    /// The GPS receiver's power mode: AUTO (the default: cyclic tracking while riding fast and straight), FULL
    /// (continuous tracking) or SAVER (cyclic tracking) ([`crate::GpsPower`]).
    GpsPower,
    /// ERG targets corrected so the power meter reads them, on or off (`power_match`, [`crate::power_match`]).
    PowerMatch,
    /// GNSS SYSTEMS (BIKE): which satellite systems the receiver tracks ([`crate::gnss::SYSTEMS`]).
    GnssSystems,
    /// BATTERY SAVER (SYSTEM): OFF, ON or AUTO ([`crate::SaverMode`]).
    BatterySaver,
    /// LEVEL STEP (BIKE): how many levels a press moves in the trainer's LEVEL mode ([`crate::LEVEL_STEPS`]).
    LevelStep,
    /// STEEP DESCENT (ALERTS): OFF, or the grade a descent ahead must pass to be said ([`crate::STEEP_DESCENT_PCT`]).
    SteepDescent,
    /// SLOPE SCALE (BIKE): how much of the course's grade the trainer is given ([`crate::SLOPE_SCALES`]).
    SlopeScale,
    /// The window of the big power value: the live reading, 3, 5 or 10 s (app/settings.rs, POWER_SMOOTH_S).
    PowerSmooth,
    /// AUTO LAP: OFF, DISTANCE or TIME (`auto_lap`).
    AutoLap,
    /// The distance of a DISTANCE auto lap, km.
    LapKm,
    /// The moving time of a TIME auto lap, minutes.
    LapMin,
    /// SUNSET: OFF, or the minutes before the sunset it says so (`sunset_alert`, [`crate::SUNSET_LEAD_MIN`]).
    SunsetAlert,
    /// CHAINRINGS: the front chainrings' teeth, a preset (`chainrings`, [`crate::CHAINRINGS`]); OFF shows gear numbers.
    Chainrings,
    /// CASSETTE: the rear cogs' teeth, a preset (`cassette`, [`crate::CASSETTES`]); OFF shows gear numbers.
    Cassette,
    /// LIVE SEGMENTS: the segments of the card are matched during a ride, on or off (`live_segments`).
    LiveSegments,
    /// The effort a live segment is ridden against: AUTO, PR, KOM, QOM or RECENT (`seg_target`).
    SegTarget,
    /// SOUND: the buzzer on or off (`sound`).
    Sound,
    /// REMINDER (BIKE): a banner and a beep when the bike moves with no ride (`start_reminder`).
    StartReminder,
    /// READY CHECK (BIKE): the card at a ride's start, GPS, sensors and battery (`ready_check`).
    ReadyCheck,
    /// UNITS: METRIC or IMPERIAL (`imperial`).
    Units,
    /// TEMPERATURE: C or F (`temp_f`).
    Temp,
    /// AUTO PAUSE: AUTO, OFF or a speed (`auto_pause`, [`crate::AUTO_PAUSE_KMH`]).
    AutoPause,
    /// TRIP 1 or TRIP 2 (0, 1): the distance ridden since it was reset; Select (twice) resets it.
    Trip(u8),
    /// ODOMETER: the distance of every ride saved; never reset.
    Odometer,
}

/// What a shifter button press does (Di2 hood buttons: [`Setting::Remote`]). Kept in 3 bits ([`crate::Settings`]),
/// so there are eight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    /// Nothing.
    None = 0,
    /// The next ride page.
    NextPage,
    /// The previous ride page.
    PrevPage,
    /// A lap, as the Lap key.
    Lap,
    /// Start, pause or resume, as the Start key.
    Start,
    /// Play or pause the phone's music.
    Play,
    /// The next track.
    NextTrack,
    /// The previous track.
    PrevTrack,
}

impl Remote {
    /// Every action, by its number.
    pub const ALL: [Remote; 8] = {
        use Remote::*;
        [None, NextPage, PrevPage, Lap, Start, Play, NextTrack, PrevTrack]
    };

    /// Its name in the menu.
    pub fn name(self) -> &'static str {
        ["NONE", "NEXT PAGE", "PREV PAGE", "LAP", "START", "PLAY", "NEXT TRK", "PREV TRK"][self as usize]
    }

    /// The action numbered `n` (its low 3 bits).
    pub fn from_bits(n: u32) -> Remote {
        Self::ALL[(n & 7) as usize]
    }

    /// The action stock's sensor ini keeps as `n` for a Di2 press (`leftpress`...: 0 none, 1 lap, 2 start / stop,
    /// 3 lock, 4 next page; the patched stock firmware's P12 adds 5 previous page, 6 to 10 its screens, 11 play /
    /// pause, 12 next and 13 previous track; bryton-S510-mods patches/P12-di2-button-actions.c), None for those that
    /// have no equivalent here.
    ///
    /// ```
    /// use bryton_rustfw::menu::Remote;
    /// assert_eq!([1, 4, 5, 13, 3].map(Remote::from_stock), [Remote::Lap, Remote::NextPage, Remote::PrevPage, Remote::PrevTrack, Remote::None]);
    /// ```
    pub fn from_stock(n: u32) -> Remote {
        match n {
            1 => Remote::Lap,
            2 => Remote::Start,
            4 => Remote::NextPage,
            5 => Remote::PrevPage,
            11 => Remote::Play,
            12 => Remote::NextTrack,
            13 => Remote::PrevTrack,
            _ => Remote::None,
        }
    }
}

/// The Di2 buttons menu: an action for each press ([`crate::sensors::Press`]).
pub const REMOTE: [Row; 6] = [
    Row::Set(Setting::Remote(0)),
    Row::Set(Setting::Remote(1)),
    Row::Set(Setting::Remote(2)),
    Row::Set(Setting::Remote(3)),
    Row::Set(Setting::Chainrings),
    Row::Set(Setting::Cassette),
];

impl Setting {
    /// The row's label: what it is, in words; the unit goes after the value ([`Setting::unit`]).
    pub fn label(self) -> &'static str {
        match self {
            Setting::Wheel => "WHEEL",
            Setting::Light => "BRIGHTNESS",
            Setting::LightOff => "OFF AFTER",
            Setting::AutoOff => "AUTO OFF",
            Setting::Touch => "TOUCH",
            Setting::MaxHr => "MAX HR",
            Setting::Ftp => "FTP",
            Setting::Weight => "WEIGHT",
            Setting::WPrime => "W'",
            Setting::BikeWeight => "WEIGHT",
            Setting::Eat => "EAT",
            Setting::Drink => "DRINK",
            Setting::HrAlert => "HR ABOVE",
            Setting::Limit(n) => [
                "PWR ABOVE",
                "PWR BELOW",
                "CAD ABOVE",
                "CAD BELOW",
                "SPD ABOVE",
                "SPD BELOW",
                "HR BELOW",
                "EACH KM",
                "EACH KCAL",
                "EACH KJ",
                "DFA A1",
            ]
            .get(usize::from(n))
            .copied()
            .unwrap_or(""),
            Setting::Altitude => "ALTITUDE",
            Setting::BikeProfile => "PROFILE",
            Setting::ClimbsAhead => "CLIMBS AHEAD",
            Setting::ClimbsMedium => "CLIMBS",
            Setting::ClimbPage => "CLIMB PAGE",
            Setting::QuickStatus => "QUICK STATUS",
            Setting::DateFormat => "DATE FORMAT",
            Setting::TimeZone => "ZONE",
            Setting::DaylightSaving => "SUMMER TIME",
            Setting::MapUp => "MAP",
            Setting::PowerMatch => "POWER MATCH",
            Setting::GnssSystems => "GNSS",
            Setting::BatterySaver => "BATT SAVER",
            Setting::LevelStep => "LEVEL STEP",
            Setting::SlopeScale => "SLOPE SCALE",
            Setting::SteepDescent => "STEEP DESC",
            Setting::Lthr => "LTHR",
            Setting::ZonesBy => "ZONES BY",
            Setting::Zone(0) => "HR Z1",
            Setting::Zone(1) => "HR Z2",
            Setting::Zone(2) => "HR Z3",
            Setting::Zone(3) => "HR Z4",
            Setting::Zone(4) => "PWR Z1",
            Setting::Zone(5) => "PWR Z2",
            Setting::Zone(6) => "PWR Z3",
            Setting::Zone(7) => "PWR Z4",
            Setting::Zone(8) => "PWR Z5",
            Setting::Zone(_) => "PWR Z6",
            Setting::ZonesReset => "RESET",
            Setting::Sex => "SEX",
            Setting::BirthYear => "BORN",
            Setting::Height => "HEIGHT",
            Setting::PowerSmooth => "POWER SMOOTH",
            Setting::AutoLap => "AUTO LAP",
            Setting::SunsetAlert => "SUNSET",
            Setting::Chainrings => "CHAINRINGS",
            Setting::Cassette => "CASSETTE",
            Setting::LiveSegments => "LIVE SEGMENTS",
            Setting::SegTarget => "TARGET",
            Setting::Sound => "SOUND",
            Setting::StartReminder => "REMINDER",
            Setting::ReadyCheck => "READY CHECK",
            Setting::Units => "UNITS",
            Setting::Temp => "TEMPERATURE",
            Setting::AutoPause => "AUTO PAUSE",
            Setting::Trip(0) => "TRIP 1",
            Setting::Trip(_) => "TRIP 2",
            Setting::Odometer => "ODOMETER",
            Setting::LapKm => "LAP KM",
            Setting::LapMin => "LAP MIN",
            Setting::Font => "FONT",
            Setting::Theme => "THEME",
            Setting::DayTheme => "DAY",
            Setting::NightTheme => "NIGHT",
            Setting::GpsPower => "GPS POWER",
            Setting::Remote(n) => ["L PRESS", "R PRESS", "L HOLD", "R HOLD"].get(usize::from(n)).copied().unwrap_or(""),
        }
    }

    /// The unit shown after a number (not while it is edited: the row has no room for it): "FTP  200 W".
    ///
    /// ```
    /// use bryton_rustfw::menu::Setting;
    /// assert_eq!([Setting::Ftp, Setting::Eat, Setting::Touch].map(Setting::unit), [" W", " MIN", ""]);
    /// ```
    pub fn unit(self) -> &'static str {
        match self {
            Setting::Wheel => " MM",
            Setting::Light => "%",
            Setting::MaxHr | Setting::HrAlert | Setting::Lthr | Setting::Limit(6) | Setting::Zone(0..=3) => " BPM",
            Setting::Zone(_) => " W",
            Setting::Height => " CM",
            Setting::Limit(0 | 1) => " W",
            Setting::Limit(2 | 3) => " RPM",
            Setting::Limit(4 | 5) => " KMH",
            Setting::Ftp => " W",
            Setting::Weight | Setting::BikeWeight => " KG",
            Setting::WPrime => " KJ",
            Setting::Eat | Setting::Drink => " MIN",
            Setting::Altitude => " M",
            Setting::LapKm => " KM",
            Setting::LapMin => " MIN",
            _ => "",
        }
    }

    /// [`Setting::unit`] with the speed limits in mph when `imperial` (they are kept in km/h).
    pub fn unit_in(self, imperial: bool) -> &'static str {
        match self {
            Setting::Limit(4 | 5) if imperial => " MPH",
            _ => self.unit(),
        }
    }

    /// For a number edited in place (Select, then up and down): its range and step. None for a choice, which
    /// Select moves to the next value.
    pub fn number(self) -> Option<(core::ops::RangeInclusive<u16>, u16)> {
        match self {
            Setting::MaxHr => Some((MAX_HR_RANGE, 1)),
            Setting::Lthr => Some((LTHR_RANGE, 1)),
            Setting::Zone(0..=3) => Some((ZONE_HR_MIN..=ZONE_HR_MAX, 1)),
            Setting::Zone(_) => Some((1..=ZONE_PW_MAX, 5)),
            Setting::BirthYear => Some((1920..=2020, 1)),
            Setting::Height => Some((100..=230, 1)),
            Setting::Ftp => Some((FTP_RANGE, 5)),
            Setting::Weight => Some((WEIGHT_RANGE, 1)),
            Setting::WPrime => Some((WPRIME_RANGE, 500)),
            Setting::BikeWeight => Some((BIKE_RANGE, 1)),
            Setting::Eat | Setting::Drink => Some((0..=120, 5)),
            Setting::HrAlert => Some((0..=230, 5)),
            Setting::Limit(0 | 1) => Some((0..=999, 10)),
            Setting::Limit(2 | 3) => Some((0..=250, 5)),
            Setting::Limit(4 | 5) => Some((0..=100, 1)),
            Setting::Limit(6) => Some((0..=230, 5)),
            Setting::Limit(7) => Some((0..=99, 1)),
            Setting::Limit(8 | 9) => Some((0..=990, 10)),
            Setting::Limit(10) => Some((0..=100, 5)),
            // -500 to 9000 m, kept + 500 (the Dead Sea's shore to above Everest); 1 m steps (a known height to the metre; a held key repeats)
            Setting::Altitude => Some((0..=9500, 1)),
            Setting::LapKm => Some((1..=50, 1)),
            Setting::LapMin => Some((1..=60, 1)),
            Setting::TimeZone => Some((0..=crate::TZ_MAX, 1)),
            _ => None,
        }
    }
}

/// Heart-rate zone bounds offered, bpm.
pub const ZONE_HR_MIN: u16 = 40;
/// See [`ZONE_HR_MIN`].
pub const ZONE_HR_MAX: u16 = 250;
/// Power zone bounds offered, W.
pub const ZONE_PW_MAX: u16 = 2000;

/// Max heart rates offered, bpm.
pub const MAX_HR_RANGE: core::ops::RangeInclusive<u16> = 100..=230;
/// LTHRs offered, bpm: 0 (the default, none set) is not one of them.
pub const LTHR_RANGE: core::ops::RangeInclusive<u16> = 100..=230;
/// FTPs offered, W.
pub const FTP_RANGE: core::ops::RangeInclusive<u16> = 50..=600;
const _: () = assert!(ride::MAX_HR >= 100 && ride::MAX_HR <= 230 && ride::FTP.is_multiple_of(5));
/// Rider weights offered, kg.
pub const WEIGHT_RANGE: core::ops::RangeInclusive<u16> = 30..=150;
/// Bike weights offered, 100 g: 5 to 25 kg (a byte in register 16: 0 there means the default).
pub const BIKE_RANGE: core::ops::RangeInclusive<u16> = 50..=250;
/// W' offered, J (0.5 kJ steps).
pub const WPRIME_RANGE: core::ops::RangeInclusive<u16> = 5000..=40_000;
const _: () = assert!(power::BIKE_HG >= 50 && power::BIKE_HG <= 250);
const _: () = assert!(power::WEIGHT_KG >= 30 && power::WEIGHT_KG <= 150 && power::WPRIME_J.is_multiple_of(500));

/// A reading shown in a menu (nothing to change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// The ambient light, lux.
    Lux,
    /// The barometer's temperature.
    TempC,
    /// The barometer's pressure.
    Hpa,
    /// The GPS fix kind.
    Fix,
    /// Satellites used.
    Sats,
    /// Latitude.
    Lat,
    /// Longitude.
    Lon,
    /// Altitude.
    Alt,
    /// UTC time from the receiver.
    Utc,
    /// UTC date from the receiver.
    Date,
}

/// What a page's switch rows do (app/pagemeta.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOp {
    /// ON or OFF: the paging skips a page that is OFF.
    Show,
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// Type a short name.
    Rename,
}

impl PageOp {
    /// The row's text.
    pub fn label(self) -> &'static str {
        match self {
            PageOp::Show => "SHOW",
            PageOp::Up => "MOVE UP",
            PageOp::Down => "MOVE DOWN",
            PageOp::Rename => "RENAME",
        }
    }
}

/// What a menu row is: the app draws it and acts on Select by its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Opens a menu.
    Open(Menu),
    /// What an empty list says, as a grey row that does nothing (not a banner that goes after 4 s and leaves a bare
    /// list): "NONE PLANNED" in WORKOUTS.
    Empty(&'static str),
    /// Opens the list of BLE devices nearby (and starts a scan).
    Nearby,
    /// The Bluetooth trainer (app/ftms.rs): Select searches for one ([`crate::Menu::BtTrainers`]), or forgets it.
    BtTrainer,
    /// Trainer `n` the search heard: Select pairs it.
    BtFound(usize),
    /// Opens ABOUT: the firmware's version and commit, and Ferris.
    About,
    /// A figure of STORAGE (the survey's value, storage/volume.rs): no action.
    Storage(StorageRow),
    /// STORAGE's oldest ride `k`: its date and size; Select opens its HISTORY card, where DELETE is.
    OldRide(usize),
    /// FROM GPS: Select averages the receiver's altitude for 30 s and calibrates the barometer to it.
    AltGps,
    /// SAVE PLACE: Select saves where the rider is (app/places.rs).
    SavePlace,
    /// Favourite place `n`: its distance and bearing from here; Select opens its menu.
    Place(usize),
    /// DELETE in place `n`'s menu (confirmed).
    DeletePlace(usize),
    /// ACTIVE BIKE (BIKES): Select makes the next bike the active one (app/bikes.rs).
    ActiveBike,
    /// Bike `k`'s odometer (no action).
    BikeOdo(usize),
    /// Part `p` of bike `k`: its distance since RESET; Select opens its menu.
    Part(usize, usize),
    /// SERVICE EVERY of part `p` of bike `k`: Select steps the interval ([`crate::bikes::EVERY_KM`]).
    PartEvery(usize, usize),
    /// RESET of part `p` of bike `k` (confirmed): back to 0.
    PartReset(usize, usize),
    /// The USB drive: Select lends the card to the computer, or takes it back (app/drive.rs).
    Drive,
    /// GIVE UP SAVE, first in SYSTEM only while a save is stuck (app/riding.rs, `give_up_save`).
    GiveUpSave,
    /// SCREENSHOT: Select leaves the menus and saves the screen under them (app/screenshot.rs).
    Screenshot,
    /// A setting.
    Set(Setting),
    /// ANT sensor `n` ([`crate::sensors::Kind::ALL`]): Select pairs or forgets it.
    Sensor(usize),
    /// A reading.
    Read(Reading),
    /// Ride page `n`'s layout: Select goes to the next one ([`crate::ui::LAYOUT_ORDER`]) its fields fit.
    Layout(usize),
    /// Opens the page editor on ride page `n` (app/editor.rs).
    EditPage(usize),
    /// Adds a ride page after the last and opens the editor on it.
    AddPage,
    /// A row of ride page `n`'s menu that switches, moves or renames it (app/pagemeta.rs).
    PageOp(usize, PageOp),
    /// AUTO SCROLL in PAGES: Select steps OFF, 5, 10, 20 s.
    AutoScroll,
    /// A row of the page editor's actions ([`EDIT_ROWS`]).
    Edit(EditRow),
    /// Ride page `n`'s field `i`.
    Field(usize, usize),
    /// A row of the stock data menu.
    Stock(StockRow),
    /// Notification `n` of the NOTIFICATIONS list, newest first: Select opens its detail.
    Notif(usize),
    /// Climb done `n` of CLIMBS DONE, newest first (no action).
    Done(usize),
    /// Month `n` of FITNESS > MONTHS, newest first: Select changes what the rows show.
    Month(usize),
    /// Past ride `n` of the history, newest first: Select opens its card.
    Past(usize),
    /// Planned workout `n` of the calendar ([`crate::storage::workout_file::Calendar`]): Select shows its steps.
    Workout(usize),
    /// Select starts the workout shown (and the ride, if none runs).
    WorkoutStart,
    /// The FTP ramp test (ramp.rs, app/ramp.rs): Select starts it, with a trainer heard.
    RampTest,
    /// RIDE A COURSE (app/courses.rs): Select lists the courses; the one picked is ridden on the trainer.
    RideCourse,
    /// Step `n` of the workout shown: Select starts the workout from it.
    Step(usize),
    /// Course `n` of the list ([`crate::storage::course_file::List`]): Select follows it, or stops following it.
    Course(usize),
    /// Segment `n` of the index (SEGMENTS): name, distance and gain; read only.
    Seg(usize),
    /// The files the index skipped (SEGMENTS), the count as the value; read only.
    SegSkipped,
    /// Group `g` of [`crate::ui::FIELD_GROUPS`]: Select lists its fields.
    Group(usize),
    /// A field of a group: Select puts it in the page's box and goes back to the page's menu.
    Pick(crate::ui::Field),
    /// A row of the phone menu.
    Phone(PhoneRow),
    /// A row of the ride menu that acts on the ride, the workout or the course.
    Ride(RideRow),
}

impl Row {
    /// The row's text when it does not depend on the app's state (the table's half of a row; the app adds the values and
    /// the labels that change). None for the rows named by what they hold: a page's number, a sensor, a ride, a trainer
    /// found, a step, a field group, PAUSE / RESUME.
    pub fn label(self) -> Option<&'static str> {
        use RideRow as R;
        Some(match self {
            Row::Open(Menu::Main) => "ALL SETTINGS",
            Row::Open(Menu::Page(crate::OVERVIEW)) => "OVERVIEW",
            Row::Open(Menu::Page(crate::QUICK)) => "QUICK FIELDS",
            Row::Open(Menu::Page(_)) => return None,
            Row::Open(m) => crate::menu_title(m),
            Row::Empty(text) => text,
            Row::Nearby => "BLE NEARBY",
            Row::BtTrainer => "BT TRAINER",
            Row::About => "ABOUT",
            Row::Storage(s) => match s {
                StorageRow::Total => "TOTAL",
                StorageRow::Free => "FREE",
                StorageRow::Rides => "RIDES",
            },
            Row::OldRide(_) => return None,
            Row::AltGps => "FROM GPS",
            Row::SavePlace => "SAVE PLACE",
            Row::DeletePlace(_) => "DELETE",
            Row::ActiveBike => "ACTIVE BIKE",
            Row::BikeOdo(_) => "ODO",
            Row::Part(_, p) => crate::bikes::PART_NAMES.get(p).copied().unwrap_or("PART"),
            Row::PartEvery(..) => "SERVICE", // "SERVICE EVERY" left no gap before "OFF"
            Row::PartReset(..) => "RESET",
            Row::Drive => "USB DRIVE",
            Row::GiveUpSave => "GIVE UP SAVE",
            Row::Screenshot => "SCREENSHOT",
            Row::Set(s) => s.label(),
            Row::Read(r) => match r {
                Reading::Lux => "LUX",
                Reading::TempC => "TEMP C",
                Reading::Hpa => "HPA",
                Reading::Fix => "FIX",
                Reading::Sats => "SATS",
                Reading::Lat => "LAT",
                Reading::Lon => "LON",
                Reading::Alt => "ALT M",
                Reading::Utc => "UTC",
                Reading::Date => "DATE",
            },
            Row::Layout(_) => "LAYOUT",
            Row::EditPage(_) => "EDIT ON PAGE",
            Row::AddPage => "ADD PAGE",
            Row::PageOp(_, o) => o.label(),
            Row::AutoScroll => "AUTO SCROLL",
            Row::Edit(r) => r.label(),
            Row::Stock(r) => r.label(),
            Row::WorkoutStart => "START",
            Row::RampTest => "RAMP TEST",
            Row::RideCourse => "RIDE A COURSE",
            Row::Phone(r) => r.label(),
            Row::Pick(field) => field.label(),
            Row::Ride(r) => match r {
                R::Pause => return None,
                R::End => "END RIDE",
                R::NextStep => "NEXT STEP",
                R::StopWorkout => "STOP WORKOUT",
                R::StopCourse => "STOP FOLLOWING",
                R::Reverse => "REVERSE COURSE",
                R::BackToStart => "BACK TO START",
                R::Trainer => "TRAINER CONTROL",
                R::CalPower => "CALIBRATE POWER",
                R::Curve => "POWER CURVE",
                R::Indoor => "INDOOR",
            },
            Row::BtFound(_)
            | Row::Sensor(_)
            | Row::Field(..)
            | Row::Past(_)
            | Row::Notif(_)
            | Row::Done(_)
            | Row::Month(_)
            | Row::Workout(_)
            | Row::Step(_)
            | Row::Course(_)
            | Row::Seg(_)
            | Row::Place(_)
            | Row::Group(_) => return None,
            Row::SegSkipped => "SKIPPED FILES",
        })
    }
}

/// What the ride menu's own rows do ([`crate::Menu::Ride`]); its other rows open a menu or change a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RideRow {
    /// Pauses the ride (the menu stays, END RIDE under it), or resumes it (back to the page), as Start.
    Pause,
    /// Ends the ride (asked first), as Start held while paused.
    End,
    /// The workout's next step, as Lap but without a lap.
    NextStep,
    /// Stops the workout; the ride goes on.
    StopWorkout,
    /// Stops following the course; the ride goes on.
    StopCourse,
    /// Follows the course the other way ([`crate::course::Course::reverse`]).
    Reverse,
    /// The ride's own track back to its start, as a course (app/courses.rs).
    BackToStart,
    /// Opens the trainer screen (app/trainer.rs).
    Trainer,
    /// Opens the power meter calibration screen (app/powercal.rs).
    CalPower,
    /// Opens the power curve screen (app/curve.rs).
    Curve,
    /// INDOOR ON / OFF: the ride's profile by hand (app/profile.rs).
    Indoor,
}

/// The workout menu of a ride (while a step runs).
pub const WORKOUT_NOW: [Row; 2] = [Row::Ride(RideRow::NextStep), Row::Ride(RideRow::StopWorkout)];

/// The course menu of a ride (while one is followed).
pub const COURSE_NOW: [Row; 2] = [Row::Ride(RideRow::StopCourse), Row::Ride(RideRow::Reverse)];

/// What the phone menu shows ([`PHONE`]): the link (app/phone.rs) and its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneRow {
    /// The link: no BLE, off, pairing, waiting for the phone, connected, secure.
    Status,
    /// Select advertises for a minute so the phone can pair.
    Pair,
    /// Select forgets the phone (the bond erased).
    Forget,
    /// The phone's notifications in the banner, on or off.
    Notify,
    /// Media keys, on or off.
    Keys,
    /// Select opens the media remote.
    Media,
}

impl PhoneRow {
    /// The row's label.
    pub fn label(self) -> &'static str {
        match self {
            PhoneRow::Status => "LINK",
            PhoneRow::Pair => "PAIR",
            PhoneRow::Forget => "FORGET",
            PhoneRow::Notify => "NOTIFY",
            PhoneRow::Keys => "MEDIA KEY",
            PhoneRow::Media => "REMOTE",
        }
    }
}

/// The phone menu.
pub const PHONE: [PhoneRow; 6] = {
    use PhoneRow::*;
    [Status, Pair, Media, Notify, Keys, Forget]
};

/// A row of FITNESS ([`FITNESS`]) or SYSTEM's import: read from the rides and settings Bryton's firmware left on the
/// card ([`crate::stock`]); only the scan and the import act, on Select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockRow {
    /// Select reads every ride (a scan spread over the passes); the value shows how far.
    Scan,
    /// Select opens the power curve (app/curve.rs).
    Curve,
    /// Select opens the TRAINING LOAD chart (app/pmc.rs): CTL, ATL and TSB over 90 days.
    Load,
    /// Select opens the month totals.
    Months,
    /// Critical power from the last 90 days' power curve.
    Cp,
    /// W'.
    WPrime,
    /// How well the 2-parameter model fits: bests it rests on of those of 2 to 30 minutes, largest gap.
    Fit,
    /// The FTP the estimates give: CP when its fit is trusted, else HRVT2 when trusted (app/fitness.rs).
    EstFtp,
    /// Which CP and W' a ride started now takes: the fit's, or the settings' FTP and W'.
    Model,
    /// Select (asked twice) copies the estimated FTP into the FTP setting.
    ApplyFtp,
    /// Select (asked twice) copies the fit's W' into the W' setting.
    ApplyWPrime,
    /// The first heart rate variability threshold (DFA alpha1 0.75, about the aerobic threshold) from recent rides.
    Hrvt1,
    /// The second (alpha1 0.5).
    Hrvt2,
    /// How many recent rides gave HRVT1 and HRVT2, of those read.
    HrvRides,
    /// Distance this week.
    WeekKm,
    /// Riding time this week.
    WeekH,
    /// Distance this year.
    YearKm,
    /// Riding time this year.
    YearH,
    /// Distance, all rides.
    AllKm,
    /// Riding time, all rides.
    AllH,
    /// Select applies stock's settings (FTP, max heart rate, wheel, sensors, pages).
    Import,
}

impl StockRow {
    /// The row's label.
    pub fn label(self) -> &'static str {
        match self {
            StockRow::Scan => "READ RIDES",
            StockRow::Curve => "POWER CURVE",
            StockRow::Load => "TRAINING LOAD",
            StockRow::Months => "MONTHS",
            StockRow::Cp => "CP",
            StockRow::WPrime => "W'",
            StockRow::Fit => "CP FIT",
            StockRow::EstFtp => "EST FTP",
            StockRow::Model => "MODEL",
            StockRow::ApplyFtp => "APPLY FTP",
            StockRow::ApplyWPrime => "APPLY W'",
            StockRow::Hrvt1 => "HRVT1 AET",
            StockRow::Hrvt2 => "HRVT2",
            StockRow::HrvRides => "RIDES",
            StockRow::WeekKm => "WEEK KM",
            StockRow::WeekH => "WEEK H",
            StockRow::YearKm => "YEAR KM",
            StockRow::YearH => "YEAR H",
            StockRow::AllKm => "TOTAL KM",
            StockRow::AllH => "TOTAL H",
            StockRow::Import => "IMPORT",
        }
    }
}

/// The fitness menu (HISTORY > FITNESS): what stock's rides give. Their settings' import is in SYSTEM.
pub const FITNESS: [StockRow; 20] = {
    use StockRow::*;
    [
        Scan,
        Curve,
        Load,
        Months,
        Cp,
        WPrime,
        Fit,
        EstFtp,
        Model,
        ApplyFtp,
        ApplyWPrime,
        Hrvt1,
        Hrvt2,
        HrvRides,
        WeekKm,
        WeekH,
        YearKm,
        YearH,
        AllKm,
        AllH,
    ]
};

/// The main menu (off a ride): what to ride (workouts, courses, segments), what was ridden, then the set-up; seven rows, the list scrolls.
pub const MAIN: [Row; 8] = [
    Row::Open(Menu::Workouts),
    Row::Open(Menu::Courses),
    Row::Open(Menu::Segments),
    Row::Open(Menu::History),
    Row::Open(Menu::Pages),
    Row::Open(Menu::Sensors),
    Row::Open(Menu::Settings),
    Row::Open(Menu::Places),
];

/// SETTINGS: a menu per subject, each a screen of settings, so any setting is three levels down at most.
pub const SETTINGS: [Row; 5] = [
    Row::Open(Menu::Me),
    Row::Open(Menu::Alerts),
    Row::Open(Menu::Display),
    Row::Open(Menu::Bike),
    Row::Open(Menu::System),
];

/// ME: the rider's numbers the metrics are computed from.
pub const ME: [Row; 10] = [
    Row::Set(Setting::MaxHr),
    Row::Set(Setting::Lthr),
    Row::Set(Setting::ZonesBy),
    Row::Open(Menu::Zones),
    Row::Set(Setting::Ftp),
    Row::Set(Setting::Weight),
    Row::Set(Setting::WPrime),
    Row::Set(Setting::Sex),
    Row::Set(Setting::BirthYear),
    Row::Set(Setting::Height),
];

/// ZONES (ME > ZONES): the top of each heart-rate zone 1 to 4 and power zone 1 to 6 (zone 5 and 7 have none), each edited
/// in place, then RESET ZONES.
pub const ZONES: [Row; 11] = [
    Row::Set(Setting::Zone(0)),
    Row::Set(Setting::Zone(1)),
    Row::Set(Setting::Zone(2)),
    Row::Set(Setting::Zone(3)),
    Row::Set(Setting::Zone(4)),
    Row::Set(Setting::Zone(5)),
    Row::Set(Setting::Zone(6)),
    Row::Set(Setting::Zone(7)),
    Row::Set(Setting::Zone(8)),
    Row::Set(Setting::Zone(9)),
    Row::Set(Setting::ZonesReset),
];

/// ALERTS: the reminders and the limits (the list scrolls).
pub const ALERTS: [Row; 16] = [
    Row::Set(Setting::Eat),
    Row::Set(Setting::Drink),
    Row::Set(Setting::Limit(7)),
    Row::Set(Setting::Limit(8)),
    Row::Set(Setting::Limit(9)),
    Row::Set(Setting::HrAlert),
    Row::Set(Setting::Limit(6)),
    Row::Set(Setting::Limit(0)),
    Row::Set(Setting::Limit(1)),
    Row::Set(Setting::Limit(2)),
    Row::Set(Setting::Limit(3)),
    Row::Set(Setting::Limit(4)),
    Row::Set(Setting::Limit(5)),
    Row::Set(Setting::Limit(10)),
    Row::Set(Setting::SunsetAlert),
    Row::Set(Setting::SteepDescent),
];

/// DISPLAY: the backlight, the touch panel, the font, the power the workout gauge shows.
pub const DISPLAY: [Row; 14] = [
    Row::Set(Setting::Light),
    Row::Set(Setting::LightOff),
    Row::Set(Setting::Touch),
    Row::Set(Setting::Font),
    Row::Set(Setting::Theme),
    Row::Set(Setting::DayTheme),
    Row::Set(Setting::NightTheme),
    Row::Set(Setting::Units),
    Row::Set(Setting::DateFormat),
    Row::Set(Setting::TimeZone),
    Row::Set(Setting::DaylightSaving),
    Row::Set(Setting::MapUp),
    Row::Set(Setting::Temp),
    Row::Set(Setting::Sound),
];

/// BIKE: the wheel, the bike's weight, the ride profile, the barometer's altitude with its calibration from the GPS (a
/// ride setting, not a diagnostic), the climbs ahead, the GPS receiver's power mode and POWER MATCH, the trips, and
/// BIKES (the active bike, each bike's odometer and parts: app/bikes.rs).
pub const BIKE: [Row; 24] = [
    Row::Set(Setting::Wheel),
    Row::Set(Setting::BikeWeight),
    Row::Set(Setting::Altitude),
    Row::AltGps,
    Row::Set(Setting::BikeProfile),
    Row::Set(Setting::ClimbsAhead),
    Row::Set(Setting::ClimbsMedium),
    Row::Set(Setting::ClimbPage),
    Row::Set(Setting::GpsPower),
    Row::Set(Setting::GnssSystems),
    Row::Set(Setting::PowerMatch),
    Row::Set(Setting::LevelStep),
    Row::Set(Setting::SlopeScale),
    Row::Set(Setting::PowerSmooth),
    Row::Set(Setting::AutoLap),
    Row::Set(Setting::LapKm),
    Row::Set(Setting::LapMin),
    Row::Set(Setting::AutoPause),
    Row::Set(Setting::StartReminder),
    Row::Set(Setting::ReadyCheck),
    Row::Set(Setting::Trip(0)),
    Row::Set(Setting::Trip(1)),
    Row::Set(Setting::Odometer),
    Row::Open(Menu::Bikes),
];

/// SYSTEM: the card, stock's settings, the diagnostics (GPS, the BLE devices nearby, the built-in sensors), ABOUT.
pub const SYSTEM: [Row; 10] = [
    Row::Drive,
    Row::Stock(StockRow::Import),
    Row::Open(Menu::Gps),
    Row::Nearby,
    Row::Open(Menu::BuiltIn),
    Row::Open(Menu::Storage),
    Row::About,
    Row::Screenshot,
    Row::Set(Setting::AutoOff),
    Row::Set(Setting::BatterySaver),
];

/// What the page editor does to the selected cell or its page (Start in the editor: UI review 2, 5.2 point 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditRow {
    /// The field picker (groups, then fields).
    Field,
    /// A second cell beside it, in its row.
    SplitAcross,
    /// A row under its row.
    SplitDown,
    /// Merged with its neighbour: the other cell of its row, else the row under it.
    Merge,
    /// Its row one share taller.
    Taller,
    /// Its row one share shorter.
    Shorter,
    /// The last action undone (and done again).
    Undo,
    /// A new page after this one.
    AddPage,
    /// This page deleted (asked first).
    DeletePage,
    /// The editor's changes dropped (asked first), after DELETE PAGE and only when there are some: Back keeps them.
    Drop,
}

impl EditRow {
    /// The row's label (the two splits have an icon beside it: no dash in the text).
    pub fn label(self) -> &'static str {
        match self {
            EditRow::Field => "FIELD",
            EditRow::SplitAcross | EditRow::SplitDown => "SPLIT",
            EditRow::Merge => "MERGE",
            EditRow::Taller => "TALLER",
            EditRow::Shorter => "SHORTER",
            EditRow::Undo => "UNDO",
            EditRow::AddPage => "ADD PAGE",
            EditRow::DeletePage => "DELETE PAGE",
            EditRow::Drop => "DROP CHANGES",
        }
    }
}

/// The page editor's actions, the cell's first (one screen), then the page's.
pub const EDIT_ROWS: [EditRow; 9] = {
    use EditRow::*;
    [Field, SplitAcross, SplitDown, Merge, Taller, Shorter, Undo, AddPage, DeletePage]
};

/// The figures of STORAGE (SYSTEM > STORAGE), each read by the card's survey (storage/volume.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageRow {
    /// The volume's size.
    Total,
    /// Its free space.
    Free,
    /// The rides in \Activities.
    Rides,
}

/// STORAGE: the figures, then the oldest rides (their rows are left out while the card has fewer).
pub const STORAGE: [Row; 8] = [
    Row::Storage(StorageRow::Total),
    Row::Storage(StorageRow::Free),
    Row::Storage(StorageRow::Rides),
    Row::OldRide(0),
    Row::OldRide(1),
    Row::OldRide(2),
    Row::OldRide(3),
    Row::OldRide(4),
];

/// The built-in sensors' readings (SYSTEM > BUILT IN).
pub const BUILT_IN: [Reading; 3] = [Reading::Lux, Reading::TempC, Reading::Hpa];

/// The GPS menu (all but the first only once the receiver has reported).
pub const GPS: [Reading; 7] =
    [Reading::Fix, Reading::Sats, Reading::Lat, Reading::Lon, Reading::Alt, Reading::Utc, Reading::Date];

/// The first row shown when row `sel` of a `total`-row menu is selected: menus longer than the screen turn a whole
/// screen at a time, so moving within a screen redraws two rows, not all of them. The last screen starts at
/// `total - rows`, so it is full (SYSTEM's last screen showed one row and six empty slots, 2026-10-29).
///
/// ```
/// use bryton_rustfw::menu::first_shown;
/// assert_eq!([0, 5, 6, 7, 12].map(|s| first_shown(s, 6, 13)), [0, 0, 6, 7, 7]);
/// assert_eq!(first_shown(3, 6, 4), 0);
/// ```
pub fn first_shown(sel: usize, rows: usize, total: usize) -> usize {
    let rows = rows.max(1);
    // the shift to the last screen happens once, at its first row: inside it, moving costs two rows
    if total > rows { if sel >= total - rows { total - rows } else { sel / rows * rows } } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The service interval's row is named SERVICE; "SERVICE EVERY" left no gap before OFF.
    #[test]
    fn service_row_is_named_service() {
        assert_eq!(Row::PartEvery(0, 0).label(), Some("SERVICE"));
    }
}
