//! The home trainer on the Sim (trainer.rs, drivers/fec.rs, app/trainer.rs): an FE-C trainer paired from the sensors
//! menu, its power, cadence and speed on the fields and in the ride, ERG under a workout, SIM on a course, and the
//! trainer screen's keys.

use super::*;
use crate::fat::tests::{Ram, read_all};
use crate::menu::{RideRow, Row};
use crate::ui::{
    Field,
    trainer::{Bottom, Button},
};
use std::string::{String, ToString};

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

/// The trainer in `\Rustfw\sensors.bin` on the card: None without a readable file.
pub(super) fn sensors_file(sim: &Sim) -> Option<Option<SensorId>> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = read_all(&mut fat, &mut d, "Rustfw/sensors.bin").ok()?;
    crate::sensors::from_sector(&v.try_into().ok()?)
}

/// The same unit started again on the same card and backup registers (a restart).
fn restart(sim: &mut Sim) -> (Sim, App) {
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.nordic.sensors.push(trainer_sensor());
    sim2.nordic.trainer.rider_w = 180;
    let app = App::start_with(&mut sim2, Some(KEY), Some([8, 7, 6, 5, 4, 3, 2, 1]));
    (sim2, app)
}

/// The control pages (48, 49, 51) the trainer received, in order.
fn commands(sim: &Sim) -> Vec<[u8; 8]> {
    sim.nordic.trainer.got.iter().copied().filter(|p| matches!(p[0], 0x30 | 0x31 | 0x33)).collect()
}

/// The power and distance of each record of the ride saved under `name`.
pub(super) fn records(sim: &Sim, name: [u8; 12]) -> Vec<(Option<u16>, Option<u32>)> {
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
    sim.emmc.image = crate::fat::tests::image();
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
    assert_eq!(sensors_file(&sim), Some(Some(ID)), "paired and kept in the sensors file");
    assert_eq!(sim.backup[14], 0, "and not in the register the shifting systems use");
    let (label, value, _) = app
        .menu_row(Menu::Sensors, super::tests::row_in(&app, Menu::Sensors, Row::Sensor(Kind::Trainer as usize)))
        .unwrap();
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
    assert!(commands(&sim).iter().all(|p| p[0] == 0x30), "only the release until taken over: {:?}", commands(&sim));
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
    let cmds: Vec<_> = commands(&sim).into_iter().filter(|p| p[0] != 0x30).collect(); // not the release at the start
    assert_eq!(cmds.first(), Some(&[0x31, 255, 255, 255, 255, 255, 0xb8, 0x01]), "110 W: 100-120 W, in 0.25 W");
    assert_eq!(sim.nordic.trainer.power, 110);
    assert!(cmds.iter().all(|p| p[0] == 0x31));
    assert_eq!(app.field_text(Field::TrainerTarget).as_str(), "110W");
    // the workout page opens with a trainer heard, first in the round
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT));
    super::tests::assert_consistent(&sim, &app);
    sim.save("workout_page");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::Ride(0), "the next place in the round is the first page");
    // in ERG the cadence box's edge bar goes red under 60 rpm (and only then)
    app.live.set(Metric::Cadence, 55);
    assert_eq!(app.field_bar(Field::Cadence), ui::RED);
    app.live.set(Metric::Cadence, 85);
    assert_eq!(app.field_bar(Field::Cadence), ui::BLACK);
    // under 50 rpm for 5 s in ERG: the warning, once
    sim.nordic.trainer.rpm = 40;
    for s in 0..8 {
        run(&mut sim, &mut app, 1000);
        assert!(s >= 4 || app.notes.text.as_str() != "CADENCE LOW", "second {s}: not before 5 s");
    }
    assert_eq!(app.notes.text.as_str(), "CADENCE LOW");
    sim.nordic.trainer.rpm = 0;
    assert_eq!(app.field_text(Field::ErgTarget).as_str(), "110", "no adjust: the watts alone");
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
    assert_eq!(app.field_text(Field::ErgTarget).as_str(), "215|+5", "what the trainer is told, the adjust small");
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
    app.set_course_memory(crate::storage::course_file::tests::memory());
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
    assert!(commands(&sim).iter().all(|p| p[0] == 0x30), "the release only");
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(sim.nordic.trainer.mode, 0x30, "only the release so far; the mode is sent at the next pass");
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
    let (mx, my) = crate::ui::trainer::bottom_centre(Bottom::Mode, Button::Mode).unwrap();
    app.input(&mut sim, Input::Touch(crate::touch::Gesture::Tap(mx, my)));
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

// ---------------------------------------------------------------------------------------------------------------
// Review of ecbe541
// ---------------------------------------------------------------------------------------------------------------

/// Indoors with a GPS fix (through a window) at 0 km/h: the trainer's speed is the ride's, whatever the phase of the
/// receiver's second against the trainer's pages. The ride at 7.56 m/s for a minute is about 450 m.
#[test]
fn a_gps_fix_at_zero_does_not_replace_the_trainers_speed() {
    for off in (0..1000).step_by(100) {
        let (mut sim, mut app) = unit();
        sim.gnss.fix = Some(0);
        run(&mut sim, &mut app, 3000);
        sim.gnss.next_ms += off;
        run(&mut sim, &mut app, 1500);
        app.press(&mut sim, Press::Short(Key::Start));
        run(&mut sim, &mut app, 60_000);
        let (dist, timer) = (app.ride.dist_m(), app.ride.timer_s());
        assert!(dist > 400 && timer >= 55, "offset {off}: {dist} m in {timer} s");
    }
}

/// The chip takes one acknowledged message per channel period and refuses the next: the set-up pages, the rider's
/// quick changes and the grade changes wait their turn, none is lost, none is refused.
#[test]
fn setup_and_commands_wait_for_the_chip() {
    let (mut sim, mut app) = unit();
    sim.nordic.ack_model = true;
    run(&mut sim, &mut app, 3000);
    let t = &sim.nordic.trainer;
    assert_eq!((t.asked.as_slice(), t.user.is_some()), (&[54][..], true), "both set-up pages came");
    app.open(Screen::Trainer);
    app.show(&mut sim);
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Page)); // +5 W each, faster than a channel period
        run(&mut sim, &mut app, 60);
    }
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.trainer.target_w, 120);
    for _ in 0..2 {
        app.press(&mut sim, Press::Short(Key::Start)); // resistance, then simulation
    }
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Lap)); // -0.5 % each
        run(&mut sim, &mut app, 60);
    }
    run(&mut sim, &mut app, 1000);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.grade_x100), (0x33, -200));
    assert_eq!(sim.nordic.ack_refused, 0, "never an acknowledged message while the last one is queued");
}

/// The trainer not acknowledging a message (`EVENT_TRANSFER_TX_FAILED`) sends it again: the capabilities request
/// first, a command later.
#[test]
fn a_message_the_trainer_did_not_acknowledge_is_sent_again() {
    let (mut sim, mut app) = unit();
    sim.nordic.ack_fail = 1;
    run(&mut sim, &mut app, 3000);
    let t = &sim.nordic.trainer;
    assert_eq!(t.asked, [54], "the first request failed on air, the second arrived");
    assert!(t.user.is_some());
    assert_eq!(app.trainer.reading.max_resistance, Some(1000), "and was answered");
    app.open(Screen::Trainer);
    sim.nordic.ack_fail = 1;
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1500);
    assert_eq!(sim.nordic.trainer.target_w, 105, "the command came the second time");
}

/// A chip that never answers an acknowledged message (the layout of op 0xc8 is inferred): the control is off for the
/// power-up with a notice, one try only, and the link watchdog does not reset the chip over it.
#[test]
fn a_chip_that_never_answers_an_ack_turns_the_control_off() {
    let (mut sim, mut app) = unit();
    sim.nordic.ack_silent = true;
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.rx.timeouts, 0, "the link watchdog does not count it");
    assert!(app.notes.list[0].as_str().contains("TRAINER CONTROL FAILED"), "{:?}", app.notes.list[0].as_str());
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 20_000);
    let acks = sim.nordic.got.iter().filter(|p| p.get(1) == Some(&0xc8)).count();
    assert_eq!(acks, 1, "tried once");
    assert!(
        sim.nordic.resets.len() == 1 && app.radio_resets() == 0,
        "only the start reset the chip: {:?}",
        sim.nordic.resets
    );
    assert_eq!(app.field_text(Field::Power).as_str(), "180", "and the readings go on");
}

/// An earlier build's pairing in register 14 is moved to the file, 14 left alone; a Di2 in 14 and a trainer in the
/// file are both there after a restart.
#[test]
fn the_trainer_is_kept_beside_a_di2_across_restarts() {
    let (mut sim, mut app) = unit(); // the old way: the trainer's word in register 14
    run(&mut sim, &mut app, 6000);
    assert_eq!(sensors_file(&sim), Some(Some(ID)), "moved to the file");
    assert_eq!(sim.backup[14], 0, "14 is free for a shifting system again");
    // a Di2 paired now
    let di2 = SensorId { device: 77, trans: 5 };
    sim.backup[14] = Kind::Di2.to_word(di2);
    let (mut sim2, mut app2) = restart(&mut sim);
    run(&mut sim2, &mut app2, 6000);
    assert_eq!(app2.sensors[Kind::Trainer as usize], Sensor::Paired(ID), "the trainer is back");
    assert_eq!(app2.sensors[Kind::Di2 as usize], Sensor::Paired(di2), "and the Di2");
    assert_eq!(sim2.backup[14], Kind::Di2.to_word(di2));
    // forgetting it is kept too
    app2.set_sensor(&mut sim2, Kind::Trainer, Sensor::Off);
    run(&mut sim2, &mut app2, 500);
    assert_eq!(sensors_file(&sim2), Some(None));
    let (mut sim3, mut app3) = restart(&mut sim2);
    run(&mut sim3, &mut app3, 6000);
    assert_eq!(app3.sensors[Kind::Trainer as usize], Sensor::Off);
    assert_eq!(sim3.backup[14], Kind::Di2.to_word(di2));
}

/// A power cut after every sector of the pairing's write, the first (the file created) and the second (forgetting
/// it): the next start has the old pairing or the new, and the volume is clean.
#[test]
fn a_cut_at_every_sector_of_the_pairing_reads_old_or_new() {
    let started = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        sim.nordic.sensors.push(trainer_sensor());
        let mut app = App::start(&mut sim, Some(KEY));
        run(&mut sim, &mut app, 4000); // the (absent) file read
        (sim, app)
    };
    for second in [false, true] {
        let setup = || {
            let (mut sim, mut app) = started();
            if second {
                app.set_sensor(&mut sim, Kind::Trainer, Sensor::Paired(ID));
                run(&mut sim, &mut app, 500);
                assert_eq!(sensors_file(&sim), Some(Some(ID)));
                app.set_sensor(&mut sim, Kind::Trainer, Sensor::Off);
            } else {
                app.set_sensor(&mut sim, Kind::Trainer, Sensor::Paired(ID));
            }
            let base = sim.emmc.sectors_written;
            (sim, app, base)
        };
        let (mut sim, mut app, base) = setup();
        run(&mut sim, &mut app, 500);
        let total = sim.emmc.sectors_written - base;
        assert!(total > 0);
        let new = if second { Sensor::Off } else { Sensor::Paired(ID) };
        let old = if second { Sensor::Paired(ID) } else { Sensor::Off };
        let (mut olds, mut news) = (0, 0);
        for cut in 0..total {
            let (mut sim, mut app, base) = setup();
            sim.emmc.cut_after = Some(base + cut);
            run(&mut sim, &mut app, 500);
            let (mut sim2, mut app2) = restart(&mut sim);
            run(&mut sim2, &mut app2, 6000);
            let got = app2.sensors[Kind::Trainer as usize];
            assert!(got == old || got == new, "second {second}, cut {cut}: {got:?}");
            // the next write (the FAT copies compared first when the file was not whole) leaves a clean volume
            app2.set_sensor(&mut sim2, Kind::Trainer, Sensor::Paired(SensorId { device: 1, trans: 1 }));
            run(&mut sim2, &mut app2, 500);
            assert!(!app2.sensors_store.dirty, "cut {cut}: written");
            let lost = crate::fat::write::tests::fsck(&sim2.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
            if got == old {
                olds += 1;
            } else {
                news += 1;
            }
        }
        std::println!("second {second}: {total} sectors, {olds} cuts read the old pairing, {news} the new");
        assert!(olds > 0);
    }
}

/// Start and Lap keep their ride meaning on the trainer screen while riding; the mode changes by the MODE button
/// (and not at all under a workout or a course).
#[test]
fn riding_the_keys_on_the_trainer_screen_are_the_rides() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    app.open(Screen::Trainer);
    app.show(&mut sim);
    run(&mut sim, &mut app, 4000);
    assert!(app.ride.lap_s() >= 3);
    app.press(&mut sim, Press::Short(Key::Lap));
    assert_eq!(app.ride.lap_s(), 0, "Lap marked a lap");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Paused, "Start paused the ride");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(app.screen(), Screen::Trainer);
    assert_eq!(app.trainer_field(Field::TrainerMode).as_str(), "ERG", "the mode is where it was");
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600);
    assert_eq!(sim.nordic.trainer.target_w, 105, "Page still steps the target");
    let (mx, my) = crate::ui::trainer::bottom_centre(Bottom::Mode, Button::Mode).unwrap();
    app.input(&mut sim, Input::Touch(crate::touch::Gesture::Tap(mx, my)));
    assert_eq!(app.trainer_field(Field::TrainerMode).as_str(), "RES", "MODE changes it");
}

/// Item 5 of the interaction review: riding on the trainer screen with the keys only, Page was the one key that
/// moved the target (up) and nothing changed the mode (a tap on MODE). Now Lap held (2 s) steps the target down and
/// Start held cycles the mode with a banner; a short Lap is still a lap and a short Start a pause. The keys are
/// held for real (timed), so the repeat flag and the long press are the unit's.
#[test]
fn riding_the_long_keys_on_the_trainer_screen() {
    // Lap held from 6 s for 2.3 s, Start held from 12 s for 2.3 s, Lap tapped at 16 s
    let keys = |ms: u64| match ms {
        6000..8300 => Key::Lap as u8,
        12_000..14_300 => Key::Start as u8,
        16_000..16_150 => Key::Lap as u8,
        _ => 0,
    };
    let (sim, mut app) = unit();
    let mut sim = sim.keys(keys);
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.open(Screen::Trainer);
    app.show(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page)); // ERG, taken over: 105 W (the unit's 100 W + 5)
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600);
    let target = |sim: &Sim| sim.nordic.trainer.target_w;
    assert_eq!(target(&sim), 110);
    let laps = app.ride.lap_s();
    let left = 7000 - sim.ms;
    run(&mut sim, &mut app, left); // Lap down 1 s: a repeat, ignored riding
    assert_eq!(target(&sim), 110, "not yet long");
    run(&mut sim, &mut app, 2500);
    assert_eq!(target(&sim), 100, "Lap held: one big step down");
    assert!(app.ride.lap_s() > laps, "and no lap at its release");
    assert_eq!(app.screen(), Screen::Trainer);
    let left = 12_000 - sim.ms;
    run(&mut sim, &mut app, left);
    run(&mut sim, &mut app, 2600);
    assert_eq!(app.trainer_field(Field::TrainerMode).as_str(), "RES", "Start held: the next mode");
    assert_eq!(app.notes.text.as_str(), "MODE RES", "named in the banner");
    assert_eq!(app.ride.state(), ride::State::Running, "not paused by its release");
    let left = 16_000 - sim.ms;
    run(&mut sim, &mut app, left);
    run(&mut sim, &mut app, 400);
    assert_eq!(app.ride.lap_s(), 0, "a short Lap is a lap");
}

/// A refusal of another command page is not "NOT ACCEPTED" for the command in force.
#[test]
fn not_accepted_is_for_the_command_in_force() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page)); // ERG, taken over
    let status = |app: &App, what: &str| app.with_trainer_now(|n| n.status == what);
    app.trainer.reading.last_cmd = Some((0x30, 3)); // a resistance command refused earlier
    assert!(!status(&app, "NOT ACCEPTED"), "the target power is not the one refused");
    app.trainer.reading.last_cmd = Some((0x31, 3));
    assert!(status(&app, "NOT ACCEPTED"));
    app.trainer.reading.last_cmd = Some((0x31, 0));
    assert!(!status(&app, "NOT ACCEPTED"));
}

/// A course followed before the ride starts sends no grade (the course's first would be put on the trainer while the
/// rider sits); it does once the ride runs.
#[test]
fn no_grade_before_the_ride_starts() {
    let (mut sim, mut app) = unit();
    app.set_course_memory(crate::storage::course_file::tests::memory());
    run(&mut sim, &mut app, 1000);
    app.open_courses(&mut sim);
    for _ in 0..100 {
        app.step(&mut sim);
    }
    app.follow_course(super::tests::hill(&app));
    run(&mut sim, &mut app, 4000);
    assert!(app.courses.as_deref().unwrap().on.is_some());
    assert!(commands(&sim).iter().all(|p| p[0] == 0x30), "only the release sent: {:?}", commands(&sim));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 4000);
    assert!(commands(&sim).iter().any(|p| p[0] == 0x33), "the grade goes out with the ride");
}

/// The rider's weight changed: the trainer is told again.
#[test]
fn a_new_weight_is_sent_to_the_trainer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    let kg = app.settings.weight_kg;
    assert_eq!(sim.nordic.trainer.user.map(|p| [p[1], p[2]]), Some((kg * 100).to_le_bytes()));
    app.settings.weight_kg = kg + 5;
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.trainer.user.map(|p| [p[1], p[2]]), Some(((kg + 5) * 100).to_le_bytes()));
}

/// The trainer screen with the rider's power moving at 4 Hz: the second's snapshot is drawn, in boxes of its own,
/// on the second's tick only (a budget in the style of `perf_budgets`). Measured 2026-10-25: 1604 px/s, 6 windows/s
/// (724, 3 of them the strip's blinking GPS search with a steady power; the power's box, 4 digits, the rest). The
/// first version drew both readings in half-width boxes: 2777, 9.
#[test]
fn the_trainer_screen_stays_within_its_pixel_budget() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.show(&mut sim);
    run(&mut sim, &mut app, 5000);
    let (px, win, t0) = (sim.pixels, sim.windows, sim.ms);
    for k in 0..240u16 {
        sim.nordic.trainer.rider_w = 180 + (k % 7) * 3;
        run(&mut sim, &mut app, 250);
    }
    let s = (sim.ms - t0) / 1000;
    super::tests::assert_consistent(&sim, &app);
    let (px, win) = ((sim.pixels - px) / s, (sim.windows - win) / s);
    std::println!("trainer screen: {px} px/s, {win} windows/s");
    assert!(px <= 2_000 && win <= 12, "{px} px/s, {win} windows/s");
}

// ---- spin-down calibration (FE-C pages 1 and 2) ----

/// The unit asked to calibrate, on the trainer screen with the ERG target taken over (105 W).
fn calibrating_unit() -> (Sim, App) {
    let (mut sim, mut app) = unit();
    sim.nordic.trainer.status = 2; // resistance calibration needed
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.show(&mut sim);
    app.press(&mut sim, Press::Short(Key::Page)); // ERG 105 W, taken over
    run(&mut sim, &mut app, 1500);
    (sim, app)
}

/// The status and source lines of the trainer screen.
fn lines(app: &App) -> (String, String) {
    app.with_trainer_now(|n| (n.status.to_string(), n.source.to_string()))
}

fn tap(sim: &mut Sim, app: &mut App, b: Button) {
    let bottom = app.with_trainer_now(|n| n.bottom);
    let (x, y) = crate::ui::trainer::bottom_centre(bottom, b).expect("the button is there");
    app.input(sim, Input::Touch(crate::touch::Gesture::Tap(x, y)));
}

/// The whole flow by keys: CALIBRATE offered, Start held asks, the screen tells to speed up (with the speed now),
/// then to stop pedalling, then the result; no other control page goes out meanwhile, and the ERG target is back
/// after.
#[test]
fn a_spin_down_by_the_keys() {
    let (mut sim, mut app) = calibrating_unit();
    assert_eq!(lines(&app).0, "CALIBRATE");
    assert_eq!(app.with_trainer_now(|n| n.bottom), Bottom::Both);
    sim.save("trainer_cal_offer");
    let n = sim.nordic.trainer.got.len();
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 2600);
    assert_eq!(sim.nordic.trainer.cal_requests, [0x80], "the spin-down, asked once");
    assert_eq!(lines(&app), ("PEDAL TO 30 KM/H".into(), "SPEED NOW 27 KM/H".into()), "180 W is 7.56 m/s");
    assert_eq!(app.with_trainer_now(|n| n.bottom), Bottom::Cancel);
    sim.save("trainer_cal_speed");
    sim.nordic.trainer.rider_w = 230; // 9.66 m/s, past the target
    run(&mut sim, &mut app, 2000);
    assert_eq!(lines(&app).0, "STOP PEDALLING");
    sim.save("trainer_cal_coast");
    run(&mut sim, &mut app, 9000);
    assert_eq!(lines(&app), ("CALIBRATED".into(), "SPIN-DOWN 2500 MS".into()));
    assert_eq!(app.with_trainer_now(|n| n.bottom), Bottom::Both, "the result stays a while");
    sim.save("trainer_cal_done");
    let pages: Vec<u8> = sim.nordic.trainer.got[n..].iter().map(|p| p[0]).collect();
    let target = pages.iter().position(|&p| p == 0x31).expect("the target comes back");
    assert!(pages[..target].iter().all(|&p| p == 0x01), "nothing but the request until the end: {pages:?}");
    let last = sim.nordic.trainer.got.last();
    assert_eq!(last, Some(&[0x31, 255, 255, 255, 255, 255, 0xa4, 0x01]), "105 W in 0.25 W units");
    assert_eq!(sim.nordic.trainer.status & 3, 0, "the trainer stopped asking");
    run(&mut sim, &mut app, 11_000);
    assert_eq!(app.trainer.cal(), crate::trainer::Cal::Idle);
    assert_eq!((lines(&app).0.as_str(), app.with_trainer_now(|n| n.bottom)), ("OK", Bottom::ModeRamp));
    super::tests::assert_consistent(&sim, &app);
}

/// By touch: CALIBRATE, then CANCEL; the target comes back and MODE is still there beside CALIBRATE.
#[test]
fn a_spin_down_by_touch_and_cancel() {
    let (mut sim, mut app) = calibrating_unit();
    tap(&mut sim, &mut app, Button::Calibrate);
    run(&mut sim, &mut app, 1500);
    assert_eq!(sim.nordic.trainer.cal_requests, [0x80]);
    assert!(app.trainer.calibrating());
    let n = sim.nordic.trainer.got.len();
    tap(&mut sim, &mut app, Button::Calibrate); // now CANCEL
    run(&mut sim, &mut app, 1000);
    assert!(!app.trainer.calibrating());
    assert_eq!(sim.nordic.trainer.got[n..].iter().map(|p| p[0]).collect::<Vec<_>>(), [0x31], "the target again");
    assert_eq!(lines(&app).0, "CALIBRATE", "the trainer still asks");
    tap(&mut sim, &mut app, Button::Mode);
    assert_eq!(app.trainer_field(Field::TrainerMode).as_str(), "RES");
}

/// Back gives up a calibration and leaves.
#[test]
fn back_cancels_a_spin_down() {
    let (mut sim, mut app) = calibrating_unit();
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 1500);
    assert!(app.trainer.calibrating());
    app.press(&mut sim, Press::Short(Key::Back));
    assert_ne!(app.screen(), Screen::Trainer);
    assert!(!app.trainer.calibrating());
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.trainer.got.last().map(|p| p[0]), Some(0x31), "the ERG target is back");
}

/// Not during a ride: a notice, nothing sent.
#[test]
fn no_spin_down_while_riding() {
    let (mut sim, mut app) = calibrating_unit();
    app.start_key(&mut sim);
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 1500);
    app.press(&mut sim, Press::Long(Key::Start)); // riding, Start held is the mode (item 5), not a calibration
    assert!(!app.notes.list[0].as_str().contains("NO CALIBRATION"));
    tap(&mut sim, &mut app, Button::Calibrate);
    assert!(app.notes.list[0].as_str().contains("NO CALIBRATION"), "{}", app.notes.list[0].as_str());
    run(&mut sim, &mut app, 2000);
    assert!(sim.nordic.trainer.cal_requests.is_empty());
    assert!(!app.trainer.calibrating());
}

/// A trainer that never answers: the request is sent again every few seconds, then "NO ANSWER" after a minute, and the
/// target is back.
#[test]
fn a_spin_down_nobody_answers() {
    let (mut sim, mut app) = calibrating_unit();
    sim.nordic.trainer.cal_mute = true;
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_eq!(lines(&app).0, "STARTING...");
    run(&mut sim, &mut app, 55_000);
    assert!(app.trainer.calibrating());
    run(&mut sim, &mut app, 6000);
    assert_eq!(lines(&app).0, "NO ANSWER");
    sim.save("trainer_cal_no_answer");
    let k = sim.nordic.trainer.cal_requests.len();
    assert!((14..=17).contains(&k), "again every 4 s: {k}");
    run(&mut sim, &mut app, 1500);
    assert_eq!(sim.nordic.trainer.got.last().map(|p| p[0]), Some(0x31));
    assert_eq!(sim.nordic.trainer.cal_requests.len(), k, "and no more");
}

/// A failure is shown with a retry, which can succeed.
#[test]
fn a_failed_spin_down_can_be_tried_again() {
    let (mut sim, mut app) = calibrating_unit();
    sim.nordic.trainer.cal_fails = true;
    sim.nordic.trainer.rider_w = 230;
    app.press(&mut sim, Press::Long(Key::Start));
    run(&mut sim, &mut app, 14_000);
    assert_eq!(lines(&app).0, "FAILED: TRY AGAIN");
    assert_eq!(sim.nordic.trainer.status & 3, 2, "still asked for");
    sim.save("trainer_cal_failed");
    sim.nordic.trainer.cal_fails = false;
    tap(&mut sim, &mut app, Button::Calibrate);
    run(&mut sim, &mut app, 14_000);
    assert_eq!(lines(&app).0, "CALIBRATED");
    assert_eq!(sim.nordic.trainer.cal_requests, [0x80, 0x80]);
}

/// The trainer screen has the "<" title bar like a menu (a tap on it goes back), and the ride's timer in its title
/// line while a ride runs, drawn again on each second.
#[test]
fn the_trainer_screen_has_a_title_bar_and_the_ride_timer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 2000);
    app.open(Screen::Trainer);
    app.show(&mut sim);
    assert!(app.with_trainer_now(|n| n.timer.is_empty()), "no ride, no timer");
    app.close();
    app.press(&mut sim, Press::Short(Key::Start)); // Start on the screen itself is the mode while idle
    app.open(Screen::Trainer);
    app.show(&mut sim);
    run(&mut sim, &mut app, 5000);
    let timer = app.with_trainer_now(|n| n.timer.to_string());
    assert!(timer.starts_with("0:0"), "{timer}");
    super::tests::assert_consistent(&sim, &app); // the panel shows what the screen says, the timer included
    sim.save("trainer_screen_timer");
    let (x, y) = (crate::ui::BACK_TAP.0 as u16 + 4, crate::ui::BACK_TAP.1 as u16 + 4);
    app.input(&mut sim, Input::Touch(crate::touch::Gesture::Tap(x, y)));
    assert_ne!(app.screen(), Screen::Trainer, "the \"<\" goes back");
}

/// The main menu stays six rows with a trainer paired (item 6: TRAINER CONTROL left it); the screen is the first row of
/// SENSORS, and in the quick panel, and a tap on a TRAINER MODE / TARGET field opens it: while riding too, but not
/// with the touch lock on.
#[test]
fn the_trainer_screen_is_one_step_from_the_menu_and_the_fields() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 2000);
    app.press(&mut sim, Press::Long(Key::Page));
    assert_eq!(app.screen(), Screen::Menu(Menu::Main, 0));
    let label = |app: &App, i| app.menu_row(Menu::Main, i).map(|(l, ..)| l.as_str().to_string());
    assert_eq!(label(&app, 2).as_deref(), Some("HISTORY"), "no TRAINER CONTROL in MAIN");
    assert_eq!(app.rows(Menu::Main), crate::menu::MAIN.len(), "one screen of six");
    run(&mut sim, &mut app, 1000);
    super::tests::assert_consistent(&sim, &app);
    sim.save("main_menu_trainer");
    for _ in 0..4 {
        app.press(&mut sim, Press::Short(Key::Page)); // SENSORS
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Sensors, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // TRAINER CONTROL, its first row
    assert_eq!(app.screen(), Screen::Trainer);
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Back));
    }
    assert_eq!(app.screen(), Screen::Main);

    // the fields
    app.pages[0] = PageCfg {
        layout: 6,
        fields: [Field::TrainerMode, Field::TrainerTarget, Field::Power, Field::Cadence, Field::Speed, Field::Timer],
    }
    .into();
    app.indoor.pages[0] = app.pages[0]; // the trainer is heard: the ride is indoors, with its own pages
    app.press(&mut sim, Press::Short(Key::Page)); // the first page
    assert_eq!(app.screen(), Screen::Ride(0));
    let at = |app: &App, f: Field| {
        app.widgets()
            .find_map(|((x, y, w, h), wd)| {
                matches!(wd, crate::Widget::Field(g) if g == f).then_some((x + w / 2, y + h / 2))
            })
            .map(|(x, y)| Input::Touch(crate::touch::Gesture::Tap(x as u16, y as u16)))
            .unwrap()
    };
    app.strip.locked = true;
    app.input(&mut sim, at(&app, Field::TrainerTarget));
    assert_eq!(app.screen(), Screen::Ride(0), "the touch lock holds");
    app.strip.locked = false;
    app.input(&mut sim, at(&app, Field::Power));
    assert_eq!(app.screen(), Screen::Ride(0), "another field: nothing");
    app.input(&mut sim, at(&app, Field::TrainerMode));
    assert_eq!(app.screen(), Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Back));
    app.press(&mut sim, Press::Short(Key::Start)); // the ride
    run(&mut sim, &mut app, 1000);
    app.input(&mut sim, at(&app, Field::TrainerTarget));
    assert_eq!(app.screen(), Screen::Trainer, "riding too");
    run(&mut sim, &mut app, 1100);
    super::tests::assert_consistent(&sim, &app);

    // no trainer: no row
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let app = App::start(&mut sim, Some(KEY));
    assert_eq!(app.rows(Menu::Main), crate::menu::MAIN.len());
    assert_eq!(label(&app, 2).as_deref(), Some("HISTORY"));
}

/// A trainer heard, the ride indoors (the profile chosen at Start, or set by hand): the map page (an arrow on black) is
/// out of the round, fix or not; a ride fixed to ROAD keeps it.
#[test]
fn the_map_page_is_skipped_indoors() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.classic_pages(); // pages 1 and 2, and a map on page 3
    sim.backup[14] = Kind::Trainer.to_word(ID);
    sim.nordic.sensors.push(trainer_sensor());
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    app.indoor.pages[2] = app.pages[2]; // a rider who put the map in the indoor set too
    assert!(app.trainer_heard(), "the trainer is heard");
    assert!(!app.pvt.is_some_and(|p| p.good()), "no fix");
    let round = |sim: &mut Sim, app: &mut App| {
        let mut seen = std::vec::Vec::new();
        for _ in 0..6 {
            app.press(sim, Press::Short(Key::Page));
            seen.push(app.screen());
        }
        seen
    };
    assert!(!round(&mut sim, &mut app).contains(&Screen::Ride(2)), "no fix, a trainer: no map page");
    sim.gnss.fix = Some(500);
    run(&mut sim, &mut app, 30_000);
    assert!(app.pvt.is_some_and(|p| p.good()));
    assert!(!round(&mut sim, &mut app).contains(&Screen::Ride(2)), "a fix too: the bike is on a trainer");
    // ROAD fixed in BIKE: the map is back
    app.settings.profile = Some(crate::ride::Profile::Road);
    assert!(round(&mut sim, &mut app).contains(&Screen::Ride(2)), "ROAD: the map page");
    app.settings.profile = None;
    // the ride starts indoors; the rider says ROAD by hand; the trainer going silent does not change that
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.indoors(), "{:?}", app.notes.text.as_str());
    assert_eq!(app.notes.text.as_str(), "INDOOR RIDE");
    app.toggle_indoor();
    assert!(!app.indoors() && round(&mut sim, &mut app).contains(&Screen::Ride(2)));
    app.toggle_indoor();
    assert!(app.indoors());
}

/// An automatic indoor ride whose trainer goes silent for two minutes goes back to ROAD; one fixed to INDOOR stays.
#[test]
fn a_silent_trainer_ends_the_indoor_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.classic_pages();
    sim.backup[14] = Kind::Trainer.to_word(ID);
    sim.nordic.sensors.push(trainer_sensor());
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.indoors());
    sim.nordic.sensors.clear(); // the trainer's power is cut
    run(&mut sim, &mut app, 100_000);
    assert!(app.indoors(), "not yet: the trainer was heard until the sensor's time-out, then under two minutes");
    run(&mut sim, &mut app, 60_000);
    assert!(!app.indoors(), "two minutes of silence: back to ROAD");
    assert_eq!(app.notes.text.as_str(), "TRAINER LOST: ROAD RIDE");
}

/// BIKE > PROFILE: AUTO, ROAD, INDOOR round; each is kept (register 11's tag: AUTO keeps the tag earlier builds read),
/// and a forced INDOOR is the profile at Start without a trainer.
#[test]
fn the_profile_setting_is_kept() {
    let mut s = crate::Settings::from_word(0);
    let mut seen = std::vec::Vec::new();
    for _ in 0..3 {
        let mut l = crate::Line::<12>::new();
        s.text(crate::menu::Setting::BikeProfile, &mut l);
        seen.push((
            std::string::String::from(l.as_str()),
            s.to_word() >> 24,
            crate::Settings::from_word(s.to_word()) == s,
        ));
        s.cycle(crate::menu::Setting::BikeProfile);
    }
    assert_eq!(
        seen,
        [("AUTO".to_string(), 0xa7, true), ("ROAD".to_string(), 0xa8, true), ("INDOOR".to_string(), 0xa9, true)]
    );
    assert_eq!(s.profile, None, "round");

    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 1000);
    app.settings.profile = Some(crate::ride::Profile::Indoor);
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.indoors(), "fixed to INDOOR: indoor with no trainer heard");
    run(&mut sim, &mut app, 130_000);
    assert!(app.indoors(), "a profile fixed by hand stays when no trainer is heard");
}

/// The unit with a trainer heard and a good receiver fix, 5 s in.
fn indoor_unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.classic_pages();
    sim.backup[14] = Kind::Trainer.to_word(ID);
    sim.nordic.sensors.push(trainer_sensor());
    sim.gnss.fix = Some(500);
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 5000);
    (sim, app)
}

/// An indoor ride switches the GNSS receiver off (the strip says off, no search) and the end of the ride (saved)
/// brings it back with a fix, configured and timed again.
#[test]
fn the_receiver_is_off_for_an_indoor_ride_and_back_after_it() {
    let (mut sim, mut app) = indoor_unit();
    assert!(sim.gnss.powered && app.pvt.is_some_and(|p| p.good()), "on and fixed before the ride");
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert!(app.indoors() && !sim.gnss.powered, "off after Start");
    assert_eq!((app.strip.gps, app.pvt.is_none()), (None, true));
    run(&mut sim, &mut app, 30_000);
    assert!(!sim.gnss.powered, "stays off");
    app.press(&mut sim, Press::Short(Key::Start)); // paused
    app.end_ride();
    let frames = sim.gnss.got.iter().filter(|&&b| b == 0xb5).count();
    run(&mut sim, &mut app, 5000);
    assert!(sim.gnss.powered, "on again at the end");
    assert!(sim.gnss.got.iter().filter(|&&b| b == 0xb5).count() >= frames + 2, "configured and timed again");
    assert!(app.pvt.is_some_and(|p| p.good()) && app.strip.gps.is_some_and(|g| g > 0), "a fix again");
}

/// The safety net back to ROAD, the rider's toggle and a discarded ride switch it on again too.
#[test]
fn the_receiver_returns_on_road_and_on_discard() {
    let (mut sim, mut app) = indoor_unit();
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 2000);
    assert!(!sim.gnss.powered);
    sim.nordic.sensors.clear(); // the trainer goes silent: ROAD after the sensor time-out and two minutes
    run(&mut sim, &mut app, 160_000);
    assert!(!app.indoors() && sim.gnss.powered, "ROAD again: the receiver is back");
    app.toggle_indoor(); // by hand
    run(&mut sim, &mut app, 2000);
    assert!(!sim.gnss.powered, "INDOOR by hand: off");
    app.toggle_indoor();
    run(&mut sim, &mut app, 5000);
    assert!(sim.gnss.powered && app.pvt.is_some_and(|p| p.good()), "ROAD by hand: on, with a fix");
    app.toggle_indoor();
    run(&mut sim, &mut app, 2000);
    assert!(!sim.gnss.powered);
    app.press(&mut sim, Press::Short(Key::Start)); // paused
    app.discard_ride();
    run(&mut sim, &mut app, 3000);
    assert!(sim.gnss.powered, "on after DISCARD");
}

/// A road ride, and no ride at all with a trainer heard, never switch the receiver off.
#[test]
fn the_receiver_stays_on_for_a_road_ride() {
    let (mut sim, mut app) = indoor_unit();
    assert!(app.indoors() && sim.gnss.powered, "idle with a trainer heard: still on");
    app.settings.profile = Some(crate::ride::Profile::Road);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 30_000);
    assert!(!app.indoors() && sim.gnss.powered, "a road ride");
}

/// A trainer heard: the ride takes the INDOOR pages (the trainer, the power numbers, the graphs), shown with live
/// values for the README; MORE of each is its default; the road pages come back when the profile is fixed to ROAD.
#[test]
fn the_default_indoor_pages() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    assert!(app.indoors());
    for n in 0..3 {
        assert_eq!(app.screen(), Screen::Ride(n));
        super::tests::assert_consistent(&sim, &app);
        sim.save(&format!("indoor_page{}", n + 1));
        app.press(&mut sim, Press::Short(Key::Page));
    }
    assert_eq!(app.screen(), Screen::Ride(0), "three indoor pages");
    app.input(&mut sim, Input::Touch(crate::touch::Gesture::Swipe(crate::touch::Dir::Up)));
    run(&mut sim, &mut app, 1100);
    sim.save("indoor_page1_more");
    app.toggle_indoor();
    assert!(!app.indoors());
}

/// How a workout ends in [`release_after`].
#[derive(Clone, Copy)]
pub(super) enum Ends {
    /// STOP WORKOUT in the ride menu.
    Stop,
    /// The ride ended and saved.
    EndRide,
}

/// A planned workout in ERG on a trainer already heard (`erg`: it holds the workout's target now), ended `how`:
/// `free` must then say the trainer was released (free resistance: FE-C page 48 or FTMS Set Target Resistance
/// Level of 0), not left at the last ERG target.
pub(super) fn release_after(sim: &mut Sim, app: &mut App, how: Ends, erg: fn(&Sim) -> bool, free: fn(&Sim) -> bool) {
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Workouts));
    app.press(sim, Press::Short(Key::Start));
    app.press(sim, Press::Short(Key::Start)); // SHORT INTERVALS
    app.press(sim, Press::Short(Key::Start)); // START
    run(sim, app, 8000);
    assert!(erg(sim) && !free(sim), "the workout's target is held");
    // +5 W on the step: an adjustment of the workout's target, not a take-over by the rider (review: it kept the
    // trainer at the manual 100 W after the workout)
    app.open(Screen::Trainer);
    app.press(sim, Press::Short(Key::Page));
    run(sim, app, 1000);
    app.press(sim, Press::Short(Key::Back));
    assert!(erg(sim));
    match how {
        Ends::Stop => {
            app.press(sim, Press::Long(Key::Page));
            super::tests::to_row(sim, app, Row::Ride(RideRow::StopWorkout));
            app.press(sim, Press::Short(Key::Start));
            assert!(app.player.is_none(), "stopped");
        }
        Ends::EndRide => {
            app.press(sim, Press::Short(Key::Start)); // pause
            save_ride(sim, app);
        }
    }
    run(sim, app, 3000);
    assert!(free(sim) && !erg(sim), "released, not left at the last target");
    run(sim, app, 10_000);
    assert!(free(sim) && !erg(sim), "and nothing holds it after");
}

/// Starts the planned workout SHORT INTERVALS (ERG) on a trainer already heard; `erg` says the trainer holds it.
pub(super) fn start_workout(sim: &mut Sim, app: &mut App, erg: fn(&Sim) -> bool) {
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Workouts));
    app.press(sim, Press::Short(Key::Start));
    app.press(sim, Press::Short(Key::Start)); // SHORT INTERVALS
    app.press(sim, Press::Short(Key::Start)); // START
    run(sim, app, 8000);
    assert!(erg(sim), "the workout's target is held");
}

/// A planned workout in ERG: the ride paused, the trainer is let go (no target to restart against at 0 rpm);
/// resumed, the step's target goes out again.
pub(super) fn pause_releases(sim: &mut Sim, app: &mut App, erg: fn(&Sim) -> bool, free: fn(&Sim) -> bool) {
    start_workout(sim, app, erg);
    app.press(sim, Press::Short(Key::Start)); // pause
    run(sim, app, 2000);
    assert!(free(sim) && !erg(sim), "paused: let go");
    assert!(app.with_trainer_now(|n| n.status == "PAUSED: FREE"), "the trainer screen says so");
    app.press(sim, Press::Short(Key::Start)); // resume
    run(sim, app, 2000);
    assert!(erg(sim) && !free(sim), "resumed: the target again");
}

/// A workout in ERG, then the switch-off: the trainer is let go before the radio chip goes off.
pub(super) fn off_releases(sim: &mut Sim, app: &mut App, erg: fn(&Sim) -> bool, free: fn(&Sim) -> bool) {
    start_workout(sim, app, erg);
    app.power_off(sim);
    assert!(sim.standby && free(sim) && !erg(sim), "let go at the switch-off");
}

fn ant_erg(sim: &Sim) -> bool {
    sim.nordic.trainer.mode == 0x31
}

fn ant_free(sim: &Sim) -> bool {
    sim.nordic.trainer.mode == 0x30 && sim.nordic.trainer.resistance == 0
}

/// A workout stopped, or its ride ended: the trainer (ANT+ FE-C) is let go (page 48, 0 %), not left at the last
/// target.
#[test]
fn a_stopped_workout_releases_the_trainer() {
    for how in [Ends::Stop, Ends::EndRide] {
        let (mut sim, mut app) = unit();
        run(&mut sim, &mut app, 4000);
        sim.nordic.trainer.mode = 0; // forget the release at the first contact
        sim.nordic.trainer.resistance = 99;
        app.trainer.resend();
        release_after(&mut sim, &mut app, how, ant_erg, ant_free);
    }
}

/// A restart (a fault, a power cut) after a workout: the new start has no workout and the trainer is still at the
/// last target; the first contact lets it go.
#[test]
fn a_restart_releases_the_trainer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 4000);
    app.set_sensor(&mut sim, Kind::Trainer, Sensor::Paired(ID)); // kept in the sensors file
    run(&mut sim, &mut app, 1000);
    assert_eq!(sensors_file(&sim), Some(Some(ID)));
    let (mut sim2, mut app2) = restart(&mut sim);
    sim2.nordic.trainer.mode = 0x31; // the same trainer, still at the target the workout left
    sim2.nordic.trainer.target_w = 210;
    run(&mut sim2, &mut app2, 8000);
    assert!(app2.player.is_none() && ant_free(&sim2), "let go at the first contact: {:#x}", sim2.nordic.trainer.mode);
}

/// ANT+: the ride paused (by hand, or by itself after 3 s without power or cadence) lets the trainer go; resumed, or
/// pedalling again, the step's target goes out again.
#[test]
fn a_paused_ride_releases_the_trainer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 4000);
    pause_releases(&mut sim, &mut app, ant_erg, ant_free);
    sim.nordic.trainer.stopped = true; // the rider stops dead: auto-pause
    run(&mut sim, &mut app, 6000);
    assert!(app.ride.state() == ride::State::Running && !app.ride.moving() && ant_free(&sim), "auto-paused: let go");
    sim.nordic.trainer.stopped = false;
    run(&mut sim, &mut app, 3000);
    assert!(ant_erg(&sim), "pedalling again: the target");
}

/// ANT+: the switch-off lets the trainer go.
#[test]
fn the_switch_off_releases_the_trainer() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 4000);
    off_releases(&mut sim, &mut app, ant_erg, ant_free);
}

/// The chip answered acknowledged messages, then one goes unanswered (a byte lost on the link): it is sent again,
/// and the control stays on (review: one stray byte left the trainer at its last ERG target for the power-up).
#[test]
fn a_lost_answer_after_the_first_does_not_turn_the_control_off() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.trainer.target_w, 105);
    sim.nordic.ack_silent = true;
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 600);
    sim.nordic.ack_silent = false;
    run(&mut sim, &mut app, 2000);
    assert_eq!(sim.nordic.trainer.target_w, 110, "sent again");
    assert!(app.notes.list.iter().all(|n| !n.as_str().contains("TRAINER CONTROL FAILED")));
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.trainer.target_w, 115, "and the control goes on");
}

/// A chip that never answered the first acknowledged message: control off, until the chip is started again (the
/// link watchdog's restart), when it is tried again.
#[test]
fn the_control_is_tried_again_after_a_chip_restart() {
    let (mut sim, mut app) = unit();
    sim.nordic.ack_silent = true;
    run(&mut sim, &mut app, 3000);
    app.open(Screen::Trainer);
    app.press(&mut sim, Press::Short(Key::Page));
    run(&mut sim, &mut app, 3000);
    assert_ne!(sim.nordic.trainer.mode, 0x31, "control off");
    sim.nordic.ack_silent = false;
    app.radio_restart(&mut sim);
    run(&mut sim, &mut app, 8000);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.target_w), (0x31, 105), "tried again on the new chip");
}

/// The release's TX_FAILED never reaches the unit (read while another command waited, or among bytes a resync
/// dropped): the release is sent again until the chip says it went through, so a stopped workout does not leave the
/// trainer at its target.
#[test]
fn a_release_whose_failure_is_lost_is_sent_again() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 4000);
    start_workout(&mut sim, &mut app, ant_erg);
    sim.nordic.ack_lost = 1;
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Ride(RideRow::StopWorkout));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 1000);
    assert_eq!(sim.nordic.ack_lost, 0, "the release went and was lost");
    assert!(ant_erg(&sim));
    run(&mut sim, &mut app, 5000);
    assert!(ant_free(&sim), "sent again");
    let n = commands(&sim).len();
    run(&mut sim, &mut app, 20_000);
    assert_eq!(commands(&sim).len(), n, "and not again once it went through");
}

/// BIKE > PROFILE fixed to INDOOR: the receiver stays off between rides too (AUTO keeps it on), and the start's
/// files are still read and written (the trainer's pairing).
#[test]
fn the_receiver_is_off_at_idle_with_indoor_fixed() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.nordic.sensors.push(trainer_sensor());
    sim.gnss.fix = Some(500);
    let mut app = App::start(&mut sim, Some(KEY));
    app.settings.profile = Some(crate::ride::Profile::Indoor);
    app.set_sensor(&mut sim, Kind::Trainer, Sensor::Paired(ID));
    run(&mut sim, &mut app, 10_000);
    assert!(!sim.gnss.powered && app.pvt.is_none(), "off at idle");
    assert_eq!(sensors_file(&sim), Some(Some(ID)), "the pairing file written: the start's clock ran");
    app.settings.profile = None; // AUTO
    run(&mut sim, &mut app, 5000);
    assert!(sim.gnss.powered && app.pvt.is_some_and(|p| p.good()), "AUTO: on, configured, a fix");
}

/// The rider's own ERG (250 W by hand) during a ride, the trainer silent when the ride is saved: the hold ends with
/// the ride all the same, so the next ride does not put 250 W back on the trainer when it is heard again.
#[test]
fn a_hold_by_hand_ends_with_the_ride_while_the_trainer_is_silent() {
    let (mut sim, mut app) = unit();
    run(&mut sim, &mut app, 3000);
    app.press(&mut sim, Press::Short(Key::Start));
    app.open(Screen::Trainer);
    for _ in 0..30 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    run(&mut sim, &mut app, 1000);
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.target_w), (0x31, 250));
    app.press(&mut sim, Press::Short(Key::Back));
    let sensors = core::mem::take(&mut sim.nordic.sensors);
    run(&mut sim, &mut app, 8000);
    assert!(!app.trainer_heard());
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    app.start_pause(); // the next ride
    assert_eq!(app.ride.state(), ride::State::Running);
    run(&mut sim, &mut app, 1000);
    sim.nordic.sensors = sensors;
    run(&mut sim, &mut app, 8000);
    assert!(app.trainer_heard());
    assert!(ant_free(&sim), "not 250 W again: {:#x} {} W", sim.nordic.trainer.mode, sim.nordic.trainer.target_w);
}

/// The boot's release (the first contact) is tried once: a trainer another controller drives (an app) is not forced
/// free again and again when nothing confirms it.
#[test]
fn the_boot_release_is_tried_once() {
    let (mut sim, mut app) = unit();
    sim.nordic.ack_lost = 1000; // nothing confirmed, nothing taken
    run(&mut sim, &mut app, 30_000);
    let releases = sim.nordic.got.iter().filter(|p| p.get(1) == Some(&0xc8) && p.get(4) == Some(&0x30)).count();
    assert_eq!(releases, 1);
}
