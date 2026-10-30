//! A past ride's lap list (HISTORY, the ride, LAPS): a ride recorded with the Lap key, saved, read back from the card.

use super::menu_tests::to_row;
use super::tests::run;
use super::*;

/// Opens the card of history row `n` (row 0 is FITNESS).
fn open_card(sim: &mut Sim, app: &mut App, n: usize) {
    for _ in 0..20 {
        match app.screen() {
            Screen::Menu(Menu::History, s) if s < n => app.press(sim, Press::Short(Key::Page)),
            Screen::Menu(Menu::History, s) if s > n => app.press(sim, Press::Short(Key::Lap)),
            _ => break,
        }
    }
    app.press(sim, Press::Short(Key::Start));
    for _ in 0..100 {
        run(sim, app, u64::from(POLL_MS));
        if app.screen() == Screen::Past {
            break;
        }
    }
    assert_eq!(app.screen(), Screen::Past);
}

fn tap(app: &mut App, sim: &mut Sim, (x, y, w, h): (u32, u32, u32, u32)) {
    app.input(sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16)));
}

/// Records a ride with `laps` Lap presses `gap_ms` apart, saves it, and opens the history on its card; returns the
/// ride's name and its row.
fn record_and_open(sim: &mut Sim, app: &mut App, laps: u32, gap_ms: u64) -> ([u8; 12], usize) {
    run(sim, app, 2000);
    app.press(sim, Press::Short(Key::Start));
    for _ in 0..laps {
        run(sim, app, gap_ms);
        app.press(sim, Press::Short(Key::Lap));
    }
    run(sim, app, 10_000);
    app.press(sim, Press::Short(Key::Start));
    let name = app.rec.clean.name();
    app.press(sim, Press::Long(Key::Start));
    app.press(sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Summary);
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 3000);
    app.press(sim, Press::Long(Key::Page));
    to_row(sim, app, menu::Row::Open(Menu::History));
    app.press(sim, Press::Short(Key::Start));
    while app.stock.status() == crate::stock::Status::Running {
        run(sim, app, u64::from(POLL_MS));
        assert!(sim.ms < 900_000);
    }
    run(sim, app, 1000);
    // the new ride's row: the one whose start is in its file name
    let d: Vec<u32> = name[..10].iter().map(|b| u32::from(b - b'0')).collect();
    let n2 = |i: usize| (d[i] * 10 + d[i + 1]) as u8;
    let want = (2000 + u16::from(n2(0)), n2(2), n2(4), n2(6), n2(8));
    let row =
        (0..8).find(|&i| app.stock.index.get(i).is_some_and(|r| r.start == want)).expect("the ride is listed") + 1;
    open_card(sim, app, row);
    (name, row)
}

/// A power meter (210 W) and a heart-rate strap (72 bpm) paired and heard (interaction_tests' pages).
fn power_and_hr(sim: &mut Sim) {
    fn power(t: u64) -> [u8; 8] {
        if t / 250 % 4 == 3 { [0x52, 0, 0xff, 0, 0, 0, 0x80, 3 | 3 << 4] } else { [0x10, 1, 0xff, 85, 0, 0, 210, 0] }
    }
    fn hr(t: u64) -> [u8; 8] {
        if t / 250 % 4 == 3 { [0x52, 0, 0xff, 0, 0, 0, 0x20, 1 | 4 << 4] } else { [0, 0, 0, 0, 0, 0, 0, 72] }
    }
    sim.backup[Kind::Power.backup()] = Kind::Power.to_word(SensorId { device: 7, trans: 5 });
    sim.backup[Kind::Hr.backup()] = Kind::Hr.to_word(SensorId { device: 9, trans: 1 });
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power, network: 0, ack: &[] });
    sim.nordic.sensors.push(AntSensor { device_type: 0x78, id: 9, data: hr, network: 0, ack: &[] });
}

/// Three Lap keys during a ride with a power meter and a strap, saved: its card has LAPS, which lists four laps (the
/// last one closed at the end), each a time and a distance and a speed, and the lap's own average power and heart
/// rate from the ride file (fit.rs, "Laps"); Page scrolls (nothing to scroll), Back returns to the card. A ride without
/// laps (the synthetic volume's) says NO LAPS. Screenshots: history_laps, history_no_laps.
#[test]
fn history_lap_list() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    power_and_hr(&mut sim);
    let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8])); // a stand-in ANT+ key
    app.set_map_memory(crate::tiles::tests::memory());
    let (_, row) = record_and_open(&mut sim, &mut app, 3, 25_000);
    run(&mut sim, &mut app, 4000);
    sim.save("history_card_laps");
    tap(&mut app, &mut sim, ui::card::LAPS_TAP);
    assert_eq!(app.screen(), Screen::Laps(0));
    let l = app.past.laps();
    assert_eq!(l.len(), 4, "{l:?}");
    assert!(l[..3].iter().all(|r| r.timer_s().is_some_and(|t| (24..=26).contains(&t))), "{l:?}");
    assert!(l.iter().all(|r| (r.power(), r.hr()) == (Some(210), Some(72))), "{l:?}");
    run(&mut sim, &mut app, 500);
    super::tests::assert_consistent(&sim, &app);
    sim.save("history_laps");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Laps(0), "four laps fit: nothing to scroll");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Past);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, row));

    // the synthetic volume's 22 Oct ride has none
    let other = if row == 1 { 2 } else { 1 };
    open_card(&mut sim, &mut app, other);
    app.press(&mut sim, Press::Short(Key::Page)); // Page opens the list too
    assert_eq!(app.screen(), Screen::Laps(0));
    assert!(app.past.laps().is_empty());
    run(&mut sim, &mut app, 500);
    super::tests::assert_consistent(&sim, &app);
    sim.save("history_no_laps");
}

/// A ride of 120 laps: the list keeps 99 and says so, Page and a swipe up stop at the last screenful, Prev and a
/// swipe down at the first, Back lands on the card.
#[test]
fn history_lap_list_scrolls_and_stops() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    record_and_open(&mut sim, &mut app, 119, 1000);
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Laps(0));
    assert_eq!((app.past.laps().len(), app.past.laps_total()), (crate::history::MAX_LAPS, 120));
    sim.save("history_laps_cut");
    let last = crate::history::MAX_LAPS - ui::card::LAP_ROWS;
    for k in 0..crate::history::MAX_LAPS + 5 {
        let i = if k % 2 == 0 { Input::Next } else { Input::Touch(Gesture::Swipe(Dir::Up)) };
        app.input(&mut sim, i);
    }
    assert_eq!(app.screen(), Screen::Laps(last));
    for k in 0..crate::history::MAX_LAPS + 5 {
        let i = if k % 2 == 0 { Input::Prev } else { Input::Touch(Gesture::Swipe(Dir::Down)) };
        app.input(&mut sim, i);
        assert!(matches!(app.screen(), Screen::Laps(t) if t <= last));
    }
    assert_eq!(app.screen(), Screen::Laps(0));
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Past);
}

/// DELETE armed on a card, then LAPS and Back: the next Start only dismisses the card, the ride stays.
#[test]
fn armed_delete_does_not_survive_the_lap_list() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    let (name, row) = record_and_open(&mut sim, &mut app, 2, 5000);
    tap(&mut app, &mut sim, ui::card::DELETE_TAP);
    assert_eq!(app.confirm, Some(PAST_DELETE), "asked");
    tap(&mut app, &mut sim, ui::card::LAPS_TAP);
    assert_eq!(app.screen(), Screen::Laps(0));
    assert_eq!(app.confirm, None);
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Past);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::History, row), "dismissed, not deleted");
    assert!(super::recorder_tests::ride_file(&sim, "Activities", name).is_some(), "the ride stays");
}
