//! Pages switched off, moved, renamed, the auto scroll and the swipe up (app/pagemeta.rs, page.rs `Meta`) on the Sim.

use super::menu_tests::to_row;
use super::page_tests::{file, press, reboot, started};
use super::*;
use crate::page::{Meta, Saved};

/// The menu of road page `n` (PAGES > ROAD PAGES > the page), by the keys.
fn open_page_menu(sim: &mut Sim, app: &mut App, n: usize) {
    app.press(sim, Press::Long(Key::Page));
    to_row(sim, app, menu::Row::Open(Menu::PageList(ride::Profile::Road)));
    press(sim, app, Key::Start);
    to_row(sim, app, menu::Row::Open(Menu::Page(n)));
    press(sim, app, Key::Start);
    assert!(matches!(app.screen(), Screen::Menu(Menu::Page(m), _) if m == n), "{:?}", app.screen());
}

/// The new bytes round trip, an older file (zeros there) reads with every page ON and the default names, and a
/// damaged part gives the defaults with the pages above it intact.
#[test]
fn pages_bin_keeps_the_switches_and_reads_an_old_file() {
    let (_, app) = started();
    let mut meta = Meta::DEFAULT;
    meta.off = 0b100010;
    meta.names[0] = *b"HILL";
    meta.names[5] = *b"Z9 A";
    meta.auto = 3;
    let s = Saved {
        seq: 5,
        pages: core::array::from_fn(|i| app.pages()[i.min(app.page_count() - 1)]),
        count: 3,
        words: [0; 6],
        was: [0; 6],
        more: [None; PAGES],
        indoor: None,
        mtb: None,
        meta,
    };
    let sector = s.to_sector();
    assert_eq!(Saved::from_sector(&sector), Some(s));
    // an older build's sector: nothing past the MTB part
    let mut old = sector;
    old[489..].fill(0);
    let read = Saved::from_sector(&old).unwrap();
    assert_eq!((read.meta, read.count, read.pages), (Meta::DEFAULT, 3, s.pages));
    assert!((0..PAGES).all(|k| read.meta.on(k) && read.meta.name(k).is_empty()));
    // a bit flipped in the part: the defaults, the pages whole
    let mut bad = sector;
    bad[493] ^= 4;
    assert_eq!(Saved::from_sector(&bad), Some(Saved { meta: Meta::DEFAULT, ..s }));
    // a code outside the alphabet (with the CRC made to match is the fuzz's job): refused through the CRC here
    assert_eq!(Meta::DEFAULT.name(0), "");
}

/// OFF pages are skipped by Page and the swipes, at least one stays ON, and it survives a restart.
#[test]
fn paging_skips_pages_that_are_off() {
    let (mut sim, mut app) = started();
    open_page_menu(&mut sim, &mut app, 1);
    to_row(&mut sim, &mut app, menu::Row::PageOp(1, menu::PageOp::Show));
    sim.save("page_menu_switches");
    press(&mut sim, &mut app, Key::Start);
    assert!(!app.sw.meta.on(1) && app.sw.meta.on(0));
    app.screen = Screen::Ride(0);
    app.show(&mut sim);
    press(&mut sim, &mut app, Key::Page);
    assert_eq!(app.screen(), Screen::Ride(2), "page 2 is OFF");
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Right)));
    assert_eq!(app.screen(), Screen::Ride(0));
    // all but one OFF: the last stays
    app.toggle_page(&mut sim, 0);
    assert!(!app.sw.meta.on(0));
    app.toggle_page(&mut sim, 2);
    assert!(app.sw.meta.on(2), "the last ON page stays");
    assert_eq!(app.notes.text.as_str(), "ONE PAGE AT LEAST");
    run(&mut sim, &mut app, 400);
    assert_eq!(file(&sim).unwrap().meta, app.sw.meta, "written with the pages");
    let (_, app2) = reboot(&mut sim);
    assert_eq!(app2.sw.meta, app.sw.meta);
    assert!(!app2.page_on(0) && !app2.page_on(1) && app2.page_on(2));
}

/// MOVE UP / DOWN swap the page with its name and switch, the menu follows it, and it is kept.
#[test]
fn pages_move_and_keep_their_names() {
    let (mut sim, mut app) = started();
    let (p0, p1) = (app.pages[0], app.pages[1]);
    app.sw.meta.names[1] = *b"HILL";
    open_page_menu(&mut sim, &mut app, 1);
    to_row(&mut sim, &mut app, menu::Row::PageOp(1, menu::PageOp::Up));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.pages[0], app.pages[1]), (p1, p0));
    assert_eq!((app.sw.meta.name(0), app.sw.meta.name(1)), ("HILL", ""));
    assert!(matches!(app.screen(), Screen::Menu(Menu::Page(0), _)), "the menu follows the page");
    assert!(app.page_ops(0).all(|o| o != menu::PageOp::Up) && app.page_ops(2).all(|o| o != menu::PageOp::Down));
    run(&mut sim, &mut app, 400);
    let f = file(&sim).unwrap();
    assert_eq!((f.pages[0], f.meta.name(0)), (p1, "HILL"));
    let (_, app2) = reboot(&mut sim);
    assert_eq!((app2.pages[0], app2.sw.meta.name(0)), (p1, "HILL"));
}

/// RENAME types four characters with Page and Start; the list and the title show the name; Back keeps what was typed.
#[test]
fn a_page_is_renamed_by_keys() {
    let (mut sim, mut app) = started();
    open_page_menu(&mut sim, &mut app, 0);
    to_row(&mut sim, &mut app, menu::Row::PageOp(0, menu::PageOp::Rename));
    press(&mut sim, &mut app, Key::Start);
    assert!(app.sw.rename.is_some());
    for steps in [3, 1, 0, 2] {
        for _ in 0..steps {
            press(&mut sim, &mut app, Key::Page); // a character on
        }
        press(&mut sim, &mut app, Key::Start); // the next one; after the last, done
    }
    assert!(app.sw.rename.is_none());
    assert_eq!(app.sw.meta.name(0), "CA B");
    assert_eq!(app.sw.meta.names[0], *b"CA B");
    sim.save("page_renamed_menu");
    let mut l = Line::<16>::new();
    app.page_label(0, &mut l);
    assert_eq!(l.as_str(), "CA B");
    // from the list too
    press(&mut sim, &mut app, Key::Back);
    sim.save("page_renamed_list");
    app.leave_menus();
    // Back ends the typing and keeps it
    open_page_menu(&mut sim, &mut app, 1);
    to_row(&mut sim, &mut app, menu::Row::PageOp(1, menu::PageOp::Rename));
    press(&mut sim, &mut app, Key::Start);
    press(&mut sim, &mut app, Key::Page);
    press(&mut sim, &mut app, Key::Back);
    assert_eq!((app.sw.rename, app.sw.meta.name(1)), (None, "A"));
    run(&mut sim, &mut app, 400);
    let (_, app2) = reboot(&mut sim);
    assert_eq!((app2.sw.meta.name(0), app2.sw.meta.name(1)), ("CA B", "A"));
}

/// A page added or deleted moves the switches and names after it.
#[test]
fn adding_and_deleting_pages_shift_the_names() {
    let (mut sim, mut app) = started();
    app.sw.meta.names[1] = *b"TWO ";
    app.sw.meta.off = 0b000100; // page 3 OFF
    app.add_page(&mut sim, 0);
    app.leave_editor(&mut sim, false);
    assert_eq!((app.sw.meta.name(2), app.sw.meta.on(3), app.sw.meta.on(0)), ("TWO", false, true));
    app.edit_page(&mut sim, 0, 0);
    app.delete_page(&mut sim);
    assert_eq!((app.sw.meta.name(1), app.sw.meta.on(2), app.page_count()), ("TWO", false, 3));
}

/// AUTO SCROLL: pages advance by themselves during a ride, only the ON ones, held by a menu, MORE, the CLIMB page and
/// any key or touch.
#[test]
fn auto_scroll_turns_pages_by_itself() {
    let (mut sim, mut app) = started();
    // off: never
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 1500);
    assert_eq!(app.ride.state(), ride::State::Running);
    app.screen = Screen::Ride(0);
    app.show(&mut sim);
    run(&mut sim, &mut app, 30_000);
    assert_eq!(app.screen(), Screen::Ride(0));
    // the row cycles OFF, 5, 10, 20 s
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Pages));
    press(&mut sim, &mut app, Key::Start);
    to_row(&mut sim, &mut app, menu::Row::AutoScroll);
    let value = |app: &App| app.menu_row(Menu::Pages, 3).map(|r| std::string::String::from(r.1.as_str()));
    assert_eq!(value(&app).as_deref(), Some("OFF"));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((value(&app).as_deref(), app.auto_secs()), (Some("5 S"), 5));
    // while the menu is up nothing turns
    run(&mut sim, &mut app, 12_000);
    assert!(matches!(app.screen(), Screen::Menu(..)));
    app.leave_menus();
    app.screen = Screen::Ride(0);
    app.show(&mut sim);
    run(&mut sim, &mut app, 4_500);
    assert_eq!(app.screen(), Screen::Ride(0));
    // a touch starts the count again
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, 200)));
    run(&mut sim, &mut app, 4_500);
    assert_eq!(app.screen(), Screen::Ride(0), "the touch reset the count");
    run(&mut sim, &mut app, 1_000);
    assert_eq!(app.screen(), Screen::Ride(1));
    // page 3 OFF: after page 2 comes page 1 again
    app.toggle_page(&mut sim, 2);
    run(&mut sim, &mut app, 5_200);
    assert_eq!(app.screen(), Screen::Ride(0), "the OFF page is skipped, the round closes");
    // MORE up: held; the climb page: held
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert!(app.more_shown().is_some());
    run(&mut sim, &mut app, 12_000);
    assert_eq!(app.screen(), Screen::Ride(0));
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Down)));
    app.screen = Screen::Ride(CLIMB);
    run(&mut sim, &mut app, 12_000);
    assert_eq!(app.screen(), Screen::Ride(CLIMB));
}

/// A swipe up on a ride page shows its MORE variant, a second one goes home (the overview).
#[test]
fn swipe_up_goes_home() {
    let (mut sim, mut app) = started();
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(1), Some(1)));
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert_eq!((app.screen(), app.more_shown()), (Screen::Main, None));
    // a page that has no MORE page of its own to show (the course page) goes home at once
    app.screen = Screen::Ride(COURSE);
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    assert_eq!(app.screen(), Screen::Main);
}
