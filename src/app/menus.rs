//! The menu screens: each menu's rows (from the tables in [`menu`], plus the lists: sensors, history, workouts,
//! courses), their text, the redraw of the rows that changed (by fingerprint), and [`App::activate`], what a row
//! does when chosen.

use crate::*;

/// How long a BLE scan runs, started when the device list opens. Scanning keeps the Nordic radio on, so it is short.
pub const SCAN_S: u16 = 10;

/// The device list and the sensors menu are redrawn at most this often while reports come in.
pub const LIST_REDRAW_MS: u32 = 1000;

/// The menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Sensors, pages, wheel, nearby.
    Main,
    /// One row per sensor kind.
    Sensors,
    /// The overview and the ride pages ([`menu::PAGES`]).
    Pages,
    /// The rider's numbers ([`menu::RIDER`]).
    Rider,
    /// Backlight, wheel, GPS, BLE ([`menu::DEVICE`]).
    Device,
    /// Ride page `n`'s layout and fields.
    Page(usize),
    /// The field groups, for box `i` of page `n` (Select on a field row).
    Fields(usize, usize),
    /// Group `g`'s fields, for box `i` of page `n`.
    Group(usize, usize, usize),
    /// What the GNSS receiver reports.
    Gps,
    /// What stock's rides and settings give ([`menu::STOCK`]).
    Stock,
    /// Reminders and alerts ([`menu::ALERTS`]).
    Alerts,
    /// Stock's rides, newest first ([`history`]).
    History,
    /// The workouts planned from today on, one row each.
    Workouts,
    /// The steps of the workout opened from [`Menu::Workouts`].
    Steps,
    /// The course files on the card ([`storage::course`]).
    Courses,
    /// The phone link ([`menu::PHONE`]).
    Phone,
}

/// A menu's title (a page's is followed by its number).
pub(crate) fn menu_title(m: Menu) -> &'static str {
    match m {
        Menu::Main => "MENU",
        Menu::Sensors => "SENSORS",
        Menu::Pages => "PAGES",
        Menu::Rider => "RIDER",
        Menu::Device => "DEVICE",
        Menu::Page(_) => "PAGE ",
        Menu::Fields(..) => "FIELD",
        Menu::Group(_, _, g) => ui::FIELD_GROUPS.get(g).map_or("", |g| g.0),
        Menu::Gps => "GPS",
        Menu::Stock => "STOCK DATA",
        Menu::Alerts => "ALERTS",
        Menu::History => "HISTORY",
        Menu::Workouts => "WORKOUTS",
        Menu::Steps => "WORKOUT",
        Menu::Courses => "COURSES",
        Menu::Phone => "PHONE",
    }
}

/// A page's layout by its name ("OFF" for none).
pub(crate) fn layout_text(l: &mut Line<12>, layout: u8) {
    l.push_str(ui::layout(layout).map_or("?", |(_, name)| name));
}

impl App {
    /// Row `i` of `menu`; None past its last.
    pub(crate) fn row(&self, menu: Menu, i: usize) -> Option<menu::Row> {
        use menu::Row;
        match menu {
            Menu::Main => menu::MAIN.get(i).copied(),
            Menu::Pages => menu::PAGES.get(i).copied(),
            Menu::Rider => menu::RIDER.get(i).copied(),
            Menu::Device => menu::DEVICE.get(i).copied(),
            Menu::Fields(..) => (i < ui::FIELD_GROUPS.len()).then_some(Row::Group(i)),
            Menu::Group(_, _, g) => ui::FIELD_GROUPS.get(g)?.1.get(i).map(|&f| Row::Pick(f)),
            Menu::Sensors if i < Kind::ALL.len() => Some(Row::Sensor(i)),
            Menu::Sensors => menu::BUILT_IN.get(i - Kind::ALL.len()).map(|&r| Row::Read(r)),
            Menu::Gps => menu::GPS.get(i).filter(|_| i == 0 || self.pvt.is_some()).map(|&r| Row::Read(r)),
            Menu::Stock => menu::STOCK.get(i).map(|&r| Row::Stock(r)),
            Menu::Alerts => menu::ALERTS.get(i).copied(),
            // no rides yet: the scan's row, which shows how far it is
            Menu::History if self.stock.index.is_empty() => (i == 0).then_some(Row::Stock(menu::StockRow::Scan)),
            Menu::History => self.stock.index.get(i).map(|_| Row::Past(i)),
            Menu::Workouts => self.calendar.get(i).map(|_| Row::Workout(i)),
            Menu::Steps if i == 0 => Some(Row::WorkoutStart),
            Menu::Steps => (i <= self.plan.steps().len()).then_some(Row::Step(i - 1)),
            Menu::Courses => self.courses.as_deref()?.list.get(i).map(|_| Row::Course(i)),
            Menu::Phone => menu::PHONE.get(i).map(|&r| Row::Phone(r)),
            Menu::Page(OVERVIEW) => (i < 4).then_some(Row::Field(OVERVIEW, i)), // always four
            Menu::Page(n) => match i {
                0 => Some(Row::Layout(n)),
                _ if i <= ui::field_count(self.pages.get(n)?.layout) => Some(Row::Field(n, i - 1)),
                _ => None,
            },
        }
    }

    /// How many rows `menu` has.
    pub(crate) fn rows(&self, menu: Menu) -> usize {
        (0..128).take_while(|&i| self.row(menu, i).is_some()).count() // a workout's steps: up to 97
    }

    /// Menu row `i` of `menu`: label, value, value colour; None past the last row.
    pub(crate) fn menu_row(&self, menu: Menu, i: usize) -> Option<(Line<16>, Line<12>, u16)> {
        use menu::{Reading, Row};
        let (mut label, mut value, mut color) = (Line::new(), Line::new(), ui::WHITE);
        match self.row(menu, i)? {
            Row::Open(Menu::Page(OVERVIEW)) => label.push_str("OVERVIEW"),
            Row::Open(Menu::Page(n)) => {
                label.push_str("PAGE ");
                label.push_u32(n as u32 + 1);
                layout_text(&mut value, self.pages.get(n)?.layout);
            }
            Row::Open(m) => label.push_str(menu_title(m)),
            Row::Nearby => label.push_str("NEARBY BLE"),
            Row::Drive => {
                label.push_str("USB DRIVE");
                value.push_str(if self.emmc.lent() { "ON" } else { "OFF" });
            }
            Row::Set(s) => {
                label.push_str(s.label());
                let edited = self.editing && self.screen == Screen::Menu(menu, i);
                value.push_str(if edited { "< " } else { "" });
                self.settings.text(s, &mut value);
                value.push_str(if edited { " >" } else { "" });
            }
            Row::Sensor(k) => {
                let kind = *Kind::ALL.get(k)?;
                label.push_str(kind.name());
                match (self.ant, self.sensors.get(k)?) {
                    (false, _) => value.push_str(if self.ble.is_some() { "NO KEY" } else { "NO BLE" }),
                    (true, Sensor::Off) => value.push_str("OFF"),
                    (true, Sensor::Search) => value.push_str("SEARCH"),
                    (true, Sensor::Paired(id)) => value.push_u32(u32::from(id.device)),
                }
                if self.heard_ms.get(k).is_some_and(|&ms| ms < sensors::STALE_MS) {
                    color = ui::GREEN;
                }
            }
            Row::Read(r @ (Reading::Lux | Reading::TempC | Reading::Hpa)) => {
                label.push_str(match r {
                    Reading::Lux => "LUX",
                    Reading::TempC => "TEMP C",
                    _ => "HPA",
                });
                let present = if r == Reading::Lux { self.chips.light } else { self.chips.baro };
                match (r, self.auto.lux10(), self.baro) {
                    _ if !present => value.push_str("NONE"),
                    (Reading::Lux, Some(l), _) => value.push_u32(l / 10),
                    (Reading::TempC, _, Some((_, t))) => push_fixed(&mut value, i32::from(t), 2, 1),
                    (Reading::Hpa, _, Some((p, _))) => push_fixed(&mut value, p as i32, 1, 0),
                    _ => value.push_str("--"),
                }
            }
            Row::Read(r) => {
                let two = |l: &mut Line<12>, v: u8| {
                    for d in [v / 10, v % 10] {
                        l.push_str(ui::DIGITS[usize::from(d)]);
                    }
                };
                let p = self.pvt;
                match (r, p) {
                    (Reading::Fix, None) => (label.push_str("FIX"), value.push_str("NO GPS")).1,
                    (Reading::Fix, Some(p)) => {
                        label.push_str("FIX");
                        value.push_str(match p.fix {
                            _ if !p.fix_ok => "NONE",
                            2 => "2D",
                            3 => "3D",
                            _ => "OTHER",
                        });
                    }
                    (_, None) => return None,
                    (Reading::Sats, Some(p)) => (label.push_str("SATS"), value.push_u32(u32::from(p.sats))).1,
                    (Reading::Lat, Some(p)) => (label.push_str("LAT"), push_fixed(&mut value, p.lat, 7, 5)).1,
                    (Reading::Lon, Some(p)) => (label.push_str("LON"), push_fixed(&mut value, p.lon, 7, 5)).1,
                    (Reading::Alt, Some(p)) => (label.push_str("ALT M"), push_fixed(&mut value, p.alt_mm, 3, 0)).1,
                    (Reading::Utc | Reading::Date, Some(p)) if !p.time_ok => {
                        label.push_str(if r == Reading::Utc { "UTC" } else { "DATE" });
                        value.push_str("--"); // the receiver's placeholder until it knows the time
                    }
                    (Reading::Utc, Some(p)) => {
                        label.push_str("UTC");
                        let (_, _, _, h, m, sec) = p.utc;
                        for (k, v) in [h, m, sec].into_iter().enumerate() {
                            if k > 0 {
                                value.push_str(":");
                            }
                            two(&mut value, v % 100);
                        }
                    }
                    (Reading::Date, Some(p)) => {
                        label.push_str("DATE");
                        let (y, mo, d, ..) = p.utc;
                        value.push_u32(u32::from(y));
                        value.push_str("-");
                        two(&mut value, mo % 100);
                        value.push_str("-");
                        two(&mut value, d % 100);
                    }
                    _ => return None,
                }
                if p.is_some_and(|p| p.good()) {
                    color = ui::GREEN;
                }
            }
            Row::Layout(n) => {
                label.push_str("FIELDS");
                layout_text(&mut value, self.pages.get(n)?.layout);
            }
            Row::Field(n, f) => {
                label.push_str("FIELD ");
                label.push_u32(f as u32 + 1);
                value.push_str(self.pages.get(n)?.fields.get(f)?.label());
            }
            Row::Stock(r) => {
                label.push_str(r.label());
                self.stock_value(r, &mut value);
            }
            Row::Past(n) => {
                // "SAT 24 OCT", "42.1" (km: a unit does not fit beside the date)
                let r = self.stock.index.get(n)?;
                let (y, mo, d, ..) = r.start;
                label.push_str(date_text((y, mo, d)).as_str());
                ui::card::push_km(&mut value, r.dist_m);
            }
            Row::Workout(k) => {
                let p = self.calendar.get(k)?;
                label.push_str(date_text(p.day).as_str());
                push_hm(&mut value, p.total_s);
            }
            Row::WorkoutStart => {
                label.push_str("START");
                push_hm(&mut value, self.plan.total_s());
                color = ui::GREEN;
            }
            Row::Step(k) => color = self.step_row(self.plan.steps().get(k)?, &mut label, &mut value),
            Row::Course(k) => color = self.course_row(k, &mut label, &mut value)?,
            Row::Group(g) => {
                label.push_str(ui::FIELD_GROUPS.get(g)?.0);
                // the field the box shows now, beside its group
                if let Menu::Fields(n, f) = menu
                    && let Some(&now) = self.pages.get(n)?.fields.get(f)
                    && now.group() == g
                {
                    value.push_str(now.label());
                    color = ui::GREEN;
                }
            }
            Row::Phone(r) => {
                use menu::PhoneRow;
                label.push_str(r.label());
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
                label.push_str(field.label());
                if let Menu::Group(n, f, _) = menu
                    && self.pages.get(n)?.fields.get(f) == Some(&field)
                {
                    color = ui::GREEN;
                    value.push_str("ON");
                }
            }
        }
        Some((label, value, color))
    }

    pub(crate) fn draw_menu_rows(&self, c: &mut impl Canvas, menu: Menu, sel: usize) {
        for i in 0..ui::MENU_ROWS {
            self.draw_menu_row(c, menu, sel, i);
        }
    }

    /// Screen row `slot` of `menu` (the menu turns a screen at a time: [`menu::first_shown`]).
    pub(crate) fn draw_menu_row(&self, c: &mut impl Canvas, menu: Menu, sel: usize, slot: usize) {
        let i = menu::first_shown(sel, ui::MENU_ROWS) + slot;
        match self.menu_row(menu, i) {
            Some((label, value, color)) => ui::draw_menu_row(c, slot, label.as_str(), value.as_str(), color, i == sel),
            None => c.fill(0, ui::LIST_Y + slot as u32 * ui::LIST_ROW, ui::W, ui::LIST_ROW, ui::BLACK),
        }
    }

    /// A fingerprint of screen row `slot` as it would be drawn (FNV-1a over its text, colour and selection).
    pub(crate) fn menu_row_print(&self, menu: Menu, sel: usize, slot: usize) -> u32 {
        let i = menu::first_shown(sel, ui::MENU_ROWS) + slot;
        let mut h = 0x811c_9dc5u32;
        let mut eat = |bytes: &[u8]| {
            for &x in bytes {
                h = (h ^ u32::from(x)).wrapping_mul(0x0100_0193);
            }
        };
        match self.menu_row(menu, i) {
            Some((label, value, color)) => {
                eat(label.as_str().as_bytes());
                eat(&[0]);
                eat(value.as_str().as_bytes());
                eat(&color.to_le_bytes());
                eat(&[u8::from(i == sel)]);
            }
            None => eat(&[1]),
        }
        h
    }

    /// Redraws the menu rows that changed since they were drawn (the selection, a value).
    pub(crate) fn update_menu_rows(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        for i in 0..ui::MENU_ROWS {
            let print = self.menu_row_print(menu, sel, i);
            if self.menu_shown[i] != print {
                self.draw_menu_row(&mut Panel(b, (0, self.bottom())), menu, sel, i);
                self.menu_shown[i] = print;
            }
        }
    }

    /// Select on row `sel` of `menu`.
    pub(crate) fn activate(&mut self, b: &mut impl Board, menu: Menu, sel: usize) {
        use menu::Row;
        match self.row(menu, sel) {
            Some(Row::Open(Menu::Workouts)) => {
                self.load_calendar(b);
                if self.calendar.get(0).is_none() {
                    self.say("NO WORKOUT PLANNED");
                }
                self.open(Screen::Menu(Menu::Workouts, 0));
            }
            Some(Row::Open(Menu::Courses)) => self.open_courses(b),
            Some(Row::Open(m)) => {
                if m == Menu::History && self.stock.status() == stock::Status::Idle {
                    self.stock.start(self.date); // the history is what the scan of stock's rides finds
                }
                self.open(Screen::Menu(m, 0))
            }
            Some(Row::Past(n)) => {
                let Some(&r) = self.stock.index.get(n) else { return };
                self.past.start(&r, self.settings.ftp); // read over the next passes (App::step_past)
                self.say("READING RIDE");
            }
            Some(Row::Workout(k)) => self.open_workout(b, k),
            Some(Row::Course(k)) => self.follow_course(k),
            Some(Row::WorkoutStart) => self.start_workout(0),
            Some(Row::Step(k)) => self.start_workout(k),
            Some(Row::Drive) if self.emmc.lent() => self.drive_off(),
            Some(Row::Drive) => {
                let _ = self.drive_on(); // why not is in the banner
            }
            Some(Row::Nearby) => {
                if self.ble.is_some() && !self.scanning {
                    self.scanning = nrf::scan_start(b, &mut self.rx, SCAN_S).is_ok();
                }
                self.open(Screen::Nearby);
            }
            Some(Row::Set(s)) if s.number().is_some() => {
                self.editing = !self.editing;
                if !self.editing {
                    self.save_misc(b);
                }
            }
            Some(Row::Set(s)) => {
                self.settings.cycle(s);
                self.save_misc(b);
                self.update_light(b);
            }
            Some(Row::Sensor(k)) => {
                if let (true, Some(&kind), Some(&now)) = (self.ant, Kind::ALL.get(k), self.sensors.get(k)) {
                    let want = if now == Sensor::Off { Sensor::Search } else { Sensor::Off };
                    self.set_sensor(b, kind, want);
                }
            }
            Some(Row::Layout(n)) => {
                if let Some(page) = self.pages.get_mut(n) {
                    let order = ui::LAYOUT_ORDER;
                    let i = order.iter().position(|&l| l == page.layout).unwrap_or(0);
                    page.layout = order[(i + 1) % order.len()];
                    if n == 0 && page.layout == 0 {
                        page.layout = order[1]; // the first page is never off
                    }
                    save_page(b, n, page);
                }
            }
            Some(Row::Field(n, f)) => {
                // the picker opens on the group of the field the box shows
                let g = self.pages.get(n).and_then(|p| p.fields.get(f)).map_or(0, |x| x.group());
                self.open(Screen::Menu(Menu::Fields(n, f), g));
            }
            Some(Row::Group(g)) => {
                if let Menu::Fields(n, f) = menu {
                    let now = self.pages.get(n).and_then(|p| p.fields.get(f));
                    let at = ui::FIELD_GROUPS.get(g).and_then(|(_, l)| l.iter().position(|x| Some(x) == now));
                    self.open(Screen::Menu(Menu::Group(n, f, g), at.unwrap_or(0)));
                }
            }
            Some(Row::Pick(field)) => {
                if let Menu::Group(n, f, _) = menu
                    && let Some(page) = self.pages.get_mut(n)
                    && let Some(slot) = page.fields.get_mut(f)
                {
                    *slot = field;
                    save_page(b, n, page);
                    self.close(); // the groups
                    self.close(); // back to the page's menu, its row showing the new field
                }
            }
            Some(Row::Stock(menu::StockRow::Scan)) if self.stock.status() != stock::Status::Running => {
                self.stock.start(self.date);
            }
            Some(Row::Stock(menu::StockRow::Import)) => self.import_stock(b, false),
            Some(Row::Phone(r)) => {
                use menu::PhoneRow;
                match r {
                    PhoneRow::Status => {}
                    PhoneRow::Pair => self.phone_pair(b),
                    PhoneRow::Forget => self.phone_forget(b),
                    PhoneRow::Notify => self.phone.stored.notify = !self.phone.stored.notify,
                    PhoneRow::Keys => self.phone.stored.keys = !self.phone.stored.keys,
                    PhoneRow::Media => self.open(Screen::Media),
                }
                if matches!(r, PhoneRow::Notify | PhoneRow::Keys) {
                    self.phone_changed();
                }
            }
            Some(Row::Read(_) | Row::Stock(_)) | None => {}
        }
    }
}
