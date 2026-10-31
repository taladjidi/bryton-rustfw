//! Favourite places on the Sim (places.rs, app/places.rs): SAVE PLACE, then after a restart the place listed with
//! its distance and bearing; a ride's start there calibrates the barometer to its altitude (typed by hand in its
//! menu); nothing written while a ride is open; a power cut after every sector of a write reads the old places or
//! the new. Synthetic places only: the Sim's fix is in the sea near 0 N 0 E ([`FIX_LAT`], [`FIX_LON`]).

extern crate std;
use super::tests::{run, to_row};
use super::*;
use crate::altitude::By;
use crate::fat::tests::{Ram, read_all};
use crate::places::Places;

/// The baseline barometer reads 300 m on the standard day.
fn flat_300(_: u64) -> i32 {
    300_000
}

/// 850 m north and 850 m east of the Sim's fix (about 1.2 km NE), standing.
fn north_east(_: u64) -> (i32, i32, i32) {
    (FIX_LAT + 76_360, FIX_LON + 76_400, 0)
}

/// A card with this firmware's settings, a barometer and a standing fix, at `h` o'clock.
fn unit(h: u64, image: Vec<u8>) -> Sim {
    let mut sim = Sim::new(h, 0, 0);
    sim.emmc.image = image;
    sim.own_settings();
    sim.gnss.fix = Some(0);
    sim.gnss.alt = Some(flat_300);
    sim.bus.baro_alt = Some(flat_300);
    sim
}

/// `\Rustfw\places.bin`'s newest copy, if any.
fn file(sim: &Sim) -> Option<(u32, Places)> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = read_all(&mut fat, &mut d, "Rustfw/places.bin").ok()?;
    Places::newest(&v.try_into().ok()?)
}

/// Passes until the places are written.
fn settle(sim: &mut Sim, app: &mut App) {
    for _ in 0..200 {
        run(sim, app, 200);
        if !app.places.dirty {
            return;
        }
    }
    panic!("the places never written");
}

/// SAVE PLACE from the main menu, then a restart 1.2 km away: PLACES lists it with its distance and which way.
#[test]
fn a_saved_place_is_listed_after_a_restart() {
    let mut sim = unit(9, crate::fat::tests::image());
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::SavePlace);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.notes.text.as_str(), "PLACE 1 SAVED");
    settle(&mut sim, &mut app);
    let (_, saved) = file(&sim).expect("written");
    let p = *saved.get(0).unwrap();
    assert_eq!((p.lat, p.lon, p.num), (FIX_LAT, FIX_LON, 1));
    assert_eq!(p.alt_mm(), Some(300_000), "the receiver's (the barometer was never calibrated)");

    let mut sim = unit(10, core::mem::take(&mut sim.emmc.image));
    sim.gnss.route = Some(north_east);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Place(0));
    let Screen::Menu(m, i) = app.screen() else { panic!() };
    let (label, value, _) = app.menu_row(m, i).unwrap();
    assert_eq!((label.as_str(), value.as_str()), ("PLACE 1", "1.2 KM SW"));
    sim.save("places_menu");
    // its menu: DELETE, asked first, then back in the list, which says none
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Place(0), 0));
    app.press(&mut sim, Press::Short(Key::Page));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.places.list.len(), 1, "asked first");
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.places.list.is_empty());
    assert!(matches!(app.screen(), Screen::Menu(Menu::Places, _)));
    assert_eq!(app.row(Menu::Places, 1), Some(menu::Row::Empty("NONE SAVED")));
    settle(&mut sim, &mut app);
    assert_eq!(file(&sim).map(|f| f.1.len()), Some(0), "the deletion written");
}

/// A place's altitude typed by hand in its menu; at the next ride's start there the barometer is calibrated to it
/// (not to the receiver's average, nor kept on the standard day), marked as a place's; a place saved during the ride
/// is written once the ride is saved, never while it is open.
#[test]
fn a_ride_start_at_a_place_calibrates_to_its_altitude() {
    let mut sim = unit(9, crate::fat::tests::image());
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    app.save_place(&mut sim);
    // its menu: ALTITUDE, typed 250 m (from the 300 m the receiver gave), kept
    app.open(Screen::Menu(Menu::Place(0), 0));
    app.press(&mut sim, Press::Short(Key::Start));
    assert!(app.editing);
    for _ in 0..50 {
        app.press(&mut sim, Press::Short(Key::Page)); // a metre down a step
    }
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.places.list.get(0).unwrap().alt_mm(), Some(250_000));
    assert_eq!(app.notes.text.as_str(), "PLACE ALTITUDE SET");
    settle(&mut sim, &mut app);
    assert_eq!(file(&sim).unwrap().1.get(0).unwrap().alt_mm(), Some(250_000));
    // a place moments ago: the barometer took its altitude when it was typed (the rider is there)
    run(&mut sim, &mut app, 2000);
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Place));

    // hours later, another start there, indoors until the ride starts: the file's calibration is stale, so the start
    // asks for one; the place gives it at the first fix, in place of the receiver's 30 s average
    let mut sim = unit(13, core::mem::take(&mut sim.emmc.image));
    sim.gnss.fix = None;
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    assert_eq!(app.alt.cal().map(|c| c.by), Some(By::Place), "the file's");
    app.start_pause();
    run(&mut sim, &mut app, 1000);
    assert!(app.alt.progress().is_some(), "stale: a calibration asked for");
    sim.gnss.fix = Some(0);
    run(&mut sim, &mut app, 3000);
    let cal = app.alt.cal().expect("calibrated at the start");
    assert_eq!(cal.by, By::Place);
    assert_eq!(app.alt.progress(), None, "no 30 s average from the receiver");
    let alt = app.alt.alt_mm().unwrap();
    assert!(alt.abs_diff(250_000) < 1000, "the place's altitude: {alt}");

    // a place saved during the ride (stopped): kept in memory, written once the ride is saved
    let before = file(&sim);
    app.save_place(&mut sim);
    assert_eq!(app.places.list.len(), 2);
    run(&mut sim, &mut app, 20_000);
    assert_eq!(file(&sim), before, "nothing written while a ride is open");
    app.start_pause();
    app.end_ride();
    run(&mut sim, &mut app, 30_000);
    assert_eq!(file(&sim).map(|f| f.1.len()), Some(2), "written once the ride is saved");
}

/// A power cut after every sector of a write of the places, the first (the file created) and a later one (a copy
/// over the older), on a plain card (the sector being written torn) and one with reliable writes: the next start
/// reads the old places or the new, never neither, and the next write leaves a clean volume for fsck.
#[test]
fn a_cut_at_every_sector_reads_old_or_new() {
    for (writes, reliable) in [(1, false), (2, false), (1, true), (2, true)] {
        let setup = || {
            let mut sim = unit(9, crate::fat::tests::image());
            if reliable {
                sim.emmc.rel_param = crate::emmc::EN_REL_WR;
            }
            sim.emmc.tear = true;
            let mut app = App::start(&mut sim, None);
            run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
            run(&mut sim, &mut app, 30_000); // the settings' first copy written (`\Rustfw\` made)
            for _ in 1..writes {
                app.save_place(&mut sim);
                settle(&mut sim, &mut app);
            }
            let old = app.places.list;
            let base = sim.emmc.sectors_written;
            app.save_place(&mut sim);
            (sim, app, old, base)
        };
        let (mut sim, mut app, _, base) = setup();
        let new = app.places.list;
        settle(&mut sim, &mut app);
        let total = sim.emmc.sectors_written - base;
        assert!(total > 0);
        let (mut olds, mut news) = (0, 0);
        for cut in 0..total {
            let (mut sim, mut app, old, base) = setup();
            sim.emmc.cut_after = Some(base + cut);
            run(&mut sim, &mut app, 5000);
            let mut sim = unit(10, core::mem::take(&mut sim.emmc.image));
            sim.emmc.rel_param = if reliable { crate::emmc::EN_REL_WR } else { 0 };
            let mut app = App::start(&mut sim, None);
            run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
            assert!(app.places.loaded, "cut {cut}: read");
            let got = app.places.list;
            if got == old {
                olds += 1;
            } else {
                assert_eq!(got, new, "writes {writes}, cut {cut}: neither old nor new");
                news += 1;
            }
            // the next write (the FAT copies compared first when the file was not whole) leaves a clean volume
            app.save_place(&mut sim);
            settle(&mut sim, &mut app);
            let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
            assert_eq!(file(&sim).map(|f| f.1), Some(app.places.list), "cut {cut}: written whole after");
        }
        std::println!(
            "writes {writes} (reliable {reliable}): {total} sectors, {olds} cuts read the old, {news} the new"
        );
        assert!(olds > 0);
    }
}

/// Both copies damaged, or one: the other whole one is read; a file of neither reads as no places, and the next
/// save writes a first copy.
#[test]
fn damaged_copies() {
    let mut sim = unit(9, crate::fat::tests::image());
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
    app.save_place(&mut sim);
    settle(&mut sim, &mut app);
    app.save_place(&mut sim);
    settle(&mut sim, &mut app);
    let base = (0..sim.emmc.image.len() / 512).map(|k| 512 * k).find(|&a| sim.emmc.image[a..a + 4] == *b"RPLC");
    let base = base.unwrap();
    let start = |image: Vec<u8>| {
        let mut sim = unit(10, image);
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, u64::from(PAGES_LOAD_MS) + 3000);
        (sim, app)
    };
    // seq 2 is in copy 0 (the newest): damaged, copy 1 (one place) is read
    let mut image = sim.emmc.image.clone();
    image[base + 20] ^= 1;
    assert_eq!(start(image.clone()).1.places.list.len(), 1, "the other copy");
    image[base + 512 + 20] ^= 1;
    let (mut sim, mut app) = start(image);
    assert!(app.places.loaded && app.places.list.is_empty(), "neither: none");
    app.save_place(&mut sim);
    settle(&mut sim, &mut app);
    assert_eq!(file(&sim).map(|f| f.1.len()), Some(1));
}
