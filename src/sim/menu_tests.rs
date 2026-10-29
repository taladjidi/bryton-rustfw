//! The menus on the Sim: the devices nearby, Back, held keys, pairing sensors, the pages configured from the menu,
//! the HRV fields, the menu tree reaching every row, and the RIDE menu.

use super::course_tests::hill;
use super::tests::run; // not the parent's `run`, which the glob would give
use super::tests::*;
use super::*;
use crate::ui::Field;

/// Holding Page opens the menu; Page / Lap move, Start on NEARBY BLE opens the list and scans: two devices in
/// three reports, strongest first, "2 NEARBY..." until the timeout. Back goes out level by level.
#[test]
fn menu_lists_the_devices_nearby() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
    assert_consistent(&sim, &app);
    sim.save("main_menu");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "Lap on the first row stays there: no wrap");
    let last = menu::MAIN.len() - 1;
    for _ in 0..last + 3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, last), "Page stops at the last row");
    for _ in 0..last {
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    to_row(&mut sim, &mut app, menu::Row::Nearby);
    let nearby = row_of(menu::Row::Nearby);
    assert_eq!(app.screen(), Screen::Menu(Menu::System, nearby), "BLE NEARBY, in SYSTEM");
    assert_consistent(&sim, &app);
    sim.save("menu");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Nearby);
    assert_eq!(sim.nordic.got.last().unwrap()[..2], [0, 0x8a], "the scan starts with the list");
    run(&mut sim, &mut app, 1500);
    let names: Vec<_> = app.radio.nearby.strongest().map(|d| (d.rssi, std::string::String::from(d.name()))).collect();
    assert_eq!(names, [(-42, "Bike".into()), (-70, "".into())], "last RSSI kept, the name from the first report");
    assert!(app.radio.scanning);
    assert_consistent(&sim, &app);
    sim.save("nearby");
    run(&mut sim, &mut app, u64::from(SCAN_S) * 1000);
    assert!(!app.radio.scanning);
    assert_consistent(&sim, &app);
    // the "<" of the title bar goes back, as on the menus
    let (x, y) = (ui::BACK_TAP.0 as u16 + 4, ui::BACK_TAP.1 as u16 + 4);
    app.input(&mut sim, Input::Touch(touch::Gesture::Tap(x, y)));
    assert_eq!(app.screen(), Screen::Menu(Menu::System, nearby), "a tap on the \"<\"");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Settings, row_of(menu::Row::Open(Menu::System))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, row_of(menu::Row::Open(Menu::Settings))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// Back returns to where the menu was opened, row by row: a submenu to its row, the menu to the ride page.
#[test]
fn back_returns_where_it_came_from() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(1));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Gps));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Gps, 0));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::System, row_of(menu::Row::Open(Menu::Gps))), "the submenu's row");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Settings, row_of(menu::Row::Open(Menu::System))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, row_of(menu::Row::Open(Menu::Settings))));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(1), "the page the menu was opened from");
    assert_consistent(&sim, &app);
    // switching that page off from the menu: Back shows the main screen instead
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(1)));
    app.press(&mut sim, Press::Short(Key::Start)); // PAGE 2
    delete_page_by_keys(&mut sim, &mut app); // pages 2 and 3 deleted from the editor
    delete_page_by_keys(&mut sim, &mut app);
    assert_eq!(app.page_count(), 1);
    for _ in 0..4 {
        back(&mut sim, &mut app); // the page's menu, ROAD PAGES, PAGES, the menu, out
    }
    assert_eq!(app.screen(), Screen::Main);
    assert_consistent(&sim, &app);
}

/// On a page's menu (its first row, EDIT ON PAGE): the editor, its actions, DELETE PAGE, sure; back on the menu.
fn delete_page_by_keys(sim: &mut Sim, app: &mut App) {
    let Screen::Menu(Menu::Page(_), 0) = app.screen() else { panic!("{:?}", app.screen()) };
    app.press(sim, Press::Short(Key::Start)); // EDIT ON PAGE
    app.press(sim, Press::Short(Key::Start)); // the actions
    for _ in 0..menu::EDIT_ROWS.len() - 1 {
        app.press(sim, Press::Short(Key::Page));
    }
    app.press(sim, Press::Short(Key::Start)); // DELETE PAGE
    app.press(sim, Press::Short(Key::Start)); // sure
    assert!(matches!(app.screen(), Screen::Menu(Menu::Page(_), _)), "{:?}", app.screen());
}

/// A key held in a menu moves the selection on each repeat, and its release does not move it once more.
#[test]
fn held_key_scrolls_a_menu() {
    let mut sim = Sim::new(9, 0, 0).keys(|ms| if (5000..5749).contains(&ms) { Key::Page as u8 } else { 0 });
    let mut app = App::start(&mut sim, None);
    assert!(sim.ms < 5000);
    app.press(&mut sim, Press::Long(Key::Page));
    let left = 7000 - sim.ms;
    run(&mut sim, &mut app, left);
    // held 750 ms: repeats at 500, 600, 700 ms, then the release
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 3));
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 4), "the next press acts");
}

/// Sensors menu: Start on a sensor searches; the first one heard is paired (kept in its backup register) and
/// its value reaches the ride page. With a key, ANT is started and every channel assigned, none opened yet.
#[test]
fn pairs_sensors_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x79, id: 555, data: spd_cad, network: 0, ack: &[] });
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    let ops: Vec<_> = sim.nordic.got.iter().filter(|p| p[0] == 6).map(|p| p[1]).collect();
    assert_eq!(ops, [0xfa, 0xcc, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2, 0xc2], "enable, key, assign the 7 ANT+ channels");
    assert!(sim.nordic.got.contains(&[&[6, 0xcc, 0][..], &KEY].concat()), "the key on network 0");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // heart rate: search
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0, 0, 0x78, 0]), "any strap");
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start)); // speed + cadence: search
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "strap paired and kept");
    assert_eq!(sim.backup[Kind::SpeedCadence.backup()], SensorId { device: 555, trans: 0xf1 }.to_word());
    assert_consistent(&sim, &app);
    sim.save("sensors");
    back(&mut sim, &mut app);
    back(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 0, &["22.7", "72", "60"], true) == sim.screen, "22.7 km/h, HR 72, 60 rpm");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 200);
    assert!(ride(&mut sim, &app, 1, &["22.7", "72", "60", "--"], true) == sim.screen);
    sim.save("ride");
}

/// After a restart the paired sensor's channel opens for it only: a friend's strap is not taken. Start on it
/// in the sensors menu, while it is silent, searches for it again; Start once more forgets it (channel closed,
/// register cleared).
#[test]
fn keeps_the_paired_sensor_until_forgotten() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 999, data: hr80, network: 0, ack: &[] }); // a friend's strap
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 0, 0x26, 0x62, 0x78, 0xf1]), "the paired strap only");
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert!(ride(&mut sim, &app, 0, &["--", "--", "--"], true) == sim.screen, "the friend's strap is not shown");
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // silent: searched for again, not forgotten
    let closes = |sim: &Sim| sim.nordic.got.iter().filter(|p| p[..2] == [6, 0xc5]).count();
    assert_eq!(closes(&sim), 1, "closed and opened for it again");
    assert_eq!(sim.backup[Kind::Hr.backup()], STRAP.to_word(), "still paired");
    assert_eq!(app.notes.text.as_str(), "SEARCHING AGAIN\nSTART: FORGET");
    assert_eq!(app.menu_row(Menu::Sensors, 0).unwrap().1.as_str(), "SURE?");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    sim.save("sensor_search_again");
    app.press(&mut sim, Press::Short(Key::Start)); // again: forgotten
    assert_eq!(closes(&sim), 2, "closed");
    assert_eq!(sim.backup[Kind::Hr.backup()], 0, "forgotten");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.backup[Kind::Hr.backup()], SensorId { device: 999, trans: 0xf1 }.to_word(), "paired anew");
}

/// A paired strap that is not sending: its channel searches 30 s, closes, rests a minute, searches again; the
/// strap is found when it comes back.
#[test]
fn a_silent_sensor_searches_then_rests() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    let mut app = App::start(&mut sim, Some(KEY));
    let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
    assert_eq!(opens(&sim), 1);
    assert!(sim.nordic.got.contains(&vec![6, 0xda, 0, crate::nrf::SEARCH_TIMEOUT]));
    run(&mut sim, &mut app, 30_000 + 1000);
    assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
    run(&mut sim, &mut app, u64::from(SEARCH_REST_MS) - 2000);
    assert_eq!(opens(&sim), 1, "resting");
    run(&mut sim, &mut app, 2000);
    assert_eq!(opens(&sim), 2, "searching again");
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: hr72, network: 0, ack: &[] });
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.live.get(Metric::Hr), Some(72), "found");
    run(&mut sim, &mut app, 60_000);
    assert!(sim.nordic.channels[0].open, "a sensor that sends keeps its channel");
}

/// A paired strap that stays away: each search that finds nothing doubles the rest after it (1, 2, then 4 min at
/// most), and a ride's Start searches again at once.
#[test]
fn an_absent_sensor_rests_longer_until_a_ride_starts() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    let mut app = App::start(&mut sim, Some(KEY));
    let opens = |sim: &Sim| sim.nordic.got.iter().filter(|p| p.starts_with(&[6, 0xc4, 0])).count();
    // searches of 30 s (the Sim models the low-priority one only), then rests of 60, 120, 240, 240 s
    let mut want = 1;
    for rest in [60_000, 120_000, 240_000, SEARCH_REST_MAX_MS] {
        run(&mut sim, &mut app, 30_000 + 1000);
        assert!(!sim.nordic.channels[0].open, "closed by its search time-out");
        run(&mut sim, &mut app, u64::from(rest) - 2000);
        assert_eq!(opens(&sim), want, "resting {rest} ms");
        run(&mut sim, &mut app, 1000);
        want += 1;
        assert_eq!(opens(&sim), want, "searching again after {rest} ms");
    }
    run(&mut sim, &mut app, 31_000);
    assert!(!sim.nordic.channels[0].open, "resting again");
    app.press(&mut sim, Press::Short(Key::Start)); // overview: the first ride page
    app.press(&mut sim, Press::Short(Key::Start)); // the ride starts
    run(&mut sim, &mut app, 200);
    assert_eq!(opens(&sim), want + 1, "a ride's Start searches at once");
    run(&mut sim, &mut app, 31_000 + 57_000);
    assert_eq!(opens(&sim), want + 1, "then the first rest again");
    run(&mut sim, &mut app, 2000);
    assert_eq!(opens(&sim), want + 2);
}

/// The page menu: Start cycles the layout and the fields; kept in a backup register, read back at the next
/// start. A power meter's value shows in the new field.
#[test]
fn pages_are_configured_from_the_menu() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Power.backup()] = SensorId { device: 7, trans: 5 }.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power300, network: 0, ack: &[] });
    sim.classic_pages();
    let mut app = App::start(&mut sim, Some(KEY));
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(0)));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 0));
    app.press(&mut sim, Press::Short(Key::Page)); // LAYOUT
    while app.pages()[0].preset_id() != Some(4) {
        app.press(&mut sim, Press::Short(Key::Start)); // layouts round to 2 x 2
    }
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start)); // field 1 (speed): its group's row selected
    assert_eq!(app.screen(), Screen::Menu(Menu::Fields(0, 0), Field::Speed.group()));
    assert_consistent(&sim, &app);
    sim.save("field_groups");
    app.press(&mut sim, Press::Short(Key::Page)); // POWER
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Group(0, 0, 1), 0));
    assert_consistent(&sim, &app);
    sim.save("field_picker");
    app.press(&mut sim, Press::Short(Key::Start)); // WATTS: back to the page's menu, on the field's row
    assert_eq!(app.screen(), Screen::Menu(Menu::Page(0), 2));
    assert_consistent(&sim, &app);
    sim.save("page_menu");
    app.step(&mut sim); // the registers written at the next pass (after the file: no volume here)
    let want =
        PageCfg { layout: 4, fields: [Field::Power, Field::Hr, Field::Cadence, Field::Time, Field::Time, Field::Time] };
    assert_eq!(app.pages()[0], want.into());
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(want));
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    let t = hm(&mut sim);
    assert_consistent(&sim, &app);
    assert!(ride(&mut sim, &app, 0, &["300", "--", "85", &t], true) == sim.screen);
    let app = App::start(&mut sim, Some(KEY));
    assert_eq!(app.pages()[0], want.into(), "read back after a restart");
}

/// When the simulated strap starts beating, ms.
const BEAT_START: u64 = 10_000;
/// The strap's interval before beat `k`, 1/1024 s: 820 plus or minus 16 (about 800 ms), scrambled (murmur3's
/// finaliser).
pub(super) fn beat_ticks(k: u32) -> u64 {
    let mut x = k.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    804 + u64::from(x >> 27)
}
/// A strap beating from [`BEAT_START`]: page 4, the last beat's time, the one before, the count.
pub(super) fn strap_beats(ms: u64) -> [u8; 8] {
    let now = ms * 1024 / 1000;
    let (mut t, mut prev, mut k) = (BEAT_START * 1024 / 1000, 0, 0u32);
    while t + beat_ticks(k + 1) <= now {
        (prev, k) = (t, k + 1);
        t += beat_ticks(k);
    }
    let ([p0, p1], [t0, t1]) = ((prev as u16).to_le_bytes(), (t as u16).to_le_bytes());
    [4, 0, p0, p1, t0, t1, k as u8, 70]
}

/// A strap sends a known series of beats: the HRV fields show "--" until there are enough, then the RMSSD and
/// DFA alpha1 of the series, and "--" again once the strap has no heart rate.
#[test]
fn hrv_fields_from_the_strap() {
    let mut sim = Sim::new(9, 0, 0);
    sim.backup[Kind::Hr.backup()] = STRAP.to_word();
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: STRAP.device, data: strap_beats, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some(KEY));
    use Field::*;
    app.pages[0] = PageCfg { layout: 4, fields: [Dfa, Rmssd, Hr, Time, Time, Time] }.into();
    app.press(&mut sim, Press::Short(Key::Page));
    let text = |app: &App, f| std::string::String::from(app.field_text(f).as_str());
    run(&mut sim, &mut app, 20_000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "about 12 beats");
    run(&mut sim, &mut app, 20_000);
    assert!(text(&app, Dfa) == "--" && text(&app, Rmssd) != "--", "about 37 beats: RMSSD, not yet DFA");
    run(&mut sim, &mut app, 140_000);
    // stop where the app has the last beat or the one before: the same 5-beat update either way
    while strap_beats(sim.ms)[6] % hrv::EVERY != 2 {
        run(&mut sim, &mut app, 100);
    }
    let mut want = hrv::Hrv::default();
    for k in 1..=u32::from(strap_beats(sim.ms)[6]) {
        want.push(((beat_ticks(k) * 1000 + 512) >> 10) as u16);
    }
    let (a, ms) = (want.alpha1().unwrap(), want.rmssd().unwrap());
    assert!((0.3..0.7).contains(&a), "scrambled beats are close to white noise: {a}");
    let (a, ms) = (std::format!("{a:.2}"), std::format!("{ms}"));
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), (a.clone(), ms.clone()));
    let t = hm(&mut sim);
    assert!(ride(&mut sim, &app, 0, &[&a, &ms, "70", &t], true) == sim.screen);
    assert_consistent(&sim, &app);
    sim.save("hrv_fields");
    sim.nordic.sensors[0].data = |_| [4, 0, 0, 0, 0, 0, 0, 0]; // off the chest: no rate, no new beat
    run(&mut sim, &mut app, 7000);
    assert_eq!((text(&app, Dfa), text(&app, Rmssd)), ("--".into(), "--".into()), "no heart rate");
}

/// The text of row `i` of SYSTEM > BUILT IN: label and value.
pub(super) fn built_in_row(app: &App, i: usize) -> std::string::String {
    let (label, value, _) = app.menu_row(Menu::BuiltIn, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// The way from menu `from` to `row`, by the rows the app shows (the first found, at most 5 menus down; the main
/// menu walked into only from the ride menu, as ALL SETTINGS): each menu on the way and the row chosen in it.
pub(crate) fn path_to(app: &App, from: Menu, row: menu::Row) -> Option<Vec<(Menu, usize)>> {
    fn walk(app: &App, from: Menu, row: menu::Row, depth: usize) -> Option<Vec<(Menu, usize)>> {
        for i in 0..app.rows(from) {
            let r = app.row(from, i)?;
            if r == row {
                return Some(std::vec![(from, i)]);
            }
            if let menu::Row::Open(m) = r
                && (m != Menu::Main || matches!(from, Menu::Ride(_)))
                && depth < 5
                && let Some(mut p) = walk(app, m, row, depth + 1)
            {
                p.insert(0, (from, i));
                return Some(p);
            }
        }
        None
    }
    walk(app, from, row, 1)
}

/// From the first row of the menu shown, down to `row`, each menu on the way opened by Page and Start.
pub(crate) fn to_row(sim: &mut Sim, app: &mut App, row: menu::Row) {
    let Screen::Menu(from, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    let path = path_to(app, from, row).unwrap_or_else(|| panic!("{row:?} not in {from:?}"));
    for (k, &(_, i)) in path.iter().enumerate() {
        for _ in 0..i {
            app.press(sim, Press::Short(Key::Page));
        }
        if k + 1 < path.len() {
            app.press(sim, Press::Short(Key::Start));
        }
    }
}

/// Every menu below `from` and how deep (the main menu is 1), through the rows the app shows, the full menu not
/// walked again from the ride menu.
fn menus_under(app: &App, from: Menu, depth: usize, out: &mut Vec<(Menu, usize)>) {
    out.push((from, depth));
    for i in 0..app.rows(from) {
        if let Some(menu::Row::Open(m)) = app.row(from, i)
            && m != Menu::Main
            && out.iter().all(|&(seen, _)| seen != m)
        {
            menus_under(app, m, depth + 1, out);
        }
    }
}

/// The menu tree (UI review 4.3): every setting and every row of the old tree is in it, a setting at most three
/// levels down (MENU > SETTINGS > ME > FTP), the diagnostics under SYSTEM, every row's label and value whole on its
/// row (a number at the end of its range too, and while edited). Each menu, reached by the keys: Lap on the first
/// row and Page on the last stay there (no wrap), Back goes up one level at a time to the screen it was opened
/// from. A screenshot of each (`target/sim/menu_<title>.bmp`).
#[test]
fn menu_tree_reaches_everything() {
    use menu::{PhoneRow, Reading, Row, StockRow};
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    assert!(app.gps.pvt.is_some(), "the GPS rows all shown");
    let mut menus = Vec::new();
    menus_under(&app, Menu::Main, 1, &mut menus);
    // every row of every menu, with its level
    let mut rows = Vec::new();
    for &(m, depth) in &menus {
        for i in 0..app.rows(m) {
            rows.push((app.row(m, i).unwrap(), depth, m, i));
        }
    }
    let level = |row: Row| rows.iter().find(|r| r.0 == row).map(|r| r.1);
    let settings = [
        Setting::Wheel,
        Setting::Light,
        Setting::LightOff,
        Setting::Touch,
        Setting::MaxHr,
        Setting::Ftp,
        Setting::Weight,
        Setting::WPrime,
        Setting::Eat,
        Setting::Drink,
        Setting::HrAlert,
    ];
    let mut wanted: Vec<Row> = settings.iter().map(|&s| Row::Set(s)).collect();
    wanted.extend((0..4).map(|n| Row::Set(Setting::Remote(n))));
    wanted.extend((0..Kind::ALL.len()).map(Row::Sensor));
    wanted.push(Row::BtTrainer);
    wanted.extend(menu::PHONE.map(Row::Phone));
    wanted.extend(menu::FITNESS.map(Row::Stock));
    wanted.extend([Row::Stock(StockRow::Import), Row::Drive, Row::Nearby, Row::About]);
    wanted.extend((0..app.page_count()).map(Row::Layout));
    wanted.extend((0..app.page_count()).map(Row::EditPage));
    wanted.push(Row::AddPage);
    wanted.extend((0..4).map(|f| Row::Field(OVERVIEW, f)));
    wanted.extend((0..app.page_count()).map(|n| Row::Field(n, 0)));
    for &row in &wanted {
        let d = level(row).unwrap_or_else(|| panic!("{row:?} not in the tree"));
        // a page's own rows are one level deeper since the two sets (item 8): MENU > PAGES > ROAD PAGES > PAGE 1 > ...
        let page_row = matches!(row, Row::Layout(_) | Row::EditPage(_) | Row::Field(..));
        assert!(d <= 3 + usize::from(page_row), "{row:?}: {d} levels down");
    }
    assert_eq!(level(Row::Phone(PhoneRow::Forget)), Some(3), "SENSORS > PHONE");
    // the readings (diagnostics, nothing to set): under SYSTEM
    for r in menu::BUILT_IN.iter().chain(&menu::GPS) {
        assert_eq!(level(Row::Read(*r)), Some(4), "{r:?}: MENU > SETTINGS > SYSTEM > ...");
    }
    assert_eq!(level(Row::Read(Reading::Lat)), Some(4));
    for m in [Menu::Workouts, Menu::Courses, Menu::History, Menu::Stock] {
        assert!(menus.iter().any(|&(x, _)| x == m), "{m:?} reached");
    }
    assert_eq!(menu::MAIN.len(), ui::MENU_ROWS, "the main menu: one screen");
    // whole on the row: label and value side by side, numbers at the end of their range, and edited
    let whole = |label: &str, value: &str| {
        assert!(ui::LABEL.width(label) <= ui::menu_label_w(label, value), "{label} {value}: the label cut");
    };
    for &(row, _, m, i) in &rows {
        let (label, value, _) = app.menu_row(m, i).unwrap();
        whole(label.as_str(), value.as_str());
        if let Row::Set(s) = row
            && s.number().is_some()
        {
            for _ in 0..200 {
                app.settings.step(s, true);
            }
            app.screen = Screen::Menu(m, i);
            for editing in [false, true] {
                app.editing = editing;
                let (label, value, _) = app.menu_row(m, i).unwrap();
                whole(label.as_str(), value.as_str());
            }
            (app.editing, app.screen) = (false, Screen::Main);
        }
    }

    // by the keys, from the overview: each menu, its ends, Back level by level; a screenshot
    let mut sim = Sim::new(9, 0, 0);
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    for &(m, depth) in &menus {
        app.press(&mut sim, Press::Long(Key::Page));
        if m != Menu::Main {
            to_row(&mut sim, &mut app, Row::Open(m));
            app.press(&mut sim, Press::Short(Key::Start));
        }
        if app.screen() != Screen::Menu(m, 0) {
            // COURSES without a card: said, not opened
            assert_eq!((m, app.notes.text.as_str()), (Menu::Courses, "NO COURSE"));
            while app.screen() != Screen::Main {
                app.press(&mut sim, Press::Short(Key::Back));
            }
            continue;
        }
        run(&mut sim, &mut app, u64::from(LIST_REDRAW_MS) + 100); // a list that changes: redrawn once a second
        assert_consistent(&sim, &app);
        let mut title = Line::<16>::new();
        title.push_str(match m {
            Menu::Page(OVERVIEW) => "OVERVIEW",
            Menu::Main => "MAIN",
            _ => menu_title(m),
        });
        if let Menu::Page(n @ 0..crate::PAGES) = m {
            title.push_u32(n as u32 + 1);
        }
        let name: std::string::String = title
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        sim.save(&std::format!("menu_{}", name.trim_end_matches('_')));
        app.press(&mut sim, Press::Short(Key::Lap));
        assert_eq!(app.screen(), Screen::Menu(m, 0), "{m:?}: no wrap at the top");
        let last = app.rows(m).max(1) - 1;
        for _ in 0..=last {
            app.press(&mut sim, Press::Short(Key::Page));
        }
        assert_eq!(app.screen(), Screen::Menu(m, last), "{m:?}: no wrap at the bottom");
        assert_consistent(&sim, &app);
        for up in 1..depth {
            app.press(&mut sim, Press::Short(Key::Back));
            assert!(matches!(app.screen(), Screen::Menu(..)), "{m:?}: Back {up} of {depth}");
        }
        assert!(matches!(app.screen(), Screen::Menu(Menu::Main, _)), "{m:?}: up to the main menu");
        app.press(&mut sim, Press::Short(Key::Back));
        assert_eq!(app.screen(), Screen::Main, "{m:?}: {depth} Backs to the overview");
    }
}

/// The labels of the menu shown, one per row.
fn labels(app: &App) -> Vec<std::string::String> {
    let Screen::Menu(m, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    (0..app.rows(m)).map(|i| app.menu_row(m, i).unwrap().0.as_str().into()).collect()
}

/// The ride menu (UI review step 8): Page held while a ride records or is paused opens RIDE instead of the full
/// menu, with only the rows that apply: WORKOUT while a step runs, COURSE while one is followed, EDIT THIS PAGE but
/// on the course page. Each row acts: PAUSE stays (RESUME then, and END RIDE under it), RESUME goes back to the page,
/// BRIGHTNESS changes in place, NEXT STEP, STOP WORKOUT and STOP FOLLOWING act and go back to riding, END RIDE
/// pauses and opens the end-ride screen (RESUME back to the page, SAVE the ride's card). SENSORS, EDIT THIS PAGE and
/// ALL SETTINGS open their menus; Back goes up one level at a time; no wrap. Screenshots: ride_menu,
/// ride_menu_paused, ride_menu_workout.
#[test]
fn ride_menu() {
    use menu::{RideRow, Row};
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_course_memory(crate::storage::course_file::tests::memory());
    let hold_page = |sim: &mut Sim, app: &mut App| app.press(sim, Press::Long(Key::Page));
    let back = |sim: &mut Sim, app: &mut App, n| (0..n).for_each(|_| app.press(sim, Press::Short(Key::Back)));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "no ride: the full menu");
    back(&mut sim, &mut app, 1);
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    assert_eq!(app.screen(), Screen::Ride(0));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 0));
    let base = ["PAUSE", "END RIDE", "INDOOR", "BRIGHTNESS", "SENSORS", "ALL SETTINGS"];
    assert_eq!(labels(&app), base, "no workout, no course: their rows not shown; no page edited while recording");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu");
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 0), "no wrap");
    // BRIGHTNESS: in place
    to_row(&mut sim, &mut app, Row::Set(Setting::Light));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((sim.light_level, app.screen()), (100, Screen::Menu(Menu::Ride(0), 3)), "AUTO by default, then 100 %");
    // SENSORS, ALL SETTINGS: their menus, Back to the ride menu's row, then to the page
    for (row, m, at) in [(Row::Open(Menu::Sensors), Menu::Sensors, 4), (Row::Open(Menu::Main), Menu::Main, 5)] {
        back(&mut sim, &mut app, 1);
        hold_page(&mut sim, &mut app);
        to_row(&mut sim, &mut app, row);
        app.press(&mut sim, Press::Short(Key::Start));
        assert_eq!(app.screen(), Screen::Menu(m, 0), "{row:?}");
        back(&mut sim, &mut app, 1);
        assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), at));
    }
    back(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Ride(0), "Back: the page");
    assert_eq!(app.ride.state(), ride::State::Running, "nothing changed the ride");

    // PAUSE stays, the row says RESUME; RESUME goes back to the page
    hold_page(&mut sim, &mut app);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Paused, Screen::Menu(Menu::Ride(0), 0)));
    assert_eq!(labels(&app)[0], "RESUME");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu_paused");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Running, Screen::Ride(0)));

    // a workout from ALL SETTINGS > WORKOUTS: WORKOUT > NEXT STEP, STOP WORKOUT
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // the first one's steps
    app.press(&mut sim, Press::Short(Key::Start)); // START
    assert_eq!(app.screen(), Screen::Ride(0));
    let step = |app: &App| app.plan.player.as_ref().and_then(|p| p.step(app.plan.steps()));
    assert_eq!(step(&app), Some(0));
    hold_page(&mut sim, &mut app);
    assert_eq!(labels(&app)[2], "WORKOUT");
    run(&mut sim, &mut app, 1100);
    assert_consistent(&sim, &app);
    sim.save("ride_menu_workout");
    to_row(&mut sim, &mut app, Row::Ride(RideRow::NextStep));
    assert_eq!(app.screen(), Screen::Menu(Menu::WorkoutNow, 0));
    back(&mut sim, &mut app, 1);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(0), 2), "Back: the ride menu, on WORKOUT");
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // NEXT STEP
    assert_eq!((step(&app), app.screen()), (Some(1), Screen::Ride(0)));
    assert!(app.notes.text.as_str().starts_with("2/"), "the step said, no lap: {:?}", app.notes.text.as_str());
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::StopWorkout));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(
        (app.plan.player.is_none(), app.screen(), app.notes.text.as_str()),
        (true, Screen::Ride(0), "WORKOUT STOPPED")
    );
    assert_eq!(app.ride.state(), ride::State::Running, "the ride goes on");

    // a course from ALL SETTINGS > COURSES: on its page, COURSE > STOP FOLLOWING, and no EDIT THIS PAGE
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!((app.screen(), app.notes.text.as_str()), (Screen::Ride(COURSE), "COURSE ON"));
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Ride(COURSE), 0));
    assert_eq!(labels(&app), ["PAUSE", "END RIDE", "COURSE", "INDOOR", "BRIGHTNESS", "SENSORS", "ALL SETTINGS"]);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::StopCourse));
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.courses.as_deref().unwrap().on.is_none(), "not followed");
    assert_eq!((app.screen(), app.notes.text.as_str()), (Screen::Ride(0), "COURSE OFF"), "its page gone: the first");
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);

    // END RIDE: the end-ride screen (the ride paused), RESUME back to the page riding, then SAVE: the card
    hold_page(&mut sim, &mut app);
    assert_eq!(labels(&app), base);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::End));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Paused, Screen::End(1)));
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Short(Key::Lap)); // SAVE is selected: RESUME is one up
    app.press(&mut sim, Press::Short(Key::Start)); // RESUME: the page, the ride running again
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Running, Screen::Ride(0)));
    hold_page(&mut sim, &mut app);
    to_row(&mut sim, &mut app, Row::Ride(RideRow::End));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // SAVE, selected
    assert_eq!((app.ride.state(), app.screen()), (ride::State::Idle, Screen::Summary));
    run(&mut sim, &mut app, 200);
    assert_consistent(&sim, &app);
    back(&mut sim, &mut app, 2); // the banner, then the card
    assert_eq!(app.screen(), Screen::Main, "the card, then the overview");
    hold_page(&mut sim, &mut app);
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0), "no ride: the full menu again");
}

/// The index of `row` in `menu` as it is now (the sensors menu's actions come first, when they apply).
pub(super) fn row_in(app: &App, menu: Menu, row: menu::Row) -> usize {
    (0..app.rows(menu)).find(|&i| app.row(menu, i) == Some(row)).unwrap_or_else(|| panic!("{row:?} in {menu:?}"))
}

/// The row index of `row` in the menu that holds it (off a ride, from the main menu).
pub(super) fn row_of(row: menu::Row) -> usize {
    let mut sim = Sim::new(9, 0, 0);
    let app = App::start(&mut sim, None);
    path_to(&app, Menu::Main, row).and_then(|p| p.last().copied()).unwrap_or_else(|| panic!("{row:?}")).1
}

/// The text of row `i` of the menu shown: label and value.
pub(super) fn main_row(app: &App, i: usize) -> std::string::String {
    let Screen::Menu(m, _) = app.screen() else { panic!("not a menu: {:?}", app.screen()) };
    let (label, value, _) = app.menu_row(m, i).unwrap();
    format!("{} {}", label.as_str(), value.as_str())
}

/// While recording, the cog on the overview still opens the menu (the owner, 2026-10-27): a setting changed there
/// applies, and the ride goes on recording.
#[test]
fn cog_opens_settings_while_recording() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.state(), crate::ride::State::Running);
    for _ in 0..10 {
        if app.screen() == Screen::Main {
            break;
        }
        app.input(&mut sim, Input::Back); // the overview is Back from the ride pages
        run(&mut sim, &mut app, 1000);
    }
    assert_eq!(app.screen(), Screen::Main);
    let g = crate::ui::GEAR_TAP;
    app.input(&mut sim, Input::Touch(Gesture::Tap((g.0 + g.2 / 2) as u16, (g.1 + g.3 / 2) as u16)));
    assert_consistent(&sim, &app);
    assert!(matches!(app.screen(), Screen::Menu(Menu::Ride(_), _)), "the cog's menu: {:?}", app.screen());
    let light = app.settings.light_level;
    to_row(&mut sim, &mut app, menu::Row::Set(menu::Setting::Light));
    app.input(&mut sim, Input::Select);
    assert_ne!(app.settings.light_level, light, "the backlight changed while recording");
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.ride.state(), crate::ride::State::Running, "still recording");
    assert_eq!(app.rec.clean.phase(), crate::recorder::Phase::Recording);
}

/// SYSTEM scrolled to its last row: the last screen is full (the last `MENU_ROWS` rows, AUTO OFF at the bottom), not
/// one row at the top and empty slots under it (2026-10-29).
#[test]
fn last_screen_of_system_is_full() {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    app.press(&mut sim, Press::Long(Key::Page)); // the main menu
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::System));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..menu::SYSTEM.len() - 1 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    let last = menu::SYSTEM.len() - 1;
    assert_eq!(app.screen(), Screen::Menu(Menu::System, last));
    let first = menu::first_shown(last, ui::MENU_ROWS, app.rows(Menu::System));
    assert_eq!(first, menu::SYSTEM.len() - ui::MENU_ROWS);
    for slot in 0..ui::MENU_ROWS {
        assert!(app.menu_row(Menu::System, first + slot).is_some(), "slot {slot} is empty");
    }
    let bottom = app.menu_row(Menu::System, first + ui::MENU_ROWS - 1).map(|r| r.0.as_str() == "AUTO OFF");
    assert_eq!(bottom, Some(true));
}
