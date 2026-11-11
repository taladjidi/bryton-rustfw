//! The tap-count metric: the rider's tasks (a UI review's list) as the shortest known input sequence that reaches a
//! target on the App, run for real on the Sim. Each task has a budget, the count before the fixes it measured; one
//! that gets longer fails. Writes target/sim/taps.txt (task, actions, budget, weight), which tools/numbers.py reads
//! for `n:taps`.
//!
//! An action is a key press (short or long), a tap on a row or a button, or a swipe that brings a row on screen:
//! the cost the owner feels (interaction_tests.rs counts a few by hand). The sequences are the best known, not
//! searched: `App` and `Sim` cannot be cloned, so a search would replay from the start for every candidate.

use super::course_tests::hill;
use super::menu_tests::row_in;
use super::tests::run;
use super::*;
use crate::touch::{Dir, Gesture};
use menu::{RideRow, Row};

/// The inputs a task is given, counted.
struct Run {
    sim: Sim,
    app: App,
    n: u32,
}

impl Run {
    /// A unit with a course memory and a card, the power meter paired; `riding`: a ride started and moving.
    fn new(riding: bool) -> Self {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        let k = crate::sensors::Kind::Power;
        sim.backup[k.backup()] = k.to_word(crate::SensorId { device: 25126, trans: 0xf1 });
        let mut app = App::start(&mut sim, Some([1, 2, 3, 4, 5, 6, 7, 8]));
        app.set_course_memory(crate::storage::course_file::tests::memory());
        run(&mut sim, &mut app, 2000);
        let mut r = Run { sim, app, n: 0 };
        if riding {
            r.app.press(&mut r.sim, Press::Short(Key::Start));
            run(&mut r.sim, &mut r.app, 1500);
            assert!(r.app.ride.moving());
        }
        r
    }

    fn key(&mut self, p: Press) {
        self.n += 1;
        self.app.press(&mut self.sim, p);
        run(&mut self.sim, &mut self.app, 1000);
    }

    fn touch(&mut self, g: Gesture) {
        self.n += 1;
        self.app.input(&mut self.sim, Input::Touch(g));
        run(&mut self.sim, &mut self.app, 1000);
    }

    /// The menu shown, and its selected row.
    fn menu(&self) -> (Menu, usize) {
        let Screen::Menu(m, sel) = self.app.screen() else { panic!("not a menu: {:?}", self.app.screen()) };
        (m, sel)
    }

    /// Is `row` in the menu shown?
    fn has(&self, row: Row) -> bool {
        let Screen::Menu(m, _) = self.app.screen() else { return false };
        (0..self.app.rows(m)).any(|i| self.app.row(m, i) == Some(row))
    }

    /// Keys: Page / Lap to `row` of the menu shown, then Start.
    fn keys_to(&mut self, row: Row) {
        // looked up again at each press: rows can appear while the rider moves (a power meter heard)
        for _ in 0..40 {
            let (m, sel) = self.menu();
            let to = row_in(&self.app, m, row);
            if to == sel {
                break;
            }
            self.key(Press::Short(if to > sel { Key::Page } else { Key::Lap }));
        }
        self.key(Press::Short(Key::Start));
    }

    /// Touch: swipe up to the screen of rows that holds `row`, then tap it (twice while the ride moves).
    fn tap_to(&mut self, row: Row) {
        let (m, sel) = self.menu();
        let to = row_in(&self.app, m, row);
        let per = self.app.menu_rows();
        let mut first = menu::first_shown(sel, per, self.app.rows(m));
        while to >= first + per {
            self.touch(Gesture::Swipe(Dir::Up));
            let (_, sel) = self.menu();
            first = menu::first_shown(sel, per, self.app.rows(m));
        }
        let at = |r: &Run, first: usize| {
            ui::LIST_Y + ((to - first) as u32) * r.app.theme.menu_row + r.app.theme.menu_row / 2
        };
        let y = at(self, first);
        self.touch(Gesture::Tap(120, y as u16));
        if matches!(self.app.screen(), Screen::Menu(x, _) if x == m) && self.app.ride.moving() {
            // the first only selects, and the window of rows may shift under the finger
            let (_, sel) = self.menu();
            let first = menu::first_shown(sel, per, self.app.rows(m));
            let y = at(self, first);
            self.touch(Gesture::Tap(120, y as u16));
        }
    }

    /// Opens the menu from where the rider is: by key (Page held) or by touch (the gear of the overview).
    fn open_menu(&mut self, touch: bool) {
        if !touch {
            return self.key(Press::Long(Key::Page));
        }
        // a ride is open: the clock of the strip; else the overview's gear
        if self.app.ride.state() != ride::State::Idle {
            let t = ui::MENU_TAP;
            let tap = Gesture::Tap((t.0 + t.2 / 2) as u16, (t.1 + t.3 / 2) as u16);
            self.touch(tap);
            if !matches!(self.app.screen(), Screen::Menu(..)) {
                self.touch(tap); // a banner was up: that tap only cleared it
            }
            return;
        }
        let t = ui::GEAR_TAP;
        self.touch(Gesture::Tap((t.0 + t.2 / 2) as u16, (t.1 + t.3 / 2) as u16));
    }

    /// Each row of the path, in turn; a row the menu shown does not have fails the task.
    fn rows(&mut self, touch: bool, rows: &[Row]) {
        for &r in rows {
            // the course list is read once the menu is open: the hill is found then
            let r =
                if matches!(r, Row::Course(_)) && self.has(Row::Course(0)) { Row::Course(hill(&self.app)) } else { r };
            assert!(self.has(r), "{r:?} is not in the menu shown: {:?}", self.app.screen());
            if touch { self.tap_to(r) } else { self.keys_to(r) }
        }
    }

    /// `target`, straight from the menu shown or, when it is not there, through `via` (the main menu, SENSORS).
    fn go(&mut self, touch: bool, target: Row, via: Row) {
        if !self.has(target) {
            self.rows(touch, &[via]);
        }
        self.rows(touch, &[target]);
    }

    /// Opens the menu, then each row of the path.
    fn path(&mut self, touch: bool, rows: &[Row]) {
        self.open_menu(touch);
        self.rows(touch, rows);
    }

    /// Follows the hill course (not counted), for the tasks that start with one followed.
    fn follow(&mut self) {
        self.open_menu(false);
        self.go(false, Row::Open(Menu::Courses), Row::Open(Menu::Main));
        self.rows(false, &[Row::Course(0)]);
        self.n = 0;
        assert!(self.app.page_on(COURSE));
    }
}

/// One task: name, weight (a task in a ride counts double), budget, and how it runs; returns the actions it took.
struct Task {
    name: &'static str,
    weight: u32,
    budget: u32,
    run: fn() -> u32,
}

fn done(r: &Run, ok: bool, name: &str) -> u32 {
    assert!(ok, "{name}: the target was not reached, screen {:?}", r.app.screen());
    r.n
}

const IDLE: u32 = 1;
const RIDING: u32 = 2;

fn on(r: &Run, m: Menu) -> bool {
    matches!(r.app.screen(), Screen::Menu(x, _) if x == m)
}

fn tasks() -> Vec<Task> {
    fn t(name: &'static str, weight: u32, budget: u32, run: fn() -> u32) -> Task {
        Task { name, weight, budget, run }
    }
    std::vec![
        t("start a ride", IDLE, 1, || {
            let mut r = Run::new(false);
            r.key(Press::Short(Key::Start));
            done(&r, r.app.ride.state() == ride::State::Running, "start")
        }),
        t("course before the ride, keys", IDLE, 5, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::Courses), Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("course before the ride, touch", IDLE, 3, || {
            let mut r = Run::new(false);
            r.path(true, &[Row::Open(Menu::Courses), Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("history menu, keys", IDLE, 5, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::History)]);
            done(&r, on(&r, Menu::History), "history")
        }),
        t("phone menu, keys", IDLE, 8, || {
            let mut r = Run::new(false);
            r.open_menu(false);
            r.go(false, Row::Open(Menu::Phone), Row::Open(Menu::Sensors));
            done(&r, on(&r, Menu::Phone), "phone")
        }),
        t("phone menu, touch", IDLE, 3, || {
            let mut r = Run::new(false);
            r.open_menu(true);
            r.go(true, Row::Open(Menu::Phone), Row::Open(Menu::Sensors));
            done(&r, on(&r, Menu::Phone), "phone")
        }),
        t("GPS menu, keys", IDLE, 17, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::Settings), Row::Open(Menu::System), Row::Open(Menu::Gps)]);
            done(&r, on(&r, Menu::Gps), "gps")
        }),
        t("course in a ride, keys", RIDING, 6, || {
            let mut r = Run::new(true);
            r.open_menu(false);
            r.go(false, Row::Open(Menu::Courses), Row::Open(Menu::Main));
            r.rows(false, &[Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("course in a ride, touch", RIDING, 6, || {
            let mut r = Run::new(true);
            r.open_menu(true);
            r.go(true, Row::Open(Menu::Courses), Row::Open(Menu::Main));
            r.rows(true, &[Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("workout in a ride, keys", RIDING, 7, || {
            let mut r = Run::new(true);
            let w = Row::Open(Menu::Workouts);
            r.open_menu(false);
            r.go(false, w, Row::Open(Menu::Main));
            r.rows(false, &[Row::Workout(0), Row::WorkoutStart]);
            done(&r, r.app.workout_running(), "workout")
        }),
        t("workout in a ride, touch", RIDING, 8, || {
            let mut r = Run::new(true);
            let w = Row::Open(Menu::Workouts);
            r.open_menu(true);
            r.go(true, w, Row::Open(Menu::Main));
            r.rows(true, &[Row::Workout(0), Row::WorkoutStart]);
            done(&r, r.app.workout_running(), "workout")
        }),
        t("stop following, keys", RIDING, 6, || {
            let mut r = Run::new(true);
            r.follow();
            r.path(
                false,
                &[Row::Open(Menu::CourseNow), Row::Ride(RideRow::StopCourse), Row::Ride(RideRow::StopCourse)],
            );
            done(&r, !r.app.page_on(COURSE), "stop")
        }),
        t("brightness in a ride, keys", RIDING, 7, || {
            let mut r = Run::new(true);
            let before = r.app.settings.light_level;
            r.path(false, &[Row::Set(menu::Setting::Light)]);
            done(&r, r.app.settings.light_level != before, "light")
        }),
        t("sensors menu in a ride, keys", RIDING, 6, || {
            let mut r = Run::new(true);
            r.path(false, &[Row::Open(Menu::Sensors)]);
            done(&r, on(&r, Menu::Sensors), "sensors")
        }),
        t("ride menu from a page, touch", RIDING, 2, || {
            let mut r = Run::new(true);
            r.open_menu(true);
            done(&r, matches!(r.app.screen(), Screen::Menu(Menu::Ride(_), _)), "menu")
        }),
        t("ride summary: second card, keys", RIDING, 4, || {
            let mut r = Run::new(true);
            run(&mut r.sim, &mut r.app, 5000); // a ride a few seconds old: Start pauses it
            r.key(Press::Short(Key::Start)); // pause
            r.key(Press::Long(Key::Start));
            r.key(Press::Short(Key::Start)); // SAVE
            r.key(Press::Short(Key::Page));
            done(&r, r.app.screen() == Screen::Summary && r.app.summary_page == 1, "totals")
        }),
        t("ride summary: laps, keys", RIDING, 5, || {
            let mut r = Run::new(true);
            run(&mut r.sim, &mut r.app, 5000); // a ride a few seconds old: Start pauses it
            r.key(Press::Short(Key::Start)); // pause
            r.key(Press::Long(Key::Start));
            r.key(Press::Short(Key::Start)); // SAVE
            run(&mut r.sim, &mut r.app, 5000); // the save is read back and the history has the ride
            r.key(Press::Short(Key::Lap)); // the first starts the history's read when it lacks the ride
            run(&mut r.sim, &mut r.app, 5000);
            if !matches!(r.app.screen(), Screen::Laps(_)) {
                r.key(Press::Short(Key::Lap));
                run(&mut r.sim, &mut r.app, 3000);
            }
            done(&r, matches!(r.app.screen(), Screen::Laps(_)), "laps")
        }),
        t("main menu from a ride, keys", RIDING, 10, || {
            let mut r = Run::new(true);
            r.path(false, &[Row::Open(Menu::Main)]);
            done(&r, on(&r, Menu::Main), "main")
        }),
    ]
}

/// Every task reaches its target within its budget; writes target/sim/taps.txt.
#[test]
fn tasks_stay_within_their_tap_budget() {
    let path = out_dir().join("taps.txt");
    let _ = std::fs::remove_file(&path); // a stale one must not outlive a failing run
    let mut out = std::string::String::from("# task | actions | budget | weight\n");
    let mut failed = Vec::new();
    for t in tasks() {
        let n = (t.run)();
        out += &format!("{} | {n} | {} | {}\n", t.name, t.budget, t.weight);
        if n > t.budget {
            failed.push(format!("{}: {n} > {}", t.name, t.budget));
        }
    }
    assert!(failed.is_empty(), "longer than the budget: {failed:?}");
    std::fs::create_dir_all(out_dir()).unwrap();
    std::fs::write(path, out).unwrap(); // only when every task reached its target within its budget
}

/// The refusals say what to do, in two lines the banner holds (shots: target/sim/refusal_N.bmp).
#[test]
fn refusals_fit_the_banner() {
    let mut r = Run::new(false);
    for (i, text) in [
        "NO COURSE\nSEND ONE FROM APP",
        "NEEDS TRAINER\nPAIR ONE: SENSORS",
        "NOT DURING A RIDE\nTRY AFTER THE RIDE",
        "THE RIDE'S SET\nOPEN THAT ONE",
        "MENU: HOLD PAGE\nOR TAP THE CLOCK",
        "STOP FOLLOWING?\nSTART: YES BACK: NO",
    ]
    .into_iter()
    .enumerate()
    {
        r.app.say(text);
        run(&mut r.sim, &mut r.app, 50);
        r.sim.save(&format!("refusal_{i}"));
    }
}

/// The menu by touch from a page while recording: a tap in the clock's 44 px box opens the ride menu, one outside
/// does not, a locked screen ignores both, and a hold says how to reach the menu. Taps scattered round the box
/// (a structured fuzz: inside, edges, outside, locked or not) never stop the ride, and the box is reached.
#[test]
fn the_clock_opens_the_menu_while_recording_and_a_stray_touch_does_not() {
    let t = ui::MENU_TAP;
    let mut opened = 0;
    for seed in 1..=40u32 {
        let mut r = Run::new(true);
        run(&mut r.sim, &mut r.app, 10_000); // the banners go: a tap that clears one only clears it
        let locked = seed % 4 == 0;
        if locked {
            r.app.press(&mut r.sim, Press::Chord);
        }
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        let mut next = |m: u32| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x % m
        };
        // the edges of the box, one pixel in and one out, and anywhere
        let (px, py) = match seed % 3 {
            0 => (t.0 + next(t.2), next(t.3)),
            1 => (t.0 + t.2 - (next(2)), t.3 - next(3)),
            _ => (next(ui::W), next(120)),
        };
        let inside = px >= t.0 && px < t.0 + t.2 && py < t.1 + t.3;
        r.touch(Gesture::Tap(px as u16, py as u16));
        let menu = matches!(r.app.screen(), Screen::Menu(Menu::Ride(_), _));
        if inside && !locked {
            assert!(menu, "seed {seed}: ({px}, {py}) is in the box: {:?}", r.app.screen());
            opened += 1;
        } else if locked {
            assert!(!menu, "seed {seed}: locked");
        }
        assert_eq!(r.app.ride.state(), ride::State::Running, "seed {seed}: the ride goes on");
    }
    assert!(opened >= 8, "the fuzz reached the box {opened} times");
    // a hold while recording: told, not ignored
    let mut r = Run::new(true);
    r.touch(Gesture::Hold(120, 200));
    assert!(r.app.notes.text.as_str().starts_with("MENU: HOLD PAGE"), "{:?}", r.app.notes.text.as_str());
}

/// The clock's tap is dropped when it only clears a banner or wakes the screen, and the box leaves the fields alone.
#[test]
fn the_clock_tap_only_acts_when_it_is_the_whole_input() {
    let t = ui::MENU_TAP;
    let mid = Gesture::Tap((t.0 + t.2 / 2) as u16, (t.1 + t.3 / 2) as u16);
    assert!(t.3 <= ui::PAGE_Y, "the box is the strip's: no field row under it");
    let is_menu = |r: &Run| matches!(r.app.screen(), Screen::Menu(..));
    // a banner up: the first tap clears it, the second opens the menu
    let mut r = Run::new(true);
    run(&mut r.sim, &mut r.app, 10_000);
    r.app.say("A BANNER");
    r.app.input(&mut r.sim, Input::Touch(mid));
    assert!(!is_menu(&r), "the tap that cleared the banner only cleared it");
    r.app.input(&mut r.sim, Input::Touch(mid));
    assert!(is_menu(&r), "the next one opens the menu");
    // a dimmed screen: the tap that wakes it only wakes it
    let mut r = Run::new(true);
    run(&mut r.sim, &mut r.app, 10_000);
    r.app.light.shown = 0;
    r.app.input(&mut r.sim, Input::Touch(mid));
    assert!(!is_menu(&r), "the waking tap did not open the menu");
    // the first field row keeps its taps: just under the box, nothing opens the menu
    let mut r = Run::new(true);
    run(&mut r.sim, &mut r.app, 10_000);
    r.app.input(&mut r.sim, Input::Touch(Gesture::Tap(t.0 as u16 + 10, ui::PAGE_Y as u16 + 2)));
    assert!(!is_menu(&r), "a tap on the first field row is the field's");
}

/// A "SURE?" does not outlive its banner: a Start after it has gone asks again instead of stopping the course.
#[test]
fn an_armed_confirm_goes_with_its_banner() {
    let mut r = Run::new(true);
    r.follow();
    r.open_menu(false);
    r.rows(false, &[Row::Open(Menu::CourseNow), Row::Ride(RideRow::StopCourse)]);
    assert!(r.app.notes.text.as_str().starts_with("STOP FOLLOWING?"));
    run(&mut r.sim, &mut r.app, 10_000); // the banner goes
    r.app.press(&mut r.sim, Press::Short(Key::Start));
    assert!(r.app.page_on(COURSE), "the course is still followed");
    assert!(r.app.notes.text.as_str().starts_with("STOP FOLLOWING?"), "it asked again");
}
