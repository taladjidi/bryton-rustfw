//! STORAGE and ABOUT on the Sim: the card's figures read by the survey over its passes, the oldest ride opened on its
//! HISTORY card and left by Back, and ABOUT's notices.

use super::menu_tests::{row_of, to_row};
use super::tests::run;
use super::*;
use crate::fat::Fat;
use crate::ui::menu::Row;
use crate::volume::{OLDEST, size_text};

/// STORAGE on the Sim's card: the free space is the FAT's own count, the rides are the ones in \Activities, the
/// oldest first; a tap-or-Select on the oldest opens its card (DELETE is there), and Back returns to STORAGE.
#[test]
fn storage_lists_the_card_and_opens_its_oldest_ride() {
    let (mut sim, mut app) = started();
    sim.emmc.image = crate::storage::trash::tests::volume();
    run(&mut sim, &mut app, 3000); // the app's own card work (the scan's cache) ends before the figures are compared
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, Row::Open(Menu::Storage));
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::Menu(Menu::Storage, 0));
    run(&mut sim, &mut app, 3000);
    assert!(app.survey.done(), "the card is read over the passes, on the Sim within 3 s");

    let mut d = crate::fat::tests::Ram(sim.emmc.image.clone(), 0);
    let mut fat = Fat::mount(&mut d).unwrap();
    let (mut next, mut free) = (2, 0);
    assert!(fat.count_free(&mut d, &mut next, &mut free, u32::MAX).unwrap());
    // the free space is FSInfo's count when it is whole (the Sim's card: FSInfo is about 16 KB off the FAT's own count,
    // as a hint is when a writer left it stale; the FAT is what the survey counts without FSInfo: volume.rs)
    let info = fat.fsinfo_free(&mut d).expect("the Sim's FSInfo is whole");
    assert_eq!(app.survey.free(), Some(fat.clusters_bytes(info)), "the free space: FSInfo's count");
    assert!(free.abs_diff(info) < 64, "the FAT's count is near it");
    assert_eq!(app.survey.total(), Some(fat.clusters_bytes(fat.clusters())));

    let mut names: Vec<std::string::String> = Vec::new();
    let dir = fat.open(&mut d, "Activities").unwrap();
    let mut c = fat.cursor(dir).unwrap();
    while let Some(e) = fat.list(&mut d, &mut c).unwrap() {
        if !e.file.dir && crate::stock::ride_start(e.name()).is_some() {
            names.push(std::string::String::from_utf8_lossy(e.name()).into_owned());
        }
    }
    names.sort();
    assert!(!names.is_empty(), "the Sim's card has rides");
    assert_eq!(app.survey.rides(), Some(names.len() as u32));
    for (k, want) in names.iter().take(OLDEST).enumerate() {
        let e = app.survey.oldest(k).unwrap_or_else(|| panic!("oldest {k}"));
        assert_eq!(e.name(), want.as_bytes(), "the oldest rides first");
    }

    let (label, value, _) = app.menu_row(Menu::Storage, 3).unwrap();
    let mut want = Line::<12>::new();
    size_text(&mut want, app.survey.oldest(0).map(|e| u64::from(e.file.size)));
    assert_eq!((label.as_str().len(), value.as_str()), (8, want.as_str()), "date and size of the oldest");
    sim.save("storage");

    // the oldest ride's card: Select on its row, read over the passes, then HISTORY's card with DELETE
    for _ in 0..3 {
        app.press(&mut sim, Press::Short(Key::Page));
    }
    app.press(&mut sim, Press::Short(Key::Start));
    run(&mut sim, &mut app, 3000);
    assert_eq!(app.screen(), Screen::Past, "the oldest ride's card");
    sim.save("storage_ride");
    app.press(&mut sim, Press::Short(Key::Start)); // a card is dismissed by Start (DELETE is asked by Start held)
    assert_eq!(app.screen(), Screen::Menu(Menu::Storage, 3), "the card closes to STORAGE");
}

/// ABOUT under SYSTEM: the version and commit, and the notices of the map data and of the font.
#[test]
fn about_has_the_notices() {
    let (mut sim, mut app) = started();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, Row::About);
    app.press(&mut sim, Press::Short(Key::Start));
    assert_eq!(app.screen(), Screen::About);
    assert!(ui::ABOUT_NOTICES.contains(&"(c) OpenStreetMap contributors (ODbL)"));
    assert!(ui::ABOUT_NOTICES.contains(&"(c) OpenMapTiles"));
    assert!(ui::ABOUT_NOTICES.contains(&"(c) IGN (Plan IGN v2, Etalab 2.0)"));
    assert!(ui::ABOUT_NOTICES.contains(&"BARLOW, SIL OFL 1.1"));
    sim.save("about");
    app.press(&mut sim, Press::Short(Key::Back));
    assert_eq!(app.screen(), Screen::Menu(Menu::System, row_of(Row::About)), "Back returns to SYSTEM");
}
