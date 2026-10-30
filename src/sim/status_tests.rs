//! QUICK STATUS (app/status.rs) on the Sim: a tap on a plain field of a ride page opens the rider's six fields, Back,
//! a tap or the time-out returns to the same page, and the fields with their own tap keep it.

use super::page_tests::{press, started};
use super::*;
use crate::page::Page;
use crate::ui::Field::{self, *};

/// A unit riding on its first ride page: four plain fields (speed, timer, distance, heart rate).
fn riding() -> (Sim, App) {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(4, [Speed, Timer, Dist, Hr, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 3000);
    (sim, app)
}

fn tap_at(sim: &mut Sim, app: &mut App, x: u32, y: u32) {
    app.input(sim, Input::Touch(Gesture::Tap(x as u16, y as u16)));
    run(sim, app, 100);
}

/// The middle of cell `k` of the page on screen.
fn cell(app: &App, k: usize) -> (u32, u32) {
    let (x, y, w, h) = app.widgets().nth(k).unwrap().0;
    (x + w / 2, y + h / 2)
}

/// A tap opens the chosen six fields in a 2 x 3 grid, the page keeps recording under it, Back / a tap / 10 s without
/// an input return to the same page; both ends of the theme's metrics draw.
#[test]
fn a_tap_on_a_field_opens_quick_status_and_back_returns() {
    for (n, theme) in crate::ui::tests::metric_ends().into_iter().enumerate() {
        let (mut sim, mut app) = riding();
        app.theme = theme;
        app.show(&mut sim);
        let (x, y) = cell(&app, 1);
        tap_at(&mut sim, &mut app, x, y);
        assert_eq!(app.screen(), Screen::Status, "end {n}");
        let shown: Vec<_> = app.widgets().map(|(_, w)| w).collect();
        let want: Vec<_> = app.settings.quick_page.fields[..6].iter().map(|&f| Widget::Field(f)).collect();
        assert_eq!(shown, want, "the chosen fields, six boxes");
        run(&mut sim, &mut app, 2500); // the values follow the seconds in their boxes
        super::tests::assert_consistent(&sim, &app);
        if n == 0 {
            sim.save("quick_status");
        }
        app.input(&mut sim, Input::Back);
        assert_eq!(app.screen(), Screen::Ride(0), "Back returns to the same page");
        assert_eq!(app.ride.state(), crate::ride::State::Running);
        // a tap closes it
        tap_at(&mut sim, &mut app, x, y);
        tap_at(&mut sim, &mut app, 100, 200);
        assert_eq!(app.screen(), Screen::Ride(0));
        // the time-out: 9 s still up, 11 s gone; an input starts the count again
        tap_at(&mut sim, &mut app, x, y);
        run(&mut sim, &mut app, 9000);
        assert_eq!(app.screen(), Screen::Status);
        app.input(&mut sim, Input::Next); // ignored, but a key: the count starts again
        run(&mut sim, &mut app, 9000);
        assert_eq!(app.screen(), Screen::Status, "an input restarts the count");
        run(&mut sim, &mut app, 2000);
        assert_eq!(app.screen(), Screen::Ride(0), "10 s without an input closes it");
        super::tests::assert_consistent(&sim, &app);
    }
}

/// OFF: a tap does nothing, as before.
#[test]
fn quick_status_off_does_nothing() {
    let (mut sim, mut app) = riding();
    app.settings.quick_status = false;
    let (x, y) = cell(&app, 1);
    tap_at(&mut sim, &mut app, x, y);
    assert_eq!(app.screen(), Screen::Ride(0));
}

/// A field with its own tap keeps it: a power average cycles its window, a map is a picture (its buttons), a graph
/// shows the next plot; and the fixed pages and the overview are not `Ride(n < PAGES)`.
#[test]
fn fields_with_their_own_tap_keep_it() {
    let (mut sim, mut app) = riding();
    app.pages[0] = Page::preset(6, [Power10s, Map, Graph, Hr, Speed, Timer, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    app.show(&mut sim);
    let avg = app.settings.wkt_avg;
    let (x, y) = cell(&app, 0);
    tap_at(&mut sim, &mut app, x, y);
    assert_eq!(app.screen(), Screen::Ride(0), "the power average");
    assert_ne!(app.settings.wkt_avg, avg, "cycled its window");
    for k in [1, 2] {
        let (x, y) = cell(&app, k);
        tap_at(&mut sim, &mut app, x, y);
        assert_eq!(app.screen(), Screen::Ride(0), "a picture ({k})");
    }
    let (x, y) = cell(&app, 3);
    tap_at(&mut sim, &mut app, x, y);
    assert_eq!(app.screen(), Screen::Status, "a plain field opens it");
    // the CLIMB page is a fixed page, not one of the rider's: a tap there does not open it
    app.input(&mut sim, Input::Back);
    app.screen = Screen::Ride(CLIMB);
    app.show(&mut sim);
    tap_at(&mut sim, &mut app, 60, 200);
    assert_ne!(app.screen(), Screen::Status);
}

/// PAGES > QUICK FIELDS: the picker sets a box, kept in the settings; a field too wide for the box is refused.
#[test]
fn quick_fields_are_picked_like_a_page_editor_field() {
    let (mut sim, mut app) = started();
    app.pick(&mut sim, QUICK, 2, Cadence);
    assert_eq!(app.settings.quick_page.fields[2], Cadence);
    let before = app.settings.quick_page;
    app.pick(&mut sim, QUICK, 3, Field::Map);
    assert_eq!(app.settings.quick_page, before, "a map does not fit a 120 x 98 box");
    assert_eq!(app.rows(Menu::Page(QUICK)), 6);
    assert!(app.rows(Menu::Pages) >= 6);
}
