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
