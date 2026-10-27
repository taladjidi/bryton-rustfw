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

/// The unit with a power meter paired and heard (it answers a zero), a heart rate strap whose battery is low, a
/// speed sensor whose battery is good, a trainer paired, and a phone paired when `phone`.
fn with_meter_and_trainer() -> (Sim, App) {
    quick_unit(false)
}

fn power_pages(t: u64) -> [u8; 8] {
    // a battery page now and then (status 3, ok), between the power pages
    if t / 250 % 4 == 3 { [0x52, 0, 0xff, 0, 0, 0, 0x80, 3 | 3 << 4] } else { [0x10, 1, 0xff, 85, 0, 0, 210, 0] }
}

fn hr_pages(t: u64) -> [u8; 8] {
    if t / 250 % 4 == 3 { [0x52, 0, 0xff, 0, 0, 0, 0x20, 1 | 4 << 4] } else { [0, 0, 0, 0, 0, 0, 0, 72] }
}

fn speed_pages(t: u64) -> [u8; 8] {
    if t / 250 % 4 == 3 {
        [0x52, 0, 0xff, 0, 0, 0, 0xa0, 3 | 2 << 4]
    } else {
        let n = (t / 100) as u16;
        let (time, count) = ((n * 102).to_le_bytes(), n.to_le_bytes());
        [0, 0, 0, 0, time[0], time[1], count[0], count[1]]
    }
}

fn quick_unit(phone: bool) -> (Sim, App) {
    const ID: SensorId = SensorId { device: 7, trans: 5 };
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(ID);
    sim.backup[Kind::Hr.backup()] = Kind::Hr.to_word(SensorId { device: 9, trans: 1 });
    sim.backup[Kind::Speed.backup()] = Kind::Speed.to_word(SensorId { device: 11, trans: 1 });
    sim.backup[14] = Kind::Trainer.to_word(SensorId { device: 5555, trans: 0xf1 });
    sim.nordic.meter.answer = MeterAnswer::Zero(-12);
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power_pages, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 9, data: hr_pages, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x7b, id: 11, data: speed_pages, network: 0, ack: &[] });
    if phone {
        let mut p = PhoneSim::new(true);
        p.connect_at = 1000;
        p.script = std::vec![(3000, Act::Track("Song", "Band"))];
        sim.nordic.phone = Some(p);
    }
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    if phone {
        app.activate(&mut sim, Menu::Phone, 1); // PAIR
    }
    run(&mut sim, &mut app, 5000);
    assert!(app.power_heard());
    (sim, app)
}

fn swipe(app: &mut App, sim: &mut Sim, d: crate::touch::Dir) {
    app.input(sim, Input::Touch(Gesture::Swipe(d)));
}

/// A tap on the middle of a rectangle.
fn tap(app: &mut App, sim: &mut Sim, (x, y, w, h): (u32, u32, u32, u32)) {
    app.input(sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16)));
}

fn media_button(b: crate::ui::media::Button) -> (u32, u32, u32, u32) {
    let &(_, x, y, w, h) = crate::ui::quick::MEDIA.iter().find(|m| m.0 == b).unwrap();
    (x, y, w, h)
}

/// The quick panel: a swipe down opens it everywhere, with the media remote at its top. Playing or pausing is 2
/// inputs (a swipe down, a tap) from the overview and from a ride page (the overview's old panel needed the same
/// two, a ride page's swipe down opened the remote and a gesture or a tap acted); zeroing the power meter is 2 (it
/// was 6 through SENSORS); the brightness is 2. A swipe up or Back closes it.
#[test]
fn the_quick_panel_has_the_remote_the_meter_and_the_light() {
    use crate::touch::Dir;
    use crate::ui::media::Button;
    use crate::ui::quick as q;
    let (mut sim, mut app) = quick_unit(true);
    assert_eq!(app.phone.link(), Link::Secure);
    let mut n = Count(0);

    // media from the overview: swipe down, tap (the phone gets the toggle)
    n.0 += 1;
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.screen(), Screen::Quick);
    run(&mut sim, &mut app, 1500);
    n.0 += 1;
    tap(&mut app, &mut sim, media_button(Button::Play));
    run(&mut sim, &mut app, 300);
    assert_eq!((n.0, sim.nordic.phone.as_ref().unwrap().commands.clone()), (2, std::vec![2]), "play / pause: 2 inputs");
    tap(&mut app, &mut sim, media_button(Button::VolUp));
    run(&mut sim, &mut app, 300);
    assert_eq!(sim.nordic.phone.as_ref().unwrap().commands, [2, 5], "volume up");
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel");

    // the meter: ZERO in place, the result on the same block
    n.0 = 1; // the swipe down
    n.0 += 1;
    tap(&mut app, &mut sim, q::ZERO);
    run(&mut sim, &mut app, 1500);
    assert_eq!((app.screen(), n.0), (Screen::Quick, 2), "ZERO: swipe down, tap, and the panel stays");
    assert_eq!(app.powercal.phase(), crate::powercal::Phase::Done(-12, None));
    assert_eq!(app.quick_text(5).0.as_str(), "ZERO -12 OK");
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel_zero");

    // the sensors' batteries, from page 82: the strap low, the speed sensor good, the meter ok
    assert_eq!(app.decoders.battery_status(Kind::Hr), Some(4));
    assert_eq!(app.decoders.battery_status(Kind::Speed), Some(2));
    assert_eq!(app.decoders.battery_status(Kind::Power), Some(3));
    sim.save("quick_panel_sensors");

    // the brightness: brighter, then AUTO at the bright end; darker down to the dimmest
    app.settings.light_level = 1;
    tap(&mut app, &mut sim, q::BRIGHT[1]);
    assert_eq!(app.settings.light_level, 0, "brighter: 100 %");
    tap(&mut app, &mut sim, q::BRIGHT[1]);
    assert_eq!(app.settings.light_level, crate::LIGHT_AUTO, "then AUTO");
    tap(&mut app, &mut sim, q::BRIGHT[0]);
    assert_eq!(app.settings.light_level, 0);
    for _ in 0..9 {
        tap(&mut app, &mut sim, q::BRIGHT[0]);
    }
    assert_eq!(app.settings.light_level, crate::LIGHT_LEVELS.len() - 1, "the dimmest, and no further");
    super::tests::assert_consistent(&sim, &app);

    // closing: a swipe up, Back
    swipe(&mut app, &mut sim, Dir::Up);
    assert_eq!(app.screen(), Screen::Main);
    swipe(&mut app, &mut sim, Dir::Down);
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Main);

    // a ride page: the same two inputs
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1100);
    assert!(matches!(app.screen(), Screen::Ride(_)));
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.screen(), Screen::Quick, "not the remote: the panel, from a ride page too");
    tap(&mut app, &mut sim, media_button(Button::Next));
    run(&mut sim, &mut app, 300);
    assert_eq!(sim.nordic.phone.as_ref().unwrap().commands.last(), Some(&3), "next: 2 inputs");
    // riding: ZERO is refused, as the calibration screen is
    assert!(app.ride.state() == ride::State::Running);
    tap(&mut app, &mut sim, q::ZERO);
    assert_eq!(app.notes.text.as_str(), "NO CALIBRATION RIDING");
    swipe(&mut app, &mut sim, Dir::Up);
    assert!(matches!(app.screen(), Screen::Ride(_)), "closed over the page it opened on");
}

/// The panel left open redraws only the boxes whose value changed: with a meter, a strap, a speed sensor, a trainer
/// and a phone, a second costs the watts' and values' boxes (a few hundred pixels), not the panel (measured below).
#[test]
fn the_quick_panel_costs_little_a_second() {
    use crate::touch::Dir;
    let (mut sim, mut app) = quick_unit(true);
    swipe(&mut app, &mut sim, Dir::Down);
    run(&mut sim, &mut app, 3000);
    let (px, t0) = (sim.pixels, sim.ms);
    run(&mut sim, &mut app, 20_000);
    let per_s = (sim.pixels - px) * 1000 / (sim.ms - t0);
    std::println!("quick panel left open: {per_s} px/s");
    assert!(per_s <= 1000, "{per_s} px/s");
}

/// Without a meter, a sensor or a phone: no power block, no sensor line, the media buttons greyed and a tap on
/// one says so. The panel shows a low battery in red (the strap's), nothing else red.
#[test]
fn the_quick_panel_shows_only_what_is_paired() {
    use crate::touch::Dir;
    let (mut sim, mut app) = unit();
    swipe(&mut app, &mut sim, Dir::Down);
    assert_eq!(app.screen(), Screen::Quick);
    assert_eq!(app.quick_shape(), 0, "no meter, no sensor line");
    tap(&mut app, &mut sim, media_button(crate::ui::media::Button::Play));
    assert_eq!(app.notes.text.as_str(), "NO PHONE");
    // a tap where the power block would be does nothing
    tap(&mut app, &mut sim, crate::ui::quick::ZERO);
    assert_eq!(app.screen(), Screen::Quick);
    assert!(!app.powercal.asking());
    run(&mut sim, &mut app, 300);
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel_empty");
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
        (Menu::Pages, menu::Row::Open(Menu::PageList(ride::Profile::Indoor))),
        (Menu::PageList(ride::Profile::Indoor), menu::Row::Open(Menu::Page(0))),
    ] {
        let i = at(&app, menu, row);
        n.tap_row(&mut sim, &mut app, i);
    }
    assert_eq!((app.screen(), n.0), (Screen::Menu(Menu::Page(0), 0), 4));
    assert!(app.view_set() == ride::Profile::Indoor, "the indoor set");
}
