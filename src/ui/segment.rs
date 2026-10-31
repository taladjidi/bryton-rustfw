//! The live segment's head box and the result card (docs/segments.md, "Screens"). The head is a field of the CLIMB
//! page ([`crate::Field::SegHead`]): the name, the delta in big digits and the target's label; the page's other boxes
//! (profile, gauge, time, distance left) are the field machinery's. Pure drawing against [`Canvas`]: [`update_head`]
//! redraws only what changed, a character of the delta at a time. Ahead is GREEN, behind ACCENT; red is for alarms only.

use super::{ACCENT, Canvas, GREEN, GREY, LABEL, VALUE};
use crate::text::{self, Align, Line, Style};
use crate::ui::W;

/// The delta, as large as a line of "+99:59" allows (6 characters of 36 px).
const BIG: Style = Style { scale: 6, ..VALUE };
/// Rows of the head box: the top line, the big line (60 px) and the small line under it.
const TOP_Y: u32 = 3;
const BIG_Y: u32 = TOP_Y + 18;
const SUB_Y: u32 = BIG_Y + 61;
/// The card's top.
const NAME_Y: u32 = crate::ui::PAGE_Y + 4;
/// Characters in the big line.
const CELLS: usize = 6;

/// What the head box shows.
#[derive(Clone, Copy, Debug, Default)]
pub struct Head<'a> {
    /// The line on top: the segment's name, or the climb's title.
    pub top: &'a str,
    /// The big line (at most 6 characters): the delta, or the time to the top.
    pub big: &'a str,
    /// The big line's colour.
    pub ink: u16,
    /// The small grey line under it: "VS PR", "TO THE TOP".
    pub sub: &'a str,
    /// Keeps the colour the box shows when the delta is within 2 s: the colour flips between ahead and behind at the
    /// line, and each flip would redraw every character of the delta.
    pub keep: bool,
}

/// What is on screen of a head box: a fingerprint of each line, the big line's characters and its ink. The default is
/// nothing drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeadShown {
    prints: [u32; 2],
    big: [u8; CELLS],
    ink: u16,
}

impl Head<'_> {
    /// What this head shows, as [`HeadShown`] keeps it.
    pub fn shown(&self) -> HeadShown {
        let mut big = [b' '; CELLS];
        // centred in its cells
        let pad = CELLS.saturating_sub(self.big.len()) / 2;
        for (w, b) in big.iter_mut().skip(pad).zip(self.big.bytes()) {
            *w = b;
        }
        HeadShown { prints: [crate::fnv_str(1, self.top), crate::fnv_str(3, self.sub)], big, ink: self.ink }
    }
}

/// `m:ss` of `s` seconds, minutes capped at 99.
fn push_ms<const N: usize>(l: &mut Line<N>, s: u32) {
    let s = s.min(99 * 60 + 59);
    l.push_u32(s / 60);
    l.push_str(":");
    l.push_2d(s % 60);
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
    crate::push_clock(&mut l, s.min(99 * 3600 + 3599));
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

/// A distance left, for the page and the field: "850" (m) or "1.2" (km), and its unit.
pub fn left_text(m: u32) -> (Line<8>, &'static str) {
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

/// The head box `bx` whole.
pub fn draw_head(c: &mut impl Canvas, bx: (u32, u32, u32, u32), h: &Head) {
    update_head(c, bx, &HeadShown::default(), h);
}

/// The head box `bx` as `old` was drawn: the lines whose text changed, and the characters of the big line that did
/// (all of them in a new colour), never a whole line for one character. Returns what is on screen now.
pub fn update_head(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), old: &HeadShown, h: &Head) -> HeadShown {
    // a kept colour is the one on screen (ink 0 is nothing drawn)
    let ink = if h.keep && old.ink != 0 { old.ink } else { h.ink };
    let new = HeadShown { ink, ..h.shown() };
    let inner = w.saturating_sub(16);
    if new.prints[0] != old.prints[0] {
        text::draw_in(c, &LABEL, x + 8, y + TOP_Y, inner, Align::Centre, h.top);
    }
    if new.prints[1] != old.prints[1] {
        text::draw_in(c, &Style { fg: GREY, ..LABEL }, x + 8, y + SUB_Y, inner, Align::Centre, h.sub);
    }
    let big = Style { fg: ink, ..BIG };
    let cw = big.width("8");
    let left = x + w.saturating_sub(CELLS as u32 * cw) / 2;
    for (k, (a, b)) in new.big.iter().zip(&old.big).enumerate() {
        // a new colour redraws the characters with ink, not the blank cells
        if a != b || (new.ink != old.ink && (*a != b' ' || *b != b' ')) {
            let mut one = [0u8; 4];
            let t = (*a as char).encode_utf8(&mut one);
            text::draw_in(c, &big, left + k as u32 * cw, y + BIG_Y, cw, Align::Left, t);
        }
    }
    new
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
    use crate::text::Face;
    use crate::ui::tests::Frame;

    fn head(delta: i32) -> (Line<8>, u16) {
        (delta_text(Some(delta)), delta_color(Some(delta)))
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
        assert_eq!(left_text(850).0.as_str(), "850");
        assert_eq!(left_text(1250).0.as_str(), "1.2");
    }

    /// A delta that goes -1, +1, -1 keeps its colour: no flip, and the step to +1 draws less than a flip does.
    #[test]
    fn a_delta_within_two_seconds_keeps_its_colour() {
        let bx = (0, crate::ui::PAGE_Y, W, 98);
        fn k<'a>(d: &'a (Line<8>, u16), keep: bool) -> Head<'a> {
            Head { top: "NAME", big: d.0.as_str(), ink: d.1, sub: "VS PR", keep }
        }
        // the same steps drawn twice: kept, and with the colour flipping
        let (mut f, mut g) = (Frame::new(), Frame::new());
        let mut shown = update_head(&mut f, bx, &HeadShown::default(), &k(&head(-1), true));
        let mut flip = update_head(&mut g, bx, &HeadShown::default(), &k(&head(-1), false));
        for d in [1, -1] {
            let (kept0, flip0) = (f.filled(), g.filled());
            shown = update_head(&mut f, bx, &shown, &k(&head(d), true));
            flip = update_head(&mut g, bx, &flip, &k(&head(d), false));
            assert_eq!(shown.ink, GREEN, "kept at {d}");
            // the sign's cell only, against every character of the delta
            assert!(f.filled() - kept0 < g.filled() - flip0, "a step to {d} redraws only its sign");
        }
    }

    /// Updating character by character gives the picture of drawing whole, at both fonts, and a second costs a few
    /// cells.
    #[test]
    fn update_equals_whole_draw_and_is_small() {
        let bx = (0, crate::ui::PAGE_Y, W, 98);
        let name = "A LONG SEGMENT NAME THAT IS CUT";
        for face in [Face::Pixel, Face::Barlow] {
            let mut f = Frame::new();
            f.face = face;
            let (a, b) = (head(-69), head(-68));
            fn h<'a>(name: &'a str, d: &'a (Line<8>, u16)) -> Head<'a> {
                Head { top: name, big: d.0.as_str(), ink: d.1, sub: "VS RECENT", keep: false }
            }
            draw_head(&mut f, bx, &h(name, &a));
            let before = f.filled();
            update_head(&mut f, bx, &h(name, &a).shown(), &h(name, &b));
            let cost = f.filled() - before;
            let mut whole = Frame::new();
            whole.face = face;
            draw_head(&mut whole, bx, &h(name, &b));
            assert!(f.px == whole.px, "face {face:?}");
            assert!(cost < 4000, "{cost} px for a second");
            update_head(&mut f, bx, &h(name, &b).shown(), &h(name, &b));
            assert_eq!(f.filled() - before, cost, "nothing changed, nothing drawn");
            // a new colour: every character again
            let c = head(5);
            update_head(&mut f, bx, &h(name, &b).shown(), &h(name, &c));
            let mut whole = Frame::new();
            whole.face = face;
            draw_head(&mut whole, bx, &h(name, &c));
            assert!(f.px == whole.px, "colour, face {face:?}");
            let mut card = Frame::new();
            card.face = face;
            let rows = [Some(("PR", -31)), Some(("KOM", 5)), Some(("QOM", -33)), Some(("RECENT", -34))];
            draw_card(&mut card, &Card { name, elapsed_s: 5999, rows, new_pr: true });
            draw_card(&mut Frame::new(), &Card { name: "X", elapsed_s: 6300, rows, new_pr: false });
        }
    }
}
