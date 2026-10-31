//! The three bikes on the Sim (bikes.rs, app/bikes.rs): the migration from the single bike of the settings; a saved
//! ride counted once on the active bike, with a power cut between the save and the write of `bikes.bin` and at every
//! sector of that write; ACTIVE BIKE switching the wheel and weight the ride uses; a part past its interval said at
//! the start, and RESET.

extern crate std;
use super::tests::{run, to_row};
use super::*;
use crate::bikes::{Bikes, Gear, Part};
use crate::fat::tests::{Ram, read_all};

/// The bikes' store, as main.rs gives it (leaked: a test's app lives for the test).
fn memory() -> &'static mut BikeStore {
    std::boxed::Box::leak(std::boxed::Box::new(BikeStore::new()))
}

/// A card with this firmware's settings and a fix, at 9 o'clock, `image` on it, the app given its bikes.
fn boot(image: Vec<u8>) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = image;
    sim.own_settings();
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, None);
    app.set_bike_memory(memory());
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    (sim, app)
}

/// `\Rustfw\bikes.bin`'s newest copy, if any.
fn file(sim: &Sim) -> Option<(u32, Bikes)> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = read_all(&mut fat, &mut d, "Rustfw/bikes.bin").ok()?;
    Bikes::newest(&v.try_into().ok()?)
}

fn bikes(app: &App) -> Bikes {
    *app.bikes().expect("read")
}

/// Passes until the bikes are written.
fn settle(sim: &mut Sim, app: &mut App) {
    for _ in 0..300 {
        run(sim, app, 200);
        if !app.bikes_due() {
            return;
        }
    }
    panic!("the bikes never written");
}

/// Passes until the ride scan has ended and its rides are counted (the store's flag).
fn scanned(sim: &mut Sim, app: &mut App) {
    for _ in 0..600 {
        run(sim, app, 200);
        if app.stock.status() == stock::Status::Done
            && app.bikes.as_deref().is_some_and(|s| s.seen_rev == app.stock.index.rev)
        {
            return;
        }
    }
    panic!("the scan never ended");
}

/// A ride of about a minute, ended and saved; the scan (begun at the start: it runs again after the save) ended
/// first, so the ride is counted at the scan after its save.
fn ride(sim: &mut Sim, app: &mut App) {
    scanned(sim, app);
    app.press(sim, Press::Short(Key::Start));
    run(sim, app, 70_000);
    app.press(sim, Press::Short(Key::Start));
    super::save_ride(sim, app);
    for _ in 0..3000 {
        run(sim, app, u64::from(POLL_MS)); // a pass: back at the first where the save is done
        if app.ride_settled() {
            return;
        }
    }
    panic!("the ride never saved");
}

/// The newest ride the scan found: its distance, m (what the bike gets).
fn last_ride_m(app: &App) -> u32 {
    app.stock.index.get(0).map_or(0, |r| r.dist_m)
}

/// No file: BIKE 1 is the settings' bike and odometer, active, written once; a ride saved goes to it, its parts too,
/// once (a restart's scan adds nothing again); stock's settings and registers untouched by the migration.
#[test]
fn migration_then_a_saved_ride_counts_once() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    let regs = sim.backup;
    let b = bikes(&app);
    assert_eq!((b.active, b.list[0].gear, b.list[0].odo_m), (0, Gear::of(&app.settings), app.settings.odo_m));
    assert_ne!(b.counted.0, 0, "the clock is set: the mark a minute before");
    settle(&mut sim, &mut app);
    assert_eq!(file(&sim).map(|f| f.1), Some(b), "the first copy");
    assert_eq!(sim.backup, regs, "the registers as they were");

    ride(&mut sim, &mut app);
    scanned(&mut sim, &mut app);
    settle(&mut sim, &mut app);
    let m = last_ride_m(&app);
    assert!(m > 300, "a ride: {m} m");
    let b = bikes(&app);
    assert_eq!(b.list[0].odo_m, app.settings.odo_m, "as the settings' ODOMETER");
    assert_eq!((b.list[0].odo_m, b.list[0].parts.map(|p| p.m)), (m, [m; 5]));
    assert_eq!((b.list[1].odo_m, b.list[2].odo_m), (0, 0));
    assert_eq!(file(&sim).map(|f| f.1), Some(b), "written");
    app.press(&mut sim, Press::Short(Key::Back)); // the ride's card closed
    app.press(&mut sim, Press::Short(Key::Back));
    app.open(Screen::Menu(Menu::BikeParts(0), 0));
    app.show(&mut sim);
    run(&mut sim, &mut app, 5000);
    assert_eq!(app.screen(), Screen::Menu(Menu::BikeParts(0), 0));
    sim.save("bikes_parts");

    let (mut sim, mut app) = boot(core::mem::take(&mut sim.emmc.image));
    scanned(&mut sim, &mut app);
    assert_eq!(bikes(&app).list[0].odo_m, m, "not counted again");
}

/// The scan after a short ride's save ends in the pass after it (a small card): the ride is counted at once, not at
/// the next scan (the list's revision, not the scan's end, says a count is due).
#[test]
fn a_rescan_ended_within_a_pass_is_counted() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    settle(&mut sim, &mut app);
    ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 2000);
    let m = last_ride_m(&app);
    assert!(m > 300);
    assert_eq!(bikes(&app).list[0].odo_m, m, "counted at once");
}

/// WHEEL changed during a ride, then a reset: the registers kept the change, `bikes.bin` (not written during the
/// ride) has the old wheel; the resumed ride keeps the new one, and the active bike takes it.
#[test]
fn a_wheel_changed_mid_ride_survives_a_reset() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    settle(&mut sim, &mut app);
    let old = app.settings.wheel_mm;
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    let new = crate::WHEELS.iter().copied().find(|&w| w != old).unwrap();
    app.settings.wheel_mm = new; // BIKE > WHEEL during the ride
    app.save_misc(&mut sim);
    run(&mut sim, &mut app, 2000);
    assert_eq!(file(&sim).map(|f| f.1.list[0].gear.wheel_mm), Some(old), "not written during the ride");

    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim2, None);
    app.set_bike_memory(memory());
    run(&mut sim2, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    assert_ne!(app.ride.state(), ride::State::Idle, "resumed");
    assert_eq!(app.settings.wheel_mm, new, "the new wheel kept");
    assert_eq!(bikes(&app).active().gear.wheel_mm, new, "and given to the active bike");
}

/// The card fails at a boot with a ride to resume: ACTIVE BIKE is refused (nothing loaded or switched while the
/// resume waits); Start then reopens the ride once the card answers (as recorder_tests'
/// `start_while_the_card_failed_at_boot`), with the WHEEL changed during it, and its save counts it.
#[test]
fn active_bike_is_refused_while_a_resume_waits() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    settle(&mut sim, &mut app);
    let old = app.settings.wheel_mm;
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 90_000);
    let new = crate::WHEELS.iter().copied().find(|&w| w != old).unwrap();
    app.settings.wheel_mm = new;
    app.save_misc(&mut sim);
    run(&mut sim, &mut app, 2000);

    let mut sim2 = Sim::new(9, 3, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.gnss.fix = Some(8000);
    sim2.emmc.dead = true;
    let mut app = App::start(&mut sim2, None);
    app.set_bike_memory(memory());
    run(&mut sim2, &mut app, 2000);
    assert!(app.rec.resume_pending, "the resume waits for the card");
    app.open(Screen::Menu(Menu::Bikes, 0));
    app.press(&mut sim2, Press::Short(Key::Start)); // ACTIVE BIKE
    assert_eq!(app.notes.text.as_str(), "END THE RIDE FIRST");
    assert!(app.bikes().is_none(), "nothing loaded");
    app.leave_menus();
    app.press(&mut sim2, Press::Short(Key::Start)); // the ride left open is reopened first
    run(&mut sim2, &mut app, 3000);
    sim2.emmc.dead = false;
    run(&mut sim2, &mut app, u64::from(PAGES_LOAD_MS) + 10_000);
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("RIDE RESUMED")));
    assert_eq!((app.settings.wheel_mm, bikes(&app).active().gear.wheel_mm), (new, new), "the new wheel kept");
    if app.ride.state() == ride::State::Running {
        app.press(&mut sim2, Press::Short(Key::Start)); // paused, to end it
    }
    super::save_ride(&mut sim2, &mut app);
    run(&mut sim2, &mut app, 10_000);
    scanned(&mut sim2, &mut app);
    let m = last_ride_m(&app);
    assert!(m > 300, "{m}");
    assert_eq!(bikes(&app).list[0].odo_m, m, "the resumed ride counted");
}

/// The first start of this build with a ride open (here: the bikes' memory given mid-ride): the mark is set before
/// that ride's start, so its save counts it on BIKE 1 (the settings' ODOMETER, read before it was paid, lacks it).
#[test]
fn a_migration_during_a_ride_counts_that_ride() {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    sim.own_settings();
    sim.gnss.fix = Some(8000);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 35_000); // the settings read, the scan done
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 180_000); // three minutes: "a minute before now" would be past the ride's start
    app.set_bike_memory(memory());
    run(&mut sim, &mut app, 1000);
    let b = bikes(&app);
    assert_eq!((b.counted, b.list[0].odo_m), ((2026, 10, 23, 8, 59), 0), "the mark a minute before the ride");
    app.press(&mut sim, Press::Short(Key::Start));
    super::save_ride(&mut sim, &mut app);
    run(&mut sim, &mut app, 10_000);
    scanned(&mut sim, &mut app);
    let m = last_ride_m(&app);
    assert!(m > 300);
    assert_eq!(bikes(&app).list[0].odo_m, m, "the open ride counted");
}

/// A power cut once the ride is saved, before `bikes.bin` is written: the next start's scan counts it, once; a cut at
/// every sector of that write: the next start reads the old copy (and counts the ride again, once) or the new one,
/// and the volume is clean after the next write.
#[test]
fn a_cut_between_the_save_and_the_write_and_during_it_counts_once() {
    let setup = || {
        let (mut sim, mut app) = boot(crate::fat::tests::image());
        sim.emmc.tear = true;
        settle(&mut sim, &mut app);
        ride(&mut sim, &mut app); // saved; the scan after it not ended: bikes.bin not written yet
        assert_eq!(bikes(&app).list[0].odo_m, 0, "not counted yet");
        let base = sim.emmc.sectors_written;
        (sim, app, base)
    };
    // between: the image as the save left it
    let (mut sim, mut app, base) = setup();
    let restart = |image: Vec<u8>| {
        let (mut sim, mut app) = boot(image);
        scanned(&mut sim, &mut app);
        (bikes(&app).list[0].odo_m, last_ride_m(&app), sim, app)
    };
    let (odo, m, ..) = restart(sim.emmc.image.clone());
    assert!(m > 300);
    assert_eq!(odo, m, "counted at the next start");
    // the whole write, to count its sectors
    scanned(&mut sim, &mut app);
    settle(&mut sim, &mut app);
    let total = sim.emmc.sectors_written - base;
    assert!(total > 0);
    let (mut olds, mut news) = (0, 0);
    for cut in 0..=total {
        let (mut sim, mut app, base) = setup();
        sim.emmc.cut_after = Some(base + cut);
        run(&mut sim, &mut app, 40_000);
        let read = file(&sim).map(|f| f.1.list[0].odo_m);
        if read == Some(0) {
            olds += 1
        } else {
            news += 1
        }
        let (odo, m, mut sim, mut app) = restart(core::mem::take(&mut sim.emmc.image));
        assert_eq!(odo, m, "cut {cut}: counted once ({read:?} read)");
        settle(&mut sim, &mut app);
        let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
        assert_eq!(file(&sim).map(|f| f.1.list[0].odo_m), Some(m), "cut {cut}: written whole after");
    }
    std::println!("{total} sectors: {olds} cuts read the old copy, {news} the new");
    assert!(olds > 0 && news > 0);
}

/// A power cut at every sector of the migration's write (the file created): the next start reads none and migrates
/// again, or reads the first copy; either way BIKE 1 is the settings' bike, and the volume is clean after.
#[test]
fn a_cut_at_every_sector_of_the_first_write() {
    let setup = || {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        sim.emmc.tear = true;
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 30_000); // the settings' first copy (`\Rustfw\` made)
        let base = sim.emmc.sectors_written;
        app.set_bike_memory(memory());
        (sim, app, base)
    };
    let (mut sim, mut app, base) = setup();
    run(&mut sim, &mut app, 1000);
    settle(&mut sim, &mut app);
    let new = file(&sim).expect("written").1;
    let total = sim.emmc.sectors_written - base;
    assert!(total > 0);
    for cut in 0..total {
        let (mut sim, mut app, base) = setup();
        sim.emmc.cut_after = Some(base + cut);
        run(&mut sim, &mut app, 5000);
        let (mut sim, mut app) = boot(core::mem::take(&mut sim.emmc.image));
        assert_eq!(bikes(&app).list, new.list, "cut {cut}");
        settle(&mut sim, &mut app);
        let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
        assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
        assert_eq!(file(&sim).map(|f| f.1.list), Some(new.list), "cut {cut}: written whole after");
    }
    std::println!("first write: {total} sectors");
}

/// BIKE > BIKES > ACTIVE BIKE: the next bike's wheel and weight are the ones used, each bike keeps its own (over a
/// restart too: the file's active bike wins).
#[test]
fn switching_the_active_bike_changes_the_wheel_and_weight() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    settle(&mut sim, &mut app);
    (app.settings.wheel_mm, app.settings.bike_hg) = (crate::WHEELS[3], 75); // BIKE 1 set in BIKE
    run(&mut sim, &mut app, 1000);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Settings));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bike));
    app.press(&mut sim, Press::Short(Key::Start));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Bikes));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Bikes, 0));
    app.press(&mut sim, Press::Short(Key::Start)); // ACTIVE BIKE: BIKE 2
    assert_eq!(app.menu_row(Menu::Bikes, 0).unwrap().1.as_str(), "2");
    assert_eq!((app.settings.wheel_mm, app.settings.bike_hg), (Gear::DEFAULT.wheel_mm, Gear::DEFAULT.bike_hg));
    app.settings.bike_hg = 110; // BIKE 2's weight
    run(&mut sim, &mut app, 5000); // the banner gone
    sim.save("bikes_menu");
    app.press(&mut sim, Press::Short(Key::Start)); // BIKE 3
    app.press(&mut sim, Press::Short(Key::Start)); // BIKE 1
    assert_eq!((app.settings.wheel_mm, app.settings.bike_hg), (crate::WHEELS[3], 75), "BIKE 1's own");
    app.press(&mut sim, Press::Short(Key::Start)); // BIKE 2 again
    assert_eq!(app.settings.bike_hg, 110, "BIKE 2's own");
    settle(&mut sim, &mut app);
    let (_, app) = boot(core::mem::take(&mut sim.emmc.image));
    let b = bikes(&app);
    assert_eq!((b.active, b.list[0].gear.wheel_mm, b.list[1].gear.bike_hg), (1, crate::WHEELS[3], 110));
    assert_eq!(app.settings.bike_hg, 110, "the active bike's weight in use");
}

/// A part past its SERVICE EVERY is said at the start ("CHAIN 500 KM: SERVICE"), red in its bike's menu; RESET
/// (asked first) puts it back to 0.
#[test]
fn a_part_past_its_interval_is_said_and_reset() {
    let (mut sim, mut app) = boot(crate::fat::tests::image());
    settle(&mut sim, &mut app);
    if let Some(st) = app.bikes.as_deref_mut() {
        st.list.list[0].parts[0] = Part { m: 600_000, every_km: 500 };
    }
    app.open(Screen::Menu(Menu::Part(0, 0), 0));
    for _ in 0..crate::bikes::EVERY_KM.len() {
        app.press(&mut sim, Press::Short(Key::Start)); // SERVICE EVERY round to where it was
    }
    assert_eq!(app.menu_row(Menu::Part(0, 0), 0).unwrap().1.as_str(), "500 KM");
    settle(&mut sim, &mut app);
    let (mut sim, mut app) = boot(core::mem::take(&mut sim.emmc.image));
    assert!(app.notes.list.iter().any(|n| n.as_str().ends_with("CHAIN 500 KM: SERVICE")), "said at the start");
    assert_eq!(app.menu_row(Menu::BikeParts(0), 1).unwrap().2, ui::RED);
    app.open(Screen::Menu(Menu::Part(0, 0), 1));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(bikes(&app).list[0].parts[0].m, 600_000, "asked first");
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(bikes(&app).list[0].parts[0], Part { m: 0, every_km: 500 });
    settle(&mut sim, &mut app);
    assert_eq!(file(&sim).map(|f| f.1.list[0].parts[0].m), Some(0));
}
