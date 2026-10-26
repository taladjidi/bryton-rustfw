//! The FTP ramp test on the Sim (ramp.rs, app/ramp.rs): started from WORKOUTS or the trainer screen, played in ERG
//! on an FE-C trainer, ended by a stop rule or Lap, the result screen and APPLY.

use super::*;
use crate::menu::Row;
use crate::ui::trainer::{Bottom, Button};

const KEY: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
const ID: SensorId = SensorId { device: 5555, trans: 0xf1 };

fn unit() -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.backup[14] = Kind::Trainer.to_word(ID);
    sim.nordic.sensors.push(AntSensor { device_type: 0x11, id: 5555, data: |_| [0; 8], network: 0, ack: &[] });
    sim.nordic.trainer.rider_w = 180;
    let mut app = App::start(&mut sim, Some(KEY));
    run(&mut sim, &mut app, 4000);
    (sim, app)
}

/// WORKOUTS > RAMP TEST.
fn start(sim: &mut Sim, app: &mut App) {
    app.press(sim, Press::Long(Key::Page));
    super::tests::to_row(sim, app, Row::Open(Menu::Workouts));
    app.press(sim, Press::Short(Key::Start)); // the calendar is read as the list opens
    super::tests::to_row(sim, app, Row::RampTest);
    app.press(sim, Press::Short(Key::Start));
}

/// Through the warm-up and `minutes` of the ramp.
fn ride(sim: &mut Sim, app: &mut App, minutes: u64) {
    run(sim, app, (ramp::WARMUP_S as u64 + 60 * minutes) * 1000);
}

/// Without a trainer heard the test is refused and nothing starts.
#[test]
fn needs_a_trainer() {
    let mut sim = Sim::new(9, 0, 0);
    sim.own_settings();
    let mut app = App::start(&mut sim, None);
    start(&mut sim, &mut app);
    assert_eq!(app.notes.text.as_str(), "NEEDS TRAINER");
    assert_eq!((app.ride.state(), app.plan.player, app.ramp.is_none()), (ride::State::Idle, None, true));
}

/// A ramp ended by the power fading: ERG climbs the steps, the best minute is just before the fade, the result
/// screen asks, Back keeps the FTP and START (on a second run) applies it; the cool-down plays and the ride saves.
#[test]
fn fading_power_ends_the_test_and_start_applies_the_ftp() {
    let (mut sim, mut app) = unit();
    let ftp = app.settings.ftp;
    start(&mut sim, &mut app);
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT), "the workout page, a trainer heard");
    let name = crate::recorder::ride_name(app.rec_time().0);
    run(&mut sim, &mut app, 5000);
    let watts = |k: usize| u16::try_from(u32::from(ftp) * (50 + 6 * k as u32) / 100).unwrap();
    assert!(sim.nordic.trainer.target_w.abs_diff(watts(0)) <= 1, "warm-up at 50 %: {}", sim.nordic.trainer.target_w);
    // through the warm-up and 5 steps of the ramp (the 6th, 56 + 6 x 4 = 80 % of FTP, has run 30 s)
    ride(&mut sim, &mut app, 4);
    run(&mut sim, &mut app, 25_000);
    super::tests::assert_consistent(&sim, &app);
    sim.save("ramp_workout_page");
    let target = sim.nordic.trainer.target_w;
    assert!(target.abs_diff(watts(4)) <= 1 || target.abs_diff(watts(5)) <= 1, "ramp step 5 or 6: {target} W");
    assert!(app.ramp.is_some() && app.ramp_result.is_none() && app.screen() == Screen::Ride(crate::WORKOUT));
    // the rider cannot hold more than 100 W: under half the target for 10 s
    sim.nordic.trainer.cap_w = 40;
    for _ in 0..8 {
        run(&mut sim, &mut app, 1000);
    }
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT), "not yet");
    run(&mut sim, &mut app, 6000);
    assert_eq!(app.screen(), Screen::RampResult);
    let o = app.ramp_result.expect("a result");
    assert!(o.best_w > 0 && o.best_w <= target, "{o:?} within the ramp's targets");
    assert!(o.ftp.abs_diff(ramp::ftp_of(u32::from(o.best_w) * 60)) <= 1, "75 % of the best minute: {o:?}");
    assert!(o.ftp.abs_diff(o.best_w * 3 / 4) <= 1);
    assert_eq!(o.aet, None, "no strap");
    assert!(o.secs > 4 * 60 && o.secs < 6 * 60, "{}", o.secs);
    sim.save("ramp_result");
    // the cool-down is playing already
    assert_eq!(app.plan.player.and_then(|p| p.step(app.plan.steps())), Some(ramp::COOL));
    assert!(app.ramp.is_none());
    // Back keeps the FTP set
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT));
    assert_eq!(app.settings.ftp, ftp);
    // the cool-down at 45 % of the FTP in force, then the ride is ended the ordinary way
    sim.nordic.trainer.cap_w = 0;
    run(&mut sim, &mut app, 3000);
    assert!(sim.nordic.trainer.target_w.abs_diff((u32::from(ftp) * 45 / 100) as u16) <= 1);
    app.press(&mut sim, Press::Short(Key::Start)); // pause
    save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    assert!(super::recorder_tests::ride_file(&sim, "Activities", name).is_some(), "the test's ride is saved");
}

/// Lap in the ramp ends it; START on the result applies the FTP found.
#[test]
fn lap_ends_the_ramp_and_apply_sets_the_ftp() {
    let (mut sim, mut app) = unit();
    start(&mut sim, &mut app);
    ride(&mut sim, &mut app, 3);
    let ftp = app.settings.ftp;
    app.press(&mut sim, Press::Short(Key::Lap));
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::RampResult);
    let o = app.ramp_result.unwrap();
    assert_ne!(o.ftp / 5 * 5, ftp, "a different FTP to apply");
    // other keys do nothing
    app.press(&mut sim, Press::Short(Key::Page));
    assert_eq!(app.screen(), Screen::RampResult);
    // Start keeps its ride meaning (it pauses), whatever the screen; APPLY is a touch button
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.ride.state(), ride::State::Paused);
    assert_eq!(app.settings.ftp, ftp, "Start did not apply");
    assert_eq!(app.screen(), Screen::RampResult);
    app.press(&mut sim, Press::Short(Key::Start));
    let (x, y) = (60, 290);
    app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT));
    assert_eq!(app.settings.ftp, (o.ftp + 2) / 5 * 5, "to the setting's 5 W step");
    assert_eq!(app.notes.text.as_str(), "FTP SET");
    // kept through a restart (the settings are saved at once)
    assert_eq!(crate::Settings::from_word(sim.backup[crate::WHEEL_BKP]).ftp, app.settings.ftp);
}

/// Cadence under 60 rpm for 5 s, and a stop before a whole minute gives no result.
#[test]
fn cadence_ends_it_and_a_short_ramp_has_no_result() {
    let (mut sim, mut app) = unit();
    start(&mut sim, &mut app);
    ride(&mut sim, &mut app, 0);
    run(&mut sim, &mut app, 12_000); // past the ramp's first 10 s, which count for nothing
    sim.nordic.trainer.rpm = 40;
    run(&mut sim, &mut app, 4000);
    assert!(app.ramp.is_some(), "4 s is not enough");
    run(&mut sim, &mut app, 4000);
    assert!(app.ramp.is_none() && app.ramp_result.is_none());
    assert_eq!(app.plan.player.and_then(|p| p.step(app.plan.steps())), Some(ramp::COOL), "cooling down");
    assert_eq!(app.screen(), Screen::Ride(crate::WORKOUT));
}

/// From the trainer screen: the RAMP TEST button before a ride, none while riding.
#[test]
fn the_trainer_screen_offers_it() {
    let (mut sim, mut app) = unit();
    app.open(Screen::Trainer);
    app.show(&mut sim);
    assert_eq!(app.trainer_bottom(), Bottom::ModeRamp);
    let (x, y) = ui::trainer::bottom_centre(Bottom::ModeRamp, Button::Ramp).expect("the button");
    app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
    assert!(app.ramp.is_some() && app.ride.state() == ride::State::Running);
    app.open(Screen::Trainer);
    assert_eq!(app.trainer_bottom(), Bottom::Mode, "riding: the ride's");
}

/// The rider stops dead mid-ramp: the ride pauses itself after 3 s without power or cadence, which would freeze the
/// stop rules while ERG held the step's target. The ramp ends at once; the trainer is let go while the ride is
/// paused (no target to restart against at 0 rpm), and the cool-down target goes out once the rider pedals again.
#[test]
fn a_rider_who_stops_dead_gets_the_cool_down_target() {
    let (mut sim, mut app) = unit();
    let ftp = app.settings.ftp;
    start(&mut sim, &mut app);
    ride(&mut sim, &mut app, 3);
    assert!(sim.nordic.trainer.target_w > (u32::from(ftp) * 70 / 100) as u16, "a hard step");
    sim.nordic.trainer.stopped = true;
    run(&mut sim, &mut app, 6000);
    assert!(app.ramp.is_none() && app.ramp_result.is_some(), "the test ended");
    assert_eq!(app.plan.player.and_then(|p| p.step(app.plan.steps())), Some(ramp::COOL));
    assert_eq!((sim.nordic.trainer.mode, sim.nordic.trainer.resistance), (0x30, 0), "let go while paused");
    sim.nordic.trainer.stopped = false;
    run(&mut sim, &mut app, 3000);
    assert_eq!(sim.nordic.trainer.mode, 0x31);
    assert!(sim.nordic.trainer.target_w.abs_diff((u32::from(ftp) * 45 / 100) as u16) <= 1);
    assert_eq!(app.screen(), Screen::RampResult);
}

/// STOP WORKOUT in the ramp ends the test through the same path (cool-down, result), not a bare stop.
#[test]
fn stop_workout_in_the_ramp_plays_the_cool_down() {
    let (mut sim, mut app) = unit();
    let ftp = app.settings.ftp;
    start(&mut sim, &mut app);
    ride(&mut sim, &mut app, 3);
    app.press(&mut sim, Press::Long(Key::Page));
    super::tests::to_row(&mut sim, &mut app, Row::Ride(crate::menu::RideRow::StopWorkout));
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.plan.player.and_then(|p| p.step(app.plan.steps())), Some(ramp::COOL));
    assert!(sim.nordic.trainer.target_w.abs_diff((u32::from(ftp) * 45 / 100) as u16) <= 1);
    assert_eq!(app.screen(), Screen::RampResult);
}
