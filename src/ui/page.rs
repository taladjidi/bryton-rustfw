//! A ride page as rows of cells: up to [`ROWS`] rows top to bottom, each a share of the height (1 to 3) and one or
//! two cells side by side, [`CELLS`] fields at most. It is the model the fixed layouts always had ([`ui::LAYOUTS`],
//! now presets of it), so the geometry stays [`ui::rows_box`] and a row keeps one value scale (UI review 2, 5.2
//! point 1: a free split tree was the other choice; it allows a quarter of a quarter and rows of mixed scales, and
//! converts the old layouts only within a pixel). The page editor (app/editor.rs) splits a cell across (a second
//! cell in its row) or down (a row under it), merges it with its neighbour, makes its row taller or shorter, and
//! never leaves a field in a box too small to read ([`min_size`]). Pure logic, and the format of
//! `\Rustfw\pages.bin` ([`Saved`]).

use crate::ui::{self, Field, Rect};
use crate::{BLANK, PageCfg};

/// Rows a page has at most.
pub const ROWS: usize = 5;

/// Cells (fields) a page has at most: what the page's boxes hold (app/screens.rs `BOXES`).
pub const CELLS: usize = 8;

/// Cells a row has at most: three would leave five characters at 2x (UI review 2, 5.2 point 4).
pub const ROW_CELLS: u8 = 2;

/// A row's share of the height at most.
pub const SHARE: u8 = 3;

/// The smallest box field `f` is readable in (w, h), which an edit may not go under: a value five characters at
/// 3x under its label ([`ui::VALUE_MIN`]); a graph, the course profile or the workout gauge 118 px high (two fifths
/// of the page, the course page's profile; under 120 a graph draws without its axes); the map half the page.
pub fn min_size(f: Field) -> (u32, u32) {
    match f {
        Field::Map => (ui::W / 2, ui::RIDE_H / 2),
        f if f.is_picture() => (ui::W / 2, 118),
        _ => ui::VALUE_MIN,
    }
}

/// Field `f` is readable in box `b` ([`min_size`]); the map wants half the page's area too.
pub fn fits(f: Field, b: Rect) -> bool {
    let (w, h) = min_size(f);
    b.2 >= w && b.3 >= h && (f != Field::Map || b.2 * b.3 >= ui::W * ui::RIDE_H / 2)
}

/// A ride page: its rows and their fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// (share, cells) per row, top first; a share of 0 ends the page.
    pub rows: [(u8, u8); ROWS],
    /// The cells' fields, row by row, left to right; those past the last cell are kept (a layout with more cells
    /// shows them again).
    pub fields: [Field; CELLS],
}

impl Page {
    /// Layout `id` ([`ui::LAYOUTS`]) with these fields; None for 0 (off) or past the last.
    pub fn preset(id: u8, fields: [Field; CELLS]) -> Option<Page> {
        let (r, _) = ui::layout(id).filter(|_| id != 0)?;
        let mut rows = [(0, 0); ROWS];
        rows.get_mut(..r.len())?.copy_from_slice(r);
        Some(Page { rows, fields })
    }

    /// The page a backup register pair kept ([`PageCfg`]); None for a page switched off.
    ///
    /// ```
    /// use bryton_rustfw::{PageCfg, page::Page, ui::Field::*};
    /// let c = PageCfg { layout: 6, fields: [Speed, Hr, Power, Cadence, Dist, Timer] };
    /// let p = Page::from_cfg(c).unwrap();
    /// assert_eq!((p.cells(), p.to_cfg()), (6, c));
    /// ```
    pub fn from_cfg(c: PageCfg) -> Option<Page> {
        let mut fields = [Field::Time; CELLS];
        fields[..6].copy_from_slice(&c.fields);
        Page::preset(c.layout, fields)
    }

    /// As the backup registers keep it, for earlier builds and as the file's fallback: its layout if it is one they
    /// know (ids 1 to 12), else the first of theirs with as many cells (six at most: its first six fields).
    pub fn to_cfg(&self) -> PageCfg {
        let mut fields = [Field::Time; 6];
        fields.copy_from_slice(&self.fields[..6]);
        let n = self.cells().min(6);
        let layout = self
            .preset_id()
            .filter(|&id| id <= OLD_LAYOUTS)
            .or_else(|| ui::LAYOUT_ORDER.iter().copied().find(|&id| id <= OLD_LAYOUTS && ui::field_count(id) == n))
            .unwrap_or(1);
        PageCfg { layout, fields }
    }

    /// The layout it is, if any ([`ui::LAYOUTS`]).
    pub fn preset_id(&self) -> Option<u8> {
        (1..ui::LAYOUTS.len() as u8).find(|&id| Page::preset(id, self.fields).is_some_and(|p| p.rows == self.rows))
    }

    /// Its rows in use.
    pub fn used(&self) -> &[(u8, u8)] {
        let n = self.rows.iter().take_while(|r| r.0 != 0).count();
        self.rows.get(..n).unwrap_or(&[])
    }

    /// Its cells.
    pub fn cells(&self) -> usize {
        self.used().iter().map(|r| usize::from(r.1)).sum()
    }

    /// Rows of shares 1 to [`SHARE`] and 1 to [`ROW_CELLS`] cells, [`CELLS`] at most, zeros after them: what any
    /// page read is checked for (the sizes are not: a page of an earlier build may have a small graph).
    pub fn well_formed(&self) -> bool {
        let used = self.used().len();
        used > 0
            && self.cells() <= CELLS
            && self.rows.iter().enumerate().all(|(i, &(s, c))| {
                if i < used { (1..=SHARE).contains(&s) && (1..=ROW_CELLS).contains(&c) } else { (s, c) == (0, 0) }
            })
    }

    /// Box of cell `k` on a ride page.
    pub fn cell_box(&self, k: usize) -> Option<Rect> {
        ui::rows_box(self.used(), ui::RIDE_H, k).filter(|_| k < self.cells()).map(Rect::from)
    }

    /// Cell `k`'s field is not readable in its box ([`fits`]).
    pub fn misfit(&self, k: usize) -> bool {
        self.cell_box(k).is_none_or(|b| !self.fields.get(k).is_some_and(|&f| fits(f, b)))
    }

    /// Cells whose field is not readable in its box.
    pub fn misfits(&self) -> usize {
        (0..self.cells()).filter(|&k| self.misfit(k)).count()
    }

    /// Cells showing `f` that is not readable there.
    pub fn misfits_of(&self, f: Field) -> usize {
        (0..self.cells()).filter(|&k| self.fields.get(k) == Some(&f) && self.misfit(k)).count()
    }

    /// Every cell at least the smallest value box ([`ui::VALUE_MIN`]): the floor of every edit, and what a page read
    /// from the file is checked for (a box under it would not even hold a value's label; drawing assumes it).
    ///
    /// ```
    /// use bryton_rustfw::{page::Page, ui::Field::*};
    /// let tiny = Page { rows: [(1, 2), (3, 1), (3, 1), (3, 1), (3, 1)], fields: [Speed; 8] };
    /// assert!(tiny.well_formed() && !tiny.roomy(), "a row 22 px high");
    /// ```
    pub fn roomy(&self) -> bool {
        (0..self.cells()).all(|k| self.cell_box(k).is_some_and(|b| b.2 >= ui::VALUE_MIN.0 && b.3 >= ui::VALUE_MIN.1))
    }

    /// Well formed, every field readable in its cell.
    pub fn valid(&self) -> bool {
        self.well_formed() && self.misfits() == 0
    }

    /// Cell `k`'s row and its place in it.
    fn place(&self, k: usize) -> Option<(usize, usize)> {
        let mut first = 0;
        for (r, &(_, n)) in self.used().iter().enumerate() {
            if k < first + usize::from(n) {
                return Some((r, k - first));
            }
            first += usize::from(n);
        }
        None
    }

    /// The first cell of row `r`.
    fn first_of(&self, r: usize) -> usize {
        self.used().iter().take(r).map(|x| usize::from(x.1)).sum()
    }

    /// `self` after an edit, if it is well formed, every box holds a value ([`Page::roomy`]) and no field is
    /// unreadable that was readable before: a field unreadable after it only where it already was (a page of an
    /// earlier build with a small graph can still be edited elsewhere, but making that graph fit may not squeeze
    /// another field, nor a value under 3x); else why not (the banner says it). Cells move in an edit, so they are
    /// matched by their field: no field has more unreadable cells after it than before.
    fn checked(&self, after: Page) -> Result<Page, &'static str> {
        if !after.well_formed() {
            Err("NO ROOM")
        } else if !after.roomy()
            || (0..after.cells()).any(|k| {
                after.misfit(k) && after.fields.get(k).is_none_or(|&f| after.misfits_of(f) > self.misfits_of(f))
            })
        {
            Err("TOO SMALL")
        } else {
            Ok(after)
        }
    }

    /// `new` in at cell `at`, the fields from there moved on (the last one out).
    fn insert_field(&mut self, at: usize, new: Field) {
        if at < CELLS {
            self.fields.copy_within(at..CELLS - 1, at + 1);
            self.fields[at] = new;
        }
    }

    /// `n` fields from cell `at` out, those after moved back.
    fn remove_fields(&mut self, at: usize, n: usize) {
        if at + n <= CELLS {
            self.fields.copy_within(at + n..CELLS, at);
            for f in self.fields.iter_mut().skip(CELLS - n) {
                *f = Field::Time;
            }
        }
    }

    /// Cell `k` split across: a second cell in its row, right of it, showing `new`.
    ///
    /// ```
    /// use bryton_rustfw::{page::Page, ui::Field::*};
    /// let p = Page::preset(1, [Speed, Hr, Time, Time, Time, Time, Time, Time]).unwrap();
    /// let p = p.split_across(0, Power).unwrap();
    /// assert_eq!((p.cells(), &p.fields[..3]), (2, &[Speed, Power, Hr][..]));
    /// assert_eq!(p.split_across(0, Hr), Err("2 A ROW AT MOST"));
    /// ```
    pub fn split_across(&self, k: usize, new: Field) -> Result<Page, &'static str> {
        let (r, _) = self.place(k).ok_or("NO BOX")?;
        if self.cells() >= CELLS {
            return Err("8 BOXES AT MOST");
        }
        let mut p = *self;
        let row = p.rows.get_mut(r).ok_or("NO BOX")?;
        if row.1 >= ROW_CELLS {
            return Err("2 A ROW AT MOST");
        }
        row.1 += 1;
        p.insert_field(k + 1, new);
        self.checked(p)
    }

    /// Cell `k` split down: a row under its row, one cell showing `new`; a row of share 2 or 3 gives it half, one of
    /// share 1 makes it as high as itself.
    ///
    /// ```
    /// use bryton_rustfw::{page::Page, ui::Field::*};
    /// let p = Page::preset(1, [Speed, Hr, Time, Time, Time, Time, Time, Time]).unwrap();
    /// let p = p.split_down(0, Power).unwrap();
    /// assert_eq!((p.used(), &p.fields[..2]), (&[(1, 1), (1, 1)][..], &[Speed, Power][..]));
    /// ```
    pub fn split_down(&self, k: usize, new: Field) -> Result<Page, &'static str> {
        let (r, _) = self.place(k).ok_or("NO BOX")?;
        if self.cells() >= CELLS {
            return Err("8 BOXES AT MOST");
        }
        if self.used().len() >= ROWS {
            return Err("5 ROWS AT MOST");
        }
        let mut p = *self;
        let share = p.rows[r].0;
        let below = (share / 2).max(1);
        p.rows[r].0 = (share - below).max(1);
        p.rows.copy_within(r + 1..ROWS - 1, r + 2);
        p.rows[r + 1] = (below, 1);
        let at = self.first_of(r + 1);
        p.insert_field(at, new);
        self.checked(p)
    }

    /// Cell `k` merged with its neighbour: the other cell of its row, or, alone in its row, the row under it (the
    /// one over it for the last row), whose height it takes. The merged cell keeps `k`'s field; the page and its
    /// number.
    ///
    /// ```
    /// use bryton_rustfw::{page::Page, ui::Field::*};
    /// let p = Page::preset(5, [Map, Speed, Hr, Power, Dist, Time, Time, Time]).unwrap(); // big over 2 x 2
    /// let (q, k) = p.merge(2).unwrap(); // heart rate takes speed's cell
    /// assert_eq!((q.cells(), k, &q.fields[..4]), (4, 1, &[Map, Hr, Power, Dist][..]));
    /// let (q, k) = q.merge(0).unwrap(); // the map takes the row under it
    /// assert_eq!((q.used(), k, &q.fields[..3]), (&[(3, 1), (1, 2)][..], 0, &[Map, Power, Dist][..]));
    /// ```
    pub fn merge(&self, k: usize) -> Result<(Page, usize), &'static str> {
        let (r, j) = self.place(k).ok_or("NO BOX")?;
        let mut p = *self;
        let first = self.first_of(r);
        let keep = *self.fields.get(k).ok_or("NO BOX")?;
        if p.rows[r].1 > 1 {
            p.rows[r].1 -= 1;
            p.remove_fields(first + 1 - j.min(1), 1); // the other cell of the row
            p.fields[first] = keep;
            return self.checked(p).map(|p| (p, first));
        }
        let rows = self.used().len();
        if rows < 2 {
            return Err("ONE BOX");
        }
        let other = if r + 1 < rows { r + 1 } else { r - 1 };
        let (top, gone) = (r.min(other), r.max(other));
        let at = self.first_of(top);
        p.remove_fields(at, 1 + usize::from(self.rows[other].1));
        p.insert_field(at, keep);
        p.rows[top] = ((self.rows[r].0 + self.rows[other].0).min(SHARE), 1);
        p.rows.copy_within(gone + 1..ROWS, gone);
        p.rows[ROWS - 1] = (0, 0);
        self.checked(p).map(|p| (p, at))
    }

    /// Cell `k`'s row one share taller (`taller`) or shorter, [`SHARE`] to 1, the others keeping theirs.
    pub fn resize(&self, k: usize, taller: bool) -> Result<Page, &'static str> {
        let (r, _) = self.place(k).ok_or("NO BOX")?;
        let mut p = *self;
        let s = &mut p.rows[r].0;
        *s = match (taller, *s) {
            (true, SHARE) => return Err("TALLEST"),
            (false, 1) => return Err("SHORTEST"),
            (true, s) => s + 1,
            (false, s) => s - 1,
        };
        if self.used().len() == 1 {
            return Err("ONE ROW");
        }
        self.checked(p)
    }

    /// The MORE variant a page has until the rider edits it: eight cells, this page's fields that are readable in
    /// one (a map or a graph is not) first, then the next useful ones it does not show yet.
    ///
    /// ```
    /// use bryton_rustfw::{page::Page, ui::Field::*};
    /// let p = Page::preset(4, [Speed, Hr, Map, Dist, Time, Time, Time, Time]).unwrap();
    /// let m = p.more_of();
    /// assert_eq!((m.cells(), &m.fields[..4]), (8, &[Speed, Hr, Dist, Power3s][..]));
    /// assert!(m.valid());
    /// ```
    pub fn more_of(&self) -> Page {
        use Field::*;
        let Some(dense) = preset_of(CELLS).and_then(|id| Page::preset(id, [Time; CELLS])) else { return *self };
        let room = dense.cell_box(0).unwrap_or(Rect(0, 0, 0, 0));
        let mut fields = [Time; CELLS];
        let mut n = 0;
        for f in self.fields.iter().take(self.cells()).copied().filter(|&f| fits(f, room)) {
            if !fields.get(..n).unwrap_or(&[]).contains(&f)
                && let Some(slot) = fields.get_mut(n)
            {
                *slot = f;
                n += 1;
            }
        }
        // ponytail: the extras are a fixed list; the rider's own MORE (the editor) is the way to anything else
        for f in [Power3s, Np, Hr, Cadence, Speed, Dist, Timer, Ascent, AvgSpeed, Kj] {
            if !fields.get(..n).unwrap_or(&[]).contains(&f)
                && let Some(slot) = fields.get_mut(n)
            {
                *slot = f;
                n += 1;
            }
        }
        Page { fields, ..dense }
    }

    /// Cell `k` showing `f`, if it is readable there.
    pub fn with_field(&self, k: usize, f: Field) -> Result<Page, &'static str> {
        let b = self.cell_box(k).ok_or("NO BOX")?;
        if !fits(f, b) {
            return Err("TOO SMALL");
        }
        let mut p = *self;
        p.fields[k] = f;
        Ok(p)
    }
}

impl From<PageCfg> for Page {
    /// [`Page::from_cfg`], a page switched off as one cell showing the time.
    fn from(c: PageCfg) -> Page {
        Page::from_cfg(c).unwrap_or(crate::BLANK)
    }
}

/// The layouts earlier builds know: ids 1 to 12.
const OLD_LAYOUTS: u8 = 12;

/// The first layout of `n` cells ([`ui::LAYOUT_ORDER`]).
pub fn preset_of(n: usize) -> Option<u8> {
    ui::LAYOUT_ORDER.iter().copied().find(|&id| ui::field_count(id) == n)
}

// ---------------------------------------------------------------------------------------------------------------
// `\Rustfw\pages.bin`
// ---------------------------------------------------------------------------------------------------------------

/// The file's name, in this firmware's folder ([`crate::recorder::DIR`]).
pub const FILE: &str = "pages.bin";

/// The file's tag and version.
const MAGIC: &[u8; 4] = b"RPG1";

/// Bytes of a page in a copy: a byte a row (share in the high nibble, cells in the low), then the fields' numbers
/// ([`Field::ALL`]).
const PAGE_BYTES: usize = ROWS + CELLS;

/// Where the pages start in a copy, and the CRC.
const PAGES_AT: usize = 60;
const CRC_AT: usize = PAGES_AT + crate::PAGES * PAGE_BYTES;

/// The MORE variants, after the first CRC where earlier builds do not look: a byte of flags (bit k: page k has one;
/// zero in a file of an earlier build, so every page then has its default), their pages, and their own CRC-16 of
/// the flags and the pages. A bad one leaves every page its default, the pages above it untouched.
const MORE_AT: usize = CRC_AT + 2;
const MORE_CRC_AT: usize = MORE_AT + 1 + crate::PAGES * PAGE_BYTES;

/// The MORE part's CRC: the flags and pages, continuing the first CRC over the pages (not over the pages with their
/// CRC, which is always 0), so a copy torn at `MORE_AT` mixes nothing: the older MORE fails against the newer pages.
fn more_crc(s: &[u8; 512]) -> u16 {
    crate::fit::crc(crate::fit::crc(0, &s[..CRC_AT]), &s[MORE_AT..MORE_CRC_AT])
}

/// The indoor set, after the MORE part's CRC where earlier builds do not look: its page count (zero in a file of an
/// earlier build: the defaults), its pages, the MORE flags and variants as above, and a CRC-16 of these continuing
/// the MORE part's CRC (so a copy torn anywhere before it mixes nothing: the older indoor set fails against the newer
/// pages). A bad one leaves the indoor defaults, everything above it untouched. Ends at byte 381 of 512.
const INDOOR_AT: usize = MORE_CRC_AT + 2;
const INDOOR_MORE_AT: usize = INDOOR_AT + 1 + crate::PAGES * PAGE_BYTES;
const INDOOR_CRC_AT: usize = INDOOR_MORE_AT + 1 + crate::PAGES * PAGE_BYTES;

/// The indoor part's CRC.
fn indoor_crc(s: &[u8; 512]) -> u16 {
    crate::fit::crc(more_crc(s), &s[INDOOR_AT..INDOOR_CRC_AT])
}

/// A set of ride pages with their MORE variants: the indoor profile's (the road set lives in the app's own fields,
/// where the backup registers can reach its first pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set {
    /// The pages, the first `count` used.
    pub pages: [Page; crate::PAGES],
    /// How many (1 to [`crate::PAGES`]).
    pub count: usize,
    /// Each page's MORE variant, if the rider made one (else [`Page::more_of`]).
    pub more: [Option<Page>; crate::PAGES],
}

impl Set {
    /// The indoor pages until the rider edits them, from existing fields: the trainer (3 s power large, the
    /// trainer's target, heart rate, cadence, time, work), the power numbers, and the power and heart rate graphs.
    ///
    /// ```
    /// let s = bryton_rustfw::page::Set::indoor_default();
    /// assert!(s.count == 3 && s.pages.iter().take(s.count).all(|p| p.valid() && p.roomy()));
    /// ```
    pub fn indoor_default() -> Set {
        use Field::*;
        let mut pages = [BLANK; crate::PAGES];
        let sets: [(u8, [Field; CELLS]); 3] = [
            (12, [Power3s, TrainerTarget, Hr, Cadence, Timer, Kj, Time, Time]),
            (6, [AvgPower, Np, Wbal, If, Kj, Tss, Time, Time]),
            (preset_of(2).unwrap_or(1), [Graph, HrGraph, Time, Time, Time, Time, Time, Time]),
        ];
        for (slot, (id, f)) in pages.iter_mut().zip(sets) {
            *slot = Page::preset(id, f).unwrap_or(BLANK);
        }
        Set { pages, count: 3, more: [None; crate::PAGES] }
    }
}

/// The ride pages as the file keeps them: two copies, one a sector, the newer whole one (the larger `seq`) the one
/// read; a write goes over the older, so a power cut while writing one leaves the other (the recorder's two
/// records). Each copy also keeps the backup registers' words written with it (`words`) and those they held before
/// (`was`): at a start the registers equal one of them, unless another build (which keeps its pages there only)
/// changed them since (app/editor.rs says which wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Higher is newer.
    pub seq: u32,
    /// The ride pages, the first `count` used.
    pub pages: [Page; crate::PAGES],
    /// How many (1 to [`crate::PAGES`]).
    pub count: usize,
    /// Registers 20 to 25 as written with this copy: the first three pages ([`Page::to_cfg`]).
    pub words: [u32; 6],
    /// What they held before.
    pub was: [u32; 6],
    /// Each page's MORE variant, if the rider made one (else [`Page::more_of`]).
    pub more: [Option<Page>; crate::PAGES],
    /// The indoor profile's set, if the copy has a good one (else [`Set::indoor_default`]).
    pub indoor: Option<Set>,
}

/// `p` as a copy keeps it.
fn write_page(p: &Page, out: &mut [u8]) {
    for (o, &(share, n)) in out.iter_mut().zip(&p.rows) {
        *o = share << 4 | n;
    }
    for (o, f) in out.iter_mut().skip(ROWS).zip(&p.fields) {
        *o = *f as u8;
    }
}

/// A page from `b` (a [`PAGE_BYTES`] chunk); None for a field this build does not have.
fn read_page(b: &[u8]) -> Option<Page> {
    let mut p = Page { rows: [(0, 0); ROWS], fields: [Field::Time; CELLS] };
    for (r, &x) in p.rows.iter_mut().zip(b) {
        *r = (x >> 4, x & 0xf);
    }
    for (f, &i) in p.fields.iter_mut().zip(b.get(ROWS..)?) {
        if usize::from(i) >= Field::ALL.len() {
            return None;
        }
        *f = Field::from_index(u32::from(i));
    }
    Some(p)
}

impl Saved {
    /// The sector: tag, seq, count, the words, the pages, a CRC-16 of all before it; then the MORE part and the indoor
    /// part, each with a CRC-16 of its own.
    ///
    /// ```
    /// use bryton_rustfw::{page::*, ui::Field};
    /// let p = Page::preset(4, [Field::Hr; 8]).unwrap();
    /// let s = Saved { seq: 7, pages: [p; 6], count: 2, words: [1, 2, 3, 4, 5, 6], was: [0; 6], more: [None; 6], indoor: None };
    /// let mut sector = s.to_sector();
    /// assert_eq!(Saved::from_sector(&sector), Some(s));
    /// sector[70] ^= 1;
    /// assert_eq!(Saved::from_sector(&sector), None, "damaged");
    /// ```
    pub fn to_sector(&self) -> [u8; 512] {
        let mut s = [0u8; 512];
        s[..4].copy_from_slice(MAGIC);
        s[4..8].copy_from_slice(&self.seq.to_le_bytes());
        s[8] = self.count as u8;
        for (k, w) in self.words.iter().chain(&self.was).enumerate() {
            s[12 + 4 * k..16 + 4 * k].copy_from_slice(&w.to_le_bytes());
        }
        for (p, out) in self.pages.iter().zip(s[PAGES_AT..CRC_AT].chunks_exact_mut(PAGE_BYTES)) {
            write_page(p, out);
        }
        let crc = crate::fit::crc(0, &s[..CRC_AT]);
        s[CRC_AT..CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());
        let mut flags = 0u8;
        for (k, (m, out)) in self.more.iter().zip(s[MORE_AT + 1..MORE_CRC_AT].chunks_exact_mut(PAGE_BYTES)).enumerate()
        {
            if let Some(p) = m {
                flags |= 1 << k;
                write_page(p, out);
            }
        }
        s[MORE_AT] = flags;
        let crc = more_crc(&s);
        s[MORE_CRC_AT..MORE_CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());
        if let Some(i) = &self.indoor {
            s[INDOOR_AT] = i.count as u8;
            for (p, out) in i.pages.iter().zip(s[INDOOR_AT + 1..INDOOR_MORE_AT].chunks_exact_mut(PAGE_BYTES)) {
                write_page(p, out);
            }
            let mut flags = 0u8;
            let outs = s[INDOOR_MORE_AT + 1..INDOOR_CRC_AT].chunks_exact_mut(PAGE_BYTES);
            for (k, (m, out)) in i.more.iter().zip(outs).enumerate() {
                if let Some(p) = m {
                    flags |= 1 << k;
                    write_page(p, out);
                }
            }
            s[INDOOR_MORE_AT] = flags;
            let crc = indoor_crc(&s);
            s[INDOOR_CRC_AT..INDOOR_CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());
        }
        s
    }

    /// The indoor part of a sector: None where it is not there (a file of an earlier build), damaged, or holds a page
    /// not well formed (the whole set then goes to its defaults, never part of one).
    fn read_indoor(s: &[u8; 512]) -> Option<Set> {
        let count = usize::from(s[INDOOR_AT]);
        let good = s.get(INDOOR_CRC_AT..INDOOR_CRC_AT + 2)? == indoor_crc(s).to_le_bytes();
        if !good || !(1..=crate::PAGES).contains(&count) {
            return None;
        }
        let mut set = Set { pages: [BLANK; crate::PAGES], count, more: [None; crate::PAGES] };
        let chunks = s.get(INDOOR_AT + 1..INDOOR_MORE_AT)?.chunks_exact(PAGE_BYTES);
        for (k, (p, b)) in set.pages.iter_mut().zip(chunks).enumerate() {
            *p = read_page(b)?;
            if k < count && !(p.well_formed() && p.roomy()) {
                return None;
            }
        }
        let flags = s[INDOOR_MORE_AT];
        let chunks = s.get(INDOOR_MORE_AT + 1..INDOOR_CRC_AT)?.chunks_exact(PAGE_BYTES);
        for (k, (m, b)) in set.more.iter_mut().zip(chunks).enumerate() {
            *m = read_page(b).filter(|p| flags & 1 << k != 0 && p.well_formed() && p.roomy());
        }
        Some(set)
    }

    /// From a sector [`Saved::to_sector`] wrote; None if it is not one (never written, damaged, a page not well
    /// formed or with a box smaller than an edit leaves, [`Page::roomy`], or a field this build does not have).
    pub fn from_sector(s: &[u8; 512]) -> Option<Saved> {
        let crc = crate::fit::crc(0, s.get(..CRC_AT)?);
        if &s[..4] != MAGIC || s.get(CRC_AT..CRC_AT + 2)? != crc.to_le_bytes() {
            return None;
        }
        let count = usize::from(s[8]);
        let word = |k: usize| u32::from_le_bytes([s[12 + 4 * k], s[13 + 4 * k], s[14 + 4 * k], s[15 + 4 * k]]);
        let mut pages = [BLANK; crate::PAGES];
        for (k, (p, b)) in pages.iter_mut().zip(s.get(PAGES_AT..CRC_AT)?.chunks_exact(PAGE_BYTES)).enumerate() {
            *p = read_page(b)?;
            if k < count && !(p.well_formed() && p.roomy()) {
                return None;
            }
        }
        // the MORE variants: none where the flags or their CRC are not there (a file of an earlier build)
        let mut more = [None; crate::PAGES];
        let flags = s[MORE_AT];
        if flags != 0 && s.get(MORE_CRC_AT..MORE_CRC_AT + 2)? == more_crc(s).to_le_bytes() {
            for (k, (m, b)) in more.iter_mut().zip(s[MORE_AT + 1..MORE_CRC_AT].chunks_exact(PAGE_BYTES)).enumerate() {
                *m = read_page(b).filter(|p| flags & 1 << k != 0 && p.well_formed() && p.roomy());
            }
        }
        (1..=crate::PAGES).contains(&count).then(|| Saved {
            seq: u32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            pages,
            count,
            words: core::array::from_fn(word),
            was: core::array::from_fn(|k| word(6 + k)),
            more,
            indoor: Self::read_indoor(s),
        })
    }

    /// The newer of the file's two copies that check out (its 1024 bytes), if any.
    pub fn newest(file: &[u8; 1024]) -> Option<Saved> {
        let copy =
            |k: usize| -> Option<Saved> { Saved::from_sector(file.get(512 * k..512 * (k + 1))?.try_into().ok()?) };
        match (copy(0), copy(1)) {
            (Some(a), Some(b)) => Some(if b.seq > a.seq { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::ui::Field::*;

    const F: [Field; CELLS] = [Speed, Hr, Power, Cadence, Dist, Timer, Ascent, Grade];

    /// Every layout is a page of as many cells, at exactly the boxes [`ui::field_box`] gives, through the registers
    /// and back; every number of cells has one.
    #[test]
    fn every_preset_round_trips() {
        for id in 1..ui::LAYOUTS.len() as u8 {
            let p = Page::preset(id, F).unwrap();
            assert!(p.valid(), "layout {id}");
            assert_eq!((p.preset_id(), p.cells()), (Some(id), ui::field_count(id)));
            for k in 0..p.cells() {
                assert_eq!(p.cell_box(k).map(Into::into), ui::field_box(id, ui::RIDE_H, k), "layout {id} box {k}");
            }
            if id <= OLD_LAYOUTS {
                let back = Page::from_cfg(p.to_cfg()).unwrap();
                assert_eq!((back.rows, &back.fields[..6]), (p.rows, &p.fields[..6]), "layout {id}");
            }
        }
        assert_eq!(Page::preset(0, F), None, "off is no page");
        let counts: std::vec::Vec<_> = (1..=8).map(|n| ui::field_count(preset_of(n).unwrap())).collect();
        assert_eq!(counts, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    /// A page the registers' layouts do not have: its first six fields, in a layout of six.
    #[test]
    fn registers_keep_a_lossy_copy() {
        let c = Page::preset(14, F).unwrap().to_cfg();
        assert_eq!((ui::field_count(c.layout), c.fields), (6, [Speed, Hr, Power, Cadence, Dist, Timer]));
        let odd = Page::preset(2, F).unwrap().resize(0, true).unwrap(); // 2:1 rows: no layout
        assert_eq!((odd.preset_id(), ui::field_count(odd.to_cfg().layout)), (None, 2));
    }

    /// Split, merge, resize: well formed, fields where they should be, never a box too small.
    #[test]
    fn edits_keep_the_page_readable() {
        let one = Page::preset(1, F).unwrap();
        assert_eq!(one.merge(0), Err("ONE BOX"));
        assert_eq!(one.resize(0, true), Err("ONE ROW"));
        // down to eight cells, then no more
        let full = Page::preset(14, F).unwrap();
        assert_eq!(full.split_down(0, Hr), Err("8 BOXES AT MOST"));
        // five rows, then no more
        let mut p = one;
        for k in 0..4 {
            p = p.split_down(k, F[k + 1]).unwrap();
        }
        assert_eq!((p.used().len(), p.split_down(0, Hr)), (5, Err("5 ROWS AT MOST")));
        assert_eq!(p.resize(0, true).and_then(|p| p.resize(0, true)), Err("TOO SMALL"), "the others under 3x");
        // merging every cell in turn ends at one box, the first field kept
        let mut p = full;
        while p.cells() > 1 {
            let (q, k) = p.merge(0).unwrap();
            assert!(q.valid() && q.cells() < p.cells() && k < q.cells(), "{q:?}");
            p = q;
        }
        assert_eq!(p.fields[0], Speed);
        // a map wants half the page, a graph 118 px
        let two = one.split_down(0, Hr).unwrap();
        assert!(two.with_field(0, Map).is_ok() && two.with_field(0, Graph).is_ok());
        let three = two.split_down(1, Dist).unwrap();
        assert_eq!(three.with_field(1, Map), Err("TOO SMALL"));
        let quarter = Page::preset(4, F).unwrap();
        assert_eq!(quarter.with_field(0, Map), Err("TOO SMALL"));
        assert_eq!(
            quarter.with_field(0, Graph),
            Ok(Page { fields: [Graph, Hr, Power, Cadence, Dist, Timer, Ascent, Grade], ..quarter })
        );
    }

    /// A page of an earlier build with a small graph can still be edited elsewhere.
    #[test]
    fn an_old_small_graph_does_not_lock_the_page() {
        let p = Page::preset(6, [Graph, Hr, Power, Cadence, Dist, Timer, Time, Time]).unwrap(); // 98 px high
        assert!(p.well_formed() && !p.valid());
        assert!(p.with_field(5, Speed).is_ok());
        assert!(p.merge(5).is_ok());
    }

    /// The newer copy that checks out is read; a torn one leaves the other.
    #[test]
    fn the_newer_whole_copy_wins() {
        let p = |id| Page::preset(id, F).unwrap();
        let a = Saved {
            seq: 3,
            pages: [p(1); crate::PAGES],
            count: 1,
            words: [0; 6],
            was: [0; 6],
            more: [None; 6],
            indoor: None,
        };
        let b = Saved { seq: 4, pages: [p(4); crate::PAGES], count: 3, ..a };
        let mut file = [0u8; 1024];
        file[..512].copy_from_slice(&b.to_sector());
        file[512..].copy_from_slice(&a.to_sector());
        assert_eq!(Saved::newest(&file), Some(b));
        file[100] ^= 0xff;
        assert_eq!(Saved::newest(&file), Some(a), "the newer torn: the older");
        assert_eq!(Saved::newest(&[0; 1024]), None);
    }

    /// A page's MORE variant: eight readable cells whatever the page holds, the page's own fields first.
    #[test]
    fn the_default_more_is_dense_and_readable() {
        for id in 1..ui::LAYOUTS.len() as u8 {
            for fields in [F, [Map; CELLS], [Graph, Hr, Map, AltGraph, Power, Cadence, Dist, Timer]] {
                let m = Page::preset(id, fields).unwrap().more_of();
                assert!(m.valid() && m.cells() == CELLS, "layout {id}: {m:?}");
                let shown = &m.fields[..CELLS];
                assert!(!shown.iter().any(|f| f.is_picture()), "layout {id}: {m:?}");
            }
        }
        let p = Page::preset(4, [Speed, Hr, Power3s, Dist, Time, Time, Time, Time]).unwrap();
        assert_eq!(p.more_of().fields, [Speed, Hr, Power3s, Dist, Np, Cadence, Timer, Ascent], "no field twice");
    }

    /// The MORE variants go to the file and back; a file of an earlier build (nothing after the first CRC) and one
    /// whose MORE part is damaged give every page its default, the pages themselves intact.
    #[test]
    fn more_variants_in_the_file() {
        let p = Page::preset(4, F).unwrap();
        let m = Page::preset(14, F).unwrap();
        let s = Saved {
            seq: 5,
            pages: [p; crate::PAGES],
            count: 3,
            words: [1; 6],
            was: [2; 6],
            more: [None, Some(m), None, None, None, Some(p)],
            indoor: None,
        };
        let sector = s.to_sector();
        assert_eq!(Saved::from_sector(&sector), Some(s));
        let plain = Saved { more: [None; crate::PAGES], ..s };
        let mut old = sector;
        old[MORE_AT..].fill(0); // as an earlier build wrote it
        assert_eq!(Saved::from_sector(&old), Some(plain));
        assert_eq!(&plain.to_sector()[..CRC_AT + 2], &sector[..CRC_AT + 2]);
        let mut bad = sector;
        bad[MORE_AT + 3] ^= 0x10;
        assert_eq!(Saved::from_sector(&bad), Some(plain), "the MORE part damaged");
        let mut bad = sector;
        bad[MORE_AT + 1 + PAGE_BYTES + ROWS] = 250; // a field this build lacks, CRC made right
        let crc = more_crc(&bad);
        bad[MORE_CRC_AT..MORE_CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Saved::from_sector(&bad), Some(Saved { more: [None, None, None, None, None, Some(p)], ..s }));
    }

    #[test]
    fn torn_at_more_keeps_no_mix() {
        let p = Page::preset(4, [Speed, Hr, Dist, Timer, Time, Time, Time, Time]).unwrap();
        let q = Page::preset(2, [Ascent, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
        let w = Page::preset(14, [Wbal, Np, Hr, Power, Cadence, Dist, Timer, Ascent]).unwrap();
        let mk = |seq, pg, more| Saved {
            seq,
            pages: [pg; crate::PAGES],
            count: 3,
            words: [0; 6],
            was: [0; 6],
            more,
            indoor: None,
        };
        let old = mk(4, p, [None, Some(w), None, None, None, None]).to_sector();
        let new = mk(6, q, [None; crate::PAGES]).to_sector();
        let mut torn = old;
        torn[..MORE_AT].copy_from_slice(&new[..MORE_AT]);
        let s = Saved::from_sector(&torn).unwrap();
        assert_eq!(s.pages[0], q);
        assert_eq!(s.more, [None; crate::PAGES], "new pages with an older copy's MORE");
    }

    /// The indoor part goes to the file and back; a file of an earlier build (zeros after the MORE part), a damaged
    /// indoor part, a page of it this build cannot read and a copy torn between the parts give the indoor defaults
    /// (None), every part above it intact; a torn copy never mixes pages with another copy's indoor set.
    #[test]
    fn the_indoor_set_in_the_file() {
        let p = Page::preset(4, F).unwrap();
        let m = Page::preset(14, F).unwrap();
        let mut set = Set::indoor_default();
        set.count = 4;
        set.pages[3] = p;
        set.more[1] = Some(m);
        let s = Saved {
            seq: 5,
            pages: [p; crate::PAGES],
            count: 3,
            words: [1; 6],
            was: [2; 6],
            more: [None, Some(m), None, None, None, None],
            indoor: Some(set),
        };
        let sector = s.to_sector();
        const { assert!(INDOOR_CRC_AT + 2 <= 512) } // fits the sector
        assert_eq!(Saved::from_sector(&sector), Some(s));
        let plain = Saved { indoor: None, ..s };
        let mut old = sector;
        old[INDOOR_AT..].fill(0); // as an earlier build wrote it
        assert_eq!(Saved::from_sector(&old), Some(plain));
        for at in [INDOOR_AT, INDOOR_AT + 5, INDOOR_MORE_AT + 3, INDOOR_CRC_AT] {
            let mut bad = sector;
            bad[at] ^= 0x10;
            assert_eq!(Saved::from_sector(&bad), Some(plain), "damaged at {at}");
        }
        let mut bad = sector;
        bad[INDOOR_AT + 1 + ROWS] = 250; // a field this build lacks, CRC made right
        let crc = indoor_crc(&bad);
        bad[INDOOR_CRC_AT..INDOOR_CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Saved::from_sector(&bad), Some(plain));
        // the MORE part damaged: the indoor part (chained to it) goes too, the pages stay
        let mut bad = sector;
        bad[MORE_AT + 3] ^= 0x10;
        assert_eq!(Saved::from_sector(&bad), Some(Saved { more: [None; crate::PAGES], ..plain }));
        // torn before a part: the pages (or MORE) of the newer copy, the older copy's indoor set refused
        let q = Page::preset(2, [Ascent, Grade, Time, Time, Time, Time, Time, Time]).unwrap();
        let new = Saved { seq: 6, pages: [q; crate::PAGES], ..s }.to_sector();
        for (at, what) in [(INDOOR_AT, "the indoor part"), (MORE_AT, "the MORE part")] {
            let mut torn = sector;
            torn[..at].copy_from_slice(&new[..at]);
            assert_eq!(Saved::from_sector(&torn).map(|t| t.indoor), Some(None), "torn before {what}");
        }
    }
}
