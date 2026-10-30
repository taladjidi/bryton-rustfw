//! What the rider sees of a live segment (docs/segments.md, "Screens"; step S4): the banners for the events of
//! app/segments.rs, the head box of the CLIMB page ([`Field::SegHead`]; the page itself is one with the climb pacer's:
//! app/profile.rs opens it at a segment's start, and it leaves the round at the finish or the loss), the result card
//! at the finish (any key, or [`CARD_MS`]), and the rows of SETTINGS > SEGMENTS. Drawing is ui/segment.rs. Nothing
//! here writes to the card.

use crate::segment::{Effort, Header, KEPT};
use crate::segment_match::Event;
use crate::*;

/// The result card stays this long (ms) unless a key closes it.
const CARD_MS: u32 = 10_000;
/// Rows of the list read from the card at a time (a screen of the menu has [`ui::MENU_ROWS`]).
pub(crate) const LIST_ROWS: usize = 6;
const _: () = assert!(ui::MENU_ROWS <= LIST_ROWS);
/// Rows of the SEGMENTS menu before the list: LIVE SEGMENTS and TARGET.
pub(crate) const FIXED_ROWS: usize = 2;

/// A segment of the list: what the header says.
#[derive(Clone, Copy, Default)]
pub(crate) struct ListRow {
    name: [u8; 12],
    dist_m: u32,
    gain_m: u16,
    /// The header could not be read (a ride runs, or the file went).
    ok: bool,
}

/// The screens' state: the page's latch, the events seen, the card, the list's window.
pub(crate) struct SegUi {
    /// A segment is being ridden and its page was offered (it stays latched until the segment ends).
    on: bool,
    /// The screen the card returns to.
    prev: Screen,
    /// Events handled ([`Segments::count`]).
    seen: usize,
    /// The segment to say "approaching" for: the pass loop reads its name from the card.
    pub(crate) announce: Option<u32>,
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

/// The ASCII start of `b` (a name is ASCII in the files; a byte past it cuts the name there, so a name never goes
/// blank).
fn ascii(b: &[u8]) -> &str {
    let n = b.iter().take_while(|c| c.is_ascii()).count();
    core::str::from_utf8(b.get(..n).unwrap_or(&[])).unwrap_or("")
}

impl SegUi {
    /// The screen the page returns to (tests).
    #[cfg(test)]
    pub(crate) fn set_prev(&mut self, s: Screen) {
        self.prev = s;
    }
}

/// The head box's lines, owned (the box borrows them: [`HeadText::head`]).
pub(crate) struct HeadText {
    top: Line<32>,
    big: Line<8>,
    ink: u16,
    sub: Line<16>,
}

impl HeadText {
    /// The lines as the box draws them.
    pub(crate) fn head(&self) -> ui::segment::Head<'_> {
        ui::segment::Head { top: self.top.as_str(), big: self.big.as_str(), ink: self.ink, sub: self.sub.as_str() }
    }
}

impl App {
    /// What the head box of the CLIMB page shows: the segment ridden (its name, the delta against the target, the
    /// target's label), else the climb (its title, the time to the top); None with neither.
    pub(crate) fn head_text(&self) -> Option<HeadText> {
        let mut h = HeadText { top: Line::new(), big: Line::new(), ink: ui::WHITE, sub: Line::new() };
        if let Some(n) = self.segment_now() {
            h.top.push_str(ascii(n.name.get(..n.name.len().min(32)).unwrap_or(&[])));
            h.big.push_str(ui::segment::delta_text(n.delta_s).as_str());
            h.ink = ui::segment::delta_color(n.delta_s);
            match n.target {
                Some(e) => {
                    h.sub.push_str("VS ");
                    h.sub.push_str(effort_label(e));
                }
                None => h.sub.push_str("NO TARGET"),
            }
            return Some(h);
        }
        let k = self.climb_view()?.climb;
        h.top.push_str("CLIMB ");
        ui::card::push_km(&mut h.top, k.len_m);
        h.top.push_str(" KM ");
        h.top.push_u32(k.grade_x10().div_ceil(10).max(1));
        h.top.push_str("%");
        match self.pace_now(0).pace {
            // in steps of 5 s: the plan is an estimate, and a big digit a second is the page's dearest redraw
            Some(s) if self.pace.plan.n > 0 => {
                h.big.push_str(ui::segment::time_text(u32::from(s).next_multiple_of(5).min(3599)).as_str())
            }
            _ => h.big.push_str("--"),
        }
        h.sub.push_str("TO THE TOP");
        Some(h)
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
        // as app/segments.rs writes the PR file: kept, and faster than the PR loaded and this ride's earlier efforts
        let new_pr = s.new_pr();
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
        // the page is the CLIMB page: app/profile.rs opens it for `climb.seg`, and it leaves the round when the
        // segment is over
        let ridden = self.segment_now().is_some();
        if ridden && !self.seg_ui.on {
            (self.seg_ui.on, self.climb.seg) = (true, true);
        } else if !ridden && self.seg_ui.on {
            (self.seg_ui.on, self.climb.seg) = (false, false);
            self.close_segment_page(b);
        }
        if self.screen == Screen::SegmentResult {
            self.seg_ui.card_ms = self.seg_ui.card_ms.saturating_add(self.dt_ms);
            if self.seg_ui.card_ms >= CARD_MS {
                self.leave_segment(b);
            }
        }
    }

    /// The segment ended (finished, lost, dropped, LIVE SEGMENTS off): the card if it finished, over the screen it
    /// ends on; else nothing (the CLIMB page leaves the round and the screen moves on).
    fn close_segment_page(&mut self, b: &mut impl Board) {
        let card = self.seg_ui.result.is_some()
            && matches!(self.screen, Screen::Main | Screen::Ride(_))
            && self.segments().is_some_and(|s| self.seg_ui.result.is_some_and(|r| s.seg().header.id == r.0));
        if card {
            self.seg_ui.prev = self.screen;
            self.screen = Screen::SegmentResult;
            self.seg_ui.card_ms = 0;
            self.show(b);
        } else {
            self.seg_ui.result = None;
        }
    }

    /// Back to the screen the card came from (the page after the CLIMB page's anchor if that one left the round,
    /// the first ride page if another is off now).
    pub(crate) fn leave_segment(&mut self, b: &mut impl Board) {
        self.seg_ui.result = None;
        self.screen = match self.seg_ui.prev {
            Screen::Ride(CLIMB) if !self.page_on(CLIMB) => self.ride_page(place(self.climb.after) + 1, 1),
            Screen::Ride(n) if n != OVERVIEW && !self.page_on(n) => self.ride_page(0, 1),
            s => s,
        };
        self.show(b);
    }

    /// The card on screen. Start and Lap are the ride's (pause, lap), never swallowed while a ride runs (the card
    /// closes first); the long presses (Lap held, Page held, Start held) go to the ride page the screen was over,
    /// after it closes; Page and Lap still held are ignored; while recording a tap, hold or drag is ignored (rain,
    /// gloves) and only a swipe closes. Anything else closes.
    pub(crate) fn input_segment(
        &mut self,
        b: &mut impl Board,
        i: Input,
        recording: bool,
        graph_hit: Option<(usize, Field)>,
        shown: Seen,
    ) {
        let running = self.ride.state() != ride::State::Idle;
        match i {
            Input::NextHeld | Input::PrevHeld => {}
            Input::Touch(Gesture::Tap(..) | Gesture::Hold(..) | Gesture::Drag(..)) if recording => {}
            Input::Select | Input::Prev if running => {
                self.leave_segment(b);
                if i == Input::Select { self.start_key(b) } else { self.lap() }
            }
            Input::Media | Input::Menu | Input::End => {
                self.leave_segment(b);
                self.input_page(b, i, recording, graph_hit, shown);
            }
            _ => self.leave_segment(b),
        }
    }

    /// The pass loop's part: the name of a segment the rider approaches (a mount, a folder lookup and one sector of the card: 3 KB in the sim), and the rows of
    /// SEGMENTS menu that are on screen. Nothing while a ride runs but the name (the card is read, never written).
    pub(crate) fn step_segment_ui(&mut self, b: &mut impl Board) {
        let announce = self.seg_ui.announce.take();
        let window = match self.screen {
            Screen::Menu(Menu::Segments, sel) if self.ride.state() == ride::State::Idle => {
                let f = menu::first_shown(sel, self.menu_rows(), self.rows(Menu::Segments));
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
                        let mut name = [0u8; 12];
                        for (o, i) in name.iter_mut().zip(h.name()) {
                            *o = *i;
                        }
                        ListRow {
                            name,
                            dist_m: h.distance_m,
                            gain_m: u16::try_from(h.elevation_m).unwrap_or(u16::MAX),
                            ok: true,
                        }
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
                value.push_u32(u32::from(r.gain_m));
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
