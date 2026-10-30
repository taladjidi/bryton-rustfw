//! What the rider sees of a live segment (docs/segments.md, "Screens"; step S4): the banners for the events of
//! app/segments.rs, the SEGMENT page that opens at a segment's start and closes at its end (as the CLIMB page does at
//! a climb: once per start, only over a ride page, only while recording, and not again once the rider dismissed it),
//! the result card at the finish (any key, or [`CARD_MS`]), and the rows of SETTINGS > SEGMENTS. Drawing is
//! ui/segment.rs: the page redraws once a second, boxes that changed only. Nothing here writes to the card.

use crate::segment::{Effort, Header, KEPT};
use crate::segment_match::Event;
use crate::*;

/// The result card stays this long (ms) unless a key closes it.
const CARD_MS: u32 = 10_000;
/// Rows of the list read from the card at a time (a screen of the menu has [`ui::MENU_ROWS`]).
pub(crate) const LIST_ROWS: usize = 8;
const _: () = assert!(ui::MENU_ROWS <= LIST_ROWS);
/// Rows of the SEGMENTS menu before the list: LIVE SEGMENTS and TARGET.
pub(crate) const FIXED_ROWS: usize = 2;

/// A segment of the list: what the header says.
#[derive(Clone, Copy, Default)]
pub(crate) struct ListRow {
    name: [u8; 16],
    dist_m: u32,
    gain_m: u32,
    /// The header could not be read (a ride runs, or the file went).
    ok: bool,
}

/// The screens' state: the page's latch, the events seen, the card, the list's window.
pub(crate) struct SegUi {
    /// A segment is being ridden and its page was offered (it stays latched until the segment ends).
    on: bool,
    /// The screen the page or the card returns to.
    prev: Screen,
    /// Events handled ([`Segments::count`]).
    seen: usize,
    /// The segment to say "approaching" for: the pass loop reads its name from the card.
    pub(crate) announce: Option<u32>,
    shown: ui::segment::Shown,
    /// The second the page was last updated at.
    second: u32,
    card_ms: u32,
    /// The finish: segment id, time, and the deltas against the kept targets.
    result: Option<(u32, u32, [Option<i32>; KEPT])>,
    /// The list's window: the first segment read, and the rows (`filled`: they were read for this menu visit).
    first: usize,
    pub(crate) filled: bool,
    rows: [ListRow; LIST_ROWS],
}

impl SegUi {
    /// Nothing ridden, nothing read.
    pub(crate) fn new() -> Self {
        SegUi {
            on: false,
            prev: Screen::Main,
            seen: 0,
            announce: None,
            shown: ui::segment::Shown::default(),
            second: 0,
            card_ms: 0,
            result: None,
            first: 0,
            filled: false,
            rows: [ListRow::default(); LIST_ROWS],
        }
    }
}

/// The label of an effort.
fn effort_label(e: Effort) -> &'static str {
    match e {
        Effort::Kom => "KOM",
        Effort::Qom => "QOM",
        Effort::Pr => "PR",
        Effort::Carrot => "CARROT",
        Effort::Wolf => "WOLF",
        Effort::Recent => "RECENT",
        Effort::Best5 => "BEST OF 5",
    }
}

fn ascii(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("")
}

impl App {
    /// `f` over what the SEGMENT page shows now; None when no segment is ridden.
    fn with_segment_now<R>(&self, f: impl FnOnce(&ui::segment::Now) -> R) -> Option<R> {
        let n = self.segment_now()?;
        Some(f(&ui::segment::Now {
            name: ascii(n.name),
            delta_s: n.delta_s,
            target: n.target.map_or("", effort_label),
            elapsed_s: n.elapsed_s,
            left_m: n.left_m,
            along_m: n.along_m,
            ghost_m: n.ghost_m,
            total_m: n.along_m.saturating_add(n.left_m),
        }))
    }

    /// The page, drawn whole (`show` then notes what it left: [`App::segment_printed`]).
    pub(crate) fn draw_segment(&self, c: &mut impl Canvas) {
        self.with_segment_now(|n| ui::segment::draw(c, n));
    }

    /// What the page shows, as drawn (set when the screen is shown).
    pub(crate) fn segment_printed(&mut self) {
        let mut s = ui::segment::Shown::default();
        self.with_segment_now(|n| ui::segment::update(&mut NoCanvas, n, &mut s));
        self.seg_ui.shown = s;
    }

    /// The result card, drawn whole.
    pub(crate) fn draw_segment_result(&self, c: &mut impl Canvas) {
        let Some(s) = self.segments() else { return };
        let (Some((id, elapsed_s, deltas)), seg) = (self.seg_ui.result, s.seg()) else { return };
        if seg.header.id != id {
            return; // another segment loaded since
        }
        let mut rows = [None; KEPT];
        for ((row, e), d) in rows.iter_mut().zip(seg.kept).zip(deltas) {
            *row = e.zip(d).map(|(e, d): (Effort, i32)| (effort_label(e), d));
        }
        let new_pr = seg
            .kept
            .iter()
            .zip(deltas)
            .any(|(&e, d): (&Option<Effort>, Option<i32>)| e == Some(Effort::Pr) && d.is_some_and(|d| d < 0));
        ui::segment::draw_card(c, &ui::segment::Card { name: ascii(seg.header.name()), elapsed_s, rows, new_pr });
    }

    /// Each pass: the events of the matcher said, the page opened or closed, the card timed, the page's second.
    pub(crate) fn settle_segment(&mut self, b: &mut impl Board) {
        let Some(s) = self.segments.as_deref() else { return };
        // the new events (at most the ring's), oldest first
        let total = s.count();
        let (kept, fresh) = (total.min(NOTES), total.wrapping_sub(self.seg_ui.seen).min(NOTES));
        let mut evs = [None; NOTES];
        for (slot, (_, e)) in evs.iter_mut().zip(s.events().skip(kept - fresh)) {
            *slot = Some(e);
        }
        let finish_delta = |d: [Option<i32>; KEPT]| s.column().and_then(|c| d.get(c).copied().flatten());
        let mut said = [None::<(Line<40>, Banner)>; 2];
        let mut result = None;
        for ev in evs.into_iter().flatten() {
            match ev {
                Event::Approach(id) => self.seg_ui.announce = Some(id),
                Event::Lost(_) => {
                    let mut l = Line::new();
                    l.push_str("SEGMENT LOST");
                    said[0] = Some((l, Banner::Ride));
                }
                Event::Finish { id, elapsed_s, deltas } => {
                    let mut l = Line::new();
                    l.push_str("SEGMENT ");
                    l.push_str(ui::segment::time_text(elapsed_s).as_str());
                    if let Some(d) = finish_delta(deltas) {
                        l.push_str(" ");
                        l.push_str(ui::segment::delta_text(Some(d)).as_str());
                    }
                    said[1] = Some((l, Banner::Summary));
                    result = Some((id, elapsed_s, deltas));
                }
                Event::Start(_) => {}
            }
        }
        self.seg_ui.seen = total;
        if result.is_some() {
            self.seg_ui.result = result;
        }
        for (l, kind) in said.into_iter().flatten() {
            self.say_as(l.as_str(), kind);
        }
        let ridden = self.segment_now().is_some();
        if ridden && !self.seg_ui.on {
            self.seg_ui.on = true;
            self.open_segment_page(b);
        } else if !ridden && self.seg_ui.on {
            self.seg_ui.on = false;
            self.close_segment_page(b);
        }
        let second = self.up_ms / 1000;
        match self.screen {
            Screen::Segment if second != self.seg_ui.second => {
                self.seg_ui.second = second;
                let mut shown = self.seg_ui.shown;
                let (area, look) = (self.area(), self.look());
                self.with_segment_now(|n| ui::segment::update(&mut Panel(b, area, look), n, &mut shown));
                self.seg_ui.shown = shown;
            }
            Screen::SegmentResult => {
                self.seg_ui.card_ms = self.seg_ui.card_ms.saturating_add(self.dt_ms);
                if self.seg_ui.card_ms >= CARD_MS {
                    self.leave_segment(b);
                }
            }
            _ => {}
        }
    }

    /// The segment started: the page opens over a ride page of a recording ride, once (a menu or other screen up: not
    /// for this segment; a finger or key down: the next pass tries again, as the CLIMB page does).
    fn open_segment_page(&mut self, b: &mut impl Board) {
        if !matches!(self.screen, Screen::Main | Screen::Ride(_)) || self.ride.state() != ride::State::Running {
            return;
        }
        if self.touch.down() || self.keys.any_down() {
            self.seg_ui.on = false; // tried again at the next pass
            return;
        }
        (self.seg_ui.prev, self.screen) = (self.screen, Screen::Segment);
        self.seg_ui.second = self.up_ms / 1000;
        self.show(b);
    }

    /// The segment ended (finished, lost, dropped, LIVE SEGMENTS off): the card if it finished, else back.
    fn close_segment_page(&mut self, b: &mut impl Board) {
        let card = self.seg_ui.result.is_some()
            && matches!(self.screen, Screen::Segment | Screen::Main | Screen::Ride(_))
            && self.segments().is_some_and(|s| self.seg_ui.result.is_some_and(|r| s.seg().header.id == r.0));
        if card {
            if self.screen != Screen::Segment {
                self.seg_ui.prev = self.screen;
            }
            self.screen = Screen::SegmentResult;
            self.seg_ui.card_ms = 0;
            self.show(b);
        } else {
            self.seg_ui.result = None;
            if self.screen == Screen::Segment {
                self.leave_segment(b);
            }
        }
    }

    /// Back to the screen the page or the card came from (the first ride page if that one is off now).
    pub(crate) fn leave_segment(&mut self, b: &mut impl Board) {
        self.seg_ui.result = None;
        self.screen = match self.seg_ui.prev {
            Screen::Ride(n) if n < PAGES && !self.page_on(n) => self.ride_page(0, 1),
            s => s,
        };
        self.show(b);
    }

    /// The page or the card on screen: Start and Lap keep the ride's meaning on the page; any other input closes it,
    /// and so does any input on the card.
    pub(crate) fn input_segment(&mut self, b: &mut impl Board, i: Input) {
        let running = self.ride.state() != ride::State::Idle;
        match (self.screen, i) {
            (Screen::Segment, Input::Select) if running => self.start_key(b),
            (Screen::Segment, Input::Prev) if running => self.lap(),
            _ => self.leave_segment(b),
        }
    }

    /// The pass loop's part: the name of a segment the rider approaches (one sector of the card), and the rows of
    /// SEGMENTS menu that are on screen. Nothing while a ride runs but the name (the card is read, never written).
    pub(crate) fn step_segment_ui(&mut self, b: &mut impl Board) {
        let announce = self.seg_ui.announce.take();
        let window = match self.screen {
            Screen::Menu(Menu::Segments, sel) if self.ride.state() == ride::State::Idle => {
                let f = menu::first_shown(sel, ui::MENU_ROWS, self.rows(Menu::Segments));
                Some(f.saturating_sub(FIXED_ROWS))
            }
            _ => {
                self.seg_ui.filled = false;
                None
            }
        };
        let refill = window.filter(|&w| !self.seg_ui.filled || w != self.seg_ui.first);
        let Some(s) = self.segments.as_deref() else { return };
        if announce.is_none() && refill.is_none() {
            return;
        }
        let mut d = emmc::Volume(&mut self.emmc, b);
        let mut line = None;
        if let Some(h) = announce.and_then(|id| Segments::header(&mut d, id)) {
            let mut l = Line::<40>::new();
            let name: &[u8] = h.name();
            l.push_str("SEGMENT ");
            l.push_str(ascii(name.get(..name.len().min(24)).unwrap_or(&[])));
            l.push_str(" 300 M");
            line = Some(l);
        }
        if let Some(w) = refill {
            self.seg_ui.first = w;
            self.seg_ui.filled = true;
            for (k, row) in self.seg_ui.rows.iter_mut().enumerate() {
                *row = s.id_at(w + k).and_then(|id| Segments::header(&mut d, id)).map_or_else(
                    ListRow::default,
                    |h: Header| {
                        let mut name = [0u8; 16];
                        for (o, i) in name.iter_mut().zip(h.name()) {
                            *o = *i;
                        }
                        ListRow { name, dist_m: h.distance_m, gain_m: h.elevation_m, ok: true }
                    },
                );
            }
        }
        if let Some(l) = line {
            self.say_as(l.as_str(), Banner::Ride);
        }
    }

    /// Row `k` of the SEGMENTS list: its id as the label until the header is read (or while a ride runs), then the
    /// name, with the distance and the gain as the value ("12.3K 250M").
    pub(crate) fn segment_row(&self, k: usize, label: &mut Line<16>, value: &mut Line<12>) -> Option<()> {
        let id = self.segments()?.id_at(k)?;
        let row = k
            .checked_sub(self.seg_ui.first)
            .and_then(|j| self.seg_ui.rows.get(j))
            .filter(|r| r.ok && self.seg_ui.filled);
        match row {
            Some(r) => {
                let n = r.name.iter().position(|&c| c == 0).unwrap_or(r.name.len());
                label.push_str(ascii(r.name.get(..n)?));
                value.push_u32(r.dist_m / 1000);
                value.push_str(".");
                value.push_u32(r.dist_m % 1000 / 100);
                value.push_str("K ");
                value.push_u32(r.gain_m);
                value.push_str("M");
            }
            None => {
                label.push_u32(id);
                value.push_str("--");
            }
        }
        Some(())
    }
}

/// A canvas that draws nothing: what a page would leave on screen, without drawing it.
struct NoCanvas;
impl Canvas for NoCanvas {
    fn fill(&mut self, _: u32, _: u32, _: u32, _: u32, _: u16) {}
    fn blit(&mut self, _: u32, _: u32, _: u32, _: u32, _: &mut dyn FnMut(u32, u32) -> u16) {}
}
