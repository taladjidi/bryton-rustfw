//! The ride pages as the rider makes them: the page editor, on the page itself (UI review 2, 5.2), and where the
//! pages are kept, `\Rustfw\pages.bin` (the format: page.rs [`page::Saved`]).
//!
//! **The editor** opens on a ride page from a hold on one of its boxes, from the ride menu's EDIT THIS PAGE (paused
//! only) or from PAGES > a page > EDIT ON PAGE; never while a ride records. The page shows at its real size with
//! live values, the selected box framed in orange, the keys' hint in the strip's place:
//!
//! ```text
//! +------------------------+
//! |  PAGE 2  START: MENU   |  the hint (orange), in the strip's place
//! +=========+--------------+
//! #KM/H     #|HR           |  the selected box: a 3 px orange frame
//! #  28.4   #|  142        |
//! ##########-+-------------+
//! |TIMER     |KM           |
//! |  1:02    |  21.84      |
//! +----------+-------------+
//! ```
//!
//! Page / Lap (or a tap) select a box, Start (or a tap on the selected one) opens its actions: FIELD (the picker by
//! groups), SPLIT with a side-by-side icon (a second cell in its row), SPLIT with a stacked icon (a row under it),
//! MERGE, TALLER, SHORTER, UNDO, then ADD PAGE and DELETE PAGE (asked first). Back leaves: at once when nothing
//! changed, and keeps them else (DROP CHANGES, last in the actions, asks and drops them). The editor works on a copy ([`EDITED`]); only keeping
//! it (or adding or deleting a page) changes the pages.
//!
//! **Storage.** The pages are written to `\Rustfw\pages.bin` when they change, between rides only (the recorder is
//! the card's one writer while a ride is open), two copies, the older one written over; then the first
//! [`REG_PAGES`] go to the backup registers in the format of earlier builds (the file first, so a cut between the two
//! leaves registers the file knows as its previous ones). At a start the registers give the pages at once; the
//! file is read [`PAGES_LOAD_MS`] later (or at the first screen other than the overview, the card not being touched at
//! boot) and wins, unless the registers differ from both sets of words its newest copy keeps: then another build
//! (an earlier one, or a read-only one, which keep their pages there only) changed them, and those pages win, the
//! others staying the file's. A build that may not write the card reads the file as any build does and waits for
//! it before an edit, but never writes it: its changes go to the registers (three pages, lossy).
//!
//! **A change the file must wait for** (made during a paused ride, with the editor still open after ADD PAGE, or
//! with the drive lent; or not written yet at a switch-off) goes to the registers at once, marked
//! [`PAGES_NEWER`] ([`App::hold_pages`]), so a switch-off, a reset or a fault before the file is written keeps it:
//! at the next start the mark makes the registers' pages win where they differ from the file's (even when they
//! equal its previous ones: a change undone), the file's others kept, and the file is written once no ride is open.
//! The registers keep three pages, lossily ([`Page::to_cfg`]): a change to pages 4 to 6, or a page added or deleted
//! before them, is lost if the unit goes off before the file is written (they come back as the file had them).
//! A MORE variant is not in the registers at all: a MORE edit made then is lost the same way, and where the registers
//! win over the file's page, that page's MORE goes back to its default (it may belong to another page).
//!
//! **Three sets** (UI review 3, 4.2.7): the ROAD pages above, the INDOOR set and the MTB set (four pages at most, the
//! editor refuses a fifth) ([`page::Set`], app/profile.rs), in the file only, after the MORE part, each with a CRC of
//! its own (page.rs). The editor, LAYOUT, the picker and PAGES work on the
//! set of the profile in force while a ride is open, else the one PAGES > ROAD, INDOOR or MTB PAGES opened (a hold on a ride page or
//! the ride menu's EDIT THIS PAGE take the set on screen). The registers never hold the indoor or MTB set: an edit of either
//! made while the file must wait (a paused ride, the drive lent) is lost to a switch-off, like pages 4 to 6.
//!
//! **No change before the file is read**: the editor, LAYOUT and the picker read it first, and refuse while it
//! cannot be ("USB DRIVE ON", "PAGES NOT READ"), so the registers' three pages are never written over the file's six.
//! A failed read is tried again ([`LOAD_TRIES`] times by the passes, then at each change asked for); a write never
//! goes out without the newest copy's number.

use crate::*;
use menu::EditRow;
use page::{Page, Saved};

/// The pages' file is read this long after the start (or at the first ride page or menu, if sooner).
pub const PAGES_LOAD_MS: u32 = 2000;

/// Failed reads of the file after which the passes stop trying (a page change still tries, once each).
const LOAD_TRIES: u8 = 3;

/// `\Rustfw\pages.bin`, read and to be written.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PageStore {
    /// Read, or known not to be there (no file, no volume). A failed read leaves it false: tried again, and no page
    /// changes until then (the next write needs the newest copy's number, and the pages beyond the registers' three).
    pub(crate) loaded: bool,
    /// Reads that failed (I/O), the passes' retries stopping at [`LOAD_TRIES`].
    pub(crate) tries: u8,
    /// The pages changed since the file was written.
    pub(crate) dirty: bool,
    /// The registers hold the changed pages already ([`App::hold_pages`]).
    pub(crate) held: bool,
    /// At the start, the registers said they were newer than the file ([`PAGES_NEWER`]).
    pub(crate) newer: bool,
    /// The newest copy's number.
    pub(crate) seq: u32,
    /// The registers of the first ride pages at the start, before the first start's import could write them
    /// ([`PAGES_NEWER`] taken out).
    pub(crate) boot: [u32; 2 * REG_PAGES],
    /// The file was read whole (1024 bytes): written over in place, one sector, no FAT sector touched. Else the next
    /// write creates or grows it, and a cut in an earlier one may have left the FAT copies different in a sector,
    /// so they are made equal first ([`fat::Fat::sync_fats_from`], spread over passes, as the scan's cache does).
    pub(crate) sound: bool,
    /// FAT sectors compared so far.
    pub(crate) sync: u32,
}

impl PageStore {
    /// As at a start: the registers read.
    pub(crate) fn at_boot(b: &mut impl Board) -> Self {
        let mut boot = [0; 2 * REG_PAGES];
        for (w, r) in boot.iter_mut().zip(PAGE_BKP.iter().take(REG_PAGES).flatten()) {
            *w = b.backup_read(*r);
        }
        let newer = boot[1] & PAGES_NEWER != 0;
        boot[1] &= !PAGES_NEWER;
        PageStore { boot, newer, ..Default::default() }
    }
}

/// The page editor's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Editing {
    /// The ride page edited.
    pub(crate) page: usize,
    /// It is the page's MORE variant ([`App::more_shown`] when it opened).
    pub(crate) more: bool,
    /// The page as edited, as it was, and before the last action (UNDO).
    pub(crate) work: Page,
    pub(crate) orig: Page,
    pub(crate) undo: Option<Page>,
}

impl App {
    /// Ride page `n` (`n` < [`App::page_count`]), the overview, or the editor's copy ([`EDITED`]).
    pub(crate) fn page_ref(&self, n: usize) -> Option<&Page> {
        match n {
            EDITED => self.edit.as_ref().map(|e| &e.work),
            OVERVIEW => self.pages.get(OVERVIEW),
            n => {
                let (pages, count, _) = self.set_ref(self.view_set());
                pages.get(n).filter(|_| n < count)
            }
        }
    }

    /// How many ride pages the set PAGES and the editor show has.
    pub(crate) fn view_count(&self) -> usize {
        self.set_ref(self.view_set()).1
    }

    /// The first ride pages and the overview written to their backup registers, marked [`PAGES_NEWER`] while the
    /// file is behind them.
    pub(crate) fn save_page_registers(&self, b: &mut impl Board) {
        let mut words = register_words(&self.pages, self.page_count);
        if self.store.dirty {
            words[1] |= PAGES_NEWER;
        }
        for (r, w) in PAGE_BKP.iter().take(REG_PAGES).flatten().zip(words) {
            b.backup_write(*r, w);
        }
        save_page(b, OVERVIEW, &self.pages[OVERVIEW]);
    }

    /// The ride pages changed: written to the file between rides ([`App::step_pages`]), or at once to the registers
    /// by a build that may not write the card. A graph's plot chosen by a tap goes (its box may have moved).
    pub(crate) fn pages_changed(&mut self, b: &mut impl Board) {
        self.plot_taps = [[None; page::CELLS]; PAGES + 1];
        if self.card_writable {
            (self.store.dirty, self.store.held) = (true, false);
        } else {
            self.save_page_registers(b);
        }
    }

    /// The file can be written in the next passes: read, not lent, no ride open, the editor closed.
    fn pages_writable_soon(&self) -> bool {
        self.store.loaded && !self.emmc.lent() && self.edit.is_none() && self.ride_settled()
    }

    /// Changed pages the file does not have yet go to the registers now, marked [`PAGES_NEWER`]: lossy (the first
    /// three, as [`Page::to_cfg`] keeps them), but kept through a switch-off, a reset or a flat battery's restart
    /// (the module doc: which pages win at the next start). Called when the file must wait (a ride, the editor, the
    /// drive) and at a switch-off.
    pub(crate) fn hold_pages(&mut self, b: &mut impl Board) {
        if self.card_writable && self.store.dirty && !self.store.held {
            self.save_page_registers(b);
            self.store.held = true;
        }
    }

    /// The pages' file read when it is time, written when the pages changed and no ride is open. A build that may not
    /// write the card reads it too: the registers keep a page of a layout earlier builds lack as their nearest one, so
    /// without the file it would show another layout (2026-10-27, a RAM build showed the owner's page 1 as 2X3).
    #[inline(never)]
    pub(crate) fn step_pages(&mut self, b: &mut impl Board) {
        if !self.pages_writable_soon() {
            self.hold_pages(b);
        }
        if self.emmc.lent() {
            return;
        }
        let due = self.screen != Screen::Main || self.gps.ms >= PAGES_LOAD_MS || self.store.dirty;
        if !self.store.loaded && self.store.tries < LOAD_TRIES && due {
            self.load_pages(b);
        }
        if self.card_writable && self.store.dirty && self.pages_writable_soon() {
            if !self.store.sound {
                match Self::sync_fats_step(&mut self.emmc, b, &mut self.store.sync) {
                    Some(false) => return, // the FAT copies compared over the next passes first
                    Some(true) => self.fats_equal(),
                    None => {}
                }
            }
            self.save_pages(b);
        }
    }

    /// FAT sectors of both copies compared from `*at` ([`App::SYNC_SECTORS`] a step) until the pass has spent
    /// [`App::SCAN_IDLE_US`]: Some(true) once all are, Some(false) to go on at the next pass, None when it cannot be
    /// (the write then says why).
    #[inline(never)]
    pub(crate) fn sync_fats_step(emmc: &mut emmc::Emmc, b: &mut impl Board, at: &mut u32) -> Option<bool> {
        let t0 = b.awake_us();
        let mut d = emmc::Volume(emmc, b);
        let mut fat = fat::Fat::mount(&mut d).ok()?;
        for _ in 0..fat.fat_sectors() / Self::SYNC_SECTORS + 1 {
            *at = fat.sync_fats_from(&mut d, *at, Self::SYNC_SECTORS).ok()?.0;
            if *at >= fat.fat_sectors() {
                return Some(true);
            }
            if d.1.awake_us().wrapping_sub(t0) >= Self::SCAN_IDLE_US {
                return Some(false);
            }
        }
        Some(false)
    }

    /// Both FAT copies compared equal: neither file's next write compares them again.
    pub(crate) fn fats_equal(&mut self) {
        (self.store.sound, self.settings_store.sound, self.fats_suspect) = (true, true, false);
    }

    /// A write failed (a FAT sector may differ between the copies): both files' next writes compare them first.
    pub(crate) fn fats_unsure(&mut self) {
        (self.store.sound, self.store.sync) = (false, 0);
        (self.settings_store.sound, self.settings_store.sync) = (false, 0);
        self.fats_suspect = true; // and a file read whole later does not say they are equal
    }

    /// `\Rustfw\<name>`, a file of two 512-byte copies (pages.bin, settings.bin), read into `file`: Ok(1024) when it
    /// is whole; NotFound when it is not there or not that size.
    pub(crate) fn read_copies(
        &mut self,
        b: &mut impl Board,
        name: &str,
        file: &mut [u8; 1024],
    ) -> Result<usize, fat::Error> {
        let mut d = emmc::Volume(&mut self.emmc, b);
        let mut fat = fat::Fat::mount(&mut d)?;
        let dir = fat.open(&mut d, recorder::DIR)?;
        let f = fat.open_in(&mut d, dir, name)?;
        if f.dir || f.size != 1024 {
            return Err(fat::Error::NotFound);
        }
        fat.read_at(&mut d, &f, 0, file)
    }

    /// Copy `slot` (0 or 1) of `\Rustfw\<name>` written with `sector`, the other untouched; the file created (or
    /// grown) with both, the other zeros, when it is not whole ([`write_copy_in`]). None: no volume.
    pub(crate) fn write_copy(
        &mut self,
        b: &mut impl Board,
        name: &str,
        slot: u32,
        sector: &[u8; 512],
    ) -> Option<Result<(), fat::Error>> {
        let t = fat::write::dos_time(self.rec_time().0);
        let mut d = emmc::Volume(&mut self.emmc, b);
        let mut fat = fat::Fat::mount(&mut d).ok()?;
        Some(
            fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)
                .and_then(|dir| write_copy_in(&mut fat, &mut d, dir, name, slot, sector, t)),
        )
    }

    /// The file read now if it was not yet: before any change to the pages, so a change made in the first seconds
    /// is made to the file's pages, not to the registers' copy (which would then be written over the file's).
    pub(crate) fn ensure_pages(&mut self, b: &mut impl Board) {
        if !self.emmc.lent() && !self.store.loaded {
            self.load_pages(b);
        }
    }

    /// The pages may be changed: the file read first ([`App::ensure_pages`]); else why not, said.
    pub(crate) fn pages_ready(&mut self, b: &mut impl Board) -> bool {
        self.ensure_pages(b);
        let why = match () {
            _ if self.store.loaded => return true,
            _ if self.emmc.lent() => "USB DRIVE ON",
            _ => "PAGES NOT READ",
        };
        self.say(why);
        false
    }

    /// Reads `\Rustfw\pages.bin` (module doc: which pages win).
    #[inline(never)] // its 1 KB buffer in its own frame
    fn load_pages(&mut self, b: &mut impl Board) {
        let mut file = [0u8; 1024];
        let read = self.read_copies(b, page::FILE, &mut file);
        if read == Err(fat::Error::Io) {
            self.store.tries = self.store.tries.saturating_add(1);
            return; // tried again: a write now would not know the newest copy's number
        }
        self.store.loaded = true;
        self.store.sound = read == Ok(1024) && !self.fats_suspect;
        let Some(s) = read.ok().filter(|&n| n == 1024).and_then(|_| Saved::newest(&file)) else {
            self.store.dirty |= self.store.newer; // no file: the registers' pages written as the first
            return;
        };
        self.store.seq = s.seq;
        let boot = self.store.boot;
        let mut list: [Option<Page>; PAGES] = core::array::from_fn(|i| (i < s.count).then_some(s.pages[i]));
        let mut more_in = s.more;
        if self.store.newer || (boot != s.words && boot != s.was) {
            // changes the file did not get (a switch-off before it was written), or another build changed the
            // registers since: the registers' pages win where they differ from the file's
            for (i, (now, then)) in boot.chunks_exact(2).zip(s.words.chunks_exact(2)).enumerate() {
                let w = [now[0], now[1]];
                if now != then
                    && let Some(c) = PageCfg::from_words(w)
                    && let Some(slot) = list.get_mut(i)
                {
                    let page = Page::from_cfg(c);
                    if *slot != page
                        && let Some(m) = more_in.get_mut(i)
                    {
                        *m = None; // not the page the file's MORE was made for (one deleted or changed)
                    }
                    *slot = page;
                }
            }
            self.store.dirty = true;
        } else if boot != s.words {
            self.save_page_registers_from(b, &s); // cut between the file and the registers: put right
        }
        self.indoor = s.indoor.unwrap_or_else(page::Set::indoor_default);
        self.mtb = s.mtb.unwrap_or_else(page::Set::mtb_default);
        self.set_shown = self.ride_profile(); // the pages on screen are drawn below
        self.plot_taps = [[None; page::CELLS]; PAGES + 1];
        let mut n = 0;
        let mut more = [None; PAGES];
        for (p, m) in list.into_iter().zip(more_in) {
            if let (Some(p), Some(slot), Some(ms)) = (p, self.pages.get_mut(n), more.get_mut(n)) {
                (*slot, *ms) = (p, m);
                n += 1;
            }
        }
        if n == 0 {
            return;
        }
        self.more = more;
        self.page_count = n;
        self.plot_taps = [[None; page::CELLS]; PAGES + 1];
        if let Screen::Ride(k) = self.screen {
            if k < PAGES && !self.page_on(k) {
                self.screen = Screen::Main;
            }
            self.show(b);
        }
    }

    /// The registers as the file's newest copy says they were written.
    fn save_page_registers_from(&self, b: &mut impl Board, s: &Saved) {
        for (r, w) in PAGE_BKP.iter().take(REG_PAGES).flatten().zip(s.words) {
            b.backup_write(*r, w);
        }
    }

    /// Writes the pages over the file's older copy (created with both if needed), then the registers.
    #[inline(never)] // its 1 KB buffer in its own frame
    fn save_pages(&mut self, b: &mut impl Board) {
        self.store.dirty = false; // once: a failure is said, and tried again at the next change
        let mut was = [0; 2 * REG_PAGES];
        for (w, r) in was.iter_mut().zip(PAGE_BKP.iter().take(REG_PAGES).flatten()) {
            *w = b.backup_read(*r);
        }
        was[1] &= !PAGES_NEWER;
        let s = Saved {
            seq: self.store.seq.wrapping_add(1),
            pages: core::array::from_fn(|i| self.pages[i]),
            count: self.page_count,
            words: register_words(&self.pages, self.page_count),
            was,
            more: self.more,
            indoor: Some(self.indoor),
            mtb: Some(self.mtb),
        };
        // no volume (a card stock never formatted, the tests' blank one): the registers only, nothing to say
        let Some(r) = self.write_copy(b, page::FILE, s.seq % 2, &s.to_sector()) else {
            return self.save_page_registers(b);
        };
        match r {
            Ok(()) => (self.store.seq, self.store.sound) = (s.seq, true),
            Err(_) => {
                self.fats_unsure(); // compared again before the next write
                self.notice("PAGES NOT SAVED");
            }
        }
        self.save_page_registers(b); // after the file (module doc), and also when it failed: three pages kept
    }

    // -----------------------------------------------------------------------------------------------------------
    // The editor
    // -----------------------------------------------------------------------------------------------------------

    /// Opens the editor on ride page `n`, box `k` selected; refused while a ride records.
    pub(crate) fn edit_page(&mut self, b: &mut impl Board, n: usize, k: usize) {
        if self.ride.state() == ride::State::Running {
            return self.say("PAUSE TO EDIT");
        }
        if !self.pages_ready(b) {
            return;
        }
        let Some(&p) = self.page_ref(n).filter(|_| n < PAGES) else { return };
        let more = self.more_shown() == Some(n);
        let p = if more { self.more_page(n) } else { p };
        self.edit = Some(Editing { page: n, more, work: p, orig: p, undo: None });
        self.open(Screen::Edit(n, k.min(p.cells().saturating_sub(1))));
    }

    /// Leaves the editor (and the menus over it), keeping its work or not, back to where it was opened from.
    pub(crate) fn leave_editor(&mut self, b: &mut impl Board, keep: bool) {
        let edit = self.edit.take();
        if let Some(e) = edit.filter(|e| keep && e.work != e.orig) {
            self.keep_edit(&e);
            self.pages_changed(b);
        }
        for _ in 0..self.under.len() {
            let was_editor = matches!(self.screen, Screen::Edit(..));
            self.close();
            if was_editor || self.depth == 0 {
                break;
            }
        }
        if let Some(e) = edit.filter(|e| e.more && self.screen == Screen::Ride(e.page)) {
            self.more_on = Some(e.page); // back on the MORE variant it was opened on
        }
        self.show(b);
    }

    /// An input in the editor (module doc).
    pub(crate) fn editor_input(&mut self, b: &mut impl Board, i: Input) {
        let (Screen::Edit(_, sel), Some(e)) = (self.screen, self.edit) else {
            return self.leave_editor(b, false);
        };
        let cells = e.work.cells().max(1);
        let back = matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)));
        match i {
            // Back keeps what was done (nothing is lost: it never means "drop"); DROP CHANGES in the actions does
            _ if back => {
                self.leave_editor(b, true);
                if e.work != e.orig {
                    self.say("CHANGES KEPT");
                }
            }
            Input::Next | Input::NextHeld => {
                self.repeated |= i == Input::NextHeld;
                self.select_cell(b, (sel + 1) % cells);
            }
            Input::Prev | Input::PrevHeld => {
                self.repeated |= i == Input::PrevHeld;
                self.select_cell(b, (sel + cells - 1) % cells);
            }
            Input::Select => self.open_actions(b, sel),
            Input::Touch(Gesture::Tap(x, y)) => match self.widgets().position(|(bx, _)| inside(bx, x, y)) {
                Some(k) if k == sel => self.open_actions(b, sel),
                Some(k) => self.select_cell(b, k),
                None => {}
            },
            _ => {}
        }
    }

    /// The actions on box `k`.
    fn open_actions(&mut self, b: &mut impl Board, k: usize) {
        self.open(Screen::Menu(Menu::Edit(k), 0));
        self.show(b);
    }

    /// Box `k` selected: the old one drawn again without its frame, the new one framed.
    fn select_cell(&mut self, b: &mut impl Board, k: usize) {
        let Screen::Edit(n, old) = self.screen else { return };
        if k == old {
            return;
        }
        self.screen = Screen::Edit(n, k);
        let area = self.area();
        if let Some((bx, w)) = self.widgets().nth(old) {
            let (x, y, wd, h) = ui::box_inside(bx);
            let mut p = Panel(b, area, self.look());
            p.fill(x, y, wd, h, ui::BLACK);
            self.draw_widget(&mut p, old, bx, w);
        }
        self.draw_edit_frame(&mut Panel(b, area, self.look()));
    }

    /// In the editor, the frame round the selected box.
    pub(crate) fn draw_edit_frame(&self, c: &mut impl Canvas) {
        if let Screen::Edit(_, k) = self.screen
            && let Some((bx, _)) = self.widgets().nth(k)
        {
            ui::draw_cell_frame(c, bx, ui::ACCENT);
        }
    }

    /// The strip, or the editor's hint in its place ([`App::hint_shown`]).
    pub(crate) fn draw_top(&self, c: &mut impl Canvas) {
        if self.hint_shown {
            let mut l = Line::<24>::new();
            l.push_str(if self.edit.is_some_and(|e| e.more) { "MORE " } else { "PAGE " });
            l.push_u32(self.edit.map_or(0, |e| e.page) as u32 + 1);
            l.push_str("  START: MENU");
            ui::draw_edit_hint(c, l.as_str());
        } else {
            ui::draw_strip_whole(c, &self.strip_shown);
        }
    }

    /// A field for a new cell: the first of the essentials the page does not show yet.
    fn new_field(p: &Page) -> Field {
        use Field::*;
        let shown = p.fields.get(..p.cells()).unwrap_or(&[]);
        [Speed, Timer, Dist, Hr, Power, Cadence, AvgSpeed, Ascent]
            .into_iter()
            .find(|f| !shown.contains(f))
            .unwrap_or(Time)
    }

    /// An editor action on box `k` (row `sel` of `menu`, for DELETE PAGE's confirmation).
    pub(crate) fn edit_action(&mut self, b: &mut impl Board, menu: Menu, sel: usize, k: usize, r: EditRow) {
        let Some(e) = self.edit else { return };
        let w = e.work;
        let done = match r {
            EditRow::Field => {
                let g = w.fields.get(k).map_or(0, |f| f.group());
                return self.open(Screen::Menu(Menu::Fields(EDITED, k), g));
            }
            EditRow::SplitAcross => w.split_across(k, Self::new_field(&w)).map(|p| (p, k)),
            EditRow::SplitDown => w.split_down(k, Self::new_field(&w)).map(|p| (p, k)),
            EditRow::Merge => w.merge(k),
            EditRow::Taller => w.resize(k, true).map(|p| (p, k)),
            EditRow::Shorter => w.resize(k, false).map(|p| (p, k)),
            EditRow::Undo => e.undo.map(|u| (u, k)).ok_or("NOTHING TO UNDO"),
            EditRow::AddPage => return self.add_page(b, e.page + 1),
            EditRow::Drop => {
                if self.confirmed(menu, sel, "DROP CHANGES") {
                    self.leave_editor(b, false);
                    self.say("CHANGES DROPPED");
                }
                return;
            }
            EditRow::DeletePage if e.more => {
                // in MORE this row resets the variant to its default; the page stays
                if self.confirmed(menu, sel, "RESET THIS MORE") {
                    let which = self.view_set();
                    if let Some(m) = self.set_mut(which).2.get_mut(e.page) {
                        *m = None;
                    }
                    self.pages_changed(b);
                    self.leave_editor(b, false);
                    self.say("MORE RESET");
                }
                return;
            }
            EditRow::DeletePage if self.view_count() <= 1 => Err("ONE PAGE AT LEAST"),
            EditRow::DeletePage => {
                if self.confirmed(menu, sel, "DELETE THIS PAGE") {
                    self.delete_page(b);
                }
                return;
            }
        };
        match done {
            Ok((p, k)) => {
                if let Some(e) = &mut self.edit {
                    (e.undo, e.work) = (Some(w), p);
                }
                self.close(); // back to the page, the result shown
                self.screen = Screen::Edit(e.page, k.min(p.cells().saturating_sub(1)));
            }
            Err(why) => self.say(why),
        }
    }

    /// A page added at `at` (past the last: after it; 2 x 2: speed, timer, distance, heart rate) and the editor opened on it; the editor's
    /// work on the page it was on kept first.
    pub(crate) fn add_page(&mut self, b: &mut impl Board, at: usize) {
        if !self.pages_ready(b) {
            return;
        }
        let which = self.view_set();
        if self.view_count() >= Self::set_cap(which) {
            return self.say(if which == ride::Profile::Mtb { "4 PAGES AT MOST" } else { "6 PAGES AT MOST" });
        }
        if let Some(e) = self.edit.take() {
            if e.work != e.orig {
                self.keep_edit(&e);
            }
            // out of the actions and the old page's editor, to the screen it was opened from
            while self.depth > 0 && !matches!(self.screen, Screen::Edit(..)) {
                self.close();
            }
            self.close();
        }
        use Field::*;
        let new = Page::preset(4, [Speed, Timer, Dist, Hr, Time, Time, Time, Time]).unwrap_or(BLANK);
        let (pages, count, more) = self.set_mut(which);
        let at = at.min(*count);
        pages.copy_within(at..PAGES - 1, at + 1);
        more.copy_within(at..PAGES - 1, at + 1);
        if let (Some(p), Some(m)) = (pages.get_mut(at), more.get_mut(at)) {
            (*p, *m) = (new, None);
        }
        *count += 1;
        self.pages_changed(b);
        let mut l = Line::<20>::new();
        l.push_str("PAGE ");
        l.push_u32(at as u32 + 1);
        l.push_str(" ADDED");
        self.say(l.as_str());
        self.edit_page(b, at, 0);
    }

    /// The edited page deleted, out of the editor.
    pub(crate) fn delete_page(&mut self, b: &mut impl Board) {
        let Some(e) = self.edit.take() else { return };
        let which = self.view_set();
        let (pages, count, more) = self.set_mut(which);
        if e.page < *count && *count > 1 {
            pages.copy_within(e.page + 1..PAGES, e.page);
            more.copy_within(e.page + 1..PAGES, e.page);
            more[PAGES - 1] = None;
            *count -= 1;
            self.pages_changed(b);
        }
        self.leave_editor(b, false);
        self.say("PAGE DELETED");
    }

    /// The menu's LAYOUT: page `n` (`n` < [`PAGES`]) to the next layout every field of it fits, its fields in order.
    pub(crate) fn next_layout(&mut self, b: &mut impl Board, n: usize) {
        if !self.pages_ready(b) {
            return;
        }
        let Some(&p) = self.page_ref(n).filter(|_| n < PAGES) else { return };
        let order = ui::LAYOUT_ORDER;
        let at = p.preset_id().and_then(|id| order.iter().position(|&l| l == id)).unwrap_or(order.len() - 1);
        let next = (1..=order.len())
            .filter_map(|d| Page::preset(order[(at + d) % order.len()], p.fields))
            .find(|q| q.misfits() == 0);
        let which = self.view_set();
        if let (Some(q), Some(slot)) = (next, self.set_mut(which).0.get_mut(n)) {
            *slot = q;
            self.pages_changed(b);
        }
    }

    /// The picker's field `f` for box `k` of page `n`: the editor's copy, a ride page (if it is readable there) or
    /// the overview; then back to the editor or the page's menu.
    pub(crate) fn pick(&mut self, b: &mut impl Board, n: usize, k: usize, f: Field) {
        if n < PAGES && !self.pages_ready(b) {
            return;
        }
        let page = match n {
            EDITED => self.edit.map(|e| e.work),
            _ => self.page_ref(n).copied(),
        };
        let Some(p) = page else { return };
        let changed = if n == OVERVIEW {
            let mut q = p;
            q.fields.get_mut(k).map(|x| *x = f).map(|_| q).ok_or("NO BOX")
        } else {
            p.with_field(k, f)
        };
        let q = match changed {
            Ok(q) => q,
            Err(why) => return self.say(why),
        };
        match n {
            EDITED => {
                if let Some(e) = &mut self.edit {
                    (e.undo, e.work) = (Some(p), q);
                }
            }
            OVERVIEW => {
                self.pages[OVERVIEW] = q;
                save_page(b, OVERVIEW, &q);
            }
            _ => {
                let which = self.view_set();
                if let Some(slot) = self.set_mut(which).0.get_mut(n) {
                    *slot = q;
                }
                self.pages_changed(b);
            }
        }
        self.close(); // the groups
        self.close(); // the page's menu, its row showing the new field, or the editor's actions
        if n == EDITED {
            self.close(); // the editor, showing it
        }
    }

    /// An editor row's value: the field, UNDO greyed with nothing to undo, the page count.
    pub(crate) fn edit_row_value(&self, menu: Menu, r: EditRow, v: &mut Line<12>, color: &mut u16) {
        let (Menu::Edit(k), Some(e)) = (menu, self.edit) else { return };
        match r {
            EditRow::Field => v.push_str(e.work.fields.get(k).map_or("", |f| f.label())),
            EditRow::Undo if e.undo.is_none() => {
                v.push_str("NONE");
                *color = ui::GREY;
            }
            EditRow::AddPage => {
                v.push_u32(self.view_count() as u32);
                v.push_str("/");
                v.push_u32(PAGES as u32);
            }
            _ => {}
        }
    }
}

/// Copy `slot` (0 or 1) of file `name` in `dir`, a file of two copies of `copy.len()` bytes (whole sectors: pages.bin,
/// settings.bin, a segment's `.pr`), written with `copy`, the other untouched; the file created (or made whole) with
/// both, the other zeros, when it is not twice that size. Its size is written last, so a reader (which takes only a
/// whole file) sees the file as it was until then; over a whole file only the copy's own sectors are written.
pub(crate) fn write_copy_in(
    fat: &mut fat::Fat,
    d: &mut impl fat::DiskWrite,
    dir: fat::File,
    name: &str,
    slot: u32,
    copy: &[u8],
    t: (u16, u16),
) -> Result<(), fat::Error> {
    let k = u32::try_from(copy.len() / 512).map_err(|_| fat::Error::Corrupt)?;
    let whole = 2 * k * 512;
    let mut h = match fat.locate(d, dir, name) {
        Err(fat::Error::NotFound) => fat.create(d, dir, name, t)?,
        other => other?,
    };
    let (mine, other) = if slot.is_multiple_of(2) { (0, k) } else { (k, 0) };
    fat.write_sectors(d, &mut h, mine, copy)?;
    if h.size == whole {
        return Ok(()); // the other copy untouched
    }
    for s in 0..k {
        fat.write_sectors(d, &mut h, other + s, &[0u8; 512])?;
    }
    fat.set_size(d, &mut h, whole, t)
}
