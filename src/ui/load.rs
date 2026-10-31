//! The TRAINING LOAD screen: 90 days of CTL (orange line), ATL (yellow line) and TSB (bars, green for form over 0, red
//! under), two columns a day, with a cursor under the plot that names one day. The plot is one blit; the cursor moves in
//! its own strip and readout, never redrawing the plot. Colours are the built-in roles, which a theme maps.

use super::*;
use crate::load::{Day, SHOWN};
use text::{Align, Line, Style, draw, draw_in};

/// The plot's left, top, columns (two a day) and rows.
pub const PLOT: (u32, u32, u32, u32) = (40, LIST_Y + 24, 2 * SHOWN as u32, 120);
const STRIP_Y: u32 = PLOT.1 + PLOT.3 + 4;
const STRIP_H: u32 = 8;
const READ_Y: u32 = STRIP_Y + STRIP_H + 4;
const READ_H: u32 = 18;

/// The day under screen column `x`.
pub fn day_at(x: u16) -> usize {
    ((u32::from(x).saturating_sub(PLOT.0)) / 2).min(SHOWN as u32 - 1) as usize
}

/// The value axis: from `lo` (0 or under, for a negative form) to `hi`, in steps of 10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axis {
    /// Bottom value.
    pub lo: i32,
    /// Top value.
    pub hi: i32,
}

impl Axis {
    /// Fits every CTL, ATL and TSB of `d`: 0 included, at least 20 high.
    pub fn of(d: &[Day]) -> Axis {
        let top = d.iter().map(|d| i32::from(d.ctl.max(d.atl))).max().unwrap_or(0);
        let bottom = d.iter().map(|d| i32::from(d.tsb)).min().unwrap_or(0);
        Axis { hi: ((top.max(0) + 9) / 10 * 10).max(20), lo: (bottom.min(0) - 9) / 10 * 10 }
    }

    /// The plot row (from its top) of value `v`.
    pub fn row(self, v: i32) -> u32 {
        let h = PLOT.3 as i32 - 1;
        (h - (v.clamp(self.lo, self.hi) - self.lo) * h / (self.hi - self.lo).max(1)).clamp(0, h) as u32
    }
}

/// The whole screen: title, legend, plot, cursor and readout. `date` is the cursor day's.
pub fn draw_screen(c: &mut impl Canvas, d: &[Day; SHOWN], cursor: usize, ready: bool, date: &str) {
    draw_title(c, "TRAINING LOAD");
    if ready {
        let sty = |fg| Style { fg, ..LABEL };
        draw(c, &sty(ACCENT), 8, LIST_Y + 2, Align::Left, "CTL");
        draw(c, &sty(YELLOW), 8 + 4 * 12, LIST_Y + 2, Align::Left, "ATL");
        draw(c, &sty(GREEN), 8 + 8 * 12, LIST_Y + 2, Align::Left, "TSB");
        draw(c, &sty(GREY), W - 8, LIST_Y + 2, Align::Right, "90 D");
    } else {
        draw_in(c, &LABEL, 0, LIST_Y + 2, W, Align::Centre, "READ RIDES FIRST");
    }
    draw_plot(c, d);
    draw_cursor(c, d, cursor, date);
}

/// The plot: a grid, the zero line, the form's bars under the two lines, in one window; the axis' numbers beside it.
pub fn draw_plot(c: &mut impl Canvas, d: &[Day; SHOWN]) {
    let ax = Axis::of(d);
    let zero = ax.row(0);
    let rows = |f: fn(&Day) -> i16| -> [u8; SHOWN] {
        core::array::from_fn(|i| d.get(i).map_or(0, |d| ax.row(i32::from(f(d)))) as u8)
    };
    let (ctl, atl, tsb) = (rows(|d| d.ctl), rows(|d| d.atl), rows(|d| d.tsb));
    let (bg, grid, zc, cc, ca, up, down) =
        (c.ink(BLACK), c.ink(DARK), c.ink(GREY), c.ink(ACCENT), c.ink(YELLOW), c.ink(GREEN), c.ink(RED));
    let on = |y: &[u8; SHOWN], i: usize, py: u32| {
        // a day covers its row and the next day's (a steep part has no gaps), two rows thick
        let Some(v) = y.get(i).copied().map(u32::from) else { return false };
        let n = y.get(i + 1).copied().map_or(v, u32::from);
        (v.min(n)..=v.max(n) + 1).contains(&py)
    };
    c.blit(PLOT.0, PLOT.1, PLOT.2, PLOT.3, &mut |x, y| {
        let (i, left) = ((x / 2) as usize, x % 2 == 0);
        let bar =
            tsb.get(i).copied().map(u32::from).filter(|_| left).filter(|&t| (t.min(zero)..=t.max(zero)).contains(&y));
        if on(&ctl, i, y) {
            cc
        } else if on(&atl, i, y) {
            ca
        } else if let Some(t) = bar {
            if t <= zero { up } else { down }
        } else if y == zero {
            zc
        } else if y % (PLOT.3 / 4) == 0 || y == PLOT.3 - 1 || (x % 60 == 0 && x > 0) {
            grid
        } else {
            bg
        }
    });
    let grey = Style { fg: GREY, ..LABEL };
    let mut mark = |v: i32, y: u32| {
        let mut l = Line::<12>::new();
        if v < 0 {
            l.push_str("-");
        }
        l.push_u32(v.unsigned_abs());
        draw_in(c, &grey, 0, y, PLOT.0 - 2, Align::Right, l.as_str());
    };
    mark(ax.hi, PLOT.1);
    mark(0, PLOT.1 + zero.saturating_sub(7).min(PLOT.3 - 14));
    if ax.lo < 0 {
        mark(ax.lo, PLOT.1 + PLOT.3 - 14);
    }
}

/// The cursor's strip (a tick a week back from today, a white mark at the chosen day) and the readout under it.
pub fn draw_cursor(c: &mut impl Canvas, d: &[Day; SHOWN], cursor: usize, date: &str) {
    c.fill(0, STRIP_Y, W, STRIP_H, BLACK);
    for k in (0..SHOWN).rev().step_by(7) {
        c.fill(PLOT.0 + 2 * k as u32, STRIP_Y, 1, 3, GREY);
    }
    let x = (PLOT.0 + 2 * cursor as u32 + 1).clamp(2, W - 3);
    c.fill(x - 2, STRIP_Y, 5, STRIP_H, WHITE);
    let day = d.get(cursor).copied().unwrap_or_default();
    let sty = |fg| Style { fg, ..LABEL };
    let num = |tag: &str, v: i32| {
        let mut l = Line::<16>::new();
        l.push_str(tag);
        l.push_str(" ");
        if v < 0 {
            l.push_str("-");
        }
        l.push_u32(v.unsigned_abs());
        l
    };
    draw_in(c, &sty(WHITE), 8, READ_Y, 120, Align::Left, date);
    draw_in(c, &sty(GREY), 128, READ_Y, W - 136, Align::Right, num("TSS", i32::from(day.tss)).as_str());
    draw_in(c, &sty(ACCENT), 8, READ_Y + READ_H, 112, Align::Left, num("CTL", i32::from(day.ctl)).as_str());
    draw_in(c, &sty(YELLOW), 120, READ_Y + READ_H, W - 128, Align::Left, num("ATL", i32::from(day.atl)).as_str());
    let form = if day.tsb < 0 { RED } else { GREEN };
    draw_in(c, &sty(form), 8, READ_Y + 2 * READ_H, 112, Align::Left, num("TSB", i32::from(day.tsb)).as_str());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{Frame, metric_ends};

    fn days(f: impl Fn(i32) -> (i16, i16, i16)) -> [Day; SHOWN] {
        core::array::from_fn(|i| {
            let (ctl, atl, tsb) = f(i as i32);
            Day { tss: 60, ctl, atl, tsb }
        })
    }

    #[test]
    fn the_axis_holds_every_value() {
        let d = days(|i| (40 + i as i16 / 3, 60 + i as i16 / 2, -25 + i as i16 / 2));
        let a = Axis::of(&d);
        assert_eq!((a.hi, a.lo), (110, -30));
        assert_eq!((a.row(a.hi), a.row(a.lo)), (0, PLOT.3 - 1));
        assert!(a.row(0) > 0 && a.row(0) < PLOT.3 - 1);
        assert_eq!(Axis::of(&days(|_| (0, 0, 0))), Axis { lo: 0, hi: 20 }, "an empty chart still has a scale");
        assert_eq!(day_at(PLOT.0 as u16), 0);
        assert_eq!(day_at(W as u16 - 1), SHOWN - 1);
    }

    /// Both lines, the bars and the cursor draw at both ends of the theme metrics without leaving the screen, and the
    /// cursor moves without touching the plot.
    #[test]
    fn draws_lines_bars_and_cursor() {
        for t in metric_ends() {
            let mut f = Frame::themed(t);
            let d = days(|i| (30 + i as i16 / 2, 50 + (i as i16 % 20) * 2, -20 + i as i16 / 3));
            draw_screen(&mut f, &d, 40, true, "FRI 23 OCT");
            assert!(f.filled() > 25_000);
            let has = |col: u16| f.px.iter().any(|&p| p == f.theme.ink(col));
            assert!(has(ACCENT) && has(YELLOW) && has(GREEN) && has(RED) && has(WHITE));
            let mut g = Frame::themed(t);
            draw_cursor(&mut g, &d, 3, "FRI 23 OCT");
            assert!(g.filled() < 16_000, "the cursor strip and readout only: {}", g.filled());
        }
    }
}
