//! Ring and bar gauges for a ride field ([`Field::dial`]): the value as any field draws it, and a gauge that fills
//! with it, in the colour of its zone. The ring surrounds the value; the bar runs under it. Both are drawn once with
//! the box and then only the part that moved is sent: a bar's columns between the old level and the new, a ring's
//! arc between them in blits of its bounding boxes, less the value's rows.
//! The ring needs no trigonometry: a pixel's step is a "pseudo-angle" (the ratio of its two offsets, per octant),
//! exact at the 8 octant borders and within 4 degrees between, and the same function draws and updates.

use super::*;

/// Steps (segments) of a ring, 6 per octant.
pub const RING_STEPS: u32 = 48;
/// Steps in a piece of an arc repainted by one blit: the smaller, the closer its box is to the arc.
const PIECE: u32 = 2;
/// A bar's height, and the gap above and below it.
const BAR_H: u32 = 10;
const BAR_GAP: u32 = 4;
/// What a bar takes from the box under the value.
pub const BAR_ROOM: u32 = BAR_H + 2 * BAR_GAP;
/// A bar's inset from the box's sides.
const BAR_SIDE: u32 = 6;

/// A ring's centre and radii in a box.
struct Ring {
    cx: i32,
    cy: i32,
    r: i32,
    t: i32,
}

fn ring_of((x, y, w, h): (u32, u32, u32, u32)) -> Ring {
    let (top, room) = (y + FIELD_LABEL_H, h.saturating_sub(FIELD_LABEL_H + 2));
    let r = w.saturating_sub(2).min(room) / 2;
    Ring { cx: (x + w / 2) as i32, cy: (top + room / 2) as i32, r: r as i32, t: (r / 7).clamp(4, 10) as i32 }
}

/// The ring's hole is a box for the value: 7/8 of the hole's width (it holds 4 or 5 characters at the largest scale
/// that fits) and 9/20 of its height, whose corners stay inside the hole.
fn ring_value_box(rg: &Ring) -> (u32, u32, u32, u32) {
    let d = 2 * (rg.r - rg.t).max(0) as u32;
    let (w, ih) = (d * 7 / 8, d * 9 / 20);
    ((rg.cx as u32).saturating_sub(w / 2), (rg.cy as u32).saturating_sub(ih / 2 + FIELD_LABEL_H), w, ih + FIELD_LABEL_H)
}

/// The box the value is drawn in: the ring's hole, or the box above the bar.
pub fn value_box(bx: (u32, u32, u32, u32), ring: bool) -> (u32, u32, u32, u32) {
    if ring { ring_value_box(&ring_of(bx)) } else { (bx.0, bx.1, bx.2, bx.3.saturating_sub(BAR_ROOM)) }
}

/// The steps of a gauge in box `bx`: a ring's, or a bar's columns.
pub fn dial_steps(bx: (u32, u32, u32, u32), ring: bool) -> u32 {
    if ring { RING_STEPS } else { bx.2.saturating_sub(2 * BAR_SIDE) }
}

/// Step (0 to 47, clockwise from 12 o'clock) of the point `x` right and `y` up of the centre: per octant, the ratio
/// of the smaller offset to the larger in sixths, rounded so that a point on a step's border is in that step.
fn step_of(x: i32, y: i32) -> u32 {
    let (ax, ay) = (x.unsigned_abs(), y.unsigned_abs());
    let (fl, ce) = (|a: u32, b: u32| a * 6 / b, |a: u32, b: u32| (a * 6).div_ceil(b));
    if y > 0 && x >= 0 {
        if ax <= ay { fl(ax, ay) } else { 12 - ce(ay, ax) }
    } else if y <= 0 && x > 0 {
        if ay <= ax { 12 + fl(ay, ax) } else { 24 - ce(ax, ay) }
    } else if y < 0 {
        if ax <= ay { 24 + fl(ax, ay) } else { 36 - ce(ay, ax) }
    } else if x < 0 {
        if ay <= ax { 36 + fl(ay, ax) } else { 48 - ce(ax, ay) }
    } else {
        0
    }
}

/// The point at radius `r` on the border of step `s` (0 to 48), relative to the centre, x right and y up.
fn edge(s: u32, r: i32) -> (i32, i32) {
    let (t, k) = ((s % 6) as i32, (s % RING_STEPS) / 6);
    let (vx, vy) =
        [(t, 6), (6, 6 - t), (6, -t), (6 - t, -6), (-t, -6), (-6, t - 6), (-6, t), (t - 6, 6)][k as usize % 8];
    let (vx, vy) = (vx * 64, vy * 64); // the root of a small number is coarse
    let n = ((vx * vx + vy * vy) as u32).isqrt().max(1) as i32;
    (vx * r / n, vy * r / n)
}

/// Pixel (`x`, `y`) of the ring with `level` steps lit: the colours `lit`, `dark` and `black` as the canvas has them.
fn ring_px(rg: &Ring, x: u32, y: u32, level: u32, (lit, dark, black): (u16, u16, u16)) -> u16 {
    let (dx, dy) = (x as i32 - rg.cx, rg.cy - y as i32);
    let d2 = dx * dx + dy * dy;
    if d2 > rg.r * rg.r || d2 < (rg.r - rg.t) * (rg.r - rg.t) {
        black
    } else if step_of(dx, dy) < level {
        lit
    } else {
        dark
    }
}

/// The bounding box (left, top, width, height) of steps `lo` to `hi` of the ring, both in one octant: an arc's extremes
/// are at its ends.
fn arc_box(rg: &Ring, (lo, hi): (u32, u32)) -> (u32, u32, u32, u32) {
    let (mut x0, mut y0, mut x1, mut y1) = (rg.cx + rg.r, rg.cy + rg.r, rg.cx - rg.r, rg.cy - rg.r);
    for (x, y) in [lo, hi].into_iter().flat_map(|s| [edge(s, rg.r), edge(s, rg.r - rg.t)]) {
        (x0, x1) = (x0.min(rg.cx + x - 1), x1.max(rg.cx + x + 1));
        (y0, y1) = (y0.min(rg.cy - y - 1), y1.max(rg.cy - y + 1));
    }
    let (x0, y0) = (x0.max(rg.cx - rg.r) as u32, y0.max(rg.cy - rg.r) as u32);
    (x0, y0, x1.min(rg.cx + rg.r) as u32 + 1 - x0, y1.min(rg.cy + rg.r) as u32 + 1 - y0)
}

/// The ring's box `(x, y, w, h)` repainted for `level` steps lit in `color`, except the value's rows, which it leaves
/// to the value: the box less that band, as up to four blits.
fn ring_paint(c: &mut impl Canvas, rg: &Ring, color: u16, level: u32, (x, y, w, h): (u32, u32, u32, u32)) {
    let ink = (c.ink(color), c.ink(DARK), c.ink(BLACK));
    let (vx, vy, vw, vh) = ring_value_box(rg);
    let (bx0, bx1, by0, by1) = (
        vx.clamp(x, x + w),
        (vx + vw).clamp(x, x + w),
        (vy + FIELD_LABEL_H).clamp(y, y + h),
        (vy + vh).clamp(y, y + h),
    );
    let mut paint = |x: u32, y: u32, w: u32, h: u32| {
        if w > 0 && h > 0 {
            c.blit(x, y, w, h, &mut |i, j| ring_px(rg, x + i, y + j, level, ink));
        }
    };
    if bx0 >= bx1 || by0 >= by1 {
        return paint(x, y, w, h);
    }
    paint(x, y, w, by0 - y);
    paint(x, by1, w, y + h - by1);
    paint(x, by0, bx0 - x, by1 - by0);
    paint(bx1, by0, x + w - bx1, by1 - by0);
}

/// A gauge field whole in box `bx` (`ring` or bar): `value` in `fg` and the gauge at `level` steps in `color`. The
/// label's frame is the caller's.
pub fn draw_dial(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    ring: bool,
    value: &str,
    fg: u16,
    color: u16,
    level: u32,
) {
    if ring {
        let rg = ring_of(bx);
        let side = 2 * rg.r as u32 + 1;
        ring_paint(c, &rg, color, level, (rg.cx as u32 - rg.r as u32, rg.cy as u32 - rg.r as u32, side, side));
    } else {
        draw_bar(c, bx, color, (0, dial_steps(bx, false)), level);
    }
    draw_field_value(c, value_box(bx, ring), value, fg);
}

/// Columns `from` to `to` of the bar, those under `level` in `color`, the rest dark.
fn draw_bar(c: &mut impl Canvas, (x, y, _, h): (u32, u32, u32, u32), color: u16, (from, to): (u32, u32), level: u32) {
    let top = (y + h).saturating_sub(BAR_GAP + BAR_H);
    let lit = level.clamp(from, to);
    c.fill(x + BAR_SIDE + from, top, lit - from, BAR_H, color);
    c.fill(x + BAR_SIDE + lit, top, to - lit, BAR_H, DARK);
}

/// A gauge field that showed `old` (text, its colour, the gauge's colour, level) now shows `new`: the bar's columns
/// or the ring's arc between the two levels (from the start when the gauge's colour changed, since the lit part is
/// repainted), and the value's changed characters.
pub fn update_dial(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    ring: bool,
    old: (&str, u16, u16, u32),
    new: (&str, u16, u16, u32),
) {
    let vb = value_box(bx, ring);
    let hi = old.3.max(new.3);
    let lo = if old.2 != new.2 { 0 } else { old.3.min(new.3) };
    if ring {
        let rg = ring_of(bx);
        let mut s = lo;
        while s < hi {
            let e = hi.min((s / PIECE + 1) * PIECE);
            ring_paint(c, &rg, new.2, new.3, arc_box(&rg, (s, e)));
            s = e;
        }
    } else if lo < hi {
        draw_bar(c, bx, new.2, (lo, hi), new.3);
    }
    if (old.0, old.1) != (new.0, new.1) {
        draw_field_value_changes(c, vb, if old.1 == new.1 { old.0 } else { "" }, new.0, new.1);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Frame, metric_ends};
    extern crate std;
    use super::*;

    const DIALS: [Field; 3] = [Field::PowerRing, Field::HrRing, Field::CadBar];

    /// The pseudo-angle and the border points agree: the points are on the circle, a border is in its step (or the
    /// one before, by the integer point's rounding), and the middle of a step is in it.
    #[test]
    fn steps_follow_their_borders() {
        for s in 0..RING_STEPS {
            let (x, y) = edge(s, 400);
            assert!(x * x + y * y <= 401 * 401 && x * x + y * y > 398 * 398, "step {s} border on the circle");
            let got = step_of(x, y);
            assert!(got == s || got + 1 == s, "step {s} border: {got}");
            let (nx, ny) = edge(s + 1, 400);
            assert!(step_of((x + nx) / 2, (y + ny) / 2) == s, "the middle of step {s}");
        }
        assert_eq!([step_of(0, 5), step_of(5, 0), step_of(0, -5), step_of(-5, 0)], [0, 12, 24, 36]);
    }

    /// Every layout at both metric ends: the ring and the bar draw (Frame panics off screen) in the boxes that hold
    /// them, the value at 2x or more, and nothing of the value reaches the ring's band.
    #[test]
    fn gauges_fit_every_layout() {
        for theme in metric_ends() {
            for n in 1..USER_LAYOUTS {
                for i in 0..8 {
                    let Some(bx) = field_box(n, RIDE_H, i) else { break };
                    for f in DIALS.into_iter().filter(|&f| crate::page::fits(f, bx.into())) {
                        let ring = f.dial().is_some_and(|d| d.1);
                        let vb = value_box(bx, ring);
                        assert!(value_scale("8888", vb.2, vb.3) >= 2, "{n} fields {f:?} in {bx:?}: value readable");
                        let mut fr = Frame::themed(theme);
                        draw_dial(&mut fr, bx, ring, "8888", WHITE, RED, 20);
                        if ring {
                            let rg = ring_of(bx);
                            for y in 0..H {
                                for x in 0..W {
                                    let (dx, dy) = (x as i32 - rg.cx, y as i32 - rg.cy);
                                    let d2 = dx * dx + dy * dy;
                                    let band = d2 <= rg.r * rg.r && d2 >= (rg.r - rg.t) * (rg.r - rg.t);
                                    assert!(!band || fr.at(x, y) != fr.ink(WHITE), "{n} fields {f:?}: value on ring");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// An update paints what drawing whole at the new level paints, for pairs of levels (a ring crossing every
    /// octant, a bar), and for a new colour.
    #[test]
    fn updates_equal_whole_draws() {
        for (bx, ring, steps) in [
            ((0, RIDE_Y, 240, RIDE_H), true, RING_STEPS),
            ((0, RIDE_Y, 120, 130), true, RING_STEPS),
            ((0, RIDE_Y, 120, 60), false, 108),
            ((0, RIDE_Y, 240, 100), false, 228),
        ] {
            for (a, b) in (0..=steps).step_by(7).flat_map(|a| (0..=steps).step_by(5).map(move |b| (a, b))) {
                let mut fr = Frame::new();
                draw_dial(&mut fr, bx, ring, "150", WHITE, GREEN, a);
                update_dial(&mut fr, bx, ring, ("150", WHITE, GREEN, a), ("151", WHITE, GREEN, b));
                let mut whole = Frame::new();
                draw_dial(&mut whole, bx, ring, "151", WHITE, GREEN, b);
                let bad: std::vec::Vec<_> = (0..fr.px.len())
                    .filter(|&i| fr.px[i] != whole.px[i])
                    .take(5)
                    .map(|i| (i as u32 % W, i as u32 / W))
                    .collect();
                assert!(bad.is_empty(), "{bx:?} ring {ring}: {a} to {b}: {bad:?}");
            }
            let mut fr = Frame::new();
            draw_dial(&mut fr, bx, ring, "150", WHITE, GREEN, 5);
            update_dial(&mut fr, bx, ring, ("150", WHITE, GREEN, 5), ("150", WHITE, RED, 5));
            let mut whole = Frame::new();
            draw_dial(&mut whole, bx, ring, "150", WHITE, RED, 5);
            assert!(fr.px == whole.px, "{bx:?}: a new colour");
        }
    }

    /// What a second costs: a power ring in a half box and in a 1-field box, the level moving 2 steps and a digit
    /// changing, against the box whole.
    #[test]
    fn a_second_of_a_ring_is_a_fraction_of_the_box() {
        for (bx, max) in [((0, RIDE_Y, 120, 130), 1500), ((0, RIDE_Y, 240, RIDE_H), 5000)] {
            let mut fr = Frame::new();
            draw_dial(&mut fr, bx, true, "150", WHITE, GREEN, 20);
            let whole = fr.filled();
            let before = fr.filled();
            update_dial(&mut fr, bx, true, ("150", WHITE, GREEN, 20), ("152", WHITE, GREEN, 22));
            let spent = fr.filled() - before;
            extern crate std;
            std::println!("ring {bx:?}: {spent} px for 2 steps and a digit, {whole} px whole");
            assert!(spent < max, "{spent} px");
        }
    }
}
