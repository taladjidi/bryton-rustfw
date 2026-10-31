//! The POWER CURVE screen: the best power for each duration from 1 s to 1 h on a log time axis, the 90-day curve
//! (orange) and this ride's (green) over it, CP and W' above, and a cursor under the plot that names one duration
//! ("5 MIN 312 W"). The plot is one blit (a window, one RAMWR); the cursor moves in its own strip and readout, never
//! redrawing the plot. Colours are the built-in roles, which a theme maps (LIGHT, OUTDOOR).

use super::*;
use crate::power_curve::{DURATIONS, N};
use text::{Align, Line, Style, draw, draw_in};

/// The plot's left, top, columns and rows.
pub const PLOT: (u32, u32, u32, u32) = (40, LIST_Y + 24, 194, 150);
const COLS: usize = PLOT.2 as usize;
const AXIS_Y: u32 = PLOT.1 + PLOT.3 + 3;
const STRIP_Y: u32 = AXIS_Y + 16;
const STRIP_H: u32 = 8;
const READ_Y: u32 = STRIP_Y + STRIP_H + 4;
/// The two readout lines' height each.
const READ_H: u32 = 18;
/// Where each duration sits on the axis, thousandths of the way from 1 s to 1 h (log scale: 1000 log2(d) / log2(3600)).
const X_PM: [u16; N] = [0, 197, 331, 415, 500, 585, 634, 697, 754, 803, 866, 915, 1000];
/// The vertical grid: 10 s, 1 min, 10 min.
const GRID_PM: [u16; 3] = [281, 500, 781];

/// A duration as text: "45 S", "5 MIN", "1 H".
pub fn dur_text(l: &mut Line<12>, s: u16) {
    let (n, unit) = match s {
        0..60 => (s, " S"),
        60..3600 => (s / 60, " MIN"),
        _ => (s / 3600, " H"),
    };
    l.push_u32(u32::from(n));
    l.push_str(unit);
}

/// The top of the watts axis for these curves: the highest best, up to the next 50 W (at least 100).
pub fn scale(a: &[u16; N], b: &[u16; N]) -> u16 {
    let top = a.iter().chain(b).copied().max().unwrap_or(0);
    (top.saturating_add(49) / 50 * 50).max(100)
}

/// The screen column (from the plot's left) of duration `i`.
pub fn col_of(i: usize) -> u32 {
    u32::from(X_PM.get(i).copied().unwrap_or(0)) * (PLOT.2 - 1) / 1000
}

/// The duration nearest to screen column `x`.
pub fn nearest(x: u16) -> usize {
    let c = u32::from(x).saturating_sub(PLOT.0);
    (0..N).min_by_key(|&i| col_of(i).abs_diff(c)).unwrap_or(0)
}

/// The row (from the plot's top) of each column of a curve, None where it has no value: the line between two bests
/// is straight in (log time, watts); it ends at the last duration held.
pub fn rows(w: &[u16; N], ymax: u16) -> [Option<u8>; COLS] {
    let mut out = [None; COLS];
    let h = PLOT.3 as i32 - 1;
    for (col, o) in out.iter_mut().enumerate() {
        let t = col as i32 * 1000 / (PLOT.2 as i32 - 1);
        let k = X_PM.iter().rposition(|&x| i32::from(x) <= t).unwrap_or(0);
        let (Some(&a), Some(&x0)) = (w.get(k), X_PM.get(k)) else { continue };
        let v = match (w.get(k + 1), X_PM.get(k + 1)) {
            _ if a == 0 => continue,
            (Some(&b), Some(&x1)) if b > 0 => {
                i32::from(a) + (i32::from(b) - i32::from(a)) * (t - i32::from(x0)) / (i32::from(x1) - i32::from(x0))
            }
            _ if t == i32::from(x0) => i32::from(a),
            _ => continue,
        };
        *o = Some((h - v * h / i32::from(ymax.max(1))).clamp(0, h) as u8);
    }
    out
}

/// The whole screen: title, CP line, plot, axis, cursor and readout.
pub fn draw_screen(c: &mut impl Canvas, head: &str, best: &[u16; N], ride: &[u16; N], cursor: usize) {
    draw_title(c, "POWER CURVE");
    draw_in(c, &LABEL, 0, LIST_Y + 2, W, Align::Centre, head);
    draw_plot(c, best, ride);
    let grey = Style { fg: GREY, ..LABEL };
    let px = |pm: u32| PLOT.0 + pm * (PLOT.2 - 1) / 1000;
    draw(c, &grey, PLOT.0, AXIS_Y, Align::Left, "1S");
    draw(c, &grey, px(500), AXIS_Y, Align::Centre, "1M");
    draw(c, &grey, px(781), AXIS_Y, Align::Centre, "10M");
    draw(c, &grey, PLOT.0 + PLOT.2 + 2, AXIS_Y, Align::Right, "1H");
    draw_cursor(c, best, ride, cursor);
}

/// The plot: a grid, the 90-day curve, this ride's over it, in one window; the watts at the top and the middle.
pub fn draw_plot(c: &mut impl Canvas, best: &[u16; N], ride: &[u16; N]) {
    let ymax = scale(best, ride);
    let (a, r) = (rows(best, ymax), rows(ride, ymax));
    let (bg, grid, ca, cr) = (c.ink(BLACK), c.ink(DARK), c.ink(ACCENT), c.ink(GREEN));
    let on = |y: &[Option<u8>; COLS], x: usize, py: u32| {
        // a column covers its row and the next column's (a steep part has no gaps), two rows thick
        let Some(v) = y.get(x).copied().flatten().map(u32::from) else { return false };
        let n = y.get(x + 1).copied().flatten().map_or(v, u32::from);
        (v.min(n)..=v.max(n) + 1).contains(&py)
    };
    let gx = GRID_PM.map(|pm| (u32::from(pm) * (PLOT.2 - 1) / 1000) as usize);
    c.blit(PLOT.0, PLOT.1, PLOT.2, PLOT.3, &mut |x, y| {
        let x = x as usize;
        if on(&r, x, y) {
            cr
        } else if on(&a, x, y) {
            ca
        } else if gx.contains(&x) || y % (PLOT.3 / 4) == 0 || y == PLOT.3 - 1 {
            grid
        } else {
            bg
        }
    });
    let grey = Style { fg: GREY, ..LABEL };
    // half size: four digits ("1400") fit the 38 px left of the plot where 2x text keeps three
    let small = Style { scale: 1, ..grey };
    for (v, y) in [(ymax, PLOT.1), (ymax / 2, PLOT.1 + PLOT.3 / 2 - 4)] {
        let mut l = Line::<12>::new();
        l.push_u32(u32::from(v));
        draw_in(c, &small, 0, y, PLOT.0 - 2, Align::Right, l.as_str());
    }
}

/// The cursor's strip (a tick for each duration, a white mark at the chosen one) and the readout under it.
pub fn draw_cursor(c: &mut impl Canvas, best: &[u16; N], ride: &[u16; N], cursor: usize) {
    c.fill(0, STRIP_Y, W, STRIP_H, BLACK);
    for i in 0..N {
        c.fill(PLOT.0 + col_of(i), STRIP_Y, 1, 3, GREY);
    }
    let x = (PLOT.0 + col_of(cursor)).clamp(2, W - 3);
    c.fill(x - 2, STRIP_Y, 5, STRIP_H, WHITE);
    let d = DURATIONS.get(cursor).copied().unwrap_or(1);
    for (tag, w, color, y) in
        [("90 D", best.get(cursor), ACCENT, READ_Y), ("RIDE", ride.get(cursor), GREEN, READ_Y + READ_H)]
    {
        let mut l = Line::<24>::new();
        let mut t = Line::<12>::new();
        dur_text(&mut t, d);
        l.push_str(t.as_str());
        l.push_str(" ");
        match w.filter(|&&w| w > 0) {
            Some(&w) => {
                l.push_u32(u32::from(w));
                l.push_str(" W");
            }
            None => l.push_str("--"),
        }
        draw_in(c, &Style { fg: color, ..LABEL }, 8, y, W - 16 - 56, Align::Left, l.as_str());
        draw_in(c, &Style { fg: GREY, ..LABEL }, W - 8 - 56, y, 56, Align::Right, tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::{Frame, metric_ends};

    fn curve(f: impl Fn(u32) -> u16) -> [u16; N] {
        core::array::from_fn(|i| f(u32::from(DURATIONS[i])))
    }

    #[test]
    fn axis_matches_durations() {
        for (i, &d) in DURATIONS.iter().enumerate() {
            let pm = 1000.0 * f64::from(d).log2() / 3600f64.log2();
            assert!((pm - f64::from(X_PM[i])).abs() < 0.6, "{d}");
        }
        for (pm, d) in GRID_PM.iter().zip([10.0, 60.0, 600.0f64]) {
            assert!((1000.0 * d.log2() / 3600f64.log2() - f64::from(*pm)).abs() < 0.6);
        }
        assert_eq!((nearest(PLOT.0 as u16), nearest(W as u16 - 1)), (0, N - 1));
        assert_eq!(nearest((PLOT.0 + col_of(7)) as u16), 7);
    }

    #[test]
    fn rows_follow_the_bests() {
        let best = curve(|d| (250 + 20_000 / d.max(1)) as u16);
        let y = rows(&best, 500);
        assert_eq!(y.iter().flatten().count(), COLS, "a full curve covers every column");
        assert!(y.windows(2).all(|p| p[0] <= p[1]), "a falling curve only goes down the screen");
        assert_eq!(rows(&[0; N], 500).iter().flatten().count(), 0);
        // a ride of 20 minutes: nothing past it
        let mut short = best;
        short[11..].fill(0);
        let y = rows(&short, 500);
        assert!(y[col_of(10) as usize].is_some());
        assert!(y[col_of(11) as usize + 1..].iter().all(Option::is_none));
    }

    #[test]
    fn text_and_scale() {
        for (s, want) in [(45, "45 S"), (300, "5 MIN"), (3600, "1 H")] {
            let mut l = Line::<12>::new();
            dur_text(&mut l, s);
            assert_eq!(l.as_str(), want);
        }
        assert_eq!(scale(&[0; N], &[0; N]), 100);
        assert_eq!(scale(&curve(|_| 301), &[0; N]), 350);
    }

    /// A four digit watt label ("1400") is drawn whole beside the plot (it was cut to "140").
    #[test]
    fn four_digit_axis_labels_fit() {
        let mut f = Frame::new();
        let best = curve(|_| 1400);
        draw_screen(&mut f, "", &best, &best, 0);
        let mut g = Frame::new();
        draw_in(&mut g, &Style { fg: GREY, scale: 1, ..LABEL }, 0, PLOT.1, PLOT.0 - 2, Align::Right, "1400");
        let w = Style { scale: 1, ..LABEL }.width("1400");
        for y in PLOT.1..PLOT.1 + 7 {
            for x in PLOT.0 - 2 - w..PLOT.0 - 2 {
                assert_eq!(f.at(x, y), g.at(x, y));
            }
        }
        assert!(w <= PLOT.0 - 2);
    }

    /// Both curves and the cursor draw at both ends of the theme metrics without leaving the screen, and the cursor
    /// moves without touching the plot.
    #[test]
    fn draws_both_curves_and_cursor() {
        for t in metric_ends() {
            let mut f = Frame::themed(t);
            let best = curve(|d| (250 + 20_000 / d.max(1)) as u16);
            let ride = curve(|d| (230 + 12_000 / d.max(1)) as u16);
            draw_screen(&mut f, "CP 250W W' 20.0KJ", &best, &ride, 7);
            assert!(f.filled() > 30_000);
            let has = |col: u16| f.px.iter().any(|&p| p == f.theme.ink(col));
            assert!(has(ACCENT) && has(GREEN) && has(WHITE));
            let mut g = Frame::themed(t);
            draw_cursor(&mut g, &best, &ride, 3);
            assert!(g.filled() < 12_000, "the cursor strip and readout only: {}", g.filled());
        }
    }
}
