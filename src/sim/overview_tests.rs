//! The overview's TODAY card and the WORKOUTS list's profile thumbnails on the Sim (src/ui/overview.rs `draw_today`,
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
        target2: Target::OPEN,
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
    app.plan.calendar = calendar(app, days);
    (app.plan.day, app.plan.zones) = (Some(app.date), (app.settings.ftp, app.settings.max_hr));
    app.plan.done_key = None;
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
    run(&mut sim, &mut app, 500);
    assert!(app.plan.day.is_none(), "not at boot");
    assert_consistent(&mut sim, &mut app);
    // the card is filled within 3 s of the start (8 s before 2026-10-27, measured in the Sim and on the unit)
    let mut ms = 500;
    while app.plan.day.is_none() && ms < 10_000 {
        run(&mut sim, &mut app, 100);
        ms += 100;
    }
    std::println!("TODAY card read {ms} ms after the start");
    assert!(ms <= 3000, "{ms} ms");
    run(&mut sim, &mut app, 1000);
    assert!(app.today().is_some(), "the synthetic card plans a workout today");
    assert_consistent(&mut sim, &mut app);
}

/// A tap anywhere on the card opens today's workout, its steps with START selected (Back returns to the overview);
/// a tap on the cog still opens the menu; before the calendar is read the tap reads it; while riding, touch does
/// nothing.
#[test]
fn a_tap_on_the_card_opens_the_calendar() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.emmc.image = crate::fat::tests::image();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 500);
    assert!(app.plan.day.is_none());
    let (x, y, w, h) = TODAY_BOX;
    let tap = |app: &mut App, sim: &mut Sim, x, y| app.input(sim, Input::Touch(Gesture::Tap(x as u16, y as u16)));
    for (tx, ty) in [(x + w / 2, y + h / 2), (x + 8, y + h - 8), (x + w - 8, y + h - 8)] {
        tap(&mut app, &mut sim, tx, ty);
        assert_eq!(app.screen(), Screen::Menu(Menu::Steps, 0), "({tx}, {ty})");
        assert_eq!(app.row(Menu::Steps, 0), Some(menu::Row::WorkoutStart), "START selected");
        assert!(app.today().is_some(), "read by the tap");
        assert_consistent(&mut sim, &mut app);
        app.input(&mut sim, Input::Back);
        assert_eq!(app.screen(), Screen::Main);
    }
    tap(&mut app, &mut sim, ui::GEAR_C.0, ui::GEAR_C.1);
    assert!(matches!(app.screen(), Screen::Menu(m, _) if m != Menu::Steps), "the cog wins its corner");
    app.input(&mut sim, Input::Back);
    tap(&mut app, &mut sim, 10, ui::OVERVIEW_Y + 10);
    assert_eq!(app.screen(), Screen::Main, "a field is not the card");
    app.press(&mut sim, Press::Short(Key::Start)); // recording
    tap(&mut app, &mut sim, x + w / 2, y + h / 2);
    assert_ne!(app.screen(), Screen::Menu(Menu::Steps, 0), "touch is off while recording");
}

/// Today's workout from the overview: before, the card (or Page held, WORKOUTS), a row to select and act, then START
/// (select, act): 5 taps. Now the card opens the steps with START selected, and one tap on START starts it (2 taps);
/// by keys, Start held then Start (2), as Page held, Start, Start, Start (4) was. A rest day opens WORKOUTS.
#[test]
fn todays_workout_in_two_inputs() {
    let (x, y, w, h) = TODAY_BOX;
    let start_row_y = (ui::LIST_Y + ui::MENU_ROW / 2) as u16;
    let card = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.own_settings();
        sim.emmc.image = crate::fat::tests::image(); // plans a workout today
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, 1500);
        (sim, app)
    };
    let (mut sim, mut app) = card();
    let mut inputs = 0;
    app.input(&mut sim, Input::Touch(Gesture::Tap((x + w / 2) as u16, (y + h / 2) as u16)));
    inputs += 1;
    assert_eq!(app.screen(), Screen::Menu(Menu::Steps, 0));
    assert_eq!(app.row(Menu::Steps, 0), Some(menu::Row::WorkoutStart));
    run(&mut sim, &mut app, 300);
    assert_consistent(&mut sim, &mut app);
    sim.save("today_steps");
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, start_row_y)));
    inputs += 1;
    assert!(app.plan.player.is_some() && app.ride.state() == ride::State::Running, "START started it");
    assert_eq!(inputs, 2, "was 5");

    // by keys
    let (mut sim, mut app) = card();
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Steps, 0), "Start held opens today's steps");
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.plan.player.is_some(), "Start starts it (2 keys, was 4)");
    // a rest day
    let (mut sim, mut app) = unit();
    plan(&mut app, &[(1, &sweet_spot())]);
    app.press(&mut sim, Press::Long(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Workouts, 0), "a rest day: the other days");
    // a ride in progress: Start held is the end-ride key, not this
    let (mut sim, mut app) = card();
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Start));
    assert_ne!(app.screen(), Screen::Menu(Menu::Steps, 0));
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
            target2: Target::OPEN,
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
    assert_eq!(app.plan.zones.0, 200);
    sim.emmc.cmds.clear();
    run(&mut sim, &mut app, 2000);
    assert!(sim.emmc.cmds.is_empty(), "nothing read while nothing changes");
    app.settings.ftp = 300;
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.plan.zones.0, 300, "read again with the new FTP");
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
    assert_eq!(app.plan.day, Some(day));
    run(&mut sim, &mut app, 12_000);
    assert_ne!(app.date, day, "midnight passed");
    assert_eq!(app.plan.day, Some(app.date), "read again");
    assert_consistent(&mut sim, &mut app);
}

/// A unit whose scan has run (so the history is ready), holding exactly `rides` (moving seconds) dated today, and
/// the sweet spot planned today.
fn with_rides(rides: &[u32]) -> (Sim, App) {
    let (mut sim, mut app) = unit();
    app.stock.start(app.date);
    while app.stock.status() == crate::stock::Status::Running {
        run(&mut sim, &mut app, u64::from(POLL_MS));
    }
    let base = app.stock.index.get(0).copied().expect("a synthetic ride");
    app.stock.index.clear();
    for &timer_s in rides {
        app.stock.index.add(crate::history::Ride {
            start: (app.date.0, app.date.1, app.date.2, 7, 0),
            timer_s,
            ..base
        });
    }
    plan(&mut app, &[(0, &sweet_spot())]);
    app.show(&mut sim);
    (sim, app)
}

fn done_pixels(sim: &Sim) -> usize {
    let (x, y, w, h) = ui::DONE_BOX;
    (y..y + h)
        .flat_map(|r| (x..x + w).map(move |c| (r, c)))
        .filter(|&(r, c)| sim.screen[(r * W + c) as usize] == ui::GREEN)
        .count()
}

/// A saved ride of the day that is long enough marks the workout done on the card and in the list; a short one
/// does not.
#[test]
fn a_ride_of_the_day_marks_the_workout_done() {
    let (mut sim, mut app) = with_rides(&[3600]);
    assert_consistent(&mut sim, &mut app);
    assert!(app.workout_done(0));
    assert!(done_pixels(&sim) > 0, "the tag");
    sim.save("overview_done");
    app.open(Screen::Menu(Menu::Workouts, 0));
    app.show(&mut sim);
    assert_consistent(&mut sim, &mut app);
    sim.save("workouts_done");
    let (mut sim, mut app) = with_rides(&[2000]);
    assert_consistent(&mut sim, &mut app);
    assert!(!app.workout_done(0));
    assert_eq!(done_pixels(&sim), 0);
}

/// Plays the planned workout of today to its Done, `ran_s` seconds after START, from the calendar or as the ramp test.
fn play_to_the_end(app: &mut App, ran_s: u32, planned: bool) {
    app.plan.workout = sweet_spot();
    app.plan.file = app.plan.calendar.iter().next().map(|p| p.file);
    (app.plan.planned, app.plan.ran_s) = (planned, ran_s);
    app.workout_event(crate::workout::Event::Done);
}

/// The workout played to its end counts whatever the ride; before the scan has the rides nothing is marked; the
/// ramp test, a Lap through every step and another day's play do not count.
#[test]
fn a_workout_played_to_its_end_is_done() {
    let (_sim, mut app) = with_rides(&[600]);
    assert!(!app.workout_done(0));
    play_to_the_end(&mut app, 10, true);
    assert!(!app.workout_done(0), "10 s of 4080: laps through the steps");
    play_to_the_end(&mut app, 4000, false);
    assert!(!app.workout_done(0), "the ramp test shares the player");
    play_to_the_end(&mut app, 3300, true);
    assert!(app.workout_done(0));
    app.plan.played = app.plan.played.map(|(d, f)| ((d.0, d.1, d.2 + 1), f));
    assert!(!app.workout_done(0), "played on another day");
    let (_, mut app) = unit();
    plan(&mut app, &[(0, &sweet_spot())]);
    app.refresh_done();
    assert!(!app.workout_done(0), "scan not ready");
}

/// Discarding the ride takes the played mark back.
#[test]
fn a_discarded_ride_is_not_a_workout_done() {
    let (mut sim, mut app) = with_rides(&[]);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    play_to_the_end(&mut app, 4000, true);
    assert!(app.workout_done(0));
    app.press(&mut sim, Press::Short(Key::Start)); // paused
    app.discard_ride();
    assert!(!app.workout_done(0));
}

/// The card redraws when a ride is saved (the history changes), and two workouts of a day need two rides.
#[test]
fn the_card_follows_the_history_and_a_ride_each() {
    let (mut sim, mut app) = with_rides(&[]);
    assert_consistent(&mut sim, &mut app);
    assert_eq!(done_pixels(&sim), 0);
    let start = (app.date.0, app.date.1, app.date.2, 7, 0);
    let base = crate::history::Ride { start, timer_s: 3600, ..crate::history::NONE };
    app.stock.index.add(base);
    assert_consistent(&mut sim, &mut app);
    assert!(done_pixels(&sim) > 0, "the card follows the new ride");
    plan(&mut app, &[(0, &vo2()), (0, &sweet_spot())]);
    app.refresh_done();
    let done = |app: &App| {
        (0..2).map(|k| (app.plan.calendar.get(k).map(|p| p.total_s), app.workout_done(k))).collect::<std::vec::Vec<_>>()
    };
    assert_eq!(done(&app).iter().filter(|x| x.1).count(), 1, "one ride, one workout");
    assert!(done(&app).iter().any(|&(t, d)| d && t == Some(4080)), "the longer");
    app.stock.index.add(crate::history::Ride { timer_s: 2700, ..base });
    app.refresh_done();
    assert_eq!(done(&app).iter().filter(|x| x.1).count(), 2, "the second ride takes the second workout");
}
