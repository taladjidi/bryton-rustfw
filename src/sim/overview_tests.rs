//! The overview's TODAY card and the WORKOUTS list's profile thumbnails on the Sim (ui.rs `draw_today`,
//! `draw_workout_row`, app/workouts.rs): what is drawn, the tap, the zones' colours, the cost.

use super::tests::assert_consistent as consistent;
use super::*;
use crate::storage::workout_file::{
    Calendar, Kind as TargetKind, Length, PROF, PROF_H, Planned, Step, Target, Workout,
};
use crate::ui::TODAY_BOX;

/// The strip catches up with the second that passed, then the screen is checked.
fn assert_consistent(sim: &mut Sim, app: &mut App) {
    run(sim, app, 1100);
    app.step(sim);
    consistent(sim, app);
}

/// The SMOOTH font, strip and page.
fn smooth(sim: &mut Sim, app: &mut App) {
    app.settings.smooth = true;
    app.redraw_top(sim);
    app.show(sim);
}

fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    app.set_usb(&mut sim, UsbDot::Ready);
    run(&mut sim, &mut app, 1000);
    (sim, app)
}

/// `secs` at `pct` % of FTP.
fn at(secs: u32, pct: u16) -> Step {
    Step {
        len: Length::Time(secs),
        target: Target { kind: TargetKind::Power, lo: pct, hi: pct, pct: true },
        intensity: 0,
    }
}

/// Sweet spot: warm-up, 3 x (12 min 90 %, 4 min 55 %), cool-down.
fn sweet_spot() -> Workout {
    let mut steps = std::vec![at(600, 55)];
    for _ in 0..3 {
        steps.extend([at(720, 90), at(240, 55)]);
    }
    steps.push(at(600, 50));
    Workout::built("SWEET SPOT 3X12", &steps)
}

/// Five hard minutes over and over.
fn vo2() -> Workout {
    let mut steps = std::vec![at(900, 60)];
    for _ in 0..5 {
        steps.extend([at(180, 120), at(180, 50)]);
    }
    steps.push(at(600, 50));
    Workout::built("VO2 5X3", &steps)
}

/// Two hours easy.
fn endurance() -> Workout {
    Workout::built("ENDURANCE", &[at(7200, 65)])
}

/// A calendar the way the card gives it: today and the days after, FTP 200 (the defaults).
fn calendar(app: &App, days: &[(u8, &Workout)]) -> Calendar {
    let mut c = Calendar::default();
    let file = crate::fat::File { cluster: 0, size: 0, dir: false };
    for &(d, w) in days {
        let day = (app.date.0, app.date.1, app.date.2 + d);
        c.add(Planned::of(day, file, w, (app.settings.ftp, app.settings.max_hr)));
    }
    c
}

/// Puts `c` where the app has it after reading the card.
fn plan(app: &mut App, days: &[(u8, &Workout)]) {
    app.calendar = calendar(app, days);
    (app.calendar_day, app.calendar_zones) = (Some(app.date), (app.settings.ftp, app.settings.max_hr));
}

/// The colour of column `k` of the TODAY card's profile, at its baseline, as the Sim's panel shows it.
fn card_pixel(sim: &Sim, k: usize) -> u16 {
    let (x, y, _, h) = TODAY_BOX;
    let row = (y + 46 + (h - 51) - 1) as usize;
    sim.screen[row * W as usize + (x + 4) as usize + 4 * k + 1]
}

/// A workout today: its name, length and profile on the card, in both fonts, and the screen is what the app draws.
#[test]
fn the_card_shows_todays_workout() {
    let (mut sim, mut app) = unit();
    let w = sweet_spot();
    plan(&mut app, &[(0, &w), (1, &vo2()), (3, &endurance())]);
    app.show(&mut sim);
    assert_consistent(&mut sim, &mut app);
    sim.save("overview_today");
    // the profile's columns are the steps' zones (FTP 200: 55 % is Z1, 90 % is Z3, 50 % Z1), column by column
    let p = app.today().unwrap().prof;
    for (k, &byte) in p.iter().enumerate() {
        let (h, color) = crate::storage::workout_file::unpack_bar(byte);
        let (want_h, want_c) = crate::workout::column(w.steps(), (200, 190), (PROF as u32, PROF_H), None, k as u32);
        assert_eq!((u32::from(h), color), (want_h, want_c), "column {k}");
        assert_eq!(card_pixel(&sim, k), color, "column {k} on the panel");
    }
    assert!(p.iter().any(|&b| crate::storage::workout_file::unpack_bar(b).1 == ride::power_color(3)), "the 90 % steps");
    smooth(&mut sim, &mut app);
    assert_consistent(&mut sim, &mut app);
    sim.save("overview_today_smooth");
}

/// Nothing planned today: REST DAY, grey, and no bars; before the calendar is read only the title.
#[test]
fn a_rest_day() {
    let (mut sim, mut app) = unit();
    plan(&mut app, &[(1, &vo2())]);
    app.show(&mut sim);
    assert_consistent(&mut sim, &mut app);
    assert!(app.today().is_none());
    assert!((0..PROF).all(|k| card_pixel(&sim, k) == ui::BLACK), "no profile");
    sim.save("overview_rest");
    smooth(&mut sim, &mut app);
    assert_consistent(&mut sim, &mut app);
    sim.save("overview_rest_smooth");
}

/// The card is read a few seconds after the start, not at boot, and then drawn without a restart of the screen.
#[test]
fn the_card_follows_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    assert!(app.calendar_day.is_none(), "not at boot");
    assert_consistent(&mut sim, &mut app);
    run(&mut sim, &mut app, 6000);
    assert_eq!(app.calendar_day, Some(app.date));
    assert!(app.today().is_some(), "the synthetic card plans a workout today");
    assert_consistent(&mut sim, &mut app);
}

/// A tap anywhere on the card opens WORKOUTS (Back returns to the overview); a tap on the cog still opens the menu;
/// before the calendar is read the tap reads it; while riding, touch does nothing.
#[test]
fn a_tap_on_the_card_opens_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 1500);
    assert!(app.calendar_day.is_none());
    let (x, y, w, h) = TODAY_BOX;
    let tap = |app: &mut App, sim: &mut Sim, x, y| app.input(sim, Input::Touch(Gesture::Tap(x as u16, y as u16)));
    for (tx, ty) in [(x + w / 2, y + h / 2), (x + 8, y + h - 8), (x + w - 8, y + h - 8)] {
        tap(&mut app, &mut sim, tx, ty);
        assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0), "({tx}, {ty})");
        assert!(app.today().is_some(), "read by the tap");
        assert_consistent(&mut sim, &mut app);
        app.input(&mut sim, Input::Back);
        assert_eq!(app.screen(), Screen::Main);
    }
    tap(&mut app, &mut sim, ui::GEAR_C.0, ui::GEAR_C.1);
    assert!(matches!(app.screen(), Screen::Menu(m, _) if m != Menu::Workouts), "the cog wins its corner");
    app.input(&mut sim, Input::Back);
    tap(&mut app, &mut sim, 10, ui::OVERVIEW_Y + 10);
    assert_eq!(app.screen(), Screen::Main, "a field is not the card");
    app.press(&mut sim, Press::Short(Key::Start)); // recording
    tap(&mut app, &mut sim, x + w / 2, y + h / 2);
    assert_ne!(app.screen(), Screen::Menu(Menu::Workouts, 0), "touch is off while recording");
}

/// The list: a profile thumbnail in each day's row beside the day and the length; the ramp test row stays plain.
#[test]
fn the_list_shows_profiles() {
    let (mut sim, mut app) = unit();
    let (a, b, c) = (sweet_spot(), vo2(), endurance());
    plan(&mut app, &[(0, &a), (1, &b), (2, &c), (5, &a)]);
    app.open(Screen::Menu(Menu::Workouts, 0));
    app.show(&mut sim);
    assert_consistent(&mut sim, &mut app);
    sim.save("workouts_list");
    app.input(&mut sim, Input::Next);
    app.input(&mut sim, Input::Next);
    assert_consistent(&mut sim, &mut app);
    sim.save("workouts_list_selected");
    smooth(&mut sim, &mut app);
    assert_consistent(&mut sim, &mut app);
    sim.save("workouts_list_smooth");
    // the thumbnail's bars are in the zones' colours, and the ramp test's row has none
    let row_px = |slot: usize| {
        let y0 = (ui::LIST_Y + slot as u32 * ui::MENU_ROW) as usize;
        (y0..y0 + ui::MENU_ROW as usize)
            .flat_map(|y| (110..ui::W as usize).map(move |x| (x, y)))
            .map(|(x, y)| sim.screen[y * W as usize + x])
            .collect::<std::vec::Vec<_>>()
    };
    assert!(row_px(0).contains(&ride::power_color(3)), "90 % of FTP");
    assert!(row_px(1).contains(&ride::power_color(ride::power_zone(240, 200))), "120 % of FTP");
    assert!(row_px(4).iter().all(|p| !ride::ZONE_COLORS.contains(p)), "no profile for the ramp test");
}

/// A new FTP recolours the profiles on the next open (the card is read again), and a new day reads the card again.
#[test]
fn zones_and_days_read_the_calendar_again() {
    // absolute watts: the zone depends on the FTP
    let w = Workout::built(
        "250W",
        &[Step {
            len: Length::Time(600),
            target: Target { kind: TargetKind::Power, lo: 250, hi: 250, pct: false },
            intensity: 0,
        }],
    );
    let file = crate::fat::File { cluster: 0, size: 0, dir: false };
    let [z200, z300] = [200, 300].map(|ftp| Planned::of((2026, 10, 23), file, &w, (ftp, 190)).prof[0]);
    assert_ne!(z200, z300, "250 W is Z5 at FTP 200 and Z3 at 300");

    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 10_000);
    assert_eq!(app.calendar_zones.0, 200);
    sim.emmc.cmds.clear();
    run(&mut sim, &mut app, 2000);
    assert!(sim.emmc.cmds.is_empty(), "nothing read while nothing changes");
    app.settings.ftp = 300;
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.calendar_zones.0, 300, "read again with the new FTP");
    assert!(!sim.emmc.cmds.is_empty());
    assert_consistent(&mut sim, &mut app);
}

/// A new day reads the calendar again and the card shows it (the Sim's clock runs through midnight).
#[test]
fn midnight_reads_the_calendar_again() {
    let mut sim = Sim::new(23, 59, 40);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 12_000);
    let day = app.date;
    assert_eq!(app.calendar_day, Some(day));
    run(&mut sim, &mut app, 12_000);
    assert_ne!(app.date, day, "midnight passed");
    assert_eq!(app.calendar_day, Some(app.date), "read again");
    assert_consistent(&mut sim, &mut app);
}
