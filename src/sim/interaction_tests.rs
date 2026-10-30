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
    let (mut sim, mut app) = started();
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
    // a torque page (88 % and 90 %, pedal smoothness 70 % and 72 %) in every fourth broadcast
    if t / 250 % 4 == 1 { [0x13, 1, 176, 180, 140, 144, 0, 0] } else { power_pages_plain(t) }
}

/// The power meter without its torque page: a battery page now and then (status 3, ok), between the power pages.
fn power_pages_plain(t: u64) -> [u8; 8] {
    if t / 250 % 4 == 3 { [0x52, 0, 0xff, 0, 0, 0, 0x80, 3 | 3 << 4] } else { [0x10, 1, 0xff, 85, 0, 0, 210, 0] }
}

/// A single-sided meter: the right torque effectiveness 0xff (not used), the rest as [`power_pages`].
fn power_pages_one_sided(t: u64) -> [u8; 8] {
    if t / 250 % 4 == 1 { [0x13, 1, 176, 0xff, 0xff, 0xff, 0, 0] } else { power_pages_plain(t) }
}

#[test]
fn a_single_sided_meter_shows_its_left_and_a_dash_for_the_right() {
    let (mut sim, mut app) = with_meter_and_trainer();
    for s in sim.nordic.sensors.iter_mut().filter(|s| s.device_type == 0x0b) {
        s.data = power_pages_one_sided;
    }
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.field_text(Field::TorqueEff).as_str(), "88/--");
    assert_eq!(app.field_text(Field::PedalSmooth).as_str(), "--/--");
}

/// Page 0x13 reaches the screen: the fields show the values, then dashes once the meter stops sending the page.
#[test]
fn torque_and_smoothness_show_then_dash_when_the_meter_stops_them() {
    let (mut sim, mut app) = with_meter_and_trainer();
    assert_eq!(app.field_text(Field::TorqueEff).as_str(), "88/90");
    assert_eq!(app.field_text(Field::PedalSmooth).as_str(), "70/72");
    for s in sim.nordic.sensors.iter_mut().filter(|s| s.device_type == 0x0b) {
        s.data = power_pages_plain;
    }
    run(&mut sim, &mut app, 6000);
    assert_eq!(app.field_text(Field::TorqueEff).as_str(), "--/--", "not sent for 5 s: no value");
    assert_eq!(app.field_text(Field::PedalSmooth).as_str(), "--/--");
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

/// The POWER line of the quick panel, as a rectangle to tap.
fn power_line(app: &App) -> (u32, u32, u32, u32) {
    use crate::ui::quick as q;
    (0, q::SENSORS_Y + app.quick_power().unwrap() as u32 * q::ROW, crate::ui::W, q::ROW)
}

fn media_button(b: crate::ui::media::Button) -> (u32, u32, u32, u32) {
    let &(_, x, y, w, h) = crate::ui::quick::MEDIA.iter().find(|m| m.0 == b).unwrap();
    (x, y, w, h)
}

/// The quick panel: a swipe down opens it everywhere, with the media remote at its top. Playing or pausing is 2
/// inputs (a swipe down, a tap) from the overview and from a ride page (the overview's old panel needed the same
/// two, a ride page's swipe down opened the remote and a gesture or a tap acted); zeroing the power meter is 3 (swipe, two
/// taps; it was 6 through SENSORS); the brightness is 2. A swipe up or Back closes it.
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

    // the meter: the POWER line arms ZERO, a second tap calibrates, the result shows on the button
    n.0 = 1; // the swipe down
    let line = power_line(&app);
    let value = |app: &App| app.quick_button().0;
    assert_eq!(value(&app).as_str(), "ZERO");
    n.0 += 1;
    tap(&mut app, &mut sim, line);
    assert_eq!((value(&app).as_str(), app.powercal.asking()), ("SURE?", false), "the first tap only arms");
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel_armed");
    n.0 += 1;
    tap(&mut app, &mut sim, line);
    run(&mut sim, &mut app, 1500);
    assert_eq!((app.screen(), n.0), (Screen::Quick, 3), "ZERO: swipe down, two taps, and the panel stays");
    assert_eq!(app.powercal.phase(), crate::powercal::Phase::Done(-12, None));
    assert_eq!(value(&app).as_str(), "-12 OK");
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel_zero");

    // the sensors' batteries, from page 82: the strap low, the speed sensor good, the meter ok
    assert_eq!(app.decoders.battery_status(Kind::Hr), Some(4));
    assert_eq!(app.decoders.battery_status(Kind::Speed), Some(2));
    assert_eq!(app.decoders.battery_status(Kind::Power), Some(3));
    sim.save("quick_panel_sensors");
    run(&mut sim, &mut app, 6000);
    assert_eq!(value(&app).as_str(), "ZERO", "the result gives way to ZERO after about 5 s");
    super::tests::assert_consistent(&sim, &app);

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
    let line = power_line(&app);
    tap(&mut app, &mut sim, line);
    assert_eq!(app.quick_button().0.as_str(), "ZERO", "riding: no arming, nothing said");
    assert!(app.quick_zero.0.is_none());
    swipe(&mut app, &mut sim, Dir::Up);
    assert!(matches!(app.screen(), Screen::Ride(_)), "closed over the page it opened on");
}

/// ZERO is armed by a first tap on the POWER line and runs on a second within 3 s; a second tap later only arms
/// again; a meter that cannot calibrate (not heard) is not armed. The panel stays consistent throughout.
#[test]
fn zero_on_the_quick_panel_needs_two_taps_in_time() {
    use crate::touch::Dir;
    let (mut sim, mut app) = quick_unit(true);
    swipe(&mut app, &mut sim, Dir::Down);
    run(&mut sim, &mut app, 1500);
    let line = power_line(&app);
    tap(&mut app, &mut sim, line);
    run(&mut sim, &mut app, 4500); // the arming lapses (3 s, the clock counts whole seconds)
    assert_eq!(app.quick_button().0.as_str(), "ZERO", "reverted");
    super::tests::assert_consistent(&sim, &app);
    tap(&mut app, &mut sim, line);
    assert_eq!((app.quick_button().0.as_str(), app.powercal.asking()), ("SURE?", false), "late: only arms again");
    super::tests::assert_consistent(&sim, &app);
    // armed, then the meter is lost: the button is greyed ZERO, not SURE?, and a tap does nothing
    app.quick_zero.0 = None;
    tap(&mut app, &mut sim, line);
    assert_eq!(app.quick_button().0.as_str(), "SURE?");
    app.radio.heard_ms[Kind::Power as usize] = u32::MAX; // the meter is lost
    assert_eq!(app.quick_button().0.as_str(), "ZERO", "cannot calibrate: SURE? is withdrawn");
    tap(&mut app, &mut sim, line);
    assert!(app.quick_zero.0.is_none() && !app.powercal.asking());
    super::tests::assert_consistent(&sim, &app);
    // heard no more: a tap does nothing
    app.quick_zero.0 = None;
    app.radio.heard_ms[Kind::Power as usize] = u32::MAX;
    assert!(!app.power_heard());
    tap(&mut app, &mut sim, line);
    assert!(app.quick_zero.0.is_none() && !app.powercal.asking(), "cannot calibrate: no arming");
}

/// The panel left open redraws only the boxes whose value changed: with a meter, a strap, a speed sensor, a trainer
/// and a phone, a second costs the values' boxes (a few hundred pixels), not the panel (measured below).
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

/// Without a meter, a sensor or a phone: no POWER line, no sensor line, the media buttons greyed and a tap on
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
    // a tap where the POWER line would be does nothing
    tap(&mut app, &mut sim, (0, crate::ui::quick::SENSORS_Y, crate::ui::W, 16));
    assert_eq!(app.screen(), Screen::Quick);
    assert!(!app.powercal.asking());
    run(&mut sim, &mut app, 300);
    super::tests::assert_consistent(&sim, &app);
    sim.save("quick_panel_empty");
}

/// A title wider than its box scrolls (a step every 250 ms, a hold at each end) and redraws its own box; a title that
/// fits never moves.
#[test]
fn a_long_title_scrolls_and_a_short_one_does_not() {
    use crate::touch::Dir;
    let (mut sim, mut app) = quick_unit(true);
    swipe(&mut app, &mut sim, Dir::Down);
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.quick_text(0).0.as_str(), "Song");
    run(&mut sim, &mut app, 5000);
    let at = sim.ms + 100;
    let song = Act::Track("A VERY LONG SONG TITLE THAT OVERFLOWS", "Band");
    sim.nordic.phone.as_mut().unwrap().script.push((at, song));
    run(&mut sim, &mut app, 1000);
    // the 5th sensor line (POWER among them), paired under the open panel (the shape changes: a full redraw)
    app.radio.sensors[Kind::Cadence as usize] = Sensor::Paired(SensorId { device: 13, trans: 1 });
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.quick_shape(), 5);
    super::tests::assert_consistent(&sim, &app);
    // a step every 250 ms: the panel is whole after each, a step is small, a hold sends nothing
    let (mut first, mut moved, mut held, mut big) = (std::string::String::new(), false, 0, 0);
    for step in 0..80 {
        let px = sim.pixels;
        run(&mut sim, &mut app, 250);
        let spent = sim.pixels - px;
        // a step is the title's 224 x 20 box (and the odd value); the rare big one is the strip's minute or the like
        big += usize::from(spent >= 2 * 224 * 20);
        super::tests::assert_consistent(&sim, &app);
        let text = std::string::String::from(app.quick_text(0).0.as_str());
        if step == 0 {
            first = text.clone();
        }
        moved |= text != first;
        held += usize::from(spent < 224 * 20);
    }
    assert!(moved, "the visible text moved");
    assert!(big <= 3, "{big} steps cost over {} px", 2 * 224 * 20);
    assert!(held >= 10, "the holds and the seconds between send nothing ({held} quiet steps)");
    assert!(app.quick_scroll.1 > 8);
    // the same title with another artist starts over
    let at = sim.ms + 100;
    let song = Act::Track("A VERY LONG SONG TITLE THAT OVERFLOWS", "Other");
    sim.nordic.phone.as_mut().unwrap().script.push((at, song));
    run(&mut sim, &mut app, 1000);
    assert!(app.quick_scroll.1 <= 5, "restarted: {}", app.quick_scroll.1);
    super::tests::assert_consistent(&sim, &app);
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

/// A power high limit crossed by a power meter's 210 W: after 5 s over it the banner and the beeps, on the ride
/// page; not before.
#[test]
fn power_high_alert_shows_the_banner() {
    let (mut sim, mut app) = quick_unit(false);
    app.settings.limits[0] = 150;
    app.press(&mut sim, Press::Short(Key::Start));
    let t0 = sim.tones.len();
    for s in 0..4 {
        run(&mut sim, &mut app, 1000);
        assert_ne!(app.notes.text.as_str(), "POWER HIGH", "second {s}: not before the hold");
    }
    run(&mut sim, &mut app, 4000);
    assert_eq!(app.notes.text.as_str(), "POWER HIGH");
    assert!(sim.tones.len() > t0, "the beeps");
    sim.save("alert_power_high");
}

/// A strap that sends its battery (page 7: 87 %) and its maker (common page 80: POLAR) between its beats.
fn strap_pages(t: u64) -> [u8; 8] {
    match t / 250 % 4 {
        1 => [0x87, 87, 0x40, 0x42, 0, 0, 0, 0],
        2 => [0x50, 0xff, 0xff, 1, 123, 0, 2, 1],
        _ => [0, 0, 0, 0, 0, 0, 0, 72],
    }
}

/// The strap's battery in percent and its maker, from its pages: the HR BATT field shows the percent, and the
/// SENSORS row names the maker (its device number is the value), which the screen shows.
#[test]
fn strap_name_and_battery() {
    const STRAP: SensorId = SensorId { device: 9, trans: 1 };
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[Kind::Hr.backup()] = Kind::Hr.to_word(STRAP);
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 9, data: strap_pages, network: 0, ack: &[] });
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
    run(&mut sim, &mut app, 5000);
    assert_eq!((app.decoders.maker(Kind::Hr), app.decoders.battery_percent(Kind::Hr)), (Some(123), Some(87)));
    assert_eq!(app.field_text(crate::ui::Field::HrBatt).as_str(), "87%");
    let i = super::tests::row_in(&app, Menu::Sensors, crate::menu::Row::Sensor(Kind::Hr as usize));
    let (label, value, _) = app.menu_row(Menu::Sensors, i).unwrap();
    assert_eq!((label.as_str(), value.as_str()), ("POLAR HR", "9"));
    run(&mut sim, &mut app, 4000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, crate::menu::Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    sim.save("sensors_names");
}
