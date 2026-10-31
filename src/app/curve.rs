//! The POWER CURVE screen in the app (ui/curve.rs draws it).
//!
//! - **The 90-day curve** (orange) is stock's rides' power curve of the last [`stock::WINDOW_DAYS`] days, what the
//!   scan of FITNESS keeps (`App::stock.out.curve`, the same bests CP and W' are fitted on); empty until the scan
//!   has run. There is no all-time curve: the scan only keeps the season's.
//! - **This ride's** (green) is [`power_curve::Rolling`], fed one sample a second while the ride runs
//!   (app/riding.rs `second`): 13 running sums over two rings (3 KB), no rescan of the file. A ride resumed after a
//!   reset starts its curve again.
//! - **CP and W'** above are the fit's, trusted or not (FITNESS shows how well it fits).
//! - **Keys:** Page steps the cursor over the durations (round), Lap steps back (the ride's lap while riding, and
//!   Start stays the ride's), a tap on the plot or the strip picks the nearest duration, Back leaves.
//! - **Redraw:** the plot once when it opens and while a ride runs when this ride's bests changed (at most every 5 s);
//!   the cursor only redraws its strip and readout.

use crate::*;

/// The cursor on a 5-minute best when the screen opens.
const FIRST: usize = 7;
/// This ride's curve is drawn again at most this often, s.
const REDRAW_S: u32 = 5;

impl App {
    /// Opens the power curve.
    pub(crate) fn open_curve(&mut self) {
        self.curve_shown = self.ride_curve.best;
        self.open(Screen::Curve(FIRST));
    }

    /// The 90-day bests, empty until the scan has run.
    fn curve_best(&self) -> [u16; power_curve::N] {
        if self.stock.ready() { self.stock.out.curve.watts } else { [0; power_curve::N] }
    }

    /// "CP 250W W' 20.0KJ", or "CP --".
    fn curve_head(&self) -> Line<24> {
        let mut l = Line::<24>::new();
        match self.cp_fit() {
            Some(c) if self.stock.ready() => {
                l.push_str("CP ");
                l.push_u32(u32::from(c.cp));
                l.push_str("W W' ");
                let mut w = Line::<12>::new();
                push_fixed(&mut w, c.w_prime as i32, 3, 1);
                l.push_str(w.as_str());
                l.push_str("KJ");
            }
            _ => l.push_str("CP --"),
        }
        l
    }

    /// The whole screen.
    pub(crate) fn draw_curve(&self, c: &mut impl Canvas, cursor: usize) {
        ui::curve::draw_screen(c, self.curve_head().as_str(), &self.curve_best(), &self.ride_curve.best, cursor);
    }

    /// An input on the curve: Page and Lap step the cursor, a tap picks a duration, Back leaves.
    pub(crate) fn input_curve(&mut self, b: &mut impl Board, i: Input) {
        let Screen::Curve(k) = self.screen else { return };
        let riding = self.ride.state() != ride::State::Idle;
        let n = power_curve::N;
        let k = match i {
            _ if matches!(i, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right))) => return self.close_curve(b),
            Input::Touch(Gesture::Tap(x, y)) if inside(ui::BACK_TAP, x, y) => return self.close_curve(b),
            Input::Select if riding => return self.start_key(b),
            Input::Prev if riding => return self.lap(),
            Input::Next => (k + 1) % n,
            Input::Prev => (k + n - 1) % n,
            Input::Touch(Gesture::Tap(x, y)) if u32::from(y) >= ui::curve::PLOT.1 => ui::curve::nearest(x),
            _ => return,
        };
        if k != self.screen_cursor() {
            self.screen = Screen::Curve(k);
            let mut p = panel::Panel(b, self.area(), self.look());
            ui::curve::draw_cursor(&mut p, &self.curve_best(), &self.ride_curve.best, k);
        }
    }

    fn screen_cursor(&self) -> usize {
        if let Screen::Curve(k) = self.screen { k } else { usize::MAX }
    }

    fn close_curve(&mut self, b: &mut impl Board) {
        self.close();
        self.show(b);
    }

    /// Once a pass: while the screen is up during a ride, the plot again when this ride's bests changed.
    pub(crate) fn step_curve(&mut self, b: &mut impl Board) {
        let Screen::Curve(k) = self.screen else { return };
        if self.ride_curve.best != self.curve_shown && self.ride.timer_s().is_multiple_of(REDRAW_S) {
            self.curve_shown = self.ride_curve.best;
            let mut p = panel::Panel(b, self.area(), self.look());
            ui::curve::draw_plot(&mut p, &self.curve_best(), &self.ride_curve.best);
            ui::curve::draw_cursor(&mut p, &self.curve_best(), &self.ride_curve.best, k);
        }
    }
}
