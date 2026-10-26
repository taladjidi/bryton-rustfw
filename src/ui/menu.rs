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
    /// menu (not while a ride records); in a menu, a tap selects a row and a second tap on it acts, the "<" is Back,
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
    /// The touch panel on or off (keys always work).
    Touch,
    /// Max heart rate (a number).
    MaxHr,
    /// FTP (a number), also the CP of W' balance.
    Ftp,
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
    /// The power the workout gauge shows (a choice of [`crate::WKT_AVGS`]: 3, 10, 30 s or each second).
    WktAvg,
    /// What a shifter button press does ([`Remote`]), for press `n` ([`crate::sensors::Press`]).
    Remote(u8),
    /// The barometric altitude (a number, m): typing it calibrates the barometer to it (altitude.rs).
    Altitude,
    /// The ride profile: AUTO (indoor when a trainer is heard at Start), ROAD or INDOOR ([`crate::ride::Profile`]).
    BikeProfile,
    /// Climbs found on the road ahead when no course is followed, on or off (`climbs_ahead`).
    ClimbsAhead,
    /// The font of every text: PIXEL (dot matrix, the default) or SMOOTH ([`crate::fonts::smooth`]).
    Font,
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
pub const REMOTE: [Row; 4] = [
    Row::Set(Setting::Remote(0)),
    Row::Set(Setting::Remote(1)),
    Row::Set(Setting::Remote(2)),
    Row::Set(Setting::Remote(3)),
];

impl Setting {
    /// The row's label: what it is, in words; the unit goes after the value ([`Setting::unit`]).
    pub fn label(self) -> &'static str {
        match self {
            Setting::Wheel => "WHEEL",
            Setting::Light => "BRIGHTNESS",
            Setting::LightOff => "OFF AFTER",
            Setting::Touch => "TOUCH",
            Setting::MaxHr => "MAX HR",
            Setting::Ftp => "FTP",
            Setting::Weight => "WEIGHT",
            Setting::WPrime => "W'",
            Setting::BikeWeight => "WEIGHT",
            Setting::Eat => "EAT",
            Setting::Drink => "DRINK",
            Setting::HrAlert => "HR ABOVE",
            Setting::WktAvg => "GAUGE POWER",
            Setting::Altitude => "ALTITUDE",
            Setting::BikeProfile => "PROFILE",
            Setting::ClimbsAhead => "CLIMBS AHEAD",
            Setting::Font => "FONT",
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
            Setting::MaxHr | Setting::HrAlert => " BPM",
            Setting::Ftp => " W",
            Setting::Weight | Setting::BikeWeight => " KG",
            Setting::WPrime => " KJ",
            Setting::Eat | Setting::Drink => " MIN",
            Setting::Altitude => " M",
            _ => "",
        }
    }

    /// For a number edited in place (Select, then up and down): its range and step. None for a choice, which
    /// Select moves to the next value.
    pub fn number(self) -> Option<(core::ops::RangeInclusive<u16>, u16)> {
        match self {
            Setting::MaxHr => Some((MAX_HR_RANGE, 1)),
            Setting::Ftp => Some((FTP_RANGE, 5)),
            Setting::Weight => Some((WEIGHT_RANGE, 1)),
            Setting::WPrime => Some((WPRIME_RANGE, 500)),
            Setting::BikeWeight => Some((BIKE_RANGE, 1)),
            Setting::Eat | Setting::Drink => Some((0..=120, 5)),
            Setting::HrAlert => Some((0..=230, 5)),
            // -500 to 9000 m, kept + 500 (the Dead Sea's shore to above Everest); 1 m steps (a known height to the metre; a held key repeats)
            Setting::Altitude => Some((0..=9500, 1)),
            _ => None,
        }
    }
}

/// Max heart rates offered, bpm.
pub const MAX_HR_RANGE: core::ops::RangeInclusive<u16> = 100..=230;
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
    /// FROM GPS: Select averages the receiver's altitude for 30 s and calibrates the barometer to it.
    AltGps,
    /// The USB drive: Select lends the card to the computer, or takes it back (app/drive.rs).
    Drive,
    /// GIVE UP SAVE, first in SYSTEM only while a save is stuck (app/riding.rs, `give_up_save`).
    GiveUpSave,
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
    /// A row of the page editor's actions ([`EDIT_ROWS`]).
    Edit(EditRow),
    /// Ride page `n`'s field `i`.
    Field(usize, usize),
    /// A row of the stock data menu.
    Stock(StockRow),
    /// Notification `n` of the NOTIFICATIONS list, newest first: Select opens its detail.
    Notif(usize),
    /// Past ride `n` of the history, newest first: Select opens its card.
    Past(usize),
    /// Planned workout `n` of the calendar ([`crate::storage::workout::Calendar`]): Select shows its steps.
    Workout(usize),
    /// Select starts the workout shown (and the ride, if none runs).
    WorkoutStart,
    /// The FTP ramp test (ramp.rs, app/ramp.rs): Select starts it, with a trainer heard.
    RampTest,
    /// Step `n` of the workout shown: Select starts the workout from it.
    Step(usize),
    /// Course `n` of the list ([`crate::storage::course::List`]): Select follows it, or stops following it.
    Course(usize),
    /// Group `g` of [`crate::ui::FIELD_GROUPS`]: Select lists its fields.
    Group(usize),
    /// A field of a group: Select puts it in the page's box and goes back to the page's menu.
    Pick(crate::ui::Field),
    /// A row of the phone menu.
    Phone(PhoneRow),
    /// A row of the ride menu that acts on the ride, the workout or the course.
    Ride(RideRow),
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
    /// Opens the trainer screen (app/trainer.rs).
    Trainer,
    /// Opens the power meter calibration screen (app/powercal.rs).
    CalPower,
    /// INDOOR ON / OFF: the ride's profile by hand (app/profile.rs).
    Indoor,
    /// PAGES: which set the list and the editor show, ROAD or INDOOR (the ride's own while one is open).
    PageSet,
}

/// The workout menu of a ride (while a step runs).
pub const WORKOUT_NOW: [Row; 2] = [Row::Ride(RideRow::NextStep), Row::Ride(RideRow::StopWorkout)];

/// The course menu of a ride (while one is followed).
pub const COURSE_NOW: [Row; 1] = [Row::Ride(RideRow::StopCourse)];

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
pub const FITNESS: [StockRow; 17] = {
    use StockRow::*;
    [
        Scan,
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

/// The main menu (off a ride): what to ride, what was ridden, then the set-up; six rows, one screen.
pub const MAIN: [Row; 6] = [
    Row::Open(Menu::Workouts),
    Row::Open(Menu::Courses),
    Row::Open(Menu::History),
    Row::Open(Menu::Pages),
    Row::Open(Menu::Sensors),
    Row::Open(Menu::Settings),
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
pub const ME: [Row; 4] =
    [Row::Set(Setting::MaxHr), Row::Set(Setting::Ftp), Row::Set(Setting::Weight), Row::Set(Setting::WPrime)];

/// ALERTS: the reminders and the heart rate alert.
pub const ALERTS: [Row; 3] = [Row::Set(Setting::Eat), Row::Set(Setting::Drink), Row::Set(Setting::HrAlert)];

/// DISPLAY: the backlight, the touch panel, the font, the power the workout gauge shows.
pub const DISPLAY: [Row; 5] = [
    Row::Set(Setting::Light),
    Row::Set(Setting::LightOff),
    Row::Set(Setting::Touch),
    Row::Set(Setting::Font),
    Row::Set(Setting::WktAvg),
];

/// BIKE: the wheel, the bike's weight, the ride profile, the barometer's altitude with its calibration from the GPS (a
/// ride setting, not a diagnostic), and the climbs ahead.
pub const BIKE: [Row; 6] = [
    Row::Set(Setting::Wheel),
    Row::Set(Setting::BikeWeight),
    Row::Set(Setting::Altitude),
    Row::AltGps,
    Row::Set(Setting::BikeProfile),
    Row::Set(Setting::ClimbsAhead),
];

/// SYSTEM: the card, stock's settings, the diagnostics (GPS, the BLE devices nearby, the built-in sensors), ABOUT.
pub const SYSTEM: [Row; 6] =
    [Row::Drive, Row::Stock(StockRow::Import), Row::Open(Menu::Gps), Row::Nearby, Row::Open(Menu::BuiltIn), Row::About];

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
        }
    }
}

/// The page editor's actions, the cell's first (one screen), then the page's.
pub const EDIT_ROWS: [EditRow; 9] = {
    use EditRow::*;
    [Field, SplitAcross, SplitDown, Merge, Taller, Shorter, Undo, AddPage, DeletePage]
};

/// The built-in sensors' readings (SYSTEM > BUILT IN).
pub const BUILT_IN: [Reading; 3] = [Reading::Lux, Reading::TempC, Reading::Hpa];

/// The GPS menu (all but the first only once the receiver has reported).
pub const GPS: [Reading; 7] =
    [Reading::Fix, Reading::Sats, Reading::Lat, Reading::Lon, Reading::Alt, Reading::Utc, Reading::Date];

/// The first row shown when row `sel` of a menu is selected: menus longer than the screen turn a whole screen at a
/// time, so moving within a screen redraws two rows, not all of them.
///
/// ```
/// use bryton_rustfw::menu::first_shown;
/// assert_eq!([0, 12, 13, 27].map(|s| first_shown(s, 13)), [0, 0, 13, 26]);
/// ```
pub fn first_shown(sel: usize, rows: usize) -> usize {
    sel / rows.max(1) * rows.max(1)
}
