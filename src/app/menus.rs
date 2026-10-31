//! The menu screens: each menu's rows (from the tables in [`menu`], plus the lists: sensors, history, workouts,
//! courses), their text, the redraw of the rows that changed (by fingerprint), and [`App::activate`], what a row
//! does when chosen.

use crate::*;

/// How long a BLE scan runs, started when the device list opens. Scanning keeps the Nordic radio on, so it is short.
pub const SCAN_S: u16 = 10;

/// The device list and the sensors menu are redrawn at most this often while reports come in.
pub const LIST_REDRAW_MS: u32 = 1000;

/// The menus (the tree: docs/features.md, "Menu").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// The main menu ([`menu::MAIN`]): workouts, courses, segments, history, pages, sensors, settings, places.
    Main,
    /// CALIBRATE POWER and TRAINER CONTROL first (when they apply), one row per sensor kind, BT TRAINER, the Di2
    /// buttons and the phone.
    Sensors,
    /// The three sets of ride pages: ROAD, INDOOR and MTB PAGES, each opening its list ([`Menu::PageList`]).
    Pages,
    /// The overview, the ride pages and ADD PAGE of one set (ROAD, INDOOR or MTB).
    PageList(ride::Profile),
    /// One menu per subject ([`menu::SETTINGS`]).
    Settings,
    /// The rider's numbers ([`menu::ME`]).
    Me,
    /// The zones' bounds, one by one ([`menu::ZONES`]).
    Zones,
    /// The backlight, the touch panel, the gauge ([`menu::DISPLAY`]).
    Display,
    /// The wheel ([`menu::BIKE`]).
    Bike,
    /// The card, stock's settings, the diagnostics, ABOUT ([`menu::SYSTEM`]).
    System,
    /// The built-in sensors' readings ([`menu::BUILT_IN`]).
    BuiltIn,
    /// STORAGE (SYSTEM > STORAGE): the card's size, free space, rides and the oldest of them (storage/volume.rs).
    Storage,
    /// The ride menu, Page held while a ride records or is paused, over ride page `n` ([`OVERVIEW`] from the
    /// overview): RESUME or PAUSE, END RIDE, WORKOUT and COURSE while one runs, BRIGHTNESS, SENSORS, EDIT THIS PAGE,
    /// ALL SETTINGS (the main menu). Rows that do not apply are not shown.
    Ride(usize),
    /// The workout running ([`menu::WORKOUT_NOW`]).
    WorkoutNow,
    /// The course followed ([`menu::COURSE_NOW`]).
    CourseNow,
    /// Ride page `n`'s editor, layout and fields (the overview's fields).
    Page(usize),
    /// The page editor's actions on cell `k` of the page it edits ([`menu::EDIT_ROWS`]).
    Edit(usize),
    /// The field groups, for box `i` of page `n` (Select on a field row).
    Fields(usize, usize),
    /// Group `g`'s fields, for box `i` of page `n`.
    Group(usize, usize, usize),
    /// What the GNSS receiver reports.
    Gps,
    /// FITNESS: what stock's rides give ([`menu::FITNESS`]), first in the history.
    Stock,
    /// Reminders and alerts ([`menu::ALERTS`]).
    Alerts,
    /// Stock's rides, newest first ([`history`]).
    History,
    /// The workouts planned from today on, one row each.
    Workouts,
    /// The steps of the workout opened from [`Menu::Workouts`].
    Steps,
    /// The course files on the card ([`storage::course_file`]).
    Courses,
    /// LIVE SEGMENTS, TARGET, then the segments of the card (name, distance, gain), then the files skipped.
    Segments,
    /// The climbs ridden to the top this ride, newest first (app/climbs_done.rs).
    Climbs,
    /// FITNESS > MONTHS: a row per month of the scan's rides, newest first (app/pmc.rs).
    Months,
    /// The phone link ([`menu::PHONE`]).
    Phone,
    /// What the Di2 buttons do ([`menu::REMOTE`]).
    Remote,
    /// The last notifications from the phone, newest first ([`crate::phone::Inbox`]).
    Notifs,
    /// The Bluetooth trainers heard by the search (app/ftms.rs): Select pairs one.
    BtTrainers,
    /// SAVE PLACE, then the favourite places (app/places.rs): Select opens one.
    Places,
    /// Place `n`: its ALTITUDE (typed by hand) and DELETE.
    Place(usize),
    /// BIKES (BIKE > BIKES, app/bikes.rs): ACTIVE BIKE, then the three bikes.
    Bikes,
    /// Bike `k`: its odometer and its parts.
    BikeParts(usize),
    /// Part `p` of bike `k`: SERVICE EVERY and RESET.
    Part(usize, usize),
}

/// A menu's title (a page's is followed by its number).
pub(crate) fn menu_title(m: Menu) -> &'static str {
    match m {
        Menu::Main => "MENU",
        Menu::Sensors => "SENSORS",
        Menu::Pages => "PAGES",
        Menu::PageList(ride::Profile::Road) => "ROAD PAGES",
        Menu::PageList(ride::Profile::Indoor) => "INDOOR PAGES",
        Menu::PageList(ride::Profile::Mtb) => "MTB PAGES",
        Menu::PageList(_) => "ROAD PAGES", // GRAVEL, CX and COMMUTE ride the road set (never opened)
        Menu::Settings => "SETTINGS",
        Menu::Me => "ME",
        Menu::Zones => "ZONES",
        Menu::Display => "DISPLAY",
        Menu::Bike => "BIKE",
        Menu::System => "SYSTEM",
        Menu::BuiltIn => "BUILT IN",
        Menu::Storage => "STORAGE",
        Menu::Ride(_) => "RIDE",
        Menu::WorkoutNow => "WORKOUT",
        Menu::CourseNow => "COURSE",
        Menu::Page(_) => "PAGE ",
        Menu::Edit(_) => "BOX ",
        Menu::Fields(..) => "FIELD",
        Menu::Group(_, _, g) => ui::FIELD_GROUPS.get(g).map_or("", |g| g.0),
        Menu::Gps => "GPS",
        Menu::Stock => "FITNESS",
        Menu::Alerts => "ALERTS",
        Menu::History => "HISTORY",
        Menu::Workouts => "WORKOUTS",
        Menu::Steps => "WORKOUT",
        Menu::Courses => "COURSES",
        Menu::Segments => "SEGMENTS",
        Menu::Climbs => "CLIMBS DONE",
        Menu::Months => "MONTHS",
        Menu::Phone => "PHONE",
        Menu::Remote => "DI2 KEYS",
        Menu::Notifs => "NOTIFICATIONS",
        Menu::BtTrainers => "BT TRAINERS",
        Menu::Places => "PLACES",
        Menu::Place(_) => "PLACE",
        Menu::Bikes => "BIKES",
        Menu::BikeParts(k) => bike_name(k),
        Menu::Part(_, p) => bikes::PART_NAMES.get(p).copied().unwrap_or("PART"),
    }
}

/// A page's layout by its name ("CUSTOM" for one made in the editor).
pub(crate) fn layout_text(l: &mut Line<12>, p: &page::Page) {
    l.push_str(p.preset_id().and_then(ui::layout).map_or("CUSTOM", |(_, name)| name));
}

impl App {
    /// Row `i` of `menu`; None past its last.
    pub(crate) fn row(&self, menu: Menu, i: usize) -> Option<menu::Row> {
        use menu::Row;
        match menu {
            Menu::Main => menu::MAIN.get(i).copied(),
            Menu::Pages => menu::PAGES.get(i).copied(),
            // the overview first, then the pages, then ADD PAGE
            Menu::PageList(_) => match i.checked_sub(1) {
                None => Some(Row::Open(Menu::Page(OVERVIEW))),
                Some(k) if k < self.view_count() => Some(Row::Open(Menu::Page(k))),
                Some(k) => (k == self.view_count()).then_some(Row::AddPage),
            },
            // DROP CHANGES last, once there is something to drop
            Menu::Edit(_) if i == menu::EDIT_ROWS.len() => {
                self.edit.is_some_and(|e| e.work != e.orig).then_some(Row::Edit(menu::EditRow::Drop))
            }
            Menu::Edit(_) => menu::EDIT_ROWS.get(i).map(|&r| Row::Edit(r)),
            Menu::Settings => menu::SETTINGS.get(i).copied(),
            Menu::Me => menu::ME.get(i).copied(),
            Menu::Zones => menu::ZONES.get(i).copied(),
            Menu::Display => menu::DISPLAY.get(i).copied(),
            Menu::Bike => menu::BIKE.get(i).copied(),
            // GIVE UP SAVE first, only while a save is stuck (app/riding.rs)
            Menu::System if self.save_stuck_any() && i == 0 => Some(Row::GiveUpSave),
            Menu::System => menu::SYSTEM.get(i - usize::from(self.save_stuck_any())).copied(),
            Menu::BuiltIn => menu::BUILT_IN.get(i).map(|&r| Row::Read(r)),
            // an oldest ride's row only while the survey has it
            Menu::Storage => menu::STORAGE
                .get(i)
                .copied()
                .filter(|r| !matches!(r, Row::OldRide(k) if self.survey.oldest(*k).is_none())),
            Menu::Ride(n) => {
                use menu::RideRow;
                let rows = [
                    (Row::Ride(RideRow::Pause), true),
                    (Row::Ride(RideRow::End), true),
                    (Row::Open(Menu::WorkoutNow), self.workout_running()),
                    (Row::Open(Menu::CourseNow), self.page_on(COURSE)),
                    (Row::Open(Menu::Climbs), self.climbs_done.len() > 0), // once one is done
                    (Row::Ride(RideRow::BackToStart), self.track.points().len() >= 2),
                    (Row::Ride(RideRow::Trainer), self.trainer_on()),
                    (Row::Ride(RideRow::Indoor), true),
                    (Row::Set(menu::Setting::Light), true),
                    (Row::Open(Menu::Sensors), true),
                    // the course page has no settings; no page edited while recording (UI review 2, 5.2 point 8)
                    (Row::Open(Menu::Page(n)), n <= OVERVIEW && self.ride.state() != ride::State::Running),
                    (Row::Ride(RideRow::Curve), self.power_paired()),
                    // stopped only: a tap acts at once then (a row more scrolls the menu under a second tap)
                    (Row::SavePlace, !self.ride.moving()),
                    (Row::Open(Menu::Main), true),
                ];
                rows.into_iter().filter(|r| r.1).map(|r| r.0).nth(i)
            }
            Menu::WorkoutNow => menu::WORKOUT_NOW.get(i).copied(),
            Menu::CourseNow => menu::COURSE_NOW.get(i).copied(),
            Menu::Fields(..) => (i < ui::FIELD_GROUPS.len()).then_some(Row::Group(i)),
            Menu::Group(_, _, g) => ui::FIELD_GROUPS.get(g)?.1.get(i).map(|&f| Row::Pick(f)),
            // the actions first (the rider comes here to calibrate or to control the trainer), then the sensors
            Menu::Sensors => {
                let top = [
                    (Row::Ride(menu::RideRow::CalPower), self.power_heard()),
                    (Row::Ride(menu::RideRow::Trainer), self.trainer_on()),
                ];
                let top = top.into_iter().filter(|r| r.1).map(|r| r.0);
                let n = top.clone().count();
                let (j, kinds) = (i.checked_sub(n), Kind::ALL.len());
                match j {
                    None => top.clone().nth(i),
                    Some(j) if j < kinds => Some(Row::Sensor(j)),
                    Some(j) => {
                        [Row::BtTrainer, Row::Open(Menu::Remote), Row::Open(Menu::Phone)].get(j - kinds).copied()
                    }
                }
            }
            Menu::Remote => menu::REMOTE.get(i).copied(),
            Menu::Gps => menu::GPS.get(i).filter(|_| i == 0 || self.gps.pvt.is_some()).map(|&r| Row::Read(r)),
            Menu::Stock => menu::FITNESS.get(i).map(|&r| Row::Stock(r)),
            Menu::Alerts => menu::ALERTS.get(i).copied(),
            // FITNESS first, then the rides; none yet: the scan's row, which shows how far it is
            Menu::History => match i.checked_sub(1) {
                None => Some(Row::Open(Menu::Stock)),
                Some(0) if self.stock.index.is_empty() => Some(Row::Stock(menu::StockRow::Scan)),
                Some(k) => self.stock.index.get(k).map(|_| Row::Past(k)),
            },
            // the planned workouts (or the row that says none), then the built-in ramp test
            Menu::Workouts => {
                let n = self.plan.calendar.iter().count().max(1);
                match i {
                    _ if i == n => Some(Row::RampTest),
                    _ if i == n + 1 => Some(Row::RideCourse),
                    0 if self.plan.calendar.get(0).is_none() => Some(Row::Empty("NONE PLANNED")),
                    _ => self.plan.calendar.get(i).map(|_| Row::Workout(i)),
                }
            }
            // no START while the workout runs: it would play it again from the top
            Menu::Steps if i == 0 && !self.workout_running() => Some(Row::WorkoutStart),
            Menu::Steps => {
                let k = if self.workout_running() { i } else { i.checked_sub(1)? };
                (k < self.plan.steps().len()).then_some(Row::Step(k))
            }
            Menu::Courses => self.courses.as_deref()?.list.get(i).map(|_| Row::Course(i)),
            Menu::Segments => {
                let s = self.segments();
                let (n, k) = (s.map_or(0, Segments::indexed), i.checked_sub(FIXED_ROWS));
                match (i, k) {
                    (0, _) => Some(Row::Set(menu::Setting::LiveSegments)),
                    (1, _) => Some(Row::Set(menu::Setting::SegTarget)),
                    (_, Some(0)) if n == 0 => {
                        Some(Row::Empty(if s.is_some_and(Segments::ready) { "NONE ON CARD" } else { "READING CARD" }))
                    }
                    (_, Some(k)) if k < n => Some(Row::Seg(k)),
                    (_, Some(k)) if k == n && s.is_some_and(|s| s.skipped > 0) => Some(Row::SegSkipped),
                    _ => None,
                }
            }
            Menu::Climbs if self.climbs_done.len() == 0 => (i == 0).then_some(Row::Empty("NONE YET")),
            Menu::Climbs => self.climbs_done.get(i).map(|_| Row::Done(i)),
            Menu::Months if self.pmc.load.months().is_empty() => (i == 0).then_some(Row::Empty("NONE YET")),
            Menu::Months => self.pmc.load.months().get(i).map(|_| Row::Month(i)),
            Menu::Phone => menu::PHONE
                .get(i)
                .map(|&r| Row::Phone(r))
                .or_else(|| (i == menu::PHONE.len()).then_some(Row::Open(Menu::Notifs))),
            Menu::Notifs if self.phone.inbox.is_empty() => (i == 0).then_some(Row::Empty("NO NOTIFICATIONS")),
            Menu::Notifs => self.phone.inbox.get(i).map(|_| Row::Notif(i)),
            Menu::BtTrainers if self.ftms.n_found == 0 => {
                (i == 0).then_some(Row::Empty(if self.radio.scanning { "SEARCHING" } else { "NONE FOUND" }))
            }
            Menu::BtTrainers => (i < self.ftms.n_found).then_some(Row::BtFound(i)),
            // SAVE PLACE first, then the places (or the row that says none)
            Menu::Places => match i.checked_sub(1) {
                None => Some(Row::SavePlace),
                Some(0) if self.places.list.is_empty() => Some(Row::Empty("NONE SAVED")),
                Some(k) => self.places.list.get(k).map(|_| Row::Place(k)),
            },
            Menu::Place(k) if k < self.places.list.len() => {
                [Row::Set(menu::Setting::Altitude), Row::DeletePlace(k)].get(i).copied()
            }
            Menu::Place(_) => None,
            Menu::Bikes | Menu::BikeParts(_) | Menu::Part(..) => self.bike_rows(menu, i),
            Menu::Page(OVERVIEW) => (i < menu::OVERVIEW_ROWS).then_some(Row::Field(OVERVIEW, i)),
            Menu::Page(QUICK) => (i < menu::QUICK_ROWS).then_some(Row::Field(QUICK, i)),
            Menu::Page(n) => {
                let head = [Row::EditPage(n), Row::Layout(n)];
                let Some(j) = i.checked_sub(head.len()) else { return head.get(i).copied() };
                let cells = self.page_ref(n)?.cells();
                if j < cells {
                    Some(Row::Field(n, j))
                } else {
                    self.page_ops(n).nth(j - cells).map(|o| Row::PageOp(n, o))
                }
            }
        }
    }

    /// How many rows `menu` has.
    pub(crate) fn rows(&self, menu: Menu) -> usize {
        // the longest lists: HISTORY's FITNESS row and its 128 rides (history::RIDES), a workout's steps (up to 97)
        const MAX_ROWS: usize = crate::history::RIDES + 2;
        (0..MAX_ROWS).take_while(|&i| self.row(menu, i).is_some()).count()
    }

    /// The dev link's `where` answer: `TITLE|row/rows|LABEL` in a menu (row counted from 0), `OVERVIEW`, `RIDE n` or
    /// `SCREEN name` otherwise. Read-only: built from what drawing computes, nothing is stored.
    pub(crate) fn where_text(&self) -> Line<48> {
        use core::fmt::Write;
        let mut l = Line::new();
        let _ = match self.screen {
            Screen::Main => l.write_str("OVERVIEW"),
            Screen::Ride(n) => write!(l, "RIDE {n}"),
            Screen::Menu(m, i) => {
                let label = self.menu_row(m, i).map_or(Line::<16>::new(), |r| r.0);
                write!(l, "{}|{i}/{}|{}", menu_title(m), self.rows(m), label.as_str())
            }
            s => write!(l, "SCREEN {}", s.name()),
        };
        l
    }

    /// Menu row `i` of `menu`: label, value, value colour; None past the last row.
    pub(crate) fn menu_row(&self, menu: Menu, i: usize) -> Option<(Line<16>, Line<12>, u16)> {
        use menu::{Reading, Row};
        let (mut label, mut value, mut color) = (Line::new(), Line::new(), ui::WHITE);
        let row = self.row(menu, i)?;
        // the fixed text of a row is the table's (Row::label); only the rows that depend on state are set here
        let edit_page = matches!(row, Row::Open(Menu::Page(_))) && matches!(menu, Menu::Ride(_));
        if edit_page {
            label.push_str("EDIT THIS PAGE");
        } else if let Some(text) = row.label() {
            label.push_str(text);
        }
        match row {
            _ if edit_page => {}
            Row::Empty(_) => color = ui::GREY,
            Row::Ride(r) => {
                use menu::RideRow;
                if r == RideRow::Pause {
                    label.push_str(if self.ride.state() == ride::State::Running { "PAUSE" } else { "RESUME" });
                }
                if r == RideRow::Indoor {
                    value.push_str(if self.indoors() { "ON" } else { "OFF" });
                }
            }
            Row::Open(Menu::Page(OVERVIEW | QUICK)) => {}
            Row::Open(Menu::Page(n)) => {
                self.page_label(n, &mut label);
                layout_text(&mut value, self.page_ref(n)?);
            }
            Row::AutoScroll => match self.auto_secs() {
                0 => value.push_str("OFF"),
                s => {
                    value.push_u32(s);
                    value.push_str(" S");
                }
            },
            Row::PageOp(n, o) => match o {
                menu::PageOp::Show => value.push_str(if self.sw.meta.on(n) { "ON" } else { "OFF" }),
                menu::PageOp::Rename => self.rename_text(n, &mut value),
                _ => {}
            },
            Row::AddPage => {
                value.push_u32(self.view_count() as u32);
                value.push_str("/");
                value.push_u32(PAGES as u32);
            }
            Row::Edit(r) => {
                if r == menu::EditRow::DeletePage && self.edit.is_some_and(|e| e.more) {
                    label.clear();
                    label.push_str("RESET MORE");
                }
                self.edit_row_value(menu, r, &mut value, &mut color);
            }
            Row::Open(Menu::BikeParts(_))
            | Row::ActiveBike
            | Row::BikeOdo(_)
            | Row::Part(..)
            | Row::PartEvery(..)
            | Row::PartReset(..) => color = self.bike_value(row, &mut value),
            Row::Open(_) | Row::Nearby | Row::About | Row::EditPage(_) | Row::Screenshot => {}
            Row::BtTrainer => {
                let heard = self.ftms_on() && self.trainer_heard();
                value.push_str(match (self.sensors_store.bt, self.ble) {
                    (_, Some(v)) if !v.central => "NO LINK",
                    (_, None) => "NO BLE",
                    (None, _) => "OFF",
                    (Some(_), _) if !self.ftms_on() => "ANT FIRST",
                    (Some(_), _) if heard && self.ftms.control => "LINKED",
                    (Some(_), _) if heard => "NO CONTROL",
                    (Some(_), _) => "PAIRED",
                });
                if heard {
                    color = ui::GREEN;
                }
            }
            Row::BtFound(k) => {
                let d = self.ftms.found.get(k)?;
                let fec = d.proto == crate::fecble::Proto::Fec;
                label.push_str(match (d.name().is_empty(), fec) {
                    (false, _) => d.name(),
                    (true, false) => "FTMS TRAINER",
                    (true, true) => "FE-C TRAINER",
                });
                value.push_str(if fec { "FE-C " } else { "FTMS " });
                value.push_str(if d.rssi < 0 { "-" } else { "" });
                value.push_u32(u32::from(d.rssi.unsigned_abs()));
            }
            Row::GiveUpSave => {
                value.push_str("STUCK");
                color = ui::RED;
            }
            Row::Drive => {
                value.push_str(if self.emmc.lent() { "ON" } else { "OFF" });
            }
            Row::Storage(s) => match s {
                menu::StorageRow::Total => volume::size_text(&mut value, self.survey.total()),
                menu::StorageRow::Free => {
                    if self.survey.free_estimate() {
                        value.push_str("~"); // FSInfo's count, the FAT not yet counted
                    }
                    volume::size_text(&mut value, self.survey.free());
                }
                menu::StorageRow::Rides => match self.survey.rides() {
                    Some(n) => value.push_u32(n),
                    None => value.push_str("--"),
                },
            },
            Row::OldRide(k) => {
                if let Some(e) = self.survey.oldest(k) {
                    if let Some(start) = stock::ride_start(e.name()) {
                        volume::day_text(&mut label, start);
                    }
                    volume::size_text(&mut value, Some(u64::from(e.file.size)));
                }
            }
            Row::AltGps => self.alt_gps_row(&mut value),
            Row::Set(menu::Setting::Altitude)
                if matches!(menu, Menu::Place(_)) && !(self.editing && self.screen == Screen::Menu(menu, i)) =>
            {
                if let Menu::Place(k) = menu {
                    self.place_alt_row(k, &mut value);
                }
            }
            Row::SavePlace | Row::DeletePlace(_) => {}
            Row::Set(menu::Setting::Altitude) if !(self.editing && self.screen == Screen::Menu(menu, i)) => {
                self.alt_row(&mut value);
            }
            Row::Set(s) => {
                let edited = self.editing && self.screen == Screen::Menu(menu, i);
                value.push_str(if edited { "< " } else { "" });
                self.settings.text(s, &mut value);
                let number = value.as_str().starts_with(|c: char| c.is_ascii_digit());
                value.push_str(if edited {
                    " >"
                } else if number {
                    s.unit_in(self.settings.imperial)
                } else {
                    ""
                });
            }
            Row::Sensor(k) => {
                let kind = *Kind::ALL.get(k)?;
                // the maker's short name once the paired sensor has sent common page 80 (16 columns: maker, short name)
                match self.decoders.maker(kind).and_then(sensors::maker_name) {
                    Some(m) => {
                        label.push_str(m);
                        label.push_str(" ");
                        label.push_str(kind.short());
                    }
                    None => label.push_str(kind.name()),
                }
                match (self.ant, self.radio.sensors.get(k)?) {
                    (false, _) => value.push_str(if self.ble.is_some() { "NO KEY" } else { "NO BLE" }),
                    (true, _) if kind.network() == 1 && !self.di2 => value.push_str("NO KEY"),
                    (true, Sensor::Off) => value.push_str("OFF"),
                    (true, Sensor::Search) => value.push_str("SEARCH"),
                    (true, Sensor::Paired(id)) => value.push_u32(u32::from(id.device)),
                }
                if self.radio.heard_ms.get(k).is_some_and(|&ms| ms < sensors::STALE_MS) {
                    color = ui::GREEN;
                }
            }
            Row::Read(r @ (Reading::Lux | Reading::TempC | Reading::Hpa)) => {
                let present = if r == Reading::Lux { self.chips.light } else { self.chips.baro };
                match (r, self.light.auto.lux10(), self.baro) {
                    _ if !present => value.push_str("NONE"),
                    (Reading::Lux, Some(l), _) => value.push_u32(l / 10),
                    (Reading::TempC, _, Some((_, t))) => push_fixed(&mut value, i32::from(t), 2, 1),
                    (Reading::Hpa, _, Some((p, _))) => push_fixed(&mut value, p as i32, 1, 0),
                    _ => value.push_str("--"),
                }
            }
            Row::Read(r) => {
                let p = self.gps.pvt;
                match (r, p) {
                    (Reading::Fix, None) => value.push_str("NO GPS"),
                    (Reading::Fix, Some(p)) => {
                        value.push_str(match p.fix {
                            _ if !p.fix_ok => "NONE",
                            2 => "2D",
                            3 => "3D",
                            _ => "OTHER",
                        });
                    }
                    (_, None) => return None,
                    (Reading::Sats, Some(p)) => value.push_u32(u32::from(p.sats)),
                    (Reading::Lat, Some(p)) => push_fixed(&mut value, p.lat, 7, 5),
                    (Reading::Lon, Some(p)) => push_fixed(&mut value, p.lon, 7, 5),
                    (Reading::Alt, Some(p)) => push_fixed(&mut value, p.alt_mm, 3, 0),
                    (Reading::Utc | Reading::Date, Some(p)) if !p.time_ok => {
                        value.push_str("--"); // the receiver's placeholder until it knows the time
                    }
                    (Reading::Utc, Some(p)) => {
                        let (_, _, _, h, m, sec) = p.utc;
                        for (k, v) in [h, m, sec].into_iter().enumerate() {
                            if k > 0 {
                                value.push_str(":");
                            }
                            value.push_2d(u32::from(v));
                        }
                    }
                    (Reading::Date, Some(p)) => {
                        let (y, mo, d, ..) = p.utc;
                        value.push_u32(u32::from(y));
                        value.push_str("-");
                        value.push_2d(u32::from(mo));
                        value.push_str("-");
                        value.push_2d(u32::from(d));
                    }
                    _ => return None,
                }
                if p.is_some_and(|p| p.good()) {
                    color = ui::GREEN;
                }
            }
            Row::Layout(n) => layout_text(&mut value, self.page_ref(n)?),
            Row::Field(n, f) => {
                label.push_str("FIELD ");
                label.push_u32(f as u32 + 1);
                value.push_str(self.page_ref(n)?.fields.get(f)?.label());
            }
            Row::Stock(r) => self.stock_value(r, &mut value),
            Row::Past(n) => {
                // "24/10/2026", "42.1" (km: a unit does not fit beside the date)
                let r = self.stock.index.get(n)?;
                let (y, mo, d, ..) = r.start;
                label.push_str(date_numbers((y, mo, d), self.settings.date_fmt).as_str());
                push_trip(&mut value, r.dist_m, self.settings.imperial);
            }
            Row::Notif(k) => {
                // the sender (the app when it sent no title), and when it came
                let n = self.phone.inbox.get(k)?;
                label.push_str(if n.title.as_str().is_empty() { n.app.as_str() } else { n.title.as_str() });
                value.push_str(hm_text(n.time).as_str());
            }
            Row::Workout(k) => {
                let p = self.plan.calendar.get(k)?;
                label.push_str(date_short(p.day, self.settings.date_fmt).as_str());
                push_hm(&mut value, p.total_s);
                if self.workout_done(k) {
                    color = ui::GREEN;
                }
            }
            Row::WorkoutStart => {
                push_hm(&mut value, self.plan.workout.total_s());
                color = ui::GREEN;
            }
            Row::RideCourse => value.push_str("SIM"),
            Row::RampTest => {
                value.push_str("FTP ");
                value.push_u32(u32::from(self.settings.ftp));
            }
            Row::Step(k) => color = self.step_row(self.plan.steps().get(k)?, &mut label, &mut value),
            Row::Course(k) => color = self.course_row(k, &mut label, &mut value)?,
            Row::Done(k) => self.climb_done_row(k, &mut label, &mut value)?,
            Row::Month(k) => self.month_row(k, &mut label, &mut value)?,
            Row::Seg(k) => self.segment_row(k, &mut label, &mut value)?,
            Row::Place(k) => self.place_row(k, &mut label, &mut value)?,
            Row::SegSkipped => value.push_u32(u32::from(self.segments()?.skipped)),
            Row::Group(g) => {
                label.push_str(ui::FIELD_GROUPS.get(g)?.0);
                // the field the box shows now, beside its group
                if let Menu::Fields(n, f) = menu
                    && let Some(&now) = self.page_ref(n)?.fields.get(f)
                    && now.group() == g
                {
                    value.push_str(now.label());
                    color = ui::GREEN;
                }
            }
            Row::Phone(r) => {
                use menu::PhoneRow;
                let on = |v| if v { "ON" } else { "OFF" };
                let link = self.phone.link();
                if r == PhoneRow::Status && link == Link::NoBle && self.ble.is_some() {
                    // the set-up failed: which command, and its error (hex), for the first runs on the unit
                    let (op, code) = self.rx.failed.unwrap_or((0, 0));
                    value.push_str("ERR ");
                    for (v, n) in [(u32::from(op), 2), (code, 4)] {
                        for k in (0..n).rev() {
                            let d = (v >> (4 * k) & 0xf) as usize;
                            value.push_str("0123456789ABCDEF".get(d..d + 1).unwrap_or("?"));
                        }
                        value.push_str(if n == 2 { "/" } else { "" });
                    }
                }
                value.push_str(match r {
                    PhoneRow::Status => match link {
                        Link::NoBle if self.ble.is_some() => "",
                        Link::NoBle => "NO BLE",
                        Link::Off => "OFF",
                        Link::Pairing => "PAIRING",
                        Link::Waiting => "WAITING",
                        Link::Connected => "LINKED",
                        Link::Secure => "SECURE",
                    },
                    PhoneRow::Pair if link == Link::Pairing => "ON",
                    PhoneRow::Forget if self.phone.bonded() => "BONDED",
                    PhoneRow::Notify => on(self.phone.stored.notify),
                    PhoneRow::Keys => on(self.phone.stored.keys),
                    _ => "",
                });
                if matches!(link, Link::Secure) && r == PhoneRow::Status {
                    color = ui::GREEN;
                }
            }
            Row::Pick(field) => {
                if let Menu::Group(n, f, _) = menu
                    && self.page_ref(n)?.fields.get(f) == Some(&field)
                {
                    color = ui::GREEN;
                    value.push_str("ON");
                }
            }
        }
        if matches!(row, Row::Open(_) | Row::Nearby | Row::About | Row::EditPage(_)) && value.as_str().is_empty() {
            value.push_str(">"); // opens a menu or a screen
            color = ui::GREY;
        }
        if self.confirm == Some((menu, i)) {
            // waiting for a second Start (App::confirmed)
            value.clear();
            value.push_str("SURE?");
            color = ui::RED;
        }
        Some((label, value, color))
    }

    pub(crate) fn draw_menu_rows(&self, c: &mut impl Canvas, menu: Menu, sel: usize) {
        let first = menu::first_shown(sel, self.menu_rows(), self.rows(menu));
        for i in 0..self.menu_rows() {
            self.draw_menu_row(c, menu, sel, first, i);
        }
    }

    /// Screen row `slot` of `menu` (the menu turns a screen at a time: [`menu::first_shown`]).
    pub(crate) fn draw_menu_row(&self, c: &mut impl Canvas, menu: Menu, sel: usize, first: usize, slot: usize) {
        let i = first + slot;
        match self.menu_row(menu, i) {
            Some((label, value, color)) => {
                let inert = matches!(self.row(menu, i), Some(menu::Row::Empty(_))); // a note is not selected
                if let Some(menu::Row::Workout(k)) = self.row(menu, i)
                    && let Some(p) = self.plan.calendar.get(k)
                {
                    // the day over the length, and the workout's profile beside them: "SAT 24 OCT" is "SAT 24"
                    let day = label.as_str().rsplit_once(' ').map_or(label.as_str(), |d| d.0);
                    return ui::draw_workout_row(
                        c,
                        slot,
                        (day, value.as_str()),
                        i == sel,
                        &p.prof,
                        self.workout_done(k),
                    );
                }
                ui::draw_menu_row(c, slot, label.as_str(), value.as_str(), color, i == sel && !inert);
                if let Some(menu::Row::Edit(r @ (menu::EditRow::SplitAcross | menu::EditRow::SplitDown))) =
                    self.row(menu, i)
                {
                    ui::draw_split_icon(c, slot, r == menu::EditRow::SplitDown, i == sel);
                }
            }
            None => c.fill(0, ui::LIST_Y + slot as u32 * c.theme().menu_row, ui::W, c.theme().menu_row, ui::BLACK),
        }
    }

    /// A fingerprint of screen row `slot` as it would be drawn (FNV-1a over its text, colour and selection).
    pub(crate) fn menu_row_print(&self, menu: Menu, sel: usize, first: usize, slot: usize) -> u32 {
        self.menu_print(menu, sel, first, slot, true)
    }

    /// [`App::menu_row_print`] with or without the value, the "head" of the row. Without it a changed value (and its
    /// colour, which the value box carries) leaves the print as it was; the value still counts when it cuts the label
    /// or the row is a workout, and the colour when the value is empty (a grey empty row is a note: the label's grey).
    pub(crate) fn menu_print(&self, menu: Menu, sel: usize, first: usize, slot: usize, with_value: bool) -> u32 {
        let i = first + slot;
        let mut h = 0x811c_9dc5u32;
        let mut eat = |bytes: &[u8]| {
            h = crate::fnv(h, bytes);
        };
        match self.menu_row(menu, i) {
            Some((label, value, color)) => {
                eat(label.as_str().as_bytes());
                eat(&[0]);
                let cut = ui::menu_label_w(label.as_str(), value.as_str()) < ui::menu_label_w(label.as_str(), "");
                let workout = matches!(self.row(menu, i), Some(menu::Row::Workout(_)));
                if with_value || cut || workout {
                    eat(value.as_str().as_bytes());
                }
                if with_value || value.as_str().is_empty() {
                    eat(&color.to_le_bytes());
                }
                eat(&[u8::from(i == sel)]);
            }
            None => eat(&[1]),
        }
        h
    }

    /// The ride menu's rows come and go (a workout, a course, CLIMBS DONE): when their count changes under an open
    /// menu, the selection stays on the same row and the menu is drawn again.
    pub(crate) fn track_ride_menu(&mut self, b: &mut impl Board) {
        let Screen::Menu(m @ Menu::Ride(_), sel) = self.screen else {
            self.ride_menu = None;
            return;
        };
        let (n, mut row) = (self.rows(m), self.row(m, sel));
        if let Some((n0, r0)) = self.ride_menu
            && n0 != n
            && row != Some(r0)
            && let Some(i) = (0..n).find(|&i| self.row(m, i) == Some(r0))
        {
            self.screen = Screen::Menu(m, i);
            self.show(b);
            row = Some(r0);
        }
        self.ride_menu = row.map(|r| (n, r));
    }

    /// Redraws the menu rows that changed since they were drawn (the selection, a value).
    pub(crate) fn update_menu_rows(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        let first = menu::first_shown(sel, self.menu_rows(), self.rows(menu));
        let title = self.menu_title(menu);
        if title_print(title.as_str()) != self.menu_title_print {
            self.menu_title_print = title_print(title.as_str()); // a page's name follows the profile
            ui::draw_title(&mut Panel(b, self.area(), self.look()), title.as_str());
        }
        for i in 0..self.menu_rows() {
            let print = self.menu_row_print(menu, sel, first, i);
            if self.menu_shown[i] != print {
                let mut p = Panel(b, self.area(), self.look());
                // only the value changed: its own box, not the 240 x 43 row
                if self.menu_print(menu, sel, first, i, false) == self.menu_head[i] {
                    let at = first + i;
                    let inert = matches!(self.row(menu, at), Some(menu::Row::Empty(_))); // as in draw_menu_row
                    if let Some((label, value, color)) = self.menu_row(menu, at) {
                        ui::draw_menu_value(&mut p, i, label.as_str(), value.as_str(), color, at == sel && !inert);
                    }
                } else {
                    self.draw_menu_row(&mut p, menu, sel, first, i);
                }
                self.menu_head[i] = self.menu_print(menu, sel, first, i, false);
                self.menu_shown[i] = print;
            }
        }
    }

    /// Before a row that cannot be undone (`what`: "FORGET HEART RATE"): true when this Select confirms the one just
    /// before on the same row (no other input between: [`App::input`] clears it); else it asks, in the banner and in
    /// the row ("SURE?"), and the row waits for a second Start or tap.
    pub(crate) fn confirmed(&mut self, menu: Menu, sel: usize, what: &str) -> bool {
        if self.confirm.take() == Some((menu, sel)) {
            return true;
        }
        self.confirm = Some((menu, sel));
        let mut l = Line::<40>::new();
        l.push_str(what);
        l.push_str("?\nSTART: YES BACK: NO");
        self.say(l.as_str());
        false
    }

    /// Select on row `sel` of `menu`.
    pub(crate) fn activate(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        use menu::Row;
        match self.row(menu, sel) {
            Some(Row::Open(m)) => {
                if m == Menu::Storage {
                    self.survey.start(); // read over the next passes (App::step_survey)
                }
                self.activate_open(b, menu, m)
            }
            Some(Row::OldRide(k)) => self.read_old_ride(k),
            Some(Row::Storage(_)) => {}
            Some(Row::Ride(r)) => self.activate_ride(r),
            Some(Row::Past(n)) => {
                let Some(&r) = self.stock.index.get(n) else { return };
                self.past.start(&r, self.settings.ftp, self.settings.imperial); // read over the next passes (App::step_past)
                self.say("READING RIDE");
            }
            Some(Row::Workout(k)) => self.open_workout(b, k),
            Some(Row::Course(k)) => self.follow_course(k),
            Some(Row::WorkoutStart) => self.start_workout(0),
            Some(Row::RampTest) => self.start_ramp(),
            Some(Row::RideCourse) => self.pick_course(b),
            Some(Row::Step(k)) => self.start_workout(k),
            Some(Row::Drive) if self.emmc.lent() => self.drive_off(),
            Some(Row::Drive) => {
                let _ = self.drive_on(b); // why not is in the banner
            }
            Some(Row::About) => self.open(Screen::About),
            Some(Row::GiveUpSave) => self.give_up_save(b, menu, sel),
            Some(Row::Screenshot) => self.screenshot(),
            Some(Row::Notif(k)) => self.open(Screen::Notif(k, 0)),
            // paired and in use: forgotten; behind an ANT+ trainer (ANT FIRST): searched again, to take its place
            Some(Row::BtTrainer) if self.ftms_on() => {
                self.ftms_retry();
                if self.confirmed(menu, sel, "FORGET BT TRAINER") {
                    self.ftms_forget(b);
                }
            }
            Some(Row::BtTrainer) => self.ftms_search(b),
            Some(Row::BtFound(k)) => self.ftms_pair(b, k),
            Some(Row::Nearby) => {
                if self.ble.is_some() && !self.radio.scanning {
                    self.ftms_hold(b);
                    self.radio.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
                }
                self.open(Screen::Nearby);
            }
            Some(Row::AltGps) => self.alt_from_gps(),
            Some(Row::SavePlace) => self.save_place(b),
            Some(Row::ActiveBike) => self.next_bike(b),
            Some(Row::BikeOdo(_)) => {}
            Some(Row::Part(k, p)) => self.open(Screen::Menu(Menu::Part(k, p), 0)),
            Some(Row::PartEvery(k, p)) => self.next_every(b, k, p),
            Some(Row::PartReset(k, p)) => {
                let mut what = Line::<24>::new();
                what.push_str("RESET ");
                what.push_str(bikes::PART_NAMES.get(p).copied().unwrap_or(""));
                if self.confirmed(menu, sel, what.as_str()) {
                    self.reset_part(b, k, p);
                }
            }
            Some(Row::Place(k)) => self.open(Screen::Menu(Menu::Place(k), 0)),
            Some(Row::DeletePlace(k)) => {
                if self.confirmed(menu, sel, "DELETE PLACE") {
                    self.delete_place(k);
                    self.close();
                }
            }
            Some(Row::Set(menu::Setting::Trip(_))) if self.ride.state() != ride::State::Idle => {
                self.say("END THE RIDE FIRST")
            }
            Some(Row::Set(menu::Setting::Trip(n)))
                if self.confirmed(menu, sel, if n == 0 { "RESET TRIP 1" } else { "RESET TRIP 2" }) =>
            {
                self.settings.reset_trip(usize::from(n));
                self.save_misc(b);
                self.say(if n == 0 { "TRIP 1 RESET" } else { "TRIP 2 RESET" });
            }
            Some(Row::Set(menu::Setting::Trip(_) | menu::Setting::Odometer)) => {}
            Some(Row::Set(s)) => self.activate_setting(b, s),
            Some(Row::Sensor(k)) => self.activate_sensor(b, menu, sel, k),
            Some(Row::Layout(n)) => self.next_layout(b, n),
            Some(Row::EditPage(n)) => self.edit_page(b, n, 0),
            Some(Row::AddPage) => self.add_page(b, PAGES), // after the last, as many as the file has
            Some(Row::AutoScroll) => self.cycle_auto(b),
            Some(Row::PageOp(n, o)) => match o {
                menu::PageOp::Show => self.toggle_page(b, n),
                menu::PageOp::Up => self.move_page(b, n, true),
                menu::PageOp::Down => self.move_page(b, n, false),
                menu::PageOp::Rename => self.start_rename(b, n),
            },
            Some(Row::Edit(r)) => {
                if let Menu::Edit(k) = menu {
                    self.edit_action(b, menu, sel, k, r);
                }
            }
            Some(Row::Field(n, f)) => {
                // the picker opens on the group of the field the box shows
                let g = self.page_ref(n).and_then(|p| p.fields.get(f)).map_or(0, |x| x.group());
                self.open(Screen::Menu(Menu::Fields(n, f), g));
            }
            Some(Row::Group(g)) => {
                if let Menu::Fields(n, f) = menu {
                    let now = self.page_ref(n).and_then(|p| p.fields.get(f));
                    let at = ui::FIELD_GROUPS.get(g).and_then(|(_, l)| l.iter().position(|x| Some(x) == now));
                    self.open(Screen::Menu(Menu::Group(n, f, g), at.unwrap_or(0)));
                }
            }
            Some(Row::Pick(field)) => {
                if let Menu::Group(n, f, _) = menu {
                    self.pick(b, n, f, field);
                }
            }
            Some(Row::Stock(menu::StockRow::Scan)) if self.stock.status() != stock::Status::Running => {
                self.rescan();
            }
            Some(Row::Stock(menu::StockRow::Import)) if self.confirmed(menu, sel, "IMPORT SETTINGS") => {
                self.import_stock(b, false)
            }
            Some(Row::Stock(menu::StockRow::Import)) => {}
            Some(Row::Stock(menu::StockRow::Curve)) => self.open_curve(),
            Some(Row::Stock(menu::StockRow::Load)) => self.open_load(),
            Some(Row::Stock(menu::StockRow::Months)) => self.open(Screen::Menu(Menu::Months, 0)),
            Some(Row::Month(_)) => self.pmc.stat = (self.pmc.stat + 1) % pmc::STATS,
            Some(Row::Stock(r @ (menu::StockRow::ApplyFtp | menu::StockRow::ApplyWPrime))) => {
                self.apply_estimate(b, menu, sel, r)
            }
            Some(Row::Phone(r)) => self.activate_phone(b, menu, sel, r),
            Some(Row::Read(_) | Row::Stock(_) | Row::Empty(_) | Row::Done(_) | Row::Seg(_) | Row::SegSkipped)
            | None => {}
        }
    }

    /// Select on a row that opens a menu: the lists that load first, the editor on a ride's page, the page set the
    /// editor works on.
    fn activate_open(&mut self, b: &mut impl Board, menu: Menu, m: Menu) {
        match m {
            Menu::Workouts => {
                self.load_calendar(b);
                self.open(Screen::Menu(Menu::Workouts, 0));
            }
            Menu::Courses => self.open_courses(b),
            Menu::Places => {
                if self.places_ready(b) {
                    self.open(Screen::Menu(Menu::Places, 0));
                }
            }
            // the ride menu's EDIT THIS PAGE: the editor on the page itself (the overview's fields: their menu)
            Menu::Page(n) if matches!(menu, Menu::Ride(_)) && n < PAGES => {
                self.pages_set = self.ride_profile().page_set(); // the set on screen
                self.edit_page(b, n, 0)
            }
            // a set's page list: the editor works on that set (while a ride is open, on the ride's own only)
            Menu::PageList(set) if self.ride.state() != ride::State::Idle && set != self.ride_profile().page_set() => {
                self.say("THE RIDE'S SET")
            }
            m => {
                if let Menu::PageList(set) = m {
                    self.pages_set = set;
                }
                if m == Menu::History && self.stock.status() == stock::Status::Idle {
                    self.rescan(); // the history is what the scan of stock's rides finds
                }
                self.open(Screen::Menu(m, 0))
            }
        }
    }

    /// Select on a row of the ride menu that acts on the ride, the workout or the course.
    fn activate_ride(&mut self, r: menu::RideRow) {
        use menu::RideRow;
        match r {
            RideRow::Pause => {
                self.start_pause();
                if self.ride.state() == ride::State::Running {
                    self.leave_menus(); // resumed: back to riding
                }
            }
            // the end-ride screen, which is the confirmation (RESUME its default)
            RideRow::End => {
                if self.ride.state() == ride::State::Running {
                    let ev = self.ride.start_pause(); // paused first, as by Start (the recorder's order)
                    self.recorder(ev);
                }
                if self.ride.state() == ride::State::Paused {
                    self.open(Screen::End(ui::END_SAVE));
                }
            }
            RideRow::NextStep => {
                self.next_step();
                self.leave_menus();
            }
            RideRow::StopWorkout => {
                if self.ramp_running() {
                    self.finish_ramp(); // the cool-down plays, the result shows
                } else {
                    self.plan.player = None;
                    self.say("WORKOUT STOPPED");
                }
                self.leave_menus();
            }
            RideRow::StopCourse => {
                self.stop_course();
                self.leave_menus();
            }
            RideRow::Reverse => {
                self.reverse_course();
                self.leave_menus();
            }
            RideRow::BackToStart => {
                self.back_to_start();
                self.leave_menus();
            }
            RideRow::Trainer => self.open(Screen::Trainer),
            RideRow::CalPower => self.open_powercal(),
            RideRow::Curve => self.open_curve(),
            RideRow::Indoor => self.toggle_indoor(),
        }
    }

    /// A number setting's editing ended: what follows from it (BORN gives MAX HR, a MAX HR by hand is no longer the age's).
    pub(crate) fn setting_edited(&mut self, s: menu::Setting) {
        match s {
            menu::Setting::BirthYear => match self.settings.max_hr_from_age(self.date.0) {
                None if self.settings.birth_year > 0 => self.say("SET THE CLOCK"),
                Some(true) => {
                    let mut l = crate::text::Line::<24>::new();
                    l.push_str("MAX HR ");
                    l.push_u32(u32::from(self.settings.max_hr));
                    l.push_str(" FROM AGE");
                    self.say(l.as_str());
                }
                _ => {}
            },
            menu::Setting::MaxHr => self.settings.max_hr_age = false, // set by hand: no longer the age's
            _ => {}
        }
    }

    /// Select on a setting: a number starts or ends its editing, a choice cycles.
    fn activate_setting(&mut self, b: &mut impl Board, s: menu::Setting) {
        if s.number().is_some() {
            self.editing = !self.editing;
            if self.editing && s == menu::Setting::Altitude {
                match self.screen {
                    Screen::Menu(Menu::Place(k), _) => self.place_alt_edit_start(k), // the place's altitude
                    _ => self.editing = self.alt_edit_start(), // from the altitude the barometer reads
                }
            }
            if !self.editing {
                self.setting_edited(s);
                self.save_misc(b);
            }
        } else if matches!(s, menu::Setting::Theme | menu::Setting::DayTheme | menu::Setting::NightTheme) {
            self.next_theme(b, s);
        } else {
            self.settings.cycle(s);
            self.save_misc(b);
            self.update_light(b);
            self.gps.reconfig |= s == menu::Setting::GpsPower; // the receiver told at its next message
            self.gps.resys |= s == menu::Setting::GnssSystems;
            if s == menu::Setting::Font {
                self.font_changed(b);
            }
        }
    }

    /// The font changed: every text changes at once, the strip and the screen drawn whole.
    pub(crate) fn font_changed(&mut self, b: &mut impl Board) {
        self.notes.drawn = false; // the banner too, if one is up
        self.strip_shown = self.strip;
        self.plan.top = None; // the next-step strip comes back at the next pass
        ui::draw_strip(&mut Panel(b, (0, ui::H), self.look()), &self.strip);
        self.show(b);
    }

    /// Select on an ANT+ sensor: pair, stop, search again, forget.
    fn activate_sensor(&mut self, b: &mut impl Board, menu: Menu, sel: usize, k: usize) {
        let Some(&kind) = Kind::ALL.get(k).filter(|_| self.ant) else { return };
        let heard = self.radio.heard_ms.get(k).is_some_and(|&ms| ms < sensors::STALE_MS);
        match self.radio.sensors.get(k).copied() {
            Some(Sensor::Off) => self.set_sensor(b, kind, Sensor::Search),
            Some(Sensor::Search) => self.set_sensor(b, kind, Sensor::Off),
            // paired but silent: Select searches for it again now (not after its rest); Select once more
            // forgets it
            Some(Sensor::Paired(id)) if !heard && self.confirm != Some((menu, sel)) => {
                self.set_sensor(b, kind, Sensor::Paired(id));
                self.confirm = Some((menu, sel));
                self.say("SEARCHING AGAIN\nSTART: FORGET");
            }
            Some(Sensor::Paired(_)) => {
                let mut l = Line::<40>::new();
                l.push_str("FORGET ");
                l.push_str(kind.name());
                if self.confirmed(menu, sel, l.as_str()) {
                    self.set_sensor(b, kind, Sensor::Off);
                }
            }
            None => {}
        }
    }

    /// Select on a row of the phone menu.
    fn activate_phone(&mut self, b: &mut impl Board, menu: Menu, sel: usize, r: menu::PhoneRow) {
        use menu::PhoneRow;
        match r {
            PhoneRow::Status => {}
            PhoneRow::Pair => self.phone_pair(b),
            PhoneRow::Forget if self.confirmed(menu, sel, "FORGET THE PHONE") => self.phone_forget(b),
            PhoneRow::Forget => {}
            PhoneRow::Notify => self.phone.stored.notify = !self.phone.stored.notify,
            PhoneRow::Keys => self.phone.stored.keys = !self.phone.stored.keys,
            PhoneRow::Media => self.open(Screen::Media),
        }
        if matches!(r, PhoneRow::Notify | PhoneRow::Keys) {
            self.phone_changed();
        }
    }
}

/// A menu title's print, to see when it changed under a menu that stays up.
pub(crate) fn title_print(s: &str) -> u32 {
    crate::fnv(0x811c_9dc5, s.as_bytes())
}
