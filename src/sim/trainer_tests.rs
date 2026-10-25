//! The home trainer on the Sim (trainer.rs, drivers/fec.rs, app/trainer.rs): an FE-C trainer paired from the sensors
//! menu, its power, cadence and speed on the fields and in the ride, ERG under a workout, SIM on a course, and the
//! trainer screen's keys.

use super::*;
use crate::menu::{RideRow, Row};
use crate::ui::Field;

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const ID: SensorId = SensorId { device: 5555, trans: 0xf1 };

fn trainer_sensor() -> AntSensor {
    AntSensor { device_type: 0x11, id: 5555, data: |_| [0; 8], network: 0, ack: &[] }
}

fn power210(_: u64) -> [u8; 8] {
    [0x10, 1, 0xff, 85, 0, 0, 210, 0]
}

/// The unit with an FE-C trainer in range, already paired (register 14), the rider making 180 W.
fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[14] = Kind::Trainer.to_word(ID);
    sim.nordic.sensors.push(trainer_sensor());
    sim.nordic.trainer.rider_w = 180;
    let app = App::start(&mut sim, Some(KEY));
    (sim, app)
}

/// The control pages (48, 49, 51) the trainer received, in order.
fn commands(sim: &Sim) -> Vec<[u8; 8]> {
    sim.nordic.trainer.got.iter().copied().filter(|p| matches!(p[0], 0x30 | 0x31 | 0x33)).collect()
}

/// The power and distance of each record of the ride saved under `name`.
fn records(sim: &Sim, name: [u8; 12]) -> Vec<(Option<u16>, Option<u32>)> {
    let f = super::recorder_tests::ride_file(sim, "Activities", name).expect("the ride is saved");
    let (mut v, mut rd) = (Vec::new(), crate::fitread::Reader::new());
    rd.push_slice(&f, |m| {
        if let crate::fitread::Msg::Record(r) = m {
            v.push((r.power, r.dist_cm));
        }
    });
    assert_eq!(rd.result(), Some(Ok(())), "a whole FIT file");
    v
}

/// Pairing from SENSORS (Start on the TRAINER row finds it, keeps it in register 14), the pages decoded onto the
/// fields, the set-up sent once (capabilities asked, the rider and the bike told), and nothing commanded: a
/// trainer just paired keeps its resistance. A power meter heard wins over the trainer's power.
#[test]
fn pairs_reads_and_waits_to_be_taken_over() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    sim.nordic.sensors.push(trainer_sensor());
    sim.nordic.trainer.rider_w = 180;
    let mut app = App::start(&mut sim, Some(KEY));
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Open(Menu::Sensors));
    app.press(&mut sim, Press::Short(Key::Start));
    for _ in 0..Kind::Trainer as usize {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(sim.nordic.got.contains(&vec![6, 0xd3, 7, 0, 0, 0x11, 0]), "any trainer, device type 17");
    assert!(sim.nordic.got.contains(&vec![6, 0xd1, 7, 0, 0x20]), "8192 / 32768 s");
    run(&mut sim, &mut app, 4000);
    assert_eq!(sim.backup[14], Kind::Trainer.to_word(ID), "paired and kept");
    let (label, value, _) = app.menu_row(Menu::Sensors, Kind::Trainer as usize).unwrap();
    assert_eq!((label.as_str(), value.as_str()), ("TRAINER", "5555"));
    let live = |app: &App, m| app.live.get(m);
    assert_eq!(
        [Metric::Power, Metric::Cadence, Metric::Speed].map(|m| live(&app, m)),
        [Some(180), Some(90), Some(272)],
        "180 W, 90 rpm, 27.2 km/h (180 W x 42 mm/s)"
    );
    let r = app.trainer.reading;
    assert_eq!((r.max_resistance, r.maker, r.modes), (Some(1000), Some(15), Some(7)), "capabilities, page 80");
    let t = &sim.nordic.trainer;
    assert_eq!(t.asked, [54], "capabilities asked once");
    assert_eq!(t.user.map(|p| p[1..3].to_vec()), Some((app.settings.weight_kg * 100).to_le_bytes().to_vec()));
    assert_eq!(t.user.map(|p| p[6]), Some(66), "a 2105 mm wheel is 67 cm across");
    assert!(commands(&sim).is_empty(), "no command until taken over: {:?}", commands(&sim));
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "ERG");
    // a power meter heard: its power and cadence, the trainer's the rest
    sim.nordic.sensors.push(AntSensor { device_type: 0x0b, id: 7, data: power210, network: 0, ack: &[] });
    app.set_sensor(&mut sim, Kind::Power, Sensor::Search);
    run(&mut sim, &mut app, 4000);
    assert_eq!(live(&app, Metric::Power), Some(210), "the power meter wins");
    assert_eq!(live(&app, Metric::Speed), Some(272), "the trainer's speed, no speed sensor");
}

/// A workout in ERG: each step's power target (the midpoint of its range) is sent as page 49 when the step starts
/// and again later; the trainer holds it; the ride records the trainer's power, and its speed gives the distance
/// (no GPS).
#[test]
fn erg_follows_the_workout_and_the_ride_records_the_trainer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Open(Menu::Workouts));
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Short(Key::Start)); // SHORT INTERVALS
    let name = crate::recorder::ride_name(app.rec_time().0);
    app.press(&mut sim, Press::Short(Key::Start)); // START
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 8000);
    let cmds = commands(&sim);
    assert_eq!(cmds.first(), Some(&[0x31, 255, 255, 255, 255, 255, 0xb8, 0x01]), "110 W: 100-120 W, in 0.25 W");
    assert_eq!(sim.nordic.trainer.power, 110);
    assert!(cmds.iter().all(|p| p[0] == 0x31));
    assert_eq!(app.field_text(Field::TrainerTarget).as_str(), "110W");
    assert_eq!(app.field_text(Field::Power).as_str(), "110");
    // the first step lasts 20 s; then 200-220 W
    run(&mut sim, &mut app, 14_000);
    assert_eq!(sim.nordic.trainer.target_w, 210, "the second step: {:?}", commands(&sim).last());
    assert_eq!(app.trainer.mode(&app.trainer_drive()).name(), "ERG");
    let before = commands(&sim).len();
    run(&mut sim, &mut app, 9000);
    assert!(commands(&sim).len() > before, "the target is sent again while it stands");
    // a hand adjustment on the step (Page on the trainer screen: +5 W)
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 500);
    assert_eq!(sim.nordic.trainer.target_w, 215);
    app.press(&mut sim, Press::Short(Key::Back));
    // end and save
    run(&mut sim, &mut app, 5000);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let rec = records(&sim, name);
    let powers: Vec<u16> = rec.iter().filter_map(|r| r.0).collect();
    assert!(powers.len() > 30, "{} records with power", powers.len());
    assert!(powers.contains(&110) && powers.contains(&210) && powers.contains(&215), "the target held: {powers:?}");
    let dist = rec.iter().filter_map(|r| r.1).max().unwrap_or(0);
    assert!(dist > 12_000, "16 km/h or more for 30 s without GPS: {dist} cm");
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
}

/// A course followed on the trainer: SIM, the grade of the course at the distance the trainer's speed has ridden
/// (page 51), the trainer's speed following the grade.
#[test]
fn sim_follows_the_course() {
    let (mut sim, mut app) = unit();
    app.set_course_memory(crate::storage::course::tests::memory());
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Open(Menu::Courses));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    for _ in 0..super::tests::hill(&app) {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 200_000); // 10 m/s on the flat: the hill starts about 1.7 km in
    assert_eq!(app.screen(), Screen::Ride(COURSE));
    let cmds = commands(&sim);
    assert!(cmds.iter().any(|p| p[0] == 0x33), "track resistance sent: {cmds:?}");
    let grade = |p: &[u8; 8]| i32::from(u16::from_le_bytes([p[5], p[6]])) - 20_000;
    let g = grade(cmds.last().unwrap());
    assert_eq!(cmds.last().unwrap()[7], 80, "tarmac");
    assert!(g > 0, "on the hill: {g}");
    assert!(sim.nordic.trainer.speed_mm_s < 10_000, "slower uphill");
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "SIM");
    assert_eq!(sim.nordic.trainer.mode, 0x33);
    assert_eq!(sim.nordic.trainer.grade_x100, g, "the trainer applies it");
    sim.save("trainer_sim_course");
}

/// The trainer screen from SENSORS: Page and Lap change the target (5 W, then 5 % of resistance, then 0.5 % of grade
/// with Start going round the modes), the touch buttons do the same, Back leaves.
#[test]
fn the_screen_changes_the_target_and_the_mode() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Ride(RideRow::Trainer));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Trainer);
    run(&mut sim, &mut app, 1100);
    sim.save("trainer_screen_idle");
    assert!(commands(&sim).is_empty());
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(sim.nordic.trainer.mode, 0, "sent at the next pass");
    run(&mut sim, &mut app, 1100);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.target_w), (0x31, 105));
    sim.save("trainer_screen_erg");
    super::tests::assert_consistent(&sim, &app);
    app.press(&mut sim, Press::Long(Key::Lap)); // not a held repeat: Media is not on this screen
    app.press(&mut sim, Press::Repeat(Key::Lap));
    run(&mut sim, &mut app, 600);
    assert_eq!(sim.nordic.trainer.target_w, 95, "a held Lap steps 10 W");
    app.press(&mut sim, Press::Short(Key::Start)); // resistance
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.resistance), (0x30, 50), "25 %");
    sim.save("trainer_screen_res");
    app.press(&mut sim, Press::Short(Key::Start)); // simulation
    app.press(&mut sim, Press::Short(Key::Lap));
    run(&mut sim, &mut app, 600);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.grade_x100), (0x33, -50));
    sim.save("trainer_screen_sim");
    super::tests::assert_consistent(&sim, &app);
    // the touch buttons: "+" (twice), then "MODE"
    let (_, bx, by, bw, bh) = crate::ui::trainer::BUTTONS[1];
    for _ in 0..2 {
        app.input(&mut sim, Input::Touch(crate::touch::Gesture::Tap((bx + bw / 2) as u16, (by + bh / 2) as u16)));
    }
    run(&mut sim, &mut app, 600);
    assert_eq!(sim.nordic.trainer.grade_x100, 50);
    let (_, mx, my, mw, mh) = crate::ui::trainer::BUTTONS[2];
    app.input(&mut sim, Input::Touch(crate::touch::Gesture::Tap((mx + mw / 2) as u16, (my + mh / 2) as u16)));
    assert_eq!(app.trainer_field(Field::TrainerMode).as_str(), "ERG");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_ne!(app.screen(), Screen::Trainer);
    // a trainer that falls silent: no more commands, and the screen says so
    run(&mut sim, &mut app, 1000);
    sim.nordic.sensors.clear();
    run(&mut sim, &mut app, 6000); // heard for 5 s more, its last command sent again at most once
    let n = sim.nordic.trainer.got.len();
    run(&mut sim, &mut app, 20_000);
    assert_eq!(sim.nordic.trainer.got.len(), n);
    assert_eq!(app.field_text(Field::TrainerMode).as_str(), "--");
}

/// The trainer fields on a page, with a trainer, and the screenshot of the page.
#[test]
fn trainer_fields_on_a_page() {
    let (mut sim, mut app) = unit();
    app.pages[0] = PageCfg {
        layout: 6,
        fields: [Field::TrainerMode, Field::TrainerTarget, Field::Power, Field::Cadence, Field::Speed, Field::Timer],
    }
    .into();
    app.show(&mut sim);
    app.trainer.step_by(&Default::default(), true, false);
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Page)); // the first page
    run(&mut sim, &mut app, 1100);
    sim.save("trainer_fields");
    super::tests::assert_consistent(&sim, &app);
    assert_eq!(app.field_text(Field::TrainerTarget).as_str(), "105W");
}
