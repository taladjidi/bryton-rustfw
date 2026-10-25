//! The page editor and `\Rustfw\pages.bin` (app/editor.rs, page.rs) on the Sim: the editor by keys and by touch, on
//! the page at its real size, keep or drop, undo, pages added and deleted and their round, never while recording;
//! the file written, read back, damaged, cut at every sector, and the backup registers beside it.

use super::*;
use crate::fat::tests::{Ram, read_all};
use crate::page::{Page, Saved};
use crate::ui::Field::{self, *};

fn run(sim: &mut Sim, app: &mut App, ms: u64) {
    let end = sim.ms + ms;
    while sim.ms < end {
        app.step(sim);
        sim.delay_ms(POLL_MS);
    }
}

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
/// a second tap opens the actions; SPLIT down adds a row; SPLIT across in a full row says why not; Back asks,
/// Start keeps; the file is written between rides, with the registers after it, and read back at the next start.
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
    assert!(app.banner.as_str().contains("2 A ROW"), "{}", app.banner.as_str());
    assert_eq!(app.screen(), Screen::Menu(Menu::Edit(3), 1), "refused: the actions stay");
    super::tests::back(&mut sim, &mut app);
    assert!(app.file_untouched_since_start(&sim), "nothing written while editing");
    press(&mut sim, &mut app, Key::Back);
    assert_eq!(app.screen(), Screen::Edit(0, 3), "Back asks first");
    assert!(app.banner.as_str().starts_with("KEEP CHANGES?"));
    press(&mut sim, &mut app, Key::Start);
    assert_eq!(app.screen(), Screen::Ride(0), "kept, back on the page");
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

/// MERGE, UNDO, MERGE again, then Back and Back: dropped, the page as it was, nothing written.
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
    press(&mut sim, &mut app, Key::Back);
    press(&mut sim, &mut app, Key::Back); // drop
    assert_eq!((app.screen(), app.pages()[1]), (Screen::Ride(1), before));
    run(&mut sim, &mut app, 500);
    assert_eq!(file(&sim), None, "nothing to write");
    super::tests::assert_consistent(&sim, &app);
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
    assert_eq!(app.banner.as_str(), "PAUSE TO EDIT");
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
    assert_eq!((app.page_count(), app.banner.as_str()), (6, "6 PAGES AT MOST"));
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
    assert_eq!((app.page_count(), app.banner.as_str()), (1, "ONE PAGE AT LEAST"));
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
/// keeps its pages in the registers only and never touches the card for them.
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

    // read only: the registers at once, the card never touched
    let mut sim = Sim::new(9, 0, 0);
    sim.emmc.image = crate::fat::tests::image();
    (sim.writable, sim.backup) = (false, other);
    let mut app = App::start(&mut sim, None);
    run(&mut sim, &mut app, 3000);
    app.pages[0] = p4;
    app.pages_changed(&mut sim);
    assert_eq!(PageCfg::from_words([sim.backup[20], sim.backup[21]]), Some(p4.to_cfg()));
    run(&mut sim, &mut app, 3000);
    assert!(sim.emmc.cmds.is_empty(), "{:?}", &sim.emmc.cmds[..4.min(sim.emmc.cmds.len())]);
}

/// A power cut after every sector of a write of the pages, the first (the file created) and a later one (a copy
/// over the older): the volume stays clean for fsck, and the next start reads the old pages or the new, never a mix.
/// The registers are as the cut left them: as before the write (they come after it), the CPU dying with the card.
/// A cut tears no sector here; on a card with reliable writes, the sector being written is torn too (sim.rs).
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
            sim.emmc.tear = reliable;
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
    assert_eq!((app.banner.as_str(), app.pages()[0]), ("USB DRIVE ON", before), "refused");
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
    let s = Saved { seq: 3, pages: [tiny; PAGES], count: 1, words: [0; 6], was: [0; 6] };
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
