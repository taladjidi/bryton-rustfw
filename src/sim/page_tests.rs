//! The page editor and `\Rustfw\pages.bin` (app/editor.rs, page.rs) on the Sim: the editor by keys and by touch, on
//! the page at its real size, keep or drop, undo, pages added and deleted and their round, never while recording;
//! the file written, read back, damaged, cut at every sector, and the backup registers beside it.

use super::*;
use crate::fat::tests::{Ram, read_all};
use crate::page::{Page, Saved};
use crate::ui::Field::{self, *};

/// A unit with this firmware's settings and the classic pages (speed over heart rate and cadence; four fields; the
/// map) on the synthetic volume, past the pages' file's read.
fn started() -> (Sim, App) {
    started_on(false)
}

/// [`started`] on a card that offers reliable writes (a cut then tears no sector) or not.
fn started_on(reliable: bool) -> (Sim, App) {
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    if reliable {
        sim.emmc.rel_param = crate::emmc::EN_REL_WR;
    }
    sim.own_settings();
    sim.classic_pages();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, PAGES_LOAD_MS as u64 + 200);
    (sim, app)
}

/// The same unit started again on the same card and backup registers.
fn reboot(sim: &mut Sim) -> (Sim, App) {
    let mut sim2 = Sim::new(9, 5, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    sim2.backup = sim.backup;
    sim2.emmc.rel_param = sim.emmc.rel_param;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, PAGES_LOAD_MS as u64 + 200);
    (sim2, app)
}

/// The file as on the card, if it is there.
fn file(sim: &Sim) -> Option<Saved> {
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).ok()?;
    let v = read_all(&mut fat, &mut d, "Rustfw/pages.bin").ok()?;
    Saved::newest(&v.try_into().ok()?)
}

/// The ride pages in use.
fn pages(app: &App) -> Vec<Page> {
    app.pages()[..app.page_count()].to_vec()
}

fn press(sim: &mut Sim, app: &mut App, k: Key) {
    app.press(sim, Press::Short(k));
}

/// The centre of box `k` of the page on screen.
fn centre(app: &App, k: usize) -> (u16, u16) {
    let (bx, _) = app.widgets().nth(k).unwrap();
    ((bx.0 + bx.2 / 2) as u16, (bx.1 + bx.3 / 2) as u16)
}

/// Row `r` of the editor's actions, from the actions menu's first row.
fn action(sim: &mut Sim, app: &mut App, r: menu::EditRow) {
    let Screen::Menu(Menu::Edit(_), 0) = app.screen() else { panic!("{:?}", app.screen()) };
    for _ in 0..menu::EDIT_ROWS.iter().position(|&x| x == r).unwrap() {
        press(sim, app, Key::Page);
    }
    press(sim, app, Key::Start);
}

/// A hold on a box opens the editor on the page as it is, live, the box framed; Page and a tap move the frame,
/// a second tap opens the actions; SPLIT down adds a row; SPLIT across in a full row says why not; Back
/// keeps (item 10: it never drops); the file is written between rides, with the registers after it, and read back at the next start.
#[test]
fn edit_a_page_on_the_page() {
    let (mut sim, mut app) = started();
    press(&mut sim, &mut app, Key::Page);
    assert_eq!(app.screen(), Screen::Ride(0));
    let (x, y) = centre(&app, 1);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    assert_eq!(app.screen(), Screen::Edit(0, 1), "the box held selected");
    run(&mut sim, &mut app, 2100); // live values under the frame
    super::tests::assert_consistent(&sim, &app);
    sim.save("page_editor");
    press(&mut sim, &mut app, Key::Page);
    assert_eq!(app.screen(), Screen::Edit(0, 2));
    super::tests::assert_consistent(&sim, &app);
    let (x, y) = centre(&app, 0);
    app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
    assert_eq!(app.screen(), Screen::Edit(0, 0), "a tap selects");
    app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
    assert_eq!(app.screen(), Screen::Menu(Menu::Edit(0), 0), "a tap on the selected one: its actions");
    run(&mut sim, &mut app, 100);
    super::tests::assert_consistent(&sim, &app);
    sim.save("page_editor_actions");
    action(&mut sim, &mut app, menu::EditRow::SplitDown);
    assert_eq!(app.screen(), Screen::Edit(0, 0));
    let e = app.edit.unwrap();
    assert_eq!((e.work.used(), &e.work.fields[..4]), (&[(1, 1), (1, 1), (1, 2)][..], &[Speed, Timer, Hr, Cadence][..]));
    run(&mut sim, &mut app, 1100);
    super::tests::assert_consistent(&sim, &app);
    press(&mut sim, &mut app, Key::Lap); // the last box: cadence
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::SplitAcross);
    assert!(app.notes.text.as_str().contains("2 A ROW"), "{}", app.notes.text.as_str());
    assert_eq!(app.screen(), Screen::Menu(Menu::Edit(3), 1), "refused: the actions stay");
    super::tests::back(&mut sim, &mut app);
    assert!(app.file_untouched_since_start(&sim), "nothing written while editing");
    press(&mut sim, &mut app, Key::Back);
    assert_eq!(app.screen(), Screen::Ride(0), "Back keeps the changes, back on the page");
    assert_eq!(app.notes.text.as_str(), "CHANGES KEPT");
    assert_eq!(app.pages()[0], e.work);
    run(&mut sim, &mut app, 1100);
    super::tests::assert_consistent(&sim, &app);
    sim.save("page_edited");
    let f = file(&sim).expect("written");
    assert_eq!((f.count, f.pages[0], f.seq), (3, e.work, 1));
    let regs: Vec<u32> = PAGE_BKP[..REG_PAGES].iter().flatten().map(|&r| sim.backup[r]).collect();
    assert_eq!(regs, f.words, "the registers as the file says");
    assert_eq!(crate::fat::write::tests::fsck(&sim.emmc.image), Ok(0));
    let (_, app) = reboot(&mut sim);
    assert_eq!(pages(&app), pages_of(&f), "read back");
}

fn pages_of(s: &Saved) -> Vec<Page> {
    s.pages[..s.count].to_vec()
}

impl App {
    /// The pages' file not written yet (the tests' check that an open editor writes nothing).
    fn file_untouched_since_start(&self, _sim: &Sim) -> bool {
        self.store.seq == 0
    }
}

/// MERGE, UNDO, MERGE again, then DROP CHANGES (the actions' last row, only once there is something to drop; asked
/// first): dropped, the page as it was, nothing written. Back, the other way out, keeps the same work.
#[test]
fn drop_and_undo() {
    let (mut sim, mut app) = started();
    let before = app.pages()[1];
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Hold(60, 100)));
    assert_eq!(app.screen(), Screen::Edit(1, 0));
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::Merge);
    assert_eq!(app.edit.unwrap().work.cells(), 3);
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::Undo);
    assert_eq!(app.edit.unwrap().work, before, "undone");
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::Merge);
    super::tests::assert_consistent(&sim, &app);
    let merged = app.edit.unwrap().work;
    press(&mut sim, &mut app, Key::Start); // the actions
    assert_eq!(app.rows(Menu::Edit(0)), menu::EDIT_ROWS.len() + 1, "DROP CHANGES is there");
    for _ in 0..menu::EDIT_ROWS.len() {
        press(&mut sim, &mut app, Key::Page);
    }
    press(&mut sim, &mut app, Key::Start); // asks
    assert!(matches!(app.screen(), Screen::Menu(Menu::Edit(_), _)), "asked, not dropped yet");
    press(&mut sim, &mut app, Key::Start); // drop
    assert_eq!((app.screen(), app.pages()[1]), (Screen::Ride(1), before));
    assert_eq!(app.notes.text.as_str(), "CHANGES DROPPED");
    run(&mut sim, &mut app, 500);
    assert_eq!(file(&sim), None, "nothing to write");
    super::tests::assert_consistent(&sim, &app);
    // nothing changed: no DROP CHANGES row; changed, Back keeps
    app.input(&mut sim, Input::Touch(Gesture::Hold(60, 100)));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!(app.rows(Menu::Edit(0)), menu::EDIT_ROWS.len(), "nothing to drop");
    action(&mut sim, &mut app, menu::EditRow::Merge);
    press(&mut sim, &mut app, Key::Back);
    assert_eq!((app.screen(), app.pages()[1]), (Screen::Ride(1), merged), "Back kept it");
}

/// While a ride records: a hold does not open the editor, the ride menu does not offer it, and asked for it says
/// to pause; paused, it opens; recording again behind it (here: by the ride menu's code) leaves it, its work kept.
#[test]
fn never_while_recording() {
    let (mut sim, mut app) = started();
    press(&mut sim, &mut app, Key::Start); // the ride, on its first page
    run(&mut sim, &mut app, 1000);
    assert_eq!(app.screen(), Screen::Ride(0));
    app.input(&mut sim, Input::Touch(Gesture::Hold(120, 100)));
    assert_eq!(app.screen(), Screen::Ride(0), "no editor by a hold");
    app.press(&mut sim, Press::Long(Key::Page));
    assert!(!(0..app.rows(Menu::Ride(0))).any(|i| app.row(Menu::Ride(0), i) == Some(menu::Row::Open(Menu::Page(0)))));
    app.edit_page(&mut sim, 0, 0);
    assert_eq!(app.notes.text.as_str(), "PAUSE TO EDIT");
    press(&mut sim, &mut app, Key::Start); // PAUSE (the ride menu's first row)
    assert_eq!(app.ride.state(), ride::State::Paused);
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(0)));
    press(&mut sim, &mut app, Key::Start); // EDIT THIS PAGE
    assert_eq!(app.screen(), Screen::Edit(0, 0));
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::Taller);
    let work = app.edit.unwrap().work;
    assert_ne!(work, app.pages()[0]);
    app.start_pause(); // recording again
    run(&mut sim, &mut app, 200);
    assert_eq!((app.edit, app.pages()[0]), (None, work), "left, kept");
    assert!(!matches!(app.screen(), Screen::Edit(..)));
    run(&mut sim, &mut app, 2000);
    assert_eq!(file(&sim), None, "not written during the ride");
    super::tests::assert_consistent(&sim, &app);
}

use super::tests::to_row;

/// PAGES > ADD PAGE: a page, the editor on it; up to six; the Page key goes round them all; a page deleted leaves the
/// round; the last one cannot go.
#[test]
fn pages_added_and_deleted() {
    let (mut sim, mut app) = started();
    for n in 4..=6 {
        app.press(&mut sim, Press::Long(Key::Page));
        to_row(&mut sim, &mut app, menu::Row::AddPage);
        press(&mut sim, &mut app, Key::Start);
        assert_eq!((app.screen(), app.page_count()), (Screen::Edit(n - 1, 0), n));
        run(&mut sim, &mut app, 4500); // its banner up and gone
        super::tests::assert_consistent(&sim, &app);
        app.leave_menus();
        app.show(&mut sim);
    }
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::AddPage);
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.page_count(), app.notes.text.as_str()), (6, "6 PAGES AT MOST"));
    app.leave_menus();
    app.show(&mut sim);
    let mut round = Vec::new();
    for _ in 0..7 {
        press(&mut sim, &mut app, Key::Page);
        round.push(app.screen());
    }
    assert_eq!(round, [0, 1, 2, 3, 4, 5, 0].map(Screen::Ride));
    // the fourth deleted from its editor: five in the round
    app.screen = Screen::Ride(3);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Hold(60, 100)));
    press(&mut sim, &mut app, Key::Start);
    action(&mut sim, &mut app, menu::EditRow::DeletePage);
    assert_eq!(app.page_count(), 6, "asked first");
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.page_count(), app.screen()), (5, Screen::Ride(3)));
    super::tests::assert_consistent(&sim, &app);
    run(&mut sim, &mut app, 200);
    assert_eq!(file(&sim).unwrap().count, 5);
    // down to one, which stays
    while app.page_count() > 1 {
        let n = app.page_count();
        app.screen = Screen::Ride(0);
        app.edit_page(&mut sim, 0, 0);
        app.open(Screen::Menu(Menu::Edit(0), 8));
        app.input(&mut sim, Input::Select);
        app.input(&mut sim, Input::Select);
        assert_eq!(app.page_count(), n - 1);
    }
    app.edit_page(&mut sim, 0, 0);
    app.open(Screen::Menu(Menu::Edit(0), 8));
    app.input(&mut sim, Input::Select);
    assert_eq!((app.page_count(), app.notes.text.as_str()), (1, "ONE PAGE AT LEAST"));
}

/// Pages made in the editor, saved, for the README: three rows of two, a big graph over four values.
#[test]
fn user_made_pages() {
    let (mut sim, mut app) = started();
    // the graph over the power and heart rate, then cadence and the timer, the graph taller
    let mut p = Page::preset(1, [Graph, Time, Time, Time, Time, Time, Time, Time]).unwrap();
    p = p.split_down(0, Power3s).unwrap().resize(0, true).unwrap().split_across(1, Hr).unwrap();
    p = p.split_down(1, Cadence).unwrap().split_across(3, Timer).unwrap();
    assert_eq!(p.used(), [(2, 1), (1, 2), (1, 2)]);
    assert!(p.valid(), "{p:?}");
    app.pages[0] = p;
    app.pages[1] = Page::preset(14, [Speed, Hr, Power, Cadence, Dist, Timer, Ascent, Np]).unwrap();
    app.pages_changed(&mut sim);
    sim.gnss.fix = Some(500);
    sim.backup[Kind::Hr.backup()] = super::tests::STRAP.to_word();
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 60_000);
    super::tests::assert_consistent(&sim, &app);
    sim.save("user_page_graph");
    press(&mut sim, &mut app, Key::Page);
    run(&mut sim, &mut app, 1000);
    super::tests::assert_consistent(&sim, &app);
    sim.save("user_page_eight");
}

/// The file and the registers: a damaged file falls back to the registers (three pages, as they keep them);
/// registers another build changed win for their pages, the file's others kept; a build that may not write the card
/// keeps its changes in the registers only and never writes the card for them.
#[test]
fn file_and_registers() {
    let (mut sim, mut app) = started();
    let p4 = Page::preset(2, [Ascent, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
    app.add_page(&mut sim, 3);
    app.leave_editor(&mut sim, true);
    app.pages[3] = p4;
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let saved = file(&sim).unwrap();
    assert_eq!(saved.count, 4);

    // another build changes page 2 in the registers
    let mut other = sim.backup;
    let changed = PageCfg { layout: 1, fields: [Wbal; 6] };
    for (r, w) in PAGE_BKP[1].iter().zip(changed.to_words()) {
        other[*r] = w;
    }
    let mut sim2 = Sim::new(9, 0, 0);
    (sim2.emmc.image, sim2.backup) = (sim.emmc.image.clone(), other);
    let mut app2 = App::start(&mut sim2, None);
    run(&mut sim2, &mut app2, 3000);
    let want = [saved.pages[0], changed.into(), saved.pages[2], p4];
    assert_eq!(pages(&app2), want, "the registers' page 2, the file's others");

    // both copies damaged: the registers
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Rustfw/pages.bin").unwrap();
    let at = fat.lba(f.cluster) as usize * 512;
    sim.emmc.image[at + 20] ^= 1;
    sim.emmc.image[at + 512 + 20] ^= 1;
    let (_, app3) = reboot(&mut sim);
    let regs: Vec<Page> = saved.pages[..3].iter().map(|p| Page::from(p.to_cfg())).collect();
    assert_eq!(pages(&app3), regs);

    // read only: the registers at once, the card never written
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    (sim.writable, sim.backup) = (false, other);
    let image = sim.emmc.image.clone();
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    app.pages[0] = p4;
    app.pages_changed(&mut sim);
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(p4.to_cfg()));
    run(&mut sim, &mut app, 3000);
    assert!(sim.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(sim.emmc.image == image, "the card unchanged");
}

/// A build that may not write the card (a RAM build without EMMC_WRITE=1) still reads the file: a page of a layout
/// the registers cannot keep (here power and heart rate full width over 2 x 2) shows as the file has it, not as the
/// registers' nearest layout (2X3, layout 6: what a RAM build showed the owner on 2026-10-27); the card unwritten.
#[test]
fn a_read_only_build_shows_the_files_pages() {
    let (mut sim, mut app) = started();
    let free = Page {
        rows: [(1, 1), (1, 1), (1, 2), (1, 2), (0, 0)],
        fields: [Power3s, Hr, Speed, Dist, Timer, Ascent, Time, Time],
    };
    assert!(free.valid() && free.preset_id().is_none(), "free-form");
    app.pages[0] = free;
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    assert_eq!(file(&sim).unwrap().pages[0], free);
    assert_eq!(reg_page(&sim).map(|c| c.layout), Some(6), "the registers keep the 2X3 fallback");

    let mut ro = Sim::new(9, 5, 0);
    (ro.emmc.image, ro.backup, ro.writable) = (sim.emmc.image.clone(), sim.backup, false);
    let (image, backup) = (ro.emmc.image.clone(), ro.backup);
    let mut app = App::start(&mut ro, None);
    assert_eq!(app.pages()[0], Page::from(reg_page(&ro).unwrap()), "the registers' page before the file is read");
    run(&mut ro, &mut app, PAGES_LOAD_MS as u64 + 200);
    assert_eq!(app.pages()[0], free, "the file's page");
    assert_eq!(pages(&app), pages(&reboot(&mut sim).1), "as a writable build shows them");
    assert!(ro.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(ro.emmc.image == image, "the card unchanged");
    assert_eq!(ro.backup[PAGE_BKP[0][0]..=PAGE_BKP[REG_PAGES][1]], backup[PAGE_BKP[0][0]..=PAGE_BKP[REG_PAGES][1]]);
}

/// A read-only start on `image` and `backup`, past the file's read: the app, and whether the card is as it was (no
/// write command, every byte the same). The registers are the caller's to check.
fn read_only(image: &[u8], backup: [u32; 32]) -> (Sim, App) {
    let mut ro = Sim::new(9, 5, 0);
    (ro.emmc.image, ro.backup, ro.writable) = (image.to_vec(), backup, false);
    let mut app = App::start(&mut ro, None);
    run(&mut ro, &mut app, PAGES_LOAD_MS as u64 + 200);
    assert!(ro.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(ro.emmc.image == image, "the card unchanged");
    (ro, app)
}

/// A writable start on `image` and `backup`, past the file's read: the app, and the sim (its registers as it left
/// them), for the pages a read-only build must show as well.
fn writable(image: &[u8], backup: [u32; 32]) -> (Sim, App) {
    let mut sim = Sim::new(9, 5, 0);
    (sim.emmc.image, sim.backup) = (image.to_vec(), backup);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, PAGES_LOAD_MS as u64 + 200);
    (sim, app)
}

/// The page registers' words.
fn page_words(sim: &Sim) -> Vec<u32> {
    PAGE_BKP.iter().flatten().map(|&r| sim.backup[r]).collect()
}

/// A read-only build on a file it cannot use, or registers that differ from it: the same pages a writable build
/// shows, the card never written, the registers left as they are or put right as a writable build does.
#[test]
fn a_read_only_build_takes_the_file_as_a_writable_one() {
    let (mut sim, mut app) = started();
    for f in [Wbal, Np] {
        app.pages[2] = Page::preset(1, [f; 8]).unwrap();
        app.pages_changed(&mut sim);
        run(&mut sim, &mut app, 200);
    }
    let saved = file(&sim).unwrap();
    let regs: Vec<Page> = saved.pages[..3].iter().map(|p| Page::from(p.to_cfg())).collect();
    let mut d = Ram(sim.emmc.image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Rustfw/pages.bin").unwrap();
    let at = fat.lba(f.cluster) as usize * 512;

    // (a) both copies damaged: the registers' pages, the registers untouched
    let mut bad = sim.emmc.image.clone();
    bad[at + 20] ^= 1;
    bad[at + 512 + 20] ^= 1;
    let (ro, app) = read_only(&bad, sim.backup);
    assert_eq!((pages(&app), page_words(&ro)), (regs.clone(), page_words(&sim)), "damaged");
    assert_eq!(pages(&app), pages(&writable(&bad, sim.backup).1), "as a writable build");

    // (a) a short file (its directory entry says 512 bytes): the same
    let mut short = sim.emmc.image.clone();
    let entry = |e: &[u8]| {
        let cluster =
            u32::from(u16::from_le_bytes([e[20], e[21]])) << 16 | u32::from(u16::from_le_bytes([e[26], e[27]]));
        e[11] != 0x0f && cluster == f.cluster && e[28..32] == 1024u32.to_le_bytes()
    };
    let name = (0..short.len() / 32).map(|k| 32 * k).find(|&o| entry(&short[o..o + 32])).unwrap();
    short[name + 28..name + 32].copy_from_slice(&512u32.to_le_bytes());
    let (ro, app) = read_only(&short, sim.backup);
    assert_eq!((pages(&app), page_words(&ro)), (regs, page_words(&sim)), "short");
    assert_eq!(pages(&app), pages(&writable(&short, sim.backup).1), "as a writable build");

    // (b) another build changed page 2 in the registers: its page wins, the file's others kept, registers untouched
    let mut other = sim.backup;
    let changed = PageCfg { layout: 1, fields: [Cadence; 6] };
    for (r, w) in PAGE_BKP[1].iter().zip(changed.to_words()) {
        other[*r] = w;
    }
    let (ro, app) = read_only(&sim.emmc.image, other);
    assert_eq!(pages(&app), [saved.pages[0], changed.into(), saved.pages[2]], "the registers' page 2");
    assert_eq!(pages(&app), pages(&writable(&sim.emmc.image, other).1), "as a writable build");
    assert_eq!(ro.backup, other, "registers untouched");

    // (b) a cut between the file and its registers (they hold the file's previous words): put right, as a
    // writable build does
    let mut cut = sim.backup;
    for (r, w) in PAGE_BKP.iter().take(REG_PAGES).flatten().zip(saved.was) {
        cut[*r] = w;
    }
    assert_ne!(cut, sim.backup);
    let (ro, app) = read_only(&sim.emmc.image, cut);
    assert_eq!((pages(&app), page_words(&ro)), (pages_of(&saved), page_words(&sim)), "registers put right");
    assert_eq!(pages(&app), pages(&writable(&sim.emmc.image, cut).1), "as a writable build");
}

/// A first start of a read-only build whose file cannot be read yet (a read error there): stock's pages are not put over
/// the file's, which show once it is read; the card is not written.
#[test]
fn a_first_start_on_a_read_only_build_keeps_the_files_pages() {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(2, [Wbal, Np, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let want = pages(&app);
    let image = sim.emmc.image.clone();
    let mut d = Ram(image.clone(), 0);
    let mut fat = crate::fat::Fat::mount(&mut d).unwrap();
    let f = fat.open(&mut d, "Rustfw/pages.bin").unwrap();
    let lba = fat.lba(f.cluster);
    let mut ro = Sim::new(9, 5, 0);
    (ro.emmc.image, ro.writable) = (image.clone(), false);
    ro.emmc.unreadable.push(lba..lba + 2);
    let mut app = App::start(&mut ro, None);
    use crate::ui::Field::*;
    assert_ne!(&app.pages()[0].fields[..4], &[Power, Hr, Cadence, Speed][..], "stock's first page not taken");
    ro.emmc.unreadable.clear();
    run(&mut ro, &mut app, PAGES_LOAD_MS as u64 + 200);
    assert_eq!(pages(&app), want, "the file's pages once read");
    assert!(ro.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(ro.emmc.image == image, "the card unchanged");
}

/// A first start of a read-only build over stock's settings and no pages.bin: stock's pages are taken into the
/// registers and shown, as a writable build takes them; the card is not written.
#[test]
fn a_first_start_on_a_read_only_build_takes_stocks_pages() {
    let image = crate::fat::tests::image();
    let (ro, app) = read_only(&image, [0; 32]);
    let (w, wapp) = writable(&image, [0; 32]);
    assert_eq!(pages(&app), pages(&wapp), "as a writable build shows them");
    assert_eq!(page_words(&ro), page_words(&w), "the registers as a writable build's");
    use crate::ui::Field::*;
    assert_eq!(&app.pages()[0].fields[..4], &[Power, Hr, Cadence, Speed][..], "stock's first page");
}

/// A page edit on a read-only build before the file could be read (the first read failing): refused, so nothing
/// shows that the file's pages would then take away; after the read, the screen's pages are the file's and the
/// registers', the card unwritten.
#[test]
fn a_read_only_edit_waits_for_the_file() {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(2, [Wbal, Np, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let want = pages(&app);
    let mut ro = Sim::new(9, 5, 0);
    (ro.emmc.image, ro.backup, ro.writable) = (sim.emmc.image.clone(), sim.backup, false);
    let image = ro.emmc.image.clone();
    ro.emmc.dead = true;
    let mut app = App::start(&mut ro, None);
    app.step(&mut ro);
    app.screen = Screen::Ride(0);
    let before = app.pages()[0];
    app.next_layout(&mut ro, 0);
    assert_eq!((app.notes.text.as_str(), app.pages()[0]), ("PAGES NOT READ", before), "refused");
    assert_eq!(page_words(&ro), page_words(&sim), "nothing in the registers");
    ro.emmc.dead = false;
    run(&mut ro, &mut app, PAGES_LOAD_MS as u64 + 200);
    assert_eq!(pages(&app), want, "the file's pages");
    let regs: Vec<PageCfg> = (0..3).map(|k| app.pages()[k].to_cfg()).collect();
    let kept: Vec<PageCfg> =
        PAGE_BKP.iter().take(3).map(|r| PageCfg::from_words([ro.backup[r[0]], ro.backup[r[1]]]).unwrap()).collect();
    assert_eq!(regs, kept, "the screen's pages are the registers'");
    assert!(ro.emmc.cmds.iter().all(|c| !matches!(c.0, 24 | 25)), "no write command");
    assert!(ro.emmc.image == image, "the card unchanged");
}

/// A power cut after every sector of a write of the pages, the first (the file created) and a later one (a copy
/// over the older): the volume stays clean for fsck, and the next start reads the old pages or the new, never a mix.
/// The registers are as the cut left them: as before the write (they come after it), the CPU dying with the card.
/// The sector being written at the cut is torn (half old, half new), unless the card offers reliable writes (src/sim/).
#[test]
fn a_cut_at_every_sector_reads_old_or_new() {
    let edit = |app: &mut App, sim: &mut Sim, k: u32| {
        let mut p = app.pages()[0];
        p.fields[0] = Field::ALL[2 + k as usize];
        app.pages[0] = p;
        app.pages[3] = Page::preset(1, [Field::ALL[10 + k as usize]; 8]).unwrap();
        app.page_count = 4;
        app.pages_changed(sim);
    };
    for (writes, reliable) in [(1, false), (2, false), (1, true), (2, true)] {
        let setup = || {
            let (mut sim, mut app) = started_on(reliable);
            sim.emmc.tear = true;
            for k in 0..writes - 1 {
                edit(&mut app, &mut sim, k);
                run(&mut sim, &mut app, 200);
            }
            let old = pages(&app);
            let (backup, base) = (sim.backup, sim.emmc.sectors_written);
            edit(&mut app, &mut sim, writes);
            (sim, app, old, backup, base)
        };
        let (mut sim, mut app, _, _, base) = setup();
        let new = pages(&app);
        run(&mut sim, &mut app, 200);
        let total = sim.emmc.sectors_written - base;
        assert!(total > 0);
        let (mut olds, mut news) = (0, 0);
        for cut in 0..total {
            let (mut sim, mut app, old, backup, base) = setup();
            sim.emmc.cut_after = Some(base + cut);
            run(&mut sim, &mut app, 200);
            sim.backup = backup; // the cut stopped the CPU before the registers
            let (mut sim, mut app) = reboot(&mut sim);
            let got = pages(&app);
            // the next write (the FAT copies compared first when the file was not whole) leaves a clean volume
            edit(&mut app, &mut sim, 7);
            run(&mut sim, &mut app, 30_000);
            assert!(!app.store.dirty, "cut {cut}: written");
            let lost = crate::fat::write::tests::fsck(&sim.emmc.image).unwrap_or_else(|e| panic!("cut {cut}: {e}"));
            assert!(lost <= 2, "cut {cut}: {lost} clusters lost");
            if got == old {
                olds += 1;
            } else {
                assert_eq!(got, new, "write {writes}, cut {cut}: neither old nor new");
                news += 1;
            }
        }
        std::println!(
            "write {writes} (reliable {reliable}): {total} sectors, {olds} cuts read the old pages, {news} the new"
        );
        assert!(olds > 0);
    }
}

/// A change in the first seconds, before the file would be read: it is read first, so the change is made to its
/// pages (the fourth and later kept), not to the registers' three.
#[test]
fn an_early_change_keeps_the_files_pages() {
    let (mut sim, mut app) = started();
    app.add_page(&mut sim, 3);
    app.leave_editor(&mut sim, true);
    run(&mut sim, &mut app, 200);
    assert_eq!(file(&sim).unwrap().count, 4);
    let mut sim2 = Sim::new(9, 0, 0);
    (sim2.emmc.image, sim2.backup) = (sim.emmc.image.clone(), sim.backup);
    let mut app = App::start(&mut sim2, None);
    app.step(&mut sim2);
    assert_eq!(app.page_count(), 3, "the registers' pages at first");
    app.screen = Screen::Ride(1);
    app.edit_page(&mut sim2, 1, 0);
    assert_eq!(app.page_count(), 4, "the file read before the editor opened");
    app.leave_editor(&mut sim2, true);
}

/// A flat backup battery (every register lost) over a card with stock's settings and this firmware's pages: the
/// first start takes stock's settings, but the pages are the file's.
#[test]
fn a_first_start_keeps_the_files_pages() {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(2, [Wbal, Np, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let want = pages(&app);
    let mut sim2 = Sim::new(9, 0, 0);
    sim2.emmc.image = core::mem::take(&mut sim.emmc.image);
    let mut app = App::start(&mut sim2, None);
    assert_eq!(app.settings.ftp, 265, "stock's settings taken");
    run(&mut sim2, &mut app, 3000);
    assert_eq!(pages(&app), want);
}

/// The first ride page as the registers keep it.
fn reg_page(sim: &Sim) -> Option<PageCfg> {
    PageCfg::from_words([sim.backup[PAGE_BKP[0][0]], sim.backup[PAGE_BKP[0][1]]])
}

/// A page changed during a paused ride (the file waits for the ride's end), then the unit switched off: the
/// registers had it at once, marked newer than the file, so the next start shows it, even when it is the page the
/// file had before (a change undone: the registers then equal the file's previous words); the file is written once
/// no ride is open.
#[test]
fn a_change_in_a_paused_ride_survives_a_switch_off() {
    let (mut sim, mut app) = started();
    let a = app.pages()[0];
    let b = Page::preset(2, [Wbal, Np, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages[0] = b;
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    assert_eq!(file(&sim).unwrap().pages[0], b, "written between rides");
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 3000);
    app.start_pause();
    run(&mut sim, &mut app, 500);
    assert_eq!(app.ride.state(), ride::State::Paused);
    app.screen = Screen::Ride(0);
    app.edit_page(&mut sim, 0, 0);
    app.edit.as_mut().unwrap().work = a; // back to the first page
    app.leave_editor(&mut sim, true);
    run(&mut sim, &mut app, 2000);
    assert_eq!(file(&sim).unwrap().pages[0], b, "not written during the ride");
    assert_eq!(reg_page(&sim), Some(a.to_cfg()), "the registers at once");
    app.power_off(&mut sim);
    let (sim2, app2) = reboot(&mut sim);
    assert_eq!(app2.pages()[0], a, "the change kept through the switch-off");
    assert_eq!(app2.pages()[1..3], app.pages()[1..3]);
    assert!(app2.store.dirty, "the file to be written once the resumed ride is saved");
    assert_eq!(file(&sim2).unwrap().pages[0], b);
}

/// A change made between rides and the unit switched off before the next pass wrote the file: the switch-off puts
/// it in the registers, and the next start writes it to the file.
#[test]
fn a_change_just_before_a_switch_off_is_kept() {
    let (mut sim, mut app) = started();
    let p = Page::preset(3, [Hr, Power, Cadence, Time, Time, Time, Time, Time]).unwrap();
    app.pages[0] = p;
    app.pages_changed(&mut sim);
    app.power_off(&mut sim);
    let (sim2, app2) = reboot(&mut sim);
    assert_eq!(app2.pages()[0], p);
    let f = file(&sim2).expect("written at the next start");
    assert_eq!(f.pages[0], p);
    assert_eq!(reg_page(&sim2), Some(p.to_cfg()));
    assert_eq!(sim2.backup[PAGE_BKP[0][1]] & PAGES_NEWER, 0, "the mark cleared with the file written");
}

/// From a page with six undersized graphs, every page the edits reach keeps each box a value's size at least
/// (values at 3x), and never makes unreadable a field that was readable.
#[test]
fn edits_never_squeeze_a_readable_field() {
    let start = Page::preset(6, [Graph, AltGraph, HrGraph, WbalGraph, Graph, Graph, Time, Time]).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut todo = std::vec![start];
    while let Some(p) = todo.pop() {
        if seen.len() > 50_000 || !seen.insert(std::format!("{p:?}")) {
            continue;
        }
        assert!(p.roomy(), "{p:?}");
        for k in 0..p.cells() {
            for q in [
                p.split_across(k, Speed),
                p.split_down(k, Speed),
                p.merge(k).map(|x| x.0),
                p.resize(k, true),
                p.resize(k, false),
                p.with_field(k, Hr),
            ]
            .into_iter()
            .flatten()
            {
                for f in Field::ALL {
                    assert!(q.misfits_of(f) <= p.misfits_of(f), "{f:?}: {p:?} -> {q:?}");
                }
                todo.push(q);
            }
        }
    }
    assert!(seen.len() > 100, "{} pages", seen.len());
}

/// The card lent to the computer before the file was read: a page change is refused (the registers' three pages
/// would otherwise be written over the file's four), and the file keeps its pages.
#[test]
fn no_change_before_the_file_is_read() {
    let (mut sim, mut app) = started();
    app.add_page(&mut sim, 3);
    app.leave_editor(&mut sim, true);
    run(&mut sim, &mut app, 200);
    let want = pages(&app);
    let mut sim2 = Sim::new(9, 0, 0);
    (sim2.emmc.image, sim2.backup) = (sim.emmc.image.clone(), sim.backup);
    let mut app = App::start(&mut sim2, None);
    app.step(&mut sim2);
    app.emmc.lend(true);
    app.screen = Screen::Ride(0);
    let before = app.pages()[0];
    app.next_layout(&mut sim2, 0);
    assert_eq!((app.notes.text.as_str(), app.pages()[0]), ("USB DRIVE ON", before), "refused");
    app.edit_page(&mut sim2, 0, 0);
    assert_eq!(app.edit, None);
    run(&mut sim2, &mut app, 500);
    app.emmc.lend(false);
    run(&mut sim2, &mut app, 3000);
    assert_eq!((pages(&app), file(&sim2).unwrap().count), (want, 4));
}

/// A tap on a graph shows the next plot until the power goes: the page as kept is not changed, so a later change to
/// another page does not save it, and the next start shows the page's own plot.
#[test]
fn a_graph_tap_is_never_saved() {
    let (mut sim, mut app) = started();
    app.pages[1] = Page::preset(2, [Graph, Hr, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 300);
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Tap(120, 100)));
    let shown = app.widgets().next().unwrap().1;
    assert!(matches!(shown, Widget::Field(f) if f != Graph && graph_plot(f).is_some()), "{shown:?}");
    assert_eq!(app.pages()[1].fields[0], Graph, "the page as kept");
    super::tests::assert_consistent(&sim, &app);
    app.next_layout(&mut sim, 0); // another page changed for good
    run(&mut sim, &mut app, 300);
    assert_eq!(file(&sim).unwrap().pages[1].fields[0], Graph, "the tap not saved");
    let (_, app2) = reboot(&mut sim);
    assert_eq!(app2.pages()[1].fields[0], Graph);
}

/// A copy of the file with a row too short for a value (CRC good, well formed) is refused at read; such a page
/// drawn anyway (as an earlier build could have saved it) does not panic.
#[test]
fn a_tiny_row_is_refused_and_drawn_safely() {
    let rows = [(1, 2), (3, 1), (3, 1), (3, 1), (3, 1)];
    let tiny = Page { rows, fields: [Speed, Hr, Power, Cadence, Dist, Time, Time, Time] };
    assert!(tiny.well_formed() && !tiny.roomy());
    let s = Saved {
        seq: 3,
        pages: [tiny; PAGES],
        count: 1,
        words: [0; 6],
        was: [0; 6],
        more: [None; PAGES],
        indoor: None,
        mtb: None,
    };
    assert_eq!(Saved::from_sector(&s.to_sector()), None);
    let (mut sim, mut app) = started();
    app.pages[0] = tiny;
    app.pages[1] = Page { fields: [Graph; 8], ..tiny };
    for n in 0..2 {
        app.screen = Screen::Ride(n);
        app.show(&mut sim);
        run(&mut sim, &mut app, 1100);
    }
}

/// The file's first read fails (the card not answering): it is read again before a change, so the change is written
/// as the newest copy, never under the older one.
#[test]
fn a_failed_read_is_tried_again() {
    let (mut sim, mut app) = started();
    for f in [Wbal, Np, Hr] {
        app.pages[2] = Page::preset(1, [f; 8]).unwrap();
        app.pages_changed(&mut sim);
        run(&mut sim, &mut app, 200);
    }
    assert_eq!(file(&sim).unwrap().seq, 3);
    let mut sim2 = Sim::new(9, 0, 0);
    (sim2.emmc.image, sim2.backup) = (core::mem::take(&mut sim.emmc.image), sim.backup);
    sim2.emmc.dead = true;
    let mut app = App::start(&mut sim2, None);
    run(&mut sim2, &mut app, PAGES_LOAD_MS as u64 + 200);
    sim2.emmc.dead = false;
    let p = Page::preset(2, [Speed, Ascent, Time, Time, Time, Time, Time, Time]).unwrap();
    app.screen = Screen::Ride(0);
    app.edit_page(&mut sim2, 0, 0);
    app.edit.as_mut().unwrap().work = p;
    app.leave_editor(&mut sim2, true);
    run(&mut sim2, &mut app, 500);
    let f = file(&sim2).unwrap();
    assert_eq!((f.seq, f.pages[0]), (4, p), "the newest copy");
}

/// PAGES > ADD PAGE before the file was read: the page goes after the file's last, not after the registers' third.
#[test]
fn a_page_added_early_goes_last() {
    let (mut sim, mut app) = started();
    app.add_page(&mut sim, 3);
    app.leave_editor(&mut sim, true);
    run(&mut sim, &mut app, 200);
    let fourth = app.pages()[3];
    let mut sim2 = Sim::new(9, 0, 0);
    (sim2.emmc.image, sim2.backup) = (sim.emmc.image.clone(), sim.backup);
    let mut app = App::start(&mut sim2, None);
    app.step(&mut sim2);
    assert_eq!(app.page_count(), 3, "the registers' pages at first");
    app.press(&mut sim2, Press::Long(Key::Page));
    to_row(&mut sim2, &mut app, menu::Row::AddPage);
    press(&mut sim2, &mut app, Key::Start);
    assert_eq!((app.page_count(), app.screen(), app.pages()[3]), (5, Screen::Edit(4, 0), fourth));
}

// ---------------------------------------------------------------------------------------------------------------
// MORE (app/more.rs)
// ---------------------------------------------------------------------------------------------------------------

fn swipe(sim: &mut Sim, app: &mut App, d: Dir) {
    app.input(sim, Input::Touch(Gesture::Swipe(d)));
    run(sim, app, 100);
}

/// A unit riding, with a fix and a strap, on its first ride page (the page, then its MORE, are saved for the README).
fn riding() -> (Sim, App) {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(4, [Speed, Timer, Dist, Hr, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    sim.gnss.fix = Some(500);
    sim.backup[Kind::Hr.backup()] = super::tests::STRAP.to_word();
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 30_000);
    (sim, app)
}

/// A swipe up on a ride page shows its denser variant (the page's fields and more, eight boxes), a swipe down or
/// Back the page again; Page goes to the next page's normal variant; the recording goes on under it all; the touch
/// lock stops the swipe; only the boxes that changed are drawn, and the panel equals the screenshot throughout.
#[test]
fn a_swipe_up_shows_more() {
    let (mut sim, mut app) = riding();
    assert_eq!(app.ride.state(), ride::State::Running);
    assert_eq!((app.screen(), app.more_shown(), app.widgets().count()), (Screen::Ride(0), None, 4));
    super::tests::assert_consistent(&sim, &app);
    sim.save("page_normal");
    swipe(&mut sim, &mut app, Dir::Up);
    assert_eq!((app.screen(), app.more_shown(), app.widgets().count()), (Screen::Ride(0), Some(0), 8));
    assert!(app.strip.more);
    let fields: Vec<_> = app.widgets().map(|(_, w)| w).collect();
    assert_eq!(fields[..4], [Speed, Timer, Dist, Hr].map(Widget::Field), "the page's own first");
    super::tests::assert_consistent(&sim, &app);
    sim.save("page_more");
    swipe(&mut sim, &mut app, Dir::Up);
    assert_eq!(app.more_shown(), Some(0), "up again changes nothing");
    run(&mut sim, &mut app, 5000);
    super::tests::assert_consistent(&sim, &app);
    assert_eq!(app.ride.state(), ride::State::Running, "recording unaffected");
    // down: the page, not the remote
    swipe(&mut sim, &mut app, Dir::Down);
    assert_eq!((app.screen(), app.more_shown(), app.strip.more), (Screen::Ride(0), None, false));
    super::tests::assert_consistent(&sim, &app);
    // Back: the page, then the overview
    swipe(&mut sim, &mut app, Dir::Up);
    press(&mut sim, &mut app, Key::Back);
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(0), None));
    press(&mut sim, &mut app, Key::Back);
    assert_eq!(app.screen(), Screen::Main);
    // down on the page with no MORE up opens the quick panel
    press(&mut sim, &mut app, Key::Page);
    swipe(&mut sim, &mut app, Dir::Down);
    assert_eq!(app.screen(), Screen::Quick);
    press(&mut sim, &mut app, Key::Back);
    // Page from MORE: the next page, normal; round to the first, normal
    swipe(&mut sim, &mut app, Dir::Up);
    press(&mut sim, &mut app, Key::Page);
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(1), None));
    for _ in 0..app.page_count() - 1 {
        press(&mut sim, &mut app, Key::Page);
    }
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(0), None), "not up again by coming back");
    // a swipe right does the same
    swipe(&mut sim, &mut app, Dir::Up);
    swipe(&mut sim, &mut app, Dir::Right);
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(2), None));
    press(&mut sim, &mut app, Key::Page);
    // the touch lock
    app.input(&mut sim, Input::TouchLock);
    swipe(&mut sim, &mut app, Dir::Up);
    assert_eq!(app.more_shown(), None, "locked");
    app.input(&mut sim, Input::TouchLock);
    // the menu over MORE: back on the page
    swipe(&mut sim, &mut app, Dir::Up);
    press(&mut sim, &mut app, Key::Start); // pause
    app.input(&mut sim, Input::Menu);
    app.input(&mut sim, Input::Back);
    assert_eq!(app.screen(), Screen::Ride(0));
    super::tests::assert_consistent(&sim, &app);
}

/// Only the changed boxes are drawn on MORE: a second later, the SPI traffic of a pass is about the digits.
#[test]
fn more_redraws_only_what_changes() {
    let (mut sim, mut app) = riding();
    swipe(&mut sim, &mut app, Dir::Up);
    let before = sim.pixels;
    run(&mut sim, &mut app, 2000);
    let drawn = sim.pixels - before;
    assert!(drawn < 4 * 240 * 40, "{drawn} pixels in two seconds");
}

/// A hold on a box of MORE edits MORE (not the page); kept, it is the variant from then on, saved with the pages,
/// read back at the next start, and the editor returns to MORE; dropped, it is the default again.
#[test]
fn the_editor_edits_more() {
    let (mut sim, mut app) = started();
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    let page = app.pages()[1];
    swipe(&mut sim, &mut app, Dir::Up);
    let (x, y) = centre(&app, 2);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    let e = app.edit.unwrap();
    assert_eq!((app.screen(), e.page, e.more, e.work), (Screen::Edit(1, 2), 1, true, page.more_of()));
    super::tests::assert_consistent(&sim, &app);
    sim.save("more_editor");
    // dropped
    let mut w = e.work;
    w.fields[2] = Wbal;
    app.edit.as_mut().unwrap().work = w;
    app.leave_editor(&mut sim, false);
    assert_eq!((app.more[1], app.more_shown()), (None, Some(1)), "back on MORE, nothing kept");
    // kept
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    app.edit.as_mut().unwrap().work = w;
    app.leave_editor(&mut sim, true);
    assert_eq!((app.more[1], app.pages()[1], app.more_shown()), (Some(w), page, Some(1)));
    assert_eq!(app.widgets().nth(2).unwrap().1, Widget::Field(Wbal));
    super::tests::assert_consistent(&sim, &app);
    run(&mut sim, &mut app, 300);
    let f = file(&sim).unwrap();
    assert_eq!((f.more[1], f.more[0], f.pages[1]), (Some(w), None, page));
    let (_, app2) = reboot(&mut sim);
    assert_eq!((app2.more[1], app2.pages()[1]), (Some(w), page));
    // the page's own edit leaves MORE alone, and its default follows the page
    let mut app = app2;
    let mut sim = sim;
    app.screen = Screen::Ride(0);
    app.edit_page(&mut sim, 0, 0);
    assert!(!app.edit.unwrap().more);
    // pages added and deleted carry their MORE
    app.leave_editor(&mut sim, false);
    app.add_page(&mut sim, 0);
    app.leave_editor(&mut sim, false);
    assert_eq!((app.more[0], app.more[2]), (None, Some(w)));
    app.screen = Screen::Ride(0);
    app.edit_page(&mut sim, 0, 0);
    app.open(Screen::Menu(Menu::Edit(0), 8));
    app.input(&mut sim, Input::Select);
    app.input(&mut sim, Input::Select);
    assert_eq!((app.more[1], app.more[2]), (Some(w), None));
}

/// Pages 2 and 3 with MORE made, a cut after every sector of the write: the next start reads the old pages and
/// MORE, or the new, never a mix (the cut test of the pages, with the MORE part in the sector).
#[test]
fn more_survives_a_cut_with_its_pages() {
    let w = Page::preset(14, [Wbal, Np, Hr, Power, Cadence, Dist, Timer, Ascent]).unwrap();
    let (mut sim, mut app) = started();
    app.more[1] = Some(w);
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let w2 = Page { fields: [Speed; 8], ..w };
    let base = sim.emmc.sectors_written;
    app.more[1] = Some(w2);
    app.pages[0] = Page::preset(2, [Ascent, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 200);
    let total = sim.emmc.sectors_written - base;
    assert!(total > 0);
    for cut in 0..total {
        let (mut sim, mut app) = started();
        app.more[1] = Some(w);
        app.pages_changed(&mut sim);
        run(&mut sim, &mut app, 200);
        let backup = sim.backup;
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + cut);
        app.more[1] = Some(w2);
        app.pages[0] = Page::preset(2, [Ascent, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
        app.pages_changed(&mut sim);
        run(&mut sim, &mut app, 200);
        sim.backup = backup;
        let (_, app2) = reboot(&mut sim);
        let new = app2.more[1] == Some(w2);
        assert!(new || app2.more[1] == Some(w), "cut {cut}: {:?}", app2.more[1]);
        assert_eq!(new, app2.pages()[0].fields[0] == Ascent, "cut {cut}: pages and MORE from one copy");
    }
}

// ---- review fixes ----

fn paused(sim: &mut Sim, app: &mut App) {
    press(sim, app, Key::Start);
    run(sim, app, 3000);
    app.start_pause();
    run(sim, app, 500);
    assert_eq!(app.ride.state(), ride::State::Paused);
}

#[test]
fn review_delete_in_paused_ride_then_off_misaligns_more() {
    let w = Page::preset(14, [Wbal, Np, Hr, Power, Cadence, Dist, Timer, Ascent]).unwrap();
    let (mut sim, mut app) = started();
    let v = Page::preset(14, [Ascent, Np, Hr, Power, Cadence, Dist, Timer, Wbal]).unwrap();
    (app.more[0], app.more[1]) = (Some(v), Some(w));
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 300);
    assert_eq!(file(&sim).unwrap().more[1], Some(w));
    let p1 = app.pages()[1];
    paused(&mut sim, &mut app);
    app.screen = Screen::Ride(0);
    app.edit_page(&mut sim, 0, 0);
    app.delete_page(&mut sim);
    assert_eq!((app.pages()[0], app.more[0]), (p1, Some(w)), "in RAM: follows");
    app.power_off(&mut sim);
    let (_, app2) = reboot(&mut sim);
    let i = (0..app2.page_count()).find(|&i| app2.pages()[i] == p1);
    assert_ne!(i.and_then(|i| app2.more[i]), Some(v), "the deleted page's MORE is not kept for another page");
}

#[test]
fn review_graph_tap_on_own_more() {
    let (mut sim, mut app) = started();
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.more[1] = Some(Page::preset(4, [Graph, Hr, Dist, Timer, Time, Time, Time, Time]).unwrap());
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    run(&mut sim, &mut app, 100);
    let k = app.widgets().position(|(_, w)| matches!(w, Widget::Field(Graph))).unwrap();
    let (x, y) = centre(&app, k);
    let px = sim.pixels;
    app.input(&mut sim, Input::Touch(Gesture::Tap(x, y)));
    assert_eq!(sim.pixels, px, "nothing changed, nothing redrawn");
}

#[test]
fn review_menu_over_more() {
    let (mut sim, mut app) = started();
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    app.input(&mut sim, Input::Menu);
    app.input(&mut sim, Input::Back);
    assert_eq!((app.screen(), app.more_shown()), (Screen::Ride(1), Some(1)), "MORE back after a menu");
}

#[test]
fn review_reset_more_in_the_more_editor() {
    let (mut sim, mut app) = started();
    let w = Page::preset(14, [Wbal, Np, Hr, Power, Cadence, Dist, Timer, Ascent]).unwrap();
    app.more[1] = Some(w);
    let n = app.page_count();
    app.screen = Screen::Ride(1);
    app.show(&mut sim);
    app.input(&mut sim, Input::Touch(Gesture::Swipe(Dir::Up)));
    let (x, y) = centre(&app, 2);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    assert!(app.edit.unwrap().more);
    let r = menu::EDIT_ROWS.iter().position(|&r| r == menu::EditRow::DeletePage).unwrap();
    let m = Menu::Edit(2);
    app.activate(&mut sim, m, r);
    assert!(app.more[1].is_some(), "asked first");
    app.activate(&mut sim, m, r);
    assert_eq!((app.more[1], app.page_count()), (None, n), "MORE reset, the page kept");
}

// ---- the INDOOR set (UI review 3, 4.2.7) ----

/// The first field of the page on screen.
fn first_field(app: &App) -> Option<Field> {
    app.widgets().find_map(|(_, w)| if let Widget::Field(f) = w { Some(f) } else { None })
}

/// The round takes the pages of the profile in force: the road set (here four pages) or the indoor one (three); a
/// switch while a page the other set lacks is up moves to its first page; a ride's view follows its profile.
#[test]
fn the_round_takes_the_profiles_pages() {
    let (mut sim, mut app) = started();
    assert_eq!(app.indoor, crate::page::Set::indoor_default(), "no file yet: the defaults");
    app.pages[3] = app.pages[0];
    app.page_count = 4;
    press(&mut sim, &mut app, Key::Page);
    assert_eq!((app.screen(), first_field(&app)), (Screen::Ride(0), Some(Speed)));
    app.settings.profile = Some(crate::ride::Profile::Indoor);
    run(&mut sim, &mut app, 100);
    assert_eq!((app.screen(), first_field(&app)), (Screen::Ride(0), Some(Power3s)), "the trainer page, drawn anew");
    super::tests::assert_consistent(&sim, &app);
    let mut round = Vec::new();
    for _ in 0..4 {
        press(&mut sim, &mut app, Key::Page);
        round.push(app.screen());
    }
    assert_eq!(round, [1, 2, 0, 1].map(Screen::Ride), "three indoor pages");
    // road again with its fourth page up: the indoor set has no such page, so it goes to the first
    app.settings.profile = Some(crate::ride::Profile::Road);
    run(&mut sim, &mut app, 100);
    app.screen = Screen::Ride(3);
    app.show(&mut sim);
    app.settings.profile = Some(crate::ride::Profile::Indoor);
    run(&mut sim, &mut app, 100);
    assert_eq!(app.screen(), Screen::Ride(0));
    super::tests::assert_consistent(&sim, &app);
    // a ride open: PAGES shows the ride's set whatever was chosen there
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 1500);
    app.pages_set = ride::Profile::Road;
    assert!(app.view_set() == ride::Profile::Indoor);
    app.settings.profile = Some(crate::ride::Profile::Road);
    app.toggle_indoor(); // by hand: ROAD
    run(&mut sim, &mut app, 100);
    assert!(app.view_set() == ride::Profile::Road);
}

/// PAGES > ROAD PAGES / INDOOR PAGES open that set's list and the editor works on it (item 8: one row for each set,
/// where PAGES OF was a toggle to press first); ADD PAGE in the indoor list grows the indoor set only, which is
/// written to the file (the registers keep the road pages) and read back at the next start.
#[test]
fn the_pages_menu_edits_the_set_it_names() {
    let (mut sim, mut app) = started();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::PageList(ride::Profile::Indoor)));
    let label = |app: &App, i| app.menu_row(Menu::Pages, i).map(|(l, ..)| std::string::String::from(l.as_str()));
    assert_eq!((label(&app, 0).as_deref(), label(&app, 1).as_deref()), (Some("ROAD PAGES"), Some("INDOOR PAGES")));
    assert_eq!((app.rows(Menu::Pages), label(&app, 2).as_deref()), (3, Some("MTB PAGES")));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!(app.screen(), Screen::Menu(Menu::PageList(ride::Profile::Indoor), 0));
    assert!(app.pages_set == ride::Profile::Indoor, "the editor works on the indoor set");
    assert_eq!(app.rows(Menu::PageList(ride::Profile::Indoor)), 1 + 3 + 1, "the overview, three pages, ADD PAGE");
    sim.save("pages_menu_indoor");
    super::tests::assert_consistent(&sim, &app);
    to_row(&mut sim, &mut app, menu::Row::AddPage);
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.screen(), app.indoor.count, app.page_count()), (Screen::Edit(3, 0), 4, 3), "the indoor set grew");
    app.leave_editor(&mut sim, false);
    run(&mut sim, &mut app, 300);
    let f = file(&sim).unwrap();
    assert_eq!((f.count, f.indoor.map(|i| i.count)), (3, Some(4)));
    assert_eq!(f.indoor.unwrap().pages[3], app.indoor.pages[3]);
    let (_, app2) = reboot(&mut sim);
    assert_eq!((app2.indoor, app2.page_count()), (app.indoor, 3));
    // the road list: the road set
    let (mut sim, mut app) = started();
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::PageList(ride::Profile::Road)));
    press(&mut sim, &mut app, Key::Start);
    assert!(app.pages_set == ride::Profile::Road);
    assert_eq!(app.rows(Menu::PageList(ride::Profile::Road)), 1 + 3 + 1);
    // while a ride is open only the ride's own set opens
    let (mut sim, mut app) = started();
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 1500);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::PageList(ride::Profile::Indoor)));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.notes.text.as_str(), app.pages_set == ride::Profile::Indoor), ("THE RIDE'S SET", false));
}

/// A hold on a box of an indoor page edits the indoor set (the page is indoor's), kept it is written, the road pages
/// untouched; a MORE variant and LAYOUT reach the same set.
#[test]
fn a_hold_edits_the_set_on_screen() {
    let (mut sim, mut app) = started();
    app.settings.profile = Some(crate::ride::Profile::Indoor);
    run(&mut sim, &mut app, 100);
    press(&mut sim, &mut app, Key::Page);
    let road = app.pages()[0];
    let (x, y) = centre(&app, 1);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    let e = app.edit.unwrap();
    assert_eq!((e.page, e.more, e.work, app.pages_set == ride::Profile::Indoor), (0, false, app.indoor.pages[0], true));
    sim.save("indoor_editor");
    let w = e.work.with_field(1, Dist).unwrap();
    app.edit.as_mut().unwrap().work = w;
    app.leave_editor(&mut sim, true);
    assert_eq!((app.indoor.pages[0], app.pages()[0]), (w, road));
    run(&mut sim, &mut app, 300);
    let f = file(&sim).unwrap();
    assert_eq!((f.indoor.unwrap().pages[0], f.pages[0]), (w, road));
    // a MORE variant of an indoor page
    swipe(&mut sim, &mut app, Dir::Up);
    let (x, y) = centre(&app, 2);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    let m = app.edit.unwrap().work.with_field(2, Wbal).unwrap();
    app.edit.as_mut().unwrap().work = m;
    app.leave_editor(&mut sim, true);
    assert_eq!((app.indoor.more[0], app.more[0]), (Some(m), None));
    run(&mut sim, &mut app, 300);
    let (mut sim, mut app) = reboot(&mut sim);
    assert_eq!((app.indoor.pages[0], app.indoor.more[0]), (w, Some(m)));
    // LAYOUT on the indoor set
    let (indoor1, road1) = (app.indoor.pages[1], app.pages()[1]);
    app.pages_set = ride::Profile::Indoor;
    app.next_layout(&mut sim, 1);
    assert_ne!(app.indoor.pages[1], indoor1);
    assert_eq!(app.pages()[1], road1, "the road page untouched");
}

/// The indoor set and the road pages from one copy: a cut after every sector of a write that changes both.
#[test]
fn the_indoor_set_survives_a_cut_with_its_pages() {
    let w = |f| Page::preset(1, [f; 8]).unwrap();
    let change = |app: &mut App, sim: &mut Sim, f| {
        app.indoor.pages[1] = w(f);
        app.pages[0] = Page::preset(2, [f, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
        app.pages_changed(sim);
        run(sim, app, 200);
    };
    let (mut sim, mut app) = started();
    change(&mut app, &mut sim, Hr);
    let base = sim.emmc.sectors_written;
    change(&mut app, &mut sim, Cadence);
    let total = sim.emmc.sectors_written - base;
    assert!(total > 0);
    let (mut olds, mut news) = (0, 0);
    for cut in 0..total {
        let (mut sim, mut app) = started();
        change(&mut app, &mut sim, Hr);
        let backup = sim.backup;
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + cut);
        change(&mut app, &mut sim, Cadence);
        sim.backup = backup;
        let (_, app2) = reboot(&mut sim);
        let new = app2.indoor.pages[1] == w(Cadence);
        assert!(new || app2.indoor.pages[1] == w(Hr), "cut {cut}: {:?}", app2.indoor.pages[1]);
        assert_eq!(new, app2.pages()[0].fields[0] == Cadence, "cut {cut}: pages and indoor set from one copy");
        if new { news += 1 } else { olds += 1 }
    }
    std::println!("indoor cut: {total} sectors, {olds} old, {news} new");
    assert!(olds > 0);
}

/// An MTB ride shows the MTB pages (four, big fields); PAGES > MTB PAGES edits that set, which refuses a fifth page
/// and is kept in the file beside the indoor and road sets.
#[test]
fn an_mtb_ride_takes_the_mtb_pages() {
    let (mut sim, mut app) = started();
    assert_eq!(app.mtb, crate::page::Set::mtb_default(), "no file yet: the defaults");
    app.settings.profile = Some(crate::ride::Profile::Mtb);
    run(&mut sim, &mut app, 100);
    press(&mut sim, &mut app, Key::Page);
    assert_eq!((app.screen(), first_field(&app)), (Screen::Ride(0), Some(Timer)));
    sim.save("mtb_page1");
    super::tests::assert_consistent(&sim, &app);
    for (n, f) in [(1, AltGraph), (2, MaxSpeed), (3, Map)] {
        press(&mut sim, &mut app, Key::Page);
        assert_eq!((app.screen(), first_field(&app)), (Screen::Ride(n), Some(f)));
        sim.save(&std::format!("mtb_page{}", n + 1));
        super::tests::assert_consistent(&sim, &app);
    }
    press(&mut sim, &mut app, Key::Page);
    assert_eq!(app.screen(), Screen::Ride(0), "four pages, then round");
    // the menu: a third row, whose list is the MTB set; ADD PAGE refused at four
    app.settings.profile = None;
    run(&mut sim, &mut app, 100);
    app.press(&mut sim, Press::Long(Key::Page));
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::PageList(ride::Profile::Mtb)));
    press(&mut sim, &mut app, Key::Start);
    assert!(app.pages_set == ride::Profile::Mtb);
    assert_eq!(app.rows(Menu::PageList(ride::Profile::Mtb)), 1 + 4 + 1);
    to_row(&mut sim, &mut app, menu::Row::AddPage);
    press(&mut sim, &mut app, Key::Start);
    assert_eq!((app.notes.text.as_str(), app.mtb.count), ("4 PAGES AT MOST", 4));
    // an edit of page 1 reaches the MTB set only, and the file
    to_row(&mut sim, &mut app, menu::Row::Open(Menu::Page(0)));
    press(&mut sim, &mut app, Key::Start);
    app.leave_editor(&mut sim, false);
    app.mtb.pages[0] = Page::preset(1, [Cadence; 8]).unwrap();
    app.pages_changed(&mut sim);
    run(&mut sim, &mut app, 300);
    let f = file(&sim).unwrap();
    assert_eq!(f.mtb, Some(app.mtb));
    assert_eq!(f.indoor, Some(app.indoor));
    let (_, app2) = reboot(&mut sim);
    assert_eq!((app2.mtb, app2.indoor), (app.mtb, app.indoor));
}

/// On the MTB set: delete a page, then add until refused: never past four.
#[test]
fn the_mtb_set_never_passes_four_pages() {
    let (mut sim, mut app) = started();
    app.pages_set = ride::Profile::Mtb;
    app.edit_page(&mut sim, 0, 0);
    app.delete_page(&mut sim);
    assert_eq!(app.mtb.count, 3);
    for _ in 0..4 {
        app.add_page(&mut sim, 3);
        assert!(app.mtb.count <= 4);
    }
    assert_eq!((app.mtb.count, app.notes.text.as_str()), (4, "4 PAGES AT MOST"));
}

/// The MTB set and the road pages from one copy: a cut after every sector of a write that changes both.
#[test]
fn the_mtb_set_survives_a_cut_with_its_pages() {
    let w = |f| Page::preset(1, [f; 8]).unwrap();
    let change = |app: &mut App, sim: &mut Sim, f| {
        app.mtb.pages[1] = w(f);
        app.pages[0] = Page::preset(2, [f, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
        app.pages_changed(sim);
        run(sim, app, 200);
    };
    let (mut sim, mut app) = started();
    change(&mut app, &mut sim, Hr);
    let base = sim.emmc.sectors_written;
    change(&mut app, &mut sim, Cadence);
    let total = sim.emmc.sectors_written - base;
    assert!(total > 0);
    let (mut olds, mut news) = (0, 0);
    for cut in 0..total {
        let (mut sim, mut app) = started();
        change(&mut app, &mut sim, Hr);
        let backup = sim.backup;
        sim.emmc.cut_after = Some(sim.emmc.sectors_written + cut);
        change(&mut app, &mut sim, Cadence);
        sim.backup = backup;
        let (_, app2) = reboot(&mut sim);
        let new = app2.mtb.pages[1] == w(Cadence);
        assert!(new || app2.mtb.pages[1] == w(Hr), "cut {cut}: {:?}", app2.mtb.pages[1]);
        assert_eq!(new, app2.pages()[0].fields[0] == Cadence, "cut {cut}: pages and MTB set from one copy");
        assert_eq!(app2.indoor, crate::page::Set::indoor_default(), "cut {cut}: the indoor set untouched");
        if new { news += 1 } else { olds += 1 }
    }
    std::println!("mtb cut: {total} sectors, {olds} old, {news} new");
    assert!(olds > 0);
}

/// Register 16 is the power word now (tag 0xa in its top bits, the smooth font in bit 24), which the first builds'
/// one-register pages (tag 0xa, the layout in bits 24..27) kept page 3 in: with the FONT set to SMOOTH and the
/// registers of the pages empty, it was read as a third page (and the boot saved it as one). Never now.
#[test]
fn the_power_word_is_never_a_page() {
    let boot = |smooth: bool| {
        let mut sim = Sim::new(9, 0, 0);
        sim.emmc.image = crate::fat::tests::image();
        sim.own_settings();
        sim.backup[crate::POWER_BKP] = crate::Settings { smooth, ..crate::Settings::from_word(0) }.power_word();
        let mut app = App::start(&mut sim, None);
        run(&mut sim, &mut app, PAGES_LOAD_MS as u64 + 200);
        (pages(&app), sim.backup[PAGE_BKP[2][0]])
    };
    assert_eq!(boot(true), boot(false));
}

/// A page with nothing changing sends the panel nothing between seconds or across them (the boxes are only looked
/// at when a second ticked), and a new sensor reading draws at the second after it.
#[test]
fn idle_ride_page_sends_no_pixels_and_a_reading_draws_at_the_second() {
    let (mut sim, mut app) = started();
    app.screen = Screen::Ride(0);
    app.show(&mut sim);
    run(&mut sim, &mut app, 10000);
    // the strip blinks on its own; the page's boxes are what is measured, pass after pass
    let px = sim.pixels;
    for _ in 0..40 {
        app.update_page(&mut sim);
    }
    assert_eq!(sim.pixels, px, "nothing changed: nothing sent");
    app.live.set(crate::sensors::Metric::Speed, 251);
    app.live.snapshot();
    app.light.idle_ms = 60_000;
    app.value_due = false;
    let px = sim.pixels;
    app.update_page(&mut sim);
    assert_eq!(sim.pixels, px, "a new reading between seconds is held back");
    app.value_due = true; // the second's tick
    app.update_page(&mut sim);
    assert!(sim.pixels > px, "the reading is drawn at the second");
}

/// The editor draws the boxes of the page it edits, not another screen's (EDITED once shared the workout page's index).
#[test]
fn editor_draws_the_edited_pages_boxes() {
    let (mut sim, mut app) = started();
    press(&mut sim, &mut app, Key::Page);
    assert_eq!(app.screen(), Screen::Ride(0));
    let ride: Vec<_> = app.widgets().map(|(r, _)| r).collect();
    let (x, y) = centre(&app, 1);
    app.input(&mut sim, Input::Touch(Gesture::Hold(x, y)));
    assert_eq!(app.screen(), Screen::Edit(0, 1));
    let edited: Vec<_> = app.widgets().map(|(r, _)| r).collect();
    assert!(!edited.is_empty());
    assert_eq!(edited, ride, "the editor's boxes are ride page 0's");
}

/// A banner dismissed over a page with a graph in its lower half computes no graph bars: the boxes below the banner's
/// rows are skipped, not computed then clipped (P3, review 2026-10-29).
#[test]
fn unbanner_skips_boxes_outside_its_rows() {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(14, [Speed, Hr, Power, Cadence, Dist, Timer, Ascent, Graph]).unwrap();
    app.pages_changed(&mut sim);
    sim.gnss.fix = Some(500);
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 5000);
    app.bars.set(0);
    app.draw_screen(&mut crate::ui::tests::Frame::new());
    let whole = app.bars.replace(0);
    app.unbanner(&mut sim);
    let top = app.bars.get();
    std::println!("graph bars: whole screen {whole}, unbanner {top}");
    assert!(whole > 0 && top < whole, "{top} of {whole}");
}

/// The banner's redraw leaves the screen as the whole page drawn, on a page with a graph in its lower half.
#[test]
fn unbanner_equals_the_whole_draw() {
    let (mut sim, mut app) = started();
    app.pages[0] = Page::preset(14, [Speed, Hr, Power, Cadence, Dist, Timer, Ascent, Graph]).unwrap();
    app.pages_changed(&mut sim);
    sim.gnss.fix = Some(500);
    press(&mut sim, &mut app, Key::Start);
    run(&mut sim, &mut app, 5000);
    app.unbanner(&mut sim);
    super::tests::assert_consistent(&sim, &app);
}
