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
    /// Back: out of a menu (on a page, the backlight on or off).
    Back,
    /// Page held: the menu.
    Menu,
    /// Start held: end a paused ride.
    End,
    /// Back held: power off.
    Off,
    /// Page or Lap still held: [`Input::Next`] or [`Input::Prev`] again (a list scrolls, a value steps).
    NextHeld,
    /// See [`Input::NextHeld`].
    PrevHeld,
    /// A gesture on the touch panel: on a page, swipe left the next page, swipe right the previous one, hold the
    /// menu; in a menu, a tap selects that row, swipe up or down turns a screen of rows, swipe right is Back; while a
    /// number is edited, swipe up or down steps it, a tap keeps it.
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
    /// Max heart rate (a number).
    MaxHr,
    /// FTP (a number), also the CP of W' balance.
    Ftp,
    // power.rs
    /// Rider weight (a number), for W/kg.
    Weight,
    /// W' for W' balance (a number, J, shown in kJ).
    WPrime,
    // alerts.rs (0: off)
    /// Eat reminder, every so many minutes of ride time.
    Eat,
    /// Drink reminder, the same.
    Drink,
    /// Heart rate alert over this, bpm.
    HrAlert,
}

impl Setting {
    /// The row's label.
    pub fn label(self) -> &'static str {
        match self {
            Setting::Wheel => "WHEEL MM",
            Setting::Light => "LIGHT %",
            Setting::LightOff => "LIGHT OFF",
            Setting::MaxHr => "MAX HR",
            Setting::Ftp => "FTP W",
            Setting::Weight => "WEIGHT KG",
            Setting::WPrime => "W' KJ",
            Setting::Eat => "EAT MIN",
            Setting::Drink => "DRINK MIN",
            Setting::HrAlert => "HR OVER",
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
            Setting::Eat | Setting::Drink => Some((0..=120, 5)),
            Setting::HrAlert => Some((0..=230, 5)),
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
/// W' offered, J (0.5 kJ steps).
pub const WPRIME_RANGE: core::ops::RangeInclusive<u16> = 5000..=40_000;
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
    /// Opens the list of BLE devices nearby (and starts a scan).
    Nearby,
    /// A setting.
    Set(Setting),
    /// ANT sensor `n` ([`crate::sensors::Kind::ALL`]): Select pairs or forgets it.
    Sensor(usize),
    /// A reading.
    Read(Reading),
    /// Ride page `n`'s field count.
    Layout(usize),
    /// Ride page `n`'s field `i`.
    Field(usize, usize),
    /// A row of the stock data menu.
    Stock(StockRow),
}

/// What the stock data menu shows ([`STOCK`]): read from the rides and settings Bryton's firmware left on the
/// card ([`crate::stock`]); only the scan and the import act, on Select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StockRow {
    /// Select reads every ride (a scan spread over the passes); the value shows how far.
    Scan,
    /// Critical power from the last 90 days' power curve.
    Cp,
    /// W'.
    WPrime,
    /// How well the 2-parameter model fits: points used, largest error.
    Fit,
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
            StockRow::Cp => "CP W",
            StockRow::WPrime => "W' KJ",
            StockRow::Fit => "CP FIT",
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

/// The stock data menu.
pub const STOCK: [StockRow; 11] = {
    use StockRow::*;
    [Scan, Cp, WPrime, Fit, WeekKm, WeekH, YearKm, YearH, AllKm, AllH, Import]
};

/// The alerts menu.
pub const ALERTS: [Row; 3] = [Row::Set(Setting::Eat), Row::Set(Setting::Drink), Row::Set(Setting::HrAlert)];

/// The main menu.
pub const MAIN: [Row; 16] = [
    Row::Open(Menu::Sensors),
    Row::Open(Menu::Page(crate::OVERVIEW)),
    Row::Open(Menu::Page(0)),
    Row::Open(Menu::Page(1)),
    Row::Open(Menu::Page(2)),
    Row::Set(Setting::Wheel),
    Row::Set(Setting::Light),
    Row::Set(Setting::LightOff),
    Row::Set(Setting::MaxHr),
    Row::Set(Setting::Ftp),
    Row::Set(Setting::Weight),
    Row::Set(Setting::WPrime),
    Row::Open(Menu::Alerts),
    Row::Open(Menu::Gps),
    Row::Nearby,
    Row::Open(Menu::Stock),
];

/// The built-in sensors' readings, under the ANT sensors in the sensors menu.
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
