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
        // a ride page: swipes up (MORE, then the overview) to where the gear is
        for _ in 0..3 {
            if self.app.screen() != Screen::Main {
                self.touch(Gesture::Swipe(Dir::Up));
            }
        }
        let t = ui::GEAR_TAP;
        self.touch(Gesture::Tap((t.0 + t.2 / 2) as u16, (t.1 + t.3 / 2) as u16));
    }

    /// Each row of the path, skipping one the menu shown does not have (a shorter way).
    fn rows(&mut self, touch: bool, rows: &[Row]) {
        for &r in rows {
            // the course list is read once the menu is open: the hill is found then
            let r =
                if matches!(r, Row::Course(_)) && self.has(Row::Course(0)) { Row::Course(hill(&self.app)) } else { r };
            if self.has(r) {
                if touch { self.tap_to(r) } else { self.keys_to(r) }
            }
        }
    }

    /// Opens the menu, then each row of the path.
    fn path(&mut self, touch: bool, rows: &[Row]) {
        self.open_menu(touch);
        self.rows(touch, rows);
    }

    /// Follows the hill course (not counted), for the tasks that start with one followed.
    fn follow(&mut self) {
        self.path(false, &[Row::Open(Menu::Main), Row::Open(Menu::Courses), Row::Course(0)]);
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
        t("history list, keys", IDLE, 5, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::History)]);
            done(&r, on(&r, Menu::History), "history")
        }),
        t("phone pairing, keys", IDLE, 18, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::Phone), Row::Open(Menu::Sensors), Row::Open(Menu::Phone)]);
            done(&r, on(&r, Menu::Phone), "phone")
        }),
        t("phone pairing, touch", IDLE, 4, || {
            let mut r = Run::new(false);
            r.path(true, &[Row::Open(Menu::Phone), Row::Open(Menu::Sensors), Row::Open(Menu::Phone)]);
            done(&r, on(&r, Menu::Phone), "phone")
        }),
        t("GPS status, keys", IDLE, 16, || {
            let mut r = Run::new(false);
            r.path(false, &[Row::Open(Menu::Settings), Row::Open(Menu::System), Row::Open(Menu::Gps)]);
            done(&r, on(&r, Menu::Gps), "gps")
        }),
        t("course in a ride, keys", RIDING, 6, || {
            let mut r = Run::new(true);
            r.path(false, &[Row::Open(Menu::Courses), Row::Open(Menu::Main), Row::Open(Menu::Courses), Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("course in a ride, touch", RIDING, 7, || {
            let mut r = Run::new(true);
            r.path(true, &[Row::Open(Menu::Courses), Row::Open(Menu::Main), Row::Open(Menu::Courses), Row::Course(0)]);
            done(&r, r.app.page_on(COURSE), "course")
        }),
        t("workout in a ride, keys", RIDING, 12, || {
            let mut r = Run::new(true);
            let w = [Row::Open(Menu::Workouts), Row::Open(Menu::Main), Row::Open(Menu::Workouts)];
            r.path(false, &[w[0], w[1], w[2], Row::Workout(0), Row::WorkoutStart]);
            done(&r, r.app.workout_running(), "workout")
        }),
        t("workout in a ride, touch", RIDING, 12, || {
            let mut r = Run::new(true);
            let w = [Row::Open(Menu::Workouts), Row::Open(Menu::Main), Row::Open(Menu::Workouts)];
            r.path(true, &[w[0], w[1], w[2], Row::Workout(0), Row::WorkoutStart]);
            done(&r, r.app.workout_running(), "workout")
        }),
        t("stop following, keys", RIDING, 5, || {
            let mut r = Run::new(true);
            r.follow();
            r.path(
                false,
                &[Row::Open(Menu::CourseNow), Row::Ride(RideRow::StopCourse), Row::Ride(RideRow::StopCourse)],
            );
            done(&r, !r.app.page_on(COURSE), "stop")
        }),
        t("brightness in a ride, keys", RIDING, 6, || {
            let mut r = Run::new(true);
            let before = r.app.settings.light_level;
            r.path(false, &[Row::Set(menu::Setting::Light)]);
            done(&r, r.app.settings.light_level != before, "light")
        }),
        t("sensors in a ride, keys", RIDING, 7, || {
            let mut r = Run::new(true);
            r.path(false, &[Row::Open(Menu::Sensors)]);
            done(&r, on(&r, Menu::Sensors), "sensors")
        }),
        t("menu from a ride page, touch", RIDING, 3, || {
            let mut r = Run::new(true);
            r.open_menu(true);
            done(&r, matches!(r.app.screen(), Screen::Menu(Menu::Ride(_), _)), "menu")
        }),
        t("main menu from a ride, keys", RIDING, 9, || {
            let mut r = Run::new(true);
            r.path(false, &[Row::Open(Menu::Main)]);
            done(&r, on(&r, Menu::Main), "main")
        }),
    ]
}

/// Every task reaches its target within its budget; writes target/sim/taps.txt.
#[test]
fn tasks_stay_within_their_tap_budget() {
    let mut out = std::string::String::from("# task | actions | budget | weight\n");
    let mut failed = Vec::new();
    for t in tasks() {
        let n = (t.run)();
        out += &format!("{} | {n} | {} | {}\n", t.name, t.budget, t.weight);
        if n > t.budget {
            failed.push(format!("{}: {n} > {}", t.name, t.budget));
        }
    }
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("taps.txt"), out).unwrap();
    assert!(failed.is_empty(), "longer than the budget: {failed:?}");
}
