//! Interaction counts (docs/features.md, "Interactions"): how many keys or taps a task takes, asserted so a menu
//! that gets deeper again fails a test. Each test names the count before the change it was written for.

use super::*;
use crate::touch::Gesture;

/// Inputs given by [`Count`]: keys and taps, the cost the owner feels.
struct Count(u32);

impl Count {
    fn key(&mut self, sim: &mut Sim, app: &mut App, p: Press) {
        self.0 += 1;
        app.press(sim, p);
    }

    /// A tap on the middle of menu row `row` of the menu shown (its screen of rows).
    fn tap_row(&mut self, sim: &mut Sim, app: &mut App, row: usize) {
        self.0 += 1;
        let y = ui::LIST_Y + (row % ui::MENU_ROWS) as u32 * ui::MENU_ROW + ui::MENU_ROW / 2;
        app.input(sim, Input::Touch(Gesture::Tap(120, y as u16)));
    }
}

/// The row of `menu` that is `row`.
fn at(app: &App, menu: Menu, row: menu::Row) -> usize {
    (0..app.rows(menu)).find(|&i| app.row(menu, i) == Some(row)).unwrap_or_else(|| panic!("{row:?} in {menu:?}"))
}

fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1000);
    (sim, app)
}

/// Item 1: one tap acts on a menu row. Before: PAGES from the overview was Page held, a tap to select, a tap to act
/// (3); now 2. While the ride runs and moves, a tap still only selects (3 again): a bump must not act.
#[test]
fn one_tap_acts_on_a_row_unless_the_ride_moves() {
    let (mut sim, mut app) = unit();
    let mut n = Count(0);
    n.key(&mut sim, &mut app, Press::Long(Key::Page));
    let pages = at(&app, Menu::Main, menu::Row::Open(Menu::Pages));
    n.tap_row(&mut sim, &mut app, pages);
    assert_eq!((app.screen(), n.0), (Screen::Menu(Menu::Pages, 0), 2), "was 3");

    // a setting that cycles: one tap, one change
    let (mut sim, mut app) = unit();
    app.open(Screen::Menu(Menu::Display, 0));
    app.show(&mut sim);
    let light = at(&app, Menu::Display, menu::Row::Set(menu::Setting::Light));
    let before = app.settings.light_level;
    Count(0).tap_row(&mut sim, &mut app, light);
    assert_ne!(app.settings.light_level, before, "one tap cycled it");

    // a ride running and moving: select, then act
    let (mut sim, mut app) = unit();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1500);
    assert!(app.ride.moving());
    app.press(&mut sim, Press::Long(Key::Page));
    let menu = Menu::Ride(0); // Start goes to the first page
    let light = at(&app, menu, menu::Row::Set(menu::Setting::Light));
    let before = app.settings.light_level;
    let mut n = Count(0);
    n.tap_row(&mut sim, &mut app, light);
    assert_eq!((app.settings.light_level, app.screen()), (before, Screen::Menu(menu, light)), "selected only");
    n.tap_row(&mut sim, &mut app, light);
    assert_ne!(app.settings.light_level, before, "the second tap acts");
    assert_eq!(n.0, 2);
}

/// The unit with a power meter paired and heard, and a trainer paired (the quick panel's conditional rows).
fn with_meter_and_trainer() -> (Sim, App) {
    const ID: SensorId = SensorId { device: 7, trans: 5 };
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(ID);
    sim.backup[14] = Kind::Trainer.to_word(SensorId { device: 5555, trans: 0xf1 });
    let data = |_| [0x10, 1, 0xff, 85, 0, 0, 210, 0];
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    run(&mut sim, &mut app, 3000);
    assert!(app.power_heard());
    (sim, app)
}

fn swipe(app: &mut App, sim: &mut Sim, d: crate::touch::Dir) {
    app.input(sim, Input::Touch(Gesture::Swipe(d)));
}

/// Item 3: the quick panel. Calibrating the power meter from the overview: before, Page held, SENSORS (tap, tap),
/// a swipe to its second screen and CALIBRATE POWER (tap, tap) = 7 touches (5 with one-tap rows); now a swipe down
/// and a tap = 2. The panel lists only what applies, closes with a swipe up or Back, and a ride page keeps the
/// media remote on a swipe down.
#[test]
fn the_quick_panel_from_the_overview() {
    use crate::touch::Dir;
    let (mut sim, mut app) = with_meter_and_trainer();
    let mut n = Count(0);
    n.0 += 1;
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.screen(), Screen::Menu(Menu::Quick, 0));
    let labels: Vec<_> = (0..app.rows(Menu::Quick))
        .map(|i| std::string::String::from(app.menu_row(Menu::Quick, i).unwrap().0.as_str()))
        .collect();
    assert_eq!(labels, ["CALIBRATE POWER", "TRAINER CONTROL", "PROFILE", "BRIGHTNESS", "USB DRIVE", "NOTIFICATIONS"]);
    run(&mut sim, &mut app, 300);
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel");
    n.tap_row(&mut sim, &mut app, 0);
    assert_eq!((app.screen(), n.0), (Screen::PowerCal, 2), "was 7");
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Menu(Menu::Quick, 0), "Back returns to the panel");
    swipe(&mut app, &mut sim, Dir::Up);
    assert_eq!(app.screen(), Screen::Main, "a swipe up closes it");
    swipe(&mut app, &mut sim, Dir::Down);
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Main, "Back closes it");

    // without a meter or a trainer: fewer rows
    let (mut sim, mut app) = unit();
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.rows(Menu::Quick), 4);

    // a ride page: the remote, as before
    app.input(&mut sim, Input::Back);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1100);
    assert!(matches!(app.screen(), Screen::Ride(_)));
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.screen(), Screen::Media);
}

/// Item 4: SENSORS puts its actions first. With a meter heard and a trainer paired, CALIBRATE POWER was row
/// ALL + 2 (a second screen of rows, that many Page presses) and TRAINER CONTROL row ALL + 1, ALL being the number
/// of sensor kinds; now rows 0 and 1, so the first one is selected when the menu opens.
#[test]
fn sensors_actions_come_first() {
    let (_sim, app) = with_meter_and_trainer();
    let cal = at(&app, Menu::Sensors, menu::Row::Ride(menu::RideRow::CalPower));
    let trainer = at(&app, Menu::Sensors, menu::Row::Ride(menu::RideRow::Trainer));
    assert_eq!((cal, trainer), (0, 1), "were {} and {}", Kind::ALL.len() + 2, Kind::ALL.len() + 1);
    assert_eq!(app.row(Menu::Sensors, 2), Some(menu::Row::Sensor(0)), "then the sensors");
    let last = app.rows(Menu::Sensors) - 1;
    assert_eq!(app.row(Menu::Sensors, last), Some(menu::Row::Open(Menu::Phone)));
    // without them the menu is as it was
    let (_sim, app) = unit();
    assert_eq!(app.row(Menu::Sensors, 0), Some(menu::Row::Sensor(0)));
}

/// Item 8: the indoor page set is a row (INDOOR PAGES), not a toggle to set first. From the overview to an indoor
/// page's menu: Page held, PAGES, INDOOR PAGES, PAGE 1 = 4 inputs (it was Page held, PAGES, the PAGES OF toggle, PAGE
/// 1 = 4: the same count, but the state a toggle left is gone, so the two sets cannot be mixed up).
#[test]
fn the_indoor_page_set_is_a_row() {
    let (mut sim, mut app) = unit();
    let mut n = Count(0);
    n.key(&mut sim, &mut app, Press::Long(Key::Page));
    for (menu, row) in [
        (Menu::Main, menu::Row::Open(Menu::Pages)),
        (Menu::Pages, menu::Row::Open(Menu::PageList(true))),
        (Menu::PageList(true), menu::Row::Open(Menu::Page(0))),
    ] {
        let i = at(&app, menu, row);
        n.tap_row(&mut sim, &mut app, i);
    }
    assert_eq!((app.screen(), n.0), (Screen::Menu(Menu::Page(0), 0), 4));
    assert!(app.view_indoor(), "the indoor set");
}
