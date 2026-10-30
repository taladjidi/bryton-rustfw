//! The live segment screens (docs/segments.md, "Screens"): the SEGMENT page, shown while a segment is ridden, and the
//! result card at its finish. Pure drawing against [`Canvas`]. The page draws whole once; then each second [`update`]
//! redraws only the boxes whose text changed (the delta, the target's label, the time, the distance left) and, when
//! one moved, the marks on the bar, never a whole line. Ahead is GREEN, behind ACCENT; red is for alarms only.

use super::{ACCENT, BLACK, Canvas, DARK, GREEN, GREY, LABEL, PAGE_Y, VALUE, W, WHITE};
use crate::text::{self, Align, Line, Style};

/// The delta, as large as a line of "+99:59" allows (6 characters of 36 px).
const BIG: Style = Style { scale: 6, ..VALUE };
const BIG_H: u32 = 60;
const NAME_Y: u32 = PAGE_Y + 4;
const DELTA_Y: u32 = NAME_Y + 26;
const TARGET_Y: u32 = DELTA_Y + BIG_H + 4;
/// The bar: its track, and the marks (taller than the track, centred on it).
const BAR_X: u32 = 12;
const BAR_W: u32 = W - 2 * BAR_X;
const BAR_Y: u32 = TARGET_Y + 36;
const TRACK_H: u32 = 12;
const MARK_W: u32 = 4;
const MARK_H: u32 = 28;
const TIME_Y: u32 = BAR_Y + 44;
const LEFT_Y: u32 = TIME_Y + 46;
/// The numbers' boxes: a caption on the left, the value right-aligned in a box of its own, a unit after it.
const NUM_X: u32 = 100;
const NUM_W: u32 = 104;
const UNIT_X: u32 = NUM_X + NUM_W + 6;
/// A colour for the ghost's mark (the rider's is white).
const GHOST: u16 = GREY;

/// What the page shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct Now<'a> {
    /// The segment's name.
    pub name: &'a str,
    /// Elapsed minus the target's time, s (negative: ahead); None without a target.
    pub delta_s: Option<i32>,
    /// The target's label ("PR", "KOM"...), empty without one.
    pub target: &'a str,
    /// Seconds since the start.
    pub elapsed_s: u32,
    /// Distance left, m.
    pub left_m: u32,
    /// The rider's place along the segment, m.
    pub along_m: u32,
    /// The target's place now, m (None without a target).
    pub ghost_m: Option<u32>,
    /// The segment's length, m.
    pub total_m: u32,
}

/// What is on screen: one fingerprint per box, then the marks' columns (None: not drawn).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The name and the target's label (drawn whole, they change once).
    prints: [u32; 2],
    /// The delta, the elapsed time, the distance left: one cell per character, only a changed character is redrawn.
    delta: Cells<6>,
    time: Cells<7>,
    left: Cells<5>,
    unit: u32,
    rider: Option<u32>,
    ghost: Option<u32>,
}

/// A number drawn in fixed character cells (the font is monospaced): the characters on screen (0: nothing yet) and
/// their ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cells<const N: usize> {
    ch: [u8; N],
    ink: u16,
}

impl<const N: usize> Default for Cells<N> {
    fn default() -> Self {
        Cells { ch: [0; N], ink: 0 }
    }
}

impl<const N: usize> Cells<N> {
    /// Draws `s` in `N` cells of `style` from `x`, right-aligned (`centre`: in the middle of them), only the cells
    /// whose character or ink differ from what is on screen.
    fn update(&mut self, c: &mut impl Canvas, style: &Style, x: u32, y: u32, s: &str, centre: bool) {
        let cw = style.width("8");
        let pad = if centre { N.saturating_sub(s.len()) / 2 } else { N.saturating_sub(s.len()) };
        let mut want = [b' '; N];
        for (w, b) in want.iter_mut().skip(pad).zip(s.bytes()) {
            *w = b;
        }
        let redraw_all = style.fg != self.ink;
        for (k, (w, old)) in want.iter().zip(&mut self.ch).enumerate() {
            if *w == *old && !redraw_all {
                continue;
            }
            *old = *w;
            let mut one = [0u8; 4];
            let t = (*w as char).encode_utf8(&mut one);
            text::draw_in(c, style, x + k as u32 * cw, y, cw, Align::Left, t);
        }
        self.ink = style.fg;
    }
}

fn fnv(h: u32, s: &str) -> u32 {
    s.bytes().chain([0]).fold(h, |h, x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// `m:ss` of `s` seconds, minutes capped at 99.
fn push_ms<const N: usize>(l: &mut Line<N>, s: u32) {
    let s = s.min(99 * 60 + 59);
    l.push_u32(s / 60);
    l.push_str(":");
    l.push_str(crate::ui::DIGITS[(s / 10 % 6) as usize]);
    l.push_str(crate::ui::DIGITS[(s % 10) as usize]);
}

/// The delta as the page and the card show it: "-0:12" (ahead) or "+0:08" (behind), "--" without one.
pub fn delta_text(d: Option<i32>) -> Line<8> {
    let mut l = Line::new();
    match d {
        Some(d) => {
            l.push_str(if d < 0 { "-" } else { "+" });
            push_ms(&mut l, d.unsigned_abs());
        }
        None => l.push_str("--"),
    }
    l
}

/// The time as the page, the card and the banner show it: "1:23", and "1:02:03" from an hour (to 99 hours).
pub fn time_text(s: u32) -> Line<10> {
    let mut l = Line::new();
    if s >= 3600 {
        let s = s.min(99 * 3600 + 3599);
        l.push_u32(s / 3600);
        l.push_str(":");
        l.push_str(crate::ui::DIGITS[(s / 600 % 6) as usize]);
        l.push_str(crate::ui::DIGITS[(s / 60 % 10) as usize]);
        l.push_str(":");
        l.push_str(crate::ui::DIGITS[(s / 10 % 6) as usize]);
        l.push_str(crate::ui::DIGITS[(s % 10) as usize]);
    } else {
        push_ms(&mut l, s);
    }
    l
}

/// The delta's colour: ahead GREEN, behind ACCENT (never red), GREY without a target.
pub fn delta_color(d: Option<i32>) -> u16 {
    match d {
        Some(d) if d <= 0 => GREEN,
        Some(_) => ACCENT,
        None => GREY,
    }
}

/// A distance left: "850" (m) or "1.2" (km), and its unit.
fn left_text(m: u32) -> (Line<8>, &'static str) {
    let mut l = Line::new();
    if m < 1000 {
        l.push_u32(m);
        (l, "M")
    } else {
        l.push_u32(m / 1000);
        l.push_str(".");
        l.push_u32(m % 1000 / 100);
        (l, "KM")
    }
}

/// The bar column (0 to the track's width less a mark) of `m` along a segment of `total` m.
fn column(m: u32, total: u32) -> u32 {
    if total == 0 { 0 } else { (u64::from(m.min(total)) * u64::from(BAR_W - MARK_W) / u64::from(total)) as u32 }
}

fn draw_marks(c: &mut impl Canvas, old: (Option<u32>, Option<u32>), new: (Option<u32>, Option<u32>)) {
    let y = BAR_Y - (MARK_H - TRACK_H) / 2;
    // a moved mark: its old column given back to the background and the track, then both marks (the rider on top)
    for x in [old.0, old.1].into_iter().flatten() {
        c.fill(BAR_X + x, y, MARK_W, MARK_H, BLACK);
        c.fill(BAR_X + x, BAR_Y, MARK_W, TRACK_H, DARK);
    }
    for (x, col) in [(new.1, GHOST), (new.0, WHITE)] {
        if let Some(x) = x {
            c.fill(BAR_X + x, y, MARK_W, MARK_H, col);
        }
    }
}

/// The whole page: the track and the captions, then every box.
pub fn draw(c: &mut impl Canvas, now: &Now) -> Shown {
    c.fill(BAR_X, BAR_Y, BAR_W, TRACK_H, DARK);
    let grey = Style { fg: GREY, ..LABEL };
    text::draw(c, &grey, 8, TIME_Y + 10, Align::Left, "TIME");
    text::draw(c, &grey, 8, LEFT_Y + 10, Align::Left, "LEFT");
    let mut shown = Shown::default();
    update(c, now, &mut shown);
    shown
}

/// Redraws the boxes whose text changed since `shown` (all of them when it is the default: a fingerprint is never 0),
/// and the marks when one moved.
pub fn update(c: &mut impl Canvas, now: &Now, shown: &mut Shown) {
    let (time, (left, unit)) = (time_text(now.elapsed_s), left_text(now.left_m));
    let delta = delta_text(now.delta_s);
    let prints = [fnv(1, now.name), fnv(3, now.target)];
    let w = W - 16;
    let grey = Style { fg: GREY, ..LABEL };
    for (i, (&p, old)) in prints.iter().zip(&mut shown.prints).enumerate() {
        if p == *old {
            continue;
        }
        *old = p;
        if i == 0 {
            text::draw_in(c, &LABEL, 8, NAME_Y, w, Align::Centre, now.name);
        } else {
            let mut l = Line::<16>::new();
            if now.target.is_empty() {
                l.push_str("NO TARGET");
            } else {
                l.push_str("VS ");
                l.push_str(now.target);
            }
            text::draw_in(c, &grey, 8, TARGET_Y, w, Align::Centre, l.as_str());
        }
    }
    let big = Style { fg: delta_color(now.delta_s), ..BIG };
    shown.delta.update(c, &big, (W - 6 * big.width("8")) / 2, DELTA_Y, delta.as_str(), true);
    shown.time.update(c, &VALUE, W - 8 - 7 * VALUE.width("8"), TIME_Y, time.as_str(), false);
    shown.left.update(c, &VALUE, NUM_X + NUM_W - 5 * VALUE.width("8"), LEFT_Y, left.as_str(), false);
    let u = fnv(5, unit);
    if u != shown.unit {
        shown.unit = u;
        text::draw_in(c, &grey, UNIT_X, LEFT_Y + 20, W - UNIT_X, Align::Left, unit);
    }
    let (rider, ghost) = (Some(column(now.along_m, now.total_m)), now.ghost_m.map(|g| column(g, now.total_m)));
    if (rider, ghost) != (shown.rider, shown.ghost) {
        draw_marks(c, (shown.rider, shown.ghost), (rider, ghost));
        (shown.rider, shown.ghost) = (rider, ghost);
    }
}

/// The result card.
#[derive(Clone, Copy, Debug, Default)]
pub struct Card<'a> {
    /// The segment's name.
    pub name: &'a str,
    /// The time, s.
    pub elapsed_s: u32,
    /// Against each kept target: its label and the delta (elapsed minus its time); None for a column with no target.
    pub rows: [Option<(&'a str, i32)>; crate::segment::KEPT],
    /// Faster than the PR.
    pub new_pr: bool,
}

/// The result card, drawn whole (it does not change while it is up).
pub fn draw_card(c: &mut impl Canvas, card: &Card) {
    let grey = Style { fg: GREY, ..LABEL };
    let w = W - 16;
    text::draw_in(c, &grey, 8, NAME_Y, w, Align::Centre, "SEGMENT DONE");
    text::draw_in(c, &LABEL, 8, NAME_Y + 24, w, Align::Centre, card.name);
    text::draw_in(
        c,
        &Style { fg: ACCENT, ..if card.elapsed_s >= 3600 { VALUE } else { BIG } }, // 7 characters from an hour
        8,
        NAME_Y + if card.elapsed_s >= 3600 { 64 } else { 54 },
        w,
        Align::Centre,
        time_text(card.elapsed_s).as_str(),
    );
    let mut y = NAME_Y + 118;
    for (label, d) in card.rows.iter().flatten() {
        let mut l = Line::<16>::new();
        l.push_str("VS ");
        l.push_str(label);
        text::draw(c, &LABEL, 12, y, Align::Left, l.as_str());
        let st = Style { fg: delta_color(Some(*d)), ..LABEL };
        text::draw(c, &st, W - 12, y, Align::Right, delta_text(Some(*d)).as_str());
        y += 24;
    }
    if card.new_pr {
        text::draw_in(c, &Style { fg: GREEN, ..VALUE }, 8, y + 4, w, Align::Centre, "NEW PR");
    }
    text::draw_in(c, &grey, 8, crate::ui::H - 24, w, Align::Centre, "ANY KEY");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tests::Frame;

    fn now(delta: i32, t: u32) -> Now<'static> {
        Now {
            name: "A LONG SEGMENT NAME THAT IS CUT",
            delta_s: Some(delta),
            target: "RECENT",
            elapsed_s: t,
            left_m: 12_345,
            along_m: 500,
            ghost_m: Some(450),
            total_m: 12_845,
        }
    }

    /// The text of the delta and its colours: ahead green, behind the accent, none grey.
    #[test]
    fn delta_text_and_colour() {
        assert_eq!(delta_text(Some(-12)).as_str(), "-0:12");
        assert_eq!(delta_text(Some(8)).as_str(), "+0:08");
        assert_eq!(delta_text(Some(-3599)).as_str(), "-59:59");
        assert_eq!(delta_text(None).as_str(), "--");
        assert_eq!([62 * 60 + 3, 105 * 60, 3599, 3600].map(|s| time_text(s).as_str().len()), [7, 7, 5, 7]);
        assert_eq!(time_text(3723).as_str(), "1:02:03");
        assert_eq!(time_text(6300).as_str(), "1:45:00");
        assert_eq!([-1, 0, 1].map(|d| delta_color(Some(d))), [GREEN, GREEN, ACCENT]);
    }

    /// Updating cell by cell gives the picture of drawing whole, at both fonts, and a second costs a few cells.
    #[test]
    fn update_equals_whole_draw_and_is_small() {
        for smooth in [false, true] {
            let mut f = Frame::new();
            f.smooth = smooth;
            let mut shown = draw(&mut f, &now(-69, 599));
            let before = f.filled();
            update(&mut f, &now(-68, 600), &mut shown);
            let cost = f.filled() - before;
            let mut whole = Frame::new();
            whole.smooth = smooth;
            draw(&mut whole, &now(-68, 600));
            assert!(f.px == whole.px, "smooth {smooth}");
            assert!(cost < 12_000, "{cost} px for the carry of a minute");
            let before = f.filled();
            update(&mut f, &now(-67, 601), &mut shown);
            assert!(f.filled() - before < 4000, "{} px for a second", f.filled() - before);
            let mut h = Frame::new();
            h.smooth = smooth;
            let mut shown = draw(&mut h, &now(5, 3723));
            update(&mut h, &now(5, 6300), &mut shown);
            let mut whole = Frame::new();
            whole.smooth = smooth;
            draw(&mut whole, &now(5, 6300));
            assert!(h.px == whole.px, "past an hour, smooth {smooth}");
            let mut card = Frame::new();
            card.smooth = smooth;
            let rows = [Some(("PR", -31)), Some(("KOM", 5)), Some(("QOM", -33)), Some(("RECENT", -34))];
            draw_card(
                &mut card,
                &Card { name: "A LONG SEGMENT NAME THAT IS CUT", elapsed_s: 5999, rows, new_pr: true },
            );
            draw_card(&mut Frame::new(), &Card { name: "X", elapsed_s: 6300, rows, new_pr: false });
        }
    }
}
