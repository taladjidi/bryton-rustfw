//! QUICK STATUS (`Screen::Status`): a tap on a plain field of a ride page opens the rider's six fields (PAGES >
//! QUICK FIELDS, the page editor's picker; layout 2 x 3, settings.bin) over the page. Back, a tap, a swipe or
//! [`STATUS_MS`] without an input closes it, back on the same page. Start and Lap still work under it.
//!
//! Stock's Quick Status is the overview's widget list (the same as our swipe-down panel, `quick.rs`); a tap-opened
//! field view is this firmware's own (roadmap 3b), so its 10 s time-out is our choice, not stock's.
//!
//! A tap keeps its own action on: the 3 / 10 / 30 s power fields (cycle the window), the TRAINER MODE / TARGET fields
//! (open the trainer), every picture (the map's buttons and zoom, a graph's next plot), and the fixed pages
//! (course, workout, climb) and the overview, which are not `Screen::Ride(n)` with `n < PAGES`. The fields redraw
//! each second in their own boxes, as on a ride page ([`App::update_page`]).

use crate::*;

/// How long QUICK STATUS stays up without an input, ms.
pub(crate) const STATUS_MS: u32 = 10_000;

impl App {
    /// A tap at (`x`, `y`) is on a plain value field of the page shown, and QUICK STATUS is on: not a picture, not a
    /// field with its own tap action (those arms of `input_page` come first or are excluded here).
    pub(crate) fn quick_at(&self, x: u16, y: u16) -> bool {
        self.settings.quick_status
            && self.widgets().any(|(bx, w)| {
                matches!(w, Widget::Field(f) if !f.is_picture() && graph_plot(f).is_none()) && inside(bx, x, y)
            })
    }

    /// Opens QUICK STATUS over the page.
    pub(crate) fn open_status(&mut self, b: &mut impl Board) {
        self.open(Screen::Status);
        self.show(b);
    }

    /// Closes it, the page it was opened on drawn again.
    pub(crate) fn close_status(&mut self, b: &mut impl Board) {
        self.close();
        self.show(b);
    }

    /// An input on QUICK STATUS: Start and Lap act as on the page, any other key, a tap or a swipe closes it.
    pub(crate) fn input_status(&mut self, b: &mut impl Board, i: Input) {
        match i {
            Input::Select => self.start_key(b),
            Input::Prev => self.lap(),
            Input::Back | Input::Touch(Gesture::Tap(..) | Gesture::Swipe(_)) => self.close_status(b),
            _ => {}
        }
    }
}
