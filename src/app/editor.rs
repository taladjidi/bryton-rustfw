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
//! changed, else "KEEP CHANGES?" (Start keeps, Back drops). The editor works on a copy ([`EDITED`]); only keeping
//! it (or adding or deleting a page) changes the pages.
//!
//! **Storage.** The pages are written to `\Rustfw\pages.bin` when they change, between rides only (the recorder is
//! the card's one writer while a ride is open), two copies, the older one written over; then the first
//! [`REG_PAGES`] go to the backup registers in the format of earlier builds (the file first, so a cut between the two
//! leaves registers the file knows as its previous ones). At a start the registers give the pages at once; the
//! file is read [`PAGES_LOAD_MS`] later (or at the first screen other than the overview, the card not being touched at
//! boot) and wins, unless the registers differ from both sets of words its newest copy keeps: then another build
//! (an earlier one, or a read-only one, which keep their pages there only) changed them, and those pages win, the
//! others staying the file's. A build that may not write the card never reads or writes the file: its pages are
//! the registers' (three at most).

use crate::*;
use menu::EditRow;
use page::{Page, Saved};

/// The pages' file is read this long after the start (or at the first ride page or menu, if sooner).
pub const PAGES_LOAD_MS: u32 = 2000;

/// `\Rustfw\pages.bin`, read and to be written.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PageStore {
    /// Read (or tried).
    pub(crate) loaded: bool,
    /// The pages changed since the file was written.
    pub(crate) dirty: bool,
    /// The newest copy's number.
    pub(crate) seq: u32,
    /// The registers of the first ride pages at the start, before the first start's import could write them.
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
        PageStore { boot, ..Default::default() }
    }
}

/// The page editor's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Editing {
    /// The ride page edited.
    pub(crate) page: usize,
    /// The page as edited, as it was, and before the last action (UNDO).
    pub(crate) work: Page,
    pub(crate) orig: Page,
    pub(crate) undo: Option<Page>,
    /// Back asked "KEEP CHANGES?": Start keeps, Back drops.
    pub(crate) asking: bool,
}

impl App {
    /// Ride page `n` (`n` < [`App::page_count`]), the overview, or the editor's copy ([`EDITED`]).
    pub(crate) fn page_ref(&self, n: usize) -> Option<&Page> {
        match n {
            EDITED => self.edit.as_ref().map(|e| &e.work),
            OVERVIEW => self.pages.get(OVERVIEW),
            n if n < self.page_count => self.pages.get(n),
            _ => None,
        }
    }

    /// The first ride pages and the overview written to their backup registers.
    pub(crate) fn save_page_registers(&self, b: &mut impl Board) {
        let words = register_words(&self.pages, self.page_count);
        for (r, w) in PAGE_BKP.iter().take(REG_PAGES).flatten().zip(words) {
            b.backup_write(*r, w);
        }
        save_page(b, OVERVIEW, &self.pages[OVERVIEW]);
    }

    /// The ride pages changed: written to the file between rides ([`App::step_pages`]), or at once to the registers
    /// by a build that may not write the card.
    pub(crate) fn pages_changed(&mut self, b: &mut impl Board) {
        if self.card_writable {
            self.store.dirty = true;
        } else {
            self.save_page_registers(b);
        }
    }

    /// The pages' file read when it is time, written when the pages changed and no ride is open.
    #[inline(never)]
    pub(crate) fn step_pages(&mut self, b: &mut impl Board) {
        if !self.card_writable || self.emmc.lent() {
            return;
        }
        let due = self.screen != Screen::Main || self.gnss_ms >= PAGES_LOAD_MS || self.store.dirty;
        if !self.store.loaded && due {
            self.load_pages(b);
        }
        if self.store.loaded && self.store.dirty && self.edit.is_none() && self.ride_settled() {
            if !self.store.sound && !self.sync_fats_step(b) {
                return; // the FAT copies compared over the next passes first
            }
            self.save_pages(b);
        }
    }

    /// FAT sectors of both copies compared ([`App::SYNC_SECTORS`] a step) until the pass has spent
    /// [`App::SCAN_IDLE_US`]; true once done (or when it cannot be: the write then says why).
    #[inline(never)]
    fn sync_fats_step(&mut self, b: &mut impl Board) -> bool {
        let t0 = b.awake_us();
        let mut d = emmc::Volume(&mut self.emmc, b);
        let Ok(mut fat) = fat::Fat::mount(&mut d) else { return true };
        for _ in 0..fat.fat_sectors() / Self::SYNC_SECTORS + 1 {
            match fat.sync_fats_from(&mut d, self.store.sync, Self::SYNC_SECTORS) {
                Ok((next, _)) => self.store.sync = next,
                Err(_) => return true,
            }
            if self.store.sync >= fat.fat_sectors() {
                self.store.sound = true;
                return true;
            }
            if d.1.awake_us().wrapping_sub(t0) >= Self::SCAN_IDLE_US {
                return false;
            }
        }
        false
    }

    /// Reads `\Rustfw\pages.bin` (module doc: which pages win).
    #[inline(never)] // its 1 KB buffer in its own frame
    fn load_pages(&mut self, b: &mut impl Board) {
        self.store.loaded = true;
        let mut file = [0u8; 1024];
        let mut d = emmc::Volume(&mut self.emmc, b);
        let read = fat::Fat::mount(&mut d).and_then(|mut fat| {
            let dir = fat.open(&mut d, recorder::DIR)?;
            let f = fat.open_in(&mut d, dir, page::FILE)?;
            if f.dir || f.size != 1024 {
                return Err(fat::Error::NotFound);
            }
            fat.read_at(&mut d, &f, 0, &mut file)
        });
        self.store.sound = read == Ok(1024);
        let Some(s) = read.ok().filter(|&n| n == 1024).and_then(|_| Saved::newest(&file)) else { return };
        self.store.seq = s.seq;
        if self.store.dirty {
            return; // changed before the file was read: those changes win, written next
        }
        let boot = self.store.boot;
        let mut list: [Option<Page>; PAGES] = core::array::from_fn(|i| (i < s.count).then_some(s.pages[i]));
        if boot != s.words && boot != s.was {
            // another build changed the registers since: its pages there win
            for (i, (now, then)) in boot.chunks_exact(2).zip(s.words.chunks_exact(2)).enumerate() {
                let w = [now[0], now[1]];
                if now != then
                    && let Some(c) = PageCfg::from_words(w)
                    && let Some(slot) = list.get_mut(i)
                {
                    *slot = Page::from_cfg(c);
                }
            }
            self.store.dirty = true;
        } else if boot != s.words {
            self.save_page_registers_from(b, &s); // cut between the file and the registers: put right
        }
        let mut n = 0;
        for p in list.into_iter().flatten() {
            self.pages[n] = p;
            n += 1;
        }
        if n == 0 {
            return;
        }
        self.page_count = n;
        if let Screen::Ride(k) = self.screen {
            if k >= n && k < PAGES {
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
        let s = Saved {
            seq: self.store.seq.wrapping_add(1),
            pages: core::array::from_fn(|i| self.pages[i]),
            count: self.page_count,
            words: register_words(&self.pages, self.page_count),
            was,
        };
        let slot = (s.seq % 2) as usize;
        let sector = s.to_sector();
        let t = fat::write::dos_time(self.rec_time().0);
        let mut d = emmc::Volume(&mut self.emmc, b);
        // no volume (a card stock never formatted, the tests' blank one): the registers only, nothing to say
        let Ok(mut fat) = fat::Fat::mount(&mut d) else { return self.save_page_registers(b) };
        let r = (|| {
            let dir = fat.ensure_dir(&mut d, fat.root(), recorder::DIR, t)?;
            let mut h = match fat.locate(&mut d, dir, page::FILE) {
                Err(fat::Error::NotFound) => fat.create(&mut d, dir, page::FILE, t)?,
                other => other?,
            };
            if h.size == 1024 {
                return fat.write_sectors(&mut d, &mut h, slot as u32, &sector); // the other copy untouched
            }
            let mut both = [0u8; 1024];
            both[512 * slot..512 * (slot + 1)].copy_from_slice(&sector);
            fat.write_sectors(&mut d, &mut h, 0, &both)?;
            fat.set_size(&mut d, &mut h, 1024, t)
        })();
        match r {
            Ok(()) => (self.store.seq, self.store.sound) = (s.seq, true),
            Err(_) => {
                (self.store.sound, self.store.sync) = (false, 0); // compared again before the next write
                self.notice("PAGES NOT SAVED");
            }
        }
        self.save_page_registers(b); // after the file (module doc), and also when it failed: three pages kept
    }

    // -----------------------------------------------------------------------------------------------------------
    // The editor
    // -----------------------------------------------------------------------------------------------------------

    /// Opens the editor on ride page `n`, box `k` selected; refused while a ride records.
    pub(crate) fn edit_page(&mut self, n: usize, k: usize) {
        if self.ride.state() == ride::State::Running {
            return self.say("PAUSE TO EDIT");
        }
        let Some(&p) = self.page_ref(n).filter(|_| n < PAGES) else { return };
        self.edit = Some(Editing { page: n, work: p, orig: p, undo: None, asking: false });
        self.open(Screen::Edit(n, k.min(p.cells().saturating_sub(1))));
    }

    /// Leaves the editor (and the menus over it), keeping its work or not, back to where it was opened from.
    pub(crate) fn leave_editor(&mut self, b: &mut impl Board, keep: bool) {
        if let Some(e) = self.edit.take()
            && keep
            && e.work != e.orig
        {
            if let Some(p) = self.pages.get_mut(e.page) {
                *p = e.work;
            }
            self.pages_changed(b);
        }
        for _ in 0..self.under.len() {
            let was_editor = matches!(self.screen, Screen::Edit(..));
            self.close();
            if was_editor || self.depth == 0 {
                break;
            }
        }
        self.show(b);
    }

    /// An input in the editor (module doc).
    pub(crate) fn editor_input(&mut self, b: &mut impl Board, i: Input) {
        let (Screen::Edit(_, sel), Some(e)) = (self.screen, self.edit) else {
            return self.leave_editor(b, false);
        };
        let asking = e.asking;
        if let Some(e) = &mut self.edit {
            e.asking = false;
        }
        let cells = e.work.cells().max(1);
        let back = matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)));
        match i {
            Input::Select if asking => self.leave_editor(b, true),
            _ if back && (asking || e.work == e.orig) => self.leave_editor(b, false),
            _ if back => {
                if let Some(e) = &mut self.edit {
                    e.asking = true;
                }
                self.say("KEEP CHANGES?\nSTART: KEEP BACK: DROP");
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
            let mut p = Panel(b, area);
            p.fill(x, y, wd, h, ui::BLACK);
            self.draw_widget(&mut p, old, bx, w);
        }
        self.draw_edit_frame(&mut Panel(b, area));
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
            l.push_str("PAGE ");
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
            EditRow::DeletePage if self.page_count <= 1 => Err("ONE PAGE AT LEAST"),
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

    /// A page added at `at` (2 x 2: speed, timer, distance, heart rate) and the editor opened on it; the editor's
    /// work on the page it was on kept first.
    pub(crate) fn add_page(&mut self, b: &mut impl Board, at: usize) {
        if self.page_count >= PAGES {
            return self.say("6 PAGES AT MOST");
        }
        if let Some(e) = self.edit.take() {
            if let Some(p) = self.pages.get_mut(e.page) {
                *p = e.work;
            }
            // out of the actions and the old page's editor, to the screen it was opened from
            while self.depth > 0 && !matches!(self.screen, Screen::Edit(..)) {
                self.close();
            }
            self.close();
        }
        let at = at.min(self.page_count);
        use Field::*;
        let new = Page::preset(4, [Speed, Timer, Dist, Hr, Time, Time, Time, Time]).unwrap_or(BLANK);
        self.pages.copy_within(at..PAGES - 1, at + 1);
        self.pages[at] = new;
        self.page_count += 1;
        self.pages_changed(b);
        let mut l = Line::<20>::new();
        l.push_str("PAGE ");
        l.push_u32(at as u32 + 1);
        l.push_str(" ADDED");
        self.say(l.as_str());
        self.edit_page(at, 0);
    }

    /// The edited page deleted, out of the editor.
    fn delete_page(&mut self, b: &mut impl Board) {
        let Some(e) = self.edit.take() else { return };
        if e.page < self.page_count && self.page_count > 1 {
            self.pages.copy_within(e.page + 1..PAGES, e.page);
            self.page_count -= 1;
            self.pages_changed(b);
        }
        self.leave_editor(b, false);
        self.say("PAGE DELETED");
    }

    /// The menu's LAYOUT: page `n` (`n` < [`PAGES`]) to the next layout every field of it fits, its fields in order.
    pub(crate) fn next_layout(&mut self, b: &mut impl Board, n: usize) {
        let Some(&p) = self.page_ref(n).filter(|_| n < PAGES) else { return };
        let order = ui::LAYOUT_ORDER;
        let at = p.preset_id().and_then(|id| order.iter().position(|&l| l == id)).unwrap_or(order.len() - 1);
        let next = (1..=order.len())
            .filter_map(|d| Page::preset(order[(at + d) % order.len()], p.fields))
            .find(|q| q.misfits() == 0);
        if let (Some(q), Some(slot)) = (next, self.pages.get_mut(n)) {
            *slot = q;
            self.pages_changed(b);
        }
    }

    /// The picker's field `f` for box `k` of page `n`: the editor's copy, a ride page (if it is readable there) or
    /// the overview; then back to the editor or the page's menu.
    pub(crate) fn pick(&mut self, b: &mut impl Board, n: usize, k: usize, f: Field) {
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
                if let Some(slot) = self.pages.get_mut(n) {
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
                v.push_u32(self.page_count as u32);
                v.push_str("/");
                v.push_u32(PAGES as u32);
            }
            _ => {}
        }
    }
}
