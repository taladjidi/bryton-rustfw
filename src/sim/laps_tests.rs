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

/// Three Lap keys during a ride, saved: its card has LAPS, which lists four laps (the last one closed at the end),
/// each a time and a distance and a speed; Page scrolls (nothing to scroll), Back returns to the card. A ride without
/// laps (the synthetic volume's) says NO LAPS. Screenshots: history_laps, history_no_laps.
#[test]
fn history_lap_list() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_map_memory(crate::tiles::tests::memory());
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..3 {
        run(&mut sim, &mut app, 25_000);
        app.press(&mut sim, Press::Short(Key::Lap));
    }
    run(&mut sim, &mut app, 10_000);
    app.press(&mut sim, Press::Short(Key::Start));
    let name = app.rec.clean.name();
    app.press(&mut sim, Press::Long(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Summary);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);

    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::History));
    app.press(&mut sim, Press::Short(Key::Start));
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
        assert!(sim.ms < 600_000);
    }
    run(&mut sim, &mut app, 1000);
    // the new ride's row: the one whose start is in its file name
    let digits: Vec<u32> = name[..10].iter().map(|b| u32::from(b - b'0')).collect();
    let n2 = |i: usize| (digits[i] * 10 + digits[i + 1]) as u8;
    let want = (2000 + u16::from(n2(0)), n2(2), n2(4), n2(6), n2(8));
    let row =
        (0..8).find(|&i| app.stock.index.get(i).is_some_and(|r| r.start == want)).expect("the ride is listed") + 1;
    open_card(&mut sim, &mut app, row);
    run(&mut sim, &mut app, 4000);
    sim.save("history_card_laps");
    tap(&mut app, &mut sim, ui::card::LAPS_TAP);
    assert_eq!(app.screen(), Screen::Laps(0));
    let l = app.past.laps();
    assert_eq!(l.len(), 4, "{l:?}");
    assert!(l[..3].iter().all(|r| (24..=26).contains(&r.timer_s)), "{l:?}");
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
